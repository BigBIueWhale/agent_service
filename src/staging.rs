//! Per-session host-side filesystem layout, plus extraction of the one
//! submitted workspace archive into the staging tree.
//!
//! Layout under `<state_dir>/sessions/<id>/`:
//!
//! ```text
//! staged/         ← bind-mounted into agent container as /workspace (rw)
//! artifacts/      ← bind-mounted into agent container as /artifacts (rw)
//!                   empty at start; agent writes any files it wants
//!                   returned to the operator. Bundled at end-of-run.
//! control/        ← bind-mounted into agent container as /run/agent (ro)
//!   prompt.txt
//!   turn-budget.json
//!   start-gate.lock
//! streams/        ← capture component rw; Qwen container ro
//!   events.sock
//!   stderr.sock
//! output/         ← bind-mounted only into the trusted capture component
//!   (Qwen never receives this mount.)
//! ```
//!
//! The workspace arrives over the connection as a hash-committed zip and is
//! extracted here into a tree the agent can mutate freely; no host path is
//! ever read or bind-mounted as a source. Symbolic-link entries are staged as
//! opaque link-target bytes and are never followed, flattened, rewritten, or
//! used as traversal roots by this service. Resolution happens later inside
//! the agent's isolated mount namespace, where Landlock still governs the
//! resolved write target. Only the submitted outermost archive is extracted:
//! an archive inside it stays an ordinary staged file.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::config::{
    MAX_SESSION_TURNS_CEILING, MAX_STAGED_BYTES, MAX_STAGED_ENTRIES, MAX_STAGED_FILES,
};
use crate::error::{io_msg, ServiceError, ServiceResult};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StagingProgress {
    pub copied_bytes: u64,
    pub copied_entries: u64,
    pub copied_regular_files: u64,
}

#[derive(Debug, Clone)]
pub struct SessionPaths {
    pub root: PathBuf,
    pub staged: PathBuf,
    pub artifacts: PathBuf,
    pub control: PathBuf,
    pub streams: PathBuf,
    pub output: PathBuf,
}

impl SessionPaths {
    pub fn new(state_dir: &Path, session_id: &str) -> Self {
        let root = state_dir.join("sessions").join(session_id);
        Self {
            staged: root.join("staged"),
            artifacts: root.join("artifacts"),
            control: root.join("control"),
            streams: root.join("streams"),
            output: root.join("output"),
            root,
        }
    }

    pub fn create_dirs(&self) -> ServiceResult<()> {
        // A session ID is never allowed to adopt a stale/pre-created tree.
        // Create the root exclusively, then every fixed child exclusively.
        // If this attempt fails after creating the root, remove only that
        // exact newly-created tree; a collision at the root is preserved for
        // startup/operator reconciliation.
        let sessions_parent = self.root.parent().ok_or_else(|| {
            ServiceError::Staging(format!(
                "session root {} has no sessions parent",
                self.root.display()
            ))
        })?;
        std::fs::create_dir(&self.root).map_err(|error| {
            ServiceError::Staging(io_msg(
                "exclusively create session root",
                &self.root,
                &error,
            ))
        })?;
        let result = (|| {
            set_directory_mode(&self.root, 0o755)?;
            // Read-only bind mount `control` and the parent `root` are 0755;
            // the fixed uid-1000 container must be able to traverse them.
            for directory in [&self.staged, &self.control] {
                create_exact_directory(directory, 0o755)?;
            }
            // The service and all three per-session containers are pinned to
            // uid/gid 1000. Private directories permit only intended writers.
            for directory in [&self.artifacts, &self.streams, &self.output] {
                create_exact_directory(directory, 0o700)?;
            }
            sync_directory(&self.root, "sync complete session layout")?;
            sync_directory(sessions_parent, "sync exclusive session-root publication")?;
            Ok(())
        })();
        if let Err(error) = result {
            return match std::fs::remove_dir_all(&self.root) {
                Ok(()) => match sync_directory(
                    sessions_parent,
                    "sync partial session-root cleanup",
                ) {
                    Ok(()) => Err(error),
                    Err(sync_error) => Err(ServiceError::Staging(format!(
                        "{error}; partial session root was removed but its parent could not be synced: {sync_error}"
                    ))),
                },
                Err(cleanup_error) => Err(ServiceError::Staging(format!(
                    "{error}; cleanup of partially-created session root {} also failed: {cleanup_error}",
                    self.root.display()
                ))),
            };
        }
        Ok(())
    }

    /// Panic recovery may run after this exact in-memory session already
    /// created its layout. Reuse is permitted only after validation; no mode,
    /// owner, type, or path is repaired or adopted. If execution panicked
    /// before creating the root, perform the normal exclusive transaction.
    pub fn ensure_recovery_dirs(&self) -> ServiceResult<()> {
        match std::fs::symlink_metadata(&self.root) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => self.create_dirs(),
            Err(error) => Err(ServiceError::Staging(io_msg(
                "stat panic-recovery session root",
                &self.root,
                &error,
            ))),
            Ok(_) => {
                for (path, mode) in [
                    (&self.root, 0o755),
                    (&self.staged, 0o755),
                    (&self.control, 0o755),
                    (&self.artifacts, 0o700),
                    (&self.streams, 0o700),
                    (&self.output, 0o700),
                ] {
                    validate_existing_session_directory(path, mode)?;
                }
                Ok(())
            }
        }
    }

    pub fn write_prompt(&self, prompt: &str) -> ServiceResult<PathBuf> {
        let p = self.control.join("prompt.txt");
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&p)
            .map_err(|e| ServiceError::Staging(io_msg("open prompt.txt", &p, &e)))?;
        f.write_all(prompt.as_bytes())
            .map_err(|e| ServiceError::Staging(io_msg("write prompt.txt", &p, &e)))?;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644))
            .map_err(|e| ServiceError::Staging(io_msg("chmod 0644 prompt.txt", &p, &e)))?;
        f.flush()
            .and_then(|_| f.sync_all())
            .map_err(|e| ServiceError::Staging(io_msg("flush/sync prompt.txt", &p, &e)))?;
        sync_directory(&self.control, "sync prompt publication")?;
        Ok(p)
    }

    /// Publish the one canonical per-session turn-budget record.
    ///
    /// This is the exact number the launcher inside the agent container reads
    /// and passes to Qwen Code as `--max-session-turns`, so it is the only
    /// place the per-session budget crosses into the agent: the agent has no
    /// environment, argument, or network channel that could carry it instead.
    /// It is deliberately canonical byte data for the same reason the history
    /// policy is -- the launcher proves the exact bytes rather than parsing a
    /// permissive document -- and it is bundled, so a completed session can be
    /// read back to see which budget it actually ran under.
    pub fn write_turn_budget(&self, max_session_turns: u32) -> ServiceResult<PathBuf> {
        let path = self.control.join("turn-budget.json");
        let contents = turn_budget_record(max_session_turns);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|error| {
                ServiceError::Staging(io_msg("open turn-budget.json", &path, &error))
            })?;
        file.write_all(contents.as_bytes()).map_err(|error| {
            ServiceError::Staging(io_msg("write turn-budget.json", &path, &error))
        })?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).map_err(
            |error| {
                ServiceError::Staging(io_msg("chmod 0444 turn-budget.json", &path, &error))
            },
        )?;
        file.flush().and_then(|_| file.sync_all()).map_err(|error| {
            ServiceError::Staging(io_msg("flush/sync turn-budget.json", &path, &error))
        })?;
        sync_directory(&self.control, "sync turn-budget publication")?;
        Ok(path)
    }

    /// Create and exclusively lock the cross-container start gate before the
    /// agent exists. The wrapper blocks in `flock(1)` on the read-only bind;
    /// the service releases this descriptor only after the broker has proved
    /// that the fixed model relay is listening.
    pub fn create_locked_start_gate(&self) -> ServiceResult<std::fs::File> {
        let path = self.control.join("start-gate.lock");
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| ServiceError::Staging(io_msg("create start gate", &path, &error)))?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).map_err(
            |error| ServiceError::Staging(io_msg("chmod 0600 start gate", &path, &error)),
        )?;
        file.sync_all()
            .map_err(|error| ServiceError::Staging(io_msg("sync start gate", &path, &error)))?;
        sync_directory(&self.control, "sync start-gate publication")?;
        std::fs::File::lock(&file)
            .map_err(|error| ServiceError::Staging(io_msg("lock start gate", &path, &error)))?;
        Ok(file)
    }

    pub fn events_jsonl(&self) -> PathBuf {
        self.output.join("events.jsonl")
    }

    /// The accepted workspace archive, relocated from its upload spool into
    /// the session tree at durable acceptance and removed immediately after
    /// extraction. It lives at the session root, outside every bundled
    /// subtree, so it can never leak into a result bundle.
    pub fn input_archive(&self) -> PathBuf {
        self.root.join("input-archive.zip")
    }
}

