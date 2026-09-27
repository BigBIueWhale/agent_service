# Record served usage on its physical response

Every physical processing outcome carries mandatory nullable `served_usage`.
A non-null value is an owned snapshot of exactly five non-negative safe integer
counts: prompt, output, total, cached prompt and reasoning output. Total equals
prompt plus output; cached does not exceed prompt; reasoning does not exceed
output. Null means no complete valid report was observed by processing. It is
not zero cost and does not assert that the raw bytes contain no usage-like data.

The shared OpenAI pipeline observes SDK completions and chunks before provider
output callbacks, telemetry, `error_finish`, content conversion or terminal
buffering. The response recorder retains the last complete valid cumulative
report. Updates replace rather than sum, even if the latest valid report is
smaller. A malformed or absent later report cannot erase an earlier valid one.
The raw response records retain the original bytes, including invalid reports.
No second raw-body parser tries to infer usage from an SDK-rejected error body.

Null or absent chunk usage is normal. A present invalid report is a processing
failure. The pipeline preserves existing converter/provider error precedence:
it records the invalid-report cause, runs conversion, and raises a diagnostic
response error if conversion otherwise succeeds. This catches false, zero and
empty-string values that truthiness checks ignored, while retaining same-chunk
text diagnostics. Valid usage survives failed or cancelled processing, parser
errors and body cleanup failure. Observation after outcome settlement starts is
refused. Utility requests, root and child chat requests, and physical retries
use the same owner; reports cannot move between request IDs.

The existing mapper supplies both converted metadata and physical observations.
It preserves absent provider details instead of fabricating zero. Supported
model configuration already requires the vLLM exact-counting profile and strict
tools. Direct permissive converter behavior remains unchanged. This adds no
configuration, benchmark mode, provider request field or model-history content.
The vLLM deployment already supplies these counts; it does not own the client's
physical response journal or canonical history. This omission belongs in the
shared client and its readers. Backend clients outside this recording path gain
no new behavior from this client change, and no backend workaround is needed.

The stdout contract is version 6 and canonical runtime recording is version 8.
TS record validation/replay, the Python and Java SDK readers, native response
admission and fake-provider verifiers require the field and count relationships.
The SDKs package the exact source schema. TypeScript SDK evidence fixtures use
that same version. Normal completed fake-provider responses compare their
recorded counts to the fixture's supplied report; this check does not claim to
rederive every failed/cancelled observation from captured bytes.

Resume reads runtime chat JSONL, not `output/events.jsonl`. The new field belongs
to response evidence, which remains structurally disjoint from history. The
history content, acceptance, startup context and compaction replay algorithms
are unchanged by source inspection. Version 8 refuses version 7 and all unknown
or unversioned files; no old file is upgraded or rewritten. Existing per-file
catalog refusals retain their location and next action: inspect with the matching
client or begin a new session. Actual new-version canonical resume execution
remains unverified pending regenerated bindings and the owner's gates.

Independent baseline source tests reproduced missing physical usage and ignored
falsey malformed reports in 21 scenarios. The candidate passed 37 independent
actual-SDK/pipeline/recorder cases using in-memory HTTP responses and the real
lower-level validators. They include zero versus null, same-response converter
failure, repeated cumulative reports, mutation, cancellation and required-thinking
retry. The HTTP 400 retry response retains null despite usage-looking raw text;
the SDK did not decode that error as a completion. Its successful second request
has its own valid report. These are source tests, not live provider observations.

The five affected permanent source suites passed 517 tests: response evidence
53, actual-SDK pipeline evidence 21, converter 247, pipeline 168 and served usage
28. An initial combined run had 20 pipeline fixture failures and five full-wire
ChatAttempt failures. The pipeline mock omitted the new actual mapping helper;
positive wire fixtures also omitted required cached/reasoning counts. Those
fixtures now supply the mapper and explicit complete reports, with expectations
for those actual reports. No production fallback was added to accommodate mocks.
The five full-wire cases encounter the unchanged version 5 generated validator;
they remain unqualified pending the owner's generation/build, rather than being
passed with a substituted validator. Their source tests remain present.

The real Python SDK/schema passed 1,135 independent cases and 310 permanent
admission cases, including 36 new usage cases. Deterministic Query routing checks
preserve exact counts for a selected resumed session and refuse malformed or
foreign-session input before delivery; they do not execute a CLI or canonical
history restoration. Eight source-only fake-provider helper test methods passed.
Native and Java regression cases are authored and source-reviewed, unexecuted.

Generated bindings were neither regenerated nor edited. Binding generation and
its check mode compile code, so both are excluded by the owner's prohibition.
Compilation, typechecking, native/Java execution, complete wire admission,
application builds, provider behavior and owner gates remain unverified and need
the owner's build and gates. No image, release, deployment or push was run.

This physical association is a prerequisite for complete logical attempt
accounting. It does not bind expected text, reasoning or tool calls to canonical
commits or stdout, prove omission/duplication refusal, or reconcile per-attempt
usage with current billing projections. Those projections are not summed again
with the new physical reports. Logical completion, runtime-origin accounting,
input-to-request interpretation and the other open audit questions remain work;
this change does not certify complete record sets or finish the standing goal.

The final fresh pinned-archive transformation matched all 1,062 declared source
identities and the reviewed authoring source. Exactly 24 intended client paths
changed, including schema replacements; unrelated landmark edits are identical.
All 6,925 review hunks matched both coordinate systems. The 26 saved test-source
identities matched the sealed result. Framework checks passed 38 tests and the
semantic concern count remains 35. Manifest/lock bindings and unchanged protected
release inputs were checked. These are patch/source identity checks, not a build.
