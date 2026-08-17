//! One fail-closed session lifecycle.
//!
//! A session is staged, then the narrow broker creates the exact network-none
//! agent plus its fixed model relay. The service never constructs Docker argv.
//! Once ready, the agent may run without an arbitrary wall-clock or
//! cumulative-turn cutoff.
//! Cancellation is explicit. Every setup, capture, parse, bundle, teardown,
//! and persistence-relevant failure is represented in the terminal body.

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::bundle;
use crate::config::Config;
use crate::docker_ops;
use crate::error::{ServiceError, ServiceResult};
use crate::result_parse;
use crate::runtime::{RunningSnapshot, SessionBody, SessionStatus};
use crate::staging::{self, SessionPaths};
use crate::validation::ValidatedRequest;

#[derive(Clone, Copy)]
struct FailureContext<'a> {
    session_id: &'a str,
    prompt_preview: &'a str,
    preserve_thinking: bool,
    started_at_unix: u64,
    wall_start: std::time::Instant,
}

#[derive(Clone, Copy, Debug)]
struct TeardownProof {
    complete: bool,
    quiescent: bool,
}

#[derive(Clone, Copy, Debug)]
enum FinalizationPhase {
    Normal,
    PanicRecovery,
    SetupFailure,
}

/// Select the sole raw-tree disposition from facts that have already been
/// proved. A non-quiescent tree can never be bundled, a failed required bundle
/// must remain available for forensics, and incomplete container removal keeps
/// the raw tree even when an independently valid bundle exists.
fn raw_retention_decision(
    teardown: TeardownProof,
    accepted_archive: bool,
    phase: FinalizationPhase,
) -> Option<(&'static str, &'static str)> {
    if !teardown.quiescent {
        Some((
            "container-quiescence-unproved",
            match phase {
                FinalizationPhase::Normal => {
                    "container quiescence was not proved; bundle was forbidden"
                }
                FinalizationPhase::PanicRecovery => {
                    "panic-recovery container quiescence was not proved; bundle was forbidden"
                }
                FinalizationPhase::SetupFailure => {
                    "setup-failure container quiescence was not proved; bundle was forbidden"
                }
            },
        ))
    } else if !accepted_archive {
        Some(match phase {
            FinalizationPhase::Normal => ("required-bundle-failure", "required bundle failure"),
            FinalizationPhase::PanicRecovery => (
                "panic-recovery-bundle-failure",
                "panic-recovery bundle failure",
            ),
            FinalizationPhase::SetupFailure => (
                "setup-forensic-bundle-failure",
                "setup-failure bundle failure",
            ),
        })
    } else if !teardown.complete {
        Some((
            "container-teardown-incomplete",
            match phase {
                FinalizationPhase::Normal => {
                    "incomplete container teardown despite successful bundle creation"
                }
                FinalizationPhase::PanicRecovery => {
                    "incomplete panic-recovery container teardown despite successful bundle creation"
                }
                FinalizationPhase::SetupFailure => {
                    "incomplete setup-failure container teardown despite successful bundle creation"
                }
            },
        ))
    } else {
        None
    }
}

/// Removal and bundle authority are related but distinct. A failed remove can
/// leave only stopped containers, in which case a stable bundle is still
/// allowed and raw state is retained for teardown forensics. If the broker
/// cannot independently prove every extant container stopped, archiving the
/// live bind-mounted tree is forbidden.
async fn remove_and_prove_quiescent(
    cfg: &Config,
    session_id: &str,
    context: &str,
    diagnostics: &mut Vec<String>,
) -> TeardownProof {
    let removal_succeeded = match docker_ops::remove_session(cfg, session_id).await {
        Ok(()) => true,
        Err(error) => {
            diagnostics.push(format!("{context}: remove session containers: {error}"));
            false
        }
    };
    match docker_ops::prove_session_quiescent(cfg, session_id).await {
        Ok(state) => {
            if !state.quiescent {
                diagnostics.push(format!(
                    "{context}: broker could not prove filesystem quiescence: agent_present={} agent_running={} relay_present={} relay_running={} capture_present={} capture_running={}",
                    state.agent_present,
                    state.agent_running,
                    state.relay_present,
                    state.relay_running,
                    state.capture_present,
                    state.capture_running,
                ));
            }
            let absent = !state.agent_present && !state.relay_present && !state.capture_present;
            if removal_succeeded && !absent {
                diagnostics.push(format!(
                    "{context}: remove_session reported success but exact-owned containers remain: agent_present={} relay_present={} capture_present={}",
                    state.agent_present, state.relay_present, state.capture_present,
                ));
            }
            TeardownProof {
                complete: removal_succeeded && absent && state.quiescent,
                quiescent: state.quiescent,
            }
        }
        Err(error) => {
            diagnostics.push(format!(
                "{context}: independent post-teardown quiescence proof failed: {error}"
            ));
            TeardownProof {
                complete: false,
                quiescent: false,
            }
        }
    }
}

