//! Per-session host-side filesystem layout, plus a recursive copy of the
//! user-supplied folder into the staging tree.
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
//!   history-policy.json
//!   start-gate.lock
//! streams/        ← capture component rw; Qwen container ro
//!   events.sock
//!   stderr.sock
//! output/         ← bind-mounted only into the trusted capture component
//!   (Qwen never receives this mount.)
//! ```
//!
//! We deliberately copy rather than bind-mounting the user's source folder
//! directly: that gives the agent a workspace it can mutate without affecting
//! the user's working tree. Symbolic links are copied as opaque link-target
//! bytes and are never followed, flattened, rewritten, or used as traversal
//! roots by this service. Resolution happens later inside the agent's isolated
//! mount namespace, where Landlock still governs the resolved write target.

use std::ffi::{CStr, CString, OsString};
use std::fs::{File, Metadata, OpenOptions};
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::config::{MAX_STAGED_BYTES, MAX_STAGED_ENTRIES, MAX_STAGED_FILES};
use crate::error::{io_msg, ServiceError, ServiceResult};

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

    /// Publish the one canonical per-session history-policy record. The API
    /// has already required a JSON boolean; this file is deliberately
    /// canonical byte data so the broker and agent wrapper can independently
    /// prove that the selected immutable Qwen home matches the request.
    pub fn write_history_policy(&self, preserve_thinking: bool) -> ServiceResult<PathBuf> {
        let path = self.control.join("history-policy.json");
        let contents: &[u8] = if preserve_thinking {
            b"{\"preserve_thinking\":true}\n"
        } else {
            b"{\"preserve_thinking\":false}\n"
        };
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|error| {
                ServiceError::Staging(io_msg("open history-policy.json", &path, &error))
            })?;
        file.write_all(contents).map_err(|error| {
            ServiceError::Staging(io_msg("write history-policy.json", &path, &error))
        })?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).map_err(
            |error| {
                ServiceError::Staging(io_msg(
                    "chmod 0444 history-policy.json",
                    &path,
                    &error,
                ))
            },
        )?;
        file.flush().and_then(|_| file.sync_all()).map_err(|error| {
            ServiceError::Staging(io_msg(
                "flush/sync history-policy.json",
                &path,
                &error,
            ))
        })?;
        sync_directory(&self.control, "sync history-policy publication")?;
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

    /// Best-effort recursive removal of the per-session tree. We never error
    /// out on cleanup; we collect what failed for the operator's info.
    pub fn remove_all(&self) -> Vec<String> {
        let mut diags = Vec::new();
        if let Err(e) = std::fs::remove_dir_all(&self.root) {
            diags.push(format!(
                "remove_dir_all({}) failed: {e}",
                self.root.display()
            ));
        }
        diags
    }
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

/// Descriptor-based recursive copy with permission normalisation. Every source
/// object is opened with `O_NOFOLLOW`; its device/inode/type/mode/size/mtime/
/// ctime snapshot is compared before and after use. A symbolic link is opened
/// itself with `O_PATH | O_NOFOLLOW` and read through that descriptor, never
/// through its target. Directory traversal is anchored to the already-open
/// descriptor through `/proc/self/fd`, so a concurrent path replacement cannot
/// redirect the copy outside the requested tree. Any observed mutation is an
/// explicit staging failure. The returned count includes every non-root source
/// entry (directories, regular files, and symbolic links).
pub fn copy_into_staged(source: &File, from: &Path, to: &Path) -> ServiceResult<(u64, u64)> {
    let mut copied_bytes = 0u64;
    let mut copied_entries = 0u64;
    let mut copied_regular_files = 0u64;
    copy_open_directory(
        source,
        from,
        to,
        &mut copied_bytes,
        &mut copied_entries,
        &mut copied_regular_files,
    )?;
    Ok((copied_bytes, copied_entries))
}

