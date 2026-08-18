# Forensic analysis: three loop-guard kills, Qwen Code 0.21.12 + qwen3.8-27b-nvfp4-k8v4

(Produced by a full-stream forensic review of all three failed runs' event
streams; every terminal streak verified byte-for-byte against the raw JSONL.)

All three runs died in the same way: `read_file` called with the PDF-only
`pages` parameter on a plain text file, repeated byte-identically until the
guard fired. In each run the last 4 logged tool calls are byte-identical and
the run halts while producing the 5th (guard threshold = 5). No run ever
reached an `edit`/`write_file` — all three died during code exploration.

## Failure 1 — arcadedb-4455 (preserve_thinking=TRUE), 94 events, num_turns=29

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

## Failure 2 — HKUDS__nanobot-4048 (preserve_thinking=TRUE), 68 events, num_turns=20

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

## Failure 3 — cloudnative-pg-10747 (preserve_thinking=FALSE), 67 events, num_turns=22

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
121. I'll try this.") — and the run died producing the opposite. With
thinking stripped (unpreserved), the lessons learned at each recovery
vanished from context and the model relapsed at the very next file both
times — surviving history showed its own prior `pages` calls, i.e. bad
examples without the corrections.

**Fault split:** model ~55%, tool/harness ~35% (inaccurate capacity error
as the only feedback; unpreserved history erasing self-corrections enabled
the relapses), loop guard 0%, serving ~10%.

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

4. **preserve_thinking made self-correction worse, not better.** The two
   preserved runs had zero recoveries once the loop began (0/13 post-onset
   `pages` calls corrected); the unpreserved run recovered twice mid-run.
   Preserved history keeps every prior turn's meta-commentary — dozens of
   additional literal `pages` tokens — lexically reinforcing the very
   pattern the semantics forbid. Stripping weakened the attractor enough to
   allow escapes but deleted the learned lesson, causing fresh relapses
   until one ran the streak out. Neither setting prevented the death; the
   preserved runs died deeper in the rut, the unpreserved run died of
   amnesia-driven relapse.

5. **Operational note (client-side).** `cache_read_input_tokens: 0` on
   every call in all three runs; totals 913k/425k/469k input tokens for
   29/20/22 turns.

## Addendum (session operator)

The operational note in (5) reflects the client-visible usage fields only.
The vLLM engine's own counters over this deployment show 176.1M of 181.8M
prefix-cache queries hit (96.8%) with mean prefill 0.63 s per request, so
prompt reprocessing was in fact almost entirely cache-served; Qwen Code's
stream-json usage simply does not propagate vLLM's cached-token accounting.
The token totals above are nominal API input, not recompute cost.
