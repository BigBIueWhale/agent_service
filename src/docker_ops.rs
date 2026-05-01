//! Subprocess wrappers around the host `docker` CLI.
//!
//! Every public function returns `ServiceResult<...>` carrying a dynamic
//! message that includes both the failed argv and the captured stderr — so
//! whatever bubbles out at the API boundary is actionable, never opaque.

use std::ffi::OsStr;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::time::timeout;

use crate::error::{ServiceError, ServiceResult};

/// Maximum wall-clock for one docker invocation (anything that isn't the long
/// `wait` call). 60s is enough for a fresh `docker run` of a several-GiB
/// image, far more than `network create` / `inspect` / `stop` ever take.
const DOCKER_OP_TIMEOUT: Duration = Duration::from_secs(60);

/// Run `docker <args>` to completion, capturing stdout/stderr. Returns the
/// stdout on success; on failure the error message includes the argv and the
/// stderr (truncated) so the caller can act on it.
pub async fn run_docker<I, S>(args: I, op_label: &str) -> ServiceResult<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let argv: Vec<std::ffi::OsString> = args.into_iter().map(|s| s.as_ref().to_os_string()).collect();
    let argv_str = argv
        .iter()
        .map(|s| s.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ");

    let mut cmd = Command::new("docker");
    cmd.args(&argv);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let fut = async {
        let out = cmd.output().await.map_err(|e| {
            ServiceError::DockerCommand(format!(
                "{op_label}: failed to spawn `docker {argv_str}`: {e}"
            ))
        })?;

        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let code = out
                .status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "<signal>".into());
            return Err(ServiceError::DockerCommand(format!(
                "{op_label}: `docker {argv_str}` exited with code {code}; stderr: {}; stdout: {}",
                truncate(&stderr, 1024),
                truncate(&stdout, 256),
            )));
        }

        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    };

    match timeout(DOCKER_OP_TIMEOUT, fut).await {
        Ok(r) => r,
        Err(_) => Err(ServiceError::Timeout(format!(
            "{op_label}: `docker {argv_str}` exceeded {DOCKER_OP_TIMEOUT:?}"
        ))),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{head}…(truncated)")
    }
}

/// `docker info` — verifies that the daemon is reachable as the running user.
pub async fn ping_daemon() -> ServiceResult<()> {
    run_docker(["info", "--format", "{{.ServerVersion}}"], "ping_daemon")
        .await
        .map_err(|e| match e {
            ServiceError::DockerCommand(m) => ServiceError::DockerUnavailable(format!(
                "Docker daemon not reachable as the running user. Verify membership in the `docker` group, or that DOCKER_HOST is set correctly. Underlying error: {m}"
            )),
            other => other,
        })?;
    Ok(())
}

/// `docker image inspect <tag>` — returns Ok if the image exists locally.
pub async fn image_exists(tag: &str) -> ServiceResult<()> {
    run_docker(
        ["image", "inspect", "--format", "{{.Id}}", tag],
        &format!("image_exists({tag})"),
    )
    .await
    .map_err(|e| match e {
        ServiceError::DockerCommand(m) => ServiceError::ImageMissing(format!(
            "image `{tag}` is not present on this host. Build or pull it first. Underlying error: {m}"
        )),
        other => other,
    })?;
    Ok(())
}

/// Create the agent's per-session bridge network with maximum isolation.
///
/// Two flags compose here:
///
/// - `--internal` blocks all NAT to/from the outside world. Without it,
///   bridge networks NAT outbound to the internet by default.
/// - `com.docker.network.bridge.gateway_mode_ipv4=isolated` (Docker ≥ 27.1)
///   suppresses the host-side bridge IP entirely, so the bridge has no
///   gateway address that containers attached to it could reach. Without
///   this, the bridge gateway IP — i.e. the host's iface on the bridge —
///   is reachable from inside the network even with `--internal`, exposing
///   any service the host has bound to `0.0.0.0` (SSHd, etc.).
///
/// Together these two are strictly stronger than `--internal` alone, and
/// exactly what the original `agent_service` design intended ("no NAT, no
/// internet, no host"). Pre-flight verifies the daemon supports the
/// isolated-gateway mode (see `probe_gateway_isolated`).
pub async fn network_create_agent(name: &str, label: &str) -> ServiceResult<()> {
    run_docker(
        [
            "network", "create",
            "--driver", "bridge",
            "--internal",
            "-o", "com.docker.network.bridge.gateway_mode_ipv4=isolated",
            "--label", label,
            name,
        ],
        &format!("network_create_agent({name})"),
    )
    .await?;
    Ok(())
}

