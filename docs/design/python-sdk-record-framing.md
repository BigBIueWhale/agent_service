# Python SDK record framing and failure ownership

The Python process reader interprets CLI stdout for both asynchronous queries and
the synchronous wrapper. It must preserve valid record values and refuse malformed
bytes before they can disappear from the record sequence. A transport read buffer
is not a record-size limit: full prompts and tool results can exceed it in ordinary
sessions. Read fixed I/O chunks and accumulate each LF-committed record without a
line-length cap. Accept CRLF; refuse every blank LF-committed record, invalid UTF-8,
JSON constants outside finite numbers, non-record roots and every nonempty EOF
suffix. Refusals name the physical line and an available diagnostic action.

The query owns failure delivery and resource cleanup. Latch the first failure,
close write admission and enqueue the original failure after the accepted prefix
before asynchronous cleanup. Run cleanup under separate task ownership so cancelling
the router cannot erase a published error. Pending controls fail immediately;
input streams stop and close their iterators when the query terminates. Slow
consumers drain the queued prefix and original failure without restarting or being
rejected by write admission. Cleanup failures remain visible without replacing the
first failure. Explicit close and clean EOF complete ordinary consumers; a valid
result on an open multi-turn stream leaves it available for subsequent controls.
Public input calls share that cleanup ownership. Closing during process startup
joins startup before closing its process, and cannot start query tasks afterward.
An input source failure is published before awaiting its iterator cleanup, so a
later transport failure cannot displace it. Reentrant close during owned cleanup
does not wait on itself.

This is client-owned interpretation after model output delivery, shared by every
Python SDK caller using the CLI transport, including vLLM-backed sessions. No
backend configuration can repair this reader's skipped lines or line-size ceiling.
The canonical runtime recording and its resume replay are unchanged, as are model
request bodies and the stdout schema. Tests use source implementations and real
asyncio streams, with only the process boundary mocked.

Versioned record admission is a separate part of the full repair, documented in
`python-sdk-record-admission.md`. Python binds to the authoritative version 5
contract, replays model request/response evidence and validates partial streams
before delivery. It refuses unknown versions and record types, including
semantically invalid known records. Framing tests alone do not prove that complete
JSON records were understood or that a complete request/response sequence was
delivered. Java and other reader boundaries retain their own corresponding work.

Validation uses frozen-source reproductions and permanent Python unit tests. The
framing fixtures exercise malformed middle records, strict decoding across every
Unicode byte boundary, all nonempty EOF suffixes, finite numbers, and full records
larger than the process reader's default buffer. Query fixtures cover both prompt
modes, slow readers, pending controls, synchronous consumption, blocked input,
startup, cancellation and cleanup-error ordering. Independent admission fixtures
also cover unknown types, invalid recognized records and preserved model evidence;
their scope and remaining causal-completeness limit are recorded in the admission
design. Byte-framing tests do not establish complete version 5 admission.

The blank-line refusal passed the focused Python framing suite and the full
Python unit suite on the authored source. It closes a silent skip at the same
physical boundary used by ordinary and long sessions; it does not prove a
generation's decoded observations came from its response bytes.
