use sha2::{Digest, Sha256};
use std::{env, fs, path::PathBuf};

#[allow(dead_code)]
#[path = "src/json.rs"]
mod json;
#[allow(dead_code)]
#[path = "src/number.rs"]
mod number;
mod schema_compiler;

fn main() {
    let path = "../stream-contract-v1.json";
    println!("cargo:rerun-if-changed={path}");
    let bytes = fs::read(path).expect("read the shared stream contract");
    let compiled =
        schema_compiler::compile(&bytes).expect("compile the exact owned stream contract");
    let hash = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    println!("cargo:rustc-env=STREAM_CONTRACT_SHA256={hash}");
    let mut generated = format!("pub const STREAM_CONTRACT_SHA256: &str = {hash:?};\n");
    generated.push_str(&format!(
        "pub const SUCCESS_SUBTYPE: &str = {:?};\npub const ERROR_SUBTYPES: [&str; {}] = {:?};\n",
        compiled.success_subtype,
        compiled.error_subtypes.len(),
        compiled.error_subtypes
    ));
    for (name, variants) in &compiled.discriminators {
        generate_enum(&mut generated, name, variants);
    }
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output directory"));
    fs::write(output.join("compiled_schema.rs"), compiled.rust)
        .expect("write compiled schema tables");
    fs::write(output.join("schema_inventory.json"), compiled.inventory)
        .expect("write complete schema inventory");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output directory"))
            .join("stream_contract.rs"),
        generated,
    )
    .expect("write generated stream bindings");
}

fn generate_enum(generated: &mut String, enum_name: &str, variants: &[String]) {
    generated.push_str(&format!(
        "#[derive(Clone, Copy, Debug, serde::Deserialize, serde::Serialize, PartialEq, Eq)]\npub enum {enum_name} {{\n"
    ));
    let mut names = std::collections::HashSet::new();
    for name in variants {
        assert!(names.insert(name), "duplicate event discriminator");
        assert!(name.bytes().all(|c| c.is_ascii_lowercase() || c == b'_'));
        let ident = name
            .split('_')
            .map(|part| format!("{}{}", part[..1].to_ascii_uppercase(), &part[1..]))
            .collect::<String>();
        generated.push_str(&format!("#[serde(rename = {name:?})] {ident},\n"));
    }
    generated.push_str("}\n");
    generated.push_str(&format!(
        "impl {enum_name} {{ pub fn from_wire(value: &str) -> Option<Self> {{ match value {{\n"
    ));
    for name in variants {
        let ident = name
            .split('_')
            .map(|part| format!("{}{}", part[..1].to_ascii_uppercase(), &part[1..]))
            .collect::<String>();
        generated.push_str(&format!("{name:?} => Some(Self::{ident}),\n"));
    }
    generated.push_str("_ => None, } } pub fn wire(self) -> &'static str { match self {\n");
    for name in variants {
        let ident = name
            .split('_')
            .map(|part| format!("{}{}", part[..1].to_ascii_uppercase(), &part[1..]))
            .collect::<String>();
        generated.push_str(&format!("Self::{ident} => {name:?},\n"));
    }
    generated.push_str("} } }\n");
}
