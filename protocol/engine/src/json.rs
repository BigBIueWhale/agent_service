//! Exact JSON at the byte boundary. Containers use an arena so parsing,
//! equality, hashing and destruction do not recurse on attacker-owned nesting.
use std::{
    collections::BTreeMap,
    hash::{Hash, Hasher},
    ops::Range,
};

use crate::number::JsonNumber;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    pub bytes: usize,
    pub nodes: usize,
    pub depth: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DecodeError {
    InvalidUtf8 {
        byte: usize,
    },
    Syntax {
        byte: usize,
        expected: &'static str,
    },
    DuplicateKey {
        byte: usize,
        key: String,
    },
    ResourceLimit {
        resource: &'static str,
        limit: usize,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct NodeId(usize);

#[derive(Clone, Debug)]
pub(crate) enum Node {
    Null,
    Bool(bool),
    Number(JsonNumber),
    String(String),
    Array(Vec<NodeId>),
    Object(BTreeMap<String, NodeId>),
}

#[derive(Clone, Debug)]
struct Entry {
    node: Node,
    span: Range<usize>,
}

#[derive(Clone, Debug)]
pub struct Document {
    source: Box<str>,
    entries: Vec<Entry>,
    root: NodeId,
}

impl Document {
    pub fn decode(bytes: &[u8], limits: Limits) -> Result<Self, DecodeError> {
        if bytes.len() > limits.bytes {
            return Err(DecodeError::ResourceLimit {
                resource: "json_bytes",
                limit: limits.bytes,
            });
        }
        let source = std::str::from_utf8(bytes).map_err(|error| DecodeError::InvalidUtf8 {
            byte: error.valid_up_to(),
        })?;
        Parser {
            source,
            at: 0,
            entries: Vec::new(),
            frames: Vec::new(),
            root: None,
            limits,
        }
        .parse()
    }

    pub fn root(&self) -> Value<'_> {
        self.value(self.root)
    }
    fn value(&self, id: NodeId) -> Value<'_> {
        Value { document: self, id }
    }
    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn node_count(&self) -> usize {
        self.entries.len()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Value<'a> {
    document: &'a Document,
    id: NodeId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Null,
    Boolean,
    Number,
    String,
    Array,
    Object,
}

impl<'a> Value<'a> {
    pub(crate) fn node(self) -> &'a Node {
        &self.document.entries[self.id.0].node
    }
    pub fn raw(self) -> &'a str {
        &self.document.source[self.document.entries[self.id.0].span.clone()]
    }
    pub(crate) fn child(self, id: NodeId) -> Self {
        self.document.value(id)
    }
    pub fn kind(self) -> Kind {
        match self.node() {
            Node::Null => Kind::Null,
            Node::Bool(_) => Kind::Boolean,
            Node::Number(_) => Kind::Number,
            Node::String(_) => Kind::String,
            Node::Array(_) => Kind::Array,
            Node::Object(_) => Kind::Object,
        }
    }
    pub fn as_object(self) -> Option<Self> {
        (self.kind() == Kind::Object).then_some(self)
    }
    pub fn is_null(self) -> bool {
        self.kind() == Kind::Null
    }
    pub fn is_array(self) -> bool {
        self.kind() == Kind::Array
    }
    pub fn is_string(self) -> bool {
        self.kind() == Kind::String
    }
    pub fn is_boolean(self) -> bool {
        self.kind() == Kind::Boolean
    }
    pub fn as_bool(self) -> Option<bool> {
        match self.node() {
            Node::Bool(value) => Some(*value),
            _ => None,
        }
    }
    pub fn elements(self) -> Option<impl ExactSizeIterator<Item = Self> + 'a> {
        match self.node() {
            Node::Array(elements) => Some(elements.iter().map(move |&id| self.child(id))),
            _ => None,
        }
    }
    pub fn members(self) -> Option<impl ExactSizeIterator<Item = (&'a str, Self)> + 'a> {
        match self.node() {
            Node::Object(members) => Some(
                members
                    .iter()
                    .map(move |(key, &id)| (key.as_str(), self.child(id))),
            ),
            _ => None,
        }
    }
    pub fn get(self, key: &str) -> Option<Self> {
        match self.node() {
            Node::Object(members) => members.get(key).map(|&id| self.child(id)),
            _ => None,
        }
    }
    pub fn as_str(self) -> Option<&'a str> {
        match self.node() {
            Node::String(text) => Some(text),
            _ => None,
        }
    }
    pub fn as_number(self) -> Option<&'a JsonNumber> {
        match self.node() {
            Node::Number(number) => Some(number),
            _ => None,
        }
    }
}

