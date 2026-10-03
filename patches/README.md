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
- Review-diff SHA-256: `91b1a80c29abbf9311ce84f10791fcbbde11f63b2e941e5b899b9dab433dfca9`
- Semantic transformer: `source_patch_v1/`
- Transformer-manifest SHA-256: `f0e4258acc5d8d973b2f9dd83e9b3adf1940ebb38f70d51dca7d5c090b05f2ed`
- Official npm package: `@qwen-code/qwen-code@0.21.12`, which this build does not fetch; it builds the commit archive above
- Pinned Node build/runtime image (linux/amd64 manifest): `node@sha256:d649c27dae7ba0137b3cef5dd75baa422c08dc3d9e3fc0c23dfb172dc3cc6436`

The transformer validates the pinned source, the reviewed diff, exact final file
identities, and 36 semantic concerns before changing the private source tree.
Removed files have an explicit absent final identity. Applying the same result
again verifies it without writing. A failed commit restores the original bytes,
permissions, and file presence. The image derives its unit test selection from the
retained final files plus their existing adjacent tests, each run with Vitest in
its owning package. Desktop's suites run under Bun (`bun:test`); they are
excluded from that selection and no gate runs them.

## Generation and ownership

Every generation uses the supported OpenAI-compatible vLLM contract: an explicit
positive context window, exact rendered-request tokenization, and strict native
tool-call commitment. Model selection is transactional. A requested route must
resolve and validate before it becomes active; failed preparation restores the
previous selection.

A required `GenerationContext` owns every chat and side request. The main scope
is the conversation's session ID. A tool-launched child owns its spawning call
ID; workflow and utility invocations own distinct IDs. A side query -- a goal
check, a fetched page's summary, an image described for a text model, a one-shot
generation -- owns a fresh, never-used ID (`sideQueryGenerationContext`), so it
neither matches as nor replaces the context the backend keeps for the
conversation that asked for it; the compaction draw, whose snapshot becomes that
conversation's history, runs under the conversation's ID. Child launch hooks,
resumption, and alternate model selection preserve that identity. The common
provider seam writes `kv_scope` after request decoration, including batch,
stream, JSON side queries, and exact counting. Count requests and generation
retain the same model, template, tool, and ownership context.

Structured calls become executable only after the full response and its
canonical assistant record are accepted. Normal completion requires an explicit
provider terminal. Recorded processing completion also requires captured 2xx
HTTP and a committed EOF or cancelled transport ending. Empty responses and
successful parsing followed by cancellation remain valid at this layer;
processing failure and history abandonment remain separate decisions. The
shared recorder, replay, native certifier and fake-provider verifier enforce
this relationship; see `docs/design/model-response-outcomes.md`.
A fresh bounded retry is permitted only before answer content
has been delivered. Text, whitespace, and literal protocol-like XML remain
verbatim. Transport failures, invalid usage, malformed calls, and post-terminal
content retain diagnostic text without publishing executable calls or a normal
terminal. Empty normally completed output is a valid response. A generation a
length terminal stopped is refused rather than committed: the call it stopped
is never made, nothing of it enters the history a request is rendered from, and
what the provider served of it is the model's output, carried, as the text it
was served as, to every record of the generation -- the stream's
`incomplete_tool_use` block, the session recording's record of a draw that
committed no turn, which a resume does not replay, a subagent's round, and a
compaction draw's accounting -- and it never becomes a function call anywhere.

The canonical assistant record and live history receive the same model-part
sequence. Structured-output payloads and approved plan arguments remain whole
through tool completion, ordinary resume, indexed restore, and fork. Loading
history never rewrites an argument from the current plan file. Operational
tool-call telemetry can omit a structured answer payload; that diagnostic
projection is never a conversation turn.

