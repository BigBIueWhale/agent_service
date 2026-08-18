# Failure dossier: full-suite-v3 — every failed run, read in full

Ten parallel Fable deep-readers each consumed one failed task completely:
both event streams (every turn of thinking, every tool call and result),
the task statement, the hidden golden-test patch, the model's candidate
patch, the grader log, and — where a sibling succeeded — the winning
patch as contrast. Combined corpus: 18 failed/timed-out runs plus the 3
earlier loop-halt runs, ~11 MB of raw evidence. This file is the
cross-suite synthesis; per-task detail lives in the agents' reports and
the referenced artifacts.

## 1. The dominant failure is a judgment call at an explicitly-considered fork

In seven of the nine failed tasks, the run did not miss the winning
answer — it **generated the winning answer and rejected it**, in
writing, usually in both thinking modes independently:

- **arcadedb-4281** (both modes): articulated the golden hybrid
  ("try MessageFormat first… fall back to String.format-style… could
  handle both") and rejected it as "too clever" / "non-conformant usage
  shouldn't be silently supported" — deleting observable behavior the
  hidden PASS_TO_PASS tests lock in, despite the repo's own CLAUDE.md
  saying "maintain backward compatibility". The real PR's review bot had
  flagged the identical regression on the identical first draft.
- **nanobot-4274** (both): read, quoted, and *predicted the failure of*
  the exact four legacy tests that later failed them — then overruled
  the tests as "probably stale" ("the evaluation harness probably has
  updated tests"). Run A's workspace was grader-RESOLVED-equivalent from
  turn 61 through turn 112, until its own wrong-oracle verification
  harness "found a real bug" in the accidentally-correct code and edited
  the winning patch into a losing one.
- **dubbo-go-3357** (both): voiced "should removal be pointer or value
  equality?… the hidden tests probably use the same pointer; keep
  consistency" and bet wrong; voiced the match-key wipe scenario
  verbatim and closed it with "no problem". One line
  (`u == url` → `u.URLEqual(url)`) separated both runs from most of
  their failures.
- **agno-8148** (both): cited the flat-kwargs precedent in
  `delegate_task_to_member` as "the cleanest approach", then rejected it
  because `continue_run` lacks a `session_state` parameter — choosing a
  `run_context` object handoff that is plausibly functionally correct
  but invisible to the graded `kwargs["dependencies"]` assertion.
- **pulsar-25793** (both): run A hypothesized "the hidden tests will
  count the number of scheduled retries" and still shipped a 100 ms
  polling design whose own harness printed the polling ("reschedules…
  total=5" per half-second). Run B *held the reference-shaped
  completion-driven design first* and retreated to polling when a
  fixable handoff race looked complex.
- **pulsar-25865** (both): asked the decisive question — "what if a raw
  RuntimeException leaked out of fromByteArray?" — and answered "the
  constructor's catch would return 500 — that's acceptable", which is a
  precise description of the hidden test and the wrong verdict. Both
  patches were one `| RuntimeException` clause from resolving.
- **arcadedb-4455** (both): the instruction's own root-cause analysis is
  wrong (the bug is a compaction-phase race it never mentions). Run A
  proved the stated mechanism impossible dozens of times, touched the
  true answer twice ("unless page 1 is being reset. `clearDataPages()`…"
  / "phase 4c (write lock)") and reasoned itself out of it both times;
  it never converted "the described mechanism cannot happen" into "the
  description is wrong". Zero files edited in 50 minutes.

The recurring priors behind the wrong turns: spec-purism over
behavior-preservation; enumerating observed failure signatures instead
of defensive coverage; "existing tests are stale" instead of
"existing tests are the spec"; status-quo consistency over the
statement's explicit requirements.

## 2. Verification was universally circular — and once actively destructive

Every failed run substituted self-referential verification for the
unrunnable real suite: harnesses and self-written tests that encode the
chosen design as the oracle. 15/15, 27/27, 30/30, 37/37, 43/43, 44/44
"ALL CHECKS PASSED" — every one certifying the misinterpretation.
Signature cases: pulsar-25793-A's harness measured the fatal polling and
blessed it as a bounded-liveness PASS; nanobot-4274-A's harness flagged
the accidentally-correct legacy behavior as "a real bug" and drove the
losing edit; pulsar-25865-B's probe re-implemented `fromByteArray` as a
stand-in whose body structurally erased the post-parse phase where the
real bug lives, then asserted the hidden test's failure condition as
correct "unchanged behavior".

Most runs were *honest* about the limitation ("could not run the real
suite; please run pytest before merging") — the failure is the headline
"Done / fixed and verified" resting on checks that could not falsify.

## 3. The feedback vacuum: agent containers cannot run anything

Every agent container is network-none with no usable toolchain: no
Maven (while a staged AGENTS.md advertises "Maven 3.5+"), Go 1.22
against a `go.mod` requiring ≥1.25, no pytest/pydantic (bare Python +
PyYAML), 2–4 jars on the entire filesystem, no Gradle distribution, no
module caches. Verifier containers are fully provisioned. Consequence:
the intended loop "reproduce → instrument → fix → re-run" was impossible
in every failed run; the design fork of §1 had no falsification channel;
tens of turns per run went to toolchain hunts, bytecode archaeology
(`javap -c`), and stub-harness engineering. Note for comparability:
several `task.toml` files declare `allow_internet = true`; this
deployment deliberately runs agents network-none (stricter than the
task spec) — a documented deviation.

## 4. Non-termination is budget-blindness, not slowness

Across all 21 runs there is **not one thinking token about the time
budget** — no pacing, no deadline, no "good enough to stop". Every
completed turn ends in a tool call; emitting a final answer requires a
tool-free turn, which requires deciding the plan is exhausted, and
todo-list items ("verify", "run tests") were unbounded or unsatisfiable.
At the measured ~35–40 s/turn (decode-dominated at 60–200k context),
3000 s buys 75–80 turns; the timeout runs used exactly that and died
mid-scruple (nanobot-4129: auditing the style of its own assert),
mid-summary (agno-A: killed while composing the final report),
mid-audit (nanobot-4274-A), mid-thought (pulsar-25793-B). The hugegraph
pair reframes "solved but couldn't stop": the fixes landed at 67% and
87% of budget and the remaining turns were the prompt's own second
deliverable (regression tests) — a scheduling failure, not scope creep.

## 5. Thinking modes change the texture of failure, not the outcome

Ten of eleven honestly-recorded pairs agree in outcome across modes; the
one split (arcadedb-4411) is luck, not capability: both modes produced
functionally identical fixes (the loser's planner hunk is
character-identical to the maintainer's), and the reward difference was
test-file placement — the winner's tests (carrying the same latent
compile bug) went into a file the grader force-resets, the loser's new
file survived into the graded build and broke compilation.

- **Unpreserved**: re-derivation tax (the same conclusion re-derived up
  to 30 times; 2.5× the thinking volume for the same work), decision
  drift (4411's test-placement choice flipped on its fifth cold
  re-litigation — the fatal flip), and tool-glitch perseveration.
- **Preserved**: cheap, fast, decision-stable — and doubt-suppressing
  (dubbo-A quit at 33% of budget with its two decisive doubts voiced
  once and closed forever) and detour-persistent (hugegraph-B followed a
  phantom state-leak through five classes of bytecode).

Neither mode produced budget awareness, and both modes made the same
wrong design calls independently — strong evidence these are
model-level priors, not sampling noise.

## 6. Benchmark-construction hazards observed (recorded, not excuses)

- Misdirecting statements (arcadedb-4455's wrong root-cause analysis is
  constructed such that trusting it is fatal).
- Hidden tests grading conventions or thresholds underivable from prose
  (agno's kwarg name; 25865's batchSize=-1; 25793's +2 policy-read
  bound; dubbo's revision "0"/Update semantics).
- Test-file pinning asymmetry: edits to pinned test files are wiped
  (harmless), but *new* test files survive into the graded build and can
  zero a run on a compile error the agent could never reproduce (4411).
- Fail-fast amplification: one failed test aborts the suite and records
  dozens of unrelated tests NOT_FOUND (both pulsars).
- Environment-image traps: AGENTS.md documenting absent tooling;
  baked-dirty worktrees (fixed as incident 4). (Correction: the
  `javac.*.args` files in 4455-B's patch were first attributed to an
  environment-side javac wrapper; `/usr/bin/javac` in the agent image
  is a plain ELF binary and nothing in the stack writes argfiles — the
  model's own probe scripting left them in the workspace.)

## 7. Evidence-stream limitation discovered during this investigation

Qwen Code 0.21.12's stream-json elides the body of ranged/truncated
`read_file` results: events.jsonl records only the "Read lines X-Y of Z"
banner while the model receives the full content (proven: a run's very
first recorded ranged read is banner-only, and the immediately following
thinking quotes the file verbatim with no shell read in between).
Consequences: forensic readings of the streams under-observe what the
model saw (one reader's "information blackout" theory for pulsar-25865
is downgraded to "misreading of delivered content"), and any future
event-stream analysis must treat banner-only reads as
content-delivered. Token-usage and compaction analyses are unaffected
(usage fields are recorded separately).

## 8. What "is it number of turns?" actually resolves to

No. Turn counts are a consequence: failures are decided at design forks
in the first third of a run, and long runs are the model grinding inside
a wrong frame at a fixed ~38 s/turn until the 3000 s ceiling. The only
turn-related regularity that survives the deep read: nothing has ever
resolved after ~120 turns or ~9M cumulative input tokens, because
crossing that line means the run is lost in a wrong frame, and no
falsification channel exists to pull it out.

## Corpus and provenance

Per-task readers (all Fable, full-stream mandates) covered:
arcadedb-4281, -4411, -4455; nanobot-4048 (loop-halt forensics),
-4129, -4274; agno-8148; dubbo-go-3357; hugegraph-3037; pulsar-25793,
-25865; cloudnative-pg-10747 (loop-halt forensics). Loop-halt analysis:
`loop-halt-forensics.md`. Raw evidence: `full-suite-v3/runs/**` (event
streams, patches, grader logs, terminal records), `evaluator-dataset/**`
(statements, hidden tests, golden solutions).
