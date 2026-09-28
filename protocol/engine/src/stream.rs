//! Exact decoding and record-local semantic admission. Incoming bytes must be
//! decoded by Document before any lossy host JSON conversion.
use crate::{
    json::{Document, Value},
    schema::{self, SchemaEntry, ValidationLimits},
    ContractError, ContractResult,
};
include!(concat!(env!("OUT_DIR"), "/stream_contract.rs"));

/// How one terminal state is reported, as the stream contract's terminal
/// table states it: the result subtype a record carries, whether that subtype
/// is an error, and the exit code a process that ended with it leaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalOutcome {
    pub subtype: &'static str,
    pub is_error: bool,
    pub exit_code: u8,
}

/// The exit code a process whose terminal record carries `subtype` leaves, as
/// the contract's terminal table states it; `None` for a subtype the contract
/// does not define.
pub fn terminal_exit_code(subtype: &str) -> Option<u8> {
    TERMINAL_OUTCOMES
        .iter()
        .find(|outcome| outcome.subtype == subtype)
        .map(|outcome| outcome.exit_code)
}

pub const SAFE_INTEGER: u64 = 9_007_199_254_740_991;

pub(crate) fn field<'a>(value: Value<'a>, key: &str, line: usize) -> ContractResult<Value<'a>> {
    value.get(key).ok_or_else(|| {
        ContractError::InvalidRecord(format!("events.jsonl line {line} lacks {key}"))
    })
}
pub(crate) fn text<'a>(value: Value<'a>, key: &str, line: usize) -> ContractResult<&'a str> {
    field(value, key, line)?
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            ContractError::InvalidRecord(format!(
                "events.jsonl line {line} lacks non-empty string {key}"
            ))
        })
}
pub(crate) fn unsigned(value: Value<'_>, name: &str, maximum: u64) -> ContractResult<u64> {
    value
        .as_number()
        .ok_or_else(|| ContractError::InvalidRecord(format!("{name} must be a number")))?
        .as_unsigned(maximum)
        .map_err(|cause| {
            ContractError::InvalidRecord(format!(
                "{name} is not an admitted non-negative integer: {cause:?}"
            ))
        })
}
pub(crate) fn validate(
    value: Value<'_>,
    entry: SchemaEntry,
    limits: ValidationLimits,
    line: usize,
) -> ContractResult<()> {
    schema::validate_owned(entry, value, limits).map_err(|error| match error {
        schema::ValidationError::Rejected { mismatch } => ContractError::InvalidRecord(format!("events.jsonl line {line} violates stream contract {STREAM_CONTRACT_SHA256} at {} (schema rule {}); inspect this record against the named contract and use a matching producer and reader for a new capture", mismatch.instance_path, mismatch.schema_path)),
        schema::ValidationError::InvalidDefinition { cause } => ContractError::InvalidDefinition(cause),
        schema::ValidationError::ResourceLimit { resource, limit } => ContractError::ValidationUnavailable(format!("{resource} limit {limit}")),
    })
}

