//! Narrow Docker authority broker for the one Qwen3.8 agent-service profile.
//!
//! The service never submits Docker argv, image names, mount paths, commands,
//! or resource limits. It can request only the typed operations in `Request`;
//! all Docker state is derived from the compiled policy and a strictly
//! validated session ID.

use std::ffi::OsStr;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{ExitCode, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::process::Command;

const POLICY_JSON: &str = include_str!("../../config/broker-policy-v1.json");
const DOCKER: &str = "/docker";
const REQUEST_LIMIT: u64 = 65_536;
const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
const RESPONSE_LIMIT: usize = 4 * 1024 * 1024;
const DOCKER_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    schema_version: u32,
    policy_id: String,
    profile: String,
    docker_server_version: String,
    broker_container_name: String,
    broker: BrokerPolicy,
    broker_socket_path: String,
    runtime_root: String,
    state_dir: String,
    model_socket_dir: String,
    service_container_name: String,
    backend_container_name: String,
    backend_cache_volume: String,
    backend_cache_mount: String,
    backend_cache_owner_mode: String,
    model_bridge_container_name: String,
    model_ingress_container_name: String,
    agent: AgentPolicy,
    relay: RelayPolicy,
    capture: CapturePolicy,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BrokerPolicy {
    image_tag: String,
    memory: String,
    memory_swap: String,
    pids_limit: u32,
    uid: u32,
    gid: u32,
    docker_socket: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentPolicy {
    image_tag: String,
    image_id: String,
    memory: String,
    memory_swap: String,
    pids_limit: u32,
    tmpfs_tmp: String,
    tmpfs_qwen_runtime: String,
    ready_event_prefix: String,
    sandbox: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RelayPolicy {
    image_tag: String,
    image_id: String,
    sandbox: String,
    memory: String,
    memory_swap: String,
    pids_limit: u32,
    role: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CapturePolicy {
    image_tag: String,
    image_id: String,
    capture_id: String,
    memory: String,
    memory_swap: String,
    pids_limit: u32,
    ready_event: String,
    complete_event_prefix: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Preflight,
    SweepOrphans,
    CreateSession { session_id: String },
    ProveSessionQuiescent { session_id: String },
    SessionLogs { session_id: String },
    WaitSession { session_id: String },
    WaitAgentReady { session_id: String },
    WaitCaptureComplete { session_id: String },
    StopSession { session_id: String },
    RemoveSession { session_id: String },
}

#[derive(Debug, Serialize)]
struct Response {
    ok: bool,
    data: Value,
    error: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Component {
    Agent,
    SessionRelay,
    SessionCapture,
}

impl Component {
    fn label(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::SessionRelay => "session-model-relay",
            Self::SessionCapture => "session-capture",
        }
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> ExitCode {
    let policy = match load_policy() {
        Ok(value) => value,
        Err(error) => {
            eprintln!("docker_broker: policy failure: {error}");
            return ExitCode::from(2);
        }
    };
    if let Err(error) = verify_self(&policy).await {
        eprintln!("docker_broker: self-preflight failed: {error}");
        return ExitCode::from(3);
    }
    let shutdown_signals = match install_termination_signals() {
        Ok(value) => value,
        Err(error) => {
            eprintln!("docker_broker: signal preflight failed: {error}");
            return ExitCode::from(6);
        }
    };
    let socket = PathBuf::from(&policy.broker_socket_path);
    if let Err(error) = validate_socket_parent(&socket) {
        eprintln!("docker_broker: socket contract failed: {error}");
        return ExitCode::from(4);
    }
    let listener = match UnixListener::bind(&socket) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("docker_broker: bind {} failed: {error}", socket.display());
            return ExitCode::from(4);
        }
    };
    if let Err(error) = std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o660)) {
        eprintln!(
            "docker_broker: chmod 0660 {} failed: {error}",
            socket.display()
        );
        return ExitCode::from(4);
    }
    println!(
        "BROKER_READY policy={} socket={}",
        policy.policy_id, policy.broker_socket_path
    );
    let policy = std::sync::Arc::new(policy);
    let mutation_lock = std::sync::Arc::new(tokio::sync::Mutex::new(()));
    let shutdown = termination_signal(shutdown_signals);
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let policy = std::sync::Arc::clone(&policy);
                    let mutation_lock = std::sync::Arc::clone(&mutation_lock);
                    tokio::spawn(async move {
                        if let Err(error) = serve_connection(stream, &policy, &mutation_lock).await {
                            eprintln!("docker_broker: request failed before response: {error}");
                        }
                    });
                }
                Err(error) => {
                    eprintln!("docker_broker: accept failed: {error}");
                    return ExitCode::from(5);
                }
            },
            signal = &mut shutdown => {
                match signal {
                    Ok(()) => {
                        // The broker is the only process authorized to remove
                        // session containers. A direct broker shutdown must
                        // therefore leave no owned agent/relay behind even if
                        // the service crashed before requesting its sweep.
                        let _mutation_guard = mutation_lock.lock().await;
                        if let Err(error) = sweep_orphans(&policy).await {
                            eprintln!("docker_broker: shutdown orphan sweep failed: {error}");
                            return ExitCode::from(7);
                        }
                        drop(listener);
                        if let Err(error) = remove_owned_broker_socket(&socket) {
                            eprintln!("docker_broker: shutdown socket cleanup failed: {error}");
                            return ExitCode::from(8);
                        }
                        println!("BROKER_STOPPED sweep=complete socket=removed");
                        return ExitCode::SUCCESS;
                    }
                    Err(error) => {
                        eprintln!("docker_broker: signal handling failed: {error}");
                        return ExitCode::from(6);
                    }
                }
            }
        }
    }
}

fn remove_owned_broker_socket(path: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("cannot stat broker socket {}: {error}", path.display()))?;
    if !metadata.file_type().is_socket()
        || metadata.uid() != 1000
        || metadata.gid() != 984
        || metadata.permissions().mode() & 0o777 != 0o660
    {
        return Err(format!(
            "refusing to remove broker socket with drift at {}: socket={} uid={} gid={} mode={:o}",
            path.display(),
            metadata.file_type().is_socket(),
            metadata.uid(),
            metadata.gid(),
            metadata.permissions().mode() & 0o777
        ));
    }
    std::fs::remove_file(path)
        .map_err(|error| format!("remove broker socket {}: {error}", path.display()))
}

fn load_policy() -> Result<Policy, String> {
    let policy: Policy = serde_json::from_str(POLICY_JSON)
        .map_err(|error| format!("compiled broker policy is invalid: {error}"))?;
    validate_policy(&policy)?;
    Ok(policy)
}

fn validate_policy(policy: &Policy) -> Result<(), String> {
    if policy.schema_version != 1
        || policy.policy_id != "qwen38-docker-broker-v1"
        || policy.profile != "qwen38-agent-service-v3"
        || policy.docker_server_version != "29.7.2"
        || policy.broker_container_name != "qwen38-docker-broker"
        || policy.service_container_name != "qwen38-agent-service"
        || policy.backend_container_name != "qwen38-agent-native"
        || policy.backend_cache_volume
            != "qwen38-vllm-cache-socket-isolated-nonroot-vision-agent-v13"
        || policy.backend_cache_mount != "/home/vllm/.cache/vllm"
        || policy.backend_cache_owner_mode != "2000:0:770"
        || policy.model_bridge_container_name != "qwen38-model-bridge"
        || policy.model_ingress_container_name != "qwen38-model-ingress"
    {
        return Err("broker identity/profile/container contract drift".into());
    }
    let runtime = Path::new(&policy.runtime_root);
    let socket = Path::new(&policy.broker_socket_path);
    let state = Path::new(&policy.state_dir);
    let model_socket = Path::new(&policy.model_socket_dir);
    if !runtime.is_absolute()
        || !socket.starts_with(runtime)
        || !state.starts_with(runtime)
        || !model_socket.starts_with(runtime)
        || socket.file_name().and_then(OsStr::to_str) != Some("broker.sock")
    {
        return Err(
            "broker runtime/socket/state/model-socket paths are not exact descendants".into(),
        );
    }
    if policy.broker.image_tag != "qwen38-docker-broker:1.0.0"
        || policy.broker.memory != "64m"
        || policy.broker.memory_swap != "64m"
        || policy.broker.pids_limit != 64
        || policy.broker.uid != 1000
        || policy.broker.gid != 984
        || policy.broker.docker_socket != "/var/run/docker.sock"
        || policy.agent.image_tag != "qwen38-agent:0.21.12-b965d5f8-v6"
        || policy.relay.image_tag != "qwen38-fixed-relay:1.0.0"
        || policy.capture.image_tag != "qwen38-session-capture:1.0.0"
        || !is_image_id(&policy.agent.image_id)
        || !is_image_id(&policy.relay.image_id)
        || !is_image_id(&policy.capture.image_id)
        || policy.agent.memory != "32g"
        || policy.agent.memory_swap != "32g"
        || policy.agent.pids_limit != 4096
        || policy.agent.tmpfs_tmp != "rw,nosuid,nodev,size=8g,mode=1777"
        || policy.agent.tmpfs_qwen_runtime
            != "rw,nosuid,nodev,noexec,size=2g,uid=1000,gid=1000,mode=0700"
        || policy.agent.ready_event_prefix
            != "AGENT_READY model=qwen3.8-27b-nvfp4-k8v4 context=262144 network=loopback-only token_count="
        || policy.agent.sandbox != "landlock-fs-v4-write-roots-v1+output-unmounted-v1"
        || policy.relay.memory != "32m"
        || policy.relay.memory_swap != "32m"
        || policy.relay.pids_limit != 32
        || policy.relay.sandbox != "landlock-net-v4+seccomp-socket-v2"
        || policy.relay.role != "agent-model"
        || policy.capture.capture_id != "unix-stream-capture-v1"
        || policy.capture.memory != "32m"
        || policy.capture.memory_swap != "32m"
        || policy.capture.pids_limit != 32
        || policy.capture.ready_event
            != "CAPTURE_READY capture=unix-stream-capture-v1 events=/streams/events.sock stderr=/streams/stderr.sock"
        || policy.capture.complete_event_prefix
            != "CAPTURE_COMPLETE capture=unix-stream-capture-v1 events_bytes="
    {
        return Err("broker image/resource/session policy drift".into());
    }
    Ok(())
}

