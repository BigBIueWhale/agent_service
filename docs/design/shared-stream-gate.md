# Shared captured-stream gate and producer contract

One versioned JSON definition, agent_service's
`protocol/stream-contract-v19.json`, owns every accepted top-level and nested
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
ordering, served accounting and terminal interpretation. The descriptor reader
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
Canonical history uses `recordingVersion: 21` and its own structurally
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

A displayed `tool_result` is exactly the tool message text the model receives
in its next request. The certifier binds every displayed tool result and
notice to the next chat request in its display scope: it refuses a call
issued by an accepted generation whose displayed result does not precede that
request, and a displayed row that request does not carry, carries out of order
or carries with other text. The binding runs one way: the runtime also adds
input it does not display (reminders, the date, the todo list), so a request
message with no displayed row is not refused; the recorded request body is the
authority for everything the model received. `is_error`
is display-only and is not bound, because nothing the model sees depends on
it; that is a stated limit of the certificate. Upstream's stream-only
65,536-byte cut of textual tool results is not shipped: the stream shows what
the model saw.