/// Create a per-session bridge network whose only purpose is to host the
/// ttyd sidecar's `-p 127.0.0.1::7681` publish to host loopback.
///
/// `--internal` silently drops `-p`, so this network deliberately omits
/// it. `enable_ip_masquerade=false` keeps NAT outbound shut so the sidecar
/// — even though it lives on a non-internal bridge — cannot reach the
/// internet. Only the sidecar is ever attached to this network; the agent
/// stays on the agent network only. Combined with the isolated-gateway
/// agent network, this means the agent has no path to the publish
/// bridge's gateway either.
pub async fn network_create_publish(name: &str, label: &str) -> ServiceResult<()> {
    run_docker(
        [
            "network", "create",
            "--driver", "bridge",
            "-o", "com.docker.network.bridge.enable_ip_masquerade=false",
            "--label", label,
            name,
        ],
        &format!("network_create_publish({name})"),
    )
    .await?;
    Ok(())
}

/// `docker network connect <network> <container>` — attach an already-running
/// container to an additional network. Used to dual-attach the ttyd sidecar
/// to both the publish network (where its `-p` lives) and the agent network
/// (where it forwards to ttyd).
pub async fn network_connect(network: &str, container: &str) -> ServiceResult<()> {
    run_docker(
        ["network", "connect", network, container],
        &format!("network_connect({network},{container})"),
    )
    .await?;
    Ok(())
}

/// Pre-flight probe: verify the local Docker daemon supports
/// `com.docker.network.bridge.gateway_mode_ipv4=isolated`. Creates a
/// throwaway labelled network with the option, immediately removes it.
///
/// Fail-loud at startup: this is the design's primary isolation primitive.
/// Without it, the agent's bridge would have a reachable gateway IP and
/// any host service bound on `0.0.0.0` would be reachable from the agent.
/// We refuse to come up rather than silently degrade to a weaker sandbox.
pub async fn probe_gateway_isolated() -> ServiceResult<()> {
    let probe_name = "agent-service-probe-gw-isolated";
    // Defensive cleanup in case a prior probe crashed mid-create.
    let _ = run_docker(
        ["network", "rm", probe_name],
        "probe_gateway_isolated_pre_rm",
    )
    .await;
    let create_result = run_docker(
        [
            "network", "create",
            "--driver", "bridge",
            "--internal",
            "-o", "com.docker.network.bridge.gateway_mode_ipv4=isolated",
            "--label", "agent_service.probe=gateway_isolated",
            probe_name,
        ],
        "probe_gateway_isolated_create",
    )
    .await;
    // Always try to remove the probe network — even if create succeeded.
    let _ = run_docker(
        ["network", "rm", probe_name],
        "probe_gateway_isolated_post_rm",
    )
    .await;
    create_result
        .map(|_| ())
        .map_err(|e| match e {
            ServiceError::DockerCommand(m) => ServiceError::Internal(format!(
                "Docker daemon does not accept \
                 `com.docker.network.bridge.gateway_mode_ipv4=isolated` (requires \
                 Docker Engine ≥ 27.1). The agent's network sandbox depends on it; \
                 without it, the bridge gateway IP would be reachable from inside \
                 the agent, exposing any 0.0.0.0-bound host service. Refusing to \
                 start. Underlying error: {m}"
            )),
            other => other,
        })
}