/// The exact bytes of the one canonical per-session turn-budget record.
///
/// One line, one field, no whitespace variation: every reader on the far side
/// of a mount boundary compares bytes instead of accepting whatever a JSON
/// parser would tolerate.
pub fn turn_budget_record(max_session_turns: u32) -> String {
    format!("{{\"max_session_turns\":{max_session_turns}}}\n")
}

/// Read a turn-budget record back, returning the budget only for the exact
/// canonical spelling of a budget this deployment can actually run.
///
/// Leading zeros, whitespace, a second field, a missing terminal newline, a
/// zero budget, and anything above the pinned ceiling are all `None`: a
/// malformed record is never repaired into a plausible-looking budget.
pub fn parse_turn_budget_record(bytes: &[u8]) -> Option<u32> {
    let digits = std::str::from_utf8(bytes)
        .ok()?
        .strip_prefix("{\"max_session_turns\":")?
        .strip_suffix("}\n")?;
    if digits.is_empty()
        || digits.len() > 10
        || digits.starts_with('0')
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let turns: u32 = digits.parse().ok()?;
    if turns == 0 || turns > MAX_SESSION_TURNS_CEILING {
        return None;
    }
    Some(turns)
}

fn create_exact_directory(path: &Path, mode: u32) -> ServiceResult<()> {
    std::fs::create_dir(path).map_err(|error| {
        ServiceError::Staging(io_msg("exclusively create session dir", path, &error))
    })?;
    set_directory_mode(path, mode)
}

fn set_directory_mode(path: &Path, mode: u32) -> ServiceResult<()> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).map_err(|error| {
        ServiceError::Staging(io_msg(
            &format!("chmod {mode:04o} session dir"),
            path,
            &error,
        ))
    })
}

fn sync_directory(path: &Path, context: &str) -> ServiceResult<()> {
    std::fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| ServiceError::Staging(io_msg(context, path, &error)))
}

fn validate_existing_session_directory(path: &Path, mode: u32) -> ServiceResult<()> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        ServiceError::Staging(io_msg(
            "stat existing panic-recovery session dir",
            path,
            &error,
        ))
    })?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != 1000
        || metadata.gid() != 1000
        || metadata.permissions().mode() & 0o777 != mode
    {
        return Err(ServiceError::Staging(format!(
            "panic-recovery session directory drift at {}: type={:?} uid={} gid={} mode={:o}; expected ordinary 1000:1000 mode={mode:04o}",
            path.display(),
            metadata.file_type(),
            metadata.uid(),
            metadata.gid(),
            metadata.permissions().mode() & 0o777,
        )));
    }
    let canonical = std::fs::canonicalize(path).map_err(|error| {
        ServiceError::Staging(io_msg(
            "canonicalize existing panic-recovery session dir",
            path,
            &error,
        ))
    })?;
    if canonical != path {
        return Err(ServiceError::Staging(format!(
            "panic-recovery session directory canonicalization drift: expected {}, observed {}",
            path.display(),
            canonical.display()
        )));
    }
    Ok(())
}

/// One structurally validated archive entry, in archive order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArchiveEntryPlan {
    /// Canonical relative path without a trailing separator.
    pub name: String,
    pub kind: ArchiveEntryKind,
    /// Central-directory declared uncompressed size in bytes.
    pub declared_bytes: u64,
    /// Any-executable bit from the recorded Unix mode. Entries without a
    /// recorded Unix mode stage as non-executable regular data.
    pub executable: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArchiveEntryKind {
    Directory,
    RegularFile,
    SymbolicLink,
}

/// Declared extraction totals from the structural pass, using the same
/// accounting as descriptor-anchored folder staging: `declared_entries`
/// counts every filesystem object extraction will create, including parent
/// directories the archive implies but does not list explicitly.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ArchiveSummary {
    pub declared_entries: u64,
    pub declared_regular_files: u64,
    pub declared_regular_file_bytes: u64,
}

/// Structural validation of a submitted workspace archive, without
/// decompressing any regular-file content. Every entry name must be a
/// canonical relative UTF-8 path (no NUL, no absolute prefix, no empty, `.`
/// or `..` component, no duplicate after directory-slash normalisation), and
/// every entry must be a directory, regular file, or symbolic link. A name
/// used as a parent directory by another entry must either be absent (an
/// implied directory) or be an explicit directory entry. Declared sizes are
/// bounded by the exact staging caps before any extraction begins. Nested
/// archives receive no special treatment anywhere in this module: an inner
/// archive is an ordinary regular file, and only the submitted outermost
/// archive is ever extracted.
pub fn validate_archive_structure(
    archive_path: &Path,
) -> ServiceResult<(Vec<ArchiveEntryPlan>, ArchiveSummary)> {
    let mut file = open_source_file(archive_path, "open submitted archive")?;
    let declared_central_entries = declared_central_entry_count(&mut file, archive_path)?;
    std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(0)).map_err(|error| {
        ServiceError::Staging(io_msg(
            "rewind submitted archive after container inspection",
            archive_path,
            &error,
        ))
    })?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| {
        ServiceError::InvalidRequest(format!(
            "submitted archive is not a readable zip file: {error}"
        ))
    })?;
    // The high-level reader indexes entries by name and silently keeps one
    // representative of a duplicated name. A shadowed entry that validation
    // cannot see is exactly the archive ambiguity this contract rejects, so
    // the reader's entry count must equal the container's declared count.
    if u64::try_from(archive.len()).ok() != Some(declared_central_entries) {
        return Err(ServiceError::InvalidRequest(format!(
            "archive central directory declares {declared_central_entries} entries but only {} are distinctly readable; duplicate or shadowed entry names are rejected",
            archive.len()
        )));
    }

    let mut plan = Vec::new();
    let mut kinds: BTreeMap<String, ArchiveEntryKind> = BTreeMap::new();
    let mut declared_regular_files = 0u64;
    let mut declared_regular_file_bytes = 0u64;

    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(|error| {
            ServiceError::InvalidRequest(format!(
                "submitted archive entry {index} is unreadable: {error}"
            ))
        })?;
        let (name, has_directory_suffix) =
            canonical_archive_entry_name(entry.name_raw(), index)?;
        let declared_bytes = entry.size();
        let mode = entry.unix_mode();
        let kind = archive_entry_kind(&name, has_directory_suffix, mode, index)?;
        match kind {
            ArchiveEntryKind::Directory => {
                if declared_bytes != 0 {
                    return Err(ServiceError::InvalidRequest(format!(
                        "archive directory entry {name:?} declares {declared_bytes} content bytes"
                    )));
                }
            }
            ArchiveEntryKind::SymbolicLink => {
                if declared_bytes == 0 || declared_bytes > MAX_TARGET_BYTES as u64 {
                    return Err(ServiceError::InvalidRequest(format!(
                        "archive symbolic-link entry {name:?} declares a {declared_bytes}-byte target; the accepted range is 1..={MAX_TARGET_BYTES}"
                    )));
                }
            }
            ArchiveEntryKind::RegularFile => {
                declared_regular_files = declared_regular_files.checked_add(1).ok_or_else(|| {
                    ServiceError::InvalidRequest(
                        "archive regular-file counter overflowed u64".into(),
                    )
                })?;
                declared_regular_file_bytes = declared_regular_file_bytes
                    .checked_add(declared_bytes)
                    .ok_or_else(|| {
                        ServiceError::InvalidRequest(
                            "archive declared-byte counter overflowed u64".into(),
                        )
                    })?;
            }
        }
        let executable = mode.is_some_and(|mode| mode & 0o111 != 0);
        if kinds.insert(name.clone(), kind).is_some() {
            return Err(ServiceError::InvalidRequest(format!(
                "archive contains duplicate entry {name:?} after directory-slash normalisation"
            )));
        }
        plan.push(ArchiveEntryPlan {
            name,
            kind,
            declared_bytes,
            executable,
        });
    }

    let mut implied_directories: BTreeSet<String> = BTreeSet::new();
    for name in kinds.keys() {
        let components: Vec<&str> = name.split('/').collect();
        let mut ancestor = String::new();
        for component in &components[..components.len() - 1] {
            if !ancestor.is_empty() {
                ancestor.push('/');
            }
            ancestor.push_str(component);
            match kinds.get(&ancestor) {
                Some(ArchiveEntryKind::Directory) => {}
                Some(conflicting) => {
                    return Err(ServiceError::InvalidRequest(format!(
                        "archive entry {ancestor:?} is a {conflicting:?} but is used as a parent directory of {name:?}"
                    )));
                }
                None => {
                    implied_directories.insert(ancestor.clone());
                }
            }
        }
    }

    let declared_entries = u64::try_from(kinds.len())
        .ok()
        .and_then(|explicit| {
            u64::try_from(implied_directories.len())
                .ok()
                .and_then(|implied| explicit.checked_add(implied))
        })
        .ok_or_else(|| {
            ServiceError::InvalidRequest("archive entry counter overflowed u64".into())
        })?;
    require_staging_caps(
        declared_regular_file_bytes,
        declared_entries,
        declared_regular_files,
    )?;
    Ok((
        plan,
        ArchiveSummary {
            declared_entries,
            declared_regular_files,
            declared_regular_file_bytes,
        },
    ))
}