fn is_image_id(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn validate_session_id(value: &str) -> Result<(), String> {
    let suffix = value
        .strip_prefix("s-")
        .ok_or_else(|| "session ID must start with s-".to_string())?;
    if suffix.len() != 32
        || !suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(format!(
            "session ID must be s- followed by exactly 32 lowercase hexadecimal characters: {value:?}"
        ));
    }
    Ok(())
}

fn validate_socket_parent(path: &Path) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("broker socket path has no parent: {}", path.display()))?;
    let metadata = std::fs::symlink_metadata(parent).map_err(|error| {
        format!(
            "cannot stat broker socket parent {}: {error}",
            parent.display()
        )
    })?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != 1000
        || metadata.permissions().mode() & 0o777 != 0o700
    {
        return Err(format!(
            "broker socket parent must be a real uid-1000 mode-0700 directory: {}",
            parent.display()
        ));
    }
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(metadata) => Err(format!(
            "refusing to replace pre-existing broker socket {} of type {:?}",
            path.display(),
            metadata.file_type()
        )),
        Err(error) => Err(format!(
            "cannot inspect broker socket {}: {error}",
            path.display()
        )),
    }
}

async fn serve_connection(
    mut stream: UnixStream,
    policy: &Policy,
    mutation_lock: &tokio::sync::Mutex<()>,
) -> Result<(), String> {
    let mut bytes = Vec::new();
    (&mut stream)
        .take(REQUEST_LIMIT + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| format!("read request: {error}"))?;
    let response = if bytes.len() as u64 > REQUEST_LIMIT {
        Response {
            ok: false,
            data: Value::Null,
            error: Some(format!("broker request exceeds {REQUEST_LIMIT} bytes")),
        }
    } else {
        match parse_request(&bytes) {
            Ok(request) => {
                let result = if request.requires_mutation_lock() {
                    let _guard = mutation_lock.lock().await;
                    execute(request, policy).await
                } else {
                    execute(request, policy).await
                };
                match result {
                    Ok(data) => Response {
                        ok: true,
                        data,
                        error: None,
                    },
                    Err(error) => Response {
                        ok: false,
                        data: Value::Null,
                        error: Some(error),
                    },
                }
            }
            Err(error) => Response {
                ok: false,
                data: Value::Null,
                error: Some(error),
            },
        }
    };
    let mut output = serde_json::to_vec(&response)
        .map_err(|error| format!("serialize broker response: {error}"))?;
    if output.len() + 1 > RESPONSE_LIMIT {
        output = serde_json::to_vec(&Response {
            ok: false,
            data: Value::Null,
            error: Some(format!(
                "broker response exceeded the exact {RESPONSE_LIMIT}-byte wire limit"
            )),
        })
        .map_err(|error| format!("serialize bounded broker error response: {error}"))?;
    }
    output.push(b'\n');
    stream
        .write_all(&output)
        .await
        .map_err(|error| format!("write response: {error}"))?;
    stream
        .shutdown()
        .await
        .map_err(|error| format!("shutdown response: {error}"))
}

impl Request {
    fn requires_mutation_lock(&self) -> bool {
        matches!(
            self,
            Self::SweepOrphans
                | Self::CreateSession { .. }
                | Self::StopSession { .. }
                | Self::RemoveSession { .. }
        )
    }
}

