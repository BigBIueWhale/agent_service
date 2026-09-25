# Chat attempt evidence

A chat attempt has one identity allocated by GeminiChat before the provider call.
Every physical provider request it issues names that identity and is explicitly
owned by chat. Utility requests explicitly name their utility ownership. The
ownership metadata is recording context, never part of the model's request body.
Connection retries may issue several physical requests for one chat attempt;
streaming retries create a new attempt identity.

The existing model_response sequence carries the chat history decision after its
transport end and processing outcome. A failed or cancelled physical processing
attempt is abandoned. A completed physical chat response stays open until the
chat either durably commits its assistant turn or abandons it. Utility processing
has no conversation-history decision. Raw bytes, SDK processing and chat history
acceptance remain separate facts owned by the layer that knows each one.

GeminiChat settles every admitted physical response in its finally path, including
consumer cancellation. It sends attempt-start identity before any output and
preserves it in child round and tool events. Output adapters retain that identity
through content-category splits and close the prior stream on retry. They preserve
abandoned reasoning verbatim. Every model-produced assistant fragment refers to
its attempt; the shared readers resolve that reference to the recorded decision.
A missing identity or missing decision refuses complete certification.

The separate child `.stream` sidecar still needs strict versioning and grouping
by attempt so its provisional and abandoned observations remain distinct from
committed assistant turns. That reader/writer change is outstanding; this change
covers canonical request/response evidence and JSON output adapters.
Canonical history remains commit-driven; evidence and response dispositions can
never become model turns or history parents. Resume and fork must reproduce the
same conversation, while retaining inspectable abandoned output separately.

This behavior belongs to the shared client runtime and recording boundaries for
ordinary sessions, main and child chat, and every output adapter. The backend
cannot know whether a caller later commits a response into conversation history;
backend-owned generation omissions require a separate backend fix. No benchmark
or deployment-configuration exception can supply this client-owned decision.

Verification must cover legal reasoning-only retry, accepted and refused output,
connection retries, cancellation before and after a durable commit, empty output,
partial-message balancing, child tool and round delivery, concurrent scopes,
missing or foreign references, and exact full/indexed/fork restoration. Native
changes receive source checks only under the owner's no-build constraint.
