# Qwen3.8 local agent service

This repository is the one supported local-agent experience for this workstation.
It runs a pinned, patched Qwen Code client in an offline Docker container against
the already deployed, corrected Qwen3.8-27B NVFP4 backend. Each session is
intentionally one long-lived main agent thread with optional sequential
foreground subagents, one model, one sampling policy, one context policy, one
network path, and one set of scripts. How many sessions run at once is
deliberately not this service's decision: it cannot know it is not one worker
behind a load balancer, so serving capacity and placement are governed above
it, and every session is isolated from every other.

There is no Claude mode, Codex mode, Qwen3.6 mode, text-only mode,
reduced-context mode, alternate port, retry downgrade, heuristic context clamp,
XML tool recovery, host client installation, or compatibility fallback. Vision is
an inseparable part of the one mode, not a switch. A contradiction is an error with
evidence, not an invitation to try something else.

The application itself runs in Docker. The host scripts perform only explicit
diagnostics and Docker lifecycle control. They do not install packages or modify
the host's Claude, Codex, Qwen, npm, Python, Rust, or shell configuration.

## Bottom line

The fixed stack is:

| Component | Locked value |
|---|---|
| Model source | `unsloth/Qwen3.8-27B-NVFP4` |
| Model revision | `16b6615af3548b88e2d8e382457bc705b00479cf` |
| Official BF16 reference | `Qwen/Qwen3.8-27B` at `1d4bf0f2ff6012fd82039f2fa52739d0dd7c60c0` |
| Deployable correction | Restore all 161 offset-RMSNorm tensors from the official revision |
| Corrected model SHA-256 | `5fd70b38b3708e47adc1e9e9ab90f5d688ec01177d0718fdd16678696fdb0988` |
| Served name | `qwen3.8-27b-nvfp4-k8v4` |
| vLLM source | `9df9b0b0a1816b6d0d0f6ecd0da563cc37fd72f5` |
| vLLM runtime | `0.27.2rc1.dev106+g9df9b0b0a`, socket-isolated non-root v21 profile |
| Weights | Mixed NVFP4/FP8, Compressed Tensors |
| KV cache | TurboQuant K8V4: FP8 keys, packed 4-bit values |
| Context | Native `262144` tokens |
| Vision | Complete BF16 tower, full released image-processor pixel budget |
| Images | At most 15 original static PNGs; 16,777,216 pixels each; aspect ratio at most 30:1 |
| Image transport | Lossless inline PNG; static 8-bit RGB/RGBA only; video/audio rejected |
| MTP/speculative decoding | Disabled |
| Thinking | Required, `xhigh` |
| Qwen Code | `0.21.12`, commit `b965d5f8c24f48e65fb0b17c7d45f34ca4ce8f38` |
| Agent image | Pinned by [`config/release.lock.json`](config/release.lock.json) (`.images.agent`) |
| Service | Rust, one session at a time, Docker-only |
| Release lock | [`config/release.lock.json`](config/release.lock.json), which pins the implementation commit and all five image IDs |
| Service image | Pinned by [`config/release.lock.json`](config/release.lock.json) (`.images.service`) |
| Docker broker image | Pinned by [`config/release.lock.json`](config/release.lock.json) (`.images.broker`) |
| Fixed-relay image | Pinned by [`config/release.lock.json`](config/release.lock.json) (`.images.relay`) |
| Stream-capture image | Pinned by [`config/release.lock.json`](config/release.lock.json) (`.images.capture`) |
| Service listener | `127.0.0.1:8090` only |
| Model listener | `127.0.0.1:8000` only |

Every longer pin—including base-image digests, package snapshot and versions,
source archive and patch hashes, Docker/BuildKit versions, image identity, live
backend command, corrected model directory/hash/correction/manifest, official BF16
revision, driver, GPU, and configuration hashes—is in
[`config/stack.lock.json`](config/stack.lock.json). The lock is compiled into the
service binary. At startup, the mounted copy must match the compiled copy
byte-for-byte, the agent image ID and labels must match it, and the live backend
container and HTTP endpoints must match it field-for-field.

The service independently requires `/model` to be the exact corrected directory as
one read-only bind mount and requires the backend container's source revision,
official revision, correction recipe, corrected model digest, and manifest digest
labels. It also requires a read-only backend root running as `2000:0`, exact bounded
`/tmp` and `/run` tmpfs contracts, exactly one labelled v21 vLLM cache volume at
`/home/vllm/.cache/vllm`, and no other mount. Every persistent JIT/cache path is
rooted beneath that exact volume; runtime writes cannot mutate the container layer.
The backend's own status performs the complete file-manifest verification.
An uncorrected Unsloth mount cannot satisfy this lock.

The backend repository remains authoritative for the server patches, model-file
manifest, VRAM accounting, native-context proof, prompt template, parser tests,
and cache benchmarks: [`../Qwen_best_model_ever/README.md`](../Qwen_best_model_ever/README.md).
Its ordered remaining-work record also makes the final post-release task an
end-to-end mathematical comparison of contextual execution under the deployed
mixed NVFP4/FP8 weights versus the exact official BF16 reference. That audit must
separately attribute weight-quantization error and the additional TurboQuant K8V4
cache error; it is not satisfied by the already completed tensor, isolated matmul,
MRoPE, or cache-kernel checks.

## Why native 262K, not a nominal one million

Qwen3.8-27B is natively trained for 262,144 tokens. A one-million-token profile
requires RoPE scaling and substantially more cache memory; neither a config flag nor
the model card creates physical VRAM.

The pinned vLLM implementation accounts for K8V4 as 24,832 bytes per token across
Qwen3.8's sixteen full-attention layers. That is 6.0625 GiB of raw full-attention
cache at 262,144 tokens and about 23.126 GiB at one million tokens. The deployed
vision-capable model logs 21.34 GiB of loaded model memory. After hybrid-state
paging and alignment, its explicit 6.45 GiB cache reservation provides 264,115
cache tokens, only 1,971 above the native limit. CUDA graphs remain enabled. The
vision encoder temporarily receives the already reserved 1,024 MiB TurboQuant
workspace plus 640 MiB fixed headroom without changing cache capacity, text-prefill
chunking, weight precision, image precision, or the context window.

Accordingly, the only honest quality-first maximum on this GPU is native 262,144.
The exact final backend proved a 262,143-token prompt plus one output token with all
fifteen maximum-size images and correctly rejected a request that exceeded the
physical model window. Fifteen distinct 4096-by-4096 images were also transcribed
successfully in one normal request; the sixteenth was rejected. A static-YaRN
startup was experimentally bracketed around 336K before OOM, but it lacks both
quality validation and operational margin and is not a supported mode.

K8V4 here is a runtime cache format. The NVFP4 checkpoint does not contain a
precomputed 4-bit value cache. TurboQuant quantizes each live key to FP8 and each
live value to packed 4-bit with per-vector FP16 scale and zero point. K8V4 was
chosen over K4V4 because keeping keys at FP8 is the more correctness-oriented
four-bit-cache tradeoff. It is never silently substituted with K4V4, ordinary FP8,
BF16, or a 2/3-bit scheme.

## Thinking, sampling, and context lifetime

The model always thinks at `xhigh`. The complete explicit sampling tuple is:

