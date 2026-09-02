//! Strict parser for pinned Qwen Code 0.21.12 stream-JSON.
//!
//! Every event names its scope in `parent_tool_use_id`: null (or absent) is
//! the main session, a non-empty string is the id of the `tool_use` content
//! block that spawned that subagent. Correlation is by id and stream order
//! alone — never by tool name — so a non-null scope must resolve to a
//! `tool_use` already issued by an assistant message, and one that does not
//! is rejected rather than pooled into some "unknown subagent" bucket. Every
//! scope, the main session included, is one row of the same accounting
//! table: billed turns, plus at most one terminal `result`. A subagent that
//! stops emits its own `result` under its tool-call id, so `result` alone
//! does not mean the session ended.
//!
//! Exactly one terminal `result` object *for the main session* is required and
//! it must be the final non-empty line. Every line must be a JSON object with
//! a supported `type`, a non-empty UUID, and the same non-empty session ID.
//! This deliberately rejects partial, duplicated, recovered, or post-terminal
//! output rather than choosing a convenient-looking last result.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{io_msg, ServiceError, ServiceResult};

#[derive(Debug, Clone)]
pub struct AgentResult {
    pub is_error: bool,
    pub response: String,
    pub duration_ms: u64,
    /// Wall time the agent spent inside model API calls, as the terminal
    /// result reports it. Already required and type-checked here; carrying it
    /// is what separates a run that stalled on the backend from one that
    /// churned in local tool execution, which `duration_ms` alone cannot.
    pub api_duration_ms: u64,
    pub num_turns: u64,
    /// Main-scope assistant events carrying billed usage: the turns the agent
    /// both started and finished. Equal to `num_turns` on a run that ended
    /// normally, one less when an error ended the run inside a turn.
    pub billed_main_turns: u64,
    /// Every subagent scope the stream resolved, in order of first
    /// appearance. Empty exactly when the run delegated nothing.
    pub scopes: Vec<AgentScope>,
}

/// One resolved subagent scope. Identification follows the Claude Code CLI
/// convention: a scope is the id of the `tool_use` content block that spawned
/// it, so consumers never need to know which tool performs delegation — and
/// this parser never assumes one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentScope {
    /// Id of the spawning `tool_use` block; the exact value every event in
    /// the scope carried in `parent_tool_use_id`.
    pub tool_use_id: String,
    /// Name of that spawning tool call. Recorded as evidence for the reader,
    /// never used as a correlation key: resolution is by id alone.
    pub tool_name: String,
    /// Assistant events in this scope carrying billed usage, counted by this
    /// parser. Usage rides the main session's model stream, so a subagent
    /// scope's assistant events carry none and this is zero for every
    /// subagent; the count a subagent reports for itself is
    /// `reported_num_turns`.
    pub billed_turns: u64,
    /// What the scope's own terminal record reported, verbatim. All four are
    /// `None` exactly when the scope never emitted a terminal record (the
    /// subagent was still running, or was torn down, when the session ended);
    /// `error_message` is additionally `None` when the record carried no
    /// error. `reported_num_turns` is deliberately never reconciled against
    /// `billed_turns` — see `finish_subagent_scope` for why.
    pub reported_num_turns: Option<u64>,
    pub is_error: Option<bool>,
    pub subtype: Option<String>,
    pub error_message: Option<String>,
}

/// A `tool_use` content block an assistant message already issued: the only
/// thing a later `parent_tool_use_id` may legally name. The recorded values
/// are what a scope needs at assembly (`name`) and what a rejection needs to
/// point back at the spawning call (`line`, issuing `scope`).
struct RecordedToolUse {
    name: String,
    line: usize,
    scope: Option<String>,
}