pub async fn run_one(
    cfg: &Config,
    session_id: &str,
    req: ValidatedRequest,
    cancel: CancellationToken,
    ready_tx: oneshot::Sender<ServiceResult<RunningSnapshot>>,
    prompt_preview: String,
) -> SessionBody {
    let wall_start = std::time::Instant::now();
    let started_at_unix = now_unix();
    let mut ready_tx = Some(ready_tx);
    let paths = SessionPaths::new(&cfg.state_dir, session_id);
    let failure_context = FailureContext {
        session_id,
        prompt_preview: &prompt_preview,
        preserve_thinking: req.preserve_thinking,
        started_at_unix,
        wall_start,
    };

    if let Err(error) = paths.create_dirs() {
        return early_failure(
            &mut ready_tx,
            failure_context,
            error,
            Vec::new(),
            SessionStatus::Completed,
        );
    }
    let (staged_bytes, staged_entries) =
        match staging::copy_into_staged(&req.source_dir, &req.folder, &paths.staged) {
            Ok(value) => value,
            Err(error) => {
                let diagnostics = paths.remove_all();
                return early_failure(
                    &mut ready_tx,
                    failure_context,
                    error,
                    diagnostics,
                    SessionStatus::Completed,
                );
            }
        };
    tracing::info!(
        session_id,
        staged_bytes,
        staged_entries,
        "descriptor-anchored source copy preserved opaque symlinks and passed size, type, and read-race validation"
    );
    if let Err(error) = paths.write_prompt(&req.prompt) {
        let diagnostics = paths.remove_all();
        return early_failure(
            &mut ready_tx,
            failure_context,
            error,
            diagnostics,
            SessionStatus::Completed,
        );
    }
    if let Err(error) = paths.write_history_policy(req.preserve_thinking) {
        let diagnostics = paths.remove_all();
        return early_failure(
            &mut ready_tx,
            failure_context,
            error,
            diagnostics,
            SessionStatus::Completed,
        );
    }
    let start_gate = match paths.create_locked_start_gate() {
        Ok(value) => value,
        Err(error) => {
            let diagnostics = paths.remove_all();
            return early_failure(
                &mut ready_tx,
                failure_context,
                error,
                diagnostics,
                SessionStatus::Completed,
            );
        }
    };
    if cancel.is_cancelled() {
        let diagnostics = paths.remove_all();
        return early_failure(
            &mut ready_tx,
            failure_context,
            ServiceError::Internal("session was cancelled before Docker setup".into()),
            diagnostics,
            SessionStatus::Cancelled,
        );
    }

    let _names = match docker_ops::create_session(cfg, session_id, req.preserve_thinking).await {
        Ok(value) => value,
        Err(error) => {
            // Release the wrapper gate before forensic finalization. The
            // broker operation is transactional, but a partial container may
            // still have emitted useful setup evidence before returning its
            // explicit error.
            drop(start_gate);
            return setup_failure_after_agent(
                &mut ready_tx,
                failure_context,
                cfg,
                session_id,
                paths,
                error,
                SessionStatus::Completed,
            )
            .await;
        }
    };
    if let Err(error) = std::fs::File::unlock(&start_gate) {
        // Dropping the descriptor is the only remaining way to release an
        // unexpectedly failed advisory unlock. Finalization stops the agent
        // before reading logs, so it cannot proceed into the task unnoticed.
        drop(start_gate);
        let setup_error = ServiceError::Internal(format!(
            "cannot release the exact agent start gate: {error}"
        ));
        let mut diagnostics = vec![format!("release agent start gate: {error}")];
        if let Some(sender) = ready_tx.take() {
            if sender.send(Err(setup_error.clone())).is_err() {
                diagnostics.push(
                    "failed to deliver start-gate setup error because the readiness receiver was dropped"
                        .into(),
                );
            }
        }
        return finalize_started_setup_failure(
            failure_context,
            cfg,
            session_id,
            paths,
            setup_error,
            SessionStatus::Completed,
            diagnostics,
        )
        .await;
    }
    drop(start_gate);
    if cancel.is_cancelled() {
        return setup_failure_after_agent(
            &mut ready_tx,
            failure_context,
            cfg,
            session_id,
            paths,
            ServiceError::Internal("session was cancelled before agent readiness".into()),
            SessionStatus::Cancelled,
        )
        .await;
    }

    let ready = match wait_for_agent_ready(
        cfg,
        session_id,
        &paths,
        &cancel,
        req.preserve_thinking,
    )
    .await
    {
        Ok(value) => value,
        Err(error) => {
            return setup_failure_after_agent(
                &mut ready_tx,
                failure_context,
                cfg,
                session_id,
                paths,
                error,
                if cancel.is_cancelled() {
                    SessionStatus::Cancelled
                } else {
                    SessionStatus::Completed
                },
            )
            .await;
        }
    };
    let snapshot = RunningSnapshot {
        session_id: session_id.to_string(),
        started_at_unix,
        prompt_preview: prompt_preview.clone(),
        model: ready.model,
        context_window: ready.context_window,
        preserve_thinking: ready.preserve_thinking,
    };
    if let Some(sender) = ready_tx.take() {
        if sender.send(Ok(snapshot)).is_err() {
            let diagnostics = vec![
                "readiness receiver disappeared after successful agent preflight; cancelling the orphaned session".into(),
            ];
            return finalize_started_setup_failure(
                failure_context,
                cfg,
                session_id,
                paths,
                ServiceError::Internal(
                    "session requester disappeared before readiness could be delivered".into(),
                ),
                SessionStatus::Cancelled,
                diagnostics,
            )
            .await;
        }
    }

    let (status, container_exit_code, mut diagnostics, producer_stopped) =
        wait_for_completion_or_cancel(cfg, session_id, &cancel).await;

    let capture_result = if producer_stopped {
        Some(docker_ops::wait_capture_complete(cfg, session_id).await)
    } else {
        diagnostics.push(
            "stream-capture completion was not awaited because the Qwen producer could not be proved stopped"
                .into(),
        );
        None
    };
    let mut capture_proved = false;
    match capture_result {
        Some(Ok(capture)) => match validate_completed_capture(&paths, &capture) {
            Ok(()) => capture_proved = true,
            Err(error) => diagnostics.push(error.to_string()),
        },
        Some(Err(error)) => diagnostics.push(format!(
            "trusted stream capture did not prove complete durable output: {error}"
        )),
        None => {}
    }
    if let Err(error) = write_private_file(
        &paths.output.join("qwen-exit-code"),
        format!("{container_exit_code}\n").as_bytes(),
    ) {
        diagnostics.push(format!("write trusted Qwen exit-code sidecar: {error}"));
    }
    if let Err(error) = sync_directory(&paths.output, "sync completion sidecars") {
        diagnostics.push(error.to_string());
    }

    let logs = docker_ops::session_logs(cfg, session_id)
        .await
        .unwrap_or_else(|error| {
            diagnostics.push(format!("read final agent logs: {error}"));
            "<agent logs unavailable>".into()
        });
    let teardown =
        remove_and_prove_quiescent(cfg, session_id, "normal completion", &mut diagnostics).await;
    if let Err(error) =
        write_private_file(&paths.control.join("container-logs.txt"), logs.as_bytes())
    {
        diagnostics.push(format!("persist bounded final container logs: {error}"));
    }

    let agent_exit_code = read_required_exit_code(&paths.output.join("qwen-exit-code"))
        .unwrap_or_else(|error| {
            diagnostics.push(error.to_string());
            -1
        });

    let parsed = if capture_proved {
        result_parse::parse_events_jsonl(&paths.events_jsonl())
    } else {
        Err(ServiceError::AgentOutputMissing(
            "trusted stream capture was not proved complete; refusing to parse or promote its event file"
                .into(),
        ))
    };
    let (mut response, agent_duration_ms, num_turns, mut is_process_error, parsed_valid) =
        match parsed {
            Ok(result) => (
                result.response,
                result.duration_ms,
                result.num_turns,
                result.is_error,
                true,
            ),
            Err(error) if status == SessionStatus::Cancelled => (
                format!("session cancelled before a terminal result event was emitted: {error}"),
                0,
                0,
                false,
                false,
            ),
            Err(error) => {
                diagnostics.push(format!("strict event parse failed: {error}"));
                (
                    format!("agent output was invalid: {error}; recent container logs:\n{logs}"),
                    0,
                    0,
                    true,
                    false,
                )
            }
        };
    let last_event_at_unix = match crate::runtime::read_running_progress(&paths.events_jsonl()) {
        Ok((observed_turns, timestamp)) => {
            if parsed_valid && observed_turns != num_turns {
                diagnostics.push(format!(
                    "live/final turn-count mismatch: live reader observed {observed_turns}, strict terminal parser observed {num_turns}"
                ));
            }
            timestamp
        }
        Err(error) => {
            diagnostics.push(format!("read final event progress metadata: {error}"));
            0
        }
    };
    if status == SessionStatus::Completed && (container_exit_code != 0 || agent_exit_code != 0) {
        is_process_error = true;
        response = format!(
            "agent exited abnormally (container={container_exit_code}, qwen={agent_exit_code}). {response}"
        );
    }
    if !diagnostics.is_empty() {
        // Cleanup/capture failures are part of process correctness. They do
        // not erase the useful response, but they may never look successful.
        is_process_error = true;
    }

    let response_path = paths.output.join("response.txt");
    if let Err(error) = write_private_file(&response_path, response.as_bytes()) {
        diagnostics.push(format!("write response sidecar: {error}"));
        is_process_error = true;
    }

    let archive = cfg.results_dir.join(session_id).join("bundle.tar.zst");
    let bundle_result = if teardown.quiescent {
        bundle::create_bundle(&paths.root, &archive).await
    } else {
        Err(ServiceError::Internal(
            "required bundle was not attempted because exact-owned container quiescence was not proved"
                .into(),
        ))
    };
    let (archive_path, compressed, uncompressed, file_count, artifacts_count) = match bundle_result
    {
        Ok(stats) => (
            stats.archive_path.display().to_string(),
            stats.compressed_bytes,
            stats.uncompressed_bytes,
            stats.file_count,
            stats.artifacts_file_count,
        ),
        Err(error) => {
            diagnostics.push(format!("required bundle creation failed: {error}"));
            is_process_error = true;
            (String::new(), 0, 0, 0, 0)
        }
    };
    let raw_session_tree_retained = raw_retention_decision(
        teardown,
        !archive_path.is_empty(),
        FinalizationPhase::Normal,
    )
    .map(|(cause, context)| retain_raw_evidence(&paths, cause, context, &mut diagnostics))
    .unwrap_or(false);
    if !diagnostics.is_empty() {
        is_process_error = true;
    }

    SessionBody {
        session_id: session_id.to_string(),
        status,
        started_at_unix,
        model: cfg.vllm_model_name.clone(),
        context_window: cfg.lock.backend.max_model_len,
        preserve_thinking: req.preserve_thinking,
        prompt_preview,
        num_turns,
        last_event_at_unix,
        finished_at_unix: now_unix(),
        duration_wall_ms: elapsed_ms(wall_start),
        container_exit_code,
        agent_exit_code,
        is_process_error,
        response,
        agent_duration_ms,
        bundle_archive_path: archive_path,
        bundle_compressed_bytes: compressed,
        bundle_uncompressed_bytes: uncompressed,
        bundle_file_count: file_count,
        bundle_artifacts_file_count: artifacts_count,
        raw_session_tree_retained,
        teardown_diagnostics: diagnostics,
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReadyFile {
    model: String,
    context_window: u64,
    token_count: u64,
    preserve_thinking: bool,
    sandbox: String,
}

async fn wait_for_agent_ready(
    cfg: &Config,
    session_id: &str,
    paths: &SessionPaths,
    cancel: &CancellationToken,
    expected_preserve_thinking: bool,
) -> ServiceResult<ReadyFile> {
    let event_wait = docker_ops::wait_agent_ready(cfg, session_id);
    tokio::pin!(event_wait);
    let broker_ready = tokio::select! {
        result = &mut event_wait => result?,
        () = cancel.cancelled() => {
            return Err(ServiceError::Internal(
                "session cancelled while awaiting the broker's exact agent-ready event".into(),
            ));
        }
    };
    let ready = ReadyFile {
        model: broker_ready.model,
        context_window: broker_ready.context_window,
        token_count: broker_ready.token_count,
        preserve_thinking: broker_ready.preserve_thinking,
        sandbox: broker_ready.sandbox,
    };
    if ready.model != cfg.vllm_model_name
        || ready.context_window != cfg.lock.backend.max_model_len
        || ready.token_count == 0
        || ready.preserve_thinking != expected_preserve_thinking
        || ready.sandbox
            != "landlock-fs-v4-write-roots-v1+private-devpts-rw-v1+output-unmounted-v1"
    {
        return Err(ServiceError::Internal(format!(
            "broker returned a drifted agent readiness contract: {ready:?}"
        )));
    }
    let ready_path = paths.output.join("ready.json");
    let mut encoded = serde_json::to_vec(&ready).map_err(|error| {
        ServiceError::Internal(format!("serialize trusted agent readiness record: {error}"))
    })?;
    encoded.push(b'\n');
    write_private_file(&ready_path, &encoded)?;
    sync_directory(&paths.output, "sync trusted agent readiness")?;
    let persisted = std::fs::read(&ready_path).map_err(|error| {
        ServiceError::Internal(format!(
            "read back trusted agent readiness {}: {error}",
            ready_path.display()
        ))
    })?;
    if persisted != encoded {
        return Err(ServiceError::Internal(format!(
            "trusted agent readiness changed after durable publication at {}",
            ready_path.display()
        )));
    }
    Ok(ready)
}

async fn wait_for_completion_or_cancel(
    cfg: &Config,
    session_id: &str,
    cancel: &CancellationToken,
) -> (SessionStatus, i32, Vec<String>, bool) {
    let mut diagnostics = Vec::new();
    let wait = docker_ops::wait_session(cfg, session_id);
    tokio::pin!(wait);
    tokio::select! {
        result = &mut wait => match result {
            Ok(code) => (SessionStatus::Completed, code, diagnostics, true),
            Err(error) => {
                diagnostics.push(format!("docker wait failed: {error}"));
                let producer_stopped = match docker_ops::stop_session(cfg, session_id).await {
                    Ok(()) => true,
                    Err(stop_error) => {
                        diagnostics.push(format!(
                            "stop agent after failed Docker wait also failed: {stop_error}"
                        ));
                        false
                    }
                };
                (SessionStatus::Completed, -1, diagnostics, producer_stopped)
            }
        },
        () = cancel.cancelled() => {
            let mut producer_stopped = true;
            if let Err(error) = docker_ops::stop_session(cfg, session_id).await {
                diagnostics.push(format!("graceful cancellation stop failed: {error}"));
                if let Err(remove_error) = docker_ops::remove_session(cfg, session_id).await {
                    diagnostics.push(format!(
                        "ownership-checked cancellation cleanup also failed: {remove_error}"
                    ));
                    producer_stopped = false;
                }
            }
            let code = match wait.await {
                Ok(value) => value,
                Err(error) => {
                    diagnostics.push(format!("docker wait after cancellation failed: {error}"));
                    -1
                }
            };
            (SessionStatus::Cancelled, code, diagnostics, producer_stopped)
        }
    }
}

/// Convert an unexpected panic/cancellation of the inner execution task into
/// an explicit terminal process error. The outer runtime supervisor calls this
/// while it still owns the singleton permit, so no new session can overlap the
/// ownership-checked Docker cleanup, forensic bundle attempt, or persistence.
#[allow(clippy::too_many_arguments)]
pub async fn recover_after_execution_panic(
    cfg: &Config,
    session_id: &str,
    full_prompt: &str,
    prompt_preview: &str,
    preserve_thinking: bool,
    started_at_unix: u64,
    wall_start: std::time::Instant,
    cancelled: bool,
    join_error: String,
) -> SessionBody {
    let paths = SessionPaths::new(&cfg.state_dir, session_id);
    let status = if cancelled {
        SessionStatus::Cancelled
    } else {
        SessionStatus::Completed
    };
    let response = format!(
        "agent session execution terminated unexpectedly; no success was inferred: {join_error}"
    );
    let mut diagnostics = vec![format!(
        "session execution task terminated unexpectedly: {join_error}"
    )];

    let producer_stopped = match docker_ops::stop_session(cfg, session_id).await {
        Ok(()) => true,
        Err(error) => {
            diagnostics.push(format!(
                "stop session after execution-task failure: {error}"
            ));
            false
        }
    };
    if producer_stopped {
        match docker_ops::wait_capture_complete(cfg, session_id).await {
            Ok(capture) => {
                if let Err(error) = validate_completed_capture(&paths, &capture) {
                    diagnostics.push(format!(
                        "panic-recovery stream capture validation failed: {error}"
                    ));
                }
            }
            Err(error) => diagnostics.push(format!(
                "panic-recovery stream capture did not prove complete durable output: {error}"
            )),
        }
    } else {
        diagnostics.push(
            "panic-recovery stream capture was not awaited because the Qwen producer could not be proved stopped"
                .into(),
        );
    }
    let captured_logs = match docker_ops::session_logs(cfg, session_id).await {
        Ok(logs) => logs,
        Err(error) => {
            diagnostics.push(format!(
                "collect bounded container logs after execution-task failure: {error}"
            ));
            "<owned container logs unavailable>\n".into()
        }
    };
    let teardown = remove_and_prove_quiescent(
        cfg,
        session_id,
        "execution-task panic recovery",
        &mut diagnostics,
    )
    .await;

    let mut archive_path = String::new();
    let mut compressed = 0;
    let mut uncompressed = 0;
    let mut file_count = 0;
    let mut artifacts_count = 0;
    match paths.ensure_recovery_dirs() {
        Ok(()) => {
            if let Err(error) = ensure_prompt_record(&paths, full_prompt) {
                diagnostics.push(format!(
                    "preserve prompt after execution-task failure: {error}"
                ));
            }
            if let Err(error) = ensure_history_policy_record(&paths, preserve_thinking) {
                diagnostics.push(format!(
                    "preserve history policy after execution-task failure: {error}"
                ));
            }
            for (path, contents) in [
                (
                    paths.output.join("ready.json"),
                    b"{\"ready\":false,\"failure\":\"execution task terminated before a valid readiness record\"}\n".as_slice(),
                ),
                (paths.output.join("events.jsonl"), b"".as_slice()),
                (paths.output.join("qwen.stderr"), b"".as_slice()),
                (paths.output.join("qwen-exit-code"), b"-1\n".as_slice()),
                (paths.output.join("response.txt"), response.as_bytes()),
                (
                    paths.control.join("container-logs.txt"),
                    captured_logs.as_bytes(),
                ),
            ] {
                if let Err(error) = ensure_private_forensic_file(&path, contents) {
                    diagnostics.push(format!(
                        "preserve forensic sidecar {}: {error}",
                        path.display()
                    ));
                }
            }
            let supervisor_failure = paths.output.join("supervisor-failure.txt");
            if let Err(error) =
                write_private_file(&supervisor_failure, format!("{response}\n").as_bytes())
            {
                diagnostics.push(format!(
                    "write supervisor failure sidecar {}: {error}",
                    supervisor_failure.display()
                ));
            }

            let archive = cfg.results_dir.join(session_id).join("bundle.tar.zst");
            match std::fs::symlink_metadata(&archive) {
                Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                    diagnostics.push(
                        "a previously published archive exists after the execution-task failure but its complete accepted counters were not durably available; it remains unaccepted forensic evidence".into(),
                    );
                }
                Ok(_) => diagnostics.push(format!(
                    "panic-recovery bundle destination is not a regular non-symlink file: {}",
                    archive.display()
                )),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    let bundle_result = if teardown.quiescent {
                        bundle::create_bundle(&paths.root, &archive).await
                    } else {
                        Err(ServiceError::Internal(
                            "panic-recovery bundle was not attempted because exact-owned container quiescence was not proved"
                                .into(),
                        ))
                    };
                    match bundle_result {
                        Ok(stats) => {
                            archive_path = stats.archive_path.display().to_string();
                            compressed = stats.compressed_bytes;
                            uncompressed = stats.uncompressed_bytes;
                            file_count = stats.file_count;
                            artifacts_count = stats.artifacts_file_count;
                        }
                        Err(bundle_error) => diagnostics.push(format!(
                            "panic-recovery forensic bundle failed: {bundle_error}"
                        )),
                    }
                }
                Err(error) => diagnostics.push(format!(
                    "stat panic-recovery bundle destination {}: {error}",
                    archive.display()
                )),
            }
        }
        Err(error) => diagnostics.push(format!(
            "reconstruct session directories for panic evidence: {error}"
        )),
    }
    let raw_session_tree_retained = raw_retention_decision(
        teardown,
        !archive_path.is_empty(),
        FinalizationPhase::PanicRecovery,
    )
    .map(|(cause, context)| retain_raw_evidence(&paths, cause, context, &mut diagnostics))
    .unwrap_or(false);

    SessionBody {
        session_id: session_id.to_string(),
        status,
        started_at_unix,
        model: cfg.vllm_model_name.clone(),
        context_window: cfg.lock.backend.max_model_len,
        preserve_thinking,
        prompt_preview: prompt_preview.to_string(),
        num_turns: 0,
        last_event_at_unix: 0,
        finished_at_unix: now_unix(),
        duration_wall_ms: elapsed_ms(wall_start),
        container_exit_code: -1,
        agent_exit_code: -1,
        is_process_error: true,
        response,
        agent_duration_ms: 0,
        bundle_archive_path: archive_path,
        bundle_compressed_bytes: compressed,
        bundle_uncompressed_bytes: uncompressed,
        bundle_file_count: file_count,
        bundle_artifacts_file_count: artifacts_count,
        raw_session_tree_retained,
        teardown_diagnostics: diagnostics,
    }
}

