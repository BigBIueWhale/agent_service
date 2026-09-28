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

Two narrower defects are real by source reading:

- `BaseJsonOutputAdapter.messagesWithRequestEvidence` synthesizes a minimal
  `init` when evidence precedes normal initialization. It omits the metadata
  that native `RuntimeContract.validate_init` requires. Schema acceptance does
  not reconcile these two shapes. Initialization needs one complete owner.
  The current authoring candidate separates `system/stream_start` (session,
  contract and journal identity) from complete `system/init` runtime metadata.
  Authentication can fail before Config initializes, so inventing a ready
  snapshot there would misstate capabilities. Native source still requires
  manifest-matching init before model requests and successful terminal output;
  zero-request startup errors can be represented without that claim. Reader
  replacement-window guards also retain each preceding terminal's accounting.
  Seven Python source suites passed 575 cases for the migrated candidate,
  including complete dual-channel accounting and refusal of unfinished
  root/child partial groups. Seventeen CLI source tests cover the producer's
  retained-prefix and journal-replacement boundaries and recording diagnostics;
  those producer tests stop before generated wire admission.
  The authoritative client splice contains this change. Native, Java and
  generated TypeScript admission remain unexecuted. The design and qualification
  limits are recorded in [generation authority](generation-authority.md).
- `TimeoutConfigSchema` omits the removed `timeout.streamClose` option and is
  not strict at the audited checkpoint. `createQuery.validateOptions` checks `safeParse` success but
  continues with the original options object. The precise defect is silent
  acceptance and non-use, not necessarily stripping the caller's object.
  The authoring candidate now makes timeout validation strict and adds an
  actionable removal instruction to unknown-key diagnostics. Four source tests
  reproduced silent acceptance before that change; all 84 tests in the affected
  schema/query suites passed afterward. Review identified the exported `Query`
  constructor as a second public entry point; two more cases reproduced that
  bypass. Both factory and constructor now use one validator, and the three
  affected suites passed 86 tests, including constructor refusal before transport
  or abort-listener access. The developer guide names only the three supported
  controls. The authoritative client splice contains this change;
  this does not establish built SDK or CLI behavior.

## Native certification and output disposition (§3)

The following control-flow questions were checked against current native source.
The transport correction below closes one real gap. No native adversarial test
was run:

- `validate_output_origin` returns immediately for runtime origins. Runtime
  output is legitimate, but that label alone does not establish which runtime
  operation accounts for it. Inventing a model attempt for runtime output would
  be wrong; the runtime owner needs explicit accounting.
- The allegation that an abandoned model partial can become an accepted round
  is contradicted by the current path. `validate_output_origin` checks attempt
  identity and scope, then `PartialStreamState` checks text, thinking and
  incomplete-call prefixes against that attempt's generation. Its
  `observe_completion` requires the same origin and retains the accepted or
  abandoned decision;
  `finish` refuses a model partial without that completion. A `tool_use`
  partial requires accepted completion, and `RuntimeContract` adds tool uses
  and root model text only for accepted completion. Abandoned output remains
  visible and billed as served work, with its abandoned decision retained.
  This is a source-reading conclusion, not an executed native refusal. The
  separate native physical-byte replay and runtime-origin ownership questions
  remain open.
- The baseline response completion validated byte counts, digests and some
  termination conditions without linking processing success to transport.
  The shared writer and readers now require observed 2xx HTTP and an EOF or
  cancelled transport ending for completed processing. The audit's broader
  implication that transport completion was unchecked is false.
- There is no comparison tying displayed user/tool-result content to its
  admission and rendering in the next provider request. Such a comparison
  must account for reminders, media rendering and rejected inputs; simple
  string equality would reject valid ordinary sessions. The request body
  captured after `buildRequest` is the authority for what was dispatched.
  A stream `user` or `tool_result` body is a display projection, not a copy
  of that provider body. Source tracing found that a partial tool response
  could lose its output from this projection when an error was present.
  The current authoring splice retains the response-part display text and
  distinct top-level or embedded errors; four source assertions reproduced
  loss and the Base adapter's 146 cases passed afterward. This presentation repair
  does not bind input provenance or prove that the display text is identical
  to model input. An immutable input receipt and explicit rendered-request
  binding remain to be designed and admitted across producer and readers.

Charging served output from an abandoned draw is correct. Served work and
history acceptance are distinct facts. The defect is missing disposition
accounting, not the inclusion of served work in usage totals.

The stdout adapter's `emitSubagentRound` consumes origin, reasoning, text and
malformed calls without copying the direct `historyDisposition` field onto its
partial rows. Source reading contradicts the audit's description of an
"ordinary assistant record": this path emits `stream_event` groups, no full
assistant row. The same attempt has generation and completion records keyed by
origin, and completion carries the accepted or abandoned decision. The partial
reader requires that completion before closing a model origin and permits a
tool claim only after accepted completion. Repeating the decision on every
partial would create a second authority for the same fact. Per-attempt output
reconciliation against physical response bytes remains open. The focused CLI
test was attempted in the current source checkout but could not collect because
the installed `@qwen-code/web-templates` package has no resolvable entry; this
paragraph is source-reading evidence, not a newly executed CLI test.

