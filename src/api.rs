//! HTTP surface — the lifecycle-explicit session API.
//!
//! Resource: a `session`, with explicit lifecycle and idempotent verbs.
//! All session-related endpoints share one wire body
//! (`runtime::SessionBody`), discriminated by a `status` field with values
//! `running` | `completed` | `cancelled`. Required-field discipline:
//! every field is always serialised; running-only fields are zeroed for
//! terminal states and vice versa, so clients have one parser.
//!
//! Errors are non-streaming JSON with shape `error::WireError`.
//!
//! Routes:
//!
//! - `POST /v1/agent/sessions` — create. Body `{prompt, folder}`. Blocks
//!   until the isolated agent verifies the pinned model and real tokenizer
//!   endpoint; returns `201 Created` with the running body.
//! - `GET /v1/agent/sessions` — list. Combines in-memory running sessions
//!   with on-disk terminal sessions.
//! - `GET /v1/agent/sessions/{id}` — pure read; idempotent. 200 / 404.
//! - `POST /v1/agent/sessions/{id}/cancel` — cancel; idempotent. 200 with
//!   the current body (running → cancelled, or already terminal).
//! - `DELETE /v1/agent/sessions/{id}` — delete a terminal session from
//!   disk. 204 / 404 / 409 (still running — `cancel` first).
//! - `GET /healthz` — plaintext `"ok"`.
//!
//! There is **no** time-based eviction anywhere — sessions live until
//! DELETE. Reads never mutate. Cancellation is idempotent; repeated deletion
//! returns 404 so the caller receives a definite already-gone state.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::{ServiceError, ServiceResult};
use crate::runtime::{Manager, SessionBody};
use crate::session;
use crate::validation;

#[derive(Clone)]
pub struct AppState {
    /// Read-only handle to the sole compiled-and-mounted configuration.
    pub cfg: Arc<Config>,
    pub manager: Arc<Manager>,
}

pub fn router(state: AppState) -> axum::Router {
    let body_limit = state.cfg.lock.service.request_body_limit_bytes;
    axum::Router::new()
        .route(
            "/v1/agent/sessions",
            post(create_session).get(list_sessions),
        )
        .route(
            "/v1/agent/sessions/{id}",
            get(get_session).delete(delete_session),
        )
        .route("/v1/agent/sessions/{id}/cancel", post(cancel_session))
        .route("/v1/agent/sessions/{id}/wait", get(wait_session))
        .route("/healthz", get(healthz))
        .with_state(state)
        .layer(tower_http::limit::RequestBodyLimitLayer::new(body_limit))
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct CreateRequest {
    pub prompt: String,
    pub folder: String,
}

#[derive(Serialize)]
struct ListResponse {
    sessions: Vec<SessionBody>,
}

async fn healthz() -> &'static str {
    "ok"
}

async fn create_session(
    State(state): State<AppState>,
    Json(body): Json<CreateRequest>,
) -> Result<(StatusCode, Json<SessionBody>), ServiceError> {
    // Synchronous validation; any failure here surfaces as a 4xx with the
    // standard WireError envelope before we ever take the singleton.
    let validated = validation::validate(
        &body.prompt,
        &body.folder,
        &state.cfg.host_input_root,
        &state.cfg.state_dir,
        &state.cfg.results_dir,
    )?;
    tracing::info!(
        prompt_chars = body.prompt.chars().count(),
        folder = %validated.folder.display(),
        "POST /v1/agent/sessions: path pre-flight ok; descriptor-based copy validation follows before agent creation"
    );
    let running = state.manager.submit(validated).await?;
    Ok((StatusCode::CREATED, Json(running)))
}

async fn list_sessions(State(state): State<AppState>) -> Result<Json<ListResponse>, ServiceError> {
    let sessions = state.manager.list().await?;
    Ok(Json(ListResponse { sessions }))
}

async fn get_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SessionBody>, ServiceError> {
    Ok(Json(state.manager.get(&id).await?))
}

async fn cancel_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SessionBody>, ServiceError> {
    Ok(Json(state.manager.cancel(&id).await?))
}

async fn wait_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SessionBody>, ServiceError> {
    Ok(Json(state.manager.wait_terminal(&id).await?))
}

async fn delete_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ServiceError> {
    state.manager.delete(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Used at startup to validate that we can actually serve traffic before
/// binding the listen socket. Surfaces a clear error to the operator if not.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BrokerPreflight {
    policy_id: String,
    profile: String,
    docker_version: String,
    agent_image: serde_json::Value,
    relay_image: serde_json::Value,
    broker: serde_json::Value,
    backend: serde_json::Value,
    service: serde_json::Value,
    model_bridge: serde_json::Value,
    model_ingress: serde_json::Value,
    backend_cache_volume: serde_json::Value,
    backend_cache_owner_mode: String,
    backend_ipv4_routes: String,
    backend_ipv6_routes: String,
    service_ipv4_routes: String,
    service_ipv6_routes: String,
    gpu_record: String,
}

pub async fn pre_flight(cfg: &Config) -> ServiceResult<()> {
    let evidence: BrokerPreflight =
        serde_json::from_value(crate::docker_ops::preflight(cfg).await?).map_err(|error| {
            ServiceError::Internal(format!(
                "broker preflight evidence violates its exact schema: {error}"
            ))
        })?;
    require_equal(
        "broker policy ID",
        &evidence.policy_id,
        &cfg.lock.broker.policy_id,
    )?;
    require_equal("broker profile", &evidence.profile, &cfg.lock.profile)?;
    require_equal(
        "Docker server version",
        &evidence.docker_version,
        &cfg.lock.host.docker_version,
    )?;
    verify_agent_image_labels(cfg, single_inspect(&evidence.agent_image, "agent image")?)?;
    verify_relay_image(cfg, single_inspect(&evidence.relay_image, "relay image")?)?;
    verify_broker_container(cfg, single_inspect(&evidence.broker, "broker container")?)?;
    verify_service_container(cfg, single_inspect(&evidence.service, "service container")?).await?;
    let backend = single_inspect(&evidence.backend, "backend container")?;
    verify_backend_container(cfg, backend)?;
    verify_backend_cache(
        cfg,
        single_inspect(&evidence.backend_cache_volume, "backend cache volume")?,
        &evidence.backend_cache_owner_mode,
    )?;
    require_equal("backend IPv4 routes", &evidence.backend_ipv4_routes, "")?;
    require_equal("backend IPv6 routes", &evidence.backend_ipv6_routes, "")?;
    require_equal("service IPv4 routes", &evidence.service_ipv4_routes, "")?;
    require_equal("service IPv6 routes", &evidence.service_ipv6_routes, "")?;
    verify_model_relays(
        cfg,
        backend,
        single_inspect(&evidence.model_bridge, "model bridge")?,
        single_inspect(&evidence.model_ingress, "model ingress")?,
    )?;
    verify_host_contract(cfg, &evidence.gpu_record)?;
    verify_model_manifest(cfg).await?;
    verify_model_socket(cfg)?;
    verify_backend_http(cfg).await?;
    crate::bundle::check_host_dependencies().await?;

    // The host launcher creates the two durable roots. The service validates
    // rather than chmodding/adopting an unexpected object. Only the fixed
    // `state/sessions` child may be created here, exclusively, before any
    // request is accepted.
    require_owned_runtime_directory(&cfg.state_dir, 0o700, 1000, 1000, false)?;
    require_owned_runtime_directory(&cfg.results_dir, 0o700, 1000, 1000, false)?;
    require_owned_runtime_directory(&cfg.state_dir.join("sessions"), 0o700, 1000, 1000, true)?;

    // Sweep any orphans from a prior crash before announcing ourselves.
    // Docker objects (containers), crash-interrupted result directories (no
    // `finished.json`), and ordinary abandoned staging dirs. A terminal that
    // explicitly retained its raw tree after bundle failure remains forensic
    // evidence and is never swept. Sweeps
    // complete (or fail loudly) BEFORE the listener binds, so no incoming
    // request can land while a half-cleaned-up prior session exists.
    session::sweep_orphans(cfg).await?;
    // Reconcile state before result-only leftovers. This ordering preserves
    // both sides of a crash-interrupted terminalization: if a raw-state tree
    // still exists beside an incomplete result directory, state reconciliation
    // either completes the durable terminal publication or refuses startup
    // without deleting either source of evidence.
    sweep_state_dir(&cfg.state_dir, &cfg.results_dir, 1000, 1000)?;
    sweep_partial_results(&cfg.results_dir, &cfg.state_dir, 1000, 1000)?;

    Ok(())
}

fn single_inspect<'a>(
    value: &'a serde_json::Value,
    label: &str,
) -> ServiceResult<&'a serde_json::Value> {
    let values = value.as_array().ok_or_else(|| {
        ServiceError::Internal(format!("broker {label} evidence is not an inspect array"))
    })?;
    if values.len() != 1 {
        return Err(ServiceError::Internal(format!(
            "broker {label} evidence contains {} objects, expected exactly one",
            values.len()
        )));
    }
    Ok(&values[0])
}

