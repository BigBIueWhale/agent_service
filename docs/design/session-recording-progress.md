# Session recording progress

Durable response recording pauses the provider's network clock. The shared
session writer must therefore refuse a stalled storage operation itself. A
provider timeout would blame the wrong owner, and a timeout only at the response
wrapper would leave recording flush and lease cleanup waiting indefinitely.

The asynchronous canonical writer allows one minute without observed storage
progress. This is an availability policy, not proof that an operation will never
complete: an individual write, sync or metadata call that takes longer is
refused even if it later succeeds. The value is declared once in the shared
writer. Progress resets it during bounded append chunks and
transcript hashing; there is no deadline on the total transcript size or total
operation duration. Acquisition and healthy terminal lease transitions remain
separate from append and read-barrier monitoring.

A stall permanently refuses the lease. Every active or queued public operation
must reject, including release and handoff. The underlying filesystem operation
continues to own the serialization tail until it actually settles. No timeout
releases the lock, claims durability, advances the expected transcript proof,
starts a queued append, or permits a successor while a late write can arrive.
After a pending operation returns, progress checks prevent further work; handle
cleanup may still run. The error identifies the recording and requires stopping
its owning Qwen process and restoring storage before inspecting the incomplete
recording. It never invites certification or replay of a partial transcript.

This boundary serves all asynchronous canonical session writers, including
ordinary interactive sessions and service invocations. It does not alter vLLM
behavior: the backend neither owns this file nor its writer lease. Synchronous
child-transcript writes and an independently stalled response-body cancellation
are distinct open obligations; an event-loop timer cannot bound synchronous I/O.

The implementation is in the shared lease. Sixteen source tests passed in
`/tmp/codex-writer-progress-expanded.log`: six held operations (write, sync,
metadata read, append open, append stat and close), each with release or handoff
queued first; a progressing large append plus its queued successor; a progressing
metadata rehash; and canonical recorder flush/close with either terminal policy.
The twelve fault cases require the same failure for every waiter and future
operation, retain the exact active lock, refuse another writer, and release the
held I/O to check possible late bytes without queued side effects. The recorder
cases also retain the durable cursor and require one failure notification.

The initial four-case baseline failed because all waiters were still pending
after the deadline (`/tmp/codex-writer-progress-before.log`). The first fix run
passed three cases; its metadata fixture also stalled the second-writer probe.
The corrected fixture holds only the first metadata operation. This fixture
error is not evidence of a production failure.

The broader three-suite source run passed 170 tests, skipped Darwin and root-only cases,
and failed two existing canonical resume fixtures at generation admission
(`/tmp/codex-writer-progress-regressions.log`). Those fixtures serialized a
resolved assistant shape without its required stored generation envelope.
The fixtures now use complete physical request, response, generation, history
and completion records. A subsequent full writer-lease source run passed 103
tests with two existing skips
(`/tmp/codex-session-writer-fixture-review/revision-1/full.log`). After correcting
the fixture's outcome-before-assistant order to match the producer, the two
affected handoff cases passed again
(`/tmp/codex-session-writer-fixture-review/revision-2/focused.log`). The
read-only fixture review at
`/tmp/codex-session-writer-fixture-review/revision-2/REPORT.md`
found no remaining issue in that bounded change. The tests assert both the
physical tail and restored text-only runtime history. This is source-test
evidence for those cases, not an application resume or native admission result.
After the physical generation verifier was added, those two source cases again
stopped at admission. The helper had no normalization seed and its constructed
generation did not match its retained SSE body. The helper now derives the
generation through the shared response decoder, records the seed before the
generation, and excludes that evidence record from the expected conversation
projection. The full writer-lease and producer-backed runtime-history suites
then passed 117 source tests with two existing skips. This strengthens the
source evidence for exact resume history; it does not run a packaged resume,
compiler, native certifier or deployed provider.
The raw-response suite passed without altering its direct recorder-clock tests
or substituting any validator.

