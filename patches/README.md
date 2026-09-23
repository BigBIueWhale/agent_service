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
- Review-diff SHA-256: `304b373a15d08bce02946f5e86a9c3b5d404cdfc97b769e772d18baba9f1f93a`
- Semantic transformer: `source_patch_v1/`
- Transformer-manifest SHA-256: `47c51ca544778164f4f0a3a33a63b0b30ce4998e40ef785778818d5a48eb0fe8`
- Official npm package: `@qwen-code/qwen-code@0.21.12`, which this build does not fetch; it builds the commit archive above
- Pinned Node build/runtime image (linux/amd64 manifest): `node@sha256:d649c27dae7ba0137b3cef5dd75baa422c08dc3d9e3fc0c23dfb172dc3cc6436`

The transformer validates the pinned source, the reviewed diff, exact final file
identities, and 35 semantic concerns before changing the private source tree.
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
terminal. Empty normally completed output is a valid response. A call a length
terminal stopped is never made, and what the provider served of it is the
model's output: it is carried, as the text it was served as, to every record of
the generation -- the stream's `incomplete_tool_use` block, the assistant
transcript record beside the message history replays, a subagent's round, and
a compaction draw's accounting -- and never becomes a function call anywhere.

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

The context partition spends the window exactly, from three declared quantities.
`D`, the static preamble, is 3W/64 — 12,288 tokens at 262,144 — a declared
capacity rather than a derivation: the system prompt and the tool declarations
are texts this repo ships, so the turn preamble is counted exactly against it by
the served tokenizer before the first turn, and again before any later turn
whose preamble has changed, with the Git snapshot's repository values bounded by
their byte caps rather than counted, and so is the startup context that opens
every history, its environment lines and folder listing bounded the same way,
and the turn budget the `## Context` section states, its number bounded by the
sixteen digits a safe integer renders to; a preamble that does not fit is a
startup refusal naming shorten-or-deploy-larger;
each compaction's preflight holds what its request adds -- the snapshot's
declaration and the directive -- to the same share. The
startup context is kept whole at the head of every history a compaction builds
rather than rebuilt after it, so it is in the candidate the compaction counts
and cannot go missing, and the proof counts the frame every compacted history
holds around its blocks -- the resume trailer, the acknowledgement turn and a
retained input's header -- with it, the blocks left out, and one todo reminder
with its list bounded by bytes. `M`, one inline block, is W/8 bytes — 32,768 — the one
declared magnitude and openly a policy: it is the most any single block placed
inline may be, and anything larger is kept whole in a file and paged back rather
than shortened. `F`, the per-message framing, is the 61 bytes the served
template wraps around one message at its widest, declared here and verified
against the template rather than copied from it.

`C`, the room every generation is issued with, and `T`, the compaction
trigger, follow. What stands in the window after a compaction is the preamble
plus `snapshot + authored input + carried turn + one result`; every term but
the turn is bounded in bytes before it exists, and a text's tokens are at most
the UTF-8 bytes of its NFC form, the form the served tokenizer splits, so `C`
is the largest room a turn may be given while `D + 3(M + F) + (C + F)` still
fits below the trigger, and `T = W - C - D`. At the served window that is
69,509 and 180,347. Both are
asserted, not assumed: the fit, the no-overrun property and the compaction
room are checked at every window the partition can be given, and a window too
small to leave a turn any room is refused rather than partitioned. `T` does
not depend on `D`, which cancels, so the preamble trades against turn room and
never against the trigger. One number is the whole of what a generation is
given: the output limit of every turn whatever its prompt, and the room a
compaction's snapshot is issued with. The route refuses a configured ceiling,
including `QWEN_CODE_MAX_OUTPUT_TOKENS`. Input, directive and candidate
histories are counted using the actual rendered request.

A tool result says whether it is complete. One notice states what was asked
for, what came back, the bound and its unit, the real total or why the tool
cannot establish it, and the exact call that continues; tools do not phrase
their own, and a tool under `src/tools` that cuts a result to a named cap
without going through `boundedContent` refuses the build. A bound that
returned nothing says so in those same words rather than claiming a cut, and
every notice names the constraint that actually applied rather than the
largest one in sight. A `read_file` page is the one exception, because
nothing cut it: a page that met the caller's own limit was not cut, and one
that ended where its bytes ran out is what the tool description promises. It
is led by upstream's own range statement, "Showing lines X-Y of N total
lines.", then either "The file continues past line Y: R lines remain." with
the exact call that reads on -- the next line, and the caller's own limit
rather than the size of the page that fit, which would shrink every page
after a long one -- or "The file ends here."; the tool description quotes
the same words. A count that says lines means lines: the split-segment
count every range calculation uses is one higher whenever a file ends with a
newline, and only the sentence subtracts it. The edit leads the excerpt it
returns with upstream's "Showing lines X-Y of N from the edited file:", and
counts the edited file's lines with the one helper read_file counts a page
with, so the two state one number for one file. Upstream's periodic todo reminder
is carried in upstream's shape -- its words and `- [status] content` lines, re-sent
beside every third tool result and on every automatic turn while items are
unfinished -- with the bound it lacked: its list is cut at 800 bytes of the NFC
form the tokenizer reads rather than at 800 characters, and the startup proof
charges one reminder at its widest, because at most one stands in the request a
compaction leaves. A cap a service
applied travels back with its items rather than being discarded, because a
layer cannot declare a limit it was never told about.