```text
temperature          = 1.0
top_p                = 0.95
top_k                = 20
min_p                = 0.0
presence_penalty     = 0.0
repetition_penalty   = 1.0
parallel_tool_calls  = false
```

`repetition_penalty=1.0` is neutral. The historical Qwen3.6 repetition detector is
not retained: Qwen3.8's published thinking recommendation no longer calls for it,
and adding one would be an untrained semantic intervention. “Deterministic prompts”
are not treated as a real guarantee; fixed seeds and greedy GPU execution do not
make a long agent trajectory bitwise deterministic.

The reasoning ceiling is 262,144 tokens and the final-response ceiling is 131,072.
Those are separate phase ceilings, not reservations and not additive capacity. The
patched server and client still enforce:

```text
rendered prompt + reasoning + tools + final response <= 262144
```

These are defaults at both layers, not suggestions in prose. vLLM defaults omitted
request fields to thinking enabled, xhigh, and the exact tuple above. The pinned Qwen Code settings send the same values explicitly,
including `thinking_token_budget=262144`,
`final_response_token_budget=131072`, and `add_vision_id=false`. The backend maps
high and max to the canonical xhigh rendering and rejects medium, low, or disabled
thinking in this profile. A client cannot accidentally obtain the old Qwen3.6
repetition intervention or a weaker fast path.

Thinking from prior assistant turns is shed rather than carried. The sealed
client settings send `chat_template_kwargs.preserve_thinking: false`, and the
backend serves the same value as a default, so the policy holds at both layers
and for any caller. It is stated rather than omitted because the chat template
gates on `preserve_thinking is undefined or preserve_thinking is true or
loop.index0 > ns.last_query_index`: an absent value satisfies the first clause,
so omitting the field does not defer the choice to the engine, it selects
retention. Measured against the deployed engine on a four-message history, an
omitted value and an explicit `true` render the same 303 tokens while `false`
renders 198.

The third clause is what decides how much is shed. `ns.last_query_index` is the
index of the most recent `role: "user"` message whose rendered content is not
wrapped in `<tool_response>`, and thinking is dropped from every assistant turn
at or before it. That wrapper test never matches in an agent loop: tool results
travel as `role: "tool"` messages and vLLM's chat parser keeps that role, so the
template wraps them in `<tool_response>` only while rendering, after the
backwards scan has already run. What does match is the client's own active-todo
reminder — Qwen Code re-injects it every third tool turn
(`ACTIVE_TODO_REMINDER_REFRESH_TURNS = 3`), its text opens with
`<system-reminder>`, and the OpenAI converter emits it as a separate
`role: "user"` message following the tool messages. Each injection becomes the
new `ns.last_query_index` and moves the cut forward.

How much reasoning survives is therefore a property of the trajectory, not of
the request: it is decided by where the model last called `todo_write`. Across
the 132 recorded full-suite runs under
`artifacts/swe-rebench-2026-07-production-service/`, `todo_write` is called in
100 and never called in the other 32; where it is never called the cut never
leaves the opening prompt and nothing is shed. That dependence is a fact about
this gate, and a run that measures retention has to record where its cut fell
rather than assume it.

The served context window is divided into five shares that spend it exactly, and
every context budget is one of them:

| share | of the window | at 262,144 |
| --- | --- | --- |
| summary reserve | 48/256 | 49,152 |
| one turn's output | 32/256 | 32,768 |
| that turn's tool results | 16/256 | 16,384 |
| the compaction directive | 2/256 | 2,048 |
| the history a turn stands on | the remainder | 161,792 |

The last four sum to the window less the summary reserve. Under those bounded
terms, a turn issued below the compaction trigger, given at most one
turn's output and appending at most one turn's tool results, produces a history
that leaves the summary reserve when the bounded directive is appended. Raising
`max_model_len` re-derives all five shares. New authored input, retained
instructions, tools, and the final compaction directive still require exact
recounting at admission; the partition does not guarantee that arbitrary new
content will fit.

The snapshot is issued at the room the window actually has — the window less
the summary request that was just counted — and the reserve is the minimum room
required to issue that request. A request lighter than the bounded worst case
therefore buys the snapshot more room than the reserve. A request that would
leave less than the reserve is refused instead of quietly shrinking the snapshot.

Every main-turn context-boundary decision uses the real vLLM tokenizer on the
fully rendered request. Before compaction and again before generation, Qwen Code
sends the exact messages, typed tool history, image parts, tool schemas, and
template arguments to the backend `/tokenize` endpoint. A turn is issued below
the compaction trigger or it is not issued at all, and it is issued with the
smaller of the configured output ceiling and its window share. The tool results
it appends are measured the same way — the rendered request counted with the
pending batch and without it, the difference being what the batch costs — and a
batch over its share is written to disk whole and replaced by references to the
files rather than shortened. There is no character division, `target // 8`,
image-token guess, padding margin, local tokenizer, or tokenizer fallback
anywhere in the compaction trigger, the outbound sizing, or the tool-result
bound. If the tokenizer is missing, malformed, or reports another model window,
the turn fails before generation.

A session is bounded by model turns, never by wall-clock time: the default
budget is 400 turns (`limits.max_session_turns`), a submission may name any
budget from 1 to the locked 2,000-turn ceiling
(`limits.max_session_turns_ceiling`) in its optional `max_session_turns` field,
the chosen budget is enforced by the client itself, and `max_wall_time_seconds`
is disabled everywhere. The budget counts the owning session's own turns, so a
foreground subagent that spends sixty turns costs its parent one; a subagent is
bounded by the same budget in its own right, and if it exhausts it the parent is
told so explicitly, with the turn count, and treats the assignment as unfinished
rather than concluding from a partial report. Child work consumes additional
provider tokens independently of the parent turn count. Returning to a parent
can reuse backend state only while that state remains retained; whole-owner
eviction can require reprocessing. Turns are the hardware-independent measure of
agent progress, so the same trajectory is judged
identically whatever the backend's generation speed; a wall-clock budget would
instead score how fast this GPU happens to run. Reaching the budget is an
ordinary terminal outcome, not a fault: the client exits 53, reports
`error_max_turns` with a turn count equal to the budget, and the work done up to
that point stands. `is_error` is true because the run holds no final answer —
the model never wrote one — and not because anything went wrong; a caller that
wants to tell "your bound stopped it" from "the harness broke" reads
`agent_result_subtype`. The budget is a degenerate-loop circuit breaker: a
repository-level fix needs roughly 80-190 turns to orient, build, diagnose,
implement, and verify, so 400 admits a complete second attempt after a wrong
hypothesis. A caller who knows a particular task is shaped differently can say
so per session, but something must stay finite or a degenerate loop simply asks
for a bigger number: the ceiling is exactly five default budgets, and a request
for zero, a negative count, a non-integer, or more than 2,000 turns is an error
naming the value, never a silently clamped session that would end as an ordinary
exit 53 and be graded as one. The effective budget is recorded in the session
body and in the bundle's `control/turn-budget.json`, so a finished session can
be read back to see the bound it actually ran under. Cumulative tool calls
have no separate cutoff, and one model turn still has a 1,000-tool-call circuit
breaker for degenerate output. Auto-compaction is due when the rendered request
reaches the compaction trigger, the share of the window the history is allowed.
Subagents run
sequentially in the foreground and return concise findings to the same main thread;
there are no background branches, teams, worktrees, alternate models, or nested
subagents.

