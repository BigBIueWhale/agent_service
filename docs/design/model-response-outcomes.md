# Response processing must agree with its transport evidence

The request's physical response records a transport ending and a processing
outcome. The transport ending precedes the recorded outcome; parsing may finish
before unread transport is cancelled. History acceptance is a later decision
owned by the chat. Served usage is independent of that history decision.

Successful processing requires a captured HTTP status in the successful
200–299 range and a recorded transport ending of `eof` or `cancelled`.
`not_dispatched`, absent HTTP headers, unsuccessful HTTP, and failed transport
cannot substantiate a successful processing claim. The shared recorder checks
this immediately before appending its processing outcome, after cancellation
and persistence have settled. The replay and native reader check the same
relationship before accepting that outcome.

Cancellation of unread transport can follow successful parsing; it does not
necessarily mean processing was cancelled. Conversely, complete transport
can be followed by a parsing failure or user cancellation. Failed and cancelled
processing therefore remain valid after each otherwise valid transport ending.
The invariant does not require a positive body length: empty provider processing
may complete before the chat rejects missing generation content or termination.

The record shape is shared: canonical runtime chat JSONL and stdout
request/response evidence use the same response recorder and replay. The rule
governs evidence admission, not conversation parts or history order. The
native certifier and the shared fake-provider verifier enforce the same
relationship, and both the composition and headless checks use that verifier.

This applies to every provider request recorded by the shared Qwen pipeline,
for both chat and utility owners, independent of session size or benchmark.
vLLM owns neither the client's transport journal nor its processing decision,
so no backend change belongs here.
