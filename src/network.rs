//! Per-session network isolation: a chain of two `socat`-running containers
//! plus a Docker `--internal` network between the agent and the inner
//! proxy. Both proxies are spawned and supervised by this Rust process,
//! using the same `qwen-agent-template` image with `--entrypoint socat` —
//! one binary, one image, one byte-forwarding pattern.
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────────────┐
//! │ HOST                                                             │
//! │                                                                  │
//! │   vLLM listening on 127.0.0.1:<vllm_port>                        │
//! │       ▲                                                          │
//! │       │ TCP                                                      │
//! │       │                                                          │
//! │   ┌───┴──────────────────────────────┐                           │
//! │   │ OUTER PROXY CONTAINER            │  --network=host           │
//! │   │  --entrypoint socat              │  socat                    │
//! │   │  qwen-agent-template:0.1.0       │   UNIX-LISTEN:/sock/...   │
//! │   │  --read-only --cap-drop ALL      │   TCP:127.0.0.1:<vllm>    │
//! │   │  --user 1000 --memory 64m        │                           │
//! │   └───┬──────────────────────────────┘                           │
//! │       │ Unix socket  (bind-mounted into both proxies as /sock)   │
//! │       │                                                          │
//! │   ┌───┴──────────────────────────────┐                           │
//! │   │ INNER PROXY CONTAINER            │  --network=agent-net-X    │
//! │   │  --entrypoint socat              │  socat                    │
//! │   │  qwen-agent-template:0.1.0       │   TCP-LISTEN:8001         │
//! │   │  --read-only --cap-drop ALL      │   UNIX-CONNECT:/sock/...  │
//! │   │  --user 1000 --memory 64m        │                           │
//! │   │  --dns 127.0.0.1                 │                           │
//! │   └───┬──────────────────────────────┘                           │
//! │       │ TCP                                                      │
//! │       ▼                                                          │
//! │   ┌──────────────────────────────────┐                           │
//! │   │ DOCKER NETWORK: agent-net-X      │  --internal               │
//! │   │   no NAT, no internet, no host   │                           │
//! │   └───┬──────────────────────────────┘                           │
//! │       │                                                          │
//! │   ┌───┴──────────────────────────────┐                           │
//! │   │ AGENT CONTAINER                  │  qwen → http://<inner_ip> │
//! │   │  qwen-agent-template:0.1.0       │             :8001/v1      │
//! │   │  ttyd 7681 → 127.0.0.1:<eph>     │                           │
//! │   │  --dns 127.0.0.1                 │                           │
//! │   └──────────────────────────────────┘                           │
//! └──────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ### Why two `socat` containers (and not a Rust forwarder, and not a
//! single hop)
//!
//! - **The agent must reach an OpenAI-compatible HTTP server**, with full
//!   support for streaming chat completions (SSE, chunked transfer) and
//!   long-lived TCP connections. socat is byte-level, so streaming and any
//!   future protocol upgrade (HTTP/2, WebSocket) traverse it transparently.
//! - **The host's vLLM is published with `-p 127.0.0.1:8001:8001`** —
//!   reachable only from the host's loopback, not from the bridge gateway
//!   IP that `host.docker.internal:host-gateway` resolves to. So the outer
//!   hop *must* run with `--network=host` to see the host's loopback.
//! - **Putting the data plane in our Rust process** would let our
//!   orchestrator's own bugs / dep CVEs reach into the proxy chain and
//!   would lose container-level isolation primitives (`--read-only`,
//!   `--cap-drop`, `--memory`, `--pids-limit`). We keep the orchestrator
//!   pure-control, the data plane pure-Docker.
//! - **The agent cannot directly reach the outer proxy.** It's on a
//!   `--internal` network that has no NAT to anywhere; the *only* peer it
//!   can reach is the inner proxy on the same network. The inner proxy
//!   then forwards via a bind-mounted Unix socket to the outer proxy, with
//!   no IP plumbing involved at all.
//!
//! ### Other hardening that lands here
//!
//! - **DNS exfiltration is closed.** Docker's embedded resolver at
//!   `127.0.0.11` forwards queries via the daemon namespace and reaches
//!   external resolvers regardless of `--internal`. We start every
//!   container in the chain with `--dns 127.0.0.1 --dns-search .` —
//!   resolv.conf points at a local loopback with nothing listening, so
//!   every DNS query (including the embedded one) fails immediately. The
//!   agent reaches the inner proxy by IP literal, never by name.
//! - **No `--gpus` flags anywhere.** Verified by inspection of every
//!   `run_detached` call site in this module.
//! - **No `--privileged`, no `CAP_*` adds beyond `NET_BIND_SERVICE`** for
//!   the agent (and even that is unused since ttyd binds 7681 — kept only
//!   to avoid surprising a future operator who lowers the port).
//! - **`--security-opt no-new-privileges:true`** on every container —
//!   `setuid` binaries inside cannot escalate.

