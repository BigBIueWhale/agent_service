//! Per-session lifecycle + the strict singleton manager.
//!
//! Workflow:
//!
//! 1. Acquire singleton (or fail with `Busy`).
//! 2. Stage the user's folder + prompt.txt.
//! 3. Build per-session isolated network + proxy.
//! 4. `docker run -d` the agent container with three bind-mounts
//!    (workspace, control, output) and ttyd published to `127.0.0.1:<ephemeral>`.
//! 5. Wait for ttyd's host port to come live, hand back the URL.
//! 6. `docker wait` the agent container.
//! 7. Parse `events.jsonl` → `AgentResult`.
//! 8. Tear down: stop & remove containers, remove network, remove session
//!    state directory. Clear the singleton.
//!
//! Failure at any step short-circuits, but always tears down whatever has
//! already been created — we never leak a docker network or container.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::bundle::{self, BundleStats};
use crate::config::Config;
use crate::docker_ops;
use crate::error::{ServiceError, ServiceResult};
use crate::network::{self, IsolatedNetwork, TTYD_CONTAINER_PORT};
use crate::result_parse::{self, AgentResult};
use crate::staging::{self, SessionPaths};
use crate::validation::ValidatedRequest;

/// Snapshot of the currently-running session (if any), exposed via
/// `GET /v1/agent/current`. Cloneable so we can `.clone()` it out of the
/// mutex without holding the lock.
#[derive(Clone, Debug, Serialize)]
pub struct RunningSession {
    pub session_id: String,
    pub ttyd_url: String,
    pub started_at_unix: u64,
    pub prompt_preview: String,
}

/// Concrete shape of the "started" event in the streaming NDJSON response.
#[derive(Clone, Debug, Serialize)]
pub struct StartedEvent {
    pub event: &'static str,
    pub session_id: String,
    pub ttyd_url: String,
    pub started_at_unix: u64,
    pub agent_image: String,
    pub model_name: String,
}

/// Concrete shape of the "finished" event in the streaming NDJSON response.
/// Every field is always present (required-field discipline). Bundle-related
/// fields are zero / empty-string only when bundling itself failed — never
/// "absent" — so a client has exactly one parser regardless of run outcome.
#[derive(Clone, Debug, Serialize)]
pub struct FinishedEvent {
    pub event: &'static str,
    pub session_id: String,
    pub finished_at_unix: u64,
    pub duration_wall_ms: u64,
    pub container_exit_code: i32,
    pub is_error: bool,
    pub response: String,
    pub agent_num_turns: u64,
    pub agent_duration_ms: u64,
    /// Absolute host filesystem path to `bundle.tar.zst` for this session.
    /// Empty string only if bundling failed (in which case
    /// `teardown_diagnostics` will explain why).
    pub bundle_archive_path: String,
    /// On-disk size of `bundle_archive_path` in bytes (post-zstd).
    pub bundle_compressed_bytes: u64,
    /// Sum of file sizes inside the bundle, pre-compression.
    pub bundle_uncompressed_bytes: u64,
    /// Total file count in the bundle (artifacts/ + sidecars).
    pub bundle_file_count: u64,
    /// File count under `artifacts/` specifically — i.e., what the agent
    /// wrote intentionally, separate from the events.jsonl forensics file.
    pub bundle_artifacts_file_count: u64,
    pub teardown_diagnostics: Vec<String>,
}

/// Strict-singleton session manager. Exactly one session may run at a time.
/// (Designed to grow into a small bounded pool when the host gains more GPUs
/// — change the `Mutex<Option<_>>` to a `Semaphore` and `Vec<RunningSession>`.)
pub struct SessionManager {
    cfg: Arc<Config>,
    /// `None` = idle. `Some(s)` = `s` is running (or being set up).
    state: Mutex<Option<RunningSession>>,
}

impl SessionManager {
    pub fn new(cfg: Arc<Config>) -> Self {
        Self {
            cfg,
            state: Mutex::new(None),
        }
    }

    pub async fn current(&self) -> Option<RunningSession> {
        self.state.lock().await.clone()
    }

