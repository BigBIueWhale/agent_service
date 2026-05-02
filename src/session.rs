//! Per-session lifecycle: a single async fn (`run_one`) drives one session
//! end-to-end and always returns a terminal `SessionBody`. It observes a
//! `CancellationToken` at every long wait so that `manager.cancel(id)` and
//! the parent `manager.shutdown()` can interrupt mid-session.
//!
//! Workflow:
//!
//! 1. Stage host filesystem (`<state_dir>/sessions/<id>/`).
//! 2. Build the per-session isolated network + inner proxy.
//! 3. `docker run -d` the agent container (no `-p`; ttyd is reached via the
//!    sidecar created in step 5).
//! 4. Verify the agent has no default route (the gateway-isolated bridge
//!    promise must hold from iteration 1).
//! 5. Wait for ttyd to bind inside the agent, attach the ttyd-publishing
//!    sidecar, and signal the runtime layer via `ttyd_tx` that the session
//!    is now reachable. Past this point, `session_id` is observable via the
//!    HTTP surface and cancel/shutdown can be triggered against it.
//! 6. `docker wait` the agent container under cancel-awareness:
//!    - normal completion: parse events.jsonl, build a `Completed` body.
//!    - cancellation: issue `docker stop`, wait for exit, build a
//!      `Cancelled` body. Whatever artifacts the agent already wrote are
//!      preserved in the bundle.
//!    - wall-clock timeout: same as cancellation but `Completed` with
//!      `is_process_error=true`.
//! 7. Write `output/response.txt` so the answer text is durable on disk.
//! 8. Bundle (best-effort — failures end up as `teardown_diagnostics`).
//! 9. Tear down docker objects + remove the state-dir tree.
//!
//! Failure at any step BEFORE ttyd-up: clean up everything that was created
//! up to that point, send an `Err` over `ttyd_tx`, and return a terminal
//! body with `is_process_error=true`. Failure AFTER ttyd-up follows the same
//! cleanup path but the runtime layer has already returned the running
//! body to the HTTP caller, so the failure surfaces only via a subsequent
//! `get` / `list` poll.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::bundle;
use crate::config::Config;
use crate::docker_ops;
use crate::error::{ServiceError, ServiceResult};
use crate::network::{self, IsolatedNetwork, TTYD_CONTAINER_PORT};
use crate::result_parse;
use crate::runtime::{RunningSnapshot, SessionBody, SessionStatus};
use crate::staging::{self, SessionPaths};
use crate::validation::ValidatedRequest;