use std::path::Path;
use std::time::Duration;

use crate::config::Config;
use crate::docker_ops;
use crate::error::{ServiceError, ServiceResult};

/// TCP port the inner proxy listens on, inside the internal network.
/// Hard-coded because the agent only ever reaches one endpoint.
pub const PROXY_LISTEN_PORT: u16 = 8001;
/// Container port ttyd binds inside the agent container.
pub const TTYD_CONTAINER_PORT: u16 = 7681;
/// In-container path of the bind-mounted Unix-socket directory. Both
/// proxies see it at the same path; only the file `vllm.sock` inside is
/// the actual rendezvous.
const IN_CONTAINER_SOCK_DIR: &str = "/sock";
/// Filename of the Unix socket inside `IN_CONTAINER_SOCK_DIR`.
const SOCK_FILENAME: &str = "vllm.sock";

/// Live handles to all per-session Docker objects. Held by `Session` for
/// its lifetime; `teardown` removes everything in reverse-creation order.
#[derive(Debug)]
pub struct IsolatedNetwork {
    pub network_name: String,
    pub outer_proxy_container_name: String,
    pub inner_proxy_container_name: String,
    /// IPv4 address of the inner proxy *on the internal network*. The
    /// agent uses this directly in `OPENAI_BASE_URL` because the agent is
    /// started with DNS pointed at a non-listening address and cannot
    /// resolve any hostname (see module-level comment).
    pub inner_proxy_ip: String,
    /// Docker label applied to every object created for this session, so
    /// `sweep_orphans` can find them on a startup sweep. Kept on the struct
    /// for diagnostics; not needed at teardown time because the names are
    /// already known.
    #[allow(dead_code)]
    pub session_label: String,
}