## Qwen3.6 audit and why the old patches were not copied

The historical repository at `/home/user/Desktop/qwen_36_agent_setup` was reviewed
against current vLLM, Qwen Code, Qwen3.8, the new prompt template, and the Unsloth
checkpoint. Qwen3.6 and Qwen3.8 share the Qwen3.5 conditional-generation
architecture, but their post-training, quantized weights, tokenizer/template policy,
sampling guidance, and frontend/parser support are materially different.

The audit retained principles, not old hunks:

- explicit model defaults and strict schemas are still right;
- exact tokenizer and render/parse tests are still necessary;
- malformed historical tool chains must still fail closed;
- the Qwen3.6 egress rename and repetition detector are obsolete and rejected;
- old Qwen3.6 multimodal workarounds are obsolete; current image handling was
  re-derived from the Qwen3.8 template, current vLLM, and the measured full-quality
  backend contract;
- several reasoning-ingress and parser reconstruction issues are fixed or superseded
  upstream;
- current code nevertheless had new tool-grammar, truncation, TurboQuant workspace,
  phase-budget, and protocol-validation defects, which received narrow reviewed
  patches and direct tests in the backend repository.

The old `agent_service` design also contained good outer-envelope ideas alongside
obsolete implementation: exclusive per-session ownership, copied disposable workspaces,
offline/no-GPU agents, explicit cancellation, durable JSONL and bundles, labels,
orphan recovery, and ordered teardown were kept. Qwen Code 0.15.6, Qwen3.6 AWQ,
port 8001, ttyd, wildcard bridge plumbing, 152K context, host installation,
warning-only probes, polling waits, lossy vision fallbacks, and mutable package
installation were removed.

## The pinned Qwen Code source transformation

Qwen Code uses the exact source behind `v0.21.12`, revision
`b965d5f8c24f48e65fb0b17c7d45f34ca4ce8f38`. The upstream archive, reviewed patch,
transformer manifest, and their hashes are recorded in the
[source contract](patches/README.md) and [stack lock](config/stack.lock.json).
The source pins describe this checkout. The independently pinned
[release](config/release.lock.json) identifies the deployed images; changing source
pins does not build, release, or update those images.

The [source transformer](patches/source_patch_v1) applies the reviewed changes to
that exact upstream tree. It checks source identities, structural landmarks, 29
semantic concerns, and final identities, including explicit absent identities for
removed paths. Drift, ambiguous landmarks, intermediate states, or concurrent
mutation refuse application. Failed publication restores original bytes, modes,
and file presence. Reapplying the completed transformation verifies without
writing. The [unified patch](patches/qwen-code-0.21.12-agent-service.patch) records
the same reviewed source changes; it is not a second application mechanism.

The Docker build starts from a fresh extraction, tests the transformer,
installs the exact upstream dependency lock, applies upstream's own package
patches, builds the CLI, and runs the unit test selection derived from retained
changed files and adjacent tests, with each suite in its package directory. Provider
integration scenarios require their own environment; the final smoke supplies the
controlled provider protocol for build-time CLI qualification. The metadata
generator derives the pinned version, commit, and timestamp from verified inputs. The final agent stage then runs the
installed CLI as UID/GID 1000 with the launcher's fixed argument vector and shipping
settings. A private loopback protocol fixture asks it to read a fresh nonce from a
file and return it. Qualification requires successful exit, actual tool execution,
structured events, exact fixture usage, and a durable owned transcript. Empty output
and nonzero exit are tested negative controls. No program or settings are installed
after this smoke gate. This qualifies CLI startup and wiring without a model;
deployment networking, sandbox mounts, and real model behavior remain separate.

Every Config constructs its session recorder. Session initialization acquires its
writer lease before chat initialization, so interactive, headless, ACP and hidden
memory requests share the same owner. Workspace-only preparation has an explicit
`initializeWorkspace` operation for replay and MCP discovery, with no writing
session or chat activation. Recorder presence and writer ownership are distinct:
a workspace helper cannot make a generation valid merely by possessing a recorder.

A terminal conversation and a headless conversation share the same obligations:

- Each chat and side request carries an immutable invocation owner. Root work owns
  the session ID; a tool-launched child owns its spawning call ID. The common
  provider seam binds batch, streaming, and exact-count requests to that owner and
  writes `kv_scope` after request decoration.
- A selected route must provide the supported OpenAI-compatible vLLM contract,
  exact request counting, strict tool calls, and an explicit context window.
  Activation publishes the provider and configuration together; failed selection
  restores the preceding pair.
- The fully rendered next request determines admission and the five context
  shares. Ordinary output uses the smaller of its configured ceiling and its
  window share. Compaction receives the room left by its exact input, requires at
  least the summary reserve, and accepts only a normally terminated, structurally
  valid six-section snapshot that leaves an issuable turn.
- Original authored inputs and ordered corrections survive outside generated
  summaries, including across repeated compaction and resume. Structural snapshot
  validation does not establish the factual correctness of the model's summary.
- A complete structured call becomes executable only after the response and its
  canonical assistant record are accepted. Literal text remains intact. Missing
  terminal evidence remains incomplete. Retrying the same confirmed request is
  permitted only before answer content or a structured call is delivered; the
  transport and invalid-stream categories each permit one fresh resample, while
  rate-limit retries follow the selected route's explicit policy.
- Dispatch and final observation share a durable request identity. Served counts,
  missing usage, and unfinalized requests remain distinct. One accumulator supplies
  session, child, history, export, and presentation summaries. Child status is
  bounded and exposes inspectable transcript evidence, including failure and
  cancellation.
- Oversized tool text and binary downloads use one immutable session artifact
  store. The durable 500 MiB quota counts retained files under exclusive ownership.
  References survive elapsed time and session recreation. No age collector deletes
  payloads that a live conversation, fork, or export can still reference.
- Session recording, replacement, mutation, and cleanup use the owning writer and
  await its obligations. Output flush joins actual write callbacks before orderly
  exit. Abrupt failure can still leave an incomplete captured JSONL tail, whose
  earlier valid observations remain available without certifying a complete result.
- Native schemas remain closed; external schemas retain their advertised JSON
  Schema semantics. Trusted editor metadata lives outside model arguments. Text
  reads preserve decoded content, and original validated PNG parts retain their
  chronology within the tool response.

The sealed deployment separately fixes its advertised capabilities: the ten tools
below, sequential foreground `general-purpose` and `Explore` children, immutable
settings and instructions, and per-child scratch and effect journals. It excludes
forks, background work, teams, worktrees, alternate child models, and nesting.
Workspace environment/configuration discovery, ambient MCP, hooks, managed memory,
custom workflows, and injected policy remain disabled through authentication;
ordinary project `QWEN.md` and `AGENTS.md` instructions remain available. Leading
slash task text is literal. Generic host prompts describe the authority actually
provided by their invocation.

All deployment roles receive the same engineering discipline. Child execution
consumes its own turns and provider tokens even though its invocation occupies one
parent turn. Backend cache reuse is conditional on retained state; delegation does
not guarantee cache residency or avoid reprocessing. Effect journals record
observed changes and do not prove exclusive causality or rollback.

