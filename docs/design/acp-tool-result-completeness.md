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
results. The text projection module and its otherwise unused JSON byte-slicing
utility are removed together. Preservation tests belong at the delivery
boundaries, not in an identity replacement for the removed projector.

Aggregate replay limits and bounded NDJSON transport admission remain explicit
resource refusals. Replay accounts for the complete serialized updates. A
result beyond an operation's resource budget must fail that operation instead
of reporting successful delivery of a shortened result. This does not promise
unbounded transport capacity or qualify memory use for arbitrarily large
results.

This change affects ACP representation. It does not change the provider request
or canonical chat-history contents, and ACP replay is not model-history
restoration. The vLLM backend neither performs nor owns this ACP projection;
there is no corresponding backend change.

The positive assistant fixtures pass stored-record validation before canonical
resolution and carry explicit observed counts, including reported zero. Adopted
assistant text has no local generation or usage claim. Ordinary accepted
assistant generations require usable observed usage; a resolver alone does not
prove acceptance. Malformed nonnull display usage still refuses projection.

The executed source baseline reproduced nine preservation/accounting failures
and passed four controls. The live fixture first required migration to an
explicit root conversation scope; that preliminary setup failure is separate
from the reproduced truncation. The complete replay suite then exposed
assistant fixtures without generation envelopes. The first fixture revision
used envelopes with null usage and led to an unjustified null-handling change.
Review caught that these fixtures could not pass accepted-record admission;
that production change was removed and the fixtures now use admission-valid
records. The intermediate passing run is not final evidence.

Final source runs passed 11 selected live/plan cases, with 616 unrelated cases
filtered, and all 39 replay-page cases. The tests exercise the actual Session
delivery method with a mocked client and
the actual HistoryReplayer with authored records. They do not establish that
those records passed complete physical-request admission or that model-history
restoration is identical. The existing bounded NDJSON source suite passed 33
cases; this is transport implementation evidence, not a live deployment test.
Logs are `/tmp/codex-acp-completeness-baseline.log`,
`/tmp/codex-acp-completeness-live-final.log`,
`/tmp/codex-acp-completeness-replay-admitted.log`, and
`/tmp/codex-acp-completeness-transport.log`.

Build, typecheck, native execution, and live application qualification are
prohibited in this session and remain unverified pending owner gates. The
standing goal and the integrated change's full review remain incomplete.