fn parse_request(bytes: &[u8]) -> Result<Request, String> {
    if bytes.is_empty() || !bytes.ends_with(b"\n") || bytes.contains(&b'\r') {
        return Err("broker request must be one terminal-LF JSON record without CR bytes".into());
    }
    if bytes[..bytes.len() - 1].contains(&b'\n') {
        return Err("broker connection may carry exactly one request record".into());
    }
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("invalid broker request: {error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "broker request must be a JSON object".to_string())?;
    let operation = object
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(|| "broker request op must be a string".to_string())?;
    let expected = match operation {
        "preflight" | "sweep_orphans" => ["op"].as_slice(),
        "create_session"
        | "prove_session_quiescent"
        | "session_logs"
        | "wait_session"
        | "wait_agent_ready"
        | "wait_capture_complete"
        | "stop_session"
        | "remove_session" => ["op", "session_id"].as_slice(),
        _ => return Err(format!("unsupported broker operation: {operation:?}")),
    };
    let observed = object
        .keys()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    let expected = expected
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    if observed != expected {
        return Err(format!(
            "broker operation {operation:?} fields differ: expected {expected:?}, observed {observed:?}"
        ));
    }
    serde_json::from_value(value).map_err(|error| format!("invalid broker request: {error}"))
}

async fn execute(request: Request, policy: &Policy) -> Result<Value, String> {
    match request {
        Request::Preflight => preflight(policy).await,
        Request::SweepOrphans => {
            sweep_orphans(policy).await?;
            Ok(json!({}))
        }
        Request::CreateSession { session_id } => {
            validate_session_id(&session_id)?;
            create_session(policy, &session_id).await?;
            Ok(json!({
                "agent": agent_name(&session_id),
                "relay": relay_name(&session_id),
                "capture": capture_name(&session_id),
            }))
        }
        Request::ProveSessionQuiescent { session_id } => {
            validate_session_id(&session_id)?;
            let agent = inspect_optional(
                &agent_name(&session_id),
                policy,
                &session_id,
                Component::Agent,
            )
            .await?;
            let relay = inspect_optional(
                &relay_name(&session_id),
                policy,
                &session_id,
                Component::SessionRelay,
            )
            .await?;
            let capture = inspect_optional(
                &capture_name(&session_id),
                policy,
                &session_id,
                Component::SessionCapture,
            )
            .await?;
            // This is deliberately tolerant of a partially completed removal:
            // one owned container may already be absent while the other is
            // stopped but could not be removed. The only fact authorizing a
            // bundle is that every extant exact-owned container is proved
            // stopped. Asymmetry is reported through presence fields rather
            // than confused with a running process.
            let agent_present = agent.is_some();
            let relay_present = relay.is_some();
            let capture_present = capture.is_some();
            let agent_running = optional_running(agent.as_ref(), "agent")?;
            let relay_running = optional_running(relay.as_ref(), "session relay")?;
            let capture_running = optional_running(capture.as_ref(), "session capture")?;
            Ok(json!({
                "agent_present": agent_present,
                "agent_running": agent_running,
                "relay_present": relay_present,
                "relay_running": relay_running,
                "capture_present": capture_present,
                "capture_running": capture_running,
                "quiescent": !agent_running && !relay_running && !capture_running,
            }))
        }
        Request::SessionLogs { session_id } => {
            validate_session_id(&session_id)?;
            let agent_logs = if inspect_optional(
                &agent_name(&session_id),
                policy,
                &session_id,
                Component::Agent,
            )
            .await?
            .is_some()
            {
                docker(
                    ["logs", "--tail", "300", agent_name(&session_id).as_str()],
                    "session_logs",
                    Some(DOCKER_TIMEOUT),
                )
                .await?
            } else {
                "<owned agent absent>".to_string()
            };
            let relay_logs = if inspect_optional(
                &relay_name(&session_id),
                policy,
                &session_id,
                Component::SessionRelay,
            )
            .await?
            .is_some()
            {
                docker(
                    ["logs", "--tail", "300", relay_name(&session_id).as_str()],
                    "session_relay_logs",
                    Some(DOCKER_TIMEOUT),
                )
                .await?
            } else {
                "<owned session relay absent>".to_string()
            };
            let capture_logs = if inspect_optional(
                &capture_name(&session_id),
                policy,
                &session_id,
                Component::SessionCapture,
            )
            .await?
            .is_some()
            {
                docker(
                    ["logs", "--tail", "300", capture_name(&session_id).as_str()],
                    "session_capture_logs",
                    Some(DOCKER_TIMEOUT),
                )
                .await?
            } else {
                "<owned session capture absent>".to_string()
            };
            Ok(json!({
                "logs": agent_logs,
                "relay_logs": relay_logs,
                "capture_logs": capture_logs,
            }))
        }
        Request::WaitSession { session_id } => {
            validate_session_id(&session_id)?;
            require_owned(
                &agent_name(&session_id),
                policy,
                &session_id,
                Component::Agent,
            )
            .await?;
            let output = docker(
                ["wait", agent_name(&session_id).as_str()],
                "wait_session",
                None,
            )
            .await?;
            let exit_code = output
                .split_whitespace()
                .next()
                .ok_or_else(|| "docker wait returned no exit code".to_string())?
                .parse::<i32>()
                .map_err(|error| format!("docker wait returned invalid exit code: {error}"))?;
            Ok(json!({"exit_code": exit_code}))
        }
        Request::WaitAgentReady { session_id } => {
            validate_session_id(&session_id)?;
            require_owned(
                &agent_name(&session_id),
                policy,
                &session_id,
                Component::Agent,
            )
            .await?;
            let line = wait_for_log_event(
                &agent_name(&session_id),
                &policy.agent.ready_event_prefix,
                Duration::from_secs(45),
                false,
            )
            .await?;
            let token_count = parse_agent_ready(&line, policy)?;
            Ok(json!({
                "model": "qwen3.8-27b-nvfp4-k8v4",
                "context_window": 262144,
                "token_count": token_count,
                "sandbox": policy.agent.sandbox,
            }))
        }
        Request::WaitCaptureComplete { session_id } => {
            validate_session_id(&session_id)?;
            require_owned(
                &capture_name(&session_id),
                policy,
                &session_id,
                Component::SessionCapture,
            )
            .await?;
            let line = wait_for_log_match(
                &capture_name(&session_id),
                &policy.capture.complete_event_prefix,
                None,
                false,
            )
            .await?;
            let (events_bytes, stderr_bytes) = parse_capture_complete(&line, policy)?;
            Ok(json!({
                "events_bytes": events_bytes,
                "stderr_bytes": stderr_bytes,
            }))
        }
        Request::StopSession { session_id } => {
            validate_session_id(&session_id)?;
            require_owned(
                &agent_name(&session_id),
                policy,
                &session_id,
                Component::Agent,
            )
            .await?;
            docker(
                ["stop", "--timeout", "-1", agent_name(&session_id).as_str()],
                "stop_session",
                None,
            )
            .await?;
            Ok(json!({}))
        }
        Request::RemoveSession { session_id } => {
            validate_session_id(&session_id)?;
            remove_session(policy, &session_id).await?;
            Ok(json!({}))
        }
    }
}

async fn preflight(policy: &Policy) -> Result<Value, String> {
    let version = docker(
        ["version", "--format", "{{.Server.Version}}"],
        "docker_version",
        Some(DOCKER_TIMEOUT),
    )
    .await?;
    if version.trim() != policy.docker_server_version {
        return Err(format!(
            "Docker server drift: expected {}, observed {:?}",
            policy.docker_server_version,
            version.trim()
        ));
    }
    let agent_image = inspect_json(&policy.agent.image_tag, "agent_image").await?;
    let relay_image = inspect_json(&policy.relay.image_tag, "relay_image").await?;
    let capture_image = inspect_json(&policy.capture.image_tag, "capture_image").await?;
    require_image_id(&agent_image, &policy.agent.image_id, "agent image")?;
    require_image_id(&relay_image, &policy.relay.image_id, "relay image")?;
    require_image_id(&capture_image, &policy.capture.image_id, "capture image")?;
    let broker = inspect_json(&policy.broker_container_name, "broker").await?;
    let backend = inspect_json(&policy.backend_container_name, "backend").await?;
    let service = inspect_json(&policy.service_container_name, "service").await?;
    let model_bridge = inspect_json(&policy.model_bridge_container_name, "model_bridge").await?;
    let model_ingress = inspect_json(&policy.model_ingress_container_name, "model_ingress").await?;
    let backend_cache_volume =
        inspect_json(&policy.backend_cache_volume, "backend_cache_volume").await?;
    let backend_cache_owner_mode = docker(
        [
            "exec",
            policy.backend_container_name.as_str(),
            "/usr/bin/stat",
            "-c",
            "%u:%g:%a",
            policy.backend_cache_mount.as_str(),
        ],
        "backend_cache_owner_mode",
        Some(DOCKER_TIMEOUT),
    )
    .await?;
    if backend_cache_owner_mode.trim() != policy.backend_cache_owner_mode {
        return Err(format!(
            "backend cache owner/mode drift: expected {}, observed {:?}",
            policy.backend_cache_owner_mode,
            backend_cache_owner_mode.trim(),
        ));
    }
    let (backend_ipv4_routes, backend_ipv6_routes) =
        require_network_none_routes(policy.backend_container_name.as_str(), "backend").await?;
    let (service_ipv4_routes, service_ipv6_routes) =
        require_network_none_routes(policy.service_container_name.as_str(), "service").await?;
    let gpu = docker(
        [
            "exec",
            policy.backend_container_name.as_str(),
            "nvidia-smi",
            "--query-gpu=name,memory.total,driver_version",
            "--format=csv,noheader,nounits",
        ],
        "backend_gpu",
        Some(DOCKER_TIMEOUT),
    )
    .await?;
    Ok(json!({
        "policy_id": policy.policy_id,
        "profile": policy.profile,
        "docker_version": version.trim(),
        "agent_image": agent_image,
        "relay_image": relay_image,
        "capture_image": capture_image,
        "broker": broker,
        "backend": backend,
        "service": service,
        "model_bridge": model_bridge,
        "model_ingress": model_ingress,
        "backend_cache_volume": backend_cache_volume,
        "backend_cache_owner_mode": backend_cache_owner_mode.trim(),
        "backend_ipv4_routes": backend_ipv4_routes.trim(),
        "backend_ipv6_routes": backend_ipv6_routes.trim(),
        "service_ipv4_routes": service_ipv4_routes.trim(),
        "service_ipv6_routes": service_ipv6_routes.trim(),
        "gpu_record": gpu.trim(),
    }))
}

/// Prove an exact network-none route state without requiring `iproute2` in the
/// inspected container.  Both the backend and service images are deliberately
/// minimal, but Linux exposes the authoritative per-network-namespace tables
/// through procfs.  The shell expression is fixed broker policy rather than
/// caller-controlled input; it distinguishes an absent IPv6 table (IPv6 is not
/// available in the namespace) from a present table that must be readable.
async fn require_network_none_routes(
    container: &str,
    label: &str,
) -> Result<(String, String), String> {
    let ipv4_label = format!("{label}_ipv4_routes");
    let ipv4 = docker(
        ["exec", container, "/usr/bin/cat", "/proc/net/route"],
        &ipv4_label,
        Some(DOCKER_TIMEOUT),
    )
    .await?;
    validate_empty_ipv4_route_table(&ipv4, label)?;

    let ipv6_label = format!("{label}_ipv6_routes");
    let ipv6 = docker(
        [
            "exec",
            container,
            "/bin/sh",
            "-ceu",
            "if [ -e /proc/net/ipv6_route ]; then /usr/bin/cat /proc/net/ipv6_route; fi",
        ],
        &ipv6_label,
        Some(DOCKER_TIMEOUT),
    )
    .await?;
    validate_empty_ipv6_route_table(&ipv6, label)?;

    // The service-side schema intentionally receives normalized empty route
    // evidence after the raw procfs records have passed the checks above.
    Ok((String::new(), String::new()))
}

fn validate_empty_ipv4_route_table(table: &str, label: &str) -> Result<(), String> {
    const EXPECTED_HEADER: [&str; 11] = [
        "Iface",
        "Destination",
        "Gateway",
        "Flags",
        "RefCnt",
        "Use",
        "Metric",
        "Mask",
        "MTU",
        "Window",
        "IRTT",
    ];

    let mut lines = table.lines();
    let header = lines
        .next()
        .ok_or_else(|| format!("{label} IPv4 procfs route table is missing its header"))?;
    if header.split_whitespace().collect::<Vec<_>>() != EXPECTED_HEADER {
        return Err(format!(
            "{label} IPv4 procfs route header drift: {:?}",
            truncate(header, 1024)
        ));
    }
    if let Some(route) = lines.find(|line| !line.trim().is_empty()) {
        return Err(format!(
            "{label} network-none namespace has a forbidden IPv4 route: {:?}",
            truncate(route, 1024)
        ));
    }
    Ok(())
}

fn validate_empty_ipv6_route_table(table: &str, label: &str) -> Result<(), String> {
    if let Some(route) = table.lines().find(|line| !line.trim().is_empty()) {
        return Err(format!(
            "{label} network-none namespace has a forbidden IPv6 route: {:?}",
            truncate(route, 1024)
        ));
    }
    Ok(())
}

fn require_image_id(value: &Value, expected: &str, label: &str) -> Result<(), String> {
    let objects = value
        .as_array()
        .filter(|objects| objects.len() == 1)
        .ok_or_else(|| format!("{label} inspect must contain exactly one object"))?;
    let observed = objects[0]
        .get("Id")
        .and_then(Value::as_str)
        .unwrap_or("<missing>");
    if observed != expected {
        return Err(format!(
            "{label} identity drift: expected {expected}, observed {observed}"
        ));
    }
    Ok(())
}

async fn verify_self(policy: &Policy) -> Result<(), String> {
    let version = docker(
        ["version", "--format", "{{.Server.Version}}"],
        "self_docker_version",
        Some(DOCKER_TIMEOUT),
    )
    .await?;
    if version.trim() != policy.docker_server_version {
        return Err(format!(
            "Docker version drift: expected {}, observed {:?}",
            policy.docker_server_version,
            version.trim()
        ));
    }
    let value = inspect_single(&policy.broker_container_name, "broker_self").await?;
    require_bool(&value, "/State/Running", true, "broker running state")?;
    let image_id = value
        .get("Image")
        .and_then(Value::as_str)
        .ok_or_else(|| "broker inspect lacks image ID".to_string())?;
    if !is_image_id(image_id) {
        return Err(format!("broker image ID is not immutable: {image_id:?}"));
    }
    require_string(
        &value,
        "/Config/Image",
        image_id,
        "broker configured image ID",
    )?;
    require_string(
        &value,
        "/Config/User",
        &format!("{}:{}", policy.broker.uid, policy.broker.gid),
        "broker user",
    )?;
    require_string(
        &value,
        "/HostConfig/NetworkMode",
        "none",
        "broker network mode",
    )?;
    require_bool(
        &value,
        "/HostConfig/ReadonlyRootfs",
        true,
        "broker read-only root",
    )?;
    require_bool(
        &value,
        "/HostConfig/Privileged",
        false,
        "broker privileged flag",
    )?;
    require_u64(
        &value,
        "/HostConfig/Memory",
        parse_memory(&policy.broker.memory)?,
        "broker memory limit",
    )?;
    require_u64(
        &value,
        "/HostConfig/MemorySwap",
        parse_memory(&policy.broker.memory_swap)?,
        "broker memory+swap limit",
    )?;
    require_u64(
        &value,
        "/HostConfig/PidsLimit",
        u64::from(policy.broker.pids_limit),
        "broker PID limit",
    )?;
    require_string(
        &value,
        "/Config/Labels/agent_service.component",
        "docker-broker",
        "broker component label",
    )?;
    require_string(
        &value,
        "/Config/Labels/agent_service.profile",
        &policy.profile,
        "broker profile label",
    )?;
    require_exact_command(
        &value,
        "/Config/Entrypoint",
        &["/docker_broker"],
        "broker entrypoint",
    )?;
    require_null_or_empty_array(&value, "/Config/Cmd", "broker command")?;
    require_hardening(&value, "broker")?;
    require_empty_object(&value, "/HostConfig/PortBindings", "broker port bindings")?;
    require_exact_mounts(
        &value,
        &[
            MountContract {
                source: &policy.broker.docker_socket,
                destination: &policy.broker.docker_socket,
                writable: false,
            },
            MountContract {
                source: Path::new(&policy.broker_socket_path)
                    .parent()
                    .and_then(Path::to_str)
                    .ok_or_else(|| "broker socket parent is not UTF-8".to_string())?,
                destination: Path::new(&policy.broker_socket_path)
                    .parent()
                    .and_then(Path::to_str)
                    .ok_or_else(|| "broker socket parent is not UTF-8".to_string())?,
                writable: true,
            },
            MountContract {
                source: &policy.state_dir,
                destination: &policy.state_dir,
                writable: false,
            },
        ],
        "broker",
    )?;
    Ok(())
}

async fn create_session(policy: &Policy, session_id: &str) -> Result<(), String> {
    validate_session_paths(policy, session_id)?;
    let agent = agent_name(session_id);
    let relay = relay_name(session_id);
    let capture = capture_name(session_id);
    if inspect_optional(&agent, policy, session_id, Component::Agent)
        .await?
        .is_some()
        || inspect_optional(&relay, policy, session_id, Component::SessionRelay)
            .await?
            .is_some()
        || inspect_optional(&capture, policy, session_id, Component::SessionCapture)
            .await?
            .is_some()
    {
        return Err(format!(
            "session container collision: {agent}, {relay}, or {capture} already exists; explicit removal is required before recreation"
        ));
    }
    let session_root = Path::new(&policy.state_dir)
        .join("sessions")
        .join(session_id);
    let mount = |leaf: &str, destination: &str, readonly: bool| {
        format!(
            "type=bind,src={},dst={destination}{}",
            session_root.join(leaf).display(),
            if readonly { ",readonly" } else { "" }
        )
    };
    let agent_args = vec![
        "run".into(),
        "-d".into(),
        "--name".into(),
        agent.clone(),
        "--label".into(),
        format!("agent_service.session={session_id}"),
        "--label".into(),
        format!("agent_service.profile={}", policy.profile),
        "--label".into(),
        format!("agent_service.component={}", Component::Agent.label()),
        "--network".into(),
        "none".into(),
        "--restart".into(),
        "no".into(),
        "--user".into(),
        "1000:1000".into(),
        "--workdir".into(),
        "/workspace".into(),
        "--read-only".into(),
        "--tmpfs".into(),
        format!("/tmp:{}", policy.agent.tmpfs_tmp),
        "--tmpfs".into(),
        format!("/qwen-runtime:{}", policy.agent.tmpfs_qwen_runtime),
        "--cap-drop".into(),
        "ALL".into(),
        "--security-opt".into(),
        "no-new-privileges:true".into(),
        "--memory".into(),
        policy.agent.memory.clone(),
        "--memory-swap".into(),
        policy.agent.memory_swap.clone(),
        "--pids-limit".into(),
        policy.agent.pids_limit.to_string(),
        "--mount".into(),
        mount("staged", "/workspace", false),
        "--mount".into(),
        mount("artifacts", "/artifacts", false),
        "--mount".into(),
        mount("control", "/run/agent", true),
        "--mount".into(),
        mount("streams", "/streams", true),
        policy.agent.image_id.clone(),
    ];
    docker_os(agent_args, "create_agent", Some(DOCKER_TIMEOUT)).await?;
    if let Err(error) = verify_agent(policy, session_id).await {
        let cleanup = remove_session(policy, session_id).await;
        return Err(format!(
            "agent verification failed: {error}; cleanup={cleanup:?}"
        ));
    }
    let agent_id = inspect_single(&agent, "agent_id")
        .await?
        .get("Id")
        .and_then(Value::as_str)
        .ok_or_else(|| "agent inspect lacks Id".to_string())?
        .to_string();
    let capture_args = vec![
        "run".into(),
        "-d".into(),
        "--name".into(),
        capture.clone(),
        "--label".into(),
        format!("agent_service.session={session_id}"),
        "--label".into(),
        format!("agent_service.profile={}", policy.profile),
        "--label".into(),
        format!(
            "agent_service.component={}",
            Component::SessionCapture.label()
        ),
        "--network".into(),
        format!("container:{agent_id}"),
        "--restart".into(),
        "no".into(),
        "--user".into(),
        "1000:1000".into(),
        "--read-only".into(),
        "--cap-drop".into(),
        "ALL".into(),
        "--security-opt".into(),
        "no-new-privileges:true".into(),
        "--memory".into(),
        policy.capture.memory.clone(),
        "--memory-swap".into(),
        policy.capture.memory_swap.clone(),
        "--pids-limit".into(),
        policy.capture.pids_limit.to_string(),
        "--mount".into(),
        mount("streams", "/streams", false),
        "--mount".into(),
        mount("output", "/output", false),
        policy.capture.image_id.clone(),
    ];
    docker_os(capture_args, "create_session_capture", Some(DOCKER_TIMEOUT)).await?;
    if let Err(error) = verify_capture(policy, session_id, &agent_id).await {
        let cleanup = remove_session(policy, session_id).await;
        return Err(format!(
            "session capture verification failed: {error}; cleanup={cleanup:?}"
        ));
    }
    if let Err(error) = wait_for_log_event(
        &capture,
        &policy.capture.ready_event,
        Duration::from_secs(30),
        true,
    )
    .await
    {
        let cleanup = remove_session(policy, session_id).await;
        return Err(format!(
            "session capture readiness failed: {error}; cleanup={cleanup:?}"
        ));
    }
    let relay_args = vec![
        "run".into(),
        "-d".into(),
        "--name".into(),
        relay.clone(),
        "--label".into(),
        format!("agent_service.session={session_id}"),
        "--label".into(),
        format!("agent_service.profile={}", policy.profile),
        "--label".into(),
        format!(
            "agent_service.component={}",
            Component::SessionRelay.label()
        ),
        "--network".into(),
        format!("container:{agent_id}"),
        "--restart".into(),
        "no".into(),
        "--user".into(),
        "1000:1000".into(),
        "--read-only".into(),
        "--cap-drop".into(),
        "ALL".into(),
        "--security-opt".into(),
        "no-new-privileges:true".into(),
        "--memory".into(),
        policy.relay.memory.clone(),
        "--memory-swap".into(),
        policy.relay.memory_swap.clone(),
        "--pids-limit".into(),
        policy.relay.pids_limit.to_string(),
        "--mount".into(),
        format!(
            "type=bind,src={},dst=/sock,readonly",
            policy.model_socket_dir
        ),
        policy.relay.image_id.clone(),
        policy.relay.role.clone(),
    ];
    if let Err(error) = docker_os(relay_args, "create_session_relay", Some(DOCKER_TIMEOUT)).await {
        let cleanup = remove_session(policy, session_id).await;
        return Err(format!(
            "session relay start failed: {error}; cleanup={cleanup:?}"
        ));
    }
    if let Err(error) = verify_relay(policy, session_id, &agent_id).await {
        let cleanup = remove_session(policy, session_id).await;
        return Err(format!(
            "session relay verification failed: {error}; cleanup={cleanup:?}"
        ));
    }
    if let Err(error) = wait_for_relay_ready(&relay, &policy.relay.role).await {
        let cleanup = remove_session(policy, session_id).await;
        return Err(format!(
            "session relay readiness failed: {error}; cleanup={cleanup:?}"
        ));
    }
    Ok(())
}

async fn wait_for_relay_ready(name: &str, role: &str) -> Result<(), String> {
    let expected = format!(
        "RELAY_READY role={role} sandbox=landlock-net-v4+seccomp-socket-v2 \
         listen=tcp:127.0.0.1:18000 target=unix:/sock/relay.sock"
    );
    wait_for_log_match(name, &expected, Some(Duration::from_secs(30)), true)
        .await
        .map(|_| ())
}

async fn wait_for_log_event(
    name: &str,
    expected: &str,
    timeout: Duration,
    exact: bool,
) -> Result<String, String> {
    wait_for_log_match(name, expected, Some(timeout), exact).await
}

async fn wait_for_log_match(
    name: &str,
    expected: &str,
    timeout: Option<Duration>,
    exact: bool,
) -> Result<String, String> {
    let mut child = Command::new(DOCKER)
        .args(["logs", "--follow", "--since", "0s", name])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| format!("spawn event-driven log wait for {name}: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("log wait for {name} has no stdout pipe"))?;
    let wait = async {
        let mut lines = BufReader::new(stdout).lines();
        while let Some(line) = lines
            .next_line()
            .await
            .map_err(|error| format!("read log event for {name}: {error}"))?
        {
            if (exact && line == expected) || (!exact && line.starts_with(expected)) {
                return Ok(line);
            }
        }
        Err(format!(
            "container {name} log stream ended before required event {expected:?}"
        ))
    };
    let result = match timeout {
        Some(timeout) => match tokio::time::timeout(timeout, wait).await {
            Ok(value) => value,
            Err(_) => Err(format!(
                "container {name} did not emit {expected:?} within {timeout:?}"
            )),
        },
        None => wait.await,
    };
    let status = match child
        .try_wait()
        .map_err(|error| format!("inspect log follower for {name}: {error}"))?
    {
        Some(status) => status,
        None => {
            child
                .kill()
                .await
                .map_err(|error| format!("stop log follower for {name}: {error}"))?;
            child
                .wait()
                .await
                .map_err(|error| format!("reap completed log follower for {name}: {error}"))?
        }
    };
    if !status.success() && status.code().is_some() {
        // Docker logs --follow is deliberately terminated after the exact
        // event; a signal/no-code status is expected, a normal nonzero exit is
        // evidence that the follower itself failed.
        return Err(format!(
            "log follower for {name} exited unexpectedly with {status}"
        ));
    }
    result
}

fn parse_agent_ready(line: &str, policy: &Policy) -> Result<u64, String> {
    let remainder = line
        .strip_prefix(&policy.agent.ready_event_prefix)
        .ok_or_else(|| format!("agent readiness record has the wrong prefix: {line:?}"))?;
    let (token_count, sandbox) = remainder
        .split_once(" sandbox=")
        .ok_or_else(|| format!("agent readiness record lacks exact sandbox field: {line:?}"))?;
    if sandbox != policy.agent.sandbox || token_count.is_empty() {
        return Err(format!(
            "agent readiness record drift: token_count={token_count:?} sandbox={sandbox:?}"
        ));
    }
    let token_count = token_count
        .parse::<u64>()
        .map_err(|error| format!("agent readiness token count is invalid: {error}"))?;
    if token_count == 0 {
        return Err("agent readiness token count must be positive".into());
    }
    Ok(token_count)
}

fn parse_capture_complete(line: &str, policy: &Policy) -> Result<(u64, u64), String> {
    let remainder = line
        .strip_prefix(&policy.capture.complete_event_prefix)
        .ok_or_else(|| format!("capture completion record has the wrong prefix: {line:?}"))?;
    let (events_bytes, stderr_bytes) = remainder
        .split_once(" stderr_bytes=")
        .ok_or_else(|| format!("capture completion record lacks exact byte fields: {line:?}"))?;
    if events_bytes.is_empty()
        || stderr_bytes.is_empty()
        || stderr_bytes.contains(char::is_whitespace)
    {
        return Err(format!(
            "capture completion record has extra or empty fields: {line:?}"
        ));
    }
    Ok((
        events_bytes
            .parse::<u64>()
            .map_err(|error| format!("capture events byte count is invalid: {error}"))?,
        stderr_bytes
            .parse::<u64>()
            .map_err(|error| format!("capture stderr byte count is invalid: {error}"))?,
    ))
}

fn validate_session_paths(policy: &Policy, session_id: &str) -> Result<(), String> {
    let root = Path::new(&policy.state_dir)
        .join("sessions")
        .join(session_id);
    for (leaf, mode) in [
        ("staged", 0o755),
        ("artifacts", 0o700),
        ("control", 0o755),
        ("streams", 0o700),
        ("output", 0o700),
    ] {
        let path = root.join(leaf);
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| format!("cannot stat session path {}: {error}", path.display()))?;
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || metadata.uid() != 1000
            || metadata.permissions().mode() & 0o777 != mode
        {
            return Err(format!(
                "session path contract drift at {}: uid={} mode={:o} type={:?}",
                path.display(),
                metadata.uid(),
                metadata.permissions().mode() & 0o777,
                metadata.file_type()
            ));
        }
        let canonical = std::fs::canonicalize(&path)
            .map_err(|error| format!("canonicalize {}: {error}", path.display()))?;
        if canonical != path {
            return Err(format!(
                "session path canonicalization drift: expected {}, observed {}",
                path.display(),
                canonical.display()
            ));
        }
    }
    for (leaf, mode) in [("prompt.txt", 0o644), ("start-gate.lock", 0o600)] {
        let path = root.join("control").join(leaf);
        let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
            format!(
                "cannot stat session control file {}: {error}",
                path.display()
            )
        })?;
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.uid() != 1000
            || metadata.gid() != 1000
            || metadata.permissions().mode() & 0o777 != mode
        {
            return Err(format!(
                "session control file contract drift at {}: uid={} gid={} mode={:o} type={:?}",
                path.display(),
                metadata.uid(),
                metadata.gid(),
                metadata.permissions().mode() & 0o777,
                metadata.file_type()
            ));
        }
    }
    Ok(())
}

