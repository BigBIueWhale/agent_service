# Physical output provenance

The response journal retains exact HTTP body bytes and the generation envelope
retains decoded observations used to construct model history. Those are distinct
authorities. TypeScript root readers now replay OpenAI response bytes through
the selected decoder and compare raw Chat observations before admission.
The native, Python and Java readers still bind declared ownership and usage
without independently establishing that byte-to-observation relation. Native
and Python source now also count the SDK values available from each retained
physical response: complete outcomes must claim the exact count, and failed or
cancelled outcomes cannot claim a value beyond the available prefix. This
narrows impossible processing claims but does not prove that a particular
decoded observation, tool argument or usage report came from those values.
The selected HTTP status and content type also determine that count. The
pinned OpenAI SDK rejects a non-2xx response before parsing, returns `null`
for nonstreaming HTTP 204, parses JSON only for its JSON media types, and
otherwise returns response text as one value, even when empty. A local probe
executed that installed SDK against JSON, text, absent media type, 204 and
malformed-JSON responses. The TypeScript replay owner and native and Python
count sources now use those transport facts. Focused TypeScript parser,
pipeline, response replay and writer-lease source tests passed; the native
Rust tests were authored but not run. Neither source tests nor code reading
prove a deployed provider or native execution; those remain unverified pending
the owner's gates. The Python
reader's focused generation and retry suite passed 70 cases after its count
check was added. The 64 previously failing Python unit cases were traced to
three stale authored fixtures: an accepted history decision before its
generation and a missing normalization seed, a Chat seed copied into a utility
request, and a stale normalized-call ID and generation hash. The fixture owners
now construct producer-valid evidence while retaining their original usage,
projection and refusal assertions. All 761 Python unit cases passed in the
authoring source. This does not qualify the SDK's packaged execution, the
native certifier, or deployed output.

An executed probe of the installed OpenAI SDK 5.11 yielded one JSON value for
an SSE `data:` line beginning with a UTF-8 byte order mark, including when that
line followed a completed earlier event. Two consecutive marks at one line's
start yielded no value. Its line decoder calls a fresh `TextDecoder` for each
physical line, so each line loses exactly one leading mark. The native value
counter now removes one mark from each buffered line; the Python counter uses
`utf-8-sig` for each line; and the TypeScript replay decodes the body while
retaining marks, then removes one from each line. Focused TypeScript SDK
differential tests passed for the initial, later and doubled cases, and the
focused Python generation test module passed 70 cases. Native behavior follows
from source inspection and an authored Rust test; it remains unverified by
execution pending the owner's gates. This is SDK value-count parity, not full
physical-to-generation proof.

The installed SDK's nonstreaming path calls `Response.json()` for JSON media
types and `Response.text()` otherwise. An executed local SDK probe on the pinned
Node 22 runtime accepted JSON bodies with one or two leading UTF-8 byte order
marks and rejected a third; `Response.text()` likewise removed two marks. The
TypeScript replay now removes the second mark after `TextDecoder`, and the
native and Python JSON value counters remove up to two before parsing. The
focused TypeScript suites passed 36 cases, including actual-SDK comparisons for
JSON and text marks; the Python generation module passed 72 cases. The native
source test was authored but not run. These observations establish the pinned
runtime's parsing boundary, not provider behavior or full generation provenance.

Generation-envelope counters also require exact numeric interpretation. A
rehashed envelope could spell an observed count as
`12.00000000000000001`: Java and native source retain the nonzero fraction,
while JavaScript and Python floating-point parsing round it to `12` before
admission. The TypeScript generation reader and Python SDK now refuse a decimal
whose exact value differs from the safe integer they decoded. A focused Python
reproducer admitted the rehashed fixture with its prior numeric interpretation
and refused it with the new one; focused TypeScript and Python source tests
passed. Java and native behavior is established by reading their decimal
parsers, not by executing their gates. This closes the numeric interpretation
gap for safe integer envelope values; it does not prove a generation came from
the retained physical response bytes.

A successful `structured_result` is an exception to matching the terminal
`result` against visible model text: the headless producer takes the first
successful `structured_output` tool submission as the terminal value. The
native certifier and the TypeScript, Python and Java readers now require that
the terminal value equal the accepted generation's arguments for that returned
tool call, and that the terminal `result` decode to the same JSON value. An
executed Python source probe admitted the valid authored stream and refused a
missing return, failed return, changed value, changed result text and omitted
structured field. The native, Java and TypeScript code and tests have been
reviewed as source only; their gates remain unrun. This tightens admission of
the existing v10 record shape; the producer and resume recording format do not
change.