## Canonical resume and listing (§4)

The location and listing problems were real at the audited checkpoint:
`decodeChatRecord` wrapped JSON decoding with physical location but invoked
`requireTranscriptRecord` outside that wrapper; `listSessions` lacked file-local
refusal handling. The completed catalog correction described below closes these
paths and propagates their diagnostics through the identified consumers. It
keeps explicit readable selection available and refuses ambiguous automatic
selection. It does not declare an incomplete catalog complete.

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

The current 14-case runtime-history source suite exposed a stale fake generator:
11 cases failed because it emitted decoded Parts without an admitted physical
request or response. Its fixture now serves in-memory OpenAI SSE through the
actual SDK and shared pipeline, retaining the same ordinary, tool-result,
compaction and post-compaction history comparisons. All 14 cases then passed.
The ordinary case checks that the canonical file contains request, response,
accepted generation and attempt-completion evidence before comparing live
history with both resume readers. This proves those tested source paths with a
local fake response; it does not run the packaged `--resume` command, a deployed
provider, compilation or the owner's gates. Full end-to-end resume remains
unverified.

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
storage delay to the provider.

The current authoritative splice adds an inactivity refusal to the shared
asynchronous canonical lease's append and verification operations. Public
waiters fail while the raw filesystem queue and active lock remain owned;
late continuations cannot start queued writes or commit a new proof. Bounded
append chunks and hash reads refresh progress rather than imposing a total
deadline on a large transcript. Sixteen source tests passed, including late
I/O, exact lock retention, progressing large operations and recorder flush/close
propagation. The broader source run passed 170 tests, skipped two, and failed
two generation-format resume fixtures. Those fixtures have since been migrated
to complete request, response, generation, history and completion records. The
full writer-lease source file passed 103 cases with two existing skips, and the
two affected handoff cases passed again after their physical event order was
aligned with the producer. These runs do not qualify an application resume or
native admission.
The bounded read-only review at
`/tmp/codex-writer-stall-review/revision-1/REPORT.md` found no introduced defect
and matched all ten frozen source/document identities. It inspected the supplied
logs without rerunning tests. The complete change audit remains unfinished.
The sealed patch then applied to a fresh pinned archive and matched all 1,129
declared paths, including 40 deletions, to the authoring source. Semantic source
contracts passed; the manifest and protected release inputs stayed unchanged
throughout that transaction. The report is
`/tmp/codex-writer-progress-final-state.json`, at review patch
`133dfe8824f60cfa68feadc3b7329764cc48c982bdade648e90662e363900630`.
This proves source application and identities, not compilation or deployment.

This closes only the asynchronous canonical append/verification wait. Shared
response cleanup can independently wait on a stalled transport cancellation,
and child-transcript writers use synchronous I/O that an event-loop timer cannot
interrupt. Those obligations remain open. Healthy acquisition and terminal lock
transitions have separate ownership semantics and are not bounded by this
policy. See [the recording progress design](session-recording-progress.md) for
the availability tradeoff, executed evidence and unverified gates.

The shared response-cancellation question was reproduced separately with the
actual source recorder. A second cancellation could falsely publish a clean
outcome while the first remained pending or later failed. The authoritative
splice gives raw cancellation one shared promise, a one-minute cleanup
deadline, and a settled exposed read on normal recorder cancellation. Its 60
response-recorder and 21 adjacent pipeline source cases passed; an independent
source review reran the 60-case file and found no remaining defect in this
bounded change. The first candidate had dropped bytes already returned by a
raw read as cancellation began; the final version records those bytes before
suppressing consumer delivery. The final splice applied to a fresh pinned
archive with all 1,133 declared path identities, 44 deletions and semantic
contracts matching the authoring source
(`/tmp/codex-response-cancel-admitted-state.json`). See the
[recording progress design](session-recording-progress.md) for the exact
boundary and remaining iterator/publication waits. This source application
does not qualify a native, application or deployed gate.

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

These questions identify real source facts. Corrections are marked below;
the other items remain open:

- `ProviderOutput.observe` uses `JSON.stringify` on decoded provider objects.
  The resulting `rawResponses` fields are not response-byte evidence. They
  coexist with the actual byte journal. Remove redundant recording or give a
  necessary decoded projection a distinct, truthful meaning; do not call
  reserialization raw wire bytes.
