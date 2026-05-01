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
//!   until ttyd is reachable; returns `201 Created` with the running body.
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
//! DELETE. Reads never mutate. Writes (cancel, delete) are idempotent.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::{ServiceError, ServiceResult};
use crate::runtime::{Manager, SessionBody};
use crate::session;
use crate::validation;

#[derive(Clone)]
pub struct AppState {
    /// Read-only handle to the loaded configuration. Currently consulted
    /// only by `pre_flight`; kept on the state for symmetry with future
    /// endpoints that may want to surface effective config (e.g.,
    /// `/v1/agent/info`).
    #[allow(dead_code)]
    pub cfg: Arc<Config>,
    pub manager: Arc<Manager>,
}

pub fn router(state: AppState) -> axum::Router {
    axum::Router::new()
        .route("/v1/agent/sessions", post(create_session).get(list_sessions))
        .route(
            "/v1/agent/sessions/{id}",
            get(get_session).delete(delete_session),
        )
        .route("/v1/agent/sessions/{id}/cancel", post(cancel_session))
        .route("/healthz", get(healthz))
        .with_state(state)
        .layer(tower_http::limit::RequestBodyLimitLayer::new(
            // 256 KiB cap on request bodies. Prompt+folder JSON should
            // never approach this in practice; prompt is capped at 1 MiB
            // by `validation`, but the body cap here is the first layer
            // of defence against an adversarial body.
            256 * 1024,
        ))
}

#[derive(Deserialize, Debug)]
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
    let validated = validation::validate(&body.prompt, &body.folder)?;
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

async fn list_sessions(
    State(state): State<AppState>,
) -> Result<Json<ListResponse>, ServiceError> {
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
    crate::docker_ops::ping_daemon().await?;
    crate::docker_ops::image_exists(&cfg.agent_image).await?;

    // The agent's network sandbox depends on
    // `com.docker.network.bridge.gateway_mode_ipv4=isolated` (Docker ≥ 27.1)
    // to suppress the host-side bridge IP. Without it, `--internal` alone
    // leaves the bridge gateway reachable, exposing 0.0.0.0-bound host
    // services to the agent. Probe once at startup; refuse to come up if
    // the daemon doesn't honour the flag.
    crate::docker_ops::probe_gateway_isolated().await?;

    // Bundle creation depends on `tar` and `zstd` on the host PATH.
    crate::bundle::check_host_dependencies().await?;

    if let Some(quota) = &cfg.agent_storage_quota {
        if let Err(e) = crate::docker_ops::probe_storage_quota(&cfg.agent_image, quota).await {
            return Err(ServiceError::Internal(format!(
                "AGENT_SERVICE_STORAGE_QUOTA={quota} is set, but your Docker storage driver does not honour `--storage-opt size=…`. \
                 Either (a) configure the daemon for per-container size quotas \
                 (overlay2 on xfs with pquota mount option, or btrfs / zfs / devicemapper), \
                 or (b) explicitly set `AGENT_SERVICE_STORAGE_QUOTA=` (empty) to disable. \
                 Probe error: {e}"
            )));
        }
        tracing::info!(quota = %quota, "storage quota supported and will be enforced");
    } else {
        tracing::warn!(
            "AGENT_SERVICE_STORAGE_QUOTA is disabled — agent containers run without per-container storage cap"
        );
    }

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
    // Docker objects (containers / networks), staging dirs, and any
    // crash-interrupted result directories (no `finished.json`). Sweeps
    // complete (or fail loudly) BEFORE the listener binds, so no incoming
    // request can land while a half-cleaned-up prior session exists.
    session::sweep_orphans().await?;
    sweep_state_dir(&cfg.state_dir)?;
    sweep_partial_results(&cfg.results_dir)?;

    Ok(())
}

/// Remove every leftover `<state_dir>/sessions/<id>/` directory.
/// Idempotent: silently OK if the parent doesn't exist yet.
///
/// Defensive against accidents:
/// - read `state_dir/sessions/` only — never `state_dir` itself or anything
///   outside it.
/// - use `symlink_metadata` so we don't follow symlinks out of bounds.
/// - skip non-directories (a stray file in this tree is unexpected; log it
///   and leave it for the operator to look at, rather than rm it).
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
        let entry = match entry {
            Ok(x) => x,
            Err(e) => {
                tracing::warn!(error = %e, "sweep_state_dir: read_dir entry");
                continue;
            }
        };
        let path = entry.path();
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(), "sweep_state_dir: stat");
                continue;
            }
        };
        if !meta.is_dir() {
            tracing::warn!(
                path = %path.display(),
                "sweep_state_dir: skipping non-directory entry under sessions/"
            );
            continue;
        }
        match std::fs::remove_dir_all(&path) {
            Ok(()) => {
                removed += 1;
                tracing::info!(dir = %path.display(), "sweep_state_dir: removed leftover");
            }
            Err(e) => {
                tracing::warn!(error = %e, dir = %path.display(), "sweep_state_dir: rm");
            }
        }
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
/// Untouched if the directory has `finished.json` (durable terminal record)
/// or is missing entirely.
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
        let entry = match entry {
            Ok(x) => x,
            Err(e) => {
                tracing::warn!(error = %e, "sweep_partial_results: read_dir entry");
                continue;
            }
        };
        let path = entry.path();
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    path = %path.display(),
                    "sweep_partial_results: stat"
                );
                continue;
            }
        };
        if !meta.is_dir() {
            // Stray file — leave it.
            continue;
        }
        if path.join("finished.json").exists() {
            continue;
        }
        match std::fs::remove_dir_all(&path) {
            Ok(()) => {
                removed += 1;
                tracing::info!(
                    dir = %path.display(),
                    "sweep_partial_results: removed crash-interrupted session dir"
                );
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    dir = %path.display(),
                    "sweep_partial_results: rm"
                );
            }
        }
    }
    if removed > 0 {
        tracing::info!(
            count = removed,
            "sweep_partial_results: complete (these sessions were interrupted by a server crash)"
        );
    }
    Ok(())
}
