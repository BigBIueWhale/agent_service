# Complete generation authority

This is the implementation design for the remaining logical-output boundary.
It is not a statement that the boundary is implemented or qualified. The standing
record-completeness goal remains open until the producer, every reader, resume,
input accounting and billing satisfy their full obligations.

## Records and owners

Stream identity is independent of runtime readiness. A `system/stream_start`
record owns the stream contract identity, session identity and request journal
origin before any data record. The adapter emits it once per output window,
including before dual-output `session_start` and startup error results. It
contains no runtime capability claims. A `system/init` record is the complete
runtime snapshot produced by the initialized CLI; it cannot double as an empty
journal header. There is one shape for each fact.

This separation is required by actual startup ordering: stream-json auth can
fail before `Config.initialize`, and SDK MCP registration must precede tool
discovery. Eagerly constructing a full snapshot there would fabricate empty
tools or agents. Generic readers need the universal journal header to interpret
evidence. The service certifier additionally binds a complete `init` to its
pinned runtime manifest before any model request and before successful terminal
output. An error with no initialized runtime and no model request can still
retain complete startup-failure evidence. This does not relax the manifest
binding for model work. These rules are present in the authoritative client
patch and native source. Owner gates remain pending; native and Java behavior
have not been executed.

A runtime snapshot never resets request replay. Opening a replacement output
window requires the previous window's terminal and closure of all physical,
logical and partial evidence. The generic readers check this before replay can
discard its prior population. The native reader owns exactly one invocation and
refuses a second header. Java binds its session identity from the admitted header
before delivering callbacks, independently of runtime metadata.

Dual output retains its channel handshake after the universal header. It does
not acquire a new runtime-discovery dependency: its physical request bodies
already record the actual tools and configuration dispatched. Pinned runtime
metadata belongs to deployment certification; the native invocation reader
already excludes dual channel lifecycle markers. No vLLM change is relevant to
these client lifecycle facts, and no TUI wait for progressive MCP was added.

Channel end carries physical request and usage summaries without a model success
or turn-count claim. Generic readers accept it only after the channel handshake,
with matching session identity and complete physical, logical and partial
evidence. A root result cannot substitute for a channel end, and a channel end
cannot substitute for a headless query result. The adapter captures its window's
session identity: journal replacement closes that window under its original
identity and publishes a new header and handshake before retained replacement
evidence. The bridge reads the adapter's current identity at shutdown. Recording
failure notifications use the active channel envelope while retaining the
affected recorder's session ID in their data.

The Python reader's seven focused source suites passed 575 cases after the
header and fixture migration, using the packaged v7 schema and actual admission
code. This includes early errors, complete runtime metadata, replacement-window
refusals, legitimate reopening, query cleanup, physical usage and generation
reconciliation, plus dual channel closure and unfinished root/child groups.
The log is `/tmp/codex-stream-start-python-tests-current.log`. Two CLI source
suites passed 17 cases in `/tmp/codex-window-lifecycle-source-tests.log`:
the actual producer boundary and request replay account for retained startup
evidence and same-/different-session journal replacement; diagnostic tests
preserve affected-recorder attribution. The producer test deliberately captures
before generated wire admission and does not qualify that gate. The harness
request/response suite passed eight cases. These
results do not establish generated TypeScript admission, native or Java
execution, the client splice, live startup behavior or whole-goal completion.
The shared partial vectors now describe explicit root and child message groups
under vector version 2; Python executed them. Their native and TypeScript
consumers are updated source and remain unqualified by execution.

The Python SDK integration peer now emits synthetic physical requests and exact
response bytes, decoded generation evidence, history decisions and completions.
Its permission exchanges include the actual authored tool result in the next
request delta, and its text partials have complete group boundaries. The actual
SDK subprocess transport, packaged schema, admission and assistant projection
remain in use. Twelve integration tests passed after the migration and again
after source review corrected missing cached/reasoning details in the authored
SSE usage. The latter run is the same corpus, not additional coverage; its log is
`/tmp/codex-sdk-python-integration-final.log`. The review and frozen identities
are in `/tmp/codex-python-integration-review/revision-2/`.

The initial green run did not catch the raw-usage mismatch: Python admission
authenticates response bytes but does not execute the provider decoder. The
corrected fixture explicitly reports the two zero counts instead of inferring
them. These integration tests use an authored local peer, not a real CLI or
provider. The resume/continue case checks session identity options only and has
been named accordingly; it does not restore canonical history. The sync bootstrap
cleanup case intentionally substitutes the query factory and makes no wire
admission claim. No compilation, generated validator or provider execution is
established by these results.

GeminiChat owns one immutable generation envelope for each admitted ChatAttempt.
It observes decoded responses before removing calls, terminals or incomplete-call
metadata for live delivery. The envelope retains those observations, including
ordered Parts, raw thought text, nontext Parts, citation metadata, incomplete
arguments and nullable observed usage. Presentation output is not an oracle for
this envelope. Exact HTTP evidence remains the physical response authority; the
decoded envelope does not claim that an SDK has faithfully interpreted every
provider byte.

Observations precede tool-call normalization. Each primary candidate retains its
original Parts, including repeated calls, and a positional mapping names the
normalized call ID or records suppression of an already observed provider ID.
The reader checks complete ordered coverage, duplicate suppression, ID uniqueness
and preparation identity/name consistency. History derives normalized calls from
that mapping and restores the original provider ID used by execution telemetry.
Generation usage and finish summaries are derived from observations; they are not
independent producer assertions. Accepted completion also compares served counts
with the accepted physical response outcome.