/// Drive one session to a terminal state. Always returns a `SessionBody`
/// with `status` ∈ {`Completed`, `Cancelled`}; setup errors before ttyd-up
/// produce a `Completed` body with `is_process_error=true` (the session "ran" in
/// the sense that we attempted to). Cancellation observed before any agent
/// turn ran also produces a `Cancelled` body — the user explicitly stopped
/// it, so the lifecycle is honest.
///
/// The `ttyd_tx` oneshot is sent EXACTLY ONCE: either with `Ok(snapshot)`
/// the moment the sidecar is up, or with `Err` if any setup step failed
/// before that point. `submit()` in the runtime layer awaits this signal.
pub async fn run_one(
    cfg: &Config,
    session_id: &str,
    req: ValidatedRequest,
    cancel: CancellationToken,
    ttyd_tx: oneshot::Sender<ServiceResult<RunningSnapshot>>,
    prompt_preview: String,
) -> SessionBody {
    let wall_start = std::time::Instant::now();
    let started_setup_at_unix = now_unix();

    // ── stage ──────────────────────────────────────────────────────────
    let paths = SessionPaths::new(&cfg.state_dir, session_id);
    if let Err(e) = paths.create_dirs() {
        return setup_failure(
            ttyd_tx,
            session_id,
            &prompt_preview,
            started_setup_at_unix,
            wall_start,
            e,
            Vec::new(),
        );
    }
    if let Err(e) = staging::copy_into_staged(&req.folder, &paths.staged) {
        let diags = paths.remove_all();
        return setup_failure(
            ttyd_tx,
            session_id,
            &prompt_preview,
            started_setup_at_unix,
            wall_start,
            e,
            diags,
        );
    }
    if let Err(e) = paths.write_prompt(&req.prompt) {
        let diags = paths.remove_all();
        return setup_failure(
            ttyd_tx,
            session_id,
            &prompt_preview,
            started_setup_at_unix,
            wall_start,
            e,
            diags,
        );
    }

    // Cancel-during-staging: tear down what we created and exit. The
    // cancel observation is best-effort here — staging is sub-second
    // typically — but we honour it for correctness.
    if cancel.is_cancelled() {
        let diags = paths.remove_all();
        return cancellation_terminal(
            session_id,
            &prompt_preview,
            started_setup_at_unix,
            wall_start,
            "session cancelled before docker setup began",
            diags,
        );
    }

    // ── network + inner proxy ──────────────────────────────────────────
    let mut net = match IsolatedNetwork::create(cfg, session_id, &paths.proxy_sock_dir).await {
        Ok(n) => n,
        Err(e) => {
            let mut diags = paths.remove_all();
            // No docker objects yet on this branch — IsolatedNetwork::create
            // is responsible for cleaning up its own partials. We just
            // surface the error.
            diags.push(format!("network setup: {e}"));
            return setup_failure(
                ttyd_tx,
                session_id,
                &prompt_preview,
                started_setup_at_unix,
                wall_start,
                e,
                diags,
            );
        }
    };

    if cancel.is_cancelled() {
        let mut diags = net.teardown().await;
        diags.extend(paths.remove_all());
        return cancellation_terminal(
            session_id,
            &prompt_preview,
            started_setup_at_unix,
            wall_start,
            "session cancelled after network setup, before agent container start",
            diags,
        );
    }

    // ── agent container ────────────────────────────────────────────────
    let agent_container_name = format!("agent-{session_id}");
    let session_label = format!("agent_service.session={session_id}");

    let workspace_arg = format!("{}:/workspace:rw", paths.staged.display());
    let artifacts_arg = format!("{}:/artifacts:rw", paths.artifacts.display());
    let control_arg = format!("{}:/run/agent:ro", paths.control.display());
    let output_arg = format!("{}:/output:rw", paths.output.display());
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
        // DNS pointed at a non-listening loopback — see the original
        // design comment in this module's history; the agent reaches the
        // proxy by IP literal (`agent_base_url`) and resolves no hostname.
        "--dns".into(),
        "127.0.0.1".into(),
        "--dns-search".into(),
        ".".into(),
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
        "--cap-drop".into(),
        "ALL".into(),
        "--cap-add".into(),
        "NET_BIND_SERVICE".into(),
        "--security-opt".into(),
        "no-new-privileges:true".into(),
        "--memory".into(),
        cfg.agent_memory_limit.clone(),
        "--memory-swap".into(),
        cfg.agent_memory_swap_limit.clone(),
        "--pids-limit".into(),
        "4096".into(),
    ];

    if let Some(quota) = &cfg.agent_storage_quota {
        agent_args.push("--storage-opt".into());
        agent_args.push(format!("size={quota}"));
    }
    agent_args.push(cfg.agent_image.clone());

    if let Err(e) = docker_ops::run_detached(&agent_args, "agent_run").await {
        let mut diags = net.teardown().await;
        diags.extend(paths.remove_all());
        return setup_failure(
            ttyd_tx,
            session_id,
            &prompt_preview,
            started_setup_at_unix,
            wall_start,
            e,
            diags,
        );
    }

    // ── post-create isolation assertion ────────────────────────────────
    if let Err(e) = docker_ops::verify_no_default_route(&agent_container_name).await {
        let mut diags = Vec::new();
        let logs = match docker_ops::container_logs_tail(&agent_container_name, 50).await {
            Ok(s) => s,
            Err(le) => {
                diags.push(format!(
                    "container_logs_tail({agent_container_name}, 50) for diagnostic context failed: {le}"
                ));
                "<unavailable; see teardown_diagnostics for the cause>".to_string()
            }
        };
        if let Err(re) = docker_ops::container_force_remove(&agent_container_name).await {
            diags.push(format!(
                "container_force_remove({agent_container_name}) during verify_no_default_route cleanup failed: {re}"
            ));
        }
        diags.extend(net.teardown().await);
        diags.extend(paths.remove_all());
        let wrapped = ServiceError::Internal(format!(
            "{e}; recent agent container logs:\n{logs}"
        ));
        return setup_failure(
            ttyd_tx,
            session_id,
            &prompt_preview,
            started_setup_at_unix,
            wall_start,
            wrapped,
            diags,
        );
    }

    // ── wait for ttyd + bring up the sidecar ───────────────────────────
    let host_port = match wait_and_attach_ttyd(&mut net, cfg, session_id, &agent_container_name)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            let mut diags = Vec::new();
            let logs = match docker_ops::container_logs_tail(&agent_container_name, 200).await {
                Ok(s) => s,
                Err(le) => {
                    diags.push(format!(
                        "container_logs_tail({agent_container_name}, 200) for diagnostic context failed: {le}"
                    ));
                    "<unavailable; see teardown_diagnostics for the cause>".to_string()
                }
            };
            if let Err(re) = docker_ops::container_force_remove(&agent_container_name).await {
                diags.push(format!(
                    "container_force_remove({agent_container_name}) during wait_and_attach_ttyd cleanup failed: {re}"
                ));
            }
            diags.extend(net.teardown().await);
            diags.extend(paths.remove_all());
            let wrapped = ServiceError::DockerCommand(format!(
                "{e}; recent agent container logs:\n{logs}"
            ));
            return setup_failure(
                ttyd_tx,
                session_id,
                &prompt_preview,
                started_setup_at_unix,
                wall_start,
                wrapped,
                diags,
            );
        }
    };

    let ttyd_url = format!("http://127.0.0.1:{host_port}/");
    let started_at_unix = now_unix();

    // ── signal "ttyd is up" ────────────────────────────────────────────
    let snapshot = RunningSnapshot {
        session_id: session_id.to_string(),
        started_at_unix,
        ttyd_url: ttyd_url.clone(),
        prompt_preview: prompt_preview.clone(),
    };
    if ttyd_tx.send(Ok(snapshot.clone())).is_err() {
        // The receiver was dropped — submit() unwound before we could hand
        // back the readiness signal. There is no in-process caller to
        // surface this to (the run task is now an orphan), so we propagate
        // every cleanup-step error into the terminal record's
        // teardown_diagnostics AND emit a tracing::error so the operator
        // sees this exceptional path even if the terminal record is later
        // DELETE'd.
        let mut diags = Vec::new();
        if let Err(re) = docker_ops::container_force_remove(&agent_container_name).await {
            diags.push(format!(
                "container_force_remove({agent_container_name}) during ttyd_tx-receiver-dropped cleanup failed: {re}"
            ));
        }
        diags.extend(net.teardown().await);
        diags.extend(paths.remove_all());
        diags.push(
            "ttyd_tx receiver was dropped before run_one could signal readiness — submit() unwound. \
             No HTTP caller will see this; the terminal record is the only durable trace."
                .into(),
        );
        tracing::error!(
            session_id = %session_id,
            "ttyd_tx receiver dropped before readiness signal — submit() unwound. \
             Run task continued tearing down. teardown_diagnostics on the terminal record \
             have the per-step details."
        );
        return early_unwind_terminal(
            session_id,
            &prompt_preview,
            started_at_unix,
            wall_start,
            diags,
        );
    }

    // ── wait for agent (cancellable) ───────────────────────────────────
    let timeout_dur = Duration::from_secs(cfg.run_timeout_secs);
    let wait_outcome = cancellable_container_wait(&agent_container_name, timeout_dur, &cancel)
        .await;

    // Whatever happened (normal exit, cancel, timeout), the container is
    // either gone or about to be force-removed below. Parse events.jsonl
    // best-effort — partial output is still useful to the operator.
    let parsed = result_parse::parse_events_jsonl(&paths.events_jsonl());
    let agent = match parsed {
        Ok(r) => r,
        Err(e) => result_parse::AgentResult {
            is_error: true,
            response: format!(
                "agent run did not produce a parseable result: {e}; \
                 the events.jsonl path was {} and the agent container exited with code {:?}",
                paths.events_jsonl().display(),
                wait_outcome.exit_code,
            ),
            duration_ms: 0,
        },
    };

    // ── teardown docker side ───────────────────────────────────────────
    // The full conversation history (every turn, every tool call/result,
    // and the final `type:"result"` event from which `agent.response`
    // was parsed) is already on disk at <session>/output/events.jsonl,
    // and the parsed `response` string is durable in the runtime layer's
    // <results_dir>/<id>/finished.json. No separate response sidecar.
    let mut teardown_diags = wait_outcome.diagnostics;
    if let Err(e) = docker_ops::container_force_remove(&agent_container_name).await {
        teardown_diags.push(format!("agent rm: {e}"));
    }
    teardown_diags.extend(net.teardown().await);

    // ── bundle (host-side) ─────────────────────────────────────────────
    let archive_path = cfg
        .results_dir
        .join(session_id)
        .join("bundle.tar.zst");
    let bundle_outcome = match bundle::create_bundle(&paths.root, &archive_path).await {
        Ok(stats) => Some(stats),
        Err(e) => {
            teardown_diags.push(format!("bundle creation failed: {e}"));
            None
        }
    };

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

    // ── snapshot frozen progress + qwen exit code BEFORE removing staging.
    // After `paths.remove_all()` events.jsonl and qwen-exit-code are gone
    // from the staging tree (only available inside the bundle from now on).
    let frozen_progress = crate::runtime::read_running_progress(&paths.events_jsonl());
    let agent_exit_code = crate::runtime::read_agent_exit_code(&paths.root);

    // ── state-dir cleanup ──────────────────────────────────────────────
    teardown_diags.extend(paths.remove_all());

    let finished_at_unix = now_unix();
    let wall_dur_ms = wall_start.elapsed().as_millis() as u64;

    let (status, is_error_final, response_final) = match wait_outcome.kind {
        WaitKind::Normal => (SessionStatus::Completed, agent.is_error, agent.response),
        WaitKind::Cancelled => (
            SessionStatus::Cancelled,
            true,
            if agent.response.is_empty() {
                "session was cancelled by request".to_string()
            } else {
                format!(
                    "session was cancelled by request; the agent's last partial response was: {}",
                    agent.response
                )
            },
        ),
        WaitKind::Timeout => (
            SessionStatus::Completed,
            true,
            format!(
                "agent run exceeded the wall-clock timeout of {}s and was killed; \
                 the agent's last partial response (if any) was: {}",
                cfg.run_timeout_secs,
                if agent.response.is_empty() {
                    "<none>"
                } else {
                    &agent.response
                }
            ),
        ),
        WaitKind::WaitFailed => (
            SessionStatus::Completed,
            true,
            format!(
                "docker wait reported an error before the container exited cleanly; \
                 the agent's last partial response (if any) was: {}",
                if agent.response.is_empty() {
                    "<none>"
                } else {
                    &agent.response
                }
            ),
        ),
    };

    SessionBody {
        session_id: session_id.to_string(),
        status,
        started_at_unix,
        ttyd_url,
        prompt_preview,
        num_turns: frozen_progress.0,
        last_event_at_unix: frozen_progress.1,
        finished_at_unix,
        duration_wall_ms: wall_dur_ms,
        container_exit_code: wait_outcome.exit_code.unwrap_or(-1),
        agent_exit_code,
        is_process_error: is_error_final,
        response: response_final,
        agent_duration_ms: agent.duration_ms,
        bundle_archive_path,
        bundle_compressed_bytes,
        bundle_uncompressed_bytes,
        bundle_file_count,
        bundle_artifacts_file_count,
        teardown_diagnostics: teardown_diags,
    }
}

