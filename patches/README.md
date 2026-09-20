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
- Review-diff SHA-256: `e4dd9c97f0acc6afadb54d1d079ba8ee6329817d70d15a4995fe26b79a18c621`
- Semantic transformer: `source_patch_v1/`
- Transformer-manifest SHA-256: `4dccb9c76e8de5fe10b30bb22221ff300fdba8e9d7d35df7b5cddd7f2e7bdac4`
- Official npm package: `@qwen-code/qwen-code@0.21.12`, which this build does not fetch; it builds the commit archive above
- Pinned Node build/runtime image (linux/amd64 manifest): `node@sha256:d649c27dae7ba0137b3cef5dd75baa422c08dc3d9e3fc0c23dfb172dc3cc6436`

The transformer validates the pinned source, the reviewed diff, exact final file
identities, and 34 semantic concerns before changing the private source tree.
Removed files have an explicit absent final identity. Applying the same result
again verifies it without writing. A failed commit restores the original bytes,
permissions, and file presence. The image derives its unit test selection from the
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

A turn the model ended itself with no tool call is its final answer only when
its visible text is one. A message with no visible text, or one carrying the
served template's tool-call markup (`<tool_call>`, `</tool_call>`, `<function=`,
`</function>`, `<parameter=`, `</parameter>`) outside a structured call, is a
slip: it stays in history exactly as produced, is answered with a user-role
notice that says nothing was executed, and is followed by the next turn, charged
like any other. The third consecutive slip is the `SLIPPED_FINAL_MESSAGE` state,
`error_slipped_final_message` on the wire with exit code 1, in the headless
session and every subagent alike, decided after the incomplete-generation check
and never in its place. One core module (`describeFinalMessageSlip`,
`finalMessageSlipNotice`, `FINAL_MESSAGE_SLIP_LIMIT`) decides it for both
reasoning loops by an exact string test on the turn's visible text; no judgment
about task completion is made, and the removed next-speaker check does not
return. The notice reaches the stream as a user record, the session recording as
a mid-turn user record, and a subagent's transcript under its own input kind; the
subagent's terminal event carries the shape of the slip and the number of
notices, and its parent is told the assignment is unfinished.

## Context and instructions

The context partition holds back 48/256 of the window as the generation
reserve and 2/256 for the compaction directive; the remainder is the trigger.
At a 262,144-token window these are 49,152, 2,048 and 210,944 tokens.
Independent floors leave rounding to the trigger. One reserve is the whole of
what a generation is given: it is the output limit of every turn, whatever its
prompt, the room a compaction's snapshot is issued with, and the bound on the
tool results one turn appends inline. What survives a compaction is the
snapshot, the turn carried behind it and the next turn's results, so the
partition refuses any window whose trigger is not above three reserves; at the
served window that is 147,456 against 210,944. The route refuses a configured
ceiling, including `QWEN_CODE_MAX_OUTPUT_TOKENS`.
Input, directive, displaced results, and candidate histories are counted using
the actual rendered request.

A tool result says whether it is complete. One notice states what was asked
for, what came back, the bound and its unit, the real total or why the tool
cannot establish it, and the exact call that continues; tools do not phrase
their own, and a tool under `src/tools` that cuts a result to a named cap
without going through `boundedContent` refuses the build. A bound that
returned nothing says so in those same words rather than claiming a cut, and
every notice names the constraint that actually applied rather than the
largest one in sight. A count that says lines means lines: the split-segment
count every range calculation uses is one higher whenever a file ends with a
newline, and only the sentence subtracts it. A cap a service
applied travels back with its items rather than being discarded, because a
layer cannot declare a limit it was never told about.

A result whose size is decided outside this process — a fetched page, an MCP
server's reply — is held to one shared budget, `MAX_TOOL_RESULT_BYTES`, before
it enters the conversation; the complete text is retained as a session artifact
and the notice names the exact call that reads it back. That a bound exists is
required: the window is guarded in exact tokens by the compaction trigger,
which refuses a request that no longer fits, and this budget is what keeps that
refusal unreachable in ordinary work. Its magnitude is a policy choice recorded
with the measurement it sits above, not a derivation, and bytes never stand in
for tokens — nothing converts between them.

A failed call's model-facing text is held to that same budget. A failure
message is the one place a tool writes model-supplied input back out — the path
it could not open, the pattern it could not compile, the tool name it did not
recognise — and an argument that arrived merged or malformed makes that copy as
large as the argument. The bound is applied where the model's copy is made
rather than in each tool, so a tool cannot opt out of it and a new one inherits
it. No continuation is named: the bytes past the cut are the tool's own prose
plus a second copy of what the model just sent, so a file holding them would
cost the window twice and return nothing the sender lacks. `error.message` is
left whole on purpose, because the scrollback, the `PostToolUseFailure` hook and
the sanitized telemetry span read it and want the operational summary in full.