The journal publishes `model_generation` with a generation identity, exact UTF-8
JSON envelope, byte length and SHA-256. The serialized envelope is frozen once.
Readers parse its retained bytes rather than trying to reproduce another
language's JSON serialization. Each artifact stores one full copy per generation.
A child recording may contain the identical envelope because it is an independent
resume artifact; its identity, hash and bytes must agree with the shared journal.

An accepted canonical assistant line contains the immutable generation envelope
and its exact history position. This single durable line owns both output and
history acceptance without a second payload or a cross-record lookup. Its
history projection uses the same pure normalization that live Chat uses.
Abandoned output and inactive branches use the disjoint `model_generation`
evidence type. Forks preserve their envelopes and identities while converting
inactive accepted records to evidence, outside the active history chain.

Canonical generation evidence declares the originating Chat's historical
`historyDisposition` and its placement: `local` for locally abandoned attempts,
`child` for parent-journal copies, or `inactive` for retained branch evidence.
Inactive evidence may have either historical disposition. Before flattening a
fork, unselected local evidence becomes inactive, so later forks cannot revive it
by assigning an active parent. These fields belong to canonical placement, not
the immutable output envelope. Completion must agree with historical disposition.

Stored records contain no duplicated message, model, origin or usage projection.
Byte readers validate stored records and return an explicit resolved view where
existing history consumers need Content. Fork selects stored records by active
identity and never serializes the resolved view. This distinction also applies
to indexed reads and object-based transcript APIs.

Runtime history selection excludes generation evidence. Presentation selection
adds locally abandoned generations anchored to that active branch, retaining
their output and observed usage without making calls executable. Child copies and
inactive evidence remain available in the original artifact. Disjoint evidence
does not block side-artifact selection.

`model_attempt_completion` binds the journal, generation identity and hash,
attempt and scope, exact ordered physical request identities, and final accepted
or abandoned disposition. Physical response outcomes retain served-usage
ownership. Completion must not manufacture a second billed usage source.

## Ordering

The required sequence is durable generation and accepted history when applicable,
physical response settlement, durable logical completion, then executable terminal
publication. The accepted canonical line commits generation and history together.
Cleanup owns the same memoized settlement promise;
it cannot create a second generation or completion. Acquisition failure after
admission records an empty observed generation. No admitted requests means there
is no physical generation to invent.

The main journal persistence callbacks await only their own canonical append.
They must not call the aggregate ChatRecordingService flush, which itself awaits
the journal. Likewise, an output drain must not wait for a completion that its
current publishing operation has not yet enqueued.

## Output and execution

Model-origin full assistant wire rows are replaced by the complete generation
record. Runtime presentation and live partial events retain their separate roles.
SDKs derive familiar assistant views from admitted generations and completion;
the complete typed generation remains available. Derived views do not re-enter
wire admission. An abandoned observed call must never appear as an executable
`tool_use` merely decorated with a disposition label.

GenerationContext owns an immutable output scope independently of its KV scope:
`conversation` names either the root or an actual spawning tool call, while
`internal` describes work outside the root journal's displayed conversation.
The generation envelope retains this declaration in its hashed bytes. A child
launched inside an internal invocation inherits internal visibility, including
after background resume. Internal generations still own local accepted history,
physical requests and usage. Their raw evidence remains in the root journal;
they do not become SDK conversation assistants or replace its final display text.

An output window obtains generation and completion routing from its existing
generation replay. It retains that routing beside queued records, without adding
a second scope field to the completion payload. Internal evidence has a null
outer tool parent and does not enter the root conversation's partial-message
state. Its physical membership and logical completion are still mandatory.
Conversation ancestry must be checked against accepted tool calls; an arbitrary
KV invocation ID is never proof of a spawning tool. The TypeScript/Python
candidate currently checks declared parent and KV consistency. Their issued-call
ancestry registry and native integration remain unfinished.

Main and child tools consume the generation's normalized arguments only after
accepted completion, including when no stdout adapter exists. Partial text,
thought and call observations must reconcile with the generation. Citation
formatting and visual summaries remain explicit presentation, not additional
model-authored text. Both JSON adapters derive terminal display from admitted
generation data; removing assistant wire rows must not empty their result text.

SDK query iterators retain the raw generation and completion records. After an
accepted completion passes all admission checks, the reader returns its owned
generation receipt and the SDK appends one derived assistant view immediately
after that completion. The view references the generation identity, hash and
completion record identity. Its text, thought and executable calls come from
the accepted history projection; its model and five usage counts come from the
generation envelope. It does not introduce another billed response. The familiar
`stop_reason` is `tool_use` when the view contains an executable call and null
otherwise; the exact provider finish reason remains in the referenced envelope.

An abandoned completion produces no SDK assistant view. Its complete observed
output remains in the typed generation evidence, and no abandoned call acquires
a `tool_use` shape through this convenience API. Runtime assistant messages have
text content, no model, null usage and no generation projection reference.

Admission produces the receipt only after scope and partial-authority checks
have succeeded. The SDK constructs its projection before exposing the mutable
raw completion to consumers, without retaining a second admission cache. Raw
record order is preserved. Completion may precede the last tool partials, so a
derived assistant can precede their message-stop event; it is a final history
projection and is never fed back through wire admission.

## Membership and late windows

The completion's ordered membership must equal all observed physical requests
for that attempt. Every listed request must have the same journal, attempt and
scope, and every observed member must be listed exactly once. Missing completion
at a closed stream refuses certification. This also exposes a window opened
between physical retries: observing the second request does not establish that
the first request was recorded.