async fn verify_service_container(cfg: &Config, value: &serde_json::Value) -> ServiceResult<()> {
    require_equal(
        "service AppArmor profile",
        value
            .pointer("/AppArmorProfile")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("<missing>"),
        &cfg.lock.host.container_apparmor_profile,
    )?;
    if value
        .pointer("/State/Running")
        .and_then(serde_json::Value::as_bool)
        != Some(true)
    {
        return Err(ServiceError::Internal(
            "service container is not running during self-preflight".into(),
        ));
    }
    let image = value
        .pointer("/Image")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ServiceError::Internal("service inspect lacks Image".into()))?;
    let configured_image = value
        .pointer("/Config/Image")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ServiceError::Internal("service inspect lacks Config.Image".into()))?;
    if image.len() != 71
        || !image.starts_with("sha256:")
        || configured_image != image
        || image[7..]
            .bytes()
            .any(|byte| !(byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')))
    {
        return Err(ServiceError::Internal(format!(
            "service was not created from one immutable image ID: Image={image:?} Config.Image={configured_image:?}"
        )));
    }
    require_equal(
        "service configured user",
        value
            .pointer("/Config/User")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("<missing>"),
        &cfg.lock.service.user,
    )?;
    require_equal(
        "service working directory",
        value
            .pointer("/Config/WorkingDir")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("<missing>"),
        "/home/user/Desktop/agent_service",
    )?;
    let entrypoint = value
        .pointer("/Config/Entrypoint")
        .and_then(serde_json::Value::as_array)
        .and_then(|items| (items.len() == 1).then(|| items[0].as_str()).flatten());
    if entrypoint != Some("/usr/local/bin/agent_service")
        || value
            .pointer("/Config/Cmd")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|items| !items.is_empty())
    {
        return Err(ServiceError::Internal(format!(
            "service process contract drift: Entrypoint={:?} Cmd={:?}",
            value.pointer("/Config/Entrypoint"),
            value.pointer("/Config/Cmd")
        )));
    }
    let mode = value
        .pointer("/HostConfig/NetworkMode")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ServiceError::Internal("service inspect lacks network mode".into()))?;
    require_equal("service network mode", mode, "none")?;
    if value
        .pointer("/HostConfig/ReadonlyRootfs")
        .and_then(serde_json::Value::as_bool)
        != Some(true)
    {
        return Err(ServiceError::Internal(
            "service root filesystem is not read-only".into(),
        ));
    }
    for (pointer, expected, label) in [
        ("/HostConfig/Privileged", false, "privileged flag"),
        ("/HostConfig/ReadonlyRootfs", true, "read-only root"),
    ] {
        if value.pointer(pointer).and_then(serde_json::Value::as_bool) != Some(expected) {
            return Err(ServiceError::Internal(format!(
                "service {label} drift at {pointer}: {:?}",
                value.pointer(pointer)
            )));
        }
    }
    for (pointer, expected, label) in [
        ("/HostConfig/Memory", 2_147_483_648_u64, "memory"),
        ("/HostConfig/MemorySwap", 2_147_483_648_u64, "memory+swap"),
        ("/HostConfig/PidsLimit", 512_u64, "PID limit"),
    ] {
        if value.pointer(pointer).and_then(serde_json::Value::as_u64) != Some(expected) {
            return Err(ServiceError::Internal(format!(
                "service {label} drift at {pointer}: {:?}",
                value.pointer(pointer)
            )));
        }
    }
    let tmpfs = value
        .pointer("/HostConfig/Tmpfs")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| ServiceError::Internal("service inspect lacks tmpfs map".into()))?;
    if tmpfs.len() != 1
        || tmpfs.get("/tmp").and_then(serde_json::Value::as_str)
            != Some(cfg.lock.service.tmpfs_tmp.as_str())
    {
        return Err(ServiceError::Internal(format!(
            "service tmpfs contract drift: {tmpfs:?}"
        )));
    }
    if value
        .pointer("/HostConfig/CapDrop")
        .and_then(serde_json::Value::as_array)
        .is_none_or(|items| items.len() != 1 || items[0].as_str() != Some("ALL"))
        || value
            .pointer("/HostConfig/SecurityOpt")
            .and_then(serde_json::Value::as_array)
            .is_none_or(|items| {
                items.len() != 1 || items[0].as_str() != Some("no-new-privileges:true")
            })
        || value
            .pointer("/HostConfig/CapAdd")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|items| !items.is_empty())
        || value
            .pointer("/HostConfig/Devices")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|items| !items.is_empty())
        || value
            .pointer("/HostConfig/DeviceRequests")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|items| !items.is_empty())
    {
        return Err(ServiceError::Internal(
            "service capabilities, no-new-privileges, or device contract drift".into(),
        ));
    }
    for (pointer, expected, label) in [
        ("/HostConfig/PidMode", "", "PID namespace"),
        ("/HostConfig/IpcMode", "private", "IPC namespace"),
        ("/HostConfig/UTSMode", "", "UTS namespace"),
    ] {
        require_equal(
            &format!("service {label}"),
            value
                .pointer(pointer)
                .and_then(serde_json::Value::as_str)
                .unwrap_or("<missing>"),
            expected,
        )?;
    }
    let bindings = value
        .pointer("/HostConfig/PortBindings")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| {
            ServiceError::Internal("service inspect lacks port bindings object".into())
        })?;
    if !bindings.is_empty() {
        return Err(ServiceError::Internal(format!(
            "service container has forbidden published ports: {bindings:?}"
        )));
    }
    let network_ports = value
        .pointer("/NetworkSettings/Ports")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| {
            ServiceError::Internal("service inspect lacks NetworkSettings.Ports".into())
        })?;
    if !network_ports.is_empty() {
        return Err(ServiceError::Internal(format!(
            "service container has forbidden network port state: {network_ports:?}"
        )));
    }
    require_equal(
        "service restart policy",
        value
            .pointer("/HostConfig/RestartPolicy/Name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("<missing>"),
        "no",
    )?;
    let mounts = value
        .get("Mounts")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| ServiceError::Internal("service inspect lacks Mounts".into()))?;
    let control_dir = cfg
        .broker_socket
        .parent()
        .ok_or_else(|| ServiceError::Internal("broker socket has no parent".into()))?;
    let control_dir = control_dir
        .to_str()
        .ok_or_else(|| ServiceError::Internal("broker control directory is not UTF-8".into()))?;
    let expected_mounts = std::collections::BTreeMap::from([
        (
            cfg.lock.service.host_input_root.as_str(),
            (cfg.lock.service.host_input_root.as_str(), false),
        ),
        (
            cfg.lock.service.state_dir.as_str(),
            (cfg.lock.service.state_dir.as_str(), true),
        ),
        (
            cfg.lock.service.results_dir.as_str(),
            (cfg.lock.service.results_dir.as_str(), true),
        ),
        (control_dir, (control_dir, false)),
        (
            cfg.lock.relay.model_socket_dir.as_str(),
            (cfg.lock.relay.model_socket_dir.as_str(), false),
        ),
    ]);
    if mounts.len() != expected_mounts.len() {
        return Err(ServiceError::Internal(format!(
            "service mount count drift: expected {}, observed {}: {mounts:?}",
            expected_mounts.len(),
            mounts.len()
        )));
    }
    let mut seen = std::collections::BTreeSet::new();
    for mount in mounts {
        let destination = mount
            .get("Destination")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                ServiceError::Internal(format!("service mount lacks destination: {mount}"))
            })?;
        if !seen.insert(destination.to_string()) {
            return Err(ServiceError::Internal(format!(
                "service has duplicate mount destination {destination:?}"
            )));
        }
        let (source, writable) = expected_mounts.get(destination).ok_or_else(|| {
            ServiceError::Internal(format!("service has unexpected mount: {mount}"))
        })?;
        if mount.get("Source").and_then(serde_json::Value::as_str) != Some(*source)
            || mount.get("Type").and_then(serde_json::Value::as_str) != Some("bind")
            || mount.get("RW").and_then(serde_json::Value::as_bool) != Some(*writable)
            || mount.get("Propagation").and_then(serde_json::Value::as_str) != Some("rprivate")
        {
            return Err(ServiceError::Internal(format!(
                "service mount drift at {destination:?}: {mount}"
            )));
        }
    }
    let labels: std::collections::HashMap<String, String> = serde_json::from_value(
        value
            .pointer("/Config/Labels")
            .cloned()
            .ok_or_else(|| ServiceError::Internal("service inspect lacks labels".into()))?,
    )
    .map_err(|error| ServiceError::Internal(format!("service labels invalid: {error}")))?;
    require_equal(
        "service profile label",
        labels
            .get("agent_service.profile")
            .map(String::as_str)
            .unwrap_or("<missing>"),
        &cfg.lock.profile,
    )?;
    require_equal(
        "service component label",
        labels
            .get("agent_service.component")
            .map(String::as_str)
            .unwrap_or("<missing>"),
        "service",
    )?;
    let mounted_lock_sha = sha256_path(std::path::Path::new(
        "/home/user/Desktop/agent_service/config/stack.lock.json",
    ))
    .await?;
    require_equal(
        "service compiled stack-lock label",
        labels
            .get("agent_service.stack-lock.sha256")
            .map(String::as_str)
            .unwrap_or("<missing>"),
        &mounted_lock_sha,
    )?;
    let mounted_cargo_sha = sha256_path(std::path::Path::new(
        "/home/user/Desktop/agent_service/Cargo.lock",
    ))
    .await?;
    require_equal(
        "service Cargo-lock label",
        labels
            .get("agent_service.cargo-lock.sha256")
            .map(String::as_str)
            .unwrap_or("<missing>"),
        &mounted_cargo_sha,
    )?;
    Ok(())
}

fn verify_host_contract(cfg: &Config, query: &str) -> ServiceResult<()> {
    let fields = query.trim().split(',').map(str::trim).collect::<Vec<_>>();
    if fields.len() != 3 {
        return Err(ServiceError::Internal(format!(
            "unexpected nvidia-smi record: {query:?}"
        )));
    }
    require_equal("GPU name", fields[0], &cfg.lock.host.gpu_name)?;
    let memory = fields[1].parse::<u64>().map_err(|error| {
        ServiceError::Internal(format!(
            "GPU memory field {:?} is invalid: {error}",
            fields[1]
        ))
    })?;
    if memory != cfg.lock.host.gpu_memory_mib {
        return Err(ServiceError::Internal(format!(
            "GPU memory drift: expected {} MiB, observed {memory} MiB",
            cfg.lock.host.gpu_memory_mib
        )));
    }
    require_equal(
        "NVIDIA driver version",
        fields[2],
        &cfg.lock.host.driver_version,
    )
}

async fn verify_model_manifest(cfg: &Config) -> ServiceResult<()> {
    let path = std::path::Path::new(&cfg.lock.backend.project_dir)
        .join("manifests")
        .join(&cfg.lock.backend.model_manifest);
    let digest = sha256_path(&path).await?;
    require_equal(
        "model manifest SHA256",
        &digest,
        &cfg.lock.backend.model_manifest_sha256,
    )
}

