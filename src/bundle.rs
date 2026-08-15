//! Deterministic, required session bundling.
//!
//! The archive contains the final mutable workspace, explicit artifacts, the
//! original prompt control record, and every output sidecar. Missing entries,
//! unreadable files, symlinks, tar failures, or rename failures are hard
//! session errors. There is no `--ignore-failed-read` path.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::{fs::OpenOptions, os::unix::fs::OpenOptionsExt};

use tokio::process::Command;

use crate::error::{io_msg, ServiceError, ServiceResult};

#[derive(Debug, Clone)]
pub struct BundleStats {
    pub archive_path: PathBuf,
    pub compressed_bytes: u64,
    pub uncompressed_bytes: u64,
    pub file_count: u64,
    pub artifacts_file_count: u64,
}

pub async fn create_bundle(session_dir: &Path, archive_path: &Path) -> ServiceResult<BundleStats> {
    for required in ["staged", "artifacts", "control", "output"] {
        let path = session_dir.join(required);
        let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
            ServiceError::Internal(io_msg("bundle required entry", &path, &error))
        })?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(ServiceError::Internal(format!(
                "bundle required entry {} is not a real directory",
                path.display()
            )));
        }
    }
    for required in [
        "control/prompt.txt",
        "output/ready.json",
        "output/events.jsonl",
        "output/qwen.stderr",
        "output/qwen-exit-code",
        "output/response.txt",
    ] {
        let path = session_dir.join(required);
        let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
            ServiceError::Internal(io_msg("bundle required file", &path, &error))
        })?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(ServiceError::Internal(format!(
                "bundle required file {} is not a regular non-symlink file",
                path.display()
            )));
        }
    }

    let stats = walk_selected(session_dir)?;
    let parent = archive_path.parent().ok_or_else(|| {
        ServiceError::Internal(format!(
            "bundle path has no parent: {}",
            archive_path.display()
        ))
    })?;
    std::fs::create_dir_all(parent).map_err(|error| {
        ServiceError::Internal(io_msg("create bundle directory", parent, &error))
    })?;
    if archive_path.exists() {
        return Err(ServiceError::Internal(format!(
            "refusing to overwrite existing bundle {}",
            archive_path.display()
        )));
    }
    let archive_name = archive_path.file_name().ok_or_else(|| {
        ServiceError::Internal(format!(
            "bundle path has no file name: {}",
            archive_path.display()
        ))
    })?;
    let mut partial_name = archive_name.to_os_string();
    partial_name.push(".partial");
    let partial = archive_path.with_file_name(partial_name);
    let partial_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&partial)
        .map_err(|error| {
            ServiceError::Internal(format!(
                "create new partial bundle {} without overwrite: {error}",
                partial.display()
            ))
        })?;

    let session = utf8_path(session_dir, "session directory")?;
    let mut command = Command::new("tar");
    command
        .args([
            "--zstd",
            "--sort=name",
            "--mtime=@0",
            "--owner=0",
            "--group=0",
            "--numeric-owner",
            "--format=posix",
            "--pax-option=delete=atime,delete=ctime",
            "-cf",
            "-",
            "-C",
            session,
            "staged",
            "artifacts",
            "control",
            "output",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::from(partial_file))
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = command.output().await.map_err(|error| {
        ServiceError::Internal(format!("bundle: cannot execute pinned tar: {error}"))
    })?;
    if !output.status.success() {
        return Err(ServiceError::Internal(format!(
            "bundle: tar exited {:?}; stderr: {}; stdout: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim(),
            "<archive bytes were directed to the exclusive partial file>"
        )));
    }
    let compressed_bytes = std::fs::metadata(&partial)
        .map_err(|error| ServiceError::Internal(io_msg("stat partial bundle", &partial, &error)))?
        .len();
    if compressed_bytes == 0 {
        return Err(ServiceError::Internal(format!(
            "bundle: pinned tar produced an empty archive at {}",
            partial.display()
        )));
    }
    std::fs::File::open(&partial)
        .and_then(|file| file.sync_all())
        .map_err(|error| ServiceError::Internal(io_msg("sync partial bundle", &partial, &error)))?;
    std::fs::hard_link(&partial, archive_path).map_err(|error| {
        ServiceError::Internal(format!(
            "bundle: no-clobber publication {} -> {} failed: {error}",
            partial.display(),
            archive_path.display()
        ))
    })?;
    std::fs::remove_file(&partial).map_err(|error| {
        ServiceError::Internal(io_msg(
            "remove published partial bundle link",
            &partial,
            &error,
        ))
    })?;
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| ServiceError::Internal(io_msg("sync bundle directory", parent, &error)))?;

    Ok(BundleStats {
        archive_path: archive_path.to_path_buf(),
        compressed_bytes,
        uncompressed_bytes: stats.uncompressed_bytes,
        file_count: stats.file_count,
        artifacts_file_count: stats.artifacts_file_count,
    })
}

#[derive(Default)]
struct TreeStats {
    uncompressed_bytes: u64,
    file_count: u64,
    artifacts_file_count: u64,
}

fn walk_selected(session_dir: &Path) -> ServiceResult<TreeStats> {
    let mut stats = TreeStats::default();
    for name in ["staged", "artifacts", "control", "output"] {
        let before = stats.file_count;
        walk(&session_dir.join(name), &mut stats)?;
        if name == "artifacts" {
            stats.artifacts_file_count = stats.file_count.saturating_sub(before);
        }
    }
    Ok(stats)
}

fn walk(path: &Path, stats: &mut TreeStats) -> ServiceResult<()> {
    for entry in std::fs::read_dir(path)
        .map_err(|error| ServiceError::Internal(io_msg("bundle read directory", path, &error)))?
    {
        let entry = entry
            .map_err(|error| ServiceError::Internal(io_msg("bundle read entry", path, &error)))?;
        let child = entry.path();
        let metadata = std::fs::symlink_metadata(&child)
            .map_err(|error| ServiceError::Internal(io_msg("bundle stat entry", &child, &error)))?;
        if metadata.file_type().is_symlink() {
            return Err(ServiceError::Internal(format!(
                "bundle refuses symlink {}",
                child.display()
            )));
        }
        if metadata.is_dir() {
            walk(&child, stats)?;
        } else if metadata.is_file() {
            stats.file_count = stats.file_count.saturating_add(1);
            stats.uncompressed_bytes = stats.uncompressed_bytes.saturating_add(metadata.len());
        } else {
            return Err(ServiceError::Internal(format!(
                "bundle refuses non-file/non-directory {}",
                child.display()
            )));
        }
    }
    Ok(())
}

fn utf8_path<'a>(path: &'a Path, role: &str) -> ServiceResult<&'a str> {
    path.to_str().ok_or_else(|| {
        ServiceError::Internal(format!("{role} path is not UTF-8: {}", path.display()))
    })
}

pub async fn check_host_dependencies() -> ServiceResult<()> {
    for binary in ["tar", "zstd"] {
        let output = Command::new(binary)
            .arg("--version")
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|error| {
                ServiceError::Internal(format!("pinned service image lacks {binary}: {error}"))
            })?;
        if !output.status.success() {
            return Err(ServiceError::Internal(format!(
                "{binary} --version failed with {:?}",
                output.status.code()
            )));
        }
    }
    Ok(())
}
