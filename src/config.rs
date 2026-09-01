//! The checked-in stack lock is the only configuration source.
//!
//! This service intentionally has one mode of operation.  There are no
//! environment-variable overrides, defaults, compatibility aliases, or
//! optional profiles.  `config/stack.lock.json` is compiled into the binary,
//! parsed with `deny_unknown_fields`, and validated before Docker is touched.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;

use serde::Deserialize;

use crate::error::{ServiceError, ServiceResult};

pub const STACK_LOCK_JSON: &str = include_str!("../config/stack.lock.json");
pub const BROKER_POLICY_JSON: &str = include_str!("../config/broker-policy-v1.json");
pub const QWEN_CODE_VERSION: &str = "0.21.12";

/// The prompt is additionally bounded by the HTTP request-body limit from
/// the lock file.  Keeping this lower semantic limit explicit makes error
/// messages useful when a syntactically valid JSON body contains an
/// unexpectedly enormous prompt.
pub const MAX_PROMPT_BYTES: usize = 1024 * 1024;
// 200 GiB: the workspace-staging bound is disk accounting, not memory. The
// archive is streamed to a disk spool and never buffered in RAM, and the staged
// tree lives on ordinary disk-backed storage, so this caps the disk a single
// session may consume -- not any model or agent memory. The largest legitimate
// benchmark workspace observed (apache__hugegraph-3037) stages ~4.8 GiB; this
// deliberately admits far larger workspaces, with the whole path proven to
// stream, bound, and fail closed at this size.
pub const MAX_STAGED_BYTES: u64 = 200 * 1024 * 1024 * 1024;
pub const MAX_STAGED_FILES: u64 = 200_000;
pub const MAX_STAGED_ENTRIES: u64 = 250_000;

/// Upper bound on the transported workspace archive itself, applied to the
/// declared and streamed submission bytes. The archive travels over the
/// connection and is spooled to disk, never buffered in memory, so this is a
/// deliberate disk-accounting bound rather than a transport artifact: it is
/// the staged-content cap plus a fixed allowance for zip container overhead
/// (headers, central directory, stored-entry framing) on an incompressible
/// maximum-size workspace.
pub const MAX_ARCHIVE_BYTES: u64 = MAX_STAGED_BYTES + 64 * 1024 * 1024;

/// The one bound on how long a session may work: model turns, never wall time.
///
/// A wall-clock budget measures how fast this GPU generates, not how capably the
/// agent reasons -- the same trajectory that fits an hour on a fast backend is
/// guillotined on a slow one, so identical work scores differently for reasons
/// that have nothing to do with the agent. Turns are the hardware-independent
/// unit of agent progress, so this is what the deployment bounds.
///
/// 400 is a degenerate-loop circuit breaker, not a performance budget, derived
/// from what one repository-level task legitimately needs: orient and locate
/// (~15-30 turns), establish the offline build and a baseline test run (~5-15),
/// diagnose across the implicated code (~20-40), implement (~10-25), iterate the
/// verify loop (~20-60), then regression-check and finalise (~10-20) -- about
/// 80-190 for a clean pass. Doubling that admits one complete recovery pass
/// after a wrong hypothesis, so an agent that reaches 400 is looping rather than
/// converging. Qwen Code stops itself at this count and exits 53, which is an
/// ordinary terminal outcome graded on the work done, never an infrastructure
/// failure. `max_wall_time_seconds` stays disabled everywhere by design.
///
/// This is the budget a submission that says nothing about turns receives. A
/// caller that knows its task is shaped differently may name another budget in
/// the creation body; this value is what the deployment chose for everything
/// else, and it stays cross-checked against the stack lock, the agent runtime
/// contract, both sealed settings files, and the launcher's own constant.
pub const DEFAULT_MAX_SESSION_TURNS: u32 = 400;

/// The largest turn budget a submission may request.
///
/// A per-session budget is a task-shape decision, not an escape hatch from the
/// circuit breaker: something has to remain finite, or a degenerate loop simply
/// asks for a bigger number. 2000 is five default budgets -- room for a task
/// whose turn count is bounded by the corpus it must read rather than by the
/// reasoning it must do, where the floor is arithmetic: a tool result is capped
/// at 25,000 characters, so a multi-megabyte corpus costs hundreds of reads
/// before a single one is wasted. It still cannot be mistaken for unbounded.
/// A request outside `1..=2000` is refused with the offending value;
/// it is never clamped, because a silently shortened budget would look like an
/// ordinary turn-exhausted exit 53 and be graded as one.
///
/// Like the default, this is pinned rather than being a bare literal: the stack
/// lock carries it for the service, the agent runtime contract carries it for
/// the in-image verifier, and the launcher compiles its own copy that the
/// verifier proves equal to the contract's.
pub const MAX_SESSION_TURNS_CEILING: u32 = 2000;

// The default must itself be a budget a caller could have asked for. A build in
// which it is not is incoherent before it ever reaches a request.
const _: () = assert!(
    DEFAULT_MAX_SESSION_TURNS >= 1 && DEFAULT_MAX_SESSION_TURNS <= MAX_SESSION_TURNS_CEILING,
    "the default session turn budget must lie inside the requestable range"
);

#[derive(Clone, Debug)]
pub struct Config {
    pub lock: StackLock,
    pub listen_addr: SocketAddr,
    pub state_dir: PathBuf,
    pub results_dir: PathBuf,
    pub broker_socket: PathBuf,
    pub model_socket: PathBuf,
    pub agent_image: String,
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
    pub broker: BrokerLock,
    pub relay: RelayLock,
    pub capture: CaptureLock,
    pub agent: AgentLock,
    pub backend: BackendLock,
    pub host: HostLock,
    pub limits: LimitsLock,
}