async fn sha256_path(path: &std::path::Path) -> ServiceResult<String> {
    use std::process::Stdio;
    let output = tokio::process::Command::new("sha256sum")
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|error| {
            ServiceError::Internal(format!("cannot hash {}: {error}", path.display()))
        })?;
    if !output.status.success() {
        return Err(ServiceError::Internal(format!(
            "sha256sum {} failed: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string())
}

fn require_equal(role: &str, actual: &str, expected: &str) -> ServiceResult<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(ServiceError::Internal(format!(
            "{role} drift: expected {expected:?}, observed {actual:?}"
        )))
    }
}

fn require_image_id(value: &serde_json::Value, expected: &str, label: &str) -> ServiceResult<()> {
    require_equal(
        &format!("{label} image ID"),
        value
            .get("Id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("<missing>"),
        expected,
    )
}

fn labels(
    value: &serde_json::Value,
    label: &str,
) -> ServiceResult<std::collections::HashMap<String, String>> {
    serde_json::from_value(
        value
            .pointer("/Config/Labels")
            .cloned()
            .ok_or_else(|| ServiceError::Internal(format!("{label} lacks Config.Labels")))?,
    )
    .map_err(|error| ServiceError::Internal(format!("{label} labels are invalid: {error}")))
}

fn verify_relay_image(cfg: &Config, value: &serde_json::Value) -> ServiceResult<()> {
    require_image_id(value, &cfg.lock.relay.image_id, "fixed relay")?;
    let labels = labels(value, "fixed relay image")?;
    for (key, expected) in [
        ("agent_service.profile", cfg.lock.profile.as_str()),
        ("agent_service.component", "fixed-relay"),
        (
            "agent_service.relay.source.sha256",
            cfg.lock.relay.source_sha256.as_str(),
        ),
    ] {
        require_equal(
            &format!("fixed relay image label {key}"),
            labels.get(key).map(String::as_str).unwrap_or("<missing>"),
            expected,
        )?;
    }
    Ok(())
}

fn verify_broker_container(cfg: &Config, value: &serde_json::Value) -> ServiceResult<()> {
    if value
        .pointer("/State/Running")
        .and_then(serde_json::Value::as_bool)
        != Some(true)
    {
        return Err(ServiceError::Internal(
            "Docker broker is not running".into(),
        ));
    }
    require_equal(
        "broker image ID",
        value
            .get("Image")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("<missing>"),
        &cfg.lock.broker.image_id,
    )?;
    require_equal(
        "broker configured image ID",
        value
            .pointer("/Config/Image")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("<missing>"),
        &cfg.lock.broker.image_id,
    )?;
    require_equal(
        "broker network mode",
        value
            .pointer("/HostConfig/NetworkMode")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("<missing>"),
        "none",
    )?;
    let labels = labels(value, "broker")?;
    for (key, expected) in [
        ("agent_service.profile", cfg.lock.profile.as_str()),
        ("agent_service.component", "docker-broker"),
        (
            "agent_service.broker.policy.sha256",
            cfg.lock.broker.policy_sha256.as_str(),
        ),
        (
            "agent_service.broker.source.sha256",
            cfg.lock.broker.source_sha256.as_str(),
        ),
    ] {
        require_equal(
            &format!("broker label {key}"),
            labels.get(key).map(String::as_str).unwrap_or("<missing>"),
            expected,
        )?;
    }
    let mounts = value
        .get("Mounts")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| ServiceError::Internal("broker lacks Mounts".into()))?;
    let docker_mounts = mounts
        .iter()
        .filter(|mount| {
            mount.get("Destination").and_then(serde_json::Value::as_str)
                == Some(cfg.lock.host.docker_socket.as_str())
        })
        .collect::<Vec<_>>();
    if docker_mounts.len() != 1
        || docker_mounts[0]
            .get("RW")
            .and_then(serde_json::Value::as_bool)
            != Some(false)
    {
        return Err(ServiceError::Internal(format!(
            "broker must have exactly one read-only raw Docker socket bind, observed {docker_mounts:?}"
        )));
    }
    Ok(())
}

fn verify_model_relays(
    cfg: &Config,
    backend: &serde_json::Value,
    bridge: &serde_json::Value,
    ingress: &serde_json::Value,
) -> ServiceResult<()> {
    let backend_id = backend
        .get("Id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ServiceError::Internal("backend inspect lacks Id".into()))?;
    verify_fixed_relay_container(
        cfg,
        bridge,
        "model bridge",
        "model-bridge",
        &format!("container:{backend_id}"),
        true,
    )?;
    verify_fixed_relay_container(
        cfg,
        ingress,
        "model ingress",
        "model-ingress",
        "host",
        false,
    )
}

fn verify_fixed_relay_container(
    cfg: &Config,
    value: &serde_json::Value,
    label: &str,
    role: &str,
    network_mode: &str,
    socket_writable: bool,
) -> ServiceResult<()> {
    require_equal(
        &format!("{label} AppArmor profile"),
        value
            .pointer("/AppArmorProfile")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("<missing>"),
        &cfg.lock.host.container_apparmor_profile,
    )?;
    if value
        .pointer("/State/Running")
        .and_then(serde_json::Value::as_bool)
        != Some(true)
    {
        return Err(ServiceError::Internal(format!("{label} is not running")));
    }
    let entrypoint = value
        .pointer("/Config/Entrypoint")
        .and_then(serde_json::Value::as_array)
        .and_then(|items| (items.len() == 1).then(|| items[0].as_str()).flatten());
    if entrypoint != Some("/fixed_relay") {
        return Err(ServiceError::Internal(format!(
            "{label} entrypoint drift: {:?}",
            value.pointer("/Config/Entrypoint")
        )));
    }
    for (pointer, expected, property) in [
        ("/Image", cfg.lock.relay.image_id.as_str(), "image ID"),
        (
            "/Config/Image",
            cfg.lock.relay.image_id.as_str(),
            "configured image ID",
        ),
        ("/Config/User", "1000:1000", "user"),
        ("/HostConfig/NetworkMode", network_mode, "network mode"),
    ] {
        require_equal(
            &format!("{label} {property}"),
            value
                .pointer(pointer)
                .and_then(serde_json::Value::as_str)
                .unwrap_or("<missing>"),
            expected,
        )?;
    }
    if value
        .pointer("/HostConfig/ReadonlyRootfs")
        .and_then(serde_json::Value::as_bool)
        != Some(true)
    {
        return Err(ServiceError::Internal(format!(
            "{label} root filesystem is not read-only"
        )));
    }
    let command = value
        .pointer("/Config/Cmd")
        .and_then(serde_json::Value::as_array)
        .and_then(|items| (items.len() == 1).then(|| items[0].as_str()).flatten());
    if command != Some(role) {
        return Err(ServiceError::Internal(format!(
            "{label} command drift: expected [{role:?}], observed {:?}",
            value.pointer("/Config/Cmd")
        )));
    }
    for (pointer, expected, property) in [
        ("/HostConfig/Memory", 33_554_432_u64, "memory"),
        ("/HostConfig/MemorySwap", 33_554_432_u64, "memory+swap"),
        ("/HostConfig/PidsLimit", 32_u64, "PID limit"),
    ] {
        if value.pointer(pointer).and_then(serde_json::Value::as_u64) != Some(expected) {
            return Err(ServiceError::Internal(format!(
                "{label} {property} drift: {:?}",
                value.pointer(pointer)
            )));
        }
    }
    if value
        .pointer("/HostConfig/Privileged")
        .and_then(serde_json::Value::as_bool)
        != Some(false)
        || value
            .pointer("/HostConfig/CapDrop")
            .and_then(serde_json::Value::as_array)
            .is_none_or(|items| items.len() != 1 || items[0].as_str() != Some("ALL"))
        || value
            .pointer("/HostConfig/SecurityOpt")
            .and_then(serde_json::Value::as_array)
            .is_none_or(|items| {
                items.len() != 1 || items[0].as_str() != Some("no-new-privileges:true")
            })
    {
        return Err(ServiceError::Internal(format!(
            "{label} capability/no-new-privileges contract drift"
        )));
    }
    for (pointer, expected, property) in [
        ("/HostConfig/RestartPolicy/Name", "no", "restart policy"),
        ("/HostConfig/PidMode", "", "PID namespace"),
        ("/HostConfig/IpcMode", "private", "IPC namespace"),
        ("/HostConfig/UTSMode", "", "UTS namespace"),
    ] {
        require_equal(
            &format!("{label} {property}"),
            value
                .pointer(pointer)
                .and_then(serde_json::Value::as_str)
                .unwrap_or("<missing>"),
            expected,
        )?;
    }
    let ports = value
        .pointer("/HostConfig/PortBindings")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| ServiceError::Internal(format!("{label} lacks port bindings")))?;
    if !ports.is_empty() {
        return Err(ServiceError::Internal(format!(
            "{label} has forbidden published ports: {ports:?}"
        )));
    }
    let container_labels = labels(value, label)?;
    for (key, expected) in [
        ("agent_service.profile", cfg.lock.profile.as_str()),
        ("agent_service.component", role),
        (
            "agent_service.relay.source.sha256",
            cfg.lock.relay.source_sha256.as_str(),
        ),
        (
            "agent_service.relay.sandbox",
            cfg.lock.relay.sandbox.as_str(),
        ),
    ] {
        require_equal(
            &format!("{label} label {key}"),
            container_labels
                .get(key)
                .map(String::as_str)
                .unwrap_or("<missing>"),
            expected,
        )?;
    }
    let mounts = value
        .get("Mounts")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| ServiceError::Internal(format!("{label} lacks Mounts")))?;
    if mounts.len() != 1
        || mounts[0].get("Source").and_then(serde_json::Value::as_str)
            != Some(cfg.lock.relay.model_socket_dir.as_str())
        || mounts[0]
            .get("Destination")
            .and_then(serde_json::Value::as_str)
            != Some("/sock")
        || mounts[0].get("RW").and_then(serde_json::Value::as_bool) != Some(socket_writable)
    {
        return Err(ServiceError::Internal(format!(
            "{label} socket mount drift: {mounts:?}"
        )));
    }
    Ok(())
}

fn verify_model_socket(cfg: &Config) -> ServiceResult<()> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
    let metadata = std::fs::symlink_metadata(&cfg.model_socket).map_err(|error| {
        ServiceError::Internal(format!(
            "cannot stat central model socket {}: {error}",
            cfg.model_socket.display()
        ))
    })?;
    if !metadata.file_type().is_socket()
        || metadata.uid() != 1000
        || metadata.gid() != 1000
        || metadata.permissions().mode() & 0o777 != 0o660
    {
        return Err(ServiceError::Internal(format!(
            "central model socket drift at {}: socket={} uid={} gid={} mode={:o}",
            cfg.model_socket.display(),
            metadata.file_type().is_socket(),
            metadata.uid(),
            metadata.gid(),
            metadata.permissions().mode() & 0o777
        )));
    }
    Ok(())
}

