# Shared captured-stream gate and producer contract

One versioned JSON definition, agent_service's
`protocol/stream-contract-v21.json`, owns every accepted top-level and nested
record variant and its structural constraints. Build-time generators derive
the Rust discriminators and the client's TypeScript validators from that exact
definition, and producer and reader agree on the SHA-256 of its bytes. Every
record variant is closed (`additionalProperties: false`), `system/init` and
served usage included; served usage has exactly five counts. The real CLI
envelope is the one production format, and the initial
`stream_event/goal_state` is part of it. Unknown or malformed variants are
refused with their bytes retained.

The service and `event_certifier` share one production Rust captured-stream
owner. It owns initialization, identity, scope and tool ancestry, partial
ordering, served accounting, terminal interpretation, the authors of every
request byte, which it holds to the session's prompt record -- the service
reads it from `control/prompt.txt`, and `event_certifier` takes it as its
second argument -- and the turns and snapshot every request owes its
conversation, as the model wrote them. The descriptor reader
supplies exact LF-framed bytes and independently established capture facts.
Native parsing retains exact JSON numbers and refuses duplicate keys and
invalid Unicode before host conversion. After the native pass accepts a
complete stream, the service runs this client's record verifier
(`dist/record-verifier.js`) over the same file, bound to the native pass's byte
count and SHA-256, to replay every generation from its recorded response
bytes; a refusal carries the verifier's stated reason. A certificate describes
conforming captured output, not provider reclamation, configuration
authorization or semantic fidelity of a model result.

Config owns one runtime contract admission. Goal admission precedes
journaling, snapshots and optional renderers, and every JSON adapter passes its
wire records and partial ordering through the same admission. A refused record
latches it; the latch is rechecked after awaited turn and dispatch admission
and immediately before later tool and hook entry. Generated validators consume
authored JavaScript values; they do not recover exact source lexemes a
separate reader already lost. The service's one-shot terminal rule is not the
lifecycle of a persistent human or SDK process.

Every request's decode policy states `strict_tool_calling` and
`exact_token_counting` as constant true. Strict tool calling and exact served
usage are the client's one decode mode: model configuration refuses any other
route, and the OpenAI converter has no lenient branch.

Request evidence is a separate `model_request` record. `system/stream_start`
declares the output window's journal and first sequence, and root results
declare that window's request evidence and usage. The engine prepares replay
and hash validation before committing state; refusal retains the observed
bytes. Evidence records cannot create conversation turns or child scopes. A
full request starts each invocation and committed compaction segment;
subsequent records retain an unchanged message prefix and carry the
replacement suffix and exact envelope. The native reader and the client's
record verifier validate physical order and reconstructed UTF-8 hashes.
Canonical history uses `recordingVersion: 24` and its own structurally
disjoint request record. Full and indexed restoration validate evidence before
projecting history; `output/events.jsonl` is byte-exact stdout evidence.

Raw provider response evidence is a distinct `model_response` record tied to a
physical request. Ordered HTTP, body, end and outcome records retain the exact
received bytes and the separate SDK processing decision, including parser
failures, and declare EOF versus failed or cancelled prefixes. A live turn
checkpoint names its open responses; the service's complete artifact
certifier refuses open or missing completions. Canonical readers validate
physical response prefixes before projecting history. Response records have no
model message and cannot advance a conversation branch. Transport completion
never claims semantic acceptance of a generation attempt.

After a conversation generation's `model_attempt_completion` with disposition
`accepted` or `refused`, the stream shows that generation as upstream-shaped
`assistant` rows, derived from it by the one display projection
(`generationDisplay`, mirrored by the native certifier), which refuses a row
that differs, is missing, is extra or is out of place. An `abandoned` attempt
is evidence only and shows no row. A partial `stream_event` carries no origin
field: it belongs to the latest chat request in its scope. A runtime-authored
answer is a `system/runtime_operation` receipt followed by a text-only
`assistant` row whose `stop_reason` and `usage` are null.

The request bodies are the one record of what the model received. A `user`
row reports that a tool returned a call -- `tool_use_id` and `is_error` -- and
carries no text: not the result, not a notice, not a prompt. Nothing on the
stream is a second copy of model input, so nothing can disagree with a request
and nothing binds one to another; the certifier holds no rule about how the
client composes its messages. It refuses a chat request sent while a call an
accepted generation issued in that scope has no row reporting its return.
`is_error` is the client's claim alone, because nothing the model sees depends
on it; that is a stated limit of the certificate. Upstream's stream-only
65,536-byte cut of textual tool results is not shipped.
