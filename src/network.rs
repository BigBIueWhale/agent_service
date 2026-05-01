//! Per-session network isolation: a chain of socat-running containers, two
//! Docker bridge networks, and the agent — all spawned and supervised by
//! this Rust process from the same `qwen-agent-template` image with
//! `--entrypoint socat` for the proxies. One binary, one image, one
//! byte-forwarding pattern.
//!
//! ```text
//! ┌────────────────────────────────────────────────────────────────────────┐
//! │ HOST                                                                   │
//! │                                                                        │
//! │   vLLM 127.0.0.1:<vllm_port>            ttyd publish 127.0.0.1:<eph>   │
//! │       ▲                                       ▲                        │
//! │       │ TCP                                   │ TCP   (-p, browser)    │
//! │   ┌───┴──────────────────────────────┐    ┌───┴──────────────────────┐ │
//! │   │ OUTER PROXY                       │    │ TTYD-SIDECAR             │ │
//! │   │  --network=host                  │    │  --entrypoint socat      │ │
//! │   │  --entrypoint socat              │    │  primary: agent-pub-X    │ │
//! │   │  UNIX-LISTEN:/sock/vllm.sock     │    │  also:    agent-net-X    │ │
//! │   │   → TCP:127.0.0.1:<vllm_port>    │    │  TCP-LISTEN:7681         │ │
//! │   │                                  │    │   → TCP:<agent_ip>:7681  │ │
//! │   └───┬──────────────────────────────┘    └───┬──────────────────────┘ │
//! │       │ Unix socket bind-mount                │ on TWO networks       │ │
//! │       │ (shared with inner proxy as /sock)    │                       │ │
//! │       │                              ┌────────┘                       │ │
//! │   ┌───┴──────────────────────────────┴┐                               │ │
//! │   │ INNER PROXY                       │  --network=agent-net-X        │ │
//! │   │  --entrypoint socat               │  TCP-LISTEN:8001              │ │
//! │   │  UNIX-CONNECT:/sock/vllm.sock     │   → host's vLLM via Unix sock │ │
//! │   └───┬───────────────────────────────┘                               │ │
//! │       │                                                               │ │
//! │   ┌───┴──────────────────────────────────────────┐                    │ │
//! │   │ DOCKER NETWORK: agent-net-X                  │                    │ │
//! │   │   --internal                                 │                    │ │
//! │   │   gateway_mode_ipv4=isolated  (no host iface)│                    │ │
//! │   │   no NAT, no internet, no host, no gateway   │                    │ │
//! │   └───┬──────────────────────────────────────────┘                    │ │
//! │       │                                                               │ │
//! │   ┌───┴────────────────────────────┐                                  │ │
//! │   │ AGENT CONTAINER                │  qwen → http://<inner_ip>:8001   │ │
//! │   │  qwen-agent-template:0.1.0     │  ttyd 7681 (container-local)     │ │
//! │   │  --dns 127.0.0.1               │  no `-p` — silently dropped on   │ │
//! │   │  on agent-net-X ONLY           │  --internal nets; the sidecar    │ │
//! │   │                                │  bridges to host loopback.       │ │
//! │   └────────────────────────────────┘                                  │ │
//! │                                                                        │ │
//! │   DOCKER NETWORK: agent-pub-X (sidecar's `-p` lives here, --internal   │ │
//! │     is incompatible with `-p` so this bridge is non-internal but has   │ │
//! │     enable_ip_masquerade=false to block NAT outbound for the sidecar.  │ │
//! │     The agent never touches it.)                                       │ │
//! └────────────────────────────────────────────────────────────────────────┘
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
///
/// The ttyd-sidecar fields are populated only after the agent itself has
/// started and `attach_ttyd_sidecar` has run — they are `None` between
/// `IsolatedNetwork::create` returning and the agent existing. Teardown
/// tolerates either state.
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
    /// Per-session non-internal bridge network the ttyd sidecar uses for
    /// `-p` host publishing. Created lazily by `attach_ttyd_sidecar`;
    /// `None` until then.
    pub publish_network_name: Option<String>,
    /// Per-session ttyd-publishing socat sidecar. Dual-attached to
    /// `publish_network_name` (primary, where `-p` lives) and
    /// `network_name` (where it forwards to ttyd inside the agent).
    /// `None` until `attach_ttyd_sidecar` runs.
    pub ttyd_sidecar_container_name: Option<String>,
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

        // ── 2. Agent network: --internal + gateway_mode_ipv4=isolated.
        //    This is the primary isolation primitive: no NAT (--internal),
        //    no reachable bridge gateway (isolated mode), no path off the
        //    bridge subnet for anything attached to it. Pre-flight has
        //    already verified the daemon honours the isolated-gateway flag.
        if let Err(e) = docker_ops::network_create_agent(&network_name, &session_label).await {
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
            publish_network_name: None,
            ttyd_sidecar_container_name: None,
            session_label,
        })
    }

    /// Endpoint the agent should use as `OPENAI_BASE_URL`. Returns an IP
    /// literal so the agent never depends on DNS.
    pub fn agent_base_url(&self) -> String {
        format!("http://{}:{PROXY_LISTEN_PORT}/v1", self.inner_proxy_ip)
    }

    /// Bring up the ttyd-publishing sidecar and return the host-loopback
    /// port the operator can browse. Called by `session::run_inner` AFTER
    /// the agent container has started and ttyd has bound on
    /// `<agent_ip>:7681` inside it.
    ///
    /// Topology:
    ///   1. Create per-session publish bridge `agent-pub-<sid>` (non-internal,
    ///      masquerade off — `-p` works, NAT outbound shut).
    ///   2. `docker run -d` sidecar with `--network=<publish bridge>` AND
    ///      `-p 127.0.0.1::7681`. socat is `TCP-LISTEN:7681,fork → TCP:<agent_ip>:7681`.
    ///      The publish bridge MUST be the primary network at run time —
    ///      Docker silently drops `-p` if the primary network is `--internal`.
    ///   3. `docker network connect <agent network> <sidecar>` to give the
    ///      sidecar a second interface on the agent's isolated bridge so it
    ///      can reach `agent_ip:7681`.
    ///   4. Look up the host port `docker port` assigned to the sidecar.
    ///   5. `wait_tcp_ready` on that port to confirm the listener is bound.
    ///
    /// On any error mid-setup, this function tears down whatever it
    /// created (sidecar container, publish network) before returning the
    /// error, so the caller's outer teardown path doesn't see partial state.
    /// Successful setup updates `self.publish_network_name` and
    /// `self.ttyd_sidecar_container_name`, so `teardown` removes them.
    pub async fn attach_ttyd_sidecar(
        &mut self,
        cfg: &Config,
        session_id: &str,
        agent_ip: &str,
    ) -> ServiceResult<u16> {
        // Defensive: refuse to attach a sidecar twice. Catches an
        // accidental double-call from session.rs in the future.
        if self.ttyd_sidecar_container_name.is_some() {
            return Err(ServiceError::Internal(
                "attach_ttyd_sidecar called twice for the same session".into(),
            ));
        }

        // Validate the agent IP — we splice it into a socat target spec.
        // container_ip_on_network already does an IpAddr parse, but we
        // re-check here as defence-in-depth: we never want shell or socat
        // metacharacters in this string.
        let _ip_check: std::net::IpAddr = agent_ip.parse().map_err(|e| {
            ServiceError::Internal(format!(
                "attach_ttyd_sidecar: agent_ip {agent_ip:?} is not a valid IP literal: {e}"
            ))
        })?;

        let publish_network_name = format!("agent-pub-{session_id}");
        let sidecar_container_name = format!("agent-ttydsc-{session_id}");

        // ── 1. Publish bridge.
        docker_ops::network_create_publish(&publish_network_name, &self.session_label)
            .await?;

        // From here, on any error path, tear down the publish network.
        let result: ServiceResult<u16> = async {
            // ── 2. Sidecar, primary on the publish bridge.
            let listen = format!("TCP-LISTEN:{TTYD_CONTAINER_PORT},fork,reuseaddr");
            let target = format!("TCP:{agent_ip}:{TTYD_CONTAINER_PORT}");
            let publish_arg = format!("127.0.0.1::{TTYD_CONTAINER_PORT}");

            let sidecar_args: Vec<String> = vec![
                "--name".into(),
                sidecar_container_name.clone(),
                "--label".into(),
                self.session_label.clone(),
                "--network".into(),
                publish_network_name.clone(),
                "-p".into(),
                publish_arg,
                "--user".into(),
                "1000:1000".into(),
                // DNS pointed at a non-listening loopback — defence-in-depth.
                // The sidecar reaches the agent by IP literal, never by name.
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
                cfg.agent_image.clone(),
                listen,
                target,
            ];

            docker_ops::run_detached(&sidecar_args, "ttyd_sidecar_run").await?;

            // ── 3. Attach the sidecar's second interface (the agent
            //    network) so it can reach agent_ip:7681. Order is
            //    load-bearing: had we attached it to the agent network
            //    first, `-p` would have silently dropped because that
            //    network is `--internal`.
            if let Err(e) =
                docker_ops::network_connect(&self.network_name, &sidecar_container_name).await
            {
                let _ =
                    docker_ops::container_force_remove(&sidecar_container_name).await;
                return Err(e);
            }

            // ── 4. Resolve the host port `-p` assigned to us.
            let host_port = docker_ops::container_published_port(
                &sidecar_container_name,
                TTYD_CONTAINER_PORT,
            )
            .await
            .map_err(|e| {
                ServiceError::DockerCommand(format!(
                    "ttyd sidecar did not publish 127.0.0.1:<eph>:{TTYD_CONTAINER_PORT}; \
                     this should never happen on a non-internal bridge — check \
                     `docker port {sidecar_container_name}` for diagnostics: {e}"
                ))
            })?;

            // ── 5. Confirm the sidecar's TCP-LISTEN is up. socat binds
            //    well under 100 ms; 5 s is generous headroom.
            docker_ops::wait_tcp_ready(host_port, Duration::from_secs(5)).await?;

            Ok(host_port)
        }
        .await;

        match result {
            Ok(host_port) => {
                self.publish_network_name = Some(publish_network_name);
                self.ttyd_sidecar_container_name = Some(sidecar_container_name);
                Ok(host_port)
            }
            Err(e) => {
                // Sidecar may or may not exist depending on which step
                // failed. force_remove is idempotent.
                let _ = docker_ops::container_force_remove(&sidecar_container_name).await;
                let _ = docker_ops::network_remove(&publish_network_name).await;
                Err(e)
            }
        }
    }

    /// Best-effort teardown — collects diagnostic detail but never
    /// propagates errors as `Err`, since the caller is already finalising.
    ///
    /// Reverse-creation order:
    ///   ttyd sidecar (if attached) → publish network (if created) →
    ///   inner proxy → agent network → outer proxy
    ///
    /// Note that the agent container itself is removed by `session.rs`
    /// before `teardown` is called — this method only handles the
    /// orchestrator's own infrastructure.
    pub async fn teardown(self) -> Vec<String> {
        let mut diagnostics = Vec::new();
        if let Some(sidecar) = &self.ttyd_sidecar_container_name {
            if let Err(e) = docker_ops::container_force_remove(sidecar).await {
                diagnostics.push(format!("ttyd sidecar rm: {e}"));
            }
        }
        if let Some(pub_net) = &self.publish_network_name {
            if let Err(e) = docker_ops::network_remove(pub_net).await {
                diagnostics.push(format!("publish network rm: {e}"));
            }
        }
        if let Err(e) = docker_ops::container_force_remove(&self.inner_proxy_container_name).await
        {
            diagnostics.push(format!("inner proxy rm: {e}"));
        }
        if let Err(e) = docker_ops::network_remove(&self.network_name).await {
            diagnostics.push(format!("agent network rm: {e}"));
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