fn verify_agent_image_labels(cfg: &Config, value: &serde_json::Value) -> ServiceResult<()> {
    require_equal(
        "agent image ID",
        value
            .get("Id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("<missing>"),
        &cfg.lock.agent.image_id,
    )?;
    let labels: std::collections::HashMap<String, String> = serde_json::from_value(
        value
            .pointer("/Config/Labels")
            .cloned()
            .ok_or_else(|| ServiceError::Internal("agent image lacks Config.Labels".into()))?,
    )
    .map_err(|error| {
        ServiceError::Internal(format!("agent image labels are not a string map: {error}"))
    })?;
    for (key, expected) in [
        ("agent_service.profile", cfg.lock.profile.as_str()),
        (
            "agent_service.qwen.version",
            cfg.lock.agent.qwen_code.version.as_str(),
        ),
        (
            "agent_service.qwen.commit",
            cfg.lock.agent.qwen_code.commit.as_str(),
        ),
        (
            "agent_service.qwen.archive.sha256",
            cfg.lock.agent.qwen_code.source_archive_sha256.as_str(),
        ),
        (
            "agent_service.qwen.patch.sha256",
            cfg.lock.agent.qwen_code.patch_sha256.as_str(),
        ),
        (
            "agent_service.qwen.source-patch-manifest.sha256",
            cfg.lock
                .agent
                .qwen_code
                .source_patch_manifest_sha256
                .as_str(),
        ),
        (
            "agent_service.settings.sha256",
            cfg.lock.agent.settings_sha256.as_str(),
        ),
        (
            "agent_service.instructions.sha256",
            cfg.lock.agent.instructions_sha256.as_str(),
        ),
        (
            "agent_service.system-prompt.sha256",
            cfg.lock.agent.system_prompt_sha256.as_str(),
        ),
        (
            "agent_service.deployment-contract.sha256",
            cfg.lock.agent.deployment_contract_sha256.as_str(),
        ),
        (
            "agent_service.toolchain-manifest.sha256",
            cfg.lock.agent.toolchain_manifest_sha256.as_str(),
        ),
        (
            "agent_service.toolchain-verifier.sha256",
            cfg.lock.build.toolchain_verifier_sha256.as_str(),
        ),
        (
            "agent_service.runtime-contract.sha256",
            cfg.lock.agent.runtime_contract_sha256.as_str(),
        ),
        (
            "agent_service.runtime-contract-verifier.sha256",
            cfg.lock.build.runtime_contract_verifier_sha256.as_str(),
        ),
        (
            "agent_service.wrapper.sha256",
            cfg.lock.agent.wrapper_sha256.as_str(),
        ),
    ] {
        require_equal(
            &format!("agent image label {key}"),
            labels.get(key).map(String::as_str).unwrap_or("<missing>"),
            expected,
        )?;
    }
    Ok(())
}

fn verify_backend_container(cfg: &Config, value: &serde_json::Value) -> ServiceResult<()> {
    let string = |pointer: &str| -> ServiceResult<&str> {
        value
            .pointer(pointer)
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                ServiceError::Internal(format!("backend inspect lacks string {pointer}"))
            })
    };
    if value
        .pointer("/State/Running")
        .and_then(serde_json::Value::as_bool)
        != Some(true)
    {
        return Err(ServiceError::Internal(
            "pinned backend container is not running".into(),
        ));
    }
    require_equal(
        "backend AppArmor profile",
        string("/AppArmorProfile")?,
        &cfg.lock.host.container_apparmor_profile,
    )?;
    require_equal(
        "backend image ID",
        string("/Image")?,
        &cfg.lock.backend.image_id,
    )?;
    require_equal(
        "backend configured immutable image ID",
        string("/Config/Image")?,
        &cfg.lock.backend.image_id,
    )?;
    require_equal(
        "backend non-root user",
        string("/Config/User")?,
        &cfg.lock.backend.user,
    )?;
    require_equal(
        "backend network mode",
        string("/HostConfig/NetworkMode")?,
        "none",
    )?;
    for (pointer, expected, label) in [
        ("/HostConfig/Privileged", false, "privileged flag"),
        ("/HostConfig/ReadonlyRootfs", true, "read-only root"),
        ("/HostConfig/AutoRemove", false, "automatic removal flag"),
    ] {
        if value.pointer(pointer).and_then(serde_json::Value::as_bool) != Some(expected) {
            return Err(ServiceError::Internal(format!(
                "backend {label} drift at {pointer}: {:?}",
                value.pointer(pointer)
            )));
        }
    }
    for (pointer, expected, label) in [
        ("/HostConfig/PidMode", "", "PID namespace mode"),
        ("/HostConfig/IpcMode", "private", "IPC namespace mode"),
        ("/HostConfig/UTSMode", "", "UTS namespace mode"),
        ("/HostConfig/RestartPolicy/Name", "no", "restart policy"),
    ] {
        require_equal(&format!("backend {label}"), string(pointer)?, expected)?;
    }
    for (pointer, expected, label) in [
        (
            "/HostConfig/ShmSize",
            8_589_934_592_u64,
            "shared-memory size",
        ),
        (
            "/HostConfig/RestartPolicy/MaximumRetryCount",
            0_u64,
            "restart retry count",
        ),
    ] {
        if value.pointer(pointer).and_then(serde_json::Value::as_u64) != Some(expected) {
            return Err(ServiceError::Internal(format!(
                "backend {label} drift at {pointer}: {:?}",
                value.pointer(pointer)
            )));
        }
    }
    let expected_cap_drop = serde_json::json!(["ALL"]);
    let expected_security = serde_json::json!(["no-new-privileges:true"]);
    let expected_devices = serde_json::json!([]);
    let expected_device_requests = serde_json::json!([{
        "Driver": "",
        "Count": -1,
        "DeviceIDs": null,
        "Capabilities": [["gpu"]],
        "Options": {}
    }]);
    for (pointer, expected, label) in [
        (
            "/HostConfig/CapDrop",
            &expected_cap_drop,
            "dropped capabilities",
        ),
        (
            "/HostConfig/SecurityOpt",
            &expected_security,
            "security options",
        ),
        ("/HostConfig/Devices", &expected_devices, "legacy devices"),
        (
            "/HostConfig/DeviceRequests",
            &expected_device_requests,
            "GPU device request",
        ),
    ] {
        if value.pointer(pointer) != Some(expected) {
            return Err(ServiceError::Internal(format!(
                "backend {label} drift at {pointer}: expected {expected}, observed {:?}",
                value.pointer(pointer)
            )));
        }
    }
    if !value
        .pointer("/HostConfig/CapAdd")
        .is_some_and(serde_json::Value::is_null)
    {
        return Err(ServiceError::Internal(format!(
            "backend added-capability state must be null, observed {:?}",
            value.pointer("/HostConfig/CapAdd")
        )));
    }
    if value
        .pointer("/HostConfig/ReadonlyRootfs")
        .and_then(serde_json::Value::as_bool)
        != Some(cfg.lock.backend.rootfs_read_only)
    {
        return Err(ServiceError::Internal(
            "backend root filesystem is not the exact read-only contract".into(),
        ));
    }
    let tmpfs: std::collections::BTreeMap<String, String> = serde_json::from_value(
        value
            .pointer("/HostConfig/Tmpfs")
            .cloned()
            .ok_or_else(|| ServiceError::Internal("backend inspect lacks tmpfs map".into()))?,
    )
    .map_err(|error| ServiceError::Internal(format!("backend tmpfs map invalid: {error}")))?;
    if tmpfs != cfg.lock.backend.tmpfs {
        return Err(ServiceError::Internal(format!(
            "backend tmpfs contract drift; expected {:?}; observed {tmpfs:?}",
            cfg.lock.backend.tmpfs
        )));
    }
    for pointer in ["/HostConfig/PortBindings", "/NetworkSettings/Ports"] {
        let ports = value
            .pointer(pointer)
            .ok_or_else(|| ServiceError::Internal(format!("backend inspect lacks {pointer}")))?;
        if !ports.as_object().is_some_and(serde_json::Map::is_empty) {
            return Err(ServiceError::Internal(format!(
                "backend has forbidden published-port state at {pointer}: {ports}"
            )));
        }
    }
    let command: Vec<String> = serde_json::from_value(
        value
            .pointer("/Config/Cmd")
            .cloned()
            .ok_or_else(|| ServiceError::Internal("backend inspect lacks Config.Cmd".into()))?,
    )
    .map_err(|error| ServiceError::Internal(format!("backend command shape invalid: {error}")))?;
    if command != cfg.lock.backend.command {
        return Err(ServiceError::Internal(format!(
            "backend command drift; expected {:?}; observed {:?}",
            cfg.lock.backend.command, command
        )));
    }
    let environment: Vec<String> = serde_json::from_value(
        value
            .pointer("/Config/Env")
            .cloned()
            .ok_or_else(|| ServiceError::Internal("backend inspect lacks Config.Env".into()))?,
    )
    .map_err(|error| {
        ServiceError::Internal(format!("backend environment shape invalid: {error}"))
    })?;
    for expected in &cfg.lock.backend.environment {
        let key = expected
            .split_once('=')
            .map(|(key, _)| key)
            .ok_or_else(|| {
                ServiceError::Internal(format!(
                    "locked backend environment entry has no '=': {expected:?}"
                ))
            })?;
        let matching = environment
            .iter()
            .filter(|entry| {
                entry
                    .split_once('=')
                    .is_some_and(|(actual_key, _)| actual_key == key)
            })
            .collect::<Vec<_>>();
        if matching.len() != 1 || matching[0].as_str() != expected {
            return Err(ServiceError::Internal(format!(
                "backend environment drift for {key}; expected exactly {expected:?}; observed {matching:?}"
            )));
        }
    }
    let mounts = value
        .pointer("/Mounts")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| ServiceError::Internal("backend inspect lacks Mounts array".into()))?;
    if mounts.len() != 2 {
        return Err(ServiceError::Internal(format!(
            "backend must have exactly the model and vLLM-cache mounts, observed {}",
            mounts.len()
        )));
    }
    let model_mounts = mounts
        .iter()
        .filter(|mount| {
            mount.get("Destination").and_then(serde_json::Value::as_str) == Some("/model")
        })
        .collect::<Vec<_>>();
    if model_mounts.len() != 1 {
        return Err(ServiceError::Internal(format!(
            "backend must have exactly one /model mount, observed {}",
            model_mounts.len()
        )));
    }
    let model_mount = model_mounts[0];
    let expected_model_source =
        std::path::Path::new(&cfg.lock.backend.project_dir).join(&cfg.lock.backend.model_directory);
    let expected_model_source = expected_model_source.to_str().ok_or_else(|| {
        ServiceError::Internal("locked corrected-model path is not valid UTF-8".into())
    })?;
    require_equal(
        "backend corrected-model mount source",
        model_mount
            .get("Source")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("<missing>"),
        expected_model_source,
    )?;
    require_equal(
        "backend corrected-model mount type",
        model_mount
            .get("Type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("<missing>"),
        "bind",
    )?;
    if model_mount.get("RW").and_then(serde_json::Value::as_bool) != Some(false) {
        return Err(ServiceError::Internal(
            "backend corrected-model mount is not read-only".into(),
        ));
    }
    let cache_mounts = mounts
        .iter()
        .filter(|mount| {
            mount.get("Destination").and_then(serde_json::Value::as_str)
                == Some(cfg.lock.backend.cache_mount.as_str())
        })
        .collect::<Vec<_>>();
    if cache_mounts.len() != 1 {
        return Err(ServiceError::Internal(format!(
            "backend must have exactly one {} mount, observed {}",
            cfg.lock.backend.cache_mount,
            cache_mounts.len(),
        )));
    }
    let cache_mount = cache_mounts[0];
    require_equal(
        "backend vLLM cache volume",
        cache_mount
            .get("Name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("<missing>"),
        &cfg.lock.backend.cache_volume,
    )?;
    require_equal(
        "backend vLLM cache mount type",
        cache_mount
            .get("Type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("<missing>"),
        "volume",
    )?;
    if cache_mount.get("RW").and_then(serde_json::Value::as_bool) != Some(true) {
        return Err(ServiceError::Internal(
            "backend vLLM cache mount is not writable".into(),
        ));
    }
    let labels: std::collections::HashMap<String, String> = serde_json::from_value(
        value
            .pointer("/Config/Labels")
            .cloned()
            .ok_or_else(|| ServiceError::Internal("backend inspect lacks Config.Labels".into()))?,
    )
    .map_err(|error| ServiceError::Internal(format!("backend labels invalid: {error}")))?;
    for (key, expected) in [
        ("qwen38.project", cfg.lock.backend.project_label.as_str()),
        (
            "qwen38.runtime.profile",
            cfg.lock.backend.profile_label.as_str(),
        ),
        (
            "qwen38.model.revision",
            cfg.lock.backend.model_revision.as_str(),
        ),
        (
            "qwen38.model.official-revision",
            cfg.lock.backend.official_model_revision.as_str(),
        ),
        (
            "qwen38.model.correction",
            cfg.lock.backend.model_correction.as_str(),
        ),
        (
            "qwen38.model.sha256",
            cfg.lock.backend.model_sha256.as_str(),
        ),
        (
            "qwen38.model.manifest.sha256",
            cfg.lock.backend.model_manifest_sha256.as_str(),
        ),
        (
            "org.opencontainers.image.revision",
            cfg.lock.backend.vllm_commit.as_str(),
        ),
    ] {
        require_equal(
            &format!("backend label {key}"),
            labels.get(key).map(String::as_str).unwrap_or("<missing>"),
            expected,
        )?;
    }
    Ok(())
}

fn verify_backend_cache(
    cfg: &Config,
    value: &serde_json::Value,
    owner_mode: &str,
) -> ServiceResult<()> {
    require_equal(
        "backend cache-volume name",
        value
            .get("Name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("<missing>"),
        &cfg.lock.backend.cache_volume,
    )?;
    let labels = value
        .get("Labels")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| ServiceError::Internal("backend cache volume lacks labels".into()))?;
    for (key, expected) in [
        ("qwen38.project", cfg.lock.backend.project_label.as_str()),
        (
            "qwen38.runtime.profile",
            cfg.lock.backend.profile_label.as_str(),
        ),
        (
            "qwen38.model.revision",
            cfg.lock.backend.model_revision.as_str(),
        ),
        (
            "qwen38.model.correction",
            cfg.lock.backend.model_correction.as_str(),
        ),
        (
            "qwen38.model.sha256",
            cfg.lock.backend.model_sha256.as_str(),
        ),
    ] {
        require_equal(
            &format!("backend cache-volume label {key}"),
            labels
                .get(key)
                .and_then(serde_json::Value::as_str)
                .unwrap_or("<missing>"),
            expected,
        )?;
    }
    if labels.len() != 5 {
        return Err(ServiceError::Internal(format!(
            "backend cache volume must have exactly five provenance labels, observed {labels:?}"
        )));
    }
    require_equal(
        "backend mounted cache owner/mode",
        owner_mode,
        &cfg.lock.backend.cache_owner_mode,
    )
}

async fn verify_backend_http(cfg: &Config) -> ServiceResult<()> {
    let version: serde_json::Value =
        curl_json(cfg, &format!("{}/version", cfg.vllm_endpoint), None).await?;
    require_equal(
        "live vLLM version",
        version
            .get("version")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("<missing>"),
        &cfg.lock.backend.version,
    )?;
    let models = curl_json(cfg, &format!("{}/v1/models", cfg.vllm_endpoint), None).await?;
    let data = models
        .get("data")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| ServiceError::Internal("vLLM /v1/models lacks data array".into()))?;
    if data.len() != 1
        || data[0].get("id").and_then(serde_json::Value::as_str)
            != Some(cfg.lock.backend.served_model.as_str())
        || data[0]
            .get("max_model_len")
            .and_then(serde_json::Value::as_u64)
            != Some(cfg.lock.backend.max_model_len)
    {
        return Err(ServiceError::Internal(format!(
            "vLLM model inventory violates the one-model contract: {models}"
        )));
    }
    let tokenize_body = serde_json::json!({
        "model": cfg.lock.backend.served_model,
        "prompt": "agent-service-exact-tokenizer-preflight"
    });
    let tokenize = curl_json(
        cfg,
        &format!("{}/tokenize", cfg.vllm_endpoint),
        Some(tokenize_body.to_string()),
    )
    .await?;
    if tokenize
        .get("count")
        .and_then(serde_json::Value::as_u64)
        .is_none_or(|v| v == 0)
        || tokenize
            .get("max_model_len")
            .and_then(serde_json::Value::as_u64)
            != Some(cfg.lock.backend.max_model_len)
        || !tokenize
            .get("tokens")
            .is_some_and(serde_json::Value::is_array)
    {
        return Err(ServiceError::Internal(format!(
            "vLLM real tokenizer preflight violates contract: {tokenize}"
        )));
    }
    Ok(())
}

async fn curl_json(
    cfg: &Config,
    url: &str,
    body: Option<String>,
) -> ServiceResult<serde_json::Value> {
    use std::process::Stdio;
    let mut command = tokio::process::Command::new("curl");
    command.args([
        "--fail-with-body",
        "--silent",
        "--show-error",
        "--connect-timeout",
        "2",
        "--max-time",
        "30",
        "--header",
        "content-type: application/json",
    ]);
    command.arg("--unix-socket").arg(&cfg.model_socket);
    if let Some(body) = &body {
        command.args(["--data-binary", body]);
    }
    command
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = command.output().await.map_err(|error| {
        ServiceError::Internal(format!("cannot execute pinned curl for {url}: {error}"))
    })?;
    if !output.status.success() {
        return Err(ServiceError::Internal(format!(
            "curl preflight {url} exited {:?}; stderr: {}; body: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim(),
            String::from_utf8_lossy(&output.stdout).trim()
        )));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| ServiceError::Internal(format!("{url} returned invalid JSON: {error}")))
}

/// Reconcile every leftover `<state_dir>/sessions/<id>/` directory.
/// Ordinary abandoned state is removed, explicitly retained raw evidence is
/// preserved, and ambiguous/incomplete terminalization refuses startup.
/// Idempotent: OK if the parent doesn't exist yet.
///
/// Defensive against accidents:
/// - read `state_dir/sessions/` only — never `state_dir` itself or anything
///   outside it.
/// - use `symlink_metadata` so we don't follow symlinks out of bounds.
/// - reject non-directories. A stray entry makes ownership ambiguous, so
///   startup stops instead of announcing partial cleanup.
fn sweep_state_dir(
    state_dir: &std::path::Path,
    results_dir: &std::path::Path,
    service_uid: u32,
    service_gid: u32,
) -> ServiceResult<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let sessions_dir = state_dir.join("sessions");
    let entries = match std::fs::read_dir(&sessions_dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(ServiceError::Internal(format!(
                "sweep_state_dir: cannot read {}: {e}",
                sessions_dir.display()
            )));
        }
    };
    let mut removed = 0u32;
    for entry in entries {
        let entry = entry.map_err(|e| {
            ServiceError::Internal(format!(
                "sweep_state_dir: cannot read an entry under {}: {e}",
                sessions_dir.display()
            ))
        })?;
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path).map_err(|e| {
            ServiceError::Internal(format!(
                "sweep_state_dir: cannot stat {}: {e}",
                path.display()
            ))
        })?;
        if !meta.is_dir()
            || meta.file_type().is_symlink()
            || meta.permissions().mode() & 0o777 != 0o755
            || meta.uid() != service_uid
            || meta.gid() != service_gid
        {
            return Err(ServiceError::Internal(format!(
                "sweep_state_dir: unexpected or unsafe session entry {}; expected ordinary mode-0755 owner={}:{}",
                path.display(), service_uid, service_gid
            )));
        }
        let name = path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .ok_or_else(|| {
                ServiceError::Internal(format!(
                    "sweep_state_dir: non-UTF-8 session directory {}",
                    path.display()
                ))
            })?;
        if !crate::runtime::is_safe_session_id(name) {
            return Err(ServiceError::Internal(format!(
                "sweep_state_dir: unexpected session directory name {name:?}"
            )));
        }
        let result_dir = results_dir.join(name);
        let terminal = committed_terminal_for_sweep(&result_dir, name, service_uid, service_gid)?;
        if let Some(body) = &terminal {
            validate_terminal_storage(&result_dir, body, service_uid, service_gid)?;
        }
        let marker = path.join("control/raw-evidence-retained.txt");
        let marker_cause = match std::fs::symlink_metadata(&marker) {
            Ok(metadata)
                if metadata.is_file()
                    && !metadata.file_type().is_symlink()
                    && metadata.permissions().mode() & 0o777 == 0o600
                    && metadata.uid() == service_uid
                    && metadata.gid() == service_gid =>
            {
                Some(validate_raw_evidence_marker(&marker, &path)?)
            }
            Ok(metadata) => {
                return Err(ServiceError::Internal(format!(
                    "sweep_state_dir: raw-evidence marker {} has unsafe type/mode/owner: type={:?} mode={:o} uid={} gid={} expected={}:{}",
                    marker.display(),
                    metadata.file_type(),
                    metadata.permissions().mode() & 0o777,
                    metadata.uid(),
                    metadata.gid(),
                    service_uid,
                    service_gid,
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(ServiceError::Internal(format!(
                    "sweep_state_dir: cannot stat raw-evidence marker {}: {error}",
                    marker.display()
                )));
            }
        };
        if let (Some(body), Some(cause)) = (&terminal, marker_cause) {
            validate_raw_cause_for_terminal(cause, body, "sweep_state_dir")?;
        }
        match terminal {
            Some(body) if body.raw_session_tree_retained && marker_cause.is_some() => {
                tracing::warn!(
                    session_id = name,
                    dir = %path.display(),
                    "sweep_state_dir: preserving explicitly retained raw forensic tree"
                );
                continue;
            }
            Some(body) if body.raw_session_tree_retained => {
                return Err(ServiceError::Internal(format!(
                    "sweep_state_dir: terminal {name} claims retained raw evidence but its service-owned marker is absent at {}",
                    marker.display()
                )));
            }
            Some(_) if marker_cause.is_some() => {
                return Err(ServiceError::Internal(format!(
                    "sweep_state_dir: session {name} has a raw-evidence marker but its terminal record does not claim retention"
                )));
            }
            None if marker_cause.is_some() => {
                return Err(ServiceError::Internal(format!(
                    "sweep_state_dir: session {name} has retained raw evidence but no committed terminal record; preserving {} and refusing startup so the operator can recover it explicitly",
                    path.display()
                )));
            }
            Some(_) => {}
            None => {
                // A result directory created during bundling/terminalization
                // is evidence, even if no terminal file became recoverable.
                // Never delete the corresponding raw state first: that would
                // turn a process crash into irreversible data loss.
                match std::fs::read_dir(&result_dir) {
                    Ok(mut entries) => {
                        if entries.next().transpose().map_err(|error| {
                            ServiceError::Internal(format!(
                                "sweep_state_dir: cannot inspect incomplete result directory {}: {error}",
                                result_dir.display()
                            ))
                        })?.is_some() {
                            return Err(ServiceError::Internal(format!(
                                "sweep_state_dir: session {name} has raw state and a nonempty crash-interrupted result directory but no recoverable terminal; preserving both {} and {} for explicit recovery",
                                path.display(),
                                result_dir.display()
                            )));
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(ServiceError::Internal(format!(
                            "sweep_state_dir: cannot read possible result directory {}: {error}",
                            result_dir.display()
                        )));
                    }
                }
            }
        }
        std::fs::remove_dir_all(&path).map_err(|e| {
            ServiceError::Internal(format!(
                "sweep_state_dir: cannot remove leftover {}: {e}",
                path.display()
            ))
        })?;
        removed += 1;
        tracing::info!(dir = %path.display(), "sweep_state_dir: removed leftover");
    }
    if removed > 0 {
        tracing::info!(count = removed, "sweep_state_dir: complete");
    }
    Ok(())
}

/// Reconcile `<results_dir>/<id>/` directories and terminal publications.
///
/// A completely empty result directory has no evidence and may be removed.
/// A nonempty directory without a recoverable terminal is preserved and blocks
/// readiness so a process crash cannot silently discard a bundle or partial
/// forensic record. A complete private terminal draft is finished through the
/// same no-clobber hard-link publication used during normal operation.
///
/// A directory with `finished.json` is retained only after validating its
/// name, file type, terminal JSON shape, and session identity.
fn sweep_partial_results(
    results_dir: &std::path::Path,
    state_dir: &std::path::Path,
    service_uid: u32,
    service_gid: u32,
) -> ServiceResult<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let entries = match std::fs::read_dir(results_dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(ServiceError::Internal(format!(
                "sweep_partial_results: cannot read {}: {e}",
                results_dir.display()
            )));
        }
    };
    let mut removed = 0u32;
    for entry in entries {
        let entry = entry.map_err(|e| {
            ServiceError::Internal(format!(
                "sweep_partial_results: cannot read an entry under {}: {e}",
                results_dir.display()
            ))
        })?;
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path).map_err(|e| {
            ServiceError::Internal(format!(
                "sweep_partial_results: cannot stat {}: {e}",
                path.display()
            ))
        })?;
        if !meta.is_dir()
            || meta.file_type().is_symlink()
            || meta.permissions().mode() & 0o777 != 0o755
            || meta.uid() != service_uid
            || meta.gid() != service_gid
        {
            return Err(ServiceError::Internal(format!(
                "sweep_partial_results: unexpected or unsafe result entry {}; expected ordinary mode-0755 owner={}:{}",
                path.display(), service_uid, service_gid
            )));
        }
        let name = path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .ok_or_else(|| {
                ServiceError::Internal(format!(
                    "sweep_partial_results: non-UTF-8 result directory {}",
                    path.display()
                ))
            })?;
        if !crate::runtime::is_safe_session_id(name) {
            return Err(ServiceError::Internal(format!(
                "sweep_partial_results: unexpected result directory name {name:?}"
            )));
        }
        if let Some(body) = committed_terminal_for_sweep(&path, name, service_uid, service_gid)? {
            validate_terminal_storage(&path, &body, service_uid, service_gid)?;
            validate_terminal_state_reconciliation(state_dir, &body, service_uid, service_gid)?;
            continue;
        }
        let mut remaining = std::fs::read_dir(&path).map_err(|error| {
            ServiceError::Internal(format!(
                "sweep_partial_results: cannot inspect {} before cleanup: {error}",
                path.display()
            ))
        })?;
        if remaining
            .next()
            .transpose()
            .map_err(|error| {
                ServiceError::Internal(format!(
                    "sweep_partial_results: cannot inspect an entry in {}: {error}",
                    path.display()
                ))
            })?
            .is_some()
        {
            return Err(ServiceError::Internal(format!(
                "sweep_partial_results: nonempty crash-interrupted result directory {} has no recoverable terminal; preserving evidence and refusing startup",
                path.display()
            )));
        }
        std::fs::remove_dir_all(&path).map_err(|e| {
            ServiceError::Internal(format!(
                "sweep_partial_results: cannot remove crash-interrupted session {}: {e}",
                path.display()
            ))
        })?;
        removed += 1;
        tracing::info!(
            dir = %path.display(),
            "sweep_partial_results: removed crash-interrupted session dir"
        );
    }
    if removed > 0 {
        tracing::info!(
            count = removed,
            "sweep_partial_results: complete (these sessions were interrupted by a server crash)"
        );
    }
    Ok(())
}