impl IsolatedNetwork {
    /// Bring up the full proxy chain for one session.
    ///
    /// `sock_dir_host` must already exist on the host filesystem with mode
    /// 0o777 (the staging code is responsible for that — see
    /// `staging::SessionPaths::create_dirs`). It will be bind-mounted into
    /// both proxies at `/sock`.
    ///
    /// Any failure mid-creation triggers a best-effort teardown of
    /// whatever was already created, so the next attempt doesn't leak
    /// orphan Docker objects.
    pub async fn create(
        cfg: &Config,
        session_id: &str,
        sock_dir_host: &Path,
    ) -> ServiceResult<Self> {
        // Bind-mount strings can't safely contain a colon (it's the
        // separator). Refuse anything funky in the path.
        let sock_dir_str = sock_dir_host
            .to_str()
            .ok_or_else(|| ServiceError::Internal(format!(
                "sock_dir_host {} is not valid UTF-8 — refusing to format a bind-mount",
                sock_dir_host.display()
            )))?;
        if sock_dir_str.contains(':') {
            return Err(ServiceError::Internal(format!(
                "sock_dir_host {sock_dir_str:?} contains a ':' — bind-mount syntax cannot represent this safely"
            )));
        }

        let session_label = format!("agent_service.session={session_id}");
        let network_name = format!("agent-net-{session_id}");
        let outer_proxy_container_name = format!("agent-outproxy-{session_id}");
        let inner_proxy_container_name = format!("agent-inproxy-{session_id}");

        // ── 1. Outer proxy: --network=host, UNIX-LISTEN → TCP:127.0.0.1:vllm
        //
        // socat options of note:
        //   `unlink-early` removes any stale socket file at the path before
        //                  binding (idempotent across restarts of any
        //                  prior, crashed orchestrator).
        //   `user=1000,group=1000,mode=0660` chowns + chmods the socket so
        //                  only uid 1000 (the inner proxy's user) can use
        //                  it; nothing else on the host can connect even
        //                  if the dir is 0o777.
        //   `fork`         each accepted connection is handled by a forked
        //                  child, so multiple chat completions multiplex.
        //   `reuseaddr`    not strictly meaningful for UNIX-LISTEN but
                  //                  harmless.
        let outer_listen = format!(
            "UNIX-LISTEN:{IN_CONTAINER_SOCK_DIR}/{SOCK_FILENAME},fork,reuseaddr,unlink-early,user=1000,group=1000,mode=0660"
        );
        let outer_target = format!("TCP:127.0.0.1:{}", cfg.vllm_port);
        let outer_sock_mount = format!("{sock_dir_str}:{IN_CONTAINER_SOCK_DIR}:rw");

        let outer_args: Vec<String> = vec![
            "--name".into(),
            outer_proxy_container_name.clone(),
            "--label".into(),
            session_label.clone(),
            "--network".into(),
            "host".into(),
            "--user".into(),
            "1000:1000".into(),
            "--entrypoint".into(),
            "socat".into(),
            "--read-only".into(),
            "--tmpfs".into(),
            "/tmp:rw,noexec,nosuid,size=4m".into(),
            "--cap-drop".into(),
            "ALL".into(),
            "--security-opt".into(),
            "no-new-privileges:true".into(),
            "--memory".into(),
            "64m".into(),
            "--pids-limit".into(),
            "32".into(),
            "-v".into(),
            outer_sock_mount,
            cfg.agent_image.clone(),
            // socat's CLI: zero-or-more options, then exactly two address
            // specs. The image's CMD is replaced by --entrypoint, so we
            // pass the two address specs as the run args.
            outer_listen,
            outer_target,
        ];

        docker_ops::run_detached(&outer_args, "outer_proxy_run").await?;

        // Wait for the outer socat to actually bind the socket file. Until
        // it does, the inner proxy's UNIX-CONNECT will get ENOENT.
        let host_sock_path = sock_dir_host.join(SOCK_FILENAME);
        if let Err(e) = wait_for_socket_file(&host_sock_path, Duration::from_secs(5)).await {
            // Capture diagnostics before tearing the outer proxy down.
            let logs = docker_ops::container_logs_tail(&outer_proxy_container_name, 100)
                .await
                .unwrap_or_else(|_| "<unavailable>".into());
            let _ = docker_ops::container_force_remove(&outer_proxy_container_name).await;
            return Err(ServiceError::Timeout(format!(
                "{e}; outer proxy logs:\n{logs}"
            )));
        }

        // ── 2. Internal network for the inner proxy + the agent.
        if let Err(e) = docker_ops::network_create_internal(&network_name, &session_label).await {
            let _ = docker_ops::container_force_remove(&outer_proxy_container_name).await;
            return Err(e);
        }

        // ── 3. Inner proxy: --network=<internal>, TCP-LISTEN → UNIX-CONNECT
        //
        // The inner proxy bind-mounts the same /sock dir (read-only — it
        // never modifies the directory; UNIX-CONNECT only opens the
        // existing socket inode for reading/writing socket data, which is
        // not a filesystem write).
        let inner_listen = format!("TCP-LISTEN:{PROXY_LISTEN_PORT},fork,reuseaddr");
        let inner_target = format!("UNIX-CONNECT:{IN_CONTAINER_SOCK_DIR}/{SOCK_FILENAME}");
        let inner_sock_mount = format!("{sock_dir_str}:{IN_CONTAINER_SOCK_DIR}:ro");

        let inner_args: Vec<String> = vec![
            "--name".into(),
            inner_proxy_container_name.clone(),
            "--label".into(),
            session_label.clone(),
            "--network".into(),
            network_name.clone(),
            "--user".into(),
            "1000:1000".into(),
            // DNS pointed at a non-listening loopback — closes embedded-DNS
            // exfiltration. See module-level comment.
            "--dns".into(),
            "127.0.0.1".into(),
            "--dns-search".into(),
            ".".into(),
            "--entrypoint".into(),
            "socat".into(),
            "--read-only".into(),
            "--tmpfs".into(),
            "/tmp:rw,noexec,nosuid,size=4m".into(),
            "--cap-drop".into(),
            "ALL".into(),
            "--security-opt".into(),
            "no-new-privileges:true".into(),
            "--memory".into(),
            "64m".into(),
            "--pids-limit".into(),
            "32".into(),
            "-v".into(),
            inner_sock_mount,
            cfg.agent_image.clone(),
            inner_listen,
            inner_target,
        ];

        if let Err(e) = docker_ops::run_detached(&inner_args, "inner_proxy_run").await {
            let _ = docker_ops::network_remove(&network_name).await;
            let _ = docker_ops::container_force_remove(&outer_proxy_container_name).await;
            return Err(e);
        }

        // ── 4. Inner proxy IP (DNS-free agent target).
        let inner_proxy_ip = match docker_ops::container_ip_on_network(
            &inner_proxy_container_name,
            &network_name,
        )
        .await
        {
            Ok(ip) => ip,
            Err(e) => {
                let _ = docker_ops::container_force_remove(&inner_proxy_container_name).await;
                let _ = docker_ops::network_remove(&network_name).await;
                let _ = docker_ops::container_force_remove(&outer_proxy_container_name).await;
                return Err(e);
            }
        };

        // ── 5. Brief settle for inner socat to bind on TCP 8001. We
        //    can't TCP-poll from here (inner is on the internal network,
        //    not visible to the host network namespace). socat binds in
        //    <50ms; 300ms gives 6× headroom.
        tokio::time::sleep(Duration::from_millis(300)).await;

        Ok(Self {
            network_name,
            outer_proxy_container_name,
            inner_proxy_container_name,
            inner_proxy_ip,
            session_label,
        })
    }