fn ensure_prompt_record(paths: &SessionPaths, prompt: &str) -> ServiceResult<()> {
    let path = paths.control.join("prompt.txt");
    match std::fs::symlink_metadata(&path) {
        Ok(metadata)
            if metadata.is_file()
                && !metadata.file_type().is_symlink()
                && metadata.uid() == 1000
                && metadata.gid() == 1000
                && metadata.permissions().mode() & 0o777 == 0o644 =>
        {
            let existing = std::fs::read(&path).map_err(|error| {
                ServiceError::Internal(format!(
                    "read existing panic-recovery prompt record {}: {error}",
                    path.display()
                ))
            })?;
            if existing == prompt.as_bytes() {
                Ok(())
            } else {
                Err(ServiceError::Internal(format!(
                    "existing panic-recovery prompt record {} does not match the in-memory submitted prompt",
                    path.display()
                )))
            }
        }
        Ok(metadata) => Err(ServiceError::Internal(format!(
            "{} is not the exact regular 1000:1000 mode-0644 prompt record: type={:?} uid={} gid={} mode={:o}",
            path.display(),
            metadata.file_type(),
            metadata.uid(),
            metadata.gid(),
            metadata.permissions().mode() & 0o777,
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            paths.write_prompt(prompt).map(|_| ())
        }
        Err(error) => Err(ServiceError::Internal(format!(
            "stat prompt record {}: {error}",
            path.display()
        ))),
    }
}

fn ensure_history_policy_record(
    paths: &SessionPaths,
    preserve_thinking: bool,
) -> ServiceResult<()> {
    let path = paths.control.join("history-policy.json");
    let expected: &[u8] = if preserve_thinking {
        b"{\"preserve_thinking\":true}\n"
    } else {
        b"{\"preserve_thinking\":false}\n"
    };
    match std::fs::symlink_metadata(&path) {
        Ok(metadata)
            if metadata.is_file()
                && !metadata.file_type().is_symlink()
                && metadata.uid() == 1000
                && metadata.gid() == 1000
                && metadata.permissions().mode() & 0o777 == 0o444 =>
        {
            let existing = std::fs::read(&path).map_err(|error| {
                ServiceError::Internal(format!(
                    "read existing panic-recovery history policy {}: {error}",
                    path.display()
                ))
            })?;
            if existing == expected {
                Ok(())
            } else {
                Err(ServiceError::Internal(format!(
                    "existing panic-recovery history policy {} contradicts the submitted preserve_thinking={preserve_thinking}",
                    path.display()
                )))
            }
        }
        Ok(metadata) => Err(ServiceError::Internal(format!(
            "{} is not the exact regular 1000:1000 mode-0444 history-policy record: type={:?} uid={} gid={} mode={:o}",
            path.display(),
            metadata.file_type(),
            metadata.uid(),
            metadata.gid(),
            metadata.permissions().mode() & 0o777
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            paths.write_history_policy(preserve_thinking).map(|_| ())
        }
        Err(error) => Err(ServiceError::Internal(format!(
            "stat panic-recovery history policy {}: {error}",
            path.display()
        ))),
    }
}

fn ensure_private_forensic_file(path: &Path, contents: &[u8]) -> ServiceResult<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(ServiceError::Internal(format!(
            "{} is not a regular non-symlink file",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            write_private_file(path, contents)
        }
        Err(error) => Err(ServiceError::Internal(format!(
            "stat forensic sidecar {}: {error}",
            path.display()
        ))),
    }
}

fn mark_raw_evidence_retained(paths: &SessionPaths, cause: &str) -> ServiceResult<()> {
    let marker = paths.control.join("raw-evidence-retained.txt");
    write_private_file(
        &marker,
        format!(
            "RAW_SESSION_TREE_RETAINED\ncause={cause}\npath={}\n",
            paths.root.display()
        )
        .as_bytes(),
    )?;
    std::fs::File::open(&paths.control)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            ServiceError::Internal(format!(
                "sync raw-evidence marker directory {} after creating {}: {error}",
                paths.control.display(),
                marker.display()
            ))
        })
}

fn retain_raw_evidence(
    paths: &SessionPaths,
    cause: &str,
    context: &str,
    diagnostics: &mut Vec<String>,
) -> bool {
    match std::fs::symlink_metadata(&paths.root) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            diagnostics.push(format!(
                "raw session tree retained after {context} at {}",
                paths.root.display()
            ));
            if let Err(error) = mark_raw_evidence_retained(paths, cause) {
                diagnostics.push(format!("write raw-evidence retention marker: {error}"));
            }
            true
        }
        Ok(_) => {
            diagnostics.push(format!(
                "{context} left no safely retainable ordinary session directory at {}",
                paths.root.display()
            ));
            false
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            diagnostics.push(format!(
                "{context} left no raw session tree at {}",
                paths.root.display()
            ));
            false
        }
        Err(error) => {
            diagnostics.push(format!(
                "cannot determine whether raw session evidence exists at {} after {context}: {error}",
                paths.root.display()
            ));
            false
        }
    }
}

