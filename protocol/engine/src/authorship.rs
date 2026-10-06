//! Who wrote each byte of a recorded request body.
//!
//! A `model_request` record carries, beside the body it stores, a table of
//! runs that tiles every stored byte: each run names its author -- the model,
//! the world, the operator or the harness -- and its length in stored bytes.
//! The certifier holds the table to the body structurally and to the authors
//! it can read: every stored byte is in exactly one run, a run any author
//! other than the harness wrote lies inside one JSON string value and starts
//! and ends on whole characters, a run the model wrote is byte for byte the
//! output it names, and a run the operator wrote is byte for byte the task at
//! the offset it names. Nothing here knows how a client composes its
//! messages: the rules are about bytes, strings and the outputs they cite.
use crate::{
    generation::Generation,
    json::{Document, Kind, Value},
    stream::{field, text, unsigned},
    ContractError, ContractResult, SAFE_INTEGER,
};
use std::{
    borrow::Cow,
    collections::BTreeMap,
    ops::Range,
    sync::Arc,
};

fn refusal(detail: &str) -> ContractError {
    ContractError::InvalidRecord(format!(
        "model request authorship {detail}; inspect the complete original recording with its matching client"
    ))
}

/// The task the operator started the session with, exactly as submitted.
#[derive(Clone, Debug)]
pub struct OperatorTask(Arc<[u8]>);

impl OperatorTask {
    pub fn new(bytes: &[u8]) -> Self {
        Self(Arc::from(bytes))
    }
}

/// What one run claims about its author.
#[derive(Debug)]
enum Claim<'a> {
    Harness,
    World,
    Operator { offset: u64 },
    Model { kind: &'a str, id: &'a str, field: &'a str, offset: u64 },
}

struct Run<'a> {
    bytes: usize,
    claim: Claim<'a>,
}

fn read_runs<'a>(value: Value<'a>, line: usize) -> ContractResult<Vec<Run<'a>>> {
    let mut runs = Vec::new();
    for item in value
        .elements()
        .ok_or_else(|| refusal("lists its runs in something other than an array"))?
    {
        let bytes = unsigned(field(item, "bytes", line)?, "authored run bytes", SAFE_INTEGER)?;
        let bytes = usize::try_from(bytes)
            .ok()
            .filter(|bytes| *bytes > 0)
            .ok_or_else(|| refusal("has a run with no bytes"))?;
        let claim = match text(item, "author", line)? {
            "harness" => Claim::Harness,
            "world" => Claim::World,
            "operator" => Claim::Operator {
                offset: unsigned(field(item, "offset", line)?, "operator offset", SAFE_INTEGER)?,
            },
            "model" => {
                let source = field(item, "source", line)?;
                Claim::Model {
                    kind: text(source, "kind", line)?,
                    id: text(source, "id", line)?,
                    field: text(source, "field", line)?,
                    offset: unsigned(field(source, "offset", line)?, "model source offset", SAFE_INTEGER)?,
                }
            }
            _ => return Err(refusal("names an unknown author")),
        };
        runs.push(Run { bytes, claim });
    }
    Ok(runs)
}

/// A segment of the stored body and the runs that must tile it.
struct Segment<'a> {
    start: usize,
    len: usize,
    runs: Vec<Run<'a>>,
}

/// The decoded text of one JSON string value, with the stored offset of
/// every character boundary inside it and the decoded offset it maps to.
struct StringValue {
    content: Range<usize>,
    boundaries: Vec<(usize, usize)>,
    decoded: String,
}