The v7 source reproducer at
`/tmp/codex-output-disposition-review/baseline/REPORT.md` keeps every physical
request, response byte, processing outcome, history decision and usage total
unchanged. It changes only `generation_json`, its own hash and the completion
reference. The Python v7 reader and the TypeScript replay/partial owners admit
erased, replaced and duplicated accepted text, and a swap between accepted and
abandoned attempts with equal usage. The accepted SDK view follows the changed
envelope. Native source has the same missing comparison; native code was not
executed. This is a certifier and reader defect, not evidence that the backend
omitted bytes in an ordinary response.

A current v11 Python admission probe at
`/tmp/codex-stream-probes/current-v11-physical-forgery.py` uses the complete
`root_accepted` fixture (SHA-256
`85cfacbb9bbdbc8b8d19813bd801daca2fbbb0659d5e2afe76eb5346a544c319`).
The original and two rehashed variants all passed the Python reader. One
variant changed an accepted thought; the other changed visible text and the
terminal result. Both updated the generation and completion hashes while
leaving the 827-byte SSE response body and every physical response record
unchanged (body SHA-256
`59afe7bb2f0442c4ebcc4c3611e79f69e2e3b799a06fe19025659bc34ed9b2c1`).
The exact output is retained at
`/tmp/codex-current-v11-physical-forgery-report.json`. This executed probe
establishes that the current Python reader still admits this particular
byte-to-observation contradiction; it does not execute the native or Java
readers or a deployed provider.

The repair must make physical response decoding and generation construction one
verifiable chain. A verifier must reconstruct provider events from each exact
recorded body, including SSE frame boundaries and nonstreaming JSON; identify
which physical retry produced each decoded observation; and check the same
normalization of thought, text, calls, terminal reasons and served usage that
the producer applies. Failed and partly decoded responses still retain their
observed bytes and valid prefixes. A successful empty response is valid, so a
nonempty-output rule cannot establish this relation. Optional stream partials
are projections and cannot be made a second output authority. A hash supplied
by the same mutable generation envelope cannot prove its relation to the body.

## Source boundaries the repair must cross

`ModelResponseRecorder` persists HTTP headers and exact body chunks before the
OpenAI SDK consumes them. `ModelResponseReplay` now retains the current
response body until TypeScript admission compares it with decoded observations;
the Python and native response owners still discard the body after checking
its byte count and SHA-256. The OpenAI SDK's SSE
reader parses `data:` frames into JSON values, ignores `[DONE]`, and can stop
before consuming a remaining body after a parser or caller failure. The
nonstreaming path follows the HTTP status and media type to return one parsed
JSON, text or no-content value. A response digest proves the
retained body, not which of its events reached the converter.

The converter is stateful. It can suppress cumulative content and reasoning
prefixes, split tagged thought text, buffer content behind reasoning, assemble
fragmented tool arguments, remap colliding call indices, and defer terminal
publication. A generated observation can also be emitted as a diagnostic after
conversion failure. The frozen baseline invented a `createTime` from the local
clock when the provider supplied zero or omitted `created`; the current source
removes that fallback.
`RequestContext` supplies strict-call behavior, a forced tool name, exact usage
requirements and provider parsing options. The request body contains some but
not all of those facts. Source inspection therefore cannot justify a verifier
that reparses response bytes using only `model_request.body`. The frozen probe
also found a non-strict streamed tool call without a provider ID received a
clock-and-random ID. The converter now derives that client ID from the served
terminal response ID and remapped parser slot, resolving collisions with IDs
the provider supplied in the same response. Its 252 source converter tests
passed, including repeated decoding and an ID collision. This removes local
randomness from that conversion; a verifier must still distinguish the derived
client ID from a provider-supplied ID.

