//! A session's declared deliverables: the files its run must leave beneath the
//! artifacts root.
//!
//! The creation body names them in its required `deliverables` field, a JSON
//! array of paths; `[]` declares none and is written explicitly, never assumed.
//! The rule a path must satisfy, and the bounds below, are declared here once:
//! the service applies them to the creation body before a single archive byte
//! is spooled, and the launcher applies them again to the control record it
//! reads before it hands the list to Qwen Code. The client, which checks each
//! declared path after the final message, applies the same rule to the
//! argument it receives.
//!
//! That check proves existence -- a non-empty regular file reached beneath
//! `/artifacts` without following a symbolic link -- and nothing more. It does
//! not judge the file's contents, and nothing checks that a declared path
//! agrees with the path the prompt names.

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// The directory every declared path is relative to: the agent container's
/// `/artifacts` mount, which starts empty and is kept at teardown.
pub const ARTIFACTS_ROOT: &str = "/artifacts";

/// Linux `NAME_MAX`: the longest single path component, in bytes, the kernel
/// accepts. A declared component longer than this names a file nobody can
/// create.
pub const NAME_MAX: usize = 255;

/// Linux `PATH_MAX`: the longest path, in bytes including its terminating NUL,
/// the kernel accepts.
pub const PATH_MAX: usize = 4096;

/// The longest declared path, in bytes: `/artifacts/` followed by the path is
/// what the client opens, and it must fit `PATH_MAX` with its NUL.
pub const MAX_DELIVERABLE_PATH_BYTES: usize = PATH_MAX - 1 - (ARTIFACTS_ROOT.len() + 1);

/// Linux `MAX_ARG_STRLEN`: the longest single argument, in bytes including its
/// NUL, that `execve` accepts -- 32 pages, and a page is 4,096 bytes on the
/// one platform (linux/x86_64) the launcher runs on.
pub const MAX_ARG_STRLEN: usize = 32 * 4096;

/// The one argument that carries the list to Qwen Code, before the list's
/// compact JSON.
pub const ARGUMENT_PREFIX: &str = "--deliverables=";

/// The bound on the whole list, as the bytes of its compact JSON
/// serialization.
///
/// It is derived, not chosen: the launcher passes the list to Qwen Code as one
/// argument, `--deliverables=<list>`, and Linux refuses any single argument
/// longer than `MAX_ARG_STRLEN` with its NUL. A list past this bound could be
/// accepted but never launched, so it is refused when it is submitted.
pub const MAX_DELIVERABLES_JSON_BYTES: usize = MAX_ARG_STRLEN - ARGUMENT_PREFIX.len() - 1;

const RECORD_PREFIX: &str = "{\"deliverables\":";
const RECORD_SUFFIX: &str = "}\n";

/// The longest control record, `{"deliverables":<list>}` and a newline, that a
/// list inside its bound produces; a larger file is malformed by construction.
pub const MAX_RECORD_BYTES: usize =
    RECORD_PREFIX.len() + MAX_DELIVERABLES_JSON_BYTES + RECORD_SUFFIX.len();

const _: () = assert!(MAX_DELIVERABLE_PATH_BYTES == 4084);
const _: () = assert!(MAX_DELIVERABLES_JSON_BYTES == 131_056);

/// A list of declared deliverables that satisfies the rule. It exists only
/// through [`Deliverables::new`], which every decoder goes through, so a value
/// of this type is a list the launcher can pass and the client can check.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<String>", into = "Vec<String>")]
pub struct Deliverables {
    paths: Vec<String>,
    /// The compact JSON serialization, which is both what the bound measures
    /// and what the control record and the argument carry.
    json: String,
}

/// Why a list, or a control record carrying one, is not a list of
/// deliverables. The message names the field, the offending entry, the rule it
/// breaks and what to do instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeliverablesError(String);

impl fmt::Display for DeliverablesError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for DeliverablesError {}