/// Total central-directory record count declared by the container's
/// end-of-central-directory record, Zip64-aware, read directly from the raw
/// bytes. The high-level reader deduplicates identical entry names, so this
/// independent count is the evidence that no entry was shadowed out of
/// validation's sight. The record must terminate the file exactly; trailing
/// bytes after the declared comment are rejected rather than skipped.
fn declared_central_entry_count(file: &mut File, archive_path: &Path) -> ServiceResult<u64> {
    use std::io::{Seek, SeekFrom};
    const EOCD_BYTES: u64 = 22;
    const EOCD64_LOCATOR_BYTES: u64 = 20;
    const MAX_COMMENT_BYTES: u64 = u16::MAX as u64;

    let length = file.seek(SeekFrom::End(0)).map_err(|error| {
        ServiceError::Staging(io_msg("measure submitted archive", archive_path, &error))
    })?;
    if length < EOCD_BYTES {
        return Err(ServiceError::InvalidRequest(format!(
            "submitted archive is {length} bytes, shorter than a minimal zip container"
        )));
    }
    let tail_length = length.min(EOCD_BYTES + MAX_COMMENT_BYTES + EOCD64_LOCATOR_BYTES);
    file.seek(SeekFrom::Start(length - tail_length)).map_err(|error| {
        ServiceError::Staging(io_msg(
            "seek submitted archive container tail",
            archive_path,
            &error,
        ))
    })?;
    let mut tail = vec![0u8; usize::try_from(tail_length).expect("bounded tail length")];
    file.read_exact(&mut tail).map_err(|error| {
        ServiceError::Staging(io_msg(
            "read submitted archive container tail",
            archive_path,
            &error,
        ))
    })?;

    let read_u16 = |offset: usize| u16::from_le_bytes([tail[offset], tail[offset + 1]]);
    let mut eocd_offset = None;
    for start in (0..=tail.len() - usize::try_from(EOCD_BYTES).expect("static size")).rev() {
        if &tail[start..start + 4] == b"PK\x05\x06" {
            let comment_bytes = usize::from(read_u16(start + 20));
            if start + 22 + comment_bytes == tail.len() {
                eocd_offset = Some(start);
                break;
            }
        }
    }
    let eocd = eocd_offset.ok_or_else(|| {
        ServiceError::InvalidRequest(
            "submitted archive has no end-of-central-directory record terminating the file"
                .into(),
        )
    })?;
    let disk_number = read_u16(eocd + 4);
    let directory_disk = read_u16(eocd + 6);
    let disk_entries = read_u16(eocd + 8);
    let total_entries = read_u16(eocd + 10);
    let needs_zip64 = disk_number == u16::MAX
        || directory_disk == u16::MAX
        || disk_entries == u16::MAX
        || total_entries == u16::MAX;
    if !needs_zip64 {
        if disk_number != 0 || directory_disk != 0 || disk_entries != total_entries {
            return Err(ServiceError::InvalidRequest(format!(
                "submitted archive is not a single-part zip: disk={disk_number} directory_disk={directory_disk} disk_entries={disk_entries} total_entries={total_entries}"
            )));
        }
        return Ok(u64::from(total_entries));
    }

    let locator_start = eocd
        .checked_sub(usize::try_from(EOCD64_LOCATOR_BYTES).expect("static size"))
        .ok_or_else(|| {
            ServiceError::InvalidRequest(
                "submitted archive declares Zip64 fields without a Zip64 locator".into(),
            )
        })?;
    if &tail[locator_start..locator_start + 4] != b"PK\x06\x07" {
        return Err(ServiceError::InvalidRequest(
            "submitted archive declares Zip64 fields without a Zip64 locator".into(),
        ));
    }
    let eocd64_position = u64::from_le_bytes(
        tail[locator_start + 8..locator_start + 16]
            .try_into()
            .expect("static slice length"),
    );
    let mut eocd64 = [0u8; 56];
    file.seek(SeekFrom::Start(eocd64_position)).map_err(|error| {
        ServiceError::Staging(io_msg(
            "seek submitted archive Zip64 directory record",
            archive_path,
            &error,
        ))
    })?;
    file.read_exact(&mut eocd64).map_err(|error| {
        ServiceError::InvalidRequest(format!(
            "submitted archive Zip64 end-of-central-directory record is unreadable: {}",
            io_msg("read Zip64 record", archive_path, &error)
        ))
    })?;
    if &eocd64[0..4] != b"PK\x06\x06" {
        return Err(ServiceError::InvalidRequest(
            "submitted archive Zip64 locator does not point at a Zip64 end-of-central-directory record"
                .into(),
        ));
    }
    let zip64_disk = u32::from_le_bytes(eocd64[16..20].try_into().expect("static slice length"));
    let zip64_directory_disk =
        u32::from_le_bytes(eocd64[20..24].try_into().expect("static slice length"));
    let zip64_disk_entries =
        u64::from_le_bytes(eocd64[24..32].try_into().expect("static slice length"));
    let zip64_total_entries =
        u64::from_le_bytes(eocd64[32..40].try_into().expect("static slice length"));
    if zip64_disk != 0 || zip64_directory_disk != 0 || zip64_disk_entries != zip64_total_entries
    {
        return Err(ServiceError::InvalidRequest(format!(
            "submitted archive is not a single-part Zip64 zip: disk={zip64_disk} directory_disk={zip64_directory_disk} disk_entries={zip64_disk_entries} total_entries={zip64_total_entries}"
        )));
    }
    Ok(zip64_total_entries)
}

