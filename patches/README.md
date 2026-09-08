# Pinned upstream source transformation

This directory is part of the reproducible build input. Changes are applied only
to the exact upstream revision recorded here. The build fails on source drift,
ambiguous landmarks, intermediate patch states, output drift, or partial writes.

## Qwen Code 0.21.12

- Repository: `https://github.com/QwenLM/qwen-code`
- Tag: `v0.21.12`
- Commit: `b965d5f8c24f48e65fb0b17c7d45f34ca4ce8f38`
- Commit archive: `https://codeload.github.com/QwenLM/qwen-code/tar.gz/b965d5f8c24f48e65fb0b17c7d45f34ca4ce8f38`
- Commit archive SHA-256: `61beddff8bde1dd2654c8714f927b46ab7cf9822b8561d11e3a2b8e085b5e745`
- Patch: `qwen-code-0.21.12-agent-service.patch`
- Review-diff SHA-256: `a04ad4ca743713a4e6bd5636b070f0d67c2821d2314ec920750a579a59279e33`
- Semantic transformer: `source_patch_v1/`
- Transformer-manifest SHA-256: `e622550f06e5242e44dc31fa2f5ce26449f4b78dab2b293540e6582db2763753`
- Official npm package: `@qwen-code/qwen-code@0.21.12`, which this build does not fetch; it builds the commit archive above
- Pinned Node build/runtime image (linux/amd64 manifest): `node@sha256:d649c27dae7ba0137b3cef5dd75baa422c08dc3d9e3fc0c23dfb172dc3cc6436`

The transformer validates the pinned source, the reviewed diff, exact final file
identities, and 28 semantic concerns before changing the private source tree.
Removed files have an explicit absent final identity. Applying the same result
again verifies it without writing. A failed commit restores the original bytes,
permissions, and file presence. The image derives its test selection from the
retained final files plus their existing adjacent tests.

## Generation and ownership

Every generation uses the supported OpenAI-compatible vLLM contract: an explicit
positive context window, exact rendered-request tokenization, and strict native
tool-call commitment. Model selection is transactional. A requested route must
resolve and validate before it becomes active; failed preparation restores the
previous selection.

A required `GenerationContext` owns every chat and side request. The main scope
is the conversation's session ID. A tool-launched child owns its spawning call
ID; workflow and utility invocations own distinct IDs. Child launch hooks,
resumption, and alternate model selection preserve that identity. The common
provider seam writes `kv_scope` after request decoration, including batch,
stream, JSON side queries, and exact counting. Count requests and generation
retain the same model, template, tool, and ownership context.

Structured calls become executable only after the full response and its
canonical assistant record are accepted. Normal completion requires an explicit
provider terminal. A fresh bounded retry is permitted only before answer content
has been delivered. Text, whitespace, and literal protocol-like XML remain
verbatim. Transport failures, invalid usage, malformed calls, and post-terminal
content retain diagnostic text without publishing executable calls or a normal
terminal. Empty normally completed output is a valid response.

## Context and instructions

The context partition reserves 48/256 for a summary, 32/256 for ordinary output,
16/256 for pending tool results, and 2/256 for the compaction directive; the
remainder is the trigger. At a 262,144-token window these shares are 49,152,
32,768, 16,384, 2,048, and 161,792 tokens. Independent floors leave rounding to
the trigger. Input, directive, displaced results, and candidate histories are
counted using the actual rendered request.

Compaction requests the room left by its exact input and refuses insufficient
space. A candidate must end normally, contain the required six-part snapshot,
reduce the request, and leave an issuable turn. Failure retains the previous
history and reports that retained count. There is no separate reasoning-phase
limit or forced reasoning-end marker.

Authored instructions and corrections have explicit provenance captured before
hooks or input transformation. Their original parts survive repeated compaction,
forks, speculative adoption, and resume independently of model summaries.
Synthetic user-role messages do not acquire authored provenance. Strict framed
UTF-8 transcript readers refuse corruption, conflicting record identities, and
incomplete active histories rather than silently dropping retained instructions.

## Durable session evidence

Every initialized recording session owns the same exclusive writer lease.
Terminal, headless, and daemon callers meet this obligation through Config.
The persistent storage anchor remains fixed while execution directories can
change. Session replacement closes the outgoing writer, acquires and restores
the incoming canonical state, and only then publishes the new owner. Failed
replacement restores the prior owner; failed restoration refuses admission.

Every chat requires a canonical commit recorder. Root chats, ordinary children,
background and resumed children, workflow calls, utility forks, and speculation
use the existing root or child transcript writer. Bootstrap history is complete.
Accepted speculative model history is recorded before admission to live history.
Failed generation records retain observed parts, the last valid served usage,
and the actual terminal or its absence. Display events do not stand in for a
durable commit.

