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

The following control-flow gaps were confirmed by reading
`protocol/engine/src/model_requests.rs`; the transport correction below closes
one of them. No native adversarial test was run:

- `validate_output_origin` returns immediately for runtime origins. Runtime
  output is legitimate, but that label alone does not establish which runtime
  operation accounts for it. Inventing a model attempt for runtime output would
  be wrong; the runtime owner needs explicit accounting.
- Model output checks the attempt's identity and scope without binding that
  output to its final history disposition. Aggregate request/response counts
  in `validate_summary` do not provide that per-attempt association.
- The baseline response completion validated byte counts, digests and some
  termination conditions without linking processing success to transport.
  The shared writer and readers now require observed 2xx HTTP and an EOF or
  cancelled transport ending for completed processing. The audit's broader
  implication that transport completion was unchecked is false.
- There is no comparison tying displayed user/tool-result content to its
  admission and rendering in the next provider request. Such a comparison
  must account for reminders, media rendering and rejected inputs; simple
  string equality would reject valid ordinary sessions.

Charging served output from an abandoned draw is correct. Served work and
history acceptance are distinct facts. The defect is missing disposition
accounting, not the inclusion of served work in usage totals.

The stdout adapter's `emitSubagentRound` consumes origin, reasoning, text,
malformed calls and usage but drops the direct `historyDisposition` field.
The whole stream retains response-history decisions and matching origins;
independent journal/adapter tests confirmed the join in stream, partial-stream
and batch modes. Direct labeling and per-attempt output reconciliation remain
open. The canonical and ACP acceptance fixes are not invalidated by this gap.

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

These questions identify real source facts. Corrections are marked below;
the other items remain open:

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
