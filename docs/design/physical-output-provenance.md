# Physical output provenance

The response journal retains exact HTTP body bytes and the generation envelope
retains decoded observations used to construct model history. Those are distinct
authorities. Request IDs, response history decisions, generation hashes and
served-usage equality currently bind their declared ownership, but do not
establish that a decoded observation came from the retained response bytes.

The current v7 source reproducer at
`/tmp/codex-output-disposition-review/baseline/REPORT.md` keeps every physical
request, response byte, processing outcome, history decision and usage total
unchanged. It changes only `generation_json`, its own hash and the completion
reference. The Python v7 reader and the TypeScript replay/partial owners admit
erased, replaced and duplicated accepted text, and a swap between accepted and
abandoned attempts with equal usage. The accepted SDK view follows the changed
envelope. Native source has the same missing comparison; native code was not
executed. This is a certifier and reader defect, not evidence that the backend
omitted bytes in an ordinary response.

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
OpenAI SDK consumes them. `ModelResponseReplay` and the Python and native
response owners currently keep byte counts and SHA-256 state, then discard the
body. None can later compare a generation to its source. The OpenAI SDK's SSE
reader parses `data:` frames into JSON values, ignores `[DONE]`, and can stop
before consuming a remaining body after a parser or caller failure. The
nonstreaming path parses one JSON completion. A response digest proves the
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
for a long session. The source inventory establishes these requirements; it
does not establish an implemented verifier.

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

The physical response owner can bind SDK values and decoded outputs as they pass
through the pipeline. Consumer membership must then be recorded at the Chat or
utility consumption site, with a response request identity carried to that site
and a closure later than the transport outcome where necessary. The admitting
reader must require this closure before treating a generation or utility output
as complete. This placement follows from the current source lifecycle; no wire
field, consumer receipt or cross-reader verifier has been implemented here.

## Decoder proof required at admission

The dispatch owner must bind the selected provider and every effective option
that affects decoding: strict tool-call handling, forced tool name, exact usage
handling and tagged-thinking parsing among the current `RequestContext` choices.
The request JSON cannot supply all of them. The proven Default/MiniMax example
above has identical request and response bytes but different correct thought
and text parts. A generation envelope cannot choose its own decoding profile
after the fact. A changed required wire field needs a new stream-contract and
canonical-recording identity, with all producers and admitting readers changed
together. The policy field uses stream contract v8 and canonical recording
version 10. A source-only migration cannot claim that generated bindings or
native gates have run.

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
An executed source test decoded identical provider content with tagged-thinking
parsing off and on and obtained the two corresponding part sequences after a
policy JSON round trip. The earlier converter, pipeline and policy source suites
passed 443 cases; four local actual-SDK attribution and cancellation cases also
passed against their authoring source.

The selected policy is now required on every durable model request. The producer
checks that provider decoration preserved the selected stream mode before
recording. Request replay requires an explicit boolean `stream` in the exact
body and the matching policy mode, and evidence rejects an absent, malformed or
unknown policy. The v8 schema requires the same closed policy shape in stdout,
Python and Java resources; the canonical runtime file uses version 10.
Version 9 canonical files cannot supply the selected policy; the version 10
reader refuses them with a matching-client or new-session action. Source reading
shows that this change adds evidence beside runtime history and does not change
the conversation parts or their order; complete version 10 resume remains
unverified pending the owner's gates. Focused TypeScript
request and pipeline tests passed 23 and 168 cases respectively; the
Python unit suite passed 731 cases, including new policy refusals. The Rust and
Java refusal cases were authored but not executed. The installed TypeScript
wire validator is still generated from v5 and rejects `stream_start` before
these new records; its full-wire tests cannot qualify v8 until the owner's
generation and build gates run. No admitting reader yet replays retained response
bytes through the selected decoder or compares them with a canonical generation.
The policy binding is therefore a necessary input to physical-to-decoded proof,
not that proof itself.

The completion's physical request membership gives the candidate requests but
does not by itself identify which retry supplied an observation. Replay needs
the request order and processing termination for each candidate. The accepted
Chat history and displayed stream projections then need to be checked against
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

This note records an open design obligation. No compiler, native certifier,
provider or deployed application execution establishes the repair yet.
