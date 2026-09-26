# Java CLI SDK versioned record admission

The shared Java session must admit the complete stdout record before updating
state, matching controls, projecting a DTO or invoking a callback. Framing alone
does not establish that a complete JSON value has the required meaning. This
work follows the shared v5 contract and the request/response and partial-stream
state machines already used by Core and Python.

The SDK packages the exact authoritative schema as a resource, bound by the
source transformer. A strict JSON parser rejects duplicate members, non-scalar
strings, non-JSON syntax and trailing values. Schema evaluation supports the
owned schema vocabulary and refuses an unsupported definition. Numeric checks
retain exact decimal values. Parser buffers cannot become a silent evidence
truncation or an arbitrary long-record limit.

Admission verifies the declared contract digest, request journal and sequence,
full-body or delta replay against exact byte count and SHA-256, response bytes
and completion, attempt ownership and disposition, terminal summaries, partial
block ordering, usage arithmetic and goal checkpoint relationships. Reconstructed
request JSON retains its original number, escape and member spelling; it is not
reserialized for equality. The first refusal remains the reason subsequent work
is unavailable.

Every admitted record has an immutable complete representation with its original
JSON text and an exhaustive protocol kind. It is evidence, structurally separate
from conversation history. Session consumers receive this record before narrower
typed callbacks. Typed messages retain the complete admitted record as well.
Unknown records cannot fall through to an optional callback. A simple text view
may select text after shared admission, but that projection cannot control whether
validation and evidence replay occur.

Initialization and idle controls read until the matching response identity,
retaining intervening admitted records for delivery. Live controls share the
current reader and register their response identity. A child result cannot finish
a root prompt, and a control failure cannot manufacture a successful empty reply.
Callbacks and startup failures close operation admission with the original cause.
Natural EOF cannot certify unfinished initialization, output or model responses.

This belongs in the Java SDK for all ordinary and long CLI sessions, including
those using vLLM. Direct backend clients do not use this interpreter; backend
record omissions require separate backend fixes. No benchmark or executor
configuration selects these invariants.

The canonical runtime recording and resume replay are separate from stdout
evidence. This change must not insert request/response evidence into replayed
history or change model request composition. The same schema applies to fresh and
resumed stream windows. Exact queued-input to terminal causality remains a
producer-owned requirement; counting results is not proof of that relationship.

Verification uses exact-source JShell execution with the pinned Java parser
dependencies, independent reproduction and review, permanent Java regression
tests and shared schema/evidence fixtures. Maven, javac, package/application/image
builds and real model calls are excluded by the owner's instruction. Any package
or class adaptation used by source execution must be disclosed; it is not a full
compiled SDK integration run. Fresh pinned-archive application, all 35 semantic
concerns and two clean final self-review passes remain required before commit.

Normal close ends input and drains the process through EOF before calling the
admission completeness check. A root prompt result may declare background open
responses; it ends that prompt, not the session evidence window. An initialized
control-only process that has never opened an evidence stream can close without
inventing a model terminal. Closing during an active reader is an explicit
nonfatal refusal with a wait/interrupt action. New control callbacks after input
ends are refused after their complete record is observed. Existing callbacks are
cancelled, and late replies cannot write into the closed input.

Resume now ends the old process before restarting with the selected resume ID.
The first resumed stream record is checked against that selection. This changes
SDK process orchestration only: the canonical history writer and replay remain
unchanged. Source fixtures cover resumed journal windows, and deterministic local
process/pipe checks verify arguments and process ownership. They do not replace a
real CLI/model replay integration test, which the no-build instruction excludes.

Public view changes follow the existing schema: `onRecord` replaces generic
unknown-message delivery; arbitrary input/content retains its JSON value;
fragments stay strings; terminal integers use `BigInteger`; status events have
concrete types. `Session` is AutoCloseable, and Transport gains required graceful
`finish` semantics. The convenience query uses try-with-resources so EOF or
cleanup refusal cannot be logged and reported as success. No wire record or
request shape changes, so the strict fake provider shape adapters are unchanged.
