# Session recording progress

Durable response recording pauses the provider's network clock. The shared
session writer must therefore refuse a stalled storage operation itself. A
provider timeout would blame the wrong owner, and a timeout only at the response
wrapper would leave recording flush and lease cleanup waiting indefinitely.

The asynchronous canonical writer allows one minute without observed storage
progress. The value is declared once, as `RECORDING_STALL_TIMEOUT_MS` in
`packages/core/src/core/recording-stall.ts`, and is an availability policy, not
a derivation: it trades a possible refusal of an extremely slow but eventually
successful storage operation for a named failure instead of an indefinite wait.
An individual write, sync or metadata call that takes longer is refused even if
it later succeeds. Progress resets the clock during bounded append chunks and
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
behavior: the backend neither owns this file nor its writer lease.

## Response cancellation

Response cancellation has a separate transport owner. The first raw
cancellation result is shared among consumer cancellation, pull-error cleanup
and finish, so a second cancellation cannot publish a clean outcome while the
first is pending or later fails. The same one-minute budget bounds that raw
cancellation, counted from when it begins and independent of recording time.
Expiry refuses the response and records failed closure and processing when the
evidence writer remains available; a later transport settlement cannot turn it
into success. The active pull still owns any body write already queued, so an
end record cannot pass that write, and bytes a raw read already returned are
recorded before consumer delivery is suppressed. Normal recorder-initiated
cancellation closes the exposed stream and settles its pending reader. The
budget does not limit response length or generation time. The existing
failed-read reason remains first when a later caller falsely claims completed
processing.

The OpenAI stream guard also owns SDK iterator closure. Once it has aborted an
unfinished request, it allows the iterator the same one-minute cleanup window.
An iterator that never settles cannot hold the pipeline before its response
outcome forever: the guard raises an actionable cleanup refusal, retains an
earlier source failure when there is one, and the pipeline passes failed
processing to the recorder rather than a clean caller cancellation. Natural SDK
EOF needs no second `return()` call.

## Stated limits

Model evidence publication to a stream-json output uses the same budget, so a
consumer that stops reading is refused rather than awaited forever. Timers
require a responsive event loop. They cannot preempt synchronous
serialization, hashing or child-file I/O: the child transcript writer uses
synchronous I/O, so a stalled child-file write is not bounded by this policy.
Arbitrary callbacks inside a read barrier, acquisition and terminal lease
transitions are also outside the budget.
