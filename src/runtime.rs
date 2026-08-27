//! Session runtime: ownership, cancellation, persistence, shutdown.
//!
//! Architecture:
//!
//! - **In-memory map** (`Inner.running`) holds **only running** sessions.
//!   Terminal sessions live exclusively on disk under
//!   `<results_dir>/<id>/finished.json`. Sessions run concurrently: capacity
//!   governance is deliberately not this service's responsibility — an
//!   operator, scheduler, or load balancer above it decides placement, and
//!   this instance cannot know it is the only one. Memory growth is bounded
//!   by the callers' live sessions; disk growth is bounded by the user
//!   (every session lives until DELETE, never auto-evicted by time or
//!   count).
//!
//! - **Supervisors** are spawned on a `TaskTracker`. Each session's
//!   supervisor is tracked through execution, panic cleanup, terminal
//!   persistence, and map eviction; `shutdown` closes the tracker and waits
//!   for it to drain, which is the strongest "no in-flight work remains"
//!   signal we have.
//!
//! - **Cancellation** uses a parent `CancellationToken` (`shutdown_token`)
//!   on the manager. Each session gets a `child_token()` of that parent.
//!   `cancel(id)` cancels only the child. `shutdown` cancels the parent,
//!   which cascades to every child. The run task observes its child token
//!   from inside `session::run_one` and tears down cleanly.
//!
//! - **Reads are non-consuming**: `get`, `list`. They never acknowledge,
//!   subscribe, cancel, or otherwise affect work. Multiple concurrent reads
//!   and retries are safe; later snapshots may naturally contain newer facts.
//!
//! - **Lifecycle writes are unambiguous**: `cancel` on a terminal session
//!   returns the current body; `delete` on a missing session returns
//!   `NotFound` (a definite "gone" rather than a silent success).
//!
//! - **Lifecycle is explicit**: `running` → terminal (`completed` |
//!   `cancelled`) → DELETE'd. There is no implicit transition; in particular,
//!   reads do not consume, and there is no time-based eviction anywhere.
//!
//! - **Connection independence**: a socket is only a short request/response
//!   transport.  The caller chooses the 256-bit session handle before POST;
//!   acceptance, progress, cancellation intent, and terminal state are
//!   durable resources.  Disconnecting never cancels work and observing
//!   progress is never required.
//!
//! - **Crash recovery**: every durably accepted but nonterminal session is
//!   converted into an explicit terminal recovery result before the service
//!   binds its listener.  Startup never silently sweeps an accepted job or
//!   leaves it as an unreadable partial directory.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, Notify};
use tokio_util::task::TaskTracker;
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::error::{io_msg, ServiceError, ServiceResult};
use crate::progress::{ProgressCounters, ProgressEvent, ProgressPhase, ProgressReporter};
use crate::session;
use crate::staging::SessionPaths;
use crate::validation::{self, ValidatedRequest};

// Durable JSON records are bounded independently of available disk/RAM so a
// corrupted service-owned path cannot make an ordinary GET allocate without
// limit. Acceptance contains at most the explicit 1 MiB prompt, one canonical
// host path, and fixed metadata. Terminal state additionally contains at most
// 4,096 progress messages of 4 KiB each plus the model's bounded final output;
// 128 MiB leaves ample structural headroom without weakening those semantic
// limits.
const MAX_ACCEPTANCE_RECORD_BYTES: u64 = (crate::config::MAX_PROMPT_BYTES as u64) + 65_536;
const MAX_TERMINAL_RECORD_BYTES: u64 = 128 * 1024 * 1024;
const MAX_CANCEL_INTENT_BYTES: u64 = 4096;
const MAX_DELETE_INTENT_BYTES: u64 = 4096;
const DELETE_INTENT_PREFIX: &str = ".delete-session-";
const DELETE_INTENT_SUFFIX: &str = ".json";

/// Wire status. Discriminator for the unioned `SessionBody` shape.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Running,
    Completed,
    Cancelled,
}

/// Single source of truth for the wire-shape body returned by **every**
/// session-related endpoint. Required-field discipline: every field is
/// always present in the JSON; running-only fields are zeroed/empty for
/// terminal states, and terminal-only fields are zeroed/empty for running.
/// Clients have one parser regardless of state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionBody {
    pub session_id: String,
    pub status: SessionStatus,

    // Carried through every state transition.
    pub started_at_unix: u64,
    pub model: String,
    pub context_window: u64,
    /// Exact per-session historical-reasoning policy. False is the default;
    /// true must have been explicitly requested as a JSON boolean.
    #[serde(default)]
    pub preserve_thinking: bool,
    /// Exact per-session turn budget this session was accepted with, and the
    /// number the launcher passed to Qwen Code, so an operator reading a
    /// finished session can tell what bound it actually ran under instead of
    /// inferring it from the deployment's current default.
    // Terminal records committed before the budget became a request field
    // could only ever have run at the locked default, so that is their one
    // semantically valid migration value. This is an explicit persisted-data
    // schema migration, not a runtime behavior fallback.
    #[serde(default = "default_recorded_max_session_turns")]
    pub max_session_turns: u32,
    /// Byte count and SHA-256 of the exact workspace archive the caller
    /// streamed over the connection for this session. Carried through every
    /// state so any later reader can re-verify which workspace bytes this
    /// session was created from.
    // Historical committed records predate the streamed-archive contract and
    // carry no archive commitment; zero/empty is their only semantically
    // valid migration value. This is an explicit persisted-data schema
    // migration, not a runtime behavior fallback.
    #[serde(default)]
    pub archive_bytes: u64,
    #[serde(default)]
    pub archive_sha256: String,
    pub prompt_preview: String,

    /// Monotonic lifecycle revision. Revision 1 is durably published before
    /// a newly accepted POST can return. It is a resource version, not a
    /// stream cursor, and reading it never consumes or acknowledges anything.
    #[serde(default)]
    pub progress_revision: u64,
    #[serde(default)]
    pub progress_at_unix_ms: u64,
    #[serde(default)]
    pub progress_phase: ProgressPhase,
    #[serde(default)]
    pub progress_message: String,
    #[serde(default)]
    pub staged_bytes: u64,
    #[serde(default)]
    pub staged_entries: u64,
    #[serde(default)]
    pub staged_regular_files: u64,
    #[serde(default)]
    pub output_event_bytes: u64,
    /// Complete durable lifecycle history.  Reading it is optional and does
    /// not subscribe, acknowledge, consume, or otherwise mutate the session.
    #[serde(default)]
    pub progress_events: Vec<ProgressEvent>,

    /// Number of distinct LLM invocations the agent has completed so far.
    /// Live for running sessions, frozen at run-end for terminal ones.
    /// Counted from completed main-thread `"type":"assistant"` messages in
    /// `events.jsonl`; Qwen Code emits one complete assistant message per
    /// model invocation when partial-message output is disabled.
    pub num_turns: u64,
    pub last_event_at_unix: u64,

    // Populated on transition to terminal. Zeroed/empty while running.
    pub finished_at_unix: u64,
    pub duration_wall_ms: u64,
    /// Exit code reported by `docker wait` for the agent container. The
    /// wrapper propagates Qwen Code's exit status after durably capturing
    /// output, so this should equal `agent_exit_code` on an ordinary run.
    pub container_exit_code: i32,
    /// Exit code reported by qwen-code itself, read from the
    /// `output/qwen-exit-code` file the wrapper writes immediately before
    /// the wrapper terminates. -1 if the file is missing or unparseable
    /// (e.g. setup failed before the wrapper ran). Common values:
    ///   0   normal completion
    ///   53  hit `--max-session-turns` (qwen-code "Reached max session turns")
    ///   137 SIGKILL'd by `docker stop` after a cancel
    ///   other non-zero: qwen-code internal error; see events.jsonl
    pub agent_exit_code: i32,
    /// True iff the qwen-code process itself terminated abnormally
    /// (structured error envelope, mid-run crash, or setup failure
    /// before model/tokenizer readiness). **Does not mean "the response is useful"**: a
    /// vLLM 400 that becomes the agent's final answer leaves this
    /// false. Inspect `response` for wire-error envelopes if needed.
    pub is_process_error: bool,
    pub response: String,
    pub agent_duration_ms: u64,
    /// SHA-256 of the published `bundle.tar.zst`, computed at bundle
    /// acceptance. Empty exactly when no bundle was accepted; the bundle
    /// itself is retrieved over the connection from the bundle endpoint,
    /// never through a shared filesystem path.
    // Historical committed records published a server-local
    // `bundle_archive_path` instead of a content hash; empty is the only
    // semantically valid migration value for them. This is an explicit
    // persisted-data schema migration, not a runtime behavior fallback.
    #[serde(default)]
    pub bundle_sha256: String,
    pub bundle_compressed_bytes: u64,
    pub bundle_uncompressed_bytes: u64,
    pub bundle_file_count: u64,
    pub bundle_artifacts_file_count: u64,
    /// True when the exact raw state/session tree was deliberately retained
    /// for forensic recovery because required archive creation failed or the
    /// broker could not prove complete container teardown. In the latter case
    /// a valid accepted bundle may coexist with the retained raw tree.
    /// DELETE removes that retained tree before deleting the terminal record.
    // Historical accepted v12 records predate this field and always deleted
    // their raw tree even when bundling failed. Their only semantically valid
    // migration value is therefore false; this is an explicit persisted-data
    // schema migration, not a runtime behavior fallback.
    #[serde(default)]
    pub raw_session_tree_retained: bool,
    pub teardown_diagnostics: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct RunningSnapshot {
    pub session_id: String,
    pub started_at_unix: u64,
    pub prompt_preview: String,
    pub model: String,
    pub context_window: u64,
    pub preserve_thinking: bool,
    pub max_session_turns: u32,
    pub archive_bytes: u64,
    pub archive_sha256: String,
}

/// The turn budget carried by a record written before the budget was a
/// request field. Those sessions ran under the sole compiled constant of
/// their day, which is this deployment's default.
fn default_recorded_max_session_turns() -> u32 {
    crate::config::DEFAULT_MAX_SESSION_TURNS
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptanceRecord {
    /// Version 2 records the streamed-archive commitment. Version 1 recorded
    /// a shared-filesystem `folder` path, but that transport predates durable
    /// acceptance records in result directories, so version 2 is the only
    /// version that can exist on disk. The record persists for the resource's
    /// whole lifetime: terminal reads of current 256-bit handles cross-check
    /// it against the published terminal body.
    pub schema_version: u32,
    pub session_id: String,
    pub accepted_at_unix: u64,
    pub archive_bytes: u64,
    pub archive_sha256: String,
    pub prompt: String,
    pub preserve_thinking: bool,
    // Acceptance records committed before the budget became a request field
    // were accepted under the locked default, which is their one
    // semantically valid migration value. Version 2 stays the only version
    // that can exist on disk: this adds a field to that shape, it does not
    // define a new record.
    #[serde(default = "default_recorded_max_session_turns")]
    pub max_session_turns: u32,
}

impl AcceptanceRecord {
    fn from_request(session_id: &str, accepted_at_unix: u64, req: &ValidatedRequest) -> Self {
        Self {
            schema_version: 2,
            session_id: session_id.to_string(),
            accepted_at_unix,
            archive_bytes: req.archive.bytes,
            archive_sha256: req.archive.sha256.clone(),
            prompt: req.prompt.clone(),
            preserve_thinking: req.preserve_thinking,
            max_session_turns: req.max_session_turns,
        }
    }

    fn matches_wire(
        &self,
        prompt: &str,
        archive_bytes: u64,
        archive_sha256: &str,
        preserve_thinking: bool,
        max_session_turns: u32,
    ) -> bool {
        self.schema_version == 2
            && self.archive_bytes == archive_bytes
            && self.archive_sha256 == archive_sha256
            && self.prompt == prompt
            && self.preserve_thinking == preserve_thinking
            && self.max_session_turns == max_session_turns
    }
}

pub struct SubmitOutcome {
    pub body: SessionBody,
    pub newly_accepted: bool,
}

struct AcceptancePreparation {
    progress: ProgressReporter,
    /// Returned to this particular POST only after the detached supervisor
    /// has taken ownership. A same-handle retry remains a pure lookup.
    response_error: Option<ServiceError>,
}

struct CancelIntentPublication {
    response_error: Option<ServiceError>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct DeleteIntent {
    schema_version: u32,
    session_id: String,
}

/// In-memory entry for a running session. Removed from the map by the run
/// task on transition-to-terminal (after `finished.json` has been
/// successfully persisted). After removal, every observer falls back to
/// the on-disk record.
struct RunningEntry {
    snapshot: RunningSnapshot,
    acceptance: AcceptanceRecord,
    progress: ProgressReporter,
    /// Child of `Manager.shutdown_token`. `cancel(id)` cancels this child;
    /// `shutdown` cancels the parent, which cascades to every child.
    cancel: CancellationToken,
    /// Linearizes release of the agent start gate against durable
    /// cancellation. If cancellation owns this fence first, the gate stays
    /// locked through teardown. If release owns it first, cancellation is
    /// unambiguously stopping an already-launched agent.
    launch_decision: Arc<Mutex<()>>,
    /// Linearizes explicit cancellation against selection of the terminal
    /// outcome. The mutex is held only for the short decision transaction,
    /// never while bundling, deleting raw state, or fsyncing the terminal
    /// record. This keeps cancellation connections disposable and bounded.
    terminal_decision: Mutex<TerminalDecision>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TerminalDecision {
    Open,
    Finalizing,
}

enum SessionResolution {
    Running(Arc<RunningEntry>),
    MemoryTerminal(SessionBody),
    DiskTerminal,
}

#[derive(Clone)]
pub struct Manager {
    cfg: Arc<Config>,
    inner: Arc<Mutex<Inner>>,
    /// Serializes the short acceptance transaction so two
    /// retries of the same idempotency key cannot race each other between
    /// the in-memory and durable collision checks.
    admission: Arc<Mutex<()>>,
    /// Every session supervisor is spawned on this tracker and runs through
    /// execution, cleanup, persistence, and map eviction under it.
    /// `shutdown()` closes the tracker and waits for it to drain, so a
    /// successful return guarantees no in-flight session or
    /// supervisor-owned cleanup remains.
    supervisors: TaskTracker,
    /// Cancelled at the top of `shutdown`. `submit` checks this first, so
    /// post-shutdown submits fail fast. Each session's per-session cancel
    /// token is a child of this one.
    shutdown_token: CancellationToken,
    /// Counts the short server-owned mutation tasks that outlive their HTTP
    /// transports. Shutdown closes this tracker before taking its lifecycle
    /// snapshot, drains every task admitted before that close, and thereby
    /// prevents a disconnected DELETE (or acceptance/cancellation transaction)
    /// from being killed by otherwise-clean process exit.
    lifecycle: Arc<LifecycleTracker>,
}

struct Inner {
    running: HashMap<String, Arc<RunningEntry>>,
    unpersisted_terminal: HashMap<String, SessionBody>,
}

struct LifecycleState {
    open: bool,
    active: usize,
}

struct LifecycleTracker {
    state: std::sync::Mutex<LifecycleState>,
    idle: Notify,
}

struct LifecycleGuard {
    tracker: Arc<LifecycleTracker>,
}

impl LifecycleTracker {
    fn new() -> Self {
        Self {
            state: std::sync::Mutex::new(LifecycleState {
                open: true,
                active: 0,
            }),
            idle: Notify::new(),
        }
    }

    fn start(self: &Arc<Self>) -> ServiceResult<LifecycleGuard> {
        let mut state = self.state.lock().map_err(|_| {
            ServiceError::Internal("server-owned lifecycle tracker mutex was poisoned".into())
        })?;
        if !state.open {
            return Err(ServiceError::ServiceShuttingDown);
        }
        state.active = state.active.checked_add(1).ok_or_else(|| {
            ServiceError::Internal("server-owned lifecycle task count overflowed usize".into())
        })?;
        Ok(LifecycleGuard {
            tracker: Arc::clone(self),
        })
    }

    fn close(&self) -> ServiceResult<()> {
        let mut state = self.state.lock().map_err(|_| {
            ServiceError::Internal("server-owned lifecycle tracker mutex was poisoned".into())
        })?;
        state.open = false;
        if state.active == 0 {
            self.idle.notify_waiters();
        }
        Ok(())
    }

    async fn wait_idle(&self) -> ServiceResult<()> {
        loop {
            // Register before observing the count so the final guard cannot
            // notify in the gap between the check and awaiting notification.
            let notified = self.idle.notified();
            let active = self
                .state
                .lock()
                .map_err(|_| {
                    ServiceError::Internal(
                        "server-owned lifecycle tracker mutex was poisoned".into(),
                    )
                })?
                .active;
            if active == 0 {
                return Ok(());
            }
            notified.await;
        }
    }
}

impl Drop for LifecycleGuard {
    fn drop(&mut self) {
        let mut state = match self.tracker.state.lock() {
            Ok(state) => state,
            Err(poisoned) => {
                tracing::error!(
                    "server-owned lifecycle tracker mutex was poisoned while releasing a task; recovering the contained count so shutdown cannot hang"
                );
                poisoned.into_inner()
            }
        };
        if state.active == 0 {
            tracing::error!(
                "server-owned lifecycle task count was already zero while releasing a guard"
            );
            self.tracker.idle.notify_waiters();
            return;
        }
        state.active -= 1;
        if state.active == 0 {
            self.tracker.idle.notify_waiters();
        }
    }
}

/// Await a task whose lifetime is owned by the service runtime rather than by
/// the awaiting transport. Tokio intentionally detaches a task when its
/// JoinHandle is dropped, so cancellation of this helper's caller cannot
/// cancel `future`. A panic is still an explicit service error when a caller
/// remains to observe it; production submission code has no await between its
/// durable-acceptance commit point and supervisor spawn.
async fn await_connection_independent<T, F>(future: F, operation: String) -> ServiceResult<T>
where
    T: Send + 'static,
    F: std::future::Future<Output = ServiceResult<T>> + Send + 'static,
{
    let join_operation = operation.clone();
    tokio::spawn(async move {
        let result = future.await;
        if let Err(error) = &result {
            tracing::warn!(
                operation = %operation,
                error = %error,
                "server-owned lifecycle task failed"
            );
        }
        result
    })
    .await
    .map_err(|error| {
        ServiceError::Internal(format!(
            "server-owned task panicked or was aborted while attempting to {join_operation}: {error}"
        ))
    })?
}

/// Before the listener binds, terminalize every request whose durable
/// acceptance survived a prior service process but whose terminal record did
/// not. The broker orphan sweep must run first. Recovery never resumes an
/// abandoned container or guesses success from partial output.
pub async fn recover_interrupted_acceptances(cfg: &Config) -> ServiceResult<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let mut entries = std::fs::read_dir(&cfg.results_dir)
        .map_err(|error| {
            ServiceError::Internal(io_msg(
                "restart recovery: read results directory",
                &cfg.results_dir,
                &error,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            ServiceError::Internal(io_msg(
                "restart recovery: read results entry",
                &cfg.results_dir,
                &error,
            ))
        })?;
    entries.sort_by_key(std::fs::DirEntry::file_name);

    for entry in entries {
        let path = entry.path();
        let name = entry.file_name().into_string().map_err(|_| {
            ServiceError::Internal(format!(
                "restart recovery: non-UTF-8 result entry at {}",
                path.display()
            ))
        })?;
        if !is_safe_session_id(&name) {
            // The existing general result sweep owns diagnostics for paths
            // that are not session resources.
            continue;
        }
        let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
            ServiceError::Internal(io_msg(
                "restart recovery: stat accepted result directory",
                &path,
                &error,
            ))
        })?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata.permissions().mode() & 0o777 != 0o755
            || metadata.uid() != 1000
            || metadata.gid() != 1000
        {
            return Err(ServiceError::Internal(format!(
                "restart recovery: accepted result directory {} has unsafe type/mode/owner",
                path.display()
            )));
        }
        let accepted = acceptance_path(&cfg.results_dir, &name);
        if !path_entry_exists(
            &accepted,
            "restart recovery: stat possible acceptance record",
        )? {
            if is_exact_uncommitted_acceptance(&path, &name)? {
                remove_exact_uncommitted_acceptance(cfg, &path, &name)?;
            }
            continue;
        }
        let acceptance = read_acceptance(&cfg.results_dir, &name)?;
        let progress = ProgressReporter::open(&progress_path(&cfg.results_dir, &name), &name)?;
        reconcile_unpublished_cancel_intent(&cfg.results_dir, &name)?;
        if path_entry_exists(
            &path.join("finished.json"),
            "restart recovery: stat terminal publication",
        )? {
            // Startup reconciliation above still owns crash-left `.next`
            // cleanup for an already terminal accepted resource. The general
            // terminal sweep below validates a possible same-inode private
            // publication link and removes only that redundant name.
            continue;
        }
        if path_entry_exists(
            &path.join("finished.json.tmp"),
            "restart recovery: stat terminal publication draft",
        )? {
            resume_prepared_terminal_transaction(cfg, &name).await?;
            tracing::warn!(
                session_id = name,
                "resumed the exact cleanup/retention/publication phases for a crash-left private terminal draft"
            );
            continue;
        }

        let cancelled = read_cancel_intent(&cfg.results_dir, &name)?.is_some();
        let prior = progress.latest()?;
        progress.publish(
            ProgressPhase::TearingDown,
            "service restart recovery began after the broker orphan sweep; no abandoned container will be adopted or resumed",
            prior.counters,
        )?;
        let mut body =
            session::recover_after_service_restart(cfg, &acceptance, cancelled).await?;
        let counters = merge_progress_counters(prior.counters, ProgressCounters {
            staged_bytes: body.staged_bytes,
            staged_entries: body.staged_entries,
            staged_regular_files: body.staged_regular_files,
            output_event_bytes: body.output_event_bytes,
            num_turns: body.num_turns,
        });
        progress.publish(
            ProgressPhase::PersistingTerminal,
            "restart recovery completed quiescence and forensic bundle handling; preparing the no-clobber terminal resource",
            counters,
        )?;
        let terminal = progress.publish(
            ProgressPhase::Terminal,
            if cancelled {
                "the durable cancellation intent is terminal after service-restart recovery; evidence handling is complete"
            } else {
                "the accepted operation is terminal with an explicit service-restart process error; evidence handling is complete"
            },
            counters,
        )?;
        apply_progress(&mut body, &terminal);
        body.progress_events = progress.events()?;
        persist_terminal_transaction(cfg, &mut body).await?;
        tracing::warn!(
            session_id = name,
            cancelled,
            "terminalized a durably accepted session interrupted by a prior service process"
        );
    }
    Ok(())
}

/// Recognize only the exact pre-commit footprint created by
/// `prepare_durable_acceptance`. Anything else remains evidence for the
/// general fail-closed sweep.
fn is_exact_uncommitted_acceptance(result_dir: &Path, session_id: &str) -> ServiceResult<bool> {
    use std::collections::BTreeSet;

    let mut names = BTreeSet::new();
    for entry in std::fs::read_dir(result_dir).map_err(|error| {
        ServiceError::Internal(io_msg(
            "inspect possible uncommitted acceptance",
            result_dir,
            &error,
        ))
    })? {
        let entry = entry.map_err(|error| {
            ServiceError::Internal(io_msg(
                "read possible uncommitted acceptance entry",
                result_dir,
                &error,
            ))
        })?;
        let name = entry.file_name().into_string().map_err(|_| {
            ServiceError::Internal(format!(
                "possible uncommitted acceptance {} has a non-UTF-8 entry",
                result_dir.display()
            ))
        })?;
        names.insert(name);
    }
    let progress_only = BTreeSet::from(["progress.json".to_string()]);
    let with_request = BTreeSet::from([
        "accepted.json.next".to_string(),
        "progress.json".to_string(),
    ]);
    if names != progress_only && names != with_request {
        return Ok(false);
    }
    crate::progress::read_progress_events(&result_dir.join("progress.json"), session_id)?;
    if names == with_request {
        let next = result_dir.join("accepted.json.next");
        let record = read_acceptance_file(&next, session_id, "unpublished acceptance record")?;
        validate_uncommitted_state_request(result_dir, &record)?;
    }
    Ok(true)
}

fn validate_uncommitted_state_request(
    result_dir: &Path,
    acceptance: &AcceptanceRecord,
) -> ServiceResult<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    // The result directory is under `<runtime>/results`; its sibling state
    // tree is derived by the caller during removal. Here we only validate the
    // record itself. Full state metadata and byte agreement are validated in
    // `remove_exact_uncommitted_acceptance`, where Config is available.
    let metadata = std::fs::symlink_metadata(result_dir).map_err(|error| {
        ServiceError::Internal(io_msg(
            "stat uncommitted result directory",
            result_dir,
            &error,
        ))
    })?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o777 != 0o755
        || metadata.uid() != 1000
        || metadata.gid() != 1000
        || acceptance.prompt.is_empty()
    {
        return Err(ServiceError::Internal(format!(
            "uncommitted acceptance {} failed its exact directory/request validation",
            acceptance.session_id
        )));
    }
    Ok(())
}

fn remove_exact_uncommitted_acceptance(
    cfg: &Config,
    result_dir: &Path,
    session_id: &str,
) -> ServiceResult<()> {

    let state = SessionPaths::new(&cfg.state_dir, session_id);
    match std::fs::symlink_metadata(&state.root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(ServiceError::Internal(io_msg(
                "stat uncommitted raw session tree",
                &state.root,
                &error,
            )));
        }
        Ok(_) => {
            // Validate, never repair or adopt, every fixed directory.
            state.ensure_recovery_dirs()?;
            let accepted_next = result_dir.join("accepted.json.next");
            let record = if path_entry_exists(
                &accepted_next,
                "stat uncommitted acceptance before state cleanup",
            )? {
                Some(read_acceptance_file(
                    &accepted_next,
                    session_id,
                    "uncommitted acceptance before state cleanup",
                )?)
            } else {
                None
            };
            validate_exact_uncommitted_state_tree(&state, record.as_ref())?;
            let diagnostics =
                remove_terminalized_state(&cfg.state_dir, session_id, 1000, 1000);
            if !diagnostics.is_empty() {
                return Err(ServiceError::Internal(format!(
                    "remove exact uncommitted state for {session_id}: {}",
                    diagnostics.join("; ")
                )));
            }
        }
    }
    std::fs::remove_dir_all(result_dir).map_err(|error| {
        ServiceError::Internal(io_msg(
            "remove exact uncommitted result directory",
            result_dir,
            &error,
        ))
    })?;
    sync_directory(
        &cfg.results_dir,
        "sync exact uncommitted acceptance cleanup",
    )?;
    tracing::warn!(
        session_id,
        "removed an exact pre-commit acceptance footprint; no POST could have returned success for it"
    );
    Ok(())
}

