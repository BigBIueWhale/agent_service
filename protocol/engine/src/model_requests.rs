//! Replay provider request evidence without reserializing its JSON body.
use crate::{
    generation::{Generation, Origin},
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

/// Both sides passed through the pinned SDK's JSON.parse boundary. Compare
/// numbers as JavaScript Numbers, including equivalent decimal spellings.
fn same_sdk_value(left: &serde_json::Value, right: &serde_json::Value) -> bool {
    use serde_json::Value as Json;
    let mut pending = vec![(left, right)];
    while let Some((left, right)) = pending.pop() {
        match (left, right) {
            (Json::Null, Json::Null) => {}
            (Json::Bool(left), Json::Bool(right)) if left == right => {}
            (Json::Number(left), Json::Number(right))
                if left.as_f64() == right.as_f64() => {}
            (Json::String(left), Json::String(right)) if left == right => {}
            (Json::Array(left), Json::Array(right)) if left.len() == right.len() => {
                pending.extend(left.iter().zip(right));
            }
            (Json::Object(left), Json::Object(right)) if left.len() == right.len() => {
                for (key, value) in left {
                    let Some(other) = right.get(key) else { return false; };
                    pending.push((value, other));
                }
            }
            _ => return false,
        }
    }
    true
}

fn compaction_request_budget(body: Value<'_>, line: usize) -> ContractResult<u64> {
    let budget = unsigned(field(body, "max_tokens", line)?, "physical compaction ceiling", SAFE_INTEGER)?;
    if budget == 0 {
        return Err(refusal("compaction request has no positive physical output ceiling"));
    }
    let last = field(body, "messages", line)?
        .elements()
        .and_then(|items| items.last())
        .ok_or_else(|| refusal("compaction request has no final directive"))?;
    if text(last, "role", line)? != "user" {
        return Err(refusal("compaction request does not end in a user directive"));
    }
    let content = field(last, "content", line)?;
    let directive = if let Some(text) = content.as_str() {
        text.to_string()
    } else {
        let parts = content
            .elements()
            .ok_or_else(|| refusal("compaction directive has no model-facing text"))?;
        let mut text = String::new();
        let mut count = 0;
        for part in parts {
            if crate::stream::text(part, "type", line)? != "text" {
                return Err(refusal("compaction directive contains non-text content"));
            }
            text.push_str(crate::stream::text(part, "text", line)?);
            count += 1;
        }
        if count == 0 {
            return Err(refusal("compaction directive has no model-facing text"));
        }
        text
    };
    let marker = "your answer may generate at most ";
    let claimed = directive
        .rfind(marker)
        .map(|at| &directive[at + marker.len()..])
        .ok_or_else(|| refusal("compaction request has no declared output ceiling"))?;
    let digits = claimed.bytes().take_while(|digit| digit.is_ascii_digit()).count();
    let stated = claimed[..digits]
        .parse::<u64>()
        .map_err(|_| refusal("compaction directive has no valid output ceiling"))?;
    if stated != budget
        || !claimed[digits..].starts_with(" tokens, reasoning included.")
        || !directive.ends_with("cannot continue.")
    {
        return Err(refusal("compaction directive differs from its physical output ceiling"));
    }
    Ok(budget)
}

/// Count the values the pinned OpenAI SDK can yield from one physical body.
/// Streaming state retains only the unfinished SSE event; nonstreaming bodies
/// have one SDK value when their HTTP status and media type permit decoding.
enum ResponseValues {
    Stream(SseValues),
    Nonstream(Vec<u8>),
}

impl ResponseValues {
    fn new(stream: bool, retain_values: bool) -> Self {
        if stream {
            Self::Stream(SseValues { retain_values, ..SseValues::default() })
        } else {
            Self::Nonstream(Vec::new())
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        match self {
            Self::Stream(values) => values.push(bytes),
            Self::Nonstream(body) => body.extend_from_slice(bytes),
        }
    }

    fn stream_values(&self, transport_eof: bool) -> Option<Vec<serde_json::Value>> {
        match self {
            Self::Stream(values) => Some(values.at_eof(transport_eof).values),
            Self::Nonstream(_) => None,
        }
    }

    fn require_prefix(
        &self,
        seen: u64,
        completed: bool,
        status: Option<u64>,
        content_type: Option<&str>,
        transport_eof: bool,
    ) -> ContractResult<()> {
        let (available, failed) = if status.is_none_or(|status| !(200..300).contains(&status)) {
            // The SDK rejects an unsuccessful HTTP response before decoding it.
            (0, false)
        } else {
            match self {
                Self::Stream(values) => {
                    if transport_eof
                        && (!values.pending.is_empty()
                            || values.after_cr
                            || !values.line.is_empty())
                        && !values.failed
                        && !values.done
                    {
                        let settled = values.at_eof(true);
                        (settled.count, settled.failed)
                    } else {
                        (values.count, values.failed)
                    }
                }
                Self::Nonstream(_) if status == Some(204) => (1, false),
                Self::Nonstream(body) => {
                    let media_type = content_type
                        .and_then(|value| value.split(';').next())
                        .map(str::trim);
                    let is_json = media_type.is_some_and(|value| {
                        value.contains("application/json") || value.ends_with("+json")
                    });
                    if is_json {
                        // Node's fetch Body parser removes up to two leading
                        // UTF-8 marks before the pinned SDK parses JSON.
                        let bom = &[0xef, 0xbb, 0xbf];
                        let without_first = body.strip_prefix(bom).unwrap_or(body);
                        let without_second =
                            without_first.strip_prefix(bom).unwrap_or(without_first);
                        let text = String::from_utf8_lossy(without_second);
                        let valid = serde_json::from_str::<serde_json::Value>(&text).is_ok();
                        (u64::from(valid), !valid)
                    } else {
                        // The SDK returns even an empty text body as one value.
                        (1, false)
                    }
                }
            }
        };
        if seen > available || (completed && (failed || seen != available)) {
            return Err(refusal(
                "SDK value count claims an impossible physical response prefix",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Default)]
struct SseValues {
    pending: Vec<u8>,
    line: Vec<u8>,
    event: Option<String>,
    data: Vec<String>,
    count: u64,
    values: Vec<serde_json::Value>,
    retain_values: bool,
    failed: bool,
    done: bool,
    after_cr: bool,
}

impl SseValues {
    fn at_eof(&self, transport_eof: bool) -> Self {
        let mut settled = self.clone();
        if transport_eof
            && (!settled.pending.is_empty() || settled.after_cr || !settled.line.is_empty())
            && !settled.failed
            && !settled.done
        {
            // The pinned SDK yields its final incomplete SSE chunk at EOF.
            let pending = std::mem::take(&mut settled.pending);
            settled.feed(&pending);
            if settled.after_cr || !settled.line.is_empty() {
                settled.after_cr = false;
                settled.finish_line();
            }
        }
        settled
    }

    fn push(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if self.failed || self.done {
                return;
            }
            self.pending.push(byte);
            let len = self.pending.len();
            let complete_chunk = (len >= 2
                && (&self.pending[len - 2..] == b"\n\n" || &self.pending[len - 2..] == b"\r\r"))
                || (len >= 4 && &self.pending[len - 4..] == b"\r\n\r\n");
            if complete_chunk {
                let chunk = std::mem::take(&mut self.pending);
                self.feed(&chunk);
            }
        }
    }

    fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if self.failed || self.done {
                return;
            }
            if self.after_cr {
                self.after_cr = false;
                self.finish_line();
                if byte == b'\n' {
                    continue;
                }
            }
            match byte {
                b'\r' => self.after_cr = true,
                b'\n' => self.finish_line(),
                _ => self.line.push(byte),
            }
        }
    }

    fn finish_line(&mut self) {
        // The pinned SDK decodes each SSE line with a fresh TextDecoder, which
        // removes one leading UTF-8 byte order mark from every physical line.
        let line = String::from_utf8_lossy(
            self.line
                .strip_prefix(&[0xef, 0xbb, 0xbf])
                .unwrap_or(&self.line),
        )
        .into_owned();
        self.line.clear();
        if line.is_empty() {
            self.finish_event();
            return;
        }
        if line.starts_with(':') {
            return;
        }
        let (field, value) = line.split_once(':').unwrap_or((&line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => self.event = Some(value.to_string()),
            "data" => self.data.push(value.to_string()),
            _ => {}
        }
    }

    fn finish_event(&mut self) {
        if self.event.as_deref().unwrap_or("").is_empty() && self.data.is_empty() {
            self.event = None;
            return;
        }
        let name = self.event.take();
        let payload = self.data.join("\n");
        self.data.clear();
        if payload.starts_with("[DONE]") {
            self.done = true;
            return;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&payload) else {
            self.failed = true;
            return;
        };
        let ordinary = !name
            .as_deref()
            .is_some_and(|event| event.starts_with("thread."));
        if ordinary && value.get("error").is_some_and(js_truthy) {
            self.failed = true;
            return;
        }
        match self
            .count
            .checked_add(1)
            .filter(|count| *count <= SAFE_INTEGER)
        {
            Some(count) => {
                self.count = count;
                if self.retain_values {
                    self.values.push(value);
                }
            }
            None => self.failed = true,
        }
    }
}

fn js_truthy(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(value) => *value,
        serde_json::Value::Number(value) => value.as_f64().is_none_or(|number| number != 0.0),
        serde_json::Value::String(value) => !value.is_empty(),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => true,
    }
}

#[derive(Default)]
pub(crate) struct ModelRequests {
    journal_id: Option<String>,
    first_sequence: Option<u64>,
    ids: BTreeSet<String>,
    scopes: BTreeMap<String, RequestBody>,
    responses: BTreeMap<String, ResponseState>,
    response_values: BTreeMap<String, ResponseValues>,
    attempts: BTreeMap<String, AttemptState>,
    compactions: BTreeMap<String, CompactionOperation>,
    claimed_compactions: BTreeSet<String>,
    generation_ids: BTreeSet<String>,
    usage: BTreeMap<String, GenerationUsageSummary>,
    all_usage: GenerationUsageSummary,
}

#[derive(Default)]
struct CompactionOperation {
    scope: String,
    budget: u64,
    first_sequence: u64,
    last_sequence: u64,
    requests: Vec<String>,
    open: BTreeSet<String>,
    outcomes: BTreeMap<String, ResponseOutcome>,
    values: BTreeMap<String, Vec<serde_json::Value>>,
    deliveries: BTreeMap<String, u64>,
}

#[derive(Default)]
struct AttemptState {
    scope: String,
    requests: Vec<String>,
    settled: BTreeMap<String, bool>,
    outcomes: BTreeMap<String, ResponseOutcome>,
    seed: Option<NormalizationSeed>,
    generation: Option<Arc<Generation>>,
    completed: bool,
}

#[derive(Clone)]
struct NormalizationSeed {
    origin: Origin,
    history_call_ids: Vec<String>,
}

pub(crate) struct SeedAdmission {
    seed: NormalizationSeed,
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
    pub sdk_values_seen: u64,
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
    compaction: Option<(String, u64)>,
    stream: bool,
    usage: GenerationUsageSummary,
    all_usage: GenerationUsageSummary,
}

impl RequestAdmission {
    pub(crate) fn is_chat_attempt(&self) -> bool {
        self.attempt.is_some()
    }
}

#[derive(Clone, Default)]
struct ResponseState {
    scope: String,
    attempt: Option<String>,
    compaction: Option<String>,
    processing: Option<bool>,
    pipeline_outputs: Option<u64>,
    sequence: u64,
    http_status: Option<u64>,
    content_type: Option<String>,
    termination: Option<String>,
    bytes: u64,
    digest: Option<Sha256>,
}
pub(crate) struct ResponseAdmission {
    request_id: String,
    state: Option<ResponseState>,
    attempt: Option<String>,
    settlement: Option<bool>,
    compaction: Option<String>,
    delivery: Option<u64>,
    decoded_values: Option<Vec<serde_json::Value>>,
    pub outcome: Option<ResponseOutcome>,
    usage: Option<(GenerationUsageSummary, GenerationUsageSummary)>,
    body: Option<Vec<u8>>,
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
        let (attempt, compaction_id) = match text(owner, "kind", line)? {
            "chat" => {
                let id = text(owner, "attempt_id", line)?;
                if self.attempts.get(id).is_some_and(|attempt| {
                    attempt.scope != scope
                        || attempt.generation.is_some()
                        || attempt.seed.is_some()
                        || attempt.completed
                        || attempt.settled.values().any(|accepted| *accepted)
                }) {
                    return Err(refusal(
                        "chat attempt changes scope or issues a request after normalization or acceptance",
                    ));
                }
                (Some(id.to_string()), None)
            }
            "utility" => {
                let id = text(owner, "operation_id", line)?;
                let purpose = text(owner, "purpose", line)?;
                (None, if purpose == "compaction" { Some(id.to_string()) } else { None })
            }
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
            return Err(refusal(
                "selected decoder contradicts the request stream mode",
            ));
        }
        let model = text(root, "model", line)?;
        if model.trim().is_empty() || model != text(policy, "model", line)? {
            return Err(refusal("selected decoder contradicts the dispatched model"));
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
        let compaction = if let Some(id) = compaction_id {
            if !stream {
                return Err(refusal("compaction request is not a streamed draw"));
            }
            let budget = compaction_request_budget(root, line)?;
            if self.claimed_compactions.contains(&id)
                || self.compactions.get(&id).is_some_and(|operation| {
                    operation.scope != scope || operation.budget != budget
                })
            {
                return Err(refusal("compaction operation changes scope or ceiling, or issues another request after its claim"));
            }
            Some((id, budget))
        } else {
            None
        };
        Ok(RequestAdmission {
            journal_id: journal_id.to_string(),
            sequence,
            scope: scope.to_string(),
            stream,
            usage: self.scope_usage(scope).admit_request()?,
            all_usage: self.all_usage.admit_request()?,
            attempt,
            compaction,
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
        if let Some((id, budget)) = &admission.compaction {
            let operation = self.compactions.entry(id.clone()).or_insert_with(|| CompactionOperation {
                scope: admission.scope.clone(),
                budget: *budget,
                first_sequence: admission.sequence,
                last_sequence: admission.sequence,
                ..CompactionOperation::default()
            });
            operation.last_sequence = admission.sequence;
            operation.requests.push(admission.body.id.clone());
            operation.open.insert(admission.body.id.clone());
        }
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
        self.response_values.insert(
            admission.body.id.clone(),
            ResponseValues::new(admission.stream, admission.compaction.is_some()),
        );
        self.responses.insert(
            admission.body.id.clone(),
            ResponseState {
                scope: admission.scope.clone(),
                digest: Some(Sha256::default()),
                attempt: admission.attempt,
                compaction: admission.compaction.map(|(id, _)| id),
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
        let mut delivery = None;
        let mut outcome = None;
        let mut body = None;
        let kind = text(event, "kind", line)?;
        if matches!(kind, "history" | "delivery") != state.processing.is_some()
            || (!matches!(kind, "history" | "delivery")
                && (kind == "outcome") != state.digest.is_none())
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
                let media_type = field(event, "content_type", line)?;
                state.content_type = if media_type.is_null() {
                    None
                } else {
                    Some(text(event, "content_type", line)?.to_string())
                };
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
                state.digest.as_mut().unwrap().update(&bytes);
                body = Some(bytes);
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
                self.response_values
                    .get(id)
                    .ok_or_else(|| refusal("response has no physical value reader"))?
                    .require_prefix(
                        sdk_values_seen,
                        completed,
                        state.http_status,
                        state.content_type.as_deref(),
                        state.termination.as_deref() == Some("eof"),
                    )?;
                outcome = Some(ResponseOutcome {
                    scope: state.scope.clone(),
                    usage: if usage.is_null() {
                        None
                    } else {
                        Some(ServedUsage::read(usage, line)?)
                    },
                    completed,
                    pipeline_outputs_delivered,
                    sdk_values_seen,
                });
                state.processing = Some(completed);
                state.pipeline_outputs = Some(pipeline_outputs_delivered);
            }
            "delivery" => {
                let delivered = unsigned(
                    field(event, "outputs_delivered", line)?,
                    "utility outputs delivered",
                    SAFE_INTEGER,
                )?;
                if state.attempt.is_some()
                    || state
                        .pipeline_outputs
                        .is_none_or(|pipeline_outputs| delivered > pipeline_outputs)
                {
                    return Err(refusal(
                        "utility delivery has no matching physical output prefix",
                    ));
                }
                delivery = Some(delivered);
                ended = true;
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
                            attempt.generation.is_none()
                                || attempt.settled.values().any(|accepted| *accepted)
                        })
                    {
                        return Err(refusal(
                            "chat history acceptance precedes generation, repeats acceptance or has no completed response",
                        ));
                    }
                }
                settlement = Some(accepted);
                ended = true;
            }
            _ => return Err(refusal("response uses an unknown event")),
        }
        state.sequence = sequence;
        let decoded_values = if outcome.is_some() && state.compaction.is_some() {
            Some(self.response_values
                .get(id)
                .and_then(|values| values.stream_values(state.termination.as_deref() == Some("eof")))
                .ok_or_else(|| refusal("compaction response has no streamed physical values"))?)
        } else {
            None
        };
        Ok(ResponseAdmission {
            request_id: id.to_string(),
            attempt: state.attempt.clone(),
            compaction: state.compaction.clone(),
            settlement,
            delivery,
            decoded_values,
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
            body,
            state: if ended { None } else { Some(state) },
        })
    }

    pub(crate) fn commit_response(&mut self, admission: ResponseAdmission) {
        if let Some(id) = &admission.compaction {
            let operation = self.compactions.get_mut(id).expect("planned compaction operation");
            if let Some(outcome) = &admission.outcome {
                operation.outcomes.insert(admission.request_id.clone(), outcome.clone());
            }
            if let Some(values) = &admission.decoded_values {
                operation.values.insert(admission.request_id.clone(), values.clone());
            }
            if let Some(delivered) = admission.delivery {
                operation.open.remove(&admission.request_id);
                operation.deliveries.insert(admission.request_id.clone(), delivered);
            }
        }
        if let Some(body) = &admission.body {
            self.response_values
                .get_mut(&admission.request_id)
                .expect("planned physical value reader")
                .push(body);
        }
        if admission.outcome.is_some() {
            self.response_values.remove(&admission.request_id);
        }
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

    pub(crate) fn has_chat_attempt_in_scope(&self, scope: &str) -> bool {
        self.attempts.values().any(|attempt| attempt.scope == scope)
    }

    pub(crate) fn plan_seed(
        &self,
        record: Value<'_>,
        line: usize,
    ) -> ContractResult<SeedAdmission> {
        let value = field(record, "normalization_seed", line)?;
        let origin = Origin::read(field(value, "origin", line)?, line)?;
        let attempt = self
            .attempts
            .get(&origin.attempt)
            .ok_or_else(|| refusal("normalization seed has no logical request owner"))?;
        let journal = text(value, "journal_id", line)?;
        let request = text(value, "request_id", line)?;
        if self.journal_id.as_deref() != Some(journal)
            || attempt.scope != origin.scope
            || attempt.completed
            || attempt.generation.is_some()
            || attempt.seed.is_some()
            || attempt.requests.last().map(String::as_str) != Some(request)
        {
            return Err(refusal(
                "normalization seed has no unique final physical request",
            ));
        }
        let mut unique = BTreeSet::new();
        let mut history_call_ids = Vec::new();
        for value in field(value, "history_call_ids", line)?
            .elements()
            .ok_or_else(|| refusal("normalization seed lacks history identities"))?
        {
            let id = value
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| refusal("normalization seed has an invalid history identity"))?;
            if !unique.insert(id.to_string()) {
                return Err(refusal("normalization seed repeats a history identity"));
            }
            history_call_ids.push(id.to_string());
        }
        Ok(SeedAdmission {
            seed: NormalizationSeed {
                origin,
                history_call_ids,
            },
        })
    }

    pub(crate) fn commit_seed(&mut self, admission: SeedAdmission) {
        self.attempts
            .get_mut(&admission.seed.origin.attempt)
            .expect("planned normalization seed owner")
            .seed = Some(admission.seed);
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
        let generation = Generation::read(
            field(record, "generation", line)?,
            line,
            json,
            schema,
            |origin| {
                self.attempts
                    .get(&origin.attempt)
                    .and_then(|attempt| attempt.seed.as_ref())
                    .filter(|seed| &seed.origin == origin)
                    .map(|seed| seed.history_call_ids.clone())
                    .ok_or_else(|| refusal("generation has no recorded normalization seed"))
            },
        )?;
        let attempt = self
            .attempts
            .get(&generation.origin.attempt)
            .ok_or_else(|| refusal("generation has no logical request owner"))?;
        if self.journal_id.as_deref() != Some(generation.journal.as_str())
            || attempt.scope != generation.origin.scope
            || attempt.completed
            || attempt.generation.is_some()
            || attempt.seed.is_none()
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
            || (accepted && consumer_observations != final_outcome.pipeline_outputs_delivered)
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
        attempt.seed = None;
        attempt.outcomes.clear();
        attempt.completed = true;
    }

    pub(crate) fn check_compaction_draw(
        &self,
        operation_id: &str,
        scope: &str,
        physical_requests: u64,
        sdk_values: Value<'_>,
    ) -> ContractResult<(u64, u64, u64)> {
        let operation = self
            .compactions
            .get(operation_id)
            .ok_or_else(|| refusal("compaction draw has no physical utility operation"))?;
        if operation.scope != scope
            || !operation.open.is_empty()
            || u64::try_from(operation.requests.len()).ok() != Some(physical_requests)
            || operation.outcomes.len() != operation.requests.len()
            || operation.deliveries.len() != operation.requests.len()
        {
            return Err(refusal("compaction draw omits, repeats or misattributes physical requests"));
        }
        let mut observed_values = 0_u64;
        let mut physical_values = Vec::new();
        for (index, request_id) in operation.requests.iter().enumerate() {
            let outcome = operation.outcomes.get(request_id)
                .ok_or_else(|| refusal("compaction request has no processing outcome"))?;
            if outcome.usage.is_some_and(|usage| usage.output > operation.budget) {
                return Err(refusal("compaction physical output exceeded its request ceiling"));
            }
            let delivered = operation.deliveries.get(request_id)
                .ok_or_else(|| refusal("compaction request has no delivery receipt"))?;
            if index + 1 < operation.requests.len() && *delivered != 0 {
                return Err(refusal("compaction draw mixes output from physical retries"));
            }
            observed_values = observed_values.checked_add(outcome.sdk_values_seen)
                .filter(|total| *total <= SAFE_INTEGER)
                .ok_or_else(|| refusal("compaction SDK value count exceeds exact range"))?;
            let values = operation.values.get(request_id)
                .ok_or_else(|| refusal("compaction request has no captured SDK values"))?;
            if values.len() as u64 != outcome.sdk_values_seen {
                return Err(refusal("compaction processing count differs from physical SDK values"));
            }
            physical_values.extend(values.iter());
        }
        let claimed = sdk_values.elements()
            .ok_or_else(|| refusal("compaction draw has no SDK value array"))?
            .map(|value| {
                let raw = value.as_str()
                    .ok_or_else(|| refusal("compaction draw has a non-string SDK value"))?;
                serde_json::from_str::<serde_json::Value>(raw)
                    .map_err(|_| refusal("compaction draw has undecodable SDK value JSON"))
            })
            .collect::<ContractResult<Vec<_>>>()?;
        if observed_values != claimed.len() as u64
            || !physical_values.into_iter().zip(&claimed).all(|(left, right)| same_sdk_value(left, right))
        {
            return Err(refusal("compaction SDK values differ from physical response bytes"));
        }
        Ok((operation.budget, operation.first_sequence, operation.last_sequence))
    }

    pub(crate) fn commit_compaction_claims(&mut self, ids: Vec<String>) {
        for id in ids {
            self.compactions.remove(&id).expect("planned compaction draw");
            self.claimed_compactions.insert(id);
        }
    }

    pub(crate) fn validate_summary(&self, record: Value<'_>, line: usize) -> ContractResult<()> {
        if !self.compactions.is_empty() {
            return Err(refusal("terminal leaves physical compaction draws unclaimed"));
        }
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
        json!({"request":{"journal_id":"j","sequence":sequence,"request_id":id,"owner":{"kind":"utility","operation_id":"utility-operation","purpose":"other"},"kv_scope":"owner","segment_id":"segment","prompt_id":"p","decode_policy":{"mode":"nonstream","model":"fixture-model","strict_tool_calling":false,"named_tool_choice":null,"exact_token_counting":false,"tagged_thinking_tags":false},"body_bytes":json.len(),"body_sha256":sha256(json),"body":body}}).to_string()
    }
    fn admit(state: &mut ModelRequests, json: &str) -> ContractResult<()> {
        let document = Document::decode(json.as_bytes(), LIMITS).unwrap();
        let admission = state.plan(document.root(), 1, LIMITS)?;
        state.commit(admission);
        Ok(())
    }
    #[test]
    fn physical_value_reader_follows_sdk_sse_boundaries_and_errors() {
        let mut stream = ResponseValues::new(true, false);
        for chunk in [
            ": keepalive\r".as_bytes(),
            "\ndata: {\"id\":1}\r".as_bytes(),
            "\n\r\nevent: thread.message\n".as_bytes(),
            "data: {\"error\":{\"message\":\"ordinary only\"}}\n\n".as_bytes(),
        ] {
            stream.push(chunk);
        }
        stream
            .require_prefix(2, true, Some(200), Some("text/event-stream"), true)
            .unwrap();
        assert!(stream
            .require_prefix(3, false, Some(200), Some("text/event-stream"), true)
            .is_err());
        stream.push(b"data: [DONE]\n\ndata: {\"id\":3}\n\n");
        stream
            .require_prefix(2, true, Some(200), Some("text/event-stream"), true)
            .unwrap();

        let mut stopped = ResponseValues::new(true, false);
        stopped.push(b"data: {\"id\":1}\n\ndata: {\"error\":{\"message\":\"stop\"}}\n\n");
        stopped
            .require_prefix(1, false, Some(200), Some("text/event-stream"), true)
            .unwrap();
        assert!(stopped
            .require_prefix(1, true, Some(200), Some("text/event-stream"), true)
            .is_err());
        assert!(stopped
            .require_prefix(2, false, Some(200), Some("text/event-stream"), true)
            .is_err());

        let mut nonstream = ResponseValues::new(false, false);
        nonstream.push(b"{");
        nonstream.push(b"}");
        nonstream
            .require_prefix(1, true, Some(200), Some("application/json"), true)
            .unwrap();
        for count in 1..=3 {
            let mut with_bom = ResponseValues::new(false, false);
            for _ in 0..count {
                with_bom.push(&[0xef, 0xbb, 0xbf]);
            }
            with_bom.push(b"{}");
            assert_eq!(
                with_bom
                    .require_prefix(
                        u64::from(count <= 2),
                        true,
                        Some(200),
                        Some("application/json"),
                        true
                    )
                    .is_ok(),
                count <= 2,
            );
        }
        assert!(nonstream
            .require_prefix(0, true, Some(200), Some("application/json"), true)
            .is_err());
        assert!(nonstream
            .require_prefix(2, false, Some(200), Some("application/json"), true)
            .is_err());
        assert!(nonstream
            .require_prefix(1, false, Some(503), Some("application/json"), true)
            .is_err());

        let mut plain = ResponseValues::new(false, false);
        plain.push(b"{not JSON}");
        plain
            .require_prefix(1, true, Some(200), Some("text/plain"), true)
            .unwrap();
        assert!(plain
            .require_prefix(1, true, Some(200), Some("application/json"), true)
            .is_err());
        let empty = ResponseValues::new(false, false);
        empty
            .require_prefix(1, false, Some(204), Some("application/json"), true)
            .unwrap();
        assert!(empty.require_prefix(1, false, None, None, true).is_err());
    }
    #[test]
    fn physical_value_reader_decodes_each_sse_line_like_the_sdk() {
        let mut stream = ResponseValues::new(true, false);
        stream.push(&[0xef]);
        stream.push(b"\xbb\xbfdata: {\"id\":1}\n\n\xef\xbb\xbfdata: {\"id\":2}\n\n");
        stream
            .require_prefix(2, true, Some(200), Some("text/event-stream"), true)
            .unwrap();

        let mut double_bom = ResponseValues::new(true, false);
        double_bom.push(b"\xef\xbb\xbf\xef\xbb\xbfdata: {\"id\":1}\n\n");
        double_bom
            .require_prefix(0, true, Some(200), Some("text/event-stream"), true)
            .unwrap();
    }
    #[test]
    fn physical_value_reader_flushes_an_unterminated_line_only_at_eof() {
        let mut stream = ResponseValues::new(true, false);
        stream.push(b"data: {\"id\":1}\n\xef");
        stream.push(b"\xbb\xbf");
        stream
            .require_prefix(1, true, Some(200), Some("text/event-stream"), true)
            .unwrap();
        assert!(stream
            .require_prefix(1, false, Some(200), Some("text/event-stream"), false)
            .is_err());

        let mut mixed = ResponseValues::new(true, false);
        mixed.push(b"data: {\"id\":1}\n\r");
        mixed
            .require_prefix(1, true, Some(200), Some("text/event-stream"), true)
            .unwrap();
        assert!(mixed
            .require_prefix(1, false, Some(200), Some("text/event-stream"), false)
            .is_err());

        let mut delayed_cr = ResponseValues::new(true, false);
        delayed_cr.push(b"data: {\"id\":1}\r\r");
        delayed_cr
            .require_prefix(1, true, Some(200), Some("text/event-stream"), true)
            .unwrap();
        assert!(delayed_cr
            .require_prefix(1, false, Some(200), Some("text/event-stream"), false)
            .is_err());
    }
    #[test]
    fn selected_decoder_mode_must_match_the_request_body() {
        let body = r#"{"kv_scope":"owner","model":"fixture-model","stream":false,"messages":[]}"#;
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
    fn selected_decoder_model_must_match_the_request_body() {
        let body = r#"{"kv_scope":"owner","model":"fixture-model","stream":false,"messages":[]}"#;
        let mut record: serde_json::Value = serde_json::from_str(&request(
            1,
            "request",
            json!({"kind":"full","json":body}),
            body,
        ))
        .unwrap();
        record["request"]["decode_policy"]["model"] = json!("another-model");
        let mut state = ModelRequests::default();
        assert!(matches!(
            admit(&mut state, &record.to_string()),
            Err(ContractError::InvalidRecord(message))
                if message.contains("selected decoder contradicts the dispatched model")
        ));
    }
    #[test]
    fn chat_request_cannot_follow_its_normalization_seed() {
        let rows: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("fixtures/ordinary-tool-wire.json")).unwrap();
        let mut state = ModelRequests::default();
        let start = Document::decode(rows[0].to_string().as_bytes(), LIMITS).unwrap();
        let origin = state
            .plan_origin(
                field(start.root(), "request_evidence_origin", 1).unwrap(),
                1,
            )
            .unwrap();
        state.commit_origin(origin);
        let request = rows
            .iter()
            .find(|row| row["type"] == "model_request")
            .unwrap();
        let document = Document::decode(request.to_string().as_bytes(), LIMITS).unwrap();
        let admission = state.plan(document.root(), 1, LIMITS).unwrap();
        state.commit(admission);
        let seed = rows
            .iter()
            .find(|row| row["type"] == "model_normalization_seed")
            .unwrap();
        let document = Document::decode(seed.to_string().as_bytes(), LIMITS).unwrap();
        let admission = state.plan_seed(document.root(), 1).unwrap();
        state.commit_seed(admission);

        let mut later = request.clone();
        later["request"]["sequence"] = json!(2);
        later["request"]["request_id"] = json!("retry-after-seed");
        later["request"]["segment_id"] = json!("new-segment");
        let document = Document::decode(later.to_string().as_bytes(), LIMITS).unwrap();
        let refusal = match state.plan(document.root(), 1, LIMITS) {
            Ok(_) => panic!("chat request followed its normalization seed"),
            Err(error) => error,
        };
        assert!(refusal.to_string().contains("after normalization"));
    }
    #[test]
    fn full_body_requires_a_new_segment_for_its_scope() {
        let body = r#"{"kv_scope":"owner","model":"fixture-model","stream":false,"messages":[]}"#;
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
                |_| Ok(vec!["provider".to_string()]),
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
                                sdk_values_seen: 0,
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
                                sdk_values_seen: 0,
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
                let body =
                    r#"{"kv_scope":"owner","model":"fixture-model","stream":false,"messages":[]}"#;
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
                    "sdk_values_seen": if status == "completed" { 1 } else { 0 }, "pipeline_outputs_delivered": 0,
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
                assert!(!state.responses.is_empty(), "outcome is not delivery");
                if status != "completed" || can_complete {
                    let raw = json!({"response": {
                        "journal_id": "j", "request_id": "r",
                        "sequence": events.len() + 1,
                        "event": {"kind": "delivery", "outputs_delivered": 0}
                    }})
                    .to_string();
                    let doc = Document::decode(raw.as_bytes(), LIMITS).unwrap();
                    let admission = state.plan_response(doc.root(), 1).unwrap();
                    state.commit_response(admission);
                    assert!(
                        state.responses.is_empty(),
                        "{http_status:?}/{termination}/{status}"
                    );
                }
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
                let body =
                    r#"{"kv_scope":"owner","model":"fixture-model","stream":false,"messages":[]}"#;
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
                    "sdk_values_seen":if status == "completed" { 1 } else { 0 },"pipeline_outputs_delivered":0,
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
        let first = r#"{"kv_scope":"owner","model":"fixture-model","stream":false,"messages":[{"role":"system","content":"tools \\"}],"tools":[{"messages":"nested"}]}"#;
        let second = r#"{"kv_scope":"owner","model":"fixture-model","stream":false,"messages":[{"role":"system","content":"tools \\"},{"role":"user","content":"שלום\n"}],"tools":[]}"#;
        let a = request(3, "a", json!({"kind":"full","json":first}), first);
        let b = request(
            4,
            "b",
            json!({"kind":"delta","base_request_id":"a","retain_messages":1,"prefix":"{\"kv_scope\":\"owner\",\"model\":\"fixture-model\",\"stream\":false,\"messages\":[","suffix":"],\"tools\":[]}","added_messages":[r#"{"role":"user","content":"שלום\n"}"#]}),
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
                json!({"kind":"outcome","served_usage":null,"status":"completed","error":null,"sdk_values_seen":1,"pipeline_outputs_delivered":0}),
                json!({"kind":"delivery","outputs_delivered":0}),
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
                "model_normalization_seed" => {
                    let plan = state.plan_seed(document.root(), 1).unwrap();
                    state.commit_seed(plan);
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
            let body =
                r#"{"kv_scope":"owner","model":"fixture-model","stream":false,"messages":[]}"#;
            let mut value: serde_json::Value =
                serde_json::from_str(&request(1, "r", json!({"kind":"full","json":body}), body))
                    .unwrap();
            value["request"]["owner"] = json!({"kind":"chat","attempt_id":"attempt"});
            admit(&mut state, &value.to_string()).unwrap();
            for (index, event) in [
                json!({"kind":"http","status":200,"content_type":"application/json"}),
                json!({"kind":"body","offset":0,"base64":"e30="}),
                json!({"kind":"end","termination":"eof","body_bytes":2,"body_sha256":sha256("{}"),"error":null}),
                json!({"kind":"outcome","served_usage":null,"status":status,"error":if status == "failed" {json!("failure")} else {json!(null)},"sdk_values_seen":1,"pipeline_outputs_delivered":0}),
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