async fn verify_agent(policy: &Policy, session_id: &str) -> Result<(), String> {
    let value = require_owned(
        &agent_name(session_id),
        policy,
        session_id,
        Component::Agent,
    )
    .await?;
    require_bool(&value, "/State/Running", true, "agent running state")?;
    require_string(&value, "/Image", &policy.agent.image_id, "agent image ID")?;
    require_string(
        &value,
        "/Config/Image",
        &policy.agent.image_id,
        "agent configured image ID",
    )?;
    require_string(&value, "/Config/User", "1000:1000", "agent user")?;
    require_string(
        &value,
        "/Config/WorkingDir",
        "/workspace",
        "agent working directory",
    )?;
    require_exact_command(
        &value,
        "/Config/Entrypoint",
        &["/opt/agent/run_agent.sh"],
        "agent entrypoint",
    )?;
    require_null_or_empty_array(&value, "/Config/Cmd", "agent command")?;
    require_string(&value, "/HostConfig/NetworkMode", "none", "agent network")?;
    require_bool(&value, "/HostConfig/ReadonlyRootfs", true, "agent root")?;
    require_u64(
        &value,
        "/HostConfig/Memory",
        parse_memory(&policy.agent.memory)?,
        "agent memory limit",
    )?;
    require_u64(
        &value,
        "/HostConfig/MemorySwap",
        parse_memory(&policy.agent.memory_swap)?,
        "agent memory+swap limit",
    )?;
    require_u64(
        &value,
        "/HostConfig/PidsLimit",
        u64::from(policy.agent.pids_limit),
        "agent PID limit",
    )?;
    require_hardening(&value, "agent")?;
    require_empty_object(&value, "/HostConfig/PortBindings", "agent port bindings")?;
    let expected_tmpfs = std::collections::BTreeMap::from([
        ("/qwen-runtime", policy.agent.tmpfs_qwen_runtime.as_str()),
        ("/tmp", policy.agent.tmpfs_tmp.as_str()),
    ]);
    let observed_tmpfs = value
        .pointer("/HostConfig/Tmpfs")
        .and_then(Value::as_object)
        .ok_or_else(|| "agent inspect lacks tmpfs map".to_string())?
        .iter()
        .map(|(key, value)| {
            value
                .as_str()
                .map(|value| (key.as_str(), value))
                .ok_or_else(|| format!("agent tmpfs option for {key} is not a string"))
        })
        .collect::<Result<std::collections::BTreeMap<_, _>, _>>()?;
    if observed_tmpfs != expected_tmpfs {
        return Err(format!(
            "agent tmpfs drift: expected {expected_tmpfs:?}, observed {observed_tmpfs:?}"
        ));
    }
    let session_root = Path::new(&policy.state_dir)
        .join("sessions")
        .join(session_id);
    // Keep owned strings alive while the borrowed mount contracts are checked.
    let staged = session_root.join("staged").to_string_lossy().into_owned();
    let artifacts = session_root
        .join("artifacts")
        .to_string_lossy()
        .into_owned();
    let control = session_root.join("control").to_string_lossy().into_owned();
    let streams = session_root.join("streams").to_string_lossy().into_owned();
    require_exact_mounts(
        &value,
        &[
            MountContract {
                source: &staged,
                destination: "/workspace",
                writable: true,
            },
            MountContract {
                source: &artifacts,
                destination: "/artifacts",
                writable: true,
            },
            MountContract {
                source: &control,
                destination: "/run/agent",
                writable: false,
            },
            MountContract {
                source: &streams,
                destination: "/streams",
                writable: false,
            },
        ],
        "agent",
    )?;
    let routes = docker(
        [
            "exec",
            agent_name(session_id).as_str(),
            "ip",
            "-4",
            "route",
            "show",
        ],
        "agent_routes",
        Some(DOCKER_TIMEOUT),
    )
    .await?;
    if !routes.trim().is_empty() {
        return Err(format!("agent has forbidden IPv4 routes: {routes:?}"));
    }
    let routes_v6 = docker(
        [
            "exec",
            agent_name(session_id).as_str(),
            "ip",
            "-6",
            "route",
            "show",
        ],
        "agent_ipv6_routes",
        Some(DOCKER_TIMEOUT),
    )
    .await?;
    if !routes_v6.trim().is_empty() {
        return Err(format!("agent has forbidden IPv6 routes: {routes_v6:?}"));
    }
    let links = docker(
        [
            "exec",
            agent_name(session_id).as_str(),
            "ip",
            "-j",
            "link",
            "show",
        ],
        "agent_links",
        Some(DOCKER_TIMEOUT),
    )
    .await?;
    let links: Value = serde_json::from_str(&links)
        .map_err(|error| format!("agent ip -j link output is invalid JSON: {error}"))?;
    let links = links
        .as_array()
        .filter(|links| links.len() == 1)
        .ok_or_else(|| format!("agent must expose exactly one loopback link: {links}"))?;
    let link = &links[0];
    require_string(link, "/ifname", "lo", "agent link name")?;
    require_string(link, "/link_type", "loopback", "agent link type")?;
    require_string(
        link,
        "/address",
        "00:00:00:00:00:00",
        "agent loopback address",
    )?;
    require_u64(link, "/mtu", 65_536, "agent loopback MTU")?;
    if link.get("flags") != Some(&json!(["LOOPBACK", "UP", "LOWER_UP"])) {
        return Err(format!(
            "agent loopback flags drift: {:?}",
            link.get("flags")
        ));
    }
    Ok(())
}