A frozen-source probe at `/tmp/codex-physical-decoded-review/baseline/REPORT.md`
ran the actual OpenAI SDK, selected provider, converter, chat generation owner
and canonical recording writer/reader against local SSE. The Default and MiniMax
cases have identical request-body bytes (SHA-256
`e213bf922d8c7ae881bf88b16fe51fa076fce1b5f0d5c864e4e0a11bdda55508`)
and identical 540-byte response bodies
(SHA-256 `a1b129ce98bd6ff0a74e9dbeda67a6b96850c8e363e1af91c4518acbe9a9f476`).
Default retains literal `<think>` text; MiniMax's tagged-thinking option turns
it into thought and visible-answer parts. The provider parsing choice is not
recorded in either physical body. All three fixture cases passed at this
source boundary; the outer CLI, service certifier and generated wire were not
executed. The same probe found that provider `created: 0` became a local
wall-clock `createTime` in the persisted generation. The converter now retains
`"0"` and leaves an absent provider time absent; its 251 source converter
tests passed. This only removes one invented metadata field, not the missing
raw-to-decoded proof.

The canonical root chat recording also contains physical request/response
records and generation evidence. Child artifacts copy generation evidence but
do not carry their own root-owned physical response records. Standalone child
evidence cannot claim verified raw-to-decoded membership unless it includes the
required physical proof or is explicitly verified against its root artifact.
Resume reads the canonical chat recording, not `output/events.jsonl`, so stdout
admission alone cannot establish resume's output provenance.

An implementable single authority needs to freeze the effective response decoder
context at dispatch and bind it to the selected runtime/provider configuration.
A reader cannot let a mutable generation choose whichever valid profile makes
its output pass. It must reconstruct the contributing source-event history for
each observation within its physical request, including events that advance
converter state without yielding an observation and observations that combine
several events. A stored one-to-one event pointer is neither required nor a
correct model of this pipeline. Every output-bearing source event must be
consumed, suppressed by a defined conversion rule, or outside a proved processed
prefix. Readers must derive or check the resulting text, thought, tool
arguments, finish reason and usage from that source, rather than accept an
unconstrained converter assertion. The root and any independently readable
child artifact need the same proof. Verification state should be scoped to the
current physical response and generation, not accumulate all response bodies
for a long session. The TypeScript source now implements the raw observation
comparison for its root readers; the remaining readers and normalization
mapping still need the same proof.

A deterministic source differential test compared the retained-response SSE
parser with the pinned OpenAI SDK on 1,044 combinations of complete, trailing,
mixed-newline, malformed and byte-order-mark frames. All three cases passed;
no parser divergence was observed in that bounded corpus. This supports the
TypeScript frame boundary used by replay but does not prove converter
equivalence for every provider value, physical output membership in
native/Python/Java, or a deployed response.

## Executed event-attribution cases

The focused source run at
`/tmp/codex-physical-decoded-review/event-attribution/REPORT.md` used the
installed OpenAI SDK 5.11, the actual request and response recorder, selected
provider, converter, Chat generation owner and canonical writer/complete reader
with local in-memory SSE responses. Three cases passed. It did not run the outer
CLI, native or Java reader, cancellation races, deployed provider or build gates.
The fixture's pre-conversion observer is at the pipeline boundary, not inside
the SDK JSON parser; its counts are SDK objects reaching conversion.

| Case | Recorded JSON SSE events | Objects reaching conversion | Generation observations | Consequence |
| --- | ---: | ---: | ---: | --- |
| Cumulative reasoning and text | 5 | 5 | 3 | A rewind yields nothing; trailing usage merges into a held terminal. |
| Fragmented tool arguments | 4 | 4 | 3 | An argument fragment yields nothing, but advances the parser used by the terminal call. |
| Conversion failure after a prefix | 3 | 2 | 3 | A failing event yields two diagnostics; a later recorded event never reaches conversion. |

The failed response recorded all 1,009 body bytes. The physical transport ended
cancelled, processing failed and history was abandoned. Its third JSON event
is in the recorded body but absent from both the pipeline observer and the
generation. The first two events produced one prefix and two diagnostics. An
admission rule that equates body presence with converter consumption would
falsely admit the suffix; one that equates event and observation counts would
reject valid cumulative and fragmented streams. The deterministic malformed-call
failure can be replayed to infer this particular stop point. A separate caller
cancellation case below proves that replay cannot infer every stop point from
the existing physical records.