async fn setup_failure_after_agent(
    ready_tx: &mut Option<oneshot::Sender<ServiceResult<RunningSnapshot>>>,
    context: FailureContext<'_>,
    cfg: &Config,
    session_id: &str,
    paths: SessionPaths,
    error: ServiceError,
    status: SessionStatus,
) -> SessionBody {
    let mut diagnostics = Vec::new();
    if let Some(sender) = ready_tx.take() {
        // The HTTP caller receives the precise setup failure. The supervisor
        // still persists the forensic terminal body returned below.
        if sender.send(Err(error.clone())).is_err() {
            diagnostics.push(
                "failed to deliver setup error because the readiness receiver was dropped".into(),
            );
        }
    }
    finalize_started_setup_failure(context, cfg, session_id, paths, error, status, diagnostics)
        .await
}

async fn finalize_started_setup_failure(
    context: FailureContext<'_>,
    cfg: &Config,
    session_id: &str,
    paths: SessionPaths,
    error: ServiceError,
    status: SessionStatus,
    mut diagnostics: Vec<String>,
) -> SessionBody {
    let response = format!(
        "agent session failed after its container topology was created; no success was inferred: {error}"
    );
    if let Err(stop_error) = docker_ops::stop_session(cfg, session_id).await {
        diagnostics.push(format!(
            "stop failed session after setup error: {stop_error}"
        ));
    }
    let mut captured_logs = String::new();
    match docker_ops::session_logs(cfg, session_id).await {
        Ok(logs) if !logs.trim().is_empty() => {
            captured_logs = logs;
            diagnostics.push(format!("agent logs:\n{captured_logs}"));
        }
        Ok(_) => {}
        Err(log_error) => diagnostics.push(format!("read agent logs: {log_error}")),
    }
    let teardown =
        remove_and_prove_quiescent(cfg, session_id, "setup-failure recovery", &mut diagnostics)
            .await;

    if let Err(sidecar_error) = write_private_file(
        &paths.control.join("setup-failure.txt"),
        format!("{response}\n").as_bytes(),
    ) {
        diagnostics.push(format!(
            "write authoritative setup-failure record: {sidecar_error}"
        ));
    }
    for (path, contents) in [
        (
            paths.output.join("ready.json"),
            b"{\"ready\":false,\"failure\":\"setup failed before a valid readiness record\"}\n"
                .as_slice(),
        ),
        (paths.output.join("events.jsonl"), b"".as_slice()),
        (paths.output.join("qwen.stderr"), captured_logs.as_bytes()),
        (paths.output.join("qwen-exit-code"), b"-1\n".as_slice()),
        (paths.output.join("response.txt"), response.as_bytes()),
        (
            paths.control.join("container-logs.txt"),
            captured_logs.as_bytes(),
        ),
    ] {
        if let Err(sidecar_error) = ensure_private_forensic_file(&path, contents) {
            diagnostics.push(format!(
                "preserve setup-failure sidecar {}: {sidecar_error}",
                path.display()
            ));
        }
    }

    let agent_exit_code = read_required_exit_code(&paths.output.join("qwen-exit-code"))
        .unwrap_or_else(|exit_error| {
            diagnostics.push(format!("read setup-failure exit code: {exit_error}"));
            -1
        });
    let (num_turns, last_event_at_unix) =
        match crate::runtime::read_running_progress(&paths.events_jsonl()) {
            Ok(progress) => progress,
            Err(progress_error) => {
                diagnostics.push(format!(
                    "read setup-failure event progress metadata: {progress_error}"
                ));
                (0, 0)
            }
        };

    let archive = cfg.results_dir.join(session_id).join("bundle.tar.zst");
    let bundle_result = if teardown.quiescent {
        bundle::create_bundle(&paths.root, &archive).await
    } else {
        Err(ServiceError::Internal(
            "setup-failure bundle was not attempted because exact-owned container quiescence was not proved"
                .into(),
        ))
    };
    let (archive_path, compressed, uncompressed, file_count, artifacts_count) = match bundle_result
    {
        Ok(stats) => (
            stats.archive_path.display().to_string(),
            stats.compressed_bytes,
            stats.uncompressed_bytes,
            stats.file_count,
            stats.artifacts_file_count,
        ),
        Err(bundle_error) => {
            diagnostics.push(format!(
                "setup-failure forensic bundle failed: {bundle_error}"
            ));
            (String::new(), 0, 0, 0, 0)
        }
    };
    let raw_session_tree_retained = raw_retention_decision(
        teardown,
        !archive_path.is_empty(),
        FinalizationPhase::SetupFailure,
    )
    .map(|(cause, context)| retain_raw_evidence(&paths, cause, context, &mut diagnostics))
    .unwrap_or(false);

    SessionBody {
        session_id: session_id.to_string(),
        status,
        started_at_unix: context.started_at_unix,
        model: cfg.vllm_model_name.clone(),
        context_window: cfg.lock.backend.max_model_len,
        preserve_thinking: context.preserve_thinking,
        prompt_preview: context.prompt_preview.to_string(),
        num_turns,
        last_event_at_unix,
        finished_at_unix: now_unix(),
        duration_wall_ms: elapsed_ms(context.wall_start),
        container_exit_code: -1,
        agent_exit_code,
        is_process_error: true,
        response,
        agent_duration_ms: 0,
        bundle_archive_path: archive_path,
        bundle_compressed_bytes: compressed,
        bundle_uncompressed_bytes: uncompressed,
        bundle_file_count: file_count,
        bundle_artifacts_file_count: artifacts_count,
        raw_session_tree_retained,
        teardown_diagnostics: diagnostics,
    }
}