There is no additional `first_request_id` field and no journal-wide admission gate
on unrelated active work. A late window needs the complete selected attempt or a
visible refusal before it can authorize its tools. Refusal by an optional renderer
must detach that renderer without poisoning unrelated canonical owners. A
mandatory persistence failure remains a session failure. The implementation must
preserve this distinction explicitly.

EOF on a live canonical file is a prefix boundary, not lifecycle closure. Generic
metadata and presentation readers validate the prefix they consume. Closed load,
indexed runtime restore, background recovery and fork require completed physical
and logical evidence before making history runnable. The index retains compact
completion counts from its existing scan. A closure refusal never retries through
a prefix reader. Child artifacts bind their local generations to completions;
their physical request/response evidence belongs to the root journal, and the
child file alone cannot establish that journal's byte completeness.

`readSessionView` returns an explicit prefix view without the runtime resume
marker. History previews, exports, session references, live task views, Goal
evidence snapshots and ACP display reloads use that owner. `loadSession` admits
closed evidence before providing `lastCompletedUuid`. Indexed display selections
read their presentation UUIDs independently of the runtime-history read set;
selection order remains the display order. Exports pass presentation records to
both replay and normalization without changing the runtime conversation.

## Evidence and qualification

At 1055f47 an independently executed actual-SDK fixture opened a replacement
window between an HTTP 500 acquisition and its successful inner retry. Both
requests had one ChatAttempt; the replacement contained only the second request.
ModelRequestStreamReplay accepted its origin, summary and stream end. No normal
CLI path was found that automatically reopens a window at this boundary. The
reproduction establishes an API-level membership omission, not a deployed-session
failure or a new billed-token loss.

Source inspection establishes that current formatters discard or transform some
thought, citation and nontext observations, and that forks retain physical
evidence while dropping inactive generation records. These observations motivate
the shared envelope and typed history-reference design. They are not candidate
test results.

Qualification requires real producer and canonical-writer fixtures, both ordinary
and indexed restore, fork preservation, held writes at each durability boundary,
accepted/refused/failed/empty attempts, root/child parity, actual local tool
dispatch, partial reconciliation and omission/duplication/scope mutations. The
owner prohibits builds, generators, typechecking, native/Java execution, release
and push. Those gates remain unverified until the owner runs them; source tests
cannot establish their success.

Focused source fixtures recorded under
`/tmp/codex-attempt-accounting-audit/call-mapping-candidate-5/REPORT.md` executed
mapping mutations, the real Chat processor, in-memory SDK decoding, real root and
child filesystem writers, and ordinary/indexed/fork boundaries. The frozen
candidate-4 snapshot passed 25 cases; candidate 5 reran nine writer/reader cases
after four placement changes. Those snapshots precede subsequent presentation
selection and partial-protocol edits. They do not qualify the current moving
draft, the outer Chat terminal gate, tool dispatch, generated validators, native
certification or the full record-completeness goal.

The partial protocol uses explicit start/stop markers for each formatter message
group in both root and child scopes. A generation may have multiple such groups.
Per-attempt reconciliation spans the groups and retains authority after completion,
because terminal delivery can follow it. Raw thought bytes belong to model partial
events; formatted citations remain presentation with original metadata in the
generation envelope. The wire, reader, accounting and reconciliation migrations
are unfinished implementation, not work awaiting only a build.

Partial message headers identify a formatter group and make no model claim. The
configured session model can differ from the root override or child model; the
generation envelope owns the selected model. Runtime assistant rows likewise have
no model field, contain only text, and have null usage. Model finalization does
not construct an additional full assistant projection. Source inspection found
runtime assistant notices only on paths that finish before the model response
loop, so returning to an earlier attempt after switching origin remains a refusal.
This is a source conclusion, not an executed full-stream result.

The SDK receipt and projection candidate has a separate focused report at
`/tmp/codex-attempt-accounting-audit/sdk-generation-view-1/REPORT.md`. Its 19
TypeScript cases execute the real request/completion replay and projector below
wire admission; they do not execute the generated v7 validator or Query end to
end. Its 26 Python cases execute the packaged v7 schema, admission and Query,
including the routing loop with an in-memory transport. No subprocess, provider,
native code or Java code was executed. These results apply to the exact source
hashes in that report, not the unfinished native/Java migration.

The 70-case writer, reader, fork and export corpus is now collocated in the
authoring tree. It also passed through the ordinary package configurations;
`/tmp/codex-attempt-accounting-audit/collocated-adoption/ordinary-config/REPORT.md`
records that relocation check. The reruns are the same cases, not additional
independent coverage.

Native implementation is now in the authoritative `protocol/engine` working
tree. `/tmp/codex-native-generation` retains the draft used to start integration;
the repository is the current owner. Generation/completion admission connects
accepted tool issuance, physical usage and independent observations. Service
consumers and parser fixtures have adopted the public accounting shape in source.
The client/contract/manifest splice contains this migration. Remaining reader
integration and the complete change review are unfinished. None of this native
or service Rust has been built or executed.

Native accounting must preserve three different populations: reported started
turns, observed Chat attempts, and physical provider requests. The CLI increments
`turnCount` before its outer send; retries within that send are separate attempts
or physical requests. Session usage summarizes all physical dispatches, including
internal fork scopes with generated identities. A KV scope is therefore not
proof of a displayed subagent, and an unknown scope cannot be silently charged to
the main session. The native and service sources now keep these populations separate across
results and recovery. Assistant-row
counts cannot establish the new physical totals.