Every tool result is held to `M`, one inline block, where its model copy is
made, and nowhere else. The scheduler finishes each call's copy once every
hook, rule and skill reminder has joined it, and each runtime that executes a
call itself — the ACP session, speculation, a subagent's refusal of a tool it
does not have — finishes its copy the same way. The joined text is measured
once, whole; a longer one keeps its start and its end, split as upstream's own
truncation split them -- one fifth of the room to the start, the rest to the
end -- and cut as it cut them: on line boundaries, a line only part of which
fits sliced and marked with upstream's `...`, and neither end meeting the cut
on a blank line. The complete text is retained as a session artifact, and one notice
stands in the cut with the true total and the exact call that reads the cut
lines back, or why they were not kept. Upstream's tool layouts are kept as
upstream lays them out: what a result says last, such as an exit code, an error
or a test summary, stays last and survives because the end does. Tools do not
bound themselves: how much a command prints, a page holds or a server replies
is not the tool's to decide, and a bound applied inside a tool was one more rule
a result could be cut by, and one more place for text joined after it to pass
it. `M` is a share of the served window, so the session is what knows it; a
provider that declares no window has no share to take and is refused. That a
bound exists is required: the window is guarded in exact tokens by the
compaction trigger, which refuses a request that no longer fits, and this bound
is what keeps that refusal unreachable in ordinary work. Its magnitude is a
declared policy, not a derivation. Bytes stand in for tokens in exactly one
place and one direction — the partition's framed block, `M + F`, where a block
of `M` bytes of NFC cannot exceed `M` tokens, because the tokenizer normalizes
to NFC and then spends at least a byte per token — and nothing else converts
between the two units. The bytes a text is written in are not that bound: NFC
can make a text three times longer, and U+1D1C0, four bytes, normalizes to
twelve. So one function, `tokenizerText`, measures every such bound in NFC, and
a bounded text is cut in that form -- a sliced line on a character boundary --
measured again whole, and handed on in the form measured; its notice is inside
the bound, not on top of it. A send, and the adoption of a speculated turn, refuse a result
past the bound as a defect of the path that made it rather than repairing it.
The service refuses a prompt not in NFC, and the suite materializer excludes
one on the same terms with the same Unicode version.

A failed call's model-facing text is held to the same bound by the same step,
and stays a failure. A failure message is the one place a tool writes
model-supplied input back out — the path it could not open, the pattern it
could not compile, the tool name it did not recognise — and an argument that
arrived merged or malformed makes that copy as large as the argument. The
operational summary follows the tool's own words to the model, so the end a
bound keeps says what failed. `error.message` is left whole on purpose, because
the scrollback, the `PostToolUseFailure` hook and the sanitized telemetry span
read it and want the operational summary in full.

Compaction summarises the prompt the last turn was issued against and carries
that turn, reasoning included, verbatim behind the snapshot, so the summary
request never holds what the turn generated. It requests the room left by that
exact input and by the widest notice a redraw may add, which is never below
`C`, and refuses insufficient space. It tells the draw that room: the request
carries the session's system prompt, which states a turn's limit and that
reaching it ends the session, and neither holds for a draw, so its directive
says the request is not a turn, states the number the request's `max_tokens` is
set to, and says that an answer reaching it is refused and asked for again, told
why, up to the draw limit, after which the conversation is not compacted and
cannot continue. The request is counted with that number at its widest, the
window, and the served tokenizer spends one token per digit, so the room it
states is the room it is issued with. The accepted snapshot is itself one inline
block: the bound is stated in the declaration the model is given, and
acceptance — not decoding — refuses a longer draw, which is redrawn whole
rather than cut. A redraw carries one message more than the request it
repeats: why the draw before it was refused and what to do instead, built from
measured values and closed names rather than the draw's text, never the draw
itself, and bounded at its widest inside the directive's share of `D`. A
candidate must end normally, carry the required six-part
snapshot, reduce the request, and leave an issuable turn. Failure retains the
previous history and reports that retained count. There is no separate
reasoning-phase limit or forced reasoning-end marker. Compaction invalidates
what the model can quote, not what it has seen: it disarms every file-read
entry's history residency and leaves the read and write evidence in place, so a
file this session wrote itself is still one it is allowed to overwrite.

