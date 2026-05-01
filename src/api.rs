//! HTTP surface — axum routes + streaming NDJSON response builder.
//!
//! The contract is deliberately tiny and opinionated:
//!
//! - `POST /v1/agent/run`  body: `{"prompt": "<str>", "folder": "<abs path>"}`.
//!   Both fields are required and rejected if empty / malformed.
//!   Successful response: `200 OK`, `Content-Type: application/x-ndjson`,
//!   exactly two NDJSON lines:
//!   1. `{"event":"started",...,"ttyd_url":"http://127.0.0.1:NNNNN/"}`
//!   2. `{"event":"finished",...,"response":"...","is_error":bool,...}`
//!
//!   On in-stream failure (e.g. agent OOM, container died) a third event
//!   shape `{"event":"error",...}` is emitted and the stream closes.
//!   Pre-stream failures (validation, busy, docker unavailable) are
//!   returned as a non-streaming JSON body with a 4xx/5xx status and the
//!   `WireError` shape from `error.rs`.
//!
//! - `GET /v1/agent/current`  returns the active session as JSON, or 404 if
//!   the service is idle. Always-present fields, no optional members.
//!
//! - `GET /healthz`  one byte of plaintext `"ok"`. For supervisor probes.
//!
//! Nothing else. There is one and only one way to invoke the agent and one
//! and only one way to receive responses; the GET endpoints exist for
//! observability, not control.

use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::config::Config;
use crate::error::{ServiceError, ServiceResult};
use crate::session::{self, FinishedEvent, RunningSession, SessionManager, StartedEvent};
use crate::validation;

#[derive(Clone)]
pub struct AppState {
    /// Read-only handle to the current configuration. Currently consulted
    /// only by `pre_flight` at startup; kept on the state for symmetry with
    /// future endpoints (e.g., `/v1/agent/info`).
    #[allow(dead_code)]
    pub cfg: Arc<Config>,
    pub manager: Arc<SessionManager>,
}

pub fn router(state: AppState) -> axum::Router {
    axum::Router::new()
        .route("/v1/agent/run", post(run_handler))
        .route("/v1/agent/current", get(current_handler))
        .route("/healthz", get(healthz_handler))
        .with_state(state)
        .layer(
            tower_http::limit::RequestBodyLimitLayer::new(
                // 256 KiB cap on request bodies. Prompt+folder JSON should
                // never approach this in practice; folder is a path string,
                // and prompt is capped at 100 KiB by `validation`.
                256 * 1024,
            ),
        )
}

#[derive(Deserialize)]
pub struct RunRequest {
    pub prompt: String,
    pub folder: String,
}

#[derive(Serialize)]
struct StreamErrorEvent<'a> {
    event: &'a str,
    kind: &'a str,
    error: String,
    session_id: String,
}

async fn healthz_handler() -> &'static str {
    "ok"
}

async fn current_handler(
    State(state): State<AppState>,
) -> Result<Response, ServiceError> {
    match state.manager.current().await {
        Some(s) => Ok((StatusCode::OK, Json(s)).into_response()),
        None => {
            // Maintain the required-field discipline: 404 carries an
            // explicit shape identical to the busy/error envelope so
            // clients have one parser for everything coming off this
            // service.
            #[derive(Serialize)]
            struct IdleResponse {
                running: bool,
                session: Option<RunningSession>,
            }
            Ok((
                StatusCode::NOT_FOUND,
                Json(IdleResponse {
                    running: false,
                    session: None,
                }),
            )
                .into_response())
        }
    }
}