Manual compaction commands share one completed outcome across terminal, ACP and
headless presentation. Cancellation retains command ownership through action and
recording settlement; it cannot admit a successor or discard a completed
checkpoint. Every failure status is named, absent counts are refused, and complete
user and hook directives reach exact admission. A later finalization failure
retains the committed checkpoint and blocks further chat use until restoration
with a new client. The [source contract](patches/README.md) describes the boundaries
and qualification limits. CPU-only native TypeScript builds and targeted runtime
checks cover the shared formatter, ACP recording and delivery, and browser SDK.
The browser SDK retains its upstream size assertion and shares exact scalar-count
validation with Core.

Six broader implementation obligations remain open: operational daemon metrics,
provider-stream lifetime, reasoning stored by reference, prompt-hook failure
policy, atomic speculative file/history acceptance, and resident background
AgentTool settlement. Review-worktree ownership was deliberately excluded from
this round because fetch-pr and PR worktrees are not used. Compaction and child
reasoning remain inline and fully retained; output settlement and surviving
terminal observations do not close the reasoning-reference requirement.

The allowed client tools are exactly:

```text
agent
edit
glob
grep_search
list_directory
notebook_edit
read_file
run_shell_command
todo_write
write_file
```

The agent image includes a curated offline toolchain: Node 22.23.2, Python 3.12,
GCC/Clang, CMake/Ninja, Go, Rust, Java 21, Git/Git LFS, GDB/strace, ripgrep/fd,
jq/yq, SQLite/PostgreSQL client, Graphviz, Pandoc, FFmpeg, ImageMagick, Poppler,
QPDF, archive tools, editors, and shell utilities. The package list is not an
unbounded legacy kitchen sink: every top-level package version is recorded in
[`config/agent-apt-packages.lock`](config/agent-apt-packages.lock), and the Ubuntu
snapshot pins all transitive packages. There is no `sudo`, SSH server, ttyd, browser
server, package bootstrap script, or runtime installer.

## Full-quality images and exact chronology

Images enter the agent only through `read_file` in the staged `/workspace`. The
creation API remains deliberately small—one workspace archive plus text prompt—so
there is no second upload protocol or alternate history renderer. The model calls `read_file`
at the point where it needs an image, and that tool result stays at that exact point
in the ongoing conversation.

The one accepted source contract is:

- static PNG with an exact PNG signature and well-formed terminal container;
- eight-bit RGB or explicit RGBA source pixels;
- at most 16,777,216 source pixels and 100 MiB on disk;
- aspect ratio at most 30:1 in either orientation;
- one complete decoder pass before egress, including IDAT validation;
- rejection of `acTL`, `fcTL`, and `fdAT`, palette, grayscale, 16-bit, tRNS,
  orientation metadata other than identity, corrupt data, and trailing bytes.

JPEG, WebP, GIF/APNG, and BMP raster sources are errors. SVG is never accepted as
vision media; its source can be read only as text. Remote/file image URLs and
generated image embeddings are errors. The client sends the original PNG bytes as
`data:image/png;base64,` without resizing, cropping, reorientation, low-detail
selection, or JPEG conversion. The server independently enforces the same source
pixel/aspect/mode contract, composites accepted RGBA deterministically onto pinned
white, and runs the complete BF16 vision tower with the released dynamic-resolution
processor. PDF remains text extraction only; it never turns into an unannounced
lossy image path.

`splitToolMedia=false` and `toolResultContentFormat=parts` are both pinned. A result
such as text → image → text is therefore one `role=tool` message with the same part
order and original tool-call ID. It is not detached, clumped into the newest user
turn, or replayed as a later attachment. Unit tests compare that exact wire shape
and reject non-PNG inline media and every file/remote image reference.

Before compaction, old image-bearing tool results remain at their chronological
positions. vLLM can reuse both the unchanged rendered prefix and the SHA-256-keyed
multimodal processor entry. On compaction, `maxRecentImagesToRetain=0`: old raw
pixels are removed with the compacted history rather than moved into a false recent
turn. The visible summary can preserve findings; the replaced turns' hidden
thinking is not carried into it, because the turns themselves are gone.

The limit is fifteen images in one rendered request, not fifteen over the lifetime
of a session. Their visual expansion counts inside the same native 262,144
total-token window and against the same shares of it as text; the exact tokenizer
reports what a rendered request costs, whatever it is made of.

## Tool-call and streaming correctness

The live model protocol is OpenAI Chat Completions with vLLM's `qwen3_coder` tool
parser and `qwen3` reasoning parser. Tool schemas are strict. Unknown tool names,
extra properties, malformed JSON arguments, unmatched tool results, duplicate IDs,
parallel calls, low/disabled thinking, and incomplete length-stopped calls are not
made executable.

Streaming and non-streaming are different parser paths and are tested as such. The
backend suite cuts real token streams inside tool markers and arguments, compares
stream/batch termination, and verifies that degenerate output never acquires a
successful executable boundary. Complete calls and tool-result continuations round
trip through render → tokenize → parse → history render with the same typed
semantics. Native Responses API streaming/non-streaming tool loops are also covered
for future Codex compatibility, but this service has only the reviewed Qwen Code
mode today.

The client itself emits stream-JSON. The service requires every completed line to
be a JSON object, a first `system/init` event whose Qwen version/model/workspace/tool
metadata exactly matches the deployed contract, a stable session ID, a scope on
every event (`parent_tool_use_id`: null for the main session, the owning `agent`
tool-call id for a subagent), exactly one main-session terminal result as the final
event, an internally consistent main-turn count, and the complete success/error
envelope. A subagent that stops without a report emits its own terminal record under
its tool-call id; that record belongs to the subagent, and it neither ends the
session nor counts toward the session's turns. Zero-usage streaming fragments are
not mistaken for additional turns. Duplicate results, post-result output, a scope
that is neither null nor an agent tool-call id, malformed lines, missing fields, an
empty successful result, and a missing main-session result are hard errors. It never
chooses a convenient-looking “last result.”

Every billed turn carries the usage the backend served for it, copied field for
field: the prompt tokens, the generated tokens, the part of them the backend's
reasoning parser counted as reasoning (`reasoning_output_tokens`), and the prompt
tokens it read back from its prefix cache. The client computes none of these — a
generation that arrives without them fails that request — so a billed event
lacking one, or one whose reasoning exceeds its output, is a stream this service
does not recognise and is refused rather than tallied. The parser sums the served
output and reasoning per scope (`main_output_tokens`, `main_reasoning_tokens` in
`terminal.agent_result`,
and `output_tokens`/`reasoning_tokens` on every subagent scope). A subagent's own
generations reach the stream too: each completed round is written under the
scope's tool-call id as its reasoning, its text and its served usage, so a
subagent's turns are billed to the subagent rather than absent, and a compaction
record (`system`/`compaction`) carries the reasoning the attempt emitted beside
its counts, validated in full. All of it is evidence for the reader; nothing in
it is ever handed back to a model.