fn early_failure(
    sender: &mut Option<oneshot::Sender<ServiceResult<RunningSnapshot>>>,
    context: FailureContext<'_>,
    error: ServiceError,
    mut diagnostics: Vec<String>,
    status: SessionStatus,
) -> SessionBody {
    if let Some(sender) = sender.take() {
        if sender.send(Err(error.clone())).is_err() {
            diagnostics.push(
                "failed to deliver setup error because the readiness receiver was dropped".into(),
            );
        }
    }
    terminal_error(
        context.session_id,
        context.prompt_preview,
        context.preserve_thinking,
        context.started_at_unix,
        context.wall_start,
        status,
        error.to_string(),
        diagnostics,
    )
}

fn terminal_error(
    session_id: &str,
    prompt_preview: &str,
    preserve_thinking: bool,
    started_at_unix: u64,
    wall_start: std::time::Instant,
    status: SessionStatus,
    response: String,
    diagnostics: Vec<String>,
) -> SessionBody {
    SessionBody {
        session_id: session_id.into(),
        status,
        started_at_unix,
        model: String::new(),
        context_window: 0,
        preserve_thinking,
        prompt_preview: prompt_preview.into(),
        num_turns: 0,
        last_event_at_unix: 0,
        finished_at_unix: now_unix(),
        duration_wall_ms: elapsed_ms(wall_start),
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
        raw_session_tree_retained: false,
        teardown_diagnostics: diagnostics,
    }
}