impl Deliverables {
    /// Admit a list exactly as declared, or refuse it naming the first entry
    /// that breaks the rule. Nothing is normalized: a path is either one the
    /// client will look for as written, or it is refused.
    pub fn new(paths: Vec<String>) -> Result<Self, DeliverablesError> {
        let mut first_index = HashMap::with_capacity(paths.len());
        for (index, path) in paths.iter().enumerate() {
            check_path(index, path)?;
            if let Some(earlier) = first_index.insert(path.as_str(), index) {
                return Err(entry_error(
                    index,
                    path,
                    &format!("repeats deliverables[{earlier}]; declare each file once and remove the repetition"),
                ));
            }
        }
        let json = serde_json::to_string(&paths).map_err(|error| {
            DeliverablesError(format!(
                "field `deliverables` could not be serialized as JSON: {error}"
            ))
        })?;
        if json.len() > MAX_DELIVERABLES_JSON_BYTES {
            return Err(DeliverablesError(format!(
                "field `deliverables` is {} bytes as compact JSON, past its {MAX_DELIVERABLES_JSON_BYTES}-byte bound: the launcher passes the list to Qwen Code as the one argument `{ARGUMENT_PREFIX}<list>`, and Linux refuses a single argument longer than MAX_ARG_STRLEN ({MAX_ARG_STRLEN} bytes with its NUL). Declare fewer or shorter paths.",
                json.len()
            )));
        }
        Ok(Self { paths, json })
    }

    /// The declared paths, in declared order.
    pub fn paths(&self) -> &[String] {
        &self.paths
    }

    /// The list's compact JSON serialization.
    pub fn json(&self) -> &str {
        &self.json
    }

    /// The one argument that carries the list to Qwen Code.
    pub fn argument(&self) -> String {
        format!("{ARGUMENT_PREFIX}{}", self.json)
    }

    /// The exact bytes of the session's control record,
    /// `{"deliverables":<compact list>}` and one newline.
    ///
    /// Every reader on the far side of a mount boundary compares these bytes
    /// rather than accepting whatever a JSON parser would tolerate.
    pub fn record(&self) -> String {
        format!("{RECORD_PREFIX}{}{RECORD_SUFFIX}", self.json)
    }

    /// Read a control record back, only in its one canonical spelling and only
    /// for a list the service could have accepted: it is parsed, admitted by
    /// the rule, serialized again and compared byte for byte, so whitespace,
    /// another escape of the same text, a second field or a missing newline is
    /// a refusal rather than a reinterpretation.
    pub fn from_record(bytes: &[u8]) -> Result<Self, DeliverablesError> {
        let malformed = |detail: String| {
            DeliverablesError(format!(
                "not one canonical {RECORD_PREFIX}<list>}} line of at most {MAX_RECORD_BYTES} bytes: {detail}"
            ))
        };
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(malformed(format!("observed {} bytes", bytes.len())));
        }
        let text = std::str::from_utf8(bytes).map_err(|error| malformed(error.to_string()))?;
        let json = text
            .strip_prefix(RECORD_PREFIX)
            .and_then(|rest| rest.strip_suffix(RECORD_SUFFIX))
            .ok_or_else(|| malformed(format!("observed {}", quoted(text))))?;
        let paths: Vec<String> = serde_json::from_str(json)
            .map_err(|error| malformed(format!("the list is not a JSON array of strings: {error}")))?;
        let deliverables = Self::new(paths)?;
        if deliverables.json != json {
            return Err(malformed(format!(
                "the list is not in its compact serialization {}",
                quoted(&deliverables.json)
            )));
        }
        Ok(deliverables)
    }
}

impl TryFrom<Vec<String>> for Deliverables {
    type Error = DeliverablesError;

    fn try_from(paths: Vec<String>) -> Result<Self, Self::Error> {
        Self::new(paths)
    }
}

impl From<Deliverables> for Vec<String> {
    fn from(deliverables: Deliverables) -> Self {
        deliverables.paths
    }
}