impl PartialEq for Value<'_> {
    fn eq(&self, other: &Self) -> bool {
        let mut pending = vec![(*self, *other)];
        while let Some((left, right)) = pending.pop() {
            match (left.node(), right.node()) {
                (Node::Null, Node::Null) => {}
                (Node::Bool(a), Node::Bool(b)) if a == b => {}
                (Node::Number(a), Node::Number(b)) if a == b => {}
                (Node::String(a), Node::String(b)) if a == b => {}
                (Node::Array(a), Node::Array(b)) if a.len() == b.len() => {
                    pending.extend(
                        a.iter()
                            .zip(b)
                            .map(|(&a, &b)| (left.child(a), right.child(b))),
                    );
                }
                (Node::Object(a), Node::Object(b)) if a.len() == b.len() => {
                    for ((a_key, &a_id), (b_key, &b_id)) in a.iter().zip(b) {
                        if a_key != b_key {
                            return false;
                        }
                        pending.push((left.child(a_id), right.child(b_id)));
                    }
                }
                _ => return false,
            }
        }
        true
    }
}
impl Eq for Value<'_> {}

impl Hash for Value<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        enum Part<'a> {
            Value(Value<'a>),
            Key(&'a str),
        }
        let mut pending = vec![Part::Value(*self)];
        while let Some(part) = pending.pop() {
            let Part::Value(value) = part else {
                let Part::Key(key) = part else { unreachable!() };
                key.hash(state);
                continue;
            };
            match value.node() {
                Node::Null => 0u8.hash(state),
                Node::Bool(boolean) => {
                    1u8.hash(state);
                    boolean.hash(state);
                }
                Node::Number(number) => {
                    2u8.hash(state);
                    number.hash(state);
                }
                Node::String(text) => {
                    3u8.hash(state);
                    text.hash(state);
                }
                Node::Array(elements) => {
                    4u8.hash(state);
                    elements.len().hash(state);
                    pending.extend(
                        elements
                            .iter()
                            .rev()
                            .map(|&id| Part::Value(value.child(id))),
                    );
                }
                Node::Object(members) => {
                    5u8.hash(state);
                    members.len().hash(state);
                    for (key, &id) in members.iter().rev() {
                        pending.push(Part::Value(value.child(id)));
                        pending.push(Part::Key(key));
                    }
                }
            }
        }
    }
}

enum Position {
    First,
    Next,
    After,
}
enum Frame {
    Array {
        start: usize,
        elements: Vec<NodeId>,
        position: Position,
    },
    Object {
        start: usize,
        members: BTreeMap<String, NodeId>,
        position: Position,
        key: Option<String>,
    },
}

struct Parser<'a> {
    source: &'a str,
    at: usize,
    entries: Vec<Entry>,
    frames: Vec<Frame>,
    root: Option<NodeId>,
    limits: Limits,
}

