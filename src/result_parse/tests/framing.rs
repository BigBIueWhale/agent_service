use super::*;

#[test]
fn read_failure_keeps_observations_and_independent_protocol_and_io_causes() {
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
    let complete = Trace::ordinary();
    complete.certify();
    let prefix = frames(&complete.rows[..complete.rows.len() - 1]);
    let terminal = frames(&complete.rows[complete.rows.len() - 1..]);
    for earlier_fault in [false, true] {
        for partial_bytes in [0, 7] {
            let before = format!("{prefix}{}", if earlier_fault { "not JSON\n" } else { "" });
            let all = format!("{before}{terminal}");
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
            assert_eq!(observed_usage(&snapshot), complete.summary(None));
            assert_eq!(snapshot.observed.num_turns, None);
            assert_eq!(snapshot.observed.output_event_bytes, all.len() as u64);
            assert_eq!(
                snapshot.observed.observed_unaccounted_records,
                u64::from(earlier_fault) + u64::from(partial_bytes != 0)
            );
            let cause = snapshot.certified.unwrap_err().to_string();
            assert!(cause.contains("os error 5"), "{cause}");
            assert!(!cause.contains("became shorter"), "{cause}");
            if earlier_fault {
                assert!(
                    cause.contains(&format!("line {}", prefix.lines().count() + 1)),
                    "{cause}"
                );
            }
        }
    }
}

#[test]
fn shortening_opened_source_cannot_certify_a_complete_retained_stream() {
    let complete = Trace::delegated();
    complete.certify();
    let retained = complete.text();
    let all = format!("{retained}not JSON\n");
    let path = std::env::temp_dir().join(format!(
        "agent-service-shortened-{}.jsonl",
        uuid::Uuid::new_v4()
    ));
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&path)
        .unwrap();
    if unsafe { libc::geteuid() } == 0 {
        std::os::unix::fs::fchown(&file, Some(1000), Some(1000)).unwrap();
    }
    file.write_all(all.as_bytes()).unwrap();
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
    assert_eq!(observed_usage(&snapshot), complete.summary(None));
    assert_eq!(snapshot.observed.observed_unaccounted_records, 0);
    assert!(snapshot
        .certified
        .unwrap_err()
        .to_string()
        .contains("became shorter"));
}

#[test]
fn torn_tail_preserves_both_scope_observations_without_a_complete_result() {
    let complete = Trace::delegated();
    complete.certify();
    let prefix = frames(&complete.rows[..complete.rows.len() - 1]);
    for tail in [
        "{\"data\":\"".to_string() + &"思".repeat(100_000),
        complete.rows.last().unwrap().to_string(),
    ] {
        let text = format!("{prefix}{tail}");
        let snapshot = snapshot_text(&text).unwrap();
        assert_eq!(observed_usage(&snapshot), complete.summary(None));
        assert_eq!(snapshot.observed.num_turns, None);
        assert_eq!(snapshot.observed.observed_subagent_scope_count, 1);
        assert_eq!(snapshot.observed.observed_unaccounted_records, 1);
        assert_eq!(snapshot.observed.output_event_bytes, text.len() as u64);
        assert!(snapshot
            .certified
            .unwrap_err()
            .to_string()
            .contains("incomplete trailing record"));
    }
}

/// A reader that wants observations and no certificate makes the certifying
/// scan without its physical replay, and observes exactly what it observes.
#[test]
fn observations_are_the_certifying_scan_without_its_replay() {
    let trace = Trace::delegated();
    trace.certify();
    let path = owned_event_file(trace.text().as_bytes());
    let observed = read_event_observations(&path).unwrap().unwrap();
    let snapshot = read_event_snapshot(&path).unwrap().unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(observed, snapshot.observed);
    assert_eq!(observed.num_turns, Some(1));
    assert!(read_event_observations(&path).unwrap().is_none());
}

#[test]
fn complete_result_and_observations_come_from_the_same_snapshot() {
    let snapshot = Trace::delegated().snapshot();
    let result = snapshot.certified.unwrap();
    assert_eq!(snapshot.observed.num_turns, Some(result.num_turns));
    assert_eq!(snapshot.observed.observed_usage, result.usage);
    assert_eq!(snapshot.observed.observed_unaccounted_records, 0);
}