pub(crate) fn validate_exact_uncommitted_state_tree(
    paths: &SessionPaths,
    acceptance: Option<&AcceptanceRecord>,
) -> ServiceResult<()> {
    use std::collections::BTreeSet;

    let names = |directory: &Path| -> ServiceResult<BTreeSet<String>> {
        std::fs::read_dir(directory)
            .map_err(|error| {
                ServiceError::Internal(io_msg(
                    "read exact uncommitted state directory",
                    directory,
                    &error,
                ))
            })?
            .map(|entry| {
                entry
                    .map_err(|error| {
                        ServiceError::Internal(io_msg(
                            "read exact uncommitted state entry",
                            directory,
                            &error,
                        ))
                    })?
                    .file_name()
                    .into_string()
                    .map_err(|_| {
                        ServiceError::Internal(format!(
                            "uncommitted state directory {} has a non-UTF-8 entry",
                            directory.display()
                        ))
                    })
            })
            .collect()
    };
    let expected_root = BTreeSet::from([
        "artifacts".to_string(),
        "control".to_string(),
        "output".to_string(),
        "staged".to_string(),
        "streams".to_string(),
    ]);
    if names(&paths.root)? != expected_root {
        return Err(ServiceError::Internal(format!(
            "uncommitted state root {} contains entries outside the exact pre-commit layout",
            paths.root.display()
        )));
    }
    for directory in [&paths.staged, &paths.artifacts, &paths.streams, &paths.output] {
        if !names(directory)?.is_empty() {
            return Err(ServiceError::Internal(format!(
                "uncommitted state directory {} is nonempty; refusing destructive cleanup",
                directory.display()
            )));
        }
    }
    let expected_control = BTreeSet::from([
        "history-policy.json".to_string(),
        "prompt.txt".to_string(),
        "turn-budget.json".to_string(),
    ]);
    if names(&paths.control)? != expected_control {
        return Err(ServiceError::Internal(format!(
            "uncommitted control directory {} differs from the exact pre-commit layout",
            paths.control.display()
        )));
    }
    let prompt_path = paths.control.join("prompt.txt");
    let history_path = paths.control.join("history-policy.json");
    let turn_budget_path = paths.control.join("turn-budget.json");
    let prompt = validate_uncommitted_regular_file(&prompt_path, 0o644, 1_048_576)?;
    let history = validate_uncommitted_regular_file(&history_path, 0o444, 64)?;
    let turn_budget = validate_uncommitted_regular_file(&turn_budget_path, 0o444, 64)?;
    if prompt.is_empty() || prompt.len() > 1_048_576 {
        return Err(ServiceError::Internal(format!(
            "uncommitted prompt {} has invalid byte length {}",
            prompt_path.display(),
            prompt.len()
        )));
    }
    if history != b"{\"preserve_thinking\":false}\n"
        && history != b"{\"preserve_thinking\":true}\n"
    {
        return Err(ServiceError::Internal(format!(
            "uncommitted history policy {} is not canonical",
            history_path.display()
        )));
    }
    if crate::staging::parse_turn_budget_record(&turn_budget).is_none() {
        return Err(ServiceError::Internal(format!(
            "uncommitted turn budget {} is not canonical",
            turn_budget_path.display()
        )));
    }
    if let Some(acceptance) = acceptance {
        let expected_history: &[u8] = if acceptance.preserve_thinking {
            b"{\"preserve_thinking\":true}\n"
        } else {
            b"{\"preserve_thinking\":false}\n"
        };
        let expected_turn_budget =
            crate::staging::turn_budget_record(acceptance.max_session_turns);
        if prompt != acceptance.prompt.as_bytes()
            || history != expected_history
            || turn_budget != expected_turn_budget.as_bytes()
        {
            return Err(ServiceError::Internal(format!(
                "uncommitted state controls for {} do not exactly match accepted.json.next",
                acceptance.session_id
            )));
        }
    }
    Ok(())
}

