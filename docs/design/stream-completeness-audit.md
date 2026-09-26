# Stream completeness audit disposition

This checks the questions in `/tmp/codex-intervention-1.md`; it does not adopt
that file's findings or its withdrawn order of work. The source checkpoint is
agent_service `023769e` and backend `6883ba2`. Client paths below refer to the
source produced by the pinned landmark transformer, not an independently
maintained client checkout. At this checkpoint, a fresh pinned archive matched
all 995 declared final identities and the reviewed authoring source.

“Source evidence” below means the named control flow was read. It does not mean
a native refusal, compiler result, release, or deployed behavior was observed.
“Executed source tests” means local TypeScript or Python tests actually ran;
it does not mean the application or an image was built. No compiler, typecheck,
native test, build, image qualification, release, deployment, or push was run
for this review. Owner gates remain authoritative.

## Contract binding and exhaustive cases (§1)

The assertion that the engine failed to compile for ten commits is unsupported
and withdrawn. There is no execution evidence establishing it. A narrower
source inconsistency did exist in committed `60e08e2`: `schema_compiler.rs`
contained two literal `:3` checks while `build.rs` selected the `:5` definition.
Commit `ca01c68` derives the identity from the selected definition. The current
source contains no such literal requirement. This distinction and the unrun
Rust tests are recorded in [the binding note](stream-contract-identity.md).

Source inspection found explicit current cases for all seven native event
kinds, thirteen system kinds, eight partial kinds, and 21 interactive
GeminiEventType members, including AttemptStarted. The historical interactive
omission does not remain in this checkpoint. This is a textual inventory, not
an exhaustive compiler proof or a claim about unexamined switches.

## SDK scope, initialization and public options (§2)

The blanket claim that SDK work is outside this goal is not established. SDK
process readers consume the versioned stream; interpreting request, response,
and attempt records there is relevant to making the record interpretable end
to end. The image's CLI/webui build does not establish Java or Python SDK
adoption. Source implementation and any separately reported SDK tests must not
be described as image qualification. Sound reader work is retained.

Two narrower defects are real by source reading and remain open:

- `BaseJsonOutputAdapter.messagesWithRequestEvidence` synthesizes a minimal
  `init` when evidence precedes normal initialization. It omits the metadata
  that native `RuntimeContract.validate_init` requires. Schema acceptance does
  not reconcile these two shapes. Initialization needs one complete owner.
- `TimeoutConfigSchema` omits the removed `timeout.streamClose` option and is
  not strict. `createQuery.validateOptions` checks `safeParse` success but
  continues with the original options object. The precise defect is silent
  acceptance and non-use, not necessarily stripping the caller's object.
  The option must have a defined purpose or an actionable refusal by name.

## Native certification and output disposition (§3)

The following control-flow gaps are confirmed by reading
`protocol/engine/src/model_requests.rs`; no native adversarial test was run:

- `validate_output_origin` returns immediately for runtime origins. Runtime
  output is legitimate, but that label alone does not establish which runtime
  operation accounts for it. Inventing a model attempt for runtime output would
  be wrong; the runtime owner needs explicit accounting.
- Model output checks the attempt's identity and scope without binding that
  output to its final history disposition. Aggregate request/response counts
  in `validate_summary` do not provide that per-attempt association.
- Response completion validates observed byte counts and digests and some
  termination conditions, but a processing outcome is not linked to the
  transport termination. The audit's broader implication that transport
  completion is unchecked is false.
- There is no comparison tying displayed user/tool-result content to its
  admission and rendering in the next provider request. Such a comparison
  must account for reminders, media rendering and rejected inputs; simple
  string equality would reject valid ordinary sessions.

Charging served output from an abandoned draw is correct. Served work and
history acceptance are distinct facts. The defect is missing disposition
accounting, not the inclusion of served work in usage totals.

The stdout adapter's `emitSubagentRound` consumes origin, reasoning, text,
malformed calls and usage but drops `historyDisposition`. This is a real
remaining publication gap. The canonical and ACP acceptance fixes are not
invalidated by that separate stdout omission.

## Canonical resume and listing (§4)

