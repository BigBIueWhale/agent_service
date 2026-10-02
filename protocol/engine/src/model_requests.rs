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
use unicode_normalization::UnicodeNormalization;

fn sha256(json: &str) -> String {
    Sha256::digest(json.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn utility_url_path(url: &str) -> Option<&str> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let slash = rest.find('/')?;
    if slash == 0 {
        return None;
    }
    rest[slash..].split(['?', '#']).next()
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
            (Json::Number(left), Json::Number(right)) if left.as_f64() == right.as_f64() => {}
            (Json::String(left), Json::String(right)) if left == right => {}
            (Json::Array(left), Json::Array(right)) if left.len() == right.len() => {
                pending.extend(left.iter().zip(right));
            }
            (Json::Object(left), Json::Object(right)) if left.len() == right.len() => {
                for (key, value) in left {
                    let Some(other) = right.get(key) else {
                        return false;
                    };
                    pending.push((value, other));
                }
            }
            _ => return false,
        }
    }
    true
}

const SNAPSHOT_ELEMENTS: [&str; 9] = [
    "primary_request_and_intent",
    "key_technical_concepts",
    "files_and_code_sections",
    "errors_and_fixes",
    "problem_solving",
    "all_user_messages",
    "pending_tasks",
    "current_work",
    "next_step",
];

fn snapshot_bytes(calls: &[serde_json::Value]) -> Option<usize> {
    let call = calls.first()?.as_object()?;
    if calls.len() != 1 || call.get("name")?.as_str()? != "state_snapshot" {
        return None;
    }
    let args = call.get("args")?.as_object()?;
    if args.len() != SNAPSHOT_ELEMENTS.len() - 1
        || SNAPSHOT_ELEMENTS
            .iter()
            .filter(|section| **section != "all_user_messages")
            .any(|section| {
                args.get(*section)
                    .and_then(serde_json::Value::as_str)
                    .is_none_or(|text| text.trim().is_empty())
            })
    {
        return None;
    }
    let element = |section: &str| -> String {
        let value = args
            .get(section)
            .and_then(serde_json::Value::as_str)
            .expect("checked section");
        let mut result = format!("    <{section}>");
        for line in value.split('\n') {
            result.push('\n');
            if !line.is_empty() {
                result.push_str("        ");
                result.push_str(line);
            }
        }
        result.push_str(&format!("\n    </{section}>"));
        result
    };
    let before = SNAPSHOT_ELEMENTS[..5]
        .iter()
        .map(|section| element(section))
        .collect::<Vec<_>>()
        .join("\n\n");
    let after = SNAPSHOT_ELEMENTS[6..]
        .iter()
        .map(|section| element(section))
        .collect::<Vec<_>>()
        .join("\n\n");
    let opening = format!("<state_snapshot>\n{before}\n\n    <all_user_messages>\n")
        .nfc()
        .collect::<String>();
    let closing = format!("\n    </all_user_messages>\n\n{after}\n</state_snapshot>")
        .nfc()
        .collect::<String>();
    Some(opening.len() + closing.len())
}

fn decoded_usage(value: Option<&serde_json::Value>) -> Option<serde_json::Value> {
    let object = value?.as_object()?;
    let count = |name: &str| {
        object
            .get(name)?
            .as_u64()
            .filter(|value| *value <= SAFE_INTEGER)
    };
    let prompt = count("promptTokenCount")?;
    let output = count("candidatesTokenCount")?;
    let total = count("totalTokenCount")?;
    let thoughts = count("thoughtsTokenCount")?;
    let cached = count("cachedContentTokenCount")?;
    if prompt.checked_add(output) != Some(total) || thoughts > output || cached > prompt {
        return None;
    }
    Some(serde_json::json!({
        "promptTokenCount": prompt, "candidatesTokenCount": output,
        "totalTokenCount": total, "thoughtsTokenCount": thoughts,
        "cachedContentTokenCount": cached,
    }))
}

fn project_decoded_draw(
    observations: &[serde_json::Value],
    failure: Option<&serde_json::Value>,
) -> ContractResult<serde_json::Value> {
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut calls = Vec::new();
    let mut incomplete = Vec::new();
    let mut usage = None;
    let mut finish_reason = None;
    let items: Vec<&serde_json::Value> = if observations.is_empty() {
        failure.into_iter().collect()
    } else {
        observations.iter().collect()
    };
    for item in items {
        let response = item
            .get("response")
            .and_then(serde_json::Value::as_object)
            .ok_or_else(|| refusal("decoded observation has no response"))?;
        let first = response
            .get("candidates")
            .and_then(serde_json::Value::as_array)
            .and_then(|values| values.first());
        let parts = first
            .and_then(|value| value.get("content"))
            .and_then(|value| value.get("parts"))
            .and_then(serde_json::Value::as_array);
        let mut part_calls = Vec::new();
        if let Some(parts) = parts {
            for part in parts {
                let Some(part) = part.as_object() else {
                    continue;
                };
                if let Some(segment) = part
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .filter(|value| !value.is_empty())
                {
                    if part.get("thought").is_some_and(js_truthy) {
                        reasoning.push_str(segment);
                    } else {
                        text.push_str(segment);
                    }
                }
                if let Some(call) = part.get("functionCall").filter(|value| js_truthy(value)) {
                    part_calls.push(call.clone());
                }
            }
        }
        if !observations.is_empty() {
            if part_calls.len() == 2 {
                let first = part_calls[0].as_object();
                let second = part_calls[1].as_object();
                if let (Some(first), Some(second)) = (first, second) {
                    let first_args = first.get("args").and_then(serde_json::Value::as_object);
                    let second_args = second.get("args").and_then(serde_json::Value::as_object);
                    if first.get("name").is_some_and(js_truthy)
                        && first_args.is_none_or(|args| args.is_empty())
                        && second.get("name").is_none_or(|name| !js_truthy(name))
                        && second_args.is_some_and(|args| !args.is_empty())
                    {
                        part_calls = vec![serde_json::json!({
                            "name": first.get("name"), "args": second.get("args")
                        })];
                    }
                }
            }
            calls.extend(part_calls);
            incomplete.extend(
                item.get("incomplete_tool_calls")
                    .and_then(serde_json::Value::as_array)
                    .ok_or_else(|| refusal("decoded observation has no incomplete-call array"))?
                    .iter()
                    .cloned(),
            );
            if let Some(reason) = first
                .and_then(|value| value.get("finishReason"))
                .filter(|value| js_truthy(value))
            {
                finish_reason = Some(reason.clone());
            }
        }
        if let Some(metadata) = response
            .get("usageMetadata")
            .filter(|value| js_truthy(value))
        {
            usage = decoded_usage(Some(metadata));
        }
    }
    let snapshot_bytes = snapshot_bytes(&calls);
    Ok(serde_json::json!({
        "text": text, "reasoning": reasoning,
        "functionCalls": calls, "incompleteToolCalls": incomplete,
        "finishReason": finish_reason, "usage": usage,
        "snapshotBytes": snapshot_bytes,
    }))
}

fn openai_served_usage(value: &serde_json::Value) -> Option<ServedUsage> {
    let usage = value.get("usage")?;
    let count = |value: Option<&serde_json::Value>| {
        let number = value?.as_f64()?;
        (number.is_finite()
            && number >= 0.0
            && number <= SAFE_INTEGER as f64
            && number.fract() == 0.0)
            .then_some(number as u64)
    };
    let served = ServedUsage {
        prompt: count(usage.get("prompt_tokens"))?,
        output: count(usage.get("completion_tokens"))?,
        total: count(usage.get("total_tokens"))?,
        cached: count(usage.get("prompt_tokens_details")?.get("cached_tokens"))?,
        thoughts: count(
            usage
                .get("completion_tokens_details")?
                .get("reasoning_tokens"),
        )?,
    };
    served.validate().ok()?;
    Some(served)
}

fn nonstream_json_value(body: &[u8]) -> Option<serde_json::Value> {
    // Node fetch removes up to two leading UTF-8 marks before SDK JSON.parse.
    let bom = &[0xef, 0xbb, 0xbf];
    let first = body.strip_prefix(bom).unwrap_or(body);
    let second = first.strip_prefix(bom).unwrap_or(first);
    serde_json::from_str(&String::from_utf8_lossy(second)).ok()
}

/// The lead every refusal notice opens with, the client's `REFUSED_ANSWER`
/// (`generation-refusal.ts`, pinned by the transformer's contract). No
/// compaction directive opens with it.
const REFUSAL_NOTICE_LEAD: &str = "Your previous answer to this request was refused because";

/// A user message's model-facing text, or `None` when it carries none.
fn user_message_text(message: Value<'_>, line: usize) -> ContractResult<Option<String>> {
    if message.get("role").and_then(Value::as_str) != Some("user") {
        return Ok(None);
    }
    let content = field(message, "content", line)?;
    if let Some(text) = content.as_str() {
        return Ok(Some(text.to_string()));
    }
    let Some(parts) = content.elements() else {
        return Ok(None);
    };
    let mut joined = String::new();
    let mut count = 0;
    for part in parts {
        if part.get("type").and_then(Value::as_str) != Some("text") {
            return Ok(None);
        }
        joined.push_str(text(part, "text", line)?);
        count += 1;
    }
    Ok((count > 0).then_some(joined))
}