fn validate_terminal_state_reconciliation(
    state_dir: &std::path::Path,
    body: &crate::runtime::SessionBody,
    service_uid: u32,
    service_gid: u32,
) -> ServiceResult<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let state_root = state_dir.join("sessions").join(&body.session_id);
    let metadata = match std::fs::symlink_metadata(&state_root) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(ServiceError::Internal(format!(
                "terminal reconciliation: cannot stat {}: {error}",
                state_root.display()
            )));
        }
    };
    if !body.raw_session_tree_retained {
        if metadata.is_some() {
            return Err(ServiceError::Internal(format!(
                "terminal reconciliation: non-retained terminal {} still has raw state at {} after the state sweep",
                body.session_id,
                state_root.display()
            )));
        }
        return Ok(());
    }
    let metadata = metadata.ok_or_else(|| {
        ServiceError::Internal(format!(
            "terminal reconciliation: terminal {} claims retained raw evidence but {} is absent",
            body.session_id,
            state_root.display()
        ))
    })?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o777 != 0o755
        || metadata.uid() != service_uid
        || metadata.gid() != service_gid
    {
        return Err(ServiceError::Internal(format!(
            "terminal reconciliation: retained state {} has unsafe type/mode/owner",
            state_root.display()
        )));
    }
    let marker = state_root.join("control/raw-evidence-retained.txt");
    let marker_metadata = std::fs::symlink_metadata(&marker).map_err(|error| {
        ServiceError::Internal(format!(
            "terminal reconciliation: terminal {} claims retained raw evidence but its marker {} is absent or unreadable: {error}",
            body.session_id,
            marker.display()
        ))
    })?;
    validate_private_service_file(
        &marker,
        &marker_metadata,
        service_uid,
        service_gid,
        "raw-evidence marker",
    )?;
    let cause = validate_raw_evidence_marker(&marker, &state_root)?;
    validate_raw_cause_for_terminal(cause, body, "terminal reconciliation")
}

