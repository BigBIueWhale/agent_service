# Record served usage on its physical response

Every physical processing outcome carries mandatory nullable `served_usage`.
A non-null value is an owned snapshot of exactly five non-negative safe integer
counts: prompt, output, total, cached prompt and reasoning output. Total equals
prompt plus output; cached does not exceed prompt; reasoning does not exceed
output. Null means no complete valid report was observed by processing. It is
not zero cost and does not assert that the raw bytes contain no usage-like data.

The shared OpenAI pipeline observes SDK completions and chunks before provider
output callbacks, telemetry, `error_finish`, content conversion or terminal
buffering. The response recorder retains the last complete valid cumulative
report. Updates replace rather than sum, even if the latest valid report is
smaller. A malformed or absent later report cannot erase an earlier valid one.
The raw response records retain the original bytes, including invalid reports.
No second raw-body parser tries to infer usage from an SDK-rejected error body.

Null or absent chunk usage is normal. A present invalid report is a processing
failure. The pipeline preserves existing converter/provider error precedence:
it records the invalid-report cause, runs conversion, and raises a diagnostic
response error if conversion otherwise succeeds. This catches false, zero and
empty-string values that truthiness checks ignored, while retaining same-chunk
text diagnostics. Valid usage survives failed or cancelled processing, parser
errors and body cleanup failure. Observation after outcome settlement starts is
refused. Utility requests, root and child chat requests, and physical retries
use the same owner; reports cannot move between request IDs.

The existing mapper supplies both converted metadata and physical observations.
It preserves absent provider details instead of fabricating zero. Exact served
usage and strict tool calling are the client's one decode mode, so there is no
permissive converter path that could report usage differently. This adds no
configuration, benchmark mode, provider request field or model-history content.
The vLLM deployment supplies these counts; it does not own the client's
physical response journal or canonical history, so no backend workaround is
needed.

## Physical response bytes determine served usage

The physical SSE reader retains the value that the pinned OpenAI SDK yields. A
`thread.*` event yields an object with `event` and `data`, even when the JSON
data is null; an ordinary event yields its JSON data directly. Readers compare
that SDK-shaped value with a compaction draw's `sdkValuesJson`, so the inner
JSON alone cannot be passed off as what the client consumed. A `thread.*` event
does not contribute a served-usage report.

The producer observes the last complete, valid OpenAI usage report among the
SDK values it processed, before attempting to convert each value, so a failed
conversion can still carry a real served-usage report. The native engine and
the client record verifier derive the same five counts from the recorded
nonstream JSON or SSE prefix named by `sdk_values_seen`. Missing or invalid
reports leave the last valid report in force; values physically present after
a failed SDK read cannot change it. The declared nullable `served_usage` must
equal that derived value. This applies to ordinary chat and utility responses
as well as compaction draws.

Resume reads runtime chat JSONL, not `output/events.jsonl`. Served usage
belongs to response evidence, which is structurally disjoint from history.
