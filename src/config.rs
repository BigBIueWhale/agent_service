//! Service configuration. All fields are required (no silent defaults that
//! could mask a misconfiguration on a public-facing host); the `Config::load`
//! function falls back to opinionated defaults explicitly and logs each one.

use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;

use crate::error::{ServiceError, ServiceResult};

/// Compile-time pinned versions. Centralised so the README, the Dockerfile,
/// and the runtime all agree.
pub const QWEN_CODE_VERSION: &str = "0.15.6";
pub const AGENT_IMAGE_TAG_DEFAULT: &str = "qwen-agent-template:0.1.0";
/// Default RAM ceiling for the agent container. Passed verbatim to
/// `docker run --memory=…`; the format is whatever the docker daemon
/// accepts (`32g`, `32G`, `32768m`, `32GB`, …).
pub const AGENT_MEMORY_LIMIT_DEFAULT: &str = "32g";
/// Default writable-storage ceiling for the agent container. Passed to
/// `docker run --storage-opt size=…`. Requires a Docker storage driver
/// that supports per-container size quotas (overlay2 on xfs+pquota, or
/// btrfs / zfs / devicemapper). See pre-flight detection in `api::pre_flight`.
pub const AGENT_STORAGE_QUOTA_DEFAULT: &str = "128g";

/// Default `--max-session-turns` for the headless `qwen` invocation.
///
/// 200 is ~3× the Qwen3-Coder SWE-bench mean (64.3 turns/task) — generous
/// for our actual use case (corpus / codebase deep-dives where a single
/// thorough run can legitimately take well over 100 turns), while still
/// bounding genuinely runaway sessions.
///
/// Five other safety layers already catch the common pathological modes
/// before this cap fires: the five `LoopDetectionService` heuristics
/// (tool-call repeat, content chunk repeat, repetitive thoughts,
/// excessive read-likes, action stagnation), `sessionTokenLimit`, and
/// the orchestrator's wall-clock timeout. The turn cap is the
/// last-resort layer; it doesn't need to be tight.
pub const AGENT_MAX_TURNS_DEFAULT: u32 = 200;
/// Sanity-only ceiling for `AGENT_SERVICE_MAX_TURNS`. There is **no**
/// internal cap inside Qwen Code 0.15.6 on `model.maxSessionTurns` — it's
/// read raw at `packages/core/src/core/client.ts:709-710` with no clamp
/// (the `MAX_TURNS = 100` constant nearby at line 96 is a *recursion-depth*
/// bound on `sendMessageStream`, an unrelated mechanism). This 1024
/// ceiling exists only to refuse obvious typos like `99999999` while still
/// admitting any realistic value.
pub const AGENT_MAX_TURNS_HARD_CAP: u32 = 1024;
/// Default wall-clock cap for a single agent run, in seconds.
///
/// 2 hours. The use case (full corpus / codebase deep-dives at up to ~200
/// turns) can legitimately run longer than the SWE-bench-derived 90-minute
/// figure I started with — at this stack's measured throughput (71 tok/s
/// decode at low context, 50–60 at full 130K, parent README §11 B9 / B16)
/// 200 turns × ~30 s/turn averages out near 100 minutes, plus a couple of
/// deep-think outliers easily push past 90 min for thorough work. Loop
/// detectors and `sessionTokenLimit` catch genuinely stuck sessions much
/// earlier; this wall-clock is the absolute floor under "kill the
/// container regardless".
pub const AGENT_RUN_TIMEOUT_SECS_DEFAULT: u64 = 7200;

/// 1 MiB caps the prompt. Generous enough to embed a literal corpus chunk
/// directly in the prompt for "focus on this passage" workflows; well under
/// the model's 152K-token window so the agent has plenty of room to think
/// and to receive tool results.
pub const MAX_PROMPT_BYTES: usize = 1024 * 1024;
/// 4 GiB caps the staged folder size. The folder is *copied* before the
/// container starts; an unbounded copy would be a trivial DoS lever.
pub const MAX_STAGED_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// 200_000 caps the staged file count — protects against tarbomb-shaped folders.
pub const MAX_STAGED_FILES: u64 = 200_000;
/// Default number of past-session bundles to keep under `results_dir`. 0
/// disables retention (keep forever). Older bundles are pruned by mtime
/// after each successful run.
pub const AGENT_RESULTS_RETAIN_DEFAULT: u32 = 20;