The envelope names which terminal state ended the run, and the service carries that
name through to the caller as `terminal.agent_result.agent_result_subtype`. `success` is the agent's
assertion that the model wrote its final message to the end. Seven `error_*`
spellings name the states that stopped a run instead, one name per state:
`error_during_execution` when the run failed on its own terms, `error_timeout`,
`error_max_turns` and `error_max_tool_calls` for the three caller-supplied bounds,
`error_loop_detected` when the loop detector halted a run that had stopped making
progress, `error_incomplete_generation` when the provider stopped the last
generation from outside, and `error_cancelled` for an abort from outside. A state
earns a name when it names an authority other than the run itself that ended the
run, or a bound the caller set and can raise; everything the run did to itself is
`error_during_execution`, told apart by the error message. Every one of the eight
names is a state the client contains a statement to produce, which the patch
asserts before it writes a byte. A subagent's own scoped record carries the error
names from the same list, so a subagent that ran out of turns and one whose tool
threw are distinguishable without reading English; a scope that finished reports
its work rather than a terminal state, so `success` is the session's own. The set
is closed in both directions on the session's record: an error envelope carrying
`success`, or a success envelope carrying an error spelling, is refused rather
than mapped onto a neighbour, and the client's build refuses to produce a name
this list does not contain anywhere. A run that never produced a terminal record
reports no state at all rather than a spelling nothing asserted. Nothing here
judges whether the work was done, which is not decidable from a stream; it
reports the shape of the ending, which is.

Orderly run endings, including turn-budget refusal and cancellation, produce
one of those terminal states. An abrupt process or transport failure can prevent
terminal delivery; a missing record does not certify an ordinary ending. The turn
budget is asked before the turn it decides is counted, so a run it stops reports
exactly the budget it was given as `num_turns` and has interrupted no turn.

Every status read reports the event snapshot it could observe through
`observed_output_tokens`, `observed_reasoning_tokens`, and
`observed_subagent_scope_count`. `observed_unaccounted_records` counts unreadable
records, including incomplete served usage and a nonempty trailing prefix without
a newline. The same observations survive terminal finalization. All four are null
only when terminal storage could not be read; no missing observation becomes zero.
`terminal` is null until an ending exists, and its `agent_result` is null unless
the whole captured stream certifies a complete result. A partial observation is
never that result, even when its unaccounted count is zero. An empty scope table
inside a certified result proves no subagents; an observed scope count of zero
only says none were observed. Reads never cancel or change execution.

Captured JSONL uses LF-committed records with an explicit incomplete tail. A final
prefix cannot certify a result, even if it happens to parse as JSON. Earlier
readable records remain observations. Output callbacks must settle before ordinary
exit; a forced kill can still tear a record on the Unix byte-stream transport.
Capture completion proves the received bytes, not unsent application buffers.
Unsafe or unreadable event storage remains an explicit read error. Canonical
journals used for restoring or mutating history retain strict framing and
ownership checks. See [the session resource contract](docs/session-resource.md).

`terminal.bundle` contains the accepted hash and all measured archive counts
as one object, or is `null` when no archive was accepted. Its
`artifacts_file_count: 0` certifies that an accepted archive contains no artifact
files. The terminal response, process-error decision, exit observations, durations,
retention decision, and teardown diagnostics all live inside `terminal`; a running
session supplies no answers for them. See the [session resource contract](docs/session-resource.md)
for exact fields and reader examples.

Every turn is issued with the same output budget, the window's share, whatever
the conversation has already cost: the trigger holds the history below the size
at which a turn plus its tool results would outgrow the room the summary reserve
keeps free, so the room a turn is given never depends on how full the
conversation is. `error_incomplete_generation` therefore reaches a caller when
the model's own generation was severed at that budget rather than when the
history had eaten it.

## Prefix caching evidence

Prefix caching is enabled in the sole backend command. It is not accepted on faith:
the accepted v13 backend's final text-history probe sent a 65,529-token cold prompt
with zero prefix hits, then reused 64,480 prefix tokens on the warm continuation.
An otherwise equivalent fresh-salt control reused zero tokens. Warm time to first
token was 32.233 times faster than cold, which proves practical reuse rather than
merely proving that a cache flag appeared in the command.

The final chronological image-history probe rendered equivalent OpenAI and
Anthropic histories to exactly the same 16,562 token IDs. A warm continuation reused
14,560 prefix tokens and hit the multimodal cache; its TTFT was 17.089 times faster
than the cold request. Changed image bytes missed the multimodal cache. Moving the
same bytes to another turn retained the multimodal hit but lost the prefix hit,
proving that byte-keyed image preprocessing reuse and chronological prompt-prefix
reuse are separate mechanisms.

The complete v8 Qwen Code acceptance then added 296,939 real prompt tokens across
twenty backend requests. Authoritative vLLM metrics recorded 241,280 local prefix
cache hits, 55,659 locally computed prompt tokens, and three multimodal-cache hits.
These are sampled live runs, so the claim is measured cache reuse and timing
separation—not deterministic prose. Qwen Code's compatibility usage field is not
treated as cache authority; only the backend counters are.

## Network and filesystem isolation

The host is assumed to be reachable from the public Internet on every non-loopback
interface. Neither project uses Docker port publication. The only host TCP listeners
are validated as exactly `127.0.0.1:8000` and `127.0.0.1:8090`, and the only
host-network containers are two tiny fixed ingress relays whose compiled roles bind
those exact addresses and connect to exact Unix sockets.

```text
host 127.0.0.1:8090 -> service ingress -> service Unix socket
                                              ^
                                              |
agent_service: --network none -> service bridge on service-local 127.0.0.1:8090
  |
  | typed, bounded Unix protocol (no raw Docker socket)
  v
Docker broker: --network none; sole holder of /var/run/docker.sock
  |
  +-> agent: --network none; Qwen -> agent-local 127.0.0.1:18000
        +-> session model relay sharing the agent namespace
                |
                v
           central model Unix socket
                ^
                |
vLLM: --network none <- model bridge on vLLM-local 127.0.0.1:8000
                         |
host 127.0.0.1:8000 -> model ingress
```

The agent literally has no non-loopback interface, IP route, DNS, bridge gateway,
published port, host network, or GPU device. Its model relay and stream-capture
sidecar share only the agent's network namespace. The relay accepts the one
agent-local model endpoint and reaches only the central model Unix socket; the
capture sidecar receives only the two exact session streams and owns the output
mount that is deliberately absent from the agent. All relays and capture containers
are non-root, read-only, capability-free, `no-new-privileges`, and bounded by exact
memory and PID limits.

The service is also `--network none`, read-only, uid 1000, and has no Docker socket.
It mounts no host input tree at all: the workspace arrives over the connection
as a hash-committed zip, so the service container sees only the single project
`.runtime` subtree at its exact writable path, its read-only control socket, and
the model relay socket. The separate network-none broker is the only component
with the raw Docker socket; its typed policy accepts only fixed session and
topology operations and validates every image, name, label, mount, namespace,
and lifecycle transition. The submitted archive is structurally proved before
durable acceptance: canonical relative UTF-8 entry names only, directory /
regular-file / symbolic-link entries only, no duplicate or shadowed names, no
entry outside the staging root, and declared totals within the exact caps —
more than 200,000 regular files, 250,000 entries, or 200 GiB of content is
rejected before anything stages. Only the outermost archive is extracted; an
archive inside the workspace stays an ordinary staged file.