- `ModelRequestJournal.capture` emits a full body when two requests in one
  segment share no message prefix at the audited checkpoint. This question was
  confirmed and the producer fallback is removed in the current authoritative
  splice. A same-scope, same-segment base selects a delta even when it retains
  zero messages, including removal of the whole list. Scope/segment starts,
  explicit checkpoints and replacement output windows still establish full
  bases. The strengthened source test failed before the change; all 24 request
  and canonical-recording cases passed afterward. It checks exact reconstructed
  bytes and refuses a detached zero-retained delta independently of sequence
  checks. All five examined readers already support zero-retained deltas by
  source reading; native and Java were not executed. The shared harness test
  also exposed a missing-base `KeyError`, now an explicit refusal with a recovery
  action. Its nine tests passed, including replacement/removal and wrong base,
  segment and scope. Logs: `/tmp/codex-zero-overlap-before.log`,
  `/tmp/codex-zero-overlap-after.log`,
  `/tmp/codex-zero-overlap-harness-after.log`.
  This changes the shared evidence representation, not the bytes sent to the
  provider. It applies to ordinary sessions and side queries alike; vLLM receives
  the same request and needs no corresponding backend change. No size or latency
  improvement is claimed: a zero-retained delta includes reference metadata.
  A later reader audit found that a second full body with the same scope and
  segment was accepted as a fresh base despite the producer's delta rule. The
  client now records a segment identity composed from its output-window epoch
  and the history context's segment, so window replacement cannot reuse the
  old identity. The TypeScript, Python, Java, native and fake-provider readers
  refuse a full body while that scope's segment is active; they still admit a
  full base when either component changes. The TypeScript request/capture suites
  passed 29 tests, Python admission passed 344, and the shared fake-provider
  verifier passed 11. Java and native tests, compilation, and owner gates remain
  unverified. This rule authenticates request-chain representation; it does not
  by itself prove that a claimed new segment corresponds to a particular
  compaction or that request messages match every prior user/tool record.
- ACP's separate 65,536-byte projection was real. The shared live and replay
  boundaries now preserve complete content blocks and raw output, and the
  projector, byte-slicing utility and their algorithm-specific tests are
  removed. Complete-payload accounting still refuses an aggregate replay
  above its byte limit. Separate transport/frame limits remain explicit
  refusals, so this is not a promise of unlimited delivery capacity.
  The executed source baseline reproduced nine failures with four controls
  passing. Final runs passed 11 selected live/plan cases (616 unrelated cases
  filtered), all 39 replay-page cases, and 33 NDJSON transport cases. The replay
  fixture migration is checked against stored-record admission; the intermediate
  null-usage change was removed after review showed its fixture was invalid.
  Malformed nonnull counts still refuse. See
  [the ACP design and evidence note](acp-tool-result-completeness.md).
  These results qualify the tested publication/replay boundaries, not complete
  physical evidence admission, model-history restoration or owner build gates.
  The final read-only review is
  `/tmp/codex-acp-completeness-review/revision-2/REPORT.md`; its seven reviewed
  identities match authoring/root. The fresh pinned-source application at
  `/tmp/codex-acp-completeness-admitted-8qltb8z3` completed with zero identity
  mismatches across 1,133 declared paths, including 44 deletions. Semantic
  contracts, manifest checks and the 35-concern identifier check passed.
  `/tmp/codex-acp-completeness-admitted-state.json` records the exact seals and
  execution scope. Protected build-input and release-lock bytes remain unchanged.
- The PostCompact empty-slot projection was real at `023769e`. The compaction
  ownership correction below takes the text from the actual composed snapshot
  message, including retained input and trailer text. The complete Content,
  including media, remains in the canonical checkpoint; the hook field is text.
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

The broad “every transcript reader” claim in `38d83aa` was too strong. The
Insight omission was real: its statistics and facet paths used the permissive
generic `read` helper and could skip damaged records while publishing a report.
The shared complete canonical reader now admits the physical file and projects
its active conversation chain before Insight counts or analyzes it. An unreadable
or incomplete file refuses the report with its path and a possible next action;
an inactive branch and model-evidence records do not become activity. This is
source implementation, not an image qualification. The focused canonical,
Insight and command source tests passed 66 cases. The adjacent static-generator
suite could not start because this source checkout has no resolvable
`@qwen-code/web-templates` package entry; no build was run to produce one.

One concrete source consumer omission remains:

- Desktop `qwen-agent.ts` parses canonical transcript lines directly in text,
  telemetry and history projections. The examined loops continue after JSON
  errors and do not validate canonical recording versions. This is a source
  consumer gap regardless of whether the image ships that application.

The Desktop package declares a separately vendored Qwen runtime at 0.15.11,
while the current patched Qwen source requires canonical recording version 13.
Hard-coding that version into Desktop's local JSONL loops would bind them to a
different runtime source and could reject an ordinary session from its pinned
runtime. The patched ACP `qwen/session/loadUpdates` path already asks the
runtime to produce validated history; the remaining local text, telemetry,
slash-command and text-element projections still read the file independently.
They need a runtime-owned validated projection, with an explicit compatibility
decision for the older vendored runtime, before the direct loops can be removed.
This is a source ownership conclusion, not an implemented repair or a measured
runtime compatibility result.