The location and listing problems are real by source reading:
`decodeChatRecord` wraps JSON decoding with the filename and record location,
but invokes `requireTranscriptRecord` outside that wrapper. `listSessions`
has no per-file refusal handling around its canonical reads. One unsupported
file can therefore fail the listing operation. The fix must identify the file
and expose its refusal without losing access to other readable sessions.

Unknown and unversioned canonical formats are deliberately refused. The claim
that every deployed file is unversioned has no inventory evidence and is not
adopted. Version 7 cannot infer complete runtime history from older display
records. The current README discloses the available actions: inspect those
files with their matching client or begin a new session. Relabeling an older
file as complete version 7 would fabricate evidence.

The structural-exclusion concern is repaired in `023769e`. The accumulator
accepts `RuntimeHistoryCommit`, a union of history changes and positioned
assistant commits. Physical record projection is a separate boundary; raw
display records and request/response evidence are not accumulator inputs.
Executed source tests cover ordinary and indexed readers, exact Content
boundaries, image pools and active-chain corruption before a later checkpoint.
The independent frozen-candidate run passed 50 cases; two later cache-copy
checks passed separately. See the transformed source design note
`docs/design/runtime-history-recording.md` for scope and unrun gates.

## Recording stalls (§5)

The source risk is real. The stream deadline clock pauses during evidence
writes; response cancellation awaits both reader cancellation and the pull
that may itself await a writer. A storage operation that never settles can
therefore defeat those stream deadlines. Durability performs synchronization
of captured response writes. No hung-filesystem execution was performed in
this audit, and no latency measurement is claimed.

The remedy belongs in shared recording/cancellation ownership. It must bound
failure without reporting incomplete bytes as durable evidence, discarding
failures, or simply reinstating a transport timer that misattributes local
storage delay to the provider. This remains open.

## Model-visible changes and backend scope (§6)

The behavior changes are real; the allegation that they were undisclosed is
contradicted by the commit bodies:

- `294c018` explicitly describes removing runtime acknowledgement words from
  the carried model turn and preserving a separate acknowledgement where
  attachments require it. Its body also states that the original defect
  affected actual model-facing history, not just a record projection.
- `c0cefb5` explicitly describes removing structured-output payload replacement
  and plan-argument rewrites. Canonical storage now retains complete model
  arguments, including values upstream redacted. This is a material privacy
  difference: payload contents remain on disk. It is required by the owner's
  literal-record requirement and must not be described as preserving the
  upstream redaction policy.
- Backend `6883ba2` removes whitespace-only content deletion in shared parsing,
  for both streaming and batch output. A client cannot recover bytes removed
  before the response reaches it. The change applies to direct vLLM callers
  and agent_service callers alike; it is not a benchmark or context-size
  exception. Its commit body expressly gives this reason and separates source
  resealing from image adoption. No new backend execution is claimed here.

These changes are retained. Current canonical replay continues to preserve
literal model parts. The standing brief's requirement to fix omissions in the
backend that owns them supports the backend change.

## Remaining mechanisms and limits (§7)

These questions identify real source facts and unfinished work:

- `ProviderOutput.observe` uses `JSON.stringify` on decoded provider objects.
  The resulting `rawResponses` fields are not response-byte evidence. They
  coexist with the actual byte journal. Remove redundant recording or give a
  necessary decoded projection a distinct, truthful meaning; do not call
  reserialization raw wire bytes.
- `ModelRequestJournal.capture` emits a full body when two requests in one
  segment share no message prefix. A delta with zero retained messages can
  represent that state; the additional full-body mode is unnecessary.
- ACP's tool-result projection still uses a 65,536-byte JSON budget and a
  truncation marker. Removing the stdout cap did not remove the ACP cap.
- PostCompact receives `outcome.renderedSnapshot`, made from the unfilled
  snapshot rendering. It does not receive the composed history's populated
  user-message section. The hook projection needs the same truthful inputs.
- Tokenizer and embedding dispatches bypass the chat request journal.
  Tokenization is a sizing operation rather than a generation, and embeddings
  have a different request/response shape. Their evidence must describe their
  actual operations instead of pretending they are chat generations.
