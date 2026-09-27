//! Descriptor and LF framing for captured agent output. The shared pure
//! runtime_contract owner decides record identity, admission and observations.
use crate::error::{io_msg, ServiceError, ServiceResult};
pub use runtime_contract::runtime::{
    terminal_exit_code, AgentResult, AgentScope, RequestScope, ERROR_SUBTYPES, SUCCESS_SUBTYPE,
};
pub use runtime_contract::usage::GenerationUsageSummary;
use runtime_contract::{
    json::{Document, Limits},
    runtime::{RuntimeBindings, RuntimeContract, RuntimeLimits},
    schema::ValidationLimits,
};
use serde::Serialize;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

fn pinned_contract() -> ServiceResult<RuntimeContract> {
    // The build validates and embeds only the fixed deployment facts used here.
    let bytes = env!("CAPTURED_STREAM_BINDINGS_JSON").as_bytes();
    let document = Document::decode(
        bytes,
        Limits {
            bytes: bytes.len(),
            nodes: bytes.len(),
            depth: bytes.len(),
        },
    )
    .map_err(|cause| ServiceError::Internal(format!("runtime bindings: {cause:?}")))?;
    Ok(RuntimeContract::new(
        RuntimeBindings::new(document)?,
        RuntimeLimits {
            json: Limits {
                bytes: MAX_EVENT_RECORD_BYTES,
                nodes: MAX_EVENT_RECORD_BYTES,
                depth: MAX_EVENT_RECORD_BYTES,
            },
            schema: ValidationLimits {
                operations: u32::MAX as usize,
            },
        },
    ))
}

// The captured-record input bound limits raw JSON bytes, independently of
// tokens or session length. It is not a bound on decoded or retained memory.
const MAX_EVENT_RECORD_BYTES: usize = 128 * 1024 * 1024;

/// Snapshot observations survive an uncertifiable ending. They sum only complete
/// records with valid served usage; they are never a complete-result assertion.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct OutputProgress {
    pub num_turns: Option<u64>,
    pub last_event_at_unix: Option<u64>,
    pub output_event_bytes: u64,
    pub observed_usage: GenerationUsageSummary,
    pub observed_subagent_scope_count: u64,
    pub observed_unaccounted_records: u64,
}

pub struct EventSnapshot {
    pub observed: OutputProgress,
    pub replay_completion: ReplayCompletion,
    pub certified: ServiceResult<AgentResult>,
}

/// Reaching the selected descriptor extent is separate from the capture
/// owner's proof that no further output can arrive.
#[derive(Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ReplayCompletion {
    ReachedBoundary,
    ReadFailed {
        bytes_read: u64,
        cause: String,
    },
    SourceShortened {
        bytes_read: u64,
        expected_bytes: u64,
    },
}

struct EventPrefix<R> {
    file: R,
    bytes: u64,
    last_event_at_unix: u64,
}

/// Captured JSONL has a named incomplete-tail rule: only LF commits a record.
/// A nonempty final prefix contributes one unaccounted record, even if it parses
/// as JSON. Abrupt exit cannot certify it. Invalid complete records also remain
/// unaccounted, while independently readable observations survive. Certification
/// still requires the entire pinned stream and its unique final main result.
/// Both running reads and terminal finalization consume this one descriptor scan.
pub fn read_event_snapshot(path: &Path) -> ServiceResult<Option<EventSnapshot>> {
    open_event_prefix(path)?
        .map(|prefix| read_opened_event_snapshot(path, prefix))
        .transpose()
}

fn open_event_prefix(path: &Path) -> ServiceResult<Option<EventPrefix<std::fs::File>>> {
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(ServiceError::AgentOutputMissing(io_msg(
                "open events.jsonl without following links",
                path,
                &error,
            )))
        }
    };
    let metadata = file.metadata().map_err(|error| {
        ServiceError::AgentOutputMissing(io_msg("fstat opened events.jsonl", path, &error))
    })?;
    if !metadata.is_file()
        || metadata.permissions().mode() & 0o777 != 0o600
        || metadata.uid() != 1000
        || metadata.gid() != 1000
    {
        return Err(ServiceError::AgentOutputMissing(format!(
            "events.jsonl at {} has unsafe opened type/mode/owner",
            path.display()
        )));
    }
    // This reader also serves live progress. The descriptor length fixes this
    // observation prefix; only the external capture owner proves finality.
    let modified = metadata.modified().map_err(|error| {
        ServiceError::AgentOutputMissing(io_msg("read event modification time", path, &error))
    })?;
    let last_event_at_unix = modified
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| {
            ServiceError::AgentOutputMissing(format!(
                "event modification time predates the Unix epoch: {error}"
            ))
        })?
        .as_secs();
    Ok(Some(EventPrefix {
        file,
        bytes: metadata.len(),
        last_event_at_unix,
    }))
}