async fn verify_capture(policy: &Policy, session_id: &str, agent_id: &str) -> Result<(), String> {
    let value = require_owned(
        &capture_name(session_id),
        policy,
        session_id,
        Component::SessionCapture,
    )
    .await?;
    require_bool(
        &value,
        "/State/Running",
        true,
        "session capture running state",
    )?;
    require_string(
        &value,
        "/Image",
        &policy.capture.image_id,
        "capture image ID",
    )?;
    require_string(
        &value,
        "/Config/Image",
        &policy.capture.image_id,
        "capture configured image ID",
    )?;
    require_string(&value, "/Config/User", "1000:1000", "capture user")?;
    require_exact_command(
        &value,
        "/Config/Entrypoint",
        &["/session_capture"],
        "capture entrypoint",
    )?;
    require_null_or_empty_array(&value, "/Config/Cmd", "capture command")?;
    require_string(
        &value,
        "/HostConfig/NetworkMode",
        &format!("container:{agent_id}"),
        "capture network namespace",
    )?;
    require_bool(&value, "/HostConfig/ReadonlyRootfs", true, "capture root")?;
    require_u64(
        &value,
        "/HostConfig/Memory",
        parse_memory(&policy.capture.memory)?,
        "capture memory limit",
    )?;
    require_u64(
        &value,
        "/HostConfig/MemorySwap",
        parse_memory(&policy.capture.memory_swap)?,
        "capture memory+swap limit",
    )?;
    require_u64(
        &value,
        "/HostConfig/PidsLimit",
        u64::from(policy.capture.pids_limit),
        "capture PID limit",
    )?;
    require_hardening(&value, "session capture")?;
    require_string(
        &value,
        "/Config/Labels/agent_service.capture.id",
        &policy.capture.capture_id,
        "capture implementation label",
    )?;
    require_empty_object(
        &value,
        "/HostConfig/PortBindings",
        "session capture port bindings",
    )?;
    let session_root = Path::new(&policy.state_dir)
        .join("sessions")
        .join(session_id);
    let streams = session_root.join("streams").to_string_lossy().into_owned();
    let output = session_root.join("output").to_string_lossy().into_owned();
    require_exact_mounts(
        &value,
        &[
            MountContract {
                source: &streams,
                destination: "/streams",
                writable: true,
            },
            MountContract {
                source: &output,
                destination: "/output",
                writable: true,
            },
        ],
        "session capture",
    )
}

