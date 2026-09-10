//! Exact decoding and record-local semantic admission. Incoming bytes must be
//! decoded by Document before any lossy host JSON conversion.
use crate::{
    json::{Document, Value},
    schema::{self, SchemaEntry, ValidationLimits},
    ContractError, ContractResult,
};
use std::collections::BTreeMap;
include!(concat!(env!("OUT_DIR"), "/stream_contract.rs"));

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
        schema::ValidationError::Rejected { mismatch } => ContractError::InvalidRecord(format!("events.jsonl line {line} violates stream contract {STREAM_CONTRACT_SHA256} at {} (schema rule {})", mismatch.instance_path, mismatch.schema_path)),
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

/// The wire groups root partials by turn, but complete assistant messages by
/// content category. A child emits block groups without root turn markers.
/// Complete messages delimit block indices in both scopes.
#[derive(Clone, Default)]
pub struct PartialStreamState {
    root_turn_open: bool,
    blocks: BTreeMap<u64, (String, bool)>,
}

impl PartialStreamState {
    pub fn observe(
        &mut self,
        partial: PartialRecord<'_>,
        root: bool,
        line: usize,
    ) -> ContractResult<()> {
        let PartialRecord { value: event, kind } = partial;
        let refuse = |cause: &str| {
            ContractError::InvalidRecord(format!(
                "events.jsonl line {line} violates partial-stream ordering: {cause}"
            ))
        };
        match kind {
            PartialKind::MessageStart => {
                if !root || self.root_turn_open || !self.blocks.is_empty() {
                    return Err(refuse("message_start requires a new root turn"));
                }
                self.root_turn_open = true;
            }
            PartialKind::ContentBlockStart => {
                if root && !self.root_turn_open {
                    return Err(refuse("root content block precedes message_start"));
                }
                let index = unsigned(field(event, "index", line)?, "partial index", SAFE_INTEGER)?;
                if index != self.blocks.len() as u64 {
                    return Err(refuse(
                        "content block index does not follow this message's prefix",
                    ));
                }
                self.blocks.insert(
                    index,
                    (
                        text(field(event, "content_block", line)?, "type", line)?.to_string(),
                        true,
                    ),
                );
            }
            PartialKind::ContentBlockDelta | PartialKind::ContentBlockStop => {
                let index = unsigned(field(event, "index", line)?, "partial index", SAFE_INTEGER)?;
                let Some((block_type, open)) = self.blocks.get_mut(&index) else {
                    return Err(refuse("content block was not started"));
                };
                if !*open {
                    return Err(refuse("content block is already closed"));
                }
                if kind == PartialKind::ContentBlockStop {
                    *open = false;
                } else {
                    let expected = match block_type.as_str() {
                        "text" => "text_delta",
                        "thinking" => "thinking_delta",
                        "tool_use" => "input_json_delta",
                        _ => return Err(refuse("this content block has no delta representation")),
                    };
                    if text(field(event, "delta", line)?, "type", line)? != expected {
                        return Err(refuse("delta type contradicts its content block"));
                    }
                }
            }
            PartialKind::MessageStop => {
                if !root || !self.root_turn_open || !self.blocks.is_empty() {
                    return Err(refuse(
                        "message_stop requires a root turn with all complete messages delivered",
                    ));
                }
                self.root_turn_open = false;
            }
            // State projections and tool liveness are complete notices, not
            // model deltas. Their payload and scope have already been decoded;
            // the enclosing immutable record retains them without billing.
            PartialKind::GoalState | PartialKind::ActiveGoal | PartialKind::ToolProgress => {}
        }
        Ok(())
    }

    pub fn complete_message(&mut self, line: usize) -> ContractResult<()> {
        if self.blocks.values().any(|(_, open)| *open) {
            return Err(ContractError::InvalidRecord(format!(
                "events.jsonl line {line} delivers an assistant message with an unclosed partial block"
            )));
        }
        self.blocks.clear();
        Ok(())
    }

