//! Parser for the agent's stream-json output file.
//!
//! Qwen Code 0.15.6 in `--output-format stream-json` emits one JSON object
//! per line. The final line of interest has `type: "result"` and carries the
//! agent's final answer in `result` (or, if `is_error` is true, a structured
//! error payload).
//!
//! Citations:
//! - `/tmp/research/qwen-code-src/packages/cli/src/nonInteractive/io/BaseJsonOutputAdapter.ts`
//!   lines 1159-1212 — `buildResultMessage()`.
//! - `/tmp/research/qwen-code-src/docs/users/features/headless.md` lines 127-167
//!   — documented event shape.

use std::path::Path;

use crate::error::{ServiceError, ServiceResult, io_msg};

#[derive(Debug, Clone)]
pub struct AgentResult {
    pub is_error: bool,
    /// Final answer text (success) or error message (failure). Always
    /// populated, never empty — we synthesise a placeholder if the upstream
    /// event is malformed, since downstream consumers rely on the field.
    pub response: String,
    /// Number of agent turns that ran (0 if the upstream event omitted it).
    pub num_turns: u64,
    /// Wall-clock duration the agent spent, in ms (0 if upstream omitted).
    pub duration_ms: u64,
}

pub fn parse_events_jsonl(path: &Path) -> ServiceResult<AgentResult> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        ServiceError::AgentOutputMissing(io_msg("read events.jsonl", path, &e))
    })?;
    if text.trim().is_empty() {
        return Err(ServiceError::AgentOutputMissing(format!(
            "events.jsonl at {} is empty — the agent produced no structured output. The container likely crashed before qwen wrote any event; check `docker logs` of the agent container if it's still around.",
            path.display()
        )));
    }

    let mut last_result: Option<serde_json::Value> = None;
    let mut last_seen_event_type: Option<String> = None;
    let mut total_lines = 0;

    for (i, raw_line) in text.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        total_lines += 1;
        let v: serde_json::Value = serde_json::from_str(line).map_err(|e| {
            ServiceError::AgentOutputMissing(format!(
                "events.jsonl line {} is not valid JSON: {e}; line content: {}",
                i + 1,
                truncate(line, 256),
            ))
        })?;
        if let Some(t) = v.get("type").and_then(|x| x.as_str()) {
            last_seen_event_type = Some(t.to_string());
            if t == "result" {
                last_result = Some(v);
            }
        }
    }

    let result_event = match last_result {
        Some(v) => v,
        None => {
            return Err(ServiceError::AgentOutputMissing(format!(
                "events.jsonl had {total_lines} non-empty lines but no `type:\"result\"` event. Last seen event type: {:?}. The agent likely exited mid-loop before emitting its final result.",
                last_seen_event_type.unwrap_or_else(|| "<none>".into())
            )));
        }
    };

    let is_error = result_event
        .get("is_error")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let response = if is_error {
        // Error shape: { is_error: true, error: { message: "...", ... } }
        // Or older alt: error_message at top level.
        let from_obj = result_event
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .map(str::to_owned);
        let from_top = result_event
            .get("error_message")
            .and_then(|m| m.as_str())
            .map(str::to_owned);
        from_obj.or(from_top).unwrap_or_else(|| {
            format!(
                "agent reported is_error=true but did not provide a structured message; raw event: {}",
                truncate(&result_event.to_string(), 1024)
            )
        })
    } else {
        match result_event.get("result").and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            Some(_) => "<agent emitted an empty result string>".to_string(),
            None => {
                return Err(ServiceError::AgentOutputMissing(format!(
                    "result event present but has no `result` string field; event: {}",
                    truncate(&result_event.to_string(), 1024)
                )));
            }
        }
    };

    let num_turns = result_event
        .get("num_turns")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let duration_ms = result_event
        .get("duration_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    Ok(AgentResult {
        is_error,
        response,
        num_turns,
        duration_ms,
    })
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{head}…(truncated)")
    }
}