The terminal observation in the cumulative case combines text from the fourth
event and usage from the fifth. The tool-call observation in the fragmented
case depends on all four events, including one that produced no observation.
The preparation-only observation has no Parts but is still evidence. Physical
usage is billed once through the outcome even when a failed generation repeats
the same cumulative usage in its prefix and diagnostics. These are executed
source-level facts; they do not establish complete physical-to-decoded
verification.

## Executed caller-cancellation ambiguity

The source-level reproducer at
`/tmp/codex-physical-decoded-review/cancellation-boundary/REPORT.md` passed
with the same SDK, recorder, pipeline, Chat and complete canonical reader. A
469-byte SSE body contains `A`, `B`, terminal `C` and `[DONE]`. Closing Chat's
generator after one delivery or after two gives identical physical `http`,
`body`, `end` and `outcome` events: the whole body is recorded, transport and
processing are cancelled, and history is abandoned in both. The first closed
generation retains only `A`; the second retains `A` and `B`. Both are valid
prefixes. The test compares the actual physical event values and the distinct
SDK objects that reached conversion, and the capture preserves the exact
generations and body. It did not exercise a deployed provider, the outer CLI
or a terminal-holding cancellation race.

The existing body, termination, processing outcome and disposition therefore
cannot independently determine what a cancelled caller received. The shared
producer must durably bind its processing progress to the physical response,
and every admitting reader must check that bound while replaying the exact
body. Progress must identify the SDK values admitted to conversion and the
observations delivered to Chat, or an equivalent boundary with those semantics.
The pipeline can hold a terminal pending EOF and expand a failed conversion
into diagnostics, so an SDK-object count alone is not established as sufficient.
This source-level result rules out a body-only replay check for cancellation;
the producer and reader migration still needs a versioned implementation.

## Executed diagnostic-delivery ambiguity

The local actual-SDK case at
`/tmp/codex-physical-decoded-review/cancellation-refactor/tests/diagnostic-boundary.test.ts`
passed against the current authoring source. Its exact capture is retained in
`docs/design/fixtures/physical-diagnostic-boundary.json`.
One 1,133-byte SSE body (SHA-256
`f2ef93330a5a9b1a7ed3976306f4f55961c4934ad2996900b8df3dfd691033c8`)
contains a valid prefix, a malformed tool-call terminal and an unprocessed
suffix. Conversion of the second SDK value yields more than one diagnostic.
The test closes Chat after the first versus second diagnostic. Both runs have
identical HTTP, body, cancelled end and cancelled outcome evidence, the same
two SDK values reaching the pipeline, and abandoned history. Their canonical
generations contain two versus three observations, respectively.

Thus an SDK-value count is also insufficient to determine a cancelled
generation. The producer needs a separate durable boundary for observations
Chat actually incorporated, associated with the physical response that yielded
them. A reader must check both boundaries against replay, including diagnostics
from one failed source value. This is established by the executed local case;
native and Python admission, terminal-holding cancellation, the outer CLI and
deployed provider were not exercised by it.

## Consumer closure follows response processing

History settlement has two valid publication paths. Failed Chat response
cleanup records `abandoned` immediately after its processing outcome, so that
decision can precede the attempt's generation. `accepted` is written by
`ModelResponseRecorder.finishHistory`, which `ChatAttempt.finish` calls after
attempting generation publication. A failed publication can leave an incomplete
fragment; a complete accepted attempt has its generation first. The native,
TypeScript, Python and Java admission paths now refuse
accepted history before that generation without refusing early abandonment.
Authored accepted fixtures were moved into this producer order. A Python source
probe admitted all 24 fixture sets and refused 23 reorderings that put an
accepted history row before its generation; the native, TypeScript and Java
tests were authored but not executed here. This ordering check does not prove
the generation observations came from the physical response bytes.

Source inspection establishes another constraint on where that boundary can be
recorded. `ContentGenerationPipeline.executeWithErrorHandling` calls
`responseEvidence.finish({ status: 'completed', error: null })` before it returns
a nonstreaming result. `BaseLlmClient.generateText` receives that result later
and only then projects its text, thought, calls, usage and terminal reason. In
the streaming path it incorporates each yielded chunk into a partial result and
preserves that partial on failure. `GeminiChat.processStreamResponse` separately
incorporates yielded chunks into its generation observations and normalizes tool
identities there. Utility calls and Chat therefore have distinct consumer sites;
neither a Chat-only marker nor a downstream-consumption count frozen into the
response `outcome` covers both correctly.

