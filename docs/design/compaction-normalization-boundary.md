# Compaction normalization boundary

The compaction producer retains three different facts. `model_response` body
events hold the bytes received from the provider. `sdkValuesJson` holds each
value yielded by the pinned OpenAI SDK before conversion. The draw's `text`,
`reasoning`, calls, usage and terminal reason come from the shared OpenAI-to-Core
converter and the utility client's delivered output prefix. These are distinct
boundaries: equal SDK values do not by themselves prove equal converted output.

The TypeScript record reader replays the captured HTTP body through the SDK
value boundaries and `OpenAIStreamDecoder` or `decodeOpenAIResponse`, then
compares the delivered output to the draw. The native, Python and Java readers
bind `sdkValuesJson` to the physical byte prefix but do not yet rederive every
converted draw field. An executed Python admission probe accepted a failed
draw with zero SDK values after its text was changed from empty to invented
text. That probe establishes a real internal-consistency gap; it is not a
provider observation.

The zero-value case has a direct source proof. The shared pipeline increments
`sdk_values_seen` before offering a value to the converter or `ProviderOutput`.
Without a value, the nonstream converter is never called. The streaming decoder
has no terminal to flush and yields no response. The utility client's initial
or failed partial therefore has empty text, reasoning and call arrays, with no
usage or terminal reason; the compaction failure path has no candidate count
or snapshot bytes. The native, Python and Java readers now require exactly
those fields when the draw's physically checked SDK value list is empty.
This rule applies to every compaction operation, regardless of model, session
length or benchmark. The backend does not own this client conversion or record.

The Python regression failed before the rule and passed afterward, together
with the Python SDK unit suite. Native and Java regressions are authored but
unexecuted. Compilation, application resume and owner gates remain unverified.
The change strengthens admission of the existing versioned stdout records; it
does not change the producer or canonical chat JSONL. Resume still reads that
canonical history, not the service's `output/events.jsonl` copy.

For nonempty SDK value lists, direct readers still need to establish the
converter's normalized text, reasoning, structured and incomplete calls,
terminal reason, usage and candidate snapshot from the physical response.
They must account for decoder state, tagged thinking, cumulative deltas,
streaming tool-call assembly, failures, and the delivered prefix. A record
that merely repeats the draw fields as an unattested claim would preserve the
same gap. This is outstanding work before complete record certification can
be claimed.