fn read_opened_event_snapshot<R: Read>(
    path: &Path,
    prefix: EventPrefix<R>,
) -> ServiceResult<EventSnapshot> {
    let mut reader = BufReader::new(prefix.file.take(prefix.bytes));
    let mut replay_completion = ReplayCompletion::ReachedBoundary;
    let mut contract = pinned_contract()?;
    let mut physical_line = 0usize;
    let mut record = Vec::new();
    loop {
        record.clear();
        let frame =
            match read_bounded_record(&mut reader, &mut record, path, MAX_EVENT_RECORD_BYTES) {
                Ok(frame) => frame,
                Err(cause) => {
                    let cause = cause.to_string();
                    if !record.is_empty() {
                        contract.observe_gap(
                            runtime_contract::ContractError::ValidationUnavailable(cause.clone()),
                        )?;
                    }
                    replay_completion = ReplayCompletion::ReadFailed {
                        bytes_read: prefix.bytes - reader.get_ref().limit(),
                        cause,
                    };
                    break;
                }
            };
        if record.is_empty() && !frame.terminated {
            break;
        }
        physical_line = physical_line.checked_add(1).ok_or_else(|| {
            ServiceError::AgentOutputMissing("events.jsonl physical line count overflowed".into())
        })?;
        if frame.over_limit {
            contract.observe_gap(runtime_contract::ContractError::InvalidRecord(format!("events.jsonl line {physical_line} exceeds the {MAX_EVENT_RECORD_BYTES}-byte record bound and is unaccounted")))?;
            if !frame.terminated {
                break;
            }
            continue;
        }
        if !frame.terminated {
            contract.observe_gap(runtime_contract::ContractError::InvalidRecord(format!("events.jsonl line {physical_line} is not newline-terminated; incomplete trailing record is unaccounted")))?;
            break;
        }
        let line = record.strip_suffix(b"\n").expect("terminated frame has LF");
        if line.iter().all(|byte| byte.is_ascii_whitespace()) {
            contract.observe_gap(runtime_contract::ContractError::InvalidRecord(format!(
                "events.jsonl line {physical_line} is blank; every completed line must be a JSON object"
            )))?;
            continue;
        }
        // A refusal latches inside the owner. Later records can contribute only
        // their independently valid observations, never a recovered certificate.
        if let Ok(mut admission) = contract.prepare_utf8(line, physical_line) {
            contract.commit(&mut admission)?;
        }
    }
    if matches!(replay_completion, ReplayCompletion::ReachedBoundary)
        && reader.get_ref().limit() != 0
    {
        replay_completion = ReplayCompletion::SourceShortened {
            bytes_read: prefix.bytes - reader.get_ref().limit(),
            expected_bytes: prefix.bytes,
        };
    }
    let facts = contract.snapshot().observations;
    let observed = OutputProgress {
        output_event_bytes: prefix.bytes,
        last_event_at_unix: Some(prefix.last_event_at_unix),
        num_turns: facts.num_turns,
        observed_usage: facts.observed_usage,
        observed_subagent_scope_count: facts.observed_subagent_scope_count,
        observed_unaccounted_records: facts.observed_unaccounted_records,
    };
    let protocol_result = contract.finish().map_err(ServiceError::from);
    let replay_cause = match &replay_completion {
        ReplayCompletion::ReachedBoundary => None,
        ReplayCompletion::ReadFailed { cause, .. } => Some(format!("captured wire replay could not finish reading: {cause}")),
        ReplayCompletion::SourceShortened { bytes_read, expected_bytes } => Some(format!("events.jsonl became shorter than its opened snapshot: read {bytes_read} of {expected_bytes} bytes")),
    };
    let certified = match (protocol_result, replay_cause) {
        (result, None) => result,
        (Ok(_), Some(cause)) => Err(ServiceError::AgentOutputMissing(cause)),
        (Err(protocol), Some(replay)) => Err(ServiceError::AgentOutputMissing(format!(
            "{protocol}; {replay}"
        ))),
    };
    Ok(EventSnapshot {
        observed,
        replay_completion,
        certified,
    })
}

struct RecordFrame {
    terminated: bool,
    over_limit: bool,
}

/// Over-limit records are drained to their delimiter without allocating their
/// payload. Their refusal cannot erase complete observations on either side.
fn read_bounded_record<R: BufRead>(
    reader: &mut R,
    record: &mut Vec<u8>,
    path: &Path,
    limit: usize,
) -> ServiceResult<RecordFrame> {
    let mut over_limit = false;
    loop {
        let available = reader.fill_buf().map_err(|error| {
            ServiceError::AgentOutputMissing(io_msg("read opened events.jsonl", path, &error))
        })?;
        if available.is_empty() {
            return Ok(RecordFrame {
                terminated: false,
                over_limit,
            });
        }
        let take = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |index| index + 1);
        if !over_limit {
            if take > limit.saturating_sub(record.len()) {
                over_limit = true;
                // Keep a bounded nonempty prefix so EOF differs from no record.
                record.extend_from_slice(&available[..take.min(512)]);
                record.truncate(512);
            } else {
                record.extend_from_slice(&available[..take]);
            }
        }
        let terminated = available.get(take.saturating_sub(1)) == Some(&b'\n');
        reader.consume(take);
        if terminated {
            return Ok(RecordFrame {
                terminated,
                over_limit,
            });
        }
    }
}

#[cfg(test)]
mod tests;