    /// Run a full session synchronously from the caller's perspective; the
    /// two callbacks are invoked at the two natural milestones so the HTTP
    /// handler can stream NDJSON in real time. The caller's stream stays
    /// open for the entire duration of the agent run.
    pub async fn run<EmitS, EmitF>(
        &self,
        req: ValidatedRequest,
        emit_started: EmitS,
        emit_finished: EmitF,
    ) -> ServiceResult<()>
    where
        EmitS: FnOnce(StartedEvent) + Send,
        EmitF: FnOnce(FinishedEvent) + Send,
    {
        // ── singleton acquire ──────────────────────────────────────────
        let session_id = format!("s-{}", Uuid::new_v4().simple());
        {
            let mut guard = self.state.lock().await;
            if let Some(running) = &*guard {
                return Err(ServiceError::Busy {
                    running_session_id: running.session_id.clone(),
                });
            }
            // Reserve the slot immediately so a parallel request gets Busy
            // even while we're still doing setup. Fields will be filled in
            // properly once ttyd is reachable.
            *guard = Some(RunningSession {
                session_id: session_id.clone(),
                ttyd_url: String::new(),
                started_at_unix: now_unix(),
                prompt_preview: preview(&req.prompt),
            });
        }

        // From here on, EVERY error path must release the singleton.
        let result = self
            .run_inner(&session_id, req, emit_started, emit_finished)
            .await;

        // Release the singleton.
        {
            let mut guard = self.state.lock().await;
            *guard = None;
        }
        result
    }

