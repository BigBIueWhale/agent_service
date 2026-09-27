//! Replay provider request evidence without reserializing its JSON body.
use crate::{
    generation::Generation,
    json::{Document, Limits, Value},
    schema::ValidationLimits,
    stream::{field, text, unsigned},
    usage::{GenerationUsageSummary, ServedUsage},
    ContractError, ContractResult, SAFE_INTEGER,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

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
    ids: BTreeSet<String>,
    scopes: BTreeMap<String, RequestBody>,
    responses: BTreeMap<String, ResponseState>,
    attempts: BTreeMap<String, AttemptState>,
    generation_ids: BTreeSet<String>,
    usage: BTreeMap<String, GenerationUsageSummary>,
    all_usage: GenerationUsageSummary,
}

#[derive(Default)]
struct AttemptState {
    scope: String,
    requests: Vec<String>,
    settled: BTreeMap<String, bool>,
    outcomes: BTreeMap<String, ResponseOutcome>,
    generation: Option<Arc<Generation>>,
    completed: bool,
}

pub(crate) struct GenerationAdmission {
    pub generation: Arc<Generation>,
}

pub(crate) struct CompletionAdmission {
    pub generation: Arc<Generation>,
    pub accepted: bool,
}

#[derive(Clone)]
pub(crate) struct ResponseOutcome {
    pub scope: String,
    pub usage: Option<ServedUsage>,
    pub completed: bool,
    pub pipeline_outputs_delivered: u64,
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
    attempt: Option<String>,
    usage: GenerationUsageSummary,
    all_usage: GenerationUsageSummary,
}