The physical response owner records the count of SDK values admitted to
conversion and decoded outputs delivered by the pipeline in each response
outcome. Chat records a separate receipt count after incorporating each output
into its generation; the attempt completion binds that count to the generation's
observation count and the outcomes of its physical requests. An accepted
completion must consume every pipeline output; an abandoned completion can
retain a shorter prefix when cancellation interrupts delivery. Stream contract v11
and canonical recording version 13 also require each generation observation to
name its physical request. The TypeScript and Python source tests exercised the
counts and per-request attribution, including early cancellation and a conversion
failure that emits several diagnostics. Native and Java source includes the same
admission checks but has not been compiled or executed here. A utility consumer
still has no corresponding receipt, and these counts do not themselves prove
that any decoded value came from the retained body bytes.

The utility owner is wider than `BaseLlmClient`. Source call-site inspection
found `generateText` and `generateJson` there, a direct generator call in
`PromptHookRunner`, a direct streamed call in the ACP generation service, and
`GeminiClient.generateContent` (also used by the goal judge). These pass through
`GenerationClient.generateContent` or `generateContentStream` with a null Chat
attempt. `ModelRequestJournal.capture` labels every such physical request
`{kind:'utility'}`, without a call identity; `ModelResponseReplay` closes it at
the pipeline outcome. A receipt added only to `BaseLlmClient`, compaction, or a
side-query call site would leave other utility responses without consumer
accounting. The shared generation-client delivery boundary needs a per-call
identity and a durable receipt tied to each physical request, including a
zero-output failure. If a claim also describes how a caller incorporated an
output, that additional claim must be recorded by the caller that made the
decision. The source census establishes these ownership paths; no receipt has
been implemented or executed by this note.

## Decoder proof required at admission

The dispatch owner must bind the selected provider and every effective option
that affects decoding: strict tool-call handling, forced tool name, exact usage
handling and tagged-thinking parsing among the current `RequestContext` choices.
The request JSON cannot supply all of them. The proven Default/MiniMax example
above has identical request and response bytes but different correct thought
and text parts. A generation envelope cannot choose its own decoding profile
after the fact. A changed required wire field needs a new stream-contract and
canonical-recording identity, with all producers and admitting readers changed
together. The policy and progress fields use stream contract v11 and canonical
recording version 13. A source-only migration cannot claim that generated
bindings or native gates have run.

For each physical response, a verifier must parse the exact stored bytes with
the pinned SDK's relevant SSE or nonstreaming semantics, replay the selected
converter and pipeline state, and compare the complete resulting observations
with the generation that Chat recorded. This includes parser preparations,
incomplete calls, normalized IDs, terminal holding, usage-only frames and
failure diagnostics, not just rendered text. It must account for successful
empty output, retries and failed prefixes without admitting later raw events
that conversion did not process. The response recorder writes bytes before
delivering them to the SDK, so its chunk boundary alone cannot prove a consumed
event prefix. For nonstreaming responses the SDK also has a distinct JSON/text
path; treating every body as SSE would be unsound.

The live pipeline uses the response decoder module for both nonstreaming
conversion and streamed conversion with terminal holding and failure-diagnostic
expansion. The module also exposes the pure served-usage observation applied
before either conversion path. Before each response attempt the pipeline selects
a validated JSON-shaped decode policy and creates fresh parser state from it.
Chat now applies `GenerationObservationNormalizer` to every delivered response;
that shared routine preserves the raw observation, normalizes tool identities
against the active history, records preparation mappings and removes executable
calls from the provisional delivered chunk. The TypeScript root readers now
replay the selected decoder from retained bytes and compare the raw response,
incomplete calls and provider preparation identities. Stream contract v11
records the exact history call-ID set at the Chat normalizer's start boundary.
The TypeScript root reader replays the same normalizer against the retained
physical response and that seed. TypeScript logical replay also derives the
expected duplicate suffixes and generated IDs from the seed before admitting
a generation, including when it cannot access a physical response. Native,
Python and Java admission derive those IDs too, but still do not decode
physical response bytes. These are source-level findings; the v11
build and native gates remain unrun in this workspace.
Source inspection narrows the required seed boundary. `processStreamResponse`
constructs `GenerationObservationNormalizer(this.history)` before consuming
the response stream, whereas the canonical generation record is committed
after consumption. `GeminiChat.addHistory` can append a versioned history splice
while a response is in flight. A reader using the history at the later
generation record could therefore derive a different duplicate suffix and
refuse an ordinary valid session. The dispatched OpenAI request is also not a
substitute: request curation and orphan cleanup can omit IDs still present in
Chat history. The v11 producer writes that seed before consuming the stream,
and root canonical admission compares it with replayed history at the same
record position. Child sidechains compare their local seed with their own
replayed history; the root's mirrored child seed still lacks a cross-file proof
that it equals the child's local seed. The concurrent history-splice case has
not been executed against a live client.
An executed source test decoded identical provider content with tagged-thinking
parsing off and on and obtained the two corresponding part sequences after a
policy JSON round trip. The earlier converter, pipeline and policy source suites
passed 443 cases; four local actual-SDK attribution and cancellation cases also
passed against their authoring source.