/// The output ceiling a compaction draw was issued with, read from the one
/// place its request states it to the model and held to its `max_tokens`.
///
/// A draw's request is the prompt it summarises followed by the directive
/// that states the ceiling. A redraw is that request with one message more
/// after the directive: the notice naming why the draw before it was refused,
/// which opens with [`REFUSAL_NOTICE_LEAD`]. So the directive is the last
/// message, or the message immediately before a final refusal notice, and
/// nowhere else; the number it states must be the request's `max_tokens`.
fn compaction_request_budget(body: Value<'_>, line: usize) -> ContractResult<u64> {
    let budget = unsigned(
        field(body, "max_tokens", line)?,
        "physical compaction ceiling",
        SAFE_INTEGER,
    )?;
    if budget == 0 {
        return Err(refusal(
            "compaction request has no positive physical output ceiling",
        ));
    }
    let messages: Vec<Value<'_>> = field(body, "messages", line)?
        .elements()
        .map(Iterator::collect)
        .unwrap_or_default();
    let last = match messages.last() {
        Some(last) => user_message_text(*last, line)?,
        None => return Err(refusal("compaction request has no final directive")),
    };
    let redrawn = last
        .as_deref()
        .is_some_and(|text| text.starts_with(REFUSAL_NOTICE_LEAD));
    let directive = if redrawn {
        match messages.len().checked_sub(2).map(|at| messages[at]) {
            Some(message) => user_message_text(message, line)?,
            None => None,
        }
        .ok_or_else(|| refusal("compaction refusal notice does not follow a user directive"))?
    } else {
        last.ok_or_else(|| refusal("compaction request does not end in a user directive"))?
    };
    let marker = "your answer may generate at most ";
    let claimed = directive
        .rfind(marker)
        .map(|at| &directive[at + marker.len()..])
        .ok_or_else(|| refusal("compaction request has no declared output ceiling"))?;
    let digits = claimed
        .bytes()
        .take_while(|digit| digit.is_ascii_digit())
        .count();
    let stated = claimed[..digits]
        .parse::<u64>()
        .map_err(|_| refusal("compaction directive has no valid output ceiling"))?;
    if stated != budget
        || !claimed[digits..].starts_with(" tokens, reasoning included.")
        || !directive.ends_with("cannot continue.")
    {
        return Err(refusal(
            "compaction directive differs from its physical output ceiling",
        ));
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
            Self::Stream(SseValues {
                retain_values,
                ..SseValues::default()
            })
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

    /// The SDK values a streamed body decoded, its final unfinished chunk
    /// flushed at EOF as the SDK flushes it. The reader is consumed: the
    /// values move to their owner rather than being copied.
    fn into_stream_values(self, transport_eof: bool) -> Option<Vec<serde_json::Value>> {
        match self {
            Self::Stream(mut values) => {
                values.settle(transport_eof);
                Some(values.values)
            }
            Self::Nonstream(_) => None,
        }
    }

    fn observed_usage(
        &self,
        seen: u64,
        status: Option<u64>,
        content_type: Option<&str>,
        transport_eof: bool,
    ) -> Option<ServedUsage> {
        if seen == 0 || !status.is_some_and(|status| (200..300).contains(&status)) {
            return None;
        }
        match self {
            Self::Stream(values) => values
                .eof_tail(transport_eof)
                .usage_reports
                .iter()
                .rev()
                .chain(values.usage_reports.iter().rev())
                .find(|(position, _)| *position <= seen)
                .map(|(_, usage)| *usage),
            Self::Nonstream(_) if status == Some(204) => None,
            Self::Nonstream(body) => {
                let media_type = content_type
                    .and_then(|value| value.split(';').next())
                    .map(str::trim);
                if !media_type.is_some_and(|value| {
                    value.contains("application/json") || value.ends_with("+json")
                }) {
                    return None;
                }
                nonstream_json_value(body)
                    .as_ref()
                    .and_then(openai_served_usage)
            }
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
                    let settled = values.eof_tail(transport_eof);
                    (settled.count, settled.failed)
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
                        let valid = nonstream_json_value(body).is_some();
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

#[derive(Default)]
struct SseValues {
    pending: Vec<u8>,
    line: Vec<u8>,
    event: Option<String>,
    data: Vec<String>,
    count: u64,
    values: Vec<serde_json::Value>,
    usage_reports: Vec<(u64, ServedUsage)>,
    retain_values: bool,
    failed: bool,
    done: bool,
    after_cr: bool,
}

impl SseValues {
    /// The pinned SDK yields its final incomplete SSE chunk at EOF.
    fn settle(&mut self, transport_eof: bool) {
        if transport_eof
            && (!self.pending.is_empty() || self.after_cr || !self.line.is_empty())
            && !self.failed
            && !self.done
        {
            let pending = std::mem::take(&mut self.pending);
            self.feed(&pending);
            if self.after_cr || !self.line.is_empty() {
                self.after_cr = false;
                self.finish_line();
            }
        }
    }

    /// The reader as EOF would leave it, holding only what settling adds:
    /// its counts are the settled counts, and its values and usage reports
    /// are those the unfinished chunk yields, without the ones already
    /// decoded, which settling never changes.
    fn eof_tail(&self, transport_eof: bool) -> Self {
        let mut tail = Self {
            pending: self.pending.clone(),
            line: self.line.clone(),
            event: self.event.clone(),
            data: self.data.clone(),
            count: self.count,
            values: Vec::new(),
            usage_reports: Vec::new(),
            retain_values: false,
            failed: self.failed,
            done: self.done,
            after_cr: self.after_cr,
        };
        tail.settle(transport_eof);
        tail
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
                if ordinary {
                    if let Some(usage) = openai_served_usage(&value) {
                        self.usage_reports.push((count, usage));
                    }
                }
                if self.retain_values {
                    self.values.push(match name {
                        Some(event) if event.starts_with("thread.") => {
                            serde_json::json!({"event": event, "data": value})
                        }
                        _ => value,
                    });
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
    physical_count: u64,
    ids: BTreeSet<String>,
    utilities: BTreeMap<String, UtilityOperation>,
    completed_utilities: BTreeSet<String>,
    completed_token_counts: BTreeMap<String, CompletedTokenCount>,
    claimed_token_counts: BTreeSet<String>,
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

/// What a compaction's draw claims of its physical request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DrawKind {
    /// An answer the rules judged: its last request completed and delivered it.
    Answer,
    /// A request that failed in transport before it produced an answer: its
    /// last request did not complete.
    Fault,
    /// The draw a compaction ended on for a cause outside both, whose request
    /// may have completed or not.
    Ended,
}

#[derive(Default)]
struct CompactionOperation {
    scope: String,
    budget: u64,
    model: String,
    first_sequence: u64,
    last_sequence: u64,
    requests: Vec<String>,
    open: BTreeSet<String>,
    outcomes: BTreeMap<String, ResponseOutcome>,
    values: BTreeMap<String, Vec<serde_json::Value>>,
    deliveries: BTreeMap<String, u64>,
    observations: BTreeMap<String, Vec<serde_json::Value>>,
    failures: BTreeMap<String, Option<serde_json::Value>>,
}

#[derive(Default)]
struct UtilityOperation {
    first_sequence: u64,
    last_sequence: u64,
    journal_id: String,
    scope: String,
    kind: String,
    model: String,
    input_count: Option<u64>,
    expected_max_model_len: Option<u64>,
    url: String,
    body: String,
    requests: Vec<String>,
    physical: BTreeMap<String, UtilityPhysical>,
}

struct UtilityPhysical {
    status: Option<u64>,
    content_type: Option<String>,
    termination: Option<String>,
    completed: bool,
    delivered: bool,
}

struct CompletedTokenCount {
    scope: String,
    first_sequence: u64,
    last_sequence: u64,
    count: u64,
    window: u64,
    model: String,
}

pub(crate) struct UtilityRequestAdmission {
    sequence: u64,
    journal_id: String,
    request_id: String,
    operation_id: String,
    scope: String,
    kind: String,
    model: String,
    input_count: Option<u64>,
    expected_max_model_len: Option<u64>,
    url: String,
    body: String,
}

pub(crate) struct UtilityCompletionAdmission {
    operation_id: String,
    request_ids: Vec<String>,
    token_count: Option<CompletedTokenCount>,
}

/// A decoded observation still being received: its bytes so far.
struct DecodedPending {
    role: String,
    index: u64,
    bytes: Vec<u8>,
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

/// How a logical attempt settled; only an accepted attempt entered history.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Disposition {
    Accepted,
    /// A turn the provider completed at its output limit and the runtime refused.
    Refused,
    Abandoned,
}

pub(crate) struct CompletionAdmission {
    pub generation: Arc<Generation>,
    pub disposition: Disposition,
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
    compaction: Option<(String, u64, String)>,
    stream: bool,
    usage: GenerationUsageSummary,
    all_usage: GenerationUsageSummary,
}

impl RequestAdmission {
    /// The chat attempt this request belongs to, when it has one.
    pub(crate) fn attempt(&self) -> Option<&str> {
        self.attempt.as_deref()
    }
    pub(crate) fn scope(&self) -> &str {
        &self.scope
    }
    /// The exact JSON text of each message the model receives, in order.
    pub(crate) fn messages(&self) -> &[String] {
        &self.body.messages
    }
}

/// What one physical response has established so far. It is changed in place
/// by each admitted event and never copied.
#[derive(Default)]
struct ResponseState {
    scope: String,
    attempt: Option<String>,
    compaction: Option<String>,
    utility: Option<String>,
    processing: Option<bool>,
    pipeline_outputs: Option<u64>,
    sequence: u64,
    http_status: Option<u64>,
    content_type: Option<String>,
    termination: Option<String>,
    bytes: u64,
    digest: Option<Sha256>,
    decoded_utility: Vec<serde_json::Value>,
    decoded_failure: Option<serde_json::Value>,
    decoded_pending: Option<DecodedPending>,
}

// A plan reads what a response has accumulated and never copies it. Neither
// the response's state nor its value reader is `Clone`, so no plan can: this
// item compiles only while neither type implements it.
const _: fn() = || {
    trait AmbiguousIfClone<A> {
        fn some_item() {}
    }
    impl<T: ?Sized> AmbiguousIfClone<()> for T {}
    struct IsClone;
    impl<T: ?Sized + Clone> AmbiguousIfClone<IsClone> for T {}
    let _ = <ResponseState as AmbiguousIfClone<_>>::some_item;
    let _ = <ResponseValues as AmbiguousIfClone<_>>::some_item;
};

pub(crate) struct ResponseAdmission {
    request_id: String,
    sequence: u64,
    step: ResponseStep,
    outcome: Option<ResponseOutcome>,
    usage: Option<(GenerationUsageSummary, GenerationUsageSummary)>,
}

/// The one change an admitted response event makes to its request's state.
enum ResponseStep {
    Http {
        status: u64,
        content_type: Option<String>,
    },
    Body(Vec<u8>),
    End {
        termination: String,
    },
    DecodedBody {
        role: String,
        index: u64,
        bytes: Vec<u8>,
    },
    DecodedEnd {
        failure: bool,
        value: serde_json::Value,
    },
    Outcome {
        completed: bool,
        pipeline_outputs_delivered: u64,
    },
    Delivery(u64),
    History(bool),
}

impl ModelRequests {
    pub(crate) fn plan_utility_request(
        &self,
        record: Value<'_>,
        line: usize,
        limits: Limits,
    ) -> ContractResult<UtilityRequestAdmission> {
        let request = field(record, "utility_request", line)?;
        let journal_id = text(request, "journal_id", line)?;
        let sequence = unsigned(
            field(request, "sequence", line)?,
            "utility request sequence",
            SAFE_INTEGER,
        )?;
        if self.journal_id.as_deref() != Some(journal_id)
            || self
                .first_sequence
                .and_then(|first| first.checked_add(self.physical_count))
                != Some(sequence)
        {
            return Err(refusal("utility request is missing, reordered or foreign"));
        }
        let request_id = text(request, "request_id", line)?;
        if self.ids.contains(request_id) {
            return Err(refusal("utility request reuses a physical identity"));
        }
        let operation_id = text(request, "operation_id", line)?;
        if self.completed_utilities.contains(operation_id) {
            return Err(refusal("utility request follows its completed operation"));
        }
        let scope = text(request, "kv_scope", line)?;
        let kind = text(request, "kind", line)?;
        let model = text(request, "requested_model", line)?;
        if model.trim().is_empty() {
            return Err(refusal("utility request has no selected model"));
        }
        let input_count = field(request, "requested_input_count", line)?;
        let expected_max = field(request, "expected_max_model_len", line)?;
        let (input_count, expected_max_model_len) = if kind == "embedding" {
            if !expected_max.is_null() {
                return Err(refusal("embedding carries a tokenizer context limit"));
            }
            let count = unsigned(input_count, "embedding input count", SAFE_INTEGER)?;
            if count == 0 {
                return Err(refusal("embedding has no separate input"));
            }
            (Some(count), None)
        } else if matches!(kind, "tokenize_chat" | "tokenize_text") {
            if !input_count.is_null() {
                return Err(refusal(
                    "tokenizer request carries an embedding input count",
                ));
            }
            let limit = unsigned(
                expected_max,
                "selected tokenizer context limit",
                SAFE_INTEGER,
            )?;
            if limit == 0 {
                return Err(refusal("tokenizer has no selected context limit"));
            }
            (None, Some(limit))
        } else {
            return Err(refusal("utility request has an unknown kind"));
        };
        let url = text(request, "request_url", line)?;
        let path =
            utility_url_path(url).ok_or_else(|| refusal("utility request has no HTTP endpoint"))?;
        if (kind == "embedding" && !path.ends_with("/embeddings"))
            || (kind != "embedding" && !path.ends_with("/tokenize"))
        {
            return Err(refusal("utility request uses the wrong endpoint"));
        }
        let body = text(request, "body_json", line)?;
        let bytes = unsigned(
            field(request, "body_bytes", line)?,
            "utility body bytes",
            SAFE_INTEGER,
        )?;
        if u64::try_from(body.len()).ok() != Some(bytes)
            || sha256(body) != text(request, "body_sha256", line)?
        {
            return Err(refusal(
                "utility request differs from its body size or SHA-256",
            ));
        }
        let document = Document::decode(body.as_bytes(), limits)
            .map_err(|_| refusal("utility request body is not JSON"))?;
        let root = document.root();
        if text(root, "model", line)? != model {
            return Err(refusal("utility body changes the selected model"));
        }
        match kind {
            "tokenize_chat" => {
                if root.get("prompt").is_some()
                    || field(root, "messages", line)?.elements().is_none()
                    || field(root, "add_generation_prompt", line)?.as_bool() != Some(true)
                {
                    return Err(refusal("chat tokenizer has no rendered message input"));
                }
            }
            "tokenize_text" => {
                if root.get("messages").is_some()
                    || field(root, "prompt", line)?.as_str().is_none()
                    || field(root, "add_special_tokens", line)?.as_bool() != Some(false)
                {
                    return Err(refusal("text tokenizer has no rendered text input"));
                }
            }
            "embedding" => {
                if text(root, "encoding_format", line)? != "base64" {
                    return Err(refusal("embedding request has no SDK base64 encoding"));
                }
                let input = field(root, "input", line)?;
                let separate = if input.as_str().is_some() {
                    1
                } else {
                    let values = input
                        .elements()
                        .ok_or_else(|| refusal("embedding input is not separate text"))?;
                    let mut count = 0;
                    for value in values {
                        if value.as_str().is_none() {
                            return Err(refusal("embedding input contains non-text"));
                        }
                        count += 1;
                    }
                    count
                };
                if Some(separate) != input_count {
                    return Err(refusal("embedding input count differs from separate texts"));
                }
            }
            _ => unreachable!("checked utility kind"),
        }
        if let Some(previous) = self.utilities.get(operation_id) {
            if previous.journal_id != journal_id
                || previous.scope != scope
                || previous.kind != kind
                || previous.model != model
                || previous.input_count != input_count
                || previous.expected_max_model_len != expected_max_model_len
                || previous.url != url
                || previous.body != body
            {
                return Err(refusal("utility retry changes its request or owner"));
            }
        }
        Ok(UtilityRequestAdmission {
            sequence,
            journal_id: journal_id.to_string(),
            request_id: request_id.to_string(),
            operation_id: operation_id.to_string(),
            scope: scope.to_string(),
            kind: kind.to_string(),
            model: model.to_string(),
            input_count,
            expected_max_model_len,
            url: url.to_string(),
            body: body.to_string(),
        })
    }

    pub(crate) fn commit_utility_request(&mut self, admission: UtilityRequestAdmission) {
        let operation = self
            .utilities
            .entry(admission.operation_id.clone())
            .or_insert_with(|| UtilityOperation {
                first_sequence: admission.sequence,
                last_sequence: admission.sequence,
                journal_id: admission.journal_id.clone(),
                scope: admission.scope.clone(),
                kind: admission.kind.clone(),
                model: admission.model.clone(),
                input_count: admission.input_count,
                expected_max_model_len: admission.expected_max_model_len,
                url: admission.url.clone(),
                body: admission.body.clone(),
                ..UtilityOperation::default()
            });
        operation.last_sequence = admission.sequence;
        operation.requests.push(admission.request_id.clone());
        self.ids.insert(admission.request_id.clone());
        self.response_values.insert(
            admission.request_id.clone(),
            ResponseValues::new(false, false),
        );
        self.responses.insert(
            admission.request_id,
            ResponseState {
                scope: admission.scope,
                utility: Some(admission.operation_id),
                digest: Some(Sha256::default()),
                ..ResponseState::default()
            },
        );
        self.physical_count += 1;
    }

    pub(crate) fn plan_utility_completion(
        &self,
        record: Value<'_>,
        line: usize,
    ) -> ContractResult<UtilityCompletionAdmission> {
        let completion = field(record, "utility_completion", line)?;
        let operation_id = text(completion, "operation_id", line)?;
        if self.completed_utilities.contains(operation_id)
            || self.journal_id.as_deref() != Some(text(completion, "journal_id", line)?)
        {
            return Err(refusal("utility completion is repeated or foreign"));
        }
        let mut ids = Vec::new();
        let mut unique = BTreeSet::new();
        for value in field(completion, "request_ids", line)?
            .elements()
            .ok_or_else(|| refusal("utility completion has no request list"))?
        {
            let id = value
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| refusal("utility completion has an invalid request identity"))?;
            if !unique.insert(id) {
                return Err(refusal("utility completion repeats a request"));
            }
            ids.push(id.to_string());
        }
        let result = field(completion, "result", line)?;
        let error = field(completion, "error", line)?;
        if result.is_null() == error.is_null()
            || (!error.is_null() && error.as_str().is_none_or(str::is_empty))
        {
            return Err(refusal("utility completion has no unique result or error"));
        }
        let operation = self.utilities.get(operation_id);
        if operation.is_none() {
            if !ids.is_empty() || !result.is_null() {
                return Err(refusal(
                    "utility completion names unrecorded physical attempts",
                ));
            }
            return Ok(UtilityCompletionAdmission {
                operation_id: operation_id.to_string(),
                request_ids: ids,
                token_count: None,
            });
        }
        let operation = operation.expect("checked operation");
        if operation.journal_id != text(completion, "journal_id", line)?
            || operation.scope != text(completion, "kv_scope", line)?
            || operation.kind != text(completion, "kind", line)?
            || operation.model != text(completion, "requested_model", line)?
            || operation.requests != ids
        {
            return Err(refusal(
                "utility completion changes its owner or omits a physical retry",
            ));
        }
        let input = field(completion, "requested_input_count", line)?;
        let limit = field(completion, "expected_max_model_len", line)?;
        if operation.input_count
            != if input.is_null() {
                None
            } else {
                Some(unsigned(input, "embedding count", SAFE_INTEGER)?)
            }
            || operation.expected_max_model_len
                != if limit.is_null() {
                    None
                } else {
                    Some(unsigned(limit, "selected context limit", SAFE_INTEGER)?)
                }
        {
            return Err(refusal(
                "utility completion changes its selected input count or context limit",
            ));
        }
        if operation.physical.len() != ids.len()
            || ids.iter().any(|id| {
                operation
                    .physical
                    .get(id)
                    .is_none_or(|physical| !physical.delivered)
            })
            || ids[..ids.len().saturating_sub(1)]
                .iter()
                .any(|id| operation.physical[id].completed)
        {
            return Err(refusal(
                "utility completion omits a physical response or successful retry",
            ));
        }
        if !result.is_null() {
            let final_id = ids
                .last()
                .ok_or_else(|| refusal("utility result has no physical response"))?;
            let final_response = &operation.physical[final_id];
            if !final_response.completed
                || !final_response
                    .status
                    .is_some_and(|status| (200..300).contains(&status))
                || !matches!(
                    final_response.termination.as_deref(),
                    Some("eof" | "cancelled")
                )
                || !final_response
                    .content_type
                    .as_deref()
                    .and_then(|value| value.split(';').next())
                    .map(str::trim)
                    .is_some_and(|value| value == "application/json" || value.ends_with("+json"))
            {
                return Err(refusal("utility result has no complete JSON HTTP response"));
            }
            let body = match self.response_values.get(final_id) {
                Some(ResponseValues::Nonstream(body)) => body,
                _ => {
                    return Err(refusal(
                        "utility result has no final physical response bytes",
                    ))
                }
            };
            let parsed = nonstream_json_value(body)
                .ok_or_else(|| refusal("utility result has invalid physical JSON"))?;
            if operation.kind == "embedding" {
                if text(result, "kind", line)? != "embedding" {
                    return Err(refusal("embedding completion claims a tokenizer result"));
                }
                let data = parsed
                    .get("data")
                    .and_then(serde_json::Value::as_array)
                    .ok_or_else(|| refusal("physical embedding response has no data"))?;
                let vectors = field(result, "vectors", line)?
                    .elements()
                    .ok_or_else(|| refusal("embedding result has no vectors"))?
                    .collect::<Vec<_>>();
                if Some(data.len() as u64) != operation.input_count || vectors.len() != data.len() {
                    return Err(refusal(
                        "embedding result count differs from separate inputs",
                    ));
                }
                let mut ordered = data.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|entry| {
                    entry
                        .get("index")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(u64::MAX)
                });
                for (index, entry) in ordered.into_iter().enumerate() {
                    let at = entry.get("index").and_then(serde_json::Value::as_u64);
                    let encoded = entry
                        .get("embedding")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| refusal("physical embedding is not base64"))?;
                    let bytes = STANDARD
                        .decode(encoded)
                        .map_err(|_| refusal("physical embedding has invalid base64"))?;
                    let values = vectors[index]
                        .elements()
                        .ok_or_else(|| refusal("embedding result vector is not an array"))?
                        .collect::<Vec<_>>();
                    if at != Some(index as u64)
                        || bytes.is_empty()
                        || bytes.len() % 4 != 0
                        || STANDARD.encode(&bytes) != encoded
                        || values.len() != bytes.len() / 4
                    {
                        return Err(refusal("physical embedding vector differs from its result"));
                    }
                    for (position, chunk) in bytes.chunks_exact(4).enumerate() {
                        let decoded =
                            f32::from_le_bytes(chunk.try_into().expect("four-byte chunk"));
                        if !decoded.is_finite()
                            || values[position]
                                .as_number()
                                .and_then(|number| number.token().parse::<f64>().ok())
                                != Some(f64::from(decoded))
                        {
                            return Err(refusal(
                                "embedding result differs from physical float32 bytes",
                            ));
                        }
                    }
                }
            } else {
                if text(result, "kind", line)? != "token_count" {
                    return Err(refusal("tokenizer completion claims an embedding result"));
                }
                let count = parsed
                    .get("count")
                    .and_then(serde_json::Value::as_u64)
                    .filter(|value| *value <= SAFE_INTEGER)
                    .ok_or_else(|| refusal("physical tokenizer has no valid count"))?;
                let max = parsed
                    .get("max_model_len")
                    .and_then(serde_json::Value::as_u64)
                    .filter(|value| *value > 0 && *value <= SAFE_INTEGER)
                    .ok_or_else(|| refusal("physical tokenizer has no valid context limit"))?;
                if Some(max) != operation.expected_max_model_len
                    || count
                        != unsigned(
                            field(result, "total_tokens", line)?,
                            "recorded token count",
                            SAFE_INTEGER,
                        )?
                    || max
                        != unsigned(
                            field(result, "max_model_len", line)?,
                            "recorded context limit",
                            SAFE_INTEGER,
                        )?
                {
                    return Err(refusal(
                        "tokenizer result differs from physical count or selected context limit",
                    ));
                }
            }
        }
        Ok(UtilityCompletionAdmission {
            operation_id: operation_id.to_string(),
            request_ids: ids,
            token_count: if operation.kind == "tokenize_chat" && !result.is_null() {
                Some(CompletedTokenCount {
                    scope: operation.scope.clone(),
                    first_sequence: operation.first_sequence,
                    last_sequence: operation.last_sequence,
                    count: unsigned(
                        field(result, "total_tokens", line)?,
                        "token count",
                        SAFE_INTEGER,
                    )?,
                    window: unsigned(
                        field(result, "max_model_len", line)?,
                        "context limit",
                        SAFE_INTEGER,
                    )?,
                    model: operation.model.clone(),
                })
            } else {
                None
            },
        })
    }

    pub(crate) fn commit_utility_completion(&mut self, admission: UtilityCompletionAdmission) {
        self.utilities.remove(&admission.operation_id);
        for id in admission.request_ids {
            self.response_values.remove(&id);
        }
        if let Some(count) = admission.token_count {
            self.completed_token_counts
                .insert(admission.operation_id.clone(), count);
        }
        self.completed_utilities.insert(admission.operation_id);
    }

    pub(crate) fn check_token_measurement(
        &self,
        operation_id: &str,
        scope: &str,
    ) -> ContractResult<(u64, u64, String, u64, u64)> {
        let count = self.completed_token_counts.get(operation_id)
            .filter(|count| count.scope == scope && !self.claimed_token_counts.contains(operation_id))
            .ok_or_else(|| refusal("compaction measurement has no unclaimed completed chat tokenizer operation in its scope"))?;
        Ok((
            count.count,
            count.window,
            count.model.clone(),
            count.first_sequence,
            count.last_sequence,
        ))
    }

    pub(crate) fn commit_token_measurement_claims(&mut self, ids: Vec<String>) {
        for id in ids {
            self.completed_token_counts
                .remove(&id)
                .expect("planned completed token measurement");
            self.claimed_token_counts.insert(id);
        }
    }

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
                    .and_then(|first| first.checked_add(self.physical_count))
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
                (
                    None,
                    if purpose == "compaction" {
                        Some(id.to_string())
                    } else {
                        None
                    },
                )
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
                    operation.scope != scope
                        || operation.budget != budget
                        || operation.model != model
                })
            {
                return Err(refusal("compaction operation changes scope, model or ceiling, or issues another request after its claim"));
            }
            Some((id, budget, model.to_string()))
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
        self.physical_count += 1;
        if let Some((id, budget, model)) = &admission.compaction {
            let operation =
                self.compactions
                    .entry(id.clone())
                    .or_insert_with(|| CompactionOperation {
                        scope: admission.scope.clone(),
                        budget: *budget,
                        model: model.clone(),
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
                compaction: admission.compaction.map(|(id, _, _)| id),
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
        // The plan reads the request's state and names the one change this
        // event makes to it; only the commit makes the change. The state holds
        // everything the response has accumulated, so it is borrowed, never
        // copied: admitting an event costs that event, however long the
        // response it belongs to has grown.
        let state = self
            .responses
            .get(id)
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
        let mut outcome = None;
        let kind = text(event, "kind", line)?;
        if state.utility.is_some() && matches!(kind, "decoded_body" | "decoded_end" | "history") {
            return Err(refusal(
                "physical utility response contains chat output evidence",
            ));
        }
        if matches!(kind, "history" | "delivery") != state.processing.is_some()
            || (kind == "outcome" && state.digest.is_some())
            || (matches!(kind, "http" | "body" | "end") && state.digest.is_none())
            || (matches!(kind, "decoded_body" | "decoded_end")
                && (state.processing.is_some() || state.http_status.is_none()))
        {
            return Err(refusal(
                "response processing outcome must follow transport completion",
            ));
        }
        let step = match kind {
            "http" => {
                if state.http_status.is_some() || state.sequence != 0 {
                    return Err(refusal("response repeats HTTP headers"));
                }
                let status = unsigned(
                    field(event, "status", line)?,
                    "HTTP response status",
                    SAFE_INTEGER,
                )?;
                let media_type = field(event, "content_type", line)?;
                let content_type = if media_type.is_null() {
                    None
                } else {
                    Some(text(event, "content_type", line)?.to_string())
                };
                ResponseStep::Http {
                    status,
                    content_type,
                }
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
                state
                    .bytes
                    .checked_add(bytes.len() as u64)
                    .filter(|bytes| *bytes <= SAFE_INTEGER)
                    .ok_or_else(|| refusal("response exceeds exact byte accounting"))?;
                ResponseStep::Body(bytes)
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
                        .expect("an open body has its digest")
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
                ResponseStep::End {
                    termination: termination.to_string(),
                }
            }
            "decoded_body" | "decoded_end" => {
                let role = text(event, "role", line)?;
                let index = unsigned(
                    field(event, "index", line)?,
                    "decoded output index",
                    SAFE_INTEGER,
                )?;
                if (role == "utility" && state.attempt.is_some())
                    || (role == "failure" && (index != 0 || state.decoded_failure.is_some()))
                    || (role != "utility" && role != "failure")
                    || (role == "utility" && index != state.decoded_utility.len() as u64)
                {
                    return Err(refusal(
                        "decoded observation has no matching response owner or index",
                    ));
                }
                if kind == "decoded_body" {
                    let offset = unsigned(
                        field(event, "offset", line)?,
                        "decoded output offset",
                        SAFE_INTEGER,
                    )?;
                    match &state.decoded_pending {
                        None if offset != 0 => {
                            return Err(refusal("decoded observation starts after a byte gap"));
                        }
                        Some(pending)
                            if pending.role != role
                                || pending.index != index
                                || pending.bytes.len() as u64 != offset =>
                        {
                            return Err(refusal(
                                "decoded observation has an interleaved or missing byte chunk",
                            ));
                        }
                        _ => {}
                    }
                    let encoded = text(event, "base64", line)?;
                    let bytes = STANDARD
                        .decode(encoded)
                        .map_err(|_| refusal("decoded observation has invalid base64 bytes"))?;
                    if bytes.is_empty() || STANDARD.encode(&bytes) != encoded {
                        return Err(refusal("decoded observation has noncanonical base64 bytes"));
                    }
                    offset
                        .checked_add(bytes.len() as u64)
                        .filter(|size| *size <= SAFE_INTEGER)
                        .ok_or_else(|| {
                            refusal("decoded observation exceeds exact byte accounting")
                        })?;
                    ResponseStep::DecodedBody {
                        role: role.to_string(),
                        index,
                        bytes,
                    }
                } else {
                    let pending = state
                        .decoded_pending
                        .as_ref()
                        .ok_or_else(|| refusal("decoded observation end has no byte sequence"))?;
                    if pending.role != role
                        || pending.index != index
                        || pending.bytes.len() as u64
                            != unsigned(
                                field(event, "body_bytes", line)?,
                                "decoded output bytes",
                                SAFE_INTEGER,
                            )?
                        || Sha256::digest(&pending.bytes)
                            .iter()
                            .map(|byte| format!("{byte:02x}"))
                            .collect::<String>()
                            != text(event, "body_sha256", line)?
                    {
                        return Err(refusal(
                            "decoded observation end differs from its exact bytes",
                        ));
                    }
                    let decoded: serde_json::Value = serde_json::from_slice(&pending.bytes)
                        .map_err(|_| refusal("decoded observation is not valid JSON"))?;
                    if !decoded.as_object().is_some_and(|object| {
                        object.len() == 3
                            && object
                                .get("response")
                                .is_some_and(serde_json::Value::is_object)
                            && object
                                .get("incomplete_tool_calls")
                                .is_some_and(serde_json::Value::is_array)
                            && object
                                .get("tool_call_preparations")
                                .is_some_and(serde_json::Value::is_array)
                    }) {
                        return Err(refusal("decoded observation has an unknown response shape"));
                    }
                    ResponseStep::DecodedEnd {
                        failure: role == "failure",
                        value: decoded,
                    }
                }
            }
            "outcome" => {
                let completed = text(event, "status", line)? == "completed";
                if completed && state.decoded_failure.is_some() {
                    return Err(refusal(
                        "completed response contains a failed decode observation",
                    ));
                }
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
                if state.utility.is_some()
                    && (!usage.is_null()
                        || pipeline_outputs_delivered != 0
                        || (completed && sdk_values_seen != 1))
                {
                    return Err(refusal(
                        "physical utility response claims chat usage or output",
                    ));
                }
                if state.decoded_pending.is_some()
                    || state.decoded_utility.len() as u64
                        != if state.attempt.is_some() {
                            0
                        } else {
                            pipeline_outputs_delivered
                        }
                {
                    return Err(refusal(
                        "response outcome omits or misattributes decoded observations",
                    ));
                }
                if (sdk_values_seen > 0 && state.http_status.is_none())
                    || (pipeline_outputs_delivered > 0 && sdk_values_seen == 0)
                {
                    return Err(refusal("decoded progress has no observed SDK response"));
                }
                let values = self
                    .response_values
                    .get(id)
                    .ok_or_else(|| refusal("response has no physical value reader"))?;
                values.require_prefix(
                    sdk_values_seen,
                    completed,
                    state.http_status,
                    state.content_type.as_deref(),
                    state.termination.as_deref() == Some("eof"),
                )?;
                let recorded_usage = if usage.is_null() {
                    None
                } else {
                    Some(ServedUsage::read(usage, line)?)
                };
                let observed_usage = values.observed_usage(
                    sdk_values_seen,
                    state.http_status,
                    state.content_type.as_deref(),
                    state.termination.as_deref() == Some("eof"),
                );
                if recorded_usage != observed_usage {
                    return Err(refusal(
                        "served usage differs from processed physical response bytes",
                    ));
                }
                // Exact served usage is the client's only decode mode: a
                // generation response (chat or utility-owned; `state.utility`
                // marks a tokenizer or embedding call, which serves none)
                // completes only with the usage its bytes served.
                if completed && state.utility.is_none() && recorded_usage.is_none() {
                    return Err(refusal(
                        "completes a generation response that served no usage",
                    ));
                }
                if state.compaction.is_some() {
                    // The commit hands the draw the SDK values its stream
                    // decoded, up to the count the SDK reached.
                    if !matches!(values, ResponseValues::Stream(_)) {
                        return Err(refusal(
                            "compaction response has no streamed physical values",
                        ));
                    }
                    usize::try_from(sdk_values_seen).map_err(|_| {
                        refusal("compaction SDK value count exceeds addressable memory")
                    })?;
                }
                outcome = Some(ResponseOutcome {
                    scope: state.scope.clone(),
                    usage: recorded_usage,
                    completed,
                    pipeline_outputs_delivered,
                    sdk_values_seen,
                });
                ResponseStep::Outcome {
                    completed,
                    pipeline_outputs_delivered,
                }
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
                    || (state.utility.is_some() && delivered != 0)
                {
                    return Err(refusal(
                        "utility delivery has no matching physical output prefix",
                    ));
                }
                ResponseStep::Delivery(delivered)
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
                ResponseStep::History(accepted)
            }
            _ => return Err(refusal("response uses an unknown event")),
        };
        Ok(ResponseAdmission {
            request_id: id.to_string(),
            sequence,
            usage: outcome
                .as_ref()
                .filter(|_| state.utility.is_none())
                .map(|outcome| -> ContractResult<_> {
                    Ok((
                        self.scope_usage(&outcome.scope).finalize(outcome.usage)?,
                        self.all_usage.finalize(outcome.usage)?,
                    ))
                })
                .transpose()?,
            outcome,
            step,
        })
    }

    /// Apply a planned response event to its request's state in place.
    pub(crate) fn commit_response(&mut self, admission: ResponseAdmission) {
        let ResponseAdmission {
            request_id,
            sequence,
            step,
            outcome,
            usage,
        } = admission;
        let state = self
            .responses
            .get_mut(&request_id)
            .expect("planned response state");
        state.sequence = sequence;
        let ended = match step {
            ResponseStep::Http {
                status,
                content_type,
            } => {
                state.http_status = Some(status);
                state.content_type = content_type;
                false
            }
            ResponseStep::Body(bytes) => {
                state.bytes += bytes.len() as u64;
                state
                    .digest
                    .as_mut()
                    .expect("planned open body digest")
                    .update(&bytes);
                self.response_values
                    .get_mut(&request_id)
                    .expect("planned physical value reader")
                    .push(&bytes);
                false
            }
            ResponseStep::End { termination } => {
                state.digest = None;
                state.termination = Some(termination);
                false
            }
            ResponseStep::DecodedBody { role, index, bytes } => {
                match &mut state.decoded_pending {
                    Some(pending) => pending.bytes.extend_from_slice(&bytes),
                    None => state.decoded_pending = Some(DecodedPending { role, index, bytes }),
                }
                false
            }
            ResponseStep::DecodedEnd { failure, value } => {
                state.decoded_pending = None;
                if failure {
                    state.decoded_failure = Some(value);
                } else {
                    state.decoded_utility.push(value);
                }
                false
            }
            ResponseStep::Outcome {
                completed,
                pipeline_outputs_delivered,
            } => {
                let outcome = outcome.expect("an outcome step carries its outcome");
                state.processing = Some(completed);
                state.pipeline_outputs = Some(pipeline_outputs_delivered);
                if let Some(operation_id) = &state.utility {
                    self.utilities
                        .get_mut(operation_id)
                        .expect("planned utility operation")
                        .physical
                        .insert(
                            request_id.clone(),
                            UtilityPhysical {
                                status: state.http_status,
                                content_type: state.content_type.clone(),
                                termination: state.termination.clone(),
                                completed,
                                delivered: false,
                            },
                        );
                }
                if let Some(operation_id) = &state.compaction {
                    // The draw's evidence is handed to its operation once,
                    // here, and is not kept twice: the SDK values its stream
                    // decoded, as far as the SDK reached (later body values
                    // remain wire evidence), and its decoded observations.
                    let mut values = self
                        .response_values
                        .remove(&request_id)
                        .and_then(|values| {
                            values.into_stream_values(state.termination.as_deref() == Some("eof"))
                        })
                        .expect("planned streamed compaction values");
                    values.truncate(
                        usize::try_from(outcome.sdk_values_seen)
                            .expect("planned addressable SDK value count"),
                    );
                    let operation = self
                        .compactions
                        .get_mut(operation_id)
                        .expect("planned compaction operation");
                    operation
                        .outcomes
                        .insert(request_id.clone(), outcome.clone());
                    operation.values.insert(request_id.clone(), values);
                    operation.observations.insert(
                        request_id.clone(),
                        std::mem::take(&mut state.decoded_utility),
                    );
                    operation
                        .failures
                        .insert(request_id.clone(), state.decoded_failure.take());
                }
                if state.utility.is_none() || !completed {
                    self.response_values.remove(&request_id);
                }
                if let Some((scope_usage, all_usage)) = usage {
                    self.usage.insert(outcome.scope.clone(), scope_usage);
                    self.all_usage = all_usage;
                }
                if let Some(attempt) = &state.attempt {
                    self.attempts
                        .get_mut(attempt)
                        .expect("planned chat attempt")
                        .outcomes
                        .insert(request_id.clone(), outcome);
                }
                false
            }
            ResponseStep::Delivery(delivered) => {
                if let Some(operation_id) = &state.utility {
                    self.utilities
                        .get_mut(operation_id)
                        .expect("planned utility operation")
                        .physical
                        .get_mut(&request_id)
                        .expect("planned utility physical outcome")
                        .delivered = true;
                }
                if let Some(operation_id) = &state.compaction {
                    let operation = self
                        .compactions
                        .get_mut(operation_id)
                        .expect("planned compaction operation");
                    operation.open.remove(&request_id);
                    operation.deliveries.insert(request_id.clone(), delivered);
                }
                true
            }
            ResponseStep::History(accepted) => {
                self.attempts
                    .get_mut(state.attempt.as_ref().expect("planned chat history owner"))
                    .expect("planned chat attempt")
                    .settled
                    .insert(request_id.clone(), accepted);
                true
            }
        };
        if ended {
            self.responses.remove(&request_id);
        }
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
        let SeedAdmission { seed } = admission;
        let owner = self
            .attempts
            .get_mut(&seed.origin.attempt)
            .expect("planned normalization seed owner");
        owner.seed = Some(seed);
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
        let disposition = match text(completion, "disposition", line)? {
            "accepted" => Disposition::Accepted,
            "refused" => Disposition::Refused,
            "abandoned" => Disposition::Abandoned,
            _ => return Err(refusal("completion has an unknown disposition")),
        };
        let accepted = disposition == Disposition::Accepted;
        let turn = disposition != Disposition::Abandoned;
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
            || (turn && consumer_observations != final_outcome.pipeline_outputs_delivered)
        {
            return Err(refusal(
                "consumer receipt contradicts decoded output or generation observations",
            ));
        }
        match disposition {
            Disposition::Accepted => generation.require_accepted()?,
            Disposition::Refused => generation.require_refused()?,
            Disposition::Abandoned => {}
        }
        if turn && (!final_outcome.completed || final_outcome.usage != generation.usage) {
            return Err(refusal(
                "completed turn usage contradicts its physical response",
            ));
        }
        Ok(CompletionAdmission {
            generation: Arc::clone(generation),
            disposition,
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
        tokenizer_model: &str,
        physical_requests: u64,
        draw: Value<'_>,
        kind: DrawKind,
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
            || operation.observations.len() != operation.requests.len()
            || operation.failures.len() != operation.requests.len()
        {
            return Err(refusal(
                "compaction draw omits, repeats or misattributes physical requests",
            ));
        }
        if operation.model != tokenizer_model {
            return Err(refusal(
                "compaction tokenizer model differs from physical draw model",
            ));
        }
        let mut observed_values = 0_u64;
        let mut physical_values = Vec::new();
        for (index, request_id) in operation.requests.iter().enumerate() {
            let outcome = operation
                .outcomes
                .get(request_id)
                .ok_or_else(|| refusal("compaction request has no processing outcome"))?;
            if outcome
                .usage
                .is_some_and(|usage| usage.output > operation.budget)
            {
                return Err(refusal(
                    "compaction physical output exceeded its request ceiling",
                ));
            }
            let delivered = operation
                .deliveries
                .get(request_id)
                .ok_or_else(|| refusal("compaction request has no delivery receipt"))?;
            if index + 1 < operation.requests.len() && *delivered != 0 {
                return Err(refusal(
                    "compaction draw mixes output from physical retries",
                ));
            }
            if index + 1 == operation.requests.len() {
                match kind {
                    DrawKind::Answer if !outcome.completed || *delivered == 0 => {
                        return Err(refusal(
                            "compaction draw claims an answer its physical request did not complete and deliver",
                        ));
                    }
                    DrawKind::Fault if outcome.completed => {
                        return Err(refusal(
                            "compaction draw claims a transport fault its physical request did not have",
                        ));
                    }
                    _ => {}
                }
            }
            observed_values = observed_values
                .checked_add(outcome.sdk_values_seen)
                .filter(|total| *total <= SAFE_INTEGER)
                .ok_or_else(|| refusal("compaction SDK value count exceeds exact range"))?;
            let values = operation
                .values
                .get(request_id)
                .ok_or_else(|| refusal("compaction request has no captured SDK values"))?;
            if values.len() as u64 != outcome.sdk_values_seen {
                return Err(refusal(
                    "compaction processing count differs from physical SDK values",
                ));
            }
            physical_values.extend(values.iter());
        }
        let claimed = field(draw, "sdkValuesJson", 0)?
            .elements()
            .ok_or_else(|| refusal("compaction draw has no SDK value array"))?
            .map(|value| {
                let raw = value
                    .as_str()
                    .ok_or_else(|| refusal("compaction draw has a non-string SDK value"))?;
                serde_json::from_str::<serde_json::Value>(raw)
                    .map_err(|_| refusal("compaction draw has undecodable SDK value JSON"))
            })
            .collect::<ContractResult<Vec<_>>>()?;
        if observed_values != claimed.len() as u64
            || !physical_values
                .into_iter()
                .zip(&claimed)
                .all(|(left, right)| same_sdk_value(left, right))
        {
            return Err(refusal(
                "compaction SDK values differ from physical response bytes",
            ));
        }
        let last_request = operation
            .requests
            .last()
            .expect("nonempty physical operation");
        let delivered = usize::try_from(
            *operation
                .deliveries
                .get(last_request)
                .ok_or_else(|| refusal("compaction has no final delivery receipt"))?,
        )
        .map_err(|_| refusal("compaction delivered count exceeds addressable memory"))?;
        let observations = operation
            .observations
            .get(last_request)
            .ok_or_else(|| refusal("compaction has no decoded output evidence"))?;
        if delivered > observations.len() {
            return Err(refusal(
                "compaction delivery exceeds decoded output evidence",
            ));
        }
        let projection = project_decoded_draw(
            &observations[..delivered],
            operation
                .failures
                .get(last_request)
                .and_then(Option::as_ref),
        )?;
        let claimed_draw: serde_json::Value =
            serde_json::from_str(draw.raw()).map_err(|_| refusal("compaction draw is not JSON"))?;
        for key in [
            "text",
            "reasoning",
            "functionCalls",
            "incompleteToolCalls",
            "finishReason",
            "usage",
            "snapshotBytes",
        ] {
            if !projection
                .get(key)
                .zip(claimed_draw.get(key))
                .is_some_and(|(left, right)| same_sdk_value(left, right))
            {
                return Err(refusal(
                    "compaction draw differs from its recorded delivered output",
                ));
            }
        }
        Ok((
            operation.budget,
            operation.first_sequence,
            operation.last_sequence,
        ))
    }

    pub(crate) fn commit_compaction_claims(&mut self, ids: Vec<String>) {
        for id in ids {
            self.compactions
                .remove(&id)
                .expect("planned compaction draw");
            self.claimed_compactions.insert(id);
        }
    }

    pub(crate) fn validate_summary(&self, record: Value<'_>, line: usize) -> ContractResult<()> {
        if !self.compactions.is_empty() {
            return Err(refusal(
                "terminal leaves physical compaction draws unclaimed",
            ));
        }
        if !self.utilities.is_empty() {
            return Err(refusal(
                "terminal leaves physical utility operations incomplete",
            ));
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
            || count != self.physical_count
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
        json!({"request":{"journal_id":"j","sequence":sequence,"request_id":id,"owner":{"kind":"utility","operation_id":"utility-operation","purpose":"other"},"kv_scope":"owner","segment_id":"segment","prompt_id":"p","decode_policy":{"mode":"nonstream","model":"fixture-model","strict_tool_calling":true,"named_tool_choice":null,"exact_token_counting":true,"tagged_thinking_tags":false},"body_bytes":json.len(),"body_sha256":sha256(json),"body":body}}).to_string()
    }
    fn admit(state: &mut ModelRequests, json: &str) -> ContractResult<()> {
        let document = Document::decode(json.as_bytes(), LIMITS).unwrap();
        let admission = state.plan(document.root(), 1, LIMITS)?;
        state.commit(admission);
        Ok(())
    }
    /// A compaction draw states its ceiling in the directive that closes its
    /// request; a redraw adds the refusal notice after that directive. Both
    /// are read, and every other trailing shape is refused.
    #[test]
    fn a_redraw_ceiling_is_read_from_the_directive_its_notice_follows() {
        let directive = json!({"role":"user","content":[{"type":"text","text":
            "system\n\nyour answer may generate at most 52102 tokens, reasoning included. If all 4 are refused, this conversation is not compacted and cannot continue."}]});
        let notice = json!({"role":"user","content":[{"type":"text","text":
            "Your previous answer to this request was refused because its snapshot renders to 38102 bytes, past the 32768-byte limit. Write the same state more briefly."}]});
        let other = json!({"role":"user","content":"more"});
        let budget = |messages: serde_json::Value, max_tokens: u64| {
            let json = json!({"max_tokens":max_tokens,"messages":messages}).to_string();
            let document = Document::decode(json.as_bytes(), LIMITS).unwrap();
            compaction_request_budget(document.root(), 1)
        };
        assert_eq!(budget(json!([directive]), 52102).unwrap(), 52102);
        assert_eq!(budget(json!([other, directive, notice]), 52102).unwrap(), 52102);
        for (messages, max_tokens, refused) in [
            (json!([directive, other]), 52102, "no declared output ceiling"),
            (json!([directive, notice, notice]), 52102, "no declared output ceiling"),
            (json!([notice]), 52102, "notice does not follow a user directive"),
            (json!([directive, notice]), 52101, "differs from its physical output ceiling"),
            (json!([]), 52102, "no final directive"),
        ] {
            let error = budget(messages, max_tokens).unwrap_err().to_string();
            assert!(error.contains(refused), "{error}");
        }
    }

    /// A draw that reasoned long is many response events against one
    /// request. Each is admitted against the state in place: every SDK value
    /// and decoded observation reaches the draw's operation once, in order,
    /// and the response keeps no second copy of them; a refused event
    /// changes nothing.
    #[test]
    fn a_long_compaction_draw_hands_its_operation_each_value_once() {
        const VALUES: usize = 4_000;
        let mut state = ModelRequests::default();
        state.commit_origin(RequestOrigin {
            journal_id: "j".into(),
            first_sequence: 1,
        });
        let body = json!({"kv_scope":"owner","model":"fixture-model","stream":true,"max_tokens":8,
            "messages":[{"role":"user","content":
                "your answer may generate at most 8 tokens, reasoning included. If all draws are refused, this conversation cannot continue."}]})
        .to_string();
        let request = json!({"request":{"journal_id":"j","sequence":1,"request_id":"draw",
            "owner":{"kind":"utility","operation_id":"compaction","purpose":"compaction"},
            "kv_scope":"owner","segment_id":"segment","prompt_id":"p",
            "decode_policy":{"mode":"stream","model":"fixture-model","strict_tool_calling":true,
                "named_tool_choice":null,"exact_token_counting":true,"tagged_thinking_tags":false},
            "body_bytes":body.len(),"body_sha256":sha256(&body),"body":{"kind":"full","json":body}}})
        .to_string();
        admit(&mut state, &request).unwrap();

        let usage = json!({"prompt_tokens":24,"completion_tokens":4,"total_tokens":28,
            "prompt_tokens_details":{"cached_tokens":0},
            "completion_tokens_details":{"reasoning_tokens":0}});
        let values: Vec<serde_json::Value> = (0..VALUES)
            .map(|index| {
                let mut value = json!({"choices":[{"index":0,"delta":{"reasoning_content":format!("step {index}")}}]});
                if index + 1 == VALUES {
                    value["usage"] = usage.clone();
                }
                value
            })
            .collect();
        let decoded = json!({"response":{"candidates":[]},"incomplete_tool_calls":[],
            "tool_call_preparations":[]})
        .to_string();
        let mut sequence = 0u64;
        // Each admitted event takes the next sequence; a refused one takes none.
        let mut event = |state: &mut ModelRequests, event: serde_json::Value| {
            let response = json!({"response":{"journal_id":"j","request_id":"draw",
                "sequence":sequence + 1,"event":event}})
            .to_string();
            let document = Document::decode(response.as_bytes(), LIMITS).unwrap();
            let admission = state.plan_response(document.root(), 1)?;
            state.commit_response(admission);
            sequence += 1;
            ContractResult::Ok(())
        };
        event(
            &mut state,
            json!({"kind":"http","status":200,"content_type":"text/event-stream"}),
        )
        .unwrap();
        let mut body_bytes = Vec::new();
        for value in &values {
            let chunk = format!("data: {value}\n\n");
            event(
                &mut state,
                json!({"kind":"body","offset":body_bytes.len(),
                "base64":STANDARD.encode(chunk.as_bytes())}),
            )
            .unwrap();
            body_bytes.extend_from_slice(chunk.as_bytes());
        }
        event(
            &mut state,
            json!({"kind":"end","termination":"eof","body_bytes":body_bytes.len(),
            "body_sha256":sha256(std::str::from_utf8(&body_bytes).unwrap()),"error":null}),
        )
        .unwrap();
        let (head, tail) = decoded.as_bytes().split_at(decoded.len() / 2);
        for index in 0..VALUES {
            event(
                &mut state,
                json!({"kind":"decoded_body","role":"utility","index":index,
                "offset":0,"base64":STANDARD.encode(head)}),
            )
            .unwrap();
            if index == VALUES / 2 {
                // A chunk past a byte gap is refused and leaves the
                // observation as it was.
                let error = event(
                    &mut state,
                    json!({"kind":"decoded_body","role":"utility",
                    "index":index,"offset":head.len() + 1,"base64":STANDARD.encode(tail)}),
                )
                .unwrap_err();
                assert!(
                    error
                        .to_string()
                        .contains("interleaved or missing byte chunk"),
                    "{error}"
                );
                let pending = state.responses["draw"].decoded_pending.as_ref().unwrap();
                assert_eq!(pending.bytes, head);
            }
            event(
                &mut state,
                json!({"kind":"decoded_body","role":"utility","index":index,
                "offset":head.len(),"base64":STANDARD.encode(tail)}),
            )
            .unwrap();
            event(
                &mut state,
                json!({"kind":"decoded_end","role":"utility","index":index,
                "body_bytes":decoded.len(),"body_sha256":sha256(&decoded)}),
            )
            .unwrap();
        }
        assert_eq!(state.responses["draw"].decoded_utility.len(), VALUES);
        event(
            &mut state,
            json!({"kind":"outcome","status":"completed","error":null,
            "sdk_values_seen":VALUES,"pipeline_outputs_delivered":VALUES,
            "served_usage":{"promptTokenCount":24,"candidatesTokenCount":4,
                "thoughtsTokenCount":0,"cachedContentTokenCount":0,"totalTokenCount":28}}),
        )
        .unwrap();
        let operation = &state.compactions["compaction"];
        assert_eq!(operation.values["draw"], values);
        assert_eq!(operation.observations["draw"].len(), VALUES);
        assert!(operation.observations["draw"]
            .iter()
            .all(|observation| observation.to_string() == decoded));
        assert_eq!(operation.failures["draw"], None);
        // Handed over, not copied: the response holds neither any longer.
        assert!(state.responses["draw"].decoded_utility.is_empty());
        assert!(!state.response_values.contains_key("draw"));
        event(
            &mut state,
            json!({"kind":"delivery","outputs_delivered":VALUES}),
        )
        .unwrap();
        assert!(state.responses.is_empty());
        assert_eq!(
            state.compactions["compaction"].deliveries["draw"],
            VALUES as u64
        );
    }

    #[test]
    fn utility_count_requires_its_physical_bytes_and_operation_completion() {
        let mut state = ModelRequests::default();
        state.commit_origin(RequestOrigin {
            journal_id: "j".into(),
            first_sequence: 1,
        });
        let body =
            json!({"model":"selected","prompt":"text","add_special_tokens":false}).to_string();
        let request = json!({"utility_request":{
            "journal_id":"j","sequence":1,"request_id":"physical-1",
            "operation_id":"count-1","kv_scope":"root","kind":"tokenize_text",
            "requested_model":"selected","requested_input_count":null,
            "expected_max_model_len":4096,"request_url":"https://fixture.invalid/tokenize",
            "body_json":body,"body_bytes":body.len(),"body_sha256":sha256(&body)
        }})
        .to_string();
        let document = Document::decode(request.as_bytes(), LIMITS).unwrap();
        let admission = state
            .plan_utility_request(document.root(), 1, LIMITS)
            .unwrap();
        state.commit_utility_request(admission);
        let raw = json!({"count":2,"max_model_len":4096}).to_string();
        for (position, event) in [
            json!({"kind":"http","status":200,"content_type":"application/json"}),
            json!({"kind":"body","offset":0,"base64":STANDARD.encode(raw.as_bytes())}),
            json!({"kind":"end","termination":"eof","body_bytes":raw.len(),"body_sha256":sha256(&raw),"error":null}),
            json!({"kind":"outcome","status":"completed","error":null,"served_usage":null,"sdk_values_seen":1,"pipeline_outputs_delivered":0}),
            json!({"kind":"delivery","outputs_delivered":0}),
        ].into_iter().enumerate() {
            let response = json!({"response":{"journal_id":"j","request_id":"physical-1","sequence":position+1,"event":event}}).to_string();
            let document = Document::decode(response.as_bytes(), LIMITS).unwrap();
            let admission = state.plan_response(document.root(), 1).unwrap();
            state.commit_response(admission);
        }
        let terminal = br#"{"request_evidence":{"journal_id":"j","first_sequence":1,"request_count":1,"open_response_ids":[],"open_attempt_ids":[]},"usage":{"requests":0,"usageReports":0,"unfinalizedRequests":0,"unreportedUsageRequests":0,"usage":null}}"#;
        let document = Document::decode(terminal, LIMITS).unwrap();
        assert!(state.validate_summary(document.root(), 1).is_err());
        let completion = json!({"utility_completion":{
            "journal_id":"j","operation_id":"count-1","kv_scope":"root",
            "kind":"tokenize_text","requested_model":"selected",
            "requested_input_count":null,"expected_max_model_len":4096,
            "request_ids":["physical-1"],
            "result":{"kind":"token_count","total_tokens":2,"max_model_len":4096},
            "error":null
        }})
        .to_string();
        let document = Document::decode(completion.as_bytes(), LIMITS).unwrap();
        let admission = state.plan_utility_completion(document.root(), 1).unwrap();
        state.commit_utility_completion(admission);
        let terminal = Document::decode(terminal, LIMITS).unwrap();
        state.validate_summary(terminal.root(), 1).unwrap();
    }
    #[test]
    fn physical_value_reader_follows_sdk_sse_boundaries_and_errors() {
        let mut stream = ResponseValues::new(true, true);
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
        // Nothing after `[DONE]` is a value the SDK yields.
        assert_eq!(
            stream.into_stream_values(true).unwrap(),
            vec![
                json!({"id": 1}),
                json!({"event": "thread.message", "data": {"error": {"message": "ordinary only"}}}),
            ]
        );

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
    fn physical_usage_comes_from_the_sdk_values_processed_before_failure() {
        let first = json!({"usage":{"prompt_tokens":5,"completion_tokens":3,
            "total_tokens":8,"prompt_tokens_details":{"cached_tokens":1},
            "completion_tokens_details":{"reasoning_tokens":2}}});
        let unread = json!({"usage":{"prompt_tokens":9,"completion_tokens":4,
            "total_tokens":13,"prompt_tokens_details":{"cached_tokens":0},
            "completion_tokens_details":{"reasoning_tokens":1}}});
        let mut response = ResponseValues::new(true, false);
        response.push(format!("data: {first}\n\ndata: {unread}\n\n").as_bytes());
        response
            .require_prefix(1, false, Some(200), Some("text/event-stream"), true)
            .unwrap();
        assert_eq!(
            response.observed_usage(1, Some(200), Some("text/event-stream"), true),
            Some(ServedUsage {
                prompt: 5,
                output: 3,
                total: 8,
                cached: 1,
                thoughts: 2
            })
        );
        assert_eq!(
            response.observed_usage(2, Some(200), Some("text/event-stream"), true),
            Some(ServedUsage {
                prompt: 9,
                output: 4,
                total: 13,
                cached: 0,
                thoughts: 1
            })
        );
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
            serde_json::from_str(include_str!("../../test-vectors/ordinary-tool-wire.json")).unwrap();
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
            serde_json::from_str(include_str!("../../test-vectors/ordinary-tool-wire.json")).unwrap();
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
        for (name, first_source, earlier_outputs, extra_final_output, refusal) in [
            ("final response", false, 0, false, None),
            ("earlier response", true, 3, false, Some("final physical request")),
            ("unclaimed earlier output", false, 1, false, Some("final physical request")),
            // The final response delivered an output no generation observation
            // claims; the consumer receipt cannot match both.
            (
                "unclaimed final output",
                false,
                0,
                true,
                Some("consumer receipt contradicts decoded output"),
            ),
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
                    seed: None,
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
            match refusal {
                None => assert!(result.is_ok(), "{name}"),
                Some(cause) => {
                    let error = result.err().expect("forged retry was admitted");
                    assert!(error.to_string().contains(cause), "{name}: {error}");
                }
            }
        }
    }
    /// A nonstream provider body that serves its usage, and that usage as
    /// the client records it.
    fn usage_body() -> (String, serde_json::Value) {
        (
            json!({"id":"served","object":"chat.completion","created":1,"model":"fixture-model",
                "choices":[{"index":0,"message":{"role":"assistant","content":""},"finish_reason":"stop"}],
                "usage":{"prompt_tokens":5,"completion_tokens":3,"total_tokens":8,
                    "prompt_tokens_details":{"cached_tokens":1},
                    "completion_tokens_details":{"reasoning_tokens":2}}})
            .to_string(),
            json!({"promptTokenCount":5,"candidatesTokenCount":3,"totalTokenCount":8,
                "cachedContentTokenCount":1,"thoughtsTokenCount":2}),
        )
    }
    #[test]
    fn processing_completion_requires_successful_http_and_compatible_transport() {
        // How a completed outcome over each transport is decided: admitted,
        // refused for its transport, or refused because the generation served
        // no usage (No Content carries no body, so it can serve none).
        #[derive(Clone, Copy, PartialEq)]
        enum Completion {
            Admitted,
            Transport,
            Unserved,
        }
        let (served_body, served_usage) = usage_body();
        for (http_status, termination, completion) in [
            (None, "not_dispatched", Completion::Transport),
            (None, "failed", Completion::Transport),
            (None, "cancelled", Completion::Transport),
            (Some(200), "eof", Completion::Admitted),
            (Some(204), "eof", Completion::Unserved),
            (Some(299), "cancelled", Completion::Admitted),
            (Some(300), "eof", Completion::Transport),
            (Some(503), "cancelled", Completion::Transport),
            (Some(200), "failed", Completion::Transport),
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
                // A successful response other than No Content serves its usage.
                let bytes = if http_status.is_some_and(|code| (200..300).contains(&code) && code != 204) {
                    served_body.as_str()
                } else {
                    ""
                };
                let mut events = Vec::new();
                if let Some(http_status) = http_status {
                    events.push(json!({
                        "kind": "http", "status": http_status,
                        "content_type": if bytes.is_empty() { json!(null) } else { json!("application/json") }
                    }));
                }
                if !bytes.is_empty() {
                    events.push(json!({"kind":"body","offset":0,"base64":STANDARD.encode(bytes.as_bytes())}));
                }
                events.push(json!({
                    "kind": "end", "termination": termination,
                    "body_bytes": bytes.len(), "body_sha256": sha256(bytes),
                    "error": if termination == "failed" { json!("read failed") } else { json!(null) }
                }));
                let completed = status == "completed";
                events.push(json!({
                    "kind": "outcome",
                    "served_usage": if completed && !bytes.is_empty() { served_usage.clone() } else { json!(null) },
                    "status": status,
                    "sdk_values_seen": if completed { 1 } else { 0 }, "pipeline_outputs_delivered": 0,
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
                    let refused = match completion {
                        _ if event["kind"] != "outcome" || !completed => None,
                        Completion::Admitted => None,
                        Completion::Transport => Some("successful HTTP transport"),
                        Completion::Unserved => Some("completes a generation response that served no usage"),
                    };
                    if let Some(cause) = refused {
                        let error = admission
                            .err()
                            .expect("contradictory completion was admitted");
                        assert!(error.to_string().contains(cause), "{http_status:?}/{termination}: {error}");
                        assert_eq!(state.responses.len(), 1);
                    } else {
                        state.commit_response(admission.unwrap());
                    }
                }
                assert!(!state.responses.is_empty(), "outcome is not delivery");
                if !completed || completion == Completion::Admitted {
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
    fn completed_generation_response_requires_served_usage() {
        let owners = [
            json!({"kind":"chat","attempt_id":"attempt"}),
            json!({"kind":"utility","operation_id":"generation","purpose":"other"}),
        ];
        // A 2xx, fully read body that serves no usage, in each decode mode.
        let unserved_stream = "data: {\"id\":\"reply\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"fixture-model\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
        let unserved_json = r#"{"id":"reply","object":"chat.completion","created":1,"model":"fixture-model","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}]}"#;
        for owner in &owners {
            for (stream, bytes, content_type) in [
                (true, unserved_stream, "text/event-stream"),
                (false, unserved_json, "application/json"),
            ] {
                for status in ["completed", "failed"] {
                    let mut state = ModelRequests::default();
                    let body = json!({"kv_scope":"owner","model":"fixture-model","stream":stream,"messages":[]}).to_string();
                    let mut record: serde_json::Value = serde_json::from_str(&request(
                        1,
                        "r",
                        json!({"kind":"full","json":body}),
                        &body,
                    ))
                    .unwrap();
                    record["request"]["owner"] = owner.clone();
                    record["request"]["decode_policy"]["mode"] =
                        json!(if stream { "stream" } else { "nonstream" });
                    admit(&mut state, &record.to_string()).unwrap();
                    let events = [
                        json!({"kind":"http","status":200,"content_type":content_type}),
                        json!({"kind":"body","offset":0,"base64":STANDARD.encode(bytes.as_bytes())}),
                        json!({"kind":"end","termination":"eof","body_bytes":bytes.len(),"body_sha256":sha256(bytes),"error":null}),
                    ];
                    for (index, event) in events.into_iter().enumerate() {
                        let raw = json!({"response":{"journal_id":"j","request_id":"r","sequence":index+1,"event":event}}).to_string();
                        let doc = Document::decode(raw.as_bytes(), LIMITS).unwrap();
                        let plan = state.plan_response(doc.root(), 1).unwrap();
                        state.commit_response(plan);
                    }
                    let outcome = json!({"response":{"journal_id":"j","request_id":"r","sequence":4,
                        "event":{"kind":"outcome","status":status,"served_usage":null,
                            "error":if status == "failed" { json!("processing failed") } else { json!(null) },
                            "sdk_values_seen":1,"pipeline_outputs_delivered":0}}})
                    .to_string();
                    let doc = Document::decode(outcome.as_bytes(), LIMITS).unwrap();
                    let plan = state.plan_response(doc.root(), 1);
                    let label = format!("{owner}/stream={stream}/{status}");
                    if status == "completed" {
                        let error = plan.err().expect(&label).to_string();
                        assert!(
                            error.contains("completes a generation response that served no usage"),
                            "{label}: {error}"
                        );
                    } else {
                        assert!(plan.is_ok(), "{label}: {:?}", plan.err());
                    }
                }
            }
        }
        // A tokenizer call is not a generation: it completes without usage.
        let mut state = ModelRequests::default();
        state.commit_origin(RequestOrigin {
            journal_id: "j".into(),
            first_sequence: 1,
        });
        let body = json!({"model":"selected","prompt":"text","add_special_tokens":false}).to_string();
        let request = json!({"utility_request":{
            "journal_id":"j","sequence":1,"request_id":"physical-1",
            "operation_id":"count-1","kv_scope":"root","kind":"tokenize_text",
            "requested_model":"selected","requested_input_count":null,
            "expected_max_model_len":4096,"request_url":"https://fixture.invalid/tokenize",
            "body_json":body,"body_bytes":body.len(),"body_sha256":sha256(&body)
        }})
        .to_string();
        let document = Document::decode(request.as_bytes(), LIMITS).unwrap();
        let admission = state.plan_utility_request(document.root(), 1, LIMITS).unwrap();
        state.commit_utility_request(admission);
        let raw = json!({"count":2,"max_model_len":4096}).to_string();
        for (position, event) in [
            json!({"kind":"http","status":200,"content_type":"application/json"}),
            json!({"kind":"body","offset":0,"base64":STANDARD.encode(raw.as_bytes())}),
            json!({"kind":"end","termination":"eof","body_bytes":raw.len(),"body_sha256":sha256(&raw),"error":null}),
            json!({"kind":"outcome","status":"completed","error":null,"served_usage":null,"sdk_values_seen":1,"pipeline_outputs_delivered":0}),
        ].into_iter().enumerate() {
            let response = json!({"response":{"journal_id":"j","request_id":"physical-1","sequence":position+1,"event":event}}).to_string();
            let document = Document::decode(response.as_bytes(), LIMITS).unwrap();
            let admission = state.plan_response(document.root(), 1).unwrap();
            state.commit_response(admission);
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
                let physical = json!({"id":"usage-test","object":"chat.completion",
                    "created":1,"model":"fixture-model",
                    "choices":[{"index":0,"message":{"role":"assistant","content":""},
                        "finish_reason":"stop"}],
                    "usage":{"prompt_tokens":5,"completion_tokens":3,
                    "total_tokens":8,"prompt_tokens_details":{"cached_tokens":1},
                    "completion_tokens_details":{"reasoning_tokens":2}}})
                .to_string();
                let mut events = vec![
                    json!({"kind":"http","status":200,"content_type":"application/json"}),
                    json!({"kind":"body","offset":0,"base64":STANDARD.encode(physical.as_bytes())}),
                    json!({"kind":"end","termination":"eof","body_bytes":physical.len(),"body_sha256":sha256(&physical),"error":null}),
                ];
                if status == "completed" {
                    // A delivered utility output is recorded as its decoded observation.
                    let decoded = json!({"response":{"candidates":[]},
                        "incomplete_tool_calls":[],"tool_call_preparations":[]})
                    .to_string();
                    events.push(json!({"kind":"decoded_body","role":"utility","index":0,
                        "offset":0,"base64":STANDARD.encode(decoded.as_bytes())}));
                    events.push(json!({"kind":"decoded_end","role":"utility","index":0,
                        "body_bytes":decoded.len(),"body_sha256":sha256(&decoded)}));
                }
                let outcome_sequence = events.len() + 1;
                for (index, event) in events.into_iter().enumerate() {
                    let raw = json!({"response":{"journal_id":"j","request_id":"r","sequence":index+1,"event":event}}).to_string();
                    let doc = Document::decode(raw.as_bytes(), LIMITS).unwrap();
                    let plan = state.plan_response(doc.root(), 1).unwrap();
                    state.commit_response(plan);
                }
                let mut event = json!({"kind":"outcome","status":status,
                    "sdk_values_seen":1,"pipeline_outputs_delivered":if status == "completed" { 1 } else { 0 },
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
                let raw = json!({"response":{"journal_id":"j","request_id":"r","sequence":outcome_sequence,"event":event}}).to_string();
                let doc = Document::decode(raw.as_bytes(), LIMITS).unwrap();
                let plan = state.plan_response(doc.root(), 1);
                assert_eq!(plan.is_ok(), defect == "valid", "{status}/{defect}: {:?}", plan.err());
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
        // Both requests complete generations, so both serve their usage.
        let (served_body, served_usage) = usage_body();
        let terminal = Document::decode(
            br#"{"request_evidence":{"journal_id":"j","first_sequence":3,"request_count":2,"open_response_ids":[],"open_attempt_ids":[]},"usage":{"requests":2,"usageReports":2,"unfinalizedRequests":0,"unreportedUsageRequests":0,"usage":{"promptTokenCount":10,"candidatesTokenCount":6,"totalTokenCount":16,"cachedContentTokenCount":2,"thoughtsTokenCount":4}}}"#,
            LIMITS,
        )
        .unwrap();
        assert!(state.validate_summary(terminal.root(), 3).is_err());
        for id in ["a", "b"] {
            for (sequence,event) in [
                json!({"kind":"http","status":200,"content_type":"application/json"}),
                json!({"kind":"body","offset":0,"base64":STANDARD.encode(served_body.as_bytes())}),
                json!({"kind":"end","termination":"eof","body_bytes":served_body.len(),"body_sha256":sha256(&served_body),"error":null}),
                json!({"kind":"outcome","served_usage":served_usage,"status":"completed","error":null,"sdk_values_seen":1,"pipeline_outputs_delivered":0}),
                json!({"kind":"delivery","outputs_delivered":0}),
            ].into_iter().enumerate() {
                if id == "b" && event["kind"] == "outcome" {
                    assert!(state.validate_summary(terminal.root(), 3).is_err());
                    let late_body = json!({"response":{"journal_id":"j","request_id":id,"sequence":sequence+1,"event":{"kind":"body","offset":served_body.len(),"base64":"e30="}}}).to_string();
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
            serde_json::from_str(include_str!("../../test-vectors/ordinary-tool-wire.json")).unwrap();
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
                // Presentation derives from the completion; it is not request evidence.
                "stream_event" | "assistant" => {}
                kind => panic!("unhandled fixture record {kind}"),
            }
        }
        assert_eq!(state.all_usage().requests, 1);
        assert_eq!(state.all_usage().usage_reports, 1);
        assert_eq!(state.all_usage().usage.unwrap().output, 7);
    }
    #[test]
    fn chat_processing_requires_one_history_decision() {
        for status in ["completed", "failed"] {
            let mut state = ModelRequests::default();
            let body =
                r#"{"kv_scope":"owner","model":"fixture-model","stream":false,"messages":[]}"#;
            let mut value: serde_json::Value =
                serde_json::from_str(&request(1, "r", json!({"kind":"full","json":body}), body))
                    .unwrap();
            value["request"]["owner"] = json!({"kind":"chat","attempt_id":"attempt"});
            admit(&mut state, &value.to_string()).unwrap();
            // A completed chat response serves its usage; a failed one here
            // delivered a body without any.
            let completed = status == "completed";
            let (served_body, served_usage) = usage_body();
            let bytes = if completed { served_body } else { "{}".to_string() };
            for (index, event) in [
                json!({"kind":"http","status":200,"content_type":"application/json"}),
                json!({"kind":"body","offset":0,"base64":STANDARD.encode(bytes.as_bytes())}),
                json!({"kind":"end","termination":"eof","body_bytes":bytes.len(),"body_sha256":sha256(&bytes),"error":null}),
                json!({"kind":"outcome","served_usage":if completed { served_usage } else { json!(null) },"status":status,"error":if status == "failed" {json!("failure")} else {json!(null)},"sdk_values_seen":1,"pipeline_outputs_delivered":0}),
            ].into_iter().enumerate() {
                let raw = json!({"response":{"journal_id":"j","request_id":"r","sequence":index+1,"event":event}}).to_string();
                let doc = Document::decode(raw.as_bytes(), LIMITS).unwrap();
                let plan = state.plan_response(doc.root(), 1).unwrap();
                state.commit_response(plan);
            }
            let summary = if completed {
                r#"{"requests":1,"usageReports":1,"unfinalizedRequests":0,"unreportedUsageRequests":0,"usage":{"promptTokenCount":5,"candidatesTokenCount":3,"totalTokenCount":8,"cachedContentTokenCount":1,"thoughtsTokenCount":2}}"#
            } else {
                r#"{"requests":1,"usageReports":0,"unfinalizedRequests":0,"unreportedUsageRequests":1,"usage":null}"#
            };
            let terminal = format!(r#"{{"request_evidence":{{"journal_id":"j","first_sequence":1,"request_count":1,"open_response_ids":[],"open_attempt_ids":[]}},"usage":{summary}}}"#);
            let terminal = Document::decode(terminal.as_bytes(), LIMITS).unwrap();
            assert!(state.validate_summary(terminal.root(), 1).is_err());
            let accepted = json!({"response":{"journal_id":"j","request_id":"r","sequence":5,"event":{"kind":"history","disposition":"accepted"}}}).to_string();
            let doc = Document::decode(accepted.as_bytes(), LIMITS).unwrap();
            // History acceptance follows the attempt's generation and a
            // completed response; neither alone is a turn.
            assert!(state
                .plan_response(doc.root(), 1)
                .err()
                .expect("acceptance before generation")
                .to_string()
                .contains("acceptance precedes generation"));
            let rows: Vec<serde_json::Value> =
                serde_json::from_str(include_str!("../../test-vectors/ordinary-tool-wire.json")).unwrap();
            let generation = rows
                .iter()
                .find(|row| row["type"] == "model_generation")
                .unwrap()
                .to_string();
            let generation = Document::decode(generation.as_bytes(), LIMITS).unwrap();
            let generation = Generation::read(
                field(generation.root(), "generation", 1).unwrap(),
                1,
                LIMITS,
                ValidationLimits {
                    operations: 1_000_000,
                },
                |_| Ok(vec!["provider".to_string()]),
            )
            .unwrap();
            state.attempts.get_mut("attempt").unwrap().generation = Some(Arc::new(generation));
            if status == "failed" {
                assert!(state
                    .plan_response(doc.root(), 1)
                    .err()
                    .expect("acceptance of a failed response")
                    .to_string()
                    .contains("no completed response"));
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
        }
    }
}
