use crate::{
    generation::{refusal, Generation, IncompleteCall, Origin},
    json::Value,
    stream::{field, text, unsigned},
    ContractResult, PartialKind, PartialRecord, SAFE_INTEGER,
};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Clone, Debug, Eq, PartialEq)]
enum OutputOrigin {
    Runtime,
    Model(Origin),
}

#[derive(Clone, Default)]
struct ObservedText {
    // Admission plans clone scope state. A prefix digest keeps that copy
    // constant-sized while the immutable capture retains the original bytes.
    bytes: usize,
    digest: Sha256,
}

impl ObservedText {
    fn append(&mut self, delta: &str, authority: Option<&str>) -> ContractResult<()> {
        let end = self
            .bytes
            .checked_add(delta.len())
            .ok_or_else(|| refusal("partial text length overflows"))?;
        if authority.is_some_and(|expected| {
            expected.as_bytes().get(self.bytes..end) != Some(delta.as_bytes())
        }) {
            return Err(refusal(
                "partial text changes or repeats observed generation bytes",
            ));
        }
        self.digest.update(delta.as_bytes());
        self.bytes = end;
        Ok(())
    }

    fn bind(&self, authority: &str) -> ContractResult<()> {
        let prefix = authority
            .as_bytes()
            .get(..self.bytes)
            .ok_or_else(|| refusal("partial text exceeds observed generation bytes"))?;
        if Sha256::digest(prefix) != self.digest.clone().finalize() {
            return Err(refusal(
                "partial text changes or repeats observed generation bytes",
            ));
        }
        Ok(())
    }
}

#[derive(Clone)]
struct PartialCall {
    expected: Arc<str>,
    observed: usize,
}

#[derive(Clone)]
struct Block {
    kind: String,
    open: bool,
    call: Option<PartialCall>,
}

/// Group markers own indexes; generation completion owns executable claims.
#[derive(Clone, Default)]
pub struct PartialStreamState {
    origin: Option<OutputOrigin>,
    authority: Option<Arc<Generation>>,
    accepted: Option<bool>,
    group_open: bool,
    text: ObservedText,
    thinking: ObservedText,
    called: BTreeSet<String>,
    incomplete: Vec<IncompleteCall>,
    blocks: BTreeMap<u64, Block>,
}

impl PartialStreamState {
    fn set_origin(&mut self, origin: OutputOrigin, line: usize) -> ContractResult<()> {
        if self.origin.as_ref() == Some(&origin) {
            return Ok(());
        }
        if self.origin.is_some() {
            self.finish(line)?;
        }
        *self = Self {
            origin: Some(origin),
            ..Self::default()
        };
        Ok(())
    }

    pub fn observe_origin(&mut self, origin: Value<'_>, line: usize) -> ContractResult<()> {
        self.set_origin(
            if text(origin, "kind", line)? == "runtime" {
                OutputOrigin::Runtime
            } else {
                OutputOrigin::Model(Origin::read(origin, line)?)
            },
            line,
        )
    }

    pub(crate) fn observe_generation(
        &mut self,
        generation: Arc<Generation>,
        line: usize,
    ) -> ContractResult<()> {
        self.set_origin(OutputOrigin::Model(generation.origin.clone()), line)?;
        if self.authority.is_some() {
            return Err(refusal("repeated partial generation authority"));
        }
        self.text.bind(&generation.text)?;
        self.thinking.bind(&generation.thinking)?;
        if self.incomplete.len() > generation.incomplete.len()
            || self
                .incomplete
                .iter()
                .zip(&generation.incomplete)
                .any(|(actual, expected)| actual != expected)
        {
            return Err(refusal(
                "incomplete call contradicts the observed generation",
            ));
        }
        self.authority = Some(generation);
        Ok(())
    }

    pub(crate) fn observe_completion(
        &mut self,
        origin: &Origin,
        accepted: bool,
    ) -> ContractResult<()> {
        if self.authority.is_none()
            || self.origin.as_ref() != Some(&OutputOrigin::Model(origin.clone()))
            || self.accepted.is_some()
        {
            return Err(refusal(
                "completion has no unique matching partial authority",
            ));
        }
        self.accepted = Some(accepted);
        Ok(())
    }