The terminal usage owner is the physical recording window. Every admitted
request, including utility work and retries, enters its request population.
Each validated processing outcome contributes once, whether completed, failed
or cancelled. A later history acceptance or abandonment never rebills it.
Unfinalized usage counts requests without a processing outcome, independently
of a chat response still awaiting its history decision. Reported zero counts
remain a report; absent usage remains null and increments the unreported count.

The TypeScript candidate obtains root `usage` and `request_evidence` from the
same output window. Root result callers provide duration, reported started turns
and display text; session telemetry is not their usage authority. Repeated root
results retain cumulative totals for that window. Closing it or selecting a new
journal starts a new population. A zero-request root result has an explicit
zero-request summary with null served usage. Child error results can have null
usage and do not inherit the whole journal's total. Optional JSON `stats` retains
its session-telemetry meaning separately.

TypeScript and Python root admission compare all four summary counters and the
five served counts against physical replay. The schema permits additional
metadata inside terminal served usage; it does not change those comparisons.
The physical outcome itself has exactly five served fields. Aggregation checks
the contract's integer range before installing the outcome, and copies the
numeric data it retains. These statements describe the current source design;
native and Java integration of this owner is still unfinished.

The internal-scope candidate has bounded executed evidence in
`/tmp/codex-internal-generation-scope-audit/REPORT.md`: real fork/SDK/Chat/writer
fixtures distinguish internal from displayed Agent output, preserve main text,
and restore accepted internal history through closed canonical admission.
The fixture invokes the real Chat processor below the outer send scheduler;
it does not execute full AgentHeadless, restart/resume or native certification.
Background metadata writes were exercised with fake Agent execution. Rebuilding
the output scope during an actual background restart remains unverified.

Terminal usage was independently reproduced on frozen producer and reader
sources. A real producer fixture admitted five physical requests but published
caller-supplied usage for one; a later cumulative result and a reopened window
also accepted the wrong populations. Frozen Python admission and Query delivered
null root usage after a known physical report. These baseline observations are
retained under `/tmp/codex-terminal-usage-audit/baseline` and
`/tmp/codex-attempt-accounting-audit/terminal-usage-baseline-1`. The final Python
candidate passed 76 physical-usage cases, 91 existing generation/view cases with
explicit fixture totals, and three freshly captured producer traces, as recorded
in `/tmp/codex-attempt-accounting-audit/terminal-usage-candidate-2/REPORT.md`.
The same raw producer traces differ from their Python fixture only in the init
contract identity; their owner-derived terminal totals were not edited.

Nine actual producer/replay cases passed for terminal usage, then passed again
through the ordinary CLI configuration after relocation to permanent test files.
That relocation rerun is the same corpus, not additional coverage. The fixtures
exercise the actual SDK, pipeline, journal and adapter result builders before
generated wire admission. Eight selected session cases passed. The auth test
file collected no tests because its generated web-template package was absent;
it was not built or replaced. Its migration remains unverified. Broad adapter
tests still require migration from caller-supplied usage and earlier generation
APIs. None of these source checks establishes a successful build, full native
certification or the goal's completion.

Native integration must expose one certified all-window usage summary and a
per-KV usage table. The validated CLI initialization owns `main_kv_scope`; the
service's job/session identifier is a different identity. `AgentScope` retains
issued-tool identity and nullable terminal claims. Internal and utility KV
scopes belong to the physical table without acquiring conversation ancestry.
Reported `num_turns` remains the terminal's started-turn assertion. It must not
be inferred from request, attempt or usage-report counts, or increased by a
`max()` merge during recovery.

The native draft keeps a compact request identity ledger before the
certification-refusal latch: the invocation's journal and whether each observed
request has a usable outcome. Schema-admitted request records establish request
ownership; the independent summary describes the whole window, not a main or
child scope inferred from KV identity. Certification stops at the first nonconformity, while later
independently readable physical outcomes must remain observable. An outcome without a recorded request owner, duplicate usable outcomes,
invalid counts and framing gaps increment unaccounted evidence. An unusable
outcome leaves that request unfinished; a later independently usable outcome
can contribute once without restoring certification. The observer must not recover certification or invent
root ownership. A missing terminal leaves reported turns unknown.

The native/public source migration spans `protocol/engine/src/runtime.rs`,
`src/result_parse.rs`, `src/session.rs`, `src/runtime.rs`, `src/progress.rs` and
`scripts/session-body.jq`. Persisted result records select schema 6 and progress
documents select version 2. Public shell-reader tests, composition assertions
and session-resource documentation use the physical accounting shape. Source
reading and focused client tests do not qualify native execution; the owner's
build and native gates remain unrun.

Source comparison also found a numeric interpretation question still to resolve
before claiming arbitrary-envelope reader parity. The native integer owner
interprets decimal lexemes exactly, whereas TypeScript/Python can round a
floating-point lexeme such as `1.00000000000000001` to 1 in decoded observation
usage. Ordinary producer JSON serialization cannot emit that spelling from the
decoded value. This does not establish parity for an independently authored
envelope; no native execution has been used to qualify it.

The current native draft removes assistant-row billing from `RuntimeContract`.
Its accepted conversation completions register normalized calls; abandoned and
internal generations do not grant displayed tool authority. Root admission
requires every logical attempt and physical response to close and compares
terminal usage against the physical request population. The per-KV and global
summaries are planned together before installation, with global totals cached
rather than recomputed over every scope for each outcome. These are source
inspection claims about the draft, not executed native refusals.