/// Workspace/transport limits carried in the lock so the shell harness and the
/// service read exactly one value. validate_lock proves the lock's numbers
/// equal the constants this binary was compiled against, so the two can never
/// silently drift -- the defect that shrank the benchmark when a stale 4 GiB
/// shell mirror rejected a task the 8 GiB service admits.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LimitsLock {
    pub max_prompt_bytes: usize,
    pub max_staged_bytes: u64,
    pub max_staged_files: u64,
    pub max_staged_entries: u64,
    pub max_archive_bytes: u64,
    /// The turn budget a submission receives when it names none.
    pub max_session_turns: u32,
    /// The largest turn budget a submission may name.
    pub max_session_turns_ceiling: u32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerLock {
    pub policy_id: String,
    pub container_name: String,
    pub image_tag: String,
    pub image_id: String,
    pub socket_path: String,
    pub policy_sha256: String,
    pub source_sha256: String,
    pub memory: String,
    pub memory_swap: String,
    pub pids_limit: u32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayLock {
    pub image_tag: String,
    pub image_id: String,
    pub source_sha256: String,
    pub sandbox: String,
    pub memory: String,
    pub memory_swap: String,
    pub pids_limit: u32,
    pub model_socket_dir: String,
    pub service_socket_dir: String,
    pub model_bridge_container: String,
    pub model_ingress_container: String,
    pub service_bridge_container: String,
    pub service_ingress_container: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureLock {
    pub image_tag: String,
    pub image_id: String,
    pub source_sha256: String,
    pub capture_id: String,
    pub memory: String,
    pub memory_swap: String,
    pub pids_limit: u32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildLock {
    pub source_date_epoch: u64,
    pub ubuntu_amd64_image: String,
    pub ubuntu_snapshot: String,
    pub node_amd64_image: String,
    pub rust_amd64_image: String,
    pub docker_cli_archive: String,
    pub docker_cli_archive_sha256: String,
    pub go_archive: String,
    pub go_archive_sha256: String,
    pub agent_apt_lock_sha256: String,
    pub toolchain_verifier_sha256: String,
    pub toolchain_verifier_test_sha256: String,
    pub runtime_contract_verifier_sha256: String,
    pub runtime_contract_verifier_test_sha256: String,
    pub wrapper_contract_test_sha256: String,
    pub jks_normalizer_sha256: String,
    pub jks_normalizer_test_sha256: String,
    pub service_apt_lock_sha256: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceLock {
    pub listen: String,
    pub container_name: String,
    pub image_tag: String,
    pub user: String,
    pub memory: String,
    pub memory_swap: String,
    pub pids_limit: u32,
    pub tmpfs_tmp: String,
    pub runtime_root: String,
    pub state_dir: String,
    pub results_dir: String,
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
    pub tmpfs_qwen_runtime: String,
    pub settings_sha256: String,
    pub instructions_sha256: String,
    pub system_prompt_sha256: String,
    pub deployment_contract_sha256: String,
    pub toolchain_manifest_sha256: String,
    pub runtime_contract_sha256: String,
    pub wrapper_sha256: String,
    pub agent_exec_source_sha256: String,
    pub agent_exec_sandbox: String,
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
    pub source_patch_manifest_sha256: String,
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
    pub user: String,
    pub rootfs_read_only: bool,
    pub tmpfs: BTreeMap<String, String>,
    pub cache_volume: String,
    pub cache_mount: String,
    pub cache_owner_mode: String,
    pub endpoint: String,
    pub version: String,
    pub vllm_commit: String,
    pub served_model: String,
    pub max_model_len: u64,
    pub model_repository: String,
    pub model_revision: String,
    pub official_model_repository: String,
    pub official_model_revision: String,
    pub model_directory: String,
    pub model_correction: String,
    pub model_sha256: String,
    pub model_manifest: String,
    pub model_manifest_sha256: String,
    pub kv_cache_dtype: String,
    pub vision: VisionLock,
    pub agent_defaults: AgentDefaultsLock,
    pub environment: Vec<String>,
    pub command: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VisionLock {
    pub enabled: bool,
    pub unquantized_dtype: String,
    pub max_images: u32,
    pub max_source_pixels: u64,
    pub max_aspect_ratio: u32,
    pub allowed_data_url_prefix: String,
    pub source_modes: Vec<String>,
    pub video_count: u32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentDefaultsLock {
    pub enable_thinking: bool,
    pub reasoning_effort: String,
    pub add_vision_id: bool,
    pub temperature: f64,
    pub top_p: f64,
    pub top_k: u32,
    pub min_p: f64,
    pub presence_penalty: f64,
    pub repetition_penalty: f64,
    pub thinking_token_budget: u64,
    pub final_response_token_budget: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
/// Functional host requirements only: the isolation features the containers
/// genuinely depend on, and the Docker control-socket wiring. Exact host
/// software versions, binary hashes, and GPU identity are deliberately not
/// recorded here — pinning them would tie the deployment to one specific
/// computer without making it more correct anywhere.
pub struct HostLock {
    pub docker_security_options: Vec<String>,
    pub container_apparmor_profile: String,
    pub docker_socket: String,
    pub docker_socket_gid: u32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BrokerPolicy {
    schema_version: u32,
    policy_id: String,
    profile: String,
    broker_container_name: String,
    broker: BrokerPolicyBroker,
    broker_socket_path: String,
    runtime_root: String,
    state_dir: String,
    model_socket_dir: String,
    service_container_name: String,
    backend_container_name: String,
    backend_cache_volume: String,
    backend_cache_mount: String,
    backend_cache_owner_mode: String,
    model_bridge_container_name: String,
    model_ingress_container_name: String,
    agent: BrokerPolicyAgent,
    relay: BrokerPolicyRelay,
    capture: BrokerPolicyCapture,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BrokerPolicyBroker {
    image_tag: String,
    memory: String,
    memory_swap: String,
    pids_limit: u32,
    uid: u32,
    gid: u32,
    docker_socket: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BrokerPolicyAgent {
    image_tag: String,
    image_id: String,
    memory: String,
    memory_swap: String,
    pids_limit: u32,
    tmpfs_tmp: String,
    tmpfs_qwen_runtime: String,
    ready_event_prefix: String,
    sandbox: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BrokerPolicyRelay {
    image_tag: String,
    image_id: String,
    sandbox: String,
    memory: String,
    memory_swap: String,
    pids_limit: u32,
    role: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BrokerPolicyCapture {
    image_tag: String,
    image_id: String,
    capture_id: String,
    memory: String,
    memory_swap: String,
    pids_limit: u32,
    ready_event: String,
    complete_event_prefix: String,
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
        if listen_addr != SocketAddr::from(([127, 0, 0, 1], 8090)) {
            return Err(ServiceError::Internal(format!(
                "the sole supported service.listen is 127.0.0.1:8090; lock contains {listen_addr}"
            )));
        }

        Ok(Self {
            listen_addr,
            state_dir: PathBuf::from(&lock.service.state_dir),
            results_dir: PathBuf::from(&lock.service.results_dir),
            broker_socket: PathBuf::from(&lock.broker.socket_path),
            model_socket: PathBuf::from(&lock.relay.model_socket_dir).join("relay.sock"),
            agent_image: lock.agent.image_tag.clone(),
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

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn validate_lock(lock: &StackLock) -> ServiceResult<()> {
    let fail = |message: String| Err(ServiceError::Internal(format!("stack lock: {message}")));

    if lock.schema_version != 1 {
        return fail(format!(
            "schema_version must be 1, got {}",
            lock.schema_version
        ));
    }
    if lock.profile != "qwen38-agent-service-v3" {
        return fail(format!("unexpected profile {:?}", lock.profile));
    }
    if lock.limits.max_prompt_bytes != MAX_PROMPT_BYTES
        || lock.limits.max_staged_bytes != MAX_STAGED_BYTES
        || lock.limits.max_staged_files != MAX_STAGED_FILES
        || lock.limits.max_staged_entries != MAX_STAGED_ENTRIES
        || lock.limits.max_archive_bytes != MAX_ARCHIVE_BYTES
        || lock.limits.max_session_turns != DEFAULT_MAX_SESSION_TURNS
        || lock.limits.max_session_turns_ceiling != MAX_SESSION_TURNS_CEILING
    {
        return fail(format!(
            "limits in the lock disagree with the compiled constants \
             (lock: prompt={} staged={} files={} entries={} archive={} turns={} turn_ceiling={}; \
             compiled: prompt={} staged={} files={} entries={} archive={} turns={} turn_ceiling={}); \
             the shell harness reads these from the lock, so they must match exactly",
            lock.limits.max_prompt_bytes,
            lock.limits.max_staged_bytes,
            lock.limits.max_staged_files,
            lock.limits.max_staged_entries,
            lock.limits.max_archive_bytes,
            lock.limits.max_session_turns,
            lock.limits.max_session_turns_ceiling,
            MAX_PROMPT_BYTES,
            MAX_STAGED_BYTES,
            MAX_STAGED_FILES,
            MAX_STAGED_ENTRIES,
            MAX_ARCHIVE_BYTES,
            DEFAULT_MAX_SESSION_TURNS,
            MAX_SESSION_TURNS_CEILING,
        ));
    }
    if lock.service.container_name != "qwen38-agent-service"
        || lock.service.image_tag != "qwen38-agent-service:3.1.0"
        || lock.service.user != "1000:1000"
        || lock.service.memory != "2g"
        || lock.service.memory_swap != "2g"
        || lock.service.pids_limit != 512
        || lock.service.tmpfs_tmp != "rw,nosuid,nodev,noexec,size=256m,mode=1777"
        || lock.service.runtime_root != "/home/user/Desktop/agent_service/.runtime"
    {
        return fail(
            "service container/image/user/resource contract differs from the sole supported deployment".into(),
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
    if lock.build.source_date_epoch != 1_786_725_153
        || lock.build.docker_cli_archive
            != "https://download.docker.com/linux/static/stable/x86_64/docker-29.7.2.tgz"
        || !is_sha256(&lock.build.docker_cli_archive_sha256)
        || lock.build.go_archive
            != "https://dl.google.com/go/go1.25.13.linux-amd64.tar.gz"
        || !is_sha256(&lock.build.go_archive_sha256)
        || !is_sha256(&lock.build.agent_apt_lock_sha256)
        || !is_sha256(&lock.build.jks_normalizer_sha256)
        || !is_sha256(&lock.build.jks_normalizer_test_sha256)
        || !is_sha256(&lock.build.service_apt_lock_sha256)
    {
        return fail(
            "source-date epoch, Docker CLI archive, apt locks, or JKS normalizer hashes are not exact"
                .into(),
        );
    }
    if lock.service.request_body_limit_bytes != 2 * 1024 * 1024 {
        return fail("service.request_body_limit_bytes must be exactly 2097152".into());
    }
    if lock.broker.policy_id != "qwen38-docker-broker-v1"
        || lock.broker.container_name != "qwen38-docker-broker"
        || lock.broker.image_tag != "qwen38-docker-broker:1.1.0"
        || lock.broker.memory != "64m"
        || lock.broker.memory_swap != "64m"
        || lock.broker.pids_limit != 64
        || !lock.broker.image_id.starts_with("sha256:")
        || lock.broker.image_id.len() != 71
        || !is_sha256(&lock.broker.policy_sha256)
        || !is_sha256(&lock.broker.source_sha256)
    {
        return fail(
            "broker identity, image, resource, or source-policy hash contract drift".into(),
        );
    }
    if lock.relay.image_tag != "qwen38-fixed-relay:1.0.0"
        || !lock.relay.image_id.starts_with("sha256:")
        || lock.relay.image_id.len() != 71
        || !is_sha256(&lock.relay.source_sha256)
        || lock.relay.sandbox != "landlock-net-v4+seccomp-socket-v2"
        || lock.relay.memory != "32m"
        || lock.relay.memory_swap != "32m"
        || lock.relay.pids_limit != 32
        || lock.relay.model_bridge_container != "qwen38-model-bridge"
        || lock.relay.model_ingress_container != "qwen38-model-ingress"
        || lock.relay.service_bridge_container != "qwen38-service-bridge"
        || lock.relay.service_ingress_container != "qwen38-service-ingress"
    {
        return fail(
            "fixed-relay identity, image, resource, or container-name contract drift".into(),
        );
    }
    if lock.capture.image_tag != "qwen38-session-capture:1.0.0"
        || !lock.capture.image_id.starts_with("sha256:")
        || lock.capture.image_id.len() != 71
        || !is_sha256(&lock.capture.source_sha256)
        || lock.capture.capture_id != "unix-stream-capture-v1"
        || lock.capture.memory != "32m"
        || lock.capture.memory_swap != "32m"
        || lock.capture.pids_limit != 32
    {
        return fail("session-capture identity, image, resource, or source contract drift".into());
    }
    for (name, path) in [
        ("runtime_root", lock.service.runtime_root.as_str()),
        ("state_dir", lock.service.state_dir.as_str()),
        ("results_dir", lock.service.results_dir.as_str()),
        ("broker.socket_path", lock.broker.socket_path.as_str()),
        (
            "relay.model_socket_dir",
            lock.relay.model_socket_dir.as_str(),
        ),
        (
            "relay.service_socket_dir",
            lock.relay.service_socket_dir.as_str(),
        ),
        ("backend.project_dir", lock.backend.project_dir.as_str()),
        ("host.docker_socket", lock.host.docker_socket.as_str()),
    ] {
        if !std::path::Path::new(path).is_absolute() {
            return fail(format!("{name} must be an absolute path, got {path:?}"));
        }
    }
    let runtime_root = PathBuf::from(&lock.service.runtime_root);
    if lock.service.state_dir != "/home/user/Desktop/agent_service/.runtime/state"
        || lock.service.results_dir != "/home/user/Desktop/agent_service/.runtime/results"
        || !PathBuf::from(&lock.service.state_dir).starts_with(&runtime_root)
        || !PathBuf::from(&lock.service.results_dir).starts_with(&runtime_root)
        || lock.broker.socket_path
            != "/home/user/Desktop/agent_service/.runtime/control/broker.sock"
        || lock.relay.model_socket_dir != "/home/user/Desktop/agent_service/.runtime/model-socket"
        || lock.relay.service_socket_dir
            != "/home/user/Desktop/agent_service/.runtime/service-socket"
        || !PathBuf::from(&lock.broker.socket_path).starts_with(&runtime_root)
        || !PathBuf::from(&lock.relay.model_socket_dir).starts_with(&runtime_root)
        || !PathBuf::from(&lock.relay.service_socket_dir).starts_with(&runtime_root)
    {
        return fail(
            "input root or broker/model/service Unix-socket paths drift from the one runtime tree"
                .into(),
        );
    }
    if lock.agent.model_base_url != "http://127.0.0.1:18000/v1"
        || lock.agent.model_proxy_port != 18000
        || lock.agent.memory != "32g"
        || lock.agent.memory_swap != "32g"
        || lock.agent.pids_limit != 4096
        || lock.agent.tmpfs_tmp != "rw,nosuid,nodev,size=8g,mode=1777"
        || lock.agent.tmpfs_qwen_runtime
            != "rw,nosuid,nodev,noexec,size=2g,uid=1000,gid=1000,mode=0700"
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
        lock.agent.system_prompt_sha256.as_str(),
        lock.agent.deployment_contract_sha256.as_str(),
        lock.agent.toolchain_manifest_sha256.as_str(),
        lock.agent.runtime_contract_sha256.as_str(),
        lock.agent.wrapper_sha256.as_str(),
        lock.agent.agent_exec_source_sha256.as_str(),
        lock.build.toolchain_verifier_sha256.as_str(),
        lock.build.toolchain_verifier_test_sha256.as_str(),
        lock.build.runtime_contract_verifier_sha256.as_str(),
        lock.build.runtime_contract_verifier_test_sha256.as_str(),
        lock.build.wrapper_contract_test_sha256.as_str(),
    ]
    .iter()
    .any(|digest| !is_sha256(digest))
    {
        return fail(
            "agent settings/instructions/prompts/toolchain/wrapper hashes are not SHA256 values"
                .into(),
        );
    }
    if lock.agent.agent_exec_sandbox
        != "landlock-fs-v4-write-roots-v1+private-devpts-rw-v1+output-unmounted-v1"
    {
        return fail("agent_exec sandbox identity drift".into());
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
    if lock.backend.container_name != "qwen38-agent-native"
        || lock.backend.endpoint != "http://127.0.0.1:8000"
        || lock.backend.profile_label != "socket-isolated-nonroot-vision-k8v4-agent-v18"
        || lock.backend.image_tag != "qwen38-vllm:qwen38-27b-nvfp4-k8v4-runtime-v18"
        || lock.backend.image_id
            != "sha256:213ac83229e4eddb6ab58a14e71a1606cd08ed3888461d5d86cc68a5546bb611"
        || lock.backend.served_model != "qwen3.8-27b-nvfp4-k8v4"
        || lock.backend.max_model_len != 262_144
        || lock.backend.kv_cache_dtype != "turboquant_k8v4"
    {
        return fail(
            "backend endpoint/profile/image/model/context/KV contract differs from the sole supported deployment"
                .into(),
        );
    }
    let expected_backend_tmpfs = BTreeMap::from([
        (
            "/run".to_string(),
            "rw,nosuid,nodev,noexec,size=64m,uid=2000,gid=0,mode=0700".to_string(),
        ),
        (
            "/tmp".to_string(),
            "rw,nosuid,nodev,exec,size=2g,mode=1777".to_string(),
        ),
    ]);
    if lock.backend.user != "2000:0"
        || !lock.backend.rootfs_read_only
        || lock.backend.tmpfs != expected_backend_tmpfs
        || lock.backend.cache_volume != "qwen38-vllm-cache-socket-isolated-nonroot-vision-agent-v18"
        || lock.backend.cache_mount != "/home/vllm/.cache/vllm"
        || lock.backend.cache_owner_mode != "2000:0:770"
    {
        return fail(
            "backend immutable-root/tmpfs/cache-volume contract differs from the sole supported deployment"
                .into(),
        );
    }
    if !lock.backend.vision.enabled
        || lock.backend.vision.unquantized_dtype != "bfloat16"
        || lock.backend.vision.max_images != 15
        || lock.backend.vision.max_source_pixels != 16_777_216
        || lock.backend.vision.max_aspect_ratio != 30
        || lock.backend.vision.allowed_data_url_prefix != "data:image/png;base64,"
        || lock
            .backend
            .vision
            .source_modes
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            != ["RGB", "RGBA"]
        || lock.backend.vision.video_count != 0
    {
        return fail("backend vision contract differs from the sole full-quality profile".into());
    }
    let defaults = &lock.backend.agent_defaults;
    if !defaults.enable_thinking
        || defaults.reasoning_effort != "xhigh"
        || defaults.add_vision_id
        || defaults.temperature != 1.0
        || defaults.top_p != 0.95
        || defaults.top_k != 20
        || defaults.min_p != 0.0
        || defaults.presence_penalty != 0.0
        || defaults.repetition_penalty != 1.0
        || defaults.thinking_token_budget != 262_144
        || defaults.final_response_token_budget != 131_072
    {
        return fail(
            "backend agent defaults differ from Qwen3.8 xhigh thinking-mode policy".into(),
        );
    }
    let required_environment = [
        "HOME=/home/vllm",
        "VLLM_CACHE_ROOT=/home/vllm/.cache/vllm",
        "XDG_CACHE_HOME=/home/vllm/.cache/vllm/xdg-cache",
        "XDG_CONFIG_HOME=/home/vllm/.cache/vllm/xdg-config",
        "CUDA_CACHE_PATH=/home/vllm/.cache/vllm/cuda",
        "HF_HOME=/home/vllm/.cache/vllm/huggingface",
        "TRITON_HOME=/home/vllm/.cache/vllm/triton",
        "TRITON_CACHE_DIR=/home/vllm/.cache/vllm/triton/cache",
        "TORCHINDUCTOR_CACHE_DIR=/home/vllm/.cache/vllm/torchinductor",
        "FLASHINFER_WORKSPACE_BASE=/home/vllm/.cache/vllm/flashinfer",
        "PYTHONDONTWRITEBYTECODE=1",
        "HF_HUB_OFFLINE=1",
        "TRANSFORMERS_OFFLINE=1",
        "DO_NOT_TRACK=1",
        "VLLM_NO_USAGE_STATS=1",
        "VLLM_DEBUG_WORKSPACE=1",
        "VLLM_ENFORCE_STRICT_TOOL_CALLING=1",
        "VLLM_QWEN38_STRICT_IMAGE_CONTRACT=1",
        "VLLM_QWEN38_VISION_HEADROOM_BYTES=671088640",
        "VLLM_MAX_IMAGE_PIXELS=16777216",
        "GLOO_SOCKET_IFNAME=lo",
        "NCCL_SOCKET_IFNAME=lo",
    ];
    if lock
        .backend
        .environment
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        != required_environment
    {
        return fail(
            "backend environment differs from the strict non-root v18 runtime contract".into(),
        );
    }
    if lock.backend.model_repository != "unsloth/Qwen3.8-27B-NVFP4"
        || lock.backend.model_revision != "16b6615af3548b88e2d8e382457bc705b00479cf"
        || lock.backend.official_model_repository != "Qwen/Qwen3.8-27B"
        || lock.backend.official_model_revision != "1d4bf0f2ff6012fd82039f2fa52739d0dd7c60c0"
        || lock.backend.model_directory != "Qwen3.8-27B-NVFP4-Corrected"
        || lock.backend.model_correction != "restore-161-offset-rmsnorms-from-official-bf16-v1"
        || lock.backend.model_manifest != "model-corrected-16b6615a-norms-1d4bf0f2.sha256"
        || lock.backend.model_sha256
            != "5fd70b38b3708e47adc1e9e9ab90f5d688ec01177d0718fdd16678696fdb0988"
        || lock.backend.model_manifest_sha256
            != "3a86177c30b97035d27ad0cf516fc4c2ddb83701c4de4fc6adcb23c7c2531bfc"
    {
        return fail(
            "backend corrected-model identity differs from the sole deployable snapshot".into(),
        );
    }
    if !lock.backend.image_id.starts_with("sha256:") || lock.backend.image_id.len() != 71 {
        return fail("backend.image_id is not an exact Docker image ID".into());
    }
    if lock.backend.model_revision.len() != 40
        || lock.backend.official_model_revision.len() != 40
        || lock.backend.vllm_commit.len() != 40
        || !is_sha256(&lock.backend.model_sha256)
        || !is_sha256(&lock.backend.model_manifest_sha256)
        || !is_sha256(&lock.agent.qwen_code.source_archive_sha256)
        || !is_sha256(&lock.agent.qwen_code.patch_sha256)
        || !is_sha256(&lock.agent.qwen_code.source_patch_manifest_sha256)
    {
        return fail(
            "one or more source/model/patch commit or lowercase SHA256 pins are malformed".into(),
        );
    }
    if lock.backend.command.is_empty() {
        return fail("backend.command may not be empty".into());
    }
    // Only the isolation features the containers depend on are asserted.
    // Host software versions, binary hashes, and GPU identity are
    // deliberately not validated: they tie the deployment to one specific
    // computer without making it more correct anywhere. The Docker socket
    // path and group id are wiring configuration, not identity assertions.
    if lock.host.docker_security_options
        != [
            "name=apparmor",
            "name=seccomp,profile=builtin",
            "name=cgroupns",
        ]
        || lock.host.container_apparmor_profile != "docker-default"
    {
        return fail(
            "host container-isolation contract (apparmor/seccomp/cgroupns) is not satisfied"
                .into(),
        );
    }
    if !lock.host.docker_socket.starts_with('/') {
        return fail("host.docker_socket must be an absolute path".into());
    }
    validate_embedded_broker_policy(lock)?;
    Ok(())
}

fn validate_embedded_broker_policy(lock: &StackLock) -> ServiceResult<()> {
    let policy: BrokerPolicy = serde_json::from_str(BROKER_POLICY_JSON).map_err(|error| {
        ServiceError::Internal(format!(
            "compiled config/broker-policy-v1.json is malformed or does not match its strict schema: {error}"
        ))
    })?;

    let mut mismatches = Vec::new();
    macro_rules! same {
        ($name:literal, $policy_value:expr, $lock_value:expr) => {
            if $policy_value != $lock_value {
                mismatches.push($name);
            }
        };
    }

    same!("schema_version", policy.schema_version, lock.schema_version);
    same!("policy_id", policy.policy_id.as_str(), lock.broker.policy_id.as_str());
    same!("profile", policy.profile.as_str(), lock.profile.as_str());
    same!(
        "broker_container_name",
        policy.broker_container_name.as_str(),
        lock.broker.container_name.as_str()
    );
    same!(
        "broker.image_tag",
        policy.broker.image_tag.as_str(),
        lock.broker.image_tag.as_str()
    );
    same!(
        "broker.memory",
        policy.broker.memory.as_str(),
        lock.broker.memory.as_str()
    );
    same!(
        "broker.memory_swap",
        policy.broker.memory_swap.as_str(),
        lock.broker.memory_swap.as_str()
    );
    same!(
        "broker.pids_limit",
        policy.broker.pids_limit,
        lock.broker.pids_limit
    );
    same!("broker.uid", policy.broker.uid, 1000);
    same!("broker.gid", policy.broker.gid, lock.host.docker_socket_gid);
    same!(
        "broker.docker_socket",
        policy.broker.docker_socket.as_str(),
        lock.host.docker_socket.as_str()
    );
    same!(
        "broker_socket_path",
        policy.broker_socket_path.as_str(),
        lock.broker.socket_path.as_str()
    );
    same!(
        "runtime_root",
        policy.runtime_root.as_str(),
        lock.service.runtime_root.as_str()
    );
    same!(
        "state_dir",
        policy.state_dir.as_str(),
        lock.service.state_dir.as_str()
    );
    same!(
        "model_socket_dir",
        policy.model_socket_dir.as_str(),
        lock.relay.model_socket_dir.as_str()
    );
    same!(
        "service_container_name",
        policy.service_container_name.as_str(),
        lock.service.container_name.as_str()
    );
    same!(
        "backend_container_name",
        policy.backend_container_name.as_str(),
        lock.backend.container_name.as_str()
    );
    same!(
        "backend_cache_volume",
        policy.backend_cache_volume.as_str(),
        lock.backend.cache_volume.as_str()
    );
    same!(
        "backend_cache_mount",
        policy.backend_cache_mount.as_str(),
        lock.backend.cache_mount.as_str()
    );
    same!(
        "backend_cache_owner_mode",
        policy.backend_cache_owner_mode.as_str(),
        lock.backend.cache_owner_mode.as_str()
    );
    same!(
        "model_bridge_container_name",
        policy.model_bridge_container_name.as_str(),
        lock.relay.model_bridge_container.as_str()
    );
    same!(
        "model_ingress_container_name",
        policy.model_ingress_container_name.as_str(),
        lock.relay.model_ingress_container.as_str()
    );
    same!(
        "agent.image_tag",
        policy.agent.image_tag.as_str(),
        lock.agent.image_tag.as_str()
    );
    same!(
        "agent.image_id",
        policy.agent.image_id.as_str(),
        lock.agent.image_id.as_str()
    );
    same!(
        "agent.memory",
        policy.agent.memory.as_str(),
        lock.agent.memory.as_str()
    );
    same!(
        "agent.memory_swap",
        policy.agent.memory_swap.as_str(),
        lock.agent.memory_swap.as_str()
    );
    same!("agent.pids_limit", policy.agent.pids_limit, lock.agent.pids_limit);
    same!(
        "agent.tmpfs_tmp",
        policy.agent.tmpfs_tmp.as_str(),
        lock.agent.tmpfs_tmp.as_str()
    );
    same!(
        "agent.tmpfs_qwen_runtime",
        policy.agent.tmpfs_qwen_runtime.as_str(),
        lock.agent.tmpfs_qwen_runtime.as_str()
    );
    same!(
        "agent.ready_event_prefix",
        policy.agent.ready_event_prefix.as_str(),
        format!(
            "AGENT_READY model={} context={} network=loopback-only token_count=",
            lock.backend.served_model, lock.backend.max_model_len
        )
        .as_str()
    );
    same!(
        "agent.sandbox",
        policy.agent.sandbox.as_str(),
        lock.agent.agent_exec_sandbox.as_str()
    );
    same!(
        "relay.image_tag",
        policy.relay.image_tag.as_str(),
        lock.relay.image_tag.as_str()
    );
    same!(
        "relay.image_id",
        policy.relay.image_id.as_str(),
        lock.relay.image_id.as_str()
    );
    same!(
        "relay.sandbox",
        policy.relay.sandbox.as_str(),
        lock.relay.sandbox.as_str()
    );
    same!(
        "relay.memory",
        policy.relay.memory.as_str(),
        lock.relay.memory.as_str()
    );
    same!(
        "relay.memory_swap",
        policy.relay.memory_swap.as_str(),
        lock.relay.memory_swap.as_str()
    );
    same!("relay.pids_limit", policy.relay.pids_limit, lock.relay.pids_limit);
    same!("relay.role", policy.relay.role.as_str(), "agent-model");
    same!(
        "capture.image_tag",
        policy.capture.image_tag.as_str(),
        lock.capture.image_tag.as_str()
    );
    same!(
        "capture.image_id",
        policy.capture.image_id.as_str(),
        lock.capture.image_id.as_str()
    );
    same!(
        "capture.capture_id",
        policy.capture.capture_id.as_str(),
        lock.capture.capture_id.as_str()
    );
    same!(
        "capture.memory",
        policy.capture.memory.as_str(),
        lock.capture.memory.as_str()
    );
    same!(
        "capture.memory_swap",
        policy.capture.memory_swap.as_str(),
        lock.capture.memory_swap.as_str()
    );
    same!(
        "capture.pids_limit",
        policy.capture.pids_limit,
        lock.capture.pids_limit
    );
    same!(
        "capture.ready_event",
        policy.capture.ready_event.as_str(),
        format!(
            "CAPTURE_READY capture={} events=/streams/events.sock stderr=/streams/stderr.sock",
            lock.capture.capture_id
        )
        .as_str()
    );
    same!(
        "capture.complete_event_prefix",
        policy.capture.complete_event_prefix.as_str(),
        format!(
            "CAPTURE_COMPLETE capture={} events_bytes=",
            lock.capture.capture_id
        )
        .as_str()
    );

    if !mismatches.is_empty() {
        return Err(ServiceError::Internal(format!(
            "compiled broker policy disagrees with the stack lock for duplicated field(s): {}",
            mismatches.join(", ")
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{validate_lock, StackLock, STACK_LOCK_JSON};

    fn checked_in_lock() -> StackLock {
        serde_json::from_str(STACK_LOCK_JSON).expect("checked-in lock must match the strict schema")
    }

    fn assert_agent_policy_rejected(lock: StackLock) {
        let error = validate_lock(&lock).expect_err("weakened agent policy must fail closed");
        assert!(
            error
                .to_string()
                .contains("backend agent defaults differ from Qwen3.8 xhigh thinking-mode policy"),
            "unexpected validation error: {error}"
        );
    }

    #[test]
    fn checked_in_lock_is_the_exact_supported_policy() {
        validate_lock(&checked_in_lock()).expect("checked-in stack lock must be accepted");
    }

    #[test]
    fn limits_drift_is_rejected() {
        // Every numeric workspace/transport cap the shell harness reads from the
        // lock must equal the constant this binary compiled against. A single
        // field off by one must fail closed, so a client mirror can never
        // quietly admit or reject a task the service would not.
        let mutations: [fn(&mut StackLock); 7] = [
            |lock: &mut StackLock| lock.limits.max_prompt_bytes += 1,
            |lock: &mut StackLock| lock.limits.max_staged_bytes += 1,
            |lock: &mut StackLock| lock.limits.max_staged_files += 1,
            |lock: &mut StackLock| lock.limits.max_staged_entries += 1,
            |lock: &mut StackLock| lock.limits.max_archive_bytes += 1,
            |lock: &mut StackLock| lock.limits.max_session_turns += 1,
            |lock: &mut StackLock| lock.limits.max_session_turns_ceiling += 1,
        ];
        for mutate in mutations {
            let mut lock = checked_in_lock();
            mutate(&mut lock);
            let error = validate_lock(&lock).expect_err("limits drift must fail closed");
            assert!(
                error
                    .to_string()
                    .contains("limits in the lock disagree with the compiled constants"),
                "unexpected validation error: {error}"
            );
        }
    }

    #[test]
    fn weakened_thinking_policy_is_rejected() {
        let mut lock = checked_in_lock();
        lock.backend.agent_defaults.enable_thinking = false;
        assert_agent_policy_rejected(lock);

        let mut lock = checked_in_lock();
        lock.backend.agent_defaults.reasoning_effort = "medium".into();
        assert_agent_policy_rejected(lock);

        let mut lock = checked_in_lock();
        lock.backend.agent_defaults.add_vision_id = true;
        assert_agent_policy_rejected(lock);
    }

    #[test]
    fn sampling_or_phase_budget_drift_is_rejected() {
        let mut lock = checked_in_lock();
        lock.backend.agent_defaults.temperature = 0.7;
        assert_agent_policy_rejected(lock);

        let mut lock = checked_in_lock();
        lock.backend.agent_defaults.top_p = 0.8;
        assert_agent_policy_rejected(lock);

        let mut lock = checked_in_lock();
        lock.backend.agent_defaults.top_k = 40;
        assert_agent_policy_rejected(lock);

        let mut lock = checked_in_lock();
        lock.backend.agent_defaults.min_p = 0.01;
        assert_agent_policy_rejected(lock);

        let mut lock = checked_in_lock();
        lock.backend.agent_defaults.presence_penalty = 1.5;
        assert_agent_policy_rejected(lock);

        let mut lock = checked_in_lock();
        lock.backend.agent_defaults.repetition_penalty = 1.1;
        assert_agent_policy_rejected(lock);

        let mut lock = checked_in_lock();
        lock.backend.agent_defaults.thinking_token_budget = 262_143;
        assert_agent_policy_rejected(lock);

        let mut lock = checked_in_lock();
        lock.backend.agent_defaults.final_response_token_budget = 131_071;
        assert_agent_policy_rejected(lock);
    }

    #[test]
    fn source_patch_manifest_must_be_a_lowercase_sha256() {
        let mut lock = checked_in_lock();
        lock.agent.qwen_code.source_patch_manifest_sha256 = "g".repeat(64);
        let error = validate_lock(&lock).expect_err("non-hex manifest digest must fail closed");
        assert!(
            error
                .to_string()
                .contains("source/model/patch commit or lowercase SHA256 pins are malformed"),
            "unexpected validation error: {error}"
        );
    }

    #[test]
    fn reproducible_build_inputs_must_remain_exact() {
        let mut lock = checked_in_lock();
        lock.build.source_date_epoch += 1;
        let error = validate_lock(&lock).expect_err("source-date drift must fail closed");
        assert!(
            error.to_string().contains(
                "source-date epoch, Docker CLI archive, apt locks, or JKS normalizer hashes are not exact"
            ),
            "unexpected validation error: {error}"
        );

        let mut lock = checked_in_lock();
        lock.build.jks_normalizer_sha256 = "G".repeat(64);
        let error = validate_lock(&lock).expect_err("normalizer hash drift must fail closed");
        assert!(
            error.to_string().contains(
                "source-date epoch, Docker CLI archive, apt locks, or JKS normalizer hashes are not exact"
            ),
            "unexpected validation error: {error}"
        );
    }

    #[test]
    fn writable_backend_or_runtime_mount_drift_is_rejected() {
        let mutations: [fn(&mut StackLock); 6] = [
            |lock: &mut StackLock| lock.backend.user = "0:0".into(),
            |lock: &mut StackLock| lock.backend.rootfs_read_only = false,
            |lock: &mut StackLock| {
                lock.backend
                    .tmpfs
                    .insert("/root".into(), "rw,size=4g".into());
            },
            |lock: &mut StackLock| lock.backend.cache_volume = "unowned-cache".into(),
            |lock: &mut StackLock| lock.backend.cache_mount = "/root/.cache/vllm".into(),
            |lock: &mut StackLock| lock.backend.cache_owner_mode = "0:0:770".into(),
        ];
        for mutate in mutations {
            let mut lock = checked_in_lock();
            mutate(&mut lock);
            let error = validate_lock(&lock).expect_err("backend write drift must fail closed");
            assert!(
                error
                    .to_string()
                    .contains("backend immutable-root/tmpfs/cache-volume contract differs"),
                "unexpected validation error: {error}"
            );
        }
    }

    #[test]
    fn uncorrected_or_unpinned_model_identity_is_rejected() {
        let mutations: [fn(&mut StackLock); 3] = [
            |lock: &mut StackLock| {
                lock.backend.model_directory = "Qwen3.8-27B-NVFP4-Unsloth".into();
            },
            |lock: &mut StackLock| lock.backend.model_correction = "none".into(),
            |lock: &mut StackLock| {
                lock.backend.official_model_repository = "unknown/model".into();
            },
        ];
        for mutate in mutations {
            let mut lock = checked_in_lock();
            mutate(&mut lock);
            let error = validate_lock(&lock).expect_err("uncorrected model identity must fail");
            assert!(
                error
                    .to_string()
                    .contains("backend corrected-model identity differs"),
                "unexpected validation error: {error}"
            );
        }

        let mut lock = checked_in_lock();
        lock.backend.model_sha256 = "0".repeat(63);
        let error = validate_lock(&lock).expect_err("malformed model digest must fail");
        assert!(
            error
                .to_string()
                .contains("backend corrected-model identity differs"),
            "unexpected validation error: {error}"
        );
    }

    #[test]
    fn component_image_ids_must_match_the_typed_broker_policy_by_name() {
        let mut lock = checked_in_lock();
        std::mem::swap(&mut lock.relay.image_id, &mut lock.capture.image_id);
        let error = validate_lock(&lock).expect_err("transposed image IDs must fail closed");
        assert!(
            error.to_string().contains(
                "compiled broker policy disagrees with the stack lock for duplicated field(s): relay.image_id, capture.image_id"
            ),
            "unexpected validation error: {error}"
        );

        let mut lock = checked_in_lock();
        std::mem::swap(&mut lock.broker.image_id, &mut lock.relay.image_id);
        let error = validate_lock(&lock).expect_err("broker/relay image transposition must fail");
        assert!(
            error
                .to_string()
                .contains("compiled broker policy disagrees with the stack lock for duplicated field(s): relay.image_id"),
            "unexpected validation error: {error}"
        );
    }
}