async fn verify_relay(policy: &Policy, session_id: &str, agent_id: &str) -> Result<(), String> {
    let value = require_owned(
        &relay_name(session_id),
        policy,
        session_id,
        Component::SessionRelay,
    )
    .await?;
    require_bool(
        &value,
        "/State/Running",
        true,
        "session relay running state",
    )?;
    require_string(&value, "/Image", &policy.relay.image_id, "relay image ID")?;
    require_string(
        &value,
        "/Config/Image",
        &policy.relay.image_id,
        "relay configured image ID",
    )?;
    require_string(&value, "/Config/User", "1000:1000", "relay user")?;
    require_exact_command(
        &value,
        "/Config/Entrypoint",
        &["/fixed_relay"],
        "relay entrypoint",
    )?;
    require_exact_command(&value, "/Config/Cmd", &["agent-model"], "relay command")?;
    require_string(
        &value,
        "/HostConfig/NetworkMode",
        &format!("container:{agent_id}"),
        "relay network namespace",
    )?;
    require_bool(&value, "/HostConfig/ReadonlyRootfs", true, "relay root")?;
    require_u64(
        &value,
        "/HostConfig/Memory",
        parse_memory(&policy.relay.memory)?,
        "relay memory limit",
    )?;
    require_u64(
        &value,
        "/HostConfig/MemorySwap",
        parse_memory(&policy.relay.memory_swap)?,
        "relay memory+swap limit",
    )?;
    require_u64(
        &value,
        "/HostConfig/PidsLimit",
        u64::from(policy.relay.pids_limit),
        "relay PID limit",
    )?;
    require_hardening(&value, "session relay")?;
    require_string(
        &value,
        "/Config/Labels/agent_service.relay.sandbox",
        &policy.relay.sandbox,
        "relay kernel sandbox label",
    )?;
    require_empty_object(
        &value,
        "/HostConfig/PortBindings",
        "session relay port bindings",
    )?;
    require_exact_mounts(
        &value,
        &[MountContract {
            source: &policy.model_socket_dir,
            destination: "/sock",
            writable: false,
        }],
        "session relay",
    )
}

async fn require_owned(
    name: &str,
    policy: &Policy,
    session_id: &str,
    component: Component,
) -> Result<Value, String> {
    inspect_optional(name, policy, session_id, component)
        .await?
        .ok_or_else(|| format!("required owned container is absent: {name}"))
}

async fn inspect_optional(
    name: &str,
    policy: &Policy,
    session_id: &str,
    component: Component,
) -> Result<Option<Value>, String> {
    let result = docker_status(["inspect", name], "inspect_owned", Some(DOCKER_TIMEOUT)).await?;
    if result.code == 1
        && result
            .stderr
            .to_ascii_lowercase()
            .contains("no such object")
    {
        return Ok(None);
    }
    if result.code != 0 {
        return Err(format!(
            "inspect {name} failed with {}: {}",
            result.code,
            truncate(&result.stderr, 4096)
        ));
    }
    let values: Vec<Value> = serde_json::from_str(&result.stdout)
        .map_err(|error| format!("inspect {name} returned invalid JSON: {error}"))?;
    if values.len() != 1 {
        return Err(format!("inspect {name} returned {} objects", values.len()));
    }
    let Some(value) = values.into_iter().next() else {
        return Err(format!(
            "inspect {name} returned no object after cardinality validation"
        ));
    };
    for (pointer, expected, label) in [
        (
            "/Config/Labels/agent_service.session",
            session_id,
            "session label",
        ),
        (
            "/Config/Labels/agent_service.profile",
            policy.profile.as_str(),
            "profile label",
        ),
        (
            "/Config/Labels/agent_service.component",
            component.label(),
            "component label",
        ),
    ] {
        require_string(&value, pointer, expected, label)?;
    }
    Ok(Some(value))
}

fn optional_running(value: Option<&Value>, label: &str) -> Result<bool, String> {
    match value {
        None => Ok(false),
        Some(value) => value
            .pointer("/State/Running")
            .and_then(Value::as_bool)
            .ok_or_else(|| format!("{label} inspect lacks boolean State.Running")),
    }
}

async fn remove_session(policy: &Policy, session_id: &str) -> Result<(), String> {
    let agent = agent_name(session_id);
    let relay = relay_name(session_id);
    let capture = capture_name(session_id);
    let mut diagnostics = Vec::new();

    // Teardown is ordered but not short-circuiting. One failed component must
    // not silently leave the other running merely because `?` returned early.
    // Every helper still independently proves exact ownership before acting.
    if let Err(error) = stop_owned_if_running(&agent, policy, session_id, Component::Agent).await {
        diagnostics.push(format!("stop exact agent {agent}: {error}"));
    }
    if let Err(error) =
        stop_owned_if_running(&relay, policy, session_id, Component::SessionRelay).await
    {
        diagnostics.push(format!("stop exact session relay {relay}: {error}"));
    }
    if let Err(error) =
        stop_owned_if_running(&capture, policy, session_id, Component::SessionCapture).await
    {
        diagnostics.push(format!("stop exact session capture {capture}: {error}"));
    }
    if let Err(error) =
        remove_stopped_owned(&relay, policy, session_id, Component::SessionRelay).await
    {
        diagnostics.push(format!("remove exact session relay {relay}: {error}"));
    }
    if let Err(error) =
        remove_stopped_owned(&capture, policy, session_id, Component::SessionCapture).await
    {
        diagnostics.push(format!("remove exact session capture {capture}: {error}"));
    }
    if let Err(error) = remove_stopped_owned(&agent, policy, session_id, Component::Agent).await {
        diagnostics.push(format!("remove exact agent {agent}: {error}"));
    }
    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "session teardown was incomplete: {}",
            diagnostics.join("; ")
        ))
    }
}

async fn stop_owned_if_running(
    name: &str,
    policy: &Policy,
    session_id: &str,
    component: Component,
) -> Result<(), String> {
    let Some(value) = inspect_optional(name, policy, session_id, component).await? else {
        return Ok(());
    };
    if value.pointer("/State/Running").and_then(Value::as_bool) == Some(true) {
        docker(["stop", "--timeout", "-1", name], "stop_owned", None).await?;
    }
    let stopped = require_owned(name, policy, session_id, component).await?;
    require_bool(
        &stopped,
        "/State/Running",
        false,
        "owned container stopped state",
    )
}

async fn remove_stopped_owned(
    name: &str,
    policy: &Policy,
    session_id: &str,
    component: Component,
) -> Result<(), String> {
    let Some(value) = inspect_optional(name, policy, session_id, component).await? else {
        return Ok(());
    };
    require_bool(
        &value,
        "/State/Running",
        false,
        "owned container stopped state before removal",
    )?;
    docker(
        ["container", "rm", name],
        "remove_owned",
        Some(DOCKER_TIMEOUT),
    )
    .await
    .map(|_| ())
}

async fn sweep_orphans(policy: &Policy) -> Result<(), String> {
    let profile_filter = format!("label=agent_service.profile={}", policy.profile);
    let ids = docker(
        [
            "ps",
            "-aq",
            "--filter",
            "label=agent_service.session",
            "--filter",
            profile_filter.as_str(),
        ],
        "list_orphans",
        Some(DOCKER_TIMEOUT),
    )
    .await?;
    let mut relays = Vec::new();
    let mut agents = Vec::new();
    let mut captures = Vec::new();
    for id in ids.lines().map(str::trim).filter(|value| !value.is_empty()) {
        let value = inspect_single(id, "inspect_orphan").await?;
        let session_id = value
            .pointer("/Config/Labels/agent_service.session")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("orphan {id} lacks session label"))?;
        validate_session_id(session_id)?;
        require_string(
            &value,
            "/Config/Labels/agent_service.profile",
            &policy.profile,
            "orphan profile label",
        )?;
        let component = value
            .pointer("/Config/Labels/agent_service.component")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("orphan {id} lacks component label"))?;
        let observed_name = value
            .get("Name")
            .and_then(Value::as_str)
            .and_then(|name| name.strip_prefix('/'))
            .ok_or_else(|| format!("orphan {id} lacks canonical container name"))?;
        let (expected_name, expected_image, target) = if component == Component::Agent.label() {
            (
                agent_name(session_id),
                policy.agent.image_id.as_str(),
                &mut agents,
            )
        } else if component == Component::SessionRelay.label() {
            (
                relay_name(session_id),
                policy.relay.image_id.as_str(),
                &mut relays,
            )
        } else if component == Component::SessionCapture.label() {
            (
                capture_name(session_id),
                policy.capture.image_id.as_str(),
                &mut captures,
            )
        } else {
            return Err(format!(
                "refusing orphan sweep with unrecognized owned component {component:?} on {id}"
            ));
        };
        if observed_name != expected_name {
            return Err(format!(
                "refusing orphan sweep for name drift on {id}: expected {expected_name:?}, observed {observed_name:?}"
            ));
        }
        require_string(&value, "/Image", expected_image, "orphan image ID")?;
        require_string(
            &value,
            "/Config/Image",
            expected_image,
            "orphan configured image ID",
        )?;
        target.push(id.to_string());
    }

    // The agent owns the network-none namespace shared by the capture and
    // relay. Stop the stream producer first, then both sidecars. Remove both
    // namespace dependants before removing the agent namespace owner.
    for id in &agents {
        stop_orphan_if_running(id, "orphan agent").await?;
    }
    for id in &relays {
        stop_orphan_if_running(id, "orphan session relay").await?;
    }
    for id in &captures {
        stop_orphan_if_running(id, "orphan session capture").await?;
    }
    for id in relays.iter().chain(captures.iter()).chain(agents.iter()) {
        docker(
            ["container", "rm", id.as_str()],
            "sweep_orphan",
            Some(DOCKER_TIMEOUT),
        )
        .await?;
    }
    Ok(())
}