Native source tests now describe physical zero/unknown usage, bounded sums,
observations after refusal, repeated outcomes, accepted tool ownership,
incomplete calls and missing logical completion. An ordinary-tool fixture was
copied byte-for-byte from the client fixture; the native test explicitly adds
its required init manifest and terminal display text. That adaptation does not
qualify the unresolved minimal-init path. The native schema tests select their
input through the build script's contract path binding, and inventory assertions
were updated from a source-only JSON schema walk. No schema compiler or generator
was executed. Direct rustfmt ran; no native test, build or typecheck ran.

The native public DTO now describes `main_kv_scope`, total physical `usage` and
`request_scopes`, and observations have nullable reported turns plus physical
usage. The service and its persisted/public readers now adopt these DTOs in the working
tree, together with the client, contract and manifest splice. This migration
remains uncommitted and needs the full integration review before it constitutes
a completed change.

Service result construction preserves the reported terminal `num_turns` as an
optional observation and an explicit certified claim. Lifecycle progress uses
`physical_requests`; it never merges requests or assistant rows into reported
turns. Terminal snapshot usage stays distinct from monotonic lifecycle counters.
A certified result must match its observed summary and per-KV table, contain no
unfinished requests, and account for at least the durable request count. Service
persistence validates complete nullable observation groups and required child
terminal fields. Invalid persisted usage maps to an internal service invariant
failure; raw capture refusal retains the agent-output error classification.
These behavior statements are established by reading the current source only.

`PYTHONDONTWRITEBYTECODE=1 python3 -P scripts/test-session-readers.py` ran after
migrating the jq reader and its fixtures: eight test methods passed. They cover
physical table totals, internal request ownership, reported turns independent of
requests, unknown/zero/pending usage, invalid counts, required nullable fields,
partial observations and lifecycle/bundle distinctions. They do not execute the
Rust service, native certifier, composition harness or an actual resume. Direct
rustfmt and `git diff --check` also ran; neither is compilation evidence. The
composition harness assertions were edited but not run.

The parser test source is now under `src/result_parse/tests.rs` and its framing,
evidence and compaction modules. Fixtures explicitly declare physical requests,
outcomes and terminal totals. Tests needing tool ownership rebind the captured
generation fixture's identities and hashes while preserving its response bytes
and raw argument lexemes. The helper does not derive requests from assistant
messages or repair mutated terminal summaries. Negative cases retain positive
controls, and raw numeric/framing mutations enter the byte reader unchanged.
Compaction field fixtures include synthetic physical draws; they do not qualify
the production compactor's ownership or exact resume composition.

The source corpus retains framing faults, independent observations after refusal,
scope ancestry, child terminal claims, closed terminal vocabulary, per-KV usage,
unknown/zero/pending distinctions, malformed summaries and exact numeric bounds.
Reported turns are tested independently of physical populations. An open request,
a fabricated request population or a well-formed but contradictory terminal
usage total is a refusal. These are test-source assertions, not executed native
results. Direct rustfmt and `git diff --check` ran; Rust tests remain unrun.

Reading the migrated boundary also exposed inconsistent reported-turn limits:
the observer and service use `SAFE_INTEGER`, while native terminal admission used
the wider `u64` domain. Native terminal admission now uses the shared exact-integer
limit for root and child claims. Source cases cover the limit and its successor.
This change is established by reading; its execution remains unverified pending
the owner's gates. The schema's general terminal integer declarations and other
readers still need the cross-reader numeric review described above.

The Java reader candidate now connects generation ownership, ordered physical
membership, exact terminal usage and generation-authorized partial output to
record admission. `StreamRecord` retains an immutable decoded envelope for both
generation and completion records. Accepted conversation completions derive a
typed assistant view immediately after their raw completion callback; abandoned
and internal completions have no assistant callback. The view's provenance points
to its generation and completion, while raw evidence remains available. These
statements describe source paths, not executed Java behavior.
This change applies to every Java SDK consumer of the shared stream. vLLM does
not read that client protocol, so this reader change has no backend implementation
counterpart; it does not replace or alter the backend-owned response evidence.

Strict JSON document parsing retains member offsets to compare exact tool argument
lexemes, including number and escape spellings. Partial text preceding a generation
uses a bounded-memory digest of the prefix, then compares subsequent fragments
directly with the immutable generation text. Java source tests cover offsets,
generation/completion omissions and ownership, exact argument fragments, physical
usage, root/child/internal disposition and callback ordering. No Java compilation
or execution was performed; parser offsets and callback behavior need the owner's
gates. The ten authored Java request/response fixtures were migrated together with
their generation and terminal populations, and the current Python reader actually
admitted and closed all ten. This is evidence about fixture validity under that
reader only. Three existing generation fixture resources were copied unchanged
from the Python SDK for Java gate coverage.

The read-only Java review is retained at
`/tmp/codex-java-generation-review/REVIEW.md` with eleven frozen production
identities. It found a stale Session test that placed a tool call in a runtime
assistant row. That test now puts its deep/null/large-integer argument into an
accepted generation and checks the derived tool plus original generation evidence
and completion receipt. The reviewer inspected the revision and reported no
outstanding finding within the reviewed Java paths. This is a bounded source
review, not evidence that Java compiles or that its tests pass.

The adapter test candidate uses real journal, response, generation and completion
owners for usage and model output. Runtime text cannot acquire model billing from
`Finished`. JSON and streaming cases retain raw thoughts, accepted and abandoned
calls, unknown and zero usage, cumulative requests across ordinary turns, and
output sink failures. Streaming assertions retain exact fragments and independent
group boundaries. The retry fixture publishes both complete generations and their
logical completions, with separate physical usage and only the accepted answer in
the root result. These fixtures author observations; they do not execute the SDK
provider decoder or establish canonical filesystem durability.