The copied repository may still contain ordinary `QWEN.md` and `AGENTS.md` files;
those remain task-level project guidance. They cannot create a second runtime
configuration. `QWEN_HOME` is the read-only `/opt/agent`, and there is no writable
`/qwen-home` mount or host Qwen state. In the mandatory foreground-agent mode, Qwen
Code does not load workspace `.qwen/settings.json`, workspace `.env`, project
hooks/extensions/skills, `.qwen/rules`, `.qwen/output-language.md`, `.mcp.json`,
`--mcp-config`, or session-injected MCP servers. It also disables managed memory,
auto-memory/dream, team memory/synchronization, auto-skill, custom slash commands,
workflows, include directories, and permission-rule persistence. A leading slash in
the submitted prompt is ordinary task text, and init metadata must report
`slash_commands: []`. These files remain ordinary copied source files and may be
inspected when relevant to the task, but they cannot replace the pinned model,
xhigh thinking, sampling tuple, tokenizer path, tool allowlist, or network boundary.
Source-level tests cover these isolation invariants; live acceptance also uses a
deliberately contradictory workspace configuration.

The submitted archive is extracted; the caller's original tree is never seen,
let alone mutated. Executable semantics are preserved while dangerous mode bits
are stripped, and symbolic-link entries stage as opaque links that resolve only
inside the agent's isolated mount namespace. The agent modifies `/workspace`;
the final workspace, `/artifacts`, prompt record, ready record, complete events,
stderr, exit code, and final response are placed in a deterministic `bundle.tar.zst`.
Missing bundle entries, symlinks, read races, or tar errors are process failures.
There is no `--ignore-failed-read` path.

Cancellation is safe at the readiness boundary as well as during later tool use.
The wrapper installs signal handlers and creates all required output sidecars before
publishing `ready.json`, launches Qwen in a dedicated `setsid` process group, and
forwards termination to that entire group. The requested exit code is synchronously
recorded before forwarding, so cancellation cannot silently lose its bundle if a
descendant delays shutdown. `util-linux=2.39.3-9ubuntu6.5`, which supplies `setsid`,
is an explicit image input rather than an incidental base-image dependency.

Production benchmarking exposed and repaired a separate late-subscriber race at
the capture boundary. The broker formerly followed Docker logs with `--since 0s`.
Docker interprets that spelling as relative "from now", so a fast capture sidecar
could emit `CAPTURE_COMPLETE` before the follower attached and the completion event
would never be replayed. The service correctly refused to promote output whose
capture completion was unproved, but the session could wait indefinitely. The sole
release now follows from exact Unix epoch `--since 0`; a unit test freezes that
argument and proves that late subscribers replay already-emitted completion events.
A real five-turn production smoke and both long production benchmark sessions then
completed with clean bundles and no session-container leftovers.

### What temporary means

Temporary is a lifecycle and ownership property, not a storage-medium promise.
Every session container, staged workspace, scratch tree, runtime tree, and stream is
freshly created for exactly one session. A name or directory collision is an error;
the service never adopts stale state or guesses that an earlier tree is safe to
reuse. Session containers and raw state are removed only after producers are
quiescent, required streams and sidecars are captured, the deterministic bundle is
complete, and terminal state is durable. If capture or bundling fails, the raw tree
and diagnostics are retained as failure evidence instead of being silently erased.

This does not impose a blanket RAM-only filesystem. The bounded `/tmp` and
`/qwen-runtime` tmpfs mounts remain because they are useful private scratch/runtime
boundaries, while staged workspaces, service state, terminal records, and bundles
may use ordinary Docker or host-backed project storage. Docker images are durable,
immutable, pinned deployment artifacts—not temporary session state. Completed
result bundles persist until the operator explicitly deletes that one terminal
record through the API. The security properties come from fresh ownership,
namespace and mount isolation, explicit retention, and exact teardown; they do not
depend on pretending that RAM versus SSD determines whether state is temporary.

## Reproducibility and real version pinning

The build is split by ownership. Everything globally known — the pinned Ubuntu
package sets, the language toolchains and the Node dependency bytes — lives in two
generic base images built by
[`docker/Dockerfile.base`](docker/Dockerfile.base), and everything that is ours —
the patched Qwen source, our logic, configuration and contracts — is built from
them by [`docker/Dockerfile`](docker/Dockerfile). The split is not a convenience:
the two concerns were previously one `RUN`, so a layer of pinned third-party
packages could not be separated from the account our code runs as.

[`scripts/build-base-images.sh`](scripts/build-base-images.sh) is the only thing in
this repository that fetches from the network, and it runs only when a pin moves.
`./build.sh` reaches the network for nothing. Ubuntu packages come from the
timestamped `20260814T120000Z` snapshot with an exact version for every requested
package; the initial TLS bootstrap authenticates repository metadata by Ubuntu's
signed `InRelease`, and after the exact CA package is installed the snapshot is
fetched again with ordinary certificate verification. Qwen, Go and Docker CLI
remote archives use BuildKit `ADD --checksum=sha256:...` and are hashed again
inside the build. The base images are digest-pinned linux/amd64 Ubuntu; our own
Rust stages are digest-pinned Rust.

Both bases are pinned by image ID under `.build.base` in
[`config/stack.lock.json`](config/stack.lock.json), and `./build.sh` refuses to
build against any other — no mutable tag is accepted and nothing is pulled. That
makes reproducibility stronger rather than weaker: our images' inputs are now a
pinned base ID plus our own sources, instead of a base plus several hundred
package fetches trusted to keep returning the same bytes. The bases travel to
another machine inside the release archive, never by a rebuild, because images do
not reproduce across hosts.

There are two bases rather than one because the two package sets are not one
thing: the toolchain set has 65 packages and the runtime set has 5, all 5 of which
the toolchain set also contains. Merging them would put 60 agent-only packages
into the service image — the component that holds the Docker socket path and
serves the API — to save a single artifact. They are siblings of the same pinned
Ubuntu rather than a chain, because they set up uid 1000 incompatibly: a login
shell for the agent, none plus Docker group membership for the service.

The Node dependency bytes are cached in the toolchain base, resolved from the
unmodified upstream lockfile, and our image installs offline against them. Our
patch does rewrite `package-lock.json`, but only its classification — both
lockfiles resolve the same 2348 packages at the same versions — so the cache is an
exact match for what an install of the patched tree needs rather than a superset
hoping to cover it. The installed `node_modules` tree is not cached: our patch and
`patch-package` both rewrite it, so it is rebuilt every time.
[`docker/scripts/npm_lock_package_set.py`](docker/scripts/npm_lock_package_set.py)
reduces a lockfile to the set of tarballs it requires, and the build compares the
patched lockfile's set against the set the base recorded. A lockfile that asks for
a package the base does not carry is refused by name; nothing is fetched to make up
the difference.
The Dockerfile frontend itself is digest-pinned. The stack lock records
`SOURCE_DATE_EPOCH=1786725153`, the exact upstream Qwen commit timestamp, and the
build passes it to BuildKit. Images are exported through a temporary Docker archive
with `rewrite-timestamp=true` and only then loaded into the local daemon; plain
`--load` is intentionally not used because it fixes image metadata but leaves
build-time file timestamps inside layers.

Reproducibility also requires removing or normalizing content that embeds wall-clock
time. The build removes Vitest and Node compile caches, canonicalizes the Git-LFS
system configuration, regenerates font caches from epoch-normalized font metadata,
and removes package-manager logs and auxiliary caches. Java's JKS trust store embeds
an eight-byte creation timestamp in every entry, so the hashed, fail-closed
[`normalize_jks.py`](docker/scripts/normalize_jks.py) validates the existing JKS
integrity digest, parses the complete version-2 structure, changes only those entry
timestamps, recalculates and verifies the integrity digest, preserves file ownership
and mode, and proves idempotence. Its focused tests run inside the pinned Docker
build before it is used on the real 121-certificate trust store. Two independent
`--no-cache` agent builds must produce the exact locked image ID; a cached rebuild
alone is not accepted as reproducibility evidence.