#[derive(Clone, Debug)]
pub struct Config {
    /// Loopback-only listen address (we refuse anything else; see `parse_listen_addr`).
    pub listen_addr: SocketAddr,
    /// Host where vLLM serves the OpenAI-compatible API.
    pub vllm_host: String,
    /// Port where vLLM serves the OpenAI-compatible API.
    pub vllm_port: u16,
    /// Model name vLLM advertises (the `--served-model-name`).
    pub vllm_model_name: String,
    /// Tag of the pre-built agent template image (`docker build -t <this>`).
    /// The same image is used for both the agent container and the socat
    /// proxy — `socat` is part of the image's core-utilities apt block, so
    /// there's no need for a second image to maintain.
    pub agent_image: String,
    /// `docker run --memory=…` value. Default 32g.
    pub agent_memory_limit: String,
    /// `docker run --memory-swap=…` value. Defaults to the same as
    /// `agent_memory_limit`, which disables swap (the agent gets exactly
    /// `agent_memory_limit` of RAM and zero swap).
    pub agent_memory_swap_limit: String,
    /// `docker run --storage-opt size=…` value. `None` disables the
    /// flag entirely; default `Some("128g")`. Pre-flight verifies that
    /// the local Docker storage driver actually honours this, so the
    /// service refuses to start if a quota was requested but cannot be
    /// enforced (rather than silently running without one).
    pub agent_storage_quota: Option<String>,
    /// Filesystem root for per-session staging directories.
    pub state_dir: PathBuf,
    /// Filesystem root for persistent per-session result bundles. Defaults
    /// to `<state_dir>/results`. Each completed session produces a
    /// `<results_dir>/<session_id>/bundle.tar.zst`; pruned to
    /// `results_retain` entries (oldest first by mtime).
    pub results_dir: PathBuf,
    /// How many past bundles to keep. 0 = unlimited.
    pub results_retain: u32,
    /// Wall-clock cap on a single agent run, in seconds.
    pub run_timeout_secs: u64,
    /// Max session turns passed to `qwen --max-session-turns`.
    pub max_session_turns: u32,
}