/// A capability for a complete admitted record. Neither this nor PartialRecord
/// can be manufactured from an unchecked Value by a caller.
#[derive(Clone, Copy, Debug)]
pub struct DecodedRecord<'a> {
    value: Value<'a>,
    kind: EventKind,
}
#[derive(Clone, Copy, Debug)]
pub struct PartialRecord<'a> {
    value: Value<'a>,
    kind: PartialKind,
}
impl<'a> PartialRecord<'a> {
    pub(crate) fn value(self) -> Value<'a> {
        self.value
    }
    pub(crate) fn kind(self) -> PartialKind {
        self.kind
    }
}
impl<'a> DecodedRecord<'a> {
    pub fn decode(value: Value<'a>, limits: ValidationLimits, line: usize) -> ContractResult<Self> {
        let kind = decode_event_kind(value, line)?;
        // The same referenced definition provides a field-level diagnostic
        // before the enclosing oneOf reports that no whole variant matched.
        if kind == EventKind::Result {
            validate(value, SchemaEntry::TerminalResult, limits, line)?;
        }
        validate(value, SchemaEntry::Record, limits, line)?;
        if kind == EventKind::StreamEvent
            && text(field(value, "event", line)?, "type", line)? == "goal_state"
        {
            validate_goal_evidence(
                field(field(value, "event", line)?, "goal_state", line)?,
                limits,
                line,
            )?;
        }
        Ok(Self { value, kind })
    }
    pub fn kind(self) -> EventKind {
        self.kind
    }
    pub fn value(self) -> Value<'a> {
        self.value
    }
    pub fn partial(self) -> Option<PartialRecord<'a>> {
        if self.kind != EventKind::StreamEvent {
            return None;
        }
        let value = self.value.get("event")?;
        Some(PartialRecord {
            value,
            kind: PartialKind::from_wire(value.get("type")?.as_str()?)?,
        })
    }
}
pub fn decode_event(
    value: Value<'_>,
    limits: ValidationLimits,
    line: usize,
) -> ContractResult<EventKind> {
    DecodedRecord::decode(value, limits, line).map(DecodedRecord::kind)
}
/// Classification establishes no payload validity and is used only with the
/// independent evidence predicates at the same runtime owner.
pub fn decode_event_kind(value: Value<'_>, line: usize) -> ContractResult<EventKind> {
    let name = text(value, "type", line)?;
    EventKind::from_wire(name).ok_or_else(|| {
        ContractError::InvalidRecord(format!(
            "events.jsonl line {line} has unsupported event type {name:?}"
        ))
    })
}

pub fn validate_goal_evidence(
    snapshot: Value<'_>,
    limits: ValidationLimits,
    line: usize,
) -> ContractResult<()> {
    validate(snapshot, SchemaEntry::GoalSnapshot, limits, line)?;
    let goal = field(snapshot, "goal", line)?;
    let Some(checkpoint) = goal.get("evidenceCheckpoint") else {
        return Ok(());
    };
    let refuse = |cause: &str| {
        ContractError::InvalidRecord(format!(
            "events.jsonl line {line} has invalid goal checkpoint evidence: {cause}"
        ))
    };
    if field(field(goal, "evidenceCursor", line)?, "recordId", line)?
        != field(checkpoint, "checkpointId", line)?
    {
        return Err(refuse("cursor does not name the checkpoint"));
    }
    let checkpoint_id = text(checkpoint, "checkpointId", line)?;
    let claims = field(checkpoint, "claims", line)?
        .elements()
        .ok_or_else(|| refuse("claims must be an array"))?;
    let mut bytes = 0usize;
    for (index, claim) in claims.enumerate() {
        let position = index
            .checked_add(1)
            .ok_or_else(|| refuse("claim position overflow"))?;
        if text(claim, "id", line)? != format!("{checkpoint_id}:{position}") {
            return Err(refuse(
                "claim identity does not match its checkpoint and position",
            ));
        }
        bytes = bytes
            .checked_add(text(claim, "claim", line)?.len())
            .ok_or_else(|| refuse("claim byte count overflow"))?;
    }
    // This is a newly derived count, not reserialization of incoming JSON.
    let count = bytes.to_string();
    let document = Document::decode(
        count.as_bytes(),
        crate::json::Limits {
            bytes: count.len(),
            nodes: 1,
            depth: 0,
        },
    )
    .map_err(|cause| {
        ContractError::InvalidDefinition(format!("derived evidence count: {cause:?}"))
    })?;
    validate(
        document.root(),
        SchemaEntry::GoalCheckpointEvidenceBytes,
        limits,
        line,
    )
    .map_err(|error| match error {
        ContractError::InvalidRecord(_) => {
            refuse("claim UTF-8 bytes exceed the shared evidence bound")
        }
        other => other,
    })
}