fn committed_terminal_for_sweep(
    result_dir: &std::path::Path,
    session_id: &str,
    service_uid: u32,
    service_gid: u32,
) -> ServiceResult<Option<crate::runtime::SessionBody>> {
    use std::os::unix::fs::MetadataExt;

    let finished = result_dir.join("finished.json");
    let temporary = result_dir.join("finished.json.tmp");
    let final_meta = match std::fs::symlink_metadata(&finished) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(ServiceError::Internal(format!(
                "terminal sweep: cannot stat {}: {error}",
                finished.display()
            )));
        }
    };
    let temporary_meta = match std::fs::symlink_metadata(&temporary) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(ServiceError::Internal(format!(
                "terminal sweep: cannot stat {}: {error}",
                temporary.display()
            )));
        }
    };
    if let Some(metadata) = &final_meta {
        validate_private_service_file(
            &finished,
            metadata,
            service_uid,
            service_gid,
            "committed terminal",
        )?;
    }
    if let Some(metadata) = &temporary_meta {
        validate_private_service_file(
            &temporary,
            metadata,
            service_uid,
            service_gid,
            "temporary terminal publication",
        )?;
    }

    let terminal_path = if final_meta.is_some() {
        &finished
    } else if temporary_meta.is_some() {
        &temporary
    } else {
        return Ok(None);
    };
    let bytes = std::fs::read(terminal_path).map_err(|error| {
        ServiceError::Internal(format!(
            "terminal sweep: cannot read {}: {error}",
            terminal_path.display()
        ))
    })?;
    let body: crate::runtime::SessionBody = serde_json::from_slice(&bytes).map_err(|error| {
        ServiceError::Internal(format!(
            "terminal sweep: terminal record {} is malformed: {error}",
            terminal_path.display()
        ))
    })?;
    if body.session_id != session_id || body.status == crate::runtime::SessionStatus::Running {
        return Err(ServiceError::Internal(format!(
            "terminal sweep: identity/status mismatch in {}",
            terminal_path.display()
        )));
    }

    match (&final_meta, &temporary_meta) {
        (Some(final_meta), Some(temporary_meta)) => {
            if temporary_meta.dev() != final_meta.dev() || temporary_meta.ino() != final_meta.ino()
            {
                return Err(ServiceError::Internal(format!(
                    "terminal sweep: {} is not the same regular hard-linked inode as {}; refusing ambiguous recovery",
                    temporary.display(),
                    finished.display()
                )));
            }
            std::fs::remove_file(&temporary).map_err(|error| {
                ServiceError::Internal(format!(
                    "terminal sweep: remove safely recoverable publication marker {}: {error}",
                    temporary.display()
                ))
            })?;
            sync_directory(
                result_dir,
                "terminal sweep after linked-publication recovery",
            )?;
            tracing::warn!(
                session_id,
                marker = %temporary.display(),
                "terminal sweep: recovered completed no-clobber publication after crash"
            );
        }
        (None, Some(_)) => {
            // A private, fully parseable temporary terminal is the durable
            // prepare phase of no-clobber publication. Validate its referenced
            // storage before making it visible, then complete the exact
            // hard-link transaction. A torn/partial file cannot parse and is
            // preserved as a startup error above.
            validate_terminal_storage(result_dir, &body, service_uid, service_gid)?;
            std::fs::hard_link(&temporary, &finished).map_err(|error| {
                ServiceError::Internal(format!(
                    "terminal sweep: complete prepared publication {} -> {}: {error}",
                    temporary.display(),
                    finished.display()
                ))
            })?;
            std::fs::remove_file(&temporary).map_err(|error| {
                ServiceError::Internal(format!(
                    "terminal sweep: remove recovered temporary publication {}: {error}",
                    temporary.display()
                ))
            })?;
            sync_directory(
                result_dir,
                "terminal sweep after prepared-publication recovery",
            )?;
            tracing::warn!(
                session_id,
                terminal = %finished.display(),
                "terminal sweep: completed durable prepared terminal publication after crash"
            );
        }
        (Some(_), None) => {}
        (None, None) => unreachable!("terminal path selection proved one file exists"),
    }
    Ok(Some(body))
}

