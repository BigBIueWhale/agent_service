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
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::bundle;
use crate::config::Config;
use crate::docker_ops;
use crate::error::{ServiceError, ServiceResult};
use crate::progress::{ProgressCounters, ProgressPhase, ProgressReporter};
use crate::result_parse;
use crate::runtime::{
    apply_progress, merge_progress_counters, AcceptanceRecord, SessionBody, SessionStatus,
};
use crate::staging::{self, SessionPaths};
use crate::validation::ValidatedRequest;

#[derive(Clone, Copy)]
struct FailureContext<'a> {
    session_id: &'a str,
    prompt_preview: &'a str,
    max_session_turns: u32,
    archive_bytes: u64,
    archive_sha256: &'a str,
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
    ServiceRestart,
}

enum TopologyCreation {
    Created,
    Failed(ServiceError),
    Cancelled,
}

/// A setup finalizer must not infer Docker authority from a session ID alone.
/// `NeverSubmitted` is a control-flow proof that this execution never sent a
/// create request to the broker, so there is no project-owned topology to log,
/// stop, or remove. `Submitted` means creation may have committed partially or
/// completely; ownership-checked removal and an independent quiescence proof
/// are therefore mandatory. The optional locked start gate is held through
/// that serialized teardown whenever it still exists.
enum TopologyFinalization {
    NeverSubmitted,
    Submitted { start_gate: Option<std::fs::File> },
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
                FinalizationPhase::ServiceRestart => {
                    "service-restart container quiescence was not proved; bundle was forbidden"
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
            FinalizationPhase::ServiceRestart => (
                "service-restart-bundle-failure",
                "service-restart forensic bundle failure",
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
                FinalizationPhase::ServiceRestart => {
                    "incomplete service-restart container teardown despite successful bundle creation"
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
    launch_decision: Arc<Mutex<()>>,
    prompt_preview: String,
    started_at_unix: u64,
    paths: SessionPaths,
    progress: ProgressReporter,
) -> SessionBody {
    let wall_start = std::time::Instant::now();
    let failure_context = FailureContext {
        session_id,
        prompt_preview: &prompt_preview,
        max_session_turns: req.max_session_turns,
        archive_bytes: req.archive.bytes,
        archive_sha256: &req.archive.sha256,
        started_at_unix,
        wall_start,
    };

    let initial_staging = match progress.publish(
        ProgressPhase::Staging,
        "workspace-archive extraction into the staging tree started",
        ProgressCounters::default(),
    ) {
        Ok(event) => event,
        Err(error) => {
            return finalize_pre_agent_exit(
                failure_context,
                cfg,
                paths,
                error,
                SessionStatus::Completed,
                progress,
                ProgressCounters::default(),
            )
            .await;
        }
    };
    let _ = initial_staging;
    let input_archive = paths.input_archive();
    let staged_destination = paths.staged.clone();
    let staging_cancel = cancel.clone();
    let staging_progress = progress.clone();
    let staging_task = tokio::task::spawn_blocking(move || {
        let mut last_bytes = 0u64;
        let mut last_entries = 0u64;
        staging::extract_archive_into_staged_cancellable(
            &input_archive,
            &staged_destination,
            &staging_cancel,
            |observed| {
                const BYTE_STEP: u64 = 64 * 1024 * 1024;
                const ENTRY_STEP: u64 = 1024;
                let publish = observed.copied_bytes.saturating_sub(last_bytes) >= BYTE_STEP
                    || observed.copied_entries.saturating_sub(last_entries) >= ENTRY_STEP;
                if !publish {
                    return Ok(());
                }
                last_bytes = observed.copied_bytes;
                last_entries = observed.copied_entries;
                staging_progress
                    .publish(
                        ProgressPhase::Staging,
                        format!(
                            "staged {} entries ({} regular files, {} bytes)",
                            observed.copied_entries,
                            observed.copied_regular_files,
                            observed.copied_bytes
                        ),
                        ProgressCounters {
                            staged_bytes: observed.copied_bytes,
                            staged_entries: observed.copied_entries,
                            staged_regular_files: observed.copied_regular_files,
                            ..ProgressCounters::default()
                        },
                    )
                    .map(|_| ())
            },
        )
    });
    let staged = match staging_task.await {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => {
            return finalize_pre_agent_exit(
                failure_context,
                cfg,
                paths,
                error,
                if cancel.is_cancelled() {
                    SessionStatus::Cancelled
                } else {
                    SessionStatus::Completed
                },
                progress,
                ProgressCounters::default(),
            )
            .await;
        }
        Err(error) => {
            return finalize_pre_agent_exit(
                failure_context,
                cfg,
                paths,
                ServiceError::Internal(format!(
                    "descriptor-anchored blocking staging task terminated unexpectedly: {error}"
                )),
                if cancel.is_cancelled() {
                    SessionStatus::Cancelled
                } else {
                    SessionStatus::Completed
                },
                progress,
                ProgressCounters::default(),
            )
            .await;
        }
    };
    // The accepted archive's bytes now live in the staged tree; the archive
    // itself is removed immediately so session disk stays bounded and no
    // consumed upload survives as a leftover. Failure paths leave it inside
    // the session tree, whose terminal cleanup/retention owns it wholesale.
    if let Err(error) = std::fs::remove_file(paths.input_archive()) {
        return finalize_pre_agent_exit(
            failure_context,
            cfg,
            paths,
            ServiceError::Staging(format!(
                "remove consumed workspace archive after extraction: {error}"
            )),
            SessionStatus::Completed,
            progress,
            ProgressCounters {
                staged_bytes: staged.copied_bytes,
                staged_entries: staged.copied_entries,
                staged_regular_files: staged.copied_regular_files,
                ..ProgressCounters::default()
            },
        )
        .await;
    }
    let staged_bytes = staged.copied_bytes;
    let staged_entries = staged.copied_entries;
    let staged_regular_files = staged.copied_regular_files;
    let staged_counters = ProgressCounters {
        staged_bytes,
        staged_entries,
        staged_regular_files,
        ..ProgressCounters::default()
    };
    if let Err(error) = progress.publish(
        ProgressPhase::PreparingAgent,
        format!(
            "workspace-archive extraction complete: {staged_entries} entries, {staged_regular_files} regular files, {staged_bytes} bytes"
        ),
        staged_counters,
    ) {
        return finalize_pre_agent_exit(
            failure_context,
            cfg,
            paths,
            error,
            SessionStatus::Completed,
            progress,
            staged_counters,
        )
        .await;
    }
    tracing::info!(
        session_id,
        staged_bytes,
        staged_entries,
        "descriptor-anchored source copy preserved opaque symlinks and passed size, type, and read-race validation"
    );
    let start_gate = match paths.create_locked_start_gate() {
        Ok(value) => value,
        Err(error) => {
            return finalize_pre_agent_exit(
                failure_context,
                cfg,
                paths,
                error,
                SessionStatus::Completed,
                progress,
                staged_counters,
            )
            .await;
        }
    };
    if cancel.is_cancelled() {
        drop(start_gate);
        return finalize_pre_agent_exit(
            failure_context,
            cfg,
            paths,
            ServiceError::Internal("session was cancelled before Docker setup".into()),
            SessionStatus::Cancelled,
            progress,
            staged_counters,
        )
        .await;
    }

    if let Err(error) = progress.publish(
        ProgressPhase::CreatingTopology,
        "creating the exact network-none agent, model relay, and trusted capture topology behind the locked start gate",
        staged_counters,
    ) {
        // No broker create request has been submitted. Release the local gate
        // explicitly before filesystem-only finalization.
        drop(start_gate);
        return finalize_pre_agent_exit(
            failure_context,
            cfg,
            paths,
            error,
            SessionStatus::Completed,
            progress,
            staged_counters,
        )
        .await;
    }

    // The broker serializes every Docker mutation. If cancellation wins this
    // select, dropping the client connection does not imply that the broker's
    // create transaction stopped. We therefore keep the start gate locked and
    // issue remove_session on a second connection; its mutation-lock position
    // is necessarily after the create transaction, so it removes whatever
    // exact-owned topology creation committed before any task code can pass
    // the gate.
    let creation = {
        let create = docker_ops::create_session(cfg, session_id);
        tokio::pin!(create);
        tokio::select! {
            result = &mut create => match result {
                Ok(_) => TopologyCreation::Created,
                Err(error) => TopologyCreation::Failed(error),
            },
            () = cancel.cancelled() => TopologyCreation::Cancelled,
        }
    };
    match creation {
        TopologyCreation::Created => {}
        TopologyCreation::Failed(error) => {
            return finalize_started_setup_failure(
                failure_context,
                cfg,
                paths,
                error,
                SessionStatus::Completed,
                Vec::new(),
                progress,
                staged_counters,
                Some(start_gate),
            )
            .await;
        }
        TopologyCreation::Cancelled => {
            return finalize_started_setup_failure(
                failure_context,
                cfg,
                paths,
                ServiceError::Internal(
                    "durable cancellation was requested while the broker was creating the session topology"
                        .into(),
                ),
                SessionStatus::Cancelled,
                vec![
                    "durable cancellation became observable while the broker's create transaction was in flight; ownership-checked removal remained serialized behind that transaction while the agent start gate stayed locked"
                        .into(),
                ],
                progress,
                staged_counters,
                Some(start_gate),
            )
            .await;
        }
    }
    if cancel.is_cancelled() {
        return finalize_started_setup_failure(
            failure_context,
            cfg,
            paths,
            ServiceError::Internal(
                "durable cancellation was requested after topology creation and before the agent start gate was released"
                    .into(),
            ),
            SessionStatus::Cancelled,
            Vec::new(),
            progress,
            staged_counters,
            Some(start_gate),
        )
        .await;
    }
    if let Err(error) = progress.publish(
        ProgressPhase::AwaitingReadiness,
        "exact topology creation succeeded; releasing the start gate and awaiting the agent's model/tokenizer readiness proof",
        staged_counters,
    ) {
        return finalize_started_setup_failure(
            failure_context,
            cfg,
            paths,
            error,
            SessionStatus::Completed,
            Vec::new(),
            progress,
            staged_counters,
            Some(start_gate),
        )
        .await;
    }
    // Cancellation and start-gate release require a single linear order.
    // Merely checking the token immediately before unlock leaves a race in
    // which a durable cancellation can land between those two operations.
    // The API and shutdown paths hold this same fence while publishing the
    // cancellation intent and making the token observable.
    let launch_guard = launch_decision.lock().await;
    if cancel.is_cancelled() {
        drop(launch_guard);
        return finalize_started_setup_failure(
            failure_context,
            cfg,
            paths,
            ServiceError::Internal(
                "durable cancellation won the start-gate release decision".into(),
            ),
            SessionStatus::Cancelled,
            Vec::new(),
            progress,
            staged_counters,
            Some(start_gate),
        )
        .await;
    }
    if let Err(error) = std::fs::File::unlock(&start_gate) {
        // Dropping the descriptor is the only remaining way to release an
        // unexpectedly failed advisory unlock. Finalization stops the agent
        // before reading logs, so it cannot proceed into the task unnoticed.
        drop(start_gate);
        drop(launch_guard);
        let setup_error = ServiceError::Internal(format!(
            "cannot release the exact agent start gate: {error}"
        ));
        let diagnostics = vec![format!("release agent start gate: {error}")];
        return finalize_started_setup_failure(
            failure_context,
            cfg,
            paths,
            setup_error,
            SessionStatus::Completed,
            diagnostics,
            progress,
            staged_counters,
            None,
        )
        .await;
    }
    drop(start_gate);
    drop(launch_guard);
    if cancel.is_cancelled() {
        return finalize_started_setup_failure(
            failure_context,
            cfg,
            paths,
            ServiceError::Internal("session was cancelled before agent readiness".into()),
            SessionStatus::Cancelled,
            Vec::new(),
            progress,
            staged_counters,
            None,
        )
        .await;
    }

    let ready = match wait_for_agent_ready(cfg, session_id, &paths, &cancel).await {
        Ok(value) => value,
        Err(error) => {
            return finalize_started_setup_failure(
                failure_context,
                cfg,
                paths,
                error,
                if cancel.is_cancelled() {
                    SessionStatus::Cancelled
                } else {
                    SessionStatus::Completed
                },
                Vec::new(),
                progress,
                staged_counters,
                None,
            )
            .await;
        }
    };
    if let Err(error) = progress.publish(
        ProgressPhase::RunningAgent,
        format!(
            "agent readiness proved: model={}, context_window={}, tokenizer_probe_tokens={}",
            ready.model, ready.context_window, ready.token_count
        ),
        staged_counters,
    ) {
        return finalize_started_setup_failure(
            failure_context,
            cfg,
            paths,
            error,
            SessionStatus::Completed,
            Vec::new(),
            progress,
            staged_counters,
            None,
        )
        .await;
    }

    let (mut status, container_exit_code, mut diagnostics, producer_stopped) =
        wait_for_completion_or_cancel(cfg, session_id, &cancel).await;

    if let Err(error) = progress.publish(
        ProgressPhase::CapturingOutput,
        if producer_stopped {
            "agent producer stopped; awaiting the trusted capture component's durable byte-count proof"
        } else {
            "agent producer stop could not be proved; capture promotion is forbidden and teardown evidence will record the failure"
        },
        staged_counters,
    ) {
        diagnostics.push(format!("publish capture progress: {error}"));
    }
    let capture_result = if producer_stopped {
        let capture = docker_ops::wait_capture_complete(cfg, session_id);
        tokio::pin!(capture);
        let result = tokio::select! {
            result = &mut capture => result,
            () = cancel.cancelled(), if status != SessionStatus::Cancelled => {
                // The producer is already stopped, so cancellation has no
                // remaining process work to interrupt. It still wins the
                // terminal status, but mandatory capture drain must complete
                // before teardown; abandoning it here would create a
                // self-inflicted truncated transcript.
                status = SessionStatus::Cancelled;
                capture.await
            }
        };
        Some(result)
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
    let pre_teardown_observed = crate::runtime::read_running_progress(&paths.events_jsonl())
        .unwrap_or_else(|error| {
            diagnostics.push(format!(
                "read pre-teardown event progress metadata: {error}"
            ));
            crate::runtime::RunningOutputProgress::default()
        });
    let observed_counters = ProgressCounters {
        output_event_bytes: pre_teardown_observed.output_event_bytes,
        num_turns: pre_teardown_observed.num_turns,
        ..staged_counters
    };
    if let Err(error) = progress.publish(
        ProgressPhase::TearingDown,
        "removing every exact-owned session container and independently proving filesystem quiescence",
        observed_counters,
    ) {
        diagnostics.push(format!("publish teardown progress: {error}"));
    }
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
    // Subagent scopes exist only as parser output: a session whose stream
    // was never strictly parsed (cancelled before terminal, capture or parse
    // refused) reports an empty scope list, because an unparsed stream can
    // prove nothing about subagents and a fabricated row would read as
    // evidence.
    let (
        mut response,
        agent_duration_ms,
        agent_api_duration_ms,
        agent_result_subtype,
        num_turns,
        billed_main_turns,
        subagent_scopes,
        mut is_process_error,
        parsed_valid,
    ) = match parsed {
        Ok(result) => (
            result.response,
            Some(result.duration_ms),
            Some(result.api_duration_ms),
            Some(result.subtype),
            result.num_turns,
            result.billed_main_turns,
            result.scopes,
            result.is_error,
            true,
        ),
        Err(error) if status == SessionStatus::Cancelled => (
            format!("session cancelled before a terminal result event was emitted: {error}"),
            None,
            None,
            None,
            0,
            0,
            Vec::new(),
            false,
            false,
        ),
        Err(error) => {
            diagnostics.push(format!("strict event parse failed: {error}"));
            (
                format!("agent output was invalid: {error}; recent container logs:\n{logs}"),
                None,
                None,
                None,
                0,
                0,
                Vec::new(),
                true,
                false,
            )
        }
    };
    let final_observed = match crate::runtime::read_running_progress(&paths.events_jsonl()) {
        Ok(observed) => {
            // Both readers count the same thing — main-scope assistant events
            // carrying billed usage — so they must agree exactly. Comparing
            // against the terminal `num_turns` instead would compare finished
            // turns with started ones and fire on every errored run.
            if parsed_valid && observed.num_turns != billed_main_turns {
                diagnostics.push(format!(
                    "live/final turn-count mismatch: live reader observed {}, strict terminal parser observed {billed_main_turns} billed of {num_turns} started",
                    observed.num_turns
                ));
            }
            observed
        }
        Err(error) => {
            diagnostics.push(format!("read final event progress metadata: {error}"));
            pre_teardown_observed
        }
    };
    let last_event_at_unix = final_observed.last_event_at_unix;
    let final_num_turns = num_turns.max(final_observed.num_turns);
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

    let final_counters = ProgressCounters {
        output_event_bytes: final_observed.output_event_bytes,
        num_turns: final_num_turns,
        ..staged_counters
    };
    if let Err(error) = progress.publish(
        ProgressPhase::Bundling,
        "creating the deterministic no-clobber result bundle from quiescent session state",
        final_counters,
    ) {
        diagnostics.push(format!("publish bundle progress: {error}"));
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
    let (bundle_sha256, compressed, uncompressed, file_count, artifacts_count) = match bundle_result
    {
        Ok(stats) => (
            stats.sha256,
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
        !bundle_sha256.is_empty(),
        FinalizationPhase::Normal,
    )
    .map(|(cause, context)| retain_raw_evidence(&paths, cause, context, &mut diagnostics))
    .unwrap_or(false);
    if !diagnostics.is_empty() {
        is_process_error = true;
    }

    let subagent_scope_count = subagent_scopes.len() as u64;
    let subagent_error_count = subagent_scopes
        .iter()
        .filter(|scope| scope.is_error == Some(true))
        .count() as u64;
    let mut body = SessionBody {
        session_id: session_id.to_string(),
        status,
        started_at_unix,
        model: cfg.vllm_model_name.clone(),
        context_window: cfg.lock.backend.max_model_len,
        max_session_turns: req.max_session_turns,
        archive_bytes: req.archive.bytes,
        archive_sha256: req.archive.sha256.clone(),
        prompt_preview,
        progress_revision: 0,
        progress_at_unix_ms: 0,
        progress_phase: ProgressPhase::Terminal,
        progress_message: String::new(),
        staged_bytes,
        staged_entries,
        staged_regular_files,
        output_event_bytes: final_counters.output_event_bytes,
        progress_events: Vec::new(),
        num_turns: final_num_turns,
        last_event_at_unix,
        finished_at_unix: now_unix(),
        duration_wall_ms: elapsed_ms(wall_start),
        container_exit_code,
        agent_exit_code,
        is_process_error,
        response,
        agent_duration_ms,
        agent_api_duration_ms,
        agent_result_subtype,
        subagent_scopes,
        subagent_scope_count,
        subagent_error_count,
        bundle_sha256,
        bundle_compressed_bytes: compressed,
        bundle_uncompressed_bytes: uncompressed,
        bundle_file_count: file_count,
        bundle_artifacts_file_count: artifacts_count,
        raw_session_tree_retained,
        teardown_diagnostics: diagnostics,
    };
    apply_reporter_progress(&mut body, &progress);
    body
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReadyFile {
    model: String,
    context_window: u64,
    token_count: u64,
    sandbox: String,
}

async fn wait_for_agent_ready(
    cfg: &Config,
    session_id: &str,
    paths: &SessionPaths,
    cancel: &CancellationToken,
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
        sandbox: broker_ready.sandbox,
    };
    if ready.model != cfg.vllm_model_name
        || ready.context_window != cfg.lock.backend.max_model_len
        || ready.token_count == 0
        || ready.sandbox != "landlock-fs-v4-write-roots-v1+private-devpts-rw-v1+output-unmounted-v1"
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
/// an explicit terminal process error. The outer tracked supervisor calls
/// this for exactly its own session, so no other owner can overlap the
/// ownership-checked Docker cleanup, forensic bundle attempt, or persistence.
#[allow(clippy::too_many_arguments)]
pub async fn recover_after_execution_panic(
    cfg: &Config,
    session_id: &str,
    full_prompt: &str,
    prompt_preview: &str,
    max_session_turns: u32,
    archive_bytes: u64,
    archive_sha256: &str,
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

    let mut bundle_sha256 = String::new();
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
            if let Err(error) = ensure_turn_budget_record(&paths, max_session_turns) {
                diagnostics.push(format!(
                    "preserve turn budget after execution-task failure: {error}"
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
                            bundle_sha256 = stats.sha256;
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
        !bundle_sha256.is_empty(),
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
        max_session_turns,
        archive_bytes,
        archive_sha256: archive_sha256.to_string(),
        prompt_preview: prompt_preview.to_string(),
        progress_revision: 0,
        progress_at_unix_ms: 0,
        progress_phase: ProgressPhase::Terminal,
        progress_message: String::new(),
        staged_bytes: 0,
        staged_entries: 0,
        staged_regular_files: 0,
        output_event_bytes: 0,
        progress_events: Vec::new(),
        num_turns: 0,
        last_event_at_unix: 0,
        finished_at_unix: now_unix(),
        duration_wall_ms: elapsed_ms(wall_start),
        container_exit_code: -1,
        agent_exit_code: -1,
        is_process_error: true,
        response,
        agent_duration_ms: None,
        agent_api_duration_ms: None,
        agent_result_subtype: None,
        subagent_scopes: Vec::new(),
        subagent_scope_count: 0,
        subagent_error_count: 0,
        bundle_sha256,
        bundle_compressed_bytes: compressed,
        bundle_uncompressed_bytes: uncompressed,
        bundle_file_count: file_count,
        bundle_artifacts_file_count: artifacts_count,
        raw_session_tree_retained,
        teardown_diagnostics: diagnostics,
    }
}

/// Convert a durably accepted session left nonterminal by a service-process
/// restart into explicit terminal evidence.  The broker orphan sweep runs
/// first, so no abandoned container is adopted or resumed.  Existing raw
/// output is preserved, never promoted as a successful result without the
/// missing live capture/terminal proof, and bundled only after an independent
/// quiescence check.
pub async fn recover_after_service_restart(
    cfg: &Config,
    acceptance: &AcceptanceRecord,
    cancellation_was_durable: bool,
) -> ServiceResult<SessionBody> {
    let session_id = &acceptance.session_id;
    let paths = SessionPaths::new(&cfg.state_dir, session_id);
    paths.ensure_recovery_dirs()?;
    ensure_prompt_record(&paths, &acceptance.prompt)?;
    ensure_turn_budget_record(&paths, acceptance.max_session_turns)?;

    let status = if cancellation_was_durable {
        SessionStatus::Cancelled
    } else {
        SessionStatus::Completed
    };
    let response = if cancellation_was_durable {
        "the durable cancellation request survived a service restart; no successful agent result was inferred"
            .to_string()
    } else {
        "the service process restarted after accepting this session but before publishing a terminal record; abandoned work was not adopted and no successful agent result was inferred"
            .to_string()
    };
    let mut diagnostics = vec![
        "startup found accepted.json without finished.json; the exact session is being terminalized before the listener binds"
            .to_string(),
    ];

    let logs = docker_ops::session_logs(cfg, session_id)
        .await
        .unwrap_or_else(|error| {
            diagnostics.push(format!(
                "collect bounded logs during restart recovery: {error}"
            ));
            "<owned session logs unavailable after orphan sweep>\n".to_string()
        });
    let teardown = remove_and_prove_quiescent(
        cfg,
        session_id,
        "service-restart recovery",
        &mut diagnostics,
    )
    .await;

    let recovery_record = paths.control.join("service-restart-recovery.txt");
    ensure_private_forensic_file(
        &recovery_record,
        format!(
            "SERVICE_RESTART_RECOVERY\nsession_id={session_id}\naccepted_at_unix={}\ncancellation_was_durable={cancellation_was_durable}\n",
            acceptance.accepted_at_unix
        )
        .as_bytes(),
    )?;
    for (path, contents) in [
        (
            paths.output.join("ready.json"),
            b"{\"ready\":false,\"failure\":\"service restarted before terminal publication\"}\n"
                .as_slice(),
        ),
        (paths.output.join("events.jsonl"), b"".as_slice()),
        (paths.output.join("qwen.stderr"), b"".as_slice()),
        (paths.output.join("qwen-exit-code"), b"-1\n".as_slice()),
        (paths.output.join("response.txt"), response.as_bytes()),
        (paths.control.join("container-logs.txt"), logs.as_bytes()),
    ] {
        ensure_private_forensic_file(&path, contents)?;
    }

    let observed = read_running_progress_for_recovery(&paths, &mut diagnostics);
    let agent_exit_code = read_required_exit_code(&paths.output.join("qwen-exit-code"))
        .unwrap_or_else(|error| {
            diagnostics.push(format!("read restart-recovery exit code: {error}"));
            -1
        });

    let archive = cfg.results_dir.join(session_id).join("bundle.tar.zst");
    let mut bundle_sha256 = String::new();
    let mut compressed = 0;
    let mut uncompressed = 0;
    let mut file_count = 0;
    let mut artifacts_count = 0;
    match std::fs::symlink_metadata(&archive) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            diagnostics.push(
                "a bundle publication survived without its terminal counters; it is retained as unaccepted forensic evidence rather than guessed into the public result"
                    .to_string(),
            );
        }
        Ok(_) => diagnostics.push(format!(
            "restart-recovery bundle destination is not a regular non-symlink file: {}",
            archive.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if teardown.quiescent {
                match bundle::create_bundle(&paths.root, &archive).await {
                    Ok(stats) => {
                        bundle_sha256 = stats.sha256;
                        compressed = stats.compressed_bytes;
                        uncompressed = stats.uncompressed_bytes;
                        file_count = stats.file_count;
                        artifacts_count = stats.artifacts_file_count;
                    }
                    Err(bundle_error) => diagnostics.push(format!(
                        "service-restart forensic bundle failed: {bundle_error}"
                    )),
                }
            } else {
                diagnostics.push(
                    "service-restart bundle was forbidden because exact-owned container quiescence was not proved"
                        .to_string(),
                );
            }
        }
        Err(error) => diagnostics.push(format!(
            "stat restart-recovery bundle destination {}: {error}",
            archive.display()
        )),
    }
    let raw_session_tree_retained = raw_retention_decision(
        teardown,
        !bundle_sha256.is_empty(),
        FinalizationPhase::ServiceRestart,
    )
    .map(|(cause, context)| retain_raw_evidence(&paths, cause, context, &mut diagnostics))
    .unwrap_or(false);

    let finished_at_unix = now_unix();
    let duration_wall_ms = finished_at_unix
        .saturating_sub(acceptance.accepted_at_unix)
        .saturating_mul(1000);
    Ok(SessionBody {
        session_id: session_id.to_string(),
        status,
        started_at_unix: acceptance.accepted_at_unix,
        model: cfg.vllm_model_name.clone(),
        context_window: cfg.lock.backend.max_model_len,
        max_session_turns: acceptance.max_session_turns,
        archive_bytes: acceptance.archive_bytes,
        archive_sha256: acceptance.archive_sha256.clone(),
        prompt_preview: crate::runtime::preview(&acceptance.prompt),
        progress_revision: 0,
        progress_at_unix_ms: 0,
        progress_phase: ProgressPhase::Terminal,
        progress_message: String::new(),
        staged_bytes: 0,
        staged_entries: 0,
        staged_regular_files: 0,
        output_event_bytes: observed.output_event_bytes,
        progress_events: Vec::new(),
        num_turns: observed.num_turns,
        last_event_at_unix: observed.last_event_at_unix,
        finished_at_unix,
        duration_wall_ms,
        container_exit_code: -1,
        agent_exit_code,
        is_process_error: true,
        response,
        agent_duration_ms: None,
        agent_api_duration_ms: None,
        agent_result_subtype: None,
        subagent_scopes: Vec::new(),
        subagent_scope_count: 0,
        subagent_error_count: 0,
        bundle_sha256,
        bundle_compressed_bytes: compressed,
        bundle_uncompressed_bytes: uncompressed,
        bundle_file_count: file_count,
        bundle_artifacts_file_count: artifacts_count,
        raw_session_tree_retained,
        teardown_diagnostics: diagnostics,
    })
}

fn read_running_progress_for_recovery(
    paths: &SessionPaths,
    diagnostics: &mut Vec<String>,
) -> crate::runtime::RunningOutputProgress {
    crate::runtime::read_running_progress(&paths.events_jsonl()).unwrap_or_else(|error| {
        diagnostics.push(format!(
            "read service-restart event progress metadata: {error}"
        ));
        crate::runtime::RunningOutputProgress::default()
    })
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
            let existing = read_exact_owned_regular_file(
                &path,
                0o644,
                crate::config::MAX_PROMPT_BYTES as u64,
                "panic-recovery prompt record",
            )?;
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

fn ensure_turn_budget_record(paths: &SessionPaths, max_session_turns: u32) -> ServiceResult<()> {
    let path = paths.control.join("turn-budget.json");
    let expected = staging::turn_budget_record(max_session_turns);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata)
            if metadata.is_file()
                && !metadata.file_type().is_symlink()
                && metadata.uid() == 1000
                && metadata.gid() == 1000
                && metadata.permissions().mode() & 0o777 == 0o444 =>
        {
            let existing =
                read_exact_owned_regular_file(&path, 0o444, 64, "panic-recovery turn budget")?;
            if existing == expected.as_bytes() {
                Ok(())
            } else {
                Err(ServiceError::Internal(format!(
                    "existing panic-recovery turn budget {} contradicts the accepted max_session_turns={max_session_turns}",
                    path.display()
                )))
            }
        }
        Ok(metadata) => Err(ServiceError::Internal(format!(
            "{} is not the exact regular 1000:1000 mode-0444 turn-budget record: type={:?} uid={} gid={} mode={:o}",
            path.display(),
            metadata.file_type(),
            metadata.uid(),
            metadata.gid(),
            metadata.permissions().mode() & 0o777
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            paths.write_turn_budget(max_session_turns).map(|_| ())
        }
        Err(error) => Err(ServiceError::Internal(format!(
            "stat panic-recovery turn budget {}: {error}",
            path.display()
        ))),
    }
}

fn read_exact_owned_regular_file(
    path: &Path,
    mode: u32,
    max_bytes: u64,
    role: &str,
) -> ServiceResult<Vec<u8>> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| {
            ServiceError::Internal(format!(
                "open existing {role} {} without following links: {error}",
                path.display()
            ))
        })?;
    let metadata = file.metadata().map_err(|error| {
        ServiceError::Internal(format!("fstat opened {role} {}: {error}", path.display()))
    })?;
    if !metadata.is_file()
        || metadata.permissions().mode() & 0o777 != mode
        || metadata.uid() != 1000
        || metadata.gid() != 1000
        || metadata.len() > max_bytes
    {
        return Err(ServiceError::Internal(format!(
            "opened {role} {} has unsafe type/mode/owner/size",
            path.display()
        )));
    }
    let capacity = usize::try_from(metadata.len()).map_err(|_| {
        ServiceError::Internal(format!(
            "opened {role} {} is too large to address on this platform",
            path.display()
        ))
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    file.read_to_end(&mut bytes).map_err(|error| {
        ServiceError::Internal(format!("read opened {role} {}: {error}", path.display()))
    })?;
    if bytes.len() as u64 != metadata.len() {
        return Err(ServiceError::Internal(format!(
            "opened {role} {} changed length while being read",
            path.display()
        )));
    }
    Ok(bytes)
}

fn ensure_private_forensic_file(path: &Path, contents: &[u8]) -> ServiceResult<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    match std::fs::symlink_metadata(path) {
        Ok(metadata)
            if metadata.is_file()
                && !metadata.file_type().is_symlink()
                && metadata.uid() == 1000
                && metadata.gid() == 1000
                && metadata.permissions().mode() & 0o777 == 0o600 =>
        {
            Ok(())
        }
        Ok(metadata) => Err(ServiceError::Internal(format!(
            "forensic sidecar {} has unsafe type/mode/owner: type={:?} mode={:o} uid={} gid={} expected regular 0600 1000:1000",
            path.display(),
            metadata.file_type(),
            metadata.permissions().mode() & 0o777,
            metadata.uid(),
            metadata.gid()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            write_private_file(path, contents)?;
            sync_directory(
                path.parent()
                    .expect("forensic sidecar has a containing directory"),
                "sync forensic-sidecar publication",
            )
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
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    match std::fs::symlink_metadata(&paths.root) {
        Ok(metadata)
            if metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && metadata.uid() == 1000
                && metadata.gid() == 1000
                && metadata.permissions().mode() & 0o777 == 0o755 =>
        {
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
                "{context} left no safely retainable exact 1000:1000 mode-0755 ordinary session directory at {}",
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

async fn finalize_pre_agent_exit(
    context: FailureContext<'_>,
    cfg: &Config,
    paths: SessionPaths,
    error: ServiceError,
    status: SessionStatus,
    progress: ProgressReporter,
    counters: ProgressCounters,
) -> SessionBody {
    finalize_setup_failure(
        context,
        cfg,
        paths,
        error,
        status,
        Vec::new(),
        progress,
        counters,
        TopologyFinalization::NeverSubmitted,
    )
    .await
}

async fn finalize_started_setup_failure(
    context: FailureContext<'_>,
    cfg: &Config,
    paths: SessionPaths,
    error: ServiceError,
    status: SessionStatus,
    diagnostics: Vec<String>,
    progress: ProgressReporter,
    counters: ProgressCounters,
    start_gate: Option<std::fs::File>,
) -> SessionBody {
    finalize_setup_failure(
        context,
        cfg,
        paths,
        error,
        status,
        diagnostics,
        progress,
        counters,
        TopologyFinalization::Submitted { start_gate },
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn finalize_setup_failure(
    context: FailureContext<'_>,
    cfg: &Config,
    paths: SessionPaths,
    error: ServiceError,
    status: SessionStatus,
    mut diagnostics: Vec<String>,
    progress: ProgressReporter,
    counters: ProgressCounters,
    topology: TopologyFinalization,
) -> SessionBody {
    let session_id = context.session_id;
    let response = if status == SessionStatus::Cancelled {
        format!(
            "session cancellation was durably requested before normal agent execution completed: {error}"
        )
    } else {
        format!(
            "agent session failed before normal execution completed; no success was inferred: {error}"
        )
    };
    let mut process_error = status != SessionStatus::Cancelled;
    let counters = match progress.latest() {
        Ok(latest) => merge_progress_counters(latest.counters, counters),
        Err(progress_error) => {
            diagnostics.push(format!(
                "read latest progress before setup/cancellation teardown: {progress_error}"
            ));
            process_error = true;
            counters
        }
    };
    let teardown_message = match &topology {
        TopologyFinalization::NeverSubmitted => {
            "normal execution did not begin and no Docker topology was submitted; finalizing filesystem evidence"
        }
        TopologyFinalization::Submitted { .. } => {
            "normal execution did not begin or did not reach readiness; removing exact-owned topology and proving quiescence before forensic bundling"
        }
    };
    if let Err(progress_error) =
        progress.publish(ProgressPhase::TearingDown, teardown_message, counters)
    {
        diagnostics.push(format!(
            "publish setup/cancellation teardown progress: {progress_error}"
        ));
        process_error = true;
    }
    let (captured_logs, teardown) = match topology {
        TopologyFinalization::NeverSubmitted => (
            "<session topology was never submitted; no owned container logs exist>\n".to_string(),
            // This is not an optimistic Docker observation. The only code
            // that can create a session topology is below the transition to
            // `Submitted`, so the execution's bind-mounted producers never
            // existed and the session tree is quiescent by construction.
            TeardownProof {
                complete: true,
                quiescent: true,
            },
        ),
        TopologyFinalization::Submitted { start_gate } => {
            let logs = match docker_ops::session_logs(cfg, session_id).await {
                Ok(logs) => logs,
                Err(log_error) => {
                    diagnostics.push(format!("read bounded session logs: {log_error}"));
                    process_error = true;
                    "<owned session logs unavailable>\n".to_string()
                }
            };
            let teardown = remove_and_prove_quiescent(
                cfg,
                session_id,
                "setup-failure recovery",
                &mut diagnostics,
            )
            .await;
            // Holding this descriptor across the broker's serialized removal
            // is the proof that a concurrently completing create transaction
            // could not let Qwen cross into task execution. It is released
            // only after removal and the independent quiescence observation.
            drop(start_gate);
            (logs, teardown)
        }
    };
    if !teardown.complete {
        process_error = true;
    }

    if let Err(sidecar_error) = write_private_file(
        &paths.control.join("setup-failure.txt"),
        format!("{response}\n").as_bytes(),
    ) {
        diagnostics.push(format!(
            "write authoritative setup-failure record: {sidecar_error}"
        ));
        process_error = true;
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
            process_error = true;
        }
    }

    let agent_exit_code = read_required_exit_code(&paths.output.join("qwen-exit-code"))
        .unwrap_or_else(|exit_error| {
            diagnostics.push(format!("read setup-failure exit code: {exit_error}"));
            process_error = true;
            -1
        });
    let observed = match crate::runtime::read_running_progress(&paths.events_jsonl()) {
        Ok(observed) => observed,
        Err(progress_error) => {
            diagnostics.push(format!(
                "read setup-failure event progress metadata: {progress_error}"
            ));
            process_error = true;
            crate::runtime::RunningOutputProgress::default()
        }
    };

    let final_counters = merge_progress_counters(
        counters,
        ProgressCounters {
            output_event_bytes: observed.output_event_bytes,
            num_turns: observed.num_turns,
            ..ProgressCounters::default()
        },
    );
    if let Err(progress_error) = progress.publish(
        ProgressPhase::Bundling,
        "creating the deterministic forensic bundle after exact container quiescence",
        final_counters,
    ) {
        diagnostics.push(format!(
            "publish setup/cancellation bundle progress: {progress_error}"
        ));
        process_error = true;
    }

    let archive = cfg.results_dir.join(session_id).join("bundle.tar.zst");
    let bundle_result = if teardown.quiescent {
        bundle::create_bundle(&paths.root, &archive).await
    } else {
        Err(ServiceError::Internal(
            "setup-failure bundle was not attempted because exact-owned container quiescence was not proved"
                .into(),
        ))
    };
    let (bundle_sha256, compressed, uncompressed, file_count, artifacts_count) = match bundle_result
    {
        Ok(stats) => (
            stats.sha256,
            stats.compressed_bytes,
            stats.uncompressed_bytes,
            stats.file_count,
            stats.artifacts_file_count,
        ),
        Err(bundle_error) => {
            diagnostics.push(format!(
                "setup-failure forensic bundle failed: {bundle_error}"
            ));
            process_error = true;
            (String::new(), 0, 0, 0, 0)
        }
    };
    let raw_session_tree_retained = raw_retention_decision(
        teardown,
        !bundle_sha256.is_empty(),
        FinalizationPhase::SetupFailure,
    )
    .map(|(cause, context)| retain_raw_evidence(&paths, cause, context, &mut diagnostics))
    .unwrap_or(false);

    let mut body = SessionBody {
        session_id: session_id.to_string(),
        status,
        started_at_unix: context.started_at_unix,
        model: cfg.vllm_model_name.clone(),
        context_window: cfg.lock.backend.max_model_len,
        max_session_turns: context.max_session_turns,
        archive_bytes: context.archive_bytes,
        archive_sha256: context.archive_sha256.to_string(),
        prompt_preview: context.prompt_preview.to_string(),
        progress_revision: 0,
        progress_at_unix_ms: 0,
        progress_phase: ProgressPhase::Terminal,
        progress_message: String::new(),
        staged_bytes: final_counters.staged_bytes,
        staged_entries: final_counters.staged_entries,
        staged_regular_files: final_counters.staged_regular_files,
        output_event_bytes: final_counters.output_event_bytes,
        progress_events: Vec::new(),
        num_turns: observed.num_turns,
        last_event_at_unix: observed.last_event_at_unix,
        finished_at_unix: now_unix(),
        duration_wall_ms: elapsed_ms(context.wall_start),
        container_exit_code: -1,
        agent_exit_code,
        is_process_error: process_error,
        response,
        agent_duration_ms: None,
        agent_api_duration_ms: None,
        agent_result_subtype: None,
        subagent_scopes: Vec::new(),
        subagent_scope_count: 0,
        subagent_error_count: 0,
        bundle_sha256,
        bundle_compressed_bytes: compressed,
        bundle_uncompressed_bytes: uncompressed,
        bundle_file_count: file_count,
        bundle_artifacts_file_count: artifacts_count,
        raw_session_tree_retained,
        teardown_diagnostics: diagnostics,
    };
    apply_reporter_progress(&mut body, &progress);
    body
}

fn read_required_exit_code(path: &Path) -> ServiceResult<i32> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| {
            ServiceError::AgentOutputMissing(format!(
                "required exit-code file {} is missing or cannot be opened without following links: {error}",
                path.display()
            ))
        })?;
    let metadata = file.metadata().map_err(|error| {
        ServiceError::AgentOutputMissing(format!(
            "required exit-code file {} cannot be fstat'd: {error}",
            path.display()
        ))
    })?;
    if !metadata.is_file()
        || metadata.permissions().mode() & 0o777 != 0o600
        || metadata.uid() != 1000
        || metadata.gid() != 1000
        || metadata.len() == 0
        || metadata.len() > 64
    {
        return Err(ServiceError::AgentOutputMissing(format!(
            "required exit-code file {} has unsafe opened type/mode/owner/size",
            path.display()
        )));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut bytes).map_err(|error| {
        ServiceError::AgentOutputMissing(format!(
            "required exit-code file {} is unreadable through its opened descriptor: {error}",
            path.display()
        ))
    })?;
    if bytes.len() as u64 != metadata.len() {
        return Err(ServiceError::AgentOutputMissing(format!(
            "required exit-code file {} changed length while open",
            path.display()
        )));
    }
    let text = std::str::from_utf8(&bytes).map_err(|error| {
        ServiceError::AgentOutputMissing(format!(
            "required exit-code file {} is not UTF-8: {error}",
            path.display()
        ))
    })?;
    let trimmed = text.strip_suffix('\n').ok_or_else(|| {
        ServiceError::AgentOutputMissing(format!(
            "exit-code file {} must be one canonical decimal i32 followed by exactly one newline",
            path.display()
        ))
    })?;
    let value = trimmed.parse::<i32>().map_err(|error| {
        ServiceError::AgentOutputMissing(format!(
            "exit-code file {} contains {trimmed:?}, not an i32: {error}",
            path.display()
        ))
    })?;
    if text != format!("{value}\n") {
        return Err(ServiceError::AgentOutputMissing(format!(
            "exit-code file {} is not the canonical decimal representation {value} followed by one newline",
            path.display()
        )));
    }
    Ok(value)
}

fn apply_reporter_progress(body: &mut SessionBody, progress: &ProgressReporter) {
    match progress.latest() {
        Ok(event) => apply_progress(body, &event),
        Err(error) => {
            body.is_process_error = true;
            body.teardown_diagnostics
                .push(format!("read latest lifecycle progress: {error}"));
        }
    }
    match progress.events() {
        Ok(events) => body.progress_events = events,
        Err(error) => {
            body.is_process_error = true;
            body.teardown_diagnostics
                .push(format!("read complete lifecycle progress history: {error}"));
        }
    }
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
        if unsafe { libc::geteuid() } == 0 {
            std::os::unix::fs::chown(&sidecar, Some(1000), Some(1000))
                .expect("assign exact service ownership to sidecar fixture");
        } else {
            assert_eq!(unsafe { libc::geteuid() }, 1000);
            assert_eq!(unsafe { libc::getegid() }, 1000);
        }
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
        if unsafe { libc::geteuid() } == 0 {
            std::os::unix::fs::chown(&paths.root, Some(1000), Some(1000))
                .expect("assign exact service ownership to retained-tree fixture");
        } else {
            assert_eq!(unsafe { libc::geteuid() }, 1000);
            assert_eq!(unsafe { libc::getegid() }, 1000);
        }
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
        assert_eq!(
            raw_retention_decision(complete, false, FinalizationPhase::ServiceRestart)
                .map(|decision| decision.0),
            Some("service-restart-bundle-failure")
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
