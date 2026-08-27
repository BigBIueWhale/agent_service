//! Strict parser for pinned Qwen Code 0.21.12 stream-JSON.
//!
//! Every event names its scope in `parent_tool_use_id`: null (or absent) is
//! the main session, an `agent` tool-call id is that subagent. A subagent that
//! stops without a report emits its own `result` under its tool-call id, so
//! `result` alone does not mean the session ended.
//!
//! Exactly one terminal `result` object *for the main session* is required and
//! it must be the final non-empty line. Every line must be a JSON object with
//! a supported `type`, a non-empty UUID, and the same non-empty session ID.
//! This deliberately rejects partial, duplicated, recovered, or post-terminal
//! output rather than choosing a convenient-looking last result.

use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

use crate::error::{io_msg, ServiceError, ServiceResult};

#[derive(Debug, Clone)]
pub struct AgentResult {
    pub is_error: bool,
    pub response: String,
    pub duration_ms: u64,
    pub num_turns: u64,
}

// A session may have indefinitely many records, but one JSON event is a
// bounded protocol object. 128 MiB is also the maximum durable terminal JSON
// size: a larger single stream event cannot be represented faithfully in the
// public terminal resource and is rejected before it can exhaust the 2 GiB
// service container. This is a per-record bound, never a turn/session bound.
const MAX_EVENT_RECORD_BYTES: usize = 128 * 1024 * 1024;