/// Accounting for one scope. The main session is the row keyed `None`,
/// created before the first event is read, so "main" is a key in the one
/// table rather than a parallel code path — there is exactly one accounting
/// mechanism for every scope in the stream.
struct ScopeState {
    tool_use_id: Option<String>,
    billed_turns: u64,
    /// The scope's terminal `result` record and its line. At most one may
    /// exist: a scope that has reported is finished, and any later event in
    /// it is post-terminal output.
    terminal: Option<(usize, serde_json::Map<String, serde_json::Value>)>,
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
    // The scope table. Row 0 is the main session, keyed `None`; a subagent
    // row is created the moment its first event resolves. The vector keeps
    // stream order of first appearance, the index map keeps lookup exact.
    let mut scope_states = vec![ScopeState {
        tool_use_id: None,
        billed_turns: 0,
        terminal: None,
    }];
    let mut scope_rows: HashMap<String, usize> = HashMap::new();
    let mut tool_uses: HashMap<String, RecordedToolUse> = HashMap::new();
    let mut physical_line = 0usize;
    let mut event_count = 0usize;
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
        if let Some((terminal_line, _)) = &scope_states[0].terminal {
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

        let scope = required_scope(object, physical_line)?;
        // Resolve the event to its scope row. A non-null scope id is only
        // admissible if the stream has already issued the `tool_use` block it
        // names: resolution is by id and stream order, never by tool name,
        // and an id with no earlier issuance is contradictory evidence.
        // There is deliberately no "unknown scope" bucket to absorb it.
        let row = match scope {
            None => 0,
            Some(id) => match scope_rows.get(id) {
                Some(row) => *row,
                None => {
                    if !tool_uses.contains_key(id) {
                        return Err(ServiceError::AgentOutputMissing(format!(
                            "events.jsonl line {physical_line} names parent_tool_use_id {id:?}, which no earlier assistant message issued as a tool_use id"
                        )));
                    }
                    let row = scope_states.len();
                    scope_rows.insert(id.to_string(), row);
                    scope_states.push(ScopeState {
                        tool_use_id: Some(id.to_string()),
                        billed_turns: 0,
                        terminal: None,
                    });
                    row
                }
            },
        };
        // A scope that has emitted its terminal record is finished. The main
        // session's record ends the whole stream (checked above, before the
        // event is even parsed); a subagent's ends only its own scope, and a
        // later event still claiming that scope is post-terminal output.
        if let Some((scope_terminal_line, _)) = &scope_states[row].terminal {
            return Err(ServiceError::AgentOutputMissing(format!(
                "events.jsonl line {physical_line} continues {} after its terminal result at line {scope_terminal_line}",
                scope_display(scope)
            )));
        }
        if event_type == "assistant" {
            record_tool_uses(object, physical_line, scope, &mut tool_uses)?;
            if has_billed_usage(object) {
                let billed = &mut scope_states[row].billed_turns;
                *billed = billed.checked_add(1).ok_or_else(|| {
                    ServiceError::AgentOutputMissing(format!(
                        "events.jsonl billed turn count overflowed in {}",
                        scope_display(scope)
                    ))
                })?;
            }
        } else if event_type == "result" {
            // A subagent's own terminal record is scoped to its spawning tool
            // call and is not the end of the session. Only the main session's
            // result terminates the stream.
            scope_states[row].terminal = Some((physical_line, object.clone()));
        }
    }

    if event_count == 0 {
        return Err(ServiceError::AgentOutputMissing(format!(
            "events.jsonl at {} contains no events",
            path.display()
        )));
    }