A turn refused at its limit is drawn again, told why, as the next turn, in the
headless session and every subagent alike, the way a refused compaction draw is
drawn again. The chat names the message the refused draw answered and admits
one kind of send against it, `redrawRefusedTurn`: its whole message is the
refusal notice, in upstream's `<system-reminder>` envelope and in the voice a
refused compaction draw is told in -- the answer reached its limit before it was
complete, is not in the conversation and ran nothing, and the next should reason
more briefly and write a long file in parts -- and it joins the conversation on
that message, so the redraw is the refused request with the notice added,
rendered from the history as every request is, never compacted in between and
joined by no reminder, context or hook. Every draw is a turn, charged to the
turn budget and billed in the record like any other. `MAX_GENERATION_DRAWS`, the
one bound every refused answer is drawn under, ends the run on the fourth
refusal in a row as the incomplete-generation state, with the limit and the last
draw's numbers; a generation stopped for any other reason is not drawn again
and is that state at once. One predicate in core (`refusedTurn`) decides it for
both reasoning loops.

A turn the model ended itself with no tool call is its final answer only when
its visible text is one. A message with no visible text, or one carrying the
served template's tool-call markup (`<tool_call>`, `</tool_call>`, `<function=`,
`</function>`, `<parameter=`, `</parameter>`) outside a structured call, is a
slip: it stays in history exactly as produced, is answered with a user-role
notice that says nothing was executed, and is followed by the next turn, charged
like any other. The notice is in upstream's shape for runtime context on a
turn: a `<system-reminder>` envelope, marking it as the runtime's words rather
than the user's, on upstream's own continuation prompt, "Please continue.",
because a message of reminders alone is, by upstream's reading, structure
rather than a turn. The third consecutive slip is the `SLIPPED_FINAL_MESSAGE` state,
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

A locked agent-service run is started owing a declared list of deliverables:
`--deliverables`, a JSON array of paths relative to `/artifacts`, `[]` when it
owes none. The locked runtime refuses to start without it, and no other runtime
accepts it, because outside the deployment no artifacts root is defined. Each
path is relative, with no empty, `.` or `..` component, no NUL, no component
past the kernel's 255-byte name limit and no whole path past its 4,095-byte one.
When the run ends with its final message, and only then, each declared path must
be a non-empty regular file, every component examined with `lstat` and none
followed, since the model can create links; the single emitter
(`settleDeclaredDeliverables`) turns a final message that left any missing into
the session-only state `SessionTerminateMode.MISSING_DELIVERABLES`,
`error_missing_deliverables` on the wire with exit code 1, whose record lists
every missing path in declared order under `missing_deliverables` and whose
message says what stands at each. It is never a success. Subagents owe nothing
and share no such state: `AgentTerminateMode` does not name it. The model is told
nothing new; the prompt names its files and the deployment contract says what
`/artifacts` keeps. The check proves existence, not quality, and nothing checks
that a declared path is the one the prompt names.

## Context and instructions

The context partition spends the window exactly, from six declared quantities.
`D`, the static preamble, is 3W/64 — 12,288 tokens at 262,144 — a declared
capacity rather than a derivation: the system prompt and the tool declarations
are texts this repo ships, so the turn preamble is counted exactly against it by
the served tokenizer before the first turn, and again before any later turn
whose preamble has changed, with the Git snapshot's repository values bounded by
their byte caps rather than counted, and so is the startup context that opens
every history, its environment lines and folder listing bounded the same way,
and the turn budget the `## Context` section states, its number bounded by the
sixteen digits a safe integer renders to; a preamble that does not fit is a
startup refusal naming shorten-or-deploy-larger. The preamble is in every
request the trigger is compared with, so `D` is charged in the fit, not held
back from the trigger. `A`, what a request adds to the prompt it was admitted
at, is 3W/256 — 3,072 tokens — a declared capacity proved the same way: a
refused turn's redraw adds its refusal notices to that prompt, and the request
that compacts it adds the snapshot's declaration and the directive, counted by
the startup proof in the shape every compaction request carries them with the
directive stating the widest ceiling, and a redraw's notice; their sum, 3,026,
is held within `A` before the first turn, and each compaction's preflight holds
what its own request adds to the same share, since a request can carry a user's
`/compress` text or a PreCompact hook's that no startup proof sees. The
startup context is kept whole at the head of every history a compaction builds
rather than rebuilt after it, so it is in the candidate the compaction counts
and cannot go missing, and the proof bounds the frame around a compacted
history's blocks -- the resume trailer -- with it, the blocks left out, and one
todo reminder
with its list bounded by bytes. `M`, one inline block, is W/8 bytes — 32,768 — a
declared magnitude and openly a policy: it is the most any single block placed
inline may be, and anything larger is kept whole in a file and paged back rather
than shortened. `S`, a compaction snapshot, is W/4 bytes — 65,536 — in the same
measure: the most an accepted snapshot may render to, declared from the
snapshots the model was recorded writing rather than borrowed from `M`
(`SNAPSHOT_SHARES` says from what). `F`, the per-message framing, is the 61 bytes the served
template wraps around one message at its widest, declared here and verified
against the template rather than copied from it. `R`, the reasoning a turn is
given beside its block, is W/32 tokens — 8,192 — a declared share, a quarter of
`M`, which is a policy rather than a measurement (`TURN_REASONING_SHARES` says
what it rests on and what recorded turns show).