fn validate_uncommitted_regular_file(
    path: &Path,
    mode: u32,
    max_bytes: u64,
) -> ServiceResult<Vec<u8>> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| {
            ServiceError::Internal(io_msg(
                "open uncommitted control file without following links",
                path,
                &error,
            ))
        })?;
    let metadata = file.metadata().map_err(|error| {
        ServiceError::Internal(io_msg(
            "fstat opened uncommitted control file",
            path,
            &error,
        ))
    })?;
    if !metadata.is_file()
        || metadata.permissions().mode() & 0o777 != mode
        || metadata.uid() != 1000
        || metadata.gid() != 1000
        || metadata.len() > max_bytes
    {
        return Err(ServiceError::Internal(format!(
            "uncommitted control file {} has unsafe opened type/mode/owner/size",
            path.display()
        )));
    }
    let capacity = usize::try_from(metadata.len()).map_err(|_| {
        ServiceError::Internal(format!(
            "uncommitted control file {} is too large to address on this platform",
            path.display()
        ))
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    file.read_to_end(&mut bytes).map_err(|error| {
        ServiceError::Internal(io_msg(
            "read opened uncommitted control file",
            path,
            &error,
        ))
    })?;
    if bytes.len() as u64 != metadata.len() {
        return Err(ServiceError::Internal(format!(
            "uncommitted control file {} changed length while open: fstat={} read={}",
            path.display(),
            metadata.len(),
            bytes.len()
        )));
    }
    Ok(bytes)
}

impl Manager {
    pub fn new(cfg: Arc<Config>) -> Self {
        Self {
            cfg,
            inner: Arc::new(Mutex::new(Inner {
                running: HashMap::new(),
                unpersisted_terminal: HashMap::new(),
            })),
            admission: Arc::new(Mutex::new(())),
            supervisors: TaskTracker::new(),
            shutdown_token: CancellationToken::new(),
            lifecycle: Arc::new(LifecycleTracker::new()),
        }
    }

    /// Transfer the complete submission to a server-owned task immediately.
    /// Dropping the caller's future (including because an HTTP peer vanished)
    /// drops only its JoinHandle; Tokio keeps the task running. The inner
    /// acceptance routine in turn publishes the durable resource and starts
    /// its long-lived supervisor without an await between those two actions.
    ///
    /// Errors:
    /// - `Internal("server is shutting down …")` if shutdown has begun.
    /// - Any failure before the durable acceptance boundary.
    ///
    /// There is deliberately no serving-capacity gate here: whether more
    /// than one session should run at once is a placement decision that
    /// belongs above this service, which cannot know it is not one worker
    /// behind a load balancer.
    pub async fn submit(
        &self,
        session_id: String,
        prompt: String,
        preserve_thinking: bool,
        max_session_turns: u32,
        archive: crate::validation::SpooledArchive,
    ) -> ServiceResult<SubmitOutcome> {
        let lifecycle = self.lifecycle.start()?;
        let manager = self.clone();
        let operation = format!("accept caller-known operation {session_id}");
        await_connection_independent(
            async move {
                let _lifecycle = lifecycle;
                manager
                    .submit_server_owned(
                        session_id,
                        prompt,
                        preserve_thinking,
                        max_session_turns,
                        archive,
                    )
                    .await
            },
            operation,
        )
        .await
    }

    /// Durably accept a new session and return immediately. Source staging,
    /// container creation, model/tokenizer readiness, execution, capture,
    /// teardown, bundling, and terminal publication are owned by the detached
    /// supervisor created below.
    async fn submit_server_owned(
        &self,
        session_id: String,
        prompt: String,
        preserve_thinking: bool,
        max_session_turns: u32,
        archive: crate::validation::SpooledArchive,
    ) -> ServiceResult<SubmitOutcome> {
        if !is_current_session_id(&session_id) {
            return Err(ServiceError::InvalidRequest(format!(
                "Idempotency-Key {session_id:?} is not the required `s-` plus 64 lowercase hexadecimal characters generated from 32 CSPRNG bytes"
            )));
        }
        let _admission = self.admission.lock().await;
        if self.shutdown_token.is_cancelled() {
            return Err(ServiceError::ServiceShuttingDown);
        }
        if delete_intent_exists(&self.cfg.results_dir, &session_id)? {
            return Err(ServiceError::SessionDeleting { session_id });
        }

        // A transport retry with the same caller-known handle is a pure
        // lookup, not a second operation.  Byte-identical semantic inputs
        // recover the existing resource; any mismatch is an explicit 409.
        // Bound before `if let` so the map guard cannot outlive the lookup;
        // an `if let` scrutinee guard would survive the whole success block
        // in edition 2021 and re-locking `inner` there would self-deadlock.
        let replayed_entry = self.inner.lock().await.running.get(&session_id).cloned();
        if let Some(entry) = replayed_entry {
            require_matching_acceptance(
                &entry.acceptance,
                &prompt,
                archive.bytes,
                &archive.sha256,
                preserve_thinking,
                max_session_turns,
            )?;
            return Ok(SubmitOutcome {
                body: running_body_for_entry(&self.cfg, &entry)?,
                newly_accepted: false,
            });
        }
        if acceptance_exists(&self.cfg.results_dir, &session_id)? {
            let acceptance = read_acceptance(&self.cfg.results_dir, &session_id)?;
            require_matching_acceptance(
                &acceptance,
                &prompt,
                archive.bytes,
                &archive.sha256,
                preserve_thinking,
                max_session_turns,
            )?;
            return Ok(SubmitOutcome {
                body: self.get_unfenced(&session_id).await.map_err(|error| {
                    ServiceError::Internal(format!(
                        "idempotency record for {session_id} exists but its session resource is not readable: {error}"
                    ))
                })?,
                newly_accepted: false,
            });
        }

        // Only a genuinely new operation proves the spooled archive against
        // the request and staging contracts. A network retry for a durably
        // accepted handle is answered above from the acceptance commitment
        // alone, without touching any spool. Central-directory parsing is
        // bounded but real file work, so it runs on the blocking pool before
        // the deliberately await-free acceptance window below.
        let validation_prompt = prompt.clone();
        let req = tokio::task::spawn_blocking(move || {
            let req = validation::validate(
                &validation_prompt,
                preserve_thinking,
                max_session_turns,
                archive,
            )?;
            crate::staging::validate_archive_structure(&req.archive.path).map(drop)?;
            Ok::<_, ServiceError>(req)
        })
        .await
        .map_err(|error| {
            ServiceError::Internal(format!(
                "blocking archive-validation task terminated unexpectedly: {error}"
            ))
        })??;
        tracing::info!(
            session_id,
            archive_bytes = req.archive.bytes,
            archive_sha256 = %req.archive.sha256,
            preserve_thinking = req.preserve_thinking,
            max_session_turns = req.max_session_turns,
            "new operation passed archive-commitment and archive-structure validation"
        );

        let session_cancel = self.shutdown_token.child_token();
        let prompt_preview = preview(&req.prompt);
        let preserve_thinking = req.preserve_thinking;
        let started_at_unix = unix_now();
        let snapshot = RunningSnapshot {
            session_id: session_id.clone(),
            started_at_unix,
            prompt_preview: prompt_preview.clone(),
            model: self.cfg.vllm_model_name.clone(),
            context_window: self.cfg.lock.backend.max_model_len,
            preserve_thinking,
            max_session_turns: req.max_session_turns,
            archive_bytes: req.archive.bytes,
            archive_sha256: req.archive.sha256.clone(),
        };
        let acceptance = AcceptanceRecord::from_request(&session_id, started_at_unix, &req);
        let paths = SessionPaths::new(&self.cfg.state_dir, &session_id);

        // Take the last asynchronous lock before publishing acceptance. From
        // the first visible accepted.json byte through map registration and
        // supervisor spawn there is deliberately no `.await`; cancellation of
        // the server-owned future cannot strand a committed operation in an
        // accepted-but-unowned state.
        let mut inner = self.inner.lock().await;
        if inner.running.contains_key(&session_id) {
            return Err(ServiceError::Internal(format!(
                "submit({session_id}): admission serialization was violated before durable acceptance"
            )));
        }
        let preparation = prepare_durable_acceptance(
            &self.cfg.results_dir,
            &paths,
            &acceptance,
            &req.archive.path,
        )?;
        let progress = preparation.progress;
        let acceptance_response_error = preparation.response_error;
        let entry = Arc::new(RunningEntry {
            snapshot: snapshot.clone(),
            acceptance,
            progress: progress.clone(),
            cancel: session_cancel.clone(),
            launch_decision: Arc::new(Mutex::new(())),
            terminal_decision: Mutex::new(TerminalDecision::Open),
        });
        inner
            .running
            .insert(session_id.clone(), Arc::clone(&entry));
        drop(inner);

        // Spawn a tracked supervisor that owns this session through
        // execution, panic cleanup, persistence, and map eviction. `run_one`
        // executes in an inner task so a panic becomes
        // an explicit JoinError instead of abandoning the map entry and every
        let cfg = Arc::clone(&self.cfg);
        let inner_for_task = Arc::clone(&self.inner);
        let admission_for_supervisor = Arc::clone(&self.admission);
        let session_id_for_task = session_id.clone();
        let cancel_for_task = session_cancel.clone();
        let cancel_for_supervisor = session_cancel.clone();
        let entry_for_task = Arc::clone(&entry);
        let launch_decision_for_task = Arc::clone(&entry.launch_decision);
        let prompt_preview_for_task = prompt_preview.clone();
        let supervisor_prompt = req.prompt.clone();
        let supervisor_preserve_thinking = req.preserve_thinking;
        let supervisor_max_session_turns = req.max_session_turns;
        let supervisor_archive_bytes = req.archive.bytes;
        let supervisor_archive_sha256 = req.archive.sha256.clone();
        let supervisor_started_at_unix = started_at_unix;
        let supervisor_wall_start = std::time::Instant::now();
        self.supervisors.spawn(async move {
            // This outer task is intentionally small. A panic in the inner
            // task still reaches the cleanup below.
            let run_cfg = Arc::clone(&cfg);
            let run_session_id = session_id_for_task.clone();
            let run_prompt_preview = prompt_preview_for_task.clone();
            let run_handle = tokio::spawn(async move {
                session::run_one(
                    &run_cfg,
                    &run_session_id,
                    req,
                    cancel_for_task,
                    launch_decision_for_task,
                    run_prompt_preview,
                    supervisor_started_at_unix,
                    paths,
                    progress,
                )
                .await
            });
            let mut body = match run_handle.await {
                Ok(body) => body,
                Err(join_error) => {
                    let counters = entry_for_task
                        .progress
                        .latest()
                        .map(|event| event.counters)
                        .unwrap_or_default();
                    if let Err(error) = entry_for_task.progress.publish(
                        ProgressPhase::TearingDown,
                        "the execution task terminated unexpectedly; stopping exact-owned producers, proving quiescence, and preserving forensic evidence",
                        counters,
                    ) {
                        tracing::error!(session_id = %session_id_for_task, error = %error,
                            "failed to publish panic-recovery progress");
                    }
                    session::recover_after_execution_panic(
                        &cfg,
                        &session_id_for_task,
                        &supervisor_prompt,
                        &prompt_preview_for_task,
                        supervisor_preserve_thinking,
                        supervisor_max_session_turns,
                        supervisor_archive_bytes,
                        &supervisor_archive_sha256,
                        supervisor_started_at_unix,
                        supervisor_wall_start,
                        cancel_for_supervisor.is_cancelled(),
                        join_error.to_string(),
                    )
                    .await
                }
            };

            // This brief decision transaction is the linearization point
            // shared with cancel(). Long terminal persistence happens only
            // after the mutex is released, so an HTTP cancellation request
            // can never wait behind bundling or filesystem cleanup.
            {
                let mut decision = entry_for_task.terminal_decision.lock().await;
                if cancel_for_supervisor.is_cancelled()
                    && body.status == SessionStatus::Completed
                {
                    body.status = SessionStatus::Cancelled;
                }
                *decision = TerminalDecision::Finalizing;
            }
            let body_counters = ProgressCounters {
                staged_bytes: body.staged_bytes,
                staged_entries: body.staged_entries,
                staged_regular_files: body.staged_regular_files,
                output_event_bytes: body.output_event_bytes,
                num_turns: body.num_turns,
            };
            let counters = entry_for_task
                .progress
                .latest()
                .map(|latest| merge_progress_counters(latest.counters, body_counters))
                .unwrap_or(body_counters);
            if let Err(error) = entry_for_task.progress.publish(
                ProgressPhase::PersistingTerminal,
                "execution, mandatory capture, teardown, and bundle handling are complete; preparing the no-clobber terminal resource",
                counters,
            ) {
                body.is_process_error = true;
                body.teardown_diagnostics.push(format!(
                    "publish terminal-persistence progress: {error}"
                ));
            }
            let terminal_progress = entry_for_task.progress.publish(
                ProgressPhase::Terminal,
                match body.status {
                    SessionStatus::Cancelled => {
                        "session cancellation is terminal; mandatory capture, teardown, and evidence handling are complete and the terminal record is ready for publication"
                    }
                    SessionStatus::Completed if body.is_process_error => {
                        "session is terminal with a process or lifecycle error; evidence handling is complete and the terminal record is ready for publication"
                    }
                    SessionStatus::Completed => {
                        "session completed successfully; durable evidence is complete and the terminal record is ready for publication"
                    }
                    SessionStatus::Running => "invalid running terminal state",
                },
                counters,
            );
            match terminal_progress {
                Ok(event) => apply_progress(&mut body, &event),
                Err(error) => {
                    body.is_process_error = true;
                    body.teardown_diagnostics.push(format!(
                        "publish final mandatory progress revision: {error}"
                    ));
                    if let Ok(event) = entry_for_task.progress.latest() {
                        apply_progress(&mut body, &event);
                    }
                }
            }
            if let Ok(events) = entry_for_task.progress.events() {
                body.progress_events = events;
            } else {
                body.is_process_error = true;
                body.teardown_diagnostics.push(
                    "copy complete progress history into terminal body: progress state mutex was poisoned"
                        .to_string(),
                );
            }

            // Terminalization is a durable prepare/cleanup/commit protocol:
            // first write and fsync the complete private terminal draft, then
            // remove raw state only when a required bundle exists, then
            // no-clobber publish the terminal. A process crash at any point
            // leaves either recoverable draft metadata, raw evidence, or both.
            let persist_error = persist_terminal_transaction(&cfg, &mut body).await.err();

            // Remove from the map only after the durable transaction above.
            // A concurrent `get` therefore sees either the running entry, a
            // committed on-disk terminal, or the explicit in-memory failure.
            // Map eviction is admission-fenced, so a same-handle retry
            // sees either the old running owner or the committed terminal
            // resource, never an identity-less state between the two.
            let _admission = admission_for_supervisor.lock().await;
            let mut inner = inner_for_task.lock().await;
            inner.running.remove(&session_id_for_task);
            if let Some(error) = persist_error {
                let mut retained = body;
                retained.is_process_error = true;
                retained.teardown_diagnostics.push(format!(
                    "terminal persistence failed; body retained only in service memory: {error}"
                ));
                tracing::error!(session_id = %retained.session_id, error = %error,
                    "terminal persistence failed; retaining terminal body in memory");
                inner
                    .unpersisted_terminal
                    .insert(session_id_for_task.clone(), retained);
            }
            drop(inner);
        });

        if let Some(error) = acceptance_response_error {
            return Err(error);
        }
        Ok(SubmitOutcome {
            body: running_body_for_entry(&self.cfg, &entry)?,
            newly_accepted: true,
        })
    }

    /// Pure read of a session by id. Looks in memory first (running
    /// sessions), falls back to disk (terminal sessions).
    pub async fn get(&self, session_id: &str) -> ServiceResult<SessionBody> {
        // Most live reads do not need the admission fence at all. If the
        // entry is absent, take the fence only long enough to close the tiny
        // accepted.json-visible/map-not-yet-inserted window and resolve
        // whether a disk resource exists. Reading a potentially large
        // terminal body never holds that global lifecycle mutex.
        // Bind the lookup so the map guard drops at this statement's end.
        // In edition 2021 an `if let` scrutinee temporary lives through the
        // success block, and `running_or_terminal_snapshot` locks `inner`
        // again — an extended guard self-deadlocks every read of a running
        // session until its connection is abandoned.
        let running_entry = self.inner.lock().await.running.get(session_id).cloned();
        if let Some(entry) = running_entry {
            return self.running_or_terminal_snapshot(session_id, entry).await;
        }
        let resolved: ServiceResult<SessionResolution> = {
            let _admission = self.admission.lock().await;
            if delete_intent_exists(&self.cfg.results_dir, session_id)? {
                return Err(ServiceError::SessionDeleting {
                    session_id: session_id.to_string(),
                });
            }
            let inner = self.inner.lock().await;
            if let Some(entry) = inner.running.get(session_id).cloned() {
                Ok(SessionResolution::Running(entry))
            } else if let Some(body) = inner.unpersisted_terminal.get(session_id).cloned() {
                Ok(SessionResolution::MemoryTerminal(body))
            } else {
                drop(inner);
                resolve_disk_session(&self.cfg, session_id)
            }
        };
        match resolved? {
            SessionResolution::Running(entry) => {
                self.running_or_terminal_snapshot(session_id, entry).await
            }
            SessionResolution::MemoryTerminal(body) => Ok(body),
            SessionResolution::DiskTerminal => read_terminal(&self.cfg, session_id).await,
        }
    }

    async fn get_unfenced(&self, session_id: &str) -> ServiceResult<SessionBody> {
        let inner = self.inner.lock().await;
        if let Some(entry) = inner.running.get(session_id).cloned() {
            drop(inner);
            return running_body_for_entry(&self.cfg, &entry);
        }
        if let Some(body) = inner.unpersisted_terminal.get(session_id).cloned() {
            return Ok(body);
        }
        drop(inner);
        read_terminal(&self.cfg, session_id).await
    }

    async fn running_or_terminal_snapshot(
        &self,
        session_id: &str,
        entry: Arc<RunningEntry>,
    ) -> ServiceResult<SessionBody> {
        let running = running_body_for_entry(&self.cfg, &entry);
        let inner = self.inner.lock().await;
        if inner
            .running
            .get(session_id)
            .is_some_and(|current| Arc::ptr_eq(current, &entry))
        {
            return running;
        }
        if let Some(body) = inner.unpersisted_terminal.get(session_id).cloned() {
            return Ok(body);
        }
        drop(inner);
        // The supervisor removes the map entry only after its terminal
        // transaction. If execution crossed that boundary while the running
        // snapshot was being assembled, return the terminal resource instead
        // of a stale `running` body or an event-file disappearance error.
        read_terminal(&self.cfg, session_id).await
    }

    /// Pure read of every visible session. Combines in-memory running
    /// entries with on-disk terminal entries (the on-disk records survive
    /// across server restart).
    pub async fn list(&self) -> ServiceResult<Vec<SessionBody>> {
        // Freeze only the set of visible resource identities under the short
        // admission fence. Each resource is then read independently after the
        // fence is released, so a large historical collection can never hold
        // cancellation or new acceptance hostage. DELETE racing after this
        // identity snapshot may naturally make one member disappear.
        let ids = {
            let _admission = self.admission.lock().await;
            let mut ids = std::collections::BTreeSet::new();
            let mut deleting = std::collections::BTreeSet::new();
            {
                let inner = self.inner.lock().await;
                ids.extend(inner.running.keys().cloned());
                ids.extend(inner.unpersisted_terminal.keys().cloned());
            }
            let dir_iter = std::fs::read_dir(&self.cfg.results_dir).map_err(|error| {
                ServiceError::Internal(io_msg(
                    "list: read_dir results_dir",
                    &self.cfg.results_dir,
                    &error,
                ))
            })?;
            for entry in dir_iter {
                let entry = entry.map_err(|error| {
                    ServiceError::Internal(io_msg(
                        "list: read results_dir entry",
                        &self.cfg.results_dir,
                        &error,
                    ))
                })?;
                let path = entry.path();
                let name = entry.file_name().into_string().map_err(|_| {
                    ServiceError::Internal(format!(
                        "list: result entry has no UTF-8 file name: {}",
                        path.display()
                    ))
                })?;
                if name.starts_with(DELETE_INTENT_PREFIX) {
                    let (session_id, unpublished) =
                        session_id_from_delete_control_name(&name).ok_or_else(|| {
                            ServiceError::Internal(format!(
                                "list: malformed deletion control entry {name:?}"
                            ))
                        })?;
                    if unpublished {
                        return Err(ServiceError::Internal(format!(
                            "list: unpublished deletion candidate remains at {} and requires restart reconciliation",
                            path.display()
                        )));
                    }
                    read_delete_intent(&self.cfg.results_dir, session_id)?
                        .ok_or_else(|| {
                            ServiceError::Internal(format!(
                                "list: deletion control {name:?} disappeared while resolving it"
                            ))
                        })?;
                    deleting.insert(session_id.to_string());
                    continue;
                }
                ids.insert(name);
            }
            for session_id in deleting {
                ids.remove(&session_id);
            }
            ids
        };

        let mut bodies = Vec::with_capacity(ids.len());
        for id in ids {
            match self.get(&id).await {
                Ok(body) => bodies.push(body),
                // A concurrent explicit DELETE after the identity snapshot
                // makes omission the accurate later observation.
                Err(ServiceError::NotFound { .. }) => {}
                Err(error) => return Err(error),
            }
        }
        bodies.sort_by_key(|b| b.started_at_unix);
        Ok(bodies)
    }

    /// Durably request cancellation of a running session.  This is a short,
    /// connection-independent transaction: the intent is synced before the
    /// response, the supervisor continues teardown independently, and a
    /// caller may later issue an ordinary GET.  Repeating the request is
    /// idempotent; a terminal session is returned unchanged.
    pub async fn cancel(&self, session_id: &str) -> ServiceResult<SessionBody> {
        let lifecycle = self.lifecycle.start()?;
        let manager = self.clone();
        let session_id = session_id.to_string();
        let operation = format!("record cancellation for {session_id}");
        await_connection_independent(
            async move {
                let _lifecycle = lifecycle;
                manager.cancel_server_owned(&session_id).await
            },
            operation,
        )
        .await
    }

    async fn cancel_server_owned(&self, session_id: &str) -> ServiceResult<SessionBody> {
        // The normal active-session path never waits on unrelated admission
        // or deletion work. Only an absent fast lookup takes the admission
        // fence, which closes the caller-known-handle race against the
        // original acceptance transaction.
        let mut entry = self.inner.lock().await.running.get(session_id).cloned();
        if entry.is_none() {
            let _admission = self.admission.lock().await;
            entry = self.inner.lock().await.running.get(session_id).cloned();
        }
        let entry = match entry {
            Some(e) => e,
            None => {
                // Not running — return whatever's on disk (or NotFound).
                return self.get_unfenced(session_id).await;
            }
        };

        // Linearize only against terminal outcome selection. If selection
        // already won, return immediately rather than blocking behind its
        // long durable publication. Otherwise sync the intent before the
        // interrupt token becomes observable.
        let decision = entry.terminal_decision.lock().await;
        if *decision == TerminalDecision::Finalizing {
            drop(decision);
            if !self.inner.lock().await.running.contains_key(session_id) {
                return self.get_unfenced(session_id).await;
            }
            return Err(ServiceError::SessionFinalizing {
                session_id: session_id.to_string(),
            });
        }
        if !self.inner.lock().await.running.contains_key(session_id) {
            drop(decision);
            return self.get_unfenced(session_id).await;
        }
        let launch_decision = entry.launch_decision.lock().await;
        let cancellation = persist_cancel_intent(&self.cfg.results_dir, session_id)?;
        entry.cancel.cancel();
        let progress_result = publish_cancellation_progress_once(
            &entry,
            "durable cancellation intent recorded; interrupting active work and completing mandatory evidence/teardown",
        );
        let body_result = running_body_for_entry(&self.cfg, &entry);
        drop(launch_decision);
        drop(decision);
        if let Some(error) = cancellation.response_error {
            return Err(error);
        }
        progress_result?;
        body_result
    }

    /// Remove a terminal session from disk. The lifecycle is explicit:
    /// `delete` refuses to act on a running session (`SessionRunning`/409)
    /// — the operator must `cancel` first.
    ///
    /// Returns `NotFound` for unknown ids (informative — repeat callers
    /// see "yes, it's gone" rather than a silent success).
    pub async fn delete(&self, session_id: &str) -> ServiceResult<()> {
        let lifecycle = self.lifecycle.start()?;
        let manager = self.clone();
        let session_id = session_id.to_string();
        let operation = format!("delete terminal resource {session_id}");
        await_connection_independent(
            async move {
                let _lifecycle = lifecycle;
                manager.delete_server_owned(&session_id).await
            },
            operation,
        )
        .await
    }

    async fn delete_server_owned(&self, session_id: &str) -> ServiceResult<()> {
        let _admission = self.admission.lock().await;
        if self.inner.lock().await.running.contains_key(session_id) {
            return Err(ServiceError::SessionRunning {
                session_id: session_id.to_string(),
            });
        }
        if !is_safe_session_id(session_id) {
            return Err(ServiceError::InvalidRequest(format!(
                "delete: session_id {session_id:?} is not a supported canonical hex-session shape — \
                 refusing to join it onto the results_dir path (defensive against path traversal \
                 even though we trust the URL router not to send arbitrary strings)"
            )));
        }
        if delete_intent_exists(&self.cfg.results_dir, session_id)? {
            finish_delete_intent(&self.cfg.state_dir, &self.cfg.results_dir, session_id)?;
            self.inner
                .lock()
                .await
                .unpersisted_terminal
                .remove(session_id);
            return Ok(());
        }
        // A guessed orphan ID is not deletion authority. Resolve the exact
        // terminal record (durable or explicitly retained in memory) before
        // touching any corresponding raw-state path.
        let terminal = self.get_unfenced(session_id).await?;
        if terminal.status == SessionStatus::Running {
            return Err(ServiceError::SessionRunning {
                session_id: session_id.to_string(),
            });
        }
        // Validate any retained-tree authority before committing deletion.
        // Once the independent results-root intent is durable, cleanup may
        // safely resume without rereading files that a prior attempt already
        // removed.
        let retained_state = self.cfg.state_dir.join("sessions").join(session_id);
        if terminal.raw_session_tree_retained {
            validate_delete_state_marker(&retained_state, &terminal)?;
        }
        persist_delete_intent(&self.cfg.results_dir, session_id)?;
        finish_delete_intent(&self.cfg.state_dir, &self.cfg.results_dir, session_id)?;
        self.inner
            .lock()
            .await
            .unpersisted_terminal
            .remove(session_id);
        Ok(())
    }

    /// Server shutdown. Cancels the parent token (cascades to every child),
    /// then closes the supervisor tracker and waits for it to drain — that
    /// is the strongest "no in-flight session remains" signal we have,
    /// since every run task is tracked until its teardown completes.
    ///
    /// There is deliberately no shutdown deadline: shutdown first cancels
    /// the session, then waits until its fail-closed teardown is genuinely
    /// finished. Exiting early would orphan state while claiming success.
    pub async fn shutdown(&self) -> ServiceResult<()> {
        // Close mutation admission before the lifecycle snapshot, then drain
        // every short operation already transferred to the service runtime.
        // No disconnected handler can mutate durable state after this point.
        self.lifecycle.close()?;
        self.lifecycle.wait_idle().await?;

        // Serialize against submission so no operation can appear between
        // the snapshot below and the shutdown fence. For every outcome still
        // open, sync the same durable cancellation intent used by the public
        // API before making interruption observable.
        let admission = self.admission.lock().await;
        let entries: Vec<(String, Arc<RunningEntry>)> = self
            .inner
            .lock()
            .await
            .running
            .iter()
            .map(|(id, entry)| (id.clone(), Arc::clone(entry)))
            .collect();
        let mut cancellation_errors = Vec::new();
        for (session_id, entry) in &entries {
            let decision = entry.terminal_decision.lock().await;
            if *decision == TerminalDecision::Open {
                let launch_decision = entry.launch_decision.lock().await;
                match persist_cancel_intent(&self.cfg.results_dir, session_id) {
                    Ok(publication) => {
                        entry.cancel.cancel();
                        if let Some(error) = publication.response_error {
                            cancellation_errors.push(error.to_string());
                        }
                        match publish_cancellation_progress_once(
                            entry,
                            "service shutdown durably recorded cancellation; interrupting active work and completing mandatory evidence/teardown",
                        ) {
                            Ok(_) => {}
                            Err(error) => cancellation_errors.push(format!(
                                "shutdown progress publication for {session_id}: {error}"
                            )),
                        }
                    }
                    Err(error) => cancellation_errors.push(format!(
                        "durably record shutdown cancellation for {session_id}: {error}"
                    )),
                }
                drop(launch_decision);
            }
            drop(decision);
        }
        // This is both the admission fence checked by future submissions and
        // the final interrupt for any entry whose per-session persistence
        // failed. Such a failure is returned after teardown, never hidden.
        self.shutdown_token.cancel();
        drop(admission);

        let in_flight: Vec<String> = entries.into_iter().map(|(id, _)| id).collect();
        tracing::info!(
            sessions = ?in_flight,
            "shutdown: cancellation cascaded; awaiting supervisor drain"
        );

        // Closing the tracker and waiting for it guarantees every spawned
        // supervisor has completed execution/panic cleanup, persistence,
        // and map eviction. Any in-flight `submit` that was between the
        // shutdown snapshot and its supervisor spawn also has its task
        // observe the cascade-cancellation and terminate under the tracker.
        self.supervisors.close();
        self.supervisors.wait().await;
        tracing::info!("shutdown: supervisor tracker drained — all sessions terminal");
        if cancellation_errors.is_empty() {
            Ok(())
        } else {
            Err(ServiceError::Internal(cancellation_errors.join("; ")))
        }
    }
}

// ── helpers ───────────────────────────────────────────────────────────────

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// Build the wire body for a running snapshot. Required-field discipline:
/// every field present, terminal-only fields zeroed. `progress` carries
/// the live `(num_turns, last_event_at_unix)` reading the caller has
/// just taken from disk.
fn running_body(
    s: &RunningSnapshot,
    progress: &ProgressEvent,
    progress_events: Vec<ProgressEvent>,
    output: RunningOutputProgress,
) -> SessionBody {
    SessionBody {
        session_id: s.session_id.clone(),
        status: SessionStatus::Running,
        started_at_unix: s.started_at_unix,
        model: s.model.clone(),
        context_window: s.context_window,
        preserve_thinking: s.preserve_thinking,
        max_session_turns: s.max_session_turns,
        archive_bytes: s.archive_bytes,
        archive_sha256: s.archive_sha256.clone(),
        prompt_preview: s.prompt_preview.clone(),
        progress_revision: progress.revision,
        progress_at_unix_ms: progress.at_unix_ms,
        progress_phase: progress.phase,
        progress_message: progress.message.clone(),
        staged_bytes: progress.counters.staged_bytes,
        staged_entries: progress.counters.staged_entries,
        staged_regular_files: progress.counters.staged_regular_files,
        output_event_bytes: output
            .output_event_bytes
            .max(progress.counters.output_event_bytes),
        progress_events,
        num_turns: output.num_turns.max(progress.counters.num_turns),
        last_event_at_unix: output.last_event_at_unix,
        finished_at_unix: 0,
        duration_wall_ms: 0,
        container_exit_code: 0,
        agent_exit_code: 0,
        is_process_error: false,
        response: String::new(),
        agent_duration_ms: 0,
        bundle_sha256: String::new(),
        bundle_compressed_bytes: 0,
        bundle_uncompressed_bytes: 0,
        bundle_file_count: 0,
        bundle_artifacts_file_count: 0,
        raw_session_tree_retained: false,
        teardown_diagnostics: Vec::new(),
    }
}

fn running_body_for_entry(cfg: &Config, entry: &RunningEntry) -> ServiceResult<SessionBody> {
    let progress_events = entry.progress.events()?;
    let progress = progress_events.last().cloned().ok_or_else(|| {
        ServiceError::Internal("progress snapshot has no initial accepted event".into())
    })?;
    let output = read_running_progress(&events_jsonl_path(cfg, &entry.snapshot.session_id))?;
    Ok(running_body(&entry.snapshot, &progress, progress_events, output))
}

pub(crate) fn apply_progress(body: &mut SessionBody, progress: &ProgressEvent) {
    body.progress_revision = progress.revision;
    body.progress_at_unix_ms = progress.at_unix_ms;
    body.progress_phase = progress.phase;
    body.progress_message = progress.message.clone();
    // Terminal evidence may have advanced beyond the last durable progress
    // publication (for example when the storage failure itself caused
    // terminalization). Counters are monotonic facts, so a stale lifecycle
    // event must never erase a larger directly observed terminal value.
    body.staged_bytes = body.staged_bytes.max(progress.counters.staged_bytes);
    body.staged_entries = body.staged_entries.max(progress.counters.staged_entries);
    body.staged_regular_files = body
        .staged_regular_files
        .max(progress.counters.staged_regular_files);
    body.output_event_bytes = body
        .output_event_bytes
        .max(progress.counters.output_event_bytes);
    body.num_turns = body.num_turns.max(progress.counters.num_turns);
}

pub(crate) fn merge_progress_counters(
    left: ProgressCounters,
    right: ProgressCounters,
) -> ProgressCounters {
    ProgressCounters {
        staged_bytes: left.staged_bytes.max(right.staged_bytes),
        staged_entries: left.staged_entries.max(right.staged_entries),
        staged_regular_files: left
            .staged_regular_files
            .max(right.staged_regular_files),
        output_event_bytes: left.output_event_bytes.max(right.output_event_bytes),
        num_turns: left.num_turns.max(right.num_turns),
    }
}

/// Path to a session's live `events.jsonl` while it is running. Once the
/// session reaches a terminal state, this file is bundled and removed
/// from staging — the on-disk path is no longer valid, and frozen values
/// from `finished.json` are returned instead.
pub fn events_jsonl_path(cfg: &Config, session_id: &str) -> PathBuf {
    cfg.state_dir
        .join("sessions")
        .join(session_id)
        .join("output")
        .join("events.jsonl")
}

/// Exact output observations available from one pure snapshot read.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RunningOutputProgress {
    pub num_turns: u64,
    pub last_event_at_unix: u64,
    pub output_event_bytes: u64,
}