Only the broker image contains Docker CLI 29.7.2 from the exact official static
archive, not the host binary; the service image contains no Docker client and mounts
no Docker socket. The Rust binaries are built by the pinned Rust 1.95.0 image with
`cargo test --locked` and `cargo build --locked --release`. The agent image is sealed
by exact image ID plus labels for the upstream version/commit/archive/patch and every
locked runtime/configuration identity. The service image is sealed by committed
source, build-input, stack-lock, and Cargo-lock labels.

The current agent image is the one
[`config/release.lock.json`](config/release.lock.json) names in `.images.agent`.
Its build reconstructed exactly 104 changed/new files from pristine upstream
and passed 5,259 tests across fifty-seven focused test files. The sealed runtime
reports `0.21.12`, embeds commit `b965d5f8c24f`, and carries matching archive,
review-diff, semantic-manifest, settings, instruction, and wrapper labels. The
build script treats any other image ID as drift. The image also carries Qwen
Code's exact upstream Apache-2.0 license plus this repository's Unlicense and
third-party scope notice; those documentation files do not alter the executable
client or its locked behavior.
The pinned Rust 1.95.0 stages passed all 117 tests: one hundred service,
nine broker, three fixed-relay, two stream-capture, and three agent-exec tests. A
full clean no-cache release build reproduced the exact locked agent, relay, capture,
broker, and service image IDs; candidate build directories were absent afterward.
[`config/release.lock.json`](config/release.lock.json) pins the implementation
commit, the build-input manifest hash, the stack-lock hash, and all five
resulting image IDs, and the commit that writes it freezes that complete
set. It is the only place those identities are stated, so there is no second
copy to disagree with it.

Pinning is not a claim that the upstream dependency graph has no security debt. The
Qwen `npm ci` build currently reports 68 audit advisories (3 low, 35 moderate, 27
high, 3 critical). The build records that fact and does not run `npm audit fix`:
doing so would introduce an unreviewed mutable dependency graph. Remediation means
reviewing a newer exact upstream tree or a narrow explicit patch, then rebuilding
and re-running every gate.

## Licensing and third-party scope

Original material in this repository for which the repository author owns the
copyright is released under [The Unlicense](LICENSE), SPDX identifier
`Unlicense`. Qwen Code and any review patch or generated transformation containing
its source remain under Qwen Code's upstream Apache-2.0 terms. The paired Qwen
model and vLLM backend retain their own upstream terms as well.

This scope is intentional: The Unlicense makes the original agent-service work
freely reusable without falsely claiming the right to dedicate Alibaba, QwenLM,
vLLM, package-author, or container-vendor material to the public domain. The exact
boundary and preserved Apache-2.0 text are in
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) and
[LICENSES/Apache-2.0.txt](LICENSES/Apache-2.0.txt).

Putting a version string in a README is not considered a pin. The scripts require a
clean repository and validate required host tools, isolation features, file hashes, image IDs,
labels, Docker modes, listener addresses, backend command, endpoint identities, real
tokenizer, model manifest, and health before reporting ready. Unexpected environment
variables beginning with `AGENT_SERVICE_` or `OPENAI_` are rejected rather than
silently changing behavior.

## Operation

No script accepts a profile, port, model, or tuning argument.

Build from committed inputs:

```bash
cd /home/user/Desktop/agent_service
./build.sh
```

`./build.sh` verifies; it never advances a pin. It rebuilds every image of ours
from the committed tree and refuses if any image ID is not the one the lock
records, so any checkout can be proved to produce exactly the images it claims.
Every layer still rebuilds on every run — `--no-cache` is unchanged, because
avoiding rebuilds was never the point — but no layer fetches.

The two generic base images are a separate, rare operation, needed only when a
pin in the stack lock moves:

```bash
./scripts/build-base-images.sh
```

It reports the image IDs it produced; record them under `.build.base` in
[`config/stack.lock.json`](config/stack.lock.json). Nothing is adopted
automatically, and `./build.sh` refuses any base but the pinned one.

Cutting a new release — changing anything the images are built from — is
`./release.sh`, with the stack stopped:

```bash
./stop.sh
./release.sh
```

It is one command because the components form a chain: the agent image ID is
compiled into the typed broker policy, that policy is compiled into the broker
and the service, and the stack lock is compiled into the service, so moving one
moves the next. It builds, adopts whichever image ID moved, commits, and repeats
until `./build.sh` agrees, then proves the release lock names a commit that
already contains the tree it records. It terminates because the service image ID
is recorded only in `config/release.lock.json`, which is excluded from the
build-input manifest, so adopting it changes no build input.

Nothing in that sequence is a manual step or a remembered exception. In
particular the release lock advances its `implementation_commit` only when the
build-input manifest actually changed, which is what keeps the service image's
baked `SOURCE_COMMIT` describing its own tree; that falls out of the rule rather
than being a special case anyone has to know. `scripts/test-release.sh` proves
the pin locations, the refusal to rewrite an ambiguous value, and the
termination condition, and it runs as part of every `./build.sh`.

Start both the pinned backend (if absent) and the agent service:

```bash
./start.sh
```

Check every live invariant:

```bash
./status.sh
```

Run one task and wait through a server-side notification, without client polling:

```bash
./run.sh /home/user/Desktop/my_project /home/user/Desktop/task-prompt.txt
```

The optional creation-body field is a named option, and omitting it selects
the deployment default rather than sending it explicitly:

```bash
./run.sh /home/user/Desktop/my_project /home/user/Desktop/task-prompt.txt \
  --max-session-turns=700
```

Cancel a known session:

```bash
./cancel.sh s-0123456789abcdef0123456789abcdef
```

Tear the service and backend down. `docker stop --timeout -1` allows the Rust service
to cancel the current session, archive what exists, remove its exact containers and
socket, and persist the terminal record before the service exits:

```bash
./stop.sh
```

Result records remain in `.runtime/results/<session-id>/` until explicitly deleted
through the API. Startup removes only labelled orphan containers, abandoned staging
trees, and incomplete result directories. It never prunes completed sessions by age
or count.

## HTTP API

All endpoints listen only on `127.0.0.1:8090`:

| Method | Path | Meaning |
|---|---|---|
| `GET` | `/healthz` | Process and startup preflight succeeded |
| `POST` | `/v1/agent/sessions` | Stream the workspace archive, prove its commitment, durably accept, return immediately |
| `GET` | `/v1/agent/sessions` | List current sessions and separately identified uninterpreted terminal records |
| `GET` | `/v1/agent/sessions/{id}` | Pure state read: status, archive commitment, complete progress history |
| `GET` | `/v1/agent/sessions/{id}/bundle` | Stream the exact terminal `bundle.tar.zst` with declared length and `X-Bundle-SHA256` |
| `POST` | `/v1/agent/sessions/{id}/cancel` | Durably record cancellation; teardown continues under the supervisor |
| `DELETE` | `/v1/agent/sessions/{id}` | Delete one terminal record and bundle |