Compaction summarises the prompt the last turn was issued against and carries
that turn, reasoning included, verbatim behind the snapshot, so the summary
request never holds what the turn generated. It requests the room left by that
exact input, which is never below the generation reserve, and refuses
insufficient space. A candidate must end normally, carry the required six-part
snapshot, reduce the request, and leave an issuable turn. Failure retains the
previous history and reports that retained count. There is no separate
reasoning-phase limit or forced reasoning-end marker. Compaction invalidates
what the model can quote, not what it has seen: it disarms every file-read
entry's history residency and leaves the read and write evidence in place, so a
file this session wrote itself is still one it is allowed to overwrite.

The snapshot is declared, not described. The compaction request replaces the
turn's tools with one function whose closed parameter schema is the six ordered
sections, and forces that call, so presence, uniqueness and ordering are
properties of the declaration rather than of markup the model types. Section
text travels as string arguments, so prose that collides with markup carries
through unchanged and nothing has to be escaped. `acceptStateSnapshot` judges
one drawn candidate against the same declaration, because a client cannot
observe whether the engine applied a constraint it declared. Swapping the tool
block does not reuse the turn's prompt-cache prefix; the conversation it carries
is otherwise unchanged.

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

## Recorder ownership and image qualification

Config constructs one required session recorder. Session initialization acquires
its writer before creating chat; every interactive, headless and ACP generation
therefore meets the same recording obligation. Workspace replay and MCP discovery
use explicit service initialization without acquiring a session writer or opening
chat. Hidden memory operations keep their bootstrap owner; UI telemetry suppression
does not suppress canonical request evidence. Source settings, CLI arguments,
SDK reservations and daemon feature negotiation expose that single contract.

The final agent image stage runs its installed CLI as UID/GID 1000 through a private
protocol fixture. The smoke consumes the same fixed Rust launcher argument vector,
substitutes only provider endpoints in shipping settings, and requires a fresh file
nonce to pass through read_file and the following response. Successful exit, nonempty
structured events, served fixture usage and the owned canonical transcript are all
required. Every emitted record passes through the production Rust reader and captured-stream
certifier, including the initial idle goal state. Producer structural admission and native
certification derive their accepted variants from the same versioned wire definition.
Empty and failing processes are negative controls; communication failure,
timeout and interruption controls prove child termination, join and pipe cleanup.
No program or settings installation follows the gate.

The build also qualifies the final images together through the launcher, relay, trusted
capture and service publication. Owned protocol fixtures exercise a complete tool cycle,
cancellation, incomplete provider output, capture failures and durable terminal publication
faults. These observations establish the tested local composition; they do not establish
provider settlement, compaction recovery or semantic preservation.

## Verification scope and remaining limits

CPU-only native package builds and regression tests qualify the browser SDK,
shared compaction formatter, ACP result recording and delivery, and affected
persistence and presentation boundaries. Python transformer and manifest checks
verify exact source reconstruction; Cargo checks cover the harness. These checks
are separate from live provider or session verification. No model serving,
provider session, release or GPU workload is needed for this qualification.

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

Review worktree lifetime and cleanup were deliberately excluded because this
project does not use fetch-pr or PR worktrees. Their existing gaps remain: per-PR
leases can be overwritten;
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
it. Fresh CPU-only tests reproduce all ten ring defects. Source review also
confirms that UI retention drops closed-session observations, nested observation
objects remain mutable, and optional telemetry catches can suppress admission
failures. A whole repair needs source-owned observation retention, acknowledged
channel publication and explicit resource measurement states. CPU qualification
is feasible; the unimplemented coordinated protocol is the remaining limit.

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
admission does not establish that lifetime contract. CPU-only tests using the
installed OpenAI SDK and in-memory SSE/ReadableStream sources reproduce hidden
cancellation failures, unstarted-generator leaks and unjoined pending reads.
The whole scoped-consumption migration remains unimplemented; it does not require
live model access to qualify.

## Manual compaction ownership and presentation

Both commands return a completed compaction observation through one result type,
without consumer branches or UI side effects. The terminal executor awaits action,
presentation and recording settlement. Cancellation retains that owner and blocks
future work; a completed fact remains visible. Confirmation continuations keep the
same owner. Queued input and external rewind admission wait for command settlement.
ACP retains its prompt owner and attempts both durable result recording and result
delivery, preserving independent failures. Headless output emits the existing
session-level compaction event even when its assistant adapter was finalized by a
previous turn.

One formatter handles all named statuses and rejects absent, non-finite, unsafe or
negative counts. A reduced candidate with insufficient request room is distinct
from a candidate that did not shrink. The terminal retains a compression marker
for history mapping, and its shared text projection survives browser replay.
Successful counts describe the committed checkpoint; restored startup context is
an independent subsequent change.

Checkpoint recording and flush precede live installation. An independent failure
after durable acceptance preserves the completed info and its cause and blocks
further chat access until restoration with a new client. Later admission refusal
never repeats the earlier completion. Synchronous command-record admission
failure enters the recorder's existing persistent failure state and is observed
by flush, preserving completed UI actions and the original recording failure.
User and hook directives are supplied whole to exact sizing; no reasoning budget,
output budget, reserve or payload retention rule changes.

The compaction factory satisfies its shared history type while preserving the
inferred object shape required by canonical recording. Native CLI compilation,
all formatter cases and held ACP recording/delivery controls qualify this seam.
These checks do not establish general extended-session correctness.