/// Read the live `events.jsonl` and return its exact completed-turn count,
/// modification time, and byte size.
/// `num_turns` is the number of completed main-thread model invocations. Qwen
/// stream-JSON can emit zero-usage thinking/text fragments before the one
/// assistant record carrying final per-invocation usage, so fragments are not
/// turns. Both fields are 0 only when the file does not exist yet; malformed or
/// unreadable state is an explicit error.
///
/// Cost: a linear byte scan per explicit API read of one session's live
/// progress, so this favors exactness over a mutable cache.
pub fn read_running_progress(events_path: &std::path::Path) -> ServiceResult<RunningOutputProgress> {
    use std::io::{BufReader, Read};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(events_path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RunningOutputProgress::default())
        }
        Err(error) => {
            return Err(ServiceError::Internal(io_msg(
                "read_running_progress: stat",
                events_path,
                &error,
            )))
        }
    };
    let meta = file.metadata().map_err(|error| {
        ServiceError::Internal(io_msg(
            "read_running_progress: fstat opened event stream",
            events_path,
            &error,
        ))
    })?;
    if !meta.is_file()
        || meta.permissions().mode() & 0o777 != 0o600
        || meta.uid() != 1000
        || meta.gid() != 1000
    {
        return Err(ServiceError::Internal(format!(
            "read_running_progress: {} has unsafe opened type/mode/owner: type={:?} mode={:o} uid={} gid={} expected=1000:1000",
            events_path.display(),
            meta.file_type(),
            meta.permissions().mode() & 0o777,
            meta.uid(),
            meta.gid()
        )));
    }
    let modified = meta.modified().map_err(|error| {
        ServiceError::Internal(io_msg(
            "read_running_progress: event modification time",
            events_path,
            &error,
        ))
    })?;
    let last_event_at_unix = modified
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| {
            ServiceError::Internal(format!(
                "read_running_progress: event modification time for {} predates the Unix epoch: {error}",
                events_path.display()
            ))
        })?
        .as_secs();
    // Freeze this read at the descriptor's observed byte length. The capture
    // process may append after fstat; those newer bytes belong to a later GET
    // and must not turn this snapshot into a moving target.
    let snapshot_len = meta.len();
    let mut reader = BufReader::new(file.take(snapshot_len));
    let mut num_turns = 0u64;
    let mut index = 0usize;
    let mut chunk = Vec::new();
    loop {
        chunk.clear();
        let terminated = crate::result_parse::read_bounded_record(
            &mut reader,
            &mut chunk,
            events_path,
        )?;
        if chunk.is_empty() && !terminated {
            break;
        }
        index = index.checked_add(1).ok_or_else(|| {
            ServiceError::Internal("read_running_progress: JSONL record count overflowed".into())
        })?;
        // `tee` can be observed after writing part of the next JSON object.
        // That is an explicit in-progress state, not malformed completed
        // data, so only newline-terminated records participate in progress.
        if !terminated {
            break;
        }
        if chunk.iter().all(|byte| byte.is_ascii_whitespace()) {
            continue;
        }
        let value: serde_json::Value = serde_json::from_slice(&chunk).map_err(|error| {
            ServiceError::Internal(format!(
                "read_running_progress: completed JSONL record {index} is malformed: {error}"
            ))
        })?;
        let object = value.as_object().ok_or_else(|| {
            ServiceError::Internal(format!(
                "read_running_progress: completed JSONL record {index} is not an object"
            ))
        })?;
        object
            .get("type")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                ServiceError::Internal(format!(
                    "read_running_progress: completed JSONL record {index} lacks string type"
                ))
            })?;
        if crate::result_parse::is_completed_main_turn(object) {
            num_turns = num_turns.checked_add(1).ok_or_else(|| {
                ServiceError::Internal("read_running_progress: turn count overflowed".into())
            })?;
        }
    }
    Ok(RunningOutputProgress {
        num_turns,
        last_event_at_unix,
        output_event_bytes: meta.len(),
    })
}

pub fn preview(s: &str) -> String {
    let truncated: String = s.chars().take(140).collect();
    if truncated.chars().count() < s.chars().count() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

fn acceptance_path(results_dir: &Path, session_id: &str) -> PathBuf {
    results_dir.join(session_id).join("accepted.json")
}

fn progress_path(results_dir: &Path, session_id: &str) -> PathBuf {
    results_dir.join(session_id).join("progress.json")
}

fn cancel_intent_path(results_dir: &Path, session_id: &str) -> PathBuf {
    results_dir.join(session_id).join("cancel-requested.json")
}

fn cancel_intent_next_path(results_dir: &Path, session_id: &str) -> PathBuf {
    results_dir
        .join(session_id)
        .join("cancel-requested.json.next")
}

fn delete_intent_path(results_dir: &Path, session_id: &str) -> PathBuf {
    results_dir.join(format!(
        "{DELETE_INTENT_PREFIX}{session_id}{DELETE_INTENT_SUFFIX}"
    ))
}

fn delete_intent_next_path(results_dir: &Path, session_id: &str) -> PathBuf {
    results_dir.join(format!(
        "{DELETE_INTENT_PREFIX}{session_id}{DELETE_INTENT_SUFFIX}.next"
    ))
}

fn session_id_from_delete_control_name(name: &str) -> Option<(&str, bool)> {
    let (base, unpublished) = match name.strip_suffix(".next") {
        Some(base) => (base, true),
        None => (name, false),
    };
    let session_id = base
        .strip_prefix(DELETE_INTENT_PREFIX)?
        .strip_suffix(DELETE_INTENT_SUFFIX)?;
    is_safe_session_id(session_id).then_some((session_id, unpublished))
}

fn prepare_durable_acceptance(
    results_dir: &Path,
    paths: &SessionPaths,
    acceptance: &AcceptanceRecord,
    input_archive_spool: &Path,
) -> ServiceResult<AcceptancePreparation> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let result_dir = results_dir.join(&acceptance.session_id);
    for (role, path) in [
        ("raw session tree", &paths.root),
        ("result directory", &result_dir),
    ] {
        match std::fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => {
                return Err(ServiceError::Internal(format!(
                    "acceptance collision: {role} already exists at {}; refusing to adopt or delete it",
                    path.display()
                )));
            }
            Err(error) => {
                return Err(ServiceError::Internal(io_msg(
                    &format!("stat prospective {role}"),
                    path,
                    &error,
                )));
            }
        }
    }

    let mut state_created = false;
    let mut result_created = false;
    let preparation = (|| {
        paths.create_dirs()?;
        state_created = true;
        paths.write_prompt(&acceptance.prompt)?;
        paths.write_history_policy(acceptance.preserve_thinking)?;
        paths.write_turn_budget(acceptance.max_session_turns)?;
        place_input_archive(paths, input_archive_spool)?;

        std::fs::create_dir(&result_dir).map_err(|error| {
            ServiceError::Internal(io_msg(
                "exclusively create accepted result directory",
                &result_dir,
                &error,
            ))
        })?;
        result_created = true;
        std::fs::set_permissions(&result_dir, std::fs::Permissions::from_mode(0o755)).map_err(
            |error| {
                ServiceError::Internal(io_msg(
                    "set exact accepted result-directory mode",
                    &result_dir,
                    &error,
                ))
            },
        )?;
        sync_directory(results_dir, "sync accepted result-directory publication")?;

        let progress = ProgressReporter::create(
            &progress_path(results_dir, &acceptance.session_id),
            &acceptance.session_id,
            "request durably accepted; source staging has not started yet",
        )?;

        let accepted_path = result_dir.join("accepted.json");
        let accepted_next = result_dir.join("accepted.json.next");
        let mut bytes = serde_json::to_vec_pretty(acceptance).map_err(|error| {
            ServiceError::Internal(format!(
                "serialize durable acceptance for {}: {error}",
                acceptance.session_id
            ))
        })?;
        bytes.push(b'\n');
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&accepted_next)
            .map_err(|error| {
                ServiceError::Internal(io_msg(
                    "create unpublished acceptance record",
                    &accepted_next,
                    &error,
                ))
            })?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| {
                ServiceError::Internal(io_msg(
                    "write/sync unpublished acceptance record",
                    &accepted_next,
                    &error,
                ))
            })?;
        sync_directory(&result_dir, "sync unpublished acceptance record")?;
        std::fs::rename(&accepted_next, &accepted_path).map_err(|error| {
            ServiceError::Internal(io_msg(
                "atomically publish durable acceptance record",
                &accepted_path,
                &error,
            ))
        })?;
        sync_directory(&result_dir, "sync durable acceptance publication")?;
        Ok(progress)
    })();
    match preparation {
        Ok(progress) => Ok(AcceptancePreparation {
            progress,
            response_error: None,
        }),
        Err(error) => {
            // Once accepted.json is visible the caller-known handle owns a
            // durable resource even if the final directory fsync reported an
            // error. Never erase that ambiguity. A retry or startup recovery
            // can diagnose it from the same handle.
            if acceptance_exists(results_dir, &acceptance.session_id)? {
                let observed = read_acceptance(results_dir, &acceptance.session_id)?;
                if observed != *acceptance {
                    return Err(ServiceError::Internal(format!(
                        "{error}; the visible acceptance record for {} does not match the transaction that published it",
                        acceptance.session_id
                    )));
                }
                let progress = ProgressReporter::open(
                    &progress_path(results_dir, &acceptance.session_id),
                    &acceptance.session_id,
                )?;
                let retry = sync_directory(
                    &result_dir,
                    "retry final durable acceptance publication barrier",
                );
                return match retry {
                    Ok(()) => {
                        tracing::warn!(
                            session_id = acceptance.session_id,
                            first_error = %error,
                            "the initial acceptance-directory sync failed, but the immediate explicit durability retry succeeded"
                        );
                        Ok(AcceptancePreparation {
                            progress,
                            response_error: None,
                        })
                    }
                    Err(retry_error) => Ok(AcceptancePreparation {
                        progress,
                        response_error: Some(ServiceError::AcceptanceDurabilityFailed {
                            session_id: acceptance.session_id.clone(),
                            detail: format!(
                                "the acceptance marker for {} is visible and its detached supervisor is continuing, but both directory durability barriers failed ({error}; retry: {retry_error}); retain this exact handle and retry the identical POST or issue GET—never generate another handle for the same operation",
                                acceptance.session_id
                            ),
                        }),
                    }),
                };
            }
            Err(rollback_uncommitted_acceptance(
                results_dir,
                paths,
                result_created,
                state_created,
                error,
            ))
        }
    }
}

/// Relocate the proved upload spool into the session tree as the accepted
/// input archive. The spool and session trees share one state filesystem, so
/// the rename atomically transfers ownership of the exact proved bytes; a
/// later closure failure unwinds it together with the uncommitted session
/// tree, after which only a fresh upload can retry the operation.
fn place_input_archive(paths: &SessionPaths, spool: &Path) -> ServiceResult<()> {
    use std::os::unix::fs::PermissionsExt;

    let destination = paths.input_archive();
    std::fs::rename(spool, &destination).map_err(|error| {
        ServiceError::Internal(io_msg(
            "relocate accepted workspace archive into the session tree",
            &destination,
            &error,
        ))
    })?;
    std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o600)).map_err(
        |error| {
            ServiceError::Internal(io_msg(
                "set service-only mode on accepted workspace archive",
                &destination,
                &error,
            ))
        },
    )?;
    sync_directory(&paths.root, "sync accepted workspace-archive relocation")?;
    if let Some(spool_parent) = spool.parent() {
        sync_directory(spool_parent, "sync workspace-archive spool consumption")?;
    }
    Ok(())
}

fn rollback_uncommitted_acceptance(
    results_dir: &Path,
    paths: &SessionPaths,
    result_created: bool,
    state_created: bool,
    original: ServiceError,
) -> ServiceError {
    let mut cleanup = Vec::new();
    let result_dir = results_dir.join(
        paths
            .root
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("invalid-session-root")),
    );
    for (created, label, path) in [
        (state_created, "uncommitted raw session tree", &paths.root),
        (result_created, "uncommitted result directory", &result_dir),
    ] {
        if !created {
            continue;
        }
        match std::fs::remove_dir_all(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => cleanup.push(io_msg(&format!("remove {label}"), path, &error)),
        }
    }
    for (label, parent) in [
        ("sessions parent", paths.root.parent()),
        ("results parent", Some(results_dir)),
    ] {
        if let Some(parent) = parent {
            if let Err(error) = sync_directory(parent, &format!("sync {label} after rollback")) {
                cleanup.push(error.to_string());
            }
        }
    }
    if cleanup.is_empty() {
        original
    } else {
        ServiceError::Internal(format!(
            "{original}; the pre-commit acceptance rollback also failed: {}",
            cleanup.join("; ")
        ))
    }
}

pub(crate) fn read_acceptance(
    results_dir: &Path,
    session_id: &str,
) -> ServiceResult<AcceptanceRecord> {
    if !is_safe_session_id(session_id) {
        return Err(ServiceError::InvalidRequest(format!(
            "session_id {session_id:?} is not a supported canonical session handle"
        )));
    }
    let path = acceptance_path(results_dir, session_id);
    read_acceptance_file(&path, session_id, "durable acceptance record")
}

