//! The checked-in stack lock is the only configuration source.
//!
//! This service intentionally has one mode of operation.  There are no
//! environment-variable overrides, defaults, compatibility aliases, or
//! optional profiles.  `config/stack.lock.json` is compiled into the binary,
//! parsed with `deny_unknown_fields`, and validated before Docker is touched.

use std::net::SocketAddr;
use std::path::PathBuf;

use serde::Deserialize;

use crate::error::{ServiceError, ServiceResult};

pub const STACK_LOCK_JSON: &str = include_str!("../config/stack.lock.json");
pub const QWEN_CODE_VERSION: &str = "0.21.12";

/// The prompt is additionally bounded by the HTTP request-body limit from
/// the lock file.  Keeping this lower semantic limit explicit makes error
/// messages useful when a syntactically valid JSON body contains an
/// unexpectedly enormous prompt.
pub const MAX_PROMPT_BYTES: usize = 1024 * 1024;
pub const MAX_STAGED_BYTES: u64 = 4 * 1024 * 1024 * 1024;
pub const MAX_STAGED_FILES: u64 = 200_000;

#[derive(Clone, Debug)]
pub struct Config {
    pub lock: StackLock,
    pub listen_addr: SocketAddr,
    pub state_dir: PathBuf,
    pub results_dir: PathBuf,
    pub host_input_root: PathBuf,
    pub agent_image: String,
    pub agent_memory_limit: String,
    pub agent_memory_swap_limit: String,
    pub vllm_model_name: String,
    pub vllm_endpoint: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StackLock {
    pub schema_version: u32,
    pub profile: String,
    pub build: BuildLock,
    pub service: ServiceLock,
    pub agent: AgentLock,
    pub backend: BackendLock,
    pub host: HostLock,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildLock {
    pub ubuntu_amd64_image: String,
    pub ubuntu_snapshot: String,
    pub node_amd64_image: String,
    pub rust_amd64_image: String,
    pub docker_cli_archive: String,
    pub docker_cli_archive_sha256: String,
    pub agent_apt_lock_sha256: String,
    pub service_apt_lock_sha256: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceLock {
    pub listen: String,
    pub container_name: String,
    pub image_tag: String,
    pub state_dir: String,
    pub results_dir: String,
    pub host_input_root: String,
    pub request_body_limit_bytes: usize,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentLock {
    pub image_tag: String,
    pub image_id: String,
    pub memory: String,
    pub memory_swap: String,
    pub pids_limit: u32,
    pub tmpfs_tmp: String,
    pub tmpfs_qwen_home: String,
    pub tmpfs_qwen_runtime: String,
    pub settings_sha256: String,
    pub instructions_sha256: String,
    pub wrapper_sha256: String,
    pub model_base_url: String,
    pub model_proxy_port: u16,
    pub strict_tools: Vec<String>,
    pub qwen_code: QwenCodeLock,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QwenCodeLock {
    pub version: String,
    pub tag: String,
    pub commit: String,
    pub source_archive: String,
    pub source_archive_sha256: String,
    pub patch_sha256: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendLock {
    pub project_dir: String,
    pub container_name: String,
    pub project_label: String,
    pub profile_label: String,
    pub image_tag: String,
    pub image_id: String,
    pub endpoint: String,
    pub version: String,
    pub vllm_commit: String,
    pub served_model: String,
    pub max_model_len: u64,
    pub model_repository: String,
    pub model_revision: String,
    pub model_manifest: String,
    pub model_manifest_sha256: String,
    pub kv_cache_dtype: String,
    pub command: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostLock {
    pub docker_version: String,
    pub docker_buildx_version: String,
    pub buildkit_version: String,
    pub git_version: String,
    pub jq_version: String,
    pub coreutils_version: String,
    pub nvidia_container_cli_version: String,
    pub gpu_name: String,
    pub gpu_memory_mib: u64,
    pub driver_version: String,
    pub docker_socket: String,
    pub docker_socket_gid: u32,
}

impl Config {
    pub fn load() -> ServiceResult<Self> {
        reject_legacy_overrides()?;

        let disk_lock_path = "/home/user/Desktop/agent_service/config/stack.lock.json";
        let disk_lock = std::fs::read_to_string(disk_lock_path).map_err(|error| {
            ServiceError::Internal(format!(
                "cannot read the mounted stack lock at {disk_lock_path}: {error}"
            ))
        })?;
        if disk_lock.as_bytes() != STACK_LOCK_JSON.as_bytes() {
            return Err(ServiceError::Internal(
                "the mounted config/stack.lock.json differs byte-for-byte from the lock compiled into the service image; rebuild before starting".into(),
            ));
        }

        let lock: StackLock = serde_json::from_str(STACK_LOCK_JSON).map_err(|e| {
            ServiceError::Internal(format!(
                "compiled config/stack.lock.json is malformed or does not match the strict schema: {e}"
            ))
        })?;
        validate_lock(&lock)?;

        let listen_addr: SocketAddr = lock.service.listen.parse().map_err(|e| {
            ServiceError::Internal(format!(
                "stack lock service.listen {:?} is not a socket address: {e}",
                lock.service.listen
            ))
        })?;
        if listen_addr != "127.0.0.1:8090".parse().expect("literal is valid") {
            return Err(ServiceError::Internal(format!(
                "the sole supported service.listen is 127.0.0.1:8090; lock contains {listen_addr}"
            )));
        }

        Ok(Self {
            listen_addr,
            state_dir: PathBuf::from(&lock.service.state_dir),
            results_dir: PathBuf::from(&lock.service.results_dir),
            host_input_root: PathBuf::from(&lock.service.host_input_root),
            agent_image: lock.agent.image_tag.clone(),
            agent_memory_limit: lock.agent.memory.clone(),
            agent_memory_swap_limit: lock.agent.memory_swap.clone(),
            vllm_model_name: lock.backend.served_model.clone(),
            vllm_endpoint: lock.backend.endpoint.clone(),
            lock,
        })
    }
}

fn reject_legacy_overrides() -> ServiceResult<()> {
    let mut present = std::env::vars_os()
        .filter_map(|(key, _)| key.into_string().ok())
        .filter(|key| key.starts_with("AGENT_SERVICE_") || key.starts_with("OPENAI_"))
        .collect::<Vec<_>>();
    present.sort();
    if !present.is_empty() {
        return Err(ServiceError::Internal(format!(
            "unsupported configuration environment variable(s) present: {}. This project has exactly one mode; edit and rebuild the pinned stack lock instead of overriding runtime semantics.",
            present.join(", ")
        )));
    }
    Ok(())
}

fn validate_lock(lock: &StackLock) -> ServiceResult<()> {
    let fail = |message: String| Err(ServiceError::Internal(format!("stack lock: {message}")));

    if lock.schema_version != 1 {
        return fail(format!(
            "schema_version must be 1, got {}",
            lock.schema_version
        ));
    }
    if lock.profile != "qwen38-agent-service-v1" {
        return fail(format!("unexpected profile {:?}", lock.profile));
    }
    if lock.service.container_name != "qwen38-agent-service"
        || lock.service.image_tag != "qwen38-agent-service:1.0.0"
    {
        return fail(
            "service container/image identity differs from the sole supported deployment".into(),
        );
    }
    if lock.agent.qwen_code.version != QWEN_CODE_VERSION
        || lock.agent.qwen_code.tag != "v0.21.12"
        || lock.agent.qwen_code.commit.len() != 40
    {
        return fail(
            "Qwen Code version/tag/commit pin is inconsistent with this source tree".into(),
        );
    }
    let expected_archive = format!(
        "https://codeload.github.com/QwenLM/qwen-code/tar.gz/{}",
        lock.agent.qwen_code.commit
    );
    if lock.agent.qwen_code.source_archive != expected_archive {
        return fail("Qwen Code source URL is not derived from its exact commit".into());
    }
    for (name, digest) in [
        ("ubuntu_amd64_image", lock.build.ubuntu_amd64_image.as_str()),
        ("node_amd64_image", lock.build.node_amd64_image.as_str()),
        ("rust_amd64_image", lock.build.rust_amd64_image.as_str()),
    ] {
        if !digest.contains("@sha256:") || digest.rsplit(':').next().map(str::len) != Some(64) {
            return fail(format!(
                "build.{name} is not an immutable sha256 image reference"
            ));
        }
    }
    if lock.build.ubuntu_snapshot.len() != 16 || !lock.build.ubuntu_snapshot.ends_with('Z') {
        return fail("build.ubuntu_snapshot must be an explicit YYYYMMDDThhmmssZ snapshot".into());
    }
    if lock.build.docker_cli_archive
        != "https://download.docker.com/linux/static/stable/x86_64/docker-29.7.2.tgz"
        || lock.build.docker_cli_archive_sha256.len() != 64
        || lock.build.agent_apt_lock_sha256.len() != 64
        || lock.build.service_apt_lock_sha256.len() != 64
    {
        return fail("Docker CLI archive or apt lock hashes are not exact".into());
    }
    if lock.service.request_body_limit_bytes != 2 * 1024 * 1024 {
        return fail("service.request_body_limit_bytes must be exactly 2097152".into());
    }
    for (name, path) in [
        ("state_dir", lock.service.state_dir.as_str()),
        ("results_dir", lock.service.results_dir.as_str()),
        ("host_input_root", lock.service.host_input_root.as_str()),
        ("backend.project_dir", lock.backend.project_dir.as_str()),
        ("host.docker_socket", lock.host.docker_socket.as_str()),
    ] {
        if !std::path::Path::new(path).is_absolute() {
            return fail(format!("{name} must be an absolute path, got {path:?}"));
        }
    }
    if !PathBuf::from(&lock.service.state_dir).starts_with(&lock.backend.project_dir)
        && !PathBuf::from(&lock.service.state_dir).starts_with("/home/user/Desktop/agent_service")
    {
        return fail("state_dir is outside the pinned project runtime directory".into());
    }
    if lock.agent.model_base_url != "http://127.0.0.1:18000/v1"
        || lock.agent.model_proxy_port != 18000
    {
        return fail("agent model proxy must be exactly http://127.0.0.1:18000/v1".into());
    }
    if !lock.agent.image_id.starts_with("sha256:") || lock.agent.image_id.len() != 71 {
        return fail(
            "agent.image_id is not an exact built Docker image ID; run the pinned build workflow"
                .into(),
        );
    }
    if [
        lock.agent.settings_sha256.as_str(),
        lock.agent.instructions_sha256.as_str(),
        lock.agent.wrapper_sha256.as_str(),
    ]
    .iter()
    .any(|digest| digest.len() != 64)
    {
        return fail("agent settings/instructions/wrapper hashes are not SHA256 values".into());
    }
    let expected_tools = [
        "agent",
        "edit",
        "glob",
        "grep_search",
        "list_directory",
        "notebook_edit",
        "read_file",
        "run_shell_command",
        "todo_write",
        "write_file",
    ];
    if lock
        .agent
        .strict_tools
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        != expected_tools
    {
        return fail("agent.strict_tools differs from the reviewed strict native tool set".into());
    }
    if lock.backend.endpoint != "http://127.0.0.1:8000"
        || lock.backend.served_model != "qwen3.8-27b-nvfp4-k8v4"
        || lock.backend.max_model_len != 262_144
        || lock.backend.kv_cache_dtype != "turboquant_k8v4"
    {
        return fail(
            "backend endpoint/model/context/KV contract differs from the sole supported deployment"
                .into(),
        );
    }
    if lock.backend.model_repository != "unsloth/Qwen3.8-27B-NVFP4"
        || lock.backend.model_manifest != "model-snapshot-16b6615a.sha256"
    {
        return fail(
            "backend model repository/manifest identity differs from the refreshed snapshot".into(),
        );
    }
    if !lock.backend.image_id.starts_with("sha256:") || lock.backend.image_id.len() != 71 {
        return fail("backend.image_id is not an exact Docker image ID".into());
    }
    if lock.backend.model_revision.len() != 40
        || lock.backend.vllm_commit.len() != 40
        || lock.backend.model_manifest_sha256.len() != 64
        || lock.agent.qwen_code.source_archive_sha256.len() != 64
        || lock.agent.qwen_code.patch_sha256.len() != 64
    {
        return fail(
            "one or more source/model/patch commit or SHA256 pins have the wrong length".into(),
        );
    }
    if lock.backend.command.is_empty() {
        return fail("backend.command may not be empty".into());
    }
    if lock.host.nvidia_container_cli_version != "1.19.1"
        || lock.host.docker_buildx_version != "v0.36.1"
        || lock.host.buildkit_version != "v0.32.2"
        || lock.host.git_version != "2.43.0"
        || lock.host.jq_version != "jq-1.7"
        || lock.host.coreutils_version != "9.4"
        || lock.host.gpu_name != "NVIDIA GeForce RTX 5090"
        || lock.host.gpu_memory_mib != 32_607
        || lock.host.driver_version != "595.71.05"
        || lock.host.docker_socket != "/var/run/docker.sock"
        || lock.host.docker_socket_gid != 984
    {
        return fail(
            "host GPU/driver/container-runtime/socket contract differs from the reviewed machine"
                .into(),
        );
    }
    Ok(())
}
