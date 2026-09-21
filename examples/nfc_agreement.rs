//! The service's normalizer over the agreement probes: the pinned
//! `unicode-normalization` that `validation.rs` refuses a prompt with.
//!
//! Reads probes.json on stdin and writes the NFC of every probe on stdout, for
//! `scripts/test-normalizer-agreement.sh` to hold against the served
//! tokenizer's own normalizer.

use std::io::{Read, Write};

use serde_json::{Map, Value};
use unicode_normalization::{UnicodeNormalization, UNICODE_VERSION};

fn main() {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .expect("probes.json on stdin");
    let probes: Value = serde_json::from_str(&input).expect("probes.json is JSON");
    let mut out = Map::new();
    let (major, minor, update) = UNICODE_VERSION;
    out.insert(
        "unicode_version".into(),
        Value::String(format!("{major}.{minor}.{update}")),
    );
    for kind in ["alone", "acute", "ypo"] {
        let normalized = probes[kind]
            .as_array()
            .expect("a list of probes for every kind")
            .iter()
            .map(|probe| {
                let text = probe.as_str().expect("a probe is text");
                Value::String(text.nfc().collect())
            })
            .collect();
        out.insert(kind.into(), Value::Array(normalized));
    }
    std::io::stdout()
        .write_all(serde_json::to_string(&out).expect("serializable").as_bytes())
        .expect("stdout");
}