`C`, the room every generation is issued with, is sized for the largest thing
a turn legitimately does, write one inline block after reasoning about it: the
block is at most `M` tokens whatever it holds, because a text's tokens are at
most the UTF-8 bytes of its NFC form, the form the served tokenizer splits, so
`C = M + R`. `T`, the compaction trigger, is what the window has left,
`T = W - C - A`. At the served window they are 40,960 and 218,112. What stands
in the window after a compaction is the preamble plus
`snapshot + authored input + carried turn + one result`; every term but the
turn is bounded in bytes before it exists, so `D + (S + F) + 2(M + F) + (C + F)`
must stand below the trigger, and does, by 33,547 at the served window. All of it is
asserted, not assumed: the fit, the no-overrun property and the compaction room
are checked at every window the partition can be given, and a window where the
fit fails is refused rather than partitioned. Every token of `C` or `A`, and
every byte of `M`, is a token `T` does not have; `D` and `S` are not in `T`,
since the preamble is counted in every request and a snapshot is never
generated as a turn. `C` is the output limit of every turn
whatever its prompt, and the least a compaction's draw is issued with. The route refuses a configured ceiling, including
`QWEN_CODE_MAX_OUTPUT_TOKENS`. Input, directive and candidate histories are
counted using the actual rendered request, and a request is counted once: the
tokenizer's answer is a function of what it is shown, so the count a turn takes
before compaction is the count of the request it sends whenever compaction left
that request as it was.

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
made, and nowhere else. JSON and stream-JSON records retain that result in full,
including characters whose JSON escaping expands its serialized byte size.
Transport serialization applies no additional content bound. The scheduler
finishes each call's copy once every hook, rule and skill reminder has joined
it, and each runtime that executes a
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

A shell command's result carries everything the command wrote before it
exited, or says what it does not carry, how much there was and the command that
gets it; a result that reports the exit status never stands over output that
was lost. The pseudo-terminal is read to the end the kernel reports. libuv
takes a hang-up that arrives with a read short of its buffer as the end of the
stream, but a pty master returns at most one line-discipline buffer, 4,095
bytes, per read, so a command that wrote more and exited before the client read
it had the rest closed over, and its result said `Exit Code: 0`. The pinned
node-pty, patched through upstream's own patch-package, reads the master
synchronously to EIO before it takes that end, and to EIO or EAGAIN before it
closes the socket after an exit. The final text is upstream's replay terminal
with every row kept: `replayTerminalOutput` takes each row as it scrolls above
the screen, where nothing a program writes can address it again, and writes
the output in pieces no write can scroll past, so a long output keeps its
start. A program's own erasures, and an alternate screen still shown at the
end, render as upstream renders them; one sequence that scrolls past the whole
window is refused rather than rendered short. Binary output, which is not
shown, and output past the capture limit, which is discarded as it arrives, are
stated through the one notice with the true total and the command to run
instead.