fn read_acceptance_file(
    path: &Path,
    session_id: &str,
    role: &str,
) -> ServiceResult<AcceptanceRecord> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(&path)
        .map_err(|error| {
            ServiceError::Internal(io_msg(
                &format!("open {role} without following links"),
                path,
                &error,
            ))
        })?;
    let metadata = file.metadata().map_err(|error| {
        ServiceError::Internal(io_msg(
            &format!("fstat opened {role}"),
            path,
            &error,
        ))
    })?;
    if !metadata.is_file()
        || metadata.permissions().mode() & 0o777 != 0o600
        || metadata.uid() != 1000
        || metadata.gid() != 1000
        || metadata.len() > MAX_ACCEPTANCE_RECORD_BYTES
    {
        return Err(ServiceError::Internal(format!(
            "{role} {} has unsafe type/mode/owner/size",
            path.display()
        )));
    }
    let capacity = usize::try_from(metadata.len()).map_err(|_| {
        ServiceError::Internal(format!(
            "{role} {} is too large to address on this platform",
            path.display()
        ))
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    file.read_to_end(&mut bytes).map_err(|error| {
        ServiceError::Internal(io_msg(
            &format!("read opened {role}"),
            path,
            &error,
        ))
    })?;
    if bytes.len() as u64 != metadata.len() {
        return Err(ServiceError::Internal(format!(
            "{role} {} changed length while open: fstat={} read={}",
            path.display(),
            metadata.len(),
            bytes.len()
        )));
    }
    let record: AcceptanceRecord = serde_json::from_slice(&bytes).map_err(|error| {
        ServiceError::Internal(format!(
            "{role} {} is malformed: {error}",
            path.display()
        ))
    })?;
    // Durable acceptance records were introduced together with the 256-bit
    // caller-known-handle wire protocol, and that protocol has only ever
    // written schema version 2. No version-1 record can legitimately exist
    // on disk, so anything else here is drift, not compatibility.
    if record.schema_version != 2 || record.session_id != session_id {
        return Err(ServiceError::Internal(format!(
            "{role} {} has identity/schema drift",
            path.display()
        )));
    }
    Ok(record)
}

fn require_matching_acceptance(
    acceptance: &AcceptanceRecord,
    prompt: &str,
    archive_bytes: u64,
    archive_sha256: &str,
    preserve_thinking: bool,
    max_session_turns: u32,
) -> ServiceResult<()> {
    if acceptance.matches_wire(
        prompt,
        archive_bytes,
        archive_sha256,
        preserve_thinking,
        max_session_turns,
    ) {
        return Ok(());
    }
    Err(ServiceError::IdempotencyConflict {
        session_id: acceptance.session_id.clone(),
        detail: "the supplied Idempotency-Key already owns a different archive commitment, prompt, preserve_thinking, or max_session_turns value; generate a fresh 32-byte CSPRNG handle for a different operation".to_string(),
    })
}

fn acceptance_exists(results_dir: &Path, session_id: &str) -> ServiceResult<bool> {
    let path = acceptance_path(results_dir, session_id);
    match std::fs::symlink_metadata(&path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(ServiceError::Internal(io_msg(
            "stat possible durable acceptance record",
            &path,
            &error,
        ))),
    }
}

fn publish_cancellation_progress_once(
    entry: &RunningEntry,
    message: &'static str,
) -> ServiceResult<()> {
    let events = entry.progress.events()?;
    if events
        .iter()
        .any(|event| event.phase == ProgressPhase::Cancelling)
    {
        return Ok(());
    }
    let counters = events
        .last()
        .ok_or_else(|| ServiceError::Internal("progress history has no accepted event".into()))?
        .counters;
    entry
        .progress
        .publish(ProgressPhase::Cancelling, message, counters)
        .map(|_| ())
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CancelIntent {
    schema_version: u32,
    session_id: String,
    requested_at_unix_ms: u64,
}

/// Publish or revalidate the exact cancellation marker. Once a byte-valid
/// marker is visible, the caller interrupts live work even if a directory
/// durability barrier reports failure; that failure is returned explicitly
/// while the same handle remains safe to retry.
fn persist_cancel_intent(
    results_dir: &Path,
    session_id: &str,
) -> ServiceResult<CancelIntentPublication> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let path = cancel_intent_path(results_dir, session_id);
    let next = cancel_intent_next_path(results_dir, session_id);
    let parent = path
        .parent()
        .expect("cancellation intent has result parent");
    match std::fs::symlink_metadata(&next) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) => {
            return Err(ServiceError::Internal(format!(
                "unpublished cancellation intent already exists at {}; restart reconciliation is required before cancellation may continue",
                next.display()
            )));
        }
        Err(error) => {
            return Err(ServiceError::Internal(io_msg(
                "stat unpublished cancellation intent",
                &next,
                &error,
            )));
        }
    }
    if read_cancel_intent(results_dir, session_id)?.is_some() {
        let response_error = sync_directory(
            parent,
            "revalidate cancellation-intent directory durability",
        )
        .err()
        .map(|error| ServiceError::CancellationDurabilityFailed {
            session_id: session_id.to_string(),
            detail: format!(
                "the valid cancellation marker for {session_id} is visible and live interruption continues, but its directory durability revalidation failed: {error}; retain this handle and retry the same cancellation or issue GET"
            ),
        });
        return Ok(CancelIntentPublication { response_error });
    }
    let intent = CancelIntent {
        schema_version: 1,
        session_id: session_id.to_string(),
        requested_at_unix_ms: unix_now_ms(),
    };
    let mut bytes = serde_json::to_vec_pretty(&intent).map_err(|error| {
        ServiceError::Internal(format!("serialize cancellation intent: {error}"))
    })?;
    bytes.push(b'\n');
    let mut candidate_created = false;
    let mut published = false;
    let publication = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&next)
            .map_err(|error| {
                ServiceError::Internal(io_msg(
                    "create unpublished cancellation intent",
                    &next,
                    &error,
                ))
            })?;
        candidate_created = true;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| {
                ServiceError::Internal(io_msg(
                    "write/sync unpublished cancellation intent",
                    &next,
                    &error,
                ))
            })?;
        sync_directory(parent, "sync unpublished cancellation intent")?;
        std::fs::rename(&next, &path).map_err(|error| {
            ServiceError::Internal(io_msg(
                "atomically publish cancellation intent",
                &path,
                &error,
            ))
        })?;
        candidate_created = false;
        published = true;
        sync_directory(parent, "sync cancellation-intent publication")
    })();

    match publication {
        Ok(()) => Ok(CancelIntentPublication {
            response_error: None,
        }),
        Err(first_error) => {
            // Once the rename is visible, the intent is the authoritative
            // cancellation decision.  Validate its exact bytes and retry the
            // durability barrier, but never roll it back or leave live work
            // running merely because the final fsync reported an error.
            let visible = published
                || match std::fs::symlink_metadata(&path) {
                    Ok(_) => true,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                    Err(error) => {
                        return Err(ServiceError::Internal(format!(
                            "{first_error}; stat cancellation intent after publication failure: {}",
                            io_msg("stat cancellation intent", &path, &error)
                        )));
                    }
                };
            if visible {
                let observed = read_cancel_intent(results_dir, session_id)?.ok_or_else(|| {
                    ServiceError::Internal(format!(
                        "{first_error}; the cancellation marker became visible and then disappeared"
                    ))
                })?;
                if observed.schema_version != intent.schema_version
                    || observed.session_id != intent.session_id
                    || observed.requested_at_unix_ms != intent.requested_at_unix_ms
                {
                    return Err(ServiceError::Internal(format!(
                        "{first_error}; the visible cancellation marker for {session_id} does not match the transaction that published it"
                    )));
                }
                return match sync_directory(
                    parent,
                    "retry cancellation-intent publication barrier",
                ) {
                    Ok(()) => {
                        tracing::warn!(
                            session_id,
                            error = %first_error,
                            "the initial cancellation-directory sync failed after publication, but its immediate explicit retry succeeded"
                        );
                        Ok(CancelIntentPublication {
                            response_error: None,
                        })
                    }
                    Err(retry_error) => Ok(CancelIntentPublication {
                        response_error: Some(ServiceError::CancellationDurabilityFailed {
                            session_id: session_id.to_string(),
                            detail: format!(
                                "the cancellation marker for {session_id} is visible and live interruption continues, but both directory durability barriers failed ({first_error}; retry: {retry_error}); retain this handle and retry the same cancellation or issue GET"
                            ),
                        }),
                    }),
                };
            }

            // Before rename there is no committed cancellation. Remove only
            // the exact candidate created by this transaction and make that
            // rollback durable. A failed rollback is reported together with
            // the original error and startup will fail closed on the residue.
            let mut rollback_errors = Vec::new();
            if candidate_created {
                match std::fs::remove_file(&next) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => rollback_errors.push(io_msg(
                        "remove unpublished cancellation intent",
                        &next,
                        &error,
                    )),
                }
            }
            if let Err(error) =
                sync_directory(parent, "sync unpublished cancellation-intent rollback")
            {
                rollback_errors.push(error.to_string());
            }
            if rollback_errors.is_empty() {
                Err(first_error)
            } else {
                Err(ServiceError::Internal(format!(
                    "{first_error}; cancellation-intent rollback also failed: {}",
                    rollback_errors.join("; ")
                )))
            }
        }
    }
}

fn read_cancel_intent(
    results_dir: &Path,
    session_id: &str,
) -> ServiceResult<Option<CancelIntent>> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

    let path = cancel_intent_path(results_dir, session_id);
    let mut file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(ServiceError::Internal(io_msg(
                "stat cancellation intent",
                &path,
                &error,
            )));
        }
    };
    let metadata = file.metadata().map_err(|error| {
        ServiceError::Internal(io_msg(
            "fstat opened cancellation intent",
            &path,
            &error,
        ))
    })?;
    if !metadata.is_file()
        || metadata.permissions().mode() & 0o777 != 0o600
        || metadata.uid() != 1000
        || metadata.gid() != 1000
        || metadata.len() > MAX_CANCEL_INTENT_BYTES
    {
        return Err(ServiceError::Internal(format!(
            "durable cancellation intent {} has unsafe type/mode/owner",
            path.display()
        )));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut bytes).map_err(|error| {
        ServiceError::Internal(io_msg(
            "read opened cancellation intent",
            &path,
            &error,
        ))
    })?;
    if bytes.len() as u64 != metadata.len() {
        return Err(ServiceError::Internal(format!(
            "durable cancellation intent {} changed length while open: fstat={} read={}",
            path.display(),
            metadata.len(),
            bytes.len()
        )));
    }
    let intent: CancelIntent = serde_json::from_slice(&bytes).map_err(|error| {
        ServiceError::Internal(format!(
            "durable cancellation intent {} is malformed: {error}",
            path.display()
        ))
    })?;
    if intent.schema_version != 1 || intent.session_id != session_id {
        return Err(ServiceError::Internal(format!(
            "durable cancellation intent {} has identity/schema drift",
            path.display()
        )));
    }
    Ok(Some(intent))
}

/// Reconcile only an unpublished cancellation candidate left by a process
/// crash. Rename is the commit point: a `.next` candidate is never promoted
/// or interpreted as intent. If the final marker exists it remains
/// authoritative; otherwise restart recovery terminalizes the accepted job
/// as an interrupted operation rather than inventing a cancellation.
fn reconcile_unpublished_cancel_intent(
    results_dir: &Path,
    session_id: &str,
) -> ServiceResult<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let next = cancel_intent_next_path(results_dir, session_id);
    let metadata = match std::fs::symlink_metadata(&next) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(ServiceError::Internal(io_msg(
                "restart recovery: stat unpublished cancellation intent",
                &next,
                &error,
            )));
        }
    };
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o777 != 0o600
        || metadata.uid() != 1000
        || metadata.gid() != 1000
        || metadata.len() > MAX_CANCEL_INTENT_BYTES
    {
        return Err(ServiceError::Internal(format!(
            "restart recovery: unpublished cancellation intent {} has unsafe type/mode/owner/size",
            next.display()
        )));
    }

    let committed = read_cancel_intent(results_dir, session_id)?.is_some();
    std::fs::remove_file(&next).map_err(|error| {
        ServiceError::Internal(io_msg(
            "restart recovery: remove unpublished cancellation intent",
            &next,
            &error,
        ))
    })?;
    sync_directory(
        next.parent()
            .expect("unpublished cancellation intent has result parent"),
        "restart recovery: sync unpublished cancellation-intent cleanup",
    )?;
    tracing::warn!(
        session_id,
        committed,
        path = %next.display(),
        "discarded a crash-left unpublished cancellation candidate; only the atomic rename is a committed intent"
    );
    Ok(())
}

fn delete_intent_exists(results_dir: &Path, session_id: &str) -> ServiceResult<bool> {
    path_entry_exists(
        &delete_intent_path(results_dir, session_id),
        "stat possible durable deletion intent",
    )
}

fn read_delete_intent(
    results_dir: &Path,
    session_id: &str,
) -> ServiceResult<Option<DeleteIntent>> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

    let path = delete_intent_path(results_dir, session_id);
    let mut file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(ServiceError::Internal(io_msg(
                "open durable deletion intent without following links",
                &path,
                &error,
            )));
        }
    };
    let metadata = file.metadata().map_err(|error| {
        ServiceError::Internal(io_msg("fstat opened deletion intent", &path, &error))
    })?;
    if !metadata.is_file()
        || metadata.permissions().mode() & 0o777 != 0o600
        || metadata.uid() != 1000
        || metadata.gid() != 1000
        || metadata.len() > MAX_DELETE_INTENT_BYTES
    {
        return Err(ServiceError::Internal(format!(
            "durable deletion intent {} has unsafe opened type/mode/owner/size",
            path.display()
        )));
    }
    let capacity = usize::try_from(metadata.len()).map_err(|_| {
        ServiceError::Internal(format!(
            "durable deletion intent {} is too large to address",
            path.display()
        ))
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    file.read_to_end(&mut bytes).map_err(|error| {
        ServiceError::Internal(io_msg("read opened deletion intent", &path, &error))
    })?;
    if bytes.len() as u64 != metadata.len() {
        return Err(ServiceError::Internal(format!(
            "durable deletion intent {} changed length while open",
            path.display()
        )));
    }
    let intent: DeleteIntent = serde_json::from_slice(&bytes).map_err(|error| {
        ServiceError::Internal(format!(
            "durable deletion intent {} is malformed: {error}",
            path.display()
        ))
    })?;
    if intent.schema_version != 1 || intent.session_id != session_id {
        return Err(ServiceError::Internal(format!(
            "durable deletion intent {} has identity/schema drift",
            path.display()
        )));
    }
    Ok(Some(intent))
}

fn persist_delete_intent(results_dir: &Path, session_id: &str) -> ServiceResult<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    if read_delete_intent(results_dir, session_id)?.is_some() {
        sync_directory(results_dir, "revalidate durable deletion-intent directory")?;
        return Ok(());
    }
    let final_path = delete_intent_path(results_dir, session_id);
    let next = delete_intent_next_path(results_dir, session_id);
    match std::fs::symlink_metadata(&next) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) => {
            return Err(ServiceError::Internal(format!(
                "unpublished deletion intent already exists at {}; restart reconciliation is required",
                next.display()
            )));
        }
        Err(error) => {
            return Err(ServiceError::Internal(io_msg(
                "stat unpublished deletion intent",
                &next,
                &error,
            )));
        }
    }
    let intent = DeleteIntent {
        schema_version: 1,
        session_id: session_id.to_string(),
    };
    let mut bytes = serde_json::to_vec_pretty(&intent).map_err(|error| {
        ServiceError::Internal(format!("serialize deletion intent for {session_id}: {error}"))
    })?;
    bytes.push(b'\n');
    let mut candidate_created = false;
    let transaction = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&next)
            .map_err(|error| {
                ServiceError::Internal(io_msg(
                    "create unpublished deletion intent",
                    &next,
                    &error,
                ))
            })?;
        candidate_created = true;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| {
                ServiceError::Internal(io_msg(
                    "write/sync unpublished deletion intent",
                    &next,
                    &error,
                ))
            })?;
        sync_directory(results_dir, "sync unpublished deletion intent")?;
        std::fs::rename(&next, &final_path).map_err(|error| {
            ServiceError::Internal(io_msg(
                "atomically publish durable deletion intent",
                &final_path,
                &error,
            ))
        })?;
        candidate_created = false;
        sync_directory(results_dir, "sync durable deletion-intent publication")
    })();
    match transaction {
        Ok(()) => Ok(()),
        Err(first_error) if read_delete_intent(results_dir, session_id)?.is_some() => {
            match sync_directory(results_dir, "retry deletion-intent publication barrier") {
                Ok(()) => {
                    tracing::warn!(
                        session_id,
                        error = %first_error,
                        "the initial deletion-intent directory sync failed, but its explicit retry succeeded"
                    );
                    Ok(())
                }
                Err(retry_error) => Err(ServiceError::Internal(format!(
                    "the deletion intent for {session_id} is visible and exact, but both directory durability barriers failed ({first_error}; retry: {retry_error}); repeat DELETE with this same handle"
                ))),
            }
        }
        Err(first_error) => {
            let mut rollback_errors = Vec::new();
            if candidate_created {
                match std::fs::remove_file(&next) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => rollback_errors.push(io_msg(
                        "remove unpublished deletion intent",
                        &next,
                        &error,
                    )),
                }
            }
            if let Err(error) =
                sync_directory(results_dir, "sync unpublished deletion-intent rollback")
            {
                rollback_errors.push(error.to_string());
            }
            if rollback_errors.is_empty() {
                Err(first_error)
            } else {
                Err(ServiceError::Internal(format!(
                    "{first_error}; deletion-intent rollback also failed: {}",
                    rollback_errors.join("; ")
                )))
            }
        }
    }
}

fn remove_exact_delete_target(path: &Path, parent: &Path, role: &str) -> ServiceResult<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(ServiceError::Internal(io_msg(
                &format!("delete: stat {role}"),
                path,
                &error,
            )));
        }
    };
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o777 != 0o755
        || metadata.uid() != 1000
        || metadata.gid() != 1000
    {
        return Err(ServiceError::Internal(format!(
            "delete: {role} {} is not the exact service-owned 1000:1000 mode-0755 ordinary directory; refusing recursive removal",
            path.display()
        )));
    }
    std::fs::remove_dir_all(path).map_err(|error| {
        ServiceError::Internal(io_msg(&format!("delete: remove {role}"), path, &error))
    })?;
    sync_directory(parent, &format!("delete: sync {role} removal"))
}

fn finish_delete_intent(
    state_dir: &Path,
    results_dir: &Path,
    session_id: &str,
) -> ServiceResult<()> {
    read_delete_intent(results_dir, session_id)?.ok_or_else(|| {
        ServiceError::Internal(format!(
            "delete: durable deletion intent for {session_id} disappeared before cleanup"
        ))
    })?;
    let sessions_parent = state_dir.join("sessions");
    remove_exact_delete_target(
        &sessions_parent.join(session_id),
        &sessions_parent,
        "raw session state",
    )?;
    remove_exact_delete_target(
        &results_dir.join(session_id),
        results_dir,
        "terminal result directory",
    )?;
    let authority = delete_intent_path(results_dir, session_id);
    std::fs::remove_file(&authority).map_err(|error| {
        ServiceError::Internal(io_msg(
            "delete: remove completed deletion authority",
            &authority,
            &error,
        ))
    })?;
    sync_directory(
        results_dir,
        "delete: sync completed deletion-authority removal",
    )
}

fn reconcile_unpublished_delete_intent(
    results_dir: &Path,
    session_id: &str,
) -> ServiceResult<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let next = delete_intent_next_path(results_dir, session_id);
    let metadata = match std::fs::symlink_metadata(&next) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(ServiceError::Internal(io_msg(
                "restart recovery: stat unpublished deletion intent",
                &next,
                &error,
            )));
        }
    };
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o777 != 0o600
        || metadata.uid() != 1000
        || metadata.gid() != 1000
        || metadata.len() > MAX_DELETE_INTENT_BYTES
    {
        return Err(ServiceError::Internal(format!(
            "restart recovery: unpublished deletion intent {} has unsafe type/mode/owner/size",
            next.display()
        )));
    }
    std::fs::remove_file(&next).map_err(|error| {
        ServiceError::Internal(io_msg(
            "restart recovery: remove unpublished deletion intent",
            &next,
            &error,
        ))
    })?;
    sync_directory(results_dir, "restart recovery: sync deletion-intent rollback")?;
    tracing::warn!(
        session_id,
        committed = delete_intent_exists(results_dir, session_id)?,
        "discarded an unpublished deletion candidate; only the durable authority file commits deletion"
    );
    Ok(())
}

/// Complete every crash-left durable DELETE before ordinary acceptance and
/// terminal recovery inspect the results namespace. An unpublished `.next`
/// file is rollback evidence only and is never promoted into authority.
pub async fn recover_interrupted_deletions(cfg: &Config) -> ServiceResult<()> {
    let mut controls = Vec::new();
    for entry in std::fs::read_dir(&cfg.results_dir).map_err(|error| {
        ServiceError::Internal(io_msg(
            "restart deletion recovery: read results directory",
            &cfg.results_dir,
            &error,
        ))
    })? {
        let entry = entry.map_err(|error| {
            ServiceError::Internal(io_msg(
                "restart deletion recovery: read results entry",
                &cfg.results_dir,
                &error,
            ))
        })?;
        let name = entry.file_name().into_string().map_err(|_| {
            ServiceError::Internal(format!(
                "restart deletion recovery: non-UTF-8 results entry at {}",
                entry.path().display()
            ))
        })?;
        if name.starts_with(DELETE_INTENT_PREFIX) {
            let (session_id, unpublished) = session_id_from_delete_control_name(&name)
                .ok_or_else(|| {
                    ServiceError::Internal(format!(
                        "restart deletion recovery: malformed control entry {name:?}"
                    ))
                })?;
            controls.push((session_id.to_string(), unpublished));
        }
    }
    controls.sort();
    for (session_id, unpublished) in &controls {
        if *unpublished {
            reconcile_unpublished_delete_intent(&cfg.results_dir, session_id)?;
        }
    }
    let committed = controls
        .into_iter()
        .filter_map(|(session_id, unpublished)| (!unpublished).then_some(session_id))
        .collect::<std::collections::BTreeSet<_>>();
    for session_id in committed {
        finish_delete_intent(&cfg.state_dir, &cfg.results_dir, &session_id)?;
        tracing::warn!(
            session_id,
            "completed a durable deletion interrupted by a prior service process"
        );
    }
    Ok(())
}

pub(crate) fn is_current_session_id(s: &str) -> bool {
    canonical_hex_session_id(s, 64)
}

fn canonical_hex_session_id(s: &str, hex_len: usize) -> bool {
    s.len() == hex_len + 2
        && s.starts_with("s-")
        && s[2..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn finished_json_path(cfg: &Config, session_id: &str) -> PathBuf {
    cfg.results_dir.join(session_id).join("finished.json")
}

fn path_entry_exists(path: &Path, context: &str) -> ServiceResult<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(ServiceError::Internal(io_msg(context, path, &error))),
    }
}

fn resolve_disk_session(cfg: &Config, session_id: &str) -> ServiceResult<SessionResolution> {
    if !is_safe_session_id(session_id) {
        return Err(ServiceError::InvalidRequest(format!(
            "session_id {session_id:?} is not a supported canonical lowercase-hex session shape"
        )));
    }
    if delete_intent_exists(&cfg.results_dir, session_id)? {
        return Err(ServiceError::SessionDeleting {
            session_id: session_id.to_string(),
        });
    }
    let final_path = finished_json_path(cfg, session_id);
    let temporary_path = final_path.with_file_name("finished.json.tmp");
    if path_entry_exists(&final_path, "resolve disk session: stat terminal record")?
        || path_entry_exists(
            &temporary_path,
            "resolve disk session: stat terminal publication draft",
        )?
    {
        return Ok(SessionResolution::DiskTerminal);
    }
    if acceptance_exists(&cfg.results_dir, session_id)? {
        return Err(ServiceError::Internal(format!(
            "durably accepted session {session_id} has neither an in-memory supervisor nor a terminal publication; restart recovery is required"
        )));
    }
    Err(ServiceError::NotFound {
        session_id: session_id.to_string(),
    })
}

/// Execute the one terminal prepare/cleanup/commit transaction.  This is
/// shared by the live supervisor and startup recovery so neither path can
/// invent a weaker publication order.
pub(crate) async fn persist_terminal_transaction(
    cfg: &Config,
    body: &mut SessionBody,
) -> ServiceResult<()> {
    prepare_terminal(&cfg.results_dir, body).await?;
    finish_prepared_terminal_transaction(cfg, body).await
}