    let mut rows = scope_states.into_iter();
    let main = rows
        .next()
        .expect("the main scope row is created before the first event is read");
    let billed_main_turns = main.billed_turns;
    let (terminal_line, result) = main.terminal.ok_or_else(|| {
        ServiceError::AgentOutputMissing(format!(
            "events.jsonl has {event_count} event(s) but no main-session terminal result"
        ))
    })?;
    let scopes = rows
        .map(|state| finish_subagent_scope(state, &tool_uses))
        .collect::<ServiceResult<Vec<_>>>()?;
    let is_error = result
        .get("is_error")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| {
            ServiceError::AgentOutputMissing("terminal result lacks boolean is_error".into())
        })?;
    let subtype = required_string(&result, "subtype", terminal_line)?;
    let duration_ms = required_u64(&result, "duration_ms")?;
    let num_turns = required_u64(&result, "num_turns")?;
    let api_duration_ms = required_u64(&result, "duration_api_ms")?;
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
    // `num_turns` counts the turns the agent started: the counter advances as
    // a turn begins, while a billed assistant event — the thing counted above
    // — is only written once that turn finishes. A run that ends normally has
    // finished every turn it started, so the two must agree exactly. An error
    // ends the run wherever it struck: between turns, leaving the counts
    // equal, or inside the turn already counted, leaving exactly one started
    // and unbilled turn behind. Anything outside that window means the result
    // event does not describe this stream, which is what this check exists to
    // catch.
    let unbilled = num_turns.checked_sub(billed_main_turns);
    let consistent = match unbilled {
        Some(0) => true,
        Some(1) => is_error,
        _ => false,
    };
    if !consistent {
        return Err(ServiceError::AgentOutputMissing(format!(
            "terminal num_turns={num_turns} is not consistent with {billed_main_turns} \
             main assistant event(s): a run bills every turn it finishes, and only an error \
             result may leave the one turn it interrupted unbilled"
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
        api_duration_ms,
        num_turns,
        billed_main_turns,
        scopes,
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

/// Read an event's scope: `None` is the main session, `Some(id)` the
/// spawning tool-call id. Which record terminates the stream and which row
/// absorbs the billing are both decided from this field, so a value of an
/// unexpected shape is contradictory evidence and is rejected here rather
/// than silently read as "some subagent".
fn required_scope(
    object: &serde_json::Map<String, serde_json::Value>,
    line: usize,
) -> ServiceResult<Option<&str>> {
    match object.get("parent_tool_use_id") {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(id)) if !id.is_empty() => Ok(Some(id)),
        Some(other) => Err(ServiceError::AgentOutputMissing(format!(
            "events.jsonl line {line} has parent_tool_use_id {other}, which is neither null nor a non-empty agent tool-call id"
        ))),
    }
}

/// Name a scope the way a rejection message needs to: by what the reader can
/// find in the stream, not by a row index that exists only in this parser.
fn scope_display(scope: Option<&str>) -> String {
    match scope {
        None => "the main session".into(),
        Some(id) => format!("subagent scope {id:?}"),
    }
}

/// Record every `tool_use` content block an assistant message issues:
/// id → (tool name, line, issuing scope). This table is the sole resolution
/// authority for `parent_tool_use_id`, which is why a block that claims to
/// be a tool_use but lacks a usable id or name is refused instead of
/// skipped: skipping it would orphan every event of the scope it was about
/// to spawn. A duplicate id is refused for the same reason — correlation is
/// by id, and a second issuance would make every later reference ambiguous.
fn record_tool_uses(
    object: &serde_json::Map<String, serde_json::Value>,
    line: usize,
    scope: Option<&str>,
    tool_uses: &mut HashMap<String, RecordedToolUse>,
) -> ServiceResult<()> {
    let Some(blocks) = object
        .get("message")
        .and_then(serde_json::Value::as_object)
        .and_then(|message| message.get("content"))
        .and_then(serde_json::Value::as_array)
    else {
        return Ok(());
    };
    for block in blocks {
        let Some(block) = block.as_object() else {
            continue;
        };
        if block.get("type").and_then(serde_json::Value::as_str) != Some("tool_use") {
            continue;
        }
        let required = |key: &str| -> ServiceResult<&str> {
            block
                .get(key)
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    ServiceError::AgentOutputMissing(format!(
                        "events.jsonl line {line} has a tool_use block lacking non-empty string {key}"
                    ))
                })
        };
        let id = required("id")?;
        let name = required("name")?;
        if let Some(previous) = tool_uses.get(id) {
            return Err(ServiceError::AgentOutputMissing(format!(
                "events.jsonl line {line} re-issues tool_use id {id:?}, first issued at line {} in {}",
                previous.line,
                scope_display(previous.scope.as_deref())
            )));
        }
        tool_uses.insert(
            id.to_string(),
            RecordedToolUse {
                name: name.to_string(),
                line,
                scope: scope.map(str::to_string),
            },
        );
    }
    Ok(())
}

/// Convert an accumulated subagent row into its public record.
///
/// Deliberate asymmetry with the main-scope `num_turns` cross-check in
/// `parse_events_jsonl`: a subagent's reported `num_turns` is recorded
/// verbatim and never reconciled against the turns this parser billed to the
/// scope. The pinned runner has a known defect here — a subagent terminated
/// by a thrown error reports `num_turns: 0` over genuinely billed turns, and
/// a MAX_TURNS abort reports every started turn while the stream billed
/// fewer — so applying the main-scope window would refuse real production
/// streams. Surfacing both numbers side by side is what lets a reader see
/// the contradiction; enforcing agreement would discard the very runs that
/// exhibit it.
fn finish_subagent_scope(
    state: ScopeState,
    tool_uses: &HashMap<String, RecordedToolUse>,
) -> ServiceResult<AgentScope> {
    let tool_use_id = state
        .tool_use_id
        .expect("only the pre-created main row carries no tool_use id");
    let tool_name = tool_uses
        .get(&tool_use_id)
        .expect("a scope row is created only after its spawning tool_use was recorded")
        .name
        .clone();
    let Some((line, terminal)) = state.terminal else {
        // The scope never reported: the subagent was still running, or was
        // torn down, when the main session ended. That is an observable
        // production state, not corruption — absent is the only honest value
        // for every terminal-record field, and inventing one would be the
        // exact fabrication this parser exists to refuse.
        return Ok(AgentScope {
            tool_use_id,
            tool_name,
            billed_turns: state.billed_turns,
            reported_num_turns: None,
            is_error: None,
            subtype: None,
            error_message: None,
        });
    };
    let subtype = required_string(&terminal, "subtype", line)?.to_string();
    let is_error = terminal
        .get("is_error")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| {
            ServiceError::AgentOutputMissing(format!(
                "subagent result at line {line} (scope {tool_use_id:?}) lacks boolean is_error"
            ))
        })?;
    let num_turns = terminal
        .get("num_turns")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            ServiceError::AgentOutputMissing(format!(
                "subagent result at line {line} (scope {tool_use_id:?}) lacks non-negative integer num_turns"
            ))
        })?;
    // `error.message` is recorded when present. An absent error object (or an
    // error object without a message) is a real state on a non-error result;
    // a present-but-malformed one is contradictory evidence and is refused
    // rather than read as "no message".
    let error_message = match terminal.get("error") {
        None => None,
        Some(error) => {
            let error = error.as_object().ok_or_else(|| {
                ServiceError::AgentOutputMissing(format!(
                    "subagent result at line {line} (scope {tool_use_id:?}) has a non-object error field"
                ))
            })?;
            match error.get("message") {
                None => None,
                Some(message) => Some(
                    message
                        .as_str()
                        .filter(|message| !message.is_empty())
                        .ok_or_else(|| {
                            ServiceError::AgentOutputMissing(format!(
                                "subagent result at line {line} (scope {tool_use_id:?}) has a non-string or empty error.message"
                            ))
                        })?
                        .to_string(),
                ),
            }
        }
    };
    Ok(AgentScope {
        tool_use_id,
        tool_name,
        billed_turns: state.billed_turns,
        reported_num_turns: Some(num_turns),
        is_error: Some(is_error),
        subtype: Some(subtype),
        error_message,
    })
}