pub fn parse_events_jsonl(path: &Path) -> ServiceResult<AgentResult> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| {
            ServiceError::AgentOutputMissing(io_msg(
                "open events.jsonl without following links",
                path,
                &error,
            ))
        })?;
    let metadata = file.metadata().map_err(|error| {
        ServiceError::AgentOutputMissing(io_msg(
            "fstat opened events.jsonl",
            path,
            &error,
        ))
    })?;
    if !metadata.is_file()
        || metadata.permissions().mode() & 0o777 != 0o600
        || metadata.uid() != 1000
        || metadata.gid() != 1000
    {
        return Err(ServiceError::AgentOutputMissing(format!(
            "events.jsonl at {} has unsafe opened type/mode/owner",
            path.display()
        )));
    }
    // Capture has already stopped and supplied an exact byte count, but take
    // the fstat length as an additional immutable parsing boundary. Any bytes
    // appended after this descriptor snapshot belong to contradictory state,
    // not to a moving parse target.
    let mut reader = BufReader::new(file.take(metadata.len()));

    let mut stream_session_id: Option<String> = None;
    let mut result: Option<serde_json::Map<String, serde_json::Value>> = None;
    let mut main_assistant_events = 0u64;
    let mut physical_line = 0usize;
    let mut event_count = 0usize;
    let mut terminal_line = 0usize;
    let mut record = Vec::new();

    loop {
        record.clear();
        let terminated = read_bounded_record(&mut reader, &mut record, path)?;
        if record.is_empty() && !terminated {
            break;
        }
        physical_line = physical_line.checked_add(1).ok_or_else(|| {
            ServiceError::AgentOutputMissing("events.jsonl physical line count overflowed".into())
        })?;
        if !terminated {
            return Err(ServiceError::AgentOutputMissing(format!(
                "events.jsonl line {physical_line} is not newline-terminated; refusing a possibly torn final record"
            )));
        }
        let line = record
            .strip_suffix(b"\n")
            .expect("bounded record reports termination only with a newline");
        if line.iter().all(|byte| byte.is_ascii_whitespace()) {
            continue;
        }
        if result.is_some() {
            return Err(ServiceError::AgentOutputMissing(format!(
                "events.jsonl terminal result at line {terminal_line} is followed by another event at line {physical_line}"
            )));
        }
        event_count = event_count.checked_add(1).ok_or_else(|| {
            ServiceError::AgentOutputMissing("events.jsonl event count overflowed".into())
        })?;
        let value: serde_json::Value = serde_json::from_slice(line).map_err(|error| {
            ServiceError::AgentOutputMissing(format!(
                "events.jsonl line {physical_line} is invalid JSON: {error}; content: {}",
                truncate_bytes(line, 512)
            ))
        })?;
        let object = value.as_object().ok_or_else(|| {
            ServiceError::AgentOutputMissing(format!(
                "events.jsonl line {physical_line} is not a JSON object"
            ))
        })?;
        let event_type = required_string(object, "type", physical_line)?;
        if !matches!(event_type, "system" | "user" | "assistant" | "result") {
            return Err(ServiceError::AgentOutputMissing(format!(
                "events.jsonl line {physical_line} has unsupported event type {event_type:?}"
            )));
        }
        required_string(object, "uuid", physical_line)?;
        if event_count == 1 {
            validate_init_event(object, physical_line)?;
        }
        let value = required_string(object, "session_id", physical_line)?;
        match &stream_session_id {
            Some(expected) if expected != value => {
                return Err(ServiceError::AgentOutputMissing(format!(
                    "events.jsonl session_id changed from {expected:?} to {value:?} at line {physical_line}"
                )));
            }
            None => stream_session_id = Some(value.to_string()),
            _ => {}
        }

        require_parent_scope(object, physical_line)?;

        if is_completed_main_turn(object) {
            main_assistant_events = main_assistant_events.checked_add(1).ok_or_else(|| {
                ServiceError::AgentOutputMissing(
                    "events.jsonl completed main-turn count overflowed".into(),
                )
            })?;
        }
        // A subagent's own terminal record is scoped to its agent tool call
        // and is not the end of the session. Only the main session's result
        // terminates the stream; anything after it is post-terminal output.
        if event_type == "result" && is_main_session_event(object) {
            terminal_line = physical_line;
            result = Some(object.clone());
        }
    }

    if event_count == 0 {
        return Err(ServiceError::AgentOutputMissing(format!(
            "events.jsonl at {} contains no events",
            path.display()
        )));
    }

    let result = result.ok_or_else(|| {
        ServiceError::AgentOutputMissing(format!(
            "events.jsonl has {event_count} event(s) but no main-session terminal result"
        ))
    })?;
    let is_error = result
        .get("is_error")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| {
            ServiceError::AgentOutputMissing("terminal result lacks boolean is_error".into())
        })?;
    let subtype = required_string(&result, "subtype", terminal_line)?;
    let duration_ms = required_u64(&result, "duration_ms")?;
    let num_turns = required_u64(&result, "num_turns")?;
    required_u64(&result, "duration_api_ms")?;
    if !result
        .get("usage")
        .is_some_and(serde_json::Value::is_object)
    {
        return Err(ServiceError::AgentOutputMissing(
            "terminal result lacks object usage".into(),
        ));
    }
    if !result
        .get("permission_denials")
        .is_some_and(serde_json::Value::is_array)
    {
        return Err(ServiceError::AgentOutputMissing(
            "terminal result lacks array permission_denials".into(),
        ));
    }
    if num_turns != main_assistant_events {
        return Err(ServiceError::AgentOutputMissing(format!(
            "terminal num_turns={num_turns} does not match {main_assistant_events} main assistant event(s)"
        )));
    }

    let response = if is_error {
        if subtype != "error_max_turns" && subtype != "error_during_execution" {
            return Err(ServiceError::AgentOutputMissing(format!(
                "error result has unsupported subtype {subtype:?}"
            )));
        }
        result
            .get("error")
            .and_then(serde_json::Value::as_object)
            .and_then(|error| error.get("message"))
            .and_then(serde_json::Value::as_str)
            .filter(|message| !message.is_empty())
            .ok_or_else(|| {
                ServiceError::AgentOutputMissing(
                    "error result lacks non-empty error.message".into(),
                )
            })?
            .to_string()
    } else {
        if subtype != "success" {
            return Err(ServiceError::AgentOutputMissing(format!(
                "successful result has unsupported subtype {subtype:?}"
            )));
        }
        result
            .get("result")
            .and_then(serde_json::Value::as_str)
            .filter(|message| !message.is_empty())
            .ok_or_else(|| {
                ServiceError::AgentOutputMissing(
                    "successful result lacks a non-empty result string".into(),
                )
            })?
            .to_string()
    };

    Ok(AgentResult {
        is_error,
        response,
        duration_ms,
        num_turns,
    })
}

pub(crate) fn read_bounded_record<R: BufRead>(
    reader: &mut R,
    record: &mut Vec<u8>,
    path: &Path,
) -> ServiceResult<bool> {
    loop {
        let available = reader.fill_buf().map_err(|error| {
            ServiceError::AgentOutputMissing(io_msg(
                "read opened events.jsonl",
                path,
                &error,
            ))
        })?;
        if available.is_empty() {
            return Ok(false);
        }
        let take = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |index| index + 1);
        let next_len = record.len().checked_add(take).ok_or_else(|| {
            ServiceError::AgentOutputMissing("events.jsonl record length overflowed".into())
        })?;
        if next_len > MAX_EVENT_RECORD_BYTES {
            return Err(ServiceError::AgentOutputMissing(format!(
                "events.jsonl at {} contains a single record larger than the exact {MAX_EVENT_RECORD_BYTES}-byte protocol bound",
                path.display()
            )));
        }
        let terminated = available.get(take.saturating_sub(1)) == Some(&b'\n');
        record.extend_from_slice(&available[..take]);
        reader.consume(take);
        if terminated {
            return Ok(true);
        }
    }
}

