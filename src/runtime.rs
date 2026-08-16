//! Session runtime: ownership, cancellation, persistence, shutdown.
//!
//! Architecture:
//!
//! - **In-memory map** (`Inner.running`) holds **only running** sessions.
//!   Terminal sessions live exclusively on disk under
//!   `<results_dir>/<id>/finished.json`. Memory growth is bounded by the
//!   strict singleton (≤1 entry); disk growth is bounded by the user (every
//!   session lives until DELETE, never auto-evicted by time or count).
//!
//! - **Singleton** is enforced by an `Arc<Semaphore>` with one permit. The
//!   permit is moved into the spawned supervisor and held through session
//!   execution, panic cleanup, terminal persistence, map eviction, and
//!   notification; submit `try_acquire`s and translates failure to `Busy`.
//!   `shutdown` waits for the permit to be released, which is the strongest
//!   "no in-flight work remains" signal we have.
//!
//! - **Cancellation** uses a parent `CancellationToken` (`shutdown_token`)
//!   on the manager. Each session gets a `child_token()` of that parent.
//!   `cancel(id)` cancels only the child. `shutdown` cancels the parent,
//!   which cascades to every child. The run task observes its child token
//!   from inside `session::run_one` and tears down cleanly.
//!
//! - **Reads are pure**: `get`, `list`. They never mutate state. Multiple
//!   concurrent reads with retries are safe and yield identical results.
//!
//! - **Lifecycle writes are unambiguous**: `cancel` on a terminal session
//!   returns the current body; `delete` on a missing session returns
//!   `NotFound` (a definite "gone" rather than a silent success).
//!
//! - **Lifecycle is explicit**: `running` → terminal (`completed` |
//!   `cancelled`) → DELETE'd. There is no implicit transition; in particular,
//!   reads do not consume, and there is no time-based eviction anywhere.
//!
//! - **Crash recovery**: a server restart drops in-flight running sessions
//!   (the user accepted this — "session not lost as long as server is
//!   running"). On startup, the orphan sweep cleans up any leftover docker
//!   containers / networks / staged dirs from such crashes; on-disk
//!   terminal records are unaffected and immediately visible via `list`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::{oneshot, Mutex, Notify, Semaphore};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::config::Config;
use crate::error::{io_msg, ServiceError, ServiceResult};
use crate::session;
use crate::validation::ValidatedRequest;

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
    pub prompt_preview: String,

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
    pub bundle_archive_path: String,
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

/// The exact model/tokenizer-preflight snapshot the run task hands back to `submit`
/// over the readiness oneshot. `submit` then uses it to fill in the
/// running-state fields of the public `SessionBody` it returns to the
/// HTTP caller, and to update the in-memory entry that `cancel` / `list` /
/// `get` see.
#[derive(Clone, Debug)]
pub struct RunningSnapshot {
    pub session_id: String,
    pub started_at_unix: u64,
    pub prompt_preview: String,
    pub model: String,
    pub context_window: u64,
}

/// In-memory entry for a running session. Removed from the map by the run
/// task on transition-to-terminal (after `finished.json` has been
/// successfully persisted). After removal, every observer falls back to
/// the on-disk record.
struct RunningEntry {
    /// Mutable across the entry's lifetime: starts as a placeholder with
    /// `started_at_unix=0`, replaced once by `submit`
    /// after the readiness oneshot fires. Held very briefly.
    snapshot: Mutex<RunningSnapshot>,
    /// Child of `Manager.shutdown_token`. `cancel(id)` cancels this child;
    /// `shutdown` cancels the parent, which cascades to every child.
    cancel: CancellationToken,
    /// Notified by the run task immediately before it returns. Lets
    /// `cancel` and `shutdown` await terminal state without polling.
    finished: Arc<Notify>,
}