The shared ACP transcript replay now projects a visible `system/slash_command`
invocation as a user update only when its canonical payload says it was not
sent to the model. It keeps hidden invocations hidden and leaves model-submitted
commands to their actual user records. This brings one source-owned view into
the validated replay for every ACP consumer; it does not remove Desktop's
direct JSONL readers for text elements, telemetry or older runtime sessions.
The focused bridge cases are authored but unexecuted here because Node and Bun
are unavailable. The reviewed patch applied to a fresh pinned archive and the
bridge source and test files matched the authored source byte for byte. This
proves the source splice, not application behavior.

Desktop's separate ACP history path also ignored the server's explicit
`partial` and `replayError` reply after a replay failure, filtering malformed
updates out and treating the remaining prefix as complete. Source reading now
shows this path refuses partial or malformed replies and propagates extension
errors; only an ACP method-not-found error uses the existing `session/load`
path for an older runtime. The Desktop session manager also now propagates a
Qwen canonical history refusal on session load, leaving it eligible for retry,
and retains a placeholder mirror when catalog inspection fails or has no
reader or payload instead of deleting it as empty. This leaves the direct
canonical-file loops above open.
The ACP and manager boundary tests are authored but unexecuted here because
Bun is unavailable. vLLM does not own Desktop history interpretation, and the
sealed service image does not ship Desktop.

A later source read found that the synchronous canonical scanner validated
individual records but did not check its model-evidence completion state at
EOF. The review cost ledger and agent-transcript report used that scanner and
could therefore report on a file with an open physical request or logical
generation. They now use a synchronous read of a complete snapshot that applies the
same completion refusal as canonical resume. The live-prefix reader remains
available for metadata while a writer is active. A source regression fixture
contains a valid open utility request and checks that only the complete read
refuses it. This is source implementation and an authored test, not an executed
client or native gate; Desktop's direct parsers remain a separate gap.

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
The compaction race was reproduced again on `20ee5e7`, through the actual
compression service. Input admitted during generation, service preflight,
candidate counting or PostCompact disappeared when the old candidate was
installed. Three failure controls retained input but reported an obsolete
retained count. Both canonical readers reproduced whatever state was installed;
reader equality alone did not establish that the admission decision was right.

The correction binds count and acceptance to a semantic history revision,
while exempting lossless image representation changes. Stale success becomes
a history-changed refusal with an unknown retained count; an earlier failure
keeps its own reason. All generated draws remain recorded. Success telemetry
and PostCompact run after durable acceptance. Input admitted during that hook
follows the accepted checkpoint, and hook failure retains the committed result
through the existing finalization-error path. Native source admission now
requires a present retained-count field and permits null only on failure.

Executed candidate evidence comprises 14 independent source cases with actual
Chat, compression service, writer and both readers; 454 core tests across Chat,
service, turn and runtime recording; 245 CLI tests; and two targeted ACP cases
(617 unrelated ACP cases filtered). The 38 transformer framework tests passed.
Independent cases include the ordinary send's precomputed count, lossless
image conversion, real append failure, held recording and flush completion,
and deferred hook mutation/failure. Generation and token-service boundaries
are deterministic local fixtures. Native tests are authored but unexecuted;
their schema prerequisites and guard order were reviewed in source. These
results do not establish compiler, native runtime, provider or image behavior.

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

## Session catalog correction after f9edfea

Canonical syntax, version and request-evidence admission now share a decoder
that retains the physical file/line or byte offset and original cause. The
ordinary and indexed readers use it without admitting evidence into the typed
runtime-history accumulator. Resume continues to read the client's runtime chat
JSONL, never `output/events.jsonl`; this correction changes refusal ownership,
not the saved conversation or its replay order. Independent actual-writer and
both-reader tests establish exact runtime-state equality for their fixtures;
that does not establish native execution or all possible histories.

The shared catalog returns readable rows with required file-local refusals.
Directory errors fail the request, cancellation retains its reason, and vanished
files remain absent. A failed stat retains unknown ordering. Timestamp ties stay
on one page; a cap inside a group is explicitly incomplete and cannot supply a
cursor that skips its tail. Automatic latest selection refuses an unreadable
newer, equal-time or unknown-order candidate. Live can use an established target
when the core scan proves every unscanned candidate is strictly older. The catalog
result remains explicitly incomplete. Explicit selection of a readable session remains
available even when automatic selection cannot be justified.

HTTP/ACP and default, organized and filtered daemon pages preserve diagnostics.
Unreadable identities are not replaced by live summaries; refusal entries count
against cache and scan capacity. SDK array helpers refuse to erase metadata and
retain the page on their error. Browser reload APIs return that page; the MCP
session-list tool exposes a cursor. The terminal has real cursor/archive options,
keeps JSON rows on stdout, reports refusals on stderr and sets a nonzero status.
Picker warnings preserve visible selection even when a later page adds them.
Completion, browser/IDE selectors and Live startup expose partial discovery.
Live's startup notes intentionally tell the model that recent-session context
is partial; presenting unreadability as absence would be false input.