impl StringValue {
    /// Decode a string token whose content (between its quotes) is
    /// `source[content]`. A surrogate pair written as two escapes is one
    /// character, so the offset between its escapes is not a boundary.
    fn read(source: &str, content: Range<usize>) -> Option<Self> {
        let raw = &source.as_bytes()[content.clone()];
        let mut boundaries = Vec::new();
        let mut decoded = String::new();
        let mut at = 0;
        let hex = |at: usize| -> Option<u32> {
            let digits = raw.get(at..at + 4)?;
            u32::from_str_radix(std::str::from_utf8(digits).ok()?, 16).ok()
        };
        while at < raw.len() {
            boundaries.push((content.start + at, decoded.len()));
            if raw[at] != b'\\' {
                let rest = std::str::from_utf8(&source.as_bytes()[content.start + at..content.end]).ok()?;
                let character = rest.chars().next()?;
                decoded.push(character);
                at += character.len_utf8();
                continue;
            }
            let escape = *raw.get(at + 1)?;
            let (character, width) = match escape {
                b'"' => ('"', 2),
                b'\\' => ('\\', 2),
                b'/' => ('/', 2),
                b'b' => ('\u{8}', 2),
                b'f' => ('\u{c}', 2),
                b'n' => ('\n', 2),
                b'r' => ('\r', 2),
                b't' => ('\t', 2),
                b'u' => {
                    let first = hex(at + 2)?;
                    if (0xd800..0xdc00).contains(&first) {
                        if raw.get(at + 6..at + 8) != Some(b"\\u") {
                            return None;
                        }
                        let second = hex(at + 8)?;
                        if !(0xdc00..0xe000).contains(&second) {
                            return None;
                        }
                        let scalar = 0x10000 + ((first - 0xd800) << 10) + (second - 0xdc00);
                        (char::from_u32(scalar)?, 12)
                    } else {
                        (char::from_u32(first)?, 6)
                    }
                }
                _ => return None,
            };
            decoded.push(character);
            at += width;
        }
        boundaries.push((content.end, decoded.len()));
        Some(Self {
            content,
            boundaries,
            decoded,
        })
    }

    fn decoded_offset(&self, stored: usize) -> Option<usize> {
        self.boundaries
            .binary_search_by_key(&stored, |(at, _)| *at)
            .ok()
            .map(|index| self.boundaries[index].1)
    }
}

/// Every string value in a document, by where its content starts.
fn string_values(document: &Document) -> BTreeMap<usize, Range<usize>> {
    let mut strings = BTreeMap::new();
    let mut pending = vec![document.root()];
    while let Some(value) = pending.pop() {
        match value.kind() {
            Kind::String => {
                let span = value.byte_range();
                strings.insert(span.start + 1, span.start + 1..span.end - 1);
            }
            Kind::Array => pending.extend(value.elements().into_iter().flatten()),
            Kind::Object => pending.extend(
                value
                    .members()
                    .into_iter()
                    .flatten()
                    .map(|(_, member)| member),
            ),
            _ => {}
        }
    }
    strings
}

/// The outputs runs of model text may cite: admitted generations by their
/// identity, and the decoded calls of claimed compaction draws by the draw's
/// physical operation.
pub(crate) struct ModelOutputs<'a> {
    pub generations: &'a BTreeMap<String, Arc<Generation>>,
    pub draws: &'a BTreeMap<String, DrawOutput>,
}

/// What a claimed compaction draw wrote, as its recorded response decodes.
#[derive(Clone, Debug, Default)]
pub(crate) struct DrawOutput {
    pub reasoning: String,
    pub text: String,
    pub calls: Vec<serde_json::Value>,
}

/// A string inside a call's arguments, by JSON pointer.
fn pointed<'a>(arguments: &'a serde_json::Value, pointer: &str) -> Option<&'a str> {
    arguments.pointer(pointer)?.as_str()
}

/// `/calls/<index>` and what follows it in a cited field.
fn call_field(field: &str) -> Option<(usize, &str)> {
    let rest = field.strip_prefix("/calls/")?;
    let (index, rest) = rest.split_at(rest.find('/')?);
    if index.is_empty() || (index.len() > 1 && index.starts_with('0')) {
        return None;
    }
    Some((index.parse().ok()?, rest))
}