/// The rule for one declared path, checked in the order a reader would fix
/// them: present, representable, relative, then each component, then length.
fn check_path(index: usize, path: &str) -> Result<(), DeliverablesError> {
    if path.is_empty() {
        return Err(entry_error(
            index,
            path,
            "is empty; name a file by its path relative to /artifacts, such as \"report.md\", or remove the entry",
        ));
    }
    if path.contains('\0') {
        return Err(entry_error(
            index,
            path,
            "contains a NUL byte, which no Linux path can hold; remove it",
        ));
    }
    if path.starts_with('/') {
        return Err(entry_error(
            index,
            path,
            "starts with \"/\"; a declared path is relative to /artifacts, so write \"report.md\" for /artifacts/report.md",
        ));
    }
    for component in path.split('/') {
        if component.is_empty() {
            return Err(entry_error(
                index,
                path,
                "has an empty component (a doubled or trailing \"/\"); separate components with exactly one \"/\", as in \"out/report.md\"",
            ));
        }
        if component == "." || component == ".." {
            return Err(entry_error(
                index,
                path,
                &format!("has a {component:?} component; name the file by its direct path beneath /artifacts, without \".\" or \"..\""),
            ));
        }
        if component.len() > NAME_MAX {
            return Err(entry_error(
                index,
                path,
                &format!(
                    "has a component of {} bytes, past the {NAME_MAX}-byte limit Linux places on one file name (NAME_MAX); shorten that component",
                    component.len()
                ),
            ));
        }
    }
    if path.len() > MAX_DELIVERABLE_PATH_BYTES {
        return Err(entry_error(
            index,
            path,
            &format!(
                "is {} bytes, so {ARTIFACTS_ROOT}/ and the path would be {} bytes, past the {} a Linux path may hold before its NUL (PATH_MAX {PATH_MAX}); shorten it to at most {MAX_DELIVERABLE_PATH_BYTES} bytes",
                path.len(),
                ARTIFACTS_ROOT.len() + 1 + path.len(),
                PATH_MAX - 1
            ),
        ));
    }
    Ok(())
}

fn entry_error(index: usize, path: &str, rule: &str) -> DeliverablesError {
    DeliverablesError(format!(
        "field `deliverables[{index}]` ({}) {rule}",
        quoted(path)
    ))
}

