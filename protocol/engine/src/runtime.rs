//! One pure owner for stream identity, semantic admission and independent
//! observations. Physical capture, storage and process receipts remain external.
use crate::{
    decode_event, decode_event_kind,
    json::{Document, Limits, Value},
    schema::ValidationLimits,
    stream::{field, text, unsigned},
    ContractError, ContractResult, DecodedRecord, EventKind, PartialStreamState, SystemKind,
    SAFE_INTEGER, STREAM_CONTRACT_SHA256,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
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
    /// Generated tokens the backend billed to those turns, summed, and the
    /// part of them it counted as reasoning. Both are read from the served
    /// usage every billed turn must carry; nothing here is estimated.
    pub main_output_tokens: u64,
    pub main_reasoning_tokens: u64,
    /// Every subagent scope the stream resolved, in order of first
    /// appearance. Empty exactly when the run delegated nothing.
    pub scopes: Vec<AgentScope>,
}

// Public terminal vocabulary is generated from the same schema used by the producer.
pub use crate::{ERROR_SUBTYPES, SUCCESS_SUBTYPE};

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
    /// parser: one per model round the subagent completed, since the client
    /// publishes every round's text, reasoning and served usage under the
    /// scope's tool-call id. The count a subagent reports for itself is
    /// `reported_num_turns`.
    pub billed_turns: u64,
    /// Generated tokens the backend billed to those rounds, summed, and the
    /// part of them it counted as reasoning, exactly as for the main scope.
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    /// What the scope's own terminal record reported, verbatim. All four are
    /// `None` exactly when the scope never emitted a terminal record (the
    /// subagent was still running, or was torn down, when the session ended);
    /// `error_message` is additionally `None` when the record carried no
    /// error. Reported started turns and independently observed billed turns
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

/// The usage a billed assistant event carries: what the backend served for
/// the generation and the client copied onto the wire, field for field.
#[derive(Debug, Clone, Copy)]
struct BilledUsage {
    output_tokens: u64,
    reasoning_output_tokens: u64,
}

/// Read a billed assistant event's usage, or `None` for an unbilled fragment.
///
/// A non-null billed usage must carry every count the backend
/// serves — the generated tokens, the part of them counted as reasoning, and
/// the prompt tokens read back from the prefix cache — as non-negative
/// integers, with reasoning no larger than the output it is part of. A count
/// the stream omitted is not read as zero: the client fails a request whose
/// usage arrived without these, so an event without them is a stream this
/// parser does not recognise, and it is refused rather than tallied.
fn billed_usage(
    object: Value<'_>,
    line: usize,
    scope: Option<&str>,
) -> ContractResult<Option<BilledUsage>> {
    let usage_value = object
        .get("message")
        .and_then(Value::as_object)
        .and_then(|message| message.get("usage"))
        .ok_or_else(|| {
            ContractError::InvalidRecord(format!(
                "events.jsonl line {line} assistant message lacks usage (object or null) in {}",
                scope_display(scope)
            ))
        })?;
    if usage_value.is_null() {
        return Ok(None);
    }
    let usage = usage_value.as_object().ok_or_else(|| {
        ContractError::InvalidRecord(format!(
            "events.jsonl line {line} assistant usage is neither an object nor null in {}",
            scope_display(scope)
        ))
    })?;
    let count = |key: &str| -> ContractResult<u64> {
        field(usage, key, line).and_then(|value| unsigned(value, key, SAFE_INTEGER)).map_err(|cause| ContractError::InvalidRecord(format!("events.jsonl line {line} bills a turn in {} whose usage lacks non-negative integer {key}: {cause}", scope_display(scope))))
    };
    let input_tokens = count("input_tokens")?;
    let output_tokens = count("output_tokens")?;
    let reasoning_output_tokens = count("reasoning_output_tokens")?;
    let cache_read_input_tokens = count("cache_read_input_tokens")?;
    if reasoning_output_tokens > output_tokens {
        return Err(ContractError::InvalidRecord(format!(
            "events.jsonl line {line} bills {reasoning_output_tokens} reasoning tokens against only {output_tokens} output tokens in {}",
            scope_display(scope)
        )));
    }
    if cache_read_input_tokens > input_tokens {
        return Err(ContractError::InvalidRecord(format!(
            "events.jsonl line {line} reads {cache_read_input_tokens} cached prompt tokens against only {input_tokens} input tokens in {}",
            scope_display(scope)
        )));
    }
    let total_tokens = count("total_tokens")?;
    if input_tokens.checked_add(output_tokens) != Some(total_tokens) {
        return Err(ContractError::InvalidRecord(format!(
            "events.jsonl line {line} total_tokens disagrees with input_tokens plus output_tokens in {}",
            scope_display(scope)
        )));
    }
    Ok(Some(BilledUsage {
        output_tokens,
        reasoning_output_tokens,
    }))
}