fn validate_init_event(
    object: &serde_json::Map<String, serde_json::Value>,
    line: usize,
) -> ServiceResult<()> {
    let exact_string = |key: &str, expected: &str| -> ServiceResult<()> {
        let actual = required_string(object, key, line)?;
        if actual == expected {
            Ok(())
        } else {
            Err(ServiceError::AgentOutputMissing(format!(
                "events.jsonl init field {key} must be {expected:?}, got {actual:?}"
            )))
        }
    };
    exact_string("type", "system")?;
    exact_string("subtype", "init")?;
    exact_string("cwd", "/workspace")?;
    exact_string("model", "qwen3.8-27b-nvfp4-k8v4")?;
    exact_string("permission_mode", "yolo")?;
    exact_string("qwen_code_version", "0.21.12")?;

    let exact_string_array = |key: &str, expected: &[&str]| -> ServiceResult<()> {
        let values = object
            .get(key)
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                ServiceError::AgentOutputMissing(format!(
                    "events.jsonl init field {key} must be an array"
                ))
            })?;
        let mut actual = values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        ServiceError::AgentOutputMissing(format!(
                            "events.jsonl init field {key} contains a non-string or empty value"
                        ))
                    })
            })
            .collect::<ServiceResult<Vec<_>>>()?;
        actual.sort_unstable();
        let mut wanted = expected.to_vec();
        wanted.sort_unstable();
        if actual == wanted {
            Ok(())
        } else {
            Err(ServiceError::AgentOutputMissing(format!(
                "events.jsonl init field {key} differs from the pinned contract: expected {wanted:?}, got {actual:?}"
            )))
        }
    };
    exact_string_array(
        "tools",
        &[
            "agent",
            "edit",
            "glob",
            "grep_search",
            "list_directory",
            "notebook_edit",
            "read_file",
            "run_shell_command",
            "todo_write",
            "write_file",
        ],
    )?;
    exact_string_array("agents", &["Explore", "general-purpose"])?;
    let mcp_servers = object
        .get("mcp_servers")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            ServiceError::AgentOutputMissing(
                "events.jsonl init field mcp_servers must be an array".into(),
            )
        })?;
    if !mcp_servers.is_empty() {
        return Err(ServiceError::AgentOutputMissing(format!(
            "events.jsonl init advertised unexpected MCP servers: {mcp_servers:?}"
        )));
    }
    exact_string_array("slash_commands", &[])?;
    Ok(())
}

/// Every emitted event names its scope: `null` and an absent field both mean
/// the main session, any other value is the owning `agent` tool-call id. Only
/// null-or-absent is read as the main session, so a value of any other shape
/// can only ever exclude an event from the main thread, never admit one to it.
fn is_main_session_event(object: &serde_json::Map<String, serde_json::Value>) -> bool {
    object
        .get("parent_tool_use_id")
        .is_none_or(serde_json::Value::is_null)
}

/// Refuse an event whose scope is neither the main session nor an agent
/// tool-call id. Which record terminates the stream is decided from this
/// field, so a value of an unexpected shape is contradictory evidence and is
/// rejected here rather than silently read as "some subagent".
fn require_parent_scope(
    object: &serde_json::Map<String, serde_json::Value>,
    line: usize,
) -> ServiceResult<()> {
    match object.get("parent_tool_use_id") {
        None | Some(serde_json::Value::Null) => Ok(()),
        Some(serde_json::Value::String(id)) if !id.is_empty() => Ok(()),
        Some(other) => Err(ServiceError::AgentOutputMissing(format!(
            "events.jsonl line {line} has parent_tool_use_id {other}, which is neither null nor a non-empty agent tool-call id"
        ))),
    }
}

