# Child transcript replay refusal

The shared virtual child-session reader must refuse a failed canonical replay
before publishing that page or reporting a successful snapshot. A complete
JSONL record can pass byte and version admission but fail presentation
conversion, such as an assistant record with incomplete served usage counts.
The replay adapter returns a partial page with a diagnostic in this case.
Virtual sessions have no partial-page response contract, so they must turn
that result into their existing read failure.

The failure belongs to the public virtual-session target. Both live refreshes
and subsequent snapshots run through its serialized refresh queue. Subsequent
snapshots use an independent reader to reconstruct the full view; failure in
that reader must close the public target's subscribers as well. The queue
latches the first error before releasing the next read, publishes one
`stream_error`, stops polling, and refuses subsequent reads with the same
error. Replacing the damaged file cannot turn that failed target into success.

This applies to every consumer of virtual child sessions, for ordinary and
long conversations. It does not apply to direct vLLM callers: the backend does
not read or convert Qwen's child transcript files. The error originates in the
shared client reader, which owns the refusal. ACP and workspace transcript
page endpoints expose the adapter's explicit partial result instead.

The resume path reads the canonical `agent-<id>.jsonl` history. The refusal
writes no records, alters no model text, and does not change which records
resume admits or their order.
