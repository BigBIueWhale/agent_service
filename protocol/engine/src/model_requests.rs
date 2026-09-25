//! Replay provider request evidence without reserializing its JSON body.
use crate::{
    json::{Document, Limits, Value},
    stream::{field, text, unsigned},
    ContractError, ContractResult, SAFE_INTEGER,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

fn sha256(json: &str) -> String {
    Sha256::digest(json.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn refusal(detail: &str) -> ContractError {
    ContractError::InvalidRecord(format!(
        "model request evidence {detail}; inspect the complete original recording with its matching client"
    ))
}

#[derive(Default)]
pub(crate) struct ModelRequests {
    journal_id: Option<String>,
    first_sequence: Option<u64>,
    count: u64,
    ids: BTreeSet<String>,
    scopes: BTreeMap<String, RequestBody>,
}

pub(crate) struct RequestBody {
    id: String,
    segment_id: String,
    messages: Vec<String>,
}

pub(crate) struct RequestOrigin {
    journal_id: String,
    first_sequence: u64,
}

pub(crate) struct RequestAdmission {
    journal_id: String,
    sequence: u64,
    scope: String,
    body: RequestBody,
}

impl ModelRequests {
    pub(crate) fn plan_origin(
        &self,
        origin: Value<'_>,
        line: usize,
    ) -> ContractResult<RequestOrigin> {
        if self.journal_id.is_some() {
            return Err(refusal("repeats its journal origin"));
        }
        Ok(RequestOrigin {
            journal_id: text(origin, "journal_id", line)?.to_string(),
            first_sequence: unsigned(
                field(origin, "first_sequence", line)?,
                "first request sequence",
                SAFE_INTEGER,
            )?,
        })
    }

    pub(crate) fn commit_origin(&mut self, origin: RequestOrigin) {
        self.journal_id = Some(origin.journal_id);
        self.first_sequence = Some(origin.first_sequence);
    }

    pub(crate) fn plan(
        &self,
        record: Value<'_>,
        line: usize,
        limits: Limits,
    ) -> ContractResult<RequestAdmission> {
        let request = field(record, "request", line)?;
        let journal_id = text(request, "journal_id", line)?;
        let sequence = unsigned(
            field(request, "sequence", line)?,
            "request sequence",
            SAFE_INTEGER,
        )?;
        if let Some(id) = &self.journal_id {
            if id != journal_id
                || self
                    .first_sequence
                    .and_then(|first| first.checked_add(self.count))
                    != Some(sequence)
            {
                return Err(refusal(
                    "has a missing, repeated, reordered or foreign request",
                ));
            }
        }
        let id = text(request, "request_id", line)?;
        if self.ids.contains(id) {
            return Err(refusal("reuses a request identity"));
        }
        let scope = text(request, "kv_scope", line)?;
        let segment_id = text(request, "segment_id", line)?;
        let body = field(request, "body", line)?;
        let json = match text(body, "kind", line)? {
            "full" => text(body, "json", line)?.to_string(),
            "delta" => {
                let previous = self
                    .scopes
                    .get(scope)
                    .ok_or_else(|| refusal("starts an invocation without a full body"))?;
                if previous.id != text(body, "base_request_id", line)?
                    || previous.segment_id != segment_id
                {
                    return Err(refusal(
                        "does not reference the last request of its invocation",
                    ));
                }
                let retained = unsigned(
                    field(body, "retain_messages", line)?,
                    "retained request messages",
                    SAFE_INTEGER,
                )?;
                let retained =
                    usize::try_from(retained).map_err(|_| refusal("retains too many messages"))?;
                if retained > previous.messages.len() {
                    return Err(refusal("retains absent messages"));
                }
                let mut messages = previous.messages[..retained].to_vec();
                for message in field(body, "added_messages", line)?
                    .elements()
                    .ok_or_else(|| refusal("has no added messages array"))?
                {
                    messages.push(
                        message
                            .as_str()
                            .ok_or_else(|| refusal("has a non-string message delta"))?
                            .to_string(),
                    );
                }
                format!(
                    "{}{}{}",
                    text(body, "prefix", line)?,
                    messages.join(","),
                    text(body, "suffix", line)?
                )
            }
            _ => return Err(refusal("uses an unknown body representation")),
        };
        let body_bytes = unsigned(
            field(request, "body_bytes", line)?,
            "request body bytes",
            SAFE_INTEGER,
        )?;
        if u64::try_from(json.len()).ok() != Some(body_bytes)
            || sha256(&json) != text(request, "body_sha256", line)?
        {
            return Err(refusal(
                "does not reproduce the stated body bytes and SHA-256",
            ));
        }
        let document = Document::decode(json.as_bytes(), limits)
            .map_err(|error| refusal(&format!("contains invalid body JSON: {error:?}")))?;
        let root = document.root();
        if root.get("kv_scope").and_then(Value::as_str) != Some(scope) {
            return Err(refusal("body belongs to another invocation"));
        }
        let array = field(root, "messages", line)?;
        if text(body, "kind", line)? == "delta" {
            let range = array.byte_range();
            if text(body, "prefix", line)? != &json[..range.start + 1]
                || text(body, "suffix", line)? != &json[range.end - 1..]
            {
                return Err(refusal(
                    "delta does not replace the top-level messages suffix",
                ));
            }
            for message in field(body, "added_messages", line)?
                .elements()
                .ok_or_else(|| refusal("has no added messages array"))?
            {
                let raw = message
                    .as_str()
                    .ok_or_else(|| refusal("has a non-string message delta"))?;
                let doc = Document::decode(raw.as_bytes(), limits)
                    .map_err(|_| refusal("has a malformed message delta"))?;
                if doc.root().as_object().is_none() {
                    return Err(refusal("has a non-object message delta"));
                }
            }
        }
        let mut messages = Vec::new();
        for message in array
            .elements()
            .ok_or_else(|| refusal("body lacks a messages array"))?
        {
            if message.as_object().is_none() {
                return Err(refusal("body contains a non-object message"));
            }
            messages.push(message.raw().to_string());
        }
        Ok(RequestAdmission {
            journal_id: journal_id.to_string(),
            sequence,
            scope: scope.to_string(),
            body: RequestBody {
                id: id.to_string(),
                segment_id: segment_id.to_string(),
                messages,
            },
        })
    }

    pub(crate) fn commit(&mut self, admission: RequestAdmission) {
        self.journal_id.get_or_insert(admission.journal_id);
        self.first_sequence.get_or_insert(admission.sequence);
        self.count += 1;
        self.ids.insert(admission.body.id.clone());
        self.scopes.insert(admission.scope, admission.body);
    }

    pub(crate) fn validate_summary(
        &self,
        record: Value<'_>,
        line: usize,
        billed_turns: u64,
    ) -> ContractResult<()> {
        let summary = field(record, "request_evidence", line)?;
        let count = unsigned(
            field(summary, "request_count", line)?,
            "request evidence count",
            SAFE_INTEGER,
        )?;
        let first = unsigned(
            field(summary, "first_sequence", line)?,
            "first request sequence",
            SAFE_INTEGER,
        )?;
        if count != self.count
            || self.count < billed_turns
            || self
                .first_sequence
                .is_some_and(|sequence| sequence != first)
            || self
                .journal_id
                .as_deref()
                .is_some_and(|id| summary.get("journal_id").and_then(Value::as_str) != Some(id))
        {
            return Err(refusal(
                "terminal does not account for every request and billed turn",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    const LIMITS: Limits = Limits {
        bytes: 1_000_000,
        nodes: 100_000,
        depth: 100,
    };
    fn request(sequence: u64, id: &str, body: serde_json::Value, json: &str) -> String {
        json!({"request":{"journal_id":"j","sequence":sequence,"request_id":id,"kv_scope":"owner","segment_id":"segment","prompt_id":"p","body_bytes":json.len(),"body_sha256":sha256(json),"body":body}}).to_string()
    }
    fn admit(state: &mut ModelRequests, json: &str) -> ContractResult<()> {
        let document = Document::decode(json.as_bytes(), LIMITS).unwrap();
        let admission = state.plan(document.root(), 1, LIMITS)?;
        state.commit(admission);
        Ok(())
    }
    #[test]
    fn exact_request_replay_and_omission_refusals() {
        let first = r#"{"kv_scope":"owner","messages":[{"role":"system","content":"tools \\"}],"tools":[{"messages":"nested"}]}"#;
        let second = r#"{"kv_scope":"owner","messages":[{"role":"system","content":"tools \\"},{"role":"user","content":"שלום\n"}],"tools":[]}"#;
        let a = request(3, "a", json!({"kind":"full","json":first}), first);
        let b = request(
            4,
            "b",
            json!({"kind":"delta","base_request_id":"a","retain_messages":1,"prefix":"{\"kv_scope\":\"owner\",\"messages\":[","suffix":"],\"tools\":[]}","added_messages":[r#"{"role":"user","content":"שלום\n"}"#]}),
            second,
        );
        let mut state = ModelRequests::default();
        assert!(admit(&mut state, &b).is_err());
        admit(&mut state, &a).unwrap();
        admit(&mut state, &b).unwrap();
        assert!(admit(&mut state, &b).is_err());
        let terminal = Document::decode(
            br#"{"request_evidence":{"journal_id":"j","first_sequence":3,"request_count":2}}"#,
            LIMITS,
        )
        .unwrap();
        state.validate_summary(terminal.root(), 3, 2).unwrap();
        assert!(state.validate_summary(terminal.root(), 3, 3).is_err());
        let mut changed: serde_json::Value = serde_json::from_str(&a).unwrap();
        changed["request"]["body_bytes"] = json!(1);
        assert!(admit(&mut ModelRequests::default(), &changed.to_string()).is_err());
    }
}
