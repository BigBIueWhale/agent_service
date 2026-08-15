//! One fail-closed session lifecycle.
//!
//! A session is staged, started with `--network none`, connected to the sole
//! pinned backend through the two-proxy loopback/Unix-socket path, and then
//! allowed to run without an arbitrary wall-clock or cumulative-turn cutoff.
//! Cancellation is explicit. Every setup, capture, parse, bundle, teardown,
//! and persistence-relevant failure is represented in the terminal body.

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::bundle;
use crate::config::Config;
use crate::docker_ops;
use crate::error::{ServiceError, ServiceResult};
use crate::network::IsolatedNetwork;
use crate::result_parse;
use crate::runtime::{RunningSnapshot, SessionBody, SessionStatus};
use crate::staging::{self, SessionPaths};
use crate::validation::ValidatedRequest;

#[derive(Clone, Copy)]
struct FailureContext<'a> {
    session_id: &'a str,
    prompt_preview: &'a str,
    started_at_unix: u64,
    wall_start: std::time::Instant,
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
    if let Err(error) = staging::copy_into_staged(&req.folder, &paths.staged) {
        let diagnostics = paths.remove_all();
        return early_failure(
            &mut ready_tx,
            failure_context,
            error,
            diagnostics,
            SessionStatus::Completed,
        );
    }
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

    let mut network = match IsolatedNetwork::create(cfg, session_id, &paths.proxy_sock_dir).await {
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
        let mut diagnostics = network.teardown().await;
        diagnostics.extend(paths.remove_all());
        return early_failure(
            &mut ready_tx,
            failure_context,
            ServiceError::Internal("session was cancelled before the agent started".into()),
            diagnostics,
            SessionStatus::Cancelled,
        );
    }

    let agent_name = format!("agent-{session_id}");
    if let Err(error) = start_agent(cfg, session_id, &agent_name, &paths).await {
        let mut diagnostics = network.teardown().await;
        diagnostics.extend(paths.remove_all());
        return early_failure(
            &mut ready_tx,
            failure_context,
            error,
            diagnostics,
            SessionStatus::Completed,
        );
    }

    if let Err(error) = docker_ops::verify_network_none(&agent_name).await {
        return setup_failure_after_agent(
            &mut ready_tx,
            failure_context,
            &agent_name,
            network,
            paths,
            error,
        )
        .await;
    }
    if let Err(error) = network
        .attach_inner_proxy(cfg, session_id, &agent_name)
        .await
    {
        return setup_failure_after_agent(
            &mut ready_tx,
            failure_context,
            &agent_name,
            network,
            paths,
            error,
        )
        .await;
    }

    let ready = match wait_for_agent_ready(cfg, &paths, &agent_name, &cancel).await {
        Ok(value) => value,
        Err(error) => {
            return setup_failure_after_agent(
                &mut ready_tx,
                failure_context,
                &agent_name,
                network,
                paths,
                error,
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
    };
    if let Some(sender) = ready_tx.take() {
        if sender.send(Ok(snapshot)).is_err() {
            let mut diagnostics = vec![
                "readiness receiver disappeared after successful agent preflight; cancelling the orphaned session".into(),
            ];
            if let Err(error) = docker_ops::container_stop(&agent_name, 10).await {
                diagnostics.push(format!("stop agent after lost readiness receiver: {error}"));
            }
            if let Err(error) = docker_ops::container_force_remove(&agent_name).await {
                diagnostics.push(format!(
                    "remove agent after lost readiness receiver: {error}"
                ));
            }
            diagnostics.extend(network.teardown().await);
            diagnostics.extend(paths.remove_all());
            return terminal_error(
                session_id,
                &prompt_preview,
                started_at_unix,
                wall_start,
                SessionStatus::Cancelled,
                "session requester disappeared before readiness could be delivered".into(),
                diagnostics,
            );
        }
    }

    let (status, container_exit_code, mut diagnostics) =
        wait_for_completion_or_cancel(&agent_name, &cancel).await;

    let logs = docker_ops::container_logs_tail(&agent_name, 300)
        .await
        .unwrap_or_else(|error| {
            diagnostics.push(format!("read final agent logs: {error}"));
            "<agent logs unavailable>".into()
        });
    if let Err(error) = docker_ops::container_force_remove(&agent_name).await {
        diagnostics.push(format!("remove agent container: {error}"));
    }
    diagnostics.extend(network.teardown().await);

    let agent_exit_code = read_required_exit_code(&paths.output.join("qwen-exit-code"))
        .unwrap_or_else(|error| {
            diagnostics.push(error.to_string());
            -1
        });

    let parsed = result_parse::parse_events_jsonl(&paths.events_jsonl());
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
    let bundle_result = bundle::create_bundle(&paths.root, &archive).await;
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
    diagnostics.extend(paths.remove_all());
    if !diagnostics.is_empty() {
        is_process_error = true;
    }

    SessionBody {
        session_id: session_id.to_string(),
        status,
        started_at_unix,
        model: cfg.vllm_model_name.clone(),
        context_window: cfg.lock.backend.max_model_len,
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
        teardown_diagnostics: diagnostics,
    }
}

async fn start_agent(
    cfg: &Config,
    session_id: &str,
    name: &str,
    paths: &SessionPaths,
) -> ServiceResult<()> {
    let mount = |source: &Path, target: &str, readonly: bool| -> ServiceResult<String> {
        let source = source.to_str().ok_or_else(|| {
            ServiceError::Internal(format!("non-UTF-8 bind path: {}", source.display()))
        })?;
        if source.contains(',') || source.contains(['\n', '\r', '\0']) {
            return Err(ServiceError::Internal(format!(
                "bind source cannot be safely represented in Docker --mount: {source:?}"
            )));
        }
        Ok(format!(
            "type=bind,src={source},dst={target}{}",
            if readonly { ",readonly" } else { "" }
        ))
    };

    let args = vec![
        "--name".into(),
        name.into(),
        "--label".into(),
        format!("agent_service.session={session_id}"),
        "--label".into(),
        format!("agent_service.profile={}", cfg.lock.profile),
        "--network".into(),
        "none".into(),
        "--user".into(),
        "1000:1000".into(),
        "--workdir".into(),
        "/workspace".into(),
        "--read-only".into(),
        "--tmpfs".into(),
        format!("/tmp:{}", cfg.lock.agent.tmpfs_tmp),
        "--tmpfs".into(),
        format!("/qwen-home:{}", cfg.lock.agent.tmpfs_qwen_home),
        "--tmpfs".into(),
        format!("/qwen-runtime:{}", cfg.lock.agent.tmpfs_qwen_runtime),
        "--cap-drop".into(),
        "ALL".into(),
        "--security-opt".into(),
        "no-new-privileges:true".into(),
        "--memory".into(),
        cfg.agent_memory_limit.clone(),
        "--memory-swap".into(),
        cfg.agent_memory_swap_limit.clone(),
        "--pids-limit".into(),
        cfg.lock.agent.pids_limit.to_string(),
        "--mount".into(),
        mount(&paths.staged, "/workspace", false)?,
        "--mount".into(),
        mount(&paths.artifacts, "/artifacts", false)?,
        "--mount".into(),
        mount(&paths.control, "/run/agent", true)?,
        "--mount".into(),
        mount(&paths.output, "/output", false)?,
        cfg.agent_image.clone(),
    ];
    docker_ops::run_detached(args, "start_agent")
        .await
        .map(|_| ())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadyFile {
    model: String,
    context_window: u64,
    token_count: u64,
}

async fn wait_for_agent_ready(
    cfg: &Config,
    paths: &SessionPaths,
    agent: &str,
    cancel: &CancellationToken,
) -> ServiceResult<ReadyFile> {
    let ready_path = paths.output.join("ready.json");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    loop {
        if cancel.is_cancelled() {
            return Err(ServiceError::Internal(
                "session cancelled while awaiting agent preflight".into(),
            ));
        }
        match std::fs::read(&ready_path) {
            Ok(bytes) => {
                let ready: ReadyFile = serde_json::from_slice(&bytes).map_err(|error| {
                    ServiceError::Internal(format!(
                        "agent ready file {} is malformed: {error}",
                        ready_path.display()
                    ))
                })?;
                if ready.model != cfg.vllm_model_name
                    || ready.context_window != cfg.lock.backend.max_model_len
                    || ready.token_count == 0
                {
                    return Err(ServiceError::Internal(format!(
                        "agent ready contract mismatch: {ready:?}"
                    )));
                }
                return Ok(ready);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(ServiceError::Internal(format!(
                    "cannot read agent ready file {}: {error}",
                    ready_path.display()
                )));
            }
        }
        if !docker_ops::container_running(agent).await? {
            let logs = docker_ops::container_logs_tail(agent, 300).await?;
            return Err(ServiceError::DockerCommand(format!(
                "agent exited before readiness; logs:\n{logs}"
            )));
        }
        if tokio::time::Instant::now() >= deadline {
            let logs = docker_ops::container_logs_tail(agent, 300).await?;
            return Err(ServiceError::Timeout(format!(
                "agent did not complete exact model/tokenizer preflight within 45s; logs:\n{logs}"
            )));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn wait_for_completion_or_cancel(
    agent: &str,
    cancel: &CancellationToken,
) -> (SessionStatus, i32, Vec<String>) {
    let mut diagnostics = Vec::new();
    let wait = docker_ops::container_wait(agent);
    tokio::pin!(wait);
    tokio::select! {
        result = &mut wait => match result {
            Ok(code) => (SessionStatus::Completed, code, diagnostics),
            Err(error) => {
                diagnostics.push(format!("docker wait failed: {error}"));
                (SessionStatus::Completed, -1, diagnostics)
            }
        },
        () = cancel.cancelled() => {
            if let Err(error) = docker_ops::container_stop(agent, 60).await {
                diagnostics.push(format!("graceful cancellation stop failed: {error}"));
                if let Err(remove_error) = docker_ops::container_force_remove(agent).await {
                    diagnostics.push(format!("forced cancellation remove failed: {remove_error}"));
                }
            }
            let code = match wait.await {
                Ok(value) => value,
                Err(error) => {
                    diagnostics.push(format!("docker wait after cancellation failed: {error}"));
                    -1
                }
            };
            (SessionStatus::Cancelled, code, diagnostics)
        }
    }
}

async fn setup_failure_after_agent(
    ready_tx: &mut Option<oneshot::Sender<ServiceResult<RunningSnapshot>>>,
    context: FailureContext<'_>,
    agent: &str,
    network: IsolatedNetwork,
    paths: SessionPaths,
    error: ServiceError,
) -> SessionBody {
    let mut diagnostics = Vec::new();
    match docker_ops::container_logs_tail(agent, 300).await {
        Ok(logs) if !logs.trim().is_empty() => diagnostics.push(format!("agent logs:\n{logs}")),
        Ok(_) => {}
        Err(log_error) => diagnostics.push(format!("read agent logs: {log_error}")),
    }
    if let Err(remove_error) = docker_ops::container_force_remove(agent).await {
        diagnostics.push(format!("remove failed agent: {remove_error}"));
    }
    diagnostics.extend(network.teardown().await);
    diagnostics.extend(paths.remove_all());
    early_failure(
        ready_tx,
        context,
        error,
        diagnostics,
        SessionStatus::Completed,
    )
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

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn elapsed_ms(start: std::time::Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

pub use crate::network::sweep_orphans;