async fn stop_orphan_if_running(id: &str, label: &str) -> Result<(), String> {
    let value = inspect_single(id, label).await?;
    if value.pointer("/State/Running").and_then(Value::as_bool) == Some(true) {
        docker(["stop", "--timeout", "-1", id], "stop_orphan", None).await?;
    }
    let stopped = inspect_single(id, label).await?;
    require_bool(
        &stopped,
        "/State/Running",
        false,
        &format!("{label} stopped state before removal"),
    )
}

fn agent_name(session_id: &str) -> String {
    format!("agent-{session_id}")
}

fn relay_name(session_id: &str) -> String {
    format!("agent-model-{session_id}")
}

fn capture_name(session_id: &str) -> String {
    format!("agent-capture-{session_id}")
}

fn require_string(value: &Value, pointer: &str, expected: &str, label: &str) -> Result<(), String> {
    let observed = value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or("<missing>");
    if observed == expected {
        Ok(())
    } else {
        Err(format!(
            "{label} drift at {pointer}: expected {expected:?}, observed {observed:?}"
        ))
    }
}

fn require_bool(value: &Value, pointer: &str, expected: bool, label: &str) -> Result<(), String> {
    let observed = value.pointer(pointer).and_then(Value::as_bool);
    if observed == Some(expected) {
        Ok(())
    } else {
        Err(format!(
            "{label} drift at {pointer}: expected {expected}, observed {observed:?}"
        ))
    }
}

fn require_u64(value: &Value, pointer: &str, expected: u64, label: &str) -> Result<(), String> {
    let observed = value.pointer(pointer).and_then(Value::as_u64);
    if observed == Some(expected) {
        Ok(())
    } else {
        Err(format!(
            "{label} drift at {pointer}: expected {expected}, observed {observed:?}"
        ))
    }
}

fn parse_memory(value: &str) -> Result<u64, String> {
    let (number, multiplier) = if let Some(number) = value.strip_suffix('g') {
        (number, 1024_u64.pow(3))
    } else if let Some(number) = value.strip_suffix('m') {
        (number, 1024_u64.pow(2))
    } else {
        return Err(format!("unsupported compiled memory syntax: {value:?}"));
    };
    number
        .parse::<u64>()
        .ok()
        .and_then(|number| number.checked_mul(multiplier))
        .ok_or_else(|| format!("invalid compiled memory value: {value:?}"))
}

fn require_exact_command(
    value: &Value,
    pointer: &str,
    expected: &[&str],
    label: &str,
) -> Result<(), String> {
    let observed = value
        .pointer(pointer)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{label} is missing or not an array at {pointer}"))?;
    let observed = observed
        .iter()
        .map(|item| {
            item.as_str()
                .ok_or_else(|| format!("{label} contains a non-string value: {item}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if observed == expected {
        Ok(())
    } else {
        Err(format!(
            "{label} drift at {pointer}: expected {expected:?}, observed {observed:?}"
        ))
    }
}

fn require_null_or_empty_array(value: &Value, pointer: &str, label: &str) -> Result<(), String> {
    match value.pointer(pointer) {
        Some(Value::Null) => Ok(()),
        Some(Value::Array(items)) if items.is_empty() => Ok(()),
        observed => Err(format!(
            "{label} must be null or an empty array at {pointer}, observed {observed:?}"
        )),
    }
}

fn require_empty_object(value: &Value, pointer: &str, label: &str) -> Result<(), String> {
    match value.pointer(pointer) {
        Some(Value::Object(items)) if items.is_empty() => Ok(()),
        observed => Err(format!(
            "{label} must be an empty object at {pointer}, observed {observed:?}"
        )),
    }
}

fn require_hardening(value: &Value, label: &str) -> Result<(), String> {
    require_string(
        value,
        "/AppArmorProfile",
        "docker-default",
        &format!("{label} AppArmor profile"),
    )?;
    require_bool(
        value,
        "/HostConfig/Privileged",
        false,
        &format!("{label} privileged flag"),
    )?;
    let cap_drop = value
        .pointer("/HostConfig/CapDrop")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{label} inspect lacks CapDrop"))?;
    if cap_drop != &[Value::String("ALL".into())] {
        return Err(format!("{label} CapDrop drift: {cap_drop:?}"));
    }
    require_null_or_empty_array(value, "/HostConfig/CapAdd", &format!("{label} CapAdd"))?;
    require_null_or_empty_array(value, "/HostConfig/Devices", &format!("{label} devices"))?;
    require_null_or_empty_array(
        value,
        "/HostConfig/DeviceRequests",
        &format!("{label} device requests"),
    )?;
    require_string(
        value,
        "/HostConfig/RestartPolicy/Name",
        "no",
        &format!("{label} restart policy"),
    )?;
    let security = value
        .pointer("/HostConfig/SecurityOpt")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{label} inspect lacks SecurityOpt"))?;
    if security.len() != 1
        || !matches!(
            security[0].as_str(),
            Some("no-new-privileges:true" | "no-new-privileges=true")
        )
    {
        return Err(format!(
            "{label} security options must contain only no-new-privileges: {security:?}"
        ));
    }
    for (pointer, expected, property) in [
        ("/HostConfig/PidMode", "", "PID namespace"),
        ("/HostConfig/IpcMode", "private", "IPC namespace"),
        ("/HostConfig/UTSMode", "", "UTS namespace"),
    ] {
        require_string(value, pointer, expected, &format!("{label} {property}"))?;
    }
    Ok(())
}

struct MountContract<'a> {
    source: &'a str,
    destination: &'a str,
    writable: bool,
}

fn require_exact_mounts(
    value: &Value,
    expected: &[MountContract<'_>],
    label: &str,
) -> Result<(), String> {
    let mounts = value
        .get("Mounts")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{label} inspect lacks Mounts"))?;
    if mounts.len() != expected.len() {
        return Err(format!(
            "{label} mount count drift: expected {}, observed {}: {mounts:?}",
            expected.len(),
            mounts.len()
        ));
    }
    let expected_by_destination = expected
        .iter()
        .map(|contract| (contract.destination, contract))
        .collect::<std::collections::BTreeMap<_, _>>();
    if expected_by_destination.len() != expected.len() {
        return Err(format!(
            "{label} compiled mount contract has duplicate destinations"
        ));
    }
    let mut observed_destinations = std::collections::BTreeSet::new();
    for mount in mounts {
        let destination = mount
            .get("Destination")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{label} mount lacks Destination: {mount}"))?;
        if !observed_destinations.insert(destination) {
            return Err(format!(
                "{label} has duplicate mount destination {destination:?}"
            ));
        }
        let contract = expected_by_destination.get(destination).ok_or_else(|| {
            format!("{label} has unexpected mount destination {destination:?}: {mount}")
        })?;
        let source = mount
            .get("Source")
            .and_then(Value::as_str)
            .unwrap_or("<missing>");
        let mount_type = mount
            .get("Type")
            .and_then(Value::as_str)
            .unwrap_or("<missing>");
        let writable = mount.get("RW").and_then(Value::as_bool);
        let propagation = mount
            .get("Propagation")
            .and_then(Value::as_str)
            .unwrap_or("<missing>");
        if source != contract.source
            || mount_type != "bind"
            || writable != Some(contract.writable)
            || propagation != "rprivate"
        {
            return Err(format!(
                "{label} mount drift at {destination}: expected source={:?} type=bind writable={} propagation=rprivate; observed {mount}",
                contract.source, contract.writable
            ));
        }
    }
    Ok(())
}

async fn inspect_single(object: &str, label: &str) -> Result<Value, String> {
    let value = inspect_json(object, label).await?;
    let values = value
        .as_array()
        .filter(|values| values.len() == 1)
        .cloned()
        .ok_or_else(|| format!("{label} inspect must contain exactly one object"))?;
    values
        .into_iter()
        .next()
        .ok_or_else(|| format!("{label} inspect unexpectedly lost its sole object"))
}

async fn inspect_json(object: &str, label: &str) -> Result<Value, String> {
    let output = docker(
        ["inspect", object],
        &format!("inspect_{label}"),
        Some(DOCKER_TIMEOUT),
    )
    .await?;
    serde_json::from_str(&output).map_err(|error| format!("inspect {label} invalid JSON: {error}"))
}

#[derive(Debug)]
struct CommandResult {
    code: i32,
    stdout: String,
    stderr: String,
}

async fn docker<I, S>(args: I, label: &str, timeout: Option<Duration>) -> Result<String, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let result = docker_status(args, label, timeout).await?;
    if result.code != 0 {
        return Err(format!(
            "{label}: Docker exited {}; stderr={}; stdout={}",
            result.code,
            truncate(&result.stderr, 4096),
            truncate(&result.stdout, 1024)
        ));
    }
    Ok(result.stdout)
}

async fn docker_os(
    args: Vec<String>,
    label: &str,
    timeout: Option<Duration>,
) -> Result<String, String> {
    docker(args, label, timeout).await
}

async fn docker_status<I, S>(
    args: I,
    label: &str,
    timeout: Option<Duration>,
) -> Result<CommandResult, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let argv = args
        .into_iter()
        .map(|value| value.as_ref().to_os_string())
        .collect::<Vec<_>>();
    let mut child = Command::new(DOCKER)
        .args(&argv)
        .env_clear()
        .env("DOCKER_HOST", "unix:///var/run/docker.sock")
        .env("HOME", "/nonexistent")
        .env("LANG", "C.UTF-8")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("{label}: cannot spawn pinned Docker CLI: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("{label}: Docker stdout pipe is absent"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| format!("{label}: Docker stderr pipe is absent"))?;
    let collect = async {
        let (stdout, stderr, status) = tokio::join!(
            drain_bounded(stdout, OUTPUT_LIMIT),
            drain_bounded(stderr, OUTPUT_LIMIT),
            child.wait(),
        );
        Ok::<_, String>((
            stdout.map_err(|error| format!("{label}: read Docker stdout: {error}"))?,
            stderr.map_err(|error| format!("{label}: read Docker stderr: {error}"))?,
            status.map_err(|error| format!("{label}: wait for Docker CLI: {error}"))?,
        ))
    };
    let collected = match timeout {
        Some(duration) => match tokio::time::timeout(duration, collect).await {
            Ok(result) => result,
            Err(_) => {
                let kill_error = child.kill().await.err();
                let wait_error = child.wait().await.err();
                return Err(format!(
                    "{label}: Docker operation exceeded {duration:?}; kill_error={kill_error:?}; reap_error={wait_error:?}"
                ));
            }
        },
        None => collect.await,
    }?;
    let (stdout, stderr, status) = collected;
    if stdout.exceeded || stderr.exceeded {
        return Err(format!(
            "{label}: Docker output exceeded the per-stream {OUTPUT_LIMIT}-byte limit; stdout_prefix={}; stderr_prefix={}",
            truncate(&String::from_utf8_lossy(&stdout.bytes), 4096),
            truncate(&String::from_utf8_lossy(&stderr.bytes), 4096),
        ));
    }
    Ok(CommandResult {
        code: status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&stdout.bytes).into_owned(),
        stderr: String::from_utf8_lossy(&stderr.bytes).into_owned(),
    })
}

struct BoundedBytes {
    bytes: Vec<u8>,
    exceeded: bool,
}

/// Drain the complete pipe so the child can never deadlock on a full pipe,
/// but retain at most `limit` bytes. This bounds broker memory before—not
/// after—a hostile or degenerate container emits excessive logs.
async fn drain_bounded<R: AsyncRead + Unpin>(
    mut reader: R,
    limit: usize,
) -> std::io::Result<BoundedBytes> {
    let mut bytes = Vec::with_capacity(limit.min(64 * 1024));
    let mut exceeded = false;
    let mut chunk = [0u8; 16 * 1024];
    loop {
        let count = reader.read(&mut chunk).await?;
        if count == 0 {
            break;
        }
        let remaining = limit.saturating_sub(bytes.len());
        let retained = count.min(remaining);
        bytes.extend_from_slice(&chunk[..retained]);
        if retained != count {
            exceeded = true;
        }
    }
    Ok(BoundedBytes { bytes, exceeded })
}

fn truncate(value: &str, max: usize) -> String {
    let value = value.trim();
    if value.chars().count() <= max {
        value.to_string()
    } else {
        format!(
            "{}…(truncated)",
            value.chars().take(max).collect::<String>()
        )
    }
}

struct TerminationSignals {
    terminate: tokio::signal::unix::Signal,
    interrupt: tokio::signal::unix::Signal,
}

fn install_termination_signals() -> Result<TerminationSignals, String> {
    let terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|error| format!("install SIGTERM handler: {error}"))?;
    let interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .map_err(|error| format!("install SIGINT handler: {error}"))?;
    Ok(TerminationSignals {
        terminate,
        interrupt,
    })
}