Compaction summarises the prompt the last turn was issued against and carries
that turn, reasoning included, verbatim behind the snapshot, so the summary
request never holds what the turn generated. Upstream's request holds the whole
history; with the turn and its result in it, a request on a prompt just below
the trigger could pass the window before the draw had a token, and the
partition's comment gives the arithmetic. It requests the room left by that
exact input and by the widest notice a redraw may add, which is never below
`C`, and refuses insufficient space. It tells the draw that room: the request
carries the session's system prompt, which states a turn's limit, and a draw's
room is what the request leaves, which is not that number, so its directive
says the request is not a turn, states the number the request's `max_tokens` is
set to, and says that an answer reaching it is refused and asked for again, told
why, up to the draw limit, after which the conversation is not compacted and
cannot continue. A turn's room is what the fit charges for the turn a
compaction carries; a draw's answer is never carried, only the snapshot it
declares, so it can be given whatever the window has left. The request is counted with that number at its widest, the
window, and the served tokenizer spends one token per digit, so the room it
states is the room it is issued with. The accepted snapshot is bounded by `S`:
the bound is stated in the declaration the model is given, and
acceptance — not decoding — refuses a longer draw, which is redrawn whole
rather than cut. A redraw carries one message more than the request it
repeats: why the draw before it was refused and what to do instead, built from
measured values and closed names rather than the draw's text, never the draw
itself, and bounded at its widest inside the additions' share, `A`. Every
reader reads a draw's ceiling where its request states it -- the last message,
or the one before a final notice, which opens as every refusal does -- and holds
it to the request's `max_tokens`, the client's replay and the native certifier
alike. A
candidate must end normally, carry the required
snapshot, reduce the request, and leave an issuable turn. Failure retains the
previous history and reports that retained count, and says why in words carried
beside the accounting rather than in it: every draw's refusal, in the clause the
next draw was told, the fault outside the model's answer its last draw ended
on, or what its preflight refused. A draw is drawn again, told why, only for a
fault in the model's own answer, under `MAX_GENERATION_DRAWS`. A draw whose
request failed in transport before it produced an answer had no answer to
judge: it is kept among the rejected attempts as
`COMPRESSION_FAILED_TRANSPORT_ERROR`, with what it had received, and the same
request is issued again, untold, under `FRESH_RESAMPLE_MAX_RETRIES`, the bound
a turn's broken stream is issued again under, by the one transport test
(`retryableStreamTransportCode`) a turn uses; it spends no draw, and a fresh
request that fails the same way ends the compaction with that status, naming
both faults. Every reader holds each draw to what its status names: a refused
or accepted answer claims a final request that completed and delivered it, and
a transport fault one that did not complete. A conversation still at or above the trigger after a
failed compaction ends there, and the ending names that failure. There is no separate
reasoning-phase limit or forced reasoning-end marker. Compaction invalidates
what the model can quote, not what it has seen: it disarms every file-read
entry's history residency and leaves the read and write evidence in place, so a
file this session wrote itself is still one it is allowed to overwrite.

Each compaction draw records converted text and reasoning alongside
`sdkValuesJson`: JSON serializations of SDK-parsed provider values captured
before Core conversion. These values preserve argument strings even when they
are malformed or cut off; they are not the provider's HTTP response bytes.
Each draw names the utility operation that produced it and states its physical
request count; its `sdkValuesJson` must equal the values decoded from that
operation's recorded response bodies, in order, so the draw is bound to the
bytes the provider sent. Every draw states its measured candidate token
count and snapshot byte count, using null when that measurement was not reached.
Rejected draws retain these fields alongside the final draw. The committed
post-compaction history has exact part boundaries and filled retained inputs.
The runtime installs that history and stores it once, as the canonical chat
checkpoint's `compressedHistory`; a canonical compaction record that also
carries `postCompactionHistory` is refused, because a second copy could only
disagree with the first. The stream's compaction record carries
`postCompactionHistory` as evidence of the committed composition. Model output
is never replaced by an empty-slot rendering of the snapshot. The shared schema
and native certifier refuse missing or unknown evidence fields.

The snapshot is declared, not described. Its sections are upstream's: the nine
elements of the `<state_snapshot>` block upstream's compression prompt asks for,
in upstream's names and order, each described in upstream's words and ours
where ours say more. The model declares all but `all_user_messages`, which is
where upstream carries the user's messages forward and which the runtime fills
with the original inputs it already retains verbatim, rather than asking the
model to copy them. The compaction request declares, after the turn's own
tools, one function whose closed parameter schema is those eight sections, in
order, and forces a call to it by name, so presence, uniqueness and
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
issued and held to the additions' share, `A`.