The selected policy is now required on every durable model request. The producer
checks that provider decoration preserved the selected stream mode before
recording. Request replay requires an explicit boolean `stream` in the exact
body and the matching policy mode, and evidence rejects an absent, malformed or
unknown policy. The v11 schema requires the same closed policy shape in stdout,
Python and Java resources; the canonical runtime file uses version 13.
Provider decoration can override the final OpenAI request model through
`extra_body`. The selected response policy now takes its model from that final
request, and TypeScript, Python, Java, native and fake-provider request readers
require the retained body's model to equal the policy's model. The producer
still sends the same request bytes; the relation fixes which model's response
decoder interprets them for every OpenAI-compatible caller. Focused TypeScript
source tests, the Python admission suite and the shared harness test exercised
this binding. The focused 27-case pipeline response suite passed against the
authoring source. Its progressing-write case holds each body write for half
the 60-second storage deadline while total storage time exceeds the 240-second
stream idle guard. The full TypeScript suite was not run for this change.
Native and Java cases were authored but not executed; the
model binding alone does not prove decoded observations came from physical
bytes.
Canonical files from before that identity lack required evidence; the version 13
reader refuses them with a matching-client or new-session action. Source reading
shows that this change adds evidence beside runtime history and does not change
the conversation parts or their order; complete version 13 resume remains
unverified pending the owner's gates. The Rust and Java refusal cases were
authored but not executed. The installed TypeScript
wire validator is still generated from v5 and rejects `stream_start` before
these new records; its full-wire tests cannot qualify v11 until the owner's
generation and build gates run. TypeScript root stream and canonical readers
now use the selected policy to replay retained bytes and compare raw Chat
observations. Native, Python and Java admission still need equivalent decoding
and comparison; the policy binding alone does not provide it.

The completion's physical request membership and each observation's
`source_request_id` identify the declared retry. The live Chat path only retries
before a stream is returned; later stream failure ends that attempt. Admission
therefore requires every observation to name the final physical request and
requires earlier requests to have delivered no decoded output. This was checked
against the actual SDK retry test and the TypeScript and Python source readers;
Java and native implementations remain unexecuted. The rule still does not prove
that the final request's bytes produced an observation. Replay must derive each
observation from the processed
prefix of that response. The accepted Chat history and displayed stream
projections then need to be checked against
that replayed generation and their declared disposition. Utility generations
and standalone child evidence also need a physical owner or an explicit root
verification dependency; a self-hash of their decoded SDK objects is not a
substitute. This is one shared proof rule for ordinary and long sessions,
bounded by one response at a time. It belongs at common producer and admission
boundaries, not in a benchmark profile or a client-only presentation patch.

The implementation must keep one output mode and bounded verification state for
ordinary and long sessions. The shared client decoding boundary is where the
observations are created, and every certifier/reader that admits a typed
generation must verify the physical relation before deriving accepted history.
The executed local fixtures supply bytes at the HTTP boundary, so they do not
demonstrate a vLLM omission. Changing backend response content or adding a
client presentation rule would not fix this admission gap. Canonical resume
continues to use the separate versioned runtime transcript, but any replay of
its generation evidence needs verified physical membership before it can claim
the model's exact output.

