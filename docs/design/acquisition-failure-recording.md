# Record admitted attempts when provider stream acquisition fails

A Chat attempt owns its physical provider requests even when provider stream
acquisition fails before the processed stream exists. That attempt needs an
abandoned canonical generation record. An attempt marker or a failed preflight
without a durable request does not establish that work was admitted.

## Evidence and ownership

The frozen `4bc2592` baseline exercised actual Chat, OpenAI SDK acquisition and
the child canonical writer against an in-process fake fetch. HTTP 500 exhaustion
persisted two requests, two failed processing outcomes and two abandoned physical
histories under one logical attempt. `processStreamResponse` never started and
the child canonical file contained no generation record. A separate reproduction
persisted a request, then refused the required output drain: the pipeline never
received the response recorder, so it never attached that request to the attempt.
Parser-failure and successful-generation controls recorded one generation each.
These are executed source observations, not deployed-provider or build evidence.

The request journal accepts the live Chat attempt, or null for utility work,
and derives the wire owner itself. It refuses a settled owner before persistence,
then attaches the response immediately after request persistence acknowledges
success, before publishing request evidence. The pipeline has no second attach
step. Capture failure after admission attempts physical cleanup outside the
journal transaction, preserving the original error and any distinct cleanup
failure. A failed journal stays failed; this change does not certify an incomplete
physical stream after persistence or publication refusal.

Chat catches only failure of the acquisition retry operation. If a physical
request was admitted, it awaits one `generation_failure` containing that
attempt's origin, the requested model, empty observed Parts and incomplete calls,
and null observed usage and finish reason. Physical errors, bytes and reports
remain in their existing response records. Empty output is not fabricated model
text or a claim of zero token use. Failed preflight without request admission
writes no generation. Successful inner acquisition retry reaches the existing
processed-stream owner and writes no separate failure generation.

The outer attempt settlement still defaults to abandoned on acquisition failure.
Writing this diagnostic does not set the terminal disposition latch: a legal
outer preterminal retry can proceed, with a distinct attempt identity. Provider
and recording errors both remain visible if the diagnostic cannot be persisted.
After acquisition succeeds, the processed-stream cleanup remains the sole owner
of generation recording, including parser failure and iterator return.

## Shared scope and resume

This belongs in the client request journal and shared Chat owner. Main and child
chats receive the same correction; utility requests retain their existing
physical evidence ownership. The vLLM backend does not own local acquisition
retry, the canonical file, or stdout publication, so no backend correction is
required for this defect. No benchmark or configuration path is introduced.

Resume reads the canonical runtime recording, not `output/events.jsonl`. The
added occurrence uses the existing versioned `system/generation_failure` shape,
which is diagnostic evidence and cannot be replayed as an assistant turn. The
provider request body, conversation admission, runtime-history edits, accepted
Parts, and replay order do not change. This is established by reading the shared
writer and history projection; full new-version resume execution remains
unverified pending the owner's bindings and gates. The stream and canonical
schema contents do not change, and fake providers need no new record shape.

## Verification boundary

Ten focused source regression cases passed after implementation: acquisition
exhaustion, preflight absence, cancellation, canonical write refusal, inner and
outer retry recovery, persistence versus publication admission, closed owners,
and pre-admission cancellation. The affected core run passed 572 of 577 cases
across Chat, request admission,
response ownership, pipeline and title cleanup. The five full-wire ChatAttempt
cases stop at the unchanged generated v5 validator and remain unqualified for
the current v6 contract. They were not deleted, bypassed or supplied with a
substitute validator. The CLI/SDK capture fixtures were migrated by source
inspection; their full-wire behavior is likewise not qualified by this run.

Eleven independent SDK/child-writer scenarios passed against a frozen candidate,
including real acquisition exhaustion, publication refusal, recovered inner and
outer retries, cancellation, zero-request failure/return and both recorder-method
and actual child-file write refusal. The final actual-I/O cleanup assertion was
also rerun: acquisition, flush and cleanup retain the exact injected I/O error,
and acquisition retains the original SDK error too. The tests use in-process
fake fetch and separately fsynced physical evidence; they do not qualify deployed
provider behavior, the main recorder's filesystem path, crash recovery, or full
wire admission. The initial I/O fixture did not reach the ESM-bound write method;
its failed logs were kept and the binding was corrected before qualification.

Independent source review read all five changed production files and seven
changed test files plus direct writer/replay consumers and found no introduced
defect. Transformer framework tests passed 38 cases. These are bounded source
and patch checks, not compilation or full resume qualification.

A fresh pinned archive transformed successfully. All 1,062 declared final
identities match the authoring tree; exactly the twelve intended source/test
paths changed. All 6,925 review hunks match both original and final coordinates,
and unrelated authoritative edits remain byte-identical. The twelve saved
candidate identities match the transformed source. The manifest and lock bind
the review patch and landmark data; protected release inputs are unchanged.
These checks exercise patch application, not an application compiler or build.

This closes the canonical acquisition omission, not full logical attempt
certification. Durable completion/output binding, omission and duplication
refusal, usage reconciliation, runtime/input accounting and explicit closed
artifact boundaries remain implementation obligations. No build, generator,
typecheck, native/Java execution, image, release, deployment or push is authorized.
