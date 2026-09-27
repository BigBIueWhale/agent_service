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
its output pass. It must tie every converted observation to the exact physical
request and provider event position, and verify that every output-bearing source
event is consumed, suppressed by a defined conversion rule, or retained as a failed
prefix. Readers must derive or check the resulting text, thought, tool
arguments, finish reason and usage from that source, rather than accept an
unconstrained converter assertion. The root and any independently readable
child artifact need the same proof. Verification state should be scoped to the
current physical response and generation, not accumulate all response bodies
for a long session. The source inventory establishes these requirements; it
does not establish an implemented verifier.

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
