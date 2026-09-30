# The producing attempt through child recording and presentation

A child's retried or cancelled draw must stay distinguishable from its
accepted turn everywhere the child's output is recorded or shown. The client
decides whether a draw entered history; the backend cannot know, so this
belongs to the shared client recorders and presentation layers.

## Origin and canonical records

`ChatAttempt` owns the immutable model origin `{kind: "model", kv_scope,
attempt_id}`. `GeminiChat` passes that exact origin to the accepted assistant
commit and to every other generation it records; adopted and realtime records
do not invent a chat attempt. Admission refuses malformed origins and
duplicate generation origins or identities, and each generation is one
complete atomic commit. Resume reads the canonical recording under the
runtime directory, never the live sidecar or the service's stdout copy;
generations other than accepted ones are evidence and cannot become history.

## The live sidecar

`AgentCore` emits each nonempty text or thought fragment with its origin, run
ID, round and attempt to `agent-<id>.jsonl.stream`, a transient file with a
strict version-2 record of exactly those fields. The writer creates it
exclusively without following symlinks, completes short writes, syncs the file
and any newly created directories, and latches timer, write and close
failures. Batching bounds I/O granularity, not output length. Live fragments
are flushed before the matching canonical commit, and the writer removes only
its own file, only after every observed attempt has a durable disposition;
failure or unresolved output keeps the evidence and refuses cleanup.

The virtual child reader captures canonical bounds before and after the
sidecar and retries until identity, size and terminal status are stable
across that capture. It keeps one live descriptor between refreshes, so an
unlinked inode cannot be reused before its evidence is drained. Retirement
requires every observed origin to have its canonical commit, and a partially
retired record refuses. Pending fragments are matched by exact origin and must
be literal prefixes of the final canonical text or thought; only missing
suffixes are emitted.

## Publication and presentation

The direct ACP tracker owns one serialized publication queue; listener setup
rolls back partially installed listeners, and publication, decoding and
cleanup failures reach the prompt owner instead of becoming successful tool
results. `AgentCore` publishes served usage per observed attempt, including
cancelled and abandoned ones, with its origin and disposition.

ACP replay marks parts with versioned `qwenGenerationAttempt` metadata. The
shared reducer applies monotonic disposition checks across usage, thought and
answer, and refuses contradictory or post-terminal output. Transcript
compaction merges only equal origins and dispositions. Labels live outside the
verbatim model text; shared SDK daemon views, CLI exports and browser
components keep the attribution and show abandoned output separately.

## Stated limits

Standalone Desktop and the VS Code legacy aggregation paths discard
attempt metadata; the service image does not build or ship them. Channel
bridges omit child message chunks, and the daemon's `thoughtChunk` event
carries no attribution, so a direct subscriber to it cannot tell attempts
apart.