fn read_required_exit_code(path: &Path) -> ServiceResult<i32> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        ServiceError::AgentOutputMissing(format!(
            "required exit-code file {} is missing or unreadable: {error}",
            path.display()
        ))
    })?;
    let trimmed = text.trim();
    let value = trimmed.parse::<i32>().map_err(|error| {
        ServiceError::AgentOutputMissing(format!(
            "exit-code file {} contains {trimmed:?}, not an i32: {error}",
            path.display()
        ))
    })?;
    if trimmed.lines().count() != 1 {
        return Err(ServiceError::AgentOutputMissing(format!(
            "exit-code file {} must contain exactly one line",
            path.display()
        )));
    }
    Ok(value)
}

fn write_private_file(path: &Path, bytes: &[u8]) -> ServiceResult<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| ServiceError::Internal(format!("create {}: {error}", path.display())))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| ServiceError::Internal(format!("write/sync {}: {error}", path.display())))
}

fn validate_capture_output(path: &Path, expected_bytes: u64, label: &str) -> ServiceResult<()> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        ServiceError::AgentOutputMissing(format!(
            "stat trusted captured {label} output {}: {error}",
            path.display()
        ))
    })?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.uid() != 1000
        || metadata.gid() != 1000
        || metadata.permissions().mode() & 0o777 != 0o600
        || metadata.len() != expected_bytes
    {
        return Err(ServiceError::AgentOutputMissing(format!(
            "trusted captured {label} output drift at {}: regular={} symlink={} uid={} gid={} mode={:o} bytes={} expected_bytes={expected_bytes}",
            path.display(),
            metadata.is_file(),
            metadata.file_type().is_symlink(),
            metadata.uid(),
            metadata.gid(),
            metadata.permissions().mode() & 0o777,
            metadata.len(),
        )));
    }
    Ok(())
}

