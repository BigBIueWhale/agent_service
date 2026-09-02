//! Request input validation.
//!
//! The host this service runs on is exposed to the public internet, so every
//! input is treated as adversarial even though we only listen on loopback.
//! All checks return `Err(ServiceError::InvalidRequest(...))` with a concrete
//! message naming the offending field; we never accept-and-log.

use std::path::PathBuf;

use crate::config::{MAX_ARCHIVE_BYTES, MAX_PROMPT_BYTES, MAX_SESSION_TURNS_CEILING};
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
    /// Exact turn budget this session runs under: the locked default unless
    /// the creation body named another one inside the pinned ceiling. The
    /// launcher passes it to Qwen Code, so every foreground subagent this
    /// session starts is bounded by the same number.
    pub max_session_turns: u32,
    /// The spooled workspace archive whose structure has been proved against
    /// the archive contract before durable acceptance.
    pub archive: SpooledArchive,
}

pub fn validate(
    prompt: &str,
    max_session_turns: u32,
    archive: SpooledArchive,
) -> ServiceResult<ValidatedRequest> {
    let prompt = validate_prompt(prompt)?;
    validate_session_turn_budget(max_session_turns)?;
    let archive = validate_spooled_archive(archive)?;
    Ok(ValidatedRequest {
        prompt,
        max_session_turns,
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

/// Decode the optional `max_session_turns` field of the creation body.
///
/// The wire type is a JSON number, so a fractional value, an exponent form
/// that is not integral, and a negative count all arrive here as
/// syntactically valid JSON and must be refused by name. Nothing is rounded,
/// truncated, or clamped: a caller that asked for a budget this deployment
/// cannot run gets an error, not a quietly different session. A silently
/// shortened budget would end as an ordinary turn-exhausted exit 53 and be
/// graded as one, which is exactly the misreading this refuses to create.
pub fn validate_max_session_turns(value: &serde_json::Number) -> ServiceResult<u32> {
    // An ordinary request: a JSON integer literal that fits an unsigned 64-bit
    // count. Anything above the ceiling is named as such before narrowing, so
    // the refusal quotes the number the caller actually sent.
    if let Some(turns) = value.as_u64() {
        if turns > u64::from(MAX_SESSION_TURNS_CEILING) {
            return Err(ServiceError::InvalidRequest(format!(
                "field `max_session_turns` ({turns}) exceeds the {MAX_SESSION_TURNS_CEILING}-turn ceiling"
            )));
        }
        let turns = turns as u32;
        validate_session_turn_budget(turns)?;
        return Ok(turns);
    }
    if value.as_i64().is_some() {
        return Err(ServiceError::InvalidRequest(format!(
            "field `max_session_turns` ({value}) is negative; the per-session turn budget must be an integer in 1..={MAX_SESSION_TURNS_CEILING}"
        )));
    }
    // Every remaining JSON number is one this deployment cannot read as a turn
    // count: a fraction, an exponent form, or a magnitude no integer type here
    // can hold. None of them is repaired into a nearby budget.
    Err(ServiceError::InvalidRequest(format!(
        "field `max_session_turns` ({value}) is not a plain integer; the per-session turn budget must be an integer in 1..={MAX_SESSION_TURNS_CEILING}"
    )))
}

/// The turn-budget range rule shared by the pre-stream API check and the
/// pre-acceptance request validation. Zero and the ceiling are separate,
/// separately named refusals: they are different mistakes.
pub fn validate_session_turn_budget(turns: u32) -> ServiceResult<()> {
    if turns == 0 {
        return Err(ServiceError::InvalidRequest(
            "field `max_session_turns` is zero; a session that may take no model turn cannot do any work".into(),
        ));
    }
    if turns > MAX_SESSION_TURNS_CEILING {
        return Err(ServiceError::InvalidRequest(format!(
            "field `max_session_turns` ({turns}) exceeds the {MAX_SESSION_TURNS_CEILING}-turn ceiling"
        )));
    }
    Ok(())
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

    use super::{
        validate_max_session_turns, validate_prompt, validate_session_turn_budget,
        validate_spooled_archive, SpooledArchive,
    };
    use crate::config::{
        DEFAULT_MAX_SESSION_TURNS, MAX_ARCHIVE_BYTES, MAX_PROMPT_BYTES, MAX_SESSION_TURNS_CEILING,
    };

    fn decode_turns(json: &str) -> crate::error::ServiceResult<u32> {
        let value: serde_json::Number =
            serde_json::from_str(json).expect("fixture must be a JSON number");
        validate_max_session_turns(&value)
    }

    #[test]
    fn session_turn_budget_validation_is_exact_and_never_clamps() {
        assert_eq!(decode_turns("1").expect("one turn is requestable"), 1);
        assert_eq!(
            decode_turns(&DEFAULT_MAX_SESSION_TURNS.to_string())
                .expect("the locked default is requestable"),
            DEFAULT_MAX_SESSION_TURNS
        );
        assert_eq!(
            decode_turns(&MAX_SESSION_TURNS_CEILING.to_string())
                .expect("the ceiling itself is requestable"),
            MAX_SESSION_TURNS_CEILING
        );

        let just_above_ceiling = (MAX_SESSION_TURNS_CEILING + 1).to_string();
        let ceiling_fragment = format!("exceeds the {MAX_SESSION_TURNS_CEILING}-turn ceiling");
        for (json, fragment) in [
            ("0", "is zero"),
            ("-1", "is negative"),
            ("-400", "is negative"),
            ("1.5", "is not a plain integer"),
            ("400.5", "is not a plain integer"),
            // Beyond u64 there is no integer representation left, so this is
            // refused as unreadable rather than silently becoming a float.
            ("100000000000000000000", "is not a plain integer"),
            (just_above_ceiling.as_str(), ceiling_fragment.as_str()),
            ("1000000", ceiling_fragment.as_str()),
        ] {
            let error = decode_turns(json).expect_err(&format!(
                "turn budget {json} must be refused, never clamped"
            ));
            let message = error.to_string();
            assert!(
                message.contains(fragment) && message.contains("max_session_turns"),
                "turn budget {json} produced an unnamed refusal: {message}"
            );
        }

        // The range rule the API boundary and the pre-acceptance validation
        // share must agree with the decoder on every edge.
        assert!(validate_session_turn_budget(0).is_err());
        assert!(validate_session_turn_budget(1).is_ok());
        assert!(validate_session_turn_budget(MAX_SESSION_TURNS_CEILING).is_ok());
        assert!(validate_session_turn_budget(MAX_SESSION_TURNS_CEILING + 1).is_err());
    }

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