async fn run_handler(
    State(state): State<AppState>,
    Json(body): Json<RunRequest>,
) -> Result<Response, ServiceError> {
    // ── Synchronous pre-flight (any failure → non-streaming 4xx/5xx) ───
    let validated = validation::validate(&body.prompt, &body.folder)?;
    let (size_bytes, file_count) =
        validation::enumerate_folder(&validated.folder)?;
    tracing::info!(
        prompt_chars = body.prompt.chars().count(),
        folder = %validated.folder.display(),
        size_bytes,
        file_count,
        "run_handler: pre-flight ok"
    );

    // ── Streaming setup ────────────────────────────────────────────────
    // Capacity 16 is plenty: we only ever send 2-3 events per session.
    let (tx, mut rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(16);

    let manager = Arc::clone(&state.manager);
    let tx_started = tx.clone();
    let tx_finished = tx.clone();
    let tx_for_error = tx.clone();

    // Detached: if the HTTP client disconnects, the agent finishes anyway.
    // The strict-singleton guarantee makes this safe — no second client can
    // step on this run, and we want strong ownership of the docker container
    // even across flaky network conditions.
    tokio::spawn(async move {
        let result = manager
            .run(
                validated,
                move |ev: StartedEvent| {
                    let bytes = encode_event_json(&ev);
                    let _ = tx_started.try_send(Ok(bytes));
                },
                move |ev: FinishedEvent| {
                    let bytes = encode_event_json(&ev);
                    let _ = tx_finished.try_send(Ok(bytes));
                },
            )
            .await;

        if let Err(e) = result {
            // Setup-phase failure (e.g., docker run failed before we ever
            // emitted "started"). Emit an error event so the client sees
            // *something* on the stream.
            let envelope = StreamErrorEvent {
                event: "error",
                kind: e.kind_str(),
                error: e.message(),
                session_id: match &e {
                    ServiceError::Busy { running_session_id } => running_session_id.clone(),
                    _ => String::new(),
                },
            };
            let bytes = encode_event_json(&envelope);
            let _ = tx_for_error.try_send(Ok(bytes));
        }
        drop(tx); // close stream
    });

    let stream = async_stream::stream! {
        while let Some(item) = rx.recv().await {
            yield item;
        }
    };

    let body = Body::from_stream(stream);
    let resp = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-ndjson")
        .header(header::CACHE_CONTROL, "no-store")
        // "X-Accel-Buffering: no" — make sure no intermediate proxy buffers
        // away the per-event flushes. We only listen on loopback, but a user
        // *could* sit a reverse proxy in front; this header is a no-op
        // otherwise.
        .header(
            "x-accel-buffering",
            HeaderValue::from_static("no"),
        )
        .body(body)
        .map_err(|e| {
            ServiceError::Internal(format!("response builder: {e}"))
        })?;
    Ok(resp)
}

fn encode_event_json<T: Serialize>(ev: &T) -> Bytes {
    // Last-resort fallback: if serde refuses the value, emit a plain-text
    // line so the client can still see *something*. This never happens in
    // practice for our concrete event types — they are all `#[derive(Serialize)]`
    // with primitive fields — but we don't `unwrap` even that.
    match serde_json::to_vec(ev) {
        Ok(mut v) => {
            v.push(b'\n');
            Bytes::from(v)
        }
        Err(e) => {
            let s = format!(
                r#"{{"event":"error","kind":"internal","error":"serde failure: {e}"}}{}"#,
                "\n"
            );
            Bytes::from(s)
        }
    }
}

/// Used at startup to validate that we can actually serve traffic before
/// binding the listen socket. Surfaces a clear error to the operator if not.
pub async fn pre_flight(cfg: &Config) -> ServiceResult<()> {
    crate::docker_ops::ping_daemon().await?;
    crate::docker_ops::image_exists(&cfg.agent_image).await?;

    // Bundle creation depends on `tar` and `zstd` on the host PATH.
    crate::bundle::check_host_dependencies().await?;

    // If a storage quota was requested, verify the daemon's storage driver
    // honours it now — better to fail at startup than to fail every session.
    if let Some(quota) = &cfg.agent_storage_quota {
        if let Err(e) = crate::docker_ops::probe_storage_quota(&cfg.agent_image, quota).await
        {
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

    // Verify the state directory exists / can be created.
    if let Err(e) = std::fs::create_dir_all(&cfg.state_dir) {
        return Err(ServiceError::Internal(format!(
            "cannot create AGENT_SERVICE_STATE_DIR at {}: {e}",
            cfg.state_dir.display()
        )));
    }
    // And the results directory. We deliberately do NOT sweep this one —
    // it's the persistent home for past-session bundles.
    if let Err(e) = std::fs::create_dir_all(&cfg.results_dir) {
        return Err(ServiceError::Internal(format!(
            "cannot create AGENT_SERVICE_RESULTS_DIR at {}: {e}",
            cfg.results_dir.display()
        )));
    }

    // Sweep any orphans from a prior crash before announcing ourselves —
    // Docker objects first, then any leftover state directories. Both
    // sweeps complete (or fail loudly) BEFORE the listener is bound, so
    // no incoming request can land while a half-cleaned-up prior session
    // is still on disk or in `docker ps`.
    session::sweep_orphans().await?;
    sweep_state_dir(&cfg.state_dir)?;

    // Apply the retention policy at boot too, in case the operator just
    // shrank `AGENT_SERVICE_RESULTS_RETAIN` and there are now-excess
    // bundles to clean up.
    let prune_diags = crate::bundle::prune_results(&cfg.results_dir, cfg.results_retain);
    for d in prune_diags {
        tracing::warn!("{d}");
    }

    Ok(())
}

/// Remove every leftover `<state_dir>/sessions/<id>/` directory.
/// Idempotent: silently OK if the parent doesn't exist yet.
///
/// Defensive against accidents:
/// - we read `state_dir/sessions/` only — never `state_dir` itself or
///   anything outside it.
/// - we use `symlink_metadata` so we don't follow symlinks out of bounds.
/// - we skip non-directories (a stray file in this tree is unexpected;
///   we log and leave it for the operator to look at, rather than rm it).
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