/// Extraction of the one submitted outermost archive into the staging tree,
/// with the same normalisation, accounting, cancellation, and progress
/// contract as descriptor-anchored folder staging: directories become 0755,
/// regular files become 0644 or 0755 by the recorded any-executable bit,
/// symbolic-link entries become symbolic links whose target bytes are staged
/// opaquely and never resolved, and the token is checked between every entry
/// and every one-MiB decompressed chunk. Every decompressed byte is counted
/// against both the entry's declared size and the global staging caps while
/// it streams, so a lying size header fails closed instead of expanding
/// without bound.
pub fn extract_archive_into_staged_cancellable<F>(
    archive_path: &Path,
    to: &Path,
    cancel: &CancellationToken,
    mut on_progress: F,
) -> ServiceResult<StagingProgress>
where
    F: FnMut(StagingProgress) -> ServiceResult<()>,
{
    let (plan, _summary) = validate_archive_structure(archive_path)?;
    let file = open_source_file(archive_path, "reopen submitted archive for extraction")?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| {
        ServiceError::Staging(format!(
            "submitted archive stopped being a readable zip file between validation and extraction: {error}"
        ))
    })?;
    if archive.len() != plan.len() {
        return Err(ServiceError::Staging(format!(
            "submitted archive changed between validation and extraction: {} validated entries, {} present",
            plan.len(),
            archive.len()
        )));
    }

    let mut created_directories: BTreeSet<String> = BTreeSet::new();
    let mut copied_bytes = 0u64;
    let mut copied_entries = 0u64;
    let mut copied_regular_files = 0u64;

    for (index, planned) in plan.iter().enumerate() {
        let logical_path = Path::new(&planned.name);
        require_not_cancelled(cancel, logical_path)?;
        let mut entry = archive.by_index(index).map_err(|error| {
            ServiceError::Staging(format!(
                "submitted archive entry {index} became unreadable between validation and extraction: {error}"
            ))
        })?;
        let (observed_name, _) = canonical_archive_entry_name(entry.name_raw(), index)?;
        if observed_name != planned.name || entry.size() != planned.declared_bytes {
            return Err(ServiceError::Staging(format!(
                "submitted archive entry {index} changed between validation and extraction: validated {:?} ({} bytes), observed {observed_name:?} ({} bytes)",
                planned.name,
                planned.declared_bytes,
                entry.size()
            )));
        }
        ensure_extracted_parents(
            to,
            &planned.name,
            &mut created_directories,
            &mut copied_entries,
            copied_bytes,
            copied_regular_files,
            &mut on_progress,
        )?;
        let destination = to.join(&planned.name);
        match planned.kind {
            ArchiveEntryKind::Directory => {
                if created_directories.insert(planned.name.clone()) {
                    create_exact_directory(&destination, 0o755)?;
                    copied_entries = copied_entries.checked_add(1).ok_or_else(|| {
                        ServiceError::Staging("staged entry counter overflowed u64".into())
                    })?;
                    require_staging_caps(copied_bytes, copied_entries, copied_regular_files)?;
                    report_progress(
                        &mut on_progress,
                        copied_bytes,
                        copied_entries,
                        copied_regular_files,
                    )?;
                }
            }
            ArchiveEntryKind::SymbolicLink => {
                let mut target_bytes = Vec::new();
                std::io::Read::take(&mut entry, planned.declared_bytes + 1)
                    .read_to_end(&mut target_bytes)
                    .map_err(|error| {
                        ServiceError::InvalidRequest(format!(
                            "read submitted archive symbolic-link entry {}: {error}",
                            logical_path.display()
                        ))
                    })?;
                if target_bytes.len() as u64 != planned.declared_bytes {
                    return Err(ServiceError::InvalidRequest(format!(
                        "archive symbolic-link entry {} produced {} target bytes but declared {}",
                        logical_path.display(),
                        target_bytes.len(),
                        planned.declared_bytes
                    )));
                }
                if target_bytes.contains(&0) {
                    return Err(ServiceError::InvalidRequest(format!(
                        "archive symbolic-link entry {} target contains a NUL byte",
                        logical_path.display()
                    )));
                }
                let target = PathBuf::from(OsString::from_vec(target_bytes));
                std::os::unix::fs::symlink(&target, &destination).map_err(|error| {
                    ServiceError::Staging(io_msg(
                        "create staged symbolic link",
                        &destination,
                        &error,
                    ))
                })?;
                let staged_meta = std::fs::symlink_metadata(&destination).map_err(|error| {
                    ServiceError::Staging(io_msg(
                        "stat staged symbolic link",
                        &destination,
                        &error,
                    ))
                })?;
                if !staged_meta.file_type().is_symlink() {
                    return Err(ServiceError::Staging(format!(
                        "staged symbolic-link publication changed type at {}",
                        destination.display()
                    )));
                }
                let staged_target = std::fs::read_link(&destination).map_err(|error| {
                    ServiceError::Staging(io_msg(
                        "read staged symbolic link",
                        &destination,
                        &error,
                    ))
                })?;
                if staged_target.as_os_str() != target.as_os_str() {
                    return Err(ServiceError::Staging(format!(
                        "staged symbolic-link target mismatch at {}: expected {:?}, observed {:?}",
                        destination.display(),
                        target,
                        staged_target
                    )));
                }
                copied_entries = copied_entries.checked_add(1).ok_or_else(|| {
                    ServiceError::Staging("staged entry counter overflowed u64".into())
                })?;
                require_staging_caps(copied_bytes, copied_entries, copied_regular_files)?;
                report_progress(
                    &mut on_progress,
                    copied_bytes,
                    copied_entries,
                    copied_regular_files,
                )?;
            }
            ArchiveEntryKind::RegularFile => {
                copied_entries = copied_entries.checked_add(1).ok_or_else(|| {
                    ServiceError::Staging("staged entry counter overflowed u64".into())
                })?;
                copied_regular_files = copied_regular_files.checked_add(1).ok_or_else(|| {
                    ServiceError::Staging("staged regular-file counter overflowed u64".into())
                })?;
                let projected_bytes = copied_bytes
                    .checked_add(planned.declared_bytes)
                    .ok_or_else(|| {
                        ServiceError::Staging(format!(
                            "staged byte counter would overflow u64 before extracting {}",
                            logical_path.display()
                        ))
                    })?;
                require_staging_caps(projected_bytes, copied_entries, copied_regular_files)?;
                let mut output = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&destination)
                    .map_err(|error| {
                        ServiceError::Staging(io_msg("create staged file", &destination, &error))
                    })?;
                let mut file_bytes = 0u64;
                let mut buffer = vec![0u8; 1024 * 1024];
                loop {
                    require_not_cancelled(cancel, logical_path)?;
                    let read = entry.read(&mut buffer).map_err(|error| {
                        ServiceError::InvalidRequest(format!(
                            "read submitted archive entry {} while extracting to {}: {error}",
                            logical_path.display(),
                            destination.display()
                        ))
                    })?;
                    if read == 0 {
                        break;
                    }
                    file_bytes = file_bytes
                        .checked_add(u64::try_from(read).map_err(|_| {
                            ServiceError::Staging(format!(
                                "read size does not fit u64 while extracting {}",
                                logical_path.display()
                            ))
                        })?)
                        .ok_or_else(|| {
                            ServiceError::Staging(format!(
                                "per-file byte counter overflowed while extracting {}",
                                logical_path.display()
                            ))
                        })?;
                    if file_bytes > planned.declared_bytes {
                        return Err(ServiceError::InvalidRequest(format!(
                            "archive entry {} produced more than its declared {} bytes",
                            logical_path.display(),
                            planned.declared_bytes
                        )));
                    }
                    output.write_all(&buffer[..read]).map_err(|error| {
                        ServiceError::Staging(format!(
                            "write staged file {} from submitted archive entry {}: {error}",
                            destination.display(),
                            logical_path.display()
                        ))
                    })?;
                    let observed_bytes = copied_bytes.checked_add(file_bytes).ok_or_else(|| {
                        ServiceError::Staging("staged byte counter overflowed u64".into())
                    })?;
                    require_staging_caps(observed_bytes, copied_entries, copied_regular_files)?;
                    report_progress(
                        &mut on_progress,
                        observed_bytes,
                        copied_entries,
                        copied_regular_files,
                    )?;
                }
                output.sync_all().map_err(|error| {
                    ServiceError::Staging(io_msg("sync staged file", &destination, &error))
                })?;
                if file_bytes != planned.declared_bytes {
                    return Err(ServiceError::InvalidRequest(format!(
                        "archive entry {} produced {file_bytes} bytes but declared {}",
                        logical_path.display(),
                        planned.declared_bytes
                    )));
                }
                let target_mode = if planned.executable { 0o755 } else { 0o644 };
                std::fs::set_permissions(
                    &destination,
                    std::fs::Permissions::from_mode(target_mode),
                )
                .map_err(|error| {
                    ServiceError::Staging(io_msg(
                        "normalize staged file mode",
                        &destination,
                        &error,
                    ))
                })?;
                copied_bytes = copied_bytes.checked_add(file_bytes).ok_or_else(|| {
                    ServiceError::Staging("staged byte counter overflowed u64".into())
                })?;
                require_staging_caps(copied_bytes, copied_entries, copied_regular_files)?;
                report_progress(
                    &mut on_progress,
                    copied_bytes,
                    copied_entries,
                    copied_regular_files,
                )?;
            }
        }
    }
    require_not_cancelled(cancel, Path::new("archive extraction completion"))?;
    let progress = StagingProgress {
        copied_bytes,
        copied_entries,
        copied_regular_files,
    };
    on_progress(progress)?;
    Ok(progress)
}

fn ensure_extracted_parents<F>(
    to: &Path,
    name: &str,
    created_directories: &mut BTreeSet<String>,
    copied_entries: &mut u64,
    copied_bytes: u64,
    copied_regular_files: u64,
    on_progress: &mut F,
) -> ServiceResult<()>
where
    F: FnMut(StagingProgress) -> ServiceResult<()>,
{
    let components: Vec<&str> = name.split('/').collect();
    let mut ancestor = String::new();
    for component in &components[..components.len() - 1] {
        if !ancestor.is_empty() {
            ancestor.push('/');
        }
        ancestor.push_str(component);
        if created_directories.insert(ancestor.clone()) {
            create_exact_directory(&to.join(&ancestor), 0o755)?;
            *copied_entries = copied_entries.checked_add(1).ok_or_else(|| {
                ServiceError::Staging("staged entry counter overflowed u64".into())
            })?;
            require_staging_caps(copied_bytes, *copied_entries, copied_regular_files)?;
            report_progress(
                on_progress,
                copied_bytes,
                *copied_entries,
                copied_regular_files,
            )?;
        }
    }
    Ok(())
}

fn canonical_archive_entry_name(raw: &[u8], index: usize) -> ServiceResult<(String, bool)> {
    if raw.is_empty() {
        return Err(ServiceError::InvalidRequest(format!(
            "archive entry {index} has an empty name"
        )));
    }
    if raw.contains(&0) {
        return Err(ServiceError::InvalidRequest(format!(
            "archive entry {index} name contains a NUL byte"
        )));
    }
    let text = std::str::from_utf8(raw).map_err(|_| {
        ServiceError::InvalidRequest(format!(
            "archive entry {index} name is not valid UTF-8"
        ))
    })?;
    if text.starts_with('/') {
        return Err(ServiceError::InvalidRequest(format!(
            "archive entry name is absolute: {text:?}"
        )));
    }
    let (name, has_directory_suffix) = match text.strip_suffix('/') {
        Some(stripped) => (stripped, true),
        None => (text, false),
    };
    if name.is_empty() {
        return Err(ServiceError::InvalidRequest(format!(
            "archive entry {index} name consists only of separators"
        )));
    }
    for component in name.split('/') {
        if component.is_empty() {
            return Err(ServiceError::InvalidRequest(format!(
                "archive entry name contains an empty component: {text:?}"
            )));
        }
        if component == "." || component == ".." {
            return Err(ServiceError::InvalidRequest(format!(
                "archive entry name contains a traversal component: {text:?}"
            )));
        }
    }
    Ok((name.to_string(), has_directory_suffix))
}

