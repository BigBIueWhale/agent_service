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
//!   permit is moved into the spawned run task and held for its entire
//!   lifetime; submit `try_acquire`s and translates failure to `Busy`.
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
//! - **Writes are idempotent**: `cancel` on a terminal session returns the
//!   current body without erroring; `delete` on a missing session returns
//!   `NotFound` (a definite "gone" rather than a silent success, because
//!   the operator may want to know).
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
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, Notify, Semaphore, oneshot};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::config::Config;
use crate::error::{ServiceError, ServiceResult, io_msg};
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
    pub ttyd_url: String,
    pub prompt_preview: String,

    // Populated on transition to terminal. Zeroed/empty while running.
    pub finished_at_unix: u64,
    pub duration_wall_ms: u64,
    pub container_exit_code: i32,
    pub is_error: bool,
    pub response: String,
    pub agent_num_turns: u64,
    pub agent_duration_ms: u64,
    pub bundle_archive_path: String,
    pub bundle_compressed_bytes: u64,
    pub bundle_uncompressed_bytes: u64,
    pub bundle_file_count: u64,
    pub bundle_artifacts_file_count: u64,
    pub teardown_diagnostics: Vec<String>,
}

/// The "ttyd is reachable" snapshot the run task hands back to `submit`
/// over the readiness oneshot. `submit` then uses it to fill in the
/// running-state fields of the public `SessionBody` it returns to the
/// HTTP caller, and to update the in-memory entry that `cancel` / `list` /
/// `get` see.
#[derive(Clone, Debug)]
pub struct RunningSnapshot {
    pub session_id: String,
    pub started_at_unix: u64,
    pub ttyd_url: String,
    pub prompt_preview: String,
}

/// In-memory entry for a running session. Removed from the map by the run
/// task on transition-to-terminal (after `finished.json` has been
/// successfully persisted). After removal, every observer falls back to
/// the on-disk record.
struct RunningEntry {
    /// Mutable across the entry's lifetime: starts as a placeholder with
    /// empty `ttyd_url` and `started_at_unix=0`, replaced once by `submit`
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
    /// One permit. Held by the spawned run task for the entire lifetime
    /// of a session. `submit()` `try_acquire`s; `shutdown()` blocks on
    /// `acquire()` so a successful return guarantees no in-flight session.
    singleton: Arc<Semaphore>,
    /// Cancelled at the top of `shutdown`. `submit` checks this before
    /// taking the permit, so post-shutdown submits fail fast. Each session's
    /// per-session cancel token is a child of this one.
    shutdown_token: CancellationToken,
}

struct Inner {
    running: HashMap<String, Arc<RunningEntry>>,
}

impl Manager {
    pub fn new(cfg: Arc<Config>) -> Self {
        Self {
            cfg,
            inner: Arc::new(Mutex::new(Inner {
                running: HashMap::new(),
            })),
            singleton: Arc::new(Semaphore::new(1)),
            shutdown_token: CancellationToken::new(),
        }
    }

