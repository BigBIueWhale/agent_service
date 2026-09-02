# Forensic analysis: three loop-guard kills, Qwen Code 0.21.12 + qwen3.8-27b-nvfp4-k8v4

(Produced by a full-stream forensic review of all three failed runs' event
streams; every terminal streak verified byte-for-byte against the raw JSONL.)

All three runs died in the same way: `read_file` called with the PDF-only
`pages` parameter on a plain text file, repeated byte-identically until the
guard fired. In each run the last 4 logged tool calls are byte-identical and
the run halts while producing the 5th (guard threshold = 5). No run ever
reached an `edit`/`write_file` — all three died during code exploration.

## Failure 1 — arcadedb-4455, 94 events, num_turns=29

**The loop.** Wanting lines 678–990 of `TransactionContext.java` (990 lines;
a prior full read returned a truncation at line 677), the model emitted
`read_file` with a `pages` value containing literal embedded double quotes:
`pages: "\"\""`, `"\"600-990\""`, `"\"677-990\""`, then 4 byte-identical
calls `{limit: 315, pages: "\"677\""}` (ev 67/71/74/77). Changing only
`limit` 315→160 at ev 80 reset the guard counter, then 4 more identical
calls; the halt came generating the 9th consecutive `pages`-bearing call.

**The tool_result** (identical each time, is_error=True):
`Invalid pages parameter: '"677"'. Use formats like '5' or '1-10'.`
Accurate that the value is malformed, but not actionable toward the real
fix: it never says `pages` is PDF-only, and its suggested formats are the
shape the model had already tried minus the quotes.