fn copy_open_directory(
    source: &File,
    logical_source: &Path,
    to: &Path,
    copied_bytes: &mut u64,
    copied_entries: &mut u64,
    copied_regular_files: &mut u64,
) -> ServiceResult<()> {
    let before = source.metadata().map_err(|error| {
        ServiceError::Staging(io_msg(
            "stat opened source directory",
            logical_source,
            &error,
        ))
    })?;
    if !before.is_dir() {
        return Err(ServiceError::SourceChanged(format!(
            "opened source is not a directory: {}",
            logical_source.display()
        )));
    }
    let descriptor_path = PathBuf::from(format!("/proc/self/fd/{}", source.as_raw_fd()));
    let entries = std::fs::read_dir(&descriptor_path).map_err(|error| {
        source_access_error("read opened source directory", logical_source, error)
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            source_access_error("read opened source directory entry", logical_source, error)
        })?;
        let name = entry.file_name();
        let src_path = entry.path();
        let logical_path = logical_source.join(&name);
        let dst_path = to.join(&name);
        let meta = std::fs::symlink_metadata(&src_path)
            .map_err(|error| source_access_error("stat source entry", &logical_path, error))?;

        *copied_entries = copied_entries
            .checked_add(1)
            .ok_or_else(|| ServiceError::Staging("staged entry counter overflowed u64".into()))?;
        require_staging_caps(*copied_bytes, *copied_entries, *copied_regular_files)?;

        if meta.file_type().is_symlink() {
            let link = open_source_symlink(&src_path, "open source symbolic link")?;
            let opened_meta = link.metadata().map_err(|error| {
                ServiceError::Staging(io_msg(
                    "stat opened source symbolic link",
                    &logical_path,
                    &error,
                ))
            })?;
            require_same_snapshot(
                &meta,
                &opened_meta,
                &logical_path,
                "symbolic link changed between directory-entry inspection and no-follow open",
            )?;
            if !opened_meta.file_type().is_symlink() {
                return Err(ServiceError::SourceChanged(format!(
                    "source entry stopped being a symbolic link during staging: {}",
                    logical_path.display()
                )));
            }
            let target = read_open_symlink_target(&link, &logical_path)?;
            require_same_snapshot(
                &opened_meta,
                &link.metadata().map_err(|error| {
                    ServiceError::Staging(io_msg(
                        "restat opened source symbolic link after reading target",
                        &logical_path,
                        &error,
                    ))
                })?,
                &logical_path,
                "source symbolic link changed while its target was being read",
            )?;
            std::os::unix::fs::symlink(&target, &dst_path).map_err(|error| {
                ServiceError::Staging(io_msg("create staged symbolic link", &dst_path, &error))
            })?;
            let staged_meta = std::fs::symlink_metadata(&dst_path).map_err(|error| {
                ServiceError::Staging(io_msg("stat staged symbolic link", &dst_path, &error))
            })?;
            if !staged_meta.file_type().is_symlink() {
                return Err(ServiceError::Staging(format!(
                    "staged symbolic-link publication changed type at {}",
                    dst_path.display()
                )));
            }
            let staged_target = std::fs::read_link(&dst_path).map_err(|error| {
                ServiceError::Staging(io_msg("read staged symbolic link", &dst_path, &error))
            })?;
            if staged_target.as_os_str() != target.as_os_str() {
                return Err(ServiceError::Staging(format!(
                    "staged symbolic-link target mismatch at {}: expected {:?}, observed {:?}",
                    dst_path.display(),
                    target,
                    staged_target
                )));
            }
        } else if meta.is_dir() {
            let child = open_source_directory(&src_path, "open child source directory")?;
            require_same_snapshot(
                &meta,
                &child.metadata().map_err(|error| {
                    ServiceError::Staging(io_msg(
                        "stat opened child source directory",
                        &logical_path,
                        &error,
                    ))
                })?,
                &logical_path,
                "directory changed between directory-entry inspection and no-follow open",
            )?;
            std::fs::create_dir(&dst_path).map_err(|e| {
                ServiceError::Staging(io_msg("create staged subdir", &dst_path, &e))
            })?;
            std::fs::set_permissions(&dst_path, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| ServiceError::Staging(io_msg("chmod 0755", &dst_path, &e)))?;
            copy_open_directory(
                &child,
                &logical_path,
                &dst_path,
                copied_bytes,
                copied_entries,
                copied_regular_files,
            )?;
        } else if meta.is_file() {
            *copied_regular_files = copied_regular_files.checked_add(1).ok_or_else(|| {
                ServiceError::Staging("staged regular-file counter overflowed u64".into())
            })?;
            require_staging_caps(*copied_bytes, *copied_entries, *copied_regular_files)?;
            let mut input = open_source_file(&src_path, "open source file")?;
            let opened_meta = input.metadata().map_err(|error| {
                ServiceError::Staging(io_msg("stat opened source file", &logical_path, &error))
            })?;
            require_same_snapshot(
                &meta,
                &opened_meta,
                &logical_path,
                "file changed between directory-entry inspection and no-follow open",
            )?;
            let projected_bytes = copied_bytes.checked_add(opened_meta.len()).ok_or_else(|| {
                ServiceError::Staging(format!(
                    "staged byte counter would overflow u64 before copying {}",
                    logical_path.display()
                ))
            })?;
            require_staging_caps(projected_bytes, *copied_entries, *copied_regular_files)?;
            let source_mode = opened_meta.permissions().mode();
            let target_mode = if source_mode & 0o111 != 0 {
                0o755
            } else {
                0o644
            };
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&dst_path)
                .map_err(|error| {
                    ServiceError::Staging(io_msg("create staged file", &dst_path, &error))
                })?;
            let bytes = std::io::copy(&mut input, &mut output).map_err(|error| {
                ServiceError::Staging(format!(
                    "copy opened source {} → {}: {error}",
                    logical_path.display(),
                    dst_path.display()
                ))
            })?;
            output.sync_all().map_err(|error| {
                ServiceError::Staging(io_msg("sync staged file", &dst_path, &error))
            })?;
            if bytes != meta.len() {
                return Err(ServiceError::SourceChanged(format!(
                    "source changed size while staging {}: pre-copy={} bytes copied={bytes}",
                    logical_path.display(),
                    meta.len()
                )));
            }
            require_same_snapshot(
                &opened_meta,
                &input.metadata().map_err(|error| {
                    ServiceError::Staging(io_msg(
                        "restat opened source file after copy",
                        &logical_path,
                        &error,
                    ))
                })?,
                &logical_path,
                "source file changed while it was being copied",
            )?;
            *copied_bytes = copied_bytes.checked_add(bytes).ok_or_else(|| {
                ServiceError::Staging("staged byte counter overflowed u64".into())
            })?;
            require_staging_caps(*copied_bytes, *copied_entries, *copied_regular_files)?;
            std::fs::set_permissions(&dst_path, std::fs::Permissions::from_mode(target_mode))
                .map_err(|e| {
                    ServiceError::Staging(io_msg("normalize staged file mode", &dst_path, &e))
                })?;
        } else {
            return Err(ServiceError::InvalidRequest(format!(
                "unsupported file type at {} (type: {:?})",
                logical_path.display(),
                meta.file_type()
            )));
        }
    }
    require_same_snapshot(
        &before,
        &source.metadata().map_err(|error| {
            ServiceError::Staging(io_msg(
                "restat opened source directory after copy",
                logical_source,
                &error,
            ))
        })?,
        logical_source,
        "source directory changed while it was being traversed",
    )?;
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
            "source exceeded a staging cap while being copied: entries={copied_entries} (max={MAX_STAGED_ENTRIES}), regular_files={copied_regular_files} (max={MAX_STAGED_FILES}), regular_file_bytes={copied_bytes} (max={MAX_STAGED_BYTES})"
        )));
    }
    Ok(())
}