The snapshot is declared, not described. The compaction request declares, after
the turn's own tools, one function whose closed parameter schema is the six
ordered sections, and forces a call to it by name, so presence, uniqueness and
ordering are properties of the declaration rather than of markup the model
types. The turn's tools stay declared because the history the request carries
calls them, and a conversation whose calls name functions its request does not
declare is a shape no tool-use conversation has; upstream's compaction request
on the session's own model declares the turn's tools for the same history. A
named choice's call ends with `stop` on the served stream, as the protocol has
it, so a strict response to a request that named its function is complete on
that terminal and only for a call to that function. Section
text travels as string arguments, so prose that collides with markup carries
through unchanged and nothing has to be escaped. `acceptStateSnapshot` judges
one drawn candidate against the same declaration, because a client cannot
observe whether the engine applied a constraint it declared. The added
declaration ends the turn's tool block differently, so the request does not
reuse the turn's prompt-cache prefix past it; the conversation it carries is
otherwise unchanged. What the request adds to the prompt it summarizes -- the
declaration and the directive -- is measured against that prompt as it was
issued and held to the static preamble's share.

How the snapshot is obtained is ours; how the history it becomes is rendered is
upstream's. The accepted sections are rendered as upstream's `<state_snapshot>`
block, one element per section, laid out as upstream's compression prompt lays
out the block it asks for, with nothing escaped. The history a compaction
commits is composed in upstream's order: the startup context, the snapshot with
its resume trailer as a user message, the model's acknowledgement in upstream's
words, the attachments -- the retained original inputs and the state
reminders, each block set off from the next -- as one user message, then the
carried turn. Upstream keeps the last turn as a model message of its own after
the attachments, and folds its call into the acknowledgement when nothing is
attached; the carried turn is kept whole in both places, reasoning included.
The trailer is upstream's own, word for word. The carried turn follows the
snapshot in time as well as in the history -- the snapshot is the state as of
the prompt that turn was issued against, and the turn is the step taken next --
so nothing calls it the most recent turn, which would place it before the
snapshot and make a snapshot that ends before the step its history shows taken
read as one that contradicts it.

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

Oversized text results and downloaded binary payloads share one immutable
session artifact store. What it keeps is named by numbers, because the model
reads an artifact back with the call a notice names and so copies its path: a
session's directory is numbered in the order sessions first kept an artifact,
and its artifacts in the order they were kept -- `session-artifacts/1/3.txt`,
where a digest of the session's id and of the bytes made some 130 tokens of
hexadecimal. Each number is taken by exclusive creation, so it names one
directory or file and never another. Which directory is a session's is a
claim recorded under a digest of its id, which only the store reads; a claim
that loses to another for the same session gives its directory back. An entry
the store did not keep is refused, and nothing is replaced. Payloads, claims
and directory ancestry are synced. Its capacity, 500 MiB, is declared in one place and is a policy rather than a
derivation. It counts actual retained files under the store's writer lock,
including interrupted-write remnants, and recreating Config does not reset it.
Reaching it is an outcome the caller states, not an error: a bounded result
keeps its start and its end and says the cut part was not kept, why, and what
to ask for instead, and a fetched binary that cannot be kept is refused with
the download to make instead.

Artifacts are retained by ownership and references. There is no age-based
collection. Forks and exports may refer to an artifact after its original chat
is deleted. A lock is not stolen based on elapsed time; an abandoned lock
requires explicit administrative recovery. Persistence, claim, lock and
cleanup failures retain their causes and refuse the operation.

A fetched body is retained whole, and its note says where, or why it was not.
Range bounds are explicit query facts. Text paging uses UTF-8 bytes and returns
the next line offset without consuming a line it did not return. Text decoding
follows a Unicode BOM or UTF-8 when absent; large streaming reads require UTF-8.
Invalid encoding, unreadable oversized lines, and unsupported media fail
explicitly.

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
cache residency. Every number of bytes, tokens or turns the deployment prompt
states is rendered once, in its `## Context` section, from the partition or the
turn budget that enforces it, and is named rather than restated everywhere
else -- "one inline block"; a prompt that states one twice, or states one the
section does not declare, is refused, and the headless qualification holds the
sealed instructions and every tool declaration to the same rule. A subagent
spends its own turn budget, as many turns as its parent's, and the shell tool
names `task_stop` only when its registry holds that tool.

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
Successful counts describe the committed checkpoint, the startup context it
keeps at its head included.

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