/// Resume a crash-left private terminal draft through the exact same
/// cleanup/retention/publication protocol used by the live supervisor.  A
/// draft is never made public merely because it parses: its acceptance,
/// progress history, bundle metadata, model identity, and raw-state
/// disposition are all revalidated first.
pub(crate) async fn resume_prepared_terminal_transaction(
    cfg: &Config,
    session_id: &str,
) -> ServiceResult<()> {
    if !is_safe_session_id(session_id) {
        return Err(ServiceError::InvalidRequest(format!(
            "resume terminal: session_id {session_id:?} is not a supported canonical session handle"
        )));
    }
    let result_dir = cfg.results_dir.join(session_id);
    let finished = result_dir.join("finished.json");
    if path_entry_exists(&finished, "resume terminal: stat committed terminal")? {
        return Err(ServiceError::Internal(format!(
            "resume terminal: committed terminal already exists at {}; generic linked-publication reconciliation owns this state",
            finished.display()
        )));
    }
    let temporary = result_dir.join("finished.json.tmp");
    let mut body = read_terminal_body_file(&temporary, session_id, "prepared terminal draft")?;
    validate_terminal_resource(cfg, session_id, &body)?;
    crate::api::validate_terminal_storage(&result_dir, &body, 1000, 1000)?;
    finish_prepared_terminal_transaction(cfg, &mut body).await
}

/// Complete the cleanup/retention/publication phases for an already durable
/// private terminal draft.  Live terminalization and startup recovery call
/// this same function so a restart cannot weaken or reorder the transaction.
async fn finish_prepared_terminal_transaction(
    cfg: &Config,
    body: &mut SessionBody,
) -> ServiceResult<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let result_dir = cfg.results_dir.join(&body.session_id);
    crate::api::validate_terminal_storage(&result_dir, body, 1000, 1000)?;
    if body.raw_session_tree_retained {
        // A retained-state claim is not self-authenticating.  The exact raw
        // tree and its durable cause marker are the deletion/recovery
        // authority, so prove both before publishing a terminal record that
        // refers to them.  In particular, a failed marker write must leave the
        // prepared terminal private rather than commit an internally
        // contradictory resource.
        validate_terminal_state_storage(cfg, body)?;
        return commit_prepared_terminal(&cfg.results_dir, &body.session_id).await;
    }

    // A terminal with no accepted bundle has no authority to erase an exact
    // raw session tree. Every legitimate bundle-failure path marks and
    // retains that tree before preparing the terminal. If a crash-left draft
    // contradicts that invariant, preserve both sources of evidence and stop
    // recovery instead of turning malformed metadata into data loss.
    if body.bundle_sha256.is_empty() {
        let state_root = cfg.state_dir.join("sessions").join(&body.session_id);
        match std::fs::symlink_metadata(&state_root) {
            Ok(_) => {
                return Err(ServiceError::Internal(format!(
                    "terminal {} has no accepted bundle and does not claim retained raw evidence, but raw state still exists at {}; refusing cleanup/publication",
                    body.session_id,
                    state_root.display()
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(ServiceError::Internal(io_msg(
                    "terminalization: stat raw state for bundleless terminal",
                    &state_root,
                    &error,
                )));
            }
        }
    }

    let cleanup_diagnostics =
        remove_terminalized_state(&cfg.state_dir, &body.session_id, 1000, 1000);
    if cleanup_diagnostics.is_empty() {
        return commit_prepared_terminal(&cfg.results_dir, &body.session_id).await;
    }

    body.is_process_error = true;
    body.teardown_diagnostics.extend(cleanup_diagnostics);
    let state_root = cfg.state_dir.join("sessions").join(&body.session_id);
    match std::fs::symlink_metadata(&state_root) {
        Ok(metadata)
            if metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && metadata.permissions().mode() & 0o777 == 0o755
                && metadata.uid() == 1000
                && metadata.gid() == 1000 =>
        {
            if body.bundle_sha256.is_empty() {
                return Err(ServiceError::Internal(format!(
                    "terminal raw-state cleanup failed for {} without an accepted bundle; refusing to invent a retained-bundle cleanup state",
                    body.session_id
                )));
            }
            body.raw_session_tree_retained = true;
            body.teardown_diagnostics.push(format!(
                "terminal raw-state cleanup failed after the bundle was accepted; the exact tree is durably retained at {}",
                state_root.display()
            ));
            // Rewrite first. If marker publication then fails, restart sees
            // a draft that requires retained evidence and refuses to delete
            // the still-present tree merely because the marker is absent.
            rewrite_prepared_terminal(&cfg.results_dir, body, 1000, 1000)
                .await
                .map_err(|error| {
                    ServiceError::Internal(format!(
                        "terminal state cleanup failed and the retained-state terminal draft could not be updated: {error}"
                    ))
            })?;
            publish_cleanup_retention_marker(&cfg.state_dir, &body.session_id)?;
            validate_terminal_state_storage(cfg, body)?;
            commit_prepared_terminal(&cfg.results_dir, &body.session_id).await
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // The namespace removal became visible but its parent fsync
            // failed. Do not claim the deletion durable. Leave the updated
            // draft unpublished; startup can re-observe whether the tree
            // stayed absent or reappeared and complete the same protocol.
            body.teardown_diagnostics.push(format!(
                "terminal raw-state removal is visible but its durability barrier failed; leaving the terminal draft unpublished for restart reconciliation at {}",
                state_root.display()
            ));
            rewrite_prepared_terminal(&cfg.results_dir, body, 1000, 1000)
                .await
                .map_err(|rewrite_error| {
                    ServiceError::Internal(format!(
                        "terminal state cleanup durability failed and the terminal draft could not be updated: {rewrite_error}"
                    ))
                })?;
            Err(ServiceError::Internal(format!(
                "terminal raw-state removal for {} is visible but not durably proved; terminal publication remains prepared but uncommitted",
                body.session_id
            )))
        }
        Ok(metadata) => {
            body.teardown_diagnostics.push(format!(
                "terminal raw-state cleanup left an unsafe object at {}: type={:?} mode={:o} uid={} gid={}",
                state_root.display(),
                metadata.file_type(),
                metadata.permissions().mode() & 0o777,
                metadata.uid(),
                metadata.gid()
            ));
            rewrite_prepared_terminal(&cfg.results_dir, body, 1000, 1000)
                .await
                .map_err(|rewrite_error| {
                    ServiceError::Internal(format!(
                        "terminal state cleanup left an unsafe object and the terminal draft could not be updated: {rewrite_error}"
                    ))
                })?;
            Err(ServiceError::Internal(format!(
                "terminal raw-state cleanup for {} left an unsafe object; terminal publication remains prepared but uncommitted",
                body.session_id
            )))
        }
        Err(error) => Err(ServiceError::Internal(io_msg(
            "terminalization: restat raw session tree after cleanup failure",
            &state_root,
            &error,
        ))),
    }
}

fn publish_cleanup_retention_marker(state_dir: &Path, session_id: &str) -> ServiceResult<()> {
    use std::io::Write;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

    let root = state_dir.join("sessions").join(session_id);
    let control = root.join("control");
    for (role, path, mode) in [
        ("retained raw session root", &root, 0o755),
        ("retained raw session control directory", &control, 0o755),
    ] {
        let metadata = std::fs::symlink_metadata(path).map_err(|error| {
            ServiceError::Internal(io_msg(&format!("stat {role}"), path, &error))
        })?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata.permissions().mode() & 0o777 != mode
            || metadata.uid() != 1000
            || metadata.gid() != 1000
        {
            return Err(ServiceError::Internal(format!(
                "{role} {} has unsafe type/mode/owner",
                path.display()
            )));
        }
    }
    let marker = control.join("raw-evidence-retained.txt");
    let bytes = format!(
        "RAW_SESSION_TREE_RETAINED\ncause=raw-state-cleanup-failure\npath={}\n",
        root.display()
    );
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&marker)
        .map_err(|error| {
            ServiceError::Internal(io_msg(
                "create raw-state cleanup retention marker",
                &marker,
                &error,
            ))
        })?;
    file.write_all(bytes.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|error| {
            ServiceError::Internal(io_msg(
                "write/sync raw-state cleanup retention marker",
                &marker,
                &error,
            ))
        })?;
    sync_directory(&control, "sync raw-state cleanup retention marker")
}

/// Durable prepare phase for terminal publication. The complete terminal body
/// is written to a private, no-clobber `finished.json.tmp` and both the file and
/// containing directory are synced before raw session state may be removed.
async fn prepare_terminal(results_dir: &Path, body: &SessionBody) -> ServiceResult<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let dir = results_dir.join(&body.session_id);
    crate::bundle::ensure_service_owned_result_directory(&dir)?;
    let final_path = dir.join("finished.json");
    let tmp_path = dir.join("finished.json.tmp");
    for path in [&final_path, &tmp_path] {
        match std::fs::symlink_metadata(path) {
            Ok(_) => {
                return Err(ServiceError::Internal(format!(
                    "prepare_terminal({}): refusing to overwrite existing {}",
                    body.session_id,
                    path.display()
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(ServiceError::Internal(io_msg(
                    "prepare_terminal: stat destination",
                    path,
                    &error,
                )));
            }
        }
    }
    let bytes = serde_json::to_vec_pretty(body).map_err(|e| {
        ServiceError::Internal(format!(
            "prepare_terminal({}): serde_json failure on terminal SessionBody: {e}",
            body.session_id
        ))
    })?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp_path)
        .map_err(|error| {
            ServiceError::Internal(io_msg("prepare_terminal: create tmp", &tmp_path, &error))
        })?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| {
            ServiceError::Internal(io_msg(
                "prepare_terminal: write/sync tmp",
                &tmp_path,
                &error,
            ))
        })?;
    sync_directory(&dir, "prepare_terminal: sync result directory")?;
    Ok(())
}

/// Rewrite the still-unpublished terminal draft after a raw-state cleanup
/// failure has been added to the public diagnostics. A crash during this rare
/// double-failure path leaves an unparseable draft and the original evidence;
/// startup then refuses recovery instead of publishing guessed metadata.
async fn rewrite_prepared_terminal(
    results_dir: &Path,
    body: &SessionBody,
    service_uid: u32,
    service_gid: u32,
) -> ServiceResult<()> {
    use std::io::{Seek, SeekFrom, Write};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

    let dir = results_dir.join(&body.session_id);
    let final_path = dir.join("finished.json");
    let tmp_path = dir.join("finished.json.tmp");
    match std::fs::symlink_metadata(&final_path) {
        Ok(_) => {
            return Err(ServiceError::Internal(format!(
                "rewrite_prepared_terminal({}): committed destination unexpectedly exists at {}",
                body.session_id,
                final_path.display()
            )));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(ServiceError::Internal(io_msg(
                "rewrite_prepared_terminal: stat final",
                &final_path,
                &error,
            )));
        }
    }
    let metadata = std::fs::symlink_metadata(&tmp_path).map_err(|error| {
        ServiceError::Internal(io_msg(
            "rewrite_prepared_terminal: stat tmp",
            &tmp_path,
            &error,
        ))
    })?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o777 != 0o600
        || metadata.uid() != service_uid
        || metadata.gid() != service_gid
    {
        return Err(ServiceError::Internal(format!(
            "rewrite_prepared_terminal({}): {} has unsafe type/mode/owner",
            body.session_id,
            tmp_path.display()
        )));
    }
    let bytes = serde_json::to_vec_pretty(body).map_err(|error| {
        ServiceError::Internal(format!(
            "rewrite_prepared_terminal({}): serialize updated terminal: {error}",
            body.session_id
        ))
    })?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(&tmp_path)
        .map_err(|error| {
            ServiceError::Internal(io_msg(
                "rewrite_prepared_terminal: open tmp without following links",
                &tmp_path,
                &error,
            ))
        })?;
    let opened_metadata = file.metadata().map_err(|error| {
        ServiceError::Internal(io_msg(
            "rewrite_prepared_terminal: fstat opened tmp",
            &tmp_path,
            &error,
        ))
    })?;
    if !opened_metadata.is_file()
        || opened_metadata.permissions().mode() & 0o777 != 0o600
        || opened_metadata.uid() != service_uid
        || opened_metadata.gid() != service_gid
    {
        return Err(ServiceError::Internal(format!(
            "rewrite_prepared_terminal({}): opened {} changed to an unsafe type/mode/owner",
            body.session_id,
            tmp_path.display()
        )));
    }
    file.set_len(0)
        .and_then(|_| file.seek(SeekFrom::Start(0)))
        .and_then(|_| file.write_all(&bytes))
        .and_then(|_| file.sync_all())
        .map_err(|error| {
            ServiceError::Internal(io_msg(
                "rewrite_prepared_terminal: write/sync tmp",
                &tmp_path,
                &error,
            ))
        })?;
    sync_directory(&dir, "rewrite_prepared_terminal: sync result directory")
}

/// No-clobber commit of a previously fsynced terminal draft. Once the hard
/// link is created we never roll it back on a later cleanup/fsync error; both
/// names are valuable recoverable evidence and startup knows how to reconcile
/// the exact same-inode pair.
async fn commit_prepared_terminal(results_dir: &Path, session_id: &str) -> ServiceResult<()> {
    let dir = results_dir.join(session_id);
    let final_path = dir.join("finished.json");
    let tmp_path = dir.join("finished.json.tmp");
    match std::fs::symlink_metadata(&final_path) {
        Ok(_) => {
            return Err(ServiceError::Internal(format!(
                "commit_prepared_terminal({session_id}): refusing to overwrite existing {}",
                final_path.display()
            )));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(ServiceError::Internal(io_msg(
                "commit_prepared_terminal: stat final",
                &final_path,
                &error,
            )));
        }
    }
    std::fs::hard_link(&tmp_path, &final_path).map_err(|error| {
        ServiceError::Internal(format!(
            "commit_prepared_terminal({session_id}): no-clobber publish {} -> {}: {error}",
            tmp_path.display(),
            final_path.display()
        ))
    })?;
    sync_directory(&dir, "commit_prepared_terminal: sync published final")?;
    std::fs::remove_file(&tmp_path).map_err(|error| {
        ServiceError::Internal(io_msg(
            "commit_prepared_terminal: remove published tmp link",
            &tmp_path,
            &error,
        ))
    })?;
    sync_directory(&dir, "commit_prepared_terminal: sync tmp-link removal")
}

/// Grant owner-write on every directory in `root`'s subtree so a subsequent
/// recursive removal can unlink their contents. A session workspace can hold
/// tool-created read-only directory trees -- most importantly Go's module
/// cache, whose directories are mode 0555 -- and a directory's entries cannot
/// be unlinked unless the directory itself is writable, so `remove_dir_all`
/// otherwise fails with EPERM and strands the raw session tree. Only
/// directories are modified and symlinks are never traversed; the caller has
/// already proved `root` is the exact service-owned session tree, so nothing
/// outside it is touched. Best-effort: any per-entry error is left for the
/// real removal to surface with its precise context. The walk is iterative so
/// a deep cache tree cannot overflow the stack.
fn grant_owner_write_recursively(root: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let meta = match std::fs::symlink_metadata(&dir) {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        if meta.file_type().is_symlink() || !meta.is_dir() {
            continue;
        }
        let mode = meta.permissions().mode();
        if mode & 0o200 == 0 {
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(mode | 0o700));
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let child = entry.path();
            match std::fs::symlink_metadata(&child) {
                Ok(child_meta)
                    if child_meta.is_dir() && !child_meta.file_type().is_symlink() =>
                {
                    stack.push(child);
                }
                _ => {}
            }
        }
    }
}

fn remove_terminalized_state(
    state_dir: &Path,
    session_id: &str,
    service_uid: u32,
    service_gid: u32,
) -> Vec<String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let path = state_dir.join("sessions").join(session_id);
    let parent = state_dir.join("sessions");
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            return vec![io_msg(
                "terminalization: stat raw session tree",
                &path,
                &error,
            )];
        }
    };
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o777 != 0o755
        || metadata.uid() != service_uid
        || metadata.gid() != service_gid
    {
        return vec![format!(
            "terminalization: raw session path {} is not the exact service-owned {}:{} mode-0755 ordinary directory; refusing recursive removal",
            path.display(), service_uid, service_gid
        )];
    }
    // The agent may have created read-only directory trees inside its workspace
    // (Go's module cache marks directories mode 0555); make every directory in
    // this exact, validated, service-owned tree owner-writable so the recursive
    // removal below can unlink their contents instead of stranding the tree.
    grant_owner_write_recursively(&path);
    if let Err(error) = std::fs::remove_dir_all(&path) {
        return vec![io_msg(
            "terminalization: remove raw session tree",
            &path,
            &error,
        )];
    }
    if let Err(error) = sync_directory(&parent, "terminalization: sync sessions directory") {
        return vec![error.to_string()];
    }
    Vec::new()
}

fn sync_directory(path: &std::path::Path, context: &str) -> ServiceResult<()> {
    std::fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| ServiceError::Internal(io_msg(context, path, &error)))
}

fn validate_delete_state_marker(
    state_root: &std::path::Path,
    terminal: &SessionBody,
) -> ServiceResult<()> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

    let marker = state_root.join("control/raw-evidence-retained.txt");
    let opened = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(&marker)
    {
        Ok(file) => Some(file),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(ServiceError::Internal(io_msg(
                "delete: open raw-evidence marker without following links",
                &marker,
                &error,
            )));
        }
    };
    match (terminal.raw_session_tree_retained, opened) {
        (false, None) => Ok(()),
        (false, Some(_)) => Err(ServiceError::Internal(format!(
            "delete: terminal {} does not claim retained evidence but marker {} exists",
            terminal.session_id,
            marker.display()
        ))),
        (true, None) => Err(ServiceError::Internal(format!(
            "delete: terminal {} claims retained evidence but marker {} is absent",
            terminal.session_id,
            marker.display()
        ))),
        (true, Some(mut file)) => {
            let metadata = file.metadata().map_err(|error| {
                ServiceError::Internal(io_msg(
                    "delete: fstat opened raw-evidence marker",
                    &marker,
                    &error,
                ))
            })?;
            if !metadata.is_file()
                || metadata.permissions().mode() & 0o777 != 0o600
                || metadata.uid() != 1000
                || metadata.gid() != 1000
                || metadata.len() > 4096
            {
                return Err(ServiceError::Internal(format!(
                    "delete: retained-evidence marker {} has unsafe opened type/mode/owner/size",
                    marker.display()
                )));
            }
            let capacity = usize::try_from(metadata.len()).map_err(|_| {
                ServiceError::Internal(format!(
                    "delete: retained-evidence marker {} is too large to address",
                    marker.display()
                ))
            })?;
            let mut bytes = Vec::with_capacity(capacity);
            file.read_to_end(&mut bytes).map_err(|error| {
                ServiceError::Internal(io_msg(
                    "delete: read opened raw-evidence marker",
                    &marker,
                    &error,
                ))
            })?;
            if bytes.len() as u64 != metadata.len() {
                return Err(ServiceError::Internal(format!(
                    "delete: retained-evidence marker {} changed length while open",
                    marker.display()
                )));
            }
            let contents = String::from_utf8(bytes).map_err(|error| {
                ServiceError::Internal(format!(
                    "delete: retained-evidence marker {} is not UTF-8: {error}",
                    marker.display()
                ))
            })?;
            let expected_path = format!("path={}", state_root.display());
            let mut lines = contents.lines();
            let header = lines.next();
            let cause = lines.next().unwrap_or_default();
            let path = lines.next();
            let known_cause = matches!(
                cause,
                "cause=required-bundle-failure"
                    | "cause=panic-recovery-bundle-failure"
                    | "cause=setup-forensic-bundle-failure"
                    | "cause=service-restart-bundle-failure"
                    | "cause=container-quiescence-unproved"
                    | "cause=container-teardown-incomplete"
                    | "cause=raw-state-cleanup-failure"
            );
            let cause_matches_bundle = if terminal.bundle_sha256.is_empty() {
                matches!(
                    cause,
                    "cause=required-bundle-failure"
                        | "cause=panic-recovery-bundle-failure"
                        | "cause=setup-forensic-bundle-failure"
                        | "cause=service-restart-bundle-failure"
                        | "cause=container-quiescence-unproved"
                )
            } else {
                matches!(
                    cause,
                    "cause=container-teardown-incomplete"
                        | "cause=raw-state-cleanup-failure"
                )
            };
            if header != Some("RAW_SESSION_TREE_RETAINED")
                || !known_cause
                || !cause_matches_bundle
                || path != Some(expected_path.as_str())
                || lines.next().is_some()
            {
                return Err(ServiceError::Internal(format!(
                    "delete: retained-evidence marker {} has invalid contents",
                    marker.display()
                )));
            }
            Ok(())
        }
    }
}

