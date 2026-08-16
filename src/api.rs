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
    let (size_bytes, file_count) = validation::enumerate_folder(&validated.folder)?;
    tracing::info!(
        prompt_chars = body.prompt.chars().count(),
        folder = %validated.folder.display(),
        size_bytes,
        file_count,
        "POST /v1/agent/sessions: pre-flight ok"
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
pub async fn pre_flight(cfg: &Config) -> ServiceResult<()> {
    let docker_version = crate::docker_ops::ping_daemon().await?;
    require_equal(
        "Docker server version",
        &docker_version,
        &cfg.lock.host.docker_version,
    )?;
    let agent_image_id = crate::docker_ops::image_id(&cfg.agent_image).await?;
    require_equal("agent image ID", &agent_image_id, &cfg.lock.agent.image_id)?;
    verify_agent_image_labels(cfg).await?;
    verify_service_container(cfg).await?;
    verify_backend_container(cfg).await?;
    verify_host_contract(cfg).await?;
    verify_model_manifest(cfg).await?;
    verify_backend_http(cfg).await?;
    verify_backend_listener().await?;
    crate::bundle::check_host_dependencies().await?;

    if let Err(e) = std::fs::create_dir_all(&cfg.state_dir) {
        return Err(ServiceError::Internal(format!(
            "cannot create AGENT_SERVICE_STATE_DIR at {}: {e}",
            cfg.state_dir.display()
        )));
    }
    if let Err(e) = std::fs::create_dir_all(&cfg.results_dir) {
        return Err(ServiceError::Internal(format!(
            "cannot create AGENT_SERVICE_RESULTS_DIR at {}: {e}",
            cfg.results_dir.display()
        )));
    }

    // Sweep any orphans from a prior crash before announcing ourselves.
    // Docker objects (containers), staging dirs, and any
    // crash-interrupted result directories (no `finished.json`). Sweeps
    // complete (or fail loudly) BEFORE the listener binds, so no incoming
    // request can land while a half-cleaned-up prior session exists.
    session::sweep_orphans().await?;
    sweep_state_dir(&cfg.state_dir)?;
    sweep_partial_results(&cfg.results_dir)?;

    Ok(())
}

async fn verify_service_container(cfg: &Config) -> ServiceResult<()> {
    let text = crate::docker_ops::run_docker(
        ["inspect", &cfg.lock.service.container_name],
        "inspect_agent_service_container",
    )
    .await?;
    let values: Vec<serde_json::Value> = serde_json::from_str(&text).map_err(|error| {
        ServiceError::Internal(format!("service docker inspect is invalid JSON: {error}"))
    })?;
    let value = values
        .first()
        .filter(|_| values.len() == 1)
        .ok_or_else(|| {
            ServiceError::Internal(format!(
                "service inspect returned {} objects, expected one",
                values.len()
            ))
        })?;
    let image = value
        .pointer("/Config/Image")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ServiceError::Internal("service inspect lacks Config.Image".into()))?;
    require_equal("service image tag", image, &cfg.lock.service.image_tag)?;
    let mode = value
        .pointer("/HostConfig/NetworkMode")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ServiceError::Internal("service inspect lacks network mode".into()))?;
    require_equal("service network mode", mode, "host")?;
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
    Ok(())
}

async fn verify_host_contract(cfg: &Config) -> ServiceResult<()> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let socket = std::fs::symlink_metadata(&cfg.lock.host.docker_socket).map_err(|error| {
        ServiceError::Internal(format!(
            "cannot stat pinned Docker socket {}: {error}",
            cfg.lock.host.docker_socket
        ))
    })?;
    if !socket.file_type().is_socket() || socket.gid() != cfg.lock.host.docker_socket_gid {
        return Err(ServiceError::Internal(format!(
            "Docker socket contract mismatch: is_socket={} gid={} expected_gid={}",
            socket.file_type().is_socket(),
            socket.gid(),
            cfg.lock.host.docker_socket_gid
        )));
    }
    let query = crate::docker_ops::run_docker(
        [
            "exec",
            &cfg.lock.backend.container_name,
            "nvidia-smi",
            "--query-gpu=name,memory.total,driver_version",
            "--format=csv,noheader,nounits",
        ],
        "verify_pinned_gpu",
    )
    .await?;
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

