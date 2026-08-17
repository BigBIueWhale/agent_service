//! Request input validation.
//!
//! The host this service runs on is exposed to the public internet, so every
//! input is treated as adversarial even though we only listen on loopback.
//! All checks return `Err(ServiceError::InvalidRequest(...))` with a concrete
//! message naming the offending field; we never accept-and-log.

use std::path::PathBuf;

use crate::config::{MAX_ARCHIVE_BYTES, MAX_PROMPT_BYTES};
use crate::error::{io_msg, ServiceError, ServiceResult};

/// One spooled, hash-committed workspace archive received over the
/// connection. The archive bytes were streamed to this service-owned regular
/// file and proved to match the caller's declared byte count and SHA-256
/// before validation sees them; the workspace never arrives as a shared
/// filesystem path.
#[derive(Clone, Debug)]
pub struct SpooledArchive {
    pub path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
}

/// Validated, normalised representation of the one session-creation body.
#[derive(Debug)]
pub struct ValidatedRequest {
    pub prompt: String,
    /// Exact typed history policy selected by the trusted API decoder.
    pub preserve_thinking: bool,
    /// The spooled workspace archive whose structure has been proved against
    /// the archive contract before durable acceptance.
    pub archive: SpooledArchive,
}

pub fn validate(
    prompt: &str,
    preserve_thinking: bool,
    archive: SpooledArchive,
) -> ServiceResult<ValidatedRequest> {
    let prompt = validate_prompt(prompt)?;
    let archive = validate_spooled_archive(archive)?;
    Ok(ValidatedRequest {
        prompt,
        preserve_thinking,
        archive,
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

/// The archive commitment rules shared by the pre-stream API check and the
/// pre-acceptance request validation: exactly 64 lowercase-hex hash
/// characters, and a non-zero declared byte count within the explicit
/// archive bound.
pub fn validate_archive_commitment(bytes: u64, sha256: &str) -> ServiceResult<()> {
    if sha256.len() != 64
        || !sha256
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err(ServiceError::InvalidRequest(format!(
            "field `archive_sha256` ({sha256:?}) is not exactly 64 lowercase hexadecimal characters"
        )));
    }
    if bytes == 0 {
        return Err(ServiceError::InvalidRequest(
            "field `archive_bytes` is zero; an empty upload cannot carry a zip container".into(),
        ));
    }
    if bytes > MAX_ARCHIVE_BYTES {
        return Err(ServiceError::InvalidRequest(format!(
            "field `archive_bytes` ({bytes}) exceeds the {MAX_ARCHIVE_BYTES}-byte archive bound"
        )));
    }
    Ok(())
}

/// Cross-check the spooled archive against its own commitment, plus the
/// spool path itself: an absolute, ordinary, non-symlink service-owned file
/// of exactly the declared size. The declared-versus-received byte and hash
/// equality was proved while the upload streamed; this validation refuses to
/// build a request around a spool that has since drifted from that proof.
/// The archive's internal structure is proved separately against the staging
/// contract before durable acceptance.
fn validate_spooled_archive(archive: SpooledArchive) -> ServiceResult<SpooledArchive> {
    validate_archive_commitment(archive.bytes, &archive.sha256)?;
    if !archive.path.is_absolute() {
        return Err(ServiceError::Internal(format!(
            "spooled archive path {} is not absolute",
            archive.path.display()
        )));
    }
    let metadata = std::fs::symlink_metadata(&archive.path).map_err(|error| {
        ServiceError::Internal(io_msg(
            "stat spooled archive before acceptance",
            &archive.path,
            &error,
        ))
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(ServiceError::Internal(format!(
            "spooled archive at {} is not an ordinary non-symlink file",
            archive.path.display()
        )));
    }
    if metadata.len() != archive.bytes {
        return Err(ServiceError::Internal(format!(
            "spooled archive at {} is {} bytes but the proved upload was {} bytes",
            archive.path.display(),
            metadata.len(),
            archive.bytes
        )));
    }
    Ok(archive)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::{validate_prompt, validate_spooled_archive, SpooledArchive};
    use crate::config::{MAX_ARCHIVE_BYTES, MAX_PROMPT_BYTES};

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
    fn spooled_archive_validation_is_exact_and_fail_closed() {
        let root = std::env::temp_dir().join(format!(
            "qwen38-validated-archive-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&root).expect("create archive fixture root");
        let spool = root.join("archive.zip");
        std::fs::write(&spool, b"exact-spool-bytes").expect("write spool fixture");
        let exact = SpooledArchive {
            path: spool.clone(),
            bytes: 17,
            sha256: "0".repeat(64),
        };

        let accepted =
            validate_spooled_archive(exact.clone()).expect("exact spool commitment is accepted");
        assert_eq!(accepted.bytes, 17);

        for (label, mutate) in [
            (
                "uppercase hash digit",
                Box::new(|archive: &mut SpooledArchive| {
                    archive.sha256 = format!("A{}", "0".repeat(63));
                }) as Box<dyn Fn(&mut SpooledArchive)>,
            ),
            (
                "short hash",
                Box::new(|archive: &mut SpooledArchive| {
                    archive.sha256 = "0".repeat(63);
                }),
            ),
            (
                "zero declared bytes",
                Box::new(|archive: &mut SpooledArchive| archive.bytes = 0),
            ),
            (
                "declared bytes beyond the archive bound",
                Box::new(|archive: &mut SpooledArchive| archive.bytes = MAX_ARCHIVE_BYTES + 1),
            ),
            (
                "declared bytes disagreeing with the spool file",
                Box::new(|archive: &mut SpooledArchive| archive.bytes = 16),
            ),
            (
                "relative spool path",
                Box::new(|archive: &mut SpooledArchive| {
                    archive.path = std::path::PathBuf::from("relative/archive.zip");
                }),
            ),
        ] {
            let mut mutated = exact.clone();
            mutate(&mut mutated);
            assert!(
                validate_spooled_archive(mutated).is_err(),
                "commitment drift was accepted: {label}"
            );
        }

        let missing = SpooledArchive {
            path: root.join("absent.zip"),
            ..exact.clone()
        };
        assert!(validate_spooled_archive(missing).is_err());

        let linked = root.join("linked.zip");
        symlink(&spool, &linked).expect("create spool symlink fixture");
        let through_link = SpooledArchive {
            path: linked,
            ..exact
        };
        assert!(
            validate_spooled_archive(through_link).is_err(),
            "a symlinked spool path was accepted"
        );

        std::fs::remove_dir_all(&root).expect("remove archive fixture");
    }
}