/// Read the on-disk terminal record. `NotFound` if the directory or
/// finished.json doesn't exist; `Internal` on read or parse failure.
async fn read_terminal(cfg: &Config, session_id: &str) -> ServiceResult<SessionBody> {
    if !is_safe_session_id(session_id) {
        return Err(ServiceError::InvalidRequest(format!(
            "session_id {session_id:?} is not a supported canonical lowercase-hex session shape"
        )));
    }
    let path = finished_json_path(cfg, session_id);
    let temporary_path = path.with_file_name("finished.json.tmp");
    match std::fs::symlink_metadata(&temporary_path) {
        Ok(_) => {
            return Err(ServiceError::Internal(format!(
                "read_terminal({session_id}): incomplete publication marker remains at {}",
                temporary_path.display()
            )));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(ServiceError::Internal(io_msg(
                "read_terminal: stat temporary publication marker",
                &temporary_path,
                &error,
            )));
        }
    }
    let body = read_terminal_body_file(&path, session_id, "committed terminal")?;
    validate_terminal_resource(cfg, session_id, &body)?;
    crate::api::validate_terminal_storage(
        &cfg.results_dir.join(session_id),
        &body,
        1000,
        1000,
    )?;
    validate_terminal_state_storage(cfg, &body)?;
    Ok(body)
}

fn read_terminal_body_file(
    path: &Path,
    session_id: &str,
    role: &str,
) -> ServiceResult<SessionBody> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

    let mut file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ServiceError::NotFound {
                session_id: session_id.to_string(),
            });
        }
        Err(error) => {
            return Err(ServiceError::Internal(io_msg(
                &format!("open {role} without following links"),
                path,
                &error,
            )));
        }
    };
    let metadata = file.metadata().map_err(|error| {
        ServiceError::Internal(io_msg(&format!("fstat opened {role}"), path, &error))
    })?;
    if !metadata.is_file()
        || metadata.permissions().mode() & 0o777 != 0o600
        || metadata.uid() != 1000
        || metadata.gid() != 1000
        || metadata.len() > MAX_TERMINAL_RECORD_BYTES
    {
        return Err(ServiceError::Internal(format!(
            "{role} for {session_id} at {} has unsafe opened type/mode/owner/size: type={:?} mode={:o} uid={} gid={} size={} expected=1000:1000 mode=0600 max={MAX_TERMINAL_RECORD_BYTES}",
            path.display(),
            metadata.file_type(),
            metadata.permissions().mode() & 0o777,
            metadata.uid(),
            metadata.gid(),
            metadata.len()
        )));
    }
    let capacity = usize::try_from(metadata.len()).map_err(|_| {
        ServiceError::Internal(format!(
            "{role} for {session_id} at {} is too large to address on this platform",
            path.display()
        ))
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    file.read_to_end(&mut bytes).map_err(|error| {
        ServiceError::Internal(io_msg(&format!("read opened {role}"), path, &error))
    })?;
    if bytes.len() as u64 != metadata.len() {
        return Err(ServiceError::Internal(format!(
            "{role} for {session_id} at {} changed length while open: fstat={} read={}",
            path.display(),
            metadata.len(),
            bytes.len()
        )));
    }
    let body = serde_json::from_slice::<SessionBody>(&bytes).map_err(|error| {
        ServiceError::Internal(format!(
            "{role} for {session_id} at {} is malformed JSON or has the wrong shape: {error}",
            path.display()
        ))
    })?;
    if body.session_id != session_id || body.status == SessionStatus::Running {
        return Err(ServiceError::Internal(format!(
            "{role} at {} has terminal identity/status drift for {session_id}",
            path.display()
        )));
    }
    Ok(body)
}

fn validate_terminal_state_storage(cfg: &Config, body: &SessionBody) -> ServiceResult<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let state_root = cfg.state_dir.join("sessions").join(&body.session_id);
    match std::fs::symlink_metadata(&state_root) {
        Ok(metadata) if !body.raw_session_tree_retained => Err(ServiceError::Internal(format!(
            "terminal {} does not claim retained raw evidence but state path {} exists with type {:?}",
            body.session_id,
            state_root.display(),
            metadata.file_type()
        ))),
        Ok(metadata)
            if metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && metadata.permissions().mode() & 0o777 == 0o755
                && metadata.uid() == 1000
                && metadata.gid() == 1000 =>
        {
            validate_delete_state_marker(&state_root, body)
        }
        Ok(_) => Err(ServiceError::Internal(format!(
            "terminal {} claims retained raw evidence but {} is not the exact service-owned 1000:1000 mode-0755 ordinary directory",
            body.session_id,
            state_root.display()
        ))),
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                && !body.raw_session_tree_retained =>
        {
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(ServiceError::Internal(format!(
                "terminal {} claims retained raw evidence but its state tree is absent at {}",
                body.session_id,
                state_root.display()
            )))
        }
        Err(error) => Err(ServiceError::Internal(io_msg(
            "validate terminal raw-state disposition",
            &state_root,
            &error,
        ))),
    }
}

fn validate_terminal_resource(
    cfg: &Config,
    session_id: &str,
    body: &SessionBody,
) -> ServiceResult<()> {
    if body.session_id != session_id || body.status == SessionStatus::Running {
        return Err(ServiceError::Internal(format!(
            "read_terminal({session_id}): terminal identity/status drift: body_id={:?} status={:?}",
            body.session_id, body.status
        )));
    }
    if body.model != cfg.lock.backend.served_model
        || body.context_window != cfg.lock.backend.max_model_len
    {
        return Err(ServiceError::Internal(format!(
            "read_terminal({session_id}): model/context drift: model={:?} context={} expected={:?}/{}",
            body.model,
            body.context_window,
            cfg.lock.backend.served_model,
            cfg.lock.backend.max_model_len
        )));
    }
    if body.started_at_unix == 0 || body.finished_at_unix < body.started_at_unix {
        return Err(ServiceError::Internal(format!(
            "read_terminal({session_id}): impossible terminal timestamps: started={} finished={}",
            body.started_at_unix, body.finished_at_unix
        )));
    }

    // Current caller-generated resources always have the acceptance and full
    // progress documents introduced with the 256-bit handle protocol.
    // Historical 128-bit terminals remain readable without fabricating a
    // migration for records that genuinely predate those documents.
    if is_current_session_id(session_id) {
        let acceptance = read_acceptance(&cfg.results_dir, session_id)?;
        if acceptance.accepted_at_unix != body.started_at_unix
            || acceptance.preserve_thinking != body.preserve_thinking
            || acceptance.max_session_turns != body.max_session_turns
            || preview(&acceptance.prompt) != body.prompt_preview
        {
            return Err(ServiceError::Internal(format!(
                "read_terminal({session_id}): terminal fields contradict the durable acceptance record"
            )));
        }
        let progress = progress_path(&cfg.results_dir, session_id);
        let progress_next = progress.with_file_name("progress.json.next");
        match std::fs::symlink_metadata(&progress_next) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => {
                return Err(ServiceError::Internal(format!(
                    "read_terminal({session_id}): unpublished progress replacement remains at {}",
                    progress_next.display()
                )));
            }
            Err(error) => {
                return Err(ServiceError::Internal(io_msg(
                    "read_terminal: stat unpublished progress replacement",
                    &progress_next,
                    &error,
                )));
            }
        }
        let events = crate::progress::read_progress_events(&progress, session_id)?;
        if events != body.progress_events {
            return Err(ServiceError::Internal(format!(
                "read_terminal({session_id}): embedded progress history differs from progress.json"
            )));
        }
        let latest = events.last().ok_or_else(|| {
            ServiceError::Internal(format!(
                "read_terminal({session_id}): durable progress history is empty"
            ))
        })?;
        if body.progress_revision != latest.revision
            || body.progress_at_unix_ms != latest.at_unix_ms
            || body.progress_phase != latest.phase
            || body.progress_message != latest.message
            || body.staged_bytes < latest.counters.staged_bytes
            || body.staged_entries < latest.counters.staged_entries
            || body.staged_regular_files < latest.counters.staged_regular_files
            || body.output_event_bytes < latest.counters.output_event_bytes
            || body.num_turns < latest.counters.num_turns
        {
            return Err(ServiceError::Internal(format!(
                "read_terminal({session_id}): terminal progress summary contradicts its latest durable event"
            )));
        }
    }
    Ok(())
}