fn archive_entry_kind(
    name: &str,
    has_directory_suffix: bool,
    unix_mode: Option<u32>,
    index: usize,
) -> ServiceResult<ArchiveEntryKind> {
    let format_bits = unix_mode.map(|mode| mode & (libc::S_IFMT as u32));
    if has_directory_suffix {
        return match format_bits {
            None | Some(0) => Ok(ArchiveEntryKind::Directory),
            Some(bits) if bits == libc::S_IFDIR as u32 => Ok(ArchiveEntryKind::Directory),
            Some(bits) => Err(ServiceError::InvalidRequest(format!(
                "archive entry {name:?} has a directory name but recorded mode type {bits:o}"
            ))),
        };
    }
    match format_bits {
        Some(bits) if bits == libc::S_IFLNK as u32 => Ok(ArchiveEntryKind::SymbolicLink),
        None | Some(0) => Ok(ArchiveEntryKind::RegularFile),
        Some(bits) if bits == libc::S_IFREG as u32 => Ok(ArchiveEntryKind::RegularFile),
        Some(bits) => Err(ServiceError::InvalidRequest(format!(
            "unsupported archive entry type at {name:?} (index {index}, recorded mode type {bits:o})"
        ))),
    }
}

fn report_progress<F>(
    on_progress: &mut F,
    copied_bytes: u64,
    copied_entries: u64,
    copied_regular_files: u64,
) -> ServiceResult<()>
where
    F: FnMut(StagingProgress) -> ServiceResult<()>,
{
    on_progress(StagingProgress {
        copied_bytes,
        copied_entries,
        copied_regular_files,
    })
}

fn require_not_cancelled(cancel: &CancellationToken, path: &Path) -> ServiceResult<()> {
    if cancel.is_cancelled() {
        return Err(ServiceError::Internal(format!(
            "session cancellation interrupted descriptor-anchored staging at {}",
            path.display()
        )));
    }
    Ok(())
}

fn require_staging_caps(
    copied_bytes: u64,
    copied_entries: u64,
    copied_regular_files: u64,
) -> ServiceResult<()> {
    if copied_entries > MAX_STAGED_ENTRIES
        || copied_regular_files > MAX_STAGED_FILES
        || copied_bytes > MAX_STAGED_BYTES
    {
        return Err(ServiceError::InvalidRequest(format!(
            "staged content exceeded a staging cap: entries={copied_entries} (max={MAX_STAGED_ENTRIES}), regular_files={copied_regular_files} (max={MAX_STAGED_FILES}), regular_file_bytes={copied_bytes} (max={MAX_STAGED_BYTES})"
        )));
    }
    Ok(())
}

fn open_source_file(path: &Path, operation: &str) -> ServiceResult<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| source_access_error(operation, path, error))
}

/// Maximum accepted symbolic-link target length, shared by descriptor-anchored
/// folder staging and archive extraction.
pub(crate) const MAX_TARGET_BYTES: usize = 1024 * 1024;

