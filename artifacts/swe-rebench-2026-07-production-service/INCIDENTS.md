# Production incidents observed while running the benchmark suite

Issues found by actually operating the production agent service under the
SWE-rebench 2026-07 paired suite. Each entry records the observable
symptom, the forensic trail, the root cause, and the fix. This file exists
because the benchmark's mandate is to evaluate performance **and record any
issues** — these are the issues.

## Incident 1 — reading a running session self-deadlocks the read

- **Observed** (first suite launch, task `ArcadeData__arcadedb-4281`,
  session
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

## Incident 3 — agent loop-detection halt killed the whole pass instead of one variant

- **Observed** (task `ArcadeData__arcadedb-4455`, session
  `s-bb316d9fc3c2e4e23a56c9c9068fc504640e191e4fd1759f7f427b7784674e8a`):
  the suite driver died with
  `production terminal body or required bundle contract failed` after 28
  turns of a healthy run.
- **What actually happened**: the model got stuck re-issuing an identical
  invalid tool call (`pages: "677"` to the file reader) and Qwen Code's
  always-on `consecutive_identical_tool_calls` loop guard halted the run
  as `error_during_execution` (agent exit 1). The terminal result event
  claimed 29 turns while only 28 main assistant events completed, so the
  service's strict event parse refused to infer success and honestly
  published a `completed` terminal with `is_process_error=true` and one
  teardown diagnostic. The service behaved correctly end to end — the
  bundle commitment, teardown, and terminal record were all intact.
- **Root cause of the pass death**: the driver's terminal gate required
  `teardown_diagnostics == []`, which conflates recorded agent failure
  with infrastructure failure. The driver's own
  `production_agent_process_failure` classification exists precisely for
  this outcome but was unreachable behind the gate. (Timeout variants
  passed the gate only because durable cancellation produces a clean
  terminal with empty diagnostics.)
- **Fix**: the gate now enforces only infrastructure invariants
  (identity, terminal status, bundle commitment, no retained raw tree)
  plus a directly observed teardown truth — no container owned by the
  session may survive. Recorded agent failure flows to
  `production_agent_process_failure` and the pass continues. The partial
  variant evidence is archived at
  `full-suite-v3/runs/ArcadeData__arcadedb-4455/02-preserved.incident3-loop-halt-archived/`
  and the variant reruns fresh.
- **Benchmark note**: the loop-halt itself is a legitimate
  model-behavior data point — the agent's own final thinking read "I'm
  repeatedly making the same mistake by including a `pages` pa…" while
  the guard fired.

## Incident 4 — dataset images with baked-dirty worktrees killed the pass silently

- **Observed** (task `apache__hugegraph-3037`, session
  `s-3a2ba23e9e3a388da594922e2b8060d043401ba340f4a23bd69578fcfece972f`):
  after a healthy timeout run and a verified 2.9 GB bundle download, the
  driver died with no error message at the patch-construction step.
- **Root cause, silent death**: the patch-construction `docker run` had
  no explicit failure handler, so `set -e` killed the driver without a
  diagnostic when the container script failed.
- **Root cause, the failure itself**: the container script required a
  pristine baked worktree
  (`test -z "$(git status --porcelain …)"`), but this dataset image
  deliberately ships `pom.xml` modified out of the box (offline-build
  version bumps: lombok 1.18.30→1.18.38, plugin 3.1→3.13.0). The
  materializer had already recorded exactly that in
  `initial-git-status.z`, and the agent's input source was materialized
  from the baked worktree, so the driver's pristine assumption
  contradicted its own recorded evidence. Five of the 41 tasks ship
  dirty baselines (hugegraph-3037, LibreChat-13166, OpenJarvis-465,
  rewrite-7784, react-hook-form-13464); each would have silently killed
  the pass.
- **Why grading stays exact**: the grader reconstructs pristine base
  (`git reset --hard` + `git clean`) and applies `candidate.patch`,
  which is diffed against the base commit — so the baked fixes flow
  through the patch and the reconstructed tree equals the agent's final
  workspace byte for byte.
- **Fix**: both container scripts stop requiring pristine worktrees; the
  patch step records the observed baked baseline as
  `patch/baseline.status` and the driver byte-compares it against the
  materializer's `initial-git-status.z` (working-tree drift dies
  loudly); both docker invocations now fail with explicit diagnostics.
  The healthy-but-unfinished variant evidence is archived at
  `full-suite-v3/runs/apache__hugegraph-3037/01-unpreserved.incident4-archived/`
  and the variant reruns fresh.