Committed JSON evidence that the current terminal schema cannot interpret is
preserved unchanged through startup, together with its result and raw-state trees.
It grants no recovery or cleanup authority. The list response contains `sessions`
and `uninterpreted_records`; the latter names each retained session and the schema
refusal, without inventing current outcomes or counts. Individual resource
operations return HTTP 409 `uninterpreted_terminal_record`. Current-schema
validation remains strict; invalid syntax, unsafe metadata and contradictory
current evidence retain their own refusal semantics. See the
[session resource contract](docs/session-resource.md).

The creation body is exactly two ordered `multipart/form-data` parts: part 1
`request` (`application/json` — `{"prompt", "max_session_turns"?,
"archive_bytes", "archive_sha256"}`, at most 2 MiB) and part 2 `archive`
(`application/zip` — the exact workspace bytes, streamed to a disk spool while
hashed, bounded only by the explicit 200 GiB + container-overhead archive cap).
A required caller-generated 256-bit `Idempotency-Key` names the operation. The
optional field is typed, not a profile name: `max_session_turns` is a JSON
integer in `1..=2000`. Omitting it selects the locked default of 400 turns; a
bad value is refused by name before the archive part is spooled, and a replay
under the same handle must repeat the same values or it is a 409 rather than a
second operation.
There is no serving-capacity gate: sessions run concurrently, each in its own
isolated topology, because whether more than one should run at once is a
placement decision for whatever sits above this service. Concurrent sessions
against one deployed backend interleave their model turns through its queue
and compete for its prefix cache; that is a throughput property of the
chosen backend, not a correctness property of this service.
Acceptance requires the streamed bytes to equal the declared count and SHA-256
exactly, so a reset or truncation can never masquerade as success; replaying
the identical receipt is a pure lookup, and every session read echoes the
accepted archive commitment. There is no waiting endpoint: the operation never
belongs to a connection, and callers poll the monotonic `progress_revision` /
`progress_events` on the ordinary session read. The prompt cap is 1 MiB;
prompt bytes enter Qwen through text stdin, not a shell argument, so Linux's
per-argument limit does not invalidate the API contract or expose the prompt
in a process listing.

All session endpoints return the [same evidence contract](docs/session-resource.md).
`session.sh` validates its live/terminal distinction before displaying the JSON;
`bundle.sh` requires an accepted `terminal.bundle` and verifies the downloaded bytes
against both its hash/size and the transport header.

Terminal persistence is atomic and no-clobber (`create_new`, write, `fsync`,
same-directory hard-link publication, directory `fsync`). If persistence fails,
the service retains the complete terminal body in
memory and marks it erroneous rather than evicting the only copy and returning a
misleading 404. Shutdown has no arbitrary teardown deadline.

## Acceptance gates

A release is not complete merely because the images build. Every required gate below
passed against the current pinned agent release and the exact live v21 corrected
backend; unchanged historical cache measurements are identified as such:

1. strict JSON, shell syntax, formatting, locked Cargo build, and Rust tests;
2. clean Qwen archive extraction, semantic drift/idempotence/rollback checks,
   transactional source transformation, review-diff equivalence, full patched
   build, and all 5,259 assertions in fifty-seven focused test files;
3. independent no-cache reproduction of all five locked images, exact label/hash/
   version checks, and network-none route proofs;
4. exact live backend container/image/user/labels/command/mounts/cache/listener/
   version/model/tokenizer plus bridge and ingress identity;
5. sealed model path from agent-local loopback through the central Unix socket,
   with no IP route, DNS, host network, or GPU;
6. real native tool call and typed tool-result continuation through Qwen Code;
7. strict stream-JSON result capture and deterministic complete bundle;
8. malformed/duplicate/post-terminal stream failures;
9. complete and truncated tool-call behavior for streaming and non-streaming server
   paths;
10. original-PNG tool reads, non-PNG rejection, chronological tool-image history,
    and no image detachment to a recent user turn;
11. repeated text/image agentic turns showing actual cached prompt tokens,
    multimodal cache hits, and materially lower time-to-first-token;
12. native-context boundary, fifteen-image/full-pixel vision proof, and K8V4 memory
    evidence inherited from the exact live backend image;
13. typed broker authority, stream-capture isolation, effect journals, PTY sandbox,
    graceful cancellation, unlimited-wait shutdown, orphan sweep, and exact
    loopback-listener audit.

The main hostile-workspace acceptance deliberately supplied contradictory `.env`,
`.qwen/settings.json`, MCP, hook, rule, skill, memory, output-language, and custom
slash-command fixtures. Init advertised exactly the ten allowed native tools,
`general-purpose` and `Explore`, no MCP servers, and no slash commands. Ordinary
project `QWEN.md` and `AGENTS.md` guidance remained active. Native text, original-PNG
vision, and shell calls all returned correlated typed results. Session
`s-f75db8944f10454099d4305a7644e485` completed seven turns, read the exact visual
code `VISION_AGENT_PTY_4827` from the original 4,096-by-4,096 PNG, emitted the exact
shell marker `QWEN38_AGENT_ISOLATION_OK`, and proved the hostile environment variable
was absent after the later authentication-validation path. The five hostile marker
files remained absent, both project-guidance files remained active, the original
fixture hashes were unchanged, and the nineteen-file result bundle was complete.

Session `s-541652f64dfc40fc9411f5ddaa4c6a37` separately exercised the real PTY shell
path in four turns and returned `QWEN38_PTY_SHELL_OK` plus the byte-exact staged
output. Session `s-93c2a6fd58744f0d8a32f437edefc611` invoked exactly one foreground
`Explore` subagent, correlated parent/child tool IDs, used native list/read and a
shell byte check, and returned to an independent main-thread reread. Explore was not
mechanically made read-only: it retained its real conversion and scratch tools, while
the trusted workspace/artifact effect journal proved zero effects for this read-only
assignment.

Finally, session `s-cd0e7f510def456c8f0c2a1eddf36563` was cancelled immediately
after published readiness. The acknowledgement returned in 785 ms; terminal teardown
completed in 1,863 ms with zero model turns, exit 143, an empty event stream, and a
complete nine-file bundle. It reported cancellation before a terminal event instead
of fabricating success, emitted no forbidden marker, and left no session container.
All successful sessions likewise left no agent, model-relay, or capture container.
Script argument-rejection probes also proved that start, status, stop, and build
reject an alternate mode before changing state.

## Production SWE-rebench pilot

The final pilot ran through this production service itself. It did not use a
Harbor/Qwen model adapter and did not call vLLM directly: the harness submitted each
task with `POST /v1/agent/sessions`, awaited the production `/wait` notification,
required the clean terminal bundle, and only then ran the pinned SWE-rebench
evaluator with no network.

On task `Gentleman-Programming__gentle-ai-595`, the production session resolved
all 11 evaluator checks in 61 turns. One completed task is a lifecycle proof for
the deployed pair — real session creation, real turns, a clean terminal bundle,
and an independent evaluator pass — not a benchmark-suite score.

The first two attempts are retained and classified as infrastructure failures, not
model scores. The second is the run that exposed the `--since 0s` capture race above.
The accepted pilot's release commit and agent, broker, and service image IDs,
with exact methodology, limitations, hashes, results, and replay instructions, are in
[`docs/production-swe-rebench-pilot.md`](docs/production-swe-rebench-pilot.md).

Any future changed input must rerun the affected gates. There is no fallback
declaration of success.