How the snapshot is obtained is ours; how the history it becomes is rendered is
upstream's, but for what upstream adds in the model's voice. The accepted
sections are rendered as upstream's `<state_snapshot>` block, one element per
section, its tags laid out as upstream's compression prompt lays them out, and
each section's text between its tags exactly as the model wrote it -- not
indented, trimmed or escaped, so whatever a section holds re-enters as written.
Its `all_user_messages` holds the retained original inputs verbatim, as the
parts they are, so their provenance survives the next compaction. The block is
bounded at acceptance with that element empty; the inputs are bounded where
they were submitted. The history a compaction commits opens with the startup
context and the snapshot with its resume trailer as a user message, then the
attachments -- the state reminders, each block set off from the next, as one
user message -- when there are any, then the carried turn. Upstream puts a
model-role acknowledgement after the snapshot and folds the carried turn into
it, keeping only its calls; neither is done here. The acknowledgement is text
in the model's voice the model never wrote, and folding deletes the model's
reasoning and text. The history a request is rendered from joins user content
that follows user content, as it joins the startup context to the snapshot, so
the carried model turn answers the snapshot and its attachments directly, its
parts whole and unchanged, reasoning included. The headless deployment attaches
nothing (plan mode cannot be entered under its fixed argv, and foreground-only
subagents leave none running at a compaction). The trailer is upstream's own,
word for word. The carried turn follows the snapshot in time as well as in the
history -- the snapshot is the state as of the prompt that turn was issued
against, and the turn is the step taken next -- so nothing calls it the most
recent turn, which would place it before the snapshot and make a snapshot that
ends before the step its history shows taken read as one that contradicts it.
The model is told all of this before its first turn, by the one statement
(`compactionDeclaration`) composed from the same element names and trailer the
frame is: that it writes the snapshot in a request of its own by calling
`state_snapshot`, what then stands where, and the trailer, quoted.

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

Every physical canonical chat record carries `recordingVersion: 21`, independently
of the client release string. A record with any other version, or none, is
refused with the file location, what the record declares (a version, or no
version), and the action: open it with the client release that wrote it, or
start a new session; the file is left unchanged. Unknown record kinds or
subtypes, malformed JSON, invalid UTF-8, and unterminated records also refuse
restoration. Root, indexed, child, fork, usage, IDE, and title readers use this
admission rule. Scans over many recordings (session list, title lookup, resume
by title, IDE readers) keep every readable result and report each unreadable
file, so one unreadable file never takes a listing down. An inactive branch
cannot hide an unsupported record. Live child
read failures reach subscribers as a terminal error and remain refusals on later
loads. Title metadata is selected from a complete validated scan with memory
holding the current physical record and the request replay state; title writes
are not duplicated to keep them within a tail window. This format governs the runtime chat JSONL that resume
reads. `output/events.jsonl` retains stdout bytes under the separate stream
contract and cannot serve as canonical history.

Each Chat generation records the tool-call identities present when its
normalizer starts, before it consumes the provider stream. The seed is bound
to the final physical request, and admitting readers derive normalized call
identities from it. Canonical root and child readers compare local seeds with
their replayed histories, and complete child admission compares a child's
seeds with their root mirrors while joining the root's physical proof.

Canonical resume replays explicit runtime history checkpoints and splices with
positioned assistant commits. Display inputs and request/response evidence cannot
be supplied to the typed history accumulator. Checkpoints include the image
payload store, and both ordinary and indexed readers preserve exact Content
boundaries and saved startup context. Current startup guidance is admitted as
new input when continuation begins. Older canonical formats cannot establish
this state and are refused with the action above. They are not promoted into
complete version 22 histories.
The complete-record evidence replay and the resume reader use the same
projection for runtime checkpoints, compaction, and rewind.
Adopted and realtime conversation records must carry content with the role
declared by their record type; non-object parts and invalid image-reference
identities are refused even on inactive branches. The same admission runs
before writing and in full and indexed readers.
Those presentation records cannot claim locally served model usage. Full and
indexed resume select served usage only from committed local model generations;
the canonical reader refuses an adopted or realtime assistant with a fabricated
usage report.