The Base adapter's raw thought helper now takes the raw fragment directly at both
production call sites. Source review found and resolved two test defects: a stale
usage-validator signature and a subagent table that checked evidence without
checking formatter output. That table now checks exact finalized groups, including
the billed-empty group. The existing formatter suite passed 12 tests after the
production change. The broad Base suite collected zero tests because the generated
`@qwen-code/web-templates` package was unavailable. The installed wire validator
still targets v5; it was neither regenerated nor replaced. No serialized v7 suite
pass is claimed. The separate JSON/streaming source review at
`/tmp/codex-json-stream-v7-review/REVIEW.md` found no remaining defect within its
five frozen paths. It explicitly distinguishes intermediate results with pending
responses from complete EOF streams; their later cleanup does not make an
earlier serialized snapshot complete. This review is not execution evidence.

The SDK timeout option question in intervention §2 is confirmed and repaired in
the authoring candidate. The timeout schema refuses every unknown key, and query
validation reports the key and tells the caller to remove it before retrying.
One shared validator serves both the factory and the exported `Query` constructor;
the factory checks before CLI preparation, and direct construction checks before
using the supplied transport or starting its lifecycle.
The three implemented timeout controls retain their values. The developer guide
advertises only those controls and explains input closure after initialization
and submitted-turn results. This serves all TypeScript SDK query callers; vLLM
does not consume client SDK options and has no corresponding backend change.

Executed source tests reproduced silent acceptance of `streamClose` and a
misspelled `controlRequest` in both schema and public query validation: four
failures and 80 passes before the fix. After the fix, the same two suites passed
all 84 cases, including refusal before CLI preparation or construction. The query
tests substitute process transport and query construction; they establish the
factory option-validation boundary, not actual CLI startup or complete stream behavior.
The logs are `/tmp/codex-timeout-option-before.log` and
`/tmp/codex-timeout-option-after.log`.

Source review then identified the exported constructor's separate entry point.
Two focused direct-constructor cases reproduced its silent acceptance. After
connecting the shared validator, all three suites passed 86 tests. These two new
cases import the actual `Query` and admission code, supply in-memory transport
spies, and assert refusal before transport access or abort-listener registration.
They admit no wire records and do not qualify stream gates. Logs:
`/tmp/codex-timeout-constructor-before.log` and
`/tmp/codex-timeout-shared-validation-after.log`. No compilation or image step ran.
The read-only follow-up in `/tmp/codex-timeout-option-review/REVIEW.md` inspected
the shared validator, both public entry points and the exact execution logs. It
found no further defect in that bounded change; it did not rerun the tests.

## Presentation consumers and attempt fixtures

The session preview, command export, server export, realtime startup and live
task fixtures use the presentation reader's complete view shape. Export fixtures
distinguish active conversation from abandoned presentation records and assert
that the export collector receives the presentation records. Four source suites
passed 27 cases; after correcting the command suite's filesystem mock, its 40
cases passed separately. Logs are `/tmp/codex-presentation-consumers-after.log`
and `/tmp/codex-export-command-after.log`. This is 67 cases in two execution
stages, not one combined run. The bounded source review is in
`/tmp/codex-presentation-consumer-review/revision-1/REPORT.md`. These fixtures do
not establish canonical admission; export formatters remain mocked.

Core attempt fixtures use journal-owned generation publication and completion.
The canonical recording test retains every actual SDK observation, commits one
accepted generation and two abandoned generations, and accounts for the retry's
two physical requests. It checks exact history through ordinary and indexed
restore and through a fork. A missing physical history decision now refuses all
three reader paths; its obsolete resume-success exception is removed. The
closed-writer oracle uses a fresh attempt and requires the writer-unavailable
error before a fetch, so prior attempt settlement cannot mask that guard.

The recording rerun exposed intermittent empty response bodies, once for the
synthetic HTTP 500 and once during successful JSON parsing. Later green repeats
alone did not resolve it. Reading the pinned Node 22.14.0 implementation identified
that `Response.clone()` registers the original stream branch against the clone's
inner state. The fixture consumed and discarded that clone to measure expected
bytes before the original acquired a reader. A forced-GC interpreter experiment
reproduced cancellation of that original with the reason `Response object has
been garbage collected`: its 64 bytes became zero. Retaining the clone and
constructing a response directly from bytes each preserved all 64 bytes. The
experiment log is `/tmp/codex-response-clone-gc-evidence.log`. This proves the
runtime mechanism; the earlier failed runs did not record the GC callback itself.
The experiment source is retained at `/tmp/codex-response-clone-gc-evidence.mjs`.

The fixture now records its expected bytes before constructing the response,
without consuming a clone. Its actual SDK and exact-byte oracle are retained.
After that correction, 77 source tests passed: 22 request-evidence cases, 53
response-evidence cases and two canonical SDK recording cases. Their log is
`/tmp/codex-attempt-recording-final.log`. The earlier two ChatAttempt settlement
cases passed separately. Five migrated wire-admission cases remain unexecuted:
the installed generated validator targets v5, while the candidate is v7. Their
headers and causal error assertions were corrected by reading the source; no
validator was replaced or bypassed to qualify them.
The bounded read-only review is recorded in
`/tmp/codex-attempt-fixture-review/revision-2/REPORT.md`. A fresh application to
the pinned archive then matched all 1,127 declared paths and the authoring source
at review patch `55985510e1f577da812b92433c6257d09e3735397cf4095c9135a52b9a29efed`.
This is source-transform evidence, not compiler or application execution.

## CLI cancellation fixture and remaining integration

