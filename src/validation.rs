//! Request input validation.
//!
//! The host this service runs on is exposed to the public internet, so every
//! input is treated as adversarial even though we only listen on loopback.
//! All checks return `Err(ServiceError::InvalidRequest(...))` with a concrete
//! message naming the offending field; we never accept-and-log.

use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};

use crate::config::MAX_PROMPT_BYTES;
use crate::error::{io_msg, ServiceError, ServiceResult};

/// Validated, normalised representation of a `RunRequest` body.
#[derive(Debug)]
pub struct ValidatedRequest {
    pub prompt: String,
    pub folder: PathBuf,
    /// Directory selected during validation, anchored beneath the pinned
    /// input-root descriptor. Holding it through staging closes the
    /// validation-to-copy rename/symlink race.
    pub source_dir: File,
}

pub fn validate(
    prompt: &str,
    folder: &str,
    host_input_root: &Path,
    state_dir: &Path,
    results_dir: &Path,
) -> ServiceResult<ValidatedRequest> {
    let prompt = validate_prompt(prompt)?;
    let (folder, source_dir) = validate_folder(folder, host_input_root, state_dir, results_dir)?;
    Ok(ValidatedRequest {
        prompt,
        folder,
        source_dir,
    })
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

fn validate_folder(
    folder_str: &str,
    host_input_root: &Path,
    state_dir: &Path,
    results_dir: &Path,
) -> ServiceResult<(PathBuf, File)> {
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

    let relative = raw.strip_prefix(host_input_root).map_err(|_| {
        ServiceError::InvalidRequest(format!(
            "field `folder` ({}) must be a strict descendant of the sole mounted input root {}",
            raw.display(),
            host_input_root.display()
        ))
    })?;
    let mut normal_components = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(value) => normal_components.push(value.to_os_string()),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(ServiceError::InvalidRequest(format!(
                    "field `folder` ({}) contains a non-normal path component ({component:?}); submit one exact descendant path without `..`",
                    raw.display()
                )));
            }
        }
    }
    if normal_components.is_empty() {
        return Err(ServiceError::InvalidRequest(format!(
            "field `folder` ({}) must be a strict descendant of the sole mounted input root {}",
            raw.display(),
            host_input_root.display()
        )));
    }

    let mut normalized = host_input_root.to_path_buf();
    for component in &normal_components {
        normalized.push(component);
    }
    for forbidden in [state_dir, results_dir] {
        if normalized.starts_with(forbidden) || forbidden.starts_with(&normalized) {
            return Err(ServiceError::InvalidRequest(format!(
                "field `folder` ({}) overlaps service-owned runtime path {}; refusing recursive/self-modifying staging",
                normalized.display(), forbidden.display()
            )));
        }
    }

    let mut current = open_pinned_input_root(host_input_root)?;
    let mut traversed = host_input_root.to_path_buf();
    for component in &normal_components {
        traversed.push(component);
        let descriptor_child =
            PathBuf::from(format!("/proc/self/fd/{}", current.as_raw_fd())).join(component);
        current = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY)
            .open(&descriptor_child)
            .map_err(|error| {
                ServiceError::InvalidRequest(format!(
                    "field `folder` cannot be opened as an all-directory, no-symlink path beneath the pinned input root at {}: {error}",
                    traversed.display()
                ))
            })?;
    }
    Ok((normalized, current))
}

fn open_pinned_input_root(path: &Path) -> ServiceResult<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open(path)
        .map_err(|error| {
            ServiceError::Internal(io_msg(
                "open pinned host_input_root without following symlinks",
                path,
                &error,
            ))
        })
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::{validate_folder, validate_prompt};
    use crate::config::MAX_PROMPT_BYTES;
    use crate::staging::copy_into_staged;

    #[test]
    fn prompt_validation_is_exact_and_fail_closed() {
        assert_eq!(
            validate_prompt("  keep surrounding space  ")
                .expect("valid prompt")
                .as_str(),
            "  keep surrounding space  "
        );
        assert!(validate_prompt(" \n\t ").is_err());
        assert!(validate_prompt("invalid\0prompt").is_err());
        assert!(validate_prompt(&"x".repeat(MAX_PROMPT_BYTES + 1)).is_err());
    }

    #[test]
    fn folder_validation_is_descriptor_anchored_and_never_follows_symlinks() {
        let root = std::env::temp_dir().join(format!(
            "qwen38-validated-folder-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let source = root.join("source");
        let moved = root.join("selected-source");
        let destination = root.join("destination");
        let state = root.join("service-state");
        let results = root.join("service-results");
        std::fs::create_dir_all(&source).expect("create source fixture");
        std::fs::write(source.join("identity.txt"), b"selected-before-rename")
            .expect("write selected source");

        let (logical, selected) = validate_folder(
            source.to_str().expect("utf8 fixture path"),
            &root,
            &state,
            &results,
        )
        .expect("validate ordinary descendant");

        // Replacing the path after validation must not redirect staging to
        // the replacement: the selected descriptor remains authoritative.
        std::fs::rename(&source, &moved).expect("rename selected source");
        std::fs::create_dir(&source).expect("create path replacement");
        std::fs::write(source.join("identity.txt"), b"wrong-replacement")
            .expect("write replacement source");
        std::fs::create_dir(&destination).expect("create staging destination");
        copy_into_staged(&selected, &logical, &destination).expect("copy selected descriptor");
        assert_eq!(
            std::fs::read(destination.join("identity.txt")).expect("read staged identity"),
            b"selected-before-rename"
        );

        let alias = root.join("alias");
        symlink(&moved, &alias).expect("create final-component symlink");
        assert!(
            validate_folder(alias.to_str().expect("utf8 alias"), &root, &state, &results,).is_err()
        );

        let parent_alias = root.join("parent-alias");
        symlink(&root, &parent_alias).expect("create intermediate symlink");
        let through_alias = parent_alias.join("selected-source");
        assert!(validate_folder(
            through_alias.to_str().expect("utf8 alias descendant"),
            &root,
            &state,
            &results,
        )
        .is_err());

        let parent_component = moved.join("..").join("selected-source");
        assert!(validate_folder(
            parent_component.to_str().expect("utf8 parent path"),
            &root,
            &state,
            &results,
        )
        .is_err());

        std::fs::remove_dir_all(&root).expect("remove validated-folder fixture");
    }
}