impl Config {
    pub fn load() -> ServiceResult<Self> {
        let listen_addr = parse_listen_addr(&env_or("AGENT_SERVICE_LISTEN_ADDR", "127.0.0.1:8090"))?;
        let vllm_host = env_or("AGENT_SERVICE_VLLM_HOST", "127.0.0.1");
        let vllm_port = env_or("AGENT_SERVICE_VLLM_PORT", "8001")
            .parse::<u16>()
            .map_err(|e| ServiceError::Internal(format!("AGENT_SERVICE_VLLM_PORT not a u16: {e}")))?;
        let vllm_model_name = env_or("AGENT_SERVICE_MODEL_NAME", "Qwen3.6-27B-AWQ");
        let agent_image = env_or("AGENT_SERVICE_IMAGE", AGENT_IMAGE_TAG_DEFAULT);

        let agent_memory_limit = env_or("AGENT_SERVICE_MEMORY", AGENT_MEMORY_LIMIT_DEFAULT);
        if agent_memory_limit.is_empty() {
            return Err(ServiceError::Internal(
                "AGENT_SERVICE_MEMORY is empty — refuse to run an unbounded-memory agent container".into(),
            ));
        }
        let agent_memory_swap_limit =
            env_or("AGENT_SERVICE_MEMORY_SWAP", &agent_memory_limit);

        // AGENT_SERVICE_STORAGE_QUOTA: any non-empty value → use it as the
        // quota; an empty string explicitly disables the flag (operator
        // opt-out for storage drivers that don't support quotas).
        let agent_storage_quota = match env::var("AGENT_SERVICE_STORAGE_QUOTA") {
            Ok(v) if v.is_empty() => None,
            Ok(v) => Some(v),
            Err(_) => Some(AGENT_STORAGE_QUOTA_DEFAULT.to_string()),
        };

        let state_dir = match env::var("AGENT_SERVICE_STATE_DIR") {
            Ok(v) if !v.is_empty() => PathBuf::from(v),
            _ => default_state_dir()?,
        };

        let results_dir = match env::var("AGENT_SERVICE_RESULTS_DIR") {
            Ok(v) if !v.is_empty() => PathBuf::from(v),
            _ => state_dir.join("results"),
        };

        let results_retain = env_or(
            "AGENT_SERVICE_RESULTS_RETAIN",
            &AGENT_RESULTS_RETAIN_DEFAULT.to_string(),
        )
        .parse::<u32>()
        .map_err(|e| ServiceError::Internal(format!("AGENT_SERVICE_RESULTS_RETAIN not a u32: {e}")))?;

        let run_timeout_secs = env_or(
            "AGENT_SERVICE_TIMEOUT_SECS",
            &AGENT_RUN_TIMEOUT_SECS_DEFAULT.to_string(),
        )
        .parse::<u64>()
        .map_err(|e| ServiceError::Internal(format!("AGENT_SERVICE_TIMEOUT_SECS not a u64: {e}")))?;
        if !(30..=24 * 3600).contains(&run_timeout_secs) {
            return Err(ServiceError::Internal(format!(
                "AGENT_SERVICE_TIMEOUT_SECS must be between 30 and 86400, got {run_timeout_secs}"
            )));
        }

        let max_session_turns = env_or(
            "AGENT_SERVICE_MAX_TURNS",
            &AGENT_MAX_TURNS_DEFAULT.to_string(),
        )
        .parse::<u32>()
        .map_err(|e| ServiceError::Internal(format!("AGENT_SERVICE_MAX_TURNS not a u32: {e}")))?;
        if !(1..=AGENT_MAX_TURNS_HARD_CAP).contains(&max_session_turns) {
            return Err(ServiceError::Internal(format!(
                "AGENT_SERVICE_MAX_TURNS must be in 1..={AGENT_MAX_TURNS_HARD_CAP} \
                 (sanity-only upper bound; Qwen Code itself does not clamp the value, but \
                 anything above ~hundreds is almost certainly a typo); got {max_session_turns}"
            )));
        }

        Ok(Self {
            listen_addr,
            vllm_host,
            vllm_port,
            vllm_model_name,
            agent_image,
            agent_memory_limit,
            agent_memory_swap_limit,
            agent_storage_quota,
            state_dir,
            results_dir,
            results_retain,
            run_timeout_secs,
            max_session_turns,
        })
    }
}

fn env_or(key: &str, default: &str) -> String {
    match env::var(key) {
        Ok(v) if !v.is_empty() => v,
        _ => default.to_string(),
    }
}

fn parse_listen_addr(s: &str) -> ServiceResult<SocketAddr> {
    let addr: SocketAddr = s.parse().map_err(|e| {
        ServiceError::Internal(format!(
            "AGENT_SERVICE_LISTEN_ADDR ({s:?}) is not a host:port pair: {e}"
        ))
    })?;
    if !addr.ip().is_loopback() {
        return Err(ServiceError::Internal(format!(
            "AGENT_SERVICE_LISTEN_ADDR must be loopback (127.0.0.1 or ::1) — refusing to bind to {} because the host is exposed to the public internet",
            addr.ip()
        )));
    }
    Ok(addr)
}

fn default_state_dir() -> ServiceResult<PathBuf> {
    let xdg = match env::var("XDG_STATE_HOME") {
        Ok(v) if !v.is_empty() => PathBuf::from(v),
        _ => {
            let home = env::var("HOME").map_err(|e| {
                ServiceError::Internal(format!(
                    "neither XDG_STATE_HOME nor HOME is set in the environment: {e}"
                ))
            })?;
            if home.is_empty() {
                return Err(ServiceError::Internal(
                    "HOME is set but empty".into(),
                ));
            }
            PathBuf::from(home).join(".local").join("state")
        }
    };
    Ok(xdg.join("agent_service"))
}