The CLI fixture review separates three boundaries: runtime controls with no
provider request; authored model observations through the journal and attempt
owners; and scheduling tests supplied with already-projected tool requests.
The scheduler's synthetic multiple-call batches cannot be represented as one
accepted provider generation: the shared Chat admission permits one executable
call per accepted generation. Its concurrency, ordering and sibling-policy
assertions still need a clearly stated scheduling boundary. Adding a default
finish reason or usage report, or dividing one invalid draw into several
accepted draws, would change those tests' meaning.

The actual-Turn cancellation fixture now records the same decoded Parts that
Turn receives. One memoized settlement publishes the generation, physical
history decision and logical completion. An accepted tool terminal awaits that
settlement before cancellation and delivery. Cancellation during a nonterminal
thought/text chunk settles abandonment in iterator cleanup, preserving both
delivered fields without another provider pull. The physical body is an authored
JSON response consumed by the real recorder; this fixture does not execute SDK
decoding or canonical filesystem persistence.

Two new direct Turn/journal tests first failed because the old fixture produced
no generation. After the correction, both passed with exact event values,
recorded Parts, usage populations, completion disposition and closed response
and attempt sets. The accepted case checks closure when the tool request is
delivered; both check iterator cleanup and zero subsequent pulls. The final log
is `/tmp/codex-cli-cancellation-source-final.log` (two passed, 180 excluded).
The source loader reads the export HTML source and refuses any use of the
unrelated insight renderer. No HTML asset or application qualification follows
from that accommodation.

The original two serialized CLI cases retain exit 130, the cancelled terminal,
zero tool executions and zero extra pulls. Their model-output assertions now
read generation/completion evidence. Both executions stopped at the unchanged
generated v5 validator's refusal of `stream_start`, before those assertions
could qualify. See `/tmp/codex-cli-cancellation-wire-after.log`. The bounded
source review is `/tmp/codex-cli-generation-fixture-review/revision-1/REPORT.md`;
its reviewed test identity is
`14364051a71cc14009d29668ebe9f586aa8af46a56d3dcbf39edec156eb64380`.
The reviewer inspected the supplied logs and did not rerun tests.
The cancellation checkpoint was applied to another copy of the pinned archive.
Its follow-up hash script initially attempted to read a deliberately deleted
file; a corrected comparison of both file hashes and declared absence matched
all 1,127 paths, including 40 deletions, against the authoring tree. The source
identity report is `/tmp/codex-cli-cancellation-final-state.json`, at review
patch `53ce9a4a8c94cab44c91250efa54cacacc127f0ec10bc3d31ceee039e5a30e25`.
This confirms source identities, not compilation or deployed behavior.

The general CLI event helper and the other output, usage and scheduling fixtures
remain migration work. Three Thought fixtures now declare their raw text, but
that does not qualify their surrounding unmigrated cases. Terminal usage must
come from recorded physical outcomes; the separate optional JSON stats still
use telemetry and retain their own presentation tests. The baseline inventory
is `/tmp/codex-cli-generation-fixture-review/baseline/REPORT.md`.

## Raw reasoning and UI display ownership

The core `ThoughtSummary` event carries the exact raw reasoning as well as its
parsed subject and description. The UI's bounded, merged display value is a
`ThoughtDisplay`, derived from only those two display fields. Incoming and
buffered events retain the complete core type; dual output receives the original
event before display reduction. Both immediate subject updates and accumulated
display state explicitly select their display fields. This prevents a reduced
UI value from claiming to be a complete raw event without accumulating another
unbounded reasoning copy in the title state. It applies to ordinary interactive
sessions as well as long sessions. It is a client display boundary and requires
no vLLM change.

The parser, Turn, client retry, loop detector and UI fixtures now supply or assert
their authored raw text, retaining the existing display, ordering, cancellation
and loop-detection assertions. Loop detector fixtures parse explicit source
strings instead of manufacturing raw text from a reduced summary. The Turn
provider-ID case also reads the normalization owner's `parts` result while
retaining its raw-provider and normalized-ID assertions.

Executed source evidence: the baseline parser suite had 13 failures and two
passes; the baseline Turn suite had eight failures and 58 passes. Seven Turn
failures were missing raw expectations; the other was the stale normalization
return shape. After migration, all 169 parser, Turn and loop-detector tests
passed in `/tmp/codex-thought-core-after.log`. The targeted client retry case
passed with 329 cases excluded in `/tmp/codex-thought-client-after.log`.

The selected UI run passed 33 cases with 190 excluded in
`/tmp/codex-thought-ui-final.log`. It covers exact event forwarding into a mocked
dual-output port, raw-free display state across immediate and buffered updates,
and the existing display bounds, timing, cancellation and retry cases selected
by their thought-related group names. The first run's new test omitted its
intended Finished control and failed the final display-reset assertion; adding
that explicit fixture control resolved it. This is not serialized admission
coverage. No generated binding, native binary, Java code or backend was run.
The read-only revision review is
`/tmp/codex-thought-fixture-review/revision-1/REPORT.md`; its eight frozen source
identities matched the authoring files, and it found no issue in this bounded
change. It inspected the supplied logs without repeating the tests.
A fresh pinned-source transaction then applied the sealed patch, checked the
semantic source contracts, and matched all 1,128 declared paths, including 40
deletions, to the authoring files. Its report is
`/tmp/codex-thought-final-state.json`, at review patch
`a60f8e687bb6c5b3febd2d10f867046dbf4dd5acc84118266dd8276c51872541`.
The manifest and transaction inputs remained unchanged throughout that check.

