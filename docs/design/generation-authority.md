# Generation authority

A model generation is recorded once, as an immutable envelope, and every other
view of it is derived from that record: the history a resume restores, the
assistant rows the stream shows, the calls the runtime executes and the
usage it reports. This note states who owns each fact and what readers check.

## Attempts and physical requests

A chat attempt has one identity, allocated by `GeminiChat` before the provider
call. Every physical provider request it issues names that identity and is
owned by chat; utility requests name utility ownership instead. Ownership is
recording context, never part of the model's request body. Connection retries
can issue several physical requests for one attempt; a streaming retry starts
a new attempt identity.

Each physical response records its transport end and its processing outcome
separately, and a chat response then carries the chat's history decision. A
failed or cancelled processing attempt is abandoned. A completed chat response
stays open until the chat durably commits its turn or abandons it. Utility
processing has no conversation-history decision. Raw bytes, SDK processing and
history acceptance remain separate facts owned by the layer that knows each.

## Stream identity and runtime metadata

`system/stream_start` owns the stream contract identity, the session identity
and the request journal origin, before any data record. It makes no runtime
capability claim. `system/init` is the complete runtime snapshot of the
initialized CLI; it is not a journal header. There is one shape for each fact,
because stream-json authentication can fail before `Config.initialize` and SDK
MCP registration precedes tool discovery, so an eager snapshot there would
invent empty tools or agents.

The service certifier binds a complete `init` to its pinned runtime manifest
before any model request and before a successful terminal. A startup error with
no model work claims no initialized capabilities. A runtime snapshot never
resets request replay. The native reader owns exactly one invocation and
refuses a second `stream_start`.

## The generation envelope

`GeminiChat` owns one immutable envelope for each admitted attempt. It
observes decoded responses before calls, terminals or incomplete-call metadata
are removed for live delivery, and retains ordered parts, raw thought text,
nontext parts, citation metadata, incomplete arguments and nullable served
usage. Observations precede tool-call normalization: each candidate keeps its
original parts, and a positional mapping names the normalized call ID or the
suppression of an already observed provider ID. The call IDs present in
history when normalization starts are recorded as the attempt's normalization
seed, before the provider stream is consumed, because a history splice can
arrive while a response is in flight. Generation usage and finish summaries are
derived from observations, not asserted beside them.

`model_generation` carries the envelope as exact UTF-8 JSON text with its byte
length and SHA-256. The text is frozen once; readers parse the retained bytes
rather than reproducing another serializer's output. A decimal whose exact
value differs from the safe integer a JavaScript reader decodes is refused, so
two readers cannot interpret one count differently.

## Completion and disposition

`model_attempt_completion` binds the journal, the generation identity and hash,
the attempt and scope, the exact ordered physical request identities, the
number of observations the chat incorporated, and one disposition:

- `accepted`: the turn entered history; its calls become executable.
- `refused`: the turn is drawn again, and it has no executable call. Either
  the provider completed it at its output limit (finish `MAX_TOKENS`), or its
  response ended in the backend's typed refusal (`RepeatedToolParameterError`)
  with no finish and no served usage; the client records that response's
  failure as `RepeatedToolParameterRefusal: ...` exactly when its bytes end in
  the refusal's error payload, which both certifiers check. It never enters
  history; the turn is drawn again on the same request with the refusal
  notice added, up to `MAX_GENERATION_DRAWS`.
- `abandoned`: the attempt ended without a turn, for example a retried or
  cancelled stream. It is evidence only.

The completion's ordered membership must equal every physical request
observed for that attempt, each with the same journal, attempt and scope.
Every observation names the final physical request, and earlier retry requests
must have delivered no decoded output. An accepted or refused completion
consumes every output its final response delivered and carries that
response's served usage -- none for a response the backend refused, which
served none; an abandoned one can keep a shorter prefix. Served
usage is owned by physical response outcomes; completion does not create a
second billed source.

## Shown turns

After an `accepted` or `refused` completion the stream shows that generation
as upstream-shaped `assistant` rows: `{id, type: "message", role, model,
content, stop_reason, usage}`. Blocks keep production order and each message
holds one block kind: `thinking`, `text`, `tool_use` with the call's
arguments, or `incomplete_tool_use` with the served name and argument text.
Usage appears on the last message only; `stop_reason` is `tool_use` exactly
when every block is a call. One function (`generationDisplay` in the client,
its counterpart in the native engine) derives these rows from the envelope and
disposition, and the native certifier refuses a row that differs, is missing,
is extra or is out of place. An `abandoned` attempt shows no row, so a retry is
never displayed as a turn that happened.

Assistant rows and `stream_event` partials carry no origin field. A partial
belongs to the latest chat request in its scope. The root scope brackets its
output in `message_start` (whose message names the model) and `message_stop`;
a subagent scope streams bare content blocks; a block index restarts at 0 once
every block is closed. A runtime-authored answer, such as a local slash
command's reply, is preceded by a `system/runtime_operation` receipt whose
identity, scope, byte length and SHA-256 match the row, and is a text-only
assistant row with null `stop_reason` and `usage`.

## Ordering

A generation is durable, and accepted history committed with it, before its
physical response settles; the logical completion is durable before any
executable terminal is published. The accepted canonical line commits the
envelope and its history position together. Stream cleanup shares the same
memoized settlement, so it cannot create a second generation or completion.
An acquisition failure after a request was admitted records an empty observed
generation; with no admitted request there is nothing to record. Journal
persistence callbacks await only their own append, never the aggregate
recorder flush that itself awaits the journal.

## Output scope

`GenerationContext` owns an output scope independently of its KV scope:
`conversation` names the root or an actual spawning tool call, and `internal`
names work outside the displayed conversation. The envelope's hashed bytes
retain the declaration. A child launched inside internal work inherits internal
visibility, including after background resume. Internal generations still own
their history, physical requests and usage; they show no conversation rows and
grant no displayed tool authority. A KV scope alone never proves a spawning
tool call: conversation ancestry is checked against issued calls.

## Canonical placement and readers

An accepted canonical assistant line holds the envelope and its exact history
position; history is projected from it by the same normalization live chat
uses. Other generations are `model_generation` evidence with a placement:
`local` (an abandoned attempt of this file), `child` (a parent-journal copy of
a child's generation) or `inactive` (a branch no longer selected). Forking
turns unselected local evidence inactive, so a later fork cannot revive it.
Runtime history never includes evidence; presentation adds a branch's
abandoned generations without making their calls executable.

A live canonical file ends at a prefix boundary, not a closure. Closed load,
indexed restore, background recovery and fork require complete physical and
logical evidence before history becomes runnable; `readSessionView` returns an
explicit live-prefix view for in-progress displays. A child artifact binds its
generations to completions, while its physical evidence lives in the root
journal, so complete child admission reads the root as well.

## Usage populations

Reported started turns, chat attempts and physical requests are three
populations and are never merged. Terminal usage comes from the physical
recording window: every admitted request, utility work and retries included,
enters it, and each validated processing outcome contributes once whether it
completed, failed or was cancelled. Later acceptance or abandonment does not
rebill it. An absent report stays null and counts as unreported; a reported
zero stays a report. `num_turns` is the terminal's started-turn assertion and
is never inferred from request, attempt or usage counts.

## Raw reasoning and display

The core `ThoughtSummary` event carries the exact raw reasoning beside its
parsed subject and description. The UI's bounded display value,
`ThoughtDisplay`, holds only the two display fields, and dual output receives
the original event before that reduction, so a reduced value never stands in
for the raw one.