fn validate_completed_capture(
    paths: &SessionPaths,
    capture: &docker_ops::CaptureComplete,
) -> ServiceResult<()> {
    validate_capture_output(&paths.events_jsonl(), capture.events_bytes, "events")?;
    validate_capture_output(
        &paths.output.join("qwen.stderr"),
        capture.stderr_bytes,
        "stderr",
    )
}

fn sync_directory(path: &Path, label: &str) -> ServiceResult<()> {
    std::fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| ServiceError::Internal(format!("{label} {}: {error}", path.display())))
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn elapsed_ms(start: std::time::Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

pub async fn sweep_orphans(cfg: &Config) -> ServiceResult<()> {
    docker_ops::sweep_orphans(cfg).await
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{symlink, PermissionsExt};

    use super::{
        ensure_private_forensic_file, ensure_prompt_record, raw_retention_decision,
        retain_raw_evidence, validate_completed_capture, FinalizationPhase, SessionPaths,
        TeardownProof,
    };
    use crate::docker_ops::CaptureComplete;

    #[test]
    fn panic_forensic_sidecars_are_no_clobber_and_reject_symlinks() {
        let state = std::env::temp_dir().join(format!(
            "qwen38-panic-sidecars-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(state.join("sessions"))
            .expect("create panic-sidecar sessions parent");
        let paths = SessionPaths::new(&state, "s-0123456789abcdef0123456789abcdef");
        paths.create_dirs().expect("create panic-sidecar fixture");

        let sidecar = paths.output.join("events.jsonl");
        ensure_private_forensic_file(&sidecar, b"first\n").expect("create sidecar");
        ensure_private_forensic_file(&sidecar, b"replacement\n")
            .expect("an existing regular sidecar is preserved");
        assert_eq!(
            std::fs::read(&sidecar).expect("read sidecar"),
            b"first\n",
            "panic recovery must never overwrite existing agent evidence"
        );

        let symlink_path = paths.output.join("ready.json");
        symlink(&sidecar, &symlink_path).expect("create hostile sidecar symlink");
        assert!(
            ensure_private_forensic_file(&symlink_path, b"{}\n").is_err(),
            "panic recovery accepted a symlinked forensic sidecar"
        );

        std::fs::remove_dir_all(&state).expect("remove panic-sidecar fixture");
    }

    #[test]
    fn panic_prompt_recovery_requires_exact_metadata_and_bytes() {
        let state = std::env::temp_dir().join(format!(
            "qwen38-panic-prompt-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(state.join("sessions"))
            .expect("create panic-prompt sessions parent");
        let paths = SessionPaths::new(&state, "s-99999999999999999999999999999999");
        paths.create_dirs().expect("create panic-prompt layout");
        let prompt = "exact submitted prompt\n";
        let path = paths
            .write_prompt(prompt)
            .expect("write exact prompt record");
        if unsafe { libc::geteuid() } == 0 {
            std::os::unix::fs::chown(&path, Some(1000), Some(1000))
                .expect("assign runtime prompt ownership in root-run fixture");
        }
        ensure_prompt_record(&paths, prompt).expect("accept exact prompt record");
        assert!(
            ensure_prompt_record(&paths, "different prompt\n").is_err(),
            "panic recovery accepted a prompt record with different bytes"
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .expect("drift prompt record mode");
        assert!(
            ensure_prompt_record(&paths, prompt).is_err(),
            "panic recovery repaired or accepted prompt metadata drift"
        );
        std::fs::remove_dir_all(&state).expect("remove panic-prompt fixture");
    }

    #[test]
    fn raw_evidence_authority_is_exact_durable_and_never_invented() {
        let state = std::env::temp_dir().join(format!(
            "qwen38-raw-evidence-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(state.join("sessions"))
            .expect("create raw-evidence sessions parent");
        let paths = SessionPaths::new(&state, "s-dddddddddddddddddddddddddddddddd");
        paths.create_dirs().expect("create raw-evidence fixture");
        let mut diagnostics = Vec::new();
        assert!(retain_raw_evidence(
            &paths,
            "required-bundle-failure",
            "test bundle failure",
            &mut diagnostics,
        ));
        let marker = paths.control.join("raw-evidence-retained.txt");
        assert_eq!(
            std::fs::metadata(&marker)
                .expect("stat retention marker")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::read_to_string(&marker).expect("read retention marker"),
            format!(
                "RAW_SESSION_TREE_RETAINED\ncause=required-bundle-failure\npath={}\n",
                paths.root.display()
            )
        );

        let missing = SessionPaths::new(&state, "s-eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");
        let mut missing_diagnostics = Vec::new();
        assert!(!retain_raw_evidence(
            &missing,
            "required-bundle-failure",
            "missing-tree failure",
            &mut missing_diagnostics,
        ));
        assert!(missing_diagnostics[0].contains("left no raw session tree"));
        assert!(!missing.control.join("raw-evidence-retained.txt").exists());

        std::fs::remove_dir_all(&state).expect("remove raw-evidence fixture");
    }

    #[test]
    fn raw_retention_state_matrix_is_single_and_fail_closed() {
        let complete = TeardownProof {
            complete: true,
            quiescent: true,
        };
        let stopped_but_present = TeardownProof {
            complete: false,
            quiescent: true,
        };
        let possibly_live = TeardownProof {
            complete: false,
            quiescent: false,
        };

        assert_eq!(
            raw_retention_decision(complete, true, FinalizationPhase::Normal),
            None
        );
        assert_eq!(
            raw_retention_decision(complete, false, FinalizationPhase::Normal)
                .map(|decision| decision.0),
            Some("required-bundle-failure")
        );
        assert_eq!(
            raw_retention_decision(stopped_but_present, true, FinalizationPhase::Normal)
                .map(|decision| decision.0),
            Some("container-teardown-incomplete")
        );
        assert_eq!(
            raw_retention_decision(
                stopped_but_present,
                false,
                FinalizationPhase::PanicRecovery
            )
            .map(|decision| decision.0),
            Some("panic-recovery-bundle-failure"),
            "a failed required bundle is the primary retention cause even when stopped containers remain"
        );
        assert_eq!(
            raw_retention_decision(possibly_live, true, FinalizationPhase::SetupFailure)
                .map(|decision| decision.0),
            Some("container-quiescence-unproved"),
            "a nominal archive can never override missing quiescence proof"
        );
        assert_eq!(
            raw_retention_decision(possibly_live, false, FinalizationPhase::Normal)
                .map(|decision| decision.0),
            Some("container-quiescence-unproved")
        );
    }

    #[test]
    fn completed_capture_requires_both_exact_files_and_byte_counts() {
        let state = std::env::temp_dir().join(format!(
            "qwen38-capture-validation-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(state.join("sessions"))
            .expect("create capture-validation sessions parent");
        let paths = SessionPaths::new(&state, "s-cccccccccccccccccccccccccccccccc");
        paths
            .create_dirs()
            .expect("create capture-validation fixture");
        ensure_private_forensic_file(&paths.events_jsonl(), b"{\"type\":\"event\"}\n")
            .expect("create exact captured events");
        ensure_private_forensic_file(&paths.output.join("qwen.stderr"), b"warning\n")
            .expect("create exact captured stderr");
        if unsafe { libc::geteuid() } == 0 {
            for path in [paths.events_jsonl(), paths.output.join("qwen.stderr")] {
                std::os::unix::fs::chown(&path, Some(1000), Some(1000))
                    .expect("assign trusted capture ownership in root-run fixture");
            }
        } else {
            assert_eq!(
                unsafe { libc::geteuid() },
                1000,
                "capture metadata fixture requires either build-root or runtime uid 1000"
            );
        }
        let exact = CaptureComplete {
            events_bytes: 17,
            stderr_bytes: 8,
        };
        validate_completed_capture(&paths, &exact).expect("accept exact capture outputs");

        let wrong_count = CaptureComplete {
            events_bytes: 16,
            stderr_bytes: 8,
        };
        assert!(validate_completed_capture(&paths, &wrong_count).is_err());

        std::fs::set_permissions(
            paths.output.join("qwen.stderr"),
            std::fs::Permissions::from_mode(0o640),
        )
        .expect("drift captured stderr mode");
        assert!(validate_completed_capture(&paths, &exact).is_err());

        std::fs::remove_dir_all(&state).expect("remove capture-validation fixture");
    }
}