pub struct Manager {
    cfg: Arc<Config>,
    inner: Arc<Mutex<Inner>>,
    /// One permit. Held by the spawned supervisor through execution, cleanup,
    /// persistence, map eviction, and notification. `submit()` `try_acquire`s;
    /// `shutdown()` blocks on `acquire()` so a successful return guarantees no
    /// in-flight session or supervisor-owned cleanup remains.
    singleton: Arc<Semaphore>,
    /// Cancelled at the top of `shutdown`. `submit` checks this before
    /// taking the permit, so post-shutdown submits fail fast. Each session's
    /// per-session cancel token is a child of this one.
    shutdown_token: CancellationToken,
}

struct Inner {
    running: HashMap<String, Arc<RunningEntry>>,
    unpersisted_terminal: HashMap<String, SessionBody>,
}

impl Manager {
    pub fn new(cfg: Arc<Config>) -> Self {
        Self {
            cfg,
            inner: Arc::new(Mutex::new(Inner {
                running: HashMap::new(),
                unpersisted_terminal: HashMap::new(),
            })),
            singleton: Arc::new(Semaphore::new(1)),
            shutdown_token: CancellationToken::new(),
        }
    }

    /// Submit a new session.
    ///
    /// Blocks until the isolated agent has verified the exact backend model
    /// and exercised its real tokenizer endpoint (typically a few seconds;
    /// bounded internally by `session::run_one`'s setup timeouts). Qwen Code
    /// then tokenizes every fully rendered request before inference. Returns
    /// the `running` view once readiness is observable via
    /// `get` / `list` / `cancel`.
    ///
    /// Errors:
    /// - `Internal("server is shutting down …")` if shutdown has begun.
    /// - `Busy{running_session_id}` if the singleton is already held.
    /// - Any setup error from `session::run_one` before model/tokenizer
    ///   readiness (Docker run failure, network setup failure, …).
    pub async fn submit(&self, req: ValidatedRequest) -> ServiceResult<SessionBody> {
        if self.shutdown_token.is_cancelled() {
            return Err(ServiceError::Internal(
                "server is shutting down — refusing to accept new sessions; \
                 wait for the current shutdown to complete and try again on the next process \
                 (in-flight sessions, if any, are being cancelled and torn down before exit)"
                    .into(),
            ));
        }

        let permit = match Arc::clone(&self.singleton).try_acquire_owned() {
            Ok(p) => p,
            Err(_) => {
                // Look up which session is holding the permit. There is at
                // most one; if the map is somehow empty the in-flight
                // submit() is between "permit acquired" and "map insert"
                // (microsecond window).
                let inner = self.inner.lock().await;
                let running_id = inner.running.keys().next().cloned().unwrap_or_else(|| {
                    "<unknown — singleton held but map empty (in-flight setup)>".into()
                });
                return Err(ServiceError::Busy {
                    running_session_id: running_id,
                });
            }
        };

        let session_id = format!("s-{}", Uuid::new_v4().simple());
        let session_cancel = self.shutdown_token.child_token();
        let finished = Arc::new(Notify::new());
        let prompt_preview = preview(&req.prompt);

        // Insert the entry FIRST, with a placeholder snapshot. This
        // guarantees that any concurrent `cancel`/`shutdown` between now
        // and "agent-ready" can find this session and signal its cancel token.
        let placeholder = RunningSnapshot {
            session_id: session_id.clone(),
            started_at_unix: 0,
            prompt_preview: prompt_preview.clone(),
            model: self.cfg.vllm_model_name.clone(),
            context_window: self.cfg.lock.backend.max_model_len,
        };
        let entry = Arc::new(RunningEntry {
            snapshot: Mutex::new(placeholder),
            cancel: session_cancel.clone(),
            finished: Arc::clone(&finished),
        });
        {
            let mut inner = self.inner.lock().await;
            // The session_id is fresh from a v4 UUID; collision is not a
            // concern in any realistic universe. But check anyway: a
            // duplicate would mean a programmer error, and we'd rather
            // fail loudly than silently overwrite.
            if inner.running.contains_key(&session_id) {
                return Err(ServiceError::Internal(format!(
                    "submit({session_id}): map already contains an entry — UUID v4 collision \
                     or programmer error inserting the same id twice"
                )));
            }
            inner.running.insert(session_id.clone(), Arc::clone(&entry));
        }

        let (ready_tx, ready_rx) = oneshot::channel::<ServiceResult<RunningSnapshot>>();

        // Spawn a supervisor that owns the singleton permit through session
        // execution, panic cleanup, persistence, map eviction, and the final
        // notification. `run_one` executes in an inner task so a panic becomes
        // an explicit JoinError instead of abandoning the map entry and every
        // waiter. The supervisor itself contains no unchecked panic sites.
        let cfg = Arc::clone(&self.cfg);
        let inner_for_task = Arc::clone(&self.inner);
        let session_id_for_task = session_id.clone();
        let cancel_for_task = session_cancel.clone();
        let cancel_for_supervisor = session_cancel.clone();
        let finished_for_task = Arc::clone(&finished);
        let prompt_preview_for_task = prompt_preview.clone();
        let supervisor_prompt = req.prompt.clone();
        let supervisor_started_at_unix = unix_now();
        let supervisor_wall_start = std::time::Instant::now();
        tokio::spawn(async move {
            // This outer task is intentionally small and owns the permit. A
            // panic in the inner task still reaches the cleanup below.
            let _permit = permit;
            let run_cfg = Arc::clone(&cfg);
            let run_session_id = session_id_for_task.clone();
            let run_prompt_preview = prompt_preview_for_task.clone();
            let run_handle = tokio::spawn(async move {
                session::run_one(
                    &run_cfg,
                    &run_session_id,
                    req,
                    cancel_for_task,
                    ready_tx,
                    run_prompt_preview,
                )
                .await
            });
            let mut body = match run_handle.await {
                Ok(body) => body,
                Err(join_error) => {
                    session::recover_after_execution_panic(
                        &cfg,
                        &session_id_for_task,
                        &supervisor_prompt,
                        &prompt_preview_for_task,
                        supervisor_started_at_unix,
                        supervisor_wall_start,
                        cancel_for_supervisor.is_cancelled(),
                        join_error.to_string(),
                    )
                    .await
                }
            };

            // Terminalization is a durable prepare/cleanup/commit protocol:
            // first write and fsync the complete private terminal draft, then
            // remove raw state only when a required bundle exists, then
            // no-clobber publish the terminal. A process crash at any point
            // leaves either recoverable draft metadata, raw evidence, or both.
            let persist_error = match prepare_terminal(&cfg.results_dir, &body).await {
                Ok(()) => {
                    if !body.raw_session_tree_retained {
                        let cleanup_diagnostics = remove_terminalized_state(
                            &cfg.state_dir,
                            &session_id_for_task,
                            1000,
                            1000,
                        );
                        if !cleanup_diagnostics.is_empty() {
                            body.is_process_error = true;
                            body.teardown_diagnostics.extend(cleanup_diagnostics);
                            if let Err(error) =
                                rewrite_prepared_terminal(&cfg.results_dir, &body, 1000, 1000).await
                            {
                                Some(ServiceError::Internal(format!(
                                    "terminal state cleanup failed and the durable terminal draft could not be updated: {error}"
                                )))
                            } else {
                                commit_prepared_terminal(&cfg.results_dir, &body.session_id)
                                    .await
                                    .err()
                            }
                        } else {
                            commit_prepared_terminal(&cfg.results_dir, &body.session_id)
                                .await
                                .err()
                        }
                    } else {
                        commit_prepared_terminal(&cfg.results_dir, &body.session_id)
                            .await
                            .err()
                    }
                }
                Err(error) => Some(error),
            };

            // Remove from the map only after the durable transaction above.
            // A concurrent `get` therefore sees either the running entry, a
            // committed on-disk terminal, or the explicit in-memory failure.
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

            // Wake every observer (cancel waiter, shutdown waiter).
            finished_for_task.notify_waiters();
        });

        // Wait for either exact agent preflight (success) or early error (the run task is
        // already running its own teardown).
        match ready_rx.await {
            Ok(Ok(snapshot)) => {
                // Update the entry with the real values now that we have
                // them. Held briefly.
                *entry.snapshot.lock().await = snapshot.clone();
                if !self.inner.lock().await.running.contains_key(&session_id) {
                    return self.get(&session_id).await;
                }
                let progress =
                    read_running_progress(&events_jsonl_path(&self.cfg, &snapshot.session_id))?;
                // A very short agent may transition after readiness and remove
                // its live files while this response is being assembled. If
                // that happened, return the already-persisted terminal body
                // instead of an impossible stale running state.
                if !self.inner.lock().await.running.contains_key(&session_id) {
                    return self.get(&session_id).await;
                }
                Ok(running_body(&snapshot, progress))
            }
            Ok(Err(e)) => {
                // The run task is doing teardown right now. Wait for it to
                // finish so the singleton is visibly free before we return
                // the error to the client (clients can otherwise observe a
                // surprising "Busy" on an immediate retry).
                self.wait_for_eviction(&session_id, &finished).await;
                Err(e)
            }
            Err(_oneshot_dropped) => {
                // The task dropped the sender without sending — by
                // construction, `session::run_one` always sends, so this
                // path indicates a panic or unwind.
                self.wait_for_eviction(&session_id, &finished).await;
                Err(ServiceError::Internal(format!(
                    "submit({session_id}): readiness channel was dropped without a value — \
                     the run task most likely panicked. Check the tracing log for the panic site \
                     and the session's container logs (if any) via `docker logs agent-{session_id}`"
                )))
            }
        }
    }