/// Verify a container has no default route — i.e. no path off its bridge
/// subnet. Run immediately after `docker run` of the agent to catch any
/// silent Docker semantic change that would otherwise re-introduce a
/// gateway. Cheap (one `docker exec`); runs once per session.
///
/// Expected output for an agent on `--internal + gateway_mode=isolated`:
///   `<subnet> dev <iface> proto kernel scope link [src <addr>]`
/// (one line, no `default via …`). Anything else is a hard failure: the
/// session aborts before any agent code runs.
pub async fn verify_no_default_route(container: &str) -> ServiceResult<()> {
    let out = run_docker(
        ["exec", container, "ip", "-4", "route", "show"],
        &format!("verify_no_default_route({container})"),
    )
    .await?;
    let trimmed = out.trim();
    if trimmed.is_empty() {
        return Err(ServiceError::Internal(format!(
            "verify_no_default_route({container}): `ip -4 route show` produced no \
             output, cannot confirm isolation; refusing to proceed"
        )));
    }
    for line in trimmed.lines() {
        let line = line.trim();
        if line.starts_with("default") {
            return Err(ServiceError::Internal(format!(
                "verify_no_default_route({container}): container has a default \
                 route ({line:?}), meaning its bridge has a reachable gateway. \
                 The isolated-gateway design requires no default route. This \
                 indicates Docker semantics changed under us — refusing to \
                 proceed rather than run with a weaker sandbox than promised."
            )));
        }
    }
    Ok(())
}

/// Inspect a container and return its IPv4 address on a specific Docker
/// network. Used to hand the agent the proxy's IP literal so the agent
/// never needs (or has) DNS — closing the embedded-DNS exfiltration channel
/// that `--internal` does NOT close on its own.
pub async fn container_ip_on_network(container: &str, network: &str) -> ServiceResult<String> {
    // The Go template needs both braces and quotes — building the format
    // string explicitly avoids a tower of escapes.
    let format = format!(
        "{{{{ (index .NetworkSettings.Networks \"{network}\").IPAddress }}}}"
    );
    let out = run_docker(
        ["inspect", "--format", format.as_str(), container],
        &format!("container_ip_on_network({container},{network})"),
    )
    .await?;
    let ip = out.trim().to_string();
    if ip.is_empty() {
        return Err(ServiceError::DockerCommand(format!(
            "container `{container}` has no IPv4 address recorded on network `{network}`; \
             is it actually attached to that network?"
        )));
    }
    // Defensive parse: refuse anything that's not a real IP address. We
    // splice this string into a URL passed to the agent — a hostname-like
    // value here would silently re-introduce DNS dependence.
    let parsed: std::net::IpAddr = ip.parse().map_err(|e| {
        ServiceError::DockerCommand(format!(
            "container `{container}` reports IP {ip:?} on network `{network}` \
             which is not parseable as an IpAddr: {e}"
        ))
    })?;
    if !matches!(parsed, std::net::IpAddr::V4(_)) {
        return Err(ServiceError::DockerCommand(format!(
            "container `{container}` reports non-IPv4 address {ip} on network `{network}`; \
             this code path expects IPv4 (Docker default for bridge networks)"
        )));
    }
    Ok(ip)
}

/// `docker network rm <name>` — best-effort.
pub async fn network_remove(name: &str) -> ServiceResult<()> {
    run_docker(["network", "rm", name], &format!("network_remove({name})"))
        .await?;
    Ok(())
}

/// One-shot probe of `--storage-opt size=…` support on the local daemon.
/// Runs a no-op container with the requested quota; returns Ok(()) iff
/// the daemon accepts the flag and the container starts and exits cleanly.
pub async fn probe_storage_quota(image: &str, quota: &str) -> ServiceResult<()> {
    let opt = format!("size={quota}");
    run_docker(
        [
            "run", "--rm",
            "--storage-opt", opt.as_str(),
            "--entrypoint", "/bin/true",
            image,
        ],
        "probe_storage_quota",
    )
    .await?;
    Ok(())
}

/// `docker rm -f <name>` — best-effort, idempotent.
pub async fn container_force_remove(name: &str) -> ServiceResult<()> {
    run_docker(
        ["rm", "-f", name],
        &format!("container_force_remove({name})"),
    )
    .await?;
    Ok(())
}

/// `docker stop -t <secs> <name>`.
pub async fn container_stop(name: &str, grace_secs: u32) -> ServiceResult<()> {
    let secs_owned = grace_secs.to_string();
    run_docker(
        ["stop", "-t", secs_owned.as_str(), name],
        &format!("container_stop({name})"),
    )
    .await?;
    Ok(())
}

/// `docker port <container> 7681/tcp` — returns the host-side `127.0.0.1:PORT`
/// the container's port is published on, or `Err(InvalidRequest...)` if no
/// mapping is set yet.
pub async fn container_published_port(name: &str, container_port: u16) -> ServiceResult<u16> {
    let arg = format!("{container_port}/tcp");
    let out = run_docker(
        ["port", name, arg.as_str()],
        &format!("container_published_port({name}:{container_port})"),
    )
    .await?;
    parse_port_mapping(&out).ok_or_else(|| {
        ServiceError::DockerCommand(format!(
            "container_published_port({name}:{container_port}): could not parse `docker port` output: {out:?}"
        ))
    })
}