    /// Submit a new session.
    ///
    /// Blocks until the agent's ttyd listener is reachable through the
    /// per-session sidecar (typically a few seconds; bounded internally by
    /// `session::run_one`'s per-step timeouts). Returns the `running` view
    /// once `ttyd_url` is populated and the session is observable via
    /// `get` / `list` / `cancel`.
    ///
    /// Errors:
    /// - `Internal("server is shutting down …")` if shutdown has begun.
    /// - `Busy{running_session_id}` if the singleton is already held.
    /// - Any setup error from `session::run_one` that fires before ttyd is
    ///   reachable (docker run failure, network setup failure, …).
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
        // and "ttyd-up" can find this session and signal its cancel token.
        let placeholder = RunningSnapshot {
            session_id: session_id.clone(),
            started_at_unix: 0,
            ttyd_url: String::new(),
            prompt_preview: prompt_preview.clone(),
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

        let (ttyd_tx, ttyd_rx) = oneshot::channel::<ServiceResult<RunningSnapshot>>();

        // Spawn the run task. It owns the permit, the cancel token (clone),
        // and the manager's `inner` handle so it can self-evict from the
        // map at the moment of transition to terminal state.
        let cfg = Arc::clone(&self.cfg);
        let inner_for_task = Arc::clone(&self.inner);
        let session_id_for_task = session_id.clone();
        let cancel_for_task = session_cancel.clone();
        let finished_for_task = Arc::clone(&finished);
        let prompt_preview_for_task = prompt_preview.clone();
        tokio::spawn(async move {
            // The permit is dropped on task exit no matter what (panic, error,
            // normal completion). This is the cornerstone of the singleton
            // guarantee.
            let _permit = permit;
            let body = session::run_one(
                &cfg,
                &session_id_for_task,
                req,
                cancel_for_task,
                ttyd_tx,
                prompt_preview_for_task,
            )
            .await;

            // Persist the terminal record before removing from the map. Order
            // matters: a concurrent `get` will fall back to disk the moment
            // the map entry is gone, so disk must be ready first.
            if let Err(e) = persist_terminal(&cfg, &body).await {
                tracing::error!(
                    session_id = %body.session_id,
                    error = %e,
                    "persist_terminal failed; the terminal record could not be written. \
                     The map entry will still be evicted (correct), but a subsequent `get` \
                     for this session will return NotFound (degraded). Investigate disk."
                );
            }

            inner_for_task
                .lock()
                .await
                .running
                .remove(&session_id_for_task);

            // Wake every observer (cancel waiter, shutdown waiter).
            finished_for_task.notify_waiters();
        });

        // Wait for either ttyd-up (success) or early error (the run task is
        // already running its own teardown).
        match ttyd_rx.await {
            Ok(Ok(snapshot)) => {
                // Update the entry with the real values now that we have
                // them. Held briefly.
                *entry.snapshot.lock().await = snapshot.clone();
                Ok(running_body(&snapshot))
            }
            Ok(Err(e)) => {
                // The run task is doing teardown right now. Wait for it to
                // finish so the singleton is visibly free before we return
                // the error to the client (clients can otherwise observe a
                // surprising "Busy" on an immediate retry).
                finished.notified().await;
                Err(e)
            }
            Err(_oneshot_dropped) => {
                // The task dropped the sender without sending — by
                // construction, `session::run_one` always sends, so this
                // path indicates a panic or unwind.
                finished.notified().await;
                Err(ServiceError::Internal(format!(
                    "submit({session_id}): readiness channel was dropped without a value — \
                     the run task most likely panicked. Check the tracing log for the panic site \
                     and the session's container logs (if any) via `docker logs agent-{session_id}`"
                )))
            }
        }
    }

    /// Pure read of a session by id. Looks in memory first (running
    /// sessions), falls back to disk (terminal sessions).
    pub async fn get(&self, session_id: &str) -> ServiceResult<SessionBody> {
        if let Some(entry) = self.inner.lock().await.running.get(session_id).cloned() {
            let snap = entry.snapshot.lock().await.clone();
            return Ok(running_body(&snap));
        }
        read_terminal(&self.cfg, session_id).await
    }