#[derive(Clone, Default)]
struct ResponseState {
    scope: String,
    attempt: Option<String>,
    processing: Option<bool>,
    sequence: u64,
    http_status: Option<u64>,
    termination: Option<String>,
    bytes: u64,
    digest: Option<Sha256>,
}
pub(crate) struct ResponseAdmission {
    request_id: String,
    state: Option<ResponseState>,
    attempt: Option<String>,
    settlement: Option<bool>,
    pub outcome: Option<ResponseOutcome>,
    usage: Option<(GenerationUsageSummary, GenerationUsageSummary)>,
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
                    .and_then(|first| first.checked_add(self.all_usage.requests))
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
        let owner = field(request, "owner", line)?;
        let attempt = match text(owner, "kind", line)? {
            "chat" => {
                let id = text(owner, "attempt_id", line)?;
                if self.attempts.get(id).is_some_and(|attempt| {
                    attempt.scope != scope
                        || attempt.generation.is_some()
                        || attempt.completed
                        || attempt.settled.values().any(|accepted| *accepted)
                }) {
                    return Err(refusal(
                        "chat attempt changes scope or issues a request after acceptance",
                    ));
                }
                Some(id.to_string())
            }
            "utility" => None,
            _ => return Err(refusal("unknown request owner")),
        };
        let segment_id = text(request, "segment_id", line)?;
        let body = field(request, "body", line)?;
        let json = match text(body, "kind", line)? {
            "full" => {
                if self
                    .scopes
                    .get(scope)
                    .is_some_and(|previous| previous.segment_id == segment_id)
                {
                    return Err(refusal("full body repeats an active invocation segment"));
                }
                text(body, "json", line)?.to_string()
            }
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
        let stream = field(root, "stream", line)?
            .as_bool()
            .ok_or_else(|| refusal("body has no stream mode"))?;
        let policy = field(request, "decode_policy", line)?;
        if stream != (text(policy, "mode", line)? == "stream") {
            return Err(refusal("selected decoder contradicts the request stream mode"));
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
            usage: self.scope_usage(scope).admit_request()?,
            all_usage: self.all_usage.admit_request()?,
            attempt,
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
        if let Some(id) = &admission.attempt {
            self.attempts
                .entry(id.clone())
                .or_insert_with(|| AttemptState {
                    scope: admission.scope.clone(),
                    ..AttemptState::default()
                })
                .requests
                .push(admission.body.id.clone());
        }
        self.ids.insert(admission.body.id.clone());
        self.responses.insert(
            admission.body.id.clone(),
            ResponseState {
                scope: admission.scope.clone(),
                digest: Some(Sha256::default()),
                attempt: admission.attempt,
                ..ResponseState::default()
            },
        );
        self.usage.insert(admission.scope.clone(), admission.usage);
        self.all_usage = admission.all_usage;
        self.scopes.insert(admission.scope, admission.body);
    }

    pub(crate) fn plan_response(
        &self,
        record: Value<'_>,
        line: usize,
    ) -> ContractResult<ResponseAdmission> {
        let response = field(record, "response", line)?;
        let id = text(response, "request_id", line)?;
        let mut state = self
            .responses
            .get(id)
            .cloned()
            .ok_or_else(|| refusal("response has no open request"))?;
        let sequence = unsigned(
            field(response, "sequence", line)?,
            "response sequence",
            SAFE_INTEGER,
        )?;
        if self.journal_id.as_deref() != Some(text(response, "journal_id", line)?)
            || state.sequence.checked_add(1) != Some(sequence)
        {
            return Err(refusal(
                "response is foreign, missing, repeated or reordered",
            ));
        }
        let event = field(response, "event", line)?;
        let mut ended = false;
        let mut settlement = None;
        let mut outcome = None;
        let kind = text(event, "kind", line)?;
        if (kind == "history") != state.processing.is_some()
            || (kind != "history" && (kind == "outcome") != state.digest.is_none())
        {
            return Err(refusal(
                "response processing outcome must follow transport completion",
            ));
        }
        match kind {
            "http" => {
                if state.http_status.is_some() || state.sequence != 0 {
                    return Err(refusal("response repeats HTTP headers"));
                }
                state.http_status = Some(unsigned(
                    field(event, "status", line)?,
                    "HTTP response status",
                    SAFE_INTEGER,
                )?);
            }
            "body" => {
                if state.http_status.is_none()
                    || state.bytes
                        != unsigned(
                            field(event, "offset", line)?,
                            "response offset",
                            SAFE_INTEGER,
                        )?
                {
                    return Err(refusal("response has no HTTP headers or has a byte gap"));
                }
                let encoded = text(event, "base64", line)?;
                let bytes = STANDARD
                    .decode(encoded)
                    .map_err(|_| refusal("response has invalid base64 bytes"))?;
                if bytes.is_empty() || STANDARD.encode(&bytes) != encoded {
                    return Err(refusal("response has noncanonical base64 bytes"));
                }
                state.bytes = state
                    .bytes
                    .checked_add(bytes.len() as u64)
                    .filter(|bytes| *bytes <= SAFE_INTEGER)
                    .ok_or_else(|| refusal("response exceeds exact byte accounting"))?;
                state.digest.as_mut().unwrap().update(bytes);
            }
            "end" => {
                let termination = text(event, "termination", line)?;
                if (termination == "eof" && state.http_status.is_none())
                    || (termination == "not_dispatched" && state.sequence != 0)
                    || state.bytes
                        != unsigned(
                            field(event, "body_bytes", line)?,
                            "response bytes",
                            SAFE_INTEGER,
                        )?
                    || state
                        .digest
                        .as_ref()
                        .unwrap()
                        .clone()
                        .finalize()
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect::<String>()
                        != text(event, "body_sha256", line)?
                {
                    return Err(refusal(
                        "response completion does not account for the exact observed bytes",
                    ));
                }
                state.digest = None;
                state.termination = Some(termination.to_string());
            }
            "outcome" => {
                let completed = text(event, "status", line)? == "completed";
                if completed
                    && (!state
                        .http_status
                        .is_some_and(|status| (200..300).contains(&status))
                        || !matches!(state.termination.as_deref(), Some("eof" | "cancelled")))
                {
                    return Err(refusal(
                        "completed processing has no successful HTTP transport completion",
                    ));
                }
                let usage = field(event, "served_usage", line)?;
                let sdk_values_seen = unsigned(
                    field(event, "sdk_values_seen", line)?,
                    "SDK values seen",
                    SAFE_INTEGER,
                )?;
                let pipeline_outputs_delivered = unsigned(
                    field(event, "pipeline_outputs_delivered", line)?,
                    "pipeline outputs delivered",
                    SAFE_INTEGER,
                )?;
                if (sdk_values_seen > 0 && state.http_status.is_none())
                    || (pipeline_outputs_delivered > 0 && sdk_values_seen == 0)
                {
                    return Err(refusal("decoded progress has no observed SDK response"));
                }
                outcome = Some(ResponseOutcome {
                    scope: state.scope.clone(),
                    usage: if usage.is_null() {
                        None
                    } else {
                        Some(ServedUsage::read(usage, line)?)
                    },
                    completed,
                    pipeline_outputs_delivered,
                });
                state.processing = Some(completed);
                ended = state.attempt.is_none();
            }
            "history" => {
                let attempt = state
                    .attempt
                    .as_ref()
                    .ok_or_else(|| refusal("utility response has a chat history decision"))?;
                let accepted = text(event, "disposition", line)? == "accepted";
                if accepted {
                    if state.processing != Some(true)
                        || self.attempts.get(attempt).is_none_or(|attempt| {
                            attempt.settled.values().any(|accepted| *accepted)
                        })
                    {
                        return Err(refusal(
                            "chat history acceptance is repeated or has no completed response",
                        ));
                    }
                }
                settlement = Some(accepted);
                ended = true;
            }
            _ => return Err(refusal("response uses an unknown event")),
        }
        state.sequence = sequence;
        Ok(ResponseAdmission {
            request_id: id.to_string(),
            attempt: state.attempt.clone(),
            settlement,
            usage: outcome
                .as_ref()
                .map(|outcome| -> ContractResult<_> {
                    Ok((
                        self.scope_usage(&outcome.scope).finalize(outcome.usage)?,
                        self.all_usage.finalize(outcome.usage)?,
                    ))
                })
                .transpose()?,
            outcome,
            state: if ended { None } else { Some(state) },
        })
    }

    pub(crate) fn commit_response(&mut self, admission: ResponseAdmission) {
        if let Some((usage, all_usage)) = admission.usage {
            self.usage.insert(
                admission
                    .outcome
                    .as_ref()
                    .expect("planned usage outcome")
                    .scope
                    .clone(),
                usage,
            );
            self.all_usage = all_usage;
        }
        if let Some(id) = &admission.attempt {
            let attempt = self.attempts.get_mut(id).expect("planned chat attempt");
            if let Some(outcome) = admission.outcome {
                attempt
                    .outcomes
                    .insert(admission.request_id.clone(), outcome);
            }
            if let Some(accepted) = admission.settlement {
                attempt
                    .settled
                    .insert(admission.request_id.clone(), accepted);
            }
        }
        if let Some(state) = admission.state {
            self.responses.insert(admission.request_id, state);
        } else {
            self.responses.remove(&admission.request_id);
        }
    }

    pub(crate) fn validate_output_origin(
        &self,
        origin: Value<'_>,
        line: usize,
    ) -> ContractResult<Option<String>> {
        if text(origin, "kind", line)? == "runtime" {
            return Ok(None);
        }
        let id = text(origin, "attempt_id", line)?;
        if self.attempts.get(id).is_none_or(|attempt| {
            Some(attempt.scope.as_str()) != origin.get("kv_scope").and_then(Value::as_str)
        }) {
            return Err(refusal("assistant output has no matching chat request"));
        }
        Ok(Some(id.to_string()))
    }

    pub(crate) fn scope_usage(&self, scope: &str) -> GenerationUsageSummary {
        self.usage.get(scope).copied().unwrap_or_default()
    }

    pub(crate) fn all_usage(&self) -> GenerationUsageSummary {
        self.all_usage
    }

    pub(crate) fn usage_scopes(&self) -> impl Iterator<Item = (&str, &GenerationUsageSummary)> {
        self.usage
            .iter()
            .map(|(scope, usage)| (scope.as_str(), usage))
    }

    pub(crate) fn plan_generation(
        &self,
        record: Value<'_>,
        line: usize,
        json: Limits,
        schema: ValidationLimits,
    ) -> ContractResult<GenerationAdmission> {
        let generation = Generation::read(field(record, "generation", line)?, line, json, schema)?;
        let attempt = self
            .attempts
            .get(&generation.origin.attempt)
            .ok_or_else(|| refusal("generation has no logical request owner"))?;
        if self.journal_id.as_deref() != Some(generation.journal.as_str())
            || attempt.scope != generation.origin.scope
            || attempt.completed
            || attempt.generation.is_some()
            || self.generation_ids.contains(&generation.id)
        {
            return Err(refusal(
                "generation has no unique open logical request owner",
            ));
        }
        Ok(GenerationAdmission {
            generation: Arc::new(generation),
        })
    }

    pub(crate) fn commit_generation(&mut self, admission: GenerationAdmission) {
        self.generation_ids.insert(admission.generation.id.clone());
        let attempt = admission.generation.origin.attempt.clone();
        self.attempts
            .get_mut(&attempt)
            .expect("planned generation owner")
            .generation = Some(admission.generation);
    }

    pub(crate) fn plan_completion(
        &self,
        record: Value<'_>,
        line: usize,
    ) -> ContractResult<CompletionAdmission> {
        let completion = field(record, "completion", line)?;
        let origin = crate::generation::Origin::read(field(completion, "origin", line)?, line)?;
        let attempt = self
            .attempts
            .get(&origin.attempt)
            .ok_or_else(|| refusal("completion has no logical request owner"))?;
        let generation = attempt
            .generation
            .as_ref()
            .ok_or_else(|| refusal("completion has no generation"))?;
        let requests = field(completion, "request_ids", line)?
            .elements()
            .ok_or_else(|| refusal("completion lacks request membership"))?
            .map(|id| {
                id.as_str()
                    .ok_or_else(|| refusal("completion request identity is not a string"))
            })
            .collect::<ContractResult<Vec<_>>>()?;
        if attempt.completed
            || generation.origin != origin
            || generation.journal != text(completion, "journal_id", line)?
            || generation.id != text(completion, "generation_id", line)?
            || generation.hash != text(completion, "generation_sha256", line)?
            || requests.len() != attempt.requests.len()
            || requests
                .iter()
                .zip(&attempt.requests)
                .any(|(actual, expected)| *actual != expected.as_str())
            || attempt.settled.len() != attempt.requests.len()
        {
            return Err(refusal(
                "completion omits, repeats or misattributes attempt evidence",
            ));
        }
        let accepted = text(completion, "disposition", line)? == "accepted";
        let selected: Vec<_> = attempt
            .settled
            .iter()
            .filter(|(_, accepted)| **accepted)
            .collect();
        if selected.len() != if accepted { 1 } else { 0 } {
            return Err(refusal(
                "logical disposition contradicts physical history decisions",
            ));
        }
        let consumer_observations = unsigned(
            field(completion, "consumer_observations", line)?,
            "consumer observations",
            SAFE_INTEGER,
        )?;
        let final_request = attempt
            .requests
            .last()
            .ok_or_else(|| refusal("generation has no physical request"))?;
        if attempt.requests[..attempt.requests.len() - 1]
            .iter()
            .any(|request| {
                attempt
                    .outcomes
                    .get(request)
                    .is_none_or(|outcome| outcome.pipeline_outputs_delivered != 0)
            })
            || generation
                .source_requests
                .iter()
                .any(|source| source != final_request)
            || (accepted && selected[0].0 != final_request)
        {
            return Err(refusal(
                "a Chat attempt received decoded output before its final physical request",
            ));
        }
        let final_outcome = attempt
            .outcomes
            .get(final_request)
            .ok_or_else(|| refusal("final physical request has no processing outcome"))?;
        if consumer_observations != generation.observation_count
            || consumer_observations > final_outcome.pipeline_outputs_delivered
            || (accepted
                && consumer_observations != final_outcome.pipeline_outputs_delivered)
        {
            return Err(refusal(
                "consumer receipt contradicts decoded output or generation observations",
            ));
        }
        if accepted {
            generation.require_accepted()?;
            if !final_outcome.completed || final_outcome.usage != generation.usage {
                return Err(refusal(
                    "accepted generation usage contradicts its physical response",
                ));
            }
        }
        Ok(CompletionAdmission {
            generation: Arc::clone(generation),
            accepted,
        })
    }

    pub(crate) fn commit_completion(&mut self, admission: CompletionAdmission) {
        let attempt = self
            .attempts
            .get_mut(&admission.generation.origin.attempt)
            .expect("planned completion owner");
        attempt.generation = None;
        attempt.outcomes.clear();
        attempt.completed = true;
    }

    pub(crate) fn validate_summary(&self, record: Value<'_>, line: usize) -> ContractResult<()> {
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
        if field(summary, "open_response_ids", line)?
            .elements()
            .ok_or_else(|| refusal("terminal lacks open-response accounting"))?
            .next()
            .is_some()
            || !self.responses.is_empty()
            || field(summary, "open_attempt_ids", line)?
                .elements()
                .ok_or_else(|| refusal("terminal lacks open-attempt accounting"))?
                .next()
                .is_some()
            || self.attempts.values().any(|attempt| !attempt.completed)
            || count != self.all_usage.requests
            || self
                .first_sequence
                .is_some_and(|sequence| sequence != first)
            || self
                .journal_id
                .as_deref()
                .is_some_and(|id| summary.get("journal_id").and_then(Value::as_str) != Some(id))
        {
            return Err(refusal(
                "terminal does not account for every physical request and logical attempt",
            ));
        }
        self.all_usage
            .require_summary(field(record, "usage", line)?, line)
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
        json!({"request":{"journal_id":"j","sequence":sequence,"request_id":id,"owner":{"kind":"utility"},"kv_scope":"owner","segment_id":"segment","prompt_id":"p","decode_policy":{"mode":"nonstream","model":"fixture-model","strict_tool_calling":false,"named_tool_choice":null,"exact_token_counting":false,"tagged_thinking_tags":false},"body_bytes":json.len(),"body_sha256":sha256(json),"body":body}}).to_string()
    }
    fn admit(state: &mut ModelRequests, json: &str) -> ContractResult<()> {
        let document = Document::decode(json.as_bytes(), LIMITS).unwrap();
        let admission = state.plan(document.root(), 1, LIMITS)?;
        state.commit(admission);
        Ok(())
    }
    #[test]
    fn selected_decoder_mode_must_match_the_request_body() {
        let body = r#"{"kv_scope":"owner","stream":false,"messages":[]}"#;
        let mut record: serde_json::Value = serde_json::from_str(&request(
            1,
            "request",
            json!({"kind":"full","json":body}),
            body,
        ))
        .unwrap();
        record["request"]["decode_policy"]["mode"] = json!("stream");
        let mut state = ModelRequests::default();
        assert!(matches!(
            admit(&mut state, &record.to_string()),
            Err(ContractError::InvalidRecord(message))
                if message.contains("selected decoder contradicts")
        ));
    }
    #[test]
    fn full_body_requires_a_new_segment_for_its_scope() {
        let body = r#"{"kv_scope":"owner","stream":false,"messages":[]}"#;
        let mut state = ModelRequests::default();
        admit(
            &mut state,
            &request(1, "first", json!({"kind":"full","json":body}), body),
        )
        .unwrap();
        let mut second: serde_json::Value = serde_json::from_str(&request(
            2,
            "second",
            json!({"kind":"full","json":body}),
            body,
        ))
        .unwrap();
        let error = admit(&mut state, &second.to_string()).unwrap_err();
        assert!(error
            .to_string()
            .contains("full body repeats an active invocation segment"));
        second["request"]["segment_id"] = json!("next-segment");
        admit(&mut state, &second.to_string()).unwrap();
    }
    #[test]
    fn only_the_final_physical_retry_can_deliver_chat_output() {
        let rows: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("fixtures/ordinary-tool-wire.json")).unwrap();
        let original_generation = rows
            .iter()
            .find(|row| row["type"] == "model_generation")
            .unwrap()["generation"]
            .clone();
        let original_completion = rows
            .iter()
            .find(|row| row["type"] == "model_attempt_completion")
            .unwrap()["completion"]
            .clone();
        for (name, first_source, earlier_outputs, extra_final_output, accepted) in [
            ("final response", false, 0, false, true),
            ("earlier response", true, 3, false, false),
            ("unclaimed earlier output", false, 1, false, false),
            ("unclaimed final output", false, 0, true, false),
        ] {
            let mut evidence = original_generation.clone();
            let mut envelope: serde_json::Value =
                serde_json::from_str(evidence["generation_json"].as_str().unwrap()).unwrap();
            for observation in envelope["observations"].as_array_mut().unwrap() {
                observation["source_request_id"] =
                    serde_json::json!(if first_source { "r1" } else { "r2" });
            }
            let bytes = envelope.to_string();
            let byte_len = bytes.len();
            let hash = crate::generation::sha256(bytes.as_bytes());
            evidence["generation_json"] = serde_json::json!(bytes);
            evidence["generation_bytes"] = serde_json::json!(byte_len);
            evidence["generation_sha256"] = serde_json::json!(hash);
            let generation_record = serde_json::json!({"generation": evidence.clone()}).to_string();
            let document = Document::decode(generation_record.as_bytes(), LIMITS).unwrap();
            let generation = Generation::read(
                field(document.root(), "generation", 1).unwrap(),
                1,
                LIMITS,
                ValidationLimits {
                    operations: 1_000_000,
                },
            )
            .unwrap();
            let mut completion = original_completion.clone();
            completion["request_ids"] = serde_json::json!(["r1", "r2"]);
            completion["generation_sha256"] = evidence["generation_sha256"].clone();
            let completion_record = serde_json::json!({"completion": completion}).to_string();
            let document = Document::decode(completion_record.as_bytes(), LIMITS).unwrap();
            let mut state = ModelRequests::default();
            state.journal_id = Some(generation.journal.clone());
            state.attempts.insert(
                generation.origin.attempt.clone(),
                AttemptState {
                    scope: generation.origin.scope.clone(),
                    requests: vec!["r1".into(), "r2".into()],
                    settled: BTreeMap::from([("r1".into(), false), ("r2".into(), true)]),
                    outcomes: BTreeMap::from([
                        (
                            "r1".into(),
                            ResponseOutcome {
                                scope: generation.origin.scope.clone(),
                                usage: None,
                                completed: false,
                                pipeline_outputs_delivered: earlier_outputs,
                            },
                        ),
                        (
                            "r2".into(),
                            ResponseOutcome {
                                scope: generation.origin.scope.clone(),
                                usage: generation.usage,
                                completed: true,
                                pipeline_outputs_delivered: generation.observation_count
                                    + u64::from(extra_final_output),
                            },
                        ),
                    ]),
                    generation: Some(Arc::new(generation)),
                    completed: false,
                },
            );
            let result = state.plan_completion(document.root(), 1);
            if accepted {
                assert!(result.is_ok(), "{name}");
            } else {
                let error = result.err().expect("forged retry was admitted");
                assert!(
                    error.to_string().contains("final physical request"),
                    "{name}"
                );
            }
        }
    }
    #[test]
    fn processing_completion_requires_successful_http_and_compatible_transport() {
        for (http_status, termination, can_complete) in [
            (None, "not_dispatched", false),
            (None, "failed", false),
            (None, "cancelled", false),
            (Some(200), "eof", true),
            (Some(204), "eof", true),
            (Some(299), "cancelled", true),
            (Some(300), "eof", false),
            (Some(503), "cancelled", false),
            (Some(200), "failed", false),
        ] {
            for status in ["completed", "failed", "cancelled"] {
                let mut state = ModelRequests::default();
                let body = r#"{"kv_scope":"owner","stream":false,"messages":[]}"#;
                admit(
                    &mut state,
                    &request(1, "r", json!({"kind":"full","json":body}), body),
                )
                .unwrap();
                let mut events = Vec::new();
                if let Some(http_status) = http_status {
                    events.push(json!({
                        "kind": "http", "status": http_status, "content_type": null
                    }));
                }
                events.push(json!({
                    "kind": "end", "termination": termination,
                    "body_bytes": 0, "body_sha256": sha256(""),
                    "error": if termination == "failed" { json!("read failed") } else { json!(null) }
                }));
                events.push(json!({
                    "kind": "outcome", "served_usage": null, "status": status,
                    "sdk_values_seen": 0, "pipeline_outputs_delivered": 0,
                    "error": if status == "failed" { json!("processing failed") } else { json!(null) }
                }));
                for (index, event) in events.iter().enumerate() {
                    let raw = json!({"response": {
                        "journal_id": "j", "request_id": "r",
                        "sequence": index + 1, "event": event
                    }})
                    .to_string();
                    let doc = Document::decode(raw.as_bytes(), LIMITS).unwrap();
                    let admission = state.plan_response(doc.root(), 1);
                    if event["kind"] == "outcome" && status == "completed" && !can_complete {
                        let error = admission
                            .err()
                            .expect("contradictory completion was admitted");
                        assert!(error.to_string().contains("successful HTTP transport"));
                        assert_eq!(state.responses.len(), 1);
                    } else {
                        state.commit_response(admission.unwrap());
                    }
                }
                assert_eq!(
                    state.responses.is_empty(),
                    status != "completed" || can_complete,
                    "{http_status:?}/{termination}/{status}"
                );
            }
        }
    }
    #[test]
    fn response_usage_requires_complete_consistent_counts() {
        for status in ["completed", "failed", "cancelled"] {
            for defect in [
                "valid", "null", "zero", "missing", "total", "cached", "thoughts", "negative",
            ] {
                let mut state = ModelRequests::default();
                let body = r#"{"kv_scope":"owner","stream":false,"messages":[]}"#;
                admit(
                    &mut state,
                    &request(1, "r", json!({"kind":"full","json":body}), body),
                )
                .unwrap();
                for (index, event) in [
                    json!({"kind":"http","status":200,"content_type":null}),
                    json!({"kind":"end","termination":"eof","body_bytes":0,"body_sha256":sha256(""),"error":null}),
                ].into_iter().enumerate() {
                    let raw = json!({"response":{"journal_id":"j","request_id":"r","sequence":index+1,"event":event}}).to_string();
                    let doc = Document::decode(raw.as_bytes(), LIMITS).unwrap();
                    let plan = state.plan_response(doc.root(), 1).unwrap();
                    state.commit_response(plan);
                }
                let mut event = json!({"kind":"outcome","status":status,
                    "sdk_values_seen":0,"pipeline_outputs_delivered":0,
                    "error":if status == "failed" {json!("processing failed")} else {json!(null)},
                    "served_usage":{"promptTokenCount":5,"candidatesTokenCount":3,"totalTokenCount":8,
                        "cachedContentTokenCount":1,"thoughtsTokenCount":2}});
                match defect {
                    "null" => event["served_usage"] = json!(null),
                    "zero" => {
                        for value in event["served_usage"].as_object_mut().unwrap().values_mut() {
                            *value = json!(0);
                        }
                    }
                    "missing" => {
                        event.as_object_mut().unwrap().remove("served_usage");
                    }
                    "total" => event["served_usage"]["totalTokenCount"] = json!(9),
                    "cached" => event["served_usage"]["cachedContentTokenCount"] = json!(6),
                    "thoughts" => event["served_usage"]["thoughtsTokenCount"] = json!(4),
                    "negative" => event["served_usage"]["thoughtsTokenCount"] = json!(-1),
                    _ => {}
                }
                let raw = json!({"response":{"journal_id":"j","request_id":"r","sequence":3,"event":event}}).to_string();
                let doc = Document::decode(raw.as_bytes(), LIMITS).unwrap();
                let plan = state.plan_response(doc.root(), 1);
                assert_eq!(
                    plan.is_ok(),
                    matches!(defect, "valid" | "null" | "zero"),
                    "{status}/{defect}"
                );
                assert_eq!(
                    state.responses.len(),
                    1,
                    "planning must not mutate admission state"
                );
            }
        }
    }
    #[test]
    fn exact_request_replay_and_omission_refusals() {
        let first = r#"{"kv_scope":"owner","stream":false,"messages":[{"role":"system","content":"tools \\"}],"tools":[{"messages":"nested"}]}"#;
        let second = r#"{"kv_scope":"owner","stream":false,"messages":[{"role":"system","content":"tools \\"},{"role":"user","content":"שלום\n"}],"tools":[]}"#;
        let a = request(3, "a", json!({"kind":"full","json":first}), first);
        let b = request(
            4,
            "b",
            json!({"kind":"delta","base_request_id":"a","retain_messages":1,"prefix":"{\"kv_scope\":\"owner\",\"stream\":false,\"messages\":[","suffix":"],\"tools\":[]}","added_messages":[r#"{"role":"user","content":"שלום\n"}"#]}),
            second,
        );
        let mut state = ModelRequests::default();
        assert!(admit(&mut state, &b).is_err());
        admit(&mut state, &a).unwrap();
        admit(&mut state, &b).unwrap();
        assert!(admit(&mut state, &b).is_err());
        let terminal = Document::decode(
            br#"{"request_evidence":{"journal_id":"j","first_sequence":3,"request_count":2,"open_response_ids":[],"open_attempt_ids":[]},"usage":{"requests":2,"usageReports":0,"unfinalizedRequests":0,"unreportedUsageRequests":2,"usage":null}}"#,
            LIMITS,
        )
        .unwrap();
        assert!(state.validate_summary(terminal.root(), 3).is_err());
        for id in ["a", "b"] {
            for (sequence,event) in [
                json!({"kind":"http","status":200,"content_type":"application/json"}),
                json!({"kind":"body","offset":0,"base64":"e30="}),
                json!({"kind":"end","termination":"eof","body_bytes":2,"body_sha256":sha256("{}"),"error":null}),
                json!({"kind":"outcome","served_usage":null,"status":"completed","error":null,"sdk_values_seen":0,"pipeline_outputs_delivered":0}),
            ].into_iter().enumerate() {
                if id == "b" && event["kind"] == "outcome" {
                    assert!(state.validate_summary(terminal.root(), 3).is_err());
                    let late_body = json!({"response":{"journal_id":"j","request_id":id,"sequence":sequence+1,"event":{"kind":"body","offset":2,"base64":"e30="}}}).to_string();
                    let doc = Document::decode(late_body.as_bytes(), LIMITS).unwrap();
                    assert!(state.plan_response(doc.root(), 1).is_err());
                }
                let record = json!({"response":{"journal_id":"j","request_id":id,"sequence":sequence+1,"event":event}}).to_string();
                let doc = Document::decode(record.as_bytes(),LIMITS).unwrap();
                let admission = state.plan_response(doc.root(),1).unwrap();
                state.commit_response(admission);
            }
        }
        state.validate_summary(terminal.root(), 3).unwrap();
        let wrong = terminal
            .source()
            .replace("\"requests\":2", "\"requests\":3");
        let wrong = Document::decode(wrong.as_bytes(), LIMITS).unwrap();
        assert!(state.validate_summary(wrong.root(), 3).is_err());
        let mut changed: serde_json::Value = serde_json::from_str(&a).unwrap();
        changed["request"]["body_bytes"] = json!(1);
        assert!(admit(&mut ModelRequests::default(), &changed.to_string()).is_err());
    }
    #[test]
    fn captured_physical_and_logical_closure_requires_exact_terminal_usage() {
        let rows: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("fixtures/ordinary-tool-wire.json")).unwrap();
        let mut state = ModelRequests::default();
        for row in rows {
            let raw = row.to_string();
            let document = Document::decode(raw.as_bytes(), LIMITS).unwrap();
            match row["type"].as_str().unwrap() {
                "system" => {
                    let origin = state
                        .plan_origin(
                            field(document.root(), "request_evidence_origin", 1).unwrap(),
                            1,
                        )
                        .unwrap();
                    state.commit_origin(origin);
                }
                "model_request" => {
                    let plan = state.plan(document.root(), 1, LIMITS).unwrap();
                    state.commit(plan);
                }
                "model_response" => {
                    let plan = state.plan_response(document.root(), 1).unwrap();
                    state.commit_response(plan);
                }
                "model_generation" => {
                    let plan = state
                        .plan_generation(
                            document.root(),
                            1,
                            LIMITS,
                            ValidationLimits {
                                operations: 1_000_000,
                            },
                        )
                        .unwrap();
                    state.commit_generation(plan);
                }
                "model_attempt_completion" => {
                    let before = state.all_usage();
                    let plan = state.plan_completion(document.root(), 1).unwrap();
                    state.commit_completion(plan);
                    assert_eq!(
                        state.all_usage(),
                        before,
                        "history acceptance cannot bill again"
                    );
                }
                "result" => {
                    state.validate_summary(document.root(), 1).unwrap();
                    let mut wrong = row.clone();
                    wrong["usage"]["usage"]["candidatesTokenCount"] = json!(8);
                    wrong["usage"]["usage"]["totalTokenCount"] = json!(20);
                    let raw = wrong.to_string();
                    let document = Document::decode(raw.as_bytes(), LIMITS).unwrap();
                    assert!(state.validate_summary(document.root(), 1).is_err());
                }
                "stream_event" => {}
                kind => panic!("unhandled fixture record {kind}"),
            }
        }
        assert_eq!(state.all_usage().requests, 1);
        assert_eq!(state.all_usage().usage_reports, 1);
        assert_eq!(state.all_usage().usage.unwrap().output, 7);
    }
    #[test]
    fn chat_processing_requires_one_history_decision_and_a_matching_origin() {
        for status in ["completed", "failed"] {
            let mut state = ModelRequests::default();
            let body = r#"{"kv_scope":"owner","stream":false,"messages":[]}"#;
            let mut value: serde_json::Value =
                serde_json::from_str(&request(1, "r", json!({"kind":"full","json":body}), body))
                    .unwrap();
            value["request"]["owner"] = json!({"kind":"chat","attempt_id":"attempt"});
            admit(&mut state, &value.to_string()).unwrap();
            for (index, event) in [
                json!({"kind":"http","status":200,"content_type":"application/json"}),
                json!({"kind":"body","offset":0,"base64":"e30="}),
                json!({"kind":"end","termination":"eof","body_bytes":2,"body_sha256":sha256("{}"),"error":null}),
                json!({"kind":"outcome","served_usage":null,"status":status,"error":if status == "failed" {json!("failure")} else {json!(null)},"sdk_values_seen":0,"pipeline_outputs_delivered":0}),
            ].into_iter().enumerate() {
                let raw = json!({"response":{"journal_id":"j","request_id":"r","sequence":index+1,"event":event}}).to_string();
                let doc = Document::decode(raw.as_bytes(), LIMITS).unwrap();
                let plan = state.plan_response(doc.root(), 1).unwrap();
                state.commit_response(plan);
            }
            let terminal = Document::decode(br#"{"request_evidence":{"journal_id":"j","first_sequence":1,"request_count":1,"open_response_ids":[],"open_attempt_ids":[]},"usage":{"requests":1,"usageReports":0,"unfinalizedRequests":0,"unreportedUsageRequests":1,"usage":null}}"#, LIMITS).unwrap();
            assert!(state.validate_summary(terminal.root(), 1).is_err());
            let accepted = json!({"response":{"journal_id":"j","request_id":"r","sequence":5,"event":{"kind":"history","disposition":"accepted"}}}).to_string();
            let doc = Document::decode(accepted.as_bytes(), LIMITS).unwrap();
            if status == "failed" {
                assert!(state.plan_response(doc.root(), 1).is_err());
            }
            let history = accepted.replace(
                "accepted",
                if status == "failed" {
                    "abandoned"
                } else {
                    "accepted"
                },
            );
            let doc = Document::decode(history.as_bytes(), LIMITS).unwrap();
            let plan = state.plan_response(doc.root(), 1).unwrap();
            state.commit_response(plan);
            assert!(state.plan_response(doc.root(), 1).is_err());
            assert!(
                state.validate_summary(terminal.root(), 1).is_err(),
                "physical settlement cannot replace generation completion"
            );
            for (id, scope, valid) in [
                ("attempt", "owner", true),
                ("unknown", "owner", false),
                ("attempt", "foreign", false),
            ] {
                let raw = json!({"kind":"model","attempt_id":id,"kv_scope":scope}).to_string();
                let doc = Document::decode(raw.as_bytes(), LIMITS).unwrap();
                assert_eq!(state.validate_output_origin(doc.root(), 1).is_ok(), valid);
            }
        }
    }
}