    /// Wait until the exact in-memory running entry is gone without a
    /// subscribe-after-notify race. `notify_waiters` does not retain a permit,
    /// so the notification future must be enabled before the map recheck.
    async fn wait_for_eviction(&self, session_id: &str, finished: &Notify) {
        loop {
            let notified = finished.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !self.inner.lock().await.running.contains_key(session_id) {
                return;
            }
            notified.as_mut().await;
        }
    }

    /// Pure read of a session by id. Looks in memory first (running
    /// sessions), falls back to disk (terminal sessions).
    pub async fn get(&self, session_id: &str) -> ServiceResult<SessionBody> {
        let inner = self.inner.lock().await;
        if let Some(entry) = inner.running.get(session_id).cloned() {
            drop(inner);
            let snap = entry.snapshot.lock().await.clone();
            let progress = read_running_progress(&events_jsonl_path(&self.cfg, session_id))?;
            return Ok(running_body(&snap, progress));
        }
        if let Some(body) = inner.unpersisted_terminal.get(session_id).cloned() {
            return Ok(body);
        }
        drop(inner);
        read_terminal(&self.cfg, session_id).await
    }

    /// Pure read of every visible session. Combines in-memory running
    /// entries with on-disk terminal entries (the on-disk records survive
    /// across server restart).
    pub async fn list(&self) -> ServiceResult<Vec<SessionBody>> {
        let mut bodies: Vec<SessionBody> = {
            let inner = self.inner.lock().await;
            let mut v = Vec::with_capacity(inner.running.len() + inner.unpersisted_terminal.len());
            for entry in inner.running.values() {
                let snap = entry.snapshot.lock().await.clone();
                let progress =
                    read_running_progress(&events_jsonl_path(&self.cfg, &snap.session_id))?;
                v.push(running_body(&snap, progress));
            }
            v.extend(inner.unpersisted_terminal.values().cloned());
            v
        };

        let running_ids: std::collections::HashSet<String> =
            bodies.iter().map(|b| b.session_id.clone()).collect();

        let dir_iter = match std::fs::read_dir(&self.cfg.results_dir) {
            Ok(it) => it,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(bodies),
            Err(e) => {
                return Err(ServiceError::Internal(io_msg(
                    "list: read_dir results_dir",
                    &self.cfg.results_dir,
                    &e,
                )));
            }
        };

        for entry in dir_iter {
            let entry = entry.map_err(|e| {
                ServiceError::Internal(io_msg(
                    "list: read results_dir entry",
                    &self.cfg.results_dir,
                    &e,
                ))
            })?;
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| {
                    ServiceError::Internal(format!(
                        "list: result entry has no UTF-8 file name: {}",
                        path.display()
                    ))
                })?
                .to_string();
            if running_ids.contains(&name) {
                continue;
            }
            // Running IDs were excluded above. Every remaining service-owned
            // directory must have a valid durable terminal record; a partial
            // or malformed entry is an invariant error, not an omitted row.
            bodies.push(read_terminal(&self.cfg, &name).await?);
        }
        bodies.sort_by_key(|b| b.started_at_unix);
        Ok(bodies)
    }

    /// Cancel a running session. Idempotent: a cancel on a terminal
    /// session is a no-op and returns the current body. Awaits the run
    /// task's teardown so the returned body reflects the final state.
    pub async fn cancel(&self, session_id: &str) -> ServiceResult<SessionBody> {
        let entry = self.inner.lock().await.running.get(session_id).cloned();
        let entry = match entry {
            Some(e) => e,
            None => {
                // Not running — return whatever's on disk (or NotFound).
                return self.get(session_id).await;
            }
        };

        // Trigger cancellation. Cloning is cheap; multiple cancel() calls
        // are safe (idempotent at the token level).
        entry.cancel.cancel();

        // Wait for the run task's teardown to land. Defensive against a
        // notify_waiters() that fires before our notified() subscribes:
        // we recheck the map between waits, and break as soon as the entry
        // has been evicted (which the task does immediately before
        // `notify_waiters`). A cap on each wait surfaces a wedge in tracing
        // rather than hanging the request indefinitely.
        let notified = entry.finished.notified();
        tokio::pin!(notified);
        loop {
            if !self.inner.lock().await.running.contains_key(session_id) {
                break;
            }
            match tokio::time::timeout(Duration::from_secs(120), notified.as_mut()).await {
                Ok(()) => break,
                Err(_) => {
                    tracing::error!(
                        session_id = %session_id,
                        "cancel: run task has not transitioned 120s after the cancel \
                         signal. The cancel-aware wait inside session::run_one should \
                         observe the token within seconds (it is checked at every step \
                         and against `docker wait`). This indicates a wedged docker daemon \
                         or a bug. Continuing to wait."
                    );
                    notified.set(entry.finished.notified());
                }
            }
        }

        self.get(session_id).await
    }

    /// Wait without polling until a running session becomes terminal. If it
    /// is already terminal, return immediately. The notification loop closes
    /// the subscribe-after-notify race by rechecking the map after creating
    /// each future.
    pub async fn wait_terminal(&self, session_id: &str) -> ServiceResult<SessionBody> {
        loop {
            let entry = self.inner.lock().await.running.get(session_id).cloned();
            let Some(entry) = entry else {
                return self.get(session_id).await;
            };
            let notified = entry.finished.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !self.inner.lock().await.running.contains_key(session_id) {
                continue;
            }
            notified.as_mut().await;
        }
    }

    /// Remove a terminal session from disk. The lifecycle is explicit:
    /// `delete` refuses to act on a running session (`SessionRunning`/409)
    /// — the operator must `cancel` first.
    ///
    /// Returns `NotFound` for unknown ids (informative — repeat callers
    /// see "yes, it's gone" rather than a silent success).
    pub async fn delete(&self, session_id: &str) -> ServiceResult<()> {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        if self.inner.lock().await.running.contains_key(session_id) {
            return Err(ServiceError::SessionRunning {
                session_id: session_id.to_string(),
            });
        }
        if !is_safe_session_id(session_id) {
            return Err(ServiceError::InvalidRequest(format!(
                "delete: session_id {session_id:?} is not the expected `s-<32-hex>` shape — \
                 refusing to join it onto the results_dir path (defensive against path traversal \
                 even though we trust the URL router not to send arbitrary strings)"
            )));
        }
        // A guessed orphan ID is not deletion authority. Resolve the exact
        // terminal record (durable or explicitly retained in memory) before
        // touching any corresponding raw-state path.
        let terminal = self.get(session_id).await?;
        if terminal.status == SessionStatus::Running {
            return Err(ServiceError::SessionRunning {
                session_id: session_id.to_string(),
            });
        }
        let retained_state = self.cfg.state_dir.join("sessions").join(session_id);
        match tokio::fs::symlink_metadata(&retained_state).await {
            Ok(metadata)
                if metadata.is_dir()
                    && !metadata.file_type().is_symlink()
                    && metadata.permissions().mode() & 0o777 == 0o755
                    && metadata.uid() == 1000
                    && metadata.gid() == 1000 =>
            {
                validate_delete_state_marker(&retained_state, &terminal)?;
                tokio::fs::remove_dir_all(&retained_state)
                    .await
                    .map_err(|error| {
                        ServiceError::Internal(io_msg(
                            "delete: remove retained forensic session tree",
                            &retained_state,
                            &error,
                        ))
                    })?;
                sync_directory(
                    &self.cfg.state_dir.join("sessions"),
                    "delete: sync sessions directory after retained-tree removal",
                )?;
            }
            Ok(_) => {
                return Err(ServiceError::Internal(format!(
                    "delete: retained session path {} is not the exact service-owned 1000:1000 mode-0755 ordinary directory; refusing recursive removal",
                    retained_state.display()
                )));
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && !terminal.raw_session_tree_retained => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(ServiceError::Internal(format!(
                    "delete: terminal {session_id} claims retained raw evidence but its session tree is absent at {}",
                    retained_state.display()
                )));
            }
            Err(error) => {
                return Err(ServiceError::Internal(io_msg(
                    "delete: stat retained forensic session tree",
                    &retained_state,
                    &error,
                )));
            }
        }

        let dir = self.cfg.results_dir.join(session_id);
        let retained = self
            .inner
            .lock()
            .await
            .unpersisted_terminal
            .contains_key(session_id);
        match tokio::fs::remove_dir_all(&dir).await {
            Ok(()) => {
                self.inner
                    .lock()
                    .await
                    .unpersisted_terminal
                    .remove(session_id);
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && retained => {
                self.inner
                    .lock()
                    .await
                    .unpersisted_terminal
                    .remove(session_id);
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(ServiceError::NotFound {
                session_id: session_id.to_string(),
            }),
            Err(e) => Err(ServiceError::Internal(io_msg(
                "delete: remove_dir_all",
                &dir,
                &e,
            ))),
        }
    }

    /// Server shutdown. Cancels the parent token (cascades to every child),
    /// then waits for the singleton permit to be free — that is the
    /// strongest "no in-flight session remains" signal we have, since the
    /// run task holds the permit until its teardown completes.
    ///
    /// There is deliberately no shutdown deadline: shutdown first cancels
    /// the session, then waits until its fail-closed teardown is genuinely
    /// finished. Exiting early would orphan state while claiming success.
    pub async fn shutdown(&self) -> ServiceResult<()> {
        // Refuse new submissions. Already-spawned setup will observe the
        // cascade-cancellation below.
        self.shutdown_token.cancel();

        let in_flight: Vec<String> = self.inner.lock().await.running.keys().cloned().collect();
        if in_flight.is_empty() && self.singleton.available_permits() == 1 {
            tracing::info!("shutdown: no in-flight session — clean exit");
            return Ok(());
        }
        tracing::info!(
            sessions = ?in_flight,
            "shutdown: cancellation cascaded; awaiting teardown"
        );

        // Acquiring the permit guarantees the supervisor has completed
        // execution/panic cleanup, persistence, map eviction, and notification
        // (it drops the permit at the very end of its closure). Any in-flight
        // `submit` that was between
        // "permit acquired" and "map insert" will also have its task
        // observe the cascade-cancellation and terminate, releasing the
        // permit.
        match Arc::clone(&self.singleton).acquire_owned().await {
            Ok(_permit) => {
                tracing::info!("shutdown: singleton permit free — all sessions terminal");
                Ok(())
            }
            Err(closed) => {
                // Semaphore::close() was called somewhere; we do not call
                // it ourselves, so this path is unexpected.
                Err(ServiceError::Internal(format!(
                    "shutdown: semaphore closed unexpectedly: {closed}"
                )))
            }
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
fn running_body(s: &RunningSnapshot, progress: (u64, u64)) -> SessionBody {
    let (num_turns, last_event_at_unix) = progress;
    SessionBody {
        session_id: s.session_id.clone(),
        status: SessionStatus::Running,
        started_at_unix: s.started_at_unix,
        model: s.model.clone(),
        context_window: s.context_window,
        prompt_preview: s.prompt_preview.clone(),
        num_turns,
        last_event_at_unix,
        finished_at_unix: 0,
        duration_wall_ms: 0,
        container_exit_code: 0,
        agent_exit_code: 0,
        is_process_error: false,
        response: String::new(),
        agent_duration_ms: 0,
        bundle_archive_path: String::new(),
        bundle_compressed_bytes: 0,
        bundle_uncompressed_bytes: 0,
        bundle_file_count: 0,
        bundle_artifacts_file_count: 0,
        raw_session_tree_retained: false,
        teardown_diagnostics: Vec::new(),
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

/// Read the live `events.jsonl` and return `(num_turns, last_event_at_unix)`.
/// `num_turns` is the number of completed main-thread model invocations. Qwen
/// stream-JSON can emit zero-usage thinking/text fragments before the one
/// assistant record carrying final per-invocation usage, so fragments are not
/// turns. Both fields are 0 only when the file does not exist yet; malformed or
/// unreadable state is an explicit error.
///
/// Cost: a linear byte scan. The service is a singleton and reads occur only
/// on explicit API requests, so this favors exactness over a mutable cache.
pub fn read_running_progress(events_path: &std::path::Path) -> ServiceResult<(u64, u64)> {
    let meta = match std::fs::metadata(events_path) {
        Ok(m) => m,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((0, 0)),
        Err(error) => {
            return Err(ServiceError::Internal(io_msg(
                "read_running_progress: stat",
                events_path,
                &error,
            )))
        }
    };
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
    let text = std::fs::read_to_string(events_path).map_err(|error| {
        ServiceError::Internal(io_msg("read_running_progress: read", events_path, &error))
    })?;
    let mut num_turns = 0u64;
    for (index, chunk) in text.split_inclusive('\n').enumerate() {
        // `tee` can be observed after writing part of the next JSON object.
        // That is an explicit in-progress state, not malformed completed
        // data, so only newline-terminated records participate in progress.
        if !chunk.ends_with('\n') {
            continue;
        }
        let line = chunk.trim();
        if line.is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(line).map_err(|error| {
            ServiceError::Internal(format!(
                "read_running_progress: completed JSONL record {} is malformed: {error}",
                index + 1
            ))
        })?;
        let object = value.as_object().ok_or_else(|| {
            ServiceError::Internal(format!(
                "read_running_progress: completed JSONL record {} is not an object",
                index + 1
            ))
        })?;
        object
            .get("type")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                ServiceError::Internal(format!(
                    "read_running_progress: completed JSONL record {} lacks string type",
                    index + 1
                ))
            })?;
        if crate::result_parse::is_completed_main_turn(object) {
            num_turns = num_turns.saturating_add(1);
        }
    }
    Ok((num_turns, last_event_at_unix))
}

pub fn preview(s: &str) -> String {
    let truncated: String = s.chars().take(140).collect();
    if truncated.chars().count() < s.chars().count() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

fn finished_json_path(cfg: &Config, session_id: &str) -> PathBuf {
    cfg.results_dir.join(session_id).join("finished.json")
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
        .open(&tmp_path)
        .map_err(|error| {
            ServiceError::Internal(io_msg(
                "rewrite_prepared_terminal: open tmp",
                &tmp_path,
                &error,
            ))
        })?;
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
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let marker = state_root.join("control/raw-evidence-retained.txt");
    let metadata = match std::fs::symlink_metadata(&marker) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(ServiceError::Internal(io_msg(
                "delete: stat raw-evidence marker",
                &marker,
                &error,
            )));
        }
    };
    match (terminal.raw_session_tree_retained, metadata) {
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
        (true, Some(metadata)) => {
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || metadata.permissions().mode() & 0o777 != 0o600
                || metadata.uid() != 1000
                || metadata.gid() != 1000
            {
                return Err(ServiceError::Internal(format!(
                    "delete: retained-evidence marker {} has unsafe type/mode/owner",
                    marker.display()
                )));
            }
            let contents = std::fs::read_to_string(&marker).map_err(|error| {
                ServiceError::Internal(io_msg("delete: read raw-evidence marker", &marker, &error))
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
                    | "cause=container-quiescence-unproved"
                    | "cause=container-teardown-incomplete"
            );
            let cause_matches_bundle = if terminal.bundle_archive_path.is_empty() {
                matches!(
                    cause,
                    "cause=required-bundle-failure"
                        | "cause=panic-recovery-bundle-failure"
                        | "cause=setup-forensic-bundle-failure"
                        | "cause=container-quiescence-unproved"
                )
            } else {
                cause == "cause=container-teardown-incomplete"
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
            "session_id {session_id:?} is not the required s-<32-lowercase-hex> shape"
        )));
    }
    let path = finished_json_path(cfg, session_id);
    let temporary_path = path.with_file_name("finished.json.tmp");
    match tokio::fs::symlink_metadata(&temporary_path).await {
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
    match tokio::fs::symlink_metadata(&path).await {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
        Ok(_) => {
            return Err(ServiceError::Internal(format!(
                "read_terminal({session_id}): {} is not a regular non-symlink file",
                path.display()
            )));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ServiceError::NotFound {
                session_id: session_id.to_string(),
            });
        }
        Err(e) => {
            return Err(ServiceError::Internal(io_msg(
                "read_terminal: stat finished.json",
                &path,
                &e,
            )));
        }
    }
    let bytes = match tokio::fs::read(&path).await {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ServiceError::NotFound {
                session_id: session_id.to_string(),
            });
        }
        Err(e) => {
            return Err(ServiceError::Internal(io_msg(
                "read_terminal: read finished.json",
                &path,
                &e,
            )));
        }
    };
    serde_json::from_slice::<SessionBody>(&bytes).map_err(|e| {
        ServiceError::Internal(format!(
            "read_terminal({session_id}): finished.json at {} is malformed JSON or wrong shape: {e}",
            path.display()
        ))
    })
}

/// Cheap defensive check that a session_id is the shape we generate.
/// Used by `delete` before joining a path. Format: `s-` + 32 lowercase hex.
pub(crate) fn is_safe_session_id(s: &str) -> bool {
    if s.len() != 34 {
        return false;
    }
    let mut chars = s.chars();
    if chars.next() != Some('s') {
        return false;
    }
    if chars.next() != Some('-') {
        return false;
    }
    chars.all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
    use std::path::{Path, PathBuf};

    use super::{
        commit_prepared_terminal, prepare_terminal, remove_terminalized_state,
        rewrite_prepared_terminal, SessionBody, SessionStatus,
    };

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

    fn body(session_id: &str) -> SessionBody {
        SessionBody {
            session_id: session_id.to_string(),
            status: SessionStatus::Completed,
            started_at_unix: 1,
            model: "qwen3.8-27b-nvfp4-k8v4".to_string(),
            context_window: 262_144,
            prompt_preview: "fixture".to_string(),
            num_turns: 1,
            last_event_at_unix: 1,
            finished_at_unix: 2,
            duration_wall_ms: 1,
            container_exit_code: 0,
            agent_exit_code: 0,
            is_process_error: false,
            response: "done".to_string(),
            agent_duration_ms: 1,
            bundle_archive_path: String::new(),
            bundle_compressed_bytes: 0,
            bundle_uncompressed_bytes: 0,
            bundle_file_count: 0,
            bundle_artifacts_file_count: 0,
            raw_session_tree_retained: false,
            teardown_diagnostics: Vec::new(),
        }
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
