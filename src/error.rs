//! Service error type.
//!
//! Every variant carries a dynamic `String` message, populated with enough
//! context (paths, IDs, the operation that failed, the underlying cause) to
//! diagnose without grepping logs. Runtime failures are returned explicitly
//! and never silently swallowed; they become `Err(ServiceError::...)` and are
//! converted to JSON HTTP responses at the API boundary by `IntoResponse`.

use std::fmt;

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

#[derive(Debug, Clone)]
pub enum ServiceError {
    /// 400 — request body / fields rejected by validation.
    InvalidRequest(String),
    /// 404 — no route exists for the requested HTTP path. This is distinct
    /// from a well-formed session resource lookup that found no resource.
    EndpointNotFound(String),
    /// 405 — the path exists, but the supplied HTTP method is not part of its
    /// single explicit contract.
    MethodNotAllowed(String),
    /// 413 — the request exceeded the one compiled body-size bound.
    PayloadTooLarge(String),
    /// 415 — a body-bearing endpoint did not receive application/json.
    UnsupportedMediaType(String),
    /// 404 — the requested session id was never durably accepted or its
    /// terminal resource was explicitly DELETE'd. Accepted resources are
    /// terminalized during restart recovery rather than silently lost.
    NotFound { session_id: String },
    /// 409 — singleton: another session is already running. Returned by
    /// `submit` when a second concurrent submission is attempted.
    Busy { running_session_id: String },
    /// 409 — an idempotency handle already durably owns a different exact
    /// request.  Reinterpreting it would make transport retry unsafe.
    IdempotencyConflict { session_id: String, detail: String },
    /// 500 — the acceptance marker became visible but the final durability
    /// barrier failed twice. The detached supervisor still owns the visible
    /// resource; the caller must retain and reuse the same handle.
    AcceptanceDurabilityFailed { session_id: String, detail: String },
    /// 500 — a valid cancellation marker is visible and live interruption
    /// proceeds, but the containing-directory durability barrier failed.
    CancellationDurabilityFailed { session_id: String, detail: String },
    /// 409 — the supervisor has irrevocably selected the terminal outcome
    /// but has not yet completed its durable publication. Cancellation must
    /// never wait behind that potentially long filesystem transaction or
    /// pretend that it changed an already-selected outcome.
    SessionFinalizing { session_id: String },
    /// 409 — a lifecycle operation was attempted on a session that is in
    /// the wrong state for it: `DELETE` or a bundle download on a running
    /// session. The lifecycle is explicit, so the caller must `cancel` or
    /// wait for the terminal state first. Carries the offending session_id
    /// so the operator can act on it.
    SessionRunning { session_id: String },
    /// 404 — the session is terminal and will never have a bundle: no
    /// accepted `bundle.tar.zst` exists for it, and none can appear later.
    BundleAbsent { session_id: String },
    /// 409 — a durable DELETE intent exists and exact cleanup is incomplete.
    /// Repeating DELETE resumes the same operation; reads must not expose a
    /// terminal whose backing evidence may already be partly removed.
    SessionDeleting { session_id: String },
    /// 503 — graceful shutdown has closed lifecycle admission. Existing
    /// server-owned mutations are drained, but no new mutation may begin.
    ServiceShuttingDown,
    /// 409 — the caller's source tree changed after its descriptor-anchored
    /// snapshot began. The caller may retry only after making the input tree
    /// quiescent; this is neither a static request error nor a server defect.
    SourceChanged(String),
    /// 503 — Docker daemon not reachable as the running user.
    DockerUnavailable(String),
    /// 502 — Docker subprocess (run / inspect / stop / logs / etc.) returned non-zero.
    DockerCommand(String),
    /// 500 — host-side filesystem failure (staging, state dir, results dir, …).
    Staging(String),
    /// 504 — wall-clock timeout waiting for an internal step.
    Timeout(String),
    /// 502 — agent ran but produced no result event (or unparseable result file).
    AgentOutputMissing(String),
    /// 500 — anything else genuinely internal.
    Internal(String),
}

impl ServiceError {
    pub fn http_status(&self) -> StatusCode {
        match self {
            Self::InvalidRequest(_) => StatusCode::BAD_REQUEST,
            Self::EndpointNotFound(_) => StatusCode::NOT_FOUND,
            Self::MethodNotAllowed(_) => StatusCode::METHOD_NOT_ALLOWED,
            Self::PayloadTooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
            Self::UnsupportedMediaType(_) => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::NotFound { .. } | Self::BundleAbsent { .. } => StatusCode::NOT_FOUND,
            Self::Busy { .. }
            | Self::IdempotencyConflict { .. }
            | Self::SessionFinalizing { .. }
            | Self::SessionRunning { .. }
            | Self::SessionDeleting { .. }
            | Self::SourceChanged(_) => {
                StatusCode::CONFLICT
            }
            Self::DockerUnavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::ServiceShuttingDown => StatusCode::SERVICE_UNAVAILABLE,
            Self::DockerCommand(_) | Self::AgentOutputMissing(_) => StatusCode::BAD_GATEWAY,
            Self::Staging(_)
            | Self::AcceptanceDurabilityFailed { .. }
            | Self::CancellationDurabilityFailed { .. }
            | Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::Timeout(_) => StatusCode::GATEWAY_TIMEOUT,
        }
    }