- The indexed transcript reader refuses snapshots above 256 MiB; native event
  parsing has a 128 MiB record limit. These are refusals, not silent truncation.
  Larger evidence increases exposure to them, but that capacity impact was not
  measured here. Any correction must preserve bounded ordinary-session memory
  and derive limits or state their reason, not merely raise constants.

Different historical definitions used the same numeric version labels. That
fact does not imply they are indistinguishable: exact schema SHA-256 binding
distinguishes definition bytes. Version labels alone are insufficient. This
review did not inspect deployed versions or prove migration compatibility.

## Reader coverage, retries and historical wording (§8)

The broad “every transcript reader” claim in `38d83aa` was too strong. Current
source still has two concrete omissions:

- Insight `DataProcessor` reads chat files through the permissive generic
  `read` helper in its statistics and facet paths. That helper can recover or
  skip malformed fragments rather than apply canonical record admission.
- Desktop `qwen-agent.ts` parses canonical transcript lines directly in text,
  telemetry and history projections. The examined loops continue after JSON
  errors and do not validate canonical recording versions. This is a source
  consumer gap regardless of whether the image ships that application.

The blanket retry allegation is also too strong. `BaseLlmClient.generateText`
and `generateJson` use `retryWithBackoff`; compaction and title paths route
through that owner. Prompt hooks directly race `generateContent` against
timeout/cancellation, and direct ACP `executeGeneration` calls the generator
without that outer retry owner. Setting provider retries to zero therefore
changes those latter paths. Their behavior needs an explicit shared retry
decision; this audit did not execute their failure paths.

The historical AttemptStarted gap is repaired in current interactive source.
That does not retroactively establish the historical diagnostic claim. The
`e575b3b` phrase “deployed browser views” was too broad for web-shell. The
[child provenance note](child-attempt-provenance.md) now distinguishes image
built webui from source-tested web-shell. The binding note likewise corrects
any implication that unexecuted native refusals were runtime-verified. Commit
history is preserved.

## Continuing work

The runtime-history splice is complete and committed; the wider goal is not.
An independent source test reproduced a summary overwriting input admitted
while compaction was running, on both the baseline and candidate. That race
remains open alongside the items above.

The separate tool-restore defect was reproduced against `023769e`: after a
new chat, restoring an image-bearing checkpoint lost its image payload pool.
An owned reference failed rendering and both canonical readers; a literal
image ID silently stopped resolving. The correction stores complete runtime
state in a versioned tool checkpoint and replaces history, image payloads and
the startup declaration through the shared Chat/Client owner. Capture freezes
state before filesystem waits and serializes one checkpoint at a time. Restore
validates the version and complete runtime state before file rewind, then
awaits canonical persistence before UI success or tool replay. Invalid UTF-8
also refuses before rewind; valid literal replacement characters remain valid.

Executed source tests against the final candidate passed 20 independent cases
using the real checkpoint hook, Chat/Client, canonical writer, and ordinary
and indexed readers. These include reset/restore, payload ownership, storage
delay/failure, capture during filesystem waits, malformed state and invalid
UTF-8. The full restore suite passed 36 cases; Client, Chat and runtime-history
suites passed 653. The unchanged slash-command and streaming-session suites
passed 126 and 222 cases before the final two-file decoding correction. The
transformer framework passed 38 tests. These are source-path results: file
rewind, scheduling and UI use controlled test boundaries; no provider or tool
was executed. CLI source aliases read actual HTML and refuse if the unrelated
insight renderer is invoked; packaged/generated assets were not qualified.

The tool snapshot's display-history check validates its envelope, not every
UI item's payload schema. Pending recording commits block replacement, but
the guard does not serialize every active generation with history mutation.
A persistence failure after admission may leave memory installed or files
already rewound; it prevents successful replay rather than promising rollback.
Older tool snapshots without complete state are refused by filename, with
instructions to use their matching client or create a new checkpoint.

This correction serves Qwen history replacement regardless of provider or
session length. Direct vLLM callers have no Qwen checkpoint or image store;
there is no corresponding backend state to change. Compilation, native
refusals, images and deployment remain unverified pending owner gates. Each
remaining correction belongs to its shared owner and must preserve ordinary
sessions, with source proof, executed tests and unrun gates reported separately.
