# SDK record framing

The SDK process transport reads the CLI's stream-json output. A malformed line
cannot disappear before record admission: deleting a line turns an incomplete
observation sequence into an apparently valid one. The framing owner must refuse
invalid JSON, invalid UTF-8, invalid message roots and uncommitted EOF suffixes.
LF commits a record. Terminated ASCII-whitespace lines carry no record, matching
the service reader. CRLF is accepted without altering text inside JSON strings.

The TypeScript parser consumes bytes directly, preserving multibyte UTF-8 across
arbitrary transport chunks and refusing decoding replacement. It accumulates
fragments until LF and joins each record once, avoiding repeated copying for large
records. Complete objects proceed to the shared versioned record admission;
unknown protocol shapes remain that owner's responsibility. There is no permissive
parsing mode. Refusals identify the physical line and a possible diagnostic action.

These changes belong to client transport interpretation, after backend delivery.
They apply to ordinary and long sessions, single-turn and streaming-input SDK
queries. They do not change model requests, stdout record shapes or canonical
history. Resume reads the canonical runtime recording and is unaffected by this
byte framing change. Source-only tests exercise the actual parser and transport,
including corruption followed by otherwise valid output, UTF-8 split boundaries,
large records, prefix retention, EOF and pending control requests.

The query owner closes its transport when the reader terminates. It settles pending
requests with the framing or admission error, and checks that same terminal state
before writing controls or user input, including after initialization and asynchronous
input waits. Input completion waits settle on the first result or termination. A
later abort or cleanup error cannot convert an established framing failure into
success. A valid result on a live multi-turn stream leaves the query open; clean
EOF completes it. The output iterator retains valid records preceding the refusal.

Other readers must receive the same audit. Python currently replaces invalid UTF-8
and skips malformed JSON, and its query router drops unknown record types. Java's
session router passes unknown types to an optional callback. Those wider record
admission gaps are not proved closed by TypeScript framing tests.