/// True when the assistant message carries billed usage: the mark of a turn
/// that both started and finished. One definition, shared by the strict
/// terminal parser (per scope) and the live progress reader (main scope), so
/// the two counters can never drift on what a billed turn is — finalization
/// cross-checks their agreement.
fn has_billed_usage(object: &serde_json::Map<String, serde_json::Value>) -> bool {
    object
        .get("message")
        .and_then(serde_json::Value::as_object)
        .and_then(|message| message.get("usage"))
        .and_then(serde_json::Value::as_object)
        .and_then(|usage| usage.get("input_tokens"))
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|tokens| tokens > 0)
}

pub(crate) fn is_completed_main_turn(object: &serde_json::Map<String, serde_json::Value>) -> bool {
    object.get("type").and_then(serde_json::Value::as_str) == Some("assistant")
        && is_main_session_event(object)
        && has_billed_usage(object)
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

    // One completed main turn that issues the delegating tool_use, one
    // completed subagent turn under that tool call, the subagent's own
    // terminal record, and the session's own terminal record. The subagent
    // turn is deliberately billed so that only the scope rule can exclude it
    // from the main-turn count, and the subagent scope resolves only because
    // the main turn issued the tool_use id its events name.
    const MAIN_TURN: &str = "{\"type\":\"assistant\",\"uuid\":\"u2\",\"session_id\":\"a\",\"parent_tool_use_id\":null,\"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"chatcmpl-tool-9d45d85b\",\"name\":\"agent\",\"input\":{}}],\"usage\":{\"input_tokens\":42}}}\n";
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
        assert_eq!(parsed.billed_main_turns, 1);
        // A run that delegated nothing reports no scopes, not a fabricated
        // main-scope row: the main session is accounting, not a subagent.
        assert!(parsed.scopes.is_empty());
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
        assert_eq!(parsed.billed_main_turns, 1);
        // The same events that used to be validated and discarded are now the
        // scope's account: the spawning call, the turn billed under it, and
        // the terminal record it reported for itself — including the reported
        // num_turns of 3 that legitimately disagrees with the 1 billed turn.
        assert_eq!(parsed.scopes.len(), 1);
        let scope = &parsed.scopes[0];
        assert_eq!(scope.tool_use_id, "chatcmpl-tool-9d45d85b");
        assert_eq!(scope.tool_name, "agent");
        assert_eq!(scope.billed_turns, 1);
        assert_eq!(scope.reported_num_turns, Some(3));
        assert_eq!(scope.is_error, Some(true));
        assert_eq!(scope.subtype.as_deref(), Some("error_during_execution"));
        assert_eq!(scope.error_message.as_deref(), Some("MAX_TURNS"));
    }

    #[test]
    fn reports_an_error_that_ended_the_run_inside_a_turn() {
        // Test A: a stream error killed turn 236, so the terminal result
        // counts 236 started turns against 235 billed ones. The +1 is the
        // defined semantic, not corruption, and refusing the capture over it
        // hid the real cause behind "agent_output_missing" and threw away the
        // run's real timings.
        let error_result = "{\"type\":\"result\",\"subtype\":\"error_during_execution\",\"uuid\":\"u5\",\"session_id\":\"a\",\"is_error\":true,\"duration_ms\":19180714,\"duration_api_ms\":18037819,\"num_turns\":2,\"usage\":{},\"permission_denials\":[],\"error\":{\"message\":\"[API Error: Context is too large to send safely after automatic compression.]\"}}\n";
        let text = format!("{INIT}{MAIN_TURN}{error_result}");
        let parsed = parse_text(&text).expect("an error result may leave its final turn unbilled");
        assert!(parsed.is_error);
        assert!(parsed.response.contains("Context is too large"));
        assert_eq!(parsed.num_turns, 2);
        assert_eq!(parsed.billed_main_turns, 1);
        // The real timings survive; they were the whole point of parsing it.
        assert_eq!(parsed.duration_ms, 19_180_714);
        // Including the API half. This run spent 18,037,819 ms of its
        // 19,180,714 ms inside model calls -- the fact that separates a
        // backend stall from a local tool loop. It was required and
        // type-checked and then dropped, so the terminal record could report
        // how long the run took but never where the time went.
        assert_eq!(parsed.api_duration_ms, 18_037_819);
    }

    #[test]
    fn refuses_a_terminal_result_whose_api_duration_is_not_a_count() {
        // Carrying the value does not weaken the check that produced it: the
        // field stays required and stays fail-closed.
        for bad in [
            "\"duration_api_ms\":-1",
            "\"duration_api_ms\":\"18037819\"",
            "\"duration_api_ms\":null",
        ] {
            let terminal = format!(
                "{{\"type\":\"result\",\"subtype\":\"success\",\"uuid\":\"u5\",\"session_id\":\"a\",\"is_error\":false,\"duration_ms\":2,{bad},\"num_turns\":1,\"result\":\"ok\",\"usage\":{{}},\"permission_denials\":[]}}\n"
            );
            let text = format!("{INIT}{MAIN_TURN}{terminal}");
            let error =
                parse_text(&text).expect_err("duration_api_ms must be a non-negative integer");
            assert!(error
                .to_string()
                .contains("terminal result lacks non-negative integer duration_api_ms"));
        }
    }

    #[test]
    fn an_error_between_turns_still_has_to_balance() {
        let error_result = "{\"type\":\"result\",\"subtype\":\"error_during_execution\",\"uuid\":\"u5\",\"session_id\":\"a\",\"is_error\":true,\"duration_ms\":9,\"duration_api_ms\":8,\"num_turns\":1,\"usage\":{},\"permission_denials\":[],\"error\":{\"message\":\"boom\"}}\n";
        let text = format!("{INIT}{MAIN_TURN}{error_result}");
        let parsed = parse_text(&text).expect("equal counts are valid for an error too");
        assert_eq!(parsed.num_turns, 1);
        assert_eq!(parsed.billed_main_turns, 1);
    }

    #[test]
    fn rejects_turn_counts_outside_the_defined_window() {
        // The integrity check is unchanged everywhere it matters: a success
        // must balance exactly, an error may be short by exactly one, and
        // nothing may claim fewer turns than the stream billed.
        let success_off_by_one = "{\"type\":\"result\",\"subtype\":\"success\",\"uuid\":\"u5\",\"session_id\":\"a\",\"is_error\":false,\"duration_ms\":2,\"duration_api_ms\":1,\"num_turns\":2,\"result\":\"ok\",\"usage\":{},\"permission_denials\":[]}\n";
        let error_off_by_two = "{\"type\":\"result\",\"subtype\":\"error_during_execution\",\"uuid\":\"u5\",\"session_id\":\"a\",\"is_error\":true,\"duration_ms\":2,\"duration_api_ms\":1,\"num_turns\":3,\"usage\":{},\"permission_denials\":[],\"error\":{\"message\":\"boom\"}}\n";
        let error_under_count = "{\"type\":\"result\",\"subtype\":\"error_during_execution\",\"uuid\":\"u5\",\"session_id\":\"a\",\"is_error\":true,\"duration_ms\":2,\"duration_api_ms\":1,\"num_turns\":0,\"usage\":{},\"permission_denials\":[],\"error\":{\"message\":\"boom\"}}\n";
        for terminal in [success_off_by_one, error_off_by_two, error_under_count] {
            let text = format!("{INIT}{MAIN_TURN}{terminal}");
            let error = parse_text(&text).expect_err("inconsistent turn accounting is refused");
            assert!(error.to_string().contains("is not consistent with"));
        }
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
    fn rejects_an_orphan_subagent_scope() {
        // Without the spawning main turn, the subagent's scope id names a
        // tool_use no assistant message ever issued. Identification is by id
        // against recorded evidence, so an unresolvable scope is refused by
        // line and id — never absorbed into an "unknown subagent" bucket.
        let text = format!("{INIT}{SUBAGENT_TURN}{MAIN_RESULT}");
        let error = parse_text(&text).expect_err("an orphan scope is contradictory evidence");
        let message = error.to_string();
        assert!(message.contains("line 2"));
        assert!(message.contains("chatcmpl-tool-9d45d85b"));
        assert!(message.contains("no earlier assistant message issued"));
    }

    #[test]
    fn rejects_a_scope_spawned_only_later_in_the_stream() {
        // The tool_use the scope names does appear — but only after the
        // scoped event. The stream is causal and the parse is one forward
        // pass: an event cannot ride a delegation that has not happened yet,
        // so resolution against a later issuance is refused at the line
        // where the premature reference occurred.
        let text = format!("{INIT}{SUBAGENT_TURN}{MAIN_TURN}{MAIN_RESULT}");
        let error = parse_text(&text).expect_err("a scope cannot borrow a future tool call");
        let message = error.to_string();
        assert!(message.contains("line 2"));
        assert!(message.contains("no earlier assistant message issued"));
    }

    #[test]
    fn resolves_a_subagent_scope_by_id_alone() {
        // The delegating call carries a name this parser has never heard of.
        // That is the point: Claude Code's convention correlates scopes by
        // tool_use id, never by tool name, so the scope must resolve and the
        // name must come back as recorded evidence, not as a filter.
        let spawn = "{\"type\":\"assistant\",\"uuid\":\"u2\",\"session_id\":\"a\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"call-77\",\"name\":\"workspace_janitor\",\"input\":{}}],\"usage\":{\"input_tokens\":9}}}\n";
        let sub_turn_one = "{\"type\":\"assistant\",\"uuid\":\"u3\",\"session_id\":\"a\",\"parent_tool_use_id\":\"call-77\",\"message\":{\"usage\":{\"input_tokens\":5}}}\n";
        let sub_turn_two = "{\"type\":\"assistant\",\"uuid\":\"u4\",\"session_id\":\"a\",\"parent_tool_use_id\":\"call-77\",\"message\":{\"usage\":{\"input_tokens\":6}}}\n";
        let sub_result = "{\"type\":\"result\",\"subtype\":\"success\",\"uuid\":\"u5\",\"session_id\":\"a\",\"parent_tool_use_id\":\"call-77\",\"is_error\":false,\"duration_ms\":4,\"duration_api_ms\":3,\"num_turns\":5,\"result\":\"sub done\",\"usage\":{},\"permission_denials\":[]}\n";
        let text = format!("{INIT}{spawn}{sub_turn_one}{sub_turn_two}{sub_result}{MAIN_RESULT}");
        let parsed = parse_text(&text).expect("an id-resolved scope parses");
        assert_eq!(parsed.billed_main_turns, 1);
        assert_eq!(parsed.scopes.len(), 1);
        let scope = &parsed.scopes[0];
        assert_eq!(scope.tool_use_id, "call-77");
        assert_eq!(scope.tool_name, "workspace_janitor");
        assert_eq!(scope.billed_turns, 2);
        // Recorded verbatim, not reconciled: the scope reports 5 started
        // turns over 2 billed ones and the stream is still valid.
        assert_eq!(scope.reported_num_turns, Some(5));
        assert_eq!(scope.is_error, Some(false));
        assert_eq!(scope.subtype.as_deref(), Some("success"));
        assert_eq!(scope.error_message, None);
    }

    #[test]
    fn accounts_two_subagent_scopes_separately() {
        let spawn = "{\"type\":\"assistant\",\"uuid\":\"u2\",\"session_id\":\"a\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"call-a\",\"name\":\"agent\",\"input\":{}},{\"type\":\"tool_use\",\"id\":\"call-b\",\"name\":\"background_probe\",\"input\":{}}],\"usage\":{\"input_tokens\":42}}}\n";
        let a_turn = "{\"type\":\"assistant\",\"uuid\":\"u3\",\"session_id\":\"a\",\"parent_tool_use_id\":\"call-a\",\"message\":{\"usage\":{\"input_tokens\":3}}}\n";
        let b_turn_one = "{\"type\":\"assistant\",\"uuid\":\"u4\",\"session_id\":\"a\",\"parent_tool_use_id\":\"call-b\",\"message\":{\"usage\":{\"input_tokens\":4}}}\n";
        let b_turn_two = "{\"type\":\"assistant\",\"uuid\":\"u5\",\"session_id\":\"a\",\"parent_tool_use_id\":\"call-b\",\"message\":{\"usage\":{\"input_tokens\":5}}}\n";
        let a_result = "{\"type\":\"result\",\"subtype\":\"error_during_execution\",\"uuid\":\"u6\",\"session_id\":\"a\",\"parent_tool_use_id\":\"call-a\",\"is_error\":true,\"duration_ms\":1,\"duration_api_ms\":1,\"num_turns\":1,\"usage\":{},\"permission_denials\":[],\"error\":{\"message\":\"boom-a\"}}\n";
        let text = format!("{INIT}{spawn}{a_turn}{b_turn_one}{b_turn_two}{a_result}{MAIN_RESULT}");
        let parsed = parse_text(&text).expect("independent scopes account independently");
        assert_eq!(parsed.billed_main_turns, 1);
        assert_eq!(parsed.scopes.len(), 2);
        // Stream order of first appearance, and strictly separate billing.
        let first = &parsed.scopes[0];
        assert_eq!(first.tool_use_id, "call-a");
        assert_eq!(first.tool_name, "agent");
        assert_eq!(first.billed_turns, 1);
        assert_eq!(first.is_error, Some(true));
        assert_eq!(first.error_message.as_deref(), Some("boom-a"));
        // The second scope never reported: it was still running when the
        // session ended. Absent terminal fields are the only honest account
        // of that — a scope is not required to have finished to be real.
        let second = &parsed.scopes[1];
        assert_eq!(second.tool_use_id, "call-b");
        assert_eq!(second.tool_name, "background_probe");
        assert_eq!(second.billed_turns, 2);
        assert_eq!(second.reported_num_turns, None);
        assert_eq!(second.is_error, None);
        assert_eq!(second.subtype, None);
        assert_eq!(second.error_message, None);
    }

    #[test]
    fn rejects_any_event_in_a_scope_after_its_terminal_result() {
        // A scope that has reported is finished. A second result and a
        // billed turn after the scope's result are the same defect: output
        // attributed to a scope that already declared its outcome.
        let second_result = SUBAGENT_RESULT.replace("\"uuid\":\"u4\"", "\"uuid\":\"u9\"");
        let late_turn = SUBAGENT_TURN.replace("\"uuid\":\"u3\"", "\"uuid\":\"u9\"");
        for post_terminal in [second_result, late_turn] {
            let text = format!(
                "{INIT}{MAIN_TURN}{SUBAGENT_TURN}{SUBAGENT_RESULT}{post_terminal}{MAIN_RESULT}"
            );
            let error = parse_text(&text).expect_err("a reported scope accepts no further output");
            let message = error.to_string();
            assert!(message.contains("subagent scope \"chatcmpl-tool-9d45d85b\""));
            assert!(message.contains("after its terminal result at line 4"));
        }
    }

    #[test]
    fn accepts_a_subagent_that_reports_zero_turns_over_billed_ones() {
        // Known upstream defect: a subagent terminated by a thrown error
        // reports num_turns: 0 even though it genuinely billed turns. The
        // main-scope consistency window would refuse this stream, which is
        // exactly why subagent scopes record the reported value instead of
        // validating it — rejecting here would throw away real production
        // runs to defend an invariant the upstream runner does not honor.
        let thrown = "{\"type\":\"result\",\"subtype\":\"error_during_execution\",\"uuid\":\"u4\",\"session_id\":\"a\",\"parent_tool_use_id\":\"chatcmpl-tool-9d45d85b\",\"is_error\":true,\"duration_ms\":2,\"duration_api_ms\":1,\"num_turns\":0,\"usage\":{},\"permission_denials\":[],\"error\":{\"message\":\"Error: fetch failed\"}}\n";
        let text = format!("{INIT}{MAIN_TURN}{SUBAGENT_TURN}{thrown}{MAIN_RESULT}");
        let parsed = parse_text(&text).expect("a zero-turn error report is recorded, not judged");
        assert_eq!(parsed.scopes.len(), 1);
        let scope = &parsed.scopes[0];
        assert_eq!(scope.billed_turns, 1);
        assert_eq!(scope.reported_num_turns, Some(0));
        assert_eq!(scope.is_error, Some(true));
        assert_eq!(scope.error_message.as_deref(), Some("Error: fetch failed"));
    }

    #[test]
    fn rejects_a_duplicate_tool_use_id() {
        // Correlation is by id; a re-issued id would make every later scope
        // reference ambiguous, so the second issuance is refused outright
        // instead of letting first-wins or last-wins pick a scope silently.
        let spawn_twice = "{\"type\":\"assistant\",\"uuid\":\"u2\",\"session_id\":\"a\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"call-dup\",\"name\":\"agent\",\"input\":{}},{\"type\":\"tool_use\",\"id\":\"call-dup\",\"name\":\"agent\",\"input\":{}}],\"usage\":{\"input_tokens\":7}}}\n";
        let text = format!("{INIT}{spawn_twice}{MAIN_RESULT}");
        let error =
            parse_text(&text).expect_err("a duplicated tool_use id is ambiguity, not reuse");
        assert!(error
            .to_string()
            .contains("re-issues tool_use id \"call-dup\""));
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