impl Parser<'_> {
    fn fail(&self, expected: &'static str) -> DecodeError {
        DecodeError::Syntax {
            byte: self.at,
            expected,
        }
    }

    fn whitespace(&mut self) {
        while matches!(
            self.source.as_bytes().get(self.at),
            Some(b' ' | b'\t' | b'\r' | b'\n')
        ) {
            self.at += 1;
        }
    }

    fn accept(&mut self, node: Node, start: usize) -> Result<(), DecodeError> {
        if self.entries.len() == self.limits.nodes {
            return Err(DecodeError::ResourceLimit {
                resource: "json_nodes",
                limit: self.limits.nodes,
            });
        }
        let id = NodeId(self.entries.len());
        self.entries.push(Entry {
            node,
            span: start..self.at,
        });
        match self.frames.last_mut() {
            Some(Frame::Array {
                elements, position, ..
            }) => {
                elements.push(id);
                *position = Position::After;
            }
            Some(Frame::Object {
                members,
                position,
                key,
                ..
            }) => {
                members.insert(key.take().expect("object value follows its key"), id);
                *position = Position::After;
            }
            None => {
                assert!(self.root.is_none());
                self.root = Some(id);
            }
        }
        Ok(())
    }

    fn close(&mut self) -> Result<(), DecodeError> {
        self.at += 1;
        let (start, node) = match self.frames.pop().expect("closing container") {
            Frame::Array {
                start, elements, ..
            } => (start, Node::Array(elements)),
            Frame::Object { start, members, .. } => (start, Node::Object(members)),
        };
        self.accept(node, start)
    }

    fn parse(mut self) -> Result<Document, DecodeError> {
        loop {
            self.whitespace();
            let next = self.source.as_bytes().get(self.at).copied();
            if self.frames.is_empty() && self.root.is_some() {
                if next.is_some() {
                    return Err(self.fail("end of JSON input"));
                }
                return Ok(Document {
                    source: self.source.into(),
                    entries: self.entries,
                    root: self.root.expect("complete root"),
                });
            }
            match self.frames.last() {
                Some(Frame::Array {
                    position: Position::First,
                    ..
                }) if next == Some(b']') => {
                    self.close()?;
                    continue;
                }
                Some(Frame::Array {
                    position: Position::After,
                    ..
                }) => match next {
                    Some(b']') => {
                        self.close()?;
                        continue;
                    }
                    Some(b',') => {
                        let Some(Frame::Array { position, .. }) = self.frames.last_mut() else {
                            unreachable!()
                        };
                        *position = Position::Next;
                        self.at += 1;
                        continue;
                    }
                    _ => return Err(self.fail("comma or closing array bracket")),
                },
                Some(Frame::Object {
                    position: Position::First,
                    key: None,
                    ..
                }) if next == Some(b'}') => {
                    self.close()?;
                    continue;
                }
                Some(Frame::Object {
                    position: Position::After,
                    ..
                }) => match next {
                    Some(b'}') => {
                        self.close()?;
                        continue;
                    }
                    Some(b',') => {
                        let Some(Frame::Object { position, .. }) = self.frames.last_mut() else {
                            unreachable!()
                        };
                        *position = Position::Next;
                        self.at += 1;
                        continue;
                    }
                    _ => return Err(self.fail("comma or closing object brace")),
                },
                Some(Frame::Object { key: None, .. }) => {
                    if next != Some(b'"') {
                        return Err(self.fail("object member string"));
                    }
                    let byte = self.at;
                    let decoded = self.string()?;
                    let Some(Frame::Object { members, key, .. }) = self.frames.last_mut() else {
                        unreachable!()
                    };
                    if members.contains_key(&decoded) {
                        return Err(DecodeError::DuplicateKey { byte, key: decoded });
                    }
                    *key = Some(decoded);
                    self.whitespace();
                    if self.source.as_bytes().get(self.at) != Some(&b':') {
                        return Err(self.fail("colon after object member"));
                    }
                    self.at += 1;
                    continue;
                }
                _ => {}
            }
            let start = self.at;
            let node = match next {
                Some(b'[' | b'{') => {
                    if self.frames.len() == self.limits.depth {
                        return Err(DecodeError::ResourceLimit {
                            resource: "json_depth",
                            limit: self.limits.depth,
                        });
                    }
                    self.frames.push(if next == Some(b'[') {
                        Frame::Array {
                            start,
                            elements: Vec::new(),
                            position: Position::First,
                        }
                    } else {
                        Frame::Object {
                            start,
                            members: BTreeMap::new(),
                            position: Position::First,
                            key: None,
                        }
                    });
                    self.at += 1;
                    continue;
                }
                Some(b'"') => Node::String(self.string()?),
                Some(b'n' | b't' | b'f') => {
                    let (literal, node) = match next {
                        Some(b'n') => ("null", Node::Null),
                        Some(b't') => ("true", Node::Bool(true)),
                        _ => ("false", Node::Bool(false)),
                    };
                    if !self.source[self.at..].starts_with(literal) {
                        return Err(self.fail("JSON literal"));
                    }
                    self.at += literal.len();
                    node
                }
                Some(b'-' | b'0'..=b'9') => {
                    while matches!(
                        self.source.as_bytes().get(self.at),
                        Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
                    ) {
                        self.at += 1;
                    }
                    Node::Number(JsonNumber::parse(&self.source[start..self.at]).map_err(
                        |error| DecodeError::Syntax {
                            byte: start + error.byte,
                            expected: "JSON number",
                        },
                    )?)
                }
                _ => return Err(self.fail("JSON value")),
            };
            self.accept(node, start)?;
        }
    }

    fn hex_unit(&mut self) -> Result<u16, DecodeError> {
        let mut value = 0u16;
        for _ in 0..4 {
            let digit = match self.source.as_bytes().get(self.at) {
                Some(byte @ b'0'..=b'9') => byte - b'0',
                Some(byte @ b'a'..=b'f') => byte - b'a' + 10,
                Some(byte @ b'A'..=b'F') => byte - b'A' + 10,
                _ => return Err(self.fail("four hexadecimal escape digits")),
            };
            value = value * 16 + u16::from(digit);
            self.at += 1;
        }
        Ok(value)
    }

    fn string(&mut self) -> Result<String, DecodeError> {
        self.at += 1; // opening quote
        let mut decoded = String::new();
        loop {
            match self.source.as_bytes().get(self.at) {
                Some(b'"') => {
                    self.at += 1;
                    return Ok(decoded);
                }
                Some(b'\\') => {
                    self.at += 1;
                    let scalar = match self.source.as_bytes().get(self.at) {
                        Some(b'"') => '"',
                        Some(b'\\') => '\\',
                        Some(b'/') => '/',
                        Some(b'b') => '\u{8}',
                        Some(b'f') => '\u{c}',
                        Some(b'n') => '\n',
                        Some(b'r') => '\r',
                        Some(b't') => '\t',
                        Some(b'u') => {
                            self.at += 1;
                            let first = self.hex_unit()?;
                            let scalar = match first {
                                0xd800..=0xdbff => {
                                    if !self.source[self.at..].starts_with("\\u") {
                                        return Err(
                                            self.fail("low-surrogate escape after high surrogate")
                                        );
                                    }
                                    self.at += 2;
                                    let second = self.hex_unit()?;
                                    if !(0xdc00..=0xdfff).contains(&second) {
                                        return Err(
                                            self.fail("low-surrogate escape after high surrogate")
                                        );
                                    }
                                    0x10000
                                        + ((u32::from(first) - 0xd800) << 10)
                                        + u32::from(second)
                                        - 0xdc00
                                }
                                0xdc00..=0xdfff => {
                                    return Err(self.fail("paired Unicode surrogate"))
                                }
                                _ => u32::from(first),
                            };
                            decoded.push(char::from_u32(scalar).expect("validated escaped scalar"));
                            continue;
                        }
                        _ => return Err(self.fail("JSON string escape")),
                    };
                    decoded.push(scalar);
                    self.at += 1;
                }
                Some(0..=31) => return Err(self.fail("escaped string control character")),
                Some(_) => {
                    let scalar = self.source[self.at..]
                        .chars()
                        .next()
                        .expect("validated UTF-8 scalar");
                    decoded.push(scalar);
                    self.at += scalar.len_utf8();
                }
                None => return Err(self.fail("closing string quote")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const LIMITS: Limits = Limits {
        bytes: 1_000_000,
        nodes: 100_000,
        depth: 20_000,
    };
    fn parse(text: &str) -> Document {
        Document::decode(text.as_bytes(), LIMITS).unwrap()
    }

    #[test]
    fn original_tokens_and_ordinary_reserved_keys_survive() {
        let source = r#" {"$serde_json::private::Number":"7","values":[-0,1E0,1e+0,9007199254740993,1e1000001],"encoding":"json","text":"{}"} "#;
        let document = parse(source);
        assert_eq!(document.source(), source);
        assert_eq!(
            document
                .root()
                .get("$serde_json::private::Number")
                .unwrap()
                .as_str(),
            Some("7")
        );
        assert_eq!(
            document.root().get("values").unwrap().raw(),
            "[-0,1E0,1e+0,9007199254740993,1e1000001]"
        );
        assert!(matches!(document.root().node(), Node::Object(_)));
    }

    #[test]
    fn strict_grammar_rejects_incomplete_or_ambiguous_values() {
        for text in [
            "",
            " ",
            "[",
            "{",
            "[1,]",
            "{\"x\":1,}",
            "{\"x\" 1}",
            "{\"x\":}",
            "[1 2]",
            "null true",
            "01",
            "NaN",
            "1.",
            "\u{feff}null",
            "\"\n\"",
            r#""\q""#,
            r#""\uD800""#,
            r#""\uDC00""#,
            r#""\uD800\u0041""#,
        ] {
            assert!(
                matches!(
                    Document::decode(text.as_bytes(), LIMITS),
                    Err(DecodeError::Syntax { .. })
                ),
                "accepted {text:?}"
            );
        }
        for text in [
            r#"{"a":1,"a":2}"#,
            r#"{"a":1,"\u0061":2}"#,
            r#"{"😀":1,"\uD83D\uDE00":2}"#,
        ] {
            assert!(matches!(
                Document::decode(text.as_bytes(), LIMITS),
                Err(DecodeError::DuplicateKey { .. })
            ));
        }
        for bytes in [
            &b"\"\xc0\x80\""[..],
            &b"\"\xed\xa0\x80\""[..],
            &b"\"\xf4\x90\x80\x80\""[..],
            &b"\"\x80\""[..],
        ] {
            assert!(matches!(
                Document::decode(bytes, LIMITS),
                Err(DecodeError::InvalidUtf8 { .. })
            ));
        }
    }

    #[test]
    fn semantic_equality_and_hash_keep_number_and_container_meanings() {
        let a = parse(r#"{"b":[1,-0,1e999999999999999999999999],"a":"é"}"#);
        let b = parse(r#"{"a":"\u00e9","b":[1.0,0,10e999999999999999999999998]}"#);
        let c = parse(r#"{"b":[1,-0,1e999999999999999999999998],"a":"é"}"#);
        assert_eq!(a.root(), b.root());
        assert_ne!(a.root(), c.root());
        let hash = |value: Value<'_>| {
            let mut state = std::hash::DefaultHasher::new();
            value.hash(&mut state);
            state.finish()
        };
        assert_eq!(hash(a.root()), hash(b.root()));
        assert_ne!(parse("[1,2]").root(), parse("[2,1]").root());
        assert_ne!(parse("1").root(), parse("\"1\"").root());
    }

    #[test]
    fn iterative_depth_and_resource_failures_are_distinct() {
        let text = format!("{}0{}", "[".repeat(12_000), "]".repeat(12_000));
        let a = parse(&text);
        let b = parse(&text);
        assert_eq!(a.root(), b.root());
        assert_eq!(a.node_count(), 12_001);
        for (limits, resource) in [
            (Limits { bytes: 1, ..LIMITS }, "json_bytes"),
            (Limits { nodes: 1, ..LIMITS }, "json_nodes"),
            (Limits { depth: 1, ..LIMITS }, "json_depth"),
        ] {
            assert_eq!(
                Document::decode(b"[[0]]", limits).unwrap_err(),
                DecodeError::ResourceLimit { resource, limit: 1 }
            );
        }
        assert_eq!(
            Document::decode(
                b"[]",
                Limits {
                    bytes: 2,
                    nodes: 1,
                    depth: 1
                }
            )
            .unwrap()
            .node_count(),
            1
        );
    }

    #[test]
    fn all_unicode_scalars_round_trip_through_bytes_and_escapes() {
        for scalar in (0..=0x10ffff).filter_map(char::from_u32) {
            let text = serde_json::to_string(&scalar.to_string()).unwrap();
            let document = parse(&text);
            assert_eq!(document.root().as_str(), Some(scalar.to_string().as_str()));
            let mut buffer = [0u16; 2];
            let escaped = format!(
                "\"{}\"",
                scalar
                    .encode_utf16(&mut buffer)
                    .iter()
                    .map(|unit| format!("\\u{unit:04x}"))
                    .collect::<String>()
            );
            assert_eq!(document.root(), parse(&escaped).root());
        }
    }
}
