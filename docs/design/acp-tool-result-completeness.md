# Complete ACP tool-result delivery

ACP session notifications carry the complete tool result supplied by the
emitter. Live delivery and transcript replay preserve every content block, its
order, the separate `rawOutput` value, and metadata. Delivery has no text
preview or per-field byte budget. A JSON round trip must recover the same
values, including escaped characters and Unicode.

`Session.sendUpdate` owns live delivery. The replay context in
`history-replay-page.ts` owns bulk and paged transcript delivery; it attaches
the record identity without replacing result fields. These shared boundaries
serve every ACP caller, including ordinary sessions, child tools, and large
results. No text projection or JSON byte-slicing step sits on these paths, and
preservation is tested at the delivery boundaries.

Aggregate replay limits and bounded NDJSON transport admission are explicit
resource refusals. Replay accounts for the complete serialized updates. A
result beyond an operation's resource budget must fail that operation instead
of reporting successful delivery of a shortened result. This does not promise
unbounded transport capacity or qualify memory use for arbitrarily large
results.

This is an ACP representation rule. It does not touch the provider request or
canonical chat-history contents, and ACP replay is not model-history
restoration. The vLLM backend neither performs nor owns this ACP delivery, so
it has no backend counterpart.