pub(crate) fn is_completed_main_turn(object: &serde_json::Map<String, serde_json::Value>) -> bool {
    object.get("type").and_then(serde_json::Value::as_str) == Some("assistant")
        && is_main_session_event(object)
        && object
            .get("message")
            .and_then(serde_json::Value::as_object)
            .and_then(|message| message.get("usage"))
            .and_then(serde_json::Value::as_object)
            .and_then(|usage| usage.get("input_tokens"))
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|tokens| tokens > 0)
}

fn required_string<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
    line: usize,
) -> ServiceResult<&'a str> {
    object
        .get(key)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ServiceError::AgentOutputMissing(format!(
                "events.jsonl line {line} lacks non-empty string {key}"
            ))
        })
}

fn required_u64(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> ServiceResult<u64> {
    object
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            ServiceError::AgentOutputMissing(format!(
                "terminal result lacks non-negative integer {key}"
            ))
        })
}

fn truncate_bytes(value: &[u8], max: usize) -> String {
    let lossy = String::from_utf8_lossy(value);
    if lossy.chars().count() <= max {
        lossy.into_owned()
    } else {
        format!(
            "{}…(truncated)",
            lossy.chars().take(max).collect::<String>()
        )
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    use super::*;

    const INIT: &str = "{\"type\":\"system\",\"subtype\":\"init\",\"uuid\":\"u1\",\"session_id\":\"a\",\"cwd\":\"/workspace\",\"tools\":[\"agent\",\"edit\",\"glob\",\"grep_search\",\"list_directory\",\"notebook_edit\",\"read_file\",\"run_shell_command\",\"todo_write\",\"write_file\"],\"mcp_servers\":[],\"model\":\"qwen3.8-27b-nvfp4-k8v4\",\"permission_mode\":\"yolo\",\"slash_commands\":[],\"qwen_code_version\":\"0.21.12\",\"agents\":[\"general-purpose\",\"Explore\"]}\n";

    // One completed main turn, one completed subagent turn under the agent
    // tool call that owns it, that subagent's own terminal record, and the
    // session's own terminal record. The subagent turn is deliberately billed
    // so that only the scope rule can exclude it from the main-turn count.
    const MAIN_TURN: &str = "{\"type\":\"assistant\",\"uuid\":\"u2\",\"session_id\":\"a\",\"parent_tool_use_id\":null,\"message\":{\"usage\":{\"input_tokens\":42}}}\n";
    const SUBAGENT_TURN: &str = "{\"type\":\"assistant\",\"uuid\":\"u3\",\"session_id\":\"a\",\"parent_tool_use_id\":\"chatcmpl-tool-9d45d85b\",\"message\":{\"usage\":{\"input_tokens\":11}}}\n";
    const SUBAGENT_RESULT: &str = "{\"type\":\"result\",\"subtype\":\"error_during_execution\",\"uuid\":\"u4\",\"session_id\":\"a\",\"parent_tool_use_id\":\"chatcmpl-tool-9d45d85b\",\"is_error\":true,\"duration_ms\":0,\"duration_api_ms\":0,\"num_turns\":3,\"usage\":{},\"permission_denials\":[],\"error\":{\"message\":\"MAX_TURNS\"}}\n";
    const MAIN_RESULT: &str = "{\"type\":\"result\",\"subtype\":\"success\",\"uuid\":\"u5\",\"session_id\":\"a\",\"is_error\":false,\"duration_ms\":2,\"duration_api_ms\":1,\"num_turns\":1,\"result\":\"ok\",\"usage\":{},\"permission_denials\":[]}\n";

    fn parse_text(text: &str) -> ServiceResult<AgentResult> {
        let path = std::env::temp_dir().join(format!(
            "agent-service-result-{}.jsonl",
            uuid::Uuid::new_v4().simple()
        ));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .expect("test creates private event file");
        file.write_all(text.as_bytes())
            .and_then(|_| file.sync_all())
            .expect("test writes and syncs event file");
        if unsafe { libc::geteuid() } == 0 {
            std::os::unix::fs::chown(&path, Some(1000), Some(1000))
                .expect("construct exact production event owner");
        }
        let result = parse_events_jsonl(&path);
        std::fs::remove_file(path).expect("test removes temp event file");
        result
    }

    #[test]
    fn accepts_one_terminal_success() {
        let text = format!(
            "{INIT}{{\"type\":\"assistant\",\"uuid\":\"u2\",\"session_id\":\"a\",\"parent_tool_use_id\":null,\"message\":{{\"usage\":{{\"input_tokens\":42}}}}}}\n{{\"type\":\"result\",\"subtype\":\"success\",\"uuid\":\"u3\",\"session_id\":\"a\",\"is_error\":false,\"duration_ms\":2,\"duration_api_ms\":1,\"num_turns\":1,\"result\":\"ok\",\"usage\":{{}},\"permission_denials\":[]}}\n"
        );
        let parsed = parse_text(&text).expect("strict valid stream parses");
        assert_eq!(parsed.response, "ok");
        assert_eq!(parsed.num_turns, 1);
    }

    #[test]
    fn a_subagent_result_does_not_terminate_the_session() {
        // A foreground subagent that exhausts its inherited turn budget emits
        // its own result under its agent tool-call id, and the parent then
        // recovers and finishes the session normally.
        let text = format!("{INIT}{MAIN_TURN}{SUBAGENT_TURN}{SUBAGENT_RESULT}{MAIN_RESULT}");
        let parsed = parse_text(&text)
            .expect("a subagent's result belongs to the subagent, not to the session");
        assert!(!parsed.is_error);
        assert_eq!(parsed.response, "ok");
        // The subagent's billed turn is the subagent's own, so the session's
        // main-turn count and its terminal cross-check are unchanged by it.
        assert_eq!(parsed.num_turns, 1);
    }

    #[test]
    fn rejects_a_stream_that_ends_at_a_subagent_result() {
        let text = format!("{INIT}{MAIN_TURN}{SUBAGENT_TURN}{SUBAGENT_RESULT}");
        let error = parse_text(&text).expect_err("the session itself never reported an outcome");
        assert!(error
            .to_string()
            .contains("no main-session terminal result"));
    }

    #[test]
    fn rejects_a_subagent_result_after_the_session_result() {
        let text = format!("{INIT}{MAIN_TURN}{MAIN_RESULT}{SUBAGENT_RESULT}");
        let error =
            parse_text(&text).expect_err("nothing may follow the session's own terminal result");
        assert!(error.to_string().contains("is followed by another event"));
    }

    #[test]
    fn rejects_a_scope_that_is_neither_null_nor_a_tool_call_id() {
        for malformed in ["0", "\"\"", "[]", "false", "{}"] {
            let corrupt = MAIN_TURN.replace(
                "\"parent_tool_use_id\":null",
                &format!("\"parent_tool_use_id\":{malformed}"),
            );
            let text = format!("{INIT}{corrupt}{MAIN_RESULT}");
            let error = parse_text(&text).expect_err(
                "a scope of an unexpected shape is contradictory evidence, not a subagent",
            );
            assert!(error.to_string().contains("parent_tool_use_id"));
        }
    }

    #[test]
    fn rejects_duplicate_or_post_terminal_events() {
        let result = "{\"type\":\"result\",\"subtype\":\"success\",\"uuid\":\"u2\",\"session_id\":\"a\",\"is_error\":false,\"duration_ms\":2,\"duration_api_ms\":1,\"num_turns\":0,\"result\":\"ok\",\"usage\":{},\"permission_denials\":[]}\n";
        assert!(parse_text(&format!("{INIT}{result}{result}")).is_err());
        assert!(parse_text(&format!("{INIT}{result}{INIT}")).is_err());
    }

    #[test]
    fn rejects_malformed_and_truncated_streams() {
        assert!(parse_text("not-json\n").is_err());
        assert!(parse_text(INIT).is_err());
        assert!(
            parse_text("{\"type\":\"system\",\"subtype\":\"init\",\"uuid\":\"u1\"}\n").is_err()
        );
        assert!(
            parse_text("{\"type\":\"stream_event\",\"uuid\":\"u1\",\"session_id\":\"a\"}\n")
                .is_err()
        );
        let complete_looking_but_torn = format!(
            "{INIT}{{\"type\":\"result\",\"subtype\":\"success\",\"uuid\":\"u2\",\"session_id\":\"a\",\"is_error\":false,\"duration_ms\":2,\"duration_api_ms\":1,\"num_turns\":0,\"result\":\"ok\",\"usage\":{{}},\"permission_denials\":[]}}"
        );
        let error = parse_text(&complete_looking_but_torn)
            .expect_err("a terminal JSON object without its record delimiter is torn evidence");
        assert!(error.to_string().contains("not newline-terminated"));
    }

    #[test]
    fn rejects_advertised_slash_commands() {
        let unexpected = INIT.replace("\"slash_commands\":[]", "\"slash_commands\":[\"status\"]");
        assert!(parse_text(&unexpected).is_err());
    }
}