Session rename, fork, deletion, archive, and restoration use the shared mutation
lease. Active-source forks wait behind the recorder's write barrier. Target
failure does not poison a healthy source owner. Usage retained for deleted
conversations is synced before deletion. Strict history loading reads retained
facts across all ages and does not depend on the renderer consuming them.

Generation dispatch and final observation are distinct durable events sharing a
request identity. A dispatch without a final observation is unfinalized; a final
observation without served usage is separately unreported. An unused stream,
failed provider, cancelled request, or process interruption cannot be presented
as a reported zero. The same observation accumulator supplies live telemetry,
retained history, exports, child accounting, workflow budgets, and terminal
presentations.

A served usage report contains five safe nonnegative integer counts: prompt,
output, reasoning, cached prompt, and total. Total equals prompt plus output;
reasoning is included in output and cached prompt is included in prompt. A
complete all-zero report is valid. An absent report is not that value. Wire and
SDK declarations preserve the full report, observed aggregates, missing and
unfinalized counts, and unavailable child durations.

Child rounds and compactions retain their owning scope, including failures and
cancellation. Child status is bounded; detail and transcript views expose the
recorded evidence. Execution rounds, API requests, and reported usage remain
different facts. A workflow watchdog cancels stalled execution without replaying
the delegated task; its cleanup and accounting settle before completion. Child totals never become a parent's provider-response usage.

## Artifact retention

Oversized text results and downloaded binary payloads share one immutable,
content-addressed session artifact store. Exclusive creation prevents collisions;
deduplication verifies existing bytes. Payloads and directory ancestry are synced.
The 500 MiB storage quota counts actual retained files under the store's writer
lock, including interrupted-write remnants. Recreating Config does not reset it.

Artifacts are retained by ownership and references. There is no age-based
collection. Forks and exports may refer to an artifact after its original chat
is deleted. A lock is not stolen based on elapsed time; an abandoned lock
requires explicit administrative recovery. Persistence, integrity, quota, and
cleanup failures retain their causes and refuse the operation.

Fetched and decoded tool content is retained whole before displacement. Range
bounds are explicit query facts. Text paging uses UTF-8 bytes and returns the
next line offset without consuming a line it did not return. Text decoding follows
a Unicode BOM or UTF-8 when absent; large streaming reads require UTF-8. Invalid
encoding, unreadable oversized lines, and unsupported media fail explicitly.

## Tool and deployment contracts

Native tool schemas are closed. External tool schemas retain their actual JSON
Schema semantics, including open objects, pattern properties, and reject-all
schemas. Trusted editor modifications live outside model JSON and survive the
scheduler's clone by explicit provenance. Lookalike model parameters cannot
acquire editor authority.

`read_file` requires an offset; zero means the beginning. Offset and limit select
text lines and do not silently apply to nontext media. PDFs use text extraction,
with explicit remedies for unsupported or overlarge input. Raster images must
pass complete original-PNG validation: static 8-bit RGB/RGBA, at most 16,777,216
pixels, 30:1 aspect ratio, and 100 MiB. The original image is not resized or
transcoded. Tool text and image parts preserve chronological order.

The sealed deployment has one tool allowlist and an explicit foreground child
policy. It admits only the advertised general-purpose and Explore variants and
refuses unsupported invocation parameters. Immutable distributor instructions,
per-invocation scratch ownership, and effect journaling govern this deployment.
Generic host prompts describe the workspace and permissions actually provided;
a report does not prove attribution of concurrent effects. The deployment frame
names its timestamp as the CLI invocation time and does not promise indefinite
cache residency.

Locked initialization and later authentication use the sealed settings boundary:
no workspace environment injection, permission-rule persistence, ambient MCP,
managed memory, custom workflows, or injected system-prompt override. Ordinary
project instructions remain available. Leading slash task text is literal in
that configured surface. This deployment policy is distinct from the generation,
accounting, and persistence obligations shared by every renderer.

## Output lifetime and partial evidence

The finite terminal-state table maps each run ending to its wire subtype, error
classification, and exit code. Started-turn counters have one owner and survive
exceptions. A child ending names the spawning call ID; only a null-scoped result
ends the main run.

Output adapters share a writer per stream and await actual write callbacks.
Result acknowledgement follows output settlement. The shared `withCleanup` and
`runCleanupSteps` preserve independent operation and cleanup failures while
awaiting their owners. Shutdown joins admitted
cleanup work, recording, bridge closure, and stdout/stderr settlement while
preserving independent failures reported by those operations. Queued turn work owns whether its result was
already delivered; subsequent cleanup failure cannot create another result.
Fatal worker failure cancels input admission even while its input pipe is open.

