# Python SDK versioned record admission

Every CLI stdout record enters one Query-owned admission state before it changes
session identity, routes a control callback or reaches an SDK consumer. The Python
package carries the exact authoritative stream-contract-v6.json bytes; the source
transformer binds this resource to the repository contract and refuses drift.
Draft 7 validation handles structural constraints. The resource digest identifies
the accepted protocol, including init and session-start declarations. There is no
optional validation mode or unknown-record branch.

Schema validation alone cannot establish completeness. Request replay preserves
the raw top-level messages slices, delta bases, segments, invocation scopes and
physical journal sequence. It checks reconstructed UTF-8 byte counts and SHA-256
without reserializing the body, following the shared Rust evidence reader. Python
JSON serialization is not the JavaScript SDK's serialization and must not be used
to rewrite evidence. Response replay validates physical sequence, canonical base64,
offsets, streaming digests, processing outcomes and accepted/abandoned history
decisions. Assistant origins refer to observed chat attempts and stable scopes.
Partial events use the same turn and block lifecycle as the shared TypeScript and
Rust readers, tested against the repository-owned partial-stream vectors.

The Query exposes validated model_request and model_response envelopes as evidence
types in SDKMessage. These types have request/response fields, not conversation
message fields, and never enter resume history. Failure is latched by the existing
Query lifecycle so the accepted prefix remains visible and subsequent work fails
with the first refusal. A locked Query refuses records from another session before
delivery. Natural EOF checks unfinished evidence and partial output, and requires
at least as many root results as submitted inputs that the CLI processes. Inputs
with no text or other content do not create a turn in the CLI. Intentional close
continues to use the explicit cancellation lifecycle.

This interpretation belongs in the shared Python SDK for every caller, including
vLLM-backed sessions. The shared outbound control schema admits named cancellation,
which the output adapter and SDKs already support; unnamed cancellation belongs to
the CLI input direction. Both SDKs validate controls before dispatch. Shared Core
admission and Python refuse repeated terminal identities: both CLI terminal
constructors assign a fresh UUID for each emission. It changes no model request,
canonical chat recording, or resume replay. Backend-owned omissions require backend
changes and cannot be repaired by this reader. The same schema is shipped as data,
not maintained as a second Python schema.

The shared JSON output adapter publishes an init containing the contract identity
and journal origin before the first record of each output window. An explicit
normal init supplies this boundary itself. When authentication or configuration
fails before normal initialization, the adapter can publish the boundary without
accessing unfinished configuration services, followed by the exact structured
error. The interactive dual-output channel keeps its session-start capability
handshake first, then publishes the journal init before any model evidence.
Authentication failure waits for output flush before cleanup and exit.

Verification covers complete and corrupted full/delta bodies, resumed journal
windows, multiple invocation scopes, response bytes and disposition, unknown
versions/types, invalid known records, Unicode scalar validity, partial lifecycle,
and Query delivery before session-state changes. Ordinary controls, synchronous
consumption and live multi-turn sessions remain part of the integration coverage.

This work does not establish a causal link between each queued SDK input and its
result. Continuations and monitor turns also emit root results; the wire has no
input identifier on the terminal. Exact causal completeness needs a producer-owned
correlation contract. The input count supplies a lower bound, not that proof.
TypeScript request replay additionally requires JSON.stringify identity; Python
uses Rust's byte-preserving interpretation, including valid raw numeric and escape
spellings. Neither reader may rewrite the committed request body.
