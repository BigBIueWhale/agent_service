//! One pure owner for stream identity, semantic admission and independent
//! observations. Physical capture, storage and process receipts remain external.
use crate::{
    generation::{Generation, OutputScope},
    json::{Document, Limits, Value},
    schema::ValidationLimits,
    stream::{field, text, unsigned},
    usage::{GenerationUsageSummary, ServedUsage},
    ContractError, ContractResult, DecodedRecord, EventKind, PartialStreamState, SystemKind,
    SAFE_INTEGER, STREAM_CONTRACT_SHA256,
};
use serde::{Deserialize, Serialize};
use base64::{engine::general_purpose::STANDARD, Engine};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}
#[derive(Debug, Clone, Serialize)]
pub struct AgentResult {
    pub is_error: bool,
    /// The terminal record's own name for how the run ended, verbatim.
    /// `success` is the agent's assertion that the model wrote its final
    /// message to the end; each `error_*` spelling names a distinct way the
    /// run stopped instead. The set is closed and checked below, so a reader
    /// can branch on it without parsing English out of `response`.
    pub subtype: String,
    pub response: String,
    pub duration_ms: u64,
    /// Wall time the agent spent inside model API calls, as the terminal
    /// result reports it. Already required and type-checked here; carrying it
    /// is what separates a run that stalled on the backend from one that
    /// churned in local tool execution, which `duration_ms` alone cannot.
    pub api_duration_ms: u64,
    pub num_turns: u64,
    pub main_kv_scope: String,
    pub usage: GenerationUsageSummary,
    pub request_scopes: Vec<RequestScope>,
    /// Every subagent scope the stream resolved, in order of first
    /// appearance. Empty exactly when the run delegated nothing.
    pub scopes: Vec<AgentScope>,
}

// Public terminal vocabulary is generated from the same schema used by the producer.
pub use crate::{terminal_exit_code, ERROR_SUBTYPES, SUCCESS_SUBTYPE};

/// One resolved subagent scope. Identification follows the Claude Code CLI
/// convention: a scope is the id of the `tool_use` content block that spawned
/// it, so consumers never need to know which tool performs delegation — and
/// this parser never assumes one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentScope {
    /// Id of the spawning `tool_use` block; the exact value every event in
    /// the scope carried in `parent_tool_use_id`.
    pub tool_use_id: String,
    /// Name of that spawning tool call. Recorded as evidence for the reader,
    /// never used as a correlation key: resolution is by id alone.
    pub tool_name: String,
    /// What the scope's own terminal record reported, verbatim. All four are
    /// `None` exactly when the scope never emitted a terminal record (the
    /// subagent was still running, or was torn down, when the session ended);
    /// `error_message` is additionally `None` when the record carried no
    /// error. Reported started turns and physical requests
    /// are separate populations; neither replaces the other.
    #[serde(deserialize_with = "required_nullable")]
    pub reported_num_turns: Option<u64>,
    #[serde(deserialize_with = "required_nullable")]
    pub is_error: Option<bool>,
    #[serde(deserialize_with = "required_nullable")]
    pub subtype: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestScope {
    pub kv_scope: String,
    pub usage: GenerationUsageSummary,
}

/// A compaction record distinguishes preflight refusal (output: null) from
/// an attempted request whose partial text is retained even without served usage.
/// Served counts, when present, obey the same five-field contract as the CLI.
///
/// A transition may draw more than one candidate when the model's own output
/// fails the acceptance rule. `rejectedAttempts` holds the ones refused before
/// the candidate `output` describes, oldest first, and is present whatever
/// `output` is: a draw that was refused and billed must not disappear behind a
/// later draw that never generated. Each carries the accounting `output`
/// carries, minus the transition-wide budget, plus the rule it failed.
fn read_token_measurement(
    value: Value<'_>,
    line: usize,
    limits: Limits,
) -> ContractResult<(u64, u64)> {
    let refuse = |detail: &str| ContractError::InvalidRecord(format!(
        "events.jsonl line {line} has invalid physical tokenizer evidence: {detail}; inspect the complete compaction record and served /tokenize response"
    ));
    let request_url = text(value, "requestUrl", line)?;
    if !(request_url.starts_with("http://") || request_url.starts_with("https://"))
        || !request_url.ends_with("/tokenize")
    {
        return Err(refuse("request URL is not a served /tokenize endpoint"));
    }
    let request_json = text(value, "requestJson", line)?;
    let request_doc = Document::decode(request_json.as_bytes(), limits)
        .map_err(|cause| refuse(&format!("request JSON: {cause:?}")))?;
    let request = request_doc.root();
    if text(request, "model", line)?.is_empty()
        || field(request, "messages", line)?.elements().is_none()
        || field(request, "add_generation_prompt", line)?.as_bool() != Some(true)
    {
        return Err(refuse("request is not a rendered chat-tokenizer request"));
    }
    let status = unsigned(field(value, "responseStatus", line)?, "tokenizer HTTP status", 599)?;
    let media_type = text(value, "responseContentType", line)?
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if !(200..300).contains(&status)
        || !(media_type == "application/json" || media_type.ends_with("+json"))
    {
        return Err(refuse("response is not successful JSON"));
    }
    let encoded = text(value, "responseBase64", line)?;
    let bytes = STANDARD.decode(encoded).map_err(|_| refuse("response is not base64"))?;
    if bytes.is_empty() || STANDARD.encode(&bytes) != encoded {
        return Err(refuse("response base64 is empty or noncanonical"));
    }
    let response_doc = Document::decode(&bytes, limits)
        .map_err(|cause| refuse(&format!("response JSON: {cause:?}")))?;
    let response = response_doc.root();
    let count = unsigned(field(response, "count", line)?, "served token count", SAFE_INTEGER)?;
    let window = unsigned(field(response, "max_model_len", line)?, "served model window", SAFE_INTEGER)?;
    if window == 0 {
        return Err(refuse("served model window is zero"));
    }
    Ok((count, window))
}

fn validate_compaction_event(
    object: Value<'_>,
    line: usize,
    scope: Option<&str>,
    json_limits: Limits,
) -> ContractResult<()> {
    let refuse = |what: &str| {
        ContractError::InvalidRecord(format!(
            "events.jsonl line {line} carries a compaction record in {} {what}",
            scope_display(scope)
        ))
    };
    let record = object
        .get("data")
        .and_then(Value::as_object)
        .ok_or_else(|| refuse("without an object data field"))?;
    let count = |holder: Value<'_>, key: &str| -> ContractResult<u64> {
        unsigned(field(holder, key, line)?, key, SAFE_INTEGER).map_err(|cause| {
            refuse(&format!(
                "whose {key} is not a non-negative integer: {cause}"
            ))
        })
    };
    let string_or_null = |holder: Value<'_>, key: &str| -> ContractResult<()> {
        match holder.get(key) {
            Some(value) if value.is_null() || value.is_string() => Ok(()),
            _ => Err(refuse(&format!("whose {key} is neither a string nor null"))),
        }
    };
    let succeeded = record
        .get("succeeded")
        .and_then(Value::as_bool)
        .ok_or_else(|| refuse("without a boolean succeeded"))?;
    let status = record
        .get("status")
        .and_then(Value::as_str)
        .ok_or_else(|| refuse("without a compaction status"))?;
    let failed_status = matches!(
        status,
        "COMPRESSION_FAILED_INFLATED_TOKEN_COUNT"
            | "COMPRESSION_FAILED_TOKEN_COUNT_ERROR"
            | "COMPRESSION_FAILED_EMPTY_SUMMARY"
            | "COMPRESSION_FAILED_OUTPUT_TRUNCATED"
            | "COMPRESSION_FAILED_PROTOCOL_ERROR"
            | "COMPRESSION_FAILED_INSUFFICIENT_ROOM"
            | "COMPRESSION_FAILED_SUMMARY_OVER_BOUND"
            | "COMPRESSION_FAILED_HISTORY_CHANGED"
    );
    if !(status == "COMPRESSED" && succeeded || failed_status && !succeeded) {
        return Err(refuse(
            "whose status contradicts whether history was replaced; inspect the compaction producer",
        ));
    }
    let original_tokens = count(record, "originalTokenCount")?;
    if field(record, "newTokenCount", line)?.is_null() {
        if succeeded {
            return Err(refuse(
                "whose successful replacement has no measured token count; inspect the compaction producer",
            ));
        }
    } else {
        let new_tokens = count(record, "newTokenCount")?;
        if !succeeded && new_tokens != original_tokens {
            return Err(refuse(
                "whose failed attempt changes the retained history token count; inspect the compaction producer",
            ));
        }
    }
    if !matches!(
        record.get("triggerReason").and_then(Value::as_str),
        Some("token_limit" | "manual")
    ) && !record.get("triggerReason").is_some_and(Value::is_null)
    {
        return Err(refuse(
            "with an unknown compaction trigger reason; retain the original stream and recapture with a corrected compaction producer",
        ));
    }
    let measurements = field(record, "tokenMeasurements", line)?
        .elements()
        .ok_or_else(|| refuse("without a tokenMeasurements array"))?
        .map(|measurement| {
            let role = text(measurement, "role", line)?.to_string();
            let evidence = field(measurement, "evidence", line)?;
            let (count, window) = read_token_measurement(evidence, line, json_limits)?;
            Ok((role, count, window))
        })
        .collect::<ContractResult<Vec<_>>>()?;
    if measurements
        .first()
        .is_some_and(|first| measurements.iter().any(|value| value.2 != first.2))
    {
        return Err(refuse("whose tokenizer measurements disagree about the served model window"));
    }
    for (index, measurement) in measurements.iter().take(3).enumerate() {
        if measurement.0 != ["original", "summary_request", "prompt_only"][index] {
            return Err(refuse("whose tokenizer preflight measurements are missing or reordered"));
        }
    }

    // What one drawn candidate spent. `budget` is the transition's frozen
    // output ceiling when the record reports it; every candidate was issued
    // under that same ceiling, so none of them may exceed it.
    let candidate = |holder: Value<'_>, whose: &str, budget: Option<u64>| -> ContractResult<()> {
        if count(holder, "physicalRequests")? == 0 {
            return Err(refuse(&format!(
                "whose {whose} reports no request attempt for a candidate that was drawn"
            )));
        }
        for key in ["reasoning", "text"] {
            if !holder.get(key).is_some_and(Value::is_string) {
                return Err(refuse(&format!("whose {whose} lacks the {key} string")));
            }
        }
        for key in ["newTokenCount", "snapshotBytes"] {
            if !field(holder, key, line)?.is_null() {
                count(holder, key)?;
            }
        }
        let responses = field(holder, "sdkValuesJson", line)?
            .elements()
            .ok_or_else(|| {
                refuse("without SDK value JSON; capture this run with the current client")
            })?;
        for response in responses {
            let raw = response.as_str().ok_or_else(|| {
                refuse("with a non-string SDK value; inspect the compaction producer")
            })?;
            Document::decode(raw.as_bytes(), json_limits).map_err(|cause| {
                refuse(&format!(
                    "with undecodable SDK value JSON ({cause:?}); inspect the captured provider value"
                ))
            })?;
        }
        // What a draw its ceiling stopped had written of a call, as served:
        // always present, a name or null and the arguments text, and nothing
        // else, so a stopped snapshot call is kept rather than lost.
        let stopped = holder
            .get("incompleteToolCalls")
            .and_then(Value::elements)
            .ok_or_else(|| {
                refuse(&format!(
                    "whose {whose} lacks the incompleteToolCalls array"
                ))
            })?;
        for call in stopped {
            let shape_holds = call.members().is_some_and(|members| {
                let mut named = false;
                let mut served = false;
                for (key, value) in members {
                    match key {
                        "name" if !named => {
                            named = value.is_null()
                                || value.as_str().is_some_and(|name| !name.is_empty());
                            if !named {
                                return false;
                            }
                        }
                        "arguments" if !served => {
                            served = value.is_string();
                            if !served {
                                return false;
                            }
                        }
                        _ => return false,
                    }
                }
                named && served
            });
            if !shape_holds {
                return Err(refuse(&format!(
                    "whose {whose} carries a stopped call that is not a name or null and its served arguments text"
                )));
            }
        }
        string_or_null(holder, "finishReason")?;
        let usage = match holder.get("usage") {
            Some(value) if value.is_null() => return Ok(()),
            Some(value) => value.as_object().ok_or_else(|| {
                refuse(&format!(
                    "whose {whose} usage is neither an object nor null"
                ))
            })?,
            None => return Err(refuse(&format!("whose {whose} has no usage field"))),
        };
        let prompt = count(usage, "promptTokenCount")?;
        let output_tokens = count(usage, "candidatesTokenCount")?;
        let thinking = count(usage, "thoughtsTokenCount")?;
        let cached = count(usage, "cachedContentTokenCount")?;
        let total = count(usage, "totalTokenCount")?;
        if thinking > output_tokens
            || cached > prompt
            || budget.is_some_and(|ceiling| output_tokens > ceiling)
            || prompt.checked_add(output_tokens) != Some(total)
        {
            return Err(refuse(&format!(
                "whose {whose} served usage does not nest within its prompt, output, total and budget"
            )));
        }
        Ok(())
    };

    let budget = match record.get("output") {
        Some(value) if value.is_null() => None,
        Some(value) => {
            let output = value
                .as_object()
                .ok_or_else(|| refuse("whose output is neither an object nor null"))?;
            let max_output_tokens = count(output, "maxOutputTokens")?;
            if max_output_tokens == 0 {
                return Err(refuse("whose output has no positive budget"));
            }
            candidate(output, "output", Some(max_output_tokens))?;
            Some(max_output_tokens)
        }
        None => return Err(refuse("without an output field")),
    };

    // Always present, so "nothing was refused" cannot be read as "refusals
    // were not recorded" — including when no candidate reached `output`.
    let rejected = record
        .get("rejectedAttempts")
        .ok_or_else(|| refuse("without a rejectedAttempts field"))?
        .elements()
        .ok_or_else(|| refuse("whose rejectedAttempts is not an array"))?;
    let mut draw_counts = Vec::new();
    for (index, attempt) in rejected.enumerate() {
        let whose = format!("rejected attempt {index}");
        let attempt = attempt
            .as_object()
            .ok_or_else(|| refuse(&format!("whose {whose} is not an object")))?;
        if !matches!(
            attempt.get("status").and_then(Value::as_str),
            Some(
                "COMPRESSION_FAILED_INFLATED_TOKEN_COUNT"
                    | "COMPRESSION_FAILED_EMPTY_SUMMARY"
                    | "COMPRESSION_FAILED_OUTPUT_TRUNCATED"
                    | "COMPRESSION_FAILED_INSUFFICIENT_ROOM"
                    | "COMPRESSION_FAILED_SUMMARY_OVER_BOUND"
            )
        ) {
            return Err(refuse(&format!(
                "whose {whose} does not name a resampleable rule it failed; retain the original stream and recapture with a corrected compaction producer"
            )));
        }
        candidate(attempt, &whose, budget)?;
        let measured = field(attempt, "newTokenCount", line)?;
        draw_counts.push(if measured.is_null() {
            None
        } else {
            Some(count(attempt, "newTokenCount")?)
        });
    }
    if let Some(output) = record.get("output").filter(|value| !value.is_null()) {
        let measured = field(output, "newTokenCount", line)?;
        draw_counts.push(if measured.is_null() {
            None
        } else {
            Some(count(output, "newTokenCount")?)
        });
    }
    if !draw_counts.is_empty() {
        if measurements.len() < 3 || measurements[0].1 != original_tokens {
            return Err(refuse("whose drawn candidate has no measured original and preflight counts"));
        }
        let mut next = 3;
        for measured in draw_counts {
            if let Some(measured) = measured {
                if !measurements.get(next).is_some_and(|count| {
                    count.0 == "candidate" && count.1 == measured
                }) {
                    return Err(refuse("whose candidate count differs from the served tokenizer response"));
                }
                next += 1;
            }
        }
        if next != measurements.len() {
            return Err(refuse("with an unclaimed tokenizer measurement"));
        }
    } else if measurements.len() > 3
        || measurements.first().is_some_and(|measured| measured.1 != original_tokens)
    {
        return Err(refuse("whose preflight count differs from the served tokenizer response"));
    }
    if succeeded {
        let output = field(record, "output", line)?;
        if output.is_null() || field(output, "newTokenCount", line)?.is_null()
            || count(output, "newTokenCount")? != count(record, "newTokenCount")?
        {
            return Err(refuse("whose replacement count differs from its accepted draw"));
        }
    }
    Ok(())
}