Catalog pages retain readable sessions and required per-file refusal metadata.
Refusals name the original file and physical location when available; directory
errors fail the request and cancellation preserves its reason. Automatic latest
selection refuses newer, tied or unknown-order unreadable candidates. Timestamp
pages finish their boundary group, and a hard scan limit remains explicitly
incomplete. An established Live target may be selected only if the core scan
proves the unscanned tail is strictly older. This does not certify the tail.

HTTP, ACP, cached catalogs and their SDK/browser/terminal/IDE consumers preserve
these diagnostics. Array convenience calls refuse to discard them; browser reloads
return the page. Desktop never infers deletion from a refused or incomplete scan.
VS Code uses the ACP catalog and its existing canonical offline message reader;
there is no alternate permissive catalog or JSONL branch. Inspect all returned
refusals with `qwen sessions list --json`, following its cursor hints and using
`--archive-state archived` for archived recordings. Live startup context labels
partial recent-session discovery. The
[canonical recording note](../docs/design/canonical-recording.md) states these
rules and their limits.

Fresh canonical generations carry the producing chat attempt's exact origin.
Accepted output is one complete assistant commit; refused and abandoned output
is generation evidence and cannot enter resume history. Child live fragments
use version 2 and settle against the same origin with literal prefix checks. Shared ACP replay,
SDK reduction, compaction, browser presentation and exports retain dispositions
and served usage without merging retries or changing model text. A retired
sidecar must be complete and settled before its cursor can be discarded.

Every supported generation request is admitted by `pipeline.ts` immediately after
`buildRequest`, before optional diagnostics and the SDK transport. The canonical
writer must accept a `model_request` record before dispatch. The record carries
the UTF-8 byte count and SHA-256 of the complete serialized body, its invocation
and compaction segment, a replayable full body or message delta, and the selected
OpenAI-compatible decoder mode, context model, strict tool-call handling, named
tool choice, exact token-counting rule and tagged thinking rule. The exact body
must state a boolean stream mode matching that policy; an absent or contradictory
mode is refused by request replay.
Serialization is frozen before diagnostics, so the SDK sends precisely those
bytes. A failed canonical write prevents dispatch. SDK-internal retries are
disabled for these calls: the existing controlled retry loop re-enters capture
for every attempt.
A request record establishes a dispatch intent, not proof of server receipt.

Every request declares chat-attempt or utility ownership outside the model body.
Chat responses retain separate transport completion, processing outcome, and
history disposition. Only the chat consumer can accept history, after its durable
commit. A failed physical retry remains abandoned when a subsequent physical
request succeeds. Root and child generation records name their producing
attempt and generation scope, and the shared readers refuse unknown attempts,
foreign scopes, repeated acceptance, or a complete certificate with an
unsettled response. Stream assistant rows and partial blocks carry no origin: a
partial belongs to the latest chat request in its scope, and a settled turn's
assistant rows are derived from its generation. A runtime-authored reply
follows a `system/runtime_operation` receipt naming its identity, scope, byte
length and SHA-256. A retried attempt keeps its exact output, reasoning
included, in its own generation as evidence.

Canonical evidence does not join the conversation parent chain. A live or
interrupted canonical file can retain an open response while exposing its durable
conversation; that does not certify complete generation evidence. Full, indexed,
and fork restoration reproduce the same committed parts with accepted and
abandoned physical attempts interleaved.

Each invocation starts with a full body, and the first request after its committed
compaction also carries a full body. Deltas retain the unchanged message prefix
and replace the rest, including changed reminders. A same-segment replacement
or removal retains zero messages in that same delta representation. They retain the exact serialized
non-message envelope, including tool declarations. The journal keeps the most
recent message slices per invocation, with no output queue when no renderer needs
one. Each active renderer owns a queue and releases it on closure. An ordinary
multi-turn renderer keeps its delta base across turn results.

