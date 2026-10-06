//! One pure owner for stream identity, semantic admission and independent
//! observations. Physical capture, storage and process receipts remain external.
use crate::{
    generation::{DisplayMessage, Generation, Origin, OutputScope},
    model_requests::{Disposition, DrawKind},
    partial_stream::OutputOrigin,
    json::{Document, Limits, Value},
    schema::ValidationLimits,
    stream::{field, text, unsigned},
    usage::{GenerationUsageSummary, ServedUsage},
    ContractError, ContractResult, DecodedRecord, EventKind, PartialStreamState, SystemKind,
    SAFE_INTEGER, STREAM_CONTRACT_SHA256,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
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
    /// The declared deliverables the run ended without, in declared order:
    /// present exactly when `subtype` is `error_missing_deliverables`, and
    /// then never empty, so a reader lists them without parsing `response`.
    pub missing_deliverables: Option<Vec<String>>,
}

// Public terminal vocabulary is generated from the same schema used by the producer.
pub use crate::{terminal_exit_code, ERROR_SUBTYPES, MISSING_DELIVERABLES_SUBTYPE, SUCCESS_SUBTYPE};

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
/// fails the acceptance rule, and issues a draw's request again when it failed
/// in transport before it produced an answer. `rejectedAttempts` holds the
/// draws that did not settle it before the one `output` describes, oldest
/// first, and is present whatever `output` is: a draw that was refused and
/// billed, or broke, must not disappear behind a later draw that never
/// generated. Each carries the accounting `output` carries, minus the
/// transition-wide budget, plus the rule it failed or the transport fault it
/// ended on, and its physical request is held to which of the two it names.
fn validate_compaction_event(
    object: Value<'_>,
    line: usize,
    scope: Option<&str>,
    json_limits: Limits,
    requests: &crate::model_requests::ModelRequests,
    kv_scope: &str,
) -> ContractResult<Vec<(String, crate::authorship::DrawOutput)>> {
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
            | "COMPRESSION_FAILED_TRANSPORT_ERROR"
            | "COMPRESSION_FAILED_RECORDING_STOPPED"
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
    // A token count is a function of the body counted, so a measurement
    // cites the completed tokenizer operation that counted its body, whenever
    // it ran: one operation may answer several roles and several records.
    let measurements = field(record, "tokenMeasurements", line)?
        .elements()
        .ok_or_else(|| refuse("without a tokenMeasurements array"))?
        .map(|measurement| {
            let role = text(measurement, "role", line)?.to_string();
            let id = text(measurement, "operationId", line)?;
            let (count, window, model, first_sequence, last_sequence) =
                requests.check_token_measurement(id, kv_scope)?;
            Ok((role, count, window, model, first_sequence, last_sequence))
        })
        .collect::<ContractResult<Vec<_>>>()?;
    if measurements
        .first()
        .is_some_and(|first| measurements.iter().any(|value| value.2 != first.2))
    {
        return Err(refuse(
            "whose tokenizer measurements disagree about the served model window",
        ));
    }
    if measurements
        .first()
        .is_some_and(|first| measurements.iter().any(|value| value.3 != first.3))
    {
        return Err(refuse("whose tokenizer measurements use different models"));
    }
    for (index, measurement) in measurements.iter().take(3).enumerate() {
        if measurement.0 != ["original", "summary_request", "prompt_only"][index] {
            return Err(refuse(
                "whose tokenizer preflight measurements are missing or reordered",
            ));
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
        let mut has_sdk_value = false;
        for response in responses {
            has_sdk_value = true;
            let raw = response.as_str().ok_or_else(|| {
                refuse("with a non-string SDK value; inspect the compaction producer")
            })?;
            Document::decode(raw.as_bytes(), json_limits).map_err(|cause| {
                refuse(&format!(
                    "with undecodable SDK value JSON ({cause:?}); inspect the captured provider value"
                ))
            })?;
        }
        if !has_sdk_value
            && (holder.get("text").and_then(Value::as_str) != Some("")
                || holder.get("reasoning").and_then(Value::as_str) != Some("")
                || holder
                    .get("functionCalls")
                    .and_then(Value::elements)
                    .is_none_or(|mut calls| calls.next().is_some())
                || holder
                    .get("incompleteToolCalls")
                    .and_then(Value::elements)
                    .is_none_or(|mut calls| calls.next().is_some())
                || !holder.get("finishReason").is_some_and(Value::is_null)
                || !holder.get("usage").is_some_and(Value::is_null)
                || !holder.get("newTokenCount").is_some_and(Value::is_null)
                || !holder.get("snapshotBytes").is_some_and(Value::is_null))
        {
            return Err(refuse(&format!(
                "whose {whose} claims decoded output without an SDK value"
            )));
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
        if !attempt
            .get("status")
            .and_then(Value::as_str)
            .is_some_and(|status| status != "COMPRESSED" && draw_kind(status) != DrawKind::Ended)
        {
            return Err(refuse(&format!(
                "whose {whose} names neither a rule its answer failed nor a transport fault; retain the original stream and recapture with a corrected compaction producer"
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
            return Err(refuse(
                "whose drawn candidate has no measured original and preflight counts",
            ));
        }
        let mut next = 3;
        for measured in draw_counts.iter().copied() {
            if let Some(measured) = measured {
                if !measurements
                    .get(next)
                    .is_some_and(|count| count.0 == "candidate" && count.1 == measured)
                {
                    return Err(refuse(
                        "whose candidate count differs from the served tokenizer response",
                    ));
                }
                next += 1;
            }
        }
        if next != measurements.len() {
            return Err(refuse("with an unclaimed tokenizer measurement"));
        }
    } else if measurements.len() > 3
        || measurements
            .first()
            .is_some_and(|measured| measured.1 != original_tokens)
    {
        return Err(refuse(
            "whose preflight count differs from the served tokenizer response",
        ));
    }
    if succeeded {
        let output = field(record, "output", line)?;
        if output.is_null()
            || field(output, "newTokenCount", line)?.is_null()
            || count(output, "newTokenCount")? != count(record, "newTokenCount")?
        {
            return Err(refuse(
                "whose replacement count differs from its accepted draw",
            ));
        }
    }
    let mut claims = Vec::new();
    let mut unique = BTreeSet::new();
    let draws = field(record, "rejectedAttempts", line)?
        .elements()
        .ok_or_else(|| refuse("without a rejectedAttempts array"))?
        .chain(record.get("output").filter(|value| !value.is_null()));
    let mut physical_budget = None;
    let mut previous_last_sequence = None;
    let mut draw_ranges = Vec::new();
    for draw in draws {
        let id = text(draw, "operationId", line)?;
        if !unique.insert(id.to_string()) {
            return Err(refuse(
                "whose candidates repeat a physical operation identity",
            ));
        }
        let physical_requests = count(draw, "physicalRequests")?;
        // A rejected attempt is named by its own status, the draw `output`
        // describes by the record's.
        let kind = draw_kind(draw.get("status").and_then(Value::as_str).unwrap_or(status));
        let (issued, first_sequence, last_sequence, output) = requests.check_compaction_draw(
            id,
            kv_scope,
            &measurements[0].3,
            physical_requests,
            draw,
            kind,
        )?;
        let served = field(draw, "usage", line)?;
        if !served.is_null() && count(served, "candidatesTokenCount")? > issued {
            return Err(refuse(
                "whose draw reports more served output than its physical ceiling",
            ));
        }
        if budget.is_some_and(|budget| budget != issued)
            || physical_budget.is_some_and(|budget| budget != issued)
            || previous_last_sequence.is_some_and(|previous| first_sequence <= previous)
        {
            return Err(refuse(
                "whose draws are reordered or disagree with their physical request ceiling",
            ));
        }
        physical_budget = Some(issued);
        previous_last_sequence = Some(last_sequence);
        draw_ranges.push((first_sequence, last_sequence));
        claims.push((id.to_string(), output));
    }
    // A draw is issued on the preflight's counts, so it follows every
    // operation they cite, and each draw follows the one before it.
    if !draw_ranges.is_empty() {
        let mut previous = measurements[..3]
            .iter()
            .map(|measurement| measurement.5)
            .max()
            .expect("three preflight measurements");
        for (first, last) in draw_ranges {
            if first <= previous {
                return Err(refuse("whose preflight or draw reorders physical requests"));
            }
            previous = last;
        }
    }
    Ok(claims)
}

/// What a compaction's draw was, read from the status that names it: an
/// answer the rules judged -- accepted, or refused by a rule another draw can
/// satisfy -- a request that failed in transport before it produced one, or a
/// draw the compaction ended on for a cause outside both.
fn draw_kind(status: &str) -> DrawKind {
    match status {
        "COMPRESSED"
        | "COMPRESSION_FAILED_INFLATED_TOKEN_COUNT"
        | "COMPRESSION_FAILED_EMPTY_SUMMARY"
        | "COMPRESSION_FAILED_OUTPUT_TRUNCATED"
        | "COMPRESSION_FAILED_INSUFFICIENT_ROOM"
        | "COMPRESSION_FAILED_SUMMARY_OVER_BOUND" => DrawKind::Answer,
        "COMPRESSION_FAILED_TRANSPORT_ERROR" => DrawKind::Fault,
        _ => DrawKind::Ended,
    }
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
    missing_deliverables: Option<Vec<String>>,
}
#[derive(Clone, Default)]
struct ScopeState {
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
/// The assistant messages a settled turn owes before any other record.
#[derive(Clone)]
struct OwedDisplay {
    scope: Option<String>,
    model: String,
    messages: VecDeque<DisplayMessage>,
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
    utility_request: Option<crate::model_requests::UtilityRequestAdmission>,
    utility_completion: Option<crate::model_requests::UtilityCompletionAdmission>,
    response: Option<crate::model_requests::ResponseAdmission>,
    seed: Option<crate::model_requests::SeedAdmission>,
    generation: Option<crate::model_requests::GenerationAdmission>,
    completion: Option<crate::model_requests::CompletionAdmission>,
    compaction_claims: Vec<(String, crate::authorship::DrawOutput)>,
    row: usize,
    state: ScopeState,
    additions: BTreeMap<String, ToolUse>,
    returns: BTreeSet<String>,
    runtime_operation_id: Option<String>,
    owed: Option<OwedDisplay>,
    partial_origin: Option<(Option<String>, OutputOrigin)>,
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
    /// The task the session was started with, which runs of operator text
    /// are held to.
    operator: crate::authorship::OperatorTask,
    session_id: Option<String>,
    runtime_initialized: bool,
    seen_uuids: BTreeSet<String>,
    observed_scopes: BTreeSet<String>,
    observed_journal: Option<String>,
    observed_requests: BTreeMap<String, bool>,
    observed_utility_requests: BTreeSet<String>,
    records_seen: u64,
    prefix: u64,
    first_refusal: Option<ContractError>,
    observations: RuntimeObservations,
    scope_states: Vec<ScopeState>,
    scope_rows: BTreeMap<String, usize>,
    tool_uses: BTreeMap<String, ToolUse>,
    runtime_operation_ids: BTreeSet<String>,
    owed: Option<OwedDisplay>,
    partial_origins: BTreeMap<Option<String>, OutputOrigin>,
    pending: Option<PendingAdmission>,
}
impl RuntimeContract {
    pub fn new(
        bindings: RuntimeBindings,
        limits: RuntimeLimits,
        operator: crate::authorship::OperatorTask,
    ) -> Self {
        Self {
            requests: crate::model_requests::ModelRequests::default(),
            identity: std::sync::Arc::new(()),
            bindings,
            limits,
            operator,
            session_id: None,
            runtime_initialized: false,
            seen_uuids: BTreeSet::new(),
            observed_scopes: BTreeSet::new(),
            observed_journal: None,
            observed_requests: BTreeMap::new(),
            observed_utility_requests: BTreeSet::new(),
            records_seen: 0,
            prefix: 0,
            first_refusal: None,
            observations: RuntimeObservations::default(),
            scope_states: vec![ScopeState::default()],
            scope_rows: BTreeMap::new(),
            tool_uses: BTreeMap::new(),
            runtime_operation_ids: BTreeSet::new(),
            owed: None,
            partial_origins: BTreeMap::new(),
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
        let mut utility_request = None;
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
            "model_utility_request" => {
                let evidence = field(object, "utility_request", line)?;
                let id = text(evidence, "request_id", line)?;
                if self.observed_journal.as_deref() != Some(text(evidence, "journal_id", line)?)
                    || self.observed_requests.contains_key(id)
                {
                    return Err(refuse(
                        "utility request has a foreign journal or repeated identity",
                    ));
                }
                request = Some(id.to_string());
                utility_request = Some(id.to_string());
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
                    if !self.observed_utility_requests.contains(id) {
                        observations.observed_usage =
                            observations.observed_usage.finalize(if usage.is_null() {
                                None
                            } else {
                                Some(ServedUsage::read(usage, line)?)
                            })?;
                    }
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
        if let Some(id) = utility_request {
            self.observed_utility_requests.insert(id);
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
        if let Some(completion) = plan.utility_completion {
            self.requests.commit_utility_completion(completion);
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
        self.requests
            .commit_compaction_claims(plan.compaction_claims);
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
        if let Some(request) = plan.utility_request {
            self.requests.commit_utility_request(request);
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
        self.owed = plan.owed;
        if let Some((scope, origin)) = plan.partial_origin {
            self.partial_origins.insert(scope, origin);
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
        // A settled turn owes exactly its assistant messages before any other
        // record; the messages derive from its generation, so a shown turn and
        // its generation cannot disagree.
        let mut owed = self.owed.clone();
        let mut shown_turn = false;
        if let Some(pending) = &mut owed {
            if record.kind() != EventKind::Assistant || scope != pending.scope.as_deref() {
                return Err(ContractError::InvalidRecord(format!(
                    "events.jsonl line {line} precedes the assistant messages its settled turn in {} shows; retain the complete original stream",
                    scope_display(pending.scope.as_deref())
                )));
            }
            let expected = pending
                .messages
                .pop_front()
                .expect("an owed turn has a message");
            self.require_shown_message(object, &expected, &pending.model, line)?;
            if pending.messages.is_empty() {
                owed = None;
            }
            shown_turn = true;
        }
        let scope_key = scope.map(str::to_string);
        let mut partial_origin = None;
        let mut request = None;
        let mut utility_request = None;
        let mut utility_completion = None;
        let mut response = None;
        let mut seed = None;
        let mut generation = None;
        let mut completion = None;
        let mut compaction_claims = Vec::new();
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
                let admission =
                    self.requests
                        .plan(object, line, self.limits.json, &self.operator)?;
                if let Some(attempt) = admission.attempt() {
                    // The display scope a chat request renders for: the root for
                    // the session's own scope, a subagent scope for the call that
                    // spawned it; any other scope is internal and shows nothing.
                    let kv_scope = admission.scope();
                    let display = if Some(kv_scope) == self.session_id.as_deref() {
                        Some(None)
                    } else if self.tool_uses.contains_key(kv_scope) {
                        Some(Some(kv_scope.to_string()))
                    } else {
                        None
                    };
                    if let Some(display) = display {
                        if display == scope_key && state.runtime_text.is_some() {
                            return Err(ContractError::InvalidRecord(format!(
                                "events.jsonl line {line} dispatches a chat attempt after runtime assistant output in the same scope; inspect the original local result and request"
                            )));
                        }
                        if let Some((id, _)) = self
                            .tool_uses
                            .iter()
                            .find(|(_, tool)| tool.scope == display && !tool.returned)
                        {
                            return Err(ContractError::InvalidRecord(format!(
                                "events.jsonl line {line} sends the next request in {} before any row reports that the tool for call {id:?} returned; retain the complete original stream",
                                scope_display(display.as_deref())
                            )));
                        }
                        partial_origin = Some((
                            display,
                            OutputOrigin::Model(Origin {
                                attempt: attempt.to_string(),
                                scope: kv_scope.to_string(),
                            }),
                        ));
                    }
                }
                request = Some(admission);
            }
            EventKind::ModelUtilityRequest => {
                if !runtime_initialized {
                    return Err(ContractError::InvalidRecord(format!(
                        "events.jsonl line {line} dispatches utility work without runtime init"
                    )));
                }
                utility_request = Some(self.requests.plan_utility_request(
                    object,
                    line,
                    self.limits.json,
                )?);
            }
            EventKind::ModelUtilityCompletion => {
                utility_completion = Some(self.requests.plan_utility_completion(object, line)?);
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
                    let accepted = admission.disposition == Disposition::Accepted;
                    state
                        .partial
                        .observe_completion(&admission.generation.origin, accepted)?;
                    if admission.disposition != Disposition::Abandoned {
                        owed = Some(OwedDisplay {
                            scope: scope_key.clone(),
                            model: admission.generation.model.clone(),
                            messages: admission
                                .generation
                                .display(admission.disposition == Disposition::Refused)?
                                .into(),
                        });
                    }
                    if accepted {
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
            EventKind::Assistant if shown_turn => {}
            EventKind::Assistant => {
                if self
                    .requests
                    .has_chat_attempt_in_scope(scope.unwrap_or(text(object, "session_id", line)?))
                {
                    return Err(ContractError::InvalidRecord(format!(
                        "events.jsonl line {line} claims runtime assistant output after a chat attempt in the same scope; inspect the original generation and its output"
                    )));
                }
                let message = field(object, "message", line)?;
                if !field(message, "stop_reason", line)?.is_null()
                    || !field(message, "usage", line)?.is_null()
                {
                    return Err(ContractError::InvalidRecord(format!(
                        "events.jsonl line {line} claims model stop or usage on runtime assistant output; inspect its producing operation"
                    )));
                }
                let content = field(message, "content", line)?
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
            // A user row reports the tool calls its scope's tools returned:
            // which call, and whether it failed. What the model was given for
            // each is in the next request's body, which is the one record of
            // the model's input.
            EventKind::User => {
                let blocks = field(field(object, "message", line)?, "content", line)?
                    .elements()
                    .ok_or_else(|| {
                        ContractError::InvalidRecord(format!(
                            "events.jsonl line {line} has user content that is not an array"
                        ))
                    })?;
                for block in blocks {
                    let id = text(block, "tool_use_id", line)?;
                    self.require_tool_owner(id, scope, line)?;
                    if block.get("is_error").and_then(Value::as_bool) == Some(false) {
                        let tool = self.tool_uses.get(id).expect("checked tool owner");
                        if tool.name == "structured_output" && state.structured_input.is_none() {
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
            EventKind::StreamEvent => {
                if let Some(origin) = self.partial_origins.get(&scope_key) {
                    state.partial.set_origin(origin.clone(), line)?;
                }
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
                    SystemKind::Compaction => {
                        let kv_scope = scope.or(self.session_id.as_deref()).ok_or_else(|| {
                            ContractError::InvalidRecord(format!("events.jsonl line {line} has no compaction request scope; retain the complete stream_start and request evidence"))
                        })?;
                        compaction_claims = validate_compaction_event(
                            object, line, scope, self.limits.json, &self.requests, kv_scope,
                        )?;
                    },
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
                        state.partial.set_origin(OutputOrigin::Runtime, line)?;
                        partial_origin = Some((scope_key.clone(), OutputOrigin::Runtime));
                    },
                    // A stopped recording is named by its reason and the client's own words
                    // for it; a refusal's record travels in the data and is not repeated here.
                    SystemKind::SessionRecordingDegraded => {
                        let data = field(object, "data", line)?;
                        return Err(ContractError::InvalidRecord(format!(
                            "events.jsonl line {line} reports operational failure {} ({}): {}",
                            subtype.wire(),
                            text(data, "reason", line)?,
                            text(data, "message", line)?,
                        )));
                    },
                    SystemKind::TurnCleanupFailed | SystemKind::VisionBridgeFailed => return Err(ContractError::InvalidRecord(format!("events.jsonl line {line} reports operational failure {}: {}", subtype.wire(), field(object, "data", line)?.raw()))),
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
            utility_request,
            utility_completion,
            response,
            seed,
            generation,
            completion,
            compaction_claims,
            row,
            state,
            additions,
            returns,
            runtime_operation_id,
            owed,
            partial_origin,
            prefix: add(self.prefix, 1, "certified prefix")?,
        })
    }
    /// One assistant message of a settled turn, exactly as its generation
    /// shows it; only the message identity is the writer's.
    fn require_shown_message(
        &self,
        object: Value<'_>,
        expected: &DisplayMessage,
        model: &str,
        line: usize,
    ) -> ContractResult<()> {
        let refuse = |what: &str| {
            ContractError::InvalidRecord(format!(
                "events.jsonl line {line} shows a settled turn with {what} its generation does not have; the assistant messages derive from the generation record"
            ))
        };
        let message = field(object, "message", line)?;
        if text(message, "model", line)? != model {
            return Err(refuse("a model"));
        }
        let stop = field(message, "stop_reason", line)?;
        let stop_ok = if expected.tool_use_only {
            stop.as_str() == Some("tool_use")
        } else {
            stop.is_null()
        };
        if !stop_ok {
            return Err(refuse("a stop reason"));
        }
        let shown = Document::decode(expected.content_json.as_bytes(), self.limits.json)
            .map_err(|cause| decode_failure(cause, line))?;
        if field(message, "content", line)? != shown.root() {
            return Err(refuse("content"));
        }
        let usage = field(message, "usage", line)?;
        match &expected.usage {
            None if usage.is_null() => {}
            Some(served) if !usage.is_null() => {
                for (key, value) in [
                    ("input_tokens", served.prompt),
                    ("output_tokens", served.output),
                    ("cache_read_input_tokens", served.cached),
                    ("reasoning_output_tokens", served.thoughts),
                    ("total_tokens", served.total),
                ] {
                    if unsigned(field(usage, key, line)?, key, SAFE_INTEGER)? != value {
                        return Err(refuse("usage"));
                    }
                }
            }
            _ => return Err(refuse("usage")),
        }
        Ok(())
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
        if let Some(owed) = &self.owed {
            return Err(ContractError::InvalidRecord(format!(
                "events.jsonl ends before the assistant messages of a settled turn in {}; retain the complete original stream",
                scope_display(owed.scope.as_deref())
            )));
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
            missing_deliverables: terminal.missing_deliverables.clone(),
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
    // Only the session's own run declares deliverables, so only its terminal
    // can name the ones it ended without; the list is the record's, never
    // recovered from the message that also names them.
    let missing_deliverables = if subtype == MISSING_DELIVERABLES_SUBTYPE {
        if !root {
            return Err(ContractError::InvalidRecord(format!(
                "a subagent result names {MISSING_DELIVERABLES_SUBTYPE}, which only the session's own terminal can report"
            )));
        }
        let list = field(object, "missing_deliverables", line)?
            .elements()
            .ok_or_else(|| {
                ContractError::InvalidRecord(
                    "missing_deliverables must be an array of paths".into(),
                )
            })?
            .map(|path| {
                path.as_str().filter(|path| !path.is_empty()).map(str::to_string).ok_or_else(|| {
                    ContractError::InvalidRecord(
                        "missing_deliverables holds a path that is not a non-empty string".into(),
                    )
                })
            })
            .collect::<ContractResult<Vec<_>>>()?;
        let distinct = list.iter().collect::<std::collections::BTreeSet<_>>();
        if list.is_empty() || distinct.len() != list.len() {
            return Err(ContractError::InvalidRecord(
                "missing_deliverables must name at least one path, each once".into(),
            ));
        }
        Some(list)
    } else {
        if object.get("missing_deliverables").is_some() {
            return Err(ContractError::InvalidRecord(format!(
                "a {subtype} result carries missing_deliverables"
            )));
        }
        None
    };
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
        missing_deliverables,
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
        RuntimeContract::new(
            RuntimeBindings::new(document).unwrap(),
            limits,
            // The task the shared wire stream's session was started with.
            crate::authorship::OperatorTask::new(b"work"),
        )
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
    fn compaction_provider_value() -> String {
        let sections = compaction_sections();
        serde_json::json!({"choices":[{"delta":{"tool_calls":[{
            "index":0,"id":"snapshot-call","type":"function",
            "function":{"name":"state_snapshot","arguments":sections.to_string()}
        }]},"finish_reason":"tool_calls"}],
            "usage":{"prompt_tokens":24,"completion_tokens":4,"total_tokens":28,
                "prompt_tokens_details":{"cached_tokens":0},
                "completion_tokens_details":{"reasoning_tokens":0}}})
        .to_string()
    }
    fn compaction_sections() -> serde_json::Value {
        serde_json::json!({
            "primary_request_and_intent":"None", "key_technical_concepts":"None",
            "files_and_code_sections":"None", "errors_and_fixes":"None",
            "problem_solving":"None", "pending_tasks":"None", "current_work":"None",
            "next_step":"None"
        })
    }
    fn compaction_decoded() -> serde_json::Value {
        serde_json::json!({
            "response":{
                "candidates":[{"content":{"parts":[{"functionCall":{
                    "id":"snapshot-call","name":"state_snapshot","args":compaction_sections()
                }}],"role":"model"},"index":0,"safetyRatings":[],"finishReason":"STOP"}],
                "usageMetadata":{"promptTokenCount":24,"candidatesTokenCount":4,
                    "thoughtsTokenCount":0,"cachedContentTokenCount":0,"totalTokenCount":28}
            },
            "incomplete_tool_calls":[],
            "tool_call_preparations":[{"callId":"snapshot-call","toolName":"state_snapshot"}]
        })
    }
    /// One completed chat tokenizer operation: its request, physical
    /// response and completion records.
    fn tokenizer_records(role: &str, sequence: u64, count: u64, model: &str) -> Vec<String> {
        let operation_id = format!("token-{role}");
        let request_id = format!("token-request-{role}");
        let body = serde_json::json!({"model":model,"messages":[],"add_generation_prompt":true})
            .to_string();
        let request = serde_json::json!({"type":"model_utility_request","uuid":format!("{operation_id}-request"),
            "session_id":"session","parent_tool_use_id":null,
            "utility_request":{"journal_id":"fixture","sequence":sequence,"request_id":request_id,
                "operation_id":operation_id,"kv_scope":"session","kind":"tokenize_chat",
                "requested_model":model,"requested_input_count":null,"expected_max_model_len":100,
                "request_url":"http://fixture.invalid/tokenize","body_json":body,
                "body_bytes":body.len(),"body_sha256":crate::generation::sha256(body.as_bytes())}});
        let mut records = vec![request.to_string()];
        let result = serde_json::json!({"count":count,"max_model_len":100}).to_string();
        let events = [
            serde_json::json!({"kind":"http","status":200,"content_type":"application/json"}),
            serde_json::json!({"kind":"body","offset":0,"base64":STANDARD.encode(result.as_bytes())}),
            serde_json::json!({"kind":"end","termination":"eof","body_bytes":result.len(),
                "body_sha256":crate::generation::sha256(result.as_bytes()),"error":null}),
            serde_json::json!({"kind":"outcome","status":"completed","error":null,
                "served_usage":null,"sdk_values_seen":1,"pipeline_outputs_delivered":0}),
            serde_json::json!({"kind":"delivery","outputs_delivered":0}),
        ];
        for (index, event) in events.iter().enumerate() {
            records.push(response(&request_id, (index + 1) as u64, &event.to_string()));
        }
        let completion = serde_json::json!({"type":"model_utility_completion",
            "uuid":format!("{operation_id}-completion"),"session_id":"session","parent_tool_use_id":null,
            "utility_completion":{"journal_id":"fixture","operation_id":operation_id,
                "kv_scope":"session","kind":"tokenize_chat","requested_model":model,
                "requested_input_count":null,"expected_max_model_len":100,"request_ids":[request_id],
                "result":{"kind":"token_count","total_tokens":count,"max_model_len":100},"error":null}});
        records.push(completion.to_string());
        records
    }
    fn record_compaction_tokenizer(
        owner: &mut RuntimeContract,
        role: &str,
        sequence: u64,
        count: u64,
        model: &str,
    ) {
        for record in tokenizer_records(role, sequence, count, model) {
            admit(owner, &record).unwrap();
        }
    }
    fn with_compaction_transport() -> RuntimeContract {
        let bytes = format!("data: {}\n\ndata: [DONE]\n\n", compaction_provider_value());
        with_compaction_transport_response(bytes.as_bytes(), 1, true)
    }
    fn with_compaction_transport_response(
        bytes: &[u8],
        sdk_values_seen: u64,
        completed: bool,
    ) -> RuntimeContract {
        with_compaction_transport_delivery(bytes, sdk_values_seen, completed, u64::from(completed))
    }
    fn with_compaction_transport_delivery(
        bytes: &[u8],
        sdk_values_seen: u64,
        completed: bool,
        delivered: u64,
    ) -> RuntimeContract {
        with_compaction_transport_delivery_models(
            bytes,
            sdk_values_seen,
            completed,
            delivered,
            ["fixture-model"; 4],
        )
    }
    fn with_compaction_transport_delivery_models(
        bytes: &[u8],
        sdk_values_seen: u64,
        completed: bool,
        delivered: u64,
        models: [&str; 4],
    ) -> RuntimeContract {
        let mut owner = initialized();
        record_compaction_tokenizer(&mut owner, "original", 1, 24, models[0]);
        record_compaction_tokenizer(&mut owner, "summary_request", 2, 20, models[1]);
        record_compaction_tokenizer(&mut owner, "prompt_only", 3, 18, models[2]);
        let body = serde_json::json!({
            "kv_scope":"session", "model":"fixture-model", "stream":true,
            "max_tokens":8,
            "messages":[{"role":"user","content":
                "your answer may generate at most 8 tokens, reasoning included. If all draws are refused, this conversation cannot continue."}]
        }).to_string();
        let body_bytes = body.len();
        let body_hash = crate::generation::sha256(body.as_bytes());
        let request = serde_json::json!({
            "type":"model_request", "uuid":"compaction-request", "session_id":"session", "parent_tool_use_id":null,
            "request":{
                "journal_id":"fixture", "request_id":"compaction-physical", "sequence":4,
                "kv_scope":"session", "segment_id":"compaction-segment", "prompt_id":"compaction-prompt",
                "owner":{"kind":"utility","operation_id":"compaction-operation","purpose":"compaction"},
                "body":{"kind":"full","json":body,"authors":[{"author":"harness","bytes":body.len()}]},
                "decode_policy":{"mode":"stream","model":"fixture-model","strict_tool_calling":true,
                    "named_tool_choice":null,"exact_token_counting":true,"tagged_thinking_tags":false},
                "body_bytes":body_bytes, "body_sha256":body_hash
            }
        });
        admit(&mut owner, &request.to_string()).unwrap();
        let mut sequence = 0;
        let mut next = |owner: &mut RuntimeContract, event: serde_json::Value| {
            sequence += 1;
            admit(
                owner,
                &response("compaction-physical", sequence, &event.to_string()),
            )
            .unwrap();
        };
        next(
            &mut owner,
            serde_json::json!({"kind":"http","status":200,"content_type":"text/event-stream"}),
        );
        // A response that delivered no bytes has no body chunk.
        if !bytes.is_empty() {
            next(
                &mut owner,
                serde_json::json!({"kind":"body","offset":0,"base64":STANDARD.encode(bytes)}),
            );
        }
        next(
            &mut owner,
            serde_json::json!({"kind":"end","termination":"eof","body_bytes":bytes.len(),
                "body_sha256":crate::generation::sha256(bytes),"error":null}),
        );
        if completed {
            let decoded = compaction_decoded().to_string();
            next(
                &mut owner,
                serde_json::json!({"kind":"decoded_body","role":"utility","index":0,
                    "offset":0,"base64":STANDARD.encode(decoded.as_bytes())}),
            );
            next(
                &mut owner,
                serde_json::json!({"kind":"decoded_end","role":"utility","index":0,
                    "body_bytes":decoded.len(),"body_sha256":crate::generation::sha256(decoded.as_bytes())}),
            );
        }
        next(
            &mut owner,
            serde_json::json!({"kind":"outcome",
                "status":if completed { "completed" } else { "failed" },
                "error":if completed { serde_json::Value::Null } else { serde_json::json!("conversion failed") },
                "sdk_values_seen":sdk_values_seen,
                "pipeline_outputs_delivered":if completed { 1 } else { 0 },
                "served_usage":if completed {
                    serde_json::json!({"promptTokenCount":24,"candidatesTokenCount":4,
                        "thoughtsTokenCount":0,"cachedContentTokenCount":0,"totalTokenCount":28})
                } else { serde_json::Value::Null }}),
        );
        next(
            &mut owner,
            serde_json::json!({"kind":"delivery", "outputs_delivered":delivered}),
        );
        if completed {
            record_compaction_tokenizer(&mut owner, "candidate", 5, 12, models[3]);
        }
        owner
    }
    fn compaction_failed() -> serde_json::Value {
        serde_json::json!({
            "type":"system", "subtype":"compaction", "uuid":"compaction",
            "session_id":"session", "parent_tool_use_id":null,
            "data":{
                "status":"COMPRESSION_FAILED_PROTOCOL_ERROR", "succeeded":false,
                "originalTokenCount":24, "newTokenCount":24,
                "triggerReason":null, "postCompactionHistory":null,
                "output":null, "rejectedAttempts":[],
                "tokenMeasurements":[]
            }
        })
    }
    fn compaction_success() -> serde_json::Value {
        let measurement = |role: &str| {
            serde_json::json!({
                "role": role, "operationId": format!("token-{role}")
            })
        };
        let mut success = compaction_failed();
        success["data"]["status"] = serde_json::json!("COMPRESSED");
        success["data"]["succeeded"] = serde_json::json!(true);
        success["data"]["newTokenCount"] = serde_json::json!(12);
        success["data"]["postCompactionHistory"] = serde_json::json!([
            {"role":"user", "parts":[{"text":"retained input"}]}
        ]);
        success["data"]["tokenMeasurements"] = serde_json::json!([
            measurement("original"),
            measurement("summary_request"),
            measurement("prompt_only"),
            measurement("candidate")
        ]);
        success["data"]["output"] = serde_json::json!({
            "maxOutputTokens":8, "physicalRequests":1,
            "operationId":"compaction-operation", "functionCalls":[{
                "id":"snapshot-call","name":"state_snapshot","args":compaction_sections()}],
            "text":"", "reasoning":"", "sdkValuesJson":[compaction_provider_value()],
            "newTokenCount":12, "snapshotBytes":524,
            "incompleteToolCalls":[], "finishReason":"STOP",
            "usage":{"promptTokenCount":24,"candidatesTokenCount":4,
                "thoughtsTokenCount":0,"cachedContentTokenCount":0,
                "totalTokenCount":28}
        });
        success
    }
    #[test]
    fn compaction_status_and_retained_count_agree_with_the_history_decision() {
        let failed = compaction_failed();
        admit(&mut initialized(), &failed.to_string()).unwrap();
        let success = compaction_success();
        admit(&mut with_compaction_transport(), &success.to_string()).unwrap();
        let mut forged_decoded_call = success.clone();
        forged_decoded_call["data"]["output"]["functionCalls"] = serde_json::json!([]);
        let error = admit(
            &mut with_compaction_transport(),
            &forged_decoded_call.to_string(),
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("differs from its recorded delivered output"));
        let bytes = format!("data: {}\n\ndata: [DONE]\n\n", compaction_provider_value());
        let error = admit(
            &mut with_compaction_transport_delivery(bytes.as_bytes(), 1, true, 0),
            &success.to_string(),
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("claims an answer its physical request did not complete and deliver"));
        let bytes = format!("data: {}\n\ndata: [DONE]\n\n", compaction_provider_value());
        let error = admit(
            &mut with_compaction_transport_delivery_models(
                bytes.as_bytes(),
                1,
                true,
                1,
                [
                    "another-model",
                    "fixture-model",
                    "fixture-model",
                    "fixture-model",
                ],
            ),
            &success.to_string(),
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("tokenizer measurements use different models"));
        let error = admit(
            &mut with_compaction_transport_delivery_models(
                bytes.as_bytes(),
                1,
                true,
                1,
                ["another-model"; 4],
            ),
            &success.to_string(),
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("tokenizer model differs from physical draw model"));
        assert!(admit(&mut initialized(), &success.to_string()).is_err());
        let mut foreign_operation = success.clone();
        foreign_operation["data"]["output"]["operationId"] = serde_json::json!("unseen-operation");
        assert!(admit(
            &mut with_compaction_transport(),
            &foreign_operation.to_string()
        )
        .is_err());
        let mut omitted_request = success.clone();
        omitted_request["data"]["output"]["physicalRequests"] = serde_json::json!(2);
        assert!(admit(
            &mut with_compaction_transport(),
            &omitted_request.to_string()
        )
        .is_err());
        let mut changed_budget = success.clone();
        changed_budget["data"]["output"]["maxOutputTokens"] = serde_json::json!(9);
        assert!(admit(
            &mut with_compaction_transport(),
            &changed_budget.to_string()
        )
        .is_err());
        let mut missing_sdk_value = success.clone();
        missing_sdk_value["data"]["output"]["sdkValuesJson"] = serde_json::json!([]);
        assert!(admit(
            &mut with_compaction_transport(),
            &missing_sdk_value.to_string()
        )
        .is_err());
        let mut changed_sdk_value = success.clone();
        changed_sdk_value["data"]["output"]["sdkValuesJson"] =
            serde_json::json!(["{\"choices\":[{\"delta\":{\"content\":\"forged\"}}]}"]);
        assert!(admit(
            &mut with_compaction_transport(),
            &changed_sdk_value.to_string()
        )
        .is_err());
        let mut forged_count = success.clone();
        forged_count["data"]["output"]["newTokenCount"] = serde_json::json!(11);
        forged_count["data"]["newTokenCount"] = serde_json::json!(11);
        assert!(admit(&mut with_compaction_transport(), &forged_count.to_string()).is_err());
        let mut missing_measurement = success.clone();
        missing_measurement["data"]["tokenMeasurements"]
            .as_array_mut()
            .unwrap()
            .pop();
        assert!(admit(
            &mut with_compaction_transport(),
            &missing_measurement.to_string()
        )
        .is_err());
        let first =
            r#"{"choices":[{"finish_reason":"error_finish","delta":{"content":"failed"}}]}"#;
        let unread = r#"{"choices":[{"delta":{"content":"unread"}}]}"#;
        let bytes = format!("data: {first}\n\ndata: {unread}\n\n");
        let mut failed_prefix = success.clone();
        failed_prefix["data"]["status"] = serde_json::json!("COMPRESSION_FAILED_PROTOCOL_ERROR");
        failed_prefix["data"]["succeeded"] = serde_json::json!(false);
        failed_prefix["data"]["newTokenCount"] = serde_json::json!(24);
        failed_prefix["data"]["postCompactionHistory"] = serde_json::Value::Null;
        failed_prefix["data"]["tokenMeasurements"]
            .as_array_mut()
            .unwrap()
            .pop();
        let draw = &mut failed_prefix["data"]["output"];
        draw["sdkValuesJson"] = serde_json::json!([first]);
        draw["text"] = serde_json::json!("");
        draw["functionCalls"] = serde_json::json!([]);
        draw["newTokenCount"] = serde_json::Value::Null;
        draw["snapshotBytes"] = serde_json::Value::Null;
        draw["finishReason"] = serde_json::Value::Null;
        draw["usage"] = serde_json::Value::Null;
        admit(
            &mut with_compaction_transport_response(bytes.as_bytes(), 1, false),
            &failed_prefix.to_string(),
        )
        .unwrap();
        let mut empty_prefix = failed_prefix.clone();
        empty_prefix["data"]["output"]["sdkValuesJson"] = serde_json::json!([]);
        admit(
            &mut with_compaction_transport_response(b"", 0, false),
            &empty_prefix.to_string(),
        )
        .unwrap();
        empty_prefix["data"]["output"]["text"] = serde_json::json!("forged text");
        let error = admit(
            &mut with_compaction_transport_response(b"", 0, false),
            &empty_prefix.to_string(),
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("decoded output without an SDK value"));
        failed_prefix["data"]["output"]["sdkValuesJson"] = serde_json::json!([first, unread]);
        assert!(admit(
            &mut with_compaction_transport_response(bytes.as_bytes(), 1, false),
            &failed_prefix.to_string()
        )
        .is_err());
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
    #[test]
    fn a_stopped_recording_names_its_refusal_and_carries_the_record_it_refused() {
        // The recorder refused a record of its own: the stream carries it,
        // whole, inside the record that reports the stop, and the stop is
        // named by the client's words rather than by the record's bytes.
        let degraded = |data: serde_json::Value| {
            serde_json::json!({
                "type":"system", "subtype":"session_recording_degraded", "uuid":"stop",
                "session_id":"session", "parent_tool_use_id":null, "data":data
            })
            .to_string()
        };
        let refused_record = serde_json::json!({
            "type":"model_response",
            "response":{"journal_id":"fixture","request_id":"refused-request","sequence":1,
                "event":{"kind":"history","disposition":"abandoned"}}
        });
        let message = "Session recording stopped because the recorder refused a record it produced: chat history decision has no completed processing owner.";
        let error = admit(
            &mut initialized(),
            &degraded(serde_json::json!({
                "session_id":"session", "reason":"refused", "message":message,
                "refused_record":refused_record
            })),
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains(&format!(
                "reports operational failure session_recording_degraded (refused): {message}"
            )),
            "{error}"
        );
        assert!(!error.contains("refused-request"), "{error}");
        // A refusal carries its record, and only a refusal does.
        for data in [
            serde_json::json!({"session_id":"session","reason":"refused","message":message}),
            serde_json::json!({"session_id":"session","reason":"write_failed","message":message,
                "refused_record":refused_record}),
        ] {
            let error = admit(&mut initialized(), &degraded(data)).unwrap_err().to_string();
            assert!(error.contains("schema rule"), "{error}");
        }
        // A compaction the stopped recording ended claims only what was settled.
        let mut stopped = compaction_failed();
        stopped["data"]["status"] = serde_json::json!("COMPRESSION_FAILED_RECORDING_STOPPED");
        admit(&mut initialized(), &stopped.to_string()).unwrap();
    }
    /// A partial as the producer writes it: no origin. The runtime owns the
    /// producer positionally (latest chat request or operation receipt).
    fn partial(id: &str, event: &str) -> String {
        format!(
            r#"{{"type":"stream_event","uuid":"{id}","session_id":"session","parent_tool_use_id":null,"event":{event}}}"#
        )
    }
    /// A runtime answer: text only, no model stop or usage, no origin.
    fn assistant(id: &str, scope: &str, content: &str) -> String {
        format!(
            r#"{{"type":"assistant","uuid":"{id}","session_id":"session","parent_tool_use_id":{scope},"message":{{"id":"presentation-{id}","type":"message","role":"assistant","model":"model","content":{content},"stop_reason":null,"usage":null}}}}"#
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
            serde_json::from_str(include_str!("../../test-vectors/ordinary-tool-wire.json")).unwrap();
        // Runtime metadata is authored from this test's manifest. The physical
        // and generation evidence retain their captured producer bytes.
        let mut metadata: serde_json::Value = serde_json::from_str(&init()).unwrap();
        metadata["session_id"] = rows[0]["session_id"].clone();
        rows.insert(1, metadata);
        rows.last_mut().unwrap()["result"] = serde_json::json!("before  after");
        rows
    }
    /// The fixture through its accepted completion and the assistant messages
    /// that completion owes, before any partial or result.
    fn issued_owner() -> RuntimeContract {
        let mut owner = owner();
        let mut completed = false;
        for record in fixture() {
            if completed && record["type"] != "assistant" {
                break;
            }
            completed |= record["type"] == "model_attempt_completion";
            admit(&mut owner, &record.to_string()).unwrap();
        }
        assert!(owner.tool_uses.contains_key("provider__qwen_dup_2"));
        assert!(owner.owed.is_none());
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
                "tool_use_id":"provider__qwen_dup_2","is_error":false}]}
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
    ) -> RuntimeContract {
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
            // An abandoned attempt is not a turn: it shows no assistant message.
            if matches!(record["type"].as_str(), Some("stream_event" | "assistant" | "result")) {
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
        owner
    }
    fn model_partial(owner: &RuntimeContract, id: &str, event: serde_json::Value) -> String {
        serde_json::json!({"type":"stream_event","uuid":id,"session_id":owner.session_id,
            "parent_tool_use_id":null,"event":event})
        .to_string()
    }
    fn utility_request(id: &str, sequence: u64) -> String {
        let body = r#"{"kv_scope":"internal-utility","model":"fixture-model","stream":false,"messages":[]}"#;
        serde_json::json!({
            "type":"model_request", "uuid":format!("request-{id}"), "session_id":"session", "parent_tool_use_id":null,
            "request":{
                "journal_id":"fixture", "request_id":id, "sequence":sequence,
                "kv_scope":"internal-utility", "segment_id":format!("utility-segment-{id}"), "prompt_id":"utility-prompt",
                "owner":{"kind":"utility","operation_id":"utility-operation","purpose":"other"},
                "body":{"kind":"full","json":body,"authors":[{"author":"harness","bytes":body.len()}]},
                "decode_policy":{"mode":"nonstream","model":"fixture-model","strict_tool_calling":true,
                    "named_tool_choice":null,"exact_token_counting":true,"tagged_thinking_tags":false},
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
        // The one output the utility delivers is recorded as its decoded observation.
        let decoded = serde_json::json!({"response":{"candidates":[{"content":{"parts":[],
            "role":"model"},"index":0,"finishReason":"STOP"}]},
            "incomplete_tool_calls":[],"tool_call_preparations":[]})
        .to_string();
        let chunk = serde_json::json!({"kind":"decoded_body","role":"utility","index":0,
            "offset":0,"base64":STANDARD.encode(decoded.as_bytes())});
        admit(owner, &response(id, 4, &chunk.to_string())).unwrap();
        let end = serde_json::json!({"kind":"decoded_end","role":"utility","index":0,
            "body_bytes":decoded.len(),"body_sha256":crate::generation::sha256(decoded.as_bytes())});
        admit(owner, &response(id, 5, &end.to_string())).unwrap();
    }
    fn outcome(id: &str, output: &str) -> String {
        response(
            id,
            6,
            &format!(
                r#"{{"kind":"outcome","status":"completed","error":null,"sdk_values_seen":1,"pipeline_outputs_delivered":1,"served_usage":{{"promptTokenCount":0,"candidatesTokenCount":{output},"thoughtsTokenCount":0,"cachedContentTokenCount":0,"totalTokenCount":{output}}}}}"#
            ),
        )
    }
    #[test]
    fn a_run_without_its_deliverables_names_each_missing_path_in_its_record() {
        let result = serde_json::json!({
            "type":"result", "subtype":"error_missing_deliverables", "uuid":"missing", "session_id":"session",
            "parent_tool_use_id":null, "is_error":true, "duration_ms":0, "duration_api_ms":0, "num_turns":0,
            "usage":{"requests":0,"usageReports":0,"unfinalizedRequests":0,"unreportedUsageRequests":0,"usage":null},
            "permission_denials":[], "error":{"message":"The run ended without 2 declared deliverables"},
            "missing_deliverables":["summary.md","out/report.pdf"],
            "request_evidence":{"journal_id":"fixture","first_sequence":1,"request_count":0,"open_response_ids":[],"open_attempt_ids":[]}
        });
        let mut accepted = owner();
        admit(&mut accepted, &stream_start()).unwrap();
        admit(&mut accepted, &result.to_string()).unwrap();
        let certified = accepted.finish().unwrap();
        assert!(certified.is_error);
        assert_eq!(certified.subtype, MISSING_DELIVERABLES_SUBTYPE);
        assert_eq!(
            certified.missing_deliverables,
            Some(vec!["summary.md".to_string(), "out/report.pdf".to_string()])
        );
        assert_eq!(crate::terminal_exit_code(MISSING_DELIVERABLES_SUBTYPE), Some(1));
        let refused = |candidate: serde_json::Value| {
            let mut rejected = owner();
            admit(&mut rejected, &stream_start()).unwrap();
            admit(&mut rejected, &candidate.to_string()).unwrap_err()
        };
        // The subtype without its list, an empty list, a repeated path, and
        // the list on any other ending are each refused.
        let mut without = result.clone();
        without.as_object_mut().unwrap().remove("missing_deliverables");
        refused(without);
        for list in [serde_json::json!([]), serde_json::json!(["a.md", "a.md"]), serde_json::json!([""])] {
            let mut candidate = result.clone();
            candidate["missing_deliverables"] = list;
            refused(candidate);
        }
        let mut other = result.clone();
        other["subtype"] = serde_json::json!("error_during_execution");
        refused(other);
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
            // The assistant messages derive from the completion, so they go too.
            if matches!(
                record["type"].as_str(),
                Some("model_attempt_completion" | "assistant" | "stream_event")
            ) {
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
            let mut owner = incomplete_owner(name.clone(), arguments);
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
                let raw = model_partial(&owner, id, event);
                admit(&mut owner, &raw).unwrap();
            }
            assert!(owner.tool_uses.is_empty());
            assert_eq!(owner.observations.observed_usage.usage_reports, 1);
            assert_eq!(owner.observations.observed_usage.usage.unwrap().output, 7);
            let delta = model_partial(
                &owner,
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
            let mut owner = incomplete_owner(serde_json::json!("write_file"), "");
            let start = model_partial(
                &owner,
                "start",
                serde_json::json!({"type":"message_start","message":{"id":"message","role":"assistant","model":"model","content":[]}}),
            );
            admit(&mut owner, &start).unwrap();
            let raw = model_partial(
                &owner,
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
                r#"{"type":"message_start","message":{"id":"runtime","role":"assistant","model":"model","content":[]}}"#,
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
            .contains("without a local operation receipt"));

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
        let repeated = raw.replace("response-served-6", "another-outcome");
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
            "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"provider__qwen_dup_2","is_error":false}]}
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
    // ---- The amended stream contract: shown turns, refused draws, closed records ----

    /// Admit rows in order; the first refusal (row index, cause) or, when
    /// every row is admitted, the semantic completion's.
    fn first_refusal(rows: &[serde_json::Value]) -> Result<AgentResult, (usize, String)> {
        let mut reader = owner();
        for (index, row) in rows.iter().enumerate() {
            admit(&mut reader, &row.to_string()).map_err(|cause| (index, cause.to_string()))?;
        }
        reader.finish().map_err(|cause| (rows.len(), cause.to_string()))
    }
    fn position(rows: &[serde_json::Value], kind: &str) -> usize {
        rows.iter().position(|row| row["type"] == kind).unwrap()
    }
    fn shown_rows(rows: &[serde_json::Value]) -> Vec<usize> {
        let completion = position(rows, "model_attempt_completion");
        (completion + 1..rows.len())
            .take_while(|index| rows[*index]["type"] == "assistant")
            .collect()
    }
    /// Rewrite the fixture's generation envelope and bind its evidence and
    /// completion to the changed bytes.
    fn rewrite_generation(rows: &mut [serde_json::Value], edit: impl FnOnce(&mut serde_json::Value)) {
        let generation = position(rows, "model_generation");
        let evidence = &mut rows[generation]["generation"];
        let mut envelope: serde_json::Value =
            serde_json::from_str(evidence["generation_json"].as_str().unwrap()).unwrap();
        edit(&mut envelope);
        let bytes = envelope.to_string();
        let hash = crate::generation::sha256(bytes.as_bytes());
        evidence["generation_bytes"] = serde_json::json!(bytes.len());
        evidence["generation_sha256"] = serde_json::json!(hash);
        evidence["generation_json"] = serde_json::json!(bytes);
        let completion = position(rows, "model_attempt_completion");
        rows[completion]["completion"]["generation_sha256"] = serde_json::json!(hash);
    }
    fn shown(
        session: &serde_json::Value,
        id: &str,
        content: serde_json::Value,
        stop: serde_json::Value,
        usage: serde_json::Value,
    ) -> serde_json::Value {
        serde_json::json!({"type":"assistant","uuid":id,"session_id":session,
            "parent_tool_use_id":null,"message":{"id":id,"type":"message","role":"assistant",
            "model":"test","content":content,"stop_reason":stop,"usage":usage}})
    }
    fn fixture_usage() -> serde_json::Value {
        serde_json::json!({"input_tokens":12,"output_tokens":7,"cache_read_input_tokens":4,
            "reasoning_output_tokens":2,"total_tokens":19})
    }
    const CUT_CALL: &str = "{\"file_path\":\"a.md\",\"content\":\"cut";
    /// The fixture's turn as a draw the provider completed at its output limit:
    /// the call it was writing is served as incomplete text, the runtime refuses
    /// the turn, and the turn shows what the model wrote, ending in the
    /// incomplete call. A refused turn is fatal, so the root ends in error.
    fn refused_draw() -> Vec<serde_json::Value> {
        let mut rows = fixture();
        rows.retain(|row| row["type"] != "stream_event");
        rewrite_generation(&mut rows, |envelope| {
            envelope["finish_reason"] = serde_json::json!("MAX_TOKENS");
            let observations = envelope["observations"].as_array_mut().unwrap();
            observations[1]["tool_call_preparations"] = serde_json::json!([]);
            let last = &mut observations[2];
            last["response"]["candidates"][0]["content"]["parts"] =
                serde_json::json!([{"text":" after"}]);
            last["response"]["candidates"][0]["finishReason"] = serde_json::json!("MAX_TOKENS");
            last["call_ids"] = serde_json::json!([]);
            last["incomplete_tool_calls"] =
                serde_json::json!([{"name":"write_file","arguments":CUT_CALL}]);
        });
        let history = rows
            .iter()
            .position(|row| row["response"]["event"]["kind"] == "history")
            .unwrap();
        rows[history]["response"]["event"]["disposition"] = serde_json::json!("abandoned");
        let completion = position(&rows, "model_attempt_completion");
        rows[completion]["completion"]["disposition"] = serde_json::json!("refused");
        let session = rows[0]["session_id"].clone();
        let display = shown_rows(&rows);
        rows.splice(
            display[0]..=display[display.len() - 1],
            [
                shown(&session, "refused-0", serde_json::json!([{"type":"thinking","thinking":"  **raw thought**\n"}]), serde_json::Value::Null, serde_json::Value::Null),
                shown(&session, "refused-1", serde_json::json!([{"type":"text","text":"before  after"}]), serde_json::Value::Null, serde_json::Value::Null),
                shown(&session, "refused-2", serde_json::json!([{"type":"incomplete_tool_use","name":"write_file","arguments":CUT_CALL}]), serde_json::Value::Null, fixture_usage()),
            ],
        );
        let terminal = rows.last_mut().unwrap();
        terminal.as_object_mut().unwrap().remove("result");
        terminal["subtype"] = serde_json::json!("error_incomplete_generation");
        terminal["is_error"] = serde_json::json!(true);
        terminal["error"] = serde_json::json!({"message":"the model reached its output limit"});
        rows
    }

    const BACKEND_REFUSAL: &str =
        "The model's call to 'audit_probe' names parameter 'value' more than once.";
    /// Rewrite the fixture's one response body and bind its end record to it.
    fn rewrite_body(rows: &mut [serde_json::Value], edit: impl FnOnce(&str) -> String) {
        let body = rows
            .iter()
            .position(|row| {
                row["type"] == "model_response" && row["response"]["event"]["kind"] == "body"
            })
            .unwrap();
        let raw = String::from_utf8(
            STANDARD
                .decode(rows[body]["response"]["event"]["base64"].as_str().unwrap())
                .unwrap(),
        )
        .unwrap();
        let bytes = edit(&raw);
        rows[body]["response"]["event"]["base64"] = serde_json::json!(STANDARD.encode(&bytes));
        let end = rows
            .iter()
            .position(|row| {
                row["type"] == "model_response" && row["response"]["event"]["kind"] == "end"
            })
            .unwrap();
        rows[end]["response"]["event"]["body_bytes"] = serde_json::json!(bytes.len());
        rows[end]["response"]["event"]["body_sha256"] =
            serde_json::json!(crate::generation::sha256(bytes.as_bytes()));
    }
    /// The fixture's turn as one its backend refused: after the reasoning, the
    /// stream ends in the refusal's error payload, with no terminal, no usage
    /// and no call. The client records the response as failing with that
    /// refusal, the runtime refuses the turn, and the turn shows what the
    /// model wrote. Four refusals in a row end the run, so the root ends in
    /// error.
    fn backend_refused_draw(error_type: &str) -> Vec<serde_json::Value> {
        let mut rows = fixture();
        rows.retain(|row| row["type"] != "stream_event");
        rewrite_body(&mut rows, |raw| {
            let first = raw.split("\n\n").next().unwrap();
            let refusal = serde_json::json!({"error":{"message":BACKEND_REFUSAL,
                "type":error_type,"param":null,"code":422}});
            format!("{first}\n\ndata: {refusal}\n\ndata: [DONE]\n\n")
        });
        let outcome = rows
            .iter()
            .position(|row| row["response"]["event"]["kind"] == "outcome")
            .unwrap();
        let event = &mut rows[outcome]["response"]["event"];
        event["status"] = serde_json::json!("failed");
        event["error"] =
            serde_json::json!(format!("RepeatedToolParameterRefusal: {BACKEND_REFUSAL}"));
        event["served_usage"] = serde_json::Value::Null;
        event["sdk_values_seen"] = serde_json::json!(1);
        event["pipeline_outputs_delivered"] = serde_json::json!(1);
        rewrite_generation(&mut rows, |envelope| {
            envelope["finish_reason"] = serde_json::Value::Null;
            envelope["usage"] = serde_json::Value::Null;
            envelope["observations"].as_array_mut().unwrap().truncate(1);
        });
        let history = rows
            .iter()
            .position(|row| row["response"]["event"]["kind"] == "history")
            .unwrap();
        rows[history]["response"]["event"]["disposition"] = serde_json::json!("abandoned");
        let completion = position(&rows, "model_attempt_completion");
        rows[completion]["completion"]["disposition"] = serde_json::json!("refused");
        rows[completion]["completion"]["consumer_observations"] = serde_json::json!(1);
        let session = rows[0]["session_id"].clone();
        let display = shown_rows(&rows);
        rows.splice(
            display[0]..=display[display.len() - 1],
            [shown(
                &session,
                "refused-0",
                serde_json::json!([{"type":"thinking","thinking":"  **raw thought**\n"}]),
                serde_json::Value::Null,
                serde_json::Value::Null,
            )],
        );
        let terminal = rows.last_mut().unwrap();
        terminal.as_object_mut().unwrap().remove("result");
        terminal["subtype"] = serde_json::json!("error_incomplete_generation");
        terminal["is_error"] = serde_json::json!(true);
        terminal["error"] = serde_json::json!({"message":"the backend refused the turn"});
        terminal["usage"] = serde_json::json!({"requests":1,"usageReports":0,
            "unfinalizedRequests":0,"unreportedUsageRequests":1,"usage":null});
        rows
    }

    #[test]
    fn the_wire_fixture_is_a_complete_stream_of_the_compiled_contract() {
        let rows: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../../test-vectors/ordinary-tool-wire.json")).unwrap();
        assert_eq!(rows[0]["stream_contract_sha256"], STREAM_CONTRACT_SHA256);
        // The captured turn shows itself at once after its completion, and
        // nothing in the stream names an output origin.
        let display = shown_rows(&rows);
        assert_eq!(display.len(), 3);
        assert!(rows.iter().all(|row| row.get("origin").is_none()));
        let result = first_refusal(&fixture()).unwrap();
        assert_eq!(result.response, "before  after");
    }

    /// A request that asks for return_token_ids is answered with the prompt's
    /// ids on the first chunk and the generated ids on each choice. They are
    /// recorded with the response bytes and change nothing the stream
    /// certifies: the fixture's generation, unchanged, still certifies.
    #[test]
    fn a_response_carrying_token_ids_certifies_with_the_same_generation() {
        let mut rows = fixture();
        let body = rows
            .iter()
            .position(|row| row["type"] == "model_response" && row["response"]["event"]["kind"] == "body")
            .unwrap();
        let raw = String::from_utf8(
            STANDARD
                .decode(rows[body]["response"]["event"]["base64"].as_str().unwrap())
                .unwrap(),
        )
        .unwrap();
        let mut next = 100_u64;
        let mut first = true;
        let with_ids = raw
            .split('\n')
            .map(|line| match line.strip_prefix("data: ") {
                Some(json) if json.starts_with('{') => {
                    let mut chunk: serde_json::Value = serde_json::from_str(json).unwrap();
                    for choice in chunk["choices"].as_array_mut().unwrap() {
                        choice["token_ids"] = serde_json::json!([next, next + 1]);
                        next += 2;
                    }
                    if first {
                        chunk["prompt_token_ids"] = serde_json::json!([1, 2, 3]);
                        first = false;
                    }
                    format!("data: {chunk}")
                }
                _ => line.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert_ne!(with_ids, raw);
        rows[body]["response"]["event"]["base64"] = serde_json::json!(STANDARD.encode(&with_ids));
        let end = rows
            .iter()
            .position(|row| row["type"] == "model_response" && row["response"]["event"]["kind"] == "end")
            .unwrap();
        rows[end]["response"]["event"]["body_bytes"] = serde_json::json!(with_ids.len());
        rows[end]["response"]["event"]["body_sha256"] =
            serde_json::json!(crate::generation::sha256(with_ids.as_bytes()));
        let result = first_refusal(&rows).unwrap();
        assert_eq!(result.response, "before  after");
    }

    #[test]
    fn an_accepted_turn_shows_exactly_its_generation_display() {
        let rows = fixture();
        let completion = position(&rows, "model_attempt_completion");
        let display = shown_rows(&rows);
        assert_eq!(display, [completion + 1, completion + 2, completion + 3]);
        first_refusal(&rows).unwrap();
        let session = rows[0]["session_id"].clone();
        let late_result = serde_json::json!({"type":"user","uuid":"early-result","session_id":session,
            "parent_tool_use_id":null,"message":{"role":"user","content":[{"type":"tool_result",
            "tool_use_id":"provider__qwen_dup_2","is_error":false}]}});
        type Mutation = Box<dyn Fn(&mut Vec<serde_json::Value>)>;
        let (thinking, text, call) = (display[0], display[1], display[2]);
        let cases: Vec<(&str, Mutation, usize, &str)> = vec![
            ("forged text", Box::new(move |rows| rows[text]["message"]["content"][0]["text"] = serde_json::json!("FORGED")), text, "with content its generation does not have"),
            ("forged thought", Box::new(move |rows| rows[thinking]["message"]["content"][0]["thinking"] = serde_json::json!("FORGED")), thinking, "with content"),
            ("changed call arguments", Box::new(move |rows| rows[call]["message"]["content"][0]["input"] = serde_json::json!({"value":2})), call, "with content"),
            ("changed call identity", Box::new(move |rows| rows[call]["message"]["content"][0]["id"] = serde_json::json!("provider")), call, "with content"),
            ("changed usage", Box::new(move |rows| rows[call]["message"]["usage"]["output_tokens"] = serde_json::json!(8)), call, "with usage"),
            ("usage on an earlier message", Box::new(move |rows| rows[thinking]["message"]["usage"] = fixture_usage()), thinking, "with usage"),
            ("usage missing from the last message", Box::new(move |rows| rows[call]["message"]["usage"] = serde_json::Value::Null), call, "with usage"),
            ("dropped tool_use stop reason", Box::new(move |rows| rows[call]["message"]["stop_reason"] = serde_json::Value::Null), call, "with a stop reason"),
            ("claimed tool_use stop reason", Box::new(move |rows| rows[text]["message"]["stop_reason"] = serde_json::json!("tool_use")), text, "with a stop reason"),
            ("changed model", Box::new(move |rows| rows[thinking]["message"]["model"] = serde_json::json!("other-model")), thinking, "with a model"),
            ("reordered messages", Box::new(move |rows| rows.swap(thinking, text)), thinking, "with content"),
            ("merged messages", Box::new(move |rows| {
                let merged = serde_json::json!([rows[thinking]["message"]["content"][0], rows[text]["message"]["content"][0]]);
                rows[thinking]["message"]["content"] = merged;
                rows.remove(text);
            }), thinking, "with content"),
            ("dropped first message", Box::new(move |rows| { rows.remove(thinking); }), thinking, "with content"),
            ("dropped last message", Box::new(move |rows| { rows.remove(call); }), call, "precedes the assistant messages its settled turn in the main session shows"),
            ("dropped display", Box::new(move |rows| { rows.drain(thinking..=call); }), completion + 1, "precedes the assistant messages"),
            ("extra message", Box::new(move |rows| {
                let mut extra = rows[call].clone();
                extra["uuid"] = serde_json::json!("extra-assistant");
                extra["message"]["id"] = serde_json::json!("extra-assistant");
                rows.insert(call + 1, extra);
            }), call + 1, "runtime assistant output after a chat attempt"),
            ("record before the display", Box::new(move |rows| rows.insert(completion + 1, late_result.clone())), completion + 1, "precedes the assistant messages"),
            ("origin label", Box::new(move |rows| rows[text]["origin"] = serde_json::json!({"kind":"model","attempt_id":"a","kv_scope":"s"})), text, "violates stream contract"),
            ("stream ends before the display", Box::new(move |rows| rows.truncate(completion + 1)), completion + 1, "ends before the assistant messages of a settled turn"),
        ];
        for (name, mutate, line, cause) in cases {
            let mut mutant = rows.clone();
            mutate(&mut mutant);
            let (index, refusal) = first_refusal(&mutant).expect_err(name);
            assert_eq!(index, line, "{name}: {refusal}");
            assert!(refusal.contains(cause), "{name}: {refusal}");
        }
    }

    #[test]
    fn a_refused_draw_shows_its_incomplete_call_and_certifies() {
        let rows = refused_draw();
        let result = first_refusal(&rows).unwrap();
        assert!(result.is_error);
        assert_eq!(result.subtype, "error_incomplete_generation");
        assert_eq!(result.usage.usage.unwrap().output, 7);
        let mut reader = owner();
        for row in &rows {
            admit(&mut reader, &row.to_string()).unwrap();
        }
        assert!(reader.tool_uses.is_empty(), "a refused draw issues no tool");

        // The same draw with its partials in upstream's root framing: text
        // streams before the generation, the incomplete call after its turn.
        let mut streamed = rows.clone();
        let session = streamed[0]["session_id"].clone();
        let partial = |id: &str, event: serde_json::Value| {
            serde_json::json!({"type":"stream_event","uuid":id,"session_id":session,
                "parent_tool_use_id":null,"event":event})
        };
        let generation = position(&streamed, "model_generation");
        streamed.splice(
            generation..generation,
            [
                partial("refused-start", serde_json::json!({"type":"message_start","message":{"id":"refused","role":"assistant","model":"test","content":[]}})),
                partial("refused-text", serde_json::json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"before "}})),
                partial("refused-text-more", serde_json::json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" after"}})),
                partial("refused-text-stop", serde_json::json!({"type":"content_block_stop","index":0})),
            ],
        );
        let last_shown = *shown_rows(&streamed).last().unwrap();
        streamed.splice(
            last_shown + 1..last_shown + 1,
            [
                partial("refused-call", serde_json::json!({"type":"content_block_start","index":0,"content_block":{"type":"incomplete_tool_use","name":"write_file","arguments":CUT_CALL}})),
                partial("refused-call-stop", serde_json::json!({"type":"content_block_stop","index":0})),
                partial("refused-stop", serde_json::json!({"type":"message_stop"})),
            ],
        );
        first_refusal(&streamed).unwrap();
        // A refused draw cannot stream an executable claim.
        let mut claimed = streamed.clone();
        let call = claimed.iter().position(|row| row["uuid"] == "refused-call").unwrap();
        claimed[call]["event"]["content_block"] = serde_json::json!({"type":"tool_use","id":"provider__qwen_dup_2","name":"write_file","input":{}});
        let (index, refusal) = first_refusal(&claimed).unwrap_err();
        assert_eq!(index, call);
        assert!(refusal.contains("tool claim precedes accepted generation completion"), "{refusal}");

        // What a refused turn shows is its generation's: the incomplete call is
        // shown as served and never as a call.
        let display = shown_rows(&rows);
        let last = *display.last().unwrap();
        type Mutation = Box<dyn Fn(&mut Vec<serde_json::Value>)>;
        let cases: Vec<(&str, Mutation, &str)> = vec![
            (
                "changed incomplete arguments",
                Box::new(move |rows| rows[last]["message"]["content"][0]["arguments"] = serde_json::json!("{}")),
                "with content",
            ),
            (
                "incomplete call shown as a call",
                Box::new(move |rows| {
                    rows[last]["message"]["content"] = serde_json::json!([{"type":"tool_use","id":"cut","name":"write_file","input":{}}]);
                    rows[last]["message"]["stop_reason"] = serde_json::json!("tool_use");
                }),
                "with a stop reason",
            ),
            (
                "dropped incomplete call",
                Box::new(move |rows| { rows.remove(last); }),
                "precedes the assistant messages",
            ),
        ];
        for (name, mutate, cause) in cases {
            let mut mutant = rows.clone();
            mutate(&mut mutant);
            let (index, refusal) = first_refusal(&mutant).expect_err(name);
            assert_eq!(index, last, "{name}: {refusal}");
            assert!(refusal.contains(cause), "{name}: {refusal}");
        }
    }

    #[test]
    fn a_refused_completion_requires_a_complete_draw_stopped_at_its_output_limit() {
        let completion = |rows: &[serde_json::Value]| position(rows, "model_attempt_completion");
        // The provider finished the turn: it was not stopped at its limit.
        let mut stopped = refused_draw();
        rewrite_generation(&mut stopped, |envelope| {
            envelope["finish_reason"] = serde_json::json!("STOP");
            envelope["observations"][2]["response"]["candidates"][0]["finishReason"] =
                serde_json::json!("STOP");
        });
        // A draw at its limit that still made an executable call.
        let mut called = fixture();
        rewrite_generation(&mut called, |envelope| {
            envelope["finish_reason"] = serde_json::json!("MAX_TOKENS");
            envelope["observations"][2]["response"]["candidates"][0]["finishReason"] =
                serde_json::json!("MAX_TOKENS");
        });
        let history = called
            .iter()
            .position(|row| row["response"]["event"]["kind"] == "history")
            .unwrap();
        called[history]["response"]["event"]["disposition"] = serde_json::json!("abandoned");
        let index = completion(&called);
        called[index]["completion"]["disposition"] = serde_json::json!("refused");
        // The consumer received fewer outputs than the response delivered.
        let mut partial_delivery = refused_draw();
        let index = completion(&partial_delivery);
        partial_delivery[index]["completion"]["consumer_observations"] = serde_json::json!(2);
        // Served usage differs from the final physical response's.
        let mut other_usage = refused_draw();
        rewrite_generation(&mut other_usage, |envelope| {
            let usage = serde_json::json!({"totalTokenCount":20,"promptTokenCount":12,
                "candidatesTokenCount":8,"thoughtsTokenCount":2,"cachedContentTokenCount":4});
            envelope["usage"] = usage.clone();
            envelope["observations"][2]["response"]["usageMetadata"] = usage;
        });
        // A refused turn never entered history.
        let mut accepted_history = refused_draw();
        let history = accepted_history
            .iter()
            .position(|row| row["response"]["event"]["kind"] == "history")
            .unwrap();
        accepted_history[history]["response"]["event"]["disposition"] = serde_json::json!("accepted");
        for (name, rows, cause) in [
            ("finished turn", stopped, "a refused draw must stop at its output limit, or end in its backend's refusal, without a call"),
            ("executable call", called, "a refused draw must stop at its output limit, or end in its backend's refusal, without a call"),
            ("partial delivery", partial_delivery, "consumer receipt contradicts"),
            ("other usage", other_usage, "completed turn usage contradicts its physical response"),
            ("history acceptance", accepted_history, "logical disposition contradicts physical history decisions"),
        ] {
            let (index, refusal) = first_refusal(&rows).expect_err(name);
            assert_eq!(index, completion(&rows), "{name}: {refusal}");
            assert!(refusal.contains(cause), "{name}: {refusal}");
        }
    }

    #[test]
    fn a_draw_its_backend_refused_shows_what_it_wrote_and_certifies_with_no_usage() {
        let rows = backend_refused_draw("RepeatedToolParameterError");
        let result = first_refusal(&rows).unwrap();
        assert!(result.is_error);
        assert_eq!(result.subtype, "error_incomplete_generation");
        // The response served no usage, so the draw bills none.
        assert!(result.usage.usage.is_none());
        let mut reader = owner();
        for row in &rows {
            admit(&mut reader, &row.to_string()).unwrap();
        }
        assert!(reader.tool_uses.is_empty(), "a refused draw issues no tool");
    }

    #[test]
    fn a_backend_refusal_is_read_from_the_bytes_that_carry_it() {
        let outcome = |rows: &[serde_json::Value]| {
            rows.iter()
                .position(|row| row["response"]["event"]["kind"] == "outcome")
                .unwrap()
        };
        // Recorded as the refusal, but the bytes end in another failure.
        let other_failure = backend_refused_draw("InternalServerError");
        // The bytes end in the refusal, but it was recorded as another failure.
        let mut renamed = backend_refused_draw("RepeatedToolParameterError");
        let index = outcome(&renamed);
        renamed[index]["response"]["event"]["error"] = serde_json::json!("Error: refused");
        for (name, rows) in [
            ("other failure", other_failure),
            ("renamed refusal", renamed),
        ] {
            let (index, refusal) = first_refusal(&rows).expect_err(name);
            assert_eq!(index, outcome(&rows), "{name}: {refusal}");
            assert!(
                refusal.contains("the response's recorded refusal differs from its physical bytes"),
                "{name}: {refusal}"
            );
        }
        // A completion that claims refusal for a response that ended in
        // another failure, recorded as that failure, is no refused draw.
        let mut failed = backend_refused_draw("InternalServerError");
        let index = outcome(&failed);
        failed[index]["response"]["event"]["error"] = serde_json::json!("Error: refused");
        let completion = position(&failed, "model_attempt_completion");
        let (index, refusal) = first_refusal(&failed).expect_err("failed draw");
        assert_eq!(index, completion, "{refusal}");
        assert!(
            refusal.contains("a refused draw must stop at its output limit, or end in its backend's refusal, without a call"),
            "{refusal}"
        );
    }

    #[test]
    fn an_abandoned_attempt_shows_no_turn() {
        let owner_after = || incomplete_owner(serde_json::json!("write_file"), CUT_CALL);
        let mut owner = owner_after();
        assert!(owner.owed.is_none(), "an abandoned attempt owes no display");
        let session = owner.session_id.clone().unwrap();
        for content in [
            serde_json::json!([{"type":"incomplete_tool_use","name":"write_file","arguments":CUT_CALL}]),
            serde_json::json!([{"type":"text","text":"before  after"}]),
        ] {
            let row = serde_json::json!({"type":"assistant","uuid":"abandoned-shown",
                "session_id":session,"parent_tool_use_id":null,"message":{"id":"abandoned-shown",
                "type":"message","role":"assistant","model":"test","content":content,
                "stop_reason":null,"usage":fixture_usage()}});
            let refusal = admit(&mut owner_after(), &row.to_string()).unwrap_err().to_string();
            assert!(
                refusal.contains("claims runtime assistant output after a chat attempt"),
                "{refusal}"
            );
        }
        // Without a display, the abandoned attempt ends its stream cleanly.
        let mut terminal = fixture().last().unwrap().clone();
        terminal.as_object_mut().unwrap().remove("result");
        terminal["subtype"] = serde_json::json!("error_during_execution");
        terminal["is_error"] = serde_json::json!(true);
        terminal["error"] = serde_json::json!({"message":"the attempt was abandoned"});
        admit(&mut owner, &terminal.to_string()).unwrap();
        owner.finish().unwrap();
    }

    #[test]
    fn partials_in_the_upstream_root_framing_certify_against_their_positional_generation() {
        let mut rows = fixture();
        rows.retain(|row| row["type"] != "stream_event");
        let session = rows[0]["session_id"].clone();
        let partial = |id: &str, event: serde_json::Value| {
            serde_json::json!({"type":"stream_event","uuid":id,"session_id":session,
                "parent_tool_use_id":null,"event":event})
        };
        let generation = position(&rows, "model_generation");
        // One group for the attempt; each message's blocks restart at zero.
        rows.splice(
            generation..generation,
            [
                partial("start", serde_json::json!({"type":"message_start","message":{"id":"turn","role":"assistant","model":"test","content":[]}})),
                partial("thought", serde_json::json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}})),
                partial("thought-delta", serde_json::json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"  **raw thought**\n"}})),
                partial("thought-stop", serde_json::json!({"type":"content_block_stop","index":0})),
                partial("text", serde_json::json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})),
                partial("text-delta", serde_json::json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"before "}})),
                partial("text-more", serde_json::json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" after"}})),
                partial("text-stop", serde_json::json!({"type":"content_block_stop","index":0})),
            ],
        );
        let last_shown = *shown_rows(&rows).last().unwrap();
        rows.splice(
            last_shown + 1..last_shown + 1,
            [
                partial("call", serde_json::json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"provider__qwen_dup_2","name":"audit_probe","input":{}}})),
                partial("call-delta", serde_json::json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{ \"value\" : 1e0 }"}})),
                partial("call-stop", serde_json::json!({"type":"content_block_stop","index":0})),
                partial("stop", serde_json::json!({"type":"message_stop"})),
            ],
        );
        assert_eq!(first_refusal(&rows).unwrap().response, "before  after");

        let at = |rows: &[serde_json::Value], id: &str| {
            rows.iter().position(|row| row["uuid"] == id).unwrap()
        };
        // Partials are owned by the scope's latest chat request: its
        // generation binds every byte they claim, before or after it arrives.
        let mut forged = rows.clone();
        let index = at(&forged, "text-more");
        forged[index]["event"]["delta"]["text"] = serde_json::json!(" AFTER");
        let (line, refusal) = first_refusal(&forged).unwrap_err();
        assert_eq!(line, position(&forged, "model_generation"));
        assert!(refusal.contains("partial text changes or repeats observed generation bytes"), "{refusal}");
        let mut forged_call = rows.clone();
        let index = at(&forged_call, "call-delta");
        forged_call[index]["event"]["delta"]["partial_json"] = serde_json::json!("{\"value\":2}");
        let (line, refusal) = first_refusal(&forged_call).unwrap_err();
        assert_eq!(line, index);
        assert!(refusal.contains("tool argument bytes contradict the generation"), "{refusal}");
        // A block index restarts only once every block of the message closed.
        let mut overlapping = rows.clone();
        let index = at(&overlapping, "thought-stop");
        let stop = overlapping.remove(index);
        let text_stop = at(&overlapping, "text-stop");
        overlapping.insert(text_stop, stop);
        let (line, refusal) = first_refusal(&overlapping).unwrap_err();
        assert_eq!(line, at(&overlapping, "text"));
        assert!(refusal.contains("not the next message index"), "{refusal}");
        // The root scope brackets its output: a group left open never closes.
        let mut unclosed = rows.clone();
        unclosed.remove(at(&unclosed, "stop"));
        let (line, refusal) = first_refusal(&unclosed).unwrap_err();
        assert_eq!(line, unclosed.len() - 1);
        assert!(refusal.contains("unfinished partial groups"), "{refusal}");
    }

    #[test]
    fn a_runtime_answer_is_a_receipt_and_a_plain_text_row() {
        let mut reader = initialized();
        admit(&mut reader, &local_operation(None, "local answer")).unwrap();
        admit(
            &mut reader,
            &assistant("local", "null", r#"[{"type":"text","text":"local answer"}]"#),
        )
        .unwrap();
        admit(&mut reader, &runtime_result("local answer")).unwrap();
        assert_eq!(reader.finish().unwrap().response, "local answer");

        let answer: serde_json::Value = serde_json::from_str(&assistant(
            "local",
            "null",
            r#"[{"type":"text","text":"local answer"}]"#,
        ))
        .unwrap();
        for (name, field, value, cause) in [
            ("model stop", "stop_reason", serde_json::json!("tool_use"), "claims model stop or usage on runtime assistant output"),
            ("model usage", "usage", fixture_usage(), "claims model stop or usage on runtime assistant output"),
            ("thought", "content", serde_json::json!([{"type":"thinking","thinking":"local answer"}]), "non-text runtime assistant output"),
        ] {
            let mut changed = answer.clone();
            changed["message"][field] = value;
            let mut reader = initialized();
            admit(&mut reader, &local_operation(None, "local answer")).unwrap();
            let refusal = admit(&mut reader, &changed.to_string()).unwrap_err().to_string();
            assert!(refusal.contains(cause), "{name}: {refusal}");
        }
        let mut labelled = answer;
        labelled["origin"] = serde_json::json!({"kind":"runtime","operation_id":"local-operation"});
        let mut reader = initialized();
        admit(&mut reader, &local_operation(None, "local answer")).unwrap();
        assert!(admit(&mut reader, &labelled.to_string())
            .unwrap_err()
            .to_string()
            .contains("violates stream contract"));
    }

    #[test]
    fn a_runtime_operation_receipt_belongs_to_the_root() {
        // Even a scope an accepted generation issued cannot carry a receipt:
        // local operations run only in the root session.
        let mut issued = issued_owner();
        let child = "provider__qwen_dup_2";
        let mut receipt: serde_json::Value =
            serde_json::from_str(&local_operation(Some(child), "local answer")).unwrap();
        receipt["session_id"] = serde_json::json!(issued.session_id);
        let refusal = admit(&mut issued, &receipt.to_string()).unwrap_err().to_string();
        assert!(refusal.contains("violates stream contract"), "{refusal}");
        for parent in [serde_json::json!("child"), serde_json::json!(child)] {
            receipt["parent_tool_use_id"] = parent;
            let mut reader = initialized();
            receipt["session_id"] = serde_json::json!("session");
            let refusal = admit(&mut reader, &receipt.to_string()).unwrap_err().to_string();
            assert!(refusal.contains("violates stream contract"), "{refusal}");
        }
        let mut root = initialized();
        admit(&mut root, &local_operation(None, "local answer")).unwrap();
    }

    #[test]
    fn every_record_kind_is_closed_at_its_top_level_and_message() {
        let unknown = |row: &serde_json::Value, pointer: &str| {
            let mut changed = row.clone();
            changed
                .pointer_mut(pointer)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("unowned_field".into(), serde_json::json!(true));
            changed
        };
        let refused_at = |rows: &[serde_json::Value], index: usize, changed: serde_json::Value| {
            let mut reader = owner();
            for row in &rows[..index] {
                admit(&mut reader, &row.to_string()).unwrap();
            }
            admit(&mut reader, &changed.to_string()).unwrap_err().to_string()
        };
        // Every kind the captured stream carries, including its shown turn.
        let rows = fixture();
        let mut kinds = BTreeSet::new();
        for (index, row) in rows.iter().enumerate() {
            kinds.insert(row["type"].as_str().unwrap().to_string());
            let mut pointers = vec![""];
            if row["type"] == "assistant" {
                pointers.push("/message");
            }
            for pointer in pointers {
                let refusal = refused_at(&rows, index, unknown(row, pointer));
                assert!(refusal.contains("violates stream contract"), "{index}{pointer}: {refusal}");
            }
        }
        // A tool result row and its message.
        let mut issued = issued_owner();
        let result = serde_json::json!({"type":"user","uuid":"returned","session_id":issued.session_id,
            "parent_tool_use_id":null,"message":{"role":"user","content":[{"type":"tool_result",
            "tool_use_id":"provider__qwen_dup_2","is_error":false}]}});
        for pointer in ["", "/message", "/message/content/0"] {
            let refusal = admit(&mut issued_owner(), &unknown(&result, pointer).to_string())
                .unwrap_err()
                .to_string();
            assert!(refusal.contains("violates stream contract"), "user{pointer}: {refusal}");
        }
        admit(&mut issued, &result.to_string()).unwrap();
        kinds.insert("user".into());
        // A runtime operation receipt and its answer.
        let receipt: serde_json::Value =
            serde_json::from_str(&local_operation(None, "local answer")).unwrap();
        let answer: serde_json::Value = serde_json::from_str(&assistant(
            "local",
            "null",
            r#"[{"type":"text","text":"local answer"}]"#,
        ))
        .unwrap();
        assert!(admit(&mut initialized(), &unknown(&receipt, "").to_string())
            .unwrap_err()
            .to_string()
            .contains("violates stream contract"));
        let mut reader = initialized();
        admit(&mut reader, &receipt.to_string()).unwrap();
        assert!(admit(&mut reader, &unknown(&answer, "/message").to_string())
            .unwrap_err()
            .to_string()
            .contains("violates stream contract"));
        // Utility work.
        let utility = tokenizer_records("original", 1, 24, "fixture-model");
        for (index, row) in utility.iter().enumerate() {
            let mut reader = initialized();
            for earlier in &utility[..index] {
                admit(&mut reader, earlier).unwrap();
            }
            let row: serde_json::Value = serde_json::from_str(row).unwrap();
            kinds.insert(row["type"].as_str().unwrap().to_string());
            assert!(admit(&mut reader, &unknown(&row, "").to_string())
                .unwrap_err()
                .to_string()
                .contains("violates stream contract"));
        }
        // Every record kind of the contract was probed.
        for kind in [
            "system", "user", "assistant", "result", "stream_event", "model_request",
            "model_utility_request", "model_utility_completion", "model_response",
            "model_normalization_seed", "model_generation", "model_attempt_completion",
        ] {
            assert!(EventKind::from_wire(kind).is_some(), "{kind}");
            assert!(kinds.contains(kind), "{kind} was not probed");
        }
    }

    #[test]
    fn served_usage_is_closed_wherever_it_is_reported() {
        let extra = |usage: &mut serde_json::Value| {
            usage["unownedTokenCount"] = serde_json::json!(0);
        };
        let rows = fixture();
        // The physical outcome and the terminal summary.
        let outcome = rows
            .iter()
            .position(|row| row["response"]["event"]["kind"] == "outcome")
            .unwrap();
        let terminal = rows.len() - 1;
        for (index, pointer) in [
            (outcome, "/response/event/served_usage"),
            (terminal, "/usage/usage"),
        ] {
            let mut mutant = rows.clone();
            extra(mutant[index].pointer_mut(pointer).unwrap());
            let (line, refusal) = first_refusal(&mutant).unwrap_err();
            assert_eq!(line, index, "{pointer}: {refusal}");
            assert!(refusal.contains("violates stream contract"), "{pointer}: {refusal}");
        }
        // The generation envelope's summary, bound to consistent bytes.
        let mut mutant = rows.clone();
        rewrite_generation(&mut mutant, |envelope| extra(&mut envelope["usage"]));
        let (line, refusal) = first_refusal(&mutant).unwrap_err();
        assert_eq!(line, position(&mutant, "model_generation"));
        assert!(refusal.contains("violates stream contract"), "{refusal}");
        // A compaction draw.
        let mut draw = compaction_success();
        extra(&mut draw["data"]["output"]["usage"]);
        assert!(admit(&mut with_compaction_transport(), &draw.to_string())
            .unwrap_err()
            .to_string()
            .contains("violates stream contract"));
        admit(&mut with_compaction_transport(), &compaction_success().to_string()).unwrap();
    }
}
