# Canonical chat recording

The client's runtime chat JSONL is the record a resume, fork, export or
catalog reads. It is not the service's `output/events.jsonl`, which is stdout
evidence under the separate stream contract and never serves as history.

## Version

Every physical canonical record carries `recordingVersion: 23`
(`CHAT_RECORDING_VERSION`), independently of the client release string. A
record with any other version, or none, is refused with the file location,
what the record declares (a version, or no version), and the action: open it
with the client release that wrote it, or start a new session. The file is left
unchanged. Older recordings cannot establish the runtime state this version
restores, so they are never promoted into version-21 histories. Unknown record
kinds or subtypes, malformed JSON, invalid UTF-8 and unterminated records are
refused on every branch, inactive ones included.

## Scans over many recordings

Session listing and counts, title and title-prefix lookup, resume by title,
usage history and its dashboards, insight reports, review transcripts and the
IDE session reader keep every readable result and report each unreadable file
with its path, physical location when known, and cause; one unreadable file
never takes a listing down, and an aggregate that excludes one says so where
it is shown. Directory errors fail the request, and a hard scan limit is
reported as an incomplete result. Automatic latest-session selection refuses
while a possibly newer recording is unreadable, including a tied or
unknown-order one; explicit selection of a readable session stays available.

## What resume restores

Resume replays explicit history changes: `runtime_history` checkpoints and
splices, positioned accepted assistant commits, and compaction checkpoints.
Display records and request/response evidence are never inputs to the typed
history accumulator. Checkpoints carry the image payload store and the saved
startup context, and ordinary and indexed readers preserve exact `Content`
boundaries. A compaction's committed history is stored once, as the
checkpoint's `compressedHistory`; a canonical compaction record that also
carries `postCompactionHistory` is refused, because a second copy could only
disagree with the first. (The stream's compaction record carries
`postCompactionHistory` as evidence.) A failed compaction asserts that
history and image payloads stayed exactly the same.

Canonical storage keeps complete model arguments, including values upstream
redacts: structured-output payloads and approved plan arguments stay whole on
disk through tool completion, resume, indexed restore and fork. Operational
telemetry can still omit a structured answer payload; that projection is never
a conversation turn.

## Closed and live readers

A live canonical file ends at a prefix, not a closure. Full load, indexed
restore, background recovery, fork, exports, session references and archived
views require closed physical and logical evidence before history becomes
runnable or is presented as complete; a session whose writer is inside a model
attempt is refused with its path and a next action until the attempt closes.
`readSessionView` is the explicit live-prefix reader for in-progress task and
startup-context views. The indexed page reader returns open evidence with the
page, so an active page is marked provisional and an idle page at the tail
refuses open evidence. Resume activation checks, inside the writer lease, that
the bytes it admitted are the bytes the lease holds (same SHA-256 and length)
before installing history.

## Render assertions and child scope

Each chat request is bound to the history it was rendered from. Immediately
after rendering, the recorder writes a `model_history_assertion` holding the
effective image thresholds, selected user index, route modalities and a
fingerprint of the rendered `Content[]`; the writer replays curation, image
projection and media slimming from canonical state and compares before
admitting it. Every attempt then writes a `model_history_binding` to that
assertion before dispatch, and the chat checks the fingerprint before calling
the generation client and around provider request construction. A complete
reader repeats the replay and requires the binding before each physical chat
request.

Every child attach, resume included, writes a `model_child_scope` row with the
child's `agentId` and `kvScope`, mirrored durably into the root before the
child proceeds. Complete child admission requires the matching first row,
compares every local declaration, assertion and binding with its root mirror
and back, and checks all root renders, bindings, physical requests and
compaction draws in that scope, so deleting a child's whole attempt still
leaves a root record with no child counterpart.

## Writer admission

Records that can carry model input for a later generation (tool results, goal
continuations, cron prompts, notifications, mid-turn input) and state a resume
restores (rewind branches, file-history and attribution snapshots, command
metadata) go through one recorder-owned admission: the writer must be active,
the complete record is validated and cloned inside the failure owner, and any
synchronous or queued failure latches the recorder so `flush` and `close`
refuse. A rejected record can therefore never be followed by a later commit
that skips it. Snapshot payloads are validated against the shape the writer
serializes on every branch.

## Stated limits

- The final provider request body is the authority for dispatched input. The
  render assertion proves the rendered `Content[]`; provider conversion and
  the provider's own request hook (which can replace `messages` through
  `extra_body`) are not replayed from the record.
- A per-file reader cannot discover a child recording it was never given; the
  root journal mirrors child requests and output, but a missing child file is
  not by itself a refusal.
- Prompt hooks and ACP `executeGeneration` call the generator without
  `retryWithBackoff`, so their failures follow no shared retry decision.
- The indexed transcript reader refuses a snapshot above 256 MiB, upstream's
  own `SESSION_TRANSCRIPT_MAX_INDEX_BYTES`. The service reads each captured
  stream record under a 128 MiB bound on raw JSON bytes, declared in
  `src/result_parse.rs` as a policy magnitude, not derived; a longer record
  refuses certification. Both are refusals, never truncation.
- Desktop's suites run under Bun (`bun:test`); the image's test selection
  excludes `packages/desktop`, and no gate runs them.
