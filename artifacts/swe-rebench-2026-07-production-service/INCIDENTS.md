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

---

Incidents 6 onward were found after pass v5, both under the suite and under the
pre-suite harness tests (the probe and Test A/B/C, which exercise the same
production service on a long single-thread workload). Each says plainly whether
it is closed.

## Incident 6 — a transport failure was delivered as a completed answer

- **Observed**: six sessions across v3, v6 and v7, on both machines, finished
  with `subtype: success`, `is_error: false`, exit 0, and a `num_turns` one
  lower than the number of model calls in `events.jsonl`. The first theory — an
  off-by-one in the service's turn counter — was wrong and is retracted.
- **Forensics**: `num_turns` equals the count of `thinking` assistant events in
  every case, and the unbilled turn is always the last one. The service counts
  assistant events with nonzero `input_tokens`, and usage attaches only to
  whichever assistant message is open when `GeminiEventType.Finished` arrives,
  so a turn that starts and then dies reports no usage.
- **Root cause**, two independent defects:
  1. `nonInteractiveCli.ts` raised `GeminiEventType.Error` only under
     `outputFormat === OutputFormat.TEXT`. Under `json` / `stream-json` — how
     the service always runs — the error text was appended to the assistant
     message as ordinary content and the session completed successfully. A hard
     transport failure reached the caller as an answer. This affected **every
     JSON-mode caller of this service**, not only the benchmark.
  2. `DEFAULT_STREAM_MAX_LIFETIME_MS = 900000`, upstream's remote-gateway
     default, is shorter than a legitimate maximum-length generation here:
     262,144 tokens at the measured ~30 tok/s is ~2.4 h. It killed a healthy
     stream delivering 30 chunks/s
     (`full-suite-v3/runs/apache__hugegraph-3037/01-unpreserved`, 27,364 chunks
     in 904 s).
- **Fix** (`330b1c0`): the format condition is gone — the JSON error-result path
  already existed — and both stream guards are pinned in the runtime
  contract, exported by `docker/config/run_agent.sh` and asserted at build time by
  `docker/scripts/verify_runtime_contract.py:324-335`,
  which also exports them into the agent's environment (`:382-383`):
  `stream_idle_timeout_ms` 240,000 and `stream_max_lifetime_ms` 21,600,000 —
  four minutes idle, six hours of life — with the lifetime required to exceed
  the idle bound, since the idle watchdog is the stall detector and the lifetime
  cap is only the drip-feed backstop.
- **Deliberately not changed**: the service's `num_turns` cross-check. It was
  never the defect; it was the only thing that caught defect 1.
- **Closed.**

## Incident 7 — a subagent exhausting its turn budget voided the whole session

- **Observed** (2026-08-27): a session in which a subagent hit `MAX_TURNS` was
  discarded in its entirety, with no result for the main thread's completed
  work.
- **Root cause**: the subagent's terminal `result` event carried no owning
  tool-call id, so the reader accepted it as the session's own terminal result.
  Everything after it was then "after the terminal", and the stream failed
  certification.
- **Fix (service half)**: a subagent `result` no longer terminates a session;
  the error is "no main-session terminal result", and nothing may follow the
  session's own. Pinned by `rejects_a_stream_that_ends_at_a_subagent_result` and
  `rejects_a_subagent_result_after_the_session_result`, with many tests running
  a `MAX_TURNS` subagent result followed by a main result and certifying
  normally. The convention is one-way by design: null-or-absent
  `parent_tool_use_id` means the main session, so any other shape can only
  exclude an event from the main thread, never admit one to it.
- **Still open (emitter half)**: that the patched client *stamps* a subagent's
  terminal `result` with its owning tool-call id, or does not emit it. This must
  not be repaired service-side with a heuristic (`num_turns == 0`, zero usage,
  last-result-wins): such a rule would also swallow genuine stream corruption,
  which is the invariant the check exists to protect. This is the open lead for
  **Test B**, which hands an entire corpus to exactly one subagent and has never
  once succeeded.

## Incident 8 — a pass cannot be resumed across a record-schema generation

- **Observed**: after the turn budget became a per-session creation field
  (`6830863`), pass v8's committed result directories were no longer
  interpretable by the service that wrote them, so the pass could only be
  restarted under a new root, not resumed.
- **Root cause**: a terminal record is parsed strictly, and a record written by
  a superseded schema is indistinguishable — to a strict parser — from a
  corrupt one. Startup treated both as a reason to refuse.
- **Fix** (`78f669c`): a well-formed record from a superseded schema is now its
  own named fact — `uninterpreted_terminal_record`, HTTP 409 — and the session
  list reports `sessions` and `uninterpreted_records` separately. Such a record
  is preserved, carries no recovery or mutation authority, and does not block
  startup.
- **Closed**, with the operational consequence recorded: a pass root is
  per-release evidence, and the driver's `PASS_ROOT` (`full-suite-run.sh:73`) is
  the only authority for which root is current.

## Incident 9 — serial compaction, not any single bug, was the dominant failure mode

- **Observed**: Test A (the main agent reads an 11-book, 1,554,654-word corpus
  itself, subagents forbidden) failed three times, each on a different defect:
  a compaction reasoning runaway; a torn 219 KB record voiding the whole
  stream; then a malformed snapshot — five of six sections, `<intent>` closed by
  `</environment>`, `<environment>` absent altogether — alongside an
  uncertifiable `stream_event`.
