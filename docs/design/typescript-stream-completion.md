# TypeScript stream completion

Clean byte framing establishes complete records, not a complete session. The
shared Core admission owner must verify that every observed session initialized,
every active output window reached its root result, every declared response
finished, and every partial message closed before a reader reports successful
EOF. A root result may legitimately name open background responses while the
stream stays live. Those responses must still close before EOF.

The TypeScript Query invokes that common completion check at natural EOF and
accounts for submitted user inputs through one path for both factory prompt
forms. Core owns the same input extraction used by the CLI, so exact blank no-ops
do not invent turns. Queued and separately timed inputs keep the control channel
open until their results arrive without an automatic deadline. The SDK removes
the streamClose timeout option because it would close a control channel still
needed by an ordinary slow turn. An accepted continuation also needs a result;
EOF while an input iterable is unfinished is a refusal. Only root results settle
submitted work or trigger single-turn end-input.
Input/result counts are a lower bound; exact queued-input causality remains a
producer-owned contract. Fixed and resumed session identities must be checked
before records are delivered. Fork and continue use the authoritative session
identity in the CLI initialize reply, before prompt construction. A custom Query
without a selected or negotiated identity binds the first stream session and
refuses a subsequent change.

The first read, input, initialization or completion failure remains available to
the output iterator and subsequent operations. Pending controls at EOF cannot
coexist with successful iteration. Cleanup must complete without replacing that
first cause or leaving iteration waiting forever.

Explicit Query.close terminates its transport. It therefore cancels an unfinished
output window; it cannot certify complete evidence. Graceful completion uses
endInput and consumes the iterator through natural EOF. Root results leave live
multi-turn queries available, and child results cannot close root input.

This belongs in the shared Core reader and TypeScript SDK for ordinary and long
sessions, including those using vLLM. Direct backend callers do not execute this
reader; backend recording omissions still require backend-owned fixes. No wire
schema, provider request shape, canonical chat recording or history projection
changes are intended. Resume continues to reconstruct history from the canonical
runtime recording, while stdout remains evidence.

Verification uses source tests with the authoritative request/response fixtures,
real local process/pipe checks where relevant, no-emit typechecking, exact pinned
archive application, all 35 semantic concerns and two clean final review passes.
The owner's restrictions exclude builds, releases, deployment and push.