fn validate_private_service_file(
    path: &std::path::Path,
    metadata: &std::fs::Metadata,
    service_uid: u32,
    service_gid: u32,
    role: &str,
) -> ServiceResult<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o777 != 0o600
        || metadata.uid() != service_uid
        || metadata.gid() != service_gid
    {
        return Err(ServiceError::Internal(format!(
            "terminal sweep: {role} {} has unsafe type/mode/owner: type={:?} mode={:o} uid={} gid={} expected={}:{}",
            path.display(),
            metadata.file_type(),
            metadata.permissions().mode() & 0o777,
            metadata.uid(),
            metadata.gid(),
            service_uid,
            service_gid,
        )));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RawEvidenceCause {
    RequiredBundleFailure,
    PanicRecoveryBundleFailure,
    SetupForensicBundleFailure,
    ContainerQuiescenceUnproved,
    ContainerTeardownIncomplete,
}

fn validate_raw_evidence_marker(
    marker: &std::path::Path,
    session_root: &std::path::Path,
) -> ServiceResult<RawEvidenceCause> {
    let contents = std::fs::read_to_string(marker).map_err(|error| {
        ServiceError::Internal(format!(
            "sweep_state_dir: cannot read raw-evidence marker {}: {error}",
            marker.display()
        ))
    })?;
    let mut lines = contents.lines();
    if lines.next() != Some("RAW_SESSION_TREE_RETAINED") {
        return Err(ServiceError::Internal(format!(
            "sweep_state_dir: raw-evidence marker {} lacks the exact authority header",
            marker.display()
        )));
    }
    let cause = lines.next().unwrap_or_default();
    let cause = match cause {
        "cause=required-bundle-failure" => RawEvidenceCause::RequiredBundleFailure,
        "cause=panic-recovery-bundle-failure" => RawEvidenceCause::PanicRecoveryBundleFailure,
        "cause=setup-forensic-bundle-failure" => RawEvidenceCause::SetupForensicBundleFailure,
        "cause=container-quiescence-unproved" => RawEvidenceCause::ContainerQuiescenceUnproved,
        "cause=container-teardown-incomplete" => RawEvidenceCause::ContainerTeardownIncomplete,
        unknown => {
            return Err(ServiceError::Internal(format!(
                "sweep_state_dir: raw-evidence marker {} has an unknown cause {unknown:?}",
                marker.display()
            )));
        }
    };
    let expected_path = format!("path={}", session_root.display());
    if lines.next() != Some(expected_path.as_str()) || lines.next().is_some() {
        return Err(ServiceError::Internal(format!(
            "sweep_state_dir: raw-evidence marker {} has a wrong path or trailing fields; expected {:?}",
            marker.display(),
            expected_path
        )));
    }
    Ok(cause)
}

fn validate_raw_cause_for_terminal(
    cause: RawEvidenceCause,
    body: &crate::runtime::SessionBody,
    context: &str,
) -> ServiceResult<()> {
    let accepted_bundle = !body.bundle_archive_path.is_empty();
    let shape_valid = if accepted_bundle {
        cause == RawEvidenceCause::ContainerTeardownIncomplete
    } else {
        matches!(
            cause,
            RawEvidenceCause::RequiredBundleFailure
                | RawEvidenceCause::PanicRecoveryBundleFailure
                | RawEvidenceCause::SetupForensicBundleFailure
                | RawEvidenceCause::ContainerQuiescenceUnproved
        )
    };
    if !shape_valid {
        return Err(ServiceError::Internal(format!(
            "{context}: terminal {} raw-evidence cause {cause:?} contradicts accepted_bundle={accepted_bundle}",
            body.session_id
        )));
    }
    Ok(())
}

fn require_owned_runtime_directory(
    path: &std::path::Path,
    mode: u32,
    service_uid: u32,
    service_gid: u32,
    create_if_missing: bool,
) -> ServiceResult<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};

    match std::fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && create_if_missing => {
            let parent = path.parent().ok_or_else(|| {
                ServiceError::Internal(format!(
                    "runtime directory {} has no parent",
                    path.display()
                ))
            })?;
            std::fs::DirBuilder::new()
                .mode(mode)
                .create(path)
                .map_err(|create_error| {
                    ServiceError::Internal(format!(
                        "exclusively create fixed runtime directory {}: {create_error}",
                        path.display()
                    ))
                })?;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).map_err(
                |chmod_error| {
                    ServiceError::Internal(format!(
                        "set exact mode on new runtime directory {}: {chmod_error}",
                        path.display()
                    ))
                },
            )?;
            sync_directory(path, "sync new fixed runtime directory")?;
            sync_directory(parent, "sync fixed runtime-directory publication")?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ServiceError::Internal(format!(
                "required runtime directory {} is absent; use the pinned start script",
                path.display()
            )));
        }
        Err(error) => {
            return Err(ServiceError::Internal(format!(
                "stat required runtime directory {}: {error}",
                path.display()
            )));
        }
    }
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        ServiceError::Internal(format!(
            "restat required runtime directory {}: {error}",
            path.display()
        ))
    })?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != service_uid
        || metadata.gid() != service_gid
        || metadata.permissions().mode() & 0o777 != mode
    {
        return Err(ServiceError::Internal(format!(
            "runtime directory contract drift at {}: type={:?} uid={} gid={} mode={:o}; expected ordinary directory {}:{} mode={mode:04o}",
            path.display(),
            metadata.file_type(),
            metadata.uid(),
            metadata.gid(),
            metadata.permissions().mode() & 0o777,
            service_uid,
            service_gid,
        )));
    }
    Ok(())
}

fn sync_directory(path: &std::path::Path, context: &str) -> ServiceResult<()> {
    std::fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            ServiceError::Internal(format!(
                "{context}: cannot sync directory {}: {error}",
                path.display()
            ))
        })
}