    pub fn kind_str(&self) -> &'static str {
        match self {
            Self::InvalidRequest(_) => "invalid_request",
            Self::EndpointNotFound(_) => "endpoint_not_found",
            Self::MethodNotAllowed(_) => "method_not_allowed",
            Self::PayloadTooLarge(_) => "payload_too_large",
            Self::UnsupportedMediaType(_) => "unsupported_media_type",
            Self::NotFound { .. } => "not_found",
            Self::Busy { .. } => "busy",
            Self::IdempotencyConflict { .. } => "idempotency_conflict",
            Self::AcceptanceDurabilityFailed { .. } => "acceptance_durability_failed",
            Self::CancellationDurabilityFailed { .. } => "cancellation_durability_failed",
            Self::SessionFinalizing { .. } => "session_finalizing",
            Self::SessionRunning { .. } => "session_running",
            Self::BundleAbsent { .. } => "bundle_absent",
            Self::SessionDeleting { .. } => "session_deleting",
            Self::ServiceShuttingDown => "service_shutting_down",
            Self::SourceChanged(_) => "source_changed",
            Self::DockerUnavailable(_) => "docker_unavailable",
            Self::DockerCommand(_) => "docker_command_failed",
            Self::Staging(_) => "staging_failed",
            Self::Timeout(_) => "timeout",
            Self::AgentOutputMissing(_) => "agent_output_missing",
            Self::Internal(_) => "internal",
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::InvalidRequest(m)
            | Self::EndpointNotFound(m)
            | Self::MethodNotAllowed(m)
            | Self::PayloadTooLarge(m)
            | Self::UnsupportedMediaType(m)
            | Self::SourceChanged(m)
            | Self::DockerUnavailable(m)
            | Self::DockerCommand(m)
            | Self::Staging(m)
            | Self::Timeout(m)
            | Self::AgentOutputMissing(m)
            | Self::Internal(m) => m.clone(),
            Self::NotFound { session_id } => format!(
                "session {session_id} is not known to this server — it was never durably accepted or its terminal resource was explicitly DELETE'd; service restarts recover every accepted handle into an explicit terminal result before listening"
            ),
            Self::Busy { running_session_id } => format!(
                "another session ({running_session_id}) is already running; this service is a strict singleton — optionally GET that resource, or POST /v1/agent/sessions/{running_session_id}/cancel to durably request teardown"
            ),
            Self::IdempotencyConflict { detail, .. } => detail.clone(),
            Self::AcceptanceDurabilityFailed { detail, .. } => detail.clone(),
            Self::CancellationDurabilityFailed { detail, .. } => detail.clone(),
            Self::SessionFinalizing { session_id } => format!(
                "session {session_id} has already selected its terminal outcome and is durably publishing evidence; cancellation cannot retroactively change it — issue an ordinary GET for the same resource later"
            ),
            Self::SessionRunning { session_id } => format!(
                "session {session_id} is still running; DELETE and bundle downloads apply only to terminal sessions — POST /v1/agent/sessions/{session_id}/cancel or wait for the terminal state, then repeat the same request"
            ),
            Self::BundleAbsent { session_id } => format!(
                "terminal session {session_id} accepted no result bundle, and none can appear later; its terminal record and any retained raw evidence are the only artifacts"
            ),
            Self::SessionDeleting { session_id } => format!(
                "session {session_id} has a durable deletion intent and cleanup is incomplete; repeat DELETE for the same resource to resume exact cleanup"
            ),
            Self::ServiceShuttingDown =>
                "the service is gracefully shutting down: new lifecycle mutations are closed while every already accepted mutation and active session is drained"
                    .to_string(),
        }
    }

    /// `session_id` carried in the wire envelope, when applicable. Empty
    /// string for variants that don't reference a specific session.
    pub fn session_id(&self) -> &str {
        match self {
            Self::NotFound { session_id }
            | Self::BundleAbsent { session_id }
            | Self::SessionRunning { session_id }
            | Self::SessionDeleting { session_id }
            | Self::SessionFinalizing { session_id }
            | Self::AcceptanceDurabilityFailed { session_id, .. }
            | Self::CancellationDurabilityFailed { session_id, .. }
            | Self::IdempotencyConflict { session_id, .. } => {
                session_id.as_str()
            }
            Self::Busy { running_session_id } => running_session_id.as_str(),
            _ => "",
        }
    }
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind_str(), self.message())
    }
}

impl std::error::Error for ServiceError {}

#[derive(Serialize)]
struct WireError<'a> {
    error: String,
    kind: &'a str,
    /// Always present; empty string when not applicable. Required-field
    /// discipline — clients have one parser for every error response.
    session_id: String,
}

impl IntoResponse for ServiceError {
    fn into_response(self) -> Response {
        let body = WireError {
            error: self.message(),
            kind: self.kind_str(),
            session_id: self.session_id().to_string(),
        };
        let mut response = (self.http_status(), Json(body)).into_response();
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        );
        response
    }
}

pub type ServiceResult<T> = Result<T, ServiceError>;

/// Helper: format a `std::io::Error` with the path that produced it. We do
/// this in many places, so the helper deduplicates the boilerplate.
pub fn io_msg(context: &str, path: &std::path::Path, err: &std::io::Error) -> String {
    format!("{context} at {}: {err}", path.display())
}