Captured JSONL is a sequence of LF-terminated records. Orderly process exit waits
for output delivery; a crash, SIGKILL, or failed transport can still leave a torn
last record. One descriptor-anchored scan retains valid owned observations and
counts malformed or incomplete records as unaccounted. These observations are
independent of complete terminal certification. A torn tail, invalid chronology,
or absent ending refuses certification without erasing earlier observed usage.
Unreadable evidence is represented as unavailable, not a fabricated zero group.

## Verification scope and remaining limits

This round uses local source tests with mocked generation boundaries, test-owned
files and streams, noEmit type checks, Python transformer tests, and Cargo checks.
It does not build or release the application or launch a provider, CLI session,
benchmark, or service. The production image will run its own build and derived
test selection after a separate deployment action.

Compaction reasoning and child reasoning still appear inline in recorded
content. Persisting these by reference remains open: a complete change needs a
stored-content representation, durable encoding through both canonical writers,
strict reconstruction for model history and exports, and reference-aware daemon
and SDK consumers. Current retention, output settlement, and partial observation
handling do not close this limit, and no reasoning is truncated to hide it.

Speculation acceptance is not atomic across workspace files and conversation
history. The overlay applies files before adopted messages are durably appended;
an append failure can leave applied files and a partial accepted history. Reported
cleanup failures retain their causes and the terminal reports acceptance failure without
resubmitting the prompt. A complete guarantee requires coordinated ownership and
recovery for both resources, including the overlay's per-file failure handling.

Review worktree lifetime and cleanup remain open. Per-PR leases can be overwritten;
creation removes fixed trees and branches without establishing that a prior owner
has ended. Separate cleanup implementations suppress errors and can force
filesystem removal after Git refusal. A correct lifecycle must establish exclusive
invocation ownership across review, probe/base trees, branches and associated
files, and use it in creation and both cleanup entry points. Shared shutdown
settlement cannot recover errors suppressed by those inner operations.

Operational daemon metrics remain incorrect. The ring admits malformed or partial
token observations, permits unsafe addition, and exposes mutable sampled buckets.
The bridge suppresses accounting callback failures and fills missing counts with
zero; the sampler also fills failed or absent readings with zero. A complete fix
must coordinate observation admission, per-frame mutation, measurement failure
and staleness, checked interval arithmetic, and immutable publication. Sampled
operational readings are distinct from complete session generation accounting.
That whole lifecycle remains open; the session summary corrections do not close
it. Ten pure actual-source failures are retained outside the shipping tests; the
bridge and sampler findings are source-confirmed, without runtime qualification.

Prompt-hook failure policy remains open. Invalid model JSON/schema and operational
failures currently allow continuation. Cancellation is inferred from message
text, and the enclosing hook event bus reports failure without a blocking output
for most event kinds. A correct policy must distinguish failed evaluation from a
model decision and respect whether an event occurs before admission or after an
irreversible action. The shared generation owner does not repair that policy.
These paths are source-confirmed; existing fixtures preserve the current behavior,
not a qualification of fail-closed operation.

Resident background AgentTool disposal still exposes a void callback, and its
unexpected asynchronous failures are logged without an owning caller joining
them. Writer attachment is shared by all children, foreground forks are awaited,
and explicit background forks use the same lifetime for every renderer. Ordinary
child resource settlement and InProcessBackend settlement are awaited. The
remaining resident-background disposal contract needs coordinated changes to the
registry, continuation, caller shutdown, and failure publication; it is not closed
by those narrower lifetime fixes.

Provider-stream lifetime and failure settlement remain open. The pipeline acquires
an SDK stream before returning lazy generators, so returning before their first
iteration can bypass their cleanup. The guarded iterator suppresses every
`return()` failure, and automatic iterator closing after a conversion failure
can hide an independent cleanup failure. Guards disabled and guards enabled also
follow different iterator ownership paths. A complete fix must own the stream
from acquisition through completion, abandonment, cancellation, and pending-read
settlement, preserving primary and cleanup failures. Strict configuration
admission does not establish that lifetime contract. These are source-confirmed
findings; no live stream or cancellation experiment was run for this assessment.

Manual compaction remains open as a complete command-lifetime and outcome
contract. Terminal cancellation races the action, releases the processing state,
and can admit a successor while compaction still mutates history. Completed
results can then be discarded. Other command renderers can label failed attempts
as successful, omit named failure statuses, or substitute zero for absent counts.
The inflated-candidate status also represents a reduced candidate that still
exceeds the admission trigger; these are distinct rejection causes. Command and
hook instruction clipping precedes an exact shared directive-size check and can
lose authored input. Fixing this requires joined command ownership, cancellation
settlement, truthful outcome projection, and exact instruction admission across
renderers. Returning the observed Core compaction result does not establish that
whole contract; no text-only or cap-only correction is counted as closure.
