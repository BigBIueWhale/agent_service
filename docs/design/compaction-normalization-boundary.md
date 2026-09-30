# Compaction draw boundaries

A compaction draw is recorded at three boundaries, and each is checked against
the one before it:

1. `model_response` body records hold the bytes the provider sent for each
   physical request of the draw's utility operation.
2. `sdkValuesJson` holds, as JSON text, each value the pinned OpenAI SDK
   yielded from those bytes before conversion. These values keep argument
   strings even when malformed or cut off; they are not the HTTP bytes.
3. The draw's `text`, `reasoning`, calls, incomplete calls, usage and terminal
   reason come from the shared OpenAI-to-Core converter and the prefix of
   outputs the utility client actually returned to compaction.

Equal SDK values do not by themselves prove equal converted output, because
the converter is stateful (see [physical output provenance](physical-output-provenance.md)).

## What binds each boundary

Each draw names its utility operation and states its physical request count.
Its `sdkValuesJson` must equal, in order, the values decoded from that
operation's recorded response bodies, up to each response's recorded
`sdk_values_seen`: a failed conversion can stop after an early value while the
captured body holds later complete values, and those later bytes stay in the
journal without becoming compaction output. A member may be any JSON value,
because the SDK returns a string for a `text/plain` body. Earlier retry
requests of the operation must have delivered nothing.

Delivery is a separate boundary. The utility wrapper counts an output only
when its caller receives it and records that count as a delivery receipt.
`generateText` needs a terminal reason and served usage before it returns a
candidate, so a successful replacement, or a candidate refused by a
redrawable rule, has at least one output delivered from its final request; a
failed draw may retain a processed value that never reached the caller.

The native engine checks the operation, scope, request count, output ceiling,
SDK values and receipts, and projects the draw from the delivered prefix of
the response's recorded decoded outputs, or from its recorded failure when
nothing was delivered; a draw that differs is refused. The client record
verifier (`dist/record-verifier.js`) then replays those decoded outputs from
the recorded bytes with the selected decoder, so the draw is bound to what the
provider sent.

## Scope

The rule applies to every compaction, whatever the model, session length or
benchmark. The backend does not own this client conversion or record. Resume
reads canonical chat JSONL, where the committed history is stored once as the
checkpoint's `compressedHistory`, not the service's `output/events.jsonl`.