/// Every emitted event names its scope: `null` and an absent field both mean
/// the main session, any other value is the owning `agent` tool-call id. Only
/// null-or-absent is read as the main session, so a value of any other shape
/// can only ever exclude an event from the main thread, never admit one to it.
fn required_scope<'a>(object: Value<'a>, line: usize) -> ContractResult<Option<&'a str>> {
    match object.get("parent_tool_use_id") {
        None => Ok(None),
        Some(value) if value.is_null() => Ok(None),
        Some(value) if value.as_str().is_some_and(|id| !id.is_empty()) => Ok(value.as_str()),
        Some(other) => Err(ContractError::InvalidRecord(format!(
            "events.jsonl line {line} has parent_tool_use_id {other:?}, which is neither null nor a non-empty agent tool-call id"
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

/// The trusted caller supplies the manifest fixed before any model dispatch.
/// This is a captured value, never a pathname or a later Config lookup.
#[derive(Clone, Debug)]
pub struct RuntimeBindings {
    manifest: Document,
}
impl RuntimeBindings {
    pub fn new(manifest: Document) -> ContractResult<Self> {
        Self::check(manifest).map_err(|cause| {
            ContractError::InvalidConfiguration(format!("runtime bindings: {cause}"))
        })
    }
    fn check(manifest: Document) -> ContractResult<Self> {
        let value = manifest.root();
        for key in ["cwd", "model", "permission_mode", "qwen_code_version"] {
            text(value, key, 0)?;
        }
        if text(value, "stream_contract_sha256", 0)? != STREAM_CONTRACT_SHA256 {
            return Err(ContractError::InvalidDefinition(
                "runtime manifest contract identity mismatch".into(),
            ));
        }
        for key in ["tools", "agents", "slash_commands"] {
            string_set(field(value, key, 0)?, key)?;
        }
        if !field(value, "mcp_servers", 0)?.is_array() {
            return Err(ContractError::InvalidDefinition(
                "runtime manifest mcp_servers must be an array".into(),
            ));
        }
        Ok(Self { manifest })
    }
    fn validate_stream_start(&self, object: Value<'_>, line: usize) -> ContractResult<()> {
        if text(object, "type", line)? != "system"
            || text(object, "subtype", line)? != "stream_start"
            || required_scope(object, line)?.is_some()
            || text(object, "stream_contract_sha256", line)? != STREAM_CONTRACT_SHA256
        {
            return Err(ContractError::InvalidRecord(format!(
                "events.jsonl line {line} is not the root stream_start for this contract; retain the complete output from the matching client"
            )));
        }
        Ok(())
    }
    fn validate_init(&self, object: Value<'_>, line: usize) -> ContractResult<()> {
        if text(object, "type", line)? != "system"
            || text(object, "subtype", line)? != "init"
            || required_scope(object, line)?.is_some()
        {
            return Err(ContractError::InvalidRecord(format!(
                "events.jsonl line {line} is not root runtime metadata; retain the initialized root invocation"
            )));
        }
        for key in [
            "stream_contract_sha256",
            "cwd",
            "model",
            "permission_mode",
            "qwen_code_version",
            "mcp_servers",
        ] {
            if field(object, key, line)? != field(self.manifest.root(), key, 0)? {
                return Err(ContractError::InvalidRecord(format!(
                    "events.jsonl init field {key} differs from the pinned contract"
                )));
            }
        }
        for key in ["tools", "agents", "slash_commands"] {
            if string_set(field(object, key, line)?, key)?
                != string_set(field(self.manifest.root(), key, 0)?, key)?
            {
                return Err(ContractError::InvalidRecord(format!(
                    "events.jsonl init field {key} differs from the pinned contract"
                )));
            }
        }
        Ok(())
    }
}
fn string_set<'a>(value: Value<'a>, name: &str) -> ContractResult<BTreeSet<&'a str>> {
    let items = value
        .elements()
        .ok_or_else(|| ContractError::InvalidRecord(format!("{name} must be an array")))?;
    let mut result = BTreeSet::new();
    for item in items {
        let text = item.as_str().filter(|s| !s.is_empty()).ok_or_else(|| {
            ContractError::InvalidRecord(format!("{name} contains a non-string or empty value"))
        })?;
        if !result.insert(text) {
            return Err(ContractError::InvalidRecord(format!(
                "{name} contains duplicate {text:?}"
            )));
        }
    }
    Ok(result)
}
#[derive(Clone, Copy, Debug)]
pub struct RuntimeLimits {
    pub json: Limits,
    pub schema: ValidationLimits,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct RuntimeObservations {
    pub num_turns: Option<u64>,
    pub observed_usage: GenerationUsageSummary,
    pub observed_subagent_scope_count: u64,
    pub observed_unaccounted_records: u64,
}
#[derive(Clone, Debug, Serialize)]
pub struct RuntimeSnapshot {
    pub records_seen: u64,
    pub certified_prefix_records: u64,
    pub first_refusal: Option<ContractError>,
    pub pending_admission: bool,
    pub observations: RuntimeObservations,
}

#[derive(Clone)]
struct Terminal {
    line: usize,
    is_error: bool,
    subtype: String,
    response: Option<String>,
    duration_ms: Option<u64>,
    api_duration_ms: Option<u64>,
    num_turns: u64,
    error_message: Option<String>,
}
#[derive(Clone, Default)]
struct ScopeState {
    attempts: BTreeSet<String>,
    id: Option<String>,
    terminal: Option<Terminal>,
    partial: PartialStreamState,
    runtime_text: Option<Arc<str>>,
    model_text: Option<Arc<str>>,
    structured_input: Option<Arc<str>>,
    runtime_operation: Option<RuntimeOperation>,
    completed_runtime_operation: Option<String>,
}
#[derive(Clone)]
struct RuntimeOperation {
    id: String,
    output_sha256: String,
    output_bytes: u64,
}
struct ToolUse {
    name: String,
    structured_input: Option<Arc<str>>,
    line: usize,
    scope: Option<String>,
    returned: bool,
}
struct AdmissionPlan {
    runtime_initialized: bool,
    request_origin: Option<crate::model_requests::RequestOrigin>,
    request: Option<crate::model_requests::RequestAdmission>,
    response: Option<crate::model_requests::ResponseAdmission>,
    seed: Option<crate::model_requests::SeedAdmission>,
    generation: Option<crate::model_requests::GenerationAdmission>,
    completion: Option<crate::model_requests::CompletionAdmission>,
    row: usize,
    state: ScopeState,
    additions: BTreeMap<String, ToolUse>,
    returns: BTreeSet<String>,
    runtime_operation_id: Option<String>,
    prefix: u64,
}
struct PendingAdmission {
    sequence: u64,
    document: Document,
    plan: AdmissionPlan,
}
/// A single-use capability bound to this exact owner instance. It cannot be
/// forged, cloned or transplanted into another instance with equal bindings.
pub struct AdmissionToken {
    owner: std::sync::Arc<()>,
    sequence: Option<u64>,
}

pub struct RuntimeContract {
    requests: crate::model_requests::ModelRequests,
    identity: std::sync::Arc<()>,
    bindings: RuntimeBindings,
    limits: RuntimeLimits,
    session_id: Option<String>,
    runtime_initialized: bool,
    seen_uuids: BTreeSet<String>,
    observed_scopes: BTreeSet<String>,
    observed_journal: Option<String>,
    observed_requests: BTreeMap<String, bool>,
    records_seen: u64,
    prefix: u64,
    first_refusal: Option<ContractError>,
    observations: RuntimeObservations,
    scope_states: Vec<ScopeState>,
    scope_rows: BTreeMap<String, usize>,
    tool_uses: BTreeMap<String, ToolUse>,
    runtime_operation_ids: BTreeSet<String>,
    pending: Option<PendingAdmission>,
}
impl RuntimeContract {
    pub fn new(bindings: RuntimeBindings, limits: RuntimeLimits) -> Self {
        Self {
            requests: crate::model_requests::ModelRequests::default(),
            identity: std::sync::Arc::new(()),
            bindings,
            limits,
            session_id: None,
            runtime_initialized: false,
            seen_uuids: BTreeSet::new(),
            observed_scopes: BTreeSet::new(),
            observed_journal: None,
            observed_requests: BTreeMap::new(),
            records_seen: 0,
            prefix: 0,
            first_refusal: None,
            observations: RuntimeObservations::default(),
            scope_states: vec![ScopeState::default()],
            scope_rows: BTreeMap::new(),
            tool_uses: BTreeMap::new(),
            runtime_operation_ids: BTreeSet::new(),
            pending: None,
        }
    }
    pub fn snapshot(&self) -> RuntimeSnapshot {
        RuntimeSnapshot {
            records_seen: self.records_seen,
            certified_prefix_records: self.prefix,
            first_refusal: self.first_refusal.clone(),
            pending_admission: self.pending.is_some(),
            observations: self.observations,
        }
    }
    fn observe(&mut self, object: Value<'_>, line: usize) -> ContractResult<()> {
        let mut observations = self.observations;
        let scope = required_scope(object, line)?;
        let mut journal = None;
        let mut request = None;
        let mut outcome = None;
        let refuse = |detail: &str| {
            ContractError::InvalidRecord(format!(
            "events.jsonl line {line} has unaccounted physical evidence: {detail}; inspect the original request and outcome records"
        ))
        };
        match text(object, "type", line)? {
            "system" if object.get("subtype").and_then(Value::as_str) == Some("stream_start") => {
                if self.observed_journal.is_some() {
                    return Err(refuse("repeated journal origin"));
                }
                journal = Some(
                    text(
                        field(object, "request_evidence_origin", line)?,
                        "journal_id",
                        line,
                    )?
                    .to_string(),
                );
            }
            "model_request" => {
                let evidence = field(object, "request", line)?;
                let id = text(evidence, "request_id", line)?;
                if self.observed_journal.as_deref() != Some(text(evidence, "journal_id", line)?)
                    || self.observed_requests.contains_key(id)
                {
                    return Err(refuse("request has a foreign journal or repeated identity"));
                }
                observations.observed_usage = observations.observed_usage.admit_request()?;
                request = Some(id.to_string());
            }
            "model_response" => {
                let evidence = field(object, "response", line)?;
                let id = text(evidence, "request_id", line)?;
                if self.observed_journal.as_deref() != Some(text(evidence, "journal_id", line)?)
                    || !self.observed_requests.contains_key(id)
                {
                    return Err(refuse("response has no observed request owner"));
                }
                let event = field(evidence, "event", line)?;
                if text(event, "kind", line)? == "outcome" {
                    if self.observed_requests.get(id) == Some(&true) {
                        return Err(refuse("request has a repeated processing outcome"));
                    }
                    let usage = field(event, "served_usage", line)?;
                    observations.observed_usage =
                        observations.observed_usage.finalize(if usage.is_null() {
                            None
                        } else {
                            Some(ServedUsage::read(usage, line)?)
                        })?;
                    outcome = Some(id.to_string());
                }
            }
            "result" if scope.is_none() => {
                if observations.num_turns.is_some() {
                    return Err(refuse("repeated root terminal"));
                }
                observations.num_turns = Some(unsigned(
                    field(object, "num_turns", line)?,
                    "reported turns",
                    SAFE_INTEGER,
                )?);
            }
            _ => {}
        }
        if scope.is_some_and(|scope| !self.observed_scopes.contains(scope)) {
            observations.observed_subagent_scope_count = add(
                observations.observed_subagent_scope_count,
                1,
                "observed scope count",
            )?;
        }
        // Observation remains independent of certification after its first
        // refusal. Keep identities, not response bodies or decoded generations.
        self.observations = observations;
        if let Some(journal) = journal {
            self.observed_journal = Some(journal);
        }
        if let Some(id) = request {
            self.observed_requests.insert(id, false);
        }
        if let Some(id) = outcome {
            self.observed_requests.insert(id, true);
        }
        if let Some(scope) = scope {
            self.observed_scopes.insert(scope.to_string());
        }
        Ok(())
    }
    fn latch(&mut self, error: ContractError) -> ContractError {
        self.first_refusal.get_or_insert(error).clone()
    }
    pub fn observe_gap(&mut self, cause: ContractError) -> ContractResult<()> {
        let result: ContractResult<()> = (|| {
            let records = add(self.records_seen, 1, "event count")?;
            let unaccounted = add(
                self.observations.observed_unaccounted_records,
                1,
                "unaccounted record count",
            )?;
            self.records_seen = records;
            self.observations.observed_unaccounted_records = unaccounted;
            Ok(())
        })();
        self.latch(cause);
        if let Err(error) = &result {
            self.latch(error.clone());
        }
        result
    }
    /// Raw completed record payload, excluding its framing LF. There is no
    /// host parse or reserialization before this exact decoder.
    pub fn prepare_utf8(&mut self, bytes: &[u8], line: usize) -> ContractResult<AdmissionToken> {
        if self.pending.is_some() {
            return Err(self.latch(ContractError::InvalidRecord(
                "admission is already pending reconciliation".into(),
            )));
        }
        match Document::decode(bytes, self.limits.json) {
            Ok(document) => {
                let result = self.prepare(document, line);
                if let Err(cause) = &result {
                    self.latch(cause.clone());
                }
                result
            }
            Err(cause) => {
                let error = decode_failure(cause, line);
                self.observe_gap(error.clone())?;
                Err(error)
            }
        }
    }
    fn prepare(&mut self, document: Document, line: usize) -> ContractResult<AdmissionToken> {
        if self.pending.is_some() {
            return Err(self.latch(ContractError::InvalidRecord(
                "admission is already pending reconciliation".into(),
            )));
        }
        self.records_seen = add(self.records_seen, 1, "event count")?;
        let object = document.root();
        let identity = (|| {
            let uuid = text(object, "uuid", line)?;
            let session = text(object, "session_id", line)?;
            if self.records_seen == 1 {
                self.bindings.validate_stream_start(object, line)?;
                self.session_id = Some(session.to_string());
            }
            match self.session_id.as_deref() {
                Some(expected) if expected == session => {},
                Some(expected) => return Err(ContractError::InvalidRecord(format!("events.jsonl session_id changed from {expected:?} to {session:?} at line {line}"))),
                None => return Err(ContractError::InvalidRecord(format!("events.jsonl line {line} has no validated initial session owner"))),
            }
            if !self.seen_uuids.insert(uuid.to_string()) {
                return Err(ContractError::InvalidRecord(format!(
                    "events.jsonl line {line} repeats event uuid {uuid:?}"
                )));
            }
            Ok(())
        })();
        if let Err(error) = identity {
            self.observations.observed_unaccounted_records = add(
                self.observations.observed_unaccounted_records,
                1,
                "unaccounted record count",
            )?;
            self.latch(error.clone());
            return Err(error);
        }
        let decoded = DecodedRecord::decode(object, self.limits.schema, line);
        if decoded.is_err() || self.observe(object, line).is_err() {
            self.observations.observed_unaccounted_records = add(
                self.observations.observed_unaccounted_records,
                1,
                "unaccounted record count",
            )?;
        }
        if let Some(error) = &self.first_refusal {
            return Err(error.clone());
        }
        let plan = decoded.and_then(|record| self.plan(record, line));
        match plan {
            Err(error) => {
                self.latch(error.clone());
                Err(error)
            }
            Ok(plan) => {
                let sequence = self.records_seen;
                self.pending = Some(PendingAdmission {
                    sequence,
                    document,
                    plan,
                });
                Ok(AdmissionToken {
                    owner: self.identity.clone(),
                    sequence: Some(sequence),
                })
            }
        }
    }
    fn check_token(&self, token: &AdmissionToken) -> ContractResult<()> {
        if !std::sync::Arc::ptr_eq(&token.owner, &self.identity)
            || token.sequence.is_none()
            || self.pending.as_ref().map(|pending| pending.sequence) != token.sequence
        {
            return Err(ContractError::InvalidRecord(
                "stale or foreign admission capability".into(),
            ));
        }
        Ok(())
    }
    pub fn pending_bytes(&self, token: &AdmissionToken) -> ContractResult<&str> {
        self.check_token(token)?;
        Ok(self
            .pending
            .as_ref()
            .expect("checked pending admission")
            .document
            .source())
    }
    /// The caller invokes this only after its required admission barrier. For a
    /// captured reader the fact is record observation, not producer durability.
    /// Persistence/settlement receipts are separate from this semantic commit.
    pub fn commit(&mut self, token: &mut AdmissionToken) -> ContractResult<()> {
        self.check_token(&token)?;
        if let Some(error) = &self.first_refusal {
            return Err(error.clone());
        }
        let PendingAdmission { plan, .. } = self.pending.take().expect("checked pending admission");
        self.runtime_initialized = plan.runtime_initialized;
        if let Some(origin) = plan.request_origin {
            self.requests.commit_origin(origin);
        }
        if let Some(response) = plan.response {
            self.requests.commit_response(response);
        }
        if let Some(seed) = plan.seed {
            self.requests.commit_seed(seed);
        }
        if let Some(generation) = plan.generation {
            self.requests.commit_generation(generation);
        }
        if let Some(completion) = plan.completion {
            self.requests.commit_completion(completion);
        }
        if plan.row == self.scope_states.len() {
            let id = plan
                .state
                .id
                .as_ref()
                .expect("new rows are child scopes")
                .clone();
            self.scope_rows.insert(id, plan.row);
            self.scope_states.push(plan.state);
        } else {
            self.scope_states[plan.row] = plan.state;
        }
        if let Some(request) = plan.request {
            self.requests.commit(request);
        }
        self.tool_uses.extend(plan.additions);
        for id in plan.returns {
            self.tool_uses
                .get_mut(&id)
                .expect("planned return names issued tool")
                .returned = true;
        }
        if let Some(id) = plan.runtime_operation_id {
            self.runtime_operation_ids.insert(id);
        }
        self.prefix = plan.prefix;
        token.sequence = None;
        Ok(())
    }
    pub fn reject_pending(
        &mut self,
        token: &mut AdmissionToken,
        cause: ContractError,
    ) -> ContractResult<()> {
        self.check_token(&token)?;
        self.pending.take();
        token.sequence = None;
        self.latch(cause);
        Ok(())
    }
    fn ensure_ancestry(&self, scope: Option<&str>, line: usize) -> ContractResult<()> {
        let mut cursor = scope;
        while let Some(id) = cursor {
            let tool = self.tool_uses.get(id).ok_or_else(|| ContractError::InvalidRecord(format!("events.jsonl line {line} names parent_tool_use_id {id:?}, which no accepted generation issued as a tool_use id")))?;
            if tool.returned {
                return Err(ContractError::InvalidRecord(format!(
                    "events.jsonl line {line} continues child of returned tool {id:?}"
                )));
            }
            if let Some(row) = self.scope_rows.get(id) {
                if let Some(terminal) = &self.scope_states[*row].terminal {
                    return Err(ContractError::InvalidRecord(format!("events.jsonl line {line} continues {} after its terminal result at line {}", scope_display(Some(id)), terminal.line)));
                }
            }
            cursor = tool.scope.as_deref();
        }
        Ok(())
    }
    fn conversation_generation(
        &self,
        generation: &Generation,
        scope: Option<&str>,
    ) -> ContractResult<bool> {
        match &generation.output_scope {
            OutputScope::Internal if scope.is_none() => Ok(false),
            OutputScope::Conversation { parent }
                if parent.as_deref() == scope
                    && Some(generation.origin.scope.as_str()) == scope.or(self.session_id.as_deref()) => Ok(true),
            _ => Err(ContractError::InvalidRecord(
                "generation output scope contradicts its envelope or request owner; inspect the hash-bound generation and its producing scope".into()
            )),
        }
    }
    fn plan(&self, record: DecodedRecord<'_>, line: usize) -> ContractResult<AdmissionPlan> {
        let object = record.value();
        if let Some(terminal) = &self.scope_states[0].terminal {
            return Err(ContractError::InvalidRecord(format!("events.jsonl terminal result at line {} is followed by another event at line {line}", terminal.line)));
        }
        let scope = required_scope(object, line)?;
        self.ensure_ancestry(scope, line)?;
        let row = scope.map_or(0, |id| {
            self.scope_rows
                .get(id)
                .copied()
                .unwrap_or(self.scope_states.len())
        });
        let mut state = self
            .scope_states
            .get(row)
            .cloned()
            .unwrap_or_else(|| ScopeState {
                id: scope.map(str::to_string),
                ..ScopeState::default()
            });
        if let Some(origin) = object.get("origin") {
            if text(origin, "kind", line)? == "runtime" {
                if self
                    .requests
                    .has_chat_attempt_in_scope(scope.unwrap_or(text(object, "session_id", line)?))
                {
                    return Err(ContractError::InvalidRecord(format!(
                        "events.jsonl line {line} claims runtime assistant output after a chat attempt in the same scope; inspect the original generation and its output origin"
                    )));
                }
                let operation_id = text(origin, "operation_id", line)?;
                let pending = state
                    .runtime_operation
                    .as_ref()
                    .map(|operation| operation.id.as_str())
                    == Some(operation_id);
                let closing = matches!(record.kind(), EventKind::StreamEvent)
                    && object
                        .get("event")
                        .and_then(|event| event.get("type"))
                        .and_then(Value::as_str)
                        == Some("message_stop")
                    && state.completed_runtime_operation.as_deref() == Some(operation_id);
                if !pending && !closing {
                    return Err(ContractError::InvalidRecord(format!(
                        "events.jsonl line {line} claims runtime output without its local operation receipt in the same scope; retain the complete original stream"
                    )));
                }
            }
            if text(origin, "kind", line)? == "model"
                && text(origin, "kv_scope", line)?
                    != scope.unwrap_or(text(object, "session_id", line)?)
            {
                return Err(ContractError::InvalidRecord("Assistant output belongs to another session scope; inspect its request ownership".into()));
            }
            if let Some(attempt) = self.requests.validate_output_origin(origin, line)? {
                if self
                    .scope_states
                    .iter()
                    .enumerate()
                    .any(|(index, state)| index != row && state.attempts.contains(&attempt))
                {
                    return Err(ContractError::InvalidRecord(
                        "Chat output changes assistant scope; inspect its request ownership".into(),
                    ));
                }
                state.attempts.insert(attempt);
            }
            state.partial.observe_origin(origin, line)?;
        }
        let mut request = None;
        let mut response = None;
        let mut seed = None;
        let mut generation = None;
        let mut completion = None;
        let mut additions = BTreeMap::new();
        let mut returns = BTreeSet::new();
        let mut runtime_operation_id = None;
        let mut runtime_initialized = self.runtime_initialized;
        match record.kind() {
            EventKind::ModelRequest => {
                if !runtime_initialized {
                    return Err(ContractError::InvalidRecord(format!(
                        "events.jsonl line {line} dispatches model work without the pinned runtime init; retain the complete initialized invocation"
                    )));
                }
                let admission = self.requests.plan(object, line, self.limits.json)?;
                if admission.is_chat_attempt() && state.runtime_text.is_some() {
                    return Err(ContractError::InvalidRecord(format!(
                        "events.jsonl line {line} dispatches a chat attempt after runtime assistant output in the same scope; inspect the original local result and request"
                    )));
                }
                request = Some(admission);
            }
            EventKind::ModelResponse => {
                response = Some(self.requests.plan_response(object, line)?);
            }
            EventKind::ModelNormalizationSeed => {
                seed = Some(self.requests.plan_seed(object, line)?);
            }
            EventKind::ModelGeneration => {
                let admission = self.requests.plan_generation(
                    object,
                    line,
                    self.limits.json,
                    self.limits.schema,
                )?;
                if self.conversation_generation(&admission.generation, scope)? {
                    state
                        .partial
                        .observe_generation(admission.generation.clone(), line)?;
                }
                generation = Some(admission);
            }
            EventKind::ModelAttemptCompletion => {
                let admission = self.requests.plan_completion(object, line)?;
                if self.conversation_generation(&admission.generation, scope)? {
                    state
                        .partial
                        .observe_completion(&admission.generation.origin, admission.accepted)?;
                    if admission.accepted {
                        if scope.is_none() {
                            state.model_text =
                                Some(Arc::from(admission.generation.display_text.as_str()));
                            state.runtime_text = None;
                        }
                        for call in &admission.generation.calls {
                            if let Some(previous) = self
                                .tool_uses
                                .get(&call.id)
                                .or_else(|| additions.get(&call.id))
                            {
                                return Err(ContractError::InvalidRecord(format!("events.jsonl line {line} re-issues tool_use id {:?}, first issued at line {} in {}; inspect the accepted generations", call.id, previous.line, scope_display(previous.scope.as_deref()))));
                            }
                            additions.insert(
                                call.id.clone(),
                                ToolUse {
                                    name: call.name.clone().expect("accepted generation call name"),
                                    structured_input: if call.name.as_deref()
                                        == Some("structured_output")
                                    {
                                        call.arguments.as_deref().map(Arc::from)
                                    } else {
                                        None
                                    },
                                    line,
                                    scope: scope.map(str::to_string),
                                    returned: false,
                                },
                            );
                        }
                    }
                }
                completion = Some(admission);
            }
            EventKind::Assistant => {
                let content = field(field(object, "message", line)?, "content", line)?
                    .elements()
                    .ok_or_else(|| ContractError::InvalidRecord(format!(
                        "events.jsonl line {line} has no runtime assistant content; inspect the original recording"
                    )))?;
                let mut rendered = String::new();
                for block in content {
                    if text(block, "type", line)? != "text" {
                        return Err(ContractError::InvalidRecord(format!(
                            "events.jsonl line {line} claims non-text runtime assistant output; inspect its producing operation"
                        )));
                    }
                    rendered.push_str(text(block, "text", line)?);
                }
                let receipt = state.runtime_operation.take().ok_or_else(|| {
                    ContractError::InvalidRecord(format!(
                        "events.jsonl line {line} has runtime assistant output without a local operation receipt; retain the complete original stream"
                    ))
                })?;
                if rendered.len() as u64 != receipt.output_bytes
                    || crate::generation::sha256(rendered.as_bytes()) != receipt.output_sha256
                {
                    return Err(ContractError::InvalidRecord(format!(
                        "events.jsonl line {line} changes the output of local operation {:?}; inspect its receipt and assistant bytes",
                        receipt.id
                    )));
                }
                state.completed_runtime_operation = Some(receipt.id);
                state.partial.complete_message(&rendered, line)?;
                state.runtime_text = Some(Arc::from(rendered));
                if scope.is_none() {
                    state.model_text = None;
                }
            }
            EventKind::User => {
                if let Some(blocks) = field(object, "message", line)?
                    .get("content")
                    .and_then(Value::elements)
                {
                    for block in blocks {
                        if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                            continue;
                        }
                        let id = text(block, "tool_use_id", line)?;
                        self.require_tool_owner(id, scope, line)?;
                        if block.get("is_error").and_then(Value::as_bool) == Some(false) {
                            let tool = self.tool_uses.get(id).expect("checked tool owner");
                            if tool.name == "structured_output" && state.structured_input.is_none()
                            {
                                state.structured_input = tool.structured_input.clone();
                            }
                        }
                        if !returns.insert(id.to_string()) {
                            return Err(ContractError::InvalidRecord(format!(
                                "events.jsonl line {line} repeats tool result {id:?}"
                            )));
                        }
                    }
                }
            }
            EventKind::StreamEvent => {
                state.partial.observe(
                    record.partial().expect("decoded stream event"),
                    scope.is_none(),
                    line,
                )?;
                let event = field(object, "event", line)?;
                if text(event, "type", line)? == "message_stop" {
                    state.completed_runtime_operation = None;
                }
                if text(event, "type", line)? == "tool_progress" {
                    self.require_tool_owner(text(event, "tool_use_id", line)?, scope, line)?;
                }
            }
            EventKind::System => {
                let subtype = SystemKind::from_wire(text(object, "subtype", line)?)
                    .expect("decoded system subtype");
                match subtype {
                    SystemKind::StreamStart if self.prefix == 0 => {},
                    SystemKind::StreamStart => return Err(ContractError::InvalidRecord(format!("events.jsonl line {line} starts another invocation before this invocation has closed; retain one complete invocation"))),
                    SystemKind::Init => {
                        self.bindings.validate_init(object, line)?;
                        runtime_initialized = true;
                    },
                    SystemKind::Compaction => validate_compaction_event(object, line, scope, self.limits.json)?,
                    SystemKind::RuntimeOperation => {
                        let data = field(object, "data", line)?;
                        let id = text(data, "operation_id", line)?;
                        if state.runtime_operation.is_some() || self.runtime_operation_ids.contains(id) {
                            return Err(ContractError::InvalidRecord(format!(
                                "events.jsonl line {line} repeats or leaves open a local operation receipt; inspect the original operation and assistant output"
                            )));
                        }
                        let output_bytes = unsigned(field(data, "output_bytes", line)?, "local output bytes", SAFE_INTEGER)?;
                        state.runtime_operation = Some(RuntimeOperation {
                            id: id.to_string(),
                            output_sha256: text(data, "output_sha256", line)?.to_string(),
                            output_bytes,
                        });
                        state.completed_runtime_operation = None;
                        runtime_operation_id = Some(id.to_string());
                    },
                    SystemKind::SessionRecordingDegraded | SystemKind::TurnCleanupFailed | SystemKind::VisionBridgeFailed => return Err(ContractError::InvalidRecord(format!("events.jsonl line {line} reports operational failure {}: {}", subtype.wire(), field(object, "data", line)?.raw()))),
                    SystemKind::SessionStart | SystemKind::SessionEnd => return Err(ContractError::InvalidRecord(format!("events.jsonl line {line} declares transport ownership inside an already owned invocation"))),
                    SystemKind::TaskNotification => { if let Some(usage) = field(object, "data", line)?.get("usage") { GenerationUsageSummary::read(field(usage, "ownerUsage", line)?, line)?; } },
                    SystemKind::TaskStarted | SystemKind::WorktreeStarted | SystemKind::WorktreeRestored | SystemKind::VisionRouting | SystemKind::VisionBridge => {},
                }
            }
            EventKind::Result => {
                if state.runtime_operation.is_some() {
                    return Err(ContractError::InvalidRecord(format!(
                        "events.jsonl line {line} ends before the local operation's assistant output; retain the complete original stream"
                    )));
                }
                state.partial.finish(line)?;
                state.completed_runtime_operation = None;
                state.terminal = Some(terminal(object, line, scope.is_none())?);
                if let Some(structured) = object.get("structured_result") {
                    if scope.is_some() || state.terminal.as_ref().expect("parsed terminal").is_error
                    {
                        return Err(ContractError::InvalidRecord(format!(
                            "events.jsonl line {line} claims structured output outside a successful root result"
                        )));
                    }
                    let input = state.structured_input.as_deref().ok_or_else(|| {
                        ContractError::InvalidRecord(format!(
                            "events.jsonl line {line} has no successful accepted structured_output submission"
                        ))
                    })?;
                    let submitted = Document::decode(input.as_bytes(), self.limits.json)
                        .map_err(|cause| decode_failure(cause, line))?;
                    let response = state
                        .terminal
                        .as_ref()
                        .expect("parsed terminal")
                        .response
                        .as_deref()
                        .expect("successful root result has text");
                    let rendered = Document::decode(response.as_bytes(), self.limits.json)
                        .map_err(|cause| decode_failure(cause, line))?;
                    if structured != submitted.root() || structured != rendered.root() {
                        return Err(ContractError::InvalidRecord(format!(
                            "events.jsonl line {line} structured result contradicts the accepted tool submission or result text"
                        )));
                    }
                } else if scope.is_none()
                    && !state.terminal.as_ref().expect("parsed terminal").is_error
                    && state.structured_input.is_some()
                {
                    return Err(ContractError::InvalidRecord(format!(
                        "events.jsonl line {line} omits the successful structured_output submission"
                    )));
                }
                if state.runtime_text.as_deref().is_some_and(|rendered| {
                    state
                        .terminal
                        .as_ref()
                        .and_then(|terminal| terminal.response.as_deref())
                        != Some(rendered)
                }) {
                    return Err(ContractError::InvalidRecord(format!(
                        "events.jsonl line {line} terminal result contradicts runtime assistant text; inspect the complete original recording"
                    )));
                }
                if scope.is_none()
                    && !state.terminal.as_ref().expect("parsed terminal").is_error
                    && object.get("structured_result").is_none()
                    && state.model_text.as_deref().is_some_and(|rendered| {
                        state
                            .terminal
                            .as_ref()
                            .and_then(|terminal| terminal.response.as_deref())
                            != Some(rendered)
                    })
                {
                    return Err(ContractError::InvalidRecord(format!(
                        "events.jsonl line {line} successful result contradicts accepted model text; inspect the complete original recording"
                    )));
                }
                if scope.is_none() {
                    if !runtime_initialized
                        && !state.terminal.as_ref().expect("parsed terminal").is_error
                    {
                        return Err(ContractError::InvalidRecord(format!(
                            "events.jsonl line {line} reports success without the pinned runtime init; retain the complete initialized invocation"
                        )));
                    }
                    for child in self.scope_states.iter().skip(1) {
                        child.partial.finish(line)?;
                    }
                    self.requests.validate_summary(object, line)?;
                }
            }
        }
        Ok(AdmissionPlan {
            runtime_initialized,
            request_origin: if self.prefix == 0 {
                Some(
                    self.requests
                        .plan_origin(field(object, "request_evidence_origin", line)?, line)?,
                )
            } else {
                None
            },
            request,
            response,
            seed,
            generation,
            completion,
            row,
            state,
            additions,
            returns,
            runtime_operation_id,
            prefix: add(self.prefix, 1, "certified prefix")?,
        })
    }
    fn require_tool_owner(&self, id: &str, scope: Option<&str>, line: usize) -> ContractResult<()> {
        let tool = self.tool_uses.get(id).ok_or_else(|| {
            ContractError::InvalidRecord(format!(
                "events.jsonl line {line} names tool {id:?} before issuance"
            ))
        })?;
        if tool.scope.as_deref() != scope || tool.returned {
            return Err(ContractError::InvalidRecord(format!(
                "events.jsonl line {line} names wrong-owner or already returned tool {id:?}"
            )));
        }
        Ok(())
    }
    /// This is semantic completion only. Capture and actor settlement must be
    /// established by the host before it publishes a complete service result.
    pub fn finish(&self) -> ContractResult<AgentResult> {
        if let Some(error) = &self.first_refusal {
            return Err(error.clone());
        }
        if self.pending.is_some() {
            return Err(ContractError::InvalidRecord(
                "admission remains pending reconciliation".into(),
            ));
        }
        if self.prefix == 0 {
            return Err(ContractError::InvalidRecord(
                "events.jsonl contains no events".into(),
            ));
        }
        if let Some(state) = self
            .scope_states
            .iter()
            .find(|state| state.runtime_operation.is_some())
        {
            return Err(ContractError::InvalidRecord(format!(
                "events.jsonl omits the assistant output of a local operation in {}; retain the complete original stream",
                scope_display(state.id.as_deref())
            )));
        }
        let main = &self.scope_states[0];
        let terminal = main.terminal.as_ref().ok_or_else(|| {
            ContractError::InvalidRecord(format!(
                "events.jsonl has {} event(s) but no main-session terminal result",
                self.prefix
            ))
        })?;
        let scopes = self
            .scope_states
            .iter()
            .skip(1)
            .map(|state| {
                let id = state.id.as_ref().expect("child row identity");
                let tool = self.tool_uses.get(id).expect("issued child owner");
                AgentScope {
                    tool_use_id: id.clone(),
                    tool_name: tool.name.clone(),
                    reported_num_turns: state.terminal.as_ref().map(|terminal| terminal.num_turns),
                    is_error: state.terminal.as_ref().map(|terminal| terminal.is_error),
                    subtype: state
                        .terminal
                        .as_ref()
                        .map(|terminal| terminal.subtype.clone()),
                    error_message: state
                        .terminal
                        .as_ref()
                        .and_then(|terminal| terminal.error_message.clone()),
                }
            })
            .collect();
        Ok(AgentResult {
            is_error: terminal.is_error,
            subtype: terminal.subtype.clone(),
            response: terminal.response.clone().expect("root response admitted"),
            duration_ms: terminal.duration_ms.expect("root duration admitted"),
            api_duration_ms: terminal.api_duration_ms.expect("root duration admitted"),
            num_turns: terminal.num_turns,
            main_kv_scope: self
                .session_id
                .clone()
                .expect("validated initial session owner"),
            usage: self.requests.all_usage(),
            request_scopes: self
                .requests
                .usage_scopes()
                .map(|(scope, usage)| RequestScope {
                    kv_scope: scope.to_string(),
                    usage: *usage,
                })
                .collect(),
            scopes,
        })
    }
}
fn add(left: u64, right: u64, name: &str) -> ContractResult<u64> {
    left.checked_add(right)
        .ok_or_else(|| ContractError::ValidationUnavailable(format!("{name} overflowed")))
}
fn terminal(object: Value<'_>, line: usize, root: bool) -> ContractResult<Terminal> {
    let is_error = field(object, "is_error", line)?.as_bool().ok_or_else(|| {
        ContractError::InvalidRecord("terminal result lacks boolean is_error".into())
    })?;
    let subtype = text(object, "subtype", line)?;
    if (is_error && !ERROR_SUBTYPES.contains(&subtype)) || (!is_error && subtype != SUCCESS_SUBTYPE)
    {
        return Err(ContractError::InvalidRecord(format!(
            "{} result has unsupported subtype {subtype:?}",
            if is_error { "error" } else { "successful" }
        )));
    }
    let duration = |key| match object.get(key) {
        Some(value) if !root && value.is_null() => Ok(None),
        Some(value) => unsigned(value, key, u64::MAX).map(Some).map_err(|cause| {
            ContractError::InvalidRecord(format!(
                "terminal result lacks non-negative integer {key}: {cause}"
            ))
        }),
        None => Err(ContractError::InvalidRecord(format!(
            "terminal result lacks non-negative integer {key}: missing field"
        ))),
    };
    let duration_ms = duration("duration_ms")?;
    let api_duration_ms = duration("duration_api_ms")?;
    let num_turns = unsigned(field(object, "num_turns", line)?, "num_turns", SAFE_INTEGER)?;
    match object.get("usage") {
        Some(usage) if !root && usage.is_null() => {}
        _ => {
            GenerationUsageSummary::read(field(object, "usage", line)?, line)?;
        }
    }
    if !object
        .get("permission_denials")
        .is_some_and(Value::is_array)
    {
        return Err(ContractError::InvalidRecord(
            "terminal result lacks array permission_denials".into(),
        ));
    }
    let error_message = match object.get("error") {
        None => None,
        Some(error) => {
            let object = error.as_object().ok_or_else(|| {
                ContractError::InvalidRecord("terminal error must be an object".into())
            })?;
            match object.get("message") {
                None => None,
                Some(_) => Some(text(object, "message", line)?.to_string()),
            }
        }
    };
    if is_error && error_message.is_none() {
        return Err(ContractError::InvalidRecord(
            "error result lacks non-empty error.message".into(),
        ));
    }
    let response = if is_error {
        error_message.clone()
    } else if root {
        Some(text(object, "result", line)?.to_string())
    } else {
        object
            .get("result")
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    Ok(Terminal {
        line,
        is_error,
        subtype: subtype.to_string(),
        response,
        duration_ms,
        api_duration_ms,
        num_turns,
        error_message,
    })
}
fn decode_failure(cause: crate::json::DecodeError, line: usize) -> ContractError {
    match cause {
        crate::json::DecodeError::ResourceLimit { resource, limit } => {
            ContractError::ValidationUnavailable(format!(
                "events.jsonl line {line}: {resource} limit {limit}"
            ))
        }
        other => ContractError::InvalidRecord(format!(
            "events.jsonl line {line} is invalid JSON: {other:?}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD, Engine};
    fn owner() -> RuntimeContract {
        let manifest = format!(
            r#"{{"stream_contract_sha256":"{STREAM_CONTRACT_SHA256}","cwd":"/owned","model":"model","permission_mode":"default","qwen_code_version":"version","tools":["tool"],"agents":[],"slash_commands":[],"mcp_servers":[]}}"#
        );
        let limits = RuntimeLimits {
            json: Limits {
                bytes: 1_000_000,
                nodes: 1_000_000,
                depth: 1000,
            },
            schema: ValidationLimits {
                operations: 1_000_000,
            },
        };
        let document = Document::decode(manifest.as_bytes(), limits.json).unwrap();
        RuntimeContract::new(RuntimeBindings::new(document).unwrap(), limits)
    }
    fn init() -> String {
        format!(
            r#"{{"type":"system","subtype":"init","uuid":"init","session_id":"session","parent_tool_use_id":null,"stream_contract_sha256":"{STREAM_CONTRACT_SHA256}","cwd":"/owned","model":"model","permission_mode":"default","qwen_code_version":"version","tools":["tool"],"agents":[],"slash_commands":[],"mcp_servers":[]}}"#
        )
    }
    fn stream_start() -> String {
        format!(
            r#"{{"type":"system","subtype":"stream_start","request_evidence_origin":{{"journal_id":"fixture","first_sequence":1}},"uuid":"stream-start","session_id":"session","parent_tool_use_id":null,"stream_contract_sha256":"{STREAM_CONTRACT_SHA256}"}}"#
        )
    }
    fn admit(owner: &mut RuntimeContract, raw: &str) -> ContractResult<()> {
        let mut token = owner.prepare_utf8(raw.as_bytes(), 1)?;
        owner.commit(&mut token)
    }
    fn initialized() -> RuntimeContract {
        let mut owner = owner();
        admit(&mut owner, &stream_start()).unwrap();
        admit(&mut owner, &init()).unwrap();
        owner
    }
    #[test]
    fn compaction_status_and_retained_count_agree_with_the_history_decision() {
        let measurement = |role: &str, count: u64| {
            let response = serde_json::json!({
                "count": count, "max_model_len": 100
            })
            .to_string();
            serde_json::json!({
                "role": role,
                "evidence": {
                    "requestUrl": "http://fixture.invalid/tokenize",
                    "requestJson": serde_json::json!({
                        "model": "fixture", "messages": [],
                        "add_generation_prompt": true
                    }).to_string(),
                    "responseStatus": 200,
                    "responseContentType": "application/json",
                    "responseBase64": STANDARD.encode(response.as_bytes())
                }
            })
        };
        let failed = serde_json::json!({
            "type":"system", "subtype":"compaction", "uuid":"compaction",
            "session_id":"session", "parent_tool_use_id":null,
            "data":{
                "status":"COMPRESSION_FAILED_PROTOCOL_ERROR", "succeeded":false,
                "originalTokenCount":24, "newTokenCount":24,
                "triggerReason":null, "postCompactionHistory":null,
                "output":null, "rejectedAttempts":[],
                "tokenMeasurements":[]
            }
        });
        admit(&mut initialized(), &failed.to_string()).unwrap();
        let mut success = failed.clone();
        success["data"]["status"] = serde_json::json!("COMPRESSED");
        success["data"]["succeeded"] = serde_json::json!(true);
        success["data"]["newTokenCount"] = serde_json::json!(12);
        success["data"]["postCompactionHistory"] = serde_json::json!([
            {"role":"user", "parts":[{"text":"retained input"}]}
        ]);
        success["data"]["tokenMeasurements"] = serde_json::json!([
            measurement("original", 24),
            measurement("summary_request", 20),
            measurement("prompt_only", 18),
            measurement("candidate", 12)
        ]);
        success["data"]["output"] = serde_json::json!({
            "maxOutputTokens":8, "physicalRequests":1,
            "operationId":"compaction-operation", "functionCalls":[],
            "text":"summary", "reasoning":"", "sdkValuesJson":["{}"],
            "newTokenCount":12, "snapshotBytes":7,
            "incompleteToolCalls":[], "finishReason":"STOP",
            "usage":{"promptTokenCount":24,"candidatesTokenCount":4,
                "thoughtsTokenCount":0,"cachedContentTokenCount":0,
                "totalTokenCount":28}
        });
        admit(&mut initialized(), &success.to_string()).unwrap();
        let mut forged_count = success.clone();
        forged_count["data"]["output"]["newTokenCount"] = serde_json::json!(11);
        forged_count["data"]["newTokenCount"] = serde_json::json!(11);
        assert!(admit(&mut initialized(), &forged_count.to_string()).is_err());
        let mut missing_measurement = success.clone();
        missing_measurement["data"]["tokenMeasurements"]
            .as_array_mut().unwrap().pop();
        assert!(admit(&mut initialized(), &missing_measurement.to_string()).is_err());
        for changed in [
            ("status", serde_json::json!("COMPRESSED")),
            ("status", serde_json::json!("NOOP")),
            ("status", serde_json::json!("UNKNOWN_STATUS")),
            ("newTokenCount", serde_json::json!(12)),
            ("triggerReason", serde_json::json!("unclassified")),
        ] {
            let mut forged = failed.clone();
            forged["data"][changed.0] = changed.1;
            assert!(admit(&mut initialized(), &forged.to_string()).is_err());
        }
        let mut forged = success;
        forged["data"]["status"] = serde_json::json!("COMPRESSION_FAILED_PROTOCOL_ERROR");
        assert!(admit(&mut initialized(), &forged.to_string()).is_err());
        let mut rejected = forged["data"]["output"].clone();
        rejected.as_object_mut().unwrap().remove("maxOutputTokens");
        rejected["status"] = serde_json::json!("COMPRESSED");
        forged["data"]["status"] = serde_json::json!("COMPRESSED");
        forged["data"]["rejectedAttempts"] = serde_json::json!([rejected]);
        assert!(admit(&mut initialized(), &forged.to_string()).is_err());
    }
    fn partial(id: &str, event: &str) -> String {
        let parsed: serde_json::Value = serde_json::from_str(event).unwrap();
        let origin = if matches!(
            parsed["type"].as_str(),
            Some("tool_progress" | "active_goal" | "goal_state")
        ) {
            ""
        } else {
            r#""origin":{"kind":"runtime","operation_id":"local-operation"},"#
        };
        format!(
            r#"{{"type":"stream_event",{origin}"uuid":"{id}","session_id":"session","parent_tool_use_id":null,"event":{event}}}"#
        )
    }
    fn assistant(id: &str, scope: &str, content: &str) -> String {
        format!(
            r#"{{"type":"assistant","origin":{{"kind":"runtime","operation_id":"local-operation"}},"uuid":"{id}","session_id":"session","parent_tool_use_id":{scope},"message":{{"id":"presentation-{id}","type":"message","role":"assistant","content":{content},"stop_reason":null,"usage":null}}}}"#
        )
    }
    fn local_operation(scope: Option<&str>, text: &str) -> String {
        serde_json::json!({
            "type": "system", "subtype": "runtime_operation", "uuid": "local-operation-receipt",
            "session_id": "session", "parent_tool_use_id": scope,
            "data": {
                "operation_id": "local-operation", "kind": "slash_message",
                "output_sha256": crate::generation::sha256(text.as_bytes()),
                "output_bytes": text.len(),
            }
        })
        .to_string()
    }
    fn runtime_result(value: &str) -> String {
        serde_json::json!({
            "type":"result", "uuid":"runtime-result", "session_id":"session", "parent_tool_use_id":null,
            "subtype":"success", "is_error":false, "duration_ms":0, "duration_api_ms":0,
            "num_turns":0, "result":value, "permission_denials":[],
            "usage":{"requests":0,"usageReports":0,"unfinalizedRequests":0,
                "unreportedUsageRequests":0,"usage":null},
            "request_evidence":{"journal_id":"fixture","first_sequence":1,"request_count":0,
                "open_response_ids":[],"open_attempt_ids":[]}
        }).to_string()
    }
    fn fixture() -> Vec<serde_json::Value> {
        let mut rows: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("fixtures/ordinary-tool-wire.json")).unwrap();
        // Runtime metadata is authored from this test's manifest. The physical
        // and generation evidence retain their captured producer bytes.
        let mut metadata: serde_json::Value = serde_json::from_str(&init()).unwrap();
        metadata["session_id"] = rows[0]["session_id"].clone();
        rows.insert(1, metadata);
        rows.last_mut().unwrap()["result"] = serde_json::json!("before  after");
        rows
    }
    fn issued_owner() -> RuntimeContract {
        let mut owner = owner();
        for record in fixture() {
            let completed = record["type"] == "model_attempt_completion";
            admit(&mut owner, &record.to_string()).unwrap();
            if completed {
                break;
            }
        }
        assert!(owner.tool_uses.contains_key("provider__qwen_dup_2"));
        owner
    }
    #[test]
    fn generation_requires_its_normalization_seed() {
        for missing in [true, false] {
            let mut rows = fixture();
            if missing {
                rows.retain(|row| row["type"] != "model_normalization_seed");
            } else {
                let seed = rows
                    .iter_mut()
                    .find(|row| row["type"] == "model_normalization_seed")
                    .unwrap();
                seed["normalization_seed"]["history_call_ids"] = serde_json::json!([]);
            }
            let mut reader = owner();
            let refusal = rows
                .iter()
                .find_map(|row| admit(&mut reader, &row.to_string()).err())
                .expect("forged generation must be refused")
                .to_string();
            assert!(refusal.contains("normalization seed") || refusal.contains("history seed"));
        }
    }
    #[test]
    fn accepted_history_requires_published_generation() {
        let mut rows = fixture();
        let history = rows
            .iter()
            .position(|row| {
                row["type"] == "model_response" && row["response"]["event"]["kind"] == "history"
            })
            .unwrap();
        let history = rows.remove(history);
        let generation = rows
            .iter()
            .position(|row| row["type"] == "model_generation")
            .unwrap();
        rows.insert(generation, history);
        let mut reader = owner();
        let refusal = rows
            .iter()
            .find_map(|row| admit(&mut reader, &row.to_string()).err())
            .expect("early accepted history must be refused")
            .to_string();
        assert!(refusal.contains("acceptance precedes generation"));
    }
    #[test]
    fn structured_result_requires_the_returned_accepted_tool_arguments() {
        let mut owner = issued_owner();
        let issued = owner.tool_uses.get_mut("provider__qwen_dup_2").unwrap();
        issued.name = "structured_output".into();
        issued.structured_input = Some(Arc::from(r#"{"value":1e0}"#));
        let returned = serde_json::json!({
            "type":"user", "uuid":"structured-return", "session_id":fixture()[0]["session_id"],
            "parent_tool_use_id":null,
            "message":{"role":"user","content":[{"type":"tool_result",
                "tool_use_id":"provider__qwen_dup_2","is_error":false,
                "content":"Structured output accepted."}]}
        });
        admit(&mut owner, &returned.to_string()).unwrap();
        let mut terminal = fixture().last().unwrap().clone();
        terminal["result"] = serde_json::json!(r#"{"value":1.0}"#);
        terminal["structured_result"] = serde_json::json!({"value":1});
        admit(&mut owner, &terminal.to_string()).unwrap();

        let mut forged = issued_owner();
        let issued = forged.tool_uses.get_mut("provider__qwen_dup_2").unwrap();
        issued.name = "structured_output".into();
        issued.structured_input = Some(Arc::from(r#"{"value":1e0}"#));
        admit(&mut forged, &returned.to_string()).unwrap();
        terminal["structured_result"] = serde_json::json!({"value":false});
        assert!(admit(&mut forged, &terminal.to_string())
            .unwrap_err()
            .to_string()
            .contains("structured result contradicts"));
    }
    fn incomplete_owner(
        name: serde_json::Value,
        arguments: &str,
    ) -> (RuntimeContract, serde_json::Value) {
        let mut records = fixture();
        let generation = records
            .iter_mut()
            .find(|row| row["type"] == "model_generation")
            .unwrap();
        let evidence = &mut generation["generation"];
        let mut envelope: serde_json::Value =
            serde_json::from_str(evidence["generation_json"].as_str().unwrap()).unwrap();
        let source_request_id = envelope["observations"][0]["source_request_id"].clone();
        envelope["finish_reason"] = serde_json::json!("MAX_TOKENS");
        envelope["observations"] = serde_json::json!([{
            "source_request_id":source_request_id,
            "response":{"candidates":[{"content":{"parts":[],"role":"model"},"finishReason":"MAX_TOKENS"}],"usageMetadata":envelope["usage"]},
            "incomplete_tool_calls":[{"name":name,"arguments":arguments}],
            "tool_call_preparations":[], "call_ids":[]
        }]);
        let bytes = envelope.to_string();
        let hash = crate::generation::sha256(bytes.as_bytes());
        evidence["generation_bytes"] = serde_json::json!(bytes.len());
        evidence["generation_sha256"] = serde_json::json!(hash);
        evidence["generation_json"] = serde_json::json!(bytes);
        let mut owner = owner();
        for mut record in records {
            if matches!(record["type"].as_str(), Some("stream_event" | "result")) {
                continue;
            }
            if record["type"] == "model_response"
                && record["response"]["event"]["kind"] == "history"
            {
                record["response"]["event"]["disposition"] = serde_json::json!("abandoned");
            }
            if record["type"] == "model_attempt_completion" {
                record["completion"]["generation_sha256"] = serde_json::json!(hash);
                record["completion"]["disposition"] = serde_json::json!("abandoned");
                record["completion"]["consumer_observations"] = serde_json::json!(1);
            }
            admit(&mut owner, &record.to_string()).unwrap();
        }
        (owner, envelope["origin"].clone())
    }
    fn model_partial(
        owner: &RuntimeContract,
        origin: &serde_json::Value,
        id: &str,
        event: serde_json::Value,
    ) -> String {
        serde_json::json!({"type":"stream_event","uuid":id,"session_id":owner.session_id,
            "parent_tool_use_id":null,"origin":origin,"event":event})
        .to_string()
    }
    fn utility_request(id: &str, sequence: u64) -> String {
        let body = r#"{"kv_scope":"internal-utility","model":"fixture-model","stream":false,"messages":[]}"#;
        serde_json::json!({
            "type":"model_request", "uuid":format!("request-{id}"), "session_id":"session", "parent_tool_use_id":null,
            "request":{
                "journal_id":"fixture", "request_id":id, "sequence":sequence,
                "kv_scope":"internal-utility", "segment_id":format!("utility-segment-{id}"), "prompt_id":"utility-prompt",
                "owner":{"kind":"utility","operation_id":"utility-operation"}, "body":{"kind":"full","json":body},
                "decode_policy":{"mode":"nonstream","model":"fixture-model","strict_tool_calling":false,
                    "named_tool_choice":null,"exact_token_counting":false,"tagged_thinking_tags":false},
                "body_bytes":body.len(), "body_sha256":crate::generation::sha256(body.as_bytes())
            }
        }).to_string()
    }
    fn response(id: &str, sequence: u64, event: &str) -> String {
        format!(
            r#"{{"type":"model_response","uuid":"response-{id}-{sequence}","session_id":"session","parent_tool_use_id":null,"response":{{"journal_id":"fixture","request_id":"{id}","sequence":{sequence},"event":{event}}}}}"#
        )
    }
    fn utility_transport(owner: &mut RuntimeContract, id: &str, sequence: u64, output: u64) {
        admit(owner, &utility_request(id, sequence)).unwrap();
        admit(
            owner,
            &response(
                id,
                1,
                r#"{"kind":"http","status":200,"content_type":"application/json"}"#,
            ),
        )
        .unwrap();
        let body = serde_json::json!({
            "id":"utility-reply","object":"chat.completion","created":1,"model":"fixture-model",
            "choices":[{"index":0,"message":{"role":"assistant","content":""},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":0,"completion_tokens":output,"total_tokens":output,
                "prompt_tokens_details":{"cached_tokens":0},
                "completion_tokens_details":{"reasoning_tokens":0}}
        }).to_string();
        let chunk = serde_json::json!({"kind":"body","offset":0,"base64":STANDARD.encode(&body)});
        admit(owner, &response(id, 2, &chunk.to_string())).unwrap();
        let end = serde_json::json!({"kind":"end","termination":"eof","body_bytes":body.len(),
            "body_sha256":crate::generation::sha256(body.as_bytes()),"error":null});
        admit(owner, &response(id, 3, &end.to_string())).unwrap();
    }
    fn outcome(id: &str, output: &str) -> String {
        response(
            id,
            4,
            &format!(
                r#"{{"kind":"outcome","status":"completed","error":null,"sdk_values_seen":1,"pipeline_outputs_delivered":1,"served_usage":{{"promptTokenCount":0,"candidatesTokenCount":{output},"thoughtsTokenCount":0,"cachedContentTokenCount":0,"totalTokenCount":{output}}}}}"#
            ),
        )
    }
    #[test]
    fn a_startup_error_requires_stream_identity_but_no_fabricated_runtime() {
        let mut startup = owner();
        admit(&mut startup, &stream_start()).unwrap();
        let result = serde_json::json!({
            "type":"result", "subtype":"error_during_execution", "uuid":"failed", "session_id":"session",
            "parent_tool_use_id":null, "is_error":true, "duration_ms":0, "duration_api_ms":0, "num_turns":0,
            "usage":{"requests":0,"usageReports":0,"unfinalizedRequests":0,"unreportedUsageRequests":0,"usage":null},
            "permission_denials":[], "error":{"message":"Authentication failed before initialization"},
            "request_evidence":{"journal_id":"fixture","first_sequence":1,"request_count":0,"open_response_ids":[],"open_attempt_ids":[]}
        });
        admit(&mut startup, &result.to_string()).unwrap();
        let certified = startup.finish().unwrap();
        assert_eq!(certified.usage.requests, 0);
        assert!(certified.is_error);
        assert_eq!(certified.subtype, "error_during_execution");
        assert_eq!(
            certified.response,
            "Authentication failed before initialization"
        );
        assert!(!startup.runtime_initialized);
        for success in [false, true] {
            let mut rejected = owner();
            if success {
                admit(&mut rejected, &stream_start()).unwrap();
            }
            let mut candidate = result.clone();
            if success {
                candidate["subtype"] = serde_json::json!("success");
                candidate["is_error"] = serde_json::json!(false);
                candidate["result"] = serde_json::json!("claimed success");
                candidate.as_object_mut().unwrap().remove("error");
            }
            let failure = admit(&mut rejected, &candidate.to_string())
                .unwrap_err()
                .to_string();
            assert!(
                failure.contains(if success {
                    "without the pinned runtime init"
                } else {
                    "stream_start"
                }),
                "{failure}"
            );
        }
    }
    #[test]
    fn model_work_requires_committed_manifest_metadata_after_stream_start() {
        let mut missing = owner();
        admit(&mut missing, &stream_start()).unwrap();
        assert!(admit(&mut missing, &utility_request("early", 1))
            .unwrap_err()
            .to_string()
            .contains("without the pinned runtime init"));

        let mut ready = owner();
        admit(&mut ready, &stream_start()).unwrap();
        let mut token = ready.prepare_utf8(init().as_bytes(), 2).unwrap();
        assert!(!ready.runtime_initialized);
        ready.commit(&mut token).unwrap();
        assert!(ready.runtime_initialized);
        utility_transport(&mut ready, "ordinary", 1, 0);

        let mut mismatch = owner();
        admit(&mut mismatch, &stream_start()).unwrap();
        assert!(admit(&mut mismatch, &init().replace("/owned", "/foreign"))
            .unwrap_err()
            .to_string()
            .contains("cwd differs from the pinned contract"));
        assert!(!mismatch.runtime_initialized);
    }
    #[test]
    fn admission_waits_for_commit_and_tokens_belong_to_one_actual_owner() {
        let mut a = owner();
        let mut b = owner();
        let mut token = a.prepare_utf8(stream_start().as_bytes(), 1).unwrap();
        assert_eq!(a.pending_bytes(&token).unwrap(), stream_start());
        assert_eq!(a.snapshot().certified_prefix_records, 0);
        assert!(a.snapshot().pending_admission);
        assert!(b
            .commit(&mut token)
            .unwrap_err()
            .to_string()
            .contains("foreign admission"));
        assert_eq!(b.snapshot().records_seen, 0);
        assert_eq!(a.snapshot().certified_prefix_records, 0);
        a.commit(&mut token).unwrap();
        assert_eq!(a.snapshot().certified_prefix_records, 1);
        assert!(!a.snapshot().pending_admission);
        assert!(a
            .commit(&mut token)
            .unwrap_err()
            .to_string()
            .contains("stale"));
        let mut token = b.prepare_utf8(stream_start().as_bytes(), 1).unwrap();
        b.commit(&mut token).unwrap();
        assert_eq!(b.snapshot().certified_prefix_records, 1);
        assert!(!b.snapshot().pending_admission);
    }
    #[test]
    fn rejected_prepared_record_keeps_semantics_and_names_the_failed_barrier() {
        let mut owner = owner();
        let mut token = owner.prepare_utf8(stream_start().as_bytes(), 1).unwrap();
        owner
            .reject_pending(
                &mut token,
                ContractError::ValidationUnavailable("journal sync uncertain".into()),
            )
            .unwrap();
        assert_eq!(owner.snapshot().certified_prefix_records, 0);
        assert!(!owner.snapshot().pending_admission);
        assert!(owner
            .finish()
            .unwrap_err()
            .to_string()
            .contains("journal sync uncertain"));
    }
    #[test]
    fn captured_generation_owns_tools_and_physical_usage() {
        let mut owner = owner();
        for record in fixture() {
            let completion = record["type"] == "model_attempt_completion";
            if completion {
                assert!(owner.tool_uses.is_empty());
            }
            admit(&mut owner, &record.to_string()).unwrap();
            if completion {
                assert_eq!(owner.tool_uses["provider__qwen_dup_2"].name, "audit_probe");
            }
        }
        let result = owner.finish().unwrap();
        assert_eq!(result.usage.requests, 1);
        assert_eq!(result.usage.usage_reports, 1);
        assert_eq!(result.usage.usage.unwrap().output, 7);
        assert_eq!(result.num_turns, 1);
        assert_eq!(result.request_scopes.len(), 1);
        assert_eq!(result.request_scopes[0].kv_scope, result.main_kv_scope);
        assert_eq!(owner.observations.observed_usage, result.usage);
    }
    #[test]
    fn physical_history_cannot_replace_logical_completion() {
        let mut owner = owner();
        for record in fixture() {
            if record["type"] == "model_attempt_completion" || record["type"] == "stream_event" {
                continue;
            }
            let terminal = record["type"] == "result";
            let admitted = admit(&mut owner, &record.to_string());
            assert_eq!(admitted.is_err(), terminal);
        }
        assert!(owner.tool_uses.is_empty());
        assert!(owner.finish().is_err());
        assert_eq!(owner.observations.observed_usage.usage_reports, 1);
    }
    #[test]
    fn an_incomplete_model_call_preserves_arguments_and_never_issues_a_tool() {
        for (name, arguments) in [
            (
                serde_json::json!("write_file"),
                "{\"file_path\":\"a.md\",\"content\":\"cut",
            ),
            (serde_json::Value::Null, ""),
        ] {
            let (mut owner, origin) = incomplete_owner(name.clone(), arguments);
            for (id, event) in [
                (
                    "start",
                    serde_json::json!({"type":"message_start","message":{"id":"message","role":"assistant","model":"model","content":[]}}),
                ),
                (
                    "block",
                    serde_json::json!({"type":"content_block_start","index":0,"content_block":{"type":"incomplete_tool_use","name":name,"arguments":arguments}}),
                ),
            ] {
                let raw = model_partial(&owner, &origin, id, event);
                admit(&mut owner, &raw).unwrap();
            }
            assert!(owner.tool_uses.is_empty());
            assert_eq!(owner.observations.observed_usage.usage_reports, 1);
            assert_eq!(owner.observations.observed_usage.usage.unwrap().output, 7);
            let delta = model_partial(
                &owner,
                &origin,
                "delta",
                serde_json::json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"more"}}),
            );
            assert!(admit(&mut owner, &delta)
                .unwrap_err()
                .to_string()
                .contains("delta type contradicts"));
        }
        for block in [
            serde_json::json!({"type":"incomplete_tool_use","name":"","arguments":""}),
            serde_json::json!({"type":"incomplete_tool_use","name":"write_file"}),
            serde_json::json!({"type":"incomplete_tool_use","arguments":""}),
            serde_json::json!({"type":"incomplete_tool_use","name":"write_file","arguments":{}}),
            serde_json::json!({"type":"incomplete_tool_use","name":"write_file","arguments":"","input":{}}),
        ] {
            let (mut owner, origin) = incomplete_owner(serde_json::json!("write_file"), "");
            let start = model_partial(
                &owner,
                &origin,
                "start",
                serde_json::json!({"type":"message_start","message":{"id":"message","role":"assistant","model":"model","content":[]}}),
            );
            admit(&mut owner, &start).unwrap();
            let raw = model_partial(
                &owner,
                &origin,
                "block",
                serde_json::json!({"type":"content_block_start","index":0,"content_block":block}),
            );
            assert!(admit(&mut owner, &raw).is_err());
        }
    }
    #[test]
    fn runtime_text_requires_matching_partial_full_and_terminal_records() {
        let parts = [
            (
                "start",
                r#"{"type":"message_start","message":{"id":"runtime","role":"assistant","content":[]}}"#,
            ),
            (
                "block",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":"answer"}}"#,
            ),
            ("close", r#"{"type":"content_block_stop","index":0}"#),
        ];
        let full = assistant(
            "runtime-full",
            "null",
            r#"[{"type":"text","text":"answer"}]"#,
        );
        let mut accepted = initialized();
        admit(&mut accepted, &local_operation(None, "answer")).unwrap();
        for (id, event) in parts {
            admit(&mut accepted, &partial(id, event)).unwrap();
        }
        admit(&mut accepted, &full).unwrap();
        admit(
            &mut accepted,
            &partial("stop", r#"{"type":"message_stop"}"#),
        )
        .unwrap();
        admit(&mut accepted, &runtime_result("answer")).unwrap();
        assert_eq!(accepted.finish().unwrap().response, "answer");

        let mut changed_full = initialized();
        admit(&mut changed_full, &local_operation(None, "other")).unwrap();
        for (id, event) in parts {
            admit(&mut changed_full, &partial(id, event)).unwrap();
        }
        let forged = assistant(
            "runtime-full",
            "null",
            r#"[{"type":"text","text":"other"}]"#,
        );
        assert!(admit(&mut changed_full, &forged)
            .unwrap_err()
            .to_string()
            .contains("runtime partial text"));

        let mut missing_full = initialized();
        admit(&mut missing_full, &local_operation(None, "answer")).unwrap();
        for (id, event) in parts {
            admit(&mut missing_full, &partial(id, event)).unwrap();
        }
        assert!(admit(
            &mut missing_full,
            &partial("stop", r#"{"type":"message_stop"}"#)
        )
        .unwrap_err()
        .to_string()
        .contains("no full assistant message"));

        let mut changed_result = initialized();
        admit(&mut changed_result, &local_operation(None, "answer")).unwrap();
        admit(&mut changed_result, &full).unwrap();
        assert!(admit(&mut changed_result, &runtime_result("other"))
            .unwrap_err()
            .to_string()
            .contains("terminal result contradicts"));
    }

    #[test]
    fn runtime_output_requires_its_local_operation_receipt() {
        let full = assistant(
            "local-answer",
            "null",
            r#"[{"type":"text","text":"local answer"}]"#,
        );
        let mut missing = initialized();
        assert!(admit(&mut missing, &full)
            .unwrap_err()
            .to_string()
            .contains("without its local operation receipt"));

        let mut altered = initialized();
        admit(&mut altered, &local_operation(None, "different")).unwrap();
        assert!(admit(&mut altered, &full)
            .unwrap_err()
            .to_string()
            .contains("changes the output of local operation"));

        let mut unfinished = initialized();
        admit(&mut unfinished, &local_operation(None, "local answer")).unwrap();
        assert!(admit(&mut unfinished, &runtime_result("local answer"))
            .unwrap_err()
            .to_string()
            .contains("ends before the local operation's assistant output"));
    }

    #[test]
    fn runtime_output_cannot_replace_chat_generation_in_the_same_scope() {
        let rows = fixture();
        let mut reader = owner();
        for row in rows.iter().take(rows.len() - 1) {
            admit(&mut reader, &row.to_string()).unwrap();
        }
        let mut replacement: serde_json::Value = serde_json::from_str(&assistant(
            "runtime-replacement",
            "null",
            r#"[{"type":"text","text":"forged result"}]"#,
        ))
        .unwrap();
        replacement["session_id"] = rows[0]["session_id"].clone();
        assert!(admit(&mut reader, &replacement.to_string())
            .unwrap_err()
            .to_string()
            .contains("runtime assistant output after a chat attempt"));
    }

    #[test]
    fn chat_request_cannot_follow_runtime_assistant_output_in_the_same_scope() {
        let rows = fixture();
        let mut reader = owner();
        for row in rows.iter().take(2) {
            admit(&mut reader, &row.to_string()).unwrap();
        }
        let mut local: serde_json::Value = serde_json::from_str(&assistant(
            "runtime-first",
            "null",
            r#"[{"type":"text","text":"local answer"}]"#,
        ))
        .unwrap();
        local["session_id"] = rows[0]["session_id"].clone();
        let mut receipt: serde_json::Value =
            serde_json::from_str(&local_operation(None, "local answer")).unwrap();
        receipt["session_id"] = rows[0]["session_id"].clone();
        admit(&mut reader, &receipt.to_string()).unwrap();
        admit(&mut reader, &local.to_string()).unwrap();
        assert!(admit(&mut reader, &rows[2].to_string())
            .unwrap_err()
            .to_string()
            .contains("chat attempt after runtime assistant output"));
    }

    #[test]
    fn utility_work_can_precede_a_local_runtime_answer() {
        let mut reader = initialized();
        utility_transport(&mut reader, "served", 1, 0);
        admit(&mut reader, &outcome("served", "0")).unwrap();
        admit(&mut reader, &local_operation(None, "local answer")).unwrap();
        admit(
            &mut reader,
            &assistant(
                "local-summary",
                "null",
                r#"[{"type":"text","text":"local answer"}]"#,
            ),
        )
        .unwrap();
    }

    #[test]
    fn successful_model_result_requires_the_last_accepted_conversation_text() {
        let mut accepted = owner();
        for row in fixture() {
            admit(&mut accepted, &row.to_string()).unwrap();
        }
        assert_eq!(accepted.finish().unwrap().response, "before  after");

        let mut changed = fixture();
        changed.last_mut().unwrap()["result"] = serde_json::json!("forged terminal");
        let mut owner = owner();
        for row in changed.iter().take(changed.len() - 1) {
            admit(&mut owner, &row.to_string()).unwrap();
        }
        assert!(admit(&mut owner, &changed.last().unwrap().to_string())
            .unwrap_err()
            .to_string()
            .contains("successful result contradicts accepted model text"));
    }

    #[test]
    fn runtime_presentation_cannot_issue_tools_or_clear_partial_state() {
        let mut owner = initialized();
        admit(&mut owner, &local_operation(None, "")).unwrap();
        for (id, event) in [
            (
                "start",
                r#"{"type":"message_start","message":{"id":"message","role":"assistant","model":"model","content":[]}}"#,
            ),
            (
                "block",
                r#"{"type":"content_block_start","index":0.0,"content_block":{"type":"text","text":""}}"#,
            ),
            ("stop", r#"{"type":"content_block_stop","index":0e0}"#),
        ] {
            admit(&mut owner, &partial(id, event)).unwrap();
        }
        let prefix = owner.prefix;
        let raw = assistant(
            "bad",
            "null",
            r#"[{"type":"tool_use","id":"earlier","name":"tool","input":{}},{"type":"tool_use","id":"later","input":{}}]"#,
        );
        assert!(admit(&mut owner, &raw).is_err());
        assert!(owner.tool_uses.is_empty());
        assert_eq!(owner.prefix, prefix);
        assert!(owner.scope_states[0].partial.finish(1).is_err());
        assert_eq!(
            owner.snapshot().observations.observed_usage,
            GenerationUsageSummary::default()
        );
    }
    #[test]
    fn raw_fractions_cannot_enter_the_integer_partial_domain() {
        for token in [
            "1e-400",
            "0.99999999999999999",
            "9007199254740991.00000000000000001",
        ] {
            let mut owner = initialized();
            admit(&mut owner, &local_operation(None, "")).unwrap();
            admit(&mut owner, &partial("start", r#"{"type":"message_start","message":{"id":"message","role":"assistant","model":"model","content":[]}}"#)).unwrap();
            let prefix = owner.prefix;
            let raw = partial(
                "fraction",
                &format!(
                    r#"{{"type":"content_block_start","index":{token},"content_block":{{"type":"text","text":""}}}}"#
                ),
            );
            assert!(
                admit(&mut owner, &raw)
                    .unwrap_err()
                    .to_string()
                    .contains("violates stream contract"),
                "{token}"
            );
            assert_eq!(owner.prefix, prefix);
        }
    }
    #[test]
    fn mathematical_served_counts_preserve_zero_and_exact_large_domains() {
        for token in ["0", "0.0", "0e999999999999999999999999999999999999"] {
            let mut owner = initialized();
            utility_transport(&mut owner, "served", 1, 0);
            admit(&mut owner, &outcome("served", token)).unwrap();
            assert_eq!(owner.observations.num_turns, None);
            assert_eq!(owner.observations.observed_usage.usage_reports, 1);
            assert_eq!(owner.observations.observed_usage.usage.unwrap().output, 0);
        }
        for token in [
            "1e-400",
            "0.99999999999999999",
            "9007199254740991.00000000000000001",
            "9007199254740992",
            "1e1000001",
        ] {
            let mut owner = initialized();
            utility_transport(&mut owner, "bad", 1, 0);
            assert!(
                admit(&mut owner, &outcome("bad", token)).is_err(),
                "{token}"
            );
            assert_eq!(owner.observations.num_turns, None);
            assert_eq!(owner.observations.observed_usage.usage, None);
            assert_eq!(owner.observations.observed_usage.unfinalized_requests, 1);
            assert_eq!(owner.observations.observed_unaccounted_records, 1);
        }
    }
    #[test]
    fn observations_after_refusal_cannot_restore_a_certificate_or_double_bill_identity() {
        let mut owner = initialized();
        utility_transport(&mut owner, "served", 1, 1);
        let prefix = owner.prefix;
        assert!(admit(&mut owner, "{broken").is_err());
        let raw = outcome("served", "1.0");
        assert!(admit(&mut owner, &raw).is_err());
        assert_eq!(owner.observations.observed_usage.usage.unwrap().output, 1);
        assert!(admit(&mut owner, &raw)
            .unwrap_err()
            .to_string()
            .contains("repeats event uuid"));
        let repeated = raw.replace("response-served-4", "another-outcome");
        assert!(admit(&mut owner, &repeated).is_err());
        assert_eq!(owner.observations.observed_usage.usage.unwrap().output, 1);
        assert_eq!(owner.observations.observed_unaccounted_records, 3);
        assert_eq!(owner.prefix, prefix);
        assert!(owner.finish().is_err());
    }
    #[test]
    fn an_outcome_without_its_request_does_not_invent_ownership() {
        let mut owner = initialized();
        assert!(admit(&mut owner, &outcome("missing", "4")).is_err());
        assert_eq!(
            owner.observations.observed_usage,
            GenerationUsageSummary::default()
        );
        assert_eq!(owner.observations.observed_unaccounted_records, 1);
    }
    #[test]
    fn wrong_owner_and_returned_tools_cannot_receive_results_or_progress() {
        let event = |owner: &RuntimeContract, scope: Option<&str>, id: &str| {
            serde_json::json!({
            "type":"user", "uuid":id, "session_id":owner.session_id,
            "parent_tool_use_id":scope,
            "message":{"content":[{"type":"tool_result","tool_use_id":"provider__qwen_dup_2","content":"done"}]}
        }).to_string()
        };
        let mut owner = issued_owner();
        let wrong = event(&owner, Some("provider__qwen_dup_2"), "wrong");
        assert!(admit(&mut owner, &wrong)
            .unwrap_err()
            .to_string()
            .contains("wrong-owner"));
        assert!(!owner.tool_uses["provider__qwen_dup_2"].returned);
        let mut owner = issued_owner();
        let result = event(&owner, None, "result");
        admit(&mut owner, &result).unwrap();
        assert!(owner.tool_uses["provider__qwen_dup_2"].returned);
        let late = event(&owner, Some("provider__qwen_dup_2"), "late");
        assert!(admit(&mut owner, &late)
            .unwrap_err()
            .to_string()
            .contains("returned tool"));
    }
    #[test]
    fn overflow_does_not_publish_a_partially_updated_usage_population() {
        let mut owner = initialized();
        utility_transport(&mut owner, "largest", 1, SAFE_INTEGER);
        admit(&mut owner, &outcome("largest", &SAFE_INTEGER.to_string())).unwrap();
        utility_transport(&mut owner, "overflow", 2, 1);
        let before = owner.observations.observed_usage;
        assert!(admit(&mut owner, &outcome("overflow", "1")).is_err());
        assert_eq!(owner.observations.observed_usage, before);
        assert_eq!(owner.observations.observed_unaccounted_records, 1);
        assert!(owner
            .first_refusal
            .as_ref()
            .unwrap()
            .to_string()
            .contains("exact integer range"));
    }
}
