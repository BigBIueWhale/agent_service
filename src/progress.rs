//! Durable, connection-independent session progress.
//!
//! Progress is a property of the session resource, never of an HTTP
//! connection.  Each publication atomically replaces one complete snapshot
//! containing the full bounded history.  A caller may read that snapshot on
//! any later request, or never read it at all; neither choice changes session
//! execution, cancellation, capture, teardown, or terminal persistence.
//!
//! The replacement protocol writes and syncs `progress.json.next`, renames it
//! over `progress.json`, and syncs the containing directory.  A process crash
//! before the rename leaves the previous complete snapshot plus a uniquely
//! recognizable unpublished file.  Startup discards that unpublished file
//! only after validating its type/owner/mode and the still-authoritative final
//! snapshot.  There is no torn append tail to guess or silently truncate.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::error::{io_msg, ServiceError, ServiceResult};

pub const MAX_PROGRESS_EVENTS: usize = 4096;
const MAX_PROGRESS_MESSAGE_BYTES: usize = 4096;
const PROGRESS_SCHEMA_VERSION: u32 = 1;
// One event contains at most a 4 KiB message plus fixed numeric/structural
// JSON. This bound is intentionally derived from the semantic event limit,
// not from available RAM, so a corrupted service-owned path cannot turn GET
// or restart recovery into an unbounded allocation.
const MAX_PROGRESS_DOCUMENT_BYTES: u64 =
    (MAX_PROGRESS_EVENTS as u64) * ((MAX_PROGRESS_MESSAGE_BYTES as u64) + 1024) + 65_536;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressPhase {
    Accepted,
    Staging,
    PreparingAgent,
    CreatingTopology,
    AwaitingReadiness,
    RunningAgent,
    Cancelling,
    CapturingOutput,
    TearingDown,
    Bundling,
    PersistingTerminal,
    #[default]
    Terminal,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProgressCounters {
    pub staged_bytes: u64,
    pub staged_entries: u64,
    pub staged_regular_files: u64,
    pub output_event_bytes: u64,
    pub num_turns: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProgressEvent {
    pub revision: u64,
    pub at_unix_ms: u64,
    pub phase: ProgressPhase,
    pub message: String,
    pub counters: ProgressCounters,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ProgressDocument {
    schema_version: u32,
    session_id: String,
    events: Vec<ProgressEvent>,
}

struct ProgressState {
    events: Vec<ProgressEvent>,
    durable_failed: bool,
}

enum ReplacementOutcome {
    Durable,
    /// The rename commit point succeeded and exact published bytes were
    /// re-read, but two containing-directory durability barriers failed.
    /// The visible snapshot remains authoritative even though callers must
    /// treat persistence as failed and stop publishing further revisions.
    PublishedWithDurabilityError(ServiceError),
}

#[derive(Clone)]
pub struct ProgressReporter {
    session_id: String,
    path: PathBuf,
    state: Arc<Mutex<ProgressState>>,
}

impl ProgressReporter {
    /// Create revision 1 before the acceptance marker is published.  The
    /// surrounding acceptance transaction owns rollback until that marker is
    /// durably committed.
    pub fn create(path: &Path, session_id: &str, message: &str) -> ServiceResult<Self> {
        validate_message(message)?;
        let initial = ProgressEvent {
            revision: 1,
            at_unix_ms: unix_now_ms(),
            phase: ProgressPhase::Accepted,
            message: message.to_string(),
            counters: ProgressCounters::default(),
        };
        let document = ProgressDocument {
            schema_version: PROGRESS_SCHEMA_VERSION,
            session_id: session_id.to_string(),
            events: vec![initial.clone()],
        };
        write_initial(path, &document)?;
        Ok(Self {
            session_id: session_id.to_string(),
            path: path.to_path_buf(),
            state: Arc::new(Mutex::new(ProgressState {
                events: vec![initial],
                durable_failed: false,
            })),
        })
    }

    /// Reopen a durable snapshot during explicit startup recovery.  Any
    /// unpublished `.next` file is reconciled conservatively first.
    pub fn open(path: &Path, session_id: &str) -> ServiceResult<Self> {
        reconcile_unpublished_replacement(path, session_id)?;
        let events = read_progress_events(path, session_id)?;
        Ok(Self {
            session_id: session_id.to_string(),
            path: path.to_path_buf(),
            state: Arc::new(Mutex::new(ProgressState {
                events,
                durable_failed: false,
            })),
        })
    }

    /// Publish one lifecycle observation.  The in-memory state changes only
    /// after the complete replacement has been synced.  Once persistence has
    /// failed, later calls refuse to imply that durability recovered.
    pub fn publish(
        &self,
        phase: ProgressPhase,
        message: impl Into<String>,
        counters: ProgressCounters,
    ) -> ServiceResult<ProgressEvent> {
        let message = message.into();
        validate_message(&message)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| ServiceError::Internal("progress state mutex was poisoned".to_string()))?;
        if state.durable_failed {
            return Err(ServiceError::Internal(format!(
                "durable progress snapshot {} had already failed; refusing another publication",
                self.path.display()
            )));
        }
        if state.events.len() >= MAX_PROGRESS_EVENTS {
            state.durable_failed = true;
            return Err(ServiceError::Internal(format!(
                "session progress exceeded the exact {MAX_PROGRESS_EVENTS}-event bound"
            )));
        }
        let previous = state.events.last().cloned().ok_or_else(|| {
            ServiceError::Internal("progress snapshot has no initial accepted event".into())
        })?;
        require_monotonic_counters(previous.counters, counters)?;
        let revision = previous
            .revision
            .checked_add(1)
            .ok_or_else(|| ServiceError::Internal("progress revision overflowed u64".into()))?;
        let event = ProgressEvent {
            revision,
            // Wall clocks can step backwards. Resource revisions are the
            // ordering authority, but keeping this diagnostic timestamp
            // nondecreasing prevents a misleading backwards timeline.
            at_unix_ms: unix_now_ms().max(previous.at_unix_ms),
            phase,
            message,
            counters,
        };
        let mut replacement = state.events.clone();
        replacement.push(event.clone());
        let document = ProgressDocument {
            schema_version: PROGRESS_SCHEMA_VERSION,
            session_id: self.session_id.clone(),
            events: replacement.clone(),
        };
        match replace_atomically(&self.path, &document) {
            Ok(ReplacementOutcome::Durable) => {
                state.events = replacement;
                Ok(event)
            }
            Ok(ReplacementOutcome::PublishedWithDurabilityError(error)) => {
                // Rename, not the final directory fsync, is the visibility
                // boundary. Keeping the old in-memory vector here would make
                // finished.json contradict the already-visible progress file.
                state.events = replacement;
                state.durable_failed = true;
                Err(error)
            }
            Err(error) => {
                state.durable_failed = true;
                Err(error)
            }
        }
    }

    pub fn latest(&self) -> ServiceResult<ProgressEvent> {
        self.state
            .lock()
            .map_err(|_| ServiceError::Internal("progress state mutex was poisoned".into()))?
            .events
            .last()
            .cloned()
            .ok_or_else(|| ServiceError::Internal("progress snapshot has no initial event".into()))
    }

    pub fn events(&self) -> ServiceResult<Vec<ProgressEvent>> {
        Ok(self
            .state
            .lock()
            .map_err(|_| ServiceError::Internal("progress state mutex was poisoned".into()))?
            .events
            .clone())
    }
}

fn validate_message(message: &str) -> ServiceResult<()> {
    if message.is_empty() || message.len() > MAX_PROGRESS_MESSAGE_BYTES {
        return Err(ServiceError::Internal(format!(
            "progress message must contain 1..={MAX_PROGRESS_MESSAGE_BYTES} UTF-8 bytes; observed {}",
            message.len()
        )));
    }
    Ok(())
}

fn require_monotonic_counters(
    previous: ProgressCounters,
    next: ProgressCounters,
) -> ServiceResult<()> {
    for (label, old, new) in [
        ("staged_bytes", previous.staged_bytes, next.staged_bytes),
        (
            "staged_entries",
            previous.staged_entries,
            next.staged_entries,
        ),
        (
            "staged_regular_files",
            previous.staged_regular_files,
            next.staged_regular_files,
        ),
        (
            "output_event_bytes",
            previous.output_event_bytes,
            next.output_event_bytes,
        ),
        ("num_turns", previous.num_turns, next.num_turns),
    ] {
        if new < old {
            return Err(ServiceError::Internal(format!(
                "progress counter {label} regressed from {old} to {new}"
            )));
        }
    }
    Ok(())
}

fn write_initial(path: &Path, document: &ProgressDocument) -> ServiceResult<()> {
    let parent = path.parent().ok_or_else(|| {
        ServiceError::Internal(format!(
            "progress path has no parent directory: {}",
            path.display()
        ))
    })?;
    let bytes = encode_document(document)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| {
            ServiceError::Internal(io_msg("create initial progress snapshot", path, &error))
        })?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| {
            ServiceError::Internal(io_msg("write/sync initial progress snapshot", path, &error))
        })?;
    sync_directory(parent, "sync initial progress snapshot")
}