VS Code has one ACP catalog and uses its existing canonical offline reader for
explicit history; the redundant permissive JSONL path is removed. Desktop
upserts readable rows but permits absence-based deletion only after a complete
scan without diagnostics. Refresh errors and completed partial results remain
visible through the refresh event and its cached-state replay. Its renderer
notification path was inspected in source; the Electron UI and IPC were not run.
Unversioned files remain deliberately refused, with file and matching-client or
new-session actions. There is no evidence that all deployed files have that
format, and no deployed migration or adoption is claimed.

Executed source evidence for this correction:

- 30 final independent core/Live cases passed, repeating the prior 23 and adding
  actual 10,001-file older/tied boundary and unknown-order controls. They use the
  actual core catalog and default private Live selection owner; they do not
  qualify public Live startup, provider/socket or lifecycle behavior. Permanent
  coordinator tests separately execute public start using supplied page fixtures.
- 12 final independent picker/completion cases passed, including actual Ink
  frames, later warning/error resizing, follow navigation and the real completion
  hook chain into InputPrompt.
- 35 independent adapter cases passed for daemon/catalog/cache, in-memory HTTP
  and ACP handlers, CLI output and Live selection/status. This run preceded the final Live
  boundary and completion wording edits; the final paths have the separate
  evidence above. It is not an application or transport deployment test.
- 64 independent SDK/VS Code/Desktop candidate cases passed; four additional
  baseline cases reproduce the old omissions. Whole production modules execute
  with HTTP/ACP/process/upsert/deletion boundaries stubbed. No physical deletion,
  process startup or full Desktop qualification is claimed.
- Permanent source suites passed: 351 core cases before the final boundary
  addition; 342 SDK cases; 125 browser catalog/dialog/overview/split cases; 83
  sidebar cases; 19 Web UI provider cases; and 16 VS Code cases. The earlier CLI
  run passed 147 of 149, with two obsolete empty-page assertions corrected and
  the full 27-case command suite then passing. Eight other files contributed
  122 passes. The final focused Live/picker/completion run passed 52 cases.
  Server/ACP catalog suites passed 70 cases; the final ACP/HTTP session-list
  selection passed 16 with 699 unrelated cases filtered.
- The transformer framework passed 38 tests and documented identifiers passed
  with the unchanged 35 semantic concerns. A first framework invocation had no
  repository PYTHONPATH and failed before importing tests; the correctly scoped
  invocation is the executed result. Early source-test failures and independent
  reproductions were retained; they are not counted as candidate passes.
- The final sealed transformation of the pinned archive passed. All 1,061
  declared final identities match the authoring source; exactly 74 paths changed
  from `f9edfea`, and authoritative edits outside those paths are unchanged.
  Manifest entries, README/stack identities and protected release inputs match.
  The 12 independent consumer files, eight picker/completion files and four
  core/Live owners match their executed final test snapshots. This is source
  transformation and byte-identity evidence, not compilation or qualification.

Final read-only gate inspection found a separate runner-ownership defect.
`docker/Dockerfile` discovers every patched `.test.ts`/`.test.tsx` and invokes
Vitest for its package, but Desktop declares Bun and its affected tests import
`bun:test`. Baseline `f9edfea` already discovers the unchanged Desktop slash-history
suite through that loop. The final discovery also includes native-history and
the adjacent SessionManager suite: three Bun suites among 448 discovered tests.
No Desktop Vitest configuration/bridge was found for these suites.
This is source evidence of a runner mismatch, not an observed image-build result.
The full gate remains unverified and needs a pinned Bun runner with mandatory
coverage preserved; excluding tests or dropping required fixture fields would
hide the defect. No gate bypass, dependency adoption or release seal was made.

This correction serves the shared Qwen recording/catalog owners for every
relevant caller, independent of model, session size or benchmark. vLLM owns none
of these local catalogs, so no backend change belongs in this correction. The
existing shared backend whitespace preservation is retained. Compilation,
typechecking, native certifier execution, full Desktop/Bun suites, application
packaging, provider behavior, image/release/deployment gates remain unverified.
No build, release, deployment or push was run. The other source-confirmed open
questions above remain work to finish; this commit does not certify complete
record sets or close the standing goal.

## Transport and processing correction after a6c1b10

The completed correction links processing success to captured successful HTTP
and an EOF or cancelled transport ending in the shared writer and TS, Python,
Java, native and fake-provider readers. It preserves zero-byte successful
responses, consumer cancellation reasons, failed/cancelled processing and the
separate history decision. Canonical resume reads the same runtime chat JSONL
and the same typed history; this change strengthens evidence admission without
changing conversation parts or their ordering. See
[the outcome note](model-response-outcomes.md) for the invariant and test scope.

Executed candidate source evidence comprises 56 independent TS cases, 59
recorder/real-SDK pipeline/attempt tests, 577 affected core unit tests, 721
independent Python checks, 274 SDK admission tests and seven helper methods.
The transformer framework passed 38 tests and identifiers retain 35 concerns.
The response settlement stub is confined to parsed-SDK unit fixtures that have
no transport; actual recorder and SDK/fetch evidence tests remain intact.
Native and Java regression tests are authored and source-reviewed, unexecuted.
No build, typecheck, release, deployment or push was run.