/// Cheap defensive check before joining a session handle onto a path.  New
/// connection-independent handles carry 256 random bits (64 hex).  The
/// historical 128-bit/UUID-width shape remains readable and deletable so an
/// upgrade never strands already committed terminal evidence.
pub(crate) fn is_safe_session_id(s: &str) -> bool {
    canonical_hex_session_id(s, 32) || canonical_hex_session_id(s, 64)
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use super::{
        apply_progress, await_connection_independent, cancel_intent_next_path,
        cancel_intent_path, commit_prepared_terminal, delete_intent_next_path,
        delete_intent_path, finish_delete_intent, grant_owner_write_recursively,
        is_current_session_id, is_safe_session_id,
        persist_cancel_intent, persist_delete_intent, persist_terminal_transaction,
        prepare_durable_acceptance, prepare_terminal, read_cancel_intent, read_delete_intent,
        read_running_progress, read_terminal, reconcile_unpublished_cancel_intent,
        reconcile_unpublished_delete_intent, remove_terminalized_state,
        resume_prepared_terminal_transaction, rewrite_prepared_terminal, AcceptanceRecord,
        CancelIntent, LifecycleTracker, SessionBody, SessionStatus,
    };
    use crate::config::{Config, StackLock, STACK_LOCK_JSON};
    use crate::error::ServiceError;
    use crate::progress::ProgressPhase;
    use crate::staging::SessionPaths;

    struct TestTree(PathBuf);

    impl TestTree {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "qwen38-runtime-{label}-{}",
                uuid::Uuid::new_v4().simple()
            ));
            std::fs::create_dir(&path).expect("create isolated runtime fixture");
            Self(path)
        }
    }

    impl Drop for TestTree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn owner(path: &Path) -> (u32, u32) {
        let metadata = std::fs::metadata(path).expect("stat owned fixture");
        (metadata.uid(), metadata.gid())
    }

    fn private_write(path: &Path, bytes: &[u8]) {
        use std::os::unix::fs::OpenOptionsExt;

        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .expect("create private runtime fixture");
        file.write_all(bytes).expect("write private runtime fixture");
        file.sync_all().expect("sync private runtime fixture");
    }

    #[test]
    fn grant_owner_write_recursively_makes_readonly_dirs_removable() {
        let tree = TestTree::new("grant-write");
        // A Go-module-cache-shaped subtree: nested directories mode 0555 with a
        // read-only file at the bottom -- exactly what strands terminalization.
        let sub = tree.0.join("go-mod");
        std::fs::create_dir(&sub).expect("create sub");
        let leaf = sub.join("libc@v1.72.3");
        std::fs::create_dir(&leaf).expect("create leaf");
        let file = leaf.join("libc.go");
        std::fs::write(&file, b"package libc\n").expect("write leaf file");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o444)).unwrap();
        std::fs::set_permissions(&leaf, std::fs::Permissions::from_mode(0o555)).unwrap();
        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o555)).unwrap();

        grant_owner_write_recursively(&tree.0);

        for dir in [tree.0.as_path(), sub.as_path(), leaf.as_path()] {
            let mode = std::fs::symlink_metadata(dir).unwrap().permissions().mode();
            assert!(
                mode & 0o200 != 0,
                "directory {} is still not owner-writable: {:o}",
                dir.display(),
                mode
            );
        }
        std::fs::remove_dir_all(&tree.0)
            .expect("a read-only Go-cache-shaped subtree must be removable after granting write");
        assert!(!tree.0.exists(), "tree must be gone after removal");
    }

    fn make_service_owned(path: &Path) {
        let metadata = std::fs::symlink_metadata(path).expect("stat service-owned fixture");
        if metadata.uid() == 1000 && metadata.gid() == 1000 {
            return;
        }
        assert_eq!(
            unsafe { libc::geteuid() },
            0,
            "the pinned test environment is either uid 1000 or root so it can construct exact production ownership"
        );
        let path = std::ffi::CString::new(path.as_os_str().as_bytes())
            .expect("fixture path has no NUL byte");
        assert_eq!(
            unsafe { libc::chown(path.as_ptr(), 1000, 1000) },
            0,
            "chown exact service-owned fixture: {}",
            std::io::Error::last_os_error()
        );
    }

    fn encoded_cancel_intent(session_id: &str, requested_at_unix_ms: u64) -> Vec<u8> {
        let mut bytes = serde_json::to_vec_pretty(&CancelIntent {
            schema_version: 1,
            session_id: session_id.to_string(),
            requested_at_unix_ms,
        })
        .expect("serialize cancellation fixture");
        bytes.push(b'\n');
        bytes
    }

    fn body(session_id: &str) -> SessionBody {
        SessionBody {
            session_id: session_id.to_string(),
            status: SessionStatus::Completed,
            started_at_unix: 1,
            model: "qwen3.8-27b-nvfp4-k8v4".to_string(),
            context_window: 262_144,
            preserve_thinking: false,
            max_session_turns: crate::config::DEFAULT_MAX_SESSION_TURNS,
            archive_bytes: 1,
            archive_sha256: "1".repeat(64),
            prompt_preview: "fixture".to_string(),
            progress_revision: 1,
            progress_at_unix_ms: 1,
            progress_phase: ProgressPhase::Terminal,
            progress_message: "terminal fixture".to_string(),
            staged_bytes: 0,
            staged_entries: 0,
            staged_regular_files: 0,
            output_event_bytes: 0,
            progress_events: Vec::new(),
            num_turns: 1,
            last_event_at_unix: 1,
            finished_at_unix: 2,
            duration_wall_ms: 1,
            container_exit_code: 0,
            agent_exit_code: 0,
            is_process_error: false,
            response: "done".to_string(),
            agent_duration_ms: 1,
            bundle_sha256: String::new(),
            bundle_compressed_bytes: 0,
            bundle_uncompressed_bytes: 0,
            bundle_file_count: 0,
            bundle_artifacts_file_count: 0,
            raw_session_tree_retained: false,
            teardown_diagnostics: Vec::new(),
        }
    }

    fn test_config(state_dir: PathBuf, results_dir: PathBuf) -> Config {
        let lock: StackLock =
            serde_json::from_str(STACK_LOCK_JSON).expect("compiled stack lock must parse");
        Config {
            listen_addr: lock.service.listen.parse().expect("locked listen address"),
            state_dir,
            results_dir,
            broker_socket: PathBuf::from(&lock.broker.socket_path),
            model_socket: PathBuf::from(&lock.relay.model_socket_dir).join("relay.sock"),
            agent_image: lock.agent.image_tag.clone(),
            vllm_model_name: lock.backend.served_model.clone(),
            vllm_endpoint: lock.backend.endpoint.clone(),
            lock,
        }
    }

    /// Reading a live running session must not re-enter the running-map
    /// mutex while the lookup's guard is still alive. Before the bind-first
    /// fix in `Manager::get`, the edition-2021 `if let` scrutinee guard
    /// survived the whole success block, so the second `inner` lock in
    /// `running_or_terminal_snapshot` self-deadlocked every read of a
    /// running session until its HTTP peer disconnected.
    #[tokio::test]
    async fn get_of_running_session_does_not_self_deadlock() {
        let tree = TestTree::new("running-read-deadlock");
        let state_dir = tree.0.join("state");
        let results_dir = tree.0.join("results");
        std::fs::create_dir(&state_dir).expect("create state root");
        std::fs::create_dir(&results_dir).expect("create results root");
        let cfg = Arc::new(test_config(state_dir.clone(), results_dir));
        let manager = super::Manager::new(Arc::clone(&cfg));

        let session_id =
            "s-1111111111111111111111111111111111111111111111111111111111111111".to_string();
        let progress = crate::progress::ProgressReporter::create(
            &state_dir.join("progress.json"),
            &session_id,
            "accepted for the running-read deadlock regression fixture",
        )
        .expect("create progress fixture");
        let entry = Arc::new(super::RunningEntry {
            snapshot: super::RunningSnapshot {
                session_id: session_id.clone(),
                started_at_unix: 1,
                prompt_preview: "running-read deadlock regression fixture".to_string(),
                model: cfg.vllm_model_name.clone(),
                context_window: 262_144,
                preserve_thinking: false,
                max_session_turns: crate::config::DEFAULT_MAX_SESSION_TURNS,
                archive_bytes: 22,
                archive_sha256: "0".repeat(64),
            },
            acceptance: AcceptanceRecord {
                schema_version: 2,
                session_id: session_id.clone(),
                accepted_at_unix: 1,
                archive_bytes: 22,
                archive_sha256: "0".repeat(64),
                prompt: "running-read deadlock regression fixture".to_string(),
                preserve_thinking: false,
                max_session_turns: crate::config::DEFAULT_MAX_SESSION_TURNS,
            },
            progress,
            cancel: tokio_util::sync::CancellationToken::new(),
            launch_decision: Arc::new(tokio::sync::Mutex::new(())),
            terminal_decision: tokio::sync::Mutex::new(super::TerminalDecision::Open),
        });
        manager
            .inner
            .lock()
            .await
            .running
            .insert(session_id.clone(), entry);

        let body = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            manager.get(&session_id),
        )
        .await
        .expect("running-session read must complete instead of self-deadlocking")
        .expect("running-session read returns the live body");
        assert_eq!(body.status, SessionStatus::Running);
        assert_eq!(body.session_id, session_id);
        assert_eq!(body.progress_revision, 1);
    }

    /// Current 256-bit handles keep their durable acceptance and progress
    /// documents for the resource's whole lifetime, and every terminal read
    /// cross-checks them. This round trip would have caught the reader that
    /// pinned acceptance records to schema version 1 while the wire protocol
    /// has only ever written version 2.
    #[tokio::test]
    async fn current_handle_terminal_read_accepts_persistent_v2_acceptance() {
        let tree = TestTree::new("current-terminal-read");
        let state = tree.0.join("state");
        let results = tree.0.join("results");
        std::fs::create_dir(&state).expect("create state root");
        std::fs::create_dir(&results).expect("create results root");
        let session_id =
            "s-2222222222222222222222222222222222222222222222222222222222222222";
        let result_dir = results.join(session_id);
        std::fs::create_dir(&result_dir).expect("create result dir");

        let progress = crate::progress::ProgressReporter::create(
            &result_dir.join("progress.json"),
            session_id,
            "accepted for the current-handle terminal fixture",
        )
        .expect("create durable progress fixture");
        let events = progress.events().expect("read progress fixture events");
        let latest = events
            .last()
            .expect("progress fixture has its initial event")
            .clone();

        let mut terminal = body(session_id);
        terminal.progress_events = events;
        terminal.progress_revision = latest.revision;
        terminal.progress_at_unix_ms = latest.at_unix_ms;
        terminal.progress_phase = latest.phase;
        terminal.progress_message = latest.message.clone();
        let acceptance = AcceptanceRecord {
            schema_version: 2,
            session_id: session_id.to_string(),
            accepted_at_unix: terminal.started_at_unix,
            archive_bytes: 22,
            archive_sha256: "3".repeat(64),
            prompt: terminal.prompt_preview.clone(),
            preserve_thinking: terminal.preserve_thinking,
            max_session_turns: terminal.max_session_turns,
        };
        private_write(
            &result_dir.join("accepted.json"),
            &serde_json::to_vec_pretty(&acceptance).expect("serialize acceptance fixture"),
        );
        private_write(
            &result_dir.join("finished.json"),
            &serde_json::to_vec_pretty(&terminal).expect("serialize terminal fixture"),
        );
        // The acceptance and terminal readers pin the exact service owner
        // 1000:1000, while the durable progress reader validates against the
        // effective process identity — so progress.json must keep the
        // creating euid (1000 in development, 0 in the hermetic build stage)
        // and is deliberately absent from this ownership normalization.
        for path in [
            &result_dir,
            &result_dir.join("accepted.json"),
            &result_dir.join("finished.json"),
        ] {
            make_service_owned(path);
        }

        let cfg = test_config(state, results.clone());
        let recovered = read_terminal(&cfg, session_id)
            .await
            .expect("terminal read must accept the persistent schema-2 acceptance record");
        assert_eq!(recovered.session_id, session_id);
        assert_eq!(recovered.status, SessionStatus::Completed);

        // No version-1 acceptance record can legitimately exist on disk, so
        // the strict reader must reject one instead of trusting it.
        let mut downgraded = acceptance;
        downgraded.schema_version = 1;
        std::fs::remove_file(result_dir.join("accepted.json"))
            .expect("remove schema-2 fixture");
        private_write(
            &result_dir.join("accepted.json"),
            &serde_json::to_vec_pretty(&downgraded).expect("serialize downgraded fixture"),
        );
        make_service_owned(&result_dir.join("accepted.json"));
        let error = read_terminal(&cfg, session_id)
            .await
            .expect_err("schema-version-1 acceptance record must be rejected");
        assert!(
            error.to_string().contains("identity/schema drift"),
            "unexpected rejection: {error}"
        );
    }

    #[tokio::test]
    async fn transport_future_abort_does_not_cancel_server_owned_task() {
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();

        let transport = tokio::spawn(async move {
            await_connection_independent(
                async move {
                    let _ = started_tx.send(());
                    release_rx.await.map_err(|error| {
                        ServiceError::Internal(format!(
                            "test release channel closed unexpectedly: {error}"
                        ))
                    })?;
                    let _ = finished_tx.send(());
                    Ok(())
                },
                "complete detached acceptance fixture".to_string(),
            )
            .await
        });

        started_rx
            .await
            .expect("server-owned task reaches its independent boundary");
        transport.abort();
        assert!(
            transport
                .await
                .expect_err("aborted transport future must not complete")
                .is_cancelled()
        );
        release_tx
            .send(())
            .expect("detached server-owned task still receives release");
        tokio::time::timeout(std::time::Duration::from_secs(1), finished_rx)
            .await
            .expect("detached task completes without the transport future")
            .expect("detached task publishes completion proof");
    }

    #[tokio::test]
    async fn shutdown_tracker_closes_admission_and_drains_detached_mutations() {
        let tracker = Arc::new(LifecycleTracker::new());
        let guard = tracker
            .start()
            .expect("register server-owned mutation before shutdown");
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();

        let transport = tokio::spawn(async move {
            await_connection_independent(
                async move {
                    let _guard = guard;
                    let _ = started_tx.send(());
                    release_rx.await.map_err(|error| {
                        ServiceError::Internal(format!(
                            "test lifecycle release channel closed unexpectedly: {error}"
                        ))
                    })?;
                    Ok(())
                },
                "complete shutdown-tracked detached mutation".to_string(),
            )
            .await
        });
        started_rx
            .await
            .expect("tracked mutation reaches detached execution");
        transport.abort();
        assert!(
            transport
                .await
                .expect_err("transport waiter is independently abortable")
                .is_cancelled()
        );

        tracker.close().expect("close lifecycle admission");
        let rejected = match tracker.start() {
            Err(error) => error,
            Ok(_) => panic!("closed tracker accepted a new mutation"),
        };
        assert!(matches!(rejected, ServiceError::ServiceShuttingDown));
        let tracker_for_wait = Arc::clone(&tracker);
        let drained = tokio::spawn(async move { tracker_for_wait.wait_idle().await });
        tokio::task::yield_now().await;
        assert!(
            !drained.is_finished(),
            "shutdown drain returned while detached mutation still held its guard"
        );

        release_tx
            .send(())
            .expect("release detached mutation after shutdown starts");
        tokio::time::timeout(std::time::Duration::from_secs(1), drained)
            .await
            .expect("shutdown drain completes after the detached mutation")
            .expect("shutdown drain task does not panic")
            .expect("shutdown drain reports success");
    }

    #[tokio::test]
    async fn terminal_prepare_rewrite_commit_is_no_clobber_and_exact() {
        let tree = TestTree::new("terminal-transaction");
        let results = tree.0.join("results");
        std::fs::create_dir(&results).expect("create results root");
        let session_id = "s-99999999999999999999999999999999";
        let mut terminal = body(session_id);

        prepare_terminal(&results, &terminal)
            .await
            .expect("durable prepare");
        let result_dir = results.join(session_id);
        let temporary = result_dir.join("finished.json.tmp");
        let final_path = result_dir.join("finished.json");
        let metadata = std::fs::symlink_metadata(&temporary).expect("stat prepared terminal");
        assert!(metadata.is_file());
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        assert!(!final_path.exists());

        terminal.is_process_error = true;
        terminal
            .teardown_diagnostics
            .push("exact cleanup failure fixture".to_string());
        let (uid, gid) = owner(&temporary);
        rewrite_prepared_terminal(&results, &terminal, uid, gid)
            .await
            .expect("rewrite unpublished terminal");
        commit_prepared_terminal(&results, session_id)
            .await
            .expect("commit prepared terminal");
        assert!(!temporary.exists());
        let observed: SessionBody =
            serde_json::from_slice(&std::fs::read(&final_path).expect("read committed terminal"))
                .expect("parse committed terminal");
        assert!(observed.is_process_error);
        assert_eq!(
            observed.teardown_diagnostics,
            vec!["exact cleanup failure fixture"]
        );

        let error = prepare_terminal(&results, &terminal)
            .await
            .expect_err("committed terminal must never be overwritten");
        assert!(error.to_string().contains("refusing to overwrite"));
    }

    #[tokio::test]
    async fn retained_terminal_without_authority_marker_stays_unpublished() {
        let tree = TestTree::new("retained-terminal-marker");
        let state = tree.0.join("state");
        let results = tree.0.join("results");
        let session_id = "s-89898989898989898989898989898989";
        let state_root = state.join("sessions").join(session_id);
        let control = state_root.join("control");
        std::fs::create_dir_all(&control).expect("create retained state control tree");
        std::fs::create_dir(&results).expect("create results root");
        std::fs::set_permissions(&state_root, std::fs::Permissions::from_mode(0o755))
            .expect("chmod retained state root");
        std::fs::set_permissions(&control, std::fs::Permissions::from_mode(0o755))
            .expect("chmod retained state control");
        make_service_owned(&state_root);
        make_service_owned(&control);

        let cfg = test_config(state, results.clone());
        let mut terminal = body(session_id);
        terminal.raw_session_tree_retained = true;
        let error = persist_terminal_transaction(&cfg, &mut terminal)
            .await
            .expect_err("a retained-state claim without its exact marker must not publish");
        assert!(error.to_string().contains("marker"));
        let result_dir = results.join(session_id);
        assert!(result_dir.join("finished.json.tmp").is_file());
        assert!(!result_dir.join("finished.json").exists());
        assert!(state_root.is_dir());
    }

    #[tokio::test]
    async fn bundleless_terminal_cannot_erase_unretained_raw_state() {
        let tree = TestTree::new("bundleless-terminal-raw-state");
        let state = tree.0.join("state");
        let results = tree.0.join("results");
        let session_id = "s-78787878787878787878787878787878";
        let state_root = state.join("sessions").join(session_id);
        std::fs::create_dir_all(&state_root).expect("create contradictory raw state");
        std::fs::create_dir(&results).expect("create results root");
        std::fs::set_permissions(&state_root, std::fs::Permissions::from_mode(0o755))
            .expect("chmod contradictory raw state");
        make_service_owned(&state_root);

        let cfg = test_config(state, results.clone());
        let mut terminal = body(session_id);
        let error = persist_terminal_transaction(&cfg, &mut terminal)
            .await
            .expect_err("bundleless metadata is not authority to erase raw evidence");
        assert!(error.to_string().contains("no accepted bundle"));
        assert!(state_root.is_dir());
        assert!(results.join(session_id).join("finished.json.tmp").is_file());
        assert!(!results.join(session_id).join("finished.json").exists());
    }

    #[tokio::test]
    async fn restart_resumes_prepared_terminal_cleanup_before_publication() {
        let tree = TestTree::new("resume-prepared-terminal");
        let state = tree.0.join("state");
        let sessions = state.join("sessions");
        let results = tree.0.join("results");
        std::fs::create_dir_all(&sessions).expect("create sessions parent");
        std::fs::create_dir(&results).expect("create results root");
        let session_id = "s-67676767676767676767676767676767";
        let paths = SessionPaths::new(&state, session_id);
        let acceptance = AcceptanceRecord {
            schema_version: 2,
            session_id: session_id.to_string(),
            accepted_at_unix: 1,
            archive_bytes: 24,
            archive_sha256: "2".repeat(64),
            prompt: "restart terminal fixture".to_string(),
            preserve_thinking: false,
            max_session_turns: crate::config::DEFAULT_MAX_SESSION_TURNS,
        };
        let spool_dir = tree.0.join("spool-fixture");
        std::fs::create_dir(&spool_dir).expect("create spool fixture dir");
        let spool_file = spool_dir.join("archive.zip");
        std::fs::write(&spool_file, b"fixture-archive-payload!").expect("write spool fixture");
        let preparation =
            prepare_durable_acceptance(&results, &paths, &acceptance, &spool_file)
                .expect("prepare durable accepted fixture");
        let terminal_event = preparation
            .progress
            .publish(
                ProgressPhase::Terminal,
                "terminal fixture is prepared for ordered restart publication",
                Default::default(),
            )
            .expect("publish terminal progress fixture");
        let mut terminal = body(session_id);
        terminal.prompt_preview = super::preview(&acceptance.prompt);
        apply_progress(&mut terminal, &terminal_event);
        terminal.progress_events = preparation
            .progress
            .events()
            .expect("read exact progress fixture");
        let archive = results.join(session_id).join("bundle.tar.zst");
        private_write(&archive, b"accepted restart bundle");
        terminal.bundle_sha256 =
            crate::bundle::hash_file_sha256(&archive).expect("hash restart bundle fixture");
        terminal.bundle_compressed_bytes = b"accepted restart bundle".len() as u64;
        terminal.bundle_uncompressed_bytes = 100;
        terminal.bundle_file_count = 2;
        prepare_terminal(&results, &terminal)
            .await
            .expect("prepare private terminal fixture");

        for path in [
            &paths.root,
            &paths.staged,
            &paths.artifacts,
            &paths.control,
            &paths.streams,
            &paths.output,
            &paths.control.join("prompt.txt"),
            &paths.control.join("history-policy.json"),
            &paths.control.join("turn-budget.json"),
            &results.join(session_id),
            &results.join(session_id).join("accepted.json"),
            &results.join(session_id).join("progress.json"),
            &archive,
            &results.join(session_id).join("finished.json.tmp"),
        ] {
            make_service_owned(path);
        }

        let cfg = test_config(state, results.clone());
        resume_prepared_terminal_transaction(&cfg, session_id)
            .await
            .expect("resume cleanup before publishing the terminal");
        assert!(!paths.root.exists());
        assert!(results.join(session_id).join("finished.json").is_file());
        assert!(!results.join(session_id).join("finished.json.tmp").exists());
        let recovered = read_terminal(&cfg, session_id)
            .await
            .expect("read exact recovered terminal");
        assert_eq!(recovered.progress_events, terminal.progress_events);
    }

    #[test]
    fn session_handle_shapes_keep_current_writes_and_historical_reads_distinct() {
        let historical = "s-0123456789abcdef0123456789abcdef";
        let current =
            "s-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert!(is_safe_session_id(historical));
        assert!(!is_current_session_id(historical));
        assert!(is_safe_session_id(current));
        assert!(is_current_session_id(current));
        for unsafe_id in [
            "s-../../etc/passwd",
            "s-0123456789ABCDEF0123456789ABCDEF",
            "s-0123456789abcdef0123456789abcdeg",
            "s-0123456789abcdef0123456789abcdef0",
            "x-0123456789abcdef0123456789abcdef",
        ] {
            assert!(!is_safe_session_id(unsafe_id), "accepted {unsafe_id:?}");
            assert!(!is_current_session_id(unsafe_id), "accepted {unsafe_id:?}");
        }
    }

    #[test]
    fn cancellation_intent_publication_is_atomic_and_idempotent() {
        let tree = TestTree::new("cancel-publication");
        let results = tree.0.join("results");
        let session_id =
            "s-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let result_dir = results.join(session_id);
        std::fs::create_dir_all(&result_dir).expect("create cancellation result directory");

        let first = persist_cancel_intent(&results, session_id)
            .expect("publish exact cancellation intent");
        assert!(first.response_error.is_none());
        let final_path = cancel_intent_path(&results, session_id);
        let next_path = cancel_intent_next_path(&results, session_id);
        assert!(final_path.is_file());
        assert!(!next_path.exists());
        assert_eq!(
            std::fs::metadata(&final_path)
                .expect("stat committed cancellation")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        // The Rust builder runs tests as root while production runs as
        // 1000:1000. Construct the exact production owner before exercising
        // the read/retry path.
        make_service_owned(&final_path);
        let first_record = read_cancel_intent(&results, session_id)
            .expect("read committed cancellation")
            .expect("committed cancellation exists");
        let second = persist_cancel_intent(&results, session_id)
            .expect("same-handle cancellation retry is idempotent");
        assert!(second.response_error.is_none());
        assert_eq!(
            read_cancel_intent(&results, session_id)
                .expect("reread cancellation")
                .expect("cancellation still exists")
                .requested_at_unix_ms,
            first_record.requested_at_unix_ms
        );
        assert!(!next_path.exists());
    }

    #[test]
    fn deletion_authority_survives_partial_cleanup_and_resumes_exactly() {
        let tree = TestTree::new("delete-transaction");
        let state = tree.0.join("state");
        let sessions = state.join("sessions");
        let results = tree.0.join("results");
        std::fs::create_dir_all(&sessions).expect("create deletion state parent");
        std::fs::create_dir(&results).expect("create deletion results root");
        let session_id =
            "s-abababababababababababababababababababababababababababababababab";
        let state_root = sessions.join(session_id);
        let result_dir = results.join(session_id);
        std::fs::create_dir(&state_root).expect("create deletion state target");
        std::fs::create_dir(&result_dir).expect("create deletion result target");
        std::fs::set_permissions(&state_root, std::fs::Permissions::from_mode(0o755))
            .expect("chmod deletion state target");
        std::fs::set_permissions(&result_dir, std::fs::Permissions::from_mode(0o755))
            .expect("chmod deletion result target");
        make_service_owned(&state_root);
        make_service_owned(&result_dir);
        std::fs::write(result_dir.join("evidence"), b"terminal evidence")
            .expect("write deletion result evidence");

        persist_delete_intent(&results, session_id).expect("publish deletion authority");
        let authority = delete_intent_path(&results, session_id);
        make_service_owned(&authority);
        assert_eq!(
            read_delete_intent(&results, session_id)
                .expect("read deletion authority")
                .expect("deletion authority exists")
                .session_id,
            session_id
        );

        // Model a reset after raw-state cleanup but before result cleanup.
        std::fs::remove_dir_all(&state_root).expect("remove raw state in partial deletion");
        finish_delete_intent(&state, &results, session_id)
            .expect("resume deletion from independent authority");
        assert!(!result_dir.exists());
        assert!(!authority.exists());

        // A crash-left unpublished candidate is rollback evidence only.
        let next = delete_intent_next_path(&results, session_id);
        private_write(&next, b"{torn");
        make_service_owned(&next);
        reconcile_unpublished_delete_intent(&results, session_id)
            .expect("discard unpublished deletion candidate");
        assert!(!next.exists());
        assert!(
            read_delete_intent(&results, session_id)
                .expect("read absent committed deletion")
                .is_none(),
            "an unpublished candidate must never invent deletion authority"
        );
    }

    #[test]
    fn restart_reconciliation_never_promotes_unpublished_cancellation() {
        let tree = TestTree::new("cancel-reconcile");
        let results = tree.0.join("results");
        let session_id =
            "s-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let result_dir = results.join(session_id);
        std::fs::create_dir_all(&result_dir).expect("create cancellation result directory");
        let final_path = cancel_intent_path(&results, session_id);
        let next_path = cancel_intent_next_path(&results, session_id);

        private_write(&next_path, &encoded_cancel_intent(session_id, 7));
        make_service_owned(&next_path);
        reconcile_unpublished_cancel_intent(&results, session_id)
            .expect("discard fully written but uncommitted candidate");
        assert!(!next_path.exists());
        assert!(
            read_cancel_intent(&results, session_id)
                .expect("read absent committed intent")
                .is_none(),
            "an unpublished candidate must never become cancellation authority"
        );

        private_write(&final_path, &encoded_cancel_intent(session_id, 11));
        private_write(&next_path, b"{torn");
        make_service_owned(&final_path);
        make_service_owned(&next_path);
        reconcile_unpublished_cancel_intent(&results, session_id)
            .expect("committed marker remains authoritative beside torn candidate");
        assert!(!next_path.exists());
        assert_eq!(
            read_cancel_intent(&results, session_id)
                .expect("read authoritative committed intent")
                .expect("committed intent exists")
                .requested_at_unix_ms,
            11
        );
    }

    #[test]
    fn cancellation_reconciliation_refuses_symlink_candidates_without_following() {
        let tree = TestTree::new("cancel-reconcile-symlink");
        let results = tree.0.join("results");
        let session_id =
            "s-cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
        let result_dir = results.join(session_id);
        std::fs::create_dir_all(&result_dir).expect("create cancellation result directory");
        let outside = tree.0.join("outside-sentinel");
        private_write(&outside, b"preserve");
        let next_path = cancel_intent_next_path(&results, session_id);
        symlink(&outside, &next_path).expect("create hostile cancellation candidate symlink");

        let error = reconcile_unpublished_cancel_intent(&results, session_id)
            .expect_err("symlink candidate must fail closed");
        assert!(error.to_string().contains("unsafe type/mode/owner/size"));
        assert_eq!(
            std::fs::read(&outside).expect("read outside sentinel"),
            b"preserve"
        );
        assert!(std::fs::symlink_metadata(&next_path)
            .expect("hostile symlink remains as evidence")
            .file_type()
            .is_symlink());
    }

    #[test]
    fn running_progress_is_descriptor_anchored_and_ignores_only_partial_tail() {
        let tree = TestTree::new("running-progress");
        let events = tree.0.join("events.jsonl");
        let bytes = concat!(
            "{\"type\":\"system\",\"session_id\":\"fixture\"}\n",
            "{\"type\":\"assistant\",\"parent_tool_use_id\":null,\"message\":{\"usage\":{\"input_tokens\":7}}}\n",
            "{\"type\":\"assistant\",\"parent_tool_use_id\":null,\"message\":{\"usage\":{\"input_tokens\":9}}}"
        );
        private_write(&events, bytes.as_bytes());
        make_service_owned(&events);

        let observed = read_running_progress(&events).expect("read exact event snapshot");
        assert_eq!(observed.num_turns, 1);
        assert_eq!(observed.output_event_bytes, bytes.len() as u64);
        assert!(observed.last_event_at_unix > 0);

        let outside = tree.0.join("outside-events");
        private_write(&outside, bytes.as_bytes());
        std::fs::remove_file(&events).expect("remove original event file");
        symlink(&outside, &events).expect("replace event path with hostile symlink");
        let error = read_running_progress(&events)
            .expect_err("descriptor open must reject a symlink rather than follow it");
        assert!(error.to_string().contains("read_running_progress: stat"));
        assert_eq!(
            std::fs::read(&outside).expect("read untouched outside events"),
            bytes.as_bytes()
        );
    }

    #[test]
    fn running_progress_rejects_malformed_completed_records() {
        let tree = TestTree::new("running-progress-malformed");
        let events = tree.0.join("events.jsonl");
        private_write(&events, b"{not-json}\n");
        make_service_owned(&events);
        let error = read_running_progress(&events)
            .expect_err("newline-terminated malformed JSON is durable bad evidence");
        assert!(error
            .to_string()
            .contains("completed JSONL record 1 is malformed"));
    }

    #[test]
    fn state_cleanup_removes_only_exact_owned_directory_and_never_follows_symlink() {
        let tree = TestTree::new("state-cleanup");
        let state = tree.0.join("state");
        let sessions = state.join("sessions");
        std::fs::create_dir_all(&sessions).expect("create sessions root");
        let session_id = "s-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let session = sessions.join(session_id);
        std::fs::create_dir(&session).expect("create owned session tree");
        std::fs::set_permissions(&session, std::fs::Permissions::from_mode(0o755))
            .expect("chmod owned session tree");
        let (uid, gid) = owner(&session);
        assert!(remove_terminalized_state(&state, session_id, uid, gid).is_empty());
        assert!(!session.exists());

        let outside = tree.0.join("outside-evidence");
        std::fs::create_dir(&outside).expect("create outside evidence");
        std::fs::write(outside.join("sentinel"), b"preserve").expect("write sentinel");
        symlink(&outside, &session).expect("create hostile state symlink");
        let diagnostics = remove_terminalized_state(&state, session_id, uid, gid);
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].contains("refusing recursive removal"));
        assert_eq!(
            std::fs::read(outside.join("sentinel")).expect("read preserved sentinel"),
            b"preserve"
        );
    }
}