fn replace_atomically(
    path: &Path,
    document: &ProgressDocument,
) -> ServiceResult<ReplacementOutcome> {
    let parent = path.parent().ok_or_else(|| {
        ServiceError::Internal(format!(
            "progress path has no parent directory: {}",
            path.display()
        ))
    })?;
    validate_private_file(path, "current progress snapshot")?;
    let next = next_path(path);
    match std::fs::symlink_metadata(&next) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) => {
            return Err(ServiceError::Internal(format!(
                "unpublished progress replacement already exists at {}; startup reconciliation is required before execution may continue",
                next.display()
            )));
        }
        Err(error) => {
            return Err(ServiceError::Internal(io_msg(
                "stat unpublished progress replacement",
                &next,
                &error,
            )));
        }
    }
    let bytes = encode_document(document)?;
    let mut created = false;
    let precommit = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&next)
            .map_err(|error| {
                ServiceError::Internal(io_msg(
                    "create unpublished progress replacement",
                    &next,
                    &error,
                ))
            })?;
        created = true;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| {
                ServiceError::Internal(io_msg(
                    "write/sync unpublished progress replacement",
                    &next,
                    &error,
                ))
            })?;
        std::fs::rename(&next, path).map_err(|error| {
            ServiceError::Internal(io_msg(
                "atomically publish progress replacement",
                path,
                &error,
            ))
        })?;
        created = false;
        Ok(())
    })();
    match precommit {
        Ok(()) => {}
        Err(error) => {
            if created {
                if let Err(cleanup) = std::fs::remove_file(&next) {
                    if cleanup.kind() != std::io::ErrorKind::NotFound {
                        return Err(ServiceError::Internal(format!(
                            "{error}; cleanup of unpublished progress replacement {} also failed: {cleanup}",
                            next.display()
                        )));
                    }
                }
                if let Err(cleanup) = sync_directory(parent, "sync unpublished progress rollback") {
                    return Err(ServiceError::Internal(format!(
                        "{error}; durable cleanup of unpublished progress replacement {} also failed: {cleanup}",
                        next.display()
                    )));
                }
            }
            return Err(error);
        }
    }

    match sync_directory(parent, "sync progress replacement publication") {
        Ok(()) => Ok(ReplacementOutcome::Durable),
        Err(first_error) => {
            // The rename already committed visibility. Verify the exact
            // bytes through an O_NOFOLLOW descriptor before adopting them in
            // memory, then retry the durability barrier once. Never roll a
            // visible resource back to an older revision.
            let observed = read_private_bytes(path, "visible progress replacement")?;
            if observed != bytes {
                return Err(ServiceError::Internal(format!(
                    "{first_error}; the visible progress replacement at {} differs from the exact transaction bytes",
                    path.display()
                )));
            }
            match sync_directory(parent, "retry progress replacement publication barrier") {
                Ok(()) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %first_error,
                        "the initial progress-directory sync failed after publication, but its immediate explicit retry succeeded"
                    );
                    Ok(ReplacementOutcome::Durable)
                }
                Err(retry_error) => Ok(ReplacementOutcome::PublishedWithDurabilityError(
                    ServiceError::Internal(format!(
                        "the progress replacement at {} is visible and byte-exact, but both directory durability barriers failed ({first_error}; retry: {retry_error}); later terminal evidence will retain this failure and no further progress revision will be published",
                        path.display()
                    )),
                )),
            }
        }
    }
}