pub use crate::partial_stream::PartialStreamState;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    fn document(value: &Value) -> Document {
        let bytes = serde_json::to_vec(value).unwrap();
        Document::decode(
            &bytes,
            crate::json::Limits {
                bytes: bytes.len(),
                nodes: bytes.len(),
                depth: bytes.len(),
            },
        )
        .unwrap()
    }
    fn fixture_decode(value: &Value) -> ContractResult<EventKind> {
        let document = document(value);
        decode_event(
            document.root(),
            ValidationLimits {
                operations: usize::MAX,
            },
            1,
        )
    }

    #[test]
    fn shared_goal_wire_vectors_agree_with_the_contract() {
        let vectors: Value =
            serde_json::from_str(include_str!("../../test-vectors/goal-state-v1.json")).unwrap();
        assert_eq!(vectors["v"], 1);
        for case in vectors["cases"].as_array().unwrap() {
            let result = case["record"]
                .as_object()
                .map(|record| fixture_decode(&Value::Object(record.clone())));
            let accepted = matches!(result, Some(Ok(_)));
            assert_eq!(
                accepted,
                case["expected"]["record"] == "accepted",
                "case {} disagrees: {result:?}",
                case["name"]
            );
        }
    }

    fn partial(state: &mut PartialStreamState, event: Value, root: bool) -> ContractResult<()> {
        let mut record = serde_json::json!({"type":"stream_event", "uuid":"fixture",
            "session_id":"session", "parent_tool_use_id": if root { Value::Null } else { Value::String("tool".into()) },
            "event":event});
        if !matches!(
            record["event"]["type"].as_str(),
            Some("tool_progress" | "active_goal" | "goal_state")
        ) {
            record["origin"] =
                serde_json::json!({"kind":"runtime","operation_id":"local-operation"});
        }
        let document = document(&record);
        let decoded = DecodedRecord::decode(
            document.root(),
            ValidationLimits {
                operations: usize::MAX,
            },
            1,
        )?;
        if let Some(origin) = document.root().get("origin") {
            state.observe_origin(origin, 1)?;
        }
        state.observe(decoded.partial().unwrap(), root, 1)
    }

    #[test]
    fn partials_follow_explicit_root_and_child_group_boundaries() {
        for root in [true, false] {
            let mut state = PartialStreamState::default();
            for id in ["first", "second"] {
                partial(
                    &mut state,
                    serde_json::json!({"type":"message_start", "message":{
                    "id":id, "role":"assistant", "content":[]}}),
                    root,
                )
                .unwrap();
                partial(
                    &mut state,
                    serde_json::json!({"type":"content_block_start", "index":0,
                    "content_block":{"type":"text", "text":""}}),
                    root,
                )
                .unwrap();
                partial(
                    &mut state,
                    serde_json::json!({"type":"content_block_delta", "index":0,
                    "delta":{"type":"text_delta", "text":"observed content"}}),
                    root,
                )
                .unwrap();
                assert!(state
                    .complete_message("observed content", 2)
                    .unwrap_err()
                    .to_string()
                    .contains("unclosed"));
                partial(
                    &mut state,
                    serde_json::json!({"type":"content_block_stop", "index":0}),
                    root,
                )
                .unwrap();
                state.complete_message("observed content", 3).unwrap();
                assert!(state.finish(4).is_err());
                partial(&mut state, serde_json::json!({"type":"message_stop"}), root).unwrap();
                state.finish(5).unwrap();
            }
        }
    }

    #[test]
    fn partials_refuse_unissued_closed_or_contradictory_deltas() {
        let mut state = PartialStreamState::default();
        let delta = serde_json::json!({"type":"content_block_delta", "index":0,
            "delta":{"type":"text_delta", "text":"value"}});
        assert!(partial(&mut state, delta.clone(), false)
            .unwrap_err()
            .to_string()
            .contains("absent or closed"));
        partial(
            &mut state,
            serde_json::json!({"type":"message_start", "message":{
            "id":"group", "role":"assistant", "content":[]}}),
            false,
        )
        .unwrap();
        partial(
            &mut state,
            serde_json::json!({"type":"content_block_start", "index":0,
            "content_block":{"type":"text", "text":""}}),
            false,
        )
        .unwrap();
        assert!(partial(
            &mut state,
            serde_json::json!({"type":"content_block_delta", "index":0,
            "delta":{"type":"thinking_delta", "thinking":"value"}}),
            false
        )
        .unwrap_err()
        .to_string()
        .contains("contradicts"));
        partial(
            &mut state,
            serde_json::json!({"type":"content_block_stop", "index":0}),
            false,
        )
        .unwrap();
        assert!(partial(&mut state, delta, false)
            .unwrap_err()
            .to_string()
            .contains("closed"));
    }

    #[test]
    fn mathematical_integer_spellings_survive_partial_admission_and_observation() {
        for token in ["0", "0.0", "0e0", "-0.0"] {
            let mut owner = PartialStreamState::default();
            partial(
                &mut owner,
                serde_json::json!({"type":"message_start", "message":{
                "id":"numeric-group", "role":"assistant", "content":[]}}),
                false,
            )
            .unwrap();
            for (kind, spelling) in [
                ("content_block_start", token),
                ("content_block_stop", "0.0"),
            ] {
                let block = if kind == "content_block_start" {
                    r#", "content_block":{"type":"text","text":""}"#
                } else {
                    ""
                };
                let bytes = format!(
                    r#"{{"type":"stream_event","origin":{{"kind":"runtime","operation_id":"local-operation"}},"uuid":"fixture","session_id":"session","parent_tool_use_id":"tool","event":{{"type":"{kind}","index":{spelling}{block}}}}}"#
                );
                let document = Document::decode(
                    bytes.as_bytes(),
                    crate::json::Limits {
                        bytes: bytes.len(),
                        nodes: bytes.len(),
                        depth: bytes.len(),
                    },
                )
                .unwrap();
                let record = DecodedRecord::decode(
                    document.root(),
                    ValidationLimits {
                        operations: usize::MAX,
                    },
                    1,
                )
                .unwrap();
                assert_eq!(
                    record
                        .value()
                        .get("event")
                        .unwrap()
                        .get("index")
                        .unwrap()
                        .as_number()
                        .unwrap()
                        .token(),
                    spelling,
                );
                owner.observe(record.partial().unwrap(), false, 1).unwrap();
            }
            owner.complete_message("", 3).unwrap();
            partial(
                &mut owner,
                serde_json::json!({"type":"message_stop"}),
                false,
            )
            .unwrap();
            owner.finish(4).unwrap();
        }
    }

    #[test]
    fn shared_partial_stream_temporal_vectors() {
        let vectors: Value =
            serde_json::from_str(include_str!("../../test-vectors/partial-stream-v2.json"))
                .unwrap();
        for case in vectors["cases"].as_array().unwrap() {
            let mut owner = PartialStreamState::default();
            let origin =
                document(&serde_json::json!({"kind":"runtime","operation_id":"local-operation"}));
            owner.observe_origin(origin.root(), 1).unwrap();
            let outcome = case["actions"]
                .as_array()
                .unwrap()
                .iter()
                .try_for_each(|action| match action["kind"].as_str().unwrap() {
                    "partial" => partial(
                        &mut owner,
                        action["event"].clone(),
                        case["root"].as_bool().unwrap(),
                    ),
                    "message_complete" => {
                        owner.complete_message(action["text"].as_str().unwrap_or(""), 1)
                    }
                    "scope_complete" => owner.finish(1),
                    other => panic!("unknown fixture action {other}"),
                });
            if case["expected"]["state"] == "accepted" {
                assert!(outcome.is_ok(), "case {}: {outcome:?}", case["name"]);
            } else {
                let cause = outcome.unwrap_err().to_string();
                assert!(
                    cause.contains(case["expected"]["cause"].as_str().unwrap()),
                    "case {} refused for the wrong cause: {cause}",
                    case["name"]
                );
            }
        }
    }
}