**Thinking between repetitions** — the model diagnosed the right fix at
least 10 times and never executed it once: ev 60 "Oops, `pages` is
exclusively for PDFs. I'll use `offset` and `limit` instead."; ev 76 "I'm
stuck in a loop — I keep including the `pages` parameter."; ev 82
"Something is systematically inserting `pages` into my tool calls."; ev 85
considered and rejected the working shell escape ("I'll just read it with
`cat`... no wait, the tool is the right way."). Not one post-ev-55 call
contains an `offset` key, despite `offset` having worked earlier in this
same run (ev 21, 27).

**Fault split:** model ~60% (clear corrective feedback + correct
self-diagnosis, zero execution), tool/harness ~30% (misleading error,
PDF-only param exposed on text files, truncation pressure), loop guard 0%
(correct, even lenient), serving stack ~10% (copy-attractor caveat; history
provably visible, generation live).

## Failure 2 — HKUDS__nanobot-4048, 68 events, num_turns=20

**The loop.** After three successful `offset` reads, the model switched to
`pages` at ev 29 and never emitted `offset` again; terminal streak ev
52/56/60/64: `{limit: 130, pages: "770"}` — halt on the 5th.

**The tool_result — the nastiest harness behavior of the three.** Two
contradictory signals, neither true: ranges >20 returned `Pages range
exceeds maximum of 20 pages per request.` (is_error=True) for a .py file —
implying `pages` works but is capped; small values returned
**is_error=False**, `Read lines 1-130 of 1487 ...` — the parameter
**silently ignored**, the file read from line 1. The terminal loop
consisted of "successful" calls returning the same useless 130 lines; only
an identical-args guard could ever have stopped it.

**Thinking.** The model quoted the tool schema verbatim at ev 58
("`pages`: Optional: For PDF files, ...") and announced the fix in
user-visible TEXT three times (ev 55, 59, 63); ev 62: "I really need to
write `offset: 770` and `limit: 130`." The next call was again
`pages: "770"`. It even detected the silent-ignore itself at ev 51.

**Fault split:** model ~50%, tool/harness ~40% (silent-ignore plus a
capacity error that affirms the wrong mental model), loop guard 0% (a
clear positive), serving ~10%.

## Failure 3 — cloudnative-pg-10747, 67 events, num_turns=22

**The loop.** Three separate `pages` episodes on Go files with two genuine
recoveries in between (ev 36 and ev 48 both switched to working
`offset`/`limit` calls). Final relapse at ev 51; terminal streak ev
54/57/60/63: `{limit: 121, pages: "600-721"}` — halt on the 5th.

**The tool_result** (identical all 10 failing times): `Pages range exceeds
maximum of 20 pages per request.` For a Go file this is flatly inaccurate
(the parameter is inapplicable, not capped) and its implied fix — shrink
the range — would have landed in the silent-ignore trap.

**Thinking.** Every terminal-loop turn re-derived the correct fix on the
spot; ev 65 spells out the exact intended JSON ("`offset`: 600, `limit`:
121. I'll try this.") — and the run died producing the opposite. Neither
mid-run recovery held: the model relapsed at the very next file both
times, with the corrections it had just written still standing in its
rendered history alongside its own prior `pages` calls.

**Fault split:** model ~55%, tool/harness ~35% (inaccurate capacity error
as the only feedback), loop guard 0%, serving ~10%.

## Cross-case synthesis

1. **One bug, three runs.** Identical signature: `read_file` + PDF-only
   `pages` used as a line-range parameter on source files (677, 770,
   600-721 are line numbers). Guard threshold 5 confirmed; the guard never
   fired prematurely — nanobot's repeats were zero-information "successes"
   that would never have self-terminated.

2. **Thinking-vs-action dissociation is the core model failure.** In every
   run the model produced the correct diagnosis — often quoting the schema
   or writing out the exact intended arguments — immediately before
   emitting the identical wrong call. All three runs prove the model could
   use `offset` (successful earlier calls) and had a shell escape it never
   took. History visibility was intact; arguments varied with live intent,
   ruling out template/history corruption. What remains is a decode-level
   copy attractor (each in-context `pages:` occurrence strengthening the
   next), plausibly aggravated by NVFP4 quantization.

3. **The harness invited the trap and never described the exit.** Three
   different `pages` failure modes were observed and not one of them says
   the only true thing: "this is not a PDF; `pages` is inapplicable; use
   `offset`/`limit`." Two actively point the wrong way. Fixes: reject
   `pages` on non-PDFs with an explicit redirect, run the file-type check
   first, never silently drop a supplied parameter.

4. **Operational note (client-side).** `cache_read_input_tokens: 0` on
   every call in all three runs; totals 913k/425k/469k input tokens for
   29/20/22 turns.

## Addendum (session operator)

The operational note in (4) reflects the client-visible usage fields only.
The vLLM engine's own counters over this deployment show 176.1M of 181.8M
prefix-cache queries hit (96.8%) with mean prefill 0.63 s per request, so
prompt reprocessing was in fact almost entirely cache-served; Qwen Code's
stream-json usage simply does not propagate vLLM's cached-token accounting.
The token totals above are nominal API input, not recompute cost.

## Post-fix confirmation — v6 on the v14 release (arcadedb-4455)

The `non-pdf-pages-rejection` contract (qwen-code source patch, baked into the
v14 agent runtime) is live and behaves exactly as designed. In the v6 rerun,
`full-suite-v6/runs/ArcadeData__arcadedb-4455/01-unpreserved` (num_turns=59,
187 events), every terminal-streak `read_file` on
`.../database/DatabaseInternal.java` with `{limit:60, pages:"40"}` returned the
new, accurate tool_result:

> The 'pages' parameter applies only to PDF files; 'DatabaseInternal.java' is
> not a PDF. For text files, use 'offset' and 'limit' instead.

This is the one true thing cross-case synthesis (3) said no prior error ever
stated: names the file type, states `pages` is PDF-only, redirects to
`offset`/`limit`. The file-type check now runs first; nothing is silently
dropped; no contradictory capacity error appears.

**And the model looped anyway.** It kept `pages:"40"` across the streak,
varying only `limit` (80→50→60→60→60→60), and Qwen Code's guard halted the run
(`error_during_execution`, "consecutive_identical_tool_calls"). This is the
decisive post-fix datapoint: with the harness giving the exact correct redirect,
fault collapses onto the model — synthesis (2)'s thinking-vs-action / decode
copy-attractor is the residual cause, plausibly NVFP4-aggravated. Harness fault
here is ~5% (residual: `pages` remains schema-exposed on `read_file` for text
files; a schema-level rejection is a possible further hardening, weighed against
never silently degrading a real capability). The clear-error fix was necessary
and correct; it is not, by itself, sufficient to stop a model that will not act
on its own correct diagnosis.

## Separate harness defect — acceptance reader masks the valid loop terminal

Independent of the loop itself, the service's strict terminal parser
(`agent_service/src/result_parse.rs`) mis-reports these halts. It requires
`num_turns == main_assistant_events` (line 184) and returns
`AgentOutputMissing("terminal num_turns=59 does not match 58 main assistant
event(s)")` **before** it ever inspects `is_error`/`subtype` (line 190+). But a
loop-detection halt is a *complete, valid* `error_during_execution` terminal:
the run stops mid-turn, so Qwen counts the halted in-flight turn
(`num_turns = completed_turns + 1`) while only the completed turns carry a
usage-bearing assistant record. Proven from the event tail: 58 usage>0 assistant
records, then a final `input_tokens:0` fragment (turn 59 begun, halted), then the
`result`. `num_turns − main_assistant_events = 1` is the *expected* relationship
for a mid-turn halt, not a lost/truncated stream.

Consequences: the durable service summary blames `agent_output_missing`
(output missing/malformed) when the output is neither; the true reason (loop
detection) survives only in `bundle/output/events.jsonl`. The benchmark
*bucket* is unaffected — `agent_exit_code=1` classifies it
`production_agent_process_failure` either way (full-suite-run.sh:423) — so this
is a reporting-accuracy defect, incidence 3 across v1–v6, no pass/fail or retry
impact.

Correct fix (deferred: needs a service rebuild + release-lock bump + redeploy,
which must not interrupt the running v6): treat a terminal with `is_error=true`
and `subtype∈{error_during_execution,error_max_turns}` as valid when
`num_turns − main_assistant_events ∈ {0,1}` (`==0` still required for
`subtype=success`), keep `main_assistant_events ≤ num_turns` fail-closed against
real stream loss, and surface the terminal's own `error.message` as the response
instead of the num_turns text. Because the 3 bundles are durable, correct
labels can be re-derived from stored evidence without rerunning the model.