fn encode_document(document: &ProgressDocument) -> ServiceResult<Vec<u8>> {
    validate_document(document, &document.session_id)?;
    let mut bytes = serde_json::to_vec_pretty(document).map_err(|error| {
        ServiceError::Internal(format!(
            "serialize progress snapshot for {}: {error}",
            document.session_id
        ))
    })?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub fn read_progress_events(path: &Path, session_id: &str) -> ServiceResult<Vec<ProgressEvent>> {
    let bytes = read_private_bytes(path, "durable progress snapshot")?;
    let document: ProgressDocument = serde_json::from_slice(&bytes).map_err(|error| {
        ServiceError::Internal(format!(
            "durable progress snapshot {} is malformed: {error}",
            path.display()
        ))
    })?;
    validate_document(&document, session_id)?;
    Ok(document.events)
}

fn validate_document(document: &ProgressDocument, session_id: &str) -> ServiceResult<()> {
    if document.schema_version != PROGRESS_SCHEMA_VERSION || document.session_id != session_id {
        return Err(ServiceError::Internal(format!(
            "progress snapshot identity/schema drift: expected session {session_id:?} schema {PROGRESS_SCHEMA_VERSION}, observed session {:?} schema {}",
            document.session_id, document.schema_version
        )));
    }
    if document.events.is_empty() || document.events.len() > MAX_PROGRESS_EVENTS {
        return Err(ServiceError::Internal(format!(
            "progress snapshot for {session_id} must contain 1..={MAX_PROGRESS_EVENTS} events; observed {}",
            document.events.len()
        )));
    }
    for (index, event) in document.events.iter().enumerate() {
        let expected = u64::try_from(index)
            .map_err(|_| ServiceError::Internal("progress index does not fit u64".into()))?
            .checked_add(1)
            .ok_or_else(|| ServiceError::Internal("progress revision overflowed u64".into()))?;
        if event.revision != expected {
            return Err(ServiceError::Internal(format!(
                "progress snapshot for {session_id} has revision {} at record {}, expected {expected}",
                event.revision,
                index + 1
            )));
        }
        validate_message(&event.message)?;
        if let Some(previous) = index
            .checked_sub(1)
            .and_then(|previous| document.events.get(previous))
        {
            if event.at_unix_ms < previous.at_unix_ms {
                return Err(ServiceError::Internal(format!(
                    "progress snapshot for {session_id} has a backwards timestamp at revision {}",
                    event.revision
                )));
            }
            require_monotonic_counters(previous.counters, event.counters)?;
        }
    }
    if document.events[0].phase != ProgressPhase::Accepted {
        return Err(ServiceError::Internal(format!(
            "progress snapshot for {session_id} does not begin with the accepted phase"
        )));
    }
    Ok(())
}

/// Remove only a provably service-owned unpublished replacement while
/// retaining the last complete snapshot.  This is startup reconciliation,
/// not a runtime fallback.
pub fn reconcile_unpublished_replacement(path: &Path, session_id: &str) -> ServiceResult<()> {
    let next = next_path(path);
    let next_metadata = match std::fs::symlink_metadata(&next) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(ServiceError::Internal(io_msg(
                "stat unpublished progress replacement",
                &next,
                &error,
            )));
        }
    };
    if next_metadata.is_none() {
        return read_progress_events(path, session_id).map(|_| ());
    }
    validate_private_metadata(
        &next,
        next_metadata.as_ref().expect("checked Some"),
        "unpublished progress replacement",
    )?;
    // The committed snapshot must be independently valid before the
    // unpublished candidate is discarded.  The candidate may be a torn file
    // because the crash can occur during its write, so its contents are not
    // interpreted as committed state.
    read_progress_events(path, session_id)?;
    std::fs::remove_file(&next).map_err(|error| {
        ServiceError::Internal(io_msg(
            "remove unpublished progress replacement",
            &next,
            &error,
        ))
    })?;
    sync_directory(
        path.parent().expect("progress snapshot has parent"),
        "sync unpublished progress cleanup",
    )?;
    tracing::warn!(
        session_id,
        path = %next.display(),
        "discarded an unpublished progress replacement after validating the last complete snapshot"
    );
    Ok(())
}