async fn verify_agent_image_labels(cfg: &Config) -> ServiceResult<()> {
    let labels_text = crate::docker_ops::run_docker(
        [
            "image",
            "inspect",
            "--format",
            "{{json .Config.Labels}}",
            &cfg.agent_image,
        ],
        "inspect_agent_image_labels",
    )
    .await?;
    let labels: std::collections::HashMap<String, String> =
        serde_json::from_str(labels_text.trim()).map_err(|error| {
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
            "agent_service.settings.sha256",
            cfg.lock.agent.settings_sha256.as_str(),
        ),
        (
            "agent_service.instructions.sha256",
            cfg.lock.agent.instructions_sha256.as_str(),
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

async fn verify_backend_container(cfg: &Config) -> ServiceResult<()> {
    let text = crate::docker_ops::run_docker(
        ["inspect", &cfg.lock.backend.container_name],
        "inspect_pinned_backend",
    )
    .await?;
    let values: Vec<serde_json::Value> = serde_json::from_str(&text).map_err(|error| {
        ServiceError::Internal(format!("backend docker inspect is invalid JSON: {error}"))
    })?;
    if values.len() != 1 {
        return Err(ServiceError::Internal(format!(
            "backend inspect returned {} objects, expected one",
            values.len()
        )));
    }
    let value = &values[0];
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
        "backend image ID",
        string("/Image")?,
        &cfg.lock.backend.image_id,
    )?;
    require_equal(
        "backend configured image tag",
        string("/Config/Image")?,
        &cfg.lock.backend.image_tag,
    )?;
    require_equal(
        "backend network mode",
        string("/HostConfig/NetworkMode")?,
        "host",
    )?;
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
    .map_err(|error| ServiceError::Internal(format!("backend environment shape invalid: {error}")))?;
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

async fn verify_backend_http(cfg: &Config) -> ServiceResult<()> {
    let version: serde_json::Value =
        curl_json(&format!("{}/version", cfg.vllm_endpoint), None).await?;
    require_equal(
        "live vLLM version",
        version
            .get("version")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("<missing>"),
        &cfg.lock.backend.version,
    )?;
    let models = curl_json(&format!("{}/v1/models", cfg.vllm_endpoint), None).await?;
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

async fn curl_json(url: &str, body: Option<String>) -> ServiceResult<serde_json::Value> {
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

async fn verify_backend_listener() -> ServiceResult<()> {
    use std::process::Stdio;
    let output = tokio::process::Command::new("ss")
        .args(["-H", "-ltn", "sport = :8000"])
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|error| ServiceError::Internal(format!("cannot execute pinned ss: {error}")))?;
    if !output.status.success() {
        return Err(ServiceError::Internal(format!(
            "ss listener preflight failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let lines = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>();
    if lines.len() != 1 || !lines[0].contains("127.0.0.1:8000") {
        return Err(ServiceError::Internal(format!(
            "expected exactly one 127.0.0.1:8000 listener, observed {lines:?}"
        )));
    }
    Ok(())
}

/// Remove every leftover `<state_dir>/sessions/<id>/` directory.
/// Idempotent: OK if the parent doesn't exist yet.
///
/// Defensive against accidents:
/// - read `state_dir/sessions/` only — never `state_dir` itself or anything
///   outside it.
/// - use `symlink_metadata` so we don't follow symlinks out of bounds.
/// - reject non-directories. A stray entry makes ownership ambiguous, so
///   startup stops instead of announcing partial cleanup.
fn sweep_state_dir(state_dir: &std::path::Path) -> ServiceResult<()> {
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
        if !meta.is_dir() {
            return Err(ServiceError::Internal(format!(
                "sweep_state_dir: unexpected non-directory entry {}; refusing partial cleanup",
                path.display()
            )));
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

/// Remove `<results_dir>/<id>/` directories that lack `finished.json`.
///
/// Such directories exist only as the result of a server crash mid-bundling
/// (between the bundle write and the `finished.json` rename) — the user
/// explicitly accepted that crash-mid-session sessions are lost, and the
/// directory has no recoverable terminal record. Removing it keeps `list`
/// honest and stops the dir from accumulating across many crashes.
///
/// A directory with `finished.json` is retained only after validating its
/// name, file type, terminal JSON shape, and session identity.
fn sweep_partial_results(results_dir: &std::path::Path) -> ServiceResult<()> {
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
        if !meta.is_dir() {
            return Err(ServiceError::Internal(format!(
                "sweep_partial_results: unexpected non-directory entry {}; refusing partial cleanup",
                path.display()
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
        let finished = path.join("finished.json");
        let finished_meta = match std::fs::symlink_metadata(&finished) {
            Ok(metadata) => Some(metadata),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(ServiceError::Internal(format!(
                    "sweep_partial_results: cannot stat {}: {error}",
                    finished.display()
                )));
            }
        };
        if let Some(finished_meta) = finished_meta {
            if !finished_meta.is_file() || finished_meta.file_type().is_symlink() {
                return Err(ServiceError::Internal(format!(
                    "sweep_partial_results: {} is not a regular non-symlink file",
                    finished.display()
                )));
            }
            let bytes = std::fs::read(&finished).map_err(|error| {
                ServiceError::Internal(format!(
                    "sweep_partial_results: cannot read {}: {error}",
                    finished.display()
                ))
            })?;
            let body: crate::runtime::SessionBody =
                serde_json::from_slice(&bytes).map_err(|error| {
                    ServiceError::Internal(format!(
                        "sweep_partial_results: terminal record {} is malformed: {error}",
                        finished.display()
                    ))
                })?;
            if body.session_id != name || body.status == crate::runtime::SessionStatus::Running {
                return Err(ServiceError::Internal(format!(
                    "sweep_partial_results: terminal identity/status mismatch in {}",
                    finished.display()
                )));
            }
            continue;
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

#[cfg(test)]
mod tests {
    use super::CreateRequest;

    #[test]
    fn create_request_rejects_unknown_fields() {
        let result = serde_json::from_str::<CreateRequest>(
            r#"{"prompt":"do work","folder":"/home/user/project","fallback":true}"#,
        );
        let error = result.expect_err("unknown request fields must fail closed");
        assert!(error.to_string().contains("unknown field `fallback`"));
    }
}
