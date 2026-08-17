# Production incidents observed while running the benchmark suite

Issues found by actually operating the production agent service under the
SWE-rebench 2026-07 paired suite. Each entry records the observable
symptom, the forensic trail, the root cause, and the fix. This file exists
because the benchmark's mandate is to evaluate performance **and record any
issues** — these are the issues.

## Incident 1 — reading a running session self-deadlocks the read

- **Observed** (first suite launch, task `ArcadeData__arcadedb-4281`,
  `preserve_thinking=false`, session
  `s-2fa664506a2bcdc615a3708099dd072e189c64c6215ee88322ed4ee5ff07c03d`):
  the very first status poll of the running session timed out after 30 s
  with zero response bytes, so the fail-closed suite driver aborted.
  `GET /healthz` answered instantly the whole time, unknown session IDs
  404'd instantly, but every `GET` of the running session and every
  collection `LIST` hung forever. The session itself was healthy: its
  containers ran, `events.jsonl` kept growing, and cancellation +
  terminalization later completed cleanly (52 agent turns).
- **Forensics**: all five service threads parked (`futex_do_wait` /
  `ep_poll`), no thread ever transitioned while a read hung, accept
  backlog empty, both global lifecycle locks provably free (the instant
  404 path acquires both).
- **Root cause**: in `Manager::get`, the running-session fast path was
  written as
  `if let Some(entry) = self.inner.lock().await.running.get(id).cloned()`.
  Under Rust edition 2021 the `if let` scrutinee temporary — the `inner`
  mutex guard — lives through the entire success block, and
  `running_or_terminal_snapshot` locks `inner` again inside that block.
  The task awaited a mutex it itself held: a permanent, silent
  self-deadlock for every read of a live session, released only when the
  HTTP peer gave up and axum dropped the handler future.
- **Fix**: bind the lookup to a variable before the `if let` so the guard
  drops at the statement boundary (both occurrences of the pattern), plus
  regression test `get_of_running_session_does_not_self_deadlock`, which
  times out in five seconds on the broken code.
- **Why tests missed it**: no test read a session while a synthetic
  running entry was registered; all read tests exercised terminal
  records.

## Incident 2 — terminal reads and restarts reject the service's own acceptance records

- **Observed**: after the session above terminalized, reading its terminal
  record returned HTTP 500:
  `durable acceptance record .../accepted.json has identity/schema drift`.
  Worse, startup recovery reads the same record, so the deployed service
  could not even have been restarted while any current-format session
  record existed on disk.
- **Root cause**: the wire-transport protocol writes acceptance records
  with `schema_version: 2` (the streamed-archive commitment), but the one
  strict acceptance reader still pinned `schema_version != 1 → reject`
  from before the migration. Acceptance records persist for the
  resource's whole lifetime and every terminal read of a current 256-bit
  handle cross-checks them, so every terminal read of a session created
  by the new protocol failed, as would every restart and every
  idempotent replay.
- **Fix**: pin the reader to the only schema that can legitimately exist
  on disk (`!= 2 → reject`), correct the stale doc comment that claimed
  acceptance records are consumed at terminal publication, and add
  regression test
  `current_handle_terminal_read_accepts_persistent_v2_acceptance`, which
  also proves a version-1 record is rejected as drift.
- **Why tests missed it**: the only round-trip through the strict reader
  used a historical 32-hex session ID, which skips the acceptance
  cross-check entirely.

Both incidents were surfaced by the suite driver's first fail-closed poll
cycle — before any benchmark variant completed — and were invisible to the
81-test suite because both defects live exactly on the running-session and
current-handle read paths that only a live production session exercises.
