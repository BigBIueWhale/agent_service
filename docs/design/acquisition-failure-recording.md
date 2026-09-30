# Admitted attempts whose stream was never acquired

A chat attempt owns its physical provider requests even when the provider
stream is never acquired: the retry loop can exhaust after one or more
requests were durably admitted and answered, before `processStreamResponse`
starts. Such an attempt still needs its generation and completion; otherwise a
reader would find physical requests that belong to no generation. A failed
preflight that admitted no request has no physical work and records none.

## Ownership

`ModelRequestJournal.capture` takes the live chat attempt, or none for utility
work under a `UtilityDelivery` owner, and derives the request's owner record
itself. Inside the journal's serialized queue it refuses a failed journal, an
attempt that can no longer issue requests, and caller cancellation that
arrived before admission; a request cancelled before admission is never
dispatched and consumes no sequence. Once the request record is durable, the
response recorder is attached to the attempt or utility owner immediately,
before the request is published to any output, so a publication failure cannot
leave a durable request without its owner. A failure after admission settles
that response as failed or cancelled, preserving the original error and any
distinct cleanup failure.

`GeminiChat` catches only the failure of the acquisition retry operation. If
the attempt admitted a request, it records the attempt's normalization seed
and a `model_generation` with no observations, naming the final physical
request; the attempt then settles `abandoned`. Physical errors, bytes and
usage stay in their response records. The empty generation is not model text
and not a claim of zero tokens. A successful inner retry reaches the ordinary
stream owner and writes no extra generation. After acquisition succeeds, the
processed-stream cleanup is the sole owner of generation recording, including
parser failure and iterator return.

## Scope and resume

This lives in the shared request journal and chat owner, so root and child
chats behave the same, and utility requests keep their own physical ownership.
The vLLM backend owns neither acquisition retry nor the canonical file, so no
backend change belongs here. Resume reads the canonical runtime recording, not
`output/events.jsonl`; the empty generation is evidence and is never replayed
as an assistant turn.
