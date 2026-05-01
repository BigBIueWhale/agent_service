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
//!    qwen-exit-code, .done here. Service-plane only — the agent
//!    doesn't write here.)
//! proxy_sock/    ← bind-mounted into the inner proxy as /sock (ro)
//!   vllm.sock     ← Unix socket the host-side Rust forwarder listens on
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

use crate::error::{ServiceError, ServiceResult, io_msg};

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
        // Read-only bind mounts (`staged`, `control`) and the parent `root`
        // are 0o755 — the container reads them as uid 1000; "other" r-x is
        // sufficient even when the host user is not uid 1000.
        for d in [&self.root, &self.staged, &self.control] {
            std::fs::create_dir_all(d)
                .map_err(|e| ServiceError::Staging(io_msg("create session dir", d, &e)))?;
            std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| ServiceError::Staging(io_msg("chmod 0755", d, &e)))?;
        }
        // Read-write bind mounts (`artifacts`, `output`, `proxy_sock_dir`)
        // are 0o777 — the container's uid 1000 must be able to *create*
        // files there regardless of which uid the host orchestrator is
        // running as. This is acceptable because the parent state dir
        // (typically `$HOME/.local/state/agent_service`) is mode 0o700 by
        // virtue of its parent — only the host user can reach this tree
        // at all. The socket file itself, when created by socat, is 0o600
        // owned by uid 1000, so cross-process access is still controlled
        // at the file level.
        for d in [&self.artifacts, &self.output, &self.proxy_sock_dir] {
            std::fs::create_dir_all(d)
                .map_err(|e| ServiceError::Staging(io_msg("create session dir", d, &e)))?;
            std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o777))
                .map_err(|e| ServiceError::Staging(io_msg("chmod 0777", d, &e)))?;
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
    copy_recursive(from, to)
}

fn copy_recursive(from: &Path, to: &Path) -> ServiceResult<()> {
    let entries = std::fs::read_dir(from)
        .map_err(|e| ServiceError::Staging(io_msg("read source dir", from, &e)))?;
    for entry in entries {
        let entry = entry
            .map_err(|e| ServiceError::Staging(io_msg("read source dir entry", from, &e)))?;
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
            copy_recursive(&src_path, &dst_path)?;
        } else if meta.is_file() {
            std::fs::copy(&src_path, &dst_path).map_err(|e| {
                ServiceError::Staging(format!(
                    "copy {} → {}: {e}",
                    src_path.display(),
                    dst_path.display()
                ))
            })?;
            // Force 0o644 so the agent container's user can read regardless
            // of the host user's umask. We deliberately discard the source's
            // exact mode bits — the agent doesn't need to know them, and
            // anything mode-sensitive (executables) the agent can re-chmod.
            std::fs::set_permissions(&dst_path, std::fs::Permissions::from_mode(0o644))
                .map_err(|e| ServiceError::Staging(io_msg("chmod 0644", &dst_path, &e)))?;
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