/// `docker port` may emit several lines (one per address family); we look for
/// a `127.0.0.1:N` line specifically. We refuse `0.0.0.0:N` because we never
/// publish to non-loopback in this service.
fn parse_port_mapping(stdout: &str) -> Option<u16> {
    for line in stdout.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("127.0.0.1:") {
            if let Ok(p) = rest.parse::<u16>() {
                return Some(p);
            }
        }
    }
    None
}

/// `docker run -d ...` — used for both proxy and agent. Returns the
/// container ID (stdout of `docker run -d`).
pub async fn run_detached<I, S>(args: I, op_label: &str) -> ServiceResult<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut argv: Vec<std::ffi::OsString> = vec!["run".into(), "-d".into()];
    for a in args {
        argv.push(a.as_ref().to_os_string());
    }
    let stdout = run_docker(argv, op_label).await?;
    let id = stdout.trim().to_string();
    if id.is_empty() {
        return Err(ServiceError::DockerCommand(format!(
            "{op_label}: `docker run -d` produced empty stdout"
        )));
    }
    Ok(id)
}

/// `docker wait <container>` — blocks indefinitely (capped by the caller's
/// own timeout) until the container exits, returning the exit code.
pub async fn container_wait(name: &str, hard_timeout: Duration) -> ServiceResult<i32> {
    let mut cmd = Command::new("docker");
    cmd.args(["wait", name]);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| {
        ServiceError::DockerCommand(format!("container_wait({name}): spawn failed: {e}"))
    })?;

    let stdout_h = child.stdout.take();
    let read_fut = async {
        let mut buf = String::new();
        if let Some(s) = stdout_h {
            let mut reader = BufReader::new(s);
            while reader
                .read_line(&mut buf)
                .await
                .map_err(|e| ServiceError::DockerCommand(format!(
                    "container_wait({name}): read failed: {e}"
                )))? > 0
            {}
        }
        let status = child.wait().await.map_err(|e| {
            ServiceError::DockerCommand(format!("container_wait({name}): wait failed: {e}"))
        })?;
        if !status.success() {
            return Err(ServiceError::DockerCommand(format!(
                "container_wait({name}): `docker wait` exited non-zero: {status}"
            )));
        }
        let trimmed = buf.trim();
        let code: i32 = trimmed.parse().map_err(|e| {
            ServiceError::DockerCommand(format!(
                "container_wait({name}): exit code {trimmed:?} not parseable: {e}"
            ))
        })?;
        Ok(code)
    };

    match timeout(hard_timeout, read_fut).await {
        Ok(r) => r,
        Err(_) => {
            // Caller is responsible for the hard kill — we just report.
            Err(ServiceError::Timeout(format!(
                "container_wait({name}) exceeded {hard_timeout:?}"
            )))
        }
    }
}

/// `docker logs --tail N <container>` — used for diagnostic context when
/// something goes wrong before we have any structured output to parse.
pub async fn container_logs_tail(name: &str, tail: u32) -> ServiceResult<String> {
    let tail_str = tail.to_string();
    run_docker(
        ["logs", "--tail", tail_str.as_str(), name],
        &format!("container_logs_tail({name})"),
    )
    .await
}

/// Wait until a TCP connection to `127.0.0.1:port` succeeds. The agent's ttyd
/// takes a moment to bind after `docker run -d` returns, and we don't want to
/// hand the user a URL that's not yet live.
pub async fn wait_tcp_ready(port: u16, hard_timeout: Duration) -> ServiceResult<()> {
    let deadline = std::time::Instant::now() + hard_timeout;
    let mut last_err: Option<String> = None;
    while std::time::Instant::now() < deadline {
        match tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
            Ok(_) => return Ok(()),
            Err(e) => last_err = Some(e.to_string()),
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    Err(ServiceError::Timeout(format!(
        "ttyd at 127.0.0.1:{port} did not become reachable within {hard_timeout:?}; last error: {}",
        last_err.unwrap_or_else(|| "<none>".into())
    )))
}