Stream contract v19 exposes these same request records, declares the journal
origin in `system/stream_start`, and accounts for the output window at every root result.
Complete `system/init` runtime metadata has its own owner and does not reset that
journal. Each checkpoint lists open response and logical attempt identities.
An ordinary turn may finish while a background child remains active; complete
EOF admission requires both populations to close. The native certifier
requires every request's response to have a transport end and a processing
outcome. Native certification replays message deltas, verifies byte lengths and
hashes, and refuses missing, repeated, reordered, foreign, or unknown evidence.
Physical requests, logical attempts and reported turns are separate populations;
generation completion binds every physical member and its history disposition.
The composition and headless harnesses compare reconstructed bodies with the
actual bytes received by their fake providers. Canonical readers validate request
evidence in physical order before selecting conversation branches. Evidence has
no conversation message or subtype and cannot become the active history tail;
resume still reads canonical chat JSONL, never stdout. Forks copy the complete
physical request and response journal, including delta bases on abandoned branches, while
copying only active conversation history. Exact provider, canonical
writer, full-history and indexed-history tests exercise this separation together.

This is shared client request construction and recording, so it applies to all
client entry points using the deployment. A direct vLLM caller does not have this
client's conversation, reminders, compaction segments or canonical recorder; this
change does not add backend logging for such callers. Backend-owned omissions
must be addressed in the backend. Request capture alone does not establish that
every response byte and abandoned attempt is recorded.

The shared pipeline wraps the provider SDK's public `fetchWithTimeout` result
before status, JSON or SSE parsing, preserving its configured transport and proxy.
The bytes are the fetch entity body after transport content decoding (such as
gzip), before any SDK text or event decoding.
Each `model_response` record names its physical request and records HTTP status
and content type, ordered base64 body bytes, a transport end with the exact
observed byte count and SHA-256, or a processing outcome. EOF, failed read,
cancellation and an undispatched intent are distinct. SDK and conversion
completion, failure or cancellation are recorded after the transport end, so an
EOF cannot hide a later parser failure. Arbitrary malformed UTF-8 and JSON survive
as bytes. Capture reads the transport from the moment the response exists,
with one read outstanding until it ends, because an HTTP client discards bytes it
holds unread when its connection fails; it durably writes each bounded record
before parser delivery. Read-ahead past its stated bound refuses the response
after recording every byte read, so a record is complete or says it is not. The
record quantum never truncates a response. Active stdout renderers
drain each admitted record before generation proceeds. Detached renderers release
their window without poisoning the canonical writer. Recording represents what
happened, so no judgement inside it removes a record from the output. A
refusal is a `RecordingRefusal`, and it carries the record it refused, whole.
The journal, which keeps the canonical history a prefix its readers accept by
never persisting a record it refuses, publishes that refusal to every window
before it stops -- or keeps it for a window not yet open -- and a stdout window
whose own replay refuses a record does the same. A window writes the refusal
in the record's place as a `session_recording_degraded` record of reason
`refused`, which tells the rule that refused and carries the refused record as
`refused_record`: the stream admits no record its contract refuses, so this is
where the record a refusal is about is kept, at its place, and nothing after it
is judged against it. A storage failure is reason `write_failed` and keeps
upstream's words. No further request is sent; what follows on the stream
settles the work that was in flight, and each stop is told once. A compaction
in flight when the recording stops commits nothing and writes no checkpoint:
its record, status `COMPRESSION_FAILED_RECORDING_STOPPED`, claiming only the
counts and draws the recording settled, reaches the stream after the stop, on
`CompactionRecordingStoppedError`, which ends the session with the stop's cause.
Recorder waits are excluded
from the request's start, idle and generation clocks, so the recorder bounds
itself: the asynchronous canonical writer and response cancellation refuse
after one minute without progress
([recording progress](../docs/design/session-recording-progress.md)).
Synchronous child-transcript file I/O cannot be preempted by that budget and is
unbounded.

Canonical recording version 22 structurally excludes response evidence from
messages, conversation branches and the active parent chain. Full, indexed and live readers
validate physical response sequence, ownership, byte offsets and terminal hashes.
They can inspect an explicitly open live prefix; they do not certify that prefix
as a complete record set. The native certifier refuses missing response and
logical completion. Both fake providers compare recorded bytes with their
actual response body, including malformed SSE and cancelled prefixes.