The fresh pinned archive transformation matches all 1,062 declared final
identities; exactly 12 intended client paths changed from the baseline.
Unrelated authoritative edits, manifest bindings and protected release inputs
were checked. Executed authored source matches the sealed result; the existing
generated test validator was not regenerated or qualified as a build artifact.

Independent baseline output tests rejected the blanket whole-stream disposition
loss claim: origins join the response-history decisions before child fragments
in the tested journal/adapter traces. They did reproduce admission of omitted
abandoned output, accepted text assigned to an abandoned same-scope attempt,
duplicated billed output and abandoned-origin tool calls. Source reading also
found empty billed root output and usage observed before a later stream error
can go unpublished. These and runtime/input ownership remain implementation
work; neither source test success nor patch integrity closes them.

## Terminal history settlement after d8f82de

The actual producer baseline confirmed a distinct ordering gap: terminal tool
calls and Turn's Finished event could reach the consumer before the physical
response-history append started. Canonical assistant acceptance already blocked
terminal delivery correctly. Sixteen independent source observation cases used
the actual SDK, pipeline, Chat, Turn and adapters with in-memory responses and
controlled recording callbacks. They establish consumer exposure, not premature
execution by a complete CLI scheduler or filesystem durability.

The shared GeminiChat stream now awaits one memoized response-history settlement
before publishing a non-null terminal decision. Cleanup awaits the same Promise;
early cleanup records abandonment. Early ordinary text remains streamable. Once
a terminal decision exists, recording failure cannot start another generation,
including empty and tool-only accepted output. See the
[settlement note](chat-attempt-settlement.md) for ownership, resume behavior and
verification limits.

This adds no record mode or version and leaves canonical history content and
order unchanged. It does not implement logical attempt completion or close the
remaining output, usage, runtime and input-accounting gaps above. Builds and
owner gates remain unrun.

Executed candidate evidence is 18 independent source cases and 446 tests across
the five relevant permanent suites, including the five new regressions. The
independent cases preserve observations of the remaining projection gaps. The
fresh transform matches all 1,062 final identities and the 17 recorded candidate
source identities, with only the two intended client paths changed. Framework
tests pass 38 cases and the semantic concern count remains 35. These results
establish the bounded source change and patch identity, not native or release
qualification or completion of the standing goal.

## Physical usage ownership after 9e22115

The physical response outcome now records the last complete valid cumulative
served-usage report observed by SDK processing, or explicit null. Observation
precedes conversion and error handling so a later failure cannot erase the
physical association. This closes the typed physical-usage omission reproduced
by 21 baseline source cases. It does not close the logical completion, output
projection or billing reconciliation gaps. See [the physical usage note](physical-response-usage.md)
for ownership, malformed-report handling, evidence and verification limits.

This shape requires stdout contract 6 and canonical recording version 8. Resume
continues to read structurally separate runtime history and evidence; history
content and replay order are unchanged by source reading. Older canonical files,
including version 7, are refused with their existing file-local diagnostics and
possible next actions. Full new-version canonical restoration remains unverified
pending the owner's binding generation and gates.

Executed candidate source checks comprise 37 independent actual-SDK cases,
517 affected permanent core tests, 1,135 independent Python/schema cases,
310 permanent SDK admission cases and eight fake-provider helper methods.
Five full-wire ChatAttempt cases encountered the intentionally unchanged version
5 generated validator. No substitute validator was installed and those cases
are unqualified. Native and Java cases are authored, not run. No build,
generator, typecheck, release, deployment or push was run.

The final fresh archive matched all 1,062 source identities and 26 saved test
source identities, with exactly 24 intended client paths changed. All 6,925
review hunks matched both coordinate systems; unrelated authoritative edits and
protected release inputs are unchanged. Framework tests passed 38 cases and
identifiers retain 35 semantic concerns. These checks qualify the source splice,
not compilation, owner gates or the standing goal's remaining implementation.

## Admitted acquisition failure after 4bc2592

The logical completion investigation reproduced another canonical omission:
provider stream acquisition can exhaust after durable physical requests, before
`processStreamResponse` ever starts. The actual SDK/child-writer baseline wrote
two failed physical responses but no canonical generation. A required stdout
publication failure after request persistence also left the request unattached
to its Chat attempt. These findings are executed source observations, distinct
from the audit's broader still-open certifier claims.

The journal now attaches the physical response after request persistence and
before publication. Shared Chat writes one existing-shape abandoned generation
on acquisition failure when at least one request was admitted. A recovered inner
retry and zero-request preflight do not acquire extra failure generations.
Canonical recording failure stays visible with the acquisition failure; eligible
outer preterminal retries retain separate attempt identities. See the
[acquisition note](acquisition-failure-recording.md) for the exact boundary,
source evidence and verification limits.