fn validate_terminal_storage(
    result_dir: &std::path::Path,
    body: &crate::runtime::SessionBody,
    service_uid: u32,
    service_gid: u32,
) -> ServiceResult<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let archive = result_dir.join("bundle.tar.zst");
    if body.raw_session_tree_retained && body.bundle_archive_path.is_empty() {
        if body.bundle_compressed_bytes != 0
            || body.bundle_uncompressed_bytes != 0
            || body.bundle_file_count != 0
            || body.bundle_artifacts_file_count != 0
        {
            return Err(ServiceError::Internal(format!(
                "terminal {} retains raw evidence after bundle failure but has nonzero bundle counters",
                body.session_id
            )));
        }
        // A failed bundle publication may deliberately leave a partial or
        // ambiguous archive as additional evidence. It is not accepted by the
        // terminal counters/path and is never deleted by startup.
        return Ok(());
    }
    if body.bundle_archive_path.is_empty() {
        if body.bundle_compressed_bytes != 0
            || body.bundle_uncompressed_bytes != 0
            || body.bundle_file_count != 0
            || body.bundle_artifacts_file_count != 0
        {
            return Err(ServiceError::Internal(format!(
                "terminal {} has empty bundle path but nonzero bundle counters",
                body.session_id
            )));
        }
        match std::fs::symlink_metadata(&archive) {
            Ok(_) => {
                return Err(ServiceError::Internal(format!(
                    "terminal {} does not accept a bundle but {} exists",
                    body.session_id,
                    archive.display()
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(ServiceError::Internal(format!(
                    "terminal {} cannot prove bundle absence at {}: {error}",
                    body.session_id,
                    archive.display()
                )));
            }
        }
        return Ok(());
    }
    if body.bundle_archive_path != archive.display().to_string() {
        return Err(ServiceError::Internal(format!(
            "terminal {} bundle path drift: expected {}, observed {:?}",
            body.session_id,
            archive.display(),
            body.bundle_archive_path
        )));
    }
    let metadata = std::fs::symlink_metadata(&archive).map_err(|error| {
        ServiceError::Internal(format!(
            "terminal {} accepted bundle cannot be stat'd at {}: {error}",
            body.session_id,
            archive.display()
        ))
    })?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o777 != 0o600
        || metadata.uid() != service_uid
        || metadata.gid() != service_gid
        || metadata.len() == 0
        || metadata.len() != body.bundle_compressed_bytes
    {
        return Err(ServiceError::Internal(format!(
            "terminal {} accepted bundle metadata drift at {}: type={:?} mode={:o} uid={} gid={} expected_owner={}:{} size={} recorded={}",
            body.session_id,
            archive.display(),
            metadata.file_type(),
            metadata.permissions().mode() & 0o777,
            metadata.uid(),
            metadata.gid(),
            service_uid,
            service_gid,
            metadata.len(),
            body.bundle_compressed_bytes
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
    use std::path::{Path, PathBuf};

    use super::{
        committed_terminal_for_sweep, require_owned_runtime_directory, sweep_partial_results,
        sweep_state_dir, validate_terminal_storage, CreateRequest,
    };
    use crate::runtime::{SessionBody, SessionStatus};

    struct TestTree(PathBuf);

    impl TestTree {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "qwen38-api-{label}-{}",
                uuid::Uuid::new_v4().simple()
            ));
            std::fs::create_dir(&path).expect("create isolated test root");
            Self(path)
        }
    }

    impl Drop for TestTree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn owner() -> (u32, u32) {
        // Newly created test files are owned by the effective process. The
        // parent temp directory may have another owner, so use a fresh file.
        let probe = std::env::temp_dir().join(format!(
            "qwen38-owner-probe-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::write(&probe, b"probe").expect("create owner probe");
        let metadata = std::fs::metadata(&probe).expect("stat owner probe");
        std::fs::remove_file(&probe).expect("remove owner probe");
        (metadata.uid(), metadata.gid())
    }

    fn private_write(path: &Path, bytes: &[u8]) {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;

        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .expect("create private fixture");
        file.write_all(bytes).expect("write private fixture");
        file.sync_all().expect("sync private fixture");
    }

    fn mkdir_0755(path: &Path) {
        std::fs::create_dir_all(path).expect("create fixture directory");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fixture directory");
    }

    fn terminal(session_id: &str, raw_retained: bool) -> SessionBody {
        SessionBody {
            session_id: session_id.to_string(),
            status: SessionStatus::Completed,
            started_at_unix: 1,
            model: "qwen3.8-27b-nvfp4-k8v4".to_string(),
            context_window: 262_144,
            prompt_preview: "fixture".to_string(),
            num_turns: 0,
            last_event_at_unix: 0,
            finished_at_unix: 2,
            duration_wall_ms: 1,
            container_exit_code: -1,
            agent_exit_code: -1,
            is_process_error: true,
            response: "fixture terminal".to_string(),
            agent_duration_ms: 0,
            bundle_archive_path: String::new(),
            bundle_compressed_bytes: 0,
            bundle_uncompressed_bytes: 0,
            bundle_file_count: 0,
            bundle_artifacts_file_count: 0,
            raw_session_tree_retained: raw_retained,
            teardown_diagnostics: Vec::new(),
        }
    }

    #[test]
    fn runtime_directory_preflight_creates_only_the_fixed_missing_child() {
        let tree = TestTree::new("runtime-directories");
        let (uid, gid) = owner();
        std::fs::set_permissions(&tree.0, std::fs::Permissions::from_mode(0o700))
            .expect("chmod runtime root fixture");
        let sessions = tree.0.join("sessions");
        require_owned_runtime_directory(&sessions, 0o700, uid, gid, true)
            .expect("create exact missing sessions directory");
        require_owned_runtime_directory(&sessions, 0o700, uid, gid, true)
            .expect("validate existing exact sessions directory");

        std::fs::set_permissions(&sessions, std::fs::Permissions::from_mode(0o755))
            .expect("drift sessions mode");
        assert!(
            require_owned_runtime_directory(&sessions, 0o700, uid, gid, true).is_err(),
            "runtime preflight silently chmodded an existing drifted directory"
        );

        let outside = tree.0.join("outside");
        std::fs::create_dir(&outside).expect("create outside runtime fixture");
        let hostile = tree.0.join("hostile-runtime-root");
        symlink(&outside, &hostile).expect("create hostile runtime symlink");
        assert!(
            require_owned_runtime_directory(&hostile, 0o700, uid, gid, true).is_err(),
            "runtime preflight followed or adopted a symlink"
        );
        assert!(
            require_owned_runtime_directory(
                &tree.0.join("required-but-missing"),
                0o700,
                uid,
                gid,
                false,
            )
            .is_err(),
            "runtime preflight silently created a host-launcher-owned root"
        );
    }

    fn write_terminal(path: &Path, body: &SessionBody) {
        private_write(
            path,
            &serde_json::to_vec_pretty(body).expect("serialize fixture terminal"),
        );
    }

    #[test]
    fn create_request_rejects_unknown_fields() {
        let result = serde_json::from_str::<CreateRequest>(
            r#"{"prompt":"do work","folder":"/home/user/project","fallback":true}"#,
        );
        let error = result.expect_err("unknown request fields must fail closed");
        assert!(error.to_string().contains("unknown field `fallback`"));
    }

    #[test]
    fn prepared_terminal_is_recovered_by_exact_no_clobber_publication() {
        let tree = TestTree::new("prepared-terminal");
        let result_dir = tree.0.join("s-11111111111111111111111111111111");
        mkdir_0755(&result_dir);
        let body = terminal("s-11111111111111111111111111111111", false);
        write_terminal(&result_dir.join("finished.json.tmp"), &body);
        let (uid, gid) = owner();

        let recovered = committed_terminal_for_sweep(&result_dir, &body.session_id, uid, gid)
            .expect("recover prepared terminal")
            .expect("terminal exists");
        assert_eq!(recovered.session_id, body.session_id);
        assert!(result_dir.join("finished.json").is_file());
        assert!(!result_dir.join("finished.json.tmp").exists());
    }

    #[test]
    fn ambiguous_dual_terminal_publication_is_preserved_and_refused() {
        let tree = TestTree::new("ambiguous-terminal");
        let result_dir = tree.0.join("s-22222222222222222222222222222222");
        mkdir_0755(&result_dir);
        let body = terminal("s-22222222222222222222222222222222", false);
        write_terminal(&result_dir.join("finished.json"), &body);
        write_terminal(&result_dir.join("finished.json.tmp"), &body);
        let (uid, gid) = owner();

        let error = committed_terminal_for_sweep(&result_dir, &body.session_id, uid, gid)
            .expect_err("different inodes must not be guessed equivalent");
        assert!(error
            .to_string()
            .contains("not the same regular hard-linked inode"));
        assert!(result_dir.join("finished.json").exists());
        assert!(result_dir.join("finished.json.tmp").exists());
    }

    #[test]
    fn retained_raw_state_requires_matching_terminal_and_authority_marker() {
        let tree = TestTree::new("retained-raw");
        let state = tree.0.join("state");
        let results = tree.0.join("results");
        let session_id = "s-33333333333333333333333333333333";
        let state_root = state.join("sessions").join(session_id);
        let control = state_root.join("control");
        let result_dir = results.join(session_id);
        mkdir_0755(&control);
        mkdir_0755(&result_dir);
        let body = terminal(session_id, true);
        write_terminal(&result_dir.join("finished.json"), &body);
        private_write(
            &control.join("raw-evidence-retained.txt"),
            format!(
                "RAW_SESSION_TREE_RETAINED\ncause=required-bundle-failure\npath={}\n",
                state_root.display()
            )
            .as_bytes(),
        );
        let (uid, gid) = owner();

        sweep_state_dir(&state, &results, uid, gid).expect("preserve authorized raw state");
        assert!(state_root.is_dir());
        sweep_partial_results(&results, &state, uid, gid).expect("retain valid terminal result");

        std::fs::write(
            control.join("raw-evidence-retained.txt"),
            format!(
                "RAW_SESSION_TREE_RETAINED\ncause=container-teardown-incomplete\npath={}\n",
                state_root.display()
            ),
        )
        .expect("write contradictory marker fixture");
        let error = sweep_state_dir(&state, &results, uid, gid)
            .expect_err("teardown marker without an accepted bundle must fail closed");
        assert!(error
            .to_string()
            .contains("contradicts accepted_bundle=false"));
        assert!(state_root.is_dir());

        std::fs::write(
            control.join("raw-evidence-retained.txt"),
            b"RAW_SESSION_TREE_RETAINED\ncause=unknown\npath=/wrong\n",
        )
        .expect("corrupt marker fixture");
        let error = sweep_state_dir(&state, &results, uid, gid)
            .expect_err("corrupt authority marker must fail closed");
        assert!(error.to_string().contains("unknown cause"));
        assert!(state_root.is_dir());
    }

    #[test]
    fn retained_terminal_without_raw_tree_is_a_startup_contradiction() {
        let tree = TestTree::new("missing-retained-raw");
        let state = tree.0.join("state");
        let results = tree.0.join("results");
        let session_id = "s-88888888888888888888888888888888";
        let result_dir = results.join(session_id);
        mkdir_0755(&result_dir);
        write_terminal(
            &result_dir.join("finished.json"),
            &terminal(session_id, true),
        );
        let (uid, gid) = owner();

        let error = sweep_partial_results(&results, &state, uid, gid)
            .expect_err("raw-retention claim without raw state must fail");
        assert!(error.to_string().contains("claims retained raw evidence"));
        assert!(result_dir.join("finished.json").is_file());
    }

    #[test]
    fn nonempty_partial_results_are_preserved_not_swept() {
        let tree = TestTree::new("partial-result");
        let results = tree.0.join("results");
        let result_dir = results.join("s-44444444444444444444444444444444");
        mkdir_0755(&result_dir);
        private_write(&result_dir.join("bundle.tar.zst.partial"), b"evidence");
        let (uid, gid) = owner();

        let error = sweep_partial_results(&results, &tree.0.join("state"), uid, gid)
            .expect_err("nonempty partial result must be preserved");
        assert!(error.to_string().contains("preserving evidence"));
        assert!(result_dir.join("bundle.tar.zst.partial").is_file());
    }

    #[test]
    fn ordinary_unretained_empty_state_is_safely_removed() {
        let tree = TestTree::new("empty-state");
        let state = tree.0.join("state");
        let results = tree.0.join("results");
        let state_root = state
            .join("sessions")
            .join("s-55555555555555555555555555555555");
        mkdir_0755(&state_root);
        mkdir_0755(&results);
        let (uid, gid) = owner();

        sweep_state_dir(&state, &results, uid, gid).expect("remove ordinary abandoned state");
        assert!(!state_root.exists());
    }

    #[test]
    fn terminal_bundle_metadata_is_exact_and_owner_checked() {
        let tree = TestTree::new("bundle-metadata");
        let result_dir = tree.0.join("s-66666666666666666666666666666666");
        mkdir_0755(&result_dir);
        let archive = result_dir.join("bundle.tar.zst");
        private_write(&archive, b"bundle-bytes");
        let mut body = terminal("s-66666666666666666666666666666666", false);
        body.bundle_archive_path = archive.display().to_string();
        body.bundle_compressed_bytes = b"bundle-bytes".len() as u64;
        body.bundle_uncompressed_bytes = 100;
        body.bundle_file_count = 2;
        let (uid, gid) = owner();

        validate_terminal_storage(&result_dir, &body, uid, gid)
            .expect("exact accepted bundle metadata");
        body.raw_session_tree_retained = true;
        validate_terminal_storage(&result_dir, &body, uid, gid)
            .expect("teardown-incomplete raw evidence may coexist with an exact bundle");
        body.raw_session_tree_retained = false;
        body.bundle_compressed_bytes += 1;
        let error = validate_terminal_storage(&result_dir, &body, uid, gid)
            .expect_err("bundle size drift must fail");
        assert!(error.to_string().contains("metadata drift"));
    }

    #[test]
    fn historical_terminal_without_raw_field_has_only_false_migration_value() {
        let body = terminal("s-77777777777777777777777777777777", false);
        let mut value = serde_json::to_value(&body).expect("serialize historical fixture");
        value
            .as_object_mut()
            .expect("terminal object")
            .remove("raw_session_tree_retained");
        let migrated: SessionBody = serde_json::from_value(value).expect("decode historical body");
        assert!(!migrated.raw_session_tree_retained);
    }
}