// ── cancellable wait ───────────────────────────────────────────────────────

/// Outcome of `cancellable_container_wait`. The shape expresses every
/// distinguishable case the run loop needs: did we exit normally, was the
/// session cancelled, did the wall-clock fire, did docker itself error
/// during `docker wait`?
#[derive(Debug)]
struct WaitOutcome {
    kind: WaitKind,
    exit_code: Option<i32>,
    diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WaitKind {
    Normal,
    Cancelled,
    Timeout,
    WaitFailed,
}

/// Race `docker wait` against the cancel token. On cancel, issue
/// `docker stop` (graceful, then SIGKILL after 10s) and re-await for
/// the actual exit code; the container exits within seconds of stop.
async fn cancellable_container_wait(
    name: &str,
    timeout: Duration,
    cancel: &CancellationToken,
) -> WaitOutcome {
    tokio::select! {
        result = docker_ops::container_wait(name, timeout) => {
            match result {
                Ok(code) => WaitOutcome {
                    kind: WaitKind::Normal,
                    exit_code: Some(code),
                    diagnostics: Vec::new(),
                },
                Err(ServiceError::Timeout(msg)) => {
                    let mut diags = vec![format!("agent run hit wall-clock timeout: {msg}")];
                    // Stop with a short grace period; the runtime cap has
                    // already expired so we don't grant another long one.
                    if let Err(e) = docker_ops::container_stop(name, 5).await {
                        diags.push(format!("agent stop after timeout: {e}"));
                    }
                    // Final code, with a small wait for the container to
                    // actually exit. If even this fails, the container
                    // will be force-removed by the caller.
                    let exit_code = match docker_ops::container_wait(name, Duration::from_secs(15))
                        .await
                    {
                        Ok(c) => Some(c),
                        Err(e) => {
                            diags.push(format!("docker wait after timeout-stop: {e}"));
                            None
                        }
                    };
                    WaitOutcome {
                        kind: WaitKind::Timeout,
                        exit_code,
                        diagnostics: diags,
                    }
                }
                Err(other) => {
                    let mut diags = vec![format!("docker wait failed: {other}")];
                    // Fall through to `docker stop` so the container is at
                    // least brought to a known state before the caller
                    // force-removes it.
                    if let Err(e) = docker_ops::container_stop(name, 5).await {
                        diags.push(format!("agent stop after wait error: {e}"));
                    }
                    WaitOutcome {
                        kind: WaitKind::WaitFailed,
                        exit_code: None,
                        diagnostics: diags,
                    }
                }
            }
        }
        _ = cancel.cancelled() => {
            let mut diags = vec![
                "cancellation observed mid-run; issuing `docker stop` (10 s grace, then SIGKILL)"
                    .into(),
            ];
            if let Err(e) = docker_ops::container_stop(name, 10).await {
                diags.push(format!("agent stop on cancel: {e}"));
            }
            // Re-await for the actual exit code. After `docker stop`, the
            // container exits within seconds.
            let exit_code = match docker_ops::container_wait(name, Duration::from_secs(30))
                .await
            {
                Ok(c) => Some(c),
                Err(e) => {
                    diags.push(format!("docker wait after cancel-stop: {e}"));
                    None
                }
            };
            WaitOutcome {
                kind: WaitKind::Cancelled,
                exit_code,
                diagnostics: diags,
            }
        }
    }
}

// ── early-failure body builders ────────────────────────────────────────────

/// Build a terminal `Completed`-with-`is_process_error` body and ALSO send the
/// `Err` over `ttyd_tx`. Used for failures that occur before ttyd-up.
fn setup_failure(
    ttyd_tx: oneshot::Sender<ServiceResult<RunningSnapshot>>,
    session_id: &str,
    prompt_preview: &str,
    started_at_unix: u64,
    wall_start: std::time::Instant,
    err: ServiceError,
    mut teardown_diagnostics: Vec<String>,
) -> SessionBody {
    if ttyd_tx.send(Err(err.clone())).is_err() {
        // The receiver was dropped. There is no caller to surface `err`
        // to. Propagate as visibly as we can: a tracing::error AND
        // teardown_diagnostics on the terminal record. The terminal body
        // we return below also embeds `err` in its `response` field, so
        // a future GET sees the same error.
        teardown_diagnostics.push(format!(
            "ttyd_tx.send(Err) failed: receiver was dropped before submit() could observe \
             the setup error. The error itself: {err}"
        ));
        tracing::error!(
            session_id = %session_id,
            error = %err,
            "setup_failure: ttyd_tx receiver dropped — the HTTP caller will not see this \
             error. It is recorded in finished.json's response and teardown_diagnostics."
        );
    }
    let response = format!(
        "session setup failed before ttyd became reachable: {err}; \
         this terminal record was generated by the run task itself — \
         the HTTP submitter received the same error synchronously (unless the receiver was \
         dropped, in which case teardown_diagnostics records that explicitly)"
    );
    SessionBody {
        session_id: session_id.to_string(),
        status: SessionStatus::Completed,
        started_at_unix,
        ttyd_url: String::new(),
        prompt_preview: prompt_preview.to_string(),
        num_turns: 0,
        last_event_at_unix: 0,
        finished_at_unix: now_unix(),
        duration_wall_ms: wall_start.elapsed().as_millis() as u64,
        container_exit_code: -1,
        agent_exit_code: -1,
        is_process_error: true,
        response,
        agent_duration_ms: 0,
        bundle_archive_path: String::new(),
        bundle_compressed_bytes: 0,
        bundle_uncompressed_bytes: 0,
        bundle_file_count: 0,
        bundle_artifacts_file_count: 0,
        teardown_diagnostics,
    }
}

/// Cancellation observed BEFORE ttyd was up — i.e., between
/// `manager.submit()` returning a session_id and the session actually
/// becoming reachable. Rare in practice. Returns a `Cancelled` body and
/// signals an error via ttyd_tx so the submit-side caller does not hang
/// (we can't return a successful "running" body for a session that was
/// cancelled before it ever ran).
#[allow(dead_code)]
fn cancellation_terminal(
    session_id: &str,
    prompt_preview: &str,
    started_at_unix: u64,
    wall_start: std::time::Instant,
    detail: &str,
    teardown_diagnostics: Vec<String>,
) -> SessionBody {
    SessionBody {
        session_id: session_id.to_string(),
        status: SessionStatus::Cancelled,
        started_at_unix,
        ttyd_url: String::new(),
        prompt_preview: prompt_preview.to_string(),
        num_turns: 0,
        last_event_at_unix: 0,
        finished_at_unix: now_unix(),
        duration_wall_ms: wall_start.elapsed().as_millis() as u64,
        container_exit_code: -1,
        agent_exit_code: -1,
        is_process_error: true,
        response: detail.to_string(),
        agent_duration_ms: 0,
        bundle_archive_path: String::new(),
        bundle_compressed_bytes: 0,
        bundle_uncompressed_bytes: 0,
        bundle_file_count: 0,
        bundle_artifacts_file_count: 0,
        teardown_diagnostics,
    }
}

/// `ttyd_tx` receiver was dropped before we could send Ok — submit()
/// unwound. Equivalent semantically to a cancellation, but with a
/// distinct response message so the operator can tell the two apart.
fn early_unwind_terminal(
    session_id: &str,
    prompt_preview: &str,
    started_at_unix: u64,
    wall_start: std::time::Instant,
    teardown_diagnostics: Vec<String>,
) -> SessionBody {
    SessionBody {
        session_id: session_id.to_string(),
        status: SessionStatus::Cancelled,
        started_at_unix,
        ttyd_url: String::new(),
        prompt_preview: prompt_preview.to_string(),
        num_turns: 0,
        last_event_at_unix: 0,
        finished_at_unix: now_unix(),
        duration_wall_ms: wall_start.elapsed().as_millis() as u64,
        container_exit_code: -1,
        agent_exit_code: -1,
        is_process_error: true,
        response: "submit() unwound before this run task could hand back ttyd readiness; \
                   no client ever received this session_id"
            .into(),
        agent_duration_ms: 0,
        bundle_archive_path: String::new(),
        bundle_compressed_bytes: 0,
        bundle_uncompressed_bytes: 0,
        bundle_file_count: 0,
        bundle_artifacts_file_count: 0,
        teardown_diagnostics,
    }
}

// ── ttyd readiness ─────────────────────────────────────────────────────────

/// Three-phase post-`docker run` ttyd readiness:
/// (1) wait for ttyd to bind on `127.0.0.1:7681` *inside* the agent
///     (probed via `docker exec ... nc -z` since the host has no route
///     to the agent on the isolated-gateway --internal network),
/// (2) resolve the agent's IPv4 address on the agent network so the
///     sidecar knows where to forward,
/// (3) bring up the ttyd-publishing sidecar via
///     `IsolatedNetwork::attach_ttyd_sidecar`, which returns the
///     host-loopback ephemeral port the operator browses to.
async fn wait_and_attach_ttyd(
    net: &mut IsolatedNetwork,
    cfg: &Config,
    session_id: &str,
    agent_container_name: &str,
) -> ServiceResult<u16> {
    wait_for_agent_ttyd_bound(agent_container_name, Duration::from_secs(20)).await?;
    let agent_ip =
        docker_ops::container_ip_on_network(agent_container_name, &net.network_name).await?;
    net.attach_ttyd_sidecar(cfg, session_id, &agent_ip).await
}

/// Poll `docker exec <container> nc -z 127.0.0.1 7681` until ttyd's
/// in-container TCP listener accepts. nc-openbsd is preinstalled in the
/// agent image (verified at the manifest layer of `qwen-agent-template`).
async fn wait_for_agent_ttyd_bound(
    agent_container_name: &str,
    hard_timeout: Duration,
) -> ServiceResult<()> {
    let deadline = std::time::Instant::now() + hard_timeout;
    let mut last_err: Option<String> = None;
    while std::time::Instant::now() < deadline {
        match docker_ops::run_docker(
            [
                "exec",
                agent_container_name,
                "nc",
                "-z",
                "-w",
                "2",
                "127.0.0.1",
                &TTYD_CONTAINER_PORT.to_string(),
            ],
            "wait_for_agent_ttyd_bound",
        )
        .await
        {
            Ok(_) => return Ok(()),
            Err(e) => last_err = Some(e.to_string()),
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    Err(ServiceError::Timeout(format!(
        "agent container {agent_container_name} did not bind ttyd on \
         127.0.0.1:{TTYD_CONTAINER_PORT} within {hard_timeout:?}; last probe error: {}",
        last_err.unwrap_or_else(|| "<none>".into())
    )))
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// Re-export `sweep_orphans` so the API layer can call it at boot without
// depending on `network::` directly — the session module is the natural
// boundary for the singleton-related surface.
pub use network::sweep_orphans;
