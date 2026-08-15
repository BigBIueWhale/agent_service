//! Strict parser for pinned Qwen Code 0.21.12 stream-JSON.
//!
//! Exactly one terminal `result` object is required and it must be the final
//! non-empty line. Every line must be a JSON object with a supported `type`,
//! a non-empty UUID, and the same non-empty session ID. This deliberately
//! rejects partial, duplicated, recovered, or post-terminal output rather
//! than choosing a convenient-looking last result.

use std::path::Path;

use crate::error::{io_msg, ServiceError, ServiceResult};

#[derive(Debug, Clone)]
pub struct AgentResult {
    pub is_error: bool,
    pub response: String,
    pub duration_ms: u64,
    pub num_turns: u64,
}

pub fn parse_events_jsonl(path: &Path) -> ServiceResult<AgentResult> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        ServiceError::AgentOutputMissing(io_msg("read events.jsonl", path, &error))
    })?;
    let lines = text
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let trimmed = line.trim();
            (!trimmed.is_empty()).then_some((index + 1, trimmed))
        })
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return Err(ServiceError::AgentOutputMissing(format!(
            "events.jsonl at {} contains no events",
            path.display()
        )));
    }

    let mut stream_session_id: Option<String> = None;
    let mut result: Option<serde_json::Map<String, serde_json::Value>> = None;
    let mut main_assistant_events = 0u64;

    for (position, (line_number, line)) in lines.iter().enumerate() {
        let value: serde_json::Value = serde_json::from_str(line).map_err(|error| {
            ServiceError::AgentOutputMissing(format!(
                "events.jsonl line {line_number} is invalid JSON: {error}; content: {}",
                truncate(line, 512)
            ))
        })?;
        let object = value.as_object().ok_or_else(|| {
            ServiceError::AgentOutputMissing(format!(
                "events.jsonl line {line_number} is not a JSON object"
            ))
        })?;
        let event_type = required_string(object, "type", *line_number)?;
        if !matches!(event_type, "system" | "user" | "assistant" | "result") {
            return Err(ServiceError::AgentOutputMissing(format!(
                "events.jsonl line {line_number} has unsupported event type {event_type:?}"
            )));
        }
        required_string(object, "uuid", *line_number)?;
        if position == 0 {
            validate_init_event(object, *line_number)?;
        }
        let value = required_string(object, "session_id", *line_number)?;
        match &stream_session_id {
            Some(expected) if expected != value => {
                return Err(ServiceError::AgentOutputMissing(format!(
                    "events.jsonl session_id changed from {expected:?} to {value:?} at line {line_number}"
                )));
            }
            None => stream_session_id = Some(value.to_string()),
            _ => {}
        }

        if is_completed_main_turn(object) {
            main_assistant_events = main_assistant_events.saturating_add(1);
        }
        if event_type == "result" {
            if result.is_some() {
                return Err(ServiceError::AgentOutputMissing(format!(
                    "events.jsonl contains more than one terminal result (duplicate at line {line_number})"
                )));
            }
            if position + 1 != lines.len() {
                return Err(ServiceError::AgentOutputMissing(format!(
                    "events.jsonl result at line {line_number} is not terminal; {} event(s) follow it",
                    lines.len() - position - 1
                )));
            }
            result = Some(object.clone());
        }
    }

    let result = result.ok_or_else(|| {
        ServiceError::AgentOutputMissing(format!(
            "events.jsonl has {} event(s) but no terminal result",
            lines.len()
        ))
    })?;
    let is_error = result
        .get("is_error")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| {
            ServiceError::AgentOutputMissing("terminal result lacks boolean is_error".into())
        })?;
    let subtype = required_string(&result, "subtype", lines.last().map(|x| x.0).unwrap_or(0))?;
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
    if !object
        .get("slash_commands")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|values| {
            !values.is_empty()
                && values
                    .iter()
                    .all(|value| value.as_str().is_some_and(|value| !value.is_empty()))
        })
    {
        return Err(ServiceError::AgentOutputMissing(
            "events.jsonl init lacks a non-empty string-array slash_commands field".into(),
        ));
    }
    Ok(())
}

pub(crate) fn is_completed_main_turn(object: &serde_json::Map<String, serde_json::Value>) -> bool {
    object.get("type").and_then(serde_json::Value::as_str) == Some("assistant")
        && object
            .get("parent_tool_use_id")
            .is_none_or(serde_json::Value::is_null)
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

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        value.into()
    } else {
        format!(
            "{}…(truncated)",
            value.chars().take(max).collect::<String>()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INIT: &str = "{\"type\":\"system\",\"subtype\":\"init\",\"uuid\":\"u1\",\"session_id\":\"a\",\"cwd\":\"/workspace\",\"tools\":[\"agent\",\"edit\",\"glob\",\"grep_search\",\"list_directory\",\"notebook_edit\",\"read_file\",\"run_shell_command\",\"todo_write\",\"write_file\"],\"mcp_servers\":[],\"model\":\"qwen3.8-27b-nvfp4-k8v4\",\"permission_mode\":\"yolo\",\"slash_commands\":[\"status\"],\"qwen_code_version\":\"0.21.12\",\"agents\":[\"general-purpose\",\"Explore\"]}\n";

    fn parse_text(text: &str) -> ServiceResult<AgentResult> {
        let path = std::env::temp_dir().join(format!(
            "agent-service-result-{}.jsonl",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::write(&path, text).expect("test writes temp event file");
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
    }
}
