# Exact observed provider responses

The shared client records a physical generation request before dispatch, then
records the returned HTTP body before the provider SDK parses it. These are fetch entity-body bytes after
transport content decoding, including decompression when present. This location
preserves malformed JSON/SSE and literal tool-argument bytes while keeping each
provider's existing fetch implementation, proxy and retry ownership intact.

Each response has a request identity and consecutive local sequence. A header
record precedes all body records. Body records contain canonical base64 and an
exact byte offset. The terminal declares EOF, cancellation, failure or an
undispatched intent, with the complete observed byte count and SHA-256. EOF
establishes transport completion, never conversation acceptance. A cancelled or
failed response establishes only its observed prefix.

One pull-through stream writes before delivering bytes to the SDK. A persistence
quantum bounds each temporary encoded record, without a total response limit.
The per-request clock pauses during recorder and output backpressure, preserving
the meaning of upstream timeout bounds. User cancellation still owns transport
abort. Failure to record poisons the durable journal and prevents later dispatch.
An explicitly detached optional renderer releases its own output ownership.
Automatic title generation is owned background work: finalization aborts and
joins it before a terminal is published, a fork snapshot is taken, or the writer
lease is released. Closing refuses new requests and conversation writes while
allowing admitted responses to record their cancellation. Live flushes leave
background generation running; final flushes surface mandatory evidence failures
even when the title generator catches its own failure.
Admission checks caller cancellation inside the journal's serialized queue. A
cancelled request that has not reached durable admission is never dispatched and
does not consume a sequence or degrade the writer. Once admitted, its terminal
evidence remains mandatory even if cancellation precedes network dispatch.
The shared proxy error redactor constructs native `DOMException` clones so abort
classification and diagnostic accessors remain valid after redaction.

Stream and canonical formats are version 3. Response evidence is structurally
separate from conversation messages and cannot advance the parent chain. Readers
validate physical ordering, hashes and ownership before projecting a branch.
Live checkpoints list unfinished responses; this supports background agents in
ordinary multi-turn sessions. Complete native certification additionally requires
all responses to have ended. The two fake-provider harnesses compare captured
request and response bytes with those actually received and served.

Source tests cover malformed wire, Unicode, server errors, cancellation, large
bodies, durable-write refusal, concurrent requests, slow local recording, unknown
fields, sequence/offset/hash corruption, title cancellation and writer handoff,
and exact full/indexed/fork restoration.
Native and full launcher/image qualification remains a release-owner step under
the implement-only brief; no build or deployment is implied by source checks.

This corrects client-owned omissions for every user of the shared generation
pipeline. Direct vLLM callers do not run this client recorder. Output omitted by
a backend parser, tokens never transmitted, and semantic accepted/abandoned
attempt attribution require evidence from their actual owning layers and remain
separate work in the standing completeness goal.