#[test]
fn unreadable_records_preserve_physical_observations_on_both_sides() {
    let mut trace = Trace::new();
    trace.chat(None, "child");
    let split = trace.rows.len();
    trace.utility("child", ordinary_usage());
    trace.notice(Some("child"), "Child scope notice.");
    trace.terminal(None, 1, None);
    trace.certify();
    for damage in [
        b"{bad-json}\n[]\n".as_slice(),
        b"{\"text\":\"\xff\"}\n",
        b"\n",
        b" \t\r\n",
    ] {
        let mut bytes = frames(&trace.rows[..split]).into_bytes();
        bytes.extend_from_slice(damage);
        bytes.extend_from_slice(frames(&trace.rows[split..]).as_bytes());
        let snapshot = snapshot_bytes(&bytes).unwrap();
        assert_eq!(observed_usage(&snapshot), trace.summary(None));
        assert_eq!(snapshot.observed.num_turns, Some(1));
        assert_eq!(snapshot.observed.observed_subagent_scope_count, 1);
        assert_eq!(
            snapshot.observed.observed_unaccounted_records,
            damage.iter().filter(|byte| **byte == b'\n').count() as u64
        );
        assert_eq!(snapshot.observed.output_event_bytes, bytes.len() as u64);
        assert!(snapshot.certified.is_err());
    }
}

#[test]
fn every_nonempty_unterminated_suffix_is_uncertified_even_when_parseable() {
    let complete = Trace::ordinary();
    complete.certify();
    for tail in [b"{}".as_slice(), b" \t\r", b"null", b"{\"text\":\"\xe6\x80"] {
        let mut bytes = complete.text().into_bytes();
        bytes.extend_from_slice(tail);
        let snapshot = snapshot_bytes(&bytes).unwrap();
        assert_eq!(observed_usage(&snapshot), complete.summary(None));
        assert_eq!(snapshot.observed.num_turns, Some(1));
        assert_eq!(snapshot.observed.observed_unaccounted_records, 1);
        assert_eq!(snapshot.observed.output_event_bytes, bytes.len() as u64);
        assert!(snapshot
            .certified
            .unwrap_err()
            .to_string()
            .contains("not newline-terminated"));
    }
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
fn full_snapshot_drains_oversized_records_without_losing_adjacent_usage() {
    let mut trace = Trace::new();
    trace.chat(None, "child");
    let first = trace.summary(None);
    let split = trace.rows.len();
    trace.utility("child", ordinary_usage());
    trace.notice(Some("child"), "Child scope notice.");
    trace.terminal(None, 1, None);
    trace.certify();
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
            file.write_all(frames(&trace.rows[..split]).as_bytes())
                .unwrap();
        }
        let block = [b'x'; 8192];
        for _ in 0..MAX_EVENT_RECORD_BYTES / block.len() {
            file.write_all(&block).unwrap();
        }
        file.write_all(b"x").unwrap();
        if terminated {
            file.write_all(b"\n").unwrap();
            file.write_all(
                frames(if initialized {
                    &trace.rows[split..]
                } else {
                    &trace.rows
                })
                .as_bytes(),
            )
            .unwrap();
        }
        file.sync_all().unwrap();
        if unsafe { libc::geteuid() } == 0 {
            std::os::unix::fs::chown(&path, Some(1000), Some(1000)).unwrap();
        }
        let expected_bytes = file.metadata().unwrap().len();
        drop(file);
        let snapshot = read_event_snapshot(&path).unwrap().unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(snapshot.observed.output_event_bytes, expected_bytes);
        if initialized {
            assert_eq!(
                observed_usage(&snapshot),
                if terminated {
                    trace.summary(None)
                } else {
                    first.clone()
                }
            );
            assert_eq!(
                snapshot.observed.num_turns,
                if terminated { Some(1) } else { None }
            );
            assert_eq!(
                snapshot.observed.observed_subagent_scope_count,
                u64::from(terminated)
            );
            assert_eq!(snapshot.observed.observed_unaccounted_records, 1);
        } else {
            assert_eq!(
                snapshot.observed.observed_usage,
                GenerationUsageSummary::default()
            );
            assert_eq!(snapshot.observed.num_turns, None);
            assert_eq!(snapshot.observed.observed_subagent_scope_count, 0);
            assert_eq!(
                snapshot.observed.observed_unaccounted_records,
                1 + trace.rows.len() as u64
            );
        }
        assert!(snapshot
            .certified
            .unwrap_err()
            .to_string()
            .contains("record bound"));
    }
}

#[test]
fn malformed_and_uninitialized_streams_do_not_certify() {
    for bytes in [
        b"not-json\n".as_slice(),
        b"{\"type\":\"system\",\"subtype\":\"init\",\"uuid\":\"u1\"}\n",
        b"{\"type\":\"stream_event\",\"uuid\":\"u1\",\"session_id\":\"a\"}\n",
    ] {
        assert!(snapshot_bytes(bytes).unwrap().certified.is_err());
    }
    assert!(Trace::new().snapshot().certified.is_err());
}