The corrected fixture's sealed landmark stage applied to a fresh pinned source
archive. All 1,133 declared path identities, including 44 deletions, matched
the authoring source, and semantic source contracts passed. The transaction
report is `/tmp/codex-writer-fixture-admitted-state.json` at review patch
`fd9e40ace5361658b218dbd10e43fec72bb14b2bb2bf9dc0c9eded4914578c2a`.
This is source-transformation evidence, not compilation or an application run.

Timers require a responsive event loop. They cannot preempt synchronous
serialization, hashing or child-file I/O. Independent transport cleanup, output
backpressure, arbitrary callbacks inside a read barrier, acquisition and terminal
transitions are not bounded by this append/verification policy. No build, native
test, generated binding, deployed application or backend execution was run;
these remain unverified pending owner gates.

Response cancellation has a separate transport owner. The source baseline at
`/tmp/codex-response-cancellation-stall/baseline/REPORT.md` executed the actual
recorder with native local streams. A held raw cancellation left finish and
history pending. More seriously, a consumer's first pending cancellation could
be bypassed by finish's second cancel: it wrote a clean cancelled outcome before
the original cancellation later failed. Recorder-initiated cancellation could
also leave a consumer's pending read unresolved.

The current authoring correction shares the first raw cancellation result among
consumer cancellation, pull-error cleanup and finish. A one-minute transport
cleanup deadline starts when that raw operation begins, independent of recording
time. Expiry refuses the response and records failed closure and processing
when the evidence writer remains available; a later transport settlement cannot
turn it into success. The active pull still owns any body write already queued,
so an end record cannot pass that write. Normal recorder-initiated cancellation
closes the exposed stream and settles its pending reader. The deadline trades
availability for a possible refusal of an unusually slow but eventually
successful transport cancel; it does not limit response length or generation
time. The existing failed-read reason remains first when a later caller falsely
claims completed processing.

The full response-recorder source file passed 60 cases, including seven new
concurrent-cancel, header-failure, timeout, pull-error, pending-consumer and
already-returned-byte controls
(`/tmp/codex-response-cancellation-after-byte-review.log`). The adjacent
pipeline response-evidence file passed 21 cases
(`/tmp/codex-response-cancellation-pipeline-final.log`). An independent
read-only review matched the final authoring identities and reran the 60-case
response file successfully; its local injected-fetch cases exercise the
installed SDK parser, and separate native local-stream probes cover both
cancellation orders, pending reads, late cleanup and exact bytes already
returned by the transport
(`/tmp/codex-response-cancellation-stall/final/REPORT.md`). This is source
and local parser evidence, not a provider or deployed application run. This
correction does not bound a separate stuck SDK iterator return or an arbitrary
output-publication callback while the recording clock is paused. Those owners
remain open, as do synchronous child-file waits.

The final cancellation splice applied to a fresh pinned archive with all
1,133 declared path identities, 44 deletions and semantic source contracts
matching the authoring source. The manifest and protected release inputs were
unchanged during that transaction. Its report is
`/tmp/codex-response-cancel-admitted-state.json`, at review patch
`e3c20d7ad371d39bbbfa351f956eed71569acdbb6cac353bb12493a90833d19e`.
This proves source application, not compilation or native admission.

A broader ChatAttempt source file ran two cases and failed five at installed
generated-v5 wire validation of the candidate's v7 `stream_start`
(`/tmp/codex-response-cancellation-attempt-final.log`). The validator was not
replaced or generated. That run does not qualify ChatAttempt admission and is
not attributed to this recorder correction.

The OpenAI stream guard also owns SDK iterator closure. Once it has aborted an
unfinished request, it allows the iterator the same one-minute cleanup window
as response transport cancellation. An iterator that never settles cannot hold
the pipeline before its response outcome forever: the guard raises an
actionable cleanup refusal, retains an earlier source failure when there is
one, and the pipeline passes failed processing to the recorder rather than a
clean caller cancellation. Natural SDK EOF needs no second `return()` call. The two affected
source suites passed 191 tests, including a held iterator, a held iterator
after a source error, natural EOF and adjacent physical-response cases. These
tests use a controlled SDK iterator and a stubbed processing recorder for the
stall; they do not prove a hung installed SDK or deployed transport will settle,
that its physical outcome is durable, or that output observations match retained
response bytes. The raw-response proof remains open.