    /// Endpoint the agent should use as `OPENAI_BASE_URL`. Returns an IP
    /// literal so the agent never depends on DNS.
    pub fn agent_base_url(&self) -> String {
        format!("http://{}:{PROXY_LISTEN_PORT}/v1", self.inner_proxy_ip)
    }

    /// Best-effort teardown — collects diagnostic detail but never
    /// propagates errors as `Err`, since the caller is already finalising.
    /// Reverse-order: inner proxy → network → outer proxy.
    pub async fn teardown(self) -> Vec<String> {
        let mut diagnostics = Vec::new();
        if let Err(e) = docker_ops::container_force_remove(&self.inner_proxy_container_name).await
        {
            diagnostics.push(format!("inner proxy rm: {e}"));
        }
        if let Err(e) = docker_ops::network_remove(&self.network_name).await {
            diagnostics.push(format!("network rm: {e}"));
        }
        if let Err(e) = docker_ops::container_force_remove(&self.outer_proxy_container_name).await
        {
            diagnostics.push(format!("outer proxy rm: {e}"));
        }
        diagnostics
    }
}

async fn wait_for_socket_file(path: &Path, hard_timeout: Duration) -> ServiceResult<()> {
    use std::os::unix::fs::FileTypeExt;
    let deadline = std::time::Instant::now() + hard_timeout;
    while std::time::Instant::now() < deadline {
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_socket() => return Ok(()),
            Ok(_) => {
                return Err(ServiceError::Internal(format!(
                    "{} appeared but is not a socket — refusing to proceed",
                    path.display()
                )));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(e) => {
                return Err(ServiceError::Internal(format!(
                    "stat {}: {e}",
                    path.display()
                )));
            }
        }
    }
    Err(ServiceError::Timeout(format!(
        "outer proxy did not create the Unix socket at {} within {hard_timeout:?}",
        path.display()
    )))
}

/// Sweep any leftover docker objects that were left behind by a prior
/// crash. Called once at startup. Filters by the well-known label so we
/// never touch containers / networks belonging to anything else on the
/// host.
pub async fn sweep_orphans() -> ServiceResult<()> {
    let cs = docker_ops::run_docker(
        ["ps", "-aq", "--filter", "label=agent_service.session"],
        "sweep_containers",
    )
    .await?;
    for cid in cs.split_whitespace() {
        if let Err(e) = docker_ops::container_force_remove(cid).await {
            tracing::warn!(error = %e, container = cid, "sweep_orphans: rm container");
        }
    }
    let nets = docker_ops::run_docker(
        ["network", "ls", "-q", "--filter", "label=agent_service.session"],
        "sweep_networks",
    )
    .await?;
    for nid in nets.split_whitespace() {
        if let Err(e) = docker_ops::network_remove(nid).await {
            tracing::warn!(error = %e, network = nid, "sweep_orphans: rm network");
        }
    }
    Ok(())
}
