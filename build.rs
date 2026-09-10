fn main() {
    println!(
        "cargo:rustc-env=STREAM_CONTRACT_SHA256={}",
        runtime_contract::STREAM_CONTRACT_SHA256
    );
    let read = |path: &str| -> serde_json::Value {
        println!("cargo:rerun-if-changed={path}");
        serde_json::from_slice(&std::fs::read(path).expect("read captured-stream build input"))
            .expect("parse captured-stream build input")
    };
    let runtime = read("config/agent-runtime-contract-v1.json");
    let stack = read("config/stack.lock.json");
    let settings = read("docker/config/settings.json");
    // Embed only facts used by captured-stream certification. Embedding the
    // whole stack lock would put the agent image's own hash into its certifier
    // and prevent a reproducible image/lock pair from ever converging.
    let bindings = serde_json::json!({
        "stream_contract_sha256": runtime_contract::STREAM_CONTRACT_SHA256,
        "cwd": runtime["filesystem"]["workspace"], "model": runtime["model"]["served_name"],
        "permission_mode": settings["tools"]["approvalMode"], "qwen_code_version": stack["agent"]["qwen_code"]["version"],
        "tools": runtime["native_tools"], "agents": runtime["subagents"]["allowed_roles"],
        "mcp_servers": [], "slash_commands": []
    }).to_string();
    let document = runtime_contract::json::Document::decode(
        bindings.as_bytes(),
        runtime_contract::json::Limits {
            bytes: bindings.len(),
            nodes: bindings.len(),
            depth: bindings.len(),
        },
    )
    .expect("decode captured-stream bindings");
    runtime_contract::runtime::RuntimeBindings::new(document)
        .expect("validate captured-stream bindings");
    println!("cargo:rustc-env=CAPTURED_STREAM_BINDINGS_JSON={bindings}");
}