/// The session summary accounts for admitted requests, including requests with
/// no served record. It does not replace the independently billed event totals.
fn validate_generation_summary(value: Option<Value<'_>>) -> ContractResult<()> {
    let refuse = |what: &str| {
        ContractError::InvalidRecord(format!("terminal result generation usage {what}"))
    };
    let summary = value
        .and_then(Value::as_object)
        .ok_or_else(|| refuse("must be an object"))?;
    let count = |holder: Value<'_>, key: &str| {
        unsigned(field(holder, key, 0)?, key, SAFE_INTEGER).map_err(|cause| {
            refuse(&format!(
                "requires non-negative safe integer {key}: {cause}"
            ))
        })
    };
    let requests = count(summary, "requests")?;
    let reports = count(summary, "usageReports")?;
    let unfinalized = count(summary, "unfinalizedRequests")?;
    let unreported = count(summary, "unreportedUsageRequests")?;
    if Some(requests)
        != reports
            .checked_add(unfinalized)
            .and_then(|n| n.checked_add(unreported))
    {
        return Err(refuse("has an inconsistent request partition"));
    }
    let usage = summary
        .get("usage")
        .ok_or_else(|| refuse("requires nullable usage"))?;
    if reports == 0 {
        return if usage.is_null() {
            Ok(())
        } else {
            Err(refuse("has served usage without reports"))
        };
    }
    let usage = usage
        .as_object()
        .ok_or_else(|| refuse("requires served usage for reported requests"))?;
    let prompt = count(usage, "promptTokenCount")?;
    let output = count(usage, "candidatesTokenCount")?;
    let reasoning = count(usage, "thoughtsTokenCount")?;
    let cached = count(usage, "cachedContentTokenCount")?;
    let total = count(usage, "totalTokenCount")?;
    if prompt.checked_add(output) != Some(total) || reasoning > output || cached > prompt {
        return Err(refuse("has inconsistent served counts"));
    }
    Ok(())
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
fn validate_compaction_event(
    object: Value<'_>,
    line: usize,
    scope: Option<&str>,
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
    if !record
        .get("status")
        .and_then(Value::as_str)
        .is_some_and(|s| !s.is_empty())
    {
        return Err(refuse("without a non-empty status"));
    }
    if !record.get("succeeded").is_some_and(Value::is_boolean) {
        return Err(refuse("without a boolean succeeded"));
    }
    count(record, "originalTokenCount")?;
    count(record, "newTokenCount")?;
    string_or_null(record, "triggerReason")?;

    // What one drawn candidate spent. `budget` is the transition's frozen
    // output ceiling when the record reports it; every candidate was issued
    // under that same ceiling, so none of them may exceed it.
    let candidate = |holder: Value<'_>, whose: &str, budget: Option<u64>| -> ContractResult<()> {
        if count(holder, "requestAttempts")? == 0 {
            return Err(refuse(&format!(
                "whose {whose} reports no request attempt for a candidate that was drawn"
            )));
        }
        for key in ["reasoning", "summary"] {
            if !holder.get(key).is_some_and(Value::is_string) {
                return Err(refuse(&format!("whose {whose} lacks the {key} string")));
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
    for (index, attempt) in rejected.enumerate() {
        let whose = format!("rejected attempt {index}");
        let attempt = attempt
            .as_object()
            .ok_or_else(|| refuse(&format!("whose {whose} is not an object")))?;
        if !attempt
            .get("status")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty())
        {
            return Err(refuse(&format!(
                "whose {whose} has no non-empty status naming the rule it failed"
            )));
        }
        candidate(attempt, &whose, budget)?;
    }
    Ok(())
}

/// Every emitted event names its scope: `null` and an absent field both mean
/// the main session, any other value is the owning `agent` tool-call id. Only
/// null-or-absent is read as the main session, so a value of any other shape
/// can only ever exclude an event from the main thread, never admit one to it.
fn is_main_session_event(object: Value<'_>) -> bool {
    object.get("parent_tool_use_id").is_none_or(Value::is_null)
}

/// Read an event's scope: `None` is the main session, `Some(id)` the
/// spawning tool-call id. Which record terminates the stream and which row
/// absorbs the billing are both decided from this field, so a value of an
/// unexpected shape is contradictory evidence and is rejected here rather
/// than silently read as "some subagent".
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

/// Independently readable evidence in one complete record. The same observations
/// survive at terminal when a missing or damaged record prevents certification.
pub struct ObservedRecord {
    /// A completed main-thread model invocation, counted as a turn.
    pub main_turn: bool,
    /// The subagent scope this record belongs to, if it is not the main
    /// session. Read only for its identity; a shape this reader does not
    /// recognise names no scope rather than inventing one.
    pub subagent_scope: Option<String>,
    /// Served output and reasoning tokens, when the record bills a turn and
    /// every count it must carry is present and consistent.
    pub usage: Option<(u64, u64)>,
    /// The record is not fully interpretable, including unreadable billed usage.
    pub usage_unreadable: bool,
}

/// Read a completed record for live progress. Never fails: an unreadable
/// usage is reported as unreadable, not raised and not ignored.
pub fn observe_record(object: Value<'_>, limits: ValidationLimits) -> ObservedRecord {
    observe_record_admission(object, decode_event(object, limits, 0).is_err())
}
fn observe_record_admission(object: Value<'_>, record_invalid: bool) -> ObservedRecord {
    let kind = decode_event_kind(object, 0);
    if kind.is_err() || required_scope(object, 0).is_err() {
        return ObservedRecord {
            main_turn: false,
            subagent_scope: None,
            usage: None,
            usage_unreadable: true,
        };
    }
    let subagent_scope = required_scope(object, 0).ok().flatten().map(str::to_string);
    let observed_usage = if matches!(kind, Ok(EventKind::Assistant)) {
        billed_usage(object, 0, subagent_scope.as_deref())
    } else {
        Ok(None)
    };
    let usage_unreadable = observed_usage.is_err() || record_invalid;
    let usage = observed_usage
        .ok()
        .flatten()
        .map(|usage| (usage.output_tokens, usage.reasoning_output_tokens));

    ObservedRecord {
        main_turn: usage.is_some() && is_main_session_event(object),
        subagent_scope,
        usage,
        usage_unreadable,
    }
}

fn required_u64(object: Value<'_>, key: &str) -> ContractResult<u64> {
    unsigned(field(object, key, 0)?, key, u64::MAX)
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
    fn validate_init(&self, object: Value<'_>, line: usize) -> ContractResult<()> {
        if text(object, "type", line)? != "system"
            || text(object, "subtype", line)? != "init"
            || required_scope(object, line)?.is_some()
        {
            return Err(ContractError::InvalidRecord(format!(
                "events.jsonl line {line} is not the root initial system event"
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
    pub num_turns: u64,
    pub observed_output_tokens: u64,
    pub observed_reasoning_tokens: u64,
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
    id: Option<String>,
    billed_turns: u64,
    output_tokens: u64,
    reasoning_tokens: u64,
    terminal: Option<Terminal>,
    partial: PartialStreamState,
}
struct ToolUse {
    name: String,
    line: usize,
    scope: Option<String>,
    returned: bool,
}
struct AdmissionPlan {
    row: usize,
    state: ScopeState,
    additions: BTreeMap<String, ToolUse>,
    returns: BTreeSet<String>,
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
    identity: std::sync::Arc<()>,
    bindings: RuntimeBindings,
    limits: RuntimeLimits,
    session_id: Option<String>,
    seen_uuids: BTreeSet<String>,
    observed_scopes: BTreeSet<String>,
    records_seen: u64,
    prefix: u64,
    first_refusal: Option<ContractError>,
    observations: RuntimeObservations,
    scope_states: Vec<ScopeState>,
    scope_rows: BTreeMap<String, usize>,
    tool_uses: BTreeMap<String, ToolUse>,
    pending: Option<PendingAdmission>,
}
impl RuntimeContract {
    pub fn new(bindings: RuntimeBindings, limits: RuntimeLimits) -> Self {
        Self {
            identity: std::sync::Arc::new(()),
            bindings,
            limits,
            session_id: None,
            seen_uuids: BTreeSet::new(),
            observed_scopes: BTreeSet::new(),
            records_seen: 0,
            prefix: 0,
            first_refusal: None,
            observations: RuntimeObservations::default(),
            scope_states: vec![ScopeState::default()],
            scope_rows: BTreeMap::new(),
            tool_uses: BTreeMap::new(),
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
                self.bindings.validate_init(object, line)?;
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
        let item = observe_record_admission(object, decoded.is_err());
        // Compute the complete numerical update before installing any partner.
        let mut observed = self.observations;
        if item.main_turn {
            observed.num_turns = add(observed.num_turns, 1, "observed main turns")?;
        }
        if let Some((output, reasoning)) = item.usage {
            observed.observed_output_tokens = add(
                observed.observed_output_tokens,
                output,
                "observed output tokens",
            )?;
            observed.observed_reasoning_tokens = add(
                observed.observed_reasoning_tokens,
                reasoning,
                "observed reasoning tokens",
            )?;
        }
        if item.usage_unreadable {
            observed.observed_unaccounted_records = add(
                observed.observed_unaccounted_records,
                1,
                "unaccounted record count",
            )?;
        }
        if let Some(scope) = item.subagent_scope {
            self.observed_scopes.insert(scope);
        }
        observed.observed_subagent_scope_count = u64::try_from(self.observed_scopes.len())
            .map_err(|_| {
                ContractError::ValidationUnavailable("observed scope count overflow".into())
            })?;
        self.observations = observed;
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
        self.tool_uses.extend(plan.additions);
        for id in plan.returns {
            self.tool_uses
                .get_mut(&id)
                .expect("planned return names issued tool")
                .returned = true;
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
            let tool = self.tool_uses.get(id).ok_or_else(|| ContractError::InvalidRecord(format!("events.jsonl line {line} names parent_tool_use_id {id:?}, which no earlier assistant message issued as a tool_use id")))?;
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
        let mut additions = BTreeMap::new();
        let mut returns = BTreeSet::new();
        match record.kind() {
            EventKind::Assistant => {
                state.partial.complete_message(line)?;
                if let Some(content) = field(object, "message", line)?.get("content") {
                    let blocks = content.elements().ok_or_else(|| {
                        ContractError::InvalidRecord(format!(
                            "events.jsonl line {line} assistant content must be an array"
                        ))
                    })?;
                    for block in blocks {
                        let kind = text(block, "type", line)?;
                        if kind != "tool_use" {
                            continue;
                        }
                        let id = text(block, "id", line)?;
                        let name = text(block, "name", line)?;
                        if let Some(previous) = self.tool_uses.get(id).or_else(|| additions.get(id))
                        {
                            return Err(ContractError::InvalidRecord(format!("events.jsonl line {line} re-issues tool_use id {id:?}, first issued at line {} in {}", previous.line, scope_display(previous.scope.as_deref()))));
                        }
                        additions.insert(
                            id.to_string(),
                            ToolUse {
                                name: name.to_string(),
                                line,
                                scope: scope.map(str::to_string),
                                returned: false,
                            },
                        );
                    }
                }
                if let Some(usage) = billed_usage(object, line, scope)? {
                    state.billed_turns = add(state.billed_turns, 1, "billed turns")?;
                    state.output_tokens = add(
                        state.output_tokens,
                        usage.output_tokens,
                        "billed output tokens",
                    )?;
                    state.reasoning_tokens = add(
                        state.reasoning_tokens,
                        usage.reasoning_output_tokens,
                        "billed reasoning tokens",
                    )?;
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
                if text(event, "type", line)? == "tool_progress" {
                    self.require_tool_owner(text(event, "tool_use_id", line)?, scope, line)?;
                }
            }
            EventKind::System => {
                let subtype = SystemKind::from_wire(text(object, "subtype", line)?)
                    .expect("decoded system subtype");
                match subtype {
                    SystemKind::Init if self.prefix == 0 => {},
                    SystemKind::Init => return Err(ContractError::InvalidRecord(format!("events.jsonl line {line} starts another invocation before this invocation has closed"))),
                    SystemKind::Compaction => validate_compaction_event(object, line, scope)?,
                    SystemKind::SessionRecordingDegraded | SystemKind::TurnCleanupFailed | SystemKind::VisionBridgeFailed => return Err(ContractError::InvalidRecord(format!("events.jsonl line {line} reports operational failure {}: {}", subtype.wire(), field(object, "data", line)?.raw()))),
                    SystemKind::SessionStart | SystemKind::SessionEnd => return Err(ContractError::InvalidRecord(format!("events.jsonl line {line} declares transport ownership inside an already owned invocation"))),
                    SystemKind::TaskNotification => { if let Some(usage) = field(object, "data", line)?.get("usage") { validate_generation_summary(usage.get("ownerUsage"))?; } },
                    SystemKind::TaskStarted | SystemKind::WorktreeStarted | SystemKind::WorktreeRestored | SystemKind::VisionRouting | SystemKind::VisionBridge => {},
                }
            }
            EventKind::Result => {
                state.partial.finish(line)?;
                state.terminal = Some(terminal(object, line, scope.is_none())?);
                if scope.is_none() {
                    validate_main_counts(&state)?;
                }
            }
        }
        Ok(AdmissionPlan {
            row,
            state,
            additions,
            returns,
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
        let main = &self.scope_states[0];
        let terminal = main.terminal.as_ref().ok_or_else(|| {
            ContractError::InvalidRecord(format!(
                "events.jsonl has {} event(s) but no main-session terminal result",
                self.prefix
            ))
        })?;
        validate_main_counts(main)?;
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
                    billed_turns: state.billed_turns,
                    output_tokens: state.output_tokens,
                    reasoning_tokens: state.reasoning_tokens,
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
            main_output_tokens: main.output_tokens,
            main_reasoning_tokens: main.reasoning_tokens,
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
    let num_turns = required_u64(object, "num_turns")?;
    match object.get("usage") {
        Some(usage) if !root && usage.is_null() => {}
        usage => validate_generation_summary(usage)?,
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
fn validate_main_counts(state: &ScopeState) -> ContractResult<()> {
    let terminal = state
        .terminal
        .as_ref()
        .expect("terminal before count reconciliation");
    if !matches!(terminal.num_turns.checked_sub(state.billed_turns), Some(0))
        && !(terminal.is_error && terminal.num_turns.checked_sub(state.billed_turns) == Some(1))
    {
        return Err(ContractError::InvalidRecord(format!(
            "terminal num_turns={} is not consistent with {} main assistant event(s)",
            terminal.num_turns, state.billed_turns
        )));
    }
    Ok(())
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
            r#"{{"type":"system","subtype":"init","uuid":"init","session_id":"session","stream_contract_sha256":"{STREAM_CONTRACT_SHA256}","cwd":"/owned","model":"model","permission_mode":"default","qwen_code_version":"version","tools":["tool"],"agents":[],"slash_commands":[],"mcp_servers":[]}}"#
        )
    }
    fn admit(owner: &mut RuntimeContract, raw: &str) -> ContractResult<()> {
        let mut token = owner.prepare_utf8(raw.as_bytes(), 1)?;
        owner.commit(&mut token)
    }
    fn initialized() -> RuntimeContract {
        let mut owner = owner();
        admit(&mut owner, &init()).unwrap();
        owner
    }
    fn partial(id: &str, event: &str) -> String {
        format!(
            r#"{{"type":"stream_event","uuid":"{id}","session_id":"session","parent_tool_use_id":null,"event":{event}}}"#
        )
    }
    fn assistant(id: &str, scope: &str, content: &str, output: &str) -> String {
        format!(
            r#"{{"type":"assistant","uuid":"{id}","session_id":"session","parent_tool_use_id":{scope},"message":{{"content":{content},"usage":{{"input_tokens":0,"output_tokens":{output},"reasoning_output_tokens":0,"cache_read_input_tokens":0,"total_tokens":{output}}}}}}}"#
        )
    }
    fn issued_owner() -> RuntimeContract {
        let mut owner = initialized();
        admit(
            &mut owner,
            &assistant(
                "issued",
                "null",
                r#"[{"type":"tool_use","id":"tool-1","name":"tool","input":{}}]"#,
                "0",
            ),
        )
        .unwrap();
        owner
    }
    #[test]
    fn admission_waits_for_commit_and_tokens_belong_to_one_actual_owner() {
        let mut a = owner();
        let mut b = owner();
        let mut token = a.prepare_utf8(init().as_bytes(), 1).unwrap();
        assert_eq!(a.pending_bytes(&token).unwrap(), init());
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
        let mut token = b.prepare_utf8(init().as_bytes(), 1).unwrap();
        b.commit(&mut token).unwrap();
        assert_eq!(b.snapshot().certified_prefix_records, 1);
        assert!(!b.snapshot().pending_admission);
    }
    #[test]
    fn rejected_prepared_record_keeps_semantics_and_names_the_failed_barrier() {
        let mut owner = owner();
        let mut token = owner.prepare_utf8(init().as_bytes(), 1).unwrap();
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
    fn failing_last_tool_does_not_insert_earlier_tools_or_clear_partial_state() {
        let mut owner = initialized();
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
            "1",
        );
        assert!(admit(&mut owner, &raw).is_err());
        assert!(owner.tool_uses.is_empty());
        assert_eq!(owner.prefix, prefix);
        assert!(owner.scope_states[0].partial.finish(1).is_err());
        // The closed block still exists: message_stop cannot complete until the
        // corresponding assistant message was actually admitted.
        let document = Document::decode(
            partial("end", r#"{"type":"message_stop"}"#).as_bytes(),
            owner.limits.json,
        )
        .unwrap();
        let record = DecodedRecord::decode(document.root(), owner.limits.schema, 1).unwrap();
        let mut original = owner.scope_states[0].partial.clone();
        assert!(original
            .observe(record.partial().unwrap(), true, 1)
            .is_err());
        assert_eq!(owner.snapshot().observations.observed_output_tokens, 1);
    }
    #[test]
    fn raw_fractions_cannot_enter_the_integer_partial_domain() {
        for token in [
            "1e-400",
            "0.99999999999999999",
            "9007199254740991.00000000000000001",
        ] {
            let mut owner = initialized();
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
            admit(&mut owner, &assistant("served", "null", "[]", token)).unwrap();
            assert_eq!(owner.observations.num_turns, 1);
            assert_eq!(owner.observations.observed_output_tokens, 0);
        }
        for token in [
            "1e-400",
            "0.99999999999999999",
            "9007199254740991.00000000000000001",
            "9007199254740992",
            "1e1000001",
        ] {
            let mut owner = initialized();
            assert!(
                admit(&mut owner, &assistant("bad", "null", "[]", token)).is_err(),
                "{token}"
            );
            assert_eq!(owner.observations.num_turns, 0);
            assert_eq!(owner.observations.observed_unaccounted_records, 1);
        }
    }
    #[test]
    fn observations_after_refusal_cannot_restore_a_certificate_or_double_bill_identity() {
        let mut owner = initialized();
        assert!(admit(&mut owner, "{broken").is_err());
        let raw = assistant("served", "null", "[]", "1.0");
        assert!(admit(&mut owner, &raw).is_err());
        assert_eq!(owner.observations.observed_output_tokens, 1);
        assert!(admit(&mut owner, &raw)
            .unwrap_err()
            .to_string()
            .contains("repeats event uuid"));
        assert_eq!(owner.observations.observed_output_tokens, 1);
        assert_eq!(owner.prefix, 1);
        assert!(owner.finish().is_err());
    }
    #[test]
    fn wrong_owner_and_returned_tools_cannot_receive_results_or_progress() {
        let mut owner = issued_owner();
        let wrong = r#"{"type":"user","uuid":"wrong","session_id":"session","parent_tool_use_id":"tool-1","message":{"content":[{"type":"tool_result","tool_use_id":"tool-1","content":"done"}]}}"#;
        assert!(admit(&mut owner, wrong)
            .unwrap_err()
            .to_string()
            .contains("wrong-owner"));
        assert!(!owner.tool_uses["tool-1"].returned);
        let mut owner = issued_owner();
        let result = r#"{"type":"user","uuid":"result","session_id":"session","parent_tool_use_id":null,"message":{"content":[{"type":"tool_result","tool_use_id":"tool-1","content":"done"}]}}"#;
        admit(&mut owner, result).unwrap();
        assert!(owner.tool_uses["tool-1"].returned);
        let child = assistant("late-child", "\"tool-1\"", "[]", "0");
        assert!(admit(&mut owner, &child)
            .unwrap_err()
            .to_string()
            .contains("returned tool"));
    }
    #[test]
    fn overflow_latches_and_does_not_publish_a_partially_updated_observation() {
        let mut owner = initialized();
        owner.observations.observed_output_tokens = u64::MAX;
        let before = owner.observations;
        assert!(admit(&mut owner, &assistant("overflow", "null", "[]", "1")).is_err());
        assert_eq!(owner.observations, before);
        assert_eq!(owner.prefix, 1);
        assert!(owner
            .first_refusal
            .as_ref()
            .unwrap()
            .to_string()
            .contains("overflow"));
    }
}