async fn termination_signal(mut signals: TerminationSignals) -> Result<(), String> {
    tokio::select! {
        value = signals.terminate.recv() => value.ok_or_else(|| "SIGTERM stream closed".to_string()),
        value = signals.interrupt.recv() => value.ok_or_else(|| "SIGINT stream closed".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::AsyncWriteExt;

    use super::{
        drain_bounded, load_policy, optional_running, parse_agent_ready, parse_capture_complete,
        parse_request, validate_empty_ipv4_route_table, validate_empty_ipv6_route_table,
        validate_session_id, Request,
    };

    #[test]
    fn compiled_policy_is_exact() {
        load_policy().expect("compiled broker policy must pass exact validation");
    }

    #[test]
    fn procfs_route_tables_require_exactly_no_routes() {
        let header =
            "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n";
        validate_empty_ipv4_route_table(header, "fixture")
            .expect("a canonical header-only IPv4 table must pass");
        validate_empty_ipv6_route_table("", "fixture")
            .expect("an absent or empty IPv6 table must pass");

        let route = format!("{header}eth0\t00000000\t0100007F\t0003\t0\t0\t0\t00000000\t0\t0\t0\n");
        assert!(validate_empty_ipv4_route_table(&route, "fixture").is_err());
        assert!(validate_empty_ipv6_route_table(
            "00000000000000000000000000000000 00 00 00 00000000000000000000000000000000 01 00000000 00000000 00000001 lo\n",
            "fixture",
        )
        .is_err());
    }

    #[test]
    fn procfs_ipv4_route_proof_rejects_missing_or_drifted_headers() {
        assert!(validate_empty_ipv4_route_table("", "fixture").is_err());
        assert!(validate_empty_ipv4_route_table(
            "Iface Destination Gateway Flags RefCnt Use Metric Mask MTU Window\n",
            "fixture",
        )
        .is_err());
    }

    #[test]
    fn accepts_only_canonical_session_ids() {
        validate_session_id("s-0123456789abcdef0123456789abcdef")
            .expect("canonical session ID must pass");
        for invalid in [
            "0123456789abcdef0123456789abcdef",
            "s-0123",
            "s-0123456789ABCDEF0123456789ABCDEF",
            "s-../../var/run/docker.sock0000000",
        ] {
            assert!(
                validate_session_id(invalid).is_err(),
                "accepted {invalid:?}"
            );
        }
    }

    #[test]
    fn protocol_rejects_arbitrary_docker_arguments_and_multiple_records() {
        let arbitrary = br#"{"op":"run","args":["--privileged"]}\n"#;
        assert!(parse_request(arbitrary).is_err());
        let multiple = b"{\"op\":\"preflight\"}\n{\"op\":\"sweep_orphans\"}\n";
        assert!(parse_request(multiple).is_err());
        let unknown = b"{\"op\":\"preflight\",\"args\":[]}\n";
        assert!(parse_request(unknown).is_err());
        assert!(matches!(
            parse_request(b"{\"op\":\"preflight\"}\n").expect("preflight must parse"),
            Request::Preflight
        ));
        let ready = parse_request(
            b"{\"op\":\"wait_agent_ready\",\"session_id\":\"s-0123456789abcdef0123456789abcdef\"}\n",
        )
        .expect("the one typed readiness wait must parse");
        assert!(matches!(&ready, Request::WaitAgentReady { .. }));
        assert!(!ready.requires_mutation_lock());
        let capture = parse_request(
            b"{\"op\":\"wait_capture_complete\",\"session_id\":\"s-0123456789abcdef0123456789abcdef\"}\n",
        )
        .expect("the one typed capture-completion wait must parse");
        assert!(matches!(&capture, Request::WaitCaptureComplete { .. }));
        assert!(!capture.requires_mutation_lock());
        let quiescence = parse_request(
            b"{\"op\":\"prove_session_quiescent\",\"session_id\":\"s-0123456789abcdef0123456789abcdef\"}\n",
        )
        .expect("the exact quiescence proof must parse");
        assert!(matches!(&quiescence, Request::ProveSessionQuiescent { .. }));
        assert!(!quiescence.requires_mutation_lock());
        let create = parse_request(
            b"{\"op\":\"create_session\",\"session_id\":\"s-0123456789abcdef0123456789abcdef\"}\n",
        )
        .expect("create must parse");
        assert!(create.requires_mutation_lock());
    }

    #[test]
    fn dynamic_readiness_and_capture_records_are_exactly_parsed() {
        let policy = load_policy().expect("load exact policy");
        let ready = format!(
            "{}42 sandbox={}",
            policy.agent.ready_event_prefix, policy.agent.sandbox
        );
        assert_eq!(
            parse_agent_ready(&ready, &policy).expect("parse exact agent readiness"),
            42
        );
        for drift in [
            format!(
                "{}0 sandbox={}",
                policy.agent.ready_event_prefix, policy.agent.sandbox
            ),
            format!(
                "{}42 sandbox={} extra=x",
                policy.agent.ready_event_prefix, policy.agent.sandbox
            ),
            format!("{}42", policy.agent.ready_event_prefix),
        ] {
            assert!(
                parse_agent_ready(&drift, &policy).is_err(),
                "accepted {drift:?}"
            );
        }

        let complete = format!(
            "{}123 stderr_bytes=456",
            policy.capture.complete_event_prefix
        );
        assert_eq!(
            parse_capture_complete(&complete, &policy).expect("parse capture completion"),
            (123, 456)
        );
        for drift in [
            format!("{}123", policy.capture.complete_event_prefix),
            format!(
                "{}123 stderr_bytes=456 extra=x",
                policy.capture.complete_event_prefix
            ),
            format!("{}x stderr_bytes=456", policy.capture.complete_event_prefix),
        ] {
            assert!(
                parse_capture_complete(&drift, &policy).is_err(),
                "accepted {drift:?}"
            );
        }
    }

    #[test]
    fn quiescence_state_requires_an_exact_boolean_running_field() {
        assert!(!optional_running(None, "absent").expect("absence is stopped"));
        assert!(optional_running(
            Some(&serde_json::json!({"State": {"Running": true}})),
            "agent"
        )
        .expect("exact running state"));
        assert!(!optional_running(
            Some(&serde_json::json!({"State": {"Running": false}})),
            "relay"
        )
        .expect("exact stopped state"));
        assert!(optional_running(
            Some(&serde_json::json!({"State": {"Running": "false"}})),
            "wrong-type"
        )
        .is_err());
        assert!(optional_running(Some(&serde_json::json!({"State": {}})), "missing").is_err());
    }

    #[tokio::test]
    async fn subprocess_pipe_drain_is_memory_bounded_without_blocking_the_writer() {
        let (mut writer, reader) = tokio::io::duplex(32);
        let write = tokio::spawn(async move {
            writer
                .write_all(b"0123456789abcdef")
                .await
                .expect("write oversized fixture");
        });
        let drained = drain_bounded(reader, 5).await.expect("drain fixture");
        write.await.expect("writer task");
        assert_eq!(drained.bytes, b"01234");
        assert!(drained.exceeded);
    }
}