Each response outcome also records how many SDK values reached conversion and
how many decoded outputs crossed the shared pipeline. A chat attempt completion
records the producing physical request on each observation and how many outputs
Chat incorporated into its generation. Readers require that receipt to equal the
generation observation count and, for an accepted attempt, the count delivered
by the final physical response. An abandoned attempt may retain a shorter
consumer prefix. Readers require earlier retry requests to deliver no decoded
output and bind every observation to the final physical request. These
boundaries distinguish early cancellation from a complete response body and a
diagnostic expansion from one SDK value. The native certifier checks these
counts, the served usage and the byte identity of every response, but does not
decode chat observations from response bytes. The pinned client's
`dist/record-verifier.js` then replays each retained OpenAI response with the
selected decoder and normalizer and compares the raw observations and
normalized call IDs with the generation. Every utility request names the
logical operation shared by its retries and ends with a separate receipt for the exact decoded prefix returned
to the shared client. A stream returned before its first read still closes its
physical iterator and writes a zero-output receipt. The client readers and the
native certifier require that receipt after the processing outcome, reject
duplicate or excessive counts, and leave a missing receipt open at EOF. The
fake-provider harness makes the same check.

These records cover the HTTP response observed by every shared-client generation,
including side queries and children. They cannot establish tokens generated but
never transmitted, parser omissions inside a backend, or why an attempt was
accepted or abandoned. Transport EOF is not semantic acceptance. Those obligations
remain separate; no backend record is inferred from client bytes.
The utility delivery receipt belongs in the shared client because only that
client can observe which decoded outputs it returned to its caller; the vLLM
backend cannot witness that boundary for either this client or direct callers.

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
complete all-zero report is valid. An absent report is not that value. Wire
declarations preserve the full report, observed aggregates, missing and
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
a report does not prove attribution of concurrent effects. The deployment prompt
carries no advice on when to delegate or how much to read, and no timestamp; the
date is the startup context's. Every number of bytes, tokens or turns the deployment prompt
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
project instructions remain available. The output-language rule is the sealed
home's own, the `auto` rule upstream writes when a start with no language set
finds none, so the system prompt carries it where upstream's does, after the
other context files; a project's `.qwen/output-language.md` is not read, and a
home without the rule refuses to start. Leading slash task text is literal in
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
preserving independent failures reported by those operations; a recording stop
a failure listener was already told of was reported when it happened and is not
raised again. The recording is settled before a run writes anything terminal,
and what it settled to is part of how the run ended: a recording that stopped
ends the run in the error state, with its cause, and the fault that cause wraps, on stderr and the error state's
exit code, and no result is written after the stop. Queued turn work owns whether its result was
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
does not suppress canonical request evidence. Source settings, CLI arguments
and daemon feature negotiation expose that single contract.

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

The gates that run are the Rust `cargo test` suites, the client typecheck
(the CLI build's `tsc --build`), the image's test selection (every patched test
file and the test beside every patched source file, run with Vitest in its
package), and the headless qualification (`docker/scripts/check_headless_cli.py`,
a fake provider driving the real client and certifying its stream). Python
transformer and manifest checks verify exact source reconstruction. None of
these needs model serving, a provider session, a release or a GPU, and none is
live provider or session verification.

Three points are not verified by any gate. Desktop's suites run under Bun and
are excluded from the test selection. `is_error` on a returned call's row
cannot be bound to the model's input, because nothing in that input carries it:
the tool message renders a function response's `output` and its `error` as the
same bare text, and the served template frames every tool response alike, so no
byte of any request tells a failed call from a successful one. Carrying the
distinction to the model would put text its template never renders into its
input. What that leaves open is real: the flag, and the result record's
`permission_denials` built from the same error, are the client's own claims, so
a reader that judges a run by its failed calls -- a harness counting tool errors
-- trusts the client for them, and a client defect or a forged stream that flips
one still certifies. The browser transcript reader validates record structure
with Core's pure preparation but does not re-verify record digests; only node
readers do.

Compaction reasoning and child reasoning appear inline in recorded
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
it. Source review also confirms that UI retention drops closed-session
observations, nested observation objects remain mutable, and optional telemetry catches can suppress admission
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
admission does not establish that lifetime contract. The whole
scoped-consumption migration remains unimplemented; it does not require
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
inferred object shape required by canonical recording. The client typecheck and
the image's test selection (every formatter case and the held ACP
recording/delivery controls) qualify this seam.
These checks do not establish general extended-session correctness.
