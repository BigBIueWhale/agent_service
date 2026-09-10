//! Standard JSON Schema assertions compiled from the owned definition at build
//! time. There is no runtime schema resolution or alternative validator here.
use std::collections::HashSet;
use std::hash::{BuildHasherDefault, DefaultHasher};

use crate::{
    json::{Node, Value},
    number::JsonNumber,
};

pub(crate) struct SchemaNode {
    pub path: &'static str,
    pub rules: &'static [Rule],
}

pub(crate) enum Rule {
    False,
    Ref(usize),
    Type(u8),
    Const(Literal),
    Enum(&'static [Literal]),
    Minimum(&'static str),
    Maximum(&'static str),
    MinLength(usize),
    MaxLength(usize),
    Pattern(Pattern),
    Object {
        properties: &'static [(&'static str, usize)],
        required: &'static [&'static str],
        additional: Additional,
    },
    Array {
        items: Option<usize>,
        min: Option<usize>,
        max: Option<usize>,
        unique: bool,
    },
    AllOf(&'static [usize]),
    AnyOf(&'static [usize]),
    OneOf(&'static [usize]),
    IfThen {
        condition: usize,
        consequence: usize,
    },
}

pub(crate) enum Literal {
    Null,
    Bool(bool),
    Number(&'static str),
    String(&'static str),
}
pub(crate) enum Additional {
    Allow,
    Schema(usize),
}
pub(crate) enum Pattern {
    LowerHex64,
    ContainsNonblankScalar,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct Mismatch {
    pub instance_path: String,
    pub schema_path: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ValidationError {
    Rejected {
        mismatch: Mismatch,
    },
    ResourceLimit {
        resource: &'static str,
        limit: usize,
    },
    InvalidDefinition {
        cause: String,
    },
}

/// Maximum interpreter operations. Exhaustion is validation-unavailable, not a
/// nonmatching branch that could enable anyOf or skip a conditional assertion.
#[derive(Clone, Copy, Debug)]
pub struct ValidationLimits {
    pub operations: usize,
}

struct Failure {
    path: usize,
    schema: &'static str,
    keyword: &'static str,
}
enum Combination {
    All,
    Any,
    One,
}
enum Task<'a> {
    Node(usize, Value<'a>, usize),
    Rule(&'static Rule, Value<'a>, usize, &'static str),
    Collect(Combination, usize, Failure),
    Conditional(usize, Value<'a>, usize),
    Failed(Failure),
}

pub(crate) fn validate(
    nodes: &'static [SchemaNode],
    entry: usize,
    value: Value<'_>,
    limits: ValidationLimits,
) -> Result<(), ValidationError> {
    let mut tasks = vec![Task::Node(entry, value, 0)];
    let mut results: Vec<Result<(), Failure>> = Vec::new();
    // Paths are an arena too: deeply nested diagnostics cannot recurse on drop.
    let mut paths: Vec<(usize, String)> = vec![(0, String::new())];
    let mut used = 0usize;
    while let Some(task) = tasks.pop() {
        if used == limits.operations {
            return Err(ValidationError::ResourceLimit {
                resource: "schema_operations",
                limit: limits.operations,
            });
        }
        used += 1;
        match task {
            Task::Node(id, value, path) => {
                let node = nodes
                    .get(id)
                    .ok_or_else(|| ValidationError::InvalidDefinition {
                        cause: format!("schema node {id} is unavailable"),
                    })?;
                tasks.push(Task::Collect(
                    Combination::All,
                    node.rules.len(),
                    Failure {
                        path,
                        schema: node.path,
                        keyword: "",
                    },
                ));
                tasks.extend(
                    node.rules
                        .iter()
                        .rev()
                        .map(|rule| Task::Rule(rule, value, path, node.path)),
                );
            }
            Task::Failed(failure) => results.push(Err(failure)),
            Task::Conditional(consequence, value, path) => {
                if results.pop().expect("condition result").is_ok() {
                    tasks.push(Task::Node(consequence, value, path));
                } else {
                    results.push(Ok(()));
                }
            }
            Task::Collect(combination, count, failure) => {
                let start = results
                    .len()
                    .checked_sub(count)
                    .expect("complete schema branches");
                let matched = results[start..]
                    .iter()
                    .filter(|result| result.is_ok())
                    .count();
                let accepted = match combination {
                    Combination::All => matched == count,
                    Combination::Any => matched > 0,
                    Combination::One => matched == 1,
                };
                let first_failure = results.drain(start..).find_map(Result::err);
                results.push(if accepted {
                    Ok(())
                } else {
                    Err(match combination {
                        Combination::All => first_failure.unwrap_or(failure),
                        _ => failure,
                    })
                });
            }
            Task::Rule(rule, value, path, schema) => {
                let fail = |keyword| Failure {
                    path,
                    schema,
                    keyword,
                };
                let simple: Result<bool, ValidationError> = match rule {
                    Rule::False => Ok(false),
                    Rule::Ref(id) => {
                        tasks.push(Task::Node(*id, value, path));
                        continue;
                    }
                    Rule::Type(mask) => {
                        let kind = match value.node() {
                            Node::Null => 1,
                            Node::Bool(_) => 2,
                            Node::Number(number) => {
                                if number.is_integer() {
                                    4 | 8
                                } else {
                                    4
                                }
                            }
                            Node::String(_) => 16,
                            Node::Array(_) => 32,
                            Node::Object(_) => 64,
                        };
                        Ok(kind & mask != 0)
                    }
                    Rule::Const(literal) => literal.matches(value),
                    Rule::Enum(literals) => {
                        let mut matched = false;
                        for literal in *literals {
                            matched |= literal.matches(value)?;
                        }
                        Ok(matched)
                    }
                    Rule::Minimum(token) => match value.node() {
                        Node::Number(number) => Ok(number >= &definition_number(token)?),
                        _ => Ok(true),
                    },
                    Rule::Maximum(token) => match value.node() {
                        Node::Number(number) => Ok(number <= &definition_number(token)?),
                        _ => Ok(true),
                    },
                    Rule::MinLength(bound) => Ok(value
                        .as_str()
                        .is_none_or(|text| text.chars().count() >= *bound)),
                    Rule::MaxLength(bound) => Ok(value
                        .as_str()
                        .is_none_or(|text| text.chars().count() <= *bound)),
                    Rule::Pattern(pattern) => {
                        Ok(value.as_str().is_none_or(|text| pattern.matches(text)))
                    }
                    Rule::AllOf(children) | Rule::AnyOf(children) | Rule::OneOf(children) => {
                        let combination = match rule {
                            Rule::AllOf(_) => Combination::All,
                            Rule::AnyOf(_) => Combination::Any,
                            _ => Combination::One,
                        };
                        tasks.push(Task::Collect(
                            combination,
                            children.len(),
                            fail(rule.keyword()),
                        ));
                        tasks.extend(children.iter().rev().map(|&id| Task::Node(id, value, path)));
                        continue;
                    }
                    Rule::IfThen {
                        condition,
                        consequence,
                    } => {
                        tasks.push(Task::Conditional(*consequence, value, path));
                        tasks.push(Task::Node(*condition, value, path));
                        continue;
                    }
                    Rule::Object {
                        properties,
                        required,
                        additional,
                    } => {
                        if let Node::Object(members) = value.node() {
                            let mut checks = Vec::new();
                            for name in *required {
                                if !members.contains_key(*name) {
                                    checks.push(Task::Failed(fail("required")));
                                    break;
                                }
                            }
                            for (key, &id) in members {
                                let selected = properties
                                    .iter()
                                    .find(|(name, _)| name == key)
                                    .map(|(_, id)| *id)
                                    .or(match additional {
                                        Additional::Allow => None,
                                        Additional::Schema(id) => Some(*id),
                                    });
                                if let Some(selected) = selected {
                                    let child_path = paths.len();
                                    paths.push((path, escape_pointer(key)));
                                    checks.push(Task::Node(selected, value.child(id), child_path));
                                }
                            }
                            tasks.push(Task::Collect(Combination::All, checks.len(), fail("")));
                            tasks.extend(checks.into_iter().rev());
                            continue;
                        }
                        Ok(true)
                    }
                    Rule::Array {
                        items,
                        min,
                        max,
                        unique,
                    } => {
                        if let Node::Array(elements) = value.node() {
                            if min.is_some_and(|min| elements.len() < min) {
                                results.push(Err(fail("minItems")));
                                continue;
                            }
                            if max.is_some_and(|max| elements.len() > max) {
                                results.push(Err(fail("maxItems")));
                                continue;
                            }
                            if *unique {
                                // A deterministic hasher needs no ambient random source;
                                // exact Value equality resolves every hash collision.
                                let mut seen: HashSet<
                                    Value<'_>,
                                    BuildHasherDefault<DefaultHasher>,
                                > = HashSet::default();
                                if elements.iter().any(|&id| !seen.insert(value.child(id))) {
                                    results.push(Err(fail("uniqueItems")));
                                    continue;
                                }
                            }
                            if let Some(selected) = items {
                                tasks.push(Task::Collect(
                                    Combination::All,
                                    elements.len(),
                                    fail("items"),
                                ));
                                for (index, &id) in elements.iter().enumerate().rev() {
                                    let child_path = paths.len();
                                    paths.push((path, index.to_string()));
                                    tasks.push(Task::Node(*selected, value.child(id), child_path));
                                }
                                continue;
                            }
                        }
                        Ok(true)
                    }
                };
                results.push(if simple? {
                    Ok(())
                } else {
                    Err(fail(rule.keyword()))
                });
            }
        }
    }
    match results.pop().expect("one complete schema result") {
        Ok(()) => Ok(()),
        Err(failure) => {
            let mut at = failure.path;
            let mut segments = Vec::new();
            while at != 0 {
                segments.push(paths[at].1.as_str());
                at = paths[at].0;
            }
            let instance_path = segments
                .iter()
                .rev()
                .map(|segment| format!("/{segment}"))
                .collect::<String>();
            let schema_path = if failure.keyword.is_empty() {
                failure.schema.into()
            } else {
                format!("{}/{}", failure.schema, failure.keyword)
            };
            Err(ValidationError::Rejected {
                mismatch: Mismatch {
                    instance_path,
                    schema_path,
                },
            })
        }
    }
}

fn definition_number(token: &str) -> Result<JsonNumber, ValidationError> {
    JsonNumber::parse(token).map_err(|error| ValidationError::InvalidDefinition {
        cause: format!("invalid generated numeric literal at {}", error.byte),
    })
}

impl Literal {
    fn matches(&self, value: Value<'_>) -> Result<bool, ValidationError> {
        Ok(match (self, value.node()) {
            (Self::Null, Node::Null) => true,
            (Self::Bool(a), Node::Bool(b)) => a == b,
            (Self::String(a), Node::String(b)) => a == b,
            (Self::Number(a), Node::Number(b)) => &definition_number(a)? == b,
            _ => false,
        })
    }
}

impl Pattern {
    fn matches(&self, text: &str) -> bool {
        match self {
            Self::LowerHex64 => text.len() == 64 && text.bytes().all(|byte| matches!(byte, b'a'..=b'f' | b'0'..=b'9')),
            Self::ContainsNonblankScalar => text.chars().any(|scalar| !matches!(scalar, '\u{9}'..='\u{d}' | '\u{20}' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')),
        }
    }
}

impl Rule {
    fn keyword(&self) -> &'static str {
        match self {
            Self::False | Self::Object { .. } => "",
            Self::Ref(_) => "$ref",
            Self::Type(_) => "type",
            Self::Const(_) => "const",
            Self::Enum(_) => "enum",
            Self::Minimum(_) => "minimum",
            Self::Maximum(_) => "maximum",
            Self::MinLength(_) => "minLength",
            Self::MaxLength(_) => "maxLength",
            Self::Pattern(_) => "pattern",
            Self::Array { .. } => "items",
            Self::AllOf(_) => "allOf",
            Self::AnyOf(_) => "anyOf",
            Self::OneOf(_) => "oneOf",
            Self::IfThen { .. } => "if",
        }
    }
}

pub(crate) fn escape_pointer(text: &str) -> String {
    text.replace('~', "~0").replace('/', "~1")
}

include!(concat!(env!("OUT_DIR"), "/compiled_schema.rs"));

pub fn validate_owned(
    entry: SchemaEntry,
    value: Value<'_>,
    limits: ValidationLimits,
) -> Result<(), ValidationError> {
    validate(NODES, entry.node(), value, limits)
}

pub const COMPILATION_INVENTORY: &str =
    include_str!(concat!(env!("OUT_DIR"), "/schema_inventory.json"));

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::{Document, Limits};

    fn document(text: &str) -> Document {
        Document::decode(
            text.as_bytes(),
            Limits {
                bytes: 1_000_000,
                nodes: 100_000,
                depth: 20_000,
            },
        )
        .unwrap()
    }
    const LIMITS: ValidationLimits = ValidationLimits {
        operations: 1_000_000,
    };

    #[test]
    fn terminal_definition_checks_vocabulary_flags_and_required_error_evidence() {
        let accepts = |value: &serde_json::Value| {
            let document = document(&serde_json::to_string(value).unwrap());
            validate_owned(SchemaEntry::Record, document.root(), LIMITS).is_ok()
        };
        // Nullable duration/usage belongs to an observed child terminal; root
        // completeness is enforced separately by the captured-stream owner.
        let mut record = serde_json::json!({
            "type": "result", "uuid": "terminal", "session_id": "session",
            "parent_tool_use_id": "child", "subtype": crate::SUCCESS_SUBTYPE,
            "is_error": false, "duration_ms": null, "duration_api_ms": null,
            "num_turns": 0, "usage": null, "permission_denials": [], "result": "ok"
        });
        assert!(accepts(&record));
        record["is_error"] = serde_json::json!(true);
        assert!(!accepts(&record));
        for subtype in crate::ERROR_SUBTYPES {
            record["subtype"] = serde_json::json!(subtype);
            record["error"] = serde_json::json!({"message": "original failure"});
            assert!(accepts(&record), "{subtype}");
            record["is_error"] = serde_json::json!(false);
            assert!(!accepts(&record), "{subtype} cannot be successful");
            record["is_error"] = serde_json::json!(true);
            for error in [
                serde_json::json!(null),
                serde_json::json!({}),
                serde_json::json!({"message": ""}),
            ] {
                record["error"] = error;
                assert!(!accepts(&record));
            }
            record.as_object_mut().unwrap().remove("error");
            assert!(!accepts(&record));
        }
        record["error"] = serde_json::json!({"message": "original failure"});
        record["subtype"] = serde_json::json!("unknown_terminal");
        assert!(!accepts(&record));
    }

    #[test]
    fn every_compiled_schema_node_agrees_with_the_pinned_oracle_on_representable_values() {
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../../stream-contract-v1.json")).unwrap();
        let mut corpus = vec![
            "null".to_string(),
            "true".into(),
            "false".into(),
            "0".into(),
            "1".into(),
            "1.0".into(),
            "1e0".into(),
            "-1".into(),
            "0.5".into(),
            "9007199254740991".into(),
            "9007199254740992".into(),
            "\"\"".into(),
            "\" \"".into(),
            "\"x\"".into(),
            "[]".into(),
            "[0]".into(),
            "[0,0.0]".into(),
            "[\"x\",\"x\"]".into(),
            "{}".into(),
            "{\"type\":\"text\",\"text\":\"x\"}".into(),
            "{\"jsonrpc\":\"2.0\",\"id\":-1,\"method\":\"x\",\"params\":{\"extra\":true}}".into(),
            "{\"jsonrpc\":\"2.0\",\"method\":\"x\",\"id\":1}".into(),
            "{\"v\":2,\"goal\":null,\"activity\":\"idle\"}".into(),
        ];
        let vectors: serde_json::Value =
            serde_json::from_str(include_str!("../../test-vectors/goal-state-v1.json")).unwrap();
        for case in vectors["cases"].as_array().unwrap() {
            corpus.push(case["record"].to_string());
        }
        for scalar in [
            '\n', '\r', '\u{85}', '\u{a0}', '\u{180e}', '\u{200b}', '\u{2028}', '\u{2029}',
            '\u{feff}', '😀',
        ] {
            corpus.push(serde_json::to_string(&scalar.to_string()).unwrap());
        }
        let documents: Vec<_> = corpus.iter().map(|text| document(text)).collect();
        let ordinary: Vec<serde_json::Value> = corpus
            .iter()
            .map(|text| serde_json::from_str(text).unwrap())
            .collect();
        for (id, node) in NODES.iter().enumerate() {
            let mut selected = schema.clone();
            if !node.path.is_empty() {
                selected["$ref"] = serde_json::json!(format!("#{}", node.path));
            }
            let oracle = jsonschema::validator_for(&selected).unwrap();
            for ((document, ordinary), source) in documents.iter().zip(&ordinary).zip(&corpus) {
                let observed = validate(NODES, id, document.root(), LIMITS);
                if let Err(error) = &observed {
                    assert!(
                        matches!(error, ValidationError::Rejected { .. }),
                        "engine failure at {}: {error:?}",
                        node.path
                    );
                }
                assert_eq!(
                    observed.is_ok(),
                    oracle.is_valid(ordinary),
                    "schema {} with {source}: {observed:?}",
                    node.path
                );
            }
        }
    }

    #[test]
    fn exact_numeric_constraints_and_recursion_do_not_use_binary64() {
        for (text, accepted) in [
            ("1.0", true),
            ("1e0", true),
            ("1e-1000001", false),
            ("1e1000001", false),
            ("16000", true),
            ("16000.00000000000000001", false),
        ] {
            let value = document(text);
            assert_eq!(
                validate_owned(
                    SchemaEntry::GoalCheckpointEvidenceBytes,
                    value.root(),
                    LIMITS
                )
                .is_ok(),
                accepted,
                "{text}"
            );
        }
        let input = "9007199254740993";
        let text = format!(
            r#"{{"type":"tool_result","tool_use_id":"outer","content":[{{"type":"tool_result","tool_use_id":"inner","content":[{{"type":"tool_use","id":"call","name":"echo","input":{input}}}]}}]}}"#
        );
        let value = document(&text);
        validate_owned(SchemaEntry::ContentBlock, value.root(), LIMITS).unwrap();
        let huge = document(r#"{"jsonrpc":"2.0","id":9007199254740993,"result":{}}"#);
        assert!(matches!(
            validate_owned(SchemaEntry::McpMessage, huge.root(), LIMITS),
            Err(ValidationError::Rejected { .. })
        ));
        let arbitrary = document(
            r#"{"jsonrpc":"2.0","id":1,"result":{"value":1e999999999999999999999999999,"$serde_json::private::Number":"ordinary"}}"#,
        );
        validate_owned(SchemaEntry::McpMessage, arbitrary.root(), LIMITS).unwrap();
    }

    #[test]
    fn combinators_never_treat_exhaustion_as_a_nonmatching_branch() {
        let value = document(r#"{"v":2,"goal":null,"activity":"idle"}"#);
        for operations in 0..8 {
            assert!(matches!(
                validate_owned(
                    SchemaEntry::GoalSnapshot,
                    value.root(),
                    ValidationLimits { operations }
                ),
                Err(ValidationError::ResourceLimit { .. })
            ));
        }
        validate_owned(SchemaEntry::GoalSnapshot, value.root(), LIMITS).unwrap();
    }
}
