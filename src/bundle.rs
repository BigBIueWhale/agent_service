//! Per-session result bundling.
//!
//! At end-of-run, the orchestrator tars three things from the session's
//! staging dir into `<results_dir>/<session_id>/bundle.tar.zst`:
//!
//! - `artifacts/` — everything the agent wrote for the operator.
//! - `output/events.jsonl` — the agent's full conversation history: every
//!   assistant turn, every tool call and result, and the final
//!   `type:"result"` event whose payload is the parsed `response`.
//! - `output/qwen-exit-code` — the agent process's exit code.
//!
//! Compression is `zstd`. Bundles persist forever, until explicitly
//! removed via `DELETE /v1/agent/sessions/:id` — the lifecycle is
//! client-controlled and never time-based.
//!
//! No "response" sidecar file: the parsed answer text already lives in
//! `<results_dir>/<id>/finished.json` (the persisted `SessionBody`),
//! and the full history surrounding it is in `events.jsonl`.
//!
//! Both `tar` and `zstd` are required on the host and are verified at
//! pre-flight (`api::pre_flight`) — failure to find them is fatal at
//! startup, so an in-progress run can never discover the gap mid-flight.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::process::Command;

use crate::error::{ServiceError, ServiceResult, io_msg};

/// Cap on how long bundling is allowed to take. Far more than enough for
/// a 128 GiB worth of artifacts at zstd default speed (~500 MB/s); if it
/// blows past this, something is genuinely wrong on the host.
const BUNDLE_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone)]
pub struct BundleStats {
    pub archive_path: PathBuf,
    pub compressed_bytes: u64,
    pub uncompressed_bytes: u64,
    pub file_count: u64,
    pub artifacts_file_count: u64,
}

/// Build the per-session bundle. Returns `Ok(stats)` on success. Returns
/// `Err(ServiceError)` if `tar` itself fails or is too slow — the caller
/// is expected to log a teardown diagnostic and surface an empty
/// `bundle_archive_path` to the client rather than propagate this as a
/// hard error (the agent's response is independently captured and worth
/// returning).
pub async fn create_bundle(
    session_dir: &Path,
    archive_path: &Path,
) -> ServiceResult<BundleStats> {
    let parent = archive_path.parent().ok_or_else(|| {
        ServiceError::Internal(format!(
            "bundle archive path has no parent: {}",
            archive_path.display()
        ))
    })?;
    std::fs::create_dir_all(parent)
        .map_err(|e| ServiceError::Internal(io_msg("create bundle parent dir", parent, &e)))?;

    let session_dir_str = session_dir.to_str().ok_or_else(|| {
        ServiceError::Internal(format!(
            "session dir contains non-UTF-8 path: {}",
            session_dir.display()
        ))
    })?;
    let archive_str = archive_path.to_str().ok_or_else(|| {
        ServiceError::Internal(format!(
            "bundle archive path contains non-UTF-8 path: {}",
            archive_path.display()
        ))
    })?;
    if session_dir_str.contains(':') || archive_str.contains(':') {
        return Err(ServiceError::Internal(format!(
            "bundle path contains a ':' which would confuse tar: session={session_dir_str:?} archive={archive_str:?}"
        )));
    }

    // Compute uncompressed stats *before* tarring, so we have them even if
    // tar partially writes.
    let stats = walk_stats(session_dir)?;

    // tar --zstd --ignore-failed-read -cf <archive> -C <session_dir>
    //     artifacts output/events.jsonl output/qwen-exit-code
    //
    // `--ignore-failed-read` makes the run robust against the case where
    // the agent crashed before writing one of the output files; we still
    // bundle whatever exists.
    let mut cmd = Command::new("tar");
    cmd.args([
        "--zstd",
        "--ignore-failed-read",
        "-cf",
        archive_str,
        "-C",
        session_dir_str,
        "artifacts",
        "output/events.jsonl",
        "output/qwen-exit-code",
    ]);
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let fut = async {
        let out = cmd.output().await.map_err(|e| {
            ServiceError::Internal(format!("bundle: failed to spawn tar: {e}"))
        })?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            let code = out
                .status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "<signal>".into());
            return Err(ServiceError::Internal(format!(
                "bundle: tar exited with code {code}; stderr: {stderr}"
            )));
        }
        Ok(())
    };
    match tokio::time::timeout(BUNDLE_TIMEOUT, fut).await {
        Ok(r) => r?,
        Err(_) => {
            return Err(ServiceError::Timeout(format!(
                "bundle: tar exceeded {BUNDLE_TIMEOUT:?}"
            )));
        }
    }

    let compressed_bytes = std::fs::metadata(archive_path)
        .map(|m| m.len())
        .map_err(|e| ServiceError::Internal(io_msg("stat bundle archive", archive_path, &e)))?;

    Ok(BundleStats {
        archive_path: archive_path.to_path_buf(),
        compressed_bytes,
        uncompressed_bytes: stats.uncompressed_bytes,
        file_count: stats.file_count,
        artifacts_file_count: stats.artifacts_file_count,
    })
}

