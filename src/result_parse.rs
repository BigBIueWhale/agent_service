//! Descriptor and LF framing for captured agent output. The shared pure
//! runtime_contract owner decides record identity, admission and observations.
use crate::error::{io_msg, ServiceError, ServiceResult};
pub use runtime_contract::runtime::{AgentResult, AgentScope, ERROR_SUBTYPES, SUCCESS_SUBTYPE};
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
    pub num_turns: u64,
    pub last_event_at_unix: Option<u64>,
    pub output_event_bytes: u64,
    pub observed_output_tokens: u64,
    pub observed_reasoning_tokens: u64,
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
        observed_output_tokens: facts.observed_output_tokens,
        observed_reasoning_tokens: facts.observed_reasoning_tokens,
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
mod tests {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    use super::*;
    fn observe_record(
        object: &serde_json::Map<String, serde_json::Value>,
    ) -> runtime_contract::runtime::ObservedRecord {
        let bytes = serde_json::to_vec(object).unwrap();
        let document = Document::decode(
            &bytes,
            Limits {
                bytes: bytes.len(),
                nodes: bytes.len(),
                depth: bytes.len(),
            },
        )
        .unwrap();
        runtime_contract::runtime::observe_record(
            document.root(),
            ValidationLimits {
                operations: u32::MAX as usize,
            },
        )
    }

    const INIT: &str = concat!("{\"type\":\"system\",\"subtype\":\"init\",\"stream_contract_sha256\":\"", env!("STREAM_CONTRACT_SHA256"), "\",\"uuid\":\"u1\",\"session_id\":\"a\",\"cwd\":\"/workspace\",\"tools\":[\"agent\",\"edit\",\"glob\",\"grep_search\",\"list_directory\",\"notebook_edit\",\"read_file\",\"run_shell_command\",\"todo_write\",\"write_file\"],\"mcp_servers\":[],\"model\":\"qwen3.8-27b-nvfp4-k8v4\",\"permission_mode\":\"yolo\",\"slash_commands\":[],\"qwen_code_version\":\"0.21.12\",\"agents\":[\"general-purpose\",\"Explore\"]}\n");

    // One completed main turn that issues the delegating tool_use, one
    // completed subagent turn under that tool call, the subagent's own
    // terminal record, and the session's own terminal record. The subagent
    // turn is deliberately billed so that only the scope rule can exclude it
    // from the main-turn count, and the subagent scope resolves only because
    // the main turn issued the tool_use id its events name.
    const MAIN_TURN: &str = "{\"type\":\"assistant\",\"uuid\":\"u2\",\"session_id\":\"a\",\"parent_tool_use_id\":null,\"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"chatcmpl-tool-9d45d85b\",\"name\":\"agent\",\"input\":{}}],\"usage\":{\"input_tokens\":42,\"output_tokens\":9,\"reasoning_output_tokens\":6,\"cache_read_input_tokens\":0,\"total_tokens\":51}}}\n";
    const SUBAGENT_TURN: &str = "{\"type\":\"assistant\",\"uuid\":\"u3\",\"session_id\":\"a\",\"parent_tool_use_id\":\"chatcmpl-tool-9d45d85b\",\"message\":{\"usage\":{\"input_tokens\":11,\"output_tokens\":9,\"reasoning_output_tokens\":6,\"cache_read_input_tokens\":0,\"total_tokens\":20}}}\n";
    const SUBAGENT_RESULT: &str = "{\"type\":\"result\",\"subtype\":\"error_during_execution\",\"uuid\":\"u4\",\"session_id\":\"a\",\"parent_tool_use_id\":\"chatcmpl-tool-9d45d85b\",\"is_error\":true,\"duration_ms\":0,\"duration_api_ms\":0,\"num_turns\":3,\"usage\":{\"requests\":1,\"usageReports\":1,\"unfinalizedRequests\":0,\"unreportedUsageRequests\":0,\"usage\":{\"promptTokenCount\":42,\"candidatesTokenCount\":9,\"cachedContentTokenCount\":0,\"thoughtsTokenCount\":6,\"totalTokenCount\":51}},\"permission_denials\":[],\"error\":{\"message\":\"MAX_TURNS\"}}\n";
    const MAIN_RESULT: &str = "{\"type\":\"result\",\"subtype\":\"success\",\"uuid\":\"u5\",\"session_id\":\"a\",\"is_error\":false,\"duration_ms\":2,\"duration_api_ms\":1,\"num_turns\":1,\"result\":\"ok\",\"usage\":{\"requests\":1,\"usageReports\":1,\"unfinalizedRequests\":0,\"unreportedUsageRequests\":0,\"usage\":{\"promptTokenCount\":42,\"candidatesTokenCount\":9,\"cachedContentTokenCount\":0,\"thoughtsTokenCount\":6,\"totalTokenCount\":51}},\"permission_denials\":[]}\n";

    fn snapshot_text(text: &str) -> ServiceResult<EventSnapshot> {
        snapshot_bytes(text.as_bytes())
    }

    fn snapshot_bytes(bytes: &[u8]) -> ServiceResult<EventSnapshot> {
        let path = std::env::temp_dir().join(format!(
            "agent-service-result-{}.jsonl",
            uuid::Uuid::new_v4().simple()
        ));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .expect("test creates private event file");
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .expect("test writes and syncs event file");
        if unsafe { libc::geteuid() } == 0 {
            std::os::unix::fs::chown(&path, Some(1000), Some(1000))
                .expect("construct exact production event owner");
        }
        let result =
            read_event_snapshot(&path).map(|snapshot| snapshot.expect("test event file exists"));
        std::fs::remove_file(path).expect("test removes temp event file");
        result
    }

    fn parse_text(text: &str) -> ServiceResult<AgentResult> {
        snapshot_text(text)?.certified
    }

