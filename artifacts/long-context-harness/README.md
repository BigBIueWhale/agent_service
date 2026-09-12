# The long-context harness: probe, Test A, Test B, Test C

These four runs exercise the production agent service on a long single-thread workload,
which is what the SWE-rebench suite does not do: a suite task is one repository and a
bounded trajectory, while a benchmark pass that survives requires the service to compact
a conversation many times in a row without losing the session. Every failure they have
found so far has been ours, not the model's, which is why they run before the suite and
why nothing skips ahead.

The corpus text is not in this repository — it is a commercial book series, derived from
the owner's own EPUBs. What is here is the part that has to be reproducible: the exact
prompts, the identities any rebuilt corpus must reproduce, and a check that refuses when
either has moved.

## What each run is for

| run | delegation | what it forces |
| --- | --- | --- |
| probe | none (no subagents) | the same *shape* as Test A at 1/50th the cost: 26 chunks read in order, one summary written. It exists to expose a harness defect in minutes instead of ninety. |
| Test A | forbidden | compaction **in the parent**. The main agent reads 1,554,654 words itself — about six times the 262,144-token window — so the session survives only if roughly 28 consecutive compactions all succeed. |
| Test B | exactly one subagent | compaction **inside a subagent scope**, where the parent's own context stays small. Has never once succeeded. |
| Test C | the agent chooses | the control. Historically it spawns one subagent per book, so nothing approaches the window, and it says what the model does when nothing is imposed. |

The three test prompts are byte-identical except for the delegation clause, so delegation
is the only variable. `./verify-harness-inputs.sh` proves that mechanically rather than
by inspection: Test C is the common text, Test A is Test C plus its clause, Test B is
Test C plus its clause, and each file's SHA-256 is pinned below.

## Pinned identities

Prompts, as committed here:

| file | SHA-256 |
| --- | --- |
| `prompts/probe.txt` | `66e4b9a9fe661c1ddca759a98e39537b3b62adec96db638fc50848c3cf29a8e0` |
| `prompts/test-a.txt` | `fa92a0c2bc559898f849f95626596c5b96117ff06d5328708290ca5cc35c0cc7` |
| `prompts/test-b.txt` | `20cdbbdd5a9c20106638b0101c59309a0a162a289ce6e32bc721dac23bbab2dc` |
| `prompts/test-c.txt` | `727c358ba9846ccd56f07037decc07151cbda577b9096b7da76f163116a5250e` |

The series corpus is eleven UTF-8 text files under `txt/`, one per volume including the
two `.5` novellas, totalling **1,554,654 words** and 9,400,777 bytes. Concatenated in
`sort` order its SHA-256 begins `4a87caf8b460bbee`; the eleven per-volume digests are pinned in
`corpus-digests.sha256`, so a single moved volume is named rather than reported as a
changed concatenation.

The probe corpus is **26** files `chunk_00.txt`..`chunk_25.txt`, cut from a single text at
50,000-byte boundaries that are moved to the nearest character boundary — which is why the
sizes are 49,998 to 50,002 bytes rather than exactly 50,000, and why the concatenation is
byte-identical to the source. Total **1,293,067 bytes**; concatenated SHA-256 begins
`68a5006393aee012`.

A corpus that does not reproduce those identities is a different corpus, and results
across it are not comparable with anything recorded here.

## Outcome history

- **probe: passes** (2026-09-08) — all 26 chunks read in order, 3 successful compactions,
  a 4,935-word summary written.
- **Test A: three failures, three different defects** — a compaction reasoning runaway; a
  torn 219 KB record that voided the whole stream; then a malformed snapshot (five of six
  sections, `<intent>` closed by `</environment>`, `<environment>` absent) alongside an
  uncertifiable `stream_event`. The arithmetic behind all three is in
  `../swe-rebench-2026-07-production-service/INCIDENTS.md`, incident 9: ~28 serial
  compactions at an observed 15/18 each is 0.6% survival, so the exponent was the problem
  rather than any one bug.
- **Test B: never succeeded.** Its recorded death (2026-08-31) is precise: the single
  subagent read book 3 in a disciplined `read_file(offset=N, limit=420)` cycle, banked ten
  chunks of notes, then emitted four identical `read_file` calls with `offset` **omitted**,
  and the client's `consecutive_identical_tool_calls` guard terminated it at turn 139. The
  guard was right to notice; discarding 139 turns of banked work over one dropped argument,
  without telling the model what it had repeated, is the part that is wrong.
- **Test C: not yet attempted** on any current build.

## Running one

Each run is a session submitted to the production service with the corpus as its
workspace and the matching prompt file. The service accepts submissions on loopback only,
so a run is launched on the machine where the service is deployed, and its session record
carries the release identity, the turn budget and the thinking-retention arm it ran under
— the same identity a suite run records, for the same reason: a result whose conditions
are not recorded cannot be compared with another.

## How a run is judged

A run is evaluated against its own evidence — `events.jsonl`, the session record and the
written artifact — not against an impression of the summary's quality. Four questions, in
this order, because the first three decide whether the fourth means anything:

1. **Did the harness hold?** Every record in the stream certifies, the terminal `result`
   belongs to the main session, and a subagent's terminal result is stamped with its
   owning tool-call id. A run that ends in an uncertifiable record is a harness failure
   regardless of what the agent wrote.
2. **Was the corpus actually read?** Every file opened, in order, to its end — countable
   from the `read_file` calls and their offsets. Coverage is the claim these tests exist to
   make; a summary written from a sample answers a different question.
3. **How many compactions, and did each survive?** The count and the token position of
   each are the measurement the whole design is for. A compaction whose snapshot comes
   back malformed — a missing section, a tag closed by the wrong tag — is a failure even
   when the run continues, and it is recorded as one.
4. **Is the summary faithful?** Judged for invention rather than polish: names, who did
   what, and whether an event described actually happens in the text. One recorded failure
   wrote "Sophie Castor" for Sophie Foster, which is the failure mode worth looking for —
   fluent prose drifting off the source under compaction pressure.

The judgement is written down per run, beside the run's evidence, with the version of the
service and the retention arm it ran under, so two runs can be compared rather than
remembered.