The TypeScript verifier passed focused source tests for streamed and nonstreamed
responses, early cancellation, malformed-call diagnostics and rehashed text or
body substitutions. A deterministic 600-body local parser probe matched the
pinned SDK's SSE value boundaries; it is not a proof for every transport shape.
The real failed-Chat fixture writes abandoned physical history before its
generation. The canonical TypeScript reader had discarded the response body at
that history record, so its later physical replay refused a valid session.
It now retains that closed response until generation verification, or checks and
releases it when the same attempt issues a retry. The three producer-backed
session-view source tests and the 63 response-evidence source tests passed; the
retry test also refused a forged decoded count against the retained body. One
session-view test now carries an accepted physical generation through a
post-compaction checkpoint, then compares the exact composed history after
load, indexed restore and fork. These tests do not establish complete resume
behavior across every session shape. A broader transcript-reader run failed 89
cases because its synthetic assistant fixtures omit the required generation
envelope; those fixtures must use canonical generations before they can serve
as resume evidence.
The native result-parse fixture now gives utility requests the required stream
mode and selected decoder, a complete nonstreaming response body and explicit
SDK/delivery counts. Its captured Chat branch rebases the normalization seed,
physical tool-call ID, source request ID, generation, completion and displayed
tool ID together, then reseals the changed response and generation bytes. The
terminal result comes from the accepted generation's visible text. A Python
source reader admitted the intended rebased Chat and utility vectors, and the
TypeScript physical verifier accepted the rebased Chat bytes and observations.
These cross-checks exercise the intended vector, not the Rust test helper; no
native test or build was run, and this fixture repair does not add native
byte-to-observation admission.
The native certifier, Python and Java readers, utility consumer receipts,
standalone child proof remain open. The earlier local rehashed replacement of
a preparation's normalized ID and its matching call mapping exposed the
TypeScript logical reader's missing seed derivation. The current source uses
the live normalizer for that check and has authored prepared and generated-ID
forgery tests. Its focused TypeScript suite passed 17 cases with the official
prebuilt Node 22.16.0 runtime; the adjacent logical completion and retained
response replay suites passed 68 more cases after a stale EOF fixture was
corrected to use a failed outcome and abandoned history. This
does not establish the byte-to-observation relation for readers without
physical response replay. No compiler, native certifier, deployed provider or
release gate has verified the full repair.

## Provider coverage at the request boundary

The patched Qwen client can select OpenAI-compatible, Qwen OAuth, Anthropic,
Gemini and Vertex generators. Qwen OAuth inherits the OpenAI generator. In the
current source, the only production call to `modelRequests.capture` is in the
OpenAI pipeline. `AnthropicContentGenerator` and `GeminiContentGenerator` build
their own requests and call their SDKs without admitting a model request or
attaching a response recorder to `request.chatAttempt`. The shared logging
wrapper passes the request through; it does not admit one. `GeminiChat` then
reads `attempt.journalId` while freezing the generation, and `ChatAttempt`
requires at least one attached physical request. By code inspection, those
provider paths cannot produce a completed, request-owned chat generation in
the current implementation. This runtime consequence has not been executed.

The provider transports do not expose one common interception point in the
installed SDKs. The Anthropic SDK accepts a custom `fetch` function. The
Google Gen AI SDK's `ApiClient.apiCall` calls the global `fetch` and its
`GoogleGenAIOptions` type has no fetch option for this models path. Capturing
the logical `GenerateContentParameters` before either SDK transforms it would
claim exact provider bytes that the recorder has not seen. A complete repair
must admit the actual dispatched body and response at each provider's
transport boundary, select a provider-specific decoder policy before dispatch,
and attach every chat retry to the common attempt journal. The journal and
admitting readers must retain one shared completeness rule while interpreting
each provider's distinct wire format.

The sealed `agent_service` deployment is narrower than the patched client's
provider menu. `docker/config/settings.json` enforces `openai` authentication
and names only its local vLLM provider; `docker/scripts/verify_runtime_contract.py`
refuses a settings file with any other provider set. An OpenAI response proof
therefore covers every model session admitted by this deployment, including
ordinary and long sessions. The other generators remain a separate client
coverage defect if the patched Qwen package is used outside the sealed service.

This finding also limits the existing physical-output proof plan. A verifier
for OpenAI SSE is necessary for the vLLM deployment but cannot be described as
end-to-end proof for Anthropic, Gemini or Vertex. No provider transport was
called and no build or test was run to establish this section; its claims are
from the current client and installed SDK source.