#[derive(Default)]
struct PreTarStats {
    uncompressed_bytes: u64,
    file_count: u64,
    artifacts_file_count: u64,
}

fn walk_stats(session_dir: &Path) -> ServiceResult<PreTarStats> {
    let mut s = PreTarStats::default();

    let artifacts = session_dir.join("artifacts");
    if artifacts.exists() {
        let (af, ab) = walk_dir(&artifacts)?;
        s.artifacts_file_count = af;
        s.file_count += af;
        s.uncompressed_bytes += ab;
    }

    for sidecar in [
        session_dir.join("output/events.jsonl"),
        session_dir.join("output/qwen-exit-code"),
    ] {
        if let Ok(meta) = std::fs::metadata(&sidecar) {
            if meta.is_file() {
                s.file_count += 1;
                s.uncompressed_bytes += meta.len();
            }
        }
    }

    Ok(s)
}

fn walk_dir(dir: &Path) -> ServiceResult<(u64, u64)> {
    let mut files = 0u64;
    let mut bytes = 0u64;
    walk_recursive(dir, &mut files, &mut bytes)?;
    Ok((files, bytes))
}

fn walk_recursive(dir: &Path, files: &mut u64, bytes: &mut u64) -> ServiceResult<()> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| ServiceError::Internal(io_msg("walk_recursive read_dir", dir, &e)))?;
    for entry in entries {
        let entry = entry
            .map_err(|e| ServiceError::Internal(io_msg("walk_recursive entry", dir, &e)))?;
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path)
            .map_err(|e| ServiceError::Internal(io_msg("walk_recursive stat", &path, &e)))?;
        if meta.file_type().is_symlink() {
            // Don't follow; don't count. Defensive.
            continue;
        }
        if meta.is_dir() {
            walk_recursive(&path, files, bytes)?;
        } else if meta.is_file() {
            *files += 1;
            *bytes = bytes.saturating_add(meta.len());
        }
    }
    Ok(())
}

/// Verify the host has both `tar` and `zstd` on PATH. Called from
/// `api::pre_flight`.
pub async fn check_host_dependencies() -> ServiceResult<()> {
    for (binary, role) in [
        ("tar", "bundle creation"),
        ("zstd", "bundle compression"),
    ] {
        let out = Command::new(binary)
            .arg("--version")
            .output()
            .await
            .map_err(|e| {
                ServiceError::Internal(format!(
                    "host is missing `{binary}` (needed for {role}): {e}"
                ))
            })?;
        if !out.status.success() {
            return Err(ServiceError::Internal(format!(
                "host's `{binary}` (needed for {role}) returned non-zero on `--version`"
            )));
        }
    }
    Ok(())
}