This adds no wire version or conversation content. Resume continues to read the
canonical runtime file; typed runtime-history projection excludes the failure
diagnostic. That exclusion and unchanged history admission are established by
reading; full version8 restoration and native certification remain unverified
pending owner gates. Logical completion/output binding, input/runtime accounting
and billing reconciliation remain required implementation work.

Executed evidence for this correction comprises eleven independent actual-SDK
and child-writer scenarios, 572 passing affected core cases including ten new
regressions, and 38 transformer framework cases. Five full-wire cases remain
unqualified at the unchanged generated v5 validator. A fresh archive matches all
1,062 final identities, twelve intended changed paths and 6,925 both-sided review
hunks; unrelated authoritative edits and protected release inputs are unchanged.
Compilation, typechecking, full version8 resume and native gates were not run.

## Current v7 output and tool ownership investigation

The source checkpoint above predates the uncommitted v7 generation/completion
candidate. A frozen reproduction of that candidate is recorded in
`/tmp/codex-output-disposition-review/baseline/REPORT.md`, with exact fixture
bytes and source identities. Thirty Python full-schema admission scenarios and
25 TypeScript replay/partial-owner scenarios executed. Several deliberately
demonstrate unsafe admission; they are not 55 passing correctness tests. The
installed TypeScript wire validator is still generated from v5, so full v7
TypeScript admission was not executed. Native, Java, provider and deployment
behavior remain unverified.

The earlier blanket claim that v7 omits whole abandoned generations or bills a
duplicated outcome is contradicted by these exact scenarios: both executed
readers refuse omitted or duplicate generation/completion records, wrong
same-scope partial attribution, and duplicate physical outcomes. Served usage
for abandoned work is accounted once without making that work accepted history.
This supersedes the projection observations in the older source checkpoint;
those observations are not evidence of the present v7 mechanism.

One narrower output defect is reproduced. Rehashing only `generation_json` and
its completion reference changes accepted text, including erasure, duplication
or swapping with an abandoned draw, while preserving every physical response
byte, transport outcome, history decision and usage count. Python admission
and the TypeScript replay owners accept the contradictory record. Native source
also lacks the physical-to-decoded comparison, but native behavior was not run.
The complete remedy remains open in
[physical output provenance](physical-output-provenance.md): a typed output must
be derived from or verified against the retained response body and its selected
physical retry. A self-declared hash or mandatory nonempty text cannot prove
that relationship.

The same full-schema Python investigation admitted a user/tool_result for an
abandoned call or a never-issued id. Native source already issues calls only
from accepted conversation generations. The current authoring splice adds that
accepted issuance, exact scope and once-only return invariant to Python,
TypeScript and Java generic admission. An independent follow-up reproduced
five analogous `tool_progress` ownership failures through Python full-schema
admission; native already checks that event. The same invariant now guards
progress without marking the call returned. The Python unit suite passed 727
cases, including the new ownership cases. The TypeScript wire suite stopped at the
unchanged generated v5 validator; Java and native tests were not run. This is
source implementation and Python test evidence, not qualification of those
other readers or the full record-completeness goal.

The read-only follow-up at
`/tmp/codex-output-disposition-review/tool-results-final/REPORT.md` matched 76
frozen source identities and found no remaining concrete ownership defect in
its bounded review. Its 19 result and seven progress controls overlap permanent
tests and are not additional distinct coverage. After the final TypeScript
event-type correction, a fresh pinned archive applied the sealed source patch
and matched all 1,133 declared identities, including 44 deletions, with no
semantic-contract or input mismatch. The exact report is
`/tmp/codex-tool-ownership-final-state.json` at review SHA-256
`d0f82e4498d46c5b6ed410c4e9da4ab9336de46355fea5c9b9b21dc4d3b9d3f4`.
This proves the source splice, not compilation or deployed admission.

## Runtime presentation coherence

Source reading found that a `runtime` origin exempted a full assistant row from
model-attempt ownership, while the native certifier and SDK readers did not
compare its text with the optional partial stream or the terminal result. In
the headless producer, `emitFinalAssistantMessage` emits the final local
slash-command or no-continuation message and passes that same text to `finish`.
The output adapter emits the full row before `message_stop` when partials are
enabled. These are producer control-flow findings, not a run of the CLI.

The native, TypeScript, Python and Java readers now require each runtime
partial group to contain exactly one matching full assistant row before its
stop, and require a runtime assistant row to agree with its scope's terminal
text. Each SDK reader also clears that terminal text binding at the end of its
turn, so a later turn must establish its own assistant text. Replacement output
windows discard the preceding partial owner so text from an earlier window
cannot be attributed to the next one. The three SDK fixture copies now carry
the same runtime full/result text; the shared partial
vectors put the full row before `message_stop`, matching producer order. This
strengthens interpretation for every relevant headless invocation, independent
of model, context length or benchmark. vLLM does not produce these local
runtime messages, so there is no backend change at this boundary.

