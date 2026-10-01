# Provider processing outcome

Each physical generation request has two independent completion facts. The
transport records its exact observed response bytes and termination. The shared
provider pipeline then records whether SDK parsing and response conversion
completed, failed, or were cancelled. EOF cannot settle that second fact: a
batch JSON parser or converter can reject after reading the entire HTTP body.

The processing outcome follows the transport end in the same response sequence.
Replay keeps the request open until both facts arrive, releasing the response
hash at transport end. A final certification refuses an omitted outcome; a live
checkpoint may name the still-open request. Unknown event shapes and old format
versions are refused. A failed outcome preserves its error independently of a
successful HTTP transport. A caller that returns an iterator before reading its
first item also settles the physical request and releases its transport.

The pipeline owns this outcome for streaming and batch generation, including
utilities, compaction, main chat, and child chat. It does not claim that decoded
output was accepted into conversation history. That later decision belongs to
the consumer that commits or rejects it. A complete SDK iteration can still
produce an abandoned chat attempt.

Stream contract v19 and canonical recording version 22 carry both facts.
Canonical response records are structurally disjoint evidence and never become
history parents or model turns. Resume reads that canonical history projection; stdout remains
evidence. Both fake providers require processing completion in addition to
byte-exact response accounting.

This belongs to the shared client provider layer. Every client caller
receives the same recording behavior, with no benchmark or configuration path.
vLLM cannot observe SDK parsing, iterator cancellation, or client conversion;
its own generation accounting belongs to its backend layer.
Normal streaming stays pull-driven and records no extra copy of the body.

Source tests use the real SDK for malformed batch JSON,
malformed SSE, conversion failure after EOF, iterator cancellation (including
before its first read), and concurrent ordinary streams. Reader tests refuse
missing, repeated, reordered, or malformed outcomes.
