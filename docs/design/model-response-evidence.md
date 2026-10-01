# Exact observed provider responses

The shared client records a physical generation request before dispatch, then
records the returned HTTP body before the provider SDK parses it. These are
fetch entity-body bytes after transport content decoding, including
decompression when present. This location preserves malformed JSON/SSE and
literal tool-argument bytes while keeping each provider's existing fetch
implementation, proxy and retry ownership intact.

Each response has a request identity and consecutive local sequence. An `http`
record with the status and content type precedes all body records. Body
records contain canonical base64 and an exact byte offset. The `end` record
declares EOF, cancellation, failure or an undispatched intent, with the
complete observed byte count and SHA-256. EOF establishes transport
completion, never conversation acceptance. A cancelled or failed response
establishes only its observed prefix. A separate processing `outcome` then
records whether SDK parsing and conversion completed, failed or were
cancelled, including a failure after transport EOF, together with the served
usage it observed. Completed processing requires a 2xx status and an EOF or
cancelled transport end. The chat's history decision, or a utility caller's
delivery receipt, is a further record of its own.

The recorder reads the transport from the moment the response exists and keeps
one read outstanding until it ends, because an HTTP client discards bytes it
still holds unread when its connection fails: a provider that sent a prefix and
then disconnected must leave that prefix in the record. Each body record is
durable before the SDK receives its bytes. Bytes read ahead of the SDK are
bounded; a provider that runs past the bound is refused, its transport cancelled
and its response ended `failed` naming the bound, after every byte read is
recorded, so a record is complete or says it is not. Because the transport is
read ahead, a response whose provider finished before the consumer stopped ends
at EOF even when processing was cancelled. A persistence quantum bounds each
temporary encoded record, without a total response limit.
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

The stream contract is v18 and the canonical recording is version 21. Response
evidence is structurally separate from conversation messages and cannot
advance the parent chain. Readers validate physical ordering, hashes and
ownership before projecting a branch. Live checkpoints list unfinished
responses, so a background agent in an ordinary multi-turn session leaves a
readable live prefix. Complete certification requires every response to have
both a transport end and a processing outcome. The fake providers of the
headless qualification and the composition check compare the captured request
and response bytes with those they actually received and served.

This serves every user of the shared generation pipeline. Direct vLLM callers
do not run this client recorder. Output a backend parser omitted and tokens
never transmitted are not visible at this boundary; they belong to the layer
that produced them.
