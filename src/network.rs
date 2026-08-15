//! The sole model-network path for a session.
//!
//! ```text
//! agent (--network none)
//!   127.0.0.1:18000
//!        │ shared network namespace
//! inner socat container (--network container:<agent>)
//!        │ connect-only bind mount of /sock/vllm.sock
//! outer socat container (--network host)
//!        │ TCP4:127.0.0.1:8000
//! pinned vLLM backend
//! ```
//!
//! The agent has no interface, route, DNS, bridge gateway, published port,
//! or host namespace.  Its only reachable TCP address is its own loopback,
//! on which the inner proxy listens.  The outer proxy is the only component
//! that can see the host loopback; the two proxies meet through one
//! per-session Unix socket.  No HTTP semantics are changed or recovered in
//! this path: socat forwards bytes in both directions.

use std::path::Path;
use std::time::Duration;

use crate::config::Config;
use crate::docker_ops;
use crate::error::{ServiceError, ServiceResult};

pub const PROXY_LISTEN_PORT: u16 = 18000;
const SOCKET_MOUNT: &str = "/sock";
const SOCKET_NAME: &str = "vllm.sock";

#[derive(Debug)]
pub struct IsolatedNetwork {
    pub outer_proxy_container_name: String,
    pub inner_proxy_container_name: Option<String>,
    pub session_label: String,
    socket_host_dir: String,
}

impl IsolatedNetwork {
    pub async fn create(
        cfg: &Config,
        session_id: &str,
        socket_host_dir: &Path,
    ) -> ServiceResult<Self> {
        let socket_host_dir = mount_path(socket_host_dir)?;
        let session_label = format!("agent_service.session={session_id}");
        let profile_label = format!("agent_service.profile={}", cfg.lock.profile);
        let outer = format!("agent-model-outer-{session_id}");
        let listen = format!(
            "UNIX-LISTEN:{SOCKET_MOUNT}/{SOCKET_NAME},fork,reuseaddr,unlink-early,user=1000,group=1000,mode=0660"
        );
        let target = "TCP4:127.0.0.1:8000".to_string();
        let args = vec![
            "--name".into(),
            outer.clone(),
            "--label".into(),
            session_label.clone(),
            "--label".into(),
            profile_label,
            "--network".into(),
            "host".into(),
            "--user".into(),
            "1000:1000".into(),
            "--entrypoint".into(),
            "/usr/bin/socat".into(),
            "--read-only".into(),
            "--tmpfs".into(),
            "/tmp:rw,nosuid,nodev,noexec,size=4m,mode=1777".into(),
            "--cap-drop".into(),
            "ALL".into(),
            "--security-opt".into(),
            "no-new-privileges:true".into(),
            "--memory".into(),
            "64m".into(),
            "--memory-swap".into(),
            "64m".into(),
            "--pids-limit".into(),
            "32".into(),
            "--mount".into(),
            format!(
                "type=bind,src={},dst={SOCKET_MOUNT}",
                escape_mount(&socket_host_dir)?
            ),
            cfg.agent_image.clone(),
            listen,
            target,
        ];

        docker_ops::run_detached(args, "start_outer_model_proxy").await?;
        let network = Self {
            outer_proxy_container_name: outer,
            inner_proxy_container_name: None,
            session_label,
            socket_host_dir,
        };
        let socket = Path::new(&network.socket_host_dir).join(SOCKET_NAME);
        if let Err(error) = wait_for_unix_socket(&socket, Duration::from_secs(10)).await {
            let logs = docker_ops::container_logs_tail(&network.outer_proxy_container_name, 200)
                .await
                .unwrap_or_else(|log_error| {
                    format!("<failed to read outer proxy logs: {log_error}>")
                });
            let diagnostics = network.teardown().await;
            return Err(ServiceError::DockerCommand(format!(
                "outer model proxy failed readiness: {error}; logs: {logs}; cleanup: {diagnostics:?}"
            )));
        }
        Ok(network)
    }

