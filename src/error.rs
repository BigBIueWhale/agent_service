//! Service error type.
//!
//! Every variant carries a dynamic `String` message. We never panic, never
//! call `.unwrap()` / `.expect()` on user-facing paths, never silently swallow
//! a failure. Failures bubble up as `Err(ServiceError::...)` and are converted
//! to JSON HTTP responses at the API boundary.

use std::fmt;

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

#[derive(Debug)]
pub enum ServiceError {
    /// 400 — request body / fields rejected by validation.
    InvalidRequest(String),
    /// 409 — singleton: another session is already running.
    Busy { running_session_id: String },
    /// 503 — Docker daemon not reachable as the running user.
    DockerUnavailable(String),
    /// 503 — agent template image (or proxy image) is not present locally.
    ImageMissing(String),
    /// 502 — Docker subprocess (run / network create / etc.) returned non-zero.
    DockerCommand(String),
    /// 500 — host-side filesystem failure (staging, state dir, etc.).
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
            Self::Busy { .. } => StatusCode::CONFLICT,
            Self::DockerUnavailable(_) | Self::ImageMissing(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::DockerCommand(_) | Self::AgentOutputMissing(_) => StatusCode::BAD_GATEWAY,
            Self::Staging(_) | Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::Timeout(_) => StatusCode::GATEWAY_TIMEOUT,
        }
    }

    pub fn kind_str(&self) -> &'static str {
        match self {
            Self::InvalidRequest(_) => "invalid_request",
            Self::Busy { .. } => "busy",
            Self::DockerUnavailable(_) => "docker_unavailable",
            Self::ImageMissing(_) => "image_missing",
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
            | Self::DockerUnavailable(m)
            | Self::ImageMissing(m)
            | Self::DockerCommand(m)
            | Self::Staging(m)
            | Self::Timeout(m)
            | Self::AgentOutputMissing(m)
            | Self::Internal(m) => m.clone(),
            Self::Busy { running_session_id } => format!(
                "another session ({running_session_id}) is already running; this service is a strict singleton — wait for it to finish or DELETE it"
            ),
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
    /// Always present; empty string when not applicable. Required-field discipline.
    running_session_id: String,
}

impl IntoResponse for ServiceError {
    fn into_response(self) -> Response {
        let running_session_id = match &self {
            Self::Busy { running_session_id } => running_session_id.clone(),
            _ => String::new(),
        };
        let body = WireError {
            error: self.message(),
            kind: self.kind_str(),
            running_session_id,
        };
        (self.http_status(), Json(body)).into_response()
    }
}

pub type ServiceResult<T> = Result<T, ServiceError>;

/// Helper: format a `std::io::Error` with the path that produced it. We do
/// this in many places, so the helper deduplicates the boilerplate.
pub fn io_msg(context: &str, path: &std::path::Path, err: &std::io::Error) -> String {
    format!("{context} at {}: {err}", path.display())
}
