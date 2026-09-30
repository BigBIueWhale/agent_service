# Physical output provenance

The response journal keeps the exact HTTP body bytes a provider returned; the
generation envelope keeps the decoded observations chat built history from.
They are distinct authorities, and a hash supplied by the same mutable envelope
cannot prove how one relates to the other. A generation is admitted only when
its observations are derived again from the recorded bytes of the physical
request that produced them.

## Why the relation must be replayed, not counted

The OpenAI SDK and the shared converter are stateful. The SDK's SSE reader
yields a value only after a blank-line boundary (`\n\n`, `\r\r` or
`\r\n\r\n`), flushes its residual bytes only at transport EOF, drops one
leading UTF-8 byte-order mark per line, ignores `[DONE]`, and can stop before
the rest of a body after a parser or caller failure. Its nonstreaming path
follows HTTP status and media type: a non-2xx response is rejected before
parsing, HTTP 204 yields `null`, JSON media types go through `Response.json()`
and any other type returns its text as one value. The converter then suppresses
cumulative content and reasoning prefixes, splits tagged thinking, buffers
content behind reasoning, assembles fragmented tool arguments, remaps
colliding call indices, holds the terminal until the finish reason and usage
arrive, and can expand one failed value into several diagnostics.

So neither equality of event and observation counts nor presence of bytes in
the body establishes an observation: a cumulative or fragmented stream has
fewer observations than events, and a failed conversion leaves recorded events
that never reached the converter. A cancelled caller is not determined by the
body either: two cancellations after different deliveries can leave identical
HTTP, body, end and outcome records. Each response outcome therefore records
how many SDK values reached conversion and how many decoded outputs the
pipeline delivered, and the chat's completion records how many it
incorporated. Replay reads exactly that prefix.

## The decoder is fixed at dispatch

Decoding depends on facts the request body does not fully carry: the same
request and response bytes decode to literal `<think>` text or to separate
thought and answer parts depending on the tagged-thinking choice. Every
durable model request therefore carries its selected decode policy: stream
mode, the model named by the final request body, `strict_tool_calling` and
`exact_token_counting` (both `const true`), the named tool choice and the
tagged-thinking rule. Readers require the body's `stream` flag and model to
match the policy. A generation cannot choose its own decoding profile after
the fact.

Strict tool calling and exact served usage are the client's one decode mode:
`validateModelConfig` refuses any route other than the OpenAI-compatible vLLM
route with both set, and the OpenAI converter has no lenient branch. A
streamed call the provider served without an ID receives one derived from the
terminal response ID and parser slot, with collisions against provider IDs in
the same response resolved deterministically, so replay reproduces it.

## Replay

For each physical response, the verifier parses the stored bytes with the
pinned SDK's SSE or nonstreaming semantics under the recorded transport end,
replays the selected converter and pipeline state over the recorded value
prefix, applies `GenerationObservationNormalizer` from the attempt's recorded
normalization seed, and compares the complete result with the generation:
parts, incomplete calls, provider preparation identities, normalized call
IDs, terminal reason and served usage. A successful empty response is valid.
Every observation names the final physical request of its attempt; earlier
retry requests must have delivered nothing. Verification state is one response
at a time, so long sessions do not accumulate bodies.

Complete child admission joins the root: a child's generations are verified
against the physical records in the root journal, which must be the unique
active or archived parent recording for that child. A missing, ambiguous or
mismatched root is a refusal.

## Utility work

`GenerationClient` wraps each non-chat call in `UtilityDelivery`. Every
physical request of the operation carries one operation ID, created before
`retryWithBackoff`'s attempt loop, and a `utility` owner. The wrapper counts an
output only when it crosses its return or iterator boundary and records that
count after the response outcome; a request that returns nothing records zero.
Readers require the receipt and refuse a count beyond the recorded pipeline
prefix. A compaction draw's `sdkValuesJson` must equal the values decoded from
its operation's recorded response bodies, in order, and its converted fields
are replayed from them.

The receipt proves delivery across the shared client boundary, not what a
caller then decided. `PromptHookRunner` races its provider promise against
timeout and cancellation, so a losing request can finish after the hook has
returned; a content-retry choice or hook judgement would need evidence from the
caller that makes it. Compaction records its own draws and acceptance.

## Where each reader stands

The service certifies a captured stream in two passes over the same bytes. The
native engine admits every record: it replays request bodies, verifies each
response's byte count and SHA-256, counts the SDK values each body can yield,
derives served usage from those values, checks the generation envelope,
completion, shown rows and displayed inputs, and hashes the exact LF-terminated
bytes it read. It does not decode chat observations from response bytes.
`dist/record-verifier.js`, built from the pinned client's converter and
normalizer, then rereads the file, requires the native-read byte count and
SHA-256, and replays every generation and compaction draw from its recorded
responses. The service runs it only for a complete native result and keeps the
verifier's stated reason in its refusal; a missing verifier is a refusal, not a
weaker mode.

## Stated limits

The proof covers the OpenAI-compatible pipeline, the only route the client
admits for generation. The opt-in `web_search` tool calls a DashScope endpoint
directly, outside `GenerationClient` and the request journal, so a standalone
Qwen session using it does not record that side call; the sealed deployment
neither enables the tool nor can reach that endpoint. Bytes the backend never
transmitted and parser omissions inside the backend are outside what a client
record can show.