    async fn run_inner<EmitS, EmitF>(
        &self,
        session_id: &str,
        req: ValidatedRequest,
        emit_started: EmitS,
        emit_finished: EmitF,
    ) -> ServiceResult<()>
    where
        EmitS: FnOnce(StartedEvent) + Send,
        EmitF: FnOnce(FinishedEvent) + Send,
    {
        let cfg = Arc::clone(&self.cfg);

        // ── stage ──────────────────────────────────────────────────────
        let paths = SessionPaths::new(&cfg.state_dir, session_id);
        paths.create_dirs()?;
        staging::copy_into_staged(&req.folder, &paths.staged)?;
        let _prompt_file = paths.write_prompt(&req.prompt)?;

        // ── network (outer proxy + internal net + inner proxy) ─────────
        let net = match IsolatedNetwork::create(&cfg, session_id, &paths.proxy_sock_dir).await {
            Ok(n) => n,
            Err(e) => {
                let _ = paths.remove_all();
                return Err(e);
            }
        };

        // ── agent container ────────────────────────────────────────────
        let agent_container_name = format!("agent-{session_id}");
        let session_label = format!("agent_service.session={session_id}");

        let workspace_arg = format!("{}:/workspace:rw", paths.staged.display());
        let artifacts_arg = format!("{}:/artifacts:rw", paths.artifacts.display());
        let control_arg = format!("{}:/run/agent:ro", paths.control.display());
        let output_arg = format!("{}:/output:rw", paths.output.display());
        let publish_arg = format!("127.0.0.1::{TTYD_CONTAINER_PORT}");
        let model_env = format!("OPENAI_MODEL={}", cfg.vllm_model_name);
        let base_url_env = format!("OPENAI_BASE_URL={}", net.agent_base_url());
        let max_turns_env = format!("AGENT_MAX_TURNS={}", cfg.max_session_turns);

        let mut agent_args: Vec<String> = vec![
            "--name".into(),
            agent_container_name.clone(),
            "--label".into(),
            session_label,
            "--network".into(),
            net.network_name.clone(),
            // DNS pointed at a non-listening loopback. The container's
            // resolv.conf becomes `nameserver 127.0.0.1` (with empty
            // search domains), so every DNS query — including those that
            // would otherwise hit Docker's embedded resolver at 127.0.0.11
            // and forward externally — fails immediately. The agent
            // reaches the proxy by IP literal (see `agent_base_url`); it
            // does not, and cannot, resolve any hostname.
            "--dns".into(),
            "127.0.0.1".into(),
            "--dns-search".into(),
            ".".into(),
            "-p".into(),
            publish_arg,
            "-v".into(),
            workspace_arg,
            "-v".into(),
            artifacts_arg,
            "-v".into(),
            control_arg,
            "-v".into(),
            output_arg,
            "-e".into(),
            base_url_env,
            "-e".into(),
            "OPENAI_API_KEY=dummy-not-used-but-required-by-client".into(),
            "-e".into(),
            model_env,
            "-e".into(),
            max_turns_env,
            "-e".into(),
            "QWEN_SANDBOX=false".into(),
            "-e".into(),
            "NO_COLOR=1".into(),
            "-e".into(),
            "QWEN_TELEMETRY_ENABLED=false".into(),
            "--user".into(),
            "1000:1000".into(),
            // No --gpus on purpose. No --privileged. No --cap-add other
            // than NET_BIND_SERVICE for forward-compat with low ttyd ports.
            "--cap-drop".into(),
            "ALL".into(),
            "--cap-add".into(),
            "NET_BIND_SERVICE".into(),
            "--security-opt".into(),
            "no-new-privileges:true".into(),
            // RAM ceiling. `--memory-swap` matched to `--memory` => no swap.
            "--memory".into(),
            cfg.agent_memory_limit.clone(),
            "--memory-swap".into(),
            cfg.agent_memory_swap_limit.clone(),
            "--pids-limit".into(),
            "4096".into(),
        ];

        // Per-container writable-storage quota (only enforced if the
        // operator's storage driver supports it; pre-flight has already
        // verified that, so we'd never reach here with an unsupported
        // value).
        if let Some(quota) = &cfg.agent_storage_quota {
            agent_args.push("--storage-opt".into());
            agent_args.push(format!("size={quota}"));
        }

        agent_args.push(cfg.agent_image.clone());

        if let Err(e) = docker_ops::run_detached(&agent_args, "agent_run").await {
            let _ = net.teardown().await;
            let _ = paths.remove_all();
            return Err(e);
        }

        // ── wait for ttyd ──────────────────────────────────────────────
        let host_port = match wait_for_ttyd(&agent_container_name).await {
            Ok(p) => p,
            Err(e) => {
                let logs = docker_ops::container_logs_tail(&agent_container_name, 200)
                    .await
                    .unwrap_or_else(|_| "<unavailable>".into());
                let _ = docker_ops::container_force_remove(&agent_container_name).await;
                let _ = net.teardown().await;
                let _ = paths.remove_all();
                return Err(ServiceError::DockerCommand(format!(
                    "{e}; recent agent container logs:\n{logs}"
                )));
            }
        };

        let ttyd_url = format!("http://127.0.0.1:{host_port}/");
        let started_at_unix = now_unix();

        // Update singleton state with the resolved URL.
        {
            let mut guard = self.state.lock().await;
            if let Some(rs) = guard.as_mut() {
                rs.ttyd_url = ttyd_url.clone();
                rs.started_at_unix = started_at_unix;
            }
        }

        emit_started(StartedEvent {
            event: "started",
            session_id: session_id.to_string(),
            ttyd_url: ttyd_url.clone(),
            started_at_unix,
            agent_image: cfg.agent_image.clone(),
            model_name: cfg.vllm_model_name.clone(),
        });

        // ── wait for agent to finish ───────────────────────────────────
        let wall_start = std::time::Instant::now();
        let timeout_dur = Duration::from_secs(cfg.run_timeout_secs);
        let exit_result = docker_ops::container_wait(&agent_container_name, timeout_dur).await;

        let (container_exit_code, agent_result, mut teardown_diags) = match exit_result {
            Ok(code) => {
                let parsed = result_parse::parse_events_jsonl(&paths.events_jsonl());
                let (is_error, response, num_turns, duration_ms) = match parsed {
                    Ok(r) => (r.is_error, r.response, r.num_turns, r.duration_ms),
                    Err(e) => (
                        true,
                        format!(
                            "container exited with code {code} but result parsing failed: {e}"
                        ),
                        0,
                        0,
                    ),
                };
                let mut diags = Vec::new();
                if let Err(e) = docker_ops::container_force_remove(&agent_container_name).await {
                    diags.push(format!("agent rm: {e}"));
                }
                diags.extend(net.teardown().await);
                let r = AgentResult {
                    is_error,
                    response,
                    num_turns,
                    duration_ms,
                };
                (code, r, diags)
            }
            Err(ServiceError::Timeout(msg)) => {
                let mut diags = vec![format!("agent run hit wall-clock timeout: {msg}")];
                if let Err(e) = docker_ops::container_stop(&agent_container_name, 5).await {
                    diags.push(format!("agent stop: {e}"));
                }
                if let Err(e) = docker_ops::container_force_remove(&agent_container_name).await {
                    diags.push(format!("agent rm: {e}"));
                }
                diags.extend(net.teardown().await);
                let r = AgentResult {
                    is_error: true,
                    response: format!(
                        "agent run exceeded the wall-clock timeout of {}s and was killed",
                        cfg.run_timeout_secs
                    ),
                    num_turns: 0,
                    duration_ms: 0,
                };
                (-1, r, diags)
            }
            Err(other) => {
                let mut diags = vec![format!("docker wait failed: {other}")];
                if let Err(e) = docker_ops::container_force_remove(&agent_container_name).await {
                    diags.push(format!("agent rm: {e}"));
                }
                diags.extend(net.teardown().await);
                // Same-branch state-dir cleanup as the success / timeout
                // branches; we are about to bail out, so do it now while
                // we still have `paths`.
                let _ = paths.remove_all();
                return Err(ServiceError::DockerCommand(format!(
                    "session {session_id}: container_wait error: {other}; teardown diagnostics: {}",
                    diags.join("; ")
                )));
            }
        };

        // ── bundle (host-side) ─────────────────────────────────────────
        let archive_path = cfg
            .results_dir
            .join(session_id)
            .join("bundle.tar.zst");
        let bundle_outcome: Option<BundleStats> =
            match bundle::create_bundle(&paths.root, &archive_path).await {
                Ok(stats) => Some(stats),
                Err(e) => {
                    teardown_diags.push(format!("bundle creation failed: {e}"));
                    None
                }
            };
        let prune_diags =
            bundle::prune_results(&cfg.results_dir, cfg.results_retain);
        teardown_diags.extend(prune_diags);

        let (
            bundle_archive_path,
            bundle_compressed_bytes,
            bundle_uncompressed_bytes,
            bundle_file_count,
            bundle_artifacts_file_count,
        ) = match bundle_outcome {
            Some(s) => (
                s.archive_path.to_string_lossy().into_owned(),
                s.compressed_bytes,
                s.uncompressed_bytes,
                s.file_count,
                s.artifacts_file_count,
            ),
            None => (String::new(), 0, 0, 0, 0),
        };

        let finished_at_unix = now_unix();
        let wall_dur_ms = wall_start.elapsed().as_millis() as u64;

        emit_finished(FinishedEvent {
            event: "finished",
            session_id: session_id.to_string(),
            finished_at_unix,
            duration_wall_ms: wall_dur_ms,
            container_exit_code,
            is_error: agent_result.is_error,
            response: agent_result.response,
            agent_num_turns: agent_result.num_turns,
            agent_duration_ms: agent_result.duration_ms,
            bundle_archive_path,
            bundle_compressed_bytes,
            bundle_uncompressed_bytes,
            bundle_file_count,
            bundle_artifacts_file_count,
            teardown_diagnostics: teardown_diags,
        });

        // Best-effort state-dir removal AFTER bundling and AFTER emitting
        // the finished event. Bundling reads from the staging dir, so this
        // sequencing is load-bearing.
        let _ = paths.remove_all();

        Ok(())
    }
}

async fn wait_for_ttyd(container_name: &str) -> ServiceResult<u16> {
    // Poll docker for the ephemeral host port that 7681/tcp got mapped to.
    // Right after `docker run -d` returns, the port mapping is set on the
    // container's metadata immediately, but ttyd may take ~1s to bind.
    let port_deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut host_port: Option<u16> = None;
    while std::time::Instant::now() < port_deadline {
        match docker_ops::container_published_port(container_name, TTYD_CONTAINER_PORT).await {
            Ok(p) => {
                host_port = Some(p);
                break;
            }
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
    let Some(host_port) = host_port else {
        return Err(ServiceError::DockerCommand(format!(
            "agent container did not publish ttyd's container port {TTYD_CONTAINER_PORT}/tcp on 127.0.0.1 within 5s"
        )));
    };
    docker_ops::wait_tcp_ready(host_port, Duration::from_secs(20)).await?;
    Ok(host_port)
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn preview(s: &str) -> String {
    let truncated: String = s.chars().take(140).collect();
    if truncated.chars().count() < s.chars().count() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

// Re-export `sweep_orphans` so the API layer can call it at boot without
// depending on `network::` directly — the session module is the natural
// boundary for the singleton-related surface.
pub use network::sweep_orphans;