Reading the native reader's migrated tests also found one stale source
expectation: an orphan child was assumed to appear on line two even though
the fixture begins with separate stream identity and runtime metadata rows.
The assertion now derives the first-reference line from those authored rows
and keeps the same refusal cause. This correction is established by source
reading only; the Rust test remains unexecuted pending owner gates.

The persisted child-result validation also now rejects a present empty error
message for either terminal status. The wire schema's `error.message` has a
nonempty constraint, and the native terminal reader preserves that value using
its nonempty text reader. A success record may carry a nonempty diagnostic under
the current contract; this correction preserves it rather than narrowing the
contract. The authored native persistence test covers absent, empty and nonempty
messages for successful and error child terminals. The source mismatch and
correction were established by reading; execution remains unverified pending
owner gates. This belongs in the service's shared persisted reader, which has
no counterpart in direct vLLM use.

## CLI projection and scheduling fixture boundaries

The fourteen direct CLI cancellation cases now use the existing typed output
port and a lazy source of authored CLI events. They create no provider request.
They retain the main/drain distinction, every abort boundary, iterator closure,
exit 130 and zero tool executions. A pre-iteration copy of the expected events
checks exact raw reasoning, text, incomplete arguments, executable call,
finish and usage without sharing a mutable oracle with the producer. The
terminal port assertions check cancellation and finalization/result/flush call
ordering. These are CLI delivery assertions; they do not qualify serialized
assistant output, partial-group closure or physical usage accounting.

The seven scheduling cases use the same explicit boundary below model admission.
Their supplied tool batches retain the concurrency gates, out-of-order completion
with request-order finalization, unsafe sequencing, mixed partitions, configured
concurrency limit, alias lookup and plan-mode sibling rules. An output window
asserts that the fixtures manufacture no request or generation evidence. Their
success result is a CLI control-flow assertion, not admission of a multi-call
provider generation. The shared mock configuration now declares an empty
subagent registry so actual runtime metadata construction can perform discovery.

Executed source evidence is `/tmp/codex-cli-projection-final.log`: 23 passed and
159 excluded, comprising these 21 cases and the unchanged two direct Turn/journal
cancellation cases. The earlier baseline had 21 failures at generated-v5
admission; the first migrated run had 21 failures because the configuration
fixture omitted subagent discovery. Both logs remain preserved. No validator
was substituted or generated. The bounded read-only review at
`/tmp/codex-cli-projection-review/revision-1/REPORT.md` found no issue and checked
all fourteen frozen source identities against authoring. It inspected the logs
without rerunning tests. The reviewed test identity is
`aa5e34a5cc081aeab10b6818bf3dde28aa442adec4bdec4e05371100dec5913c`.
The general event helper and the remaining model-output/usage fixture migration
are still open; these passing unit cases do not qualify the full CLI suite.
The sealed client patch was applied to a fresh pinned archive, and all 1,128
declared paths, including 40 deletions, matched authoring. Semantic source
contracts passed and transaction inputs remained stable. The report is
`/tmp/codex-cli-projection-final-state.json`, at review patch
`87dacfeafa2ce265e076b63471905e405413ecd2561b465ad27e4487e2258cce`.
This verifies source application and identities, not compilation or behavior.

Comparison with the removed native reader tests found that two framing fixtures
had lost their child-scope observations after a malformed or oversized record.
Those fixtures now issue the child before the damaged record, retain physical
usage on both sides, and assert the later observed child scope. The unterminated
and uninitialized controls still require no observed child scope. This restores
the original observation obligation under physical accounting. The correction
is based on source reading; the Rust tests remain unexecuted pending owner gates.

## Native conditional schema ownership

The current v7 definition uses an `else` branch on the partial-event origin
conditional: runtime controls exclude an origin, while presentation events
require one. Source review found that the native schema compiler rejected
`else` as an unknown keyword and the native interpreter had no corresponding
branch. This is a defect in the uncommitted v7 integration, separate from the
withdrawn claim about historical version binding.

The compiler and interpreter now use one conditional rule with a required true
branch and an optional false branch. The compiler owns both branches, including
reference resolution and recursion checks. Ordinary failure of the condition
selects the false branch when present; resource exhaustion or an invalid
definition still exits validation rather than selecting either branch. The
owned vocabulary continues to require `if` and `then` together, so an unbound
`else` is refused. All existing `if`/`then` definitions use the same rule.

The authored native test matrix covers all three runtime controls and a
presentation event, each with root/child scope and absent/present origin.
Compiler refusal tests now exercise external, missing and cyclic references
inside `else`, as well as conditional definitions lacking their required
partners. An interpreted JSON traversal of the current schema confirmed the
inventory assertions: 675 object schemas, 97 false schemas, 46 references and
25 distinct keywords. It emitted no bindings or native code. The traversal
report is `/tmp/codex-generation-self-audit/schema-vocabulary-after.json`.

The mismatch and branch correction are established by source reading. The Rust
tests, native compilation and native admission remain unverified pending owner
gates. This fix belongs in the shared native schema owner used by every service
reader and certifier. Direct vLLM HTTP callers do not consume this stream
contract, so there is no corresponding backend change.

The authoritative landmark stage, semantic contracts, manifest and stack lock
contain the current header, channel lifecycle, presentation, core attempt and
reasoning fixture changes. The v7 source splice applies to the pinned archive
with exact declared path identities; that is source evidence, not a qualified
build. Physical-to-decoded output provenance, input/display binding and the
remaining completeness and resume questions are open. Generated admission and
Java/native behavior remain unverified pending owner gates. The standing loop
is not complete.
