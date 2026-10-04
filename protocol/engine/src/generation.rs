//! Decoded generation authority; exact HTTP bytes remain physical evidence.
use crate::{
    json::{Document, Kind, Limits, Value},
    schema::{SchemaEntry, ValidationLimits},
    stream::{field, text, unsigned, validate},
    usage::ServedUsage,
    ContractError, ContractResult, SAFE_INTEGER,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn refusal(detail: &str) -> ContractError {
    ContractError::InvalidRecord(format!(
        "invalid generation evidence: {detail}; inspect the complete original recording with its matching client, or start a new session"
    ))
}

pub(crate) fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum OutputScope {
    Conversation { parent: Option<String> },
    Internal,
}

impl OutputScope {
    fn read(value: Value<'_>, line: usize) -> ContractResult<Self> {
        match text(value, "kind", line)? {
            "conversation" => {
                let parent = field(value, "parent_tool_use_id", line)?;
                Ok(Self::Conversation {
                    parent: if parent.is_null() {
                        None
                    } else {
                        Some(text(value, "parent_tool_use_id", line)?.into())
                    },
                })
            }
            "internal" => Ok(Self::Internal),
            _ => Err(refusal("generation has no declared output scope")),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Origin {
    pub attempt: String,
    pub scope: String,
}

impl Origin {
    pub fn read(value: Value<'_>, line: usize) -> ContractResult<Self> {
        if text(value, "kind", line)? != "model" {
            return Err(refusal("generation has no model origin"));
        }
        Ok(Self {
            attempt: text(value, "attempt_id", line)?.into(),
            scope: text(value, "kv_scope", line)?.into(),
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Call {
    pub id: String,
    pub name: Option<String>,
    pub arguments: Option<String>,
    object_arguments: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct IncompleteCall {
    pub name: Option<String>,
    pub arguments: String,
}

/// One non-thought part of the history an accepted generation commits, in order.
#[derive(Clone, Debug)]
enum Shown {
    Text(String),
    Call(usize),
    /// A part no headless assistant block represents.
    Unshowable,
}

/// One upstream-shaped assistant message a settled turn shows.
#[derive(Clone, Debug)]
pub(crate) struct DisplayMessage {
    /// The exact `content` array, as JSON text with call arguments verbatim.
    pub content_json: String,
    pub tool_use_only: bool,
    pub usage: Option<ServedUsage>,
}

#[derive(Clone, Debug)]
pub(crate) struct Generation {
    pub journal: String,
    pub id: String,
    pub hash: String,
    pub origin: Origin,
    pub output_scope: OutputScope,
    pub model: String,
    pub usage: Option<ServedUsage>,
    pub finish: Option<String>,
    pub observation_count: u64,
    pub source_requests: Vec<String>,
    pub text: String,
    pub display_text: String,
    pub thinking: String,
    pub calls: Vec<Call>,
    pub incomplete: Vec<IncompleteCall>,
    accepted_images_valid: bool,
    shown_thinking: String,
    shown: Vec<Shown>,
}

fn values(value: Option<Value<'_>>) -> Vec<Value<'_>> {
    value
        .and_then(Value::elements)
        .map(|items| items.collect())
        .unwrap_or_default()
}

fn truthy(value: Option<Value<'_>>) -> bool {
    value.is_some_and(|value| match value.kind() {
        Kind::Null => false,
        Kind::Boolean => value.as_bool() == Some(true),
        Kind::String => value.as_str() != Some(""),
        Kind::Number => value.raw().parse::<f64>().is_ok_and(|number| number != 0.0),
        Kind::Array | Kind::Object => true,
    })
}

fn primary_parts(response: Value<'_>) -> Vec<Value<'_>> {
    values(
        values(response.get("candidates"))
            .first()
            .and_then(|candidate| {
                candidate
                    .get("content")
                    .and_then(|content| content.get("parts"))
            }),
    )
}

fn valid_part(part: Value<'_>) -> bool {
    truthy(part.get("thought"))
        || truthy(part.get("thoughtSignature"))
        || part.get("text").and_then(Value::as_str) != Some("")
        || part.get("functionCall").is_some()
}

fn plain_text(part: Value<'_>) -> bool {
    part.get("text").is_some_and(Value::is_string)
        && [
            "thought",
            "thoughtSignature",
            "functionCall",
            "functionResponse",
            "inlineData",
            "fileData",
        ]
        .iter()
        .all(|key| !truthy(part.get(key)))
}

fn valid_response(response: Value<'_>, parts: &[Value<'_>]) -> bool {
    truthy(response.get("usageMetadata"))
        || values(response.get("candidates"))
            .iter()
            .any(|candidate| truthy(candidate.get("finishReason")))
        || (!parts.is_empty()
            && parts.iter().all(|part| {
                part.members()
                    .is_some_and(|mut members| members.next().is_some())
                    && valid_part(*part)
            }))
}

fn valid_image_references(part: Value<'_>) -> bool {
    let mut parts = vec![part];
    parts.extend(values(
        part.get("functionResponse")
            .and_then(|response| response.get("parts")),
    ));
    parts.into_iter().all(|source| {
        !source.is_null() && source.get("imageReferenceId").is_none_or(Value::is_string)
    })
}

fn finite_numbers(root: Value<'_>) -> ContractResult<()> {
    let mut pending = vec![root];
    while let Some(value) = pending.pop() {
        if value.kind() == Kind::Number && !value.raw().parse::<f64>().is_ok_and(f64::is_finite) {
            return Err(refusal("decoded observations contain a nonfinite number"));
        }
        if let Some(items) = value.elements() {
            pending.extend(items);
        }
        if let Some(members) = value.members() {
            pending.extend(members.map(|(_, value)| value));
        }
    }
    Ok(())
}

fn next_normalized_id(raw: Option<&str>, used: &BTreeSet<String>) -> ContractResult<String> {
    let (base, first) = match raw.filter(|id| !id.is_empty()) {
        Some(id) if !used.contains(id) => return Ok(id.to_string()),
        Some(id) => (id.to_string(), 2_u64),
        None => ("call_qwen".to_string(), 1_u64),
    };
    for suffix in first..=SAFE_INTEGER {
        let candidate = if raw.is_some_and(|id| !id.is_empty()) {
            format!("{base}__qwen_dup_{suffix}")
        } else {
            format!("{base}_{suffix}")
        };
        if !used.contains(&candidate) {
            return Ok(candidate);
        }
    }
    Err(refusal("normalization identity space is exhausted"))
}

impl Generation {
    pub fn read<F>(
        evidence: Value<'_>,
        line: usize,
        json: Limits,
        schema: ValidationLimits,
        seed_for_origin: F,
    ) -> ContractResult<Self>
    where
        F: FnOnce(&Origin) -> ContractResult<Vec<String>>,
    {
        validate(evidence, SchemaEntry::ModelGenerationEvidence, schema, line)?;
        let raw = text(evidence, "generation_json", line)?;
        let hash = text(evidence, "generation_sha256", line)?;
        if u64::try_from(raw.len()).ok()
            != Some(unsigned(
                field(evidence, "generation_bytes", line)?,
                "generation bytes",
                SAFE_INTEGER,
            )?)
            || sha256(raw.as_bytes()) != hash
        {
            return Err(refusal(
                "byte count or SHA-256 differs from the retained envelope",
            ));
        }
        let document = Document::decode(raw.as_bytes(), json).map_err(|error| match error {
            crate::json::DecodeError::ResourceLimit { resource, limit } => {
                ContractError::ValidationUnavailable(format!("generation {resource} limit {limit}"))
            }
            other => refusal(&format!("retained envelope is invalid JSON: {other:?}")),
        })?;
        let envelope = document.root();
        validate(envelope, SchemaEntry::GenerationEnvelope, schema, line)?;
        finite_numbers(envelope)?;
        let usage = field(envelope, "usage", line)?;
        let observations = values(envelope.get("observations"));
        let observation_count = u64::try_from(observations.len())
            .map_err(|_| refusal("observation count exceeds exact integer range"))?;
        let mut generation = Self {
            journal: text(evidence, "journal_id", line)?.into(),
            id: text(evidence, "generation_id", line)?.into(),
            hash: hash.into(),
            origin: Origin::read(field(envelope, "origin", line)?, line)?,
            output_scope: OutputScope::read(field(envelope, "output_scope", line)?, line)?,
            model: text(envelope, "model", line)?.into(),
            usage: if usage.is_null() {
                None
            } else {
                Some(ServedUsage::read(usage, line)?)
            },
            finish: field(envelope, "finish_reason", line)?
                .as_str()
                .map(str::to_string),
            observation_count,
            source_requests: Vec::with_capacity(observations.len()),
            text: String::new(),
            display_text: String::new(),
            thinking: String::new(),
            calls: Vec::new(),
            incomplete: Vec::new(),
            accepted_images_valid: true,
            shown_thinking: String::new(),
            shown: Vec::new(),
        };
        if let OutputScope::Conversation {
            parent: Some(parent),
        } = &generation.output_scope
        {
            if parent != &generation.origin.scope {
                return Err(refusal(
                    "conversation tool parent differs from the generation invocation",
                ));
            }
        }
        let mut raw_ids = BTreeSet::new();
        let mut used_ids: BTreeSet<String> = seed_for_origin(&generation.origin)?.into_iter().collect();
        let mut normalized_ids = BTreeSet::new();
        let mut preparations: BTreeMap<&str, (&str, &str)> = BTreeMap::new();
        let mut reservations: BTreeMap<&str, &str> = BTreeMap::new();
        let mut observed_usage = None;
        let mut observed_finish = None;
        let mut previous_plain_nonempty = false;
        for observation in observations {
            generation
                .source_requests
                .push(text(observation, "source_request_id", line)?.into());
            let response = field(observation, "response", line)?;
            if let Some(usage) = response
                .get("usageMetadata")
                .and_then(|usage| ServedUsage::read(usage, line).ok())
            {
                observed_usage = Some(usage);
            }
            for candidate in values(response.get("candidates")) {
                if observed_finish.is_none() {
                    observed_finish = candidate
                        .get("finishReason")
                        .and_then(Value::as_str)
                        .filter(|reason| !reason.is_empty())
                        .map(str::to_string);
                }
            }
            for preparation in values(observation.get("tool_call_preparations")) {
                let provider = text(preparation, "provider_call_id", line)?;
                let id = text(preparation, "callId", line)?;
                let name = text(preparation, "toolName", line)?;
                let expected = if let Some((reserved, _)) = preparations.get(provider) {
                    (*reserved).to_string()
                } else {
                    next_normalized_id(Some(provider), &used_ids)?
                };
                if id != expected {
                    return Err(refusal("preparation does not follow the recorded history seed"));
                }
                if preparations
                    .get(provider)
                    .is_some_and(|previous| *previous != (id, name))
                    || reservations
                        .get(id)
                        .is_some_and(|previous| *previous != provider)
                    || (!preparations.contains_key(provider) && normalized_ids.contains(id))
                {
                    return Err(refusal(
                        "preparation changes or reuses a normalization identity",
                    ));
                }
                preparations.insert(provider, (id, name));
                reservations.insert(id, provider);
                used_ids.insert(id.to_string());
            }
            let parts = primary_parts(response);
            for part in &parts {
                if part.get("text").is_some_and(|value| !value.is_string()) {
                    return Err(refusal("observed Part text is not a string"));
                }
                if !truthy(part.get("thought")) {
                    generation
                        .display_text
                        .push_str(part.get("text").and_then(Value::as_str).unwrap_or(""));
                }
                if !truthy(part.get("functionCall")) {
                    let target = if truthy(part.get("thought")) {
                        &mut generation.thinking
                    } else {
                        &mut generation.text
                    };
                    target.push_str(part.get("text").and_then(Value::as_str).unwrap_or(""));
                }
            }
            let mappings = values(observation.get("call_ids"));
            let mut mapped = 0;
            if valid_response(response, &parts) {
                for (position, part) in parts.iter().copied().enumerate() {
                    let call = part
                        .get("functionCall")
                        .filter(|value| truthy(Some(*value)));
                    let mut normalized_id = None;
                    if let Some(call) = call {
                        if call.as_object().is_none()
                            || call.get("id").is_some_and(|id| !id.is_string())
                        {
                            return Err(refusal("observed function call identity is invalid"));
                        }
                        let mapping = mappings
                            .get(mapped)
                            .ok_or_else(|| refusal("call mapping omits an observed call"))?;
                        mapped += 1;
                        let provider = call.get("id").and_then(Value::as_str);
                        let duplicate =
                            provider.is_some_and(|id| !id.is_empty() && raw_ids.contains(id));
                        let identity = field(*mapping, "normalized_id", line)?;
                        if unsigned(
                            field(*mapping, "part_index", line)?,
                            "call part index",
                            SAFE_INTEGER,
                        )? != position as u64
                            || identity.is_null() != duplicate
                        {
                            return Err(refusal(
                                "call mapping changes order or suppresses original output",
                            ));
                        }
                        if duplicate {
                            continue;
                        }
                        let id = text(*mapping, "normalized_id", line)?;
                        let expected = if let Some((reserved, _)) = provider
                            .filter(|value| !value.is_empty())
                            .and_then(|value| preparations.get(value))
                        {
                            (*reserved).to_string()
                        } else {
                            next_normalized_id(provider, &used_ids)?
                        };
                        if id != expected {
                            return Err(refusal("call mapping does not follow the recorded history seed"));
                        }
                        used_ids.insert(id.to_string());
                        if !normalized_ids.insert(id)
                            || reservations
                                .get(id)
                                .is_some_and(|reserved| Some(*reserved) != provider)
                            || provider
                                .filter(|id| !id.is_empty())
                                .and_then(|id| preparations.get(id))
                                .is_some_and(|(prepared, name)| {
                                    *prepared != id
                                        || Some(*name) != call.get("name").and_then(Value::as_str)
                                })
                        {
                            return Err(refusal(
                                "call mapping repeats or contradicts its preparation",
                            ));
                        }
                        if let Some(provider) = provider.filter(|id| !id.is_empty()) {
                            raw_ids.insert(provider);
                        }
                        normalized_id = Some(id);
                    }
                    // Thought consolidation discards nontext fields. Adjacent
                    // plain text merges retain only the earlier Part metadata.
                    if truthy(part.get("thought")) {
                        generation
                            .shown_thinking
                            .push_str(part.get("text").and_then(Value::as_str).unwrap_or(""));
                        continue;
                    }
                    let plain = plain_text(part);
                    if previous_plain_nonempty && plain {
                        if let Some(Shown::Text(last)) = generation.shown.last_mut() {
                            last.push_str(part.get("text").and_then(Value::as_str).unwrap_or(""));
                        }
                        continue;
                    }
                    if !valid_part(part) {
                        continue;
                    }
                    previous_plain_nonempty = plain && truthy(part.get("text"));
                    generation.accepted_images_valid &= valid_image_references(part);
                    if let (Some(call), Some(id)) = (call, normalized_id) {
                        generation.calls.push(Call {
                            id: id.into(),
                            name: call.get("name").and_then(Value::as_str).map(str::to_string),
                            arguments: call.get("args").map(|args| args.raw().to_string()),
                            object_arguments: call
                                .get("args")
                                .is_some_and(|args| args.as_object().is_some()),
                        });
                        generation.shown.push(Shown::Call(generation.calls.len() - 1));
                    } else {
                        let text_only = part.members().is_some_and(|mut members| {
                            members.all(|(key, _)| matches!(key, "text" | "thoughtSignature"))
                        });
                        generation.shown.push(match part.get("text").and_then(Value::as_str) {
                            Some(text) if text_only => Shown::Text(text.to_string()),
                            _ => Shown::Unshowable,
                        });
                    }
                }
            }
            if mapped != mappings.len() {
                return Err(refusal("call mapping invents an observed call"));
            }
            for call in values(observation.get("incomplete_tool_calls")) {
                generation.incomplete.push(IncompleteCall {
                    name: field(call, "name", line)?.as_str().map(str::to_string),
                    arguments: field(call, "arguments", line)?
                        .as_str()
                        .expect("schema-admitted call arguments")
                        .into(),
                });
            }
        }
        if generation.usage != observed_usage || generation.finish != observed_finish {
            return Err(refusal("summary contradicts complete observations"));
        }
        Ok(generation)
    }

    /// A refused draw is a turn the runtime draws again, and it has no call.
    /// The provider completed it at its output limit -- a length stop serves
    /// any call it was writing as text -- or the backend refused it
    /// (`by_backend`), and its response ended in that refusal having served
    /// no terminal, no usage and no call.
    pub fn require_refused(&self, by_backend: bool) -> ContractResult<()> {
        let limit = !by_backend
            && self.usage.is_some()
            && self.finish.as_deref() == Some("MAX_TOKENS");
        let backend = by_backend
            && self.usage.is_none()
            && self.finish.is_none()
            && self.incomplete.is_empty();
        if !(limit || backend) || !self.calls.is_empty() {
            return Err(refusal(
                "a refused draw must stop at its output limit, or end in its backend's refusal, without a call",
            ));
        }
        Ok(())
    }

    /// The assistant messages a settled turn shows, in upstream's headless
    /// grouping: one block kind per message, in the order the model produced
    /// them, the turn's served usage on the last message. The client derives
    /// the same messages from the same generation (`generationDisplay`).
    pub fn display(&self, refused: bool) -> ContractResult<Vec<DisplayMessage>> {
        let quote = |value: &str| serde_json::to_string(value).expect("string JSON");
        let mut blocks: Vec<(&'static str, String)> = Vec::new();
        if !self.shown_thinking.is_empty() {
            blocks.push((
                "thinking",
                format!(r#"{{"type":"thinking","thinking":{}}}"#, quote(&self.shown_thinking)),
            ));
        }
        for shown in &self.shown {
            match shown {
                Shown::Text(text) if text.is_empty() => {}
                Shown::Text(text) => blocks.push((
                    "text",
                    format!(r#"{{"type":"text","text":{}}}"#, quote(text)),
                )),
                Shown::Call(index) => {
                    let call = &self.calls[*index];
                    let (Some(name), Some(arguments), false) =
                        (&call.name, &call.arguments, refused)
                    else {
                        return Err(refusal("a displayed turn carries a call it cannot have made"));
                    };
                    blocks.push((
                        "tool_use",
                        format!(
                            r#"{{"type":"tool_use","id":{},"name":{},"input":{}}}"#,
                            quote(&call.id),
                            quote(name),
                            arguments
                        ),
                    ));
                }
                Shown::Unshowable => {
                    return Err(refusal(
                        "a displayed turn carries a model part no assistant block represents",
                    ))
                }
            }
        }
        for call in &self.incomplete {
            blocks.push((
                "incomplete_tool_use",
                format!(
                    r#"{{"type":"incomplete_tool_use","name":{},"arguments":{}}}"#,
                    call.name.as_deref().map_or_else(|| "null".to_string(), quote),
                    quote(&call.arguments)
                ),
            ));
        }
        let mut groups: Vec<Vec<(&'static str, String)>> = Vec::new();
        for block in blocks {
            match groups.last_mut() {
                Some(group) if group[0].0 == block.0 => group.push(block),
                _ => groups.push(vec![block]),
            }
        }
        if groups.is_empty() {
            groups.push(Vec::new());
        }
        let last = groups.len() - 1;
        Ok(groups
            .into_iter()
            .enumerate()
            .map(|(index, group)| DisplayMessage {
                tool_use_only: !group.is_empty() && group.iter().all(|(kind, _)| *kind == "tool_use"),
                content_json: format!(
                    "[{}]",
                    group.into_iter().map(|(_, json)| json).collect::<Vec<_>>().join(",")
                ),
                usage: if index == last { self.usage } else { None },
            })
            .collect())
    }

    pub fn require_accepted(&self) -> ContractResult<()> {
        if self.usage.is_none()
            || self.finish.is_none()
            || self.finish.as_deref() == Some("MAX_TOKENS")
            || !self.incomplete.is_empty()
        {
            return Err(refusal(
                "accepted history lacks a complete usable generation",
            ));
        }
        if !self.accepted_images_valid {
            return Err(refusal("accepted history has an invalid image reference"));
        }
        if self.calls.len() > 1
            || self.calls.iter().any(|call| {
                !call.name.as_ref().is_some_and(|name| !name.is_empty()) || !call.object_arguments
            })
        {
            return Err(refusal("accepted generation has invalid executable calls"));
        }
        Ok(())
    }
}