    /// Pure read of every visible session. Combines in-memory running
    /// entries with on-disk terminal entries (the on-disk records survive
    /// across server restart).
    pub async fn list(&self) -> ServiceResult<Vec<SessionBody>> {
        let mut bodies: Vec<SessionBody> = {
            let inner = self.inner.lock().await;
            let mut v = Vec::with_capacity(inner.running.len());
            for entry in inner.running.values() {
                let snap = entry.snapshot.lock().await.clone();
                v.push(running_body(&snap));
            }
            v
        };

        let running_ids: std::collections::HashSet<String> = bodies
            .iter()
            .map(|b| b.session_id.clone())
            .collect();

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
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!(error = %e, "list: read_dir entry — skipping");
                    continue;
                }
            };
            let path = entry.path();
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            if running_ids.contains(&name) {
                continue;
            }
            // Skip dirs that don't have finished.json yet — these are
            // either partial bundles from a crashed server (rare) or
            // half-written rename targets (microsecond window). Either
            // way they'd show up the next time `list` is called once
            // finished.json lands.
            match read_terminal(&self.cfg, &name).await {
                Ok(body) => bodies.push(body),
                Err(ServiceError::NotFound { .. }) => continue,
                Err(e) => {
                    tracing::warn!(
                        session_id = %name,
                        error = %e,
                        "list: skipping unreadable terminal record"
                    );
                }
            }
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

    /// Remove a terminal session from disk. The lifecycle is explicit:
    /// `delete` refuses to act on a running session (`SessionRunning`/409)
    /// — the operator must `cancel` first.
    ///
    /// Returns `NotFound` for unknown ids (informative — repeat callers
    /// see "yes, it's gone" rather than a silent success).
    pub async fn delete(&self, session_id: &str) -> ServiceResult<()> {
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
        let dir = self.cfg.results_dir.join(session_id);
        match tokio::fs::remove_dir_all(&dir).await {
            Ok(()) => Ok(()),
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
    /// `ceiling` caps the total wait. On overrun, returns `Timeout` —
    /// the server may then exit with a non-zero status; the OS reclaims
    /// any leaked resources, and the next startup's orphan sweep picks up
    /// any leftover docker objects.
    pub async fn shutdown(&self, ceiling: Duration) -> ServiceResult<()> {
        // Refuse new submissions. Already-spawned setup will observe the
        // cascade-cancellation below.
        self.shutdown_token.cancel();

        let in_flight: Vec<String> = self
            .inner
            .lock()
            .await
            .running
            .keys()
            .cloned()
            .collect();
        if in_flight.is_empty() && self.singleton.available_permits() == 1 {
            tracing::info!("shutdown: no in-flight session — clean exit");
            return Ok(());
        }
        tracing::info!(
            sessions = ?in_flight,
            "shutdown: cancellation cascaded; awaiting teardown"
        );

        // Acquiring the permit guarantees the run task has fully exited
        // (the run task drops the permit at the very end of its
        // tokio::spawn closure). Any in-flight `submit` that was between
        // "permit acquired" and "map insert" will also have its task
        // observe the cascade-cancellation and terminate, releasing the
        // permit.
        let permit_acquire = Arc::clone(&self.singleton).acquire_owned();
        match tokio::time::timeout(ceiling, permit_acquire).await {
            Ok(Ok(_permit)) => {
                tracing::info!("shutdown: singleton permit free — all sessions terminal");
                Ok(())
            }
            Ok(Err(closed)) => {
                // Semaphore::close() was called somewhere; we do not call
                // it ourselves, so this path is unexpected.
                Err(ServiceError::Internal(format!(
                    "shutdown: semaphore closed unexpectedly: {closed}"
                )))
            }
            Err(_elapsed) => {
                let still: Vec<String> = self
                    .inner
                    .lock()
                    .await
                    .running
                    .keys()
                    .cloned()
                    .collect();
                Err(ServiceError::Timeout(format!(
                    "shutdown: {} session(s) did not reach terminal state within {:?} ({}); \
                     leftover docker containers and networks (if any) will be reaped on the \
                     next startup's orphan sweep",
                    still.len(),
                    ceiling,
                    if still.is_empty() {
                        "in-flight setup did not yield".to_string()
                    } else {
                        still.join(", ")
                    }
                )))
            }
        }
    }
}

// ── helpers ───────────────────────────────────────────────────────────────

/// Build the wire body for a running snapshot. Required-field discipline:
/// every field present, terminal-only fields zeroed.
fn running_body(s: &RunningSnapshot) -> SessionBody {
    SessionBody {
        session_id: s.session_id.clone(),
        status: SessionStatus::Running,
        started_at_unix: s.started_at_unix,
        ttyd_url: s.ttyd_url.clone(),
        prompt_preview: s.prompt_preview.clone(),
        finished_at_unix: 0,
        duration_wall_ms: 0,
        container_exit_code: 0,
        is_error: false,
        response: String::new(),
        agent_num_turns: 0,
        agent_duration_ms: 0,
        bundle_archive_path: String::new(),
        bundle_compressed_bytes: 0,
        bundle_uncompressed_bytes: 0,
        bundle_file_count: 0,
        bundle_artifacts_file_count: 0,
        teardown_diagnostics: Vec::new(),
    }
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

/// Persist the terminal record atomically to disk. Writes `finished.json.tmp`,
/// then renames over `finished.json` so observers never see a partial file.
async fn persist_terminal(cfg: &Config, body: &SessionBody) -> ServiceResult<()> {
    let dir = cfg.results_dir.join(&body.session_id);
    tokio::fs::create_dir_all(&dir).await.map_err(|e| {
        ServiceError::Internal(io_msg("persist_terminal: create_dir_all", &dir, &e))
    })?;
    let final_path = dir.join("finished.json");
    let tmp_path = dir.join("finished.json.tmp");
    let bytes = serde_json::to_vec_pretty(body).map_err(|e| {
        ServiceError::Internal(format!(
            "persist_terminal({}): serde_json failure on terminal SessionBody: {e}",
            body.session_id
        ))
    })?;
    tokio::fs::write(&tmp_path, &bytes).await.map_err(|e| {
        ServiceError::Internal(io_msg("persist_terminal: write tmp", &tmp_path, &e))
    })?;
    tokio::fs::rename(&tmp_path, &final_path).await.map_err(|e| {
        ServiceError::Internal(format!(
            "persist_terminal({}): rename {} → {}: {e}",
            body.session_id,
            tmp_path.display(),
            final_path.display()
        ))
    })?;
    Ok(())
}

/// Read the on-disk terminal record. `NotFound` if the directory or
/// finished.json doesn't exist; `Internal` on read or parse failure.
async fn read_terminal(cfg: &Config, session_id: &str) -> ServiceResult<SessionBody> {
    let path = finished_json_path(cfg, session_id);
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
fn is_safe_session_id(s: &str) -> bool {
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