fn source_access_error(operation: &str, path: &Path, error: std::io::Error) -> ServiceError {
    let message = io_msg(operation, path, &error);
    match error.raw_os_error() {
        // The source existed but the submitted tree is not readable by the
        // fixed service identity. This is a property the caller must correct.
        Some(libc::EACCES) | Some(libc::EPERM) => ServiceError::InvalidRequest(message),
        // A descriptor-anchored entry disappeared or changed type between
        // enumeration and no-follow open. It is safe to retry only once the
        // source becomes quiescent.
        Some(libc::ENOENT) | Some(libc::ENOTDIR) | Some(libc::ELOOP) | Some(libc::ESTALE) => {
            ServiceError::SourceChanged(message)
        }
        _ => ServiceError::Staging(message),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};

    use super::{require_staging_caps, SessionPaths};
    use crate::config::{
        DEFAULT_MAX_SESSION_TURNS, MAX_SESSION_TURNS_CEILING, MAX_STAGED_BYTES,
        MAX_STAGED_ENTRIES, MAX_STAGED_FILES,
    };
    use crate::error::ServiceError;

    #[test]
    fn start_gate_exists_and_is_locked_before_container_creation() {
        let root = std::env::temp_dir().join(format!("qwen38-start-gate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("sessions")).expect("create fixed sessions parent");
        let paths = SessionPaths::new(&root, "s-0123456789abcdef0123456789abcdef");
        paths.create_dirs().expect("create fixture layout");
        let gate = paths
            .create_locked_start_gate()
            .expect("create and lock start gate");
        let gate_path = paths.control.join("start-gate.lock");
        let metadata = std::fs::symlink_metadata(&gate_path).expect("stat gate");
        assert!(metadata.is_file());
        let control_metadata = std::fs::symlink_metadata(&paths.control).expect("stat control");
        assert_eq!(metadata.uid(), control_metadata.uid());
        assert_eq!(metadata.gid(), control_metadata.gid());
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        let second = std::fs::OpenOptions::new()
            .read(true)
            .open(&gate_path)
            .expect("open second gate descriptor");
        assert!(std::fs::File::try_lock(&second).is_err());
        std::fs::File::unlock(&gate).expect("release gate");
        std::fs::File::try_lock(&second).expect("second descriptor acquires after release");
        std::fs::File::unlock(&second).expect("release second descriptor");
        drop(gate);
        std::fs::remove_dir_all(&root).expect("remove fixture");
    }

    #[test]
    fn turn_budget_records_are_canonical_no_clobber_byte_data() {
        for turns in [1u32, DEFAULT_MAX_SESSION_TURNS, MAX_SESSION_TURNS_CEILING] {
            let root = std::env::temp_dir().join(format!(
                "qwen38-turn-budget-{turns}-{}",
                uuid::Uuid::new_v4().simple()
            ));
            std::fs::create_dir_all(root.join("sessions"))
                .expect("create fixed sessions parent");
            let paths = SessionPaths::new(&root, "s-22222222222222222222222222222222");
            paths.create_dirs().expect("create fixture layout");
            let path = paths
                .write_turn_budget(turns)
                .expect("publish canonical turn budget");
            let written = std::fs::read(&path).expect("read turn budget");
            assert_eq!(
                written,
                format!("{{\"max_session_turns\":{turns}}}\n").as_bytes()
            );
            assert_eq!(super::parse_turn_budget_record(&written), Some(turns));
            assert_eq!(
                std::fs::metadata(&path)
                    .expect("stat turn budget")
                    .permissions()
                    .mode()
                    & 0o777,
                0o444
            );
            assert!(
                paths.write_turn_budget(turns).is_err(),
                "turn-budget publication silently overwrote an existing record"
            );
            std::fs::remove_dir_all(&root).expect("remove fixture");
        }

        // The launcher inside the agent container compares these exact bytes,
        // so every near-miss spelling and every unrunnable budget must read
        // back as "no budget", never as a repaired one.
        for malformed in [
            "{\"max_session_turns\":400}".as_bytes(),
            "{\"max_session_turns\": 400}\n".as_bytes(),
            "{\"max_session_turns\":0400}\n".as_bytes(),
            "{\"max_session_turns\":+400}\n".as_bytes(),
            "{\"max_session_turns\":400}\n\n".as_bytes(),
            "{\"max_session_turns\":400,\"unrelated_field\":false}\n".as_bytes(),
            "{\"max_session_turns\":\"400\"}\n".as_bytes(),
            "{\"unrelated_record\":false}\n".as_bytes(),
            "{\"max_session_turns\":0}\n".as_bytes(),
            "{\"max_session_turns\":-1}\n".as_bytes(),
            format!("{{\"max_session_turns\":{}}}\n", MAX_SESSION_TURNS_CEILING + 1).as_bytes(),
            format!("{{\"max_session_turns\":{}}}\n", u64::from(u32::MAX) + 1).as_bytes(),
            "".as_bytes(),
        ] {
            assert_eq!(
                super::parse_turn_budget_record(malformed),
                None,
                "a non-canonical turn-budget record was accepted: {:?}",
                String::from_utf8_lossy(malformed)
            );
        }
    }

    #[test]
    fn session_layout_is_exclusive_and_never_adopts_a_stale_or_symlinked_tree() {
        let root = std::env::temp_dir().join(format!(
            "qwen38-exclusive-session-layout-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(root.join("sessions")).expect("create fixed sessions parent");
        let paths = SessionPaths::new(&root, "s-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        paths.create_dirs().expect("create first exact layout");
        std::fs::write(paths.root.join("stale-evidence"), b"preserve")
            .expect("write stale-tree sentinel");
        assert!(
            paths.create_dirs().is_err(),
            "a second constructor adopted an existing session tree"
        );
        assert_eq!(
            std::fs::read(paths.root.join("stale-evidence")).expect("read preserved sentinel"),
            b"preserve"
        );

        std::fs::remove_dir_all(&paths.root).expect("remove first exact layout");
        let outside = root.with_extension("outside");
        std::fs::create_dir(&outside).expect("create outside fixture");
        std::fs::write(outside.join("sentinel"), b"outside-preserved")
            .expect("write outside sentinel");
        symlink(&outside, &paths.root).expect("create hostile session-root symlink");
        assert!(
            paths.create_dirs().is_err(),
            "session constructor followed a pre-existing root symlink"
        );
        assert_eq!(
            std::fs::read(outside.join("sentinel")).expect("read outside sentinel"),
            b"outside-preserved"
        );
        assert!(
            std::fs::symlink_metadata(&paths.root)
                .expect("stat preserved hostile symlink")
                .file_type()
                .is_symlink(),
            "collision handling removed or replaced the hostile path"
        );

        std::fs::remove_file(&paths.root).expect("remove hostile session-root symlink");
        std::fs::remove_dir_all(&outside).expect("remove outside fixture");
        std::fs::remove_dir_all(&root).expect("remove exclusive-layout fixture");
    }

    #[test]
    fn panic_recovery_reuses_only_an_exact_existing_layout_without_repair() {
        let root = std::env::temp_dir().join(format!(
            "qwen38-recovery-session-layout-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(root.join("sessions")).expect("create fixed sessions parent");
        let paths = SessionPaths::new(&root, "s-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        paths.create_dirs().expect("create exact recovery layout");
        if unsafe { libc::geteuid() } == 0 {
            for path in [
                &paths.root,
                &paths.staged,
                &paths.control,
                &paths.artifacts,
                &paths.streams,
                &paths.output,
            ] {
                std::os::unix::fs::chown(path, Some(1000), Some(1000))
                    .expect("assign runtime ownership in root-run fixture");
            }
        } else {
            assert_eq!(
                unsafe { libc::geteuid() },
                1000,
                "recovery-layout fixture requires either build-root or runtime uid 1000"
            );
        }
        paths
            .ensure_recovery_dirs()
            .expect("reuse exact in-memory session layout");

        std::fs::set_permissions(&paths.output, std::fs::Permissions::from_mode(0o755))
            .expect("drift output mode");
        assert!(
            paths.ensure_recovery_dirs().is_err(),
            "panic recovery silently repaired and adopted drifted state"
        );
        assert_eq!(
            std::fs::metadata(&paths.output)
                .expect("stat unmodified drifted output")
                .permissions()
                .mode()
                & 0o777,
            0o755
        );

        std::fs::remove_dir_all(&root).expect("remove recovery-layout fixture");
    }

    #[test]
    fn staging_caps_bound_regular_files_bytes_and_all_entry_types_independently() {
        require_staging_caps(MAX_STAGED_BYTES, MAX_STAGED_ENTRIES, MAX_STAGED_FILES)
            .expect("every exact cap value is accepted");
        for (bytes, entries, regular_files, expected_field) in [
            (
                MAX_STAGED_BYTES + 1,
                MAX_STAGED_ENTRIES,
                MAX_STAGED_FILES,
                "regular_file_bytes",
            ),
            (
                MAX_STAGED_BYTES,
                MAX_STAGED_ENTRIES + 1,
                MAX_STAGED_FILES,
                "entries",
            ),
            (
                MAX_STAGED_BYTES,
                MAX_STAGED_ENTRIES,
                MAX_STAGED_FILES + 1,
                "regular_files",
            ),
        ] {
            let error = require_staging_caps(bytes, entries, regular_files)
                .expect_err("one over any independent cap must fail closed");
            assert!(error.to_string().contains(expected_field));
        }
    }

    use super::{extract_archive_into_staged_cancellable, validate_archive_structure};
    use tokio_util::sync::CancellationToken;
    use zip::write::SimpleFileOptions;
    use zip::CompressionMethod;

    fn stored_options() -> SimpleFileOptions {
        SimpleFileOptions::default().compression_method(CompressionMethod::Stored)
    }

    fn build_zip<F>(populate: F) -> Vec<u8>
    where
        F: FnOnce(&mut zip::ZipWriter<std::io::Cursor<Vec<u8>>>),
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        populate(&mut writer);
        writer.finish().expect("finish fixture archive").into_inner()
    }

    fn archive_fixture(root: &std::path::Path, bytes: &[u8]) -> std::path::PathBuf {
        let path = root.join("fixture.zip");
        std::fs::write(&path, bytes).expect("write fixture archive");
        path
    }

    /// Replace every occurrence of a unique needle. Lengths must match so
    /// header offsets stay valid.
    fn patch_all(buffer: &mut [u8], needle: &[u8], replacement: &[u8]) -> usize {
        assert_eq!(needle.len(), replacement.len(), "patch must preserve length");
        let mut patched = 0;
        let mut index = 0;
        while index + needle.len() <= buffer.len() {
            if &buffer[index..index + needle.len()] == needle {
                buffer[index..index + needle.len()].copy_from_slice(replacement);
                patched += 1;
                index += needle.len();
            } else {
                index += 1;
            }
        }
        patched
    }

    fn find_header_offset(buffer: &[u8], signature: &[u8; 4], name: &[u8], fixed: usize) -> usize {
        let mut found = None;
        for index in 0..buffer.len().saturating_sub(fixed + name.len()) {
            if &buffer[index..index + 4] == signature
                && &buffer[index + fixed..index + fixed + name.len()] == name
            {
                assert!(found.is_none(), "fixture entry name is not unique in archive");
                found = Some(index);
            }
        }
        found.expect("fixture header not found")
    }

    fn patch_declared_sizes(buffer: &mut [u8], name: &str, declared: u32, patch_local: bool) {
        let central = find_header_offset(buffer, b"PK\x01\x02", name.as_bytes(), 46);
        buffer[central + 24..central + 28].copy_from_slice(&declared.to_le_bytes());
        if patch_local {
            let local = find_header_offset(buffer, b"PK\x03\x04", name.as_bytes(), 30);
            buffer[local + 22..local + 26].copy_from_slice(&declared.to_le_bytes());
        }
    }

    fn patch_central_unix_mode(buffer: &mut [u8], name: &str, mode: u32) {
        let central = find_header_offset(buffer, b"PK\x01\x02", name.as_bytes(), 46);
        buffer[central + 38..central + 42].copy_from_slice(&(mode << 16).to_le_bytes());
    }

    #[test]
    fn archive_extraction_preserves_content_exec_and_opaque_symlink_targets() {
        let root = std::env::temp_dir().join(format!(
            "qwen38-archive-extract-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let destination = root.join("staged");
        std::fs::create_dir_all(&destination).expect("create destination fixture");

        let inner_archive = build_zip(|writer| {
            writer
                .start_file("hidden.txt", stored_options())
                .expect("start inner entry");
            std::io::Write::write_all(writer, b"never extracted")
                .expect("write inner entry");
        });
        let bytes = build_zip(|writer| {
            writer
                .start_file("run.sh", stored_options().unix_permissions(0o775))
                .expect("start executable");
            std::io::Write::write_all(writer, b"#!/bin/sh\nprintf proof\n")
                .expect("write executable");
            writer
                .add_directory("nested", stored_options().unix_permissions(0o775))
                .expect("add explicit directory");
            writer
                .start_file("nested/data.bin", stored_options().unix_permissions(0o664))
                .expect("start nested data");
            std::io::Write::write_all(writer, b"archive-extract-proof")
                .expect("write nested data");
            writer
                .add_symlink(
                    "dangling",
                    "missing-relative-target",
                    stored_options(),
                )
                .expect("add dangling symlink");
            writer
                .add_symlink(
                    "relative-escape",
                    "../outside-tree",
                    stored_options(),
                )
                .expect("add relative-escape symlink");
            writer
                .add_symlink(
                    "absolute",
                    "/usr/local/bin/python3.13",
                    stored_options(),
                )
                .expect("add absolute symlink");
            writer
                .start_file("inner.zip", stored_options().unix_permissions(0o664))
                .expect("start inner archive entry");
            std::io::Write::write_all(writer, &inner_archive)
                .expect("write inner archive entry");
        });
        let archive_path = archive_fixture(&root, &bytes);

        let (plan, summary) =
            validate_archive_structure(&archive_path).expect("structural validation succeeds");
        assert_eq!(plan.len(), 7);
        assert_eq!(summary.declared_entries, 7);
        assert_eq!(summary.declared_regular_files, 3);
        let expected_bytes = 23 + 21 + inner_archive.len() as u64;
        assert_eq!(summary.declared_regular_file_bytes, expected_bytes);

        let progress = extract_archive_into_staged_cancellable(
            &archive_path,
            &destination,
            &CancellationToken::new(),
            |_| Ok(()),
        )
        .expect("extraction succeeds");
        assert_eq!(progress.copied_bytes, expected_bytes);
        assert_eq!(progress.copied_entries, 7);
        assert_eq!(progress.copied_regular_files, 3);

        assert_eq!(
            std::fs::read(destination.join("nested/data.bin")).expect("read staged data"),
            b"archive-extract-proof"
        );
        assert_eq!(
            std::fs::metadata(destination.join("run.sh"))
                .expect("stat staged executable")
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        assert_eq!(
            std::fs::metadata(destination.join("nested/data.bin"))
                .expect("stat staged plain file")
                .permissions()
                .mode()
                & 0o777,
            0o644
        );
        assert_eq!(
            std::fs::metadata(destination.join("nested"))
                .expect("stat staged directory")
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        for (name, expected) in [
            ("dangling", "missing-relative-target"),
            ("relative-escape", "../outside-tree"),
            ("absolute", "/usr/local/bin/python3.13"),
        ] {
            let staged = destination.join(name);
            assert!(std::fs::symlink_metadata(&staged)
                .expect("stat staged link")
                .file_type()
                .is_symlink());
            assert_eq!(
                std::fs::read_link(&staged).expect("read staged link"),
                std::path::PathBuf::from(expected)
            );
        }
        // The nested archive is an ordinary regular file: outermost-only
        // extraction leaves its contents untouched.
        assert_eq!(
            std::fs::read(destination.join("inner.zip")).expect("read staged inner archive"),
            inner_archive
        );
        assert!(
            std::fs::symlink_metadata(destination.join("hidden.txt")).is_err(),
            "inner archive content leaked into the staged workspace"
        );

        std::fs::remove_dir_all(&root).expect("remove archive-extract fixture");
    }

    #[test]
    fn archive_names_escaping_or_colliding_fail_closed_before_extraction() {
        let hostile: [(&str, Box<dyn Fn(&mut zip::ZipWriter<std::io::Cursor<Vec<u8>>>)>); 6] = [
            (
                "duplicate entry",
                Box::new(|writer| {
                    writer
                        .start_file("samename", stored_options())
                        .expect("start file colliding with directory");
                    writer
                        .add_directory("samename", stored_options())
                        .expect("add directory colliding with file");
                }),
            ),
            (
                "traversal component",
                Box::new(|writer| {
                    writer
                        .start_file("../escape.txt", stored_options())
                        .expect("start traversal entry");
                }),
            ),
            (
                "absolute",
                Box::new(|writer| {
                    writer
                        .start_file("/etc/hostile", stored_options())
                        .expect("start absolute entry");
                }),
            ),
            (
                "empty component",
                Box::new(|writer| {
                    writer
                        .start_file("a//b.txt", stored_options())
                        .expect("start doubled-separator entry");
                }),
            ),
            (
                "traversal component",
                Box::new(|writer| {
                    writer
                        .start_file("./relative.txt", stored_options())
                        .expect("start dot-prefixed entry");
                }),
            ),
            (
                "parent directory",
                Box::new(|writer| {
                    writer
                        .start_file("collide", stored_options())
                        .expect("start colliding file");
                    writer
                        .start_file("collide/child.txt", stored_options())
                        .expect("start child under file");
                }),
            ),
        ];
        for (expected_fragment, populate) in hostile {
            let root = std::env::temp_dir().join(format!(
                "qwen38-archive-hostile-{}",
                uuid::Uuid::new_v4().simple()
            ));
            let destination = root.join("staged");
            std::fs::create_dir_all(&destination).expect("create destination fixture");
            let archive_path = archive_fixture(&root, &build_zip(populate));
            let error = validate_archive_structure(&archive_path)
                .expect_err("hostile archive must fail structural validation");
            assert!(matches!(error, ServiceError::InvalidRequest(_)));
            assert!(
                error.to_string().contains(expected_fragment),
                "error {error} does not name the violated rule {expected_fragment:?}"
            );
            let extraction_error = extract_archive_into_staged_cancellable(
                &archive_path,
                &destination,
                &CancellationToken::new(),
                |_| Ok(()),
            )
            .expect_err("hostile archive must fail extraction");
            assert!(matches!(extraction_error, ServiceError::InvalidRequest(_)));
            assert_eq!(
                std::fs::read_dir(&destination)
                    .expect("list destination")
                    .count(),
                0,
                "hostile archive created staged entries before rejection"
            );
            std::fs::remove_dir_all(&root).expect("remove hostile fixture");
        }
    }

    #[test]
    fn archive_symlink_target_and_entry_type_rules_fail_closed() {
        let root = std::env::temp_dir().join(format!(
            "qwen38-archive-types-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&root).expect("create type fixture root");

        let empty_target = build_zip(|writer| {
            writer
                .add_symlink("empty-target", "", stored_options())
                .expect("add empty-target symlink");
        });
        let error = validate_archive_structure(&archive_fixture(&root, &empty_target))
            .expect_err("empty symlink target must fail");
        assert!(error.to_string().contains("symbolic-link"));

        let mut fifo = build_zip(|writer| {
            writer
                .start_file("special.fifo", stored_options())
                .expect("start special entry");
        });
        patch_central_unix_mode(&mut fifo, "special.fifo", 0o010644);
        let error = validate_archive_structure(&archive_fixture(&root, &fifo))
            .expect_err("special entry type must fail");
        assert!(matches!(error, ServiceError::InvalidRequest(_)));
        assert!(error.to_string().contains("unsupported archive entry type"));

        std::fs::remove_dir_all(&root).expect("remove type fixture");
    }

    #[test]
    fn archive_declared_sizes_beyond_staging_caps_are_rejected_structurally() {
        let root = std::env::temp_dir().join(format!(
            "qwen38-archive-declared-cap-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&root).expect("create declared-cap fixture root");
        // Enough near-4-GiB entries that their declared aggregate exceeds the
        // staging byte cap. The count derives from MAX_STAGED_BYTES so the test
        // scales with the cap instead of hard-coding one deployment's size, and
        // stays far below the file/entry caps so only the byte bound trips.
        let entry_count = (crate::config::MAX_STAGED_BYTES / u32::MAX as u64 + 1) as usize;
        let names: Vec<String> =
            (0..entry_count).map(|i| format!("oversized-{i}.bin")).collect();
        let mut bytes = build_zip(|writer| {
            for name in &names {
                writer
                    .start_file(name, stored_options())
                    .expect("start oversized entry");
                std::io::Write::write_all(writer, b"small").expect("write oversized entry");
            }
        });
        // The declared central-directory sizes are the structural authority the
        // caps consult; no content is decompressed to discover the lie. One
        // 32-bit field cannot exceed the byte cap alone, so enough declared
        // near-4-GiB entries prove the aggregate regular-file-bytes bound.
        for name in &names {
            patch_declared_sizes(&mut bytes, name, u32::MAX, false);
        }
        let error = validate_archive_structure(&archive_fixture(&root, &bytes))
            .expect_err("declared over-cap size must fail structurally");
        assert!(matches!(error, ServiceError::InvalidRequest(_)));
        assert!(error.to_string().contains("regular_file_bytes"));
        std::fs::remove_dir_all(&root).expect("remove declared-cap fixture");
    }

    #[test]
    fn archive_duplicate_names_are_rejected_structurally() {
        let root = std::env::temp_dir().join(format!(
            "qwen38-archive-duplicate-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&root).expect("create duplicate fixture root");
        // The writer itself refuses duplicate names, so the duplicate is
        // introduced the way a hostile producer would: by renaming a second
        // entry to the first entry's exact bytes in both headers.
        let mut bytes = build_zip(|writer| {
            for name in ["dup-a.txt", "dup-b.txt"] {
                writer
                    .start_file(name, stored_options())
                    .expect("start duplicate-candidate entry");
                std::io::Write::write_all(writer, b"dup").expect("write duplicate entry");
            }
        });
        assert_eq!(patch_all(&mut bytes, b"dup-b.txt", b"dup-a.txt"), 2);
        let error = validate_archive_structure(&archive_fixture(&root, &bytes))
            .expect_err("duplicate entry names must fail structurally");
        assert!(matches!(error, ServiceError::InvalidRequest(_)));
        assert!(error.to_string().contains("shadowed"));
        std::fs::remove_dir_all(&root).expect("remove duplicate fixture");
    }

    #[test]
    fn archive_entry_streaming_more_than_declared_fails_closed() {
        let root = std::env::temp_dir().join(format!(
            "qwen38-archive-lying-size-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let destination = root.join("staged");
        std::fs::create_dir_all(&destination).expect("create destination fixture");
        let mut bytes = build_zip(|writer| {
            writer
                .start_file("lying.bin", stored_options())
                .expect("start lying entry");
            std::io::Write::write_all(writer, &[0xa5u8; 2048]).expect("write lying entry");
        });
        patch_declared_sizes(&mut bytes, "lying.bin", 1024, true);
        let archive_path = archive_fixture(&root, &bytes);
        let error = extract_archive_into_staged_cancellable(
            &archive_path,
            &destination,
            &CancellationToken::new(),
            |_| Ok(()),
        )
        .expect_err("under-declared entry must fail while streaming");
        assert!(matches!(error, ServiceError::InvalidRequest(_)));
        assert!(error.to_string().contains("declared"));
        std::fs::remove_dir_all(&root).expect("remove lying-size fixture");
    }

    #[test]
    fn archive_content_corruption_is_rejected_by_checksum() {
        let root = std::env::temp_dir().join(format!(
            "qwen38-archive-crc-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let destination = root.join("staged");
        std::fs::create_dir_all(&destination).expect("create destination fixture");
        let mut bytes = build_zip(|writer| {
            writer
                .start_file("payload.bin", stored_options())
                .expect("start payload entry");
            std::io::Write::write_all(writer, b"crc-proof-payload-original")
                .expect("write payload entry");
        });
        assert_eq!(
            patch_all(
                &mut bytes,
                b"crc-proof-payload-original",
                b"crc-proof-payload-originaX",
            ),
            1
        );
        let error = extract_archive_into_staged_cancellable(
            &archive_fixture(&root, &bytes),
            &destination,
            &CancellationToken::new(),
            |_| Ok(()),
        )
        .expect_err("corrupted content must fail closed");
        assert!(matches!(error, ServiceError::InvalidRequest(_)));
        std::fs::remove_dir_all(&root).expect("remove crc fixture");
    }

    #[test]
    fn archive_non_utf8_names_are_rejected() {
        let root = std::env::temp_dir().join(format!(
            "qwen38-archive-nonutf8-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&root).expect("create non-utf8 fixture root");
        let mut bytes = build_zip(|writer| {
            writer
                .start_file("nonutf8-marker.txt", stored_options())
                .expect("start non-utf8 entry");
        });
        assert_eq!(
            patch_all(
                &mut bytes,
                b"nonutf8-marker.txt",
                b"nonutf8-mark\xffr.txt",
            ),
            2,
            "entry name must be patched in both the local and central headers"
        );
        let error = validate_archive_structure(&archive_fixture(&root, &bytes))
            .expect_err("non-UTF-8 entry name must fail");
        assert!(matches!(error, ServiceError::InvalidRequest(_)));
        assert!(error.to_string().contains("UTF-8"));
        std::fs::remove_dir_all(&root).expect("remove non-utf8 fixture");
    }

    #[test]
    fn archive_entries_without_unix_modes_stage_as_plain_data() {
        let root = std::env::temp_dir().join(format!(
            "qwen38-archive-dos-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let destination = root.join("staged");
        std::fs::create_dir_all(&destination).expect("create destination fixture");
        let mut bytes = build_zip(|writer| {
            writer
                .start_file("dos-made.txt", stored_options().unix_permissions(0o777))
                .expect("start dos entry");
            std::io::Write::write_all(writer, b"dos").expect("write dos entry");
        });
        // Zero external attributes: no recorded Unix mode bits at all, the
        // shape a non-Unix producer emits.
        patch_central_unix_mode(&mut bytes, "dos-made.txt", 0);
        extract_archive_into_staged_cancellable(
            &archive_fixture(&root, &bytes),
            &destination,
            &CancellationToken::new(),
            |_| Ok(()),
        )
        .expect("entry without a recorded Unix mode extracts");
        assert_eq!(
            std::fs::metadata(destination.join("dos-made.txt"))
                .expect("stat dos-made file")
                .permissions()
                .mode()
                & 0o777,
            0o644
        );
        std::fs::remove_dir_all(&root).expect("remove dos fixture");
    }

    #[test]
    fn archive_implied_parents_are_created_exactly_and_never_double_counted() {
        for (label, populate) in [
            (
                "implied-only",
                Box::new(|writer: &mut zip::ZipWriter<std::io::Cursor<Vec<u8>>>| {
                    writer
                        .start_file("a/b/c.txt", stored_options())
                        .expect("start deep entry");
                    std::io::Write::write_all(writer, b"deep").expect("write deep entry");
                }) as Box<dyn Fn(&mut zip::ZipWriter<std::io::Cursor<Vec<u8>>>)>,
            ),
            (
                "explicit-after-implied",
                Box::new(|writer: &mut zip::ZipWriter<std::io::Cursor<Vec<u8>>>| {
                    writer
                        .start_file("a/b/c.txt", stored_options())
                        .expect("start deep entry");
                    std::io::Write::write_all(writer, b"deep").expect("write deep entry");
                    writer
                        .add_directory("a", stored_options())
                        .expect("add late explicit parent");
                    writer
                        .add_directory("a/b", stored_options())
                        .expect("add late explicit nested parent");
                }),
            ),
        ] {
            let root = std::env::temp_dir().join(format!(
                "qwen38-archive-parents-{label}-{}",
                uuid::Uuid::new_v4().simple()
            ));
            let destination = root.join("staged");
            std::fs::create_dir_all(&destination).expect("create destination fixture");
            let archive_path = archive_fixture(&root, &build_zip(populate));
            let (_, summary) =
                validate_archive_structure(&archive_path).expect("structural validation");
            assert_eq!(summary.declared_entries, 3, "case {label}");
            let progress = extract_archive_into_staged_cancellable(
                &archive_path,
                &destination,
                &CancellationToken::new(),
                |_| Ok(()),
            )
            .expect("extraction succeeds");
            assert_eq!(progress.copied_entries, 3, "case {label}");
            assert_eq!(progress.copied_regular_files, 1, "case {label}");
            assert_eq!(
                std::fs::metadata(destination.join("a"))
                    .expect("stat implied parent")
                    .permissions()
                    .mode()
                    & 0o777,
                0o755
            );
            assert_eq!(
                std::fs::read(destination.join("a/b/c.txt")).expect("read deep entry"),
                b"deep"
            );
            std::fs::remove_dir_all(&root).expect("remove parents fixture");
        }
    }

    #[test]
    fn zip64_archives_with_many_entries_validate_and_count_exactly() {
        // Real workspaces exceed 65,535 entries, which switches the container
        // to Zip64 end-of-central-directory records; the independent
        // shadowed-entry count cross-check must read that path correctly.
        let root = std::env::temp_dir().join(format!(
            "qwen38-archive-zip64-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&root).expect("create zip64 fixture root");
        const ENTRIES: usize = 70_000;
        let bytes = build_zip(|writer| {
            for index in 0..ENTRIES {
                writer
                    .start_file(format!("d{}/f{index}.txt", index % 97), stored_options())
                    .expect("start zip64 entry");
            }
        });
        let archive_path = archive_fixture(&root, &bytes);
        let (plan, summary) =
            validate_archive_structure(&archive_path).expect("zip64 structural validation");
        assert_eq!(plan.len(), ENTRIES);
        assert_eq!(summary.declared_regular_files, ENTRIES as u64);
        assert_eq!(summary.declared_entries, ENTRIES as u64 + 97);
        std::fs::remove_dir_all(&root).expect("remove zip64 fixture");
    }

    #[test]
    fn archive_extraction_observes_cancellation_and_empty_archives_extract_empty() {
        let root = std::env::temp_dir().join(format!(
            "qwen38-archive-cancel-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let destination = root.join("staged");
        std::fs::create_dir_all(&destination).expect("create destination fixture");
        let bytes = build_zip(|writer| {
            writer
                .start_file("never-created.txt", stored_options())
                .expect("start cancellable entry");
            std::io::Write::write_all(writer, b"cancelled").expect("write cancellable entry");
        });
        let archive_path = archive_fixture(&root, &bytes);
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        let error = extract_archive_into_staged_cancellable(
            &archive_path,
            &destination,
            &cancelled,
            |_| Ok(()),
        )
        .expect_err("cancelled extraction must fail");
        assert!(error.to_string().contains("cancellation"));
        assert_eq!(
            std::fs::read_dir(&destination)
                .expect("list destination")
                .count(),
            0
        );

        let empty_path = root.join("empty.zip");
        std::fs::write(&empty_path, build_zip(|_| {})).expect("write empty archive");
        let progress = extract_archive_into_staged_cancellable(
            &empty_path,
            &destination,
            &CancellationToken::new(),
            |_| Ok(()),
        )
        .expect("empty archive extracts to an empty workspace");
        assert_eq!(progress, super::StagingProgress::default());

        std::fs::remove_dir_all(&root).expect("remove cancel fixture");
    }
}
