# Response processing must agree with its transport evidence

The request's physical response records a transport ending and a processing
outcome. The transport ending precedes the recorded outcome; parsing may finish
before unread transport is cancelled. History acceptance is a later decision
owned by the chat. Served usage is independent of that history decision.

Successful processing requires a captured HTTP status in the successful
200–299 range and a recorded transport ending of `eof` or `cancelled`.
`not_dispatched`, absent HTTP headers, unsuccessful HTTP, and failed transport
cannot substantiate a successful processing claim. The shared recorder checks
this immediately before appending its processing outcome, after cancellation
and persistence have settled. The replay and native reader check the same
relationship before accepting that outcome.

Cancellation of unread transport can follow successful parsing; it does not
necessarily mean processing was cancelled. Conversely, complete transport
can be followed by a parsing failure or user cancellation. Failed and cancelled
processing therefore remain valid after each otherwise valid transport ending.
The invariant does not require a positive body length: empty provider processing
may complete before the chat rejects missing generation content or termination.

The record shape does not change. Canonical runtime chat JSONL and stdout
request/response evidence use the shared response recorder and replay. This
correction changes evidence admission, not conversation parts or history order.
The native certifier, Python and Java SDK readers and the shared fake-provider
verifier enforce the same relationship in source. Both composition and
headless checks use that verifier.

This applies to every provider request recorded by the shared Qwen pipeline,
for both chat and utility owners, independent of session size or benchmark.
vLLM owns neither the client's transport journal nor its processing decision,
so no backend change belongs here.

The independent frozen baseline reproduced the four false completion cases
with the actual recorder and replay: no dispatch, dispatch without headers,
failed transport and HTTP 503. Positive controls include ordinary EOF,
successful parsing with unread transport cancellation, and empty HTTP 204.
The independent candidate source run passed 56 recorder/replay cases, including
sticky refusal identity and cancellation failure aggregation. The permanent
recorder, real-SDK pipeline and attempt suites passed 59 cases. A further 577
pipeline, Chat, batch-client, timeout and concurrent-stream cases passed.
Parsed-SDK unit fixtures use an explicit test-only response-settlement stub:
they retain real request admission and per-response clocks but make no claim
to transport evidence. Actual SDK/fetch evidence fixtures keep real recorders.
These source tests use in-memory responses and controlled persistence; they
are not deployed-provider or filesystem durability qualification.

Independent Python execution passed 721 candidate checks across the evidence
reader, full record admission and the shared fake-provider verifier. Fourteen
separate baseline checks reproduced invalid completion acceptance. The final
SDK admission suite passed 274 tests, and the helper suite passed seven methods
including its 36-case transport/outcome matrix. Explicit empty HTTP 204 and
non-null cancellation-reason controls passed independently. Baseline acceptance
checks are observations of the defect, not candidate verification.

The transformer framework passed 38 tests and documented identifiers retain
35 semantic concerns. A fresh pinned archive transformation matched all 1,062
declared final identities and exactly 12 intended client paths changed from
the baseline. Unrelated authoritative edits and protected release inputs are
unchanged. The executed authored source matches the sealed result. The source
tests use the existing generated stream validator; regeneration was not run.
Source transformation and hash checks are distinct from compilation and
runtime qualification.

Rust and Java regression tests are authored and inspected, not executed.
Compilation, typechecking, native/Java execution, application builds and owner
gates remain unverified. No build, image, release, deployment or push was run.

Separate open work remains in per-attempt output accounting. Existing response
history decisions remain joinable from stdout origins; the missing subagent
field is a direct-label omission, not loss of all disposition evidence. Actual
source-admission reproductions also accept omitted abandoned output, reassigned
accepted text, duplicated billed output and abandoned-origin tool calls. The
terminal tool chunk's ordering is addressed by the separate
[chat settlement correction](chat-attempt-settlement.md). Empty billed root
output and usage before a later stream error need a complete attempt projection.
This transport correction does not claim to close those separate requirements.
