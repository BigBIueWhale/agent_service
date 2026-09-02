//! Narrow client for the session broker's single-record Unix-socket protocol.
//!
//! The service process contains no Docker CLI and never receives the raw
//! Docker socket.  It can request only the fixed operations represented by
//! the functions in this module.  Image names, commands, mounts, network
//! modes, resource limits, and arbitrary Docker arguments never cross this
//! boundary.

use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use crate::config::Config;
use crate::error::{ServiceError, ServiceResult};

const REQUEST_LIMIT: usize = 65_536;
const RESPONSE_LIMIT: u64 = 4 * 1024 * 1024;
const BROKER_OP_TIMEOUT: Duration = Duration::from_secs(120);
const BROKER_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BrokerResponse {
    ok: bool,
    data: Value,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionNames {
    pub agent: String,
    pub relay: String,
    pub capture: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LogsData {
    logs: String,
    relay_logs: String,
    capture_logs: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WaitData {
    exit_code: i32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionQuiescence {
    pub agent_present: bool,
    pub agent_running: bool,
    pub relay_present: bool,
    pub relay_running: bool,
    pub capture_present: bool,
    pub capture_running: bool,
    pub quiescent: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadyData {
    pub model: String,
    pub context_window: u64,
    pub token_count: u64,
    pub sandbox: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureComplete {
    pub events_bytes: u64,
    pub stderr_bytes: u64,
}

pub async fn preflight(cfg: &Config) -> ServiceResult<Value> {
    call(cfg, json!({"op": "preflight"}), Some(BROKER_OP_TIMEOUT)).await
}

pub async fn sweep_orphans(cfg: &Config) -> ServiceResult<()> {
    expect_empty(
        call(cfg, json!({"op": "sweep_orphans"}), Some(BROKER_OP_TIMEOUT)).await?,
        "sweep_orphans",
    )
}

pub async fn create_session(cfg: &Config, session_id: &str) -> ServiceResult<SessionNames> {
    validate_session_id(session_id)?;
    let value = call(
        cfg,
        json!({
            "op": "create_session",
            "session_id": session_id,
        }),
        Some(BROKER_OP_TIMEOUT),
    )
    .await?;
    let names: SessionNames = serde_json::from_value(value).map_err(|error| {
        ServiceError::DockerCommand(format!(
            "broker create_session response violates its exact schema: {error}"
        ))
    })?;
    let expected_agent = format!("agent-{session_id}");
    let expected_relay = format!("agent-model-{session_id}");
    let expected_capture = format!("agent-capture-{session_id}");
    if names.agent != expected_agent
        || names.relay != expected_relay
        || names.capture != expected_capture
    {
        return Err(ServiceError::DockerCommand(format!(
            "broker returned drifted session names: expected {expected_agent:?}/{expected_relay:?}/{expected_capture:?}, observed {:?}/{:?}/{:?}",
            names.agent, names.relay, names.capture
        )));
    }
    Ok(names)
}

pub async fn session_logs(cfg: &Config, session_id: &str) -> ServiceResult<String> {
    validate_session_id(session_id)?;
    let value = call(
        cfg,
        json!({"op": "session_logs", "session_id": session_id}),
        Some(BROKER_OP_TIMEOUT),
    )
    .await?;
    let data: LogsData = serde_json::from_value(value).map_err(|error| {
        ServiceError::DockerCommand(format!(
            "broker session_logs response violates its exact schema: {error}"
        ))
    })?;
    Ok(format!(
        "agent logs:\n{}\nmodel-relay logs:\n{}\nsession-capture logs:\n{}",
        data.logs, data.relay_logs, data.capture_logs
    ))
}

/// Waits without an arbitrary session deadline.  Cancellation uses a second
/// broker connection to stop the agent; that makes Docker's wait complete.
pub async fn wait_session(cfg: &Config, session_id: &str) -> ServiceResult<i32> {
    validate_session_id(session_id)?;
    let value = call(
        cfg,
        json!({"op": "wait_session", "session_id": session_id}),
        None,
    )
    .await?;
    let data: WaitData = serde_json::from_value(value).map_err(|error| {
        ServiceError::DockerCommand(format!(
            "broker wait_session response violates its exact schema: {error}"
        ))
    })?;
    Ok(data.exit_code)
}

/// Waits on the broker's event-driven Docker log stream until the wrapper has
/// completed its one model/tokenizer preflight and durably published
/// `ready.json`. There is no service-side file polling loop.
pub async fn wait_agent_ready(cfg: &Config, session_id: &str) -> ServiceResult<ReadyData> {
    validate_session_id(session_id)?;
    let value = call(
        cfg,
        json!({"op": "wait_agent_ready", "session_id": session_id}),
        Some(Duration::from_secs(60)),
    )
    .await?;
    serde_json::from_value(value).map_err(|error| {
        ServiceError::DockerCommand(format!(
            "broker wait_agent_ready response violates its exact schema: {error}"
        ))
    })
}

/// Waits without a drain deadline after the agent container has stopped. The
/// capture either emits its exact durable byte-count event or exits and the
/// Docker log stream closes with an explicit failure.
pub async fn wait_capture_complete(
    cfg: &Config,
    session_id: &str,
) -> ServiceResult<CaptureComplete> {
    validate_session_id(session_id)?;
    let value = call(
        cfg,
        json!({"op": "wait_capture_complete", "session_id": session_id}),
        None,
    )
    .await?;
    serde_json::from_value(value).map_err(|error| {
        ServiceError::DockerCommand(format!(
            "broker wait_capture_complete response violates its exact schema: {error}"
        ))
    })
}

pub async fn stop_session(cfg: &Config, session_id: &str) -> ServiceResult<()> {
    validate_session_id(session_id)?;
    expect_empty(
        call(
            cfg,
            json!({"op": "stop_session", "session_id": session_id}),
            None,
        )
        .await?,
        "stop_session",
    )
}

pub async fn remove_session(cfg: &Config, session_id: &str) -> ServiceResult<()> {
    validate_session_id(session_id)?;
    expect_empty(
        call(
            cfg,
            json!({"op": "remove_session", "session_id": session_id}),
            Some(BROKER_OP_TIMEOUT),
        )
        .await?,
        "remove_session",
    )
}

/// Independently prove that no exact-owned session container can still
/// mutate the staged/output tree. This read is required before bundling even
/// when removal reported success: a missing object is quiescent, and an
/// extant object is quiescent only when Docker reports State.Running=false.
pub async fn prove_session_quiescent(
    cfg: &Config,
    session_id: &str,
) -> ServiceResult<SessionQuiescence> {
    validate_session_id(session_id)?;
    let value = call(
        cfg,
        json!({"op": "prove_session_quiescent", "session_id": session_id}),
        Some(BROKER_OP_TIMEOUT),
    )
    .await?;
    let state: SessionQuiescence = serde_json::from_value(value).map_err(|error| {
        ServiceError::DockerCommand(format!(
            "broker prove_session_quiescent response violates its exact schema: {error}"
        ))
    })?;
    let derived = !state.agent_running && !state.relay_running && !state.capture_running;
    if state.quiescent != derived
        || (state.agent_running && !state.agent_present)
        || (state.relay_running && !state.relay_present)
        || (state.capture_running && !state.capture_present)
    {
        return Err(ServiceError::DockerCommand(format!(
            "broker returned internally contradictory quiescence state: {state:?}"
        )));
    }
    Ok(state)
}

fn expect_empty(value: Value, operation: &str) -> ServiceResult<()> {
    if value.as_object().is_some_and(serde_json::Map::is_empty) {
        Ok(())
    } else {
        Err(ServiceError::DockerCommand(format!(
            "broker {operation} returned nonempty data: {value}"
        )))
    }
}

fn validate_session_id(value: &str) -> ServiceResult<()> {
    let suffix = value.strip_prefix("s-").ok_or_else(|| {
        ServiceError::Internal(format!("broker session ID lacks s- prefix: {value:?}"))
    })?;
    if !matches!(suffix.len(), 32 | 64)
        || !suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(ServiceError::Internal(format!(
            "broker session ID is not s- plus 64 lowercase hexadecimal characters (or the readable historical 32-character shape): {value:?}"
        )));
    }
    Ok(())
}

fn verify_socket(path: &Path) -> ServiceResult<()> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        ServiceError::DockerUnavailable(format!(
            "cannot stat the sole broker socket {}: {error}",
            path.display()
        ))
    })?;
    if !metadata.file_type().is_socket()
        || metadata.uid() != 1000
        || metadata.gid() != 984
        || metadata.permissions().mode() & 0o777 != 0o660
    {
        return Err(ServiceError::DockerUnavailable(format!(
            "broker socket contract drift at {}: socket={} uid={} gid={} mode={:o}; required socket=true uid=1000 gid=984 mode=660",
            path.display(),
            metadata.file_type().is_socket(),
            metadata.uid(),
            metadata.gid(),
            metadata.permissions().mode() & 0o777
        )));
    }
    Ok(())
}

async fn call(cfg: &Config, request: Value, timeout: Option<Duration>) -> ServiceResult<Value> {
    verify_socket(&cfg.broker_socket)?;
    let mut request_bytes = serde_json::to_vec(&request).map_err(|error| {
        ServiceError::Internal(format!("cannot encode the fixed broker request: {error}"))
    })?;
    request_bytes.push(b'\n');
    if request_bytes.len() > REQUEST_LIMIT
        || request_bytes[..request_bytes.len() - 1].contains(&b'\n')
    {
        return Err(ServiceError::Internal(
            "internally generated broker request violates the single-record size contract".into(),
        ));
    }

    let exchange = async {
        let mut stream = tokio::time::timeout(
            BROKER_CONNECT_TIMEOUT,
            UnixStream::connect(&cfg.broker_socket),
        )
        .await
        .map_err(|_| {
            ServiceError::DockerUnavailable(format!(
                "connection to the sole broker socket {} exceeded {BROKER_CONNECT_TIMEOUT:?}",
                cfg.broker_socket.display()
            ))
        })?
        .map_err(|error| {
            ServiceError::DockerUnavailable(format!(
                "cannot connect to the sole broker socket {}: {error}",
                cfg.broker_socket.display()
            ))
        })?;
        stream.write_all(&request_bytes).await.map_err(|error| {
            ServiceError::DockerCommand(format!("cannot write broker request: {error}"))
        })?;
        stream.shutdown().await.map_err(|error| {
            ServiceError::DockerCommand(format!("cannot half-close broker request: {error}"))
        })?;
        let mut response_bytes = Vec::new();
        (&mut stream)
            .take(RESPONSE_LIMIT + 1)
            .read_to_end(&mut response_bytes)
            .await
            .map_err(|error| {
                ServiceError::DockerCommand(format!("cannot read broker response: {error}"))
            })?;
        if response_bytes.len() as u64 > RESPONSE_LIMIT {
            return Err(ServiceError::DockerCommand(format!(
                "broker response exceeded {RESPONSE_LIMIT} bytes"
            )));
        }
        parse_response(&response_bytes)
    };

    match timeout {
        Some(duration) => tokio::time::timeout(duration, exchange)
            .await
            .map_err(|_| {
                ServiceError::Timeout(format!(
                    "broker operation exceeded its exact {duration:?} control-plane deadline"
                ))
            })?,
        None => exchange.await,
    }
}

fn parse_response(bytes: &[u8]) -> ServiceResult<Value> {
    if bytes.is_empty()
        || !bytes.ends_with(b"\n")
        || bytes.contains(&b'\r')
        || bytes[..bytes.len() - 1].contains(&b'\n')
    {
        return Err(ServiceError::DockerCommand(
            "broker response is not one terminal-LF JSON record without CR bytes".into(),
        ));
    }
    let response: BrokerResponse = serde_json::from_slice(bytes).map_err(|error| {
        ServiceError::DockerCommand(format!("broker response is malformed: {error}"))
    })?;
    match (response.ok, response.data, response.error) {
        (true, data, None) => Ok(data),
        (true, _, Some(error)) => Err(ServiceError::DockerCommand(format!(
            "broker returned ok=true with an error field: {error:?}"
        ))),
        (false, Value::Null, Some(error)) if !error.trim().is_empty() => Err(
            ServiceError::DockerCommand(format!("broker refused operation: {error}")),
        ),
        (false, data, error) => Err(ServiceError::DockerCommand(format!(
            "broker failure envelope violates the exact contract: data={data} error={error:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_response, validate_session_id};

    #[test]
    fn response_envelope_is_fail_closed() {
        assert_eq!(
            parse_response(b"{\"ok\":true,\"data\":{\"x\":1},\"error\":null}\n")
                .expect("canonical success")
                .get("x")
                .and_then(serde_json::Value::as_u64),
            Some(1)
        );
        for invalid in [
            b"{\"ok\":true,\"data\":{},\"error\":\"bad\"}\n".as_slice(),
            b"{\"ok\":false,\"data\":{},\"error\":\"bad\"}\n".as_slice(),
            b"{\"ok\":false,\"data\":null,\"error\":null}\n".as_slice(),
            b"{\"ok\":true,\"data\":{},\"error\":null}\n{}\n".as_slice(),
            b"{\"ok\":true,\"data\":{},\"error\":null,\"extra\":1}\n".as_slice(),
        ] {
            assert!(parse_response(invalid).is_err(), "accepted {invalid:?}");
        }
    }

    #[test]
    fn session_ids_are_canonical() {
        validate_session_id("s-0123456789abcdef0123456789abcdef").expect("canonical ID");
        assert!(validate_session_id("s-0123").is_err());
        assert!(validate_session_id("s-0123456789ABCDEF0123456789ABCDEF").is_err());
    }
}