impl ModelOutputs<'_> {
    fn field(&self, kind: &str, id: &str, field: &str) -> ContractResult<Cow<'_, str>> {
        let missing = || refusal(&format!("cites model output {kind} {id:?} at {field:?}, which it does not have"));
        match kind {
            "generation" => {
                let generation = self.generations.get(id).ok_or_else(missing)?;
                match field {
                    "/reasoning" => Ok(Cow::Borrowed(generation.shown_reasoning())),
                    "/text" => Ok(Cow::Owned(generation.shown_text())),
                    _ => {
                        let (index, rest) = call_field(field).ok_or_else(missing)?;
                        let call = generation.calls.get(index).ok_or_else(missing)?;
                        match rest {
                            "/name" => call.name.as_deref().map(Cow::Borrowed).ok_or_else(missing),
                            _ => {
                                let pointer = rest.strip_prefix("/args").ok_or_else(missing)?;
                                let arguments: serde_json::Value = serde_json::from_str(
                                    call.arguments.as_deref().ok_or_else(missing)?,
                                )
                                .map_err(|_| missing())?;
                                pointed(&arguments, pointer)
                                    .map(|text| Cow::Owned(text.to_string()))
                                    .ok_or_else(missing)
                            }
                        }
                    }
                }
            }
            "compaction_draw" => {
                let draw = self.draws.get(id).ok_or_else(missing)?;
                match field {
                    "/reasoning" => Ok(Cow::Borrowed(draw.reasoning.as_str())),
                    "/text" => Ok(Cow::Borrowed(draw.text.as_str())),
                    _ => {
                        let (index, rest) = call_field(field).ok_or_else(missing)?;
                        let call = draw.calls.get(index).ok_or_else(missing)?;
                        if rest == "/name" {
                            return call
                                .get("name")
                                .and_then(serde_json::Value::as_str)
                                .map(Cow::Borrowed)
                                .ok_or_else(missing);
                        }
                        let pointer = rest.strip_prefix("/args").ok_or_else(missing)?;
                        call.get("args")
                            .and_then(|arguments| pointed(arguments, pointer))
                            .map(Cow::Borrowed)
                            .ok_or_else(missing)
                    }
                }
            }
            _ => Err(refusal("cites an unknown kind of model output")),
        }
    }
}