    pub async fn attach_inner_proxy(
        &mut self,
        cfg: &Config,
        session_id: &str,
        agent_container: &str,
    ) -> ServiceResult<()> {
        if self.inner_proxy_container_name.is_some() {
            return Err(ServiceError::Internal(
                "attach_inner_proxy called twice for one session".into(),
            ));
        }
        let inner = format!("agent-model-inner-{session_id}");
        let listen = format!("TCP4-LISTEN:{PROXY_LISTEN_PORT},bind=127.0.0.1,fork,reuseaddr");
        let args = vec![
            "--name".into(),
            inner.clone(),
            "--label".into(),
            self.session_label.clone(),
            "--label".into(),
            format!("agent_service.profile={}", cfg.lock.profile),
            "--network".into(),
            format!("container:{agent_container}"),
            "--user".into(),
            "1000:1000".into(),
            "--entrypoint".into(),
            "/usr/bin/socat".into(),
            "--read-only".into(),
            "--tmpfs".into(),
            "/tmp:rw,nosuid,nodev,noexec,size=4m,mode=1777".into(),
            "--cap-drop".into(),
            "ALL".into(),
            "--security-opt".into(),
            "no-new-privileges:true".into(),
            "--memory".into(),
            "64m".into(),
            "--memory-swap".into(),
            "64m".into(),
            "--pids-limit".into(),
            "32".into(),
            "--mount".into(),
            format!(
                "type=bind,src={},dst={SOCKET_MOUNT},readonly",
                escape_mount(&self.socket_host_dir)?
            ),
            cfg.agent_image.clone(),
            listen,
            format!("UNIX-CONNECT:{SOCKET_MOUNT}/{SOCKET_NAME}"),
        ];
        docker_ops::run_detached(args, "start_inner_model_proxy").await?;
        self.inner_proxy_container_name = Some(inner.clone());

        if let Err(error) =
            docker_ops::verify_shared_network_namespace(&inner, agent_container).await
        {
            let logs = docker_ops::container_logs_tail(&inner, 200)
                .await
                .unwrap_or_else(|log_error| {
                    format!("<failed to read inner proxy logs: {log_error}>")
                });
            return Err(ServiceError::Internal(format!(
                "inner model proxy namespace verification failed: {error}; logs: {logs}"
            )));
        }
        Ok(())
    }

    pub async fn teardown(self) -> Vec<String> {
        let mut diagnostics = Vec::new();
        if let Some(inner) = self.inner_proxy_container_name {
            if let Err(error) = docker_ops::container_force_remove(&inner).await {
                diagnostics.push(format!("remove inner proxy {inner}: {error}"));
            }
        }
        if let Err(error) =
            docker_ops::container_force_remove(&self.outer_proxy_container_name).await
        {
            diagnostics.push(format!(
                "remove outer proxy {}: {error}",
                self.outer_proxy_container_name
            ));
        }
        diagnostics
    }
}

fn mount_path(path: &Path) -> ServiceResult<String> {
    let value = path.to_str().ok_or_else(|| {
        ServiceError::Internal(format!("mount path is not UTF-8: {}", path.display()))
    })?;
    if !path.is_absolute() {
        return Err(ServiceError::Internal(format!(
            "mount path is not absolute: {value:?}"
        )));
    }
    if value.contains(['\n', '\r', '\0']) {
        return Err(ServiceError::Internal(format!(
            "mount path contains a forbidden control character: {value:?}"
        )));
    }
    Ok(value.to_string())
}

fn escape_mount(value: &str) -> ServiceResult<String> {
    if value.contains(',') {
        return Err(ServiceError::Internal(format!(
            "bind source contains a comma and cannot be represented safely in Docker --mount: {value:?}"
        )));
    }
    Ok(value.to_string())
}

async fn wait_for_unix_socket(path: &Path, hard_timeout: Duration) -> ServiceResult<()> {
    let deadline = tokio::time::Instant::now() + hard_timeout;
    loop {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if std::os::unix::fs::FileTypeExt::is_socket(&metadata.file_type()) => {
                return Ok(())
            }
            Ok(metadata) => {
                return Err(ServiceError::Internal(format!(
                    "expected Unix socket at {}, found {:?}",
                    path.display(),
                    metadata.file_type()
                )))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(ServiceError::Internal(format!(
                    "cannot stat expected Unix socket {}: {error}",
                    path.display()
                )))
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(ServiceError::Timeout(format!(
                "Unix socket {} did not appear within {hard_timeout:?}",
                path.display()
            )));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Remove only objects bearing the service label. Any failure is returned;
/// startup never announces readiness after a partial orphan sweep.
pub async fn sweep_orphans() -> ServiceResult<()> {
    let ids = docker_ops::run_docker(
        ["ps", "-aq", "--filter", "label=agent_service.session"],
        "list_agent_service_orphans",
    )
    .await?;
    let mut failures = Vec::new();
    for id in ids.lines().map(str::trim).filter(|id| !id.is_empty()) {
        if let Err(error) = docker_ops::container_force_remove(id).await {
            failures.push(format!("container {id}: {error}"));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(ServiceError::Internal(format!(
            "orphan sweep was incomplete: {}",
            failures.join("; ")
        )))
    }
}