## Incident 5 — indexing the candidate patch defeated the dataset's test-collision guard

- **Observed** (task `docker__docker-agent-2992`, session
  `s-56e5f62d4cc1df716743c83c621be613338347e0fd3e6baef08f37d9632f7bb6`):
  the grader ran but the official verifier wrote no reward
  (`official verifier did not write its reward and report`); its log
  ends with `error: pkg/runtime/streaming_test.go: already exists in
  working directory`.
- **What actually happened**: the agent worked test-first and created a
  test at the canonical Go path `pkg/runtime/streaming_test.go` — the
  same path the benchmark's hidden test patch creates. SWE-rebench
  anticipates exactly this: `test.sh` removes a colliding path before
  applying its test patch, but only when the path is **untracked**,
  because the protocol applies candidate patches to the worktree. Our
  grader applied `candidate.patch` with `git apply --index`, making the
  agent's file tracked, so the guard declined to remove it and the
  official test patch could not apply.
- **Fix**: the grader applies the candidate worktree-only (no
  `--index`), restoring the dataset's assumed state. Rewards read
  worktree bytes, so the 26 previously graded variants are unaffected.
  The partial variant evidence is archived at
  `full-suite-v3/runs/docker__docker-agent-2992/01-preserved.incident5-archived/`
  and the variant reruns fresh.
- **Benchmark note**: the collision itself is informative — the model
  independently chose the exact test path the maintainers used.

## Remediation — read_file pages fix and pass v4

The loop-halt forensics (loop-halt-forensics.md) established that all three
guard kills shared one harness trap: `read_file`'s PDF-only `pages`
parameter was syntax-checked before the file type was known, produced a PDF
capacity error on text files, and was silently ignored when small enough to
pass — never once naming the real remedy. The agent image now rejects
`pages` on non-PDF files at both the validation layer (before any syntax
check) and the consumption layer (no supplied parameter is ever silently
dropped), with an error that names `offset`/`limit`. The change is carried
by the landmark transformer (`patches/source_patch_v1`, new concern
`non-pdf-pages-rejection`) and three `[pages-contract]` tests execute the
boundary inside the hermetic image build. PDF behavior is byte-for-byte
unchanged, and no other read_many_files/pathReader caller can pass `pages`,
so nothing else changes.

Pass consequences: pairs fully accepted in `full-suite-v3` under its
recorded release remain valid single-release pairs. The two pairs whose
recorded results contain a `production_agent_process_failure` caused by the
trap (`HKUDS__nanobot-4048`, `cloudnative-pg__cloudnative-pg-10747`) are
superseded and rerun in full in `full-suite-v4` under the fixed release,
together with every task not yet accepted. Every pair therefore remains
internally single-release, and the final report will pool within-pair
deltas across both passes with this split disclosed.

## Corrected conditions — pass v5

The failure dossier established that no agent session could compile or
run anything, knew what time it was, or left a fully faithful evidence
stream. Pass v5 corrects each at its proper layer, still on Qwen Code:

- **Task toolchains and caches** (`warm-task-env.sh`): each task's own
  `tests/test.sh` runs once with network at materialization time —
  exactly the grader's posture — and the toolchain plus dependency
  caches are harvested into a per-task `task-env.tar.gz` with a
  relocation-aware `env.sh`. The v5 driver ships it inside the
  workspace archive over the existing wire contract; agents remain
  network-none.
- **Prompt layer** (`prompt-preamble.md`): reduced to the one
  mechanically necessary fact — the tarball exists and how to load it.
  An earlier draft also injected time-budget and grading-mechanics
  coaching; that was wrong (teaching-to-the-test, and noise in every
  prompt) and was removed before any v5 run.
- **Time** (agent image, `session-time-anchor` concern): the deployment
  contract now ends with one session-start timestamp computed once at
  process start — an absolute anchor that keeps the system prompt
  byte-stable for the session, so prefix caching is unaffected; live
  time stays observable via `date`.
- **Evidence stream** (agent image, `headless-stream-evidence`
  concern): stream-json tool results now carry the model-facing
  responseParts instead of the human-facing display banner, so captured
  event streams record what the model actually received.
- **Baseline toolchains** (agent image): Maven 3.8.7 and pytest 7.4.4
  from the pinned Ubuntu snapshot, Go 1.25.13 from a checksum-pinned
  upstream archive — permanent, versioned parts of the agent image
  rather than benchmark-only shims.

Passes v3 and v4 are retained as historical evidence; v5 runs all 41
pairs fresh under the corrected conditions with composed-prompt and
task-env hashes recorded per run (result schema 3).