    pub fn finish(&self, line: usize) -> ContractResult<()> {
        if self.root_turn_open || !self.blocks.is_empty() {
            return Err(ContractError::InvalidRecord(format!(
                "events.jsonl line {line} terminates a scope with unfinished partial output"
            )));
        }
        Ok(())
    }
}

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
        let record = serde_json::json!({"type":"stream_event", "uuid":"fixture",
            "session_id":"session", "parent_tool_use_id": if root { Value::Null } else { Value::String("tool".into()) },
            "event":event});
        let document = document(&record);
        let decoded = DecodedRecord::decode(
            document.root(),
            ValidationLimits {
                operations: usize::MAX,
            },
            1,
        )?;
        state.observe(decoded.partial().unwrap(), root, 1)
    }

    #[test]
    fn partials_follow_actual_root_and_child_message_boundaries() {
        let mut root = PartialStreamState::default();
        partial(
            &mut root,
            serde_json::json!({"type":"message_start", "message":{
            "id":"first", "role":"assistant", "model":"model", "content":[]}}),
            true,
        )
        .unwrap();
        for (kind, delta) in [("thinking", "thinking_delta"), ("text", "text_delta")] {
            let field = if kind == "thinking" {
                "thinking"
            } else {
                "text"
            };
            partial(
                &mut root,
                serde_json::json!({"type":"content_block_start", "index":0,
                "content_block":{"type":kind, field:""}}),
                true,
            )
            .unwrap();
            partial(
                &mut root,
                serde_json::json!({"type":"content_block_delta", "index":0,
                "delta":{"type":delta,field:"observed content"}}),
                true,
            )
            .unwrap();
            assert!(root
                .complete_message(2)
                .unwrap_err()
                .to_string()
                .contains("unclosed"));
            partial(
                &mut root,
                serde_json::json!({"type":"content_block_stop", "index":0}),
                true,
            )
            .unwrap();
            root.complete_message(3).unwrap();
        }
        assert!(root.finish(4).is_err());
        partial(&mut root, serde_json::json!({"type":"message_stop"}), true).unwrap();
        root.finish(5).unwrap();
        let mut child = PartialStreamState::default();
        partial(
            &mut child,
            serde_json::json!({"type":"content_block_start", "index":0,
            "content_block":{"type":"text", "text":""}}),
            false,
        )
        .unwrap();
        partial(
            &mut child,
            serde_json::json!({"type":"content_block_stop", "index":0}),
            false,
        )
        .unwrap();
        child.complete_message(6).unwrap();
        child.finish(7).unwrap();
    }

    #[test]
    fn partials_refuse_unissued_closed_or_contradictory_deltas() {
        let mut state = PartialStreamState::default();
        let delta = serde_json::json!({"type":"content_block_delta", "index":0,
            "delta":{"type":"text_delta", "text":"value"}});
        assert!(partial(&mut state, delta.clone(), false)
            .unwrap_err()
            .to_string()
            .contains("not started"));
        partial(
            &mut state,
            serde_json::json!({"type":"content_block_start", "index":0,
            "content_block":{"type":"thinking", "thinking":""}}),
            false,
        )
        .unwrap();
        assert!(partial(&mut state, delta.clone(), false)
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
            for (kind, spelling) in [("content_block_start", token), ("content_block_stop", "0.0")] {
                let block = if kind == "content_block_start" {
                    r#", "content_block":{"type":"text","text":""}"#
                } else {
                    ""
                };
                let bytes = format!(r#"{{"type":"stream_event","uuid":"fixture","session_id":"session","parent_tool_use_id":"tool","event":{{"type":"{kind}","index":{spelling}{block}}}}}"#);
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
                    record.value().get("event").unwrap().get("index").unwrap()
                        .as_number().unwrap().token(),
                    spelling,
                );
                owner.observe(record.partial().unwrap(), false, 1).unwrap();
            }
            owner.complete_message(3).unwrap();
            owner.finish(4).unwrap();
        }
    }

    #[test]
    fn shared_partial_stream_temporal_vectors() {
        let vectors: Value =
            serde_json::from_str(include_str!("../../test-vectors/partial-stream-v1.json"))
                .unwrap();
        for case in vectors["cases"].as_array().unwrap() {
            let mut owner = PartialStreamState::default();
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
                    "message_complete" => owner.complete_message(1),
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
