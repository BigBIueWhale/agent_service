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
//! output/         ← bind-mounted into agent container as /output (rw)
//!   (initially empty; the in-container wrapper writes events.jsonl,
//!    qwen-exit-code, and response.txt here.)
//! proxy_sock/     ← shared by the two isolated socat containers
//!   vllm.sock     ← Unix socket connecting outer and inner proxies
//! ```
//!
//! We deliberately copy rather than bind-mounting the user's source folder
//! directly: that gives the agent a workspace it can mutate without affecting
//! the user's working tree, and it stops a buggy / hostile agent from
//! reaching outside the staged tree via symlink shenanigans (we already
//! reject symlinks in `validation.rs`, and the staged tree contains none).

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::config::{MAX_STAGED_BYTES, MAX_STAGED_FILES};
use crate::error::{io_msg, ServiceError, ServiceResult};

#[derive(Debug, Clone)]
pub struct SessionPaths {
    pub root: PathBuf,
    pub staged: PathBuf,
    pub artifacts: PathBuf,
    pub control: PathBuf,
    pub output: PathBuf,
    pub proxy_sock_dir: PathBuf,
}

impl SessionPaths {
    pub fn new(state_dir: &Path, session_id: &str) -> Self {
        let root = state_dir.join("sessions").join(session_id);
        Self {
            staged: root.join("staged"),
            artifacts: root.join("artifacts"),
            control: root.join("control"),
            output: root.join("output"),
            proxy_sock_dir: root.join("proxy_sock"),
            root,
        }
    }

    pub fn create_dirs(&self) -> ServiceResult<()> {
        // Read-only bind mount `control` and the parent `root`
        // are 0o755 — the container reads them as uid 1000; "other" r-x is
        // sufficient even when the host user is not uid 1000.
        for d in [&self.root, &self.staged, &self.control] {
            std::fs::create_dir_all(d)
                .map_err(|e| ServiceError::Staging(io_msg("create session dir", d, &e)))?;
            std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| ServiceError::Staging(io_msg("chmod 0755", d, &e)))?;
        }
        // The service and all three per-session containers are pinned to
        // uid/gid 1000, so world-writable staging is unnecessary. Private
        // 0700 directories permit the intended writers and nobody else.
        for d in [&self.artifacts, &self.output, &self.proxy_sock_dir] {
            std::fs::create_dir_all(d)
                .map_err(|e| ServiceError::Staging(io_msg("create session dir", d, &e)))?;
            std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| ServiceError::Staging(io_msg("chmod 0700", d, &e)))?;
        }
        Ok(())
    }

    pub fn write_prompt(&self, prompt: &str) -> ServiceResult<PathBuf> {
        let p = self.control.join("prompt.txt");
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&p)
            .map_err(|e| ServiceError::Staging(io_msg("open prompt.txt", &p, &e)))?;
        f.write_all(prompt.as_bytes())
            .map_err(|e| ServiceError::Staging(io_msg("write prompt.txt", &p, &e)))?;
        f.flush()
            .map_err(|e| ServiceError::Staging(io_msg("flush prompt.txt", &p, &e)))?;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644))
            .map_err(|e| ServiceError::Staging(io_msg("chmod 0644 prompt.txt", &p, &e)))?;
        Ok(p)
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

/// Recursive copy with permission normalisation. Both `from` and `to` must be
/// existing directories (caller's responsibility). Symlinks are rejected — by
/// the time we get here, `validation::enumerate_folder` has already scanned
/// the source for them, but we re-check defensively.
pub fn copy_into_staged(from: &Path, to: &Path) -> ServiceResult<()> {
    let mut copied_bytes = 0u64;
    let mut copied_files = 0u64;
    copy_recursive(from, to, &mut copied_bytes, &mut copied_files)
}

fn copy_recursive(
    from: &Path,
    to: &Path,
    copied_bytes: &mut u64,
    copied_files: &mut u64,
) -> ServiceResult<()> {
    let entries = std::fs::read_dir(from)
        .map_err(|e| ServiceError::Staging(io_msg("read source dir", from, &e)))?;
    for entry in entries {
        let entry =
            entry.map_err(|e| ServiceError::Staging(io_msg("read source dir entry", from, &e)))?;
        let src_path = entry.path();
        let meta = std::fs::symlink_metadata(&src_path)
            .map_err(|e| ServiceError::Staging(io_msg("stat source entry", &src_path, &e)))?;

        let name = match src_path.file_name() {
            Some(n) => n.to_owned(),
            None => {
                return Err(ServiceError::Staging(format!(
                    "source entry has no file name: {}",
                    src_path.display()
                )));
            }
        };
        let dst_path = to.join(&name);

        if meta.file_type().is_symlink() {
            return Err(ServiceError::Staging(format!(
                "refusing to copy symlink at {} (validation should have caught this)",
                src_path.display()
            )));
        }
        if meta.is_dir() {
            std::fs::create_dir(&dst_path).map_err(|e| {
                ServiceError::Staging(io_msg("create staged subdir", &dst_path, &e))
            })?;
            std::fs::set_permissions(&dst_path, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| ServiceError::Staging(io_msg("chmod 0755", &dst_path, &e)))?;
            copy_recursive(&src_path, &dst_path, copied_bytes, copied_files)?;
        } else if meta.is_file() {
            let bytes = std::fs::copy(&src_path, &dst_path).map_err(|e| {
                ServiceError::Staging(format!(
                    "copy {} → {}: {e}",
                    src_path.display(),
                    dst_path.display()
                ))
            })?;
            if bytes != meta.len() {
                return Err(ServiceError::Staging(format!(
                    "source changed size while staging {}: pre-copy={} bytes copied={bytes}",
                    src_path.display(),
                    meta.len()
                )));
            }
            *copied_files = copied_files.saturating_add(1);
            *copied_bytes = copied_bytes.saturating_add(bytes);
            if *copied_files > MAX_STAGED_FILES || *copied_bytes > MAX_STAGED_BYTES {
                return Err(ServiceError::Staging(format!(
                    "source exceeded the staging cap while being copied: files={} (max={MAX_STAGED_FILES}), bytes={} (max={MAX_STAGED_BYTES})",
                    *copied_files, *copied_bytes
                )));
            }
            // Preserve only the semantic executable bit, discarding all
            // setuid/setgid/sticky and group/world-write bits.
            let source_mode = meta.permissions().mode();
            let target_mode = if source_mode & 0o111 != 0 {
                0o755
            } else {
                0o644
            };
            std::fs::set_permissions(&dst_path, std::fs::Permissions::from_mode(target_mode))
                .map_err(|e| {
                    ServiceError::Staging(io_msg("normalize staged file mode", &dst_path, &e))
                })?;
        } else {
            return Err(ServiceError::Staging(format!(
                "unsupported file type at {} (type: {:?})",
                src_path.display(),
                meta.file_type()
            )));
        }
    }
    Ok(())
}