fn next_path(path: &Path) -> PathBuf {
    path.with_file_name("progress.json.next")
}

fn validate_private_file(path: &Path, role: &str) -> ServiceResult<()> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| ServiceError::Internal(io_msg(&format!("stat {role}"), path, &error)))?;
    validate_private_metadata(path, &metadata, role)
}

fn validate_private_metadata(
    path: &Path,
    metadata: &std::fs::Metadata,
    role: &str,
) -> ServiceResult<()> {
    let expected_uid = unsafe { libc::geteuid() };
    let expected_gid = unsafe { libc::getegid() };
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o777 != 0o600
        || metadata.uid() != expected_uid
        || metadata.gid() != expected_gid
        || metadata.len() > MAX_PROGRESS_DOCUMENT_BYTES
    {
        return Err(ServiceError::Internal(format!(
            "{role} {} has unsafe type/mode/owner/size: type={:?} mode={:o} uid={} gid={} size={} max_size={} expected={}:{}",
            path.display(),
            metadata.file_type(),
            metadata.permissions().mode() & 0o777,
            metadata.uid(),
            metadata.gid(),
            metadata.len(),
            MAX_PROGRESS_DOCUMENT_BYTES,
            expected_uid,
            expected_gid
        )));
    }
    Ok(())
}

fn read_private_bytes(path: &Path, role: &str) -> ServiceResult<Vec<u8>> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| {
            ServiceError::Internal(io_msg(
                &format!("open {role} without following links"),
                path,
                &error,
            ))
        })?;
    let metadata = file.metadata().map_err(|error| {
        ServiceError::Internal(io_msg(&format!("fstat opened {role}"), path, &error))
    })?;
    validate_private_metadata(path, &metadata, role)?;
    let capacity = usize::try_from(metadata.len()).map_err(|_| {
        ServiceError::Internal(format!(
            "{role} {} is too large to address on this platform",
            path.display()
        ))
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    file.read_to_end(&mut bytes).map_err(|error| {
        ServiceError::Internal(io_msg(&format!("read opened {role}"), path, &error))
    })?;
    if bytes.len() as u64 != metadata.len() {
        return Err(ServiceError::Internal(format!(
            "{role} {} changed length while open: fstat={} read={}",
            path.display(),
            metadata.len(),
            bytes.len()
        )));
    }
    Ok(bytes)
}

fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn sync_directory(path: &Path, context: &str) -> ServiceResult<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| ServiceError::Internal(io_msg(context, path, &error)))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::OpenOptionsExt;

    use super::{read_progress_events, ProgressCounters, ProgressPhase, ProgressReporter};

    #[test]
    fn progress_is_atomic_monotonic_complete_and_reopenable() {
        let root =
            std::env::temp_dir().join(format!("qwen38-progress-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir(&root).expect("create progress fixture");
        let path = root.join("progress.json");
        let session_id = "s-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let reporter =
            ProgressReporter::create(&path, session_id, "accepted").expect("create reporter");
        let second = reporter
            .publish(
                ProgressPhase::Staging,
                "copied source entries",
                ProgressCounters {
                    staged_bytes: 17,
                    staged_entries: 2,
                    staged_regular_files: 1,
                    ..ProgressCounters::default()
                },
            )
            .expect("publish second event");
        assert_eq!(second.revision, 2);
        assert_eq!(
            read_progress_events(&path, session_id).expect("read durable snapshot"),
            reporter.events().expect("read in-memory snapshot")
        );
        let reopened = ProgressReporter::open(&path, session_id).expect("reopen reporter");
        assert_eq!(reopened.latest().expect("latest after reopen"), second);

        let regression = reopened.publish(
            ProgressPhase::RunningAgent,
            "counter regression must fail",
            ProgressCounters::default(),
        );
        assert!(regression.is_err());
        assert_eq!(
            reopened.latest().expect("latest after rejected regression"),
            second
        );

        // A crash before rename may leave a partial unpublished candidate.
        // Reopen validates the committed snapshot and removes only that exact
        // mode-0600 candidate without interpreting its torn contents.
        let next = root.join("progress.json.next");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);
        let mut file = options.open(&next).expect("create torn next fixture");
        std::io::Write::write_all(&mut file, b"{torn").expect("write torn next fixture");
        file.sync_all().expect("sync torn next fixture");
        let reopened = ProgressReporter::open(&path, session_id).expect("reconcile torn next");
        assert!(!next.exists());
        assert_eq!(reopened.latest().expect("latest after recovery"), second);

        std::fs::remove_dir_all(root).expect("remove progress fixture");
    }
}