/// Quote an offending value for a refusal: escaped, and cut after its first
/// 128 characters with its full length stated, so a refusal names the entry
/// without echoing an arbitrarily long value back.
fn quoted(value: &str) -> String {
    const SHOWN_CHARS: usize = 128;
    match value.char_indices().nth(SHOWN_CHARS) {
        None => format!("{value:?}"),
        Some((cut, _)) => format!("{:?}... ({} bytes)", &value[..cut], value.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn admit(paths: &[&str]) -> Result<Deliverables, DeliverablesError> {
        Deliverables::new(paths.iter().map(|path| path.to_string()).collect())
    }

    #[test]
    fn the_bounds_are_the_kernels_and_the_argument_derives_the_list_bound() {
        assert_eq!(NAME_MAX, 255);
        assert_eq!(PATH_MAX, 4096);
        assert_eq!(MAX_ARG_STRLEN, 131_072);
        assert_eq!(ARGUMENT_PREFIX, "--deliverables=");
        // The argument at the list bound, with its NUL, is exactly the kernel's
        // largest single argument.
        assert_eq!(
            ARGUMENT_PREFIX.len() + MAX_DELIVERABLES_JSON_BYTES + 1,
            MAX_ARG_STRLEN
        );
        assert_eq!(
            ARTIFACTS_ROOT.len() + 1 + MAX_DELIVERABLE_PATH_BYTES + 1,
            PATH_MAX
        );
        assert_eq!(MAX_RECORD_BYTES, MAX_DELIVERABLES_JSON_BYTES + 18);
    }

    #[test]
    fn a_list_is_admitted_exactly_as_declared_and_carried_canonically() {
        let none = admit(&[]).unwrap();
        assert!(none.paths().is_empty());
        assert_eq!(none.json(), "[]");
        assert_eq!(none.argument(), "--deliverables=[]");
        assert_eq!(none.record(), "{\"deliverables\":[]}\n");

        let declared = admit(&["report.md", "out/summary.json", "a b/\u{e9}\"q\\\n.md", ".hidden", "a..b"]).unwrap();
        assert_eq!(
            declared.paths(),
            ["report.md", "out/summary.json", "a b/\u{e9}\"q\\\n.md", ".hidden", "a..b"]
        );
        assert_eq!(
            declared.json(),
            "[\"report.md\",\"out/summary.json\",\"a b/\u{e9}\\\"q\\\\\\n.md\",\".hidden\",\"a..b\"]"
        );
        assert_eq!(declared.argument(), format!("--deliverables={}", declared.json()));
        assert_eq!(
            Deliverables::from_record(declared.record().as_bytes()).unwrap(),
            declared
        );
        // The wire form is the plain array, both ways.
        assert_eq!(
            serde_json::to_string(&declared).unwrap(),
            declared.json()
        );
        assert_eq!(
            serde_json::from_str::<Deliverables>(declared.json()).unwrap(),
            declared
        );
    }

    #[test]
    fn every_broken_rule_is_refused_by_name_with_its_entry_and_a_next_action() {
        let long_component = "c".repeat(NAME_MAX + 1);
        // Paths of whole NAME_MAX components, so only their length is at fault.
        let components = |count: usize| vec!["d".repeat(NAME_MAX); count].join("/");
        let long_path = components(16);
        assert_eq!(long_path.len(), PATH_MAX - 1);
        let longest_path = format!(
            "{}/{}",
            components(15),
            "e".repeat(MAX_DELIVERABLE_PATH_BYTES - components(15).len() - 1)
        );
        assert_eq!(longest_path.len(), MAX_DELIVERABLE_PATH_BYTES);
        assert!(admit(&[&longest_path, &"n".repeat(NAME_MAX)]).is_ok());
        for (paths, fragment) in [
            (vec![""], "is empty"),
            (vec!["ok.md", "a\0b"], "NUL byte"),
            (vec!["/artifacts/report.md"], "starts with \"/\""),
            (vec!["/"], "starts with \"/\""),
            (vec!["out//report.md"], "empty component"),
            (vec!["out/"], "empty component"),
            (vec!["./report.md"], "\".\" component"),
            (vec!["out/../../etc/passwd"], "\"..\" component"),
            (vec![".."], "\"..\" component"),
            (vec![long_component.as_str()], "NAME_MAX"),
            (vec![long_path.as_str()], "PATH_MAX"),
            (vec!["a.md", "b.md", "a.md"], "repeats deliverables[0]"),
        ] {
            let error = admit(&paths).expect_err(&format!("{paths:?} was admitted"));
            let message = error.to_string();
            let offender = paths.len() - 1;
            assert!(
                message.starts_with(&format!("field `deliverables[{offender}]` (")),
                "{message}"
            );
            assert!(message.contains(fragment), "{fragment}: {message}");
        }
        // A refusal quotes a long entry without echoing all of it.
        let error = admit(&[&long_component]).unwrap_err().to_string();
        assert!(error.contains(&format!("... ({} bytes)", NAME_MAX + 1)), "{error}");
        assert!(error.len() < 600, "{error}");
    }

    #[test]
    fn the_whole_list_is_bounded_by_the_one_argument_that_carries_it() {
        // Distinct entries of 4,084 bytes, each component within NAME_MAX:
        // each costs its bytes, two quotes and a comma in the compact form.
        let directory = vec!["d".repeat(NAME_MAX); 15].join("/");
        let entry = |index: usize| {
            format!(
                "{directory}/{index:05}{}",
                "x".repeat(MAX_DELIVERABLE_PATH_BYTES - directory.len() - 1 - 5)
            )
        };
        assert_eq!(entry(0).len(), MAX_DELIVERABLE_PATH_BYTES);
        let mut paths = (0..32).map(entry).collect::<Vec<_>>();
        // 32 entries: 2 + 32 * 4086 + 31 = 130,785 bytes, inside the bound.
        let at_most = Deliverables::new(paths.clone()).unwrap();
        assert_eq!(at_most.json().len(), 130_785);
        // Fill to exactly the bound with one more entry of 268 bytes, in two
        // components within NAME_MAX.
        let filler = MAX_DELIVERABLES_JSON_BYTES - 130_785 - 3;
        assert_eq!(filler, 268);
        paths.push(format!("{}/{}", "f".repeat(filler / 2), "g".repeat(filler - filler / 2 - 1)));
        let exactly = Deliverables::new(paths.clone()).unwrap();
        assert_eq!(exactly.json().len(), MAX_DELIVERABLES_JSON_BYTES);
        assert_eq!(exactly.argument().len() + 1, MAX_ARG_STRLEN);
        assert_eq!(exactly.record().len(), MAX_RECORD_BYTES);
        assert_eq!(Deliverables::from_record(exactly.record().as_bytes()).unwrap(), exactly);
        // One byte more is refused, naming the bound and its reason.
        paths.last_mut().unwrap().push('g');
        let message = Deliverables::new(paths).unwrap_err().to_string();
        assert!(
            message.contains(&format!("{}", MAX_DELIVERABLES_JSON_BYTES + 1))
                && message.contains("MAX_ARG_STRLEN")
                && message.contains("Declare fewer or shorter paths"),
            "{message}"
        );
    }

    #[test]
    fn a_control_record_is_read_only_in_its_one_canonical_spelling() {
        let canonical = admit(&["report.md", "\u{e9}.md"]).unwrap();
        assert_eq!(
            canonical.record(),
            "{\"deliverables\":[\"report.md\",\"\u{e9}.md\"]}\n"
        );
        for malformed in [
            "{\"deliverables\":[\"report.md\",\"\u{e9}.md\"]}".to_string(),
            "{\"deliverables\":[\"report.md\",\"\u{e9}.md\"]}\n\n".to_string(),
            "{\"deliverables\": [\"report.md\",\"\u{e9}.md\"]}\n".to_string(),
            "{\"deliverables\":[\"report.md\", \"\u{e9}.md\"]}\n".to_string(),
            "{\"deliverables\":[\"report.md\",\"\\u00e9.md\"]}\n".to_string(),
            "{\"deliverables\":[\"report.md\",\"\u{e9}.md\"],\"extra\":1}\n".to_string(),
            " {\"deliverables\":[]}\n".to_string(),
            "{\"deliverables\":null}\n".to_string(),
            "{\"deliverables\":\"report.md\"}\n".to_string(),
            "{\"deliverables\":[1]}\n".to_string(),
            "{\"max_session_turns\":400}\n".to_string(),
            String::new(),
        ] {
            let error = Deliverables::from_record(malformed.as_bytes())
                .expect_err(&format!("{malformed:?} was read"));
            assert!(error.to_string().contains("canonical"), "{error}");
        }
        // A canonical spelling of a list the rule refuses is refused by the rule.
        let error = Deliverables::from_record(b"{\"deliverables\":[\"../escape\"]}\n").unwrap_err();
        assert!(error.to_string().contains("\"..\" component"), "{error}");
        let error = Deliverables::from_record(b"{\"deliverables\":[\"a\",\"a\"]}\n").unwrap_err();
        assert!(error.to_string().contains("repeats"), "{error}");
        // Invalid UTF-8 and an oversized record are refused before parsing.
        assert!(Deliverables::from_record(b"{\"deliverables\":[\"\xff\"]}\n").is_err());
        assert!(Deliverables::from_record(&vec![b' '; MAX_RECORD_BYTES + 1])
            .unwrap_err()
            .to_string()
            .contains(&format!("observed {} bytes", MAX_RECORD_BYTES + 1)));
    }

    #[test]
    fn a_decoded_list_goes_through_the_rule() {
        assert!(serde_json::from_str::<Deliverables>("[\"report.md\"]").is_ok());
        for refused in ["[\"/etc/passwd\"]", "[\"a\",\"a\"]", "[\"\"]"] {
            let error = serde_json::from_str::<Deliverables>(refused).unwrap_err();
            assert!(error.to_string().contains("field `deliverables[0]`") || error.to_string().contains("field `deliverables[1]`"), "{error}");
        }
    }
}