/// Read the table a request's body carries and hold it to the body.
///
/// `json` is the reconstructed body and `document` its decoding; `messages`
/// is the stored length of every message in it, and `added` how many of the
/// last ones this record adds (all of them for a full body).
pub(crate) fn check(
    body: Value<'_>,
    json: &str,
    document: &Document,
    messages: &[usize],
    outputs: &ModelOutputs<'_>,
    operator: &OperatorTask,
    line: usize,
) -> ContractResult<()> {
    let authors = field(body, "authors", line)?;
    let segments = match text(body, "kind", line)? {
        "full" => vec![Segment {
            start: 0,
            len: json.len(),
            runs: read_runs(authors, line)?,
        }],
        "delta" => {
            let prefix = text(body, "prefix", line)?.len();
            let suffix = text(body, "suffix", line)?.len();
            let added = field(body, "added_messages", line)?
                .elements()
                .map(|items| items.len())
                .unwrap_or_default();
            let tables: Vec<Value<'_>> = field(authors, "added_messages", line)?
                .elements()
                .ok_or_else(|| refusal("lists its added messages' runs in something other than an array"))?
                .collect();
            if tables.len() != added || added > messages.len() {
                return Err(refusal("does not give one table to every message the record adds"));
            }
            let mut segments = vec![Segment {
                start: 0,
                len: prefix,
                runs: read_runs(field(authors, "prefix", line)?, line)?,
            }];
            let mut at = prefix;
            let first = messages.len() - added;
            for (index, length) in messages.iter().enumerate() {
                if index >= first {
                    segments.push(Segment {
                        start: at,
                        len: *length,
                        runs: read_runs(tables[index - first], line)?,
                    });
                }
                at += length + 1;
            }
            segments.push(Segment {
                start: json.len() - suffix,
                len: suffix,
                runs: read_runs(field(authors, "suffix", line)?, line)?,
            });
            segments
        }
        _ => return Err(refusal("belongs to an unknown body representation")),
    };
    let strings = string_values(document);
    let mut decoded: BTreeMap<usize, StringValue> = BTreeMap::new();
    for segment in &segments {
        let mut at = segment.start;
        let mut previous: Option<(&Claim<'_>, usize)> = None;
        for run in &segment.runs {
            let start = at;
            let end = start
                .checked_add(run.bytes)
                .filter(|end| *end <= segment.start + segment.len)
                .ok_or_else(|| refusal("has runs that reach past the bytes they describe"))?;
            at = end;
            let written = match run.claim {
                Claim::Harness => {
                    if matches!(previous, Some((Claim::Harness, _))) {
                        return Err(refusal("splits one author's bytes into adjacent runs"));
                    }
                    previous = Some((&run.claim, 0));
                    continue;
                }
                _ => {
                    let (&content_start, content) = strings
                        .range(..=start)
                        .next_back()
                        .ok_or_else(|| refusal("attributes bytes outside a string value to an author other than the harness"))?;
                    if end > content.end {
                        return Err(refusal("attributes bytes outside a string value to an author other than the harness"));
                    }
                    if !decoded.contains_key(&content_start) {
                        let value = StringValue::read(json, content.clone())
                            .ok_or_else(|| refusal("covers a string value it cannot decode"))?;
                        decoded.insert(content_start, value);
                    }
                    let value = &decoded[&content_start];
                    let (Some(from), Some(to)) = (value.decoded_offset(start), value.decoded_offset(end)) else {
                        return Err(refusal("starts or ends a run inside a character"));
                    };
                    debug_assert!(value.content.start <= start);
                    &value.decoded.as_bytes()[from..to]
                }
            };
            let contiguous = |offset: u64| {
                previous.is_some_and(|(claim, length)| match (claim, &run.claim) {
                    (Claim::Operator { offset: before }, Claim::Operator { .. }) => {
                        before + length as u64 == offset
                    }
                    (
                        Claim::Model { kind, id, field, offset: before },
                        Claim::Model { kind: k, id: i, field: f, .. },
                    ) => kind == k && id == i && field == f && before + length as u64 == offset,
                    _ => false,
                })
            };
            match run.claim {
                Claim::Harness => unreachable!("handled above"),
                Claim::World => {
                    if matches!(previous, Some((Claim::World, _))) {
                        return Err(refusal("splits one author's bytes into adjacent runs"));
                    }
                }
                Claim::Operator { offset } => {
                    if contiguous(offset) {
                        return Err(refusal("splits one author's bytes into adjacent runs"));
                    }
                    let from = usize::try_from(offset).map_err(|_| refusal("cites an operator offset past the task"))?;
                    if operator.0.get(from..from + written.len()) != Some(written) {
                        return Err(refusal(&format!(
                            "attributes {} bytes to the operator that are not the task's bytes at offset {offset}",
                            written.len()
                        )));
                    }
                }
                Claim::Model { kind, id, field, offset } => {
                    if contiguous(offset) {
                        return Err(refusal("splits one author's bytes into adjacent runs"));
                    }
                    let source = outputs.field(kind, id, field)?;
                    let from = usize::try_from(offset).map_err(|_| refusal("cites a model offset past its output"))?;
                    if source.as_bytes().get(from..from + written.len()) != Some(written) {
                        return Err(refusal(&format!(
                            "attributes {} bytes to the model that are not the bytes of {kind} {id:?} at {field:?} offset {offset}",
                            written.len()
                        )));
                    }
                }
            }
            previous = Some((&run.claim, written.len()));
        }
        if at != segment.start + segment.len {
            return Err(refusal(&format!(
                "leaves stored bytes {at}..{} without an author",
                segment.start + segment.len
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::Limits;
    use serde_json::json;

    const LIMITS: Limits = Limits {
        bytes: 1_000_000,
        nodes: 100_000,
        depth: 100,
    };

    fn draws() -> BTreeMap<String, DrawOutput> {
        BTreeMap::from([(
            "draw".to_string(),
            DrawOutput {
                reasoning: String::new(),
                text: String::new(),
                calls: vec![json!({"name":"state_snapshot","args":{"current_work":"edit \"a\"\nthen b 😀"}})],
            },
        )])
    }

    /// Check `authors` against the full body `json` with the task `task`.
    fn full(json: &str, authors: serde_json::Value, task: &[u8]) -> ContractResult<()> {
        let record = json!({"kind":"full","json":json,"authors":authors}).to_string();
        let record = Document::decode(record.as_bytes(), LIMITS).unwrap();
        let body = Document::decode(json.as_bytes(), LIMITS).unwrap();
        let generations = BTreeMap::new();
        let draws = draws();
        check(
            record.root(),
            json,
            &body,
            &[],
            &ModelOutputs {
                generations: &generations,
                draws: &draws,
            },
            &OperatorTask::new(task),
            1,
        )
    }

    fn harness(bytes: usize) -> serde_json::Value {
        json!({"author":"harness","bytes":bytes})
    }

    /// The body every case below attributes: a user message holding the
    /// operator's task, a frame, and a section the model wrote in a draw.
    fn body() -> (String, usize, usize, usize) {
        let json = json!({"messages":[{"role":"user","content":"Fix it.<s>edit \"a\"\nthen b 😀</s>"}]}).to_string();
        let task = json.find("Fix it.").unwrap();
        let section = json.find("edit").unwrap();
        let end = json.find("</s>").unwrap();
        (json, task, section, end)
    }

    #[test]
    fn a_table_that_tiles_the_body_with_checked_authors_is_admitted() {
        let (json, task, section, end) = body();
        let authors = json!([
            harness(task),
            {"author":"operator","bytes":7,"offset":0},
            harness(section - task - 7),
            {"author":"model","bytes":end - section,"source":{"kind":"compaction_draw","id":"draw","field":"/calls/0/args/current_work","offset":0}},
            harness(json.len() - end),
        ]);
        full(&json, authors, b"Fix it.").unwrap();
    }

    #[test]
    fn every_stored_byte_has_exactly_one_author() {
        let (json, ..) = body();
        let short = full(&json, json!([harness(json.len() - 1)]), b"").unwrap_err();
        assert!(short.to_string().contains("without an author"), "{short}");
        let long = full(&json, json!([harness(json.len() + 1)]), b"").unwrap_err();
        assert!(long.to_string().contains("reach past"), "{long}");
        full(&json, json!([harness(json.len())]), b"").unwrap();
        let split = full(&json, json!([harness(3), harness(json.len() - 3)]), b"").unwrap_err();
        assert!(split.to_string().contains("adjacent runs"), "{split}");
    }

    #[test]
    fn only_the_harness_writes_outside_string_values() {
        let (json, task, ..) = body();
        // A world run that starts on the quote before the task.
        let authors = json!([harness(task - 1), {"author":"world","bytes":8}, harness(json.len() - task - 7)]);
        let error = full(&json, authors, b"").unwrap_err();
        assert!(error.to_string().contains("outside a string value"), "{error}");
        // A key is not a string value either.
        let key = json.find("role").unwrap();
        let authors = json!([harness(key), {"author":"world","bytes":4}, harness(json.len() - key - 4)]);
        let error = full(&json, authors, b"").unwrap_err();
        assert!(error.to_string().contains("outside a string value"), "{error}");
    }

    #[test]
    fn a_run_starts_and_ends_on_whole_characters() {
        let (json, _, section, _) = body();
        // Inside the escape `\"` of the section.
        let quote = section + json[section..].find('\\').unwrap() + 1;
        let authors = json!([harness(quote), {"author":"world","bytes":3}, harness(json.len() - quote - 3)]);
        let error = full(&json, authors, b"").unwrap_err();
        assert!(error.to_string().contains("inside a character"), "{error}");
        // Inside the four bytes of the emoji.
        let emoji = json.find('😀').unwrap() + 1;
        let authors = json!([harness(emoji), {"author":"world","bytes":3}, harness(json.len() - emoji - 3)]);
        let error = full(&json, authors, b"").unwrap_err();
        assert!(error.to_string().contains("inside a character"), "{error}");
    }

    #[test]
    fn operator_text_is_the_task_at_its_offset() {
        let (json, task, ..) = body();
        let attribute = |offset: u64, submitted: &[u8]| {
            full(
                &json,
                json!([harness(task), {"author":"operator","bytes":7,"offset":offset}, harness(json.len() - task - 7)]),
                submitted,
            )
        };
        attribute(4, b"Do: Fix it. Then stop.").unwrap();
        for (offset, submitted) in [(0, b"Fix it!".as_slice()), (1, b"Fix it."), (0, b"Fix")] {
            let error = attribute(offset, submitted).unwrap_err();
            assert!(error.to_string().contains("not the task's bytes"), "{error}");
        }
    }

    #[test]
    fn model_text_is_the_output_it_cites_byte_for_byte() {
        let (json, _, section, end) = body();
        let attribute = |source: serde_json::Value| {
            full(
                &json,
                json!([harness(section), {"author":"model","bytes":end - section,"source":source}, harness(json.len() - end)]),
                b"",
            )
        };
        let source = |field: &str, offset: u64, id: &str| {
            json!({"kind":"compaction_draw","id":id,"field":field,"offset":offset})
        };
        attribute(source("/calls/0/args/current_work", 0, "draw")).unwrap();
        let shifted = attribute(source("/calls/0/args/current_work", 1, "draw")).unwrap_err();
        assert!(shifted.to_string().contains("not the bytes of"), "{shifted}");
        for missing in [
            source("/calls/0/args/next_step", 0, "draw"),
            source("/calls/1/args/current_work", 0, "draw"),
            source("/calls/00/args/current_work", 0, "draw"),
            source("/calls/0/args/current_work", 0, "other"),
            source("/calls/0/arguments", 0, "draw"),
        ] {
            let error = attribute(missing).unwrap_err();
            assert!(error.to_string().contains("does not have"), "{error}");
        }
        let unknown = attribute(json!({"kind":"guess","id":"draw","field":"/text","offset":0})).unwrap_err();
        assert!(unknown.to_string().contains("unknown kind"), "{unknown}");
    }

    #[test]
    fn a_delta_tiles_its_prefix_each_message_it_adds_and_its_suffix() {
        let first = json!({"role":"user","content":"a"}).to_string();
        let second = json!({"role":"user","content":"Fix it."}).to_string();
        let (prefix, suffix) = ("{\"messages\":[", "],\"stream\":true}");
        let json = format!("{prefix}{first},{second}{suffix}");
        let document = Document::decode(json.as_bytes(), LIMITS).unwrap();
        let task = second.find("Fix").unwrap();
        let check_delta = |added: serde_json::Value| {
            let record = json!({"kind":"delta","prefix":prefix,"suffix":suffix,"added_messages":[second],
                "authors":{"prefix":[harness(prefix.len())],"added_messages":added,"suffix":[harness(suffix.len())]}})
            .to_string();
            let record = Document::decode(record.as_bytes(), LIMITS).unwrap();
            let generations = BTreeMap::new();
            let draws = BTreeMap::new();
            check(
                record.root(),
                &json,
                &document,
                &[first.len(), second.len()],
                &ModelOutputs { generations: &generations, draws: &draws },
                &OperatorTask::new(b"Fix it."),
                1,
            )
        };
        check_delta(json!([[harness(task), {"author":"operator","bytes":7,"offset":0}, harness(second.len() - task - 7)]])).unwrap();
        // The operator run held to the bytes where the added message really is.
        let error = check_delta(json!([[harness(task - 1), {"author":"operator","bytes":7,"offset":0}, harness(second.len() - task - 6)]])).unwrap_err();
        assert!(error.to_string().contains("outside a string value"), "{error}");
        let error = check_delta(json!([])).unwrap_err();
        assert!(error.to_string().contains("one table to every message"), "{error}");
    }
}
