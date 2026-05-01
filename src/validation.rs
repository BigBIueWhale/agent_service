//! Request input validation.
//!
//! The host this service runs on is exposed to the public internet, so every
//! input is treated as adversarial even though we only listen on loopback.
//! All checks return `Err(ServiceError::InvalidRequest(...))` with a concrete
//! message naming the offending field; we never accept-and-log.

use std::path::{Path, PathBuf};

use crate::config::{MAX_PROMPT_BYTES, MAX_STAGED_BYTES, MAX_STAGED_FILES};
use crate::error::{ServiceError, ServiceResult, io_msg};

/// Validated, normalised representation of a `RunRequest` body.
#[derive(Debug)]
pub struct ValidatedRequest {
    pub prompt: String,
    pub folder: PathBuf,
}

pub fn validate(prompt: &str, folder: &str) -> ServiceResult<ValidatedRequest> {
    let prompt = validate_prompt(prompt)?;
    let folder = validate_folder(folder)?;
    Ok(ValidatedRequest { prompt, folder })
}

fn validate_prompt(prompt: &str) -> ServiceResult<String> {
    let trimmed = prompt.trim();
    if trimmed.is_empty() {
        return Err(ServiceError::InvalidRequest(
            "field `prompt` is empty after trimming whitespace".into(),
        ));
    }
    if prompt.len() > MAX_PROMPT_BYTES {
        return Err(ServiceError::InvalidRequest(format!(
            "field `prompt` is {} bytes, exceeding the {MAX_PROMPT_BYTES}-byte limit",
            prompt.len()
        )));
    }
    if prompt.contains('\0') {
        return Err(ServiceError::InvalidRequest(
            "field `prompt` contains a NUL byte".into(),
        ));
    }
    Ok(prompt.to_string())
}

fn validate_folder(folder_str: &str) -> ServiceResult<PathBuf> {
    if folder_str.is_empty() {
        return Err(ServiceError::InvalidRequest(
            "field `folder` is empty".into(),
        ));
    }
    if folder_str.contains('\0') {
        return Err(ServiceError::InvalidRequest(
            "field `folder` contains a NUL byte".into(),
        ));
    }
    let raw = Path::new(folder_str);
    if !raw.is_absolute() {
        return Err(ServiceError::InvalidRequest(format!(
            "field `folder` ({folder_str:?}) is not an absolute path"
        )));
    }

    // Canonicalise to resolve `..`, symlinks, and dedup separators. If
    // canonicalisation fails the path is invalid for our purposes.
    let canonical = std::fs::canonicalize(raw).map_err(|e| {
        ServiceError::InvalidRequest(io_msg(
            "field `folder` cannot be resolved",
            raw,
            &e,
        ))
    })?;

    let meta = std::fs::symlink_metadata(&canonical).map_err(|e| {
        ServiceError::InvalidRequest(io_msg(
            "field `folder` (canonicalised) cannot be stat'd",
            &canonical,
            &e,
        ))
    })?;
    if !meta.is_dir() {
        return Err(ServiceError::InvalidRequest(format!(
            "field `folder` ({}) is not a directory",
            canonical.display()
        )));
    }

    // Refuse system-level roots — copying them would be a self-DoS even with
    // the size cap, and there's no legitimate reason to point at them.
    let forbidden_prefixes: &[&str] = &[
        "/", "/etc", "/proc", "/sys", "/dev", "/boot", "/var/run", "/run",
    ];
    let canonical_str = canonical.to_string_lossy();
    for fb in forbidden_prefixes {
        if canonical_str == *fb {
            return Err(ServiceError::InvalidRequest(format!(
                "field `folder` ({canonical_str}) is a forbidden system path"
            )));
        }
    }

    Ok(canonical)
}

/// Pre-flight folder enumeration — returns the cumulative byte size and file
/// count, returning `InvalidRequest` if either limit is breached.
pub fn enumerate_folder(root: &Path) -> ServiceResult<(u64, u64)> {
    let mut bytes: u64 = 0;
    let mut files: u64 = 0;
    walk(root, &mut bytes, &mut files)?;
    Ok((bytes, files))
}

fn walk(dir: &Path, bytes: &mut u64, files: &mut u64) -> ServiceResult<()> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| ServiceError::InvalidRequest(io_msg("cannot read folder", dir, &e)))?;
    for entry in entries {
        let entry = entry.map_err(|e| {
            ServiceError::InvalidRequest(io_msg("cannot read folder entry", dir, &e))
        })?;
        let path = entry.path();
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) => {
                return Err(ServiceError::InvalidRequest(io_msg(
                    "cannot stat folder entry",
                    &path,
                    &e,
                )));
            }
        };

        // Reject symlinks unconditionally. They're a portable copy hazard
        // and can escape the staging root if followed naively. Users can
        // dereference manually before invoking the service if they want
        // the contents.
        if meta.file_type().is_symlink() {
            return Err(ServiceError::InvalidRequest(format!(
                "folder contains a symbolic link at {}; symlinks are not supported in the staged input",
                path.display()
            )));
        }

        if meta.is_dir() {
            walk(&path, bytes, files)?;
        } else if meta.is_file() {
            *files += 1;
            *bytes = bytes.saturating_add(meta.len());
            if *files > MAX_STAGED_FILES {
                return Err(ServiceError::InvalidRequest(format!(
                    "folder contains more than {MAX_STAGED_FILES} files (file-count cap)"
                )));
            }
            if *bytes > MAX_STAGED_BYTES {
                return Err(ServiceError::InvalidRequest(format!(
                    "folder content exceeds {MAX_STAGED_BYTES} bytes (size cap)"
                )));
            }
        } else {
            return Err(ServiceError::InvalidRequest(format!(
                "folder contains a non-regular, non-directory entry at {} (type: {:?})",
                path.display(),
                meta.file_type()
            )));
        }
    }
    Ok(())
}