    pub fn observe(
        &mut self,
        partial: PartialRecord<'_>,
        _root: bool,
        line: usize,
    ) -> ContractResult<()> {
        let event = partial.value();
        let kind = partial.kind();
        if matches!(
            kind,
            PartialKind::GoalState | PartialKind::ActiveGoal | PartialKind::ToolProgress
        ) {
            return Ok(());
        }
        let model = matches!(self.origin, Some(OutputOrigin::Model(_)));
        if self.origin.is_none() {
            return Err(refusal("message group has no producing origin"));
        }
        match kind {
            PartialKind::MessageStart => {
                if self.group_open || !self.blocks.is_empty() {
                    return Err(refusal("message group is already open"));
                }
                self.group_open = true;
            }
            PartialKind::ContentBlockStart => {
                if !self.group_open {
                    return Err(refusal("content block precedes its message group"));
                }
                let index = unsigned(field(event, "index", line)?, "partial index", SAFE_INTEGER)?;
                if usize::try_from(index).ok() != Some(self.blocks.len()) {
                    return Err(refusal("content block index is not the next group index"));
                }
                let content = field(event, "content_block", line)?;
                let kind = text(content, "type", line)?;
                let mut block = Block {
                    kind: kind.into(),
                    open: true,
                    call: None,
                };
                match kind {
                    "text" => self.text.append(
                        field(content, "text", line)?.as_str().unwrap_or(""),
                        self.authority
                            .as_ref()
                            .map(|authority| authority.text.as_str()),
                    )?,
                    "thinking" => {
                        if !model {
                            return Err(refusal("runtime presentation cannot claim model thought"));
                        }
                        self.thinking.append(
                            field(content, "thinking", line)?.as_str().unwrap_or(""),
                            self.authority
                                .as_ref()
                                .map(|authority| authority.thinking.as_str()),
                        )?;
                    }
                    "tool_use" => {
                        if !model || self.accepted != Some(true) {
                            return Err(refusal(
                                "tool claim precedes accepted generation completion",
                            ));
                        }
                        let id = text(content, "id", line)?;
                        let name = text(content, "name", line)?;
                        let call = self
                            .authority
                            .as_ref()
                            .expect("accepted authority")
                            .calls
                            .iter()
                            .find(|call| call.id == id)
                            .ok_or_else(|| {
                                refusal("tool claim has no normalized generation call")
                            })?;
                        if self.called.contains(id)
                            || call.name.as_deref() != Some(name)
                            || call.arguments.is_none()
                        {
                            return Err(refusal(
                                "tool claim has no unique normalized generation call",
                            ));
                        }
                        if !field(content, "input", line)?
                            .members()
                            .is_some_and(|mut members| members.next().is_none())
                        {
                            return Err(refusal(
                                "streamed tool must start with an empty argument placeholder",
                            ));
                        }
                        block.call = Some(PartialCall {
                            expected: Arc::from(call.arguments.as_deref().unwrap()),
                            observed: 0,
                        });
                        self.called.insert(id.into());
                    }
                    "incomplete_tool_use" => {
                        if !model {
                            return Err(refusal(
                                "runtime presentation cannot claim incomplete model calls",
                            ));
                        }
                        let call = IncompleteCall {
                            name: field(content, "name", line)?.as_str().map(str::to_string),
                            arguments: field(content, "arguments", line)?
                                .as_str()
                                .expect("schema-admitted incomplete arguments")
                                .into(),
                        };
                        if self.authority.as_ref().is_some_and(|authority| {
                            authority.incomplete.get(self.incomplete.len()) != Some(&call)
                        }) {
                            return Err(refusal("incomplete call contradicts observed generation"));
                        }
                        self.incomplete.push(call);
                    }
                    _ => return Err(refusal("unsupported partial content block")),
                }
                self.blocks.insert(index, block);
            }
            PartialKind::ContentBlockDelta | PartialKind::ContentBlockStop => {
                let index = unsigned(field(event, "index", line)?, "partial index", SAFE_INTEGER)?;
                let block = self
                    .blocks
                    .get_mut(&index)
                    .filter(|block| block.open)
                    .ok_or_else(|| refusal("content block is absent or closed"))?;
                if kind == PartialKind::ContentBlockStop {
                    if block
                        .call
                        .as_ref()
                        .is_some_and(|call| call.observed != call.expected.len())
                    {
                        return Err(refusal("tool arguments omit or change generation bytes"));
                    }
                    block.open = false;
                    return Ok(());
                }
                let delta = field(event, "delta", line)?;
                match (block.kind.as_str(), text(delta, "type", line)?) {
                    ("text", "text_delta") => self.text.append(
                        field(delta, "text", line)?
                            .as_str()
                            .expect("schema-admitted text"),
                        self.authority
                            .as_ref()
                            .map(|authority| authority.text.as_str()),
                    )?,
                    ("thinking", "thinking_delta") => self.thinking.append(
                        field(delta, "thinking", line)?
                            .as_str()
                            .expect("schema-admitted thought"),
                        self.authority
                            .as_ref()
                            .map(|authority| authority.thinking.as_str()),
                    )?,
                    ("tool_use", "input_json_delta") => {
                        let call = block.call.as_mut().expect("accepted tool claim");
                        let chunk = field(delta, "partial_json", line)?
                            .as_str()
                            .expect("schema-admitted argument text");
                        let end = call
                            .observed
                            .checked_add(chunk.len())
                            .ok_or_else(|| refusal("partial argument length overflows"))?;
                        if call.expected.as_bytes().get(call.observed..end)
                            != Some(chunk.as_bytes())
                        {
                            return Err(refusal("tool argument bytes contradict the generation"));
                        }
                        call.observed = end;
                    }
                    _ => return Err(refusal("delta type contradicts its content block")),
                }
            }
            PartialKind::MessageStop => {
                if !self.group_open || self.blocks.values().any(|block| block.open) {
                    return Err(refusal("message group stops before all its blocks close"));
                }
                self.group_open = false;
                self.blocks.clear();
                if !model {
                    self.text = ObservedText::default();
                }
            }
            PartialKind::GoalState | PartialKind::ActiveGoal | PartialKind::ToolProgress => {
                unreachable!("notice returned before group admission")
            }
        }
        Ok(())
    }

    pub fn complete_message(&mut self, _line: usize) -> ContractResult<()> {
        if matches!(self.origin, Some(OutputOrigin::Model(_))) {
            return Err(refusal(
                "model output requires generation and completion records",
            ));
        }
        if self.blocks.values().any(|block| block.open) {
            return Err(refusal(
                "runtime presentation has an unclosed partial block",
            ));
        }
        Ok(())
    }

    pub fn finish(&self, _line: usize) -> ContractResult<()> {
        if self.group_open || !self.blocks.is_empty() {
            return Err(refusal("scope has unfinished partial groups"));
        }
        if matches!(self.origin, Some(OutputOrigin::Model(_)))
            && (self.authority.is_none() || self.accepted.is_none())
        {
            return Err(refusal(
                "model partials have no completed generation authority",
            ));
        }
        Ok(())
    }
}