The Python admission suite passed 348 source cases, including changed full
text, omitted full row, changed terminal text refusals, and a later turn with
its own result. Two direct TypeScript partial-owner source cases passed. The
full TypeScript wire suites still encounter the installed generated-v5 validator
before v10 admission;
the new wire path is unverified. Native and Java refusal tests were authored
and read but not executed. A fresh pinned archive applied the sealed patch and
matched all 1,141 final identities; the 13 edited authoring files matched that
archive byte for byte. No compiler, native/Java test, build, image, release,
deployment or push was run. Runtime operation identity itself, physical model
output replay in the native/Python/Java readers, and input-rendering provenance
remain open. Canonical resume reads its separate runtime chat JSONL; this
change only tightens stdout admission and does not rewrite or reorder that
history by source reading. An end-to-end resumed run remains unverified.

## Successful model result coherence

Source reading of `BaseJsonOutputAdapter.observeAdmittedMessage` and
`buildResultMessage` shows that an ordinary successful root result uses the
last accepted conversation generation's non-thought Part text. A runtime
assistant row replaces that display text; an error carries its own diagnostic,
and a structured result serializes its explicit value. The admitting readers
previously allowed a successful result to disagree with the accepted
generation. A Python full-schema source probe admitted both the fixture's
unrelated `done` result and an arbitrary replacement while the generation and
physical evidence stayed fixed.

Native, TypeScript, Python and Java admission now retain the last accepted
root conversation display text for the current turn and compare it with an
ordinary successful result. Runtime assistant text takes precedence, while
error and structured results keep their separate producer-defined sources.
The readers derive this display text from every non-thought Part, including a
Part that also contains a function call, matching the producer's
`generationParts` projection. The SDK fixtures now give seven ordinary
successful results the exact accepted-generation text instead of `done`.
This applies to every normal headless session, regardless of model or context
length. vLLM does not construct the client's terminal result, so the backend
has no corresponding edit at this boundary.

The Python admission suite passed 350 source cases after the change, including
a changed-result refusal and valid runtime and structured overrides. The wider
Python unit suite passed 746 cases. Its first run exposed synthetic positive
records that repeated a full body within one request segment or paired runtime
assistant text with an unrelated result; those fixtures now express the
already-required delta and text bindings across their SDK copies. TypeScript
formatting and lint passed for the changed files. The targeted TypeScript wire
test was run and stopped at the installed generated-v5 validator's refusal of
`stream_start`, before v10 admission. Native and Java cases were authored but
not executed. This establishes a relation between
two declared record views; it does not independently establish that the model
generation came from the retained physical response bytes. Error diagnostics
and structured-result serialization still need their own admission proof. A
fresh pinned archive applied the sealed patch and matched all 1,141 final
identities and the authoring source byte for byte. Native compilation, owner
gates and end-to-end resume remain unverified. The canonical runtime chat JSONL
and its replay order are unchanged by source reading.

## Terminal result source

Source reading found a producer inconsistency after ordinary successful
results acquired an accepted-model-text admission rule: the shared
`BaseJsonOutputAdapter.buildResultMessage` still let a caller's optional
`summary` replace that text. The adapter now refuses a supplied summary,
including a present property whose value is `undefined`, and directs a caller
to emit a runtime assistant message for local text or use `structuredResult`
for structured output. An ordinary success therefore uses the last accepted
assistant text; runtime and structured results retain their distinct sources.
vLLM does not construct this client-owned terminal record.

A later source read found that `runNonInteractive` still supplied
`summary: ending.message` on every terminal result. That property made the
adapter refuse ordinary results, even when `ending.message` was `undefined`.
The caller now supplies `errorMessage` for errors and leaves successful local
text in the runtime assistant message it already emits. Its JSON result test
checks the successful exit code and accepted model text. The targeted source
test was attempted but stopped before collection because the installed
`@qwen-code/web-templates` package has no `dist/index.js`; execution of this
caller change, compilation, and owner gates remain unverified.

The source fixture for physical JSON output was also brought into line with
the existing request and response evidence protocol: it supplies the selected
nonstream decode policy, decodes its recorded OpenAI completion through the
shared converter, and publishes the observed output and source request ID
through the normalizer. Its pending request closes as cancelled, and synthetic
failure/cancellation cases no longer claim completed processing. These fixture
changes support the affected source tests; they do not establish deployed
behavior. The focused adapter and refusal suites passed 149 source tests.
Prettier passed on six changed TypeScript files, and ESLint reported zero
errors (eight duplicate-import warnings). The broader JSON/stream suites were
run but did not qualify: 115 of 136 tests failed after the installed
generated-v5 validator refused `system/stream_start` before current admission.
A fresh pinned archive accepted the sealed landmark transform and matched all
1,143 final identities, including 44 intentional deletions; the six edited
client files matched the authoring source byte for byte. Native, Java,
compilation, owner gates, and end-to-end resume are unverified.