    #[test]
    fn read_failure_keeps_observed_usage_and_independent_protocol_and_io_causes() {
        struct FailingRead<'a> {
            remaining: &'a [u8],
        }
        impl Read for FailingRead<'_> {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                if self.remaining.is_empty() {
                    return Err(std::io::Error::from_raw_os_error(libc::EIO));
                }
                self.remaining.read(buffer)
            }
        }
        for earlier_fault in [false, true] {
            for partial_bytes in [0, 7] {
                let before = format!(
                    "{INIT}{MAIN_TURN}{}",
                    if earlier_fault { "not JSON\n" } else { "" }
                );
                let all = format!("{before}{MAIN_RESULT}");
                let end = before.len() + partial_bytes;
                let snapshot = read_opened_event_snapshot(
                    Path::new("faulted-events.jsonl"),
                    EventPrefix {
                        file: FailingRead {
                            remaining: &all.as_bytes()[..end],
                        },
                        bytes: all.len() as u64,
                        last_event_at_unix: 123,
                    },
                )
                .unwrap();
                assert!(matches!(&snapshot.replay_completion,
                    ReplayCompletion::ReadFailed { bytes_read, cause }
                    if *bytes_read == end as u64 && cause.contains("os error 5")));
                assert_eq!(snapshot.observed.observed_output_tokens, 9);
                assert_eq!(snapshot.observed.observed_reasoning_tokens, 6);
                assert_eq!(snapshot.observed.output_event_bytes, all.len() as u64);
                assert_eq!(
                    snapshot.observed.observed_unaccounted_records,
                    u64::from(earlier_fault) + u64::from(partial_bytes != 0)
                );
                let cause = snapshot.certified.unwrap_err().to_string();
                assert!(cause.contains("os error 5"), "{cause}");
                assert!(!cause.contains("became shorter"), "{cause}");
                if earlier_fault {
                    assert!(cause.contains("line 3"), "{cause}");
                }
            }
        }
    }

    #[test]
    fn shortening_opened_source_cannot_certify_even_a_complete_retained_stream() {
        let path = std::env::temp_dir().join(format!(
            "agent-service-shortened-{}.jsonl",
            uuid::Uuid::new_v4()
        ));
        let retained = format!("{INIT}{MAIN_TURN}{SUBAGENT_TURN}{SUBAGENT_RESULT}{MAIN_RESULT}");
        let all = format!("{retained}not JSON\n");
        let file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        use std::os::unix::fs::fchown;
        fchown(&file, Some(1000), Some(1000)).unwrap();
        (&file).write_all(all.as_bytes()).unwrap();
        let prefix = open_event_prefix(&path).unwrap().unwrap();
        file.set_len(retained.len() as u64).unwrap();
        let snapshot = read_opened_event_snapshot(&path, prefix).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            snapshot.replay_completion,
            ReplayCompletion::SourceShortened {
                bytes_read: retained.len() as u64,
                expected_bytes: all.len() as u64,
            }
        );
        assert_eq!(snapshot.observed.observed_output_tokens, 18);
        assert_eq!(snapshot.observed.observed_unaccounted_records, 0);
        assert!(snapshot
            .certified
            .unwrap_err()
            .to_string()
            .contains("became shorter"));
    }

    #[test]
    fn torn_tail_preserves_both_scope_observations_without_a_complete_result() {
        for tail in [
            "{\"data\":\"".to_string() + &"思".repeat(100_000),
            MAIN_RESULT.trim_end().to_string(),
        ] {
            let text = format!("{INIT}{MAIN_TURN}{SUBAGENT_TURN}{SUBAGENT_RESULT}{tail}");
            let snapshot = snapshot_text(&text).unwrap();
            assert!(snapshot
                .certified
                .unwrap_err()
                .to_string()
                .contains("incomplete trailing record"));
            assert_eq!(snapshot.observed.num_turns, 1);
            assert_eq!(snapshot.observed.observed_output_tokens, 18);
            assert_eq!(snapshot.observed.observed_reasoning_tokens, 12);
            assert_eq!(snapshot.observed.observed_subagent_scope_count, 1);
            assert_eq!(snapshot.observed.observed_unaccounted_records, 1);
            assert_eq!(snapshot.observed.output_event_bytes, text.len() as u64);
        }
    }

    #[test]
    fn complete_result_and_observations_come_from_the_same_snapshot() {
        let snapshot = snapshot_text(&format!(
            "{INIT}{MAIN_TURN}{SUBAGENT_TURN}{SUBAGENT_RESULT}{MAIN_RESULT}"
        ))
        .unwrap();
        let result = snapshot.certified.unwrap();
        assert_eq!(snapshot.observed.num_turns, result.num_turns);
        assert_eq!(
            snapshot.observed.observed_output_tokens,
            result.main_output_tokens + result.scopes[0].output_tokens
        );
        assert_eq!(snapshot.observed.observed_unaccounted_records, 0);
    }

    #[test]
    fn unreadable_records_remain_unaccounted_between_readable_records() {
        let text = format!(
            "{INIT}{MAIN_TURN}{{bad-json}}\n[]{newline}{SUBAGENT_TURN}{MAIN_RESULT}",
            newline = "\n"
        );
        let snapshot = snapshot_text(&text).unwrap();
        assert!(snapshot.certified.is_err());
        assert_eq!(snapshot.observed.observed_output_tokens, 18);
        assert_eq!(snapshot.observed.observed_reasoning_tokens, 12);
        assert_eq!(snapshot.observed.observed_unaccounted_records, 2);
    }

    #[test]
    fn invalid_served_counts_cannot_increment_observed_completed_turns() {
        let invalid = MAIN_TURN.replace(
            "\"reasoning_output_tokens\":6",
            "\"reasoning_output_tokens\":10",
        );
        let snapshot = snapshot_text(&format!("{INIT}{invalid}{SUBAGENT_TURN}")).unwrap();
        assert!(snapshot.certified.is_err());
        assert_eq!(snapshot.observed.num_turns, 0);
        assert_eq!(snapshot.observed.observed_output_tokens, 9);
        assert_eq!(snapshot.observed.observed_unaccounted_records, 1);
    }

    #[test]
    fn oversized_record_is_drained_without_consuming_the_next_record() {
        for ending in ["\n{}\n", ""] {
            let bytes = format!("{}{}", "x".repeat(10_000), ending);
            let mut reader = BufReader::with_capacity(37, std::io::Cursor::new(bytes));
            let mut record = Vec::new();
            let frame =
                read_bounded_record(&mut reader, &mut record, Path::new("test-owned"), 64).unwrap();
            assert!(frame.over_limit);
            assert_eq!(frame.terminated, !ending.is_empty());
            assert!(record.len() <= 512);
            record.clear();
            let next =
                read_bounded_record(&mut reader, &mut record, Path::new("test-owned"), 64).unwrap();
            assert!(!next.over_limit);
            assert_eq!(
                record,
                if ending.is_empty() {
                    b"".as_slice()
                } else {
                    b"{}\n".as_slice()
                }
            );
        }
    }

    #[test]
    fn served_zero_and_unavailable_usage_have_distinct_shared_meanings() {
        let mut record = serde_json::json!({
            "type": "assistant", "parent_tool_use_id": null,
            "uuid": "observed", "session_id": "a",
            "message": { "usage": {
                "input_tokens": 0, "output_tokens": 0,
                "cache_read_input_tokens": 0, "reasoning_output_tokens": 0,
                "total_tokens": 0
            }}
        });
        let observed = observe_record(record.as_object().unwrap());
        assert!(observed.main_turn);
        assert_eq!(observed.usage, Some((0, 0)));
        assert!(!observed.usage_unreadable);
        record["message"]["usage"] = serde_json::Value::Null;
        let observed = observe_record(record.as_object().unwrap());
        assert!(!observed.main_turn);
        assert_eq!(observed.usage, None);
        assert!(!observed.usage_unreadable);
        for malformed in [
            serde_json::json!({}),
            serde_json::json!("0"),
            serde_json::json!({
                "input_tokens": 0, "output_tokens": 0, "cache_read_input_tokens": 0,
                "reasoning_output_tokens": 0, "total_tokens": 1
            }),
        ] {
            record["message"]["usage"] = malformed;
            assert!(observe_record(record.as_object().unwrap()).usage_unreadable);
        }
    }

    #[test]
    fn accepts_one_terminal_success() {
        let text = format!(
            "{INIT}{{\"type\":\"assistant\",\"uuid\":\"u2\",\"session_id\":\"a\",\"parent_tool_use_id\":null,\"message\":{{\"usage\":{{\"input_tokens\":42,\"output_tokens\":9,\"reasoning_output_tokens\":6,\"cache_read_input_tokens\":0,\"total_tokens\":51}}}}}}\n{{\"type\":\"result\",\"subtype\":\"success\",\"uuid\":\"u3\",\"session_id\":\"a\",\"is_error\":false,\"duration_ms\":2,\"duration_api_ms\":1,\"num_turns\":1,\"result\":\"ok\",\"usage\":{{\"requests\":1,\"usageReports\":1,\"unfinalizedRequests\":0,\"unreportedUsageRequests\":0,\"usage\":{{\"promptTokenCount\":42,\"candidatesTokenCount\":9,\"cachedContentTokenCount\":0,\"thoughtsTokenCount\":6,\"totalTokenCount\":51}}}},\"permission_denials\":[]}}\n"
        );
        let parsed = parse_text(&text).expect("strict valid stream parses");
        assert_eq!(parsed.response, "ok");
        assert_eq!(parsed.num_turns, 1);
        assert_eq!(snapshot_text(&text).unwrap().observed.num_turns, 1);
        // A run that delegated nothing reports no scopes, not a fabricated
        // main-scope row: the main session is accounting, not a subagent.
        assert!(parsed.scopes.is_empty());
    }

    #[test]
    fn a_subagent_result_does_not_terminate_the_session() {
        // A foreground subagent that exhausts its inherited turn budget emits
        // its own result under its agent tool-call id, and the parent then
        // recovers and finishes the session normally.
        let text = format!("{INIT}{MAIN_TURN}{SUBAGENT_TURN}{SUBAGENT_RESULT}{MAIN_RESULT}");
        let parsed = parse_text(&text)
            .expect("a subagent's result belongs to the subagent, not to the session");
        assert!(!parsed.is_error);
        assert_eq!(parsed.response, "ok");
        // The subagent's billed turn is the subagent's own, so the session's
        // main-turn count and its terminal cross-check are unchanged by it.
        assert_eq!(parsed.num_turns, 1);
        assert_eq!(snapshot_text(&text).unwrap().observed.num_turns, 1);
        // The same events that used to be validated and discarded are now the
        // scope's account: the spawning call, the turn billed under it, and
        // the terminal record it reported for itself — including the reported
        // num_turns of 3 that legitimately disagrees with the 1 billed turn.
        assert_eq!(parsed.scopes.len(), 1);
        let scope = &parsed.scopes[0];
        assert_eq!(scope.tool_use_id, "chatcmpl-tool-9d45d85b");
        assert_eq!(scope.tool_name, "agent");
        assert_eq!(scope.billed_turns, 1);
        assert_eq!(scope.reported_num_turns, Some(3));
        assert_eq!(scope.is_error, Some(true));
        assert_eq!(scope.subtype.as_deref(), Some("error_during_execution"));
        assert_eq!(scope.error_message.as_deref(), Some("MAX_TURNS"));
    }

    #[test]
    fn reports_an_error_that_ended_the_run_inside_a_turn() {
        // Test A: a stream error killed turn 236, so the terminal result
        // counts 236 started turns against 235 billed ones. The +1 is the
        // defined semantic, not corruption, and refusing the capture over it
        // hid the real cause behind "agent_output_missing" and threw away the
        // run's real timings.
        let error_result = "{\"type\":\"result\",\"subtype\":\"error_during_execution\",\"uuid\":\"u5\",\"session_id\":\"a\",\"is_error\":true,\"duration_ms\":19180714,\"duration_api_ms\":18037819,\"num_turns\":2,\"usage\":{\"requests\":1,\"usageReports\":1,\"unfinalizedRequests\":0,\"unreportedUsageRequests\":0,\"usage\":{\"promptTokenCount\":42,\"candidatesTokenCount\":9,\"cachedContentTokenCount\":0,\"thoughtsTokenCount\":6,\"totalTokenCount\":51}},\"permission_denials\":[],\"error\":{\"message\":\"[API Error: Connection error.]\"}}\n";
        let text = format!("{INIT}{MAIN_TURN}{error_result}");
        let parsed = parse_text(&text).expect("an error result may leave its final turn unbilled");
        assert!(parsed.is_error);
        assert!(parsed.response.contains("Connection error"));
        assert_eq!(parsed.num_turns, 2);
        assert_eq!(snapshot_text(&text).unwrap().observed.num_turns, 1);
        // The real timings survive; they were the whole point of parsing it.
        assert_eq!(parsed.duration_ms, 19_180_714);
        // Including the API half. This run spent 18,037,819 ms of its
        // 19,180,714 ms inside model calls -- the fact that separates a
        // backend stall from a local tool loop. It was required and
        // type-checked and then dropped, so the terminal record could report
        // how long the run took but never where the time went.
        assert_eq!(parsed.api_duration_ms, 18_037_819);
    }

    #[test]
    fn a_generation_the_provider_cut_short_is_its_own_terminal_state() {
        // The agent reports the shape of the ending, not a verdict on the
        // work: this run stopped because the model's last generation was
        // severed at the output cap, which is a different fact from a failure
        // during execution and from an exhausted turn budget.
        let terminal = "{\"type\":\"result\",\"subtype\":\"error_incomplete_generation\",\"uuid\":\"u5\",\"session_id\":\"a\",\"is_error\":true,\"duration_ms\":5562113,\"duration_api_ms\":5560686,\"num_turns\":1,\"usage\":{\"requests\":1,\"usageReports\":1,\"unfinalizedRequests\":0,\"unreportedUsageRequests\":0,\"usage\":{\"promptTokenCount\":42,\"candidatesTokenCount\":9,\"cachedContentTokenCount\":0,\"thoughtsTokenCount\":6,\"totalTokenCount\":51}},\"permission_denials\":[],\"error\":{\"message\":\"Generation on turn 66 ended as MAX_TOKENS rather than the model completing its message, so its text is a cut-off prefix and this run carries no final answer.\"}}\n";
        let text = format!("{INIT}{MAIN_TURN}{terminal}");
        let parsed = parse_text(&text).expect("a severed generation is a reportable ending");
        assert!(parsed.is_error);
        assert_eq!(parsed.subtype, "error_incomplete_generation");
        assert!(parsed.response.contains("MAX_TOKENS"));
        assert_eq!(parsed.duration_ms, 5_562_113);
    }

    #[test]
    fn every_terminal_state_reaches_the_caller_under_its_own_name() {
        // A caller distinguishes the endings by this field alone, so each
        // spelling the agent can emit must survive the parse verbatim, and
        // any other spelling must be refused rather than mapped onto one of
        // them.
        let success = "{\"type\":\"result\",\"subtype\":\"success\",\"uuid\":\"u5\",\"session_id\":\"a\",\"is_error\":false,\"duration_ms\":2,\"duration_api_ms\":1,\"num_turns\":1,\"result\":\"ok\",\"usage\":{\"requests\":1,\"usageReports\":1,\"unfinalizedRequests\":0,\"unreportedUsageRequests\":0,\"usage\":{\"promptTokenCount\":42,\"candidatesTokenCount\":9,\"cachedContentTokenCount\":0,\"thoughtsTokenCount\":6,\"totalTokenCount\":51}},\"permission_denials\":[]}\n";
        let parsed =
            parse_text(&format!("{INIT}{MAIN_TURN}{success}")).expect("a completed run parses");
        assert_eq!(parsed.subtype, SUCCESS_SUBTYPE);

        for subtype in ERROR_SUBTYPES {
            let terminal = format!(
                "{{\"type\":\"result\",\"subtype\":\"{subtype}\",\"uuid\":\"u5\",\"session_id\":\"a\",\"is_error\":true,\"duration_ms\":9,\"duration_api_ms\":8,\"num_turns\":1,\"usage\":{{\"requests\":1,\"usageReports\":1,\"unfinalizedRequests\":0,\"unreportedUsageRequests\":0,\"usage\":{{\"promptTokenCount\":42,\"candidatesTokenCount\":9,\"cachedContentTokenCount\":0,\"thoughtsTokenCount\":6,\"totalTokenCount\":51}}}},\"permission_denials\":[],\"error\":{{\"message\":\"boom\"}}}}\n"
            );
            let parsed = parse_text(&format!("{INIT}{MAIN_TURN}{terminal}"))
                .expect("every defined error state parses");
            assert!(parsed.is_error);
            assert_eq!(parsed.subtype, subtype);
        }

        // Negative control: the set is closed in both directions.
        for (is_error, subtype) in [
            (true, "error_incomplete"),
            (true, "success"),
            (false, "error_incomplete_generation"),
        ] {
            let tail = if is_error {
                "\"error\":{\"message\":\"boom\"}"
            } else {
                "\"result\":\"ok\""
            };
            let terminal = format!(
                "{{\"type\":\"result\",\"subtype\":\"{subtype}\",\"uuid\":\"u5\",\"session_id\":\"a\",\"is_error\":{is_error},\"duration_ms\":9,\"duration_api_ms\":8,\"num_turns\":1,\"usage\":{{\"requests\":1,\"usageReports\":1,\"unfinalizedRequests\":0,\"unreportedUsageRequests\":0,\"usage\":{{\"promptTokenCount\":42,\"candidatesTokenCount\":9,\"cachedContentTokenCount\":0,\"thoughtsTokenCount\":6,\"totalTokenCount\":51}}}},\"permission_denials\":[],{tail}}}\n"
            );
            let error = parse_text(&format!("{INIT}{MAIN_TURN}{terminal}"))
                .expect_err("an undefined terminal state is not interpretable");
            assert!(
                error.to_string().contains("at /subtype (schema rule "),
                "unexpected refusal for {subtype:?}: {error}"
            );
        }
    }

    #[test]
    fn refuses_a_terminal_result_whose_api_duration_is_not_a_count() {
        // Carrying the value does not weaken the check that produced it: the
        // field stays required and stays fail-closed.
        for bad in [
            "\"duration_api_ms\":-1",
            "\"duration_api_ms\":\"18037819\"",
            "\"duration_api_ms\":null",
        ] {
            let terminal = format!(
                "{{\"type\":\"result\",\"subtype\":\"success\",\"uuid\":\"u5\",\"session_id\":\"a\",\"is_error\":false,\"duration_ms\":2,{bad},\"num_turns\":1,\"result\":\"ok\",\"usage\":{{\"requests\":1,\"usageReports\":1,\"unfinalizedRequests\":0,\"unreportedUsageRequests\":0,\"usage\":{{\"promptTokenCount\":42,\"candidatesTokenCount\":9,\"cachedContentTokenCount\":0,\"thoughtsTokenCount\":6,\"totalTokenCount\":51}}}},\"permission_denials\":[]}}\n"
            );
            let valid = terminal.replace(bad, "\"duration_api_ms\":1");
            parse_text(&format!("{INIT}{MAIN_TURN}{valid}"))
                .expect("the identical stream with a valid duration is certified");
            let text = format!("{INIT}{MAIN_TURN}{terminal}");
            let error =
                parse_text(&text).expect_err("duration_api_ms must be a non-negative integer");
            let expected = if bad.ends_with(":null") {
                "terminal result lacks non-negative integer duration_api_ms"
            } else {
                "at /duration_api_ms (schema rule "
            };
            assert!(error.to_string().contains(expected), "{error}");
        }
    }

    #[test]
    fn an_error_between_turns_still_has_to_balance() {
        let error_result = "{\"type\":\"result\",\"subtype\":\"error_during_execution\",\"uuid\":\"u5\",\"session_id\":\"a\",\"is_error\":true,\"duration_ms\":9,\"duration_api_ms\":8,\"num_turns\":1,\"usage\":{\"requests\":1,\"usageReports\":1,\"unfinalizedRequests\":0,\"unreportedUsageRequests\":0,\"usage\":{\"promptTokenCount\":42,\"candidatesTokenCount\":9,\"cachedContentTokenCount\":0,\"thoughtsTokenCount\":6,\"totalTokenCount\":51}},\"permission_denials\":[],\"error\":{\"message\":\"boom\"}}\n";
        let text = format!("{INIT}{MAIN_TURN}{error_result}");
        let parsed = parse_text(&text).expect("equal counts are valid for an error too");
        assert_eq!(parsed.num_turns, 1);
        assert_eq!(snapshot_text(&text).unwrap().observed.num_turns, 1);
    }

    #[test]
    fn rejects_turn_counts_outside_the_defined_window() {
        // The integrity check is unchanged everywhere it matters: a success
        // must balance exactly, an error may be short by exactly one, and
        // nothing may claim fewer turns than the stream billed.
        let success_off_by_one = "{\"type\":\"result\",\"subtype\":\"success\",\"uuid\":\"u5\",\"session_id\":\"a\",\"is_error\":false,\"duration_ms\":2,\"duration_api_ms\":1,\"num_turns\":2,\"result\":\"ok\",\"usage\":{\"requests\":1,\"usageReports\":1,\"unfinalizedRequests\":0,\"unreportedUsageRequests\":0,\"usage\":{\"promptTokenCount\":42,\"candidatesTokenCount\":9,\"cachedContentTokenCount\":0,\"thoughtsTokenCount\":6,\"totalTokenCount\":51}},\"permission_denials\":[]}\n";
        let error_off_by_two = "{\"type\":\"result\",\"subtype\":\"error_during_execution\",\"uuid\":\"u5\",\"session_id\":\"a\",\"is_error\":true,\"duration_ms\":2,\"duration_api_ms\":1,\"num_turns\":3,\"usage\":{\"requests\":1,\"usageReports\":1,\"unfinalizedRequests\":0,\"unreportedUsageRequests\":0,\"usage\":{\"promptTokenCount\":42,\"candidatesTokenCount\":9,\"cachedContentTokenCount\":0,\"thoughtsTokenCount\":6,\"totalTokenCount\":51}},\"permission_denials\":[],\"error\":{\"message\":\"boom\"}}\n";
        let error_under_count = "{\"type\":\"result\",\"subtype\":\"error_during_execution\",\"uuid\":\"u5\",\"session_id\":\"a\",\"is_error\":true,\"duration_ms\":2,\"duration_api_ms\":1,\"num_turns\":0,\"usage\":{\"requests\":1,\"usageReports\":1,\"unfinalizedRequests\":0,\"unreportedUsageRequests\":0,\"usage\":{\"promptTokenCount\":42,\"candidatesTokenCount\":9,\"cachedContentTokenCount\":0,\"thoughtsTokenCount\":6,\"totalTokenCount\":51}},\"permission_denials\":[],\"error\":{\"message\":\"boom\"}}\n";
        for terminal in [success_off_by_one, error_off_by_two, error_under_count] {
            let text = format!("{INIT}{MAIN_TURN}{terminal}");
            let error = parse_text(&text).expect_err("inconsistent turn accounting is refused");
            assert!(error.to_string().contains("is not consistent with"));
        }
    }

    #[test]
    fn rejects_a_stream_that_ends_at_a_subagent_result() {
        let text = format!("{INIT}{MAIN_TURN}{SUBAGENT_TURN}{SUBAGENT_RESULT}");
        let error = parse_text(&text).expect_err("the session itself never reported an outcome");
        assert!(error
            .to_string()
            .contains("no main-session terminal result"));
    }

    #[test]
    fn rejects_a_subagent_result_after_the_session_result() {
        let text = format!("{INIT}{MAIN_TURN}{MAIN_RESULT}{SUBAGENT_RESULT}");
        let error =
            parse_text(&text).expect_err("nothing may follow the session's own terminal result");
        assert!(error.to_string().contains("is followed by another event"));
    }

    #[test]
    fn rejects_an_orphan_subagent_scope() {
        // Without the spawning main turn, the subagent's scope id names a
        // tool_use no assistant message ever issued. Identification is by id
        // against recorded evidence, so an unresolvable scope is refused by
        // line and id — never absorbed into an "unknown subagent" bucket.
        let text = format!("{INIT}{SUBAGENT_TURN}{MAIN_RESULT}");
        let error = parse_text(&text).expect_err("an orphan scope is contradictory evidence");
        let message = error.to_string();
        assert!(message.contains("line 2"));
        assert!(message.contains("chatcmpl-tool-9d45d85b"));
        assert!(message.contains("no earlier assistant message issued"));
    }

    #[test]
    fn rejects_a_scope_spawned_only_later_in_the_stream() {
        // The tool_use the scope names does appear — but only after the
        // scoped event. The stream is causal and the parse is one forward
        // pass: an event cannot ride a delegation that has not happened yet,
        // so resolution against a later issuance is refused at the line
        // where the premature reference occurred.
        let text = format!("{INIT}{SUBAGENT_TURN}{MAIN_TURN}{MAIN_RESULT}");
        let error = parse_text(&text).expect_err("a scope cannot borrow a future tool call");
        let message = error.to_string();
        assert!(message.contains("line 2"));
        assert!(message.contains("no earlier assistant message issued"));
    }

    #[test]
    fn resolves_a_subagent_scope_by_id_alone() {
        // The delegating call carries a name this parser has never heard of.
        // That is the point: Claude Code's convention correlates scopes by
        // tool_use id, never by tool name, so the scope must resolve and the
        // name must come back as recorded evidence, not as a filter.
        let spawn = "{\"type\":\"assistant\",\"uuid\":\"u2\",\"session_id\":\"a\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"call-77\",\"name\":\"workspace_janitor\",\"input\":{}}],\"usage\":{\"input_tokens\":9,\"output_tokens\":9,\"reasoning_output_tokens\":6,\"cache_read_input_tokens\":0,\"total_tokens\":18}}}\n";
        let sub_turn_one = "{\"type\":\"assistant\",\"uuid\":\"u3\",\"session_id\":\"a\",\"parent_tool_use_id\":\"call-77\",\"message\":{\"usage\":{\"input_tokens\":5,\"output_tokens\":9,\"reasoning_output_tokens\":6,\"cache_read_input_tokens\":0,\"total_tokens\":14}}}\n";
        let sub_turn_two = "{\"type\":\"assistant\",\"uuid\":\"u4\",\"session_id\":\"a\",\"parent_tool_use_id\":\"call-77\",\"message\":{\"usage\":{\"input_tokens\":6,\"output_tokens\":9,\"reasoning_output_tokens\":6,\"cache_read_input_tokens\":0,\"total_tokens\":15}}}\n";
        let sub_result = "{\"type\":\"result\",\"subtype\":\"success\",\"uuid\":\"u5\",\"session_id\":\"a\",\"parent_tool_use_id\":\"call-77\",\"is_error\":false,\"duration_ms\":4,\"duration_api_ms\":3,\"num_turns\":5,\"result\":\"sub done\",\"usage\":{\"requests\":1,\"usageReports\":1,\"unfinalizedRequests\":0,\"unreportedUsageRequests\":0,\"usage\":{\"promptTokenCount\":42,\"candidatesTokenCount\":9,\"cachedContentTokenCount\":0,\"thoughtsTokenCount\":6,\"totalTokenCount\":51}},\"permission_denials\":[]}\n";
        let main_result = MAIN_RESULT.replace("\"uuid\":\"u5\"", "\"uuid\":\"main-result\"");
        let text = format!("{INIT}{spawn}{sub_turn_one}{sub_turn_two}{sub_result}{main_result}");
        let parsed = parse_text(&text).expect("an id-resolved scope parses");
        assert_eq!(snapshot_text(&text).unwrap().observed.num_turns, 1);
        assert_eq!(parsed.scopes.len(), 1);
        let scope = &parsed.scopes[0];
        assert_eq!(scope.tool_use_id, "call-77");
        assert_eq!(scope.tool_name, "workspace_janitor");
        assert_eq!(scope.billed_turns, 2);
        // Recorded verbatim, not reconciled: the scope reports 5 started
        // turns over 2 billed ones and the stream is still valid.
        assert_eq!(scope.reported_num_turns, Some(5));
        assert_eq!(scope.is_error, Some(false));
        assert_eq!(scope.subtype.as_deref(), Some("success"));
        assert_eq!(scope.error_message, None);
    }

    #[test]
    fn accounts_two_subagent_scopes_separately() {
        let spawn = "{\"type\":\"assistant\",\"uuid\":\"u2\",\"session_id\":\"a\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"call-a\",\"name\":\"agent\",\"input\":{}},{\"type\":\"tool_use\",\"id\":\"call-b\",\"name\":\"background_probe\",\"input\":{}}],\"usage\":{\"input_tokens\":42,\"output_tokens\":9,\"reasoning_output_tokens\":6,\"cache_read_input_tokens\":0,\"total_tokens\":51}}}\n";
        let a_turn = "{\"type\":\"assistant\",\"uuid\":\"u3\",\"session_id\":\"a\",\"parent_tool_use_id\":\"call-a\",\"message\":{\"usage\":{\"input_tokens\":3,\"output_tokens\":9,\"reasoning_output_tokens\":6,\"cache_read_input_tokens\":0,\"total_tokens\":12}}}\n";
        let b_turn_one = "{\"type\":\"assistant\",\"uuid\":\"u4\",\"session_id\":\"a\",\"parent_tool_use_id\":\"call-b\",\"message\":{\"usage\":{\"input_tokens\":4,\"output_tokens\":9,\"reasoning_output_tokens\":6,\"cache_read_input_tokens\":0,\"total_tokens\":13}}}\n";
        let b_turn_two = "{\"type\":\"assistant\",\"uuid\":\"u5\",\"session_id\":\"a\",\"parent_tool_use_id\":\"call-b\",\"message\":{\"usage\":{\"input_tokens\":5,\"output_tokens\":9,\"reasoning_output_tokens\":6,\"cache_read_input_tokens\":0,\"total_tokens\":14}}}\n";
        let a_result = "{\"type\":\"result\",\"subtype\":\"error_during_execution\",\"uuid\":\"u6\",\"session_id\":\"a\",\"parent_tool_use_id\":\"call-a\",\"is_error\":true,\"duration_ms\":1,\"duration_api_ms\":1,\"num_turns\":1,\"usage\":{\"requests\":1,\"usageReports\":1,\"unfinalizedRequests\":0,\"unreportedUsageRequests\":0,\"usage\":{\"promptTokenCount\":42,\"candidatesTokenCount\":9,\"cachedContentTokenCount\":0,\"thoughtsTokenCount\":6,\"totalTokenCount\":51}},\"permission_denials\":[],\"error\":{\"message\":\"boom-a\"}}\n";
        let main_result = MAIN_RESULT.replace("\"uuid\":\"u5\"", "\"uuid\":\"main-result\"");
        let text = format!("{INIT}{spawn}{a_turn}{b_turn_one}{b_turn_two}{a_result}{main_result}");
        let parsed = parse_text(&text).expect("independent scopes account independently");
        assert_eq!(snapshot_text(&text).unwrap().observed.num_turns, 1);
        assert_eq!(parsed.scopes.len(), 2);
        // Stream order of first appearance, and strictly separate billing.
        let first = &parsed.scopes[0];
        assert_eq!(first.tool_use_id, "call-a");
        assert_eq!(first.tool_name, "agent");
        assert_eq!(first.billed_turns, 1);
        assert_eq!(first.is_error, Some(true));
        assert_eq!(first.error_message.as_deref(), Some("boom-a"));
        // The second scope never reported: it was still running when the
        // session ended. Absent terminal fields are the only honest account
        // of that — a scope is not required to have finished to be real.
        let second = &parsed.scopes[1];
        assert_eq!(second.tool_use_id, "call-b");
        assert_eq!(second.tool_name, "background_probe");
        assert_eq!(second.billed_turns, 2);
        assert_eq!(second.reported_num_turns, None);
        assert_eq!(second.is_error, None);
        assert_eq!(second.subtype, None);
        assert_eq!(second.error_message, None);
    }

    #[test]
    fn rejects_any_event_in_a_scope_after_its_terminal_result() {
        // A scope that has reported is finished. A second result and a
        // billed turn after the scope's result are the same defect: output
        // attributed to a scope that already declared its outcome.
        let second_result = SUBAGENT_RESULT.replace("\"uuid\":\"u4\"", "\"uuid\":\"u9\"");
        let late_turn = SUBAGENT_TURN.replace("\"uuid\":\"u3\"", "\"uuid\":\"u9\"");
        for post_terminal in [second_result, late_turn] {
            let text = format!(
                "{INIT}{MAIN_TURN}{SUBAGENT_TURN}{SUBAGENT_RESULT}{post_terminal}{MAIN_RESULT}"
            );
            let error = parse_text(&text).expect_err("a reported scope accepts no further output");
            let message = error.to_string();
            assert!(message.contains("subagent scope \"chatcmpl-tool-9d45d85b\""));
            assert!(message.contains("after its terminal result at line 4"));
        }
    }

    #[test]
    fn accepts_a_subagent_that_reports_zero_turns_over_billed_ones() {
        // The terminal's reported turn count and observed billed turns are
        // independent evidence. Preserve both values under the same child
        // owner even when the report does not reconcile with the stream.
        let thrown = "{\"type\":\"result\",\"subtype\":\"error_during_execution\",\"uuid\":\"u4\",\"session_id\":\"a\",\"parent_tool_use_id\":\"chatcmpl-tool-9d45d85b\",\"is_error\":true,\"duration_ms\":2,\"duration_api_ms\":1,\"num_turns\":0,\"usage\":{\"requests\":1,\"usageReports\":1,\"unfinalizedRequests\":0,\"unreportedUsageRequests\":0,\"usage\":{\"promptTokenCount\":42,\"candidatesTokenCount\":9,\"cachedContentTokenCount\":0,\"thoughtsTokenCount\":6,\"totalTokenCount\":51}},\"permission_denials\":[],\"error\":{\"message\":\"Error: fetch failed\"}}\n";
        let text = format!("{INIT}{MAIN_TURN}{SUBAGENT_TURN}{thrown}{MAIN_RESULT}");
        let parsed = parse_text(&text).expect("a zero-turn error report is recorded, not judged");
        assert_eq!(parsed.scopes.len(), 1);
        let scope = &parsed.scopes[0];
        assert_eq!(scope.billed_turns, 1);
        assert_eq!(scope.reported_num_turns, Some(0));
        assert_eq!(scope.is_error, Some(true));
        assert_eq!(scope.error_message.as_deref(), Some("Error: fetch failed"));
    }

    #[test]
    fn rejects_a_duplicate_tool_use_id() {
        // Correlation is by id; a re-issued id would make every later scope
        // reference ambiguous, so the second issuance is refused outright
        // instead of letting first-wins or last-wins pick a scope silently.
        let spawn_twice = "{\"type\":\"assistant\",\"uuid\":\"u2\",\"session_id\":\"a\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"call-dup\",\"name\":\"agent\",\"input\":{}},{\"type\":\"tool_use\",\"id\":\"call-dup\",\"name\":\"agent\",\"input\":{}}],\"usage\":{\"input_tokens\":7,\"output_tokens\":9,\"reasoning_output_tokens\":6,\"cache_read_input_tokens\":0,\"total_tokens\":16}}}\n";
        let text = format!("{INIT}{spawn_twice}{MAIN_RESULT}");
        let error =
            parse_text(&text).expect_err("a duplicated tool_use id is ambiguity, not reuse");
        assert!(error
            .to_string()
            .contains("re-issues tool_use id \"call-dup\""));
    }

    #[test]
    fn rejects_a_scope_that_is_neither_null_nor_a_tool_call_id() {
        for malformed in ["0", "\"\"", "[]", "false", "{}"] {
            let corrupt = MAIN_TURN.replace(
                "\"parent_tool_use_id\":null",
                &format!("\"parent_tool_use_id\":{malformed}"),
            );
            let text = format!("{INIT}{corrupt}{MAIN_RESULT}");
            let error = parse_text(&text).expect_err(
                "a scope of an unexpected shape is contradictory evidence, not a subagent",
            );
            assert!(error.to_string().contains("parent_tool_use_id"));
        }
    }

    #[test]
    fn rejects_duplicate_or_post_terminal_events() {
        let result = "{\"type\":\"result\",\"subtype\":\"success\",\"uuid\":\"u2\",\"session_id\":\"a\",\"is_error\":false,\"duration_ms\":2,\"duration_api_ms\":1,\"num_turns\":0,\"result\":\"ok\",\"usage\":{\"requests\":1,\"usageReports\":1,\"unfinalizedRequests\":0,\"unreportedUsageRequests\":0,\"usage\":{\"promptTokenCount\":42,\"candidatesTokenCount\":9,\"cachedContentTokenCount\":0,\"thoughtsTokenCount\":6,\"totalTokenCount\":51}},\"permission_denials\":[]}\n";
        let second_result = result.replace("\"uuid\":\"u2\"", "\"uuid\":\"u3\"");
        let late_init = INIT.replace("\"uuid\":\"u1\"", "\"uuid\":\"u4\"");
        for later in [second_result, late_init] {
            let error = parse_text(&format!("{INIT}{result}{later}")).unwrap_err();
            assert!(error
                .to_string()
                .contains("terminal result at line 2 is followed by another event"));
        }
    }

    #[test]
    fn rejects_malformed_and_truncated_streams() {
        assert!(parse_text("not-json\n").is_err());
        assert!(parse_text(INIT).is_err());
        assert!(
            parse_text("{\"type\":\"system\",\"subtype\":\"init\",\"uuid\":\"u1\"}\n").is_err()
        );
        assert!(
            parse_text("{\"type\":\"stream_event\",\"uuid\":\"u1\",\"session_id\":\"a\"}\n")
                .is_err()
        );
        let complete_looking_but_torn = format!(
            "{INIT}{{\"type\":\"result\",\"subtype\":\"success\",\"uuid\":\"u2\",\"session_id\":\"a\",\"is_error\":false,\"duration_ms\":2,\"duration_api_ms\":1,\"num_turns\":0,\"result\":\"ok\",\"usage\":{{\"requests\":1,\"usageReports\":1,\"unfinalizedRequests\":0,\"unreportedUsageRequests\":0,\"usage\":{{\"promptTokenCount\":42,\"candidatesTokenCount\":9,\"cachedContentTokenCount\":0,\"thoughtsTokenCount\":6,\"totalTokenCount\":51}}}},\"permission_denials\":[]}}"
        );
        let error = parse_text(&complete_looking_but_torn)
            .expect_err("a terminal JSON object without its record delimiter is torn evidence");
        assert!(error.to_string().contains("not newline-terminated"));
    }

    #[test]
    fn observes_a_billed_main_turn_without_borrowing_the_strict_verdict() {
        let record = serde_json::json!({
            "type": "assistant",
            "uuid": "observed", "session_id": "a",
            "message": {"usage": {
                "input_tokens": 100, "output_tokens": 40,
                "reasoning_output_tokens": 30, "cache_read_input_tokens": 0, "total_tokens": 140}}
        });
        let observed = observe_record(record.as_object().expect("object"));
        assert!(observed.main_turn);
        assert_eq!(observed.usage, Some((40, 30)));
        assert!(!observed.usage_unreadable);
        assert_eq!(observed.subagent_scope, None);
    }

    #[test]
    fn observes_a_subagent_scope_without_counting_it_as_a_main_turn() {
        let record = serde_json::json!({
            "type": "assistant",
            "uuid": "observed", "session_id": "a",
            "parent_tool_use_id": "call_abc",
            "message": {"usage": {
                "input_tokens": 10, "output_tokens": 5,
                "reasoning_output_tokens": 1, "cache_read_input_tokens": 0, "total_tokens": 15}}
        });
        let observed = observe_record(record.as_object().expect("object"));
        assert!(!observed.main_turn);
        assert_eq!(observed.subagent_scope.as_deref(), Some("call_abc"));
        assert_eq!(observed.usage, Some((5, 1)));
    }

    #[test]
    fn reports_an_unreadable_usage_rather_than_skipping_or_refusing_it() {
        // The strict parse refuses reasoning that exceeds output. A live read
        // may not refuse a whole status request over one such record, and may
        // not quietly drop it either: it says the tally is short instead.
        let record = serde_json::json!({
            "type": "assistant",
            "uuid": "observed", "session_id": "a",
            "message": {"usage": {
                "input_tokens": 10, "output_tokens": 5,
                "reasoning_output_tokens": 9, "cache_read_input_tokens": 0, "total_tokens": 15}}
        });
        let observed = observe_record(record.as_object().expect("object"));
        assert_eq!(observed.usage, None);
        assert!(observed.usage_unreadable);
    }

    #[test]
    fn accounts_served_output_and_reasoning_per_scope() {
        // Every billed turn carries the backend's own split of its output
        // into reasoning and the rest; the parser sums it per scope, and a
        // subagent's rounds are billed to the subagent, never to the session.
        let main_turn = "{\"type\":\"assistant\",\"uuid\":\"u2\",\"session_id\":\"a\",\"parent_tool_use_id\":null,\"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"chatcmpl-tool-9d45d85b\",\"name\":\"agent\",\"input\":{}}],\"usage\":{\"input_tokens\":18204,\"output_tokens\":96,\"reasoning_output_tokens\":71,\"cache_read_input_tokens\":18176,\"total_tokens\":18300}}}\n";
        let sub_thinking = "{\"type\":\"assistant\",\"uuid\":\"u3\",\"session_id\":\"a\",\"parent_tool_use_id\":\"chatcmpl-tool-9d45d85b\",\"message\":{\"content\":[{\"type\":\"thinking\",\"thinking\":\"read the manifest first\"}],\"usage\":null}}\n";
        let sub_round_one = "{\"type\":\"assistant\",\"uuid\":\"u4\",\"session_id\":\"a\",\"parent_tool_use_id\":\"chatcmpl-tool-9d45d85b\",\"message\":{\"content\":[],\"usage\":{\"input_tokens\":900,\"output_tokens\":40,\"reasoning_output_tokens\":33,\"cache_read_input_tokens\":896,\"total_tokens\":940}}}\n";
        let sub_round_two = "{\"type\":\"assistant\",\"uuid\":\"u5\",\"session_id\":\"a\",\"parent_tool_use_id\":\"chatcmpl-tool-9d45d85b\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"three services\"}],\"usage\":{\"input_tokens\":1200,\"output_tokens\":25,\"reasoning_output_tokens\":0,\"cache_read_input_tokens\":1200,\"total_tokens\":1225}}}\n";
        let main_final = "{\"type\":\"assistant\",\"uuid\":\"u6\",\"session_id\":\"a\",\"parent_tool_use_id\":null,\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"done\"}],\"usage\":{\"input_tokens\":18400,\"output_tokens\":10,\"reasoning_output_tokens\":4,\"cache_read_input_tokens\":18300,\"total_tokens\":18410}}}\n";
        let main_result = "{\"type\":\"result\",\"subtype\":\"success\",\"uuid\":\"u7\",\"session_id\":\"a\",\"is_error\":false,\"duration_ms\":2,\"duration_api_ms\":1,\"num_turns\":2,\"result\":\"ok\",\"usage\":{\"requests\":1,\"usageReports\":1,\"unfinalizedRequests\":0,\"unreportedUsageRequests\":0,\"usage\":{\"promptTokenCount\":42,\"candidatesTokenCount\":9,\"cachedContentTokenCount\":0,\"thoughtsTokenCount\":6,\"totalTokenCount\":51}},\"permission_denials\":[]}\n";
        let text = format!(
            "{INIT}{main_turn}{sub_thinking}{sub_round_one}{sub_round_two}{main_final}{main_result}"
        );
        let parsed = parse_text(&text).expect("served splits parse");
        assert_eq!(snapshot_text(&text).unwrap().observed.num_turns, 2);
        assert_eq!(parsed.main_output_tokens, 106);
        assert_eq!(parsed.main_reasoning_tokens, 75);
        let scope = &parsed.scopes[0];
        // The zero-usage thinking fragment is not a round; the two billed
        // rounds are, and their split is the backend's.
        assert_eq!(scope.billed_turns, 2);
        assert_eq!(scope.output_tokens, 65);
        assert_eq!(scope.reasoning_tokens, 33);
    }

    #[test]
    fn refuses_a_billed_turn_without_a_served_reasoning_count() {
        // A count the stream omitted is not read as zero: the client refuses
        // a usage without it, so its absence marks a stream this service
        // does not recognise.
        let unsplit = "{\"type\":\"assistant\",\"uuid\":\"u2\",\"session_id\":\"a\",\"parent_tool_use_id\":null,\"message\":{\"usage\":{\"input_tokens\":42,\"output_tokens\":9,\"cache_read_input_tokens\":0,\"total_tokens\":51}}}\n";
        let error = parse_text(&format!("{INIT}{unsplit}{MAIN_RESULT}"))
            .expect_err("a billed turn without its reasoning split is refused");
        assert!(
            error
                .to_string()
                .contains("lacks non-negative integer reasoning_output_tokens"),
            "{error}"
        );
        let uncached = "{\"type\":\"assistant\",\"uuid\":\"u2\",\"session_id\":\"a\",\"parent_tool_use_id\":null,\"message\":{\"usage\":{\"input_tokens\":42,\"output_tokens\":9,\"reasoning_output_tokens\":6,\"total_tokens\":51}}}\n";
        let error = parse_text(&format!("{INIT}{uncached}{MAIN_RESULT}"))
            .expect_err("a billed turn without its cached-prompt count is refused");
        assert!(
            error
                .to_string()
                .contains("lacks non-negative integer cache_read_input_tokens"),
            "{error}"
        );
    }

    #[test]
    fn refuses_a_split_that_does_not_nest() {
        let inverted = "{\"type\":\"assistant\",\"uuid\":\"u2\",\"session_id\":\"a\",\"parent_tool_use_id\":null,\"message\":{\"usage\":{\"input_tokens\":42,\"output_tokens\":9,\"reasoning_output_tokens\":10,\"cache_read_input_tokens\":0,\"total_tokens\":51}}}\n";
        let error = parse_text(&format!("{INIT}{inverted}{MAIN_RESULT}"))
            .expect_err("reasoning larger than the output it is part of is refused");
        assert!(
            error
                .to_string()
                .contains("bills 10 reasoning tokens against only 9 output tokens"),
            "{error}"
        );
        let overcached = "{\"type\":\"assistant\",\"uuid\":\"u2\",\"session_id\":\"a\",\"parent_tool_use_id\":null,\"message\":{\"usage\":{\"input_tokens\":42,\"output_tokens\":9,\"reasoning_output_tokens\":6,\"cache_read_input_tokens\":43,\"total_tokens\":51}}}\n";
        let error = parse_text(&format!("{INIT}{overcached}{MAIN_RESULT}"))
            .expect_err("more cached prompt tokens than prompt tokens is refused");
        assert!(
            error
                .to_string()
                .contains("reads 43 cached prompt tokens against only 42 input tokens"),
            "{error}"
        );
    }

    fn compaction_record(output: serde_json::Value) -> String {
        format!(
            "{}\n",
            serde_json::json!({
                "type": "system", "subtype": "compaction", "uuid": "u3",
                "session_id": "a", "parent_tool_use_id": null,
                "data": {"status": "COMPRESSION_FAILED_OUTPUT_TRUNCATED", "succeeded": false,
                    "originalTokenCount": 233926, "newTokenCount": 233926,
                    "triggerReason": "token_limit", "output": output}
            })
        )
    }

    fn observed_compaction_output() -> serde_json::Value {
        serde_json::json!({
            "maxOutputTokens": 49152, "requestAttempts": 2,
            "summary": "  partial snapshot", "reasoning": "Observed reasoning.  ",
            "finishReason": "MAX_TOKENS",
            "usage": {"promptTokenCount": 233926, "candidatesTokenCount": 49152,
                "thoughtsTokenCount": 49152, "cachedContentTokenCount": 100,
                "totalTokenCount": 283078}
        })
    }

    #[test]
    fn accepts_compaction_observations_and_preflight_refusal() {
        let output = observed_compaction_output();
        let mut interrupted = output.clone();
        interrupted["usage"] = serde_json::Value::Null;
        interrupted["finishReason"] = serde_json::Value::Null;
        for observation in [output, interrupted, serde_json::Value::Null] {
            let compaction = compaction_record(observation);
            let result = MAIN_RESULT.replace("\"num_turns\":1", "\"num_turns\":0");
            parse_text(&format!("{INIT}{compaction}{result}"))
                .expect("served, interrupted, and unattempted records are distinct valid facts");
        }
    }

    #[test]
    fn refuses_incomplete_or_inconsistent_compaction_observations() {
        let output = observed_compaction_output();
        let cases = [
            (
                "reasoning",
                serde_json::Value::Null,
                "lacks the reasoning string",
            ),
            (
                "summary",
                serde_json::Value::Null,
                "lacks the summary string",
            ),
            (
                "requestAttempts",
                serde_json::json!(0),
                "positive budget or request-attempt count",
            ),
            ("usage", serde_json::json!(7), "neither an object nor null"),
        ];
        for (field, value, expected) in cases {
            let mut bad = output.clone();
            bad[field] = value;
            let error = parse_text(&format!("{INIT}{}{MAIN_RESULT}", compaction_record(bad)))
                .expect_err(field);
            assert!(error.to_string().contains(expected), "{error}");
        }
        for (field, value) in [
            ("thoughtsTokenCount", 49153),
            ("cachedContentTokenCount", 233927),
            ("totalTokenCount", 42),
        ] {
            let mut bad = output.clone();
            bad["usage"][field] = serde_json::json!(value);
            let error = parse_text(&format!("{INIT}{}{MAIN_RESULT}", compaction_record(bad)))
                .expect_err(field);
            assert!(error.to_string().contains("does not nest"), "{error}");
        }
        for field in ["usage", "requestAttempts", "finishReason"] {
            let mut bad = output.clone();
            bad.as_object_mut().unwrap().remove(field);
            parse_text(&format!("{INIT}{}{MAIN_RESULT}", compaction_record(bad)))
                .expect_err("mandatory observation fields cannot be omitted");
        }
        let mut partial = output;
        partial["usage"]
            .as_object_mut()
            .unwrap()
            .remove("promptTokenCount");
        parse_text(&format!(
            "{INIT}{}{MAIN_RESULT}",
            compaction_record(partial)
        ))
        .expect_err("partial served usage is refused");
    }

    #[test]
    fn rejects_advertised_slash_commands() {
        let unexpected = INIT.replace("\"slash_commands\":[]", "\"slash_commands\":[\"status\"]");
        assert!(parse_text(&unexpected).is_err());
    }
    #[test]
    fn every_nonempty_unterminated_suffix_is_uncertified_even_when_parseable() {
        for tail in [b"{}".as_slice(), b" \t\r", b"null", b"{\"text\":\"\xe6\x80"] {
            let mut bytes = format!("{INIT}{MAIN_TURN}{MAIN_RESULT}").into_bytes();
            bytes.extend_from_slice(tail);
            let snapshot = snapshot_bytes(&bytes).unwrap();
            assert!(snapshot.certified.is_err());
            assert_eq!(snapshot.observed.num_turns, 1);
            assert_eq!(snapshot.observed.observed_output_tokens, 9);
            assert_eq!(snapshot.observed.observed_reasoning_tokens, 6);
            assert_eq!(snapshot.observed.observed_unaccounted_records, 1);
            assert_eq!(snapshot.observed.output_event_bytes, bytes.len() as u64);
        }
    }

    #[test]
    fn invalid_utf8_middle_record_cannot_erase_readable_records_on_either_side() {
        let mut bytes = format!("{INIT}{MAIN_TURN}").into_bytes();
        bytes.extend_from_slice(b"{\"type\":\"system\",\"text\":\"\xff\"}\n");
        bytes.extend_from_slice(SUBAGENT_TURN.as_bytes());
        bytes.extend_from_slice(MAIN_RESULT.as_bytes());
        let snapshot = snapshot_bytes(&bytes).unwrap();
        assert!(snapshot.certified.is_err());
        assert_eq!(snapshot.observed.num_turns, 1);
        assert_eq!(snapshot.observed.observed_output_tokens, 18);
        assert_eq!(snapshot.observed.observed_reasoning_tokens, 12);
        assert_eq!(snapshot.observed.observed_subagent_scope_count, 1);
        assert_eq!(snapshot.observed.observed_unaccounted_records, 1);
    }

    #[test]
    fn incomplete_usage_never_increments_completed_turns_between_valid_turns() {
        let valid: serde_json::Value = serde_json::from_str(MAIN_TURN).unwrap();
        let mut middle = valid.clone();
        middle["uuid"] = serde_json::json!("middle");
        middle["message"]["content"] = serde_json::json!([]);
        let mut later = middle.clone();
        later["uuid"] = serde_json::json!("later");
        let malformed = [
            serde_json::json!({"input_tokens": 0, "output_tokens": 0}),
            serde_json::json!(0),
            serde_json::json!([]),
            serde_json::json!({"input_tokens": 42, "output_tokens": 9, "reasoning_output_tokens": 6, "cache_read_input_tokens": 0, "total_tokens": 50}),
            serde_json::json!({"input_tokens": 42, "output_tokens": 9.5, "reasoning_output_tokens": 6, "cache_read_input_tokens": 0, "total_tokens": 51.5}),
            serde_json::json!({"input_tokens": 9007199254740992u64, "output_tokens": 0, "reasoning_output_tokens": 0, "cache_read_input_tokens": 0, "total_tokens": 9007199254740992u64}),
        ];
        for usage in malformed {
            middle["message"]["usage"] = usage;
            let text = format!("{INIT}{MAIN_TURN}{middle}\n{later}\n{MAIN_RESULT}");
            let snapshot = snapshot_text(&text).unwrap();
            assert!(snapshot.certified.is_err());
            assert_eq!(snapshot.observed.num_turns, 2);
            assert_eq!(snapshot.observed.observed_output_tokens, 18);
            assert_eq!(snapshot.observed.observed_reasoning_tokens, 12);
            assert_eq!(snapshot.observed.observed_unaccounted_records, 1);
        }
        for (usage, turns) in [
            (serde_json::Value::Null, 2),
            (
                serde_json::json!({"input_tokens": 0, "output_tokens": 0, "reasoning_output_tokens": 0, "cache_read_input_tokens": 0, "total_tokens": 0}),
                3,
            ),
        ] {
            middle["message"]["usage"] = usage;
            let result = MAIN_RESULT.replace("\"num_turns\":1", &format!("\"num_turns\":{turns}"));
            let snapshot =
                snapshot_text(&format!("{INIT}{MAIN_TURN}{middle}\n{later}\n{result}")).unwrap();
            assert!(snapshot.certified.is_ok());
            assert_eq!(snapshot.observed.num_turns, turns);
            assert_eq!(snapshot.observed.observed_output_tokens, 18);
            assert_eq!(snapshot.observed.observed_unaccounted_records, 0);
        }
    }

    #[test]
    fn full_snapshot_drains_oversized_records_without_losing_adjacent_usage() {
        for (terminated, initialized) in [(false, true), (true, true), (true, false)] {
            let path = std::env::temp_dir().join(format!(
                "agent-service-overlimit-{}.jsonl",
                uuid::Uuid::new_v4()
            ));
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
                .unwrap();
            if initialized {
                file.write_all(format!("{INIT}{MAIN_TURN}").as_bytes())
                    .unwrap();
            }
            let block = [b'x'; 8192];
            for _ in 0..MAX_EVENT_RECORD_BYTES / block.len() {
                file.write_all(&block).unwrap();
            }
            file.write_all(b"x").unwrap();
            if terminated {
                let suffix = if initialized {
                    format!("\n{SUBAGENT_TURN}{MAIN_RESULT}")
                } else {
                    format!("\n{INIT}{MAIN_TURN}{MAIN_RESULT}")
                };
                file.write_all(suffix.as_bytes()).unwrap();
            }
            file.sync_all().unwrap();
            if unsafe { libc::geteuid() } == 0 {
                std::os::unix::fs::chown(&path, Some(1000), Some(1000)).unwrap();
            }
            let expected_bytes = file.metadata().unwrap().len();
            drop(file);
            let snapshot = read_event_snapshot(&path).unwrap().unwrap();
            std::fs::remove_file(&path).unwrap();
            assert!(snapshot
                .certified
                .unwrap_err()
                .to_string()
                .contains("record bound"));
            if !initialized {
                assert_eq!(snapshot.observed.num_turns, 0);
                assert_eq!(snapshot.observed.observed_output_tokens, 0);
                assert_eq!(snapshot.observed.observed_reasoning_tokens, 0);
                assert_eq!(snapshot.observed.observed_subagent_scope_count, 0);
                assert_eq!(snapshot.observed.observed_unaccounted_records, 4);
                continue;
            }
            assert_eq!(snapshot.observed.num_turns, 1);
            assert_eq!(
                snapshot.observed.observed_output_tokens,
                if terminated { 18 } else { 9 }
            );
            assert_eq!(
                snapshot.observed.observed_reasoning_tokens,
                if terminated { 12 } else { 6 }
            );
            assert_eq!(
                snapshot.observed.observed_subagent_scope_count,
                u64::from(terminated)
            );
            assert_eq!(snapshot.observed.observed_unaccounted_records, 1);
            assert_eq!(snapshot.observed.output_event_bytes, expected_bytes);
        }
    }
    #[test]
    fn invalid_or_duplicate_identity_cannot_attribute_usage_or_child_scopes() {
        let mut base: serde_json::Value = serde_json::from_str(SUBAGENT_TURN).unwrap();
        base["uuid"] = serde_json::json!("bad");
        base["parent_tool_use_id"] = serde_json::json!("unissued-child");
        let mut later: serde_json::Value = serde_json::from_str(MAIN_TURN).unwrap();
        later["uuid"] = serde_json::json!("later");
        later["message"]["content"] = serde_json::json!([]);
        for (key, replacement) in [
            ("session_id", None),
            ("session_id", Some(serde_json::json!("other-session"))),
            ("session_id", Some(serde_json::json!(""))),
            ("session_id", Some(serde_json::json!(7))),
            ("uuid", None),
            ("uuid", Some(serde_json::json!(""))),
            ("uuid", Some(serde_json::json!(7))),
            ("uuid", Some(serde_json::json!("u2"))),
        ] {
            let mut invalid = base.clone();
            if let Some(value) = replacement {
                invalid[key] = value;
            } else {
                invalid.as_object_mut().unwrap().remove(key);
            }
            let snapshot = snapshot_text(&format!(
                "{INIT}{MAIN_TURN}{invalid}\n{later}\n{MAIN_RESULT}"
            ))
            .unwrap();
            assert!(snapshot.certified.is_err());
            assert_eq!(snapshot.observed.num_turns, 2, "{invalid}");
            assert_eq!(snapshot.observed.observed_output_tokens, 18, "{invalid}");
            assert_eq!(snapshot.observed.observed_reasoning_tokens, 12, "{invalid}");
            assert_eq!(
                snapshot.observed.observed_subagent_scope_count, 0,
                "{invalid}"
            );
            assert_eq!(
                snapshot.observed.observed_unaccounted_records, 1,
                "{invalid}"
            );
        }
    }

    #[test]
    fn absent_pinned_initialization_cannot_attribute_apparent_served_usage() {
        for init in [
            String::new(),
            INIT.replace("qwen3.8-27b-nvfp4-k8v4", "other-model"),
        ] {
            let snapshot = snapshot_text(&format!("{init}{MAIN_TURN}{MAIN_RESULT}")).unwrap();
            assert!(snapshot.certified.is_err());
            assert_eq!(snapshot.observed.num_turns, 0);
            assert_eq!(snapshot.observed.observed_output_tokens, 0);
            assert_eq!(snapshot.observed.observed_reasoning_tokens, 0);
            assert_eq!(snapshot.observed.observed_subagent_scope_count, 0);
            assert_eq!(
                snapshot.observed.observed_unaccounted_records,
                if init.is_empty() { 2 } else { 3 }
            );
        }
    }

    fn reported_generation_summary() -> serde_json::Value {
        serde_json::json!({
            "requests": 1, "usageReports": 1,
            "unfinalizedRequests": 0, "unreportedUsageRequests": 0,
            "usage": {
                "promptTokenCount": 42, "candidatesTokenCount": 9,
                "cachedContentTokenCount": 0, "thoughtsTokenCount": 6,
                "totalTokenCount": 51
            }
        })
    }

    fn terminal_with_generation_summary(summary: serde_json::Value) -> String {
        let mut result: serde_json::Value = serde_json::from_str(MAIN_RESULT).unwrap();
        result["usage"] = summary;
        format!("{result}\n")
    }

    fn assert_generation_summaries_refused(cases: Vec<(String, serde_json::Value)>) {
        let mut accepted = Vec::new();
        for (label, usage) in cases {
            let terminal = terminal_with_generation_summary(usage);
            let text = format!("{INIT}{MAIN_TURN}{SUBAGENT_TURN}{SUBAGENT_RESULT}{terminal}");
            let snapshot = snapshot_text(&text).unwrap();
            assert_eq!(snapshot.observed.num_turns, 1, "{label}");
            assert_eq!(snapshot.observed.observed_output_tokens, 18, "{label}");
            assert_eq!(snapshot.observed.observed_reasoning_tokens, 12, "{label}");
            assert_eq!(
                snapshot.observed.observed_subagent_scope_count, 1,
                "{label}"
            );
            assert_eq!(
                snapshot.observed.output_event_bytes,
                text.len() as u64,
                "{label}"
            );
            if snapshot.certified.is_ok() {
                accepted.push(label);
            }
        }
        assert!(
            accepted.is_empty(),
            "invalid terminal generation summaries accepted: {accepted:?}"
        );
    }

    #[test]
    fn terminal_generation_summary_refuses_absent_and_nonobject_claims() {
        assert_generation_summaries_refused(
            [
                serde_json::Value::Null,
                serde_json::json!(0),
                serde_json::json!(false),
                serde_json::json!("usage"),
                serde_json::json!([]),
            ]
            .into_iter()
            .map(|value| (value.to_string(), value))
            .collect(),
        );
        let mut terminal: serde_json::Value = serde_json::from_str(MAIN_RESULT).unwrap();
        terminal.as_object_mut().unwrap().remove("usage");
        let snapshot = snapshot_text(&format!("{INIT}{MAIN_TURN}{terminal}\n")).unwrap();
        assert!(snapshot.certified.is_err());
        assert_eq!(snapshot.observed.observed_output_tokens, 9);
    }

    #[test]
    fn terminal_generation_summary_refuses_empty_and_flat_token_objects() {
        assert_generation_summaries_refused(vec![
            ("empty object".into(), serde_json::json!({})),
            (
                "flat counters".into(),
                serde_json::json!({
                    "input_tokens": 42, "output_tokens": 9,
                    "cache_read_input_tokens": 0, "reasoning_output_tokens": 6,
                    "total_tokens": 51, "usage_reports": 1,
                    "observations_without_usage": 0, "unfinalized_observations": 0
                }),
            ),
        ]);
    }

    #[test]
    fn terminal_generation_summary_refuses_inconsistent_request_partitions() {
        let mut cases = Vec::new();
        for (field, value) in [
            ("requests", 0),
            ("requests", 2),
            ("usageReports", 2),
            ("unfinalizedRequests", 1),
            ("unreportedUsageRequests", 1),
        ] {
            let mut summary = reported_generation_summary();
            summary[field] = serde_json::json!(value);
            cases.push((format!("{field}={value}"), summary));
        }
        assert_generation_summaries_refused(cases);
    }

    #[test]
    fn terminal_generation_summary_requires_every_safe_integer_observation_count() {
        let mut cases = Vec::new();
        for field in [
            "requests",
            "usageReports",
            "unfinalizedRequests",
            "unreportedUsageRequests",
        ] {
            let mut missing = reported_generation_summary();
            missing.as_object_mut().unwrap().remove(field);
            cases.push((format!("missing {field}"), missing));
            for value in [
                serde_json::Value::Null,
                serde_json::json!(-1),
                serde_json::json!(1.5),
                serde_json::json!("1"),
                serde_json::json!(false),
                serde_json::json!(9_007_199_254_740_992u64),
            ] {
                let mut summary = reported_generation_summary();
                summary[field] = value.clone();
                cases.push((format!("{field}={value}"), summary));
            }
        }
        assert_generation_summaries_refused(cases);
    }

    #[test]
    fn terminal_generation_summary_requires_usage_presence_to_match_report_count() {
        let mut reported_without_usage = reported_generation_summary();
        reported_without_usage["usage"] = serde_json::Value::Null;
        let mut absent_usage = reported_generation_summary();
        absent_usage.as_object_mut().unwrap().remove("usage");
        let mut known_without_reports = reported_generation_summary();
        known_without_reports["requests"] = serde_json::json!(0);
        known_without_reports["usageReports"] = serde_json::json!(0);
        let mut zero_without_reports = known_without_reports.clone();
        for value in zero_without_reports["usage"]
            .as_object_mut()
            .unwrap()
            .values_mut()
        {
            *value = serde_json::json!(0);
        }
        assert_generation_summaries_refused(vec![
            ("report without usage".into(), reported_without_usage),
            ("missing usage".into(), absent_usage),
            ("known counts without reports".into(), known_without_reports),
            ("zero counts without reports".into(), zero_without_reports),
        ]);
    }

    #[test]
    fn terminal_generation_summary_requires_full_safe_served_usage() {
        let mut cases = Vec::new();
        for field in [
            "promptTokenCount",
            "candidatesTokenCount",
            "cachedContentTokenCount",
            "thoughtsTokenCount",
            "totalTokenCount",
        ] {
            let mut missing = reported_generation_summary();
            missing["usage"].as_object_mut().unwrap().remove(field);
            cases.push((format!("missing {field}"), missing));
            for value in [
                serde_json::Value::Null,
                serde_json::json!(-1),
                serde_json::json!(0.5),
                serde_json::json!("0"),
                serde_json::json!(9_007_199_254_740_992u64),
            ] {
                let mut summary = reported_generation_summary();
                summary["usage"][field] = value.clone();
                cases.push((format!("{field}={value}"), summary));
            }
        }
        for value in [
            serde_json::json!([]),
            serde_json::json!(false),
            serde_json::json!(1),
        ] {
            let mut summary = reported_generation_summary();
            summary["usage"] = value.clone();
            cases.push((format!("usage={value}"), summary));
        }
        assert_generation_summaries_refused(cases);
    }

    #[test]
    fn terminal_generation_summary_refuses_totals_that_do_not_nest() {
        let mut cases = Vec::new();
        for (field, value) in [
            ("thoughtsTokenCount", 10),
            ("cachedContentTokenCount", 43),
            ("totalTokenCount", 50),
            ("totalTokenCount", 57),
        ] {
            let mut summary = reported_generation_summary();
            summary["usage"][field] = serde_json::json!(value);
            cases.push((format!("{field}={value}"), summary));
        }
        assert_generation_summaries_refused(cases);
    }

    #[test]
    fn terminal_generation_summary_accepts_distinct_zero_unknown_open_and_served_states() {
        let no_dispatch = serde_json::json!({"requests":0,"usageReports":0,
            "unfinalizedRequests":0,"unreportedUsageRequests":0,"usage":null});
        let mut zero = reported_generation_summary();
        for value in zero["usage"].as_object_mut().unwrap().values_mut() {
            *value = serde_json::json!(0);
        }
        let open = serde_json::json!({"requests":1,"usageReports":0,
            "unfinalizedRequests":1,"unreportedUsageRequests":0,"usage":null});
        let unknown = serde_json::json!({"requests":1,"usageReports":0,
            "unfinalizedRequests":0,"unreportedUsageRequests":1,"usage":null});
        for (label, usage, billed, failed) in [
            ("no dispatch", no_dispatch, false, false),
            ("served zero", zero, true, false),
            ("open", open, false, true),
            ("unreported", unknown, false, true),
            ("served", reported_generation_summary(), true, false),
        ] {
            let mut result: serde_json::Value = serde_json::from_str(MAIN_RESULT).unwrap();
            result["usage"] = usage;
            result["num_turns"] = serde_json::json!(u64::from(billed || failed));
            let mut assistant: serde_json::Value = serde_json::from_str(MAIN_TURN).unwrap();
            if label == "served zero" {
                for value in assistant["message"]["usage"]
                    .as_object_mut()
                    .unwrap()
                    .values_mut()
                {
                    *value = serde_json::json!(0);
                }
            }
            if failed {
                result["is_error"] = serde_json::json!(true);
                result["subtype"] = serde_json::json!("error_during_execution");
                result["error"] = serde_json::json!({"message":"generation unavailable"});
                result.as_object_mut().unwrap().remove("result");
            }
            let prefix = if billed {
                format!("{INIT}{assistant}\n")
            } else {
                INIT.to_string()
            };
            let snapshot = snapshot_text(&format!("{prefix}{result}\n")).unwrap();
            let parsed = snapshot.certified.expect(label);
            assert_eq!(parsed.is_error, failed, "{label}");
            assert_eq!(
                parsed.main_output_tokens,
                if label == "served" { 9 } else { 0 },
                "{label}"
            );
            assert_eq!(snapshot.observed.observed_unaccounted_records, 0, "{label}");
        }
    }

    #[test]
    fn terminal_generation_summary_accepts_exact_safe_integer_boundaries() {
        let max = 9_007_199_254_740_991u64;
        for usage in [
            serde_json::json!({"requests":max,"usageReports":max,
                "unfinalizedRequests":0,"unreportedUsageRequests":0,"usage":{
                    "promptTokenCount":max-1,"candidatesTokenCount":1,
                    "cachedContentTokenCount":max-1,"thoughtsTokenCount":1,"totalTokenCount":max}}),
            serde_json::json!({"requests":max,"usageReports":0,
                "unfinalizedRequests":max,"unreportedUsageRequests":0,"usage":null}),
            serde_json::json!({"requests":max,"usageReports":0,
                "unfinalizedRequests":0,"unreportedUsageRequests":max,"usage":null}),
        ] {
            let terminal = terminal_with_generation_summary(usage);
            let snapshot = snapshot_text(&format!("{INIT}{MAIN_TURN}{terminal}")).unwrap();
            let parsed = snapshot
                .certified
                .expect("each maximum is valid and independent of billed event totals");
            assert_eq!(parsed.main_output_tokens, 9);
            assert_eq!(parsed.main_reasoning_tokens, 6);
            assert_eq!(snapshot.observed.observed_unaccounted_records, 0);
        }
    }

    #[test]
    fn terminal_generation_summary_does_not_replace_independent_billed_evidence() {
        let summary = serde_json::json!({"requests":3,"usageReports":1,
            "unfinalizedRequests":1,"unreportedUsageRequests":1,"usage":{
                "promptTokenCount":100,"candidatesTokenCount":20,
                "cachedContentTokenCount":80,"thoughtsTokenCount":15,"totalTokenCount":120}});
        let terminal = terminal_with_generation_summary(summary);
        let snapshot = snapshot_text(&format!(
            "{INIT}{MAIN_TURN}{SUBAGENT_TURN}{SUBAGENT_RESULT}{terminal}"
        ))
        .unwrap();
        let parsed = snapshot.certified.unwrap();
        assert_eq!(parsed.main_output_tokens, 9);
        assert_eq!(parsed.main_reasoning_tokens, 6);
        assert_eq!(parsed.scopes[0].output_tokens, 9);
        assert_eq!(snapshot.observed.observed_output_tokens, 18);
    }

    #[test]
    fn compaction_counts_refuse_unsafe_integers_in_root_and_child_scopes() {
        let unsafe_count = 9_007_199_254_740_992u64;
        let mut accepted = Vec::new();
        for child in [false, true] {
            for field in [
                "originalTokenCount",
                "newTokenCount",
                "maxOutputTokens",
                "requestAttempts",
                "promptTokenCount",
                "candidatesTokenCount",
                "cachedContentTokenCount",
                "thoughtsTokenCount",
                "totalTokenCount",
            ] {
                let mut record: serde_json::Value =
                    serde_json::from_str(&compaction_record(observed_compaction_output())).unwrap();
                if child {
                    record["parent_tool_use_id"] = serde_json::json!("chatcmpl-tool-9d45d85b");
                }
                if ["originalTokenCount", "newTokenCount"].contains(&field) {
                    record["data"][field] = serde_json::json!(unsafe_count);
                } else if ["maxOutputTokens", "requestAttempts"].contains(&field) {
                    record["data"]["output"][field] = serde_json::json!(unsafe_count);
                } else {
                    let output = &mut record["data"]["output"];
                    let usage = &mut output["usage"];
                    match field {
                        "promptTokenCount" | "cachedContentTokenCount" => {
                            usage["promptTokenCount"] = serde_json::json!(unsafe_count);
                            usage[field] = serde_json::json!(unsafe_count);
                            usage["totalTokenCount"] = serde_json::json!(unsafe_count + 49_152);
                        }
                        "candidatesTokenCount" | "thoughtsTokenCount" => {
                            usage["candidatesTokenCount"] = serde_json::json!(unsafe_count);
                            usage[field] = serde_json::json!(unsafe_count);
                            usage["totalTokenCount"] = serde_json::json!(unsafe_count + 233_926);
                            output["maxOutputTokens"] = serde_json::json!(unsafe_count);
                        }
                        "totalTokenCount" => {
                            usage["promptTokenCount"] = serde_json::json!(unsafe_count - 1);
                            usage["candidatesTokenCount"] = serde_json::json!(1);
                            usage["thoughtsTokenCount"] = serde_json::json!(1);
                            usage["totalTokenCount"] = serde_json::json!(unsafe_count);
                        }
                        _ => unreachable!("the field table contains only served counts here"),
                    }
                }
                let snapshot =
                    snapshot_text(&format!("{INIT}{MAIN_TURN}{record}\n{MAIN_RESULT}")).unwrap();
                assert_eq!(snapshot.observed.num_turns, 1);
                assert_eq!(snapshot.observed.observed_output_tokens, 9);
                assert_eq!(snapshot.observed.observed_reasoning_tokens, 6);
                if snapshot.certified.is_ok() {
                    accepted.push(format!("{field} child={child}"));
                }
            }
        }
        assert!(
            accepted.is_empty(),
            "unsafe compaction counts accepted: {accepted:?}"
        );
    }

    #[test]
    fn compaction_counts_accept_exact_safe_integer_boundaries() {
        let max = 9_007_199_254_740_991u64;
        for child in [false, true] {
            let mut output = observed_compaction_output();
            output["maxOutputTokens"] = serde_json::json!(max);
            output["requestAttempts"] = serde_json::json!(max);
            output["usage"] = serde_json::json!({"promptTokenCount":max-1,
                "candidatesTokenCount":1,"cachedContentTokenCount":max-1,
                "thoughtsTokenCount":1,"totalTokenCount":max});
            let mut record: serde_json::Value =
                serde_json::from_str(&compaction_record(output)).unwrap();
            record["data"]["originalTokenCount"] = serde_json::json!(max);
            record["data"]["newTokenCount"] = serde_json::json!(max);
            if child {
                record["parent_tool_use_id"] = serde_json::json!("chatcmpl-tool-9d45d85b");
            }
            let snapshot =
                snapshot_text(&format!("{INIT}{MAIN_TURN}{record}\n{MAIN_RESULT}")).unwrap();
            let parsed = snapshot
                .certified
                .expect("all counts fit the shared safe-integer contract");
            assert_eq!(parsed.main_output_tokens, 9);
            assert_eq!(parsed.main_reasoning_tokens, 6);
            assert_eq!(snapshot.observed.observed_unaccounted_records, 0);
        }
    }
}