- **Arithmetic**: a full Test A run needs ~427 reads, ~970 turns, ~18 h and
  **~28 serial compactions**. At the observed per-compaction success of 15/18
  (83.3%), single-attempt survival is 0.6%. Retrying a failed compaction lifts
  the *base*, not the exponent: k=2 gives 45.4%, k=3 gives 87.8%, k=4 gives
  97.9%, for 17–20% more attempts because the geometric sum is bounded. No
  single fix could have saved these runs; the exponent was the problem.
- **Fixes**: bounded resampling of a malformed compaction candidate
  (`d588713`), and the compaction output budget derived rather than clamped —
  `contextLimit - summaryRequestTokenCount` instead of a flat reserve, so a
  request lighter than the bounded worst case buys the snapshot more room, and
  one that would leave less than the reserve is refused rather than quietly
  shrunk.
- **Closed for the mechanism, open for the evidence**: no Test A run has yet
  completed under the resampling build.

## Incident 10 — the retention comparison silently became a repeated run

- **Observed** (2026-09-12): the suite still produces two attempts per task and
  a `pair-summary.json` that calls them a pair, but both attempts now run the
  same configuration. The dimension the pair was named for is gone, and nothing
  fails.
- **Forensics**: `full-suite-v8` and `full-suite-v9` hold `01-unpreserved` and
  `02-preserved` per task, with `"variant": {"preserve_thinking": false|true}`
  in both the pair summary and the session record. The current driver
  (`full-suite-run.sh:690-697`) names attempts by `printf '%02d'` alone, and
  `grep -c variant` over the driver returns 0. `full-suite-v10` — the only pass
  opened since — holds a bare `01-unpreserved` and no pair summary, under the
  older naming, so nothing has ever run under the current driver.
- **Root cause**, two edits with one shape:
  1. `58b1ec7` deleted the per-session retention surface, including the sealed
     settings file for the preserved arm, on the premise that "the inference
     engine's own template governs thinking retention". The template
     (`chat_template.jinja:117`) gates on `preserve_thinking is undefined or ...
     is true`, so **absence is the enabled state**. Removing the entry did not
     delegate the choice; it flipped the production default to preserved-ON,
     which is the mode the requirements exclude ("preserved thinking off ... by
     default in our deployment").
  2. The driver then lost the arm from its ordinals, leaving the pairing
     structure with nothing to compare.
- **Why this is the worst shape a regression can take**: every other defect this
  week announced itself — a build refused, a gate failed, a lock disagreed. This
  one changed what a recorded number *means* while every name, schema version
  and directory layout stayed identical.
- **Open.** The fix spans all three layers: the engine default
  (`config/runtime-v1.sh` gains `"preserve_thinking":false` in
  `--default-chat-template-kwargs`), the service (the sealed entry stated at
  `false`, one optional creation field, two pinned settings artifacts, the arm
  recorded in every session record), and the driver (the ordinal carries the arm
  again). Because retention under `false` sheds at `ns.last_query_index`, which
  the client's active-todo reminder advances every third tool turn, each run
  will also record how much thinking actually survived — the covariate that
  makes the comparison readable instead of merely available.

## Incident 11 — the workstation hard-crashes under serving load

- **Observed**: repeated whole-machine crashes on the primary workstation while
  serving the model under benchmark load, ending sessions with no terminal
  record and leaving pass v10's single run with `created.json` and nothing else.
- **Disposition** (2026-09-06): all GPU work moved to a second machine and runs
  there exclusively. Images are **not** reproducible across hosts, so a release
  reaches the second machine only through its pinned offline image archive,
  never a rebuild.
- **Closed as an operating decision**, not as a repair: the crash itself was not
  root-caused, and the workstation is no longer asked to serve.

## Incident 12 — a container-runtime upgrade mid-release faked an image-ID drift

- **Observed** (2026-09-02): `release.sh` chased an image ID that had not really
  moved, and an in-flight build was cancelled.
- **Root cause**: a `containerd.io` upgrade and service restart underneath the
  build truncated BuildKit's timestamp rewrite, leaving orphaned
  `containerd-shim` processes. `docker.service` was never restarted, so
  `journalctl -u docker` shows nothing and the cause is easy to misattribute —
  it was first blamed on concurrent Docker work of our own.
- **Disposition**: recorded rather than guarded. A release must not span a
  runtime upgrade; if one happens, the round is void and is re-run rather than
  adopted.

## Incident 13 — every build re-fetched its dependencies from third-party mirrors

- **Observed**: each `./build.sh` fetched ~500 Ubuntu packages from
  `snapshot.ubuntu.com` and 2,060 npm tarballs from `registry.npmjs`, and the
  host's root filesystem reached 100% with **359.7 GB** of BuildKit cache that
  could not be attributed to this project by name.
- **Root cause**: generic third-party layers — the pinned apt set, the
  toolchains and the Node dependency bytes — were rebuilt on our own release
  cadence, which moves many times a day, rather than on the cadence of the pins
  that actually change.
- **Fix**: those layers now live in two base images built by one script
  (`scripts/build-base-images.sh`), pinned by exact ID in the stack lock,
  verified before every build and transported inside the release archive.
  Measured after the split: `snapshot.ubuntu.com` 0 lines,
  `registry.npmjs` 0 lines, 2,060 packages installed offline in 7 s, all 337
  in-image test files still executed. Cost: a 4.98 GB toolchain base and a
  147 MB runtime base, on our own disk and removable by name.
- **Closed**, with one leftover being finished: the pinned upstream client
  tarball is still fetched from `codeload.github.com` twice per build, and moves
  into the toolchain base so the common path fetches nothing at all.