fn open_source_directory(path: &Path, operation: &str) -> ServiceResult<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open(path)
        .map_err(|error| source_access_error(operation, path, error))
}

fn open_source_file(path: &Path, operation: &str) -> ServiceResult<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| source_access_error(operation, path, error))
}

fn open_source_symlink(path: &Path, operation: &str) -> ServiceResult<File> {
    let path_bytes = path.as_os_str().as_bytes();
    let path_c = CString::new(path_bytes).map_err(|_| {
        ServiceError::InvalidRequest(format!(
            "{operation}: source path contains a NUL byte: {}",
            path.display()
        ))
    })?;
    let descriptor = unsafe {
        libc::open(
            path_c.as_ptr(),
            libc::O_PATH | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if descriptor < 0 {
        return Err(source_access_error(
            operation,
            path,
            std::io::Error::last_os_error(),
        ));
    }
    // SAFETY: libc::open returned a new owned descriptor on the success path.
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

fn read_open_symlink_target(link: &File, logical_path: &Path) -> ServiceResult<PathBuf> {
    const MAX_TARGET_BYTES: usize = 1024 * 1024;
    let metadata = link.metadata().map_err(|error| {
        ServiceError::Staging(io_msg(
            "stat opened source symbolic link before reading target",
            logical_path,
            &error,
        ))
    })?;
    let reported = usize::try_from(metadata.len()).map_err(|_| {
        ServiceError::InvalidRequest(format!(
            "symbolic-link target length does not fit usize at {}: {} bytes",
            logical_path.display(),
            metadata.len()
        ))
    })?;
    if reported > MAX_TARGET_BYTES {
        return Err(ServiceError::InvalidRequest(format!(
            "symbolic-link target exceeds the {MAX_TARGET_BYTES}-byte staging limit at {}: {reported} bytes",
            logical_path.display()
        )));
    }

    let mut capacity = reported.saturating_add(1).max(256);
    capacity = capacity.min(MAX_TARGET_BYTES);
    let empty_path = CStr::from_bytes_with_nul(b"\0").expect("static empty C string");
    loop {
        let mut target = vec![0u8; capacity];
        let length = unsafe {
            libc::readlinkat(
                link.as_raw_fd(),
                empty_path.as_ptr(),
                target.as_mut_ptr().cast(),
                target.len(),
            )
        };
        if length < 0 {
            return Err(source_access_error(
                "read opened source symbolic-link target",
                logical_path,
                std::io::Error::last_os_error(),
            ));
        }
        let length = usize::try_from(length).map_err(|_| {
            ServiceError::Staging(format!(
                "readlinkat returned a negative or overflowing length for {}",
                logical_path.display()
            ))
        })?;
        if length < target.len() {
            if length == 0 {
                return Err(ServiceError::SourceChanged(format!(
                    "opened source symbolic link has an empty target at {}",
                    logical_path.display()
                )));
            }
            target.truncate(length);
            return Ok(PathBuf::from(OsString::from_vec(target)));
        }
        if capacity == MAX_TARGET_BYTES {
            return Err(ServiceError::InvalidRequest(format!(
                "symbolic-link target reaches or exceeds the {MAX_TARGET_BYTES}-byte staging limit at {}",
                logical_path.display()
            )));
        }
        capacity = capacity
            .checked_mul(2)
            .unwrap_or(MAX_TARGET_BYTES)
            .min(MAX_TARGET_BYTES);
    }
}

fn require_same_snapshot(
    before: &Metadata,
    after: &Metadata,
    path: &Path,
    context: &str,
) -> ServiceResult<()> {
    if before.dev() == after.dev()
        && before.ino() == after.ino()
        && before.mode() == after.mode()
        && before.len() == after.len()
        && before.mtime() == after.mtime()
        && before.mtime_nsec() == after.mtime_nsec()
        && before.ctime() == after.ctime()
        && before.ctime_nsec() == after.ctime_nsec()
    {
        return Ok(());
    }
    Err(ServiceError::SourceChanged(format!(
        "{context} at {}: before dev:ino={}:{} mode={:o} size={} mtime={}.{} ctime={}.{}; after dev:ino={}:{} mode={:o} size={} mtime={}.{} ctime={}.{}",
        path.display(),
        before.dev(),
        before.ino(),
        before.mode(),
        before.len(),
        before.mtime(),
        before.mtime_nsec(),
        before.ctime(),
        before.ctime_nsec(),
        after.dev(),
        after.ino(),
        after.mode(),
        after.len(),
        after.mtime(),
        after.mtime_nsec(),
        after.ctime(),
        after.ctime_nsec(),
    )))
}

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
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
    use std::os::unix::net::UnixListener;

    use super::{
        copy_into_staged, open_source_directory, open_source_symlink, read_open_symlink_target,
        require_same_snapshot, require_staging_caps, SessionPaths,
    };
    use crate::config::{MAX_STAGED_BYTES, MAX_STAGED_ENTRIES, MAX_STAGED_FILES};
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
    fn history_policy_records_are_canonical_no_clobber_byte_data() {
        for (preserve, expected) in [
            (false, b"{\"preserve_thinking\":false}\n".as_slice()),
            (true, b"{\"preserve_thinking\":true}\n".as_slice()),
        ] {
            let root = std::env::temp_dir().join(format!(
                "qwen38-history-policy-{preserve}-{}",
                uuid::Uuid::new_v4().simple()
            ));
            std::fs::create_dir_all(root.join("sessions"))
                .expect("create fixed sessions parent");
            let paths = SessionPaths::new(
                &root,
                if preserve {
                    "s-11111111111111111111111111111111"
                } else {
                    "s-00000000000000000000000000000000"
                },
            );
            paths.create_dirs().expect("create fixture layout");
            let path = paths
                .write_history_policy(preserve)
                .expect("publish canonical policy");
            assert_eq!(std::fs::read(&path).expect("read policy"), expected);
            assert_eq!(
                std::fs::metadata(&path)
                    .expect("stat policy")
                    .permissions()
                    .mode()
                    & 0o777,
                0o444
            );
            assert!(
                paths.write_history_policy(preserve).is_err(),
                "policy publication silently overwrote an existing record"
            );
            std::fs::remove_dir_all(&root).expect("remove fixture");
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
    fn descriptor_copy_preserves_content_exec_and_opaque_symlink_targets() {
        let root = std::env::temp_dir().join(format!(
            "qwen38-descriptor-copy-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let source = root.join("source");
        let destination = root.join("destination");
        std::fs::create_dir_all(source.join("nested")).expect("create source fixture");
        std::fs::create_dir(&destination).expect("create destination fixture");
        let executable = source.join("run.sh");
        std::fs::write(&executable, b"#!/bin/sh\nprintf proof\n").expect("write executable");
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o775))
            .expect("chmod executable");
        std::fs::write(source.join("nested/data.bin"), b"descriptor-copy-proof")
            .expect("write nested data");

        assert_eq!(
            copy_into_staged(
                &open_source_directory(&source, "open test source").expect("open source"),
                &source,
                &destination,
            )
            .expect("descriptor copy succeeds"),
            (44, 3)
        );
        assert_eq!(
            std::fs::read(destination.join("nested/data.bin")).expect("read staged data"),
            b"descriptor-copy-proof"
        );
        assert_eq!(
            std::fs::metadata(destination.join("run.sh"))
                .expect("stat staged executable")
                .permissions()
                .mode()
                & 0o777,
            0o755
        );

        let source_with_links = root.join("source-with-links");
        let destination_with_links = root.join("destination-with-links");
        std::fs::create_dir(&source_with_links).expect("create linked source fixture");
        std::fs::create_dir(&destination_with_links).expect("create linked destination fixture");
        symlink(
            "missing-relative-target",
            source_with_links.join("dangling"),
        )
        .expect("create dangling source symlink fixture");
        symlink("../outside-tree", source_with_links.join("relative-escape"))
            .expect("create relative-escape source symlink fixture");
        symlink(
            "/usr/local/bin/python3.13",
            source_with_links.join("absolute"),
        )
        .expect("create absolute source symlink fixture");
        let non_utf8_target = OsString::from_vec(vec![b'n', b'o', b'n', b'-', 0xff]);
        symlink(&non_utf8_target, source_with_links.join("non-utf8"))
            .expect("create non-UTF8 source symlink fixture");
        assert_eq!(
            copy_into_staged(
                &open_source_directory(&source_with_links, "open linked test source")
                    .expect("open source containing links"),
                &source_with_links,
                &destination_with_links,
            )
            .expect("opaque source links are preserved without resolution"),
            (0, 4)
        );
        for (name, expected) in [
            ("dangling", OsString::from("missing-relative-target")),
            ("relative-escape", OsString::from("../outside-tree")),
            ("absolute", OsString::from("/usr/local/bin/python3.13")),
            ("non-utf8", non_utf8_target),
        ] {
            let staged = destination_with_links.join(name);
            assert!(std::fs::symlink_metadata(&staged)
                .expect("stat staged link")
                .file_type()
                .is_symlink());
            assert_eq!(
                std::fs::read_link(&staged)
                    .expect("read staged link")
                    .into_os_string(),
                expected
            );
        }

        let source_with_socket = root.join("source-with-socket");
        let destination_with_socket = root.join("destination-with-socket");
        std::fs::create_dir(&source_with_socket).expect("create socket source fixture");
        std::fs::create_dir(&destination_with_socket).expect("create socket destination fixture");
        let _socket = UnixListener::bind(source_with_socket.join("special.sock"))
            .expect("create source Unix socket");
        let special_error = copy_into_staged(
            &open_source_directory(&source_with_socket, "open special test source")
                .expect("open source containing special file"),
            &source_with_socket,
            &destination_with_socket,
        )
        .expect_err("special source file must fail");
        assert!(matches!(special_error, ServiceError::InvalidRequest(_)));

        let source_link = root.join("source-link");
        symlink(&source, &source_link).expect("create root symlink fixture");
        assert!(open_source_directory(&source_link, "open root symlink").is_err());

        std::fs::remove_dir_all(&root).expect("remove descriptor-copy fixture");
    }

    #[test]
    fn opened_symlink_descriptor_keeps_exact_target_and_parent_detects_replacement() {
        let root = std::env::temp_dir().join(format!(
            "qwen38-symlink-race-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir(&root).expect("create symlink-race fixture");
        let parent =
            open_source_directory(&root, "open symlink-race parent").expect("open fixture parent");
        let path = root.join("link");
        symlink("selected-target", &path).expect("create selected source link");
        let parent_before = parent
            .metadata()
            .expect("snapshot parent before replacement");
        let link_before = std::fs::symlink_metadata(&path).expect("lstat selected link");
        let selected = open_source_symlink(&path, "open selected source link")
            .expect("open source link itself");
        require_same_snapshot(
            &link_before,
            &selected.metadata().expect("fstat selected link"),
            &path,
            "test link selection",
        )
        .expect("descriptor selects exact lstat link");

        let replacement = root.join("replacement");
        symlink("replacement-target", &replacement).expect("create replacement link");
        std::fs::rename(&replacement, &path).expect("replace source directory entry");
        assert_eq!(
            read_open_symlink_target(&selected, &path)
                .expect("read target from selected descriptor"),
            std::path::PathBuf::from("selected-target")
        );
        assert_eq!(
            std::fs::read_link(&path).expect("read replacement path target"),
            std::path::PathBuf::from("replacement-target")
        );
        assert!(
            require_same_snapshot(
                &parent_before,
                &parent.metadata().expect("restat parent after replacement"),
                &root,
                "source directory changed while traversed",
            )
            .is_err(),
            "parent descriptor did not expose the namespace replacement"
        );

        std::fs::remove_dir_all(&root).expect("remove symlink-race fixture");
    }

    #[test]
    fn regular_file_byte_cap_is_rejected_before_destination_creation() {
        let root = std::env::temp_dir().join(format!(
            "qwen38-precopy-byte-cap-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let source = root.join("source");
        let destination = root.join("destination");
        std::fs::create_dir_all(&source).expect("create over-cap source fixture");
        std::fs::create_dir(&destination).expect("create over-cap destination fixture");
        let oversized = source.join("oversized-sparse.bin");
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&oversized)
            .expect("create sparse over-cap source file");
        file.set_len(MAX_STAGED_BYTES + 1)
            .expect("extend sparse source beyond byte cap");
        drop(file);

        let error = copy_into_staged(
            &open_source_directory(&source, "open over-cap source").expect("open source"),
            &source,
            &destination,
        )
        .expect_err("over-cap file must fail before copying");
        assert!(matches!(error, ServiceError::InvalidRequest(_)));
        assert!(error.to_string().contains("regular_file_bytes"));
        let staged_oversized = destination.join("oversized-sparse.bin");
        match std::fs::symlink_metadata(&staged_oversized) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            other => {
                panic!("over-cap source created a destination entry before rejection: {other:?}")
            }
        }

        std::fs::remove_dir_all(&root).expect("remove over-cap fixture");
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
}
