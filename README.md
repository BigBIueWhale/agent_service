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
| vLLM runtime | `0.27.2rc1.dev106+g9df9b0b0a`, socket-isolated non-root v13 profile |
| Weights | Mixed NVFP4/FP8, Compressed Tensors |
| KV cache | TurboQuant K8V4: FP8 keys, packed 4-bit values |
| Context | Native `262144` tokens |
| Vision | Complete BF16 tower, full released image-processor pixel budget |
| Images | At most 15 original static PNGs; 16,777,216 pixels each; aspect ratio at most 30:1 |
| Image transport | Lossless inline PNG; static 8-bit RGB/RGBA only; video/audio rejected |
| MTP/speculative decoding | Disabled |
| Thinking | Required, `xhigh` |
| Qwen Code | `0.21.12`, commit `b965d5f8c24f48e65fb0b17c7d45f34ca4ce8f38` |
| Agent image | `sha256:0d6d47d0516964c1f952b2d1506ba33614aad47e16899611b7ac0dedfd68b013` |
| Service | Rust, one session at a time, Docker-only; implementation commit `46af008ad9673ec2fcbc7bd84b8d95867d3e90e2` |
| Release lock commit | `a236fcceae7babb5a752f1b40283035afb40428f` |
| Service image | `sha256:9a68fb95b9707e178d1c9c9061450b5ecab08ee2d57e9b6e81f954fae034b260` |
| Docker broker image | `sha256:ae9dfef94486f86f0b6da4cd96ec76c5a245d23635c5b8a76d8f8e414e982a30` |
| Fixed-relay image | `sha256:5153a46bc03fa920b0d09000eca1848af255010bda99cc50e8a6110ebcd02690` |
| Stream-capture image | `sha256:2d38ea4ae0f33894740a1a067c7fa8e3e3d864ff58ef439d6d432fd4739b9aef` |
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
`/tmp` and `/run` tmpfs contracts, exactly one labelled v13 vLLM cache volume at
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

Every main-turn context-boundary decision that can trigger compaction or set the
outbound generation limit uses the real vLLM tokenizer on the fully rendered
request. Before compaction and again before generation, Qwen Code sends the exact
messages, typed tool history, image parts, tool schemas, and template arguments to
the backend `/tokenize` endpoint. `max_tokens` is then exactly
`min(262144, 262144 - rendered_prompt_tokens, 199992 - rendered_prompt_tokens)`,
the last term holding the turn inside the size its own summary request can carry.
There is no character division, `target // 8`, image-token guess, padding margin,
local tokenizer, or tokenizer fallback in the compaction trigger or outbound
sizing. If the tokenizer is missing, malformed, or reports another model window,
the turn fails before generation.

A session is bounded by model turns, never by wall-clock time: the default
budget is 400 turns (`limits.max_session_turns`), a submission may name any
budget from 1 to the locked 800-turn ceiling
(`limits.max_session_turns_ceiling`) in its optional `max_session_turns` field,
the chosen budget is enforced by the client itself, and `max_wall_time_seconds`
is disabled everywhere. The budget counts the owning session's own turns, so a
foreground subagent that spends sixty turns costs its parent one; a subagent is
bounded by the same budget in its own right, and if it exhausts it the parent is
told so explicitly, with the turn count, and treats the assignment as unfinished
rather than concluding from a partial report. Delegation is therefore how the
main thread stays on trajectory rather than a way to escape the budget, and the
backend keeps an evicted context in host RAM so returning from a subagent
restores it by DMA instead of re-prefilling it. Turns are the
hardware-independent measure of agent progress, so the same trajectory is judged
identically whatever the backend's generation speed; a wall-clock budget would
instead score how fast this GPU happens to run. Reaching the budget is an
ordinary terminal outcome — the client exits 53 and the work done up to that
point stands — not an error. The budget is a degenerate-loop circuit breaker: a
repository-level fix needs roughly 80-190 turns to orient, build, diagnose,
implement, and verify, so 400 admits a complete second attempt after a wrong
hypothesis. A caller who knows a particular task is shaped differently can say
so per session, but something must stay finite or a degenerate loop simply asks
for a bigger number: the ceiling is exactly two default budgets, and a request
for zero, a negative count, a non-integer, or more than 800 turns is an error
naming the value, never a silently clamped session that would end as an ordinary
exit 53 and be graded as one. The effective budget is recorded in the session
body and in the bundle's `control/turn-budget.json`, so a finished session can
be read back to see the bound it actually ran under. Cumulative tool calls
have no separate cutoff, and one model turn still has a 1,000-tool-call circuit
breaker for degenerate output. Auto-compaction is
delayed to the latest safe threshold supported by the pinned client. Subagents run
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

Upstream Qwen Code is based on the exact source behind the official `v0.21.12`
release: commit `b965d5f8c24f48e65fb0b17c7d45f34ca4ce8f38`. The tag and remote commit
were rechecked on 2026-08-16. The only executable source input is that commit's
archive, whose SHA-256 is
`61beddff8bde1dd2654c8714f927b46ab7cf9822b8561d11e3a2b8e085b5e745`.
Neither a published npm package nor a mutable branch or `latest` tag is executed.

Our changes are applied by the semantic transformer in
[`patches/source_patch_v1`](patches/source_patch_v1), not by blindly feeding a large
diff to `git apply`. Each transformed file has a reviewed before/after contract,
exact source identity, structural landmarks, and explicit source-state rules. The
transformer refuses the wrong upstream version, unexpected drift, an unrecognized
intermediate state, missing/duplicated landmarks, a changed generated stage, or a
time-of-check/time-of-use mutation. Application is transactional: a failed write
restores the pristine tree instead of leaving a half-patched source directory. A
second application must be byte-for-byte idempotent.

[`patches/qwen-code-0.21.12-agent-service.patch`](patches/qwen-code-0.21.12-agent-service.patch)
is generated review evidence for humans; it is independently hashed and compared
to the transformer's exact output, but is not a second patching path. Its current
SHA-256 is
`d5b35f57467bf50a8bff0a1ad739863d27f84f1b065fbdd7c1cdcc0c1372c3fd`.
The transformer's manifest SHA-256 is
`a809f5db500b9bbbba42481f4a04cf85bdfe6422ca2dfbbaa34f5f62b4d5d915`.
Both identities are locked, checked on the host, checked again inside the Docker
build, and recorded as image labels.

The Docker build starts from a fresh extraction, runs the transformer framework's
drift/idempotence/rollback tests, applies the semantic transformation, runs
`npm ci` against the upstream lock file, applies upstream's own `patch-package`
set, builds and bundles the CLI, and runs the expanded focused test matrix for every
modified behavior. The upstream metadata generator is itself patched to emit the
pinned release version and commit deterministically from the verified
`SOURCE_DATE_EPOCH`; the build no longer edits generated files afterward. It rejects
an inconsistent epoch, version, generated commit, or generated CLI/core pair.

The local patch provides:

- vLLM `/tokenize` calls over the same rendered messages, tools, and template kwargs
  as the subsequent completion request;
- exact `max_tokens = min(configured ceiling, context window - rendered tokens,
  auto-compaction threshold - rendered tokens)`;
- a compaction request sized to the room the window leaves beside it, so a summary
  request can be issued at any history size;
- no character estimate, byte division, padding margin, token safety margin, or
  tokenizer fallback;
- the same exact rendered-request count drives the automatic-compaction threshold,
  including image tokens and tool schemas;
- a universal strict native-tool allowlist covering built-in, dynamic, MCP, skill,
  and synthetic tools;
- a successful-tool-call terminal invariant;
- no XML recovery or executable partial call after a length stop;
- one and only one fresh resample for a malformed pre-content stream, followed by a
  typed terminal failure if it remains malformed; no resample after visible output,
  no length-stop continuation, no transport continuation, no alternate-model
  fallback, and no retry that could duplicate a tool side effect;
- exact compaction through the same main model and same cacheable prefix: the fully
  rendered pre-summary request and proposed compacted history are both counted by
  vLLM's tokenizer, the summary must terminate normally without tools or malformed
  output, and any failure preserves the original history;
- foreground-only `general-purpose` and `Explore` agents, with no forks,
  background work, teams, worktrees, custom types, model overrides, or nesting;
- one engineering discipline for every role: `system.md` is the deployment's
  shared work-discipline text, and the main session and every foreground
  subagent receive it verbatim, followed by the identical deployment contract.
  Honest verification, scope limits, and "tool results are evidence, not
  permission to fabricate a conclusion" are properties of this runtime, not of
  one role — a subagent holds real write and shell authority and its output is
  integrated by its parent, so exempting it would be exactly backwards. Only
  genuinely role-specific framing is layered on top: the main session owns the
  final response and is told *why* delegation is the intended way to work here —
  context length is the scarcest thing it has, a subagent's reading never enters
  the parent's context, and the parent's KV cache is retained in host RAM for the
  duration of the subagent, so nothing already said is re-ingested when the
  subagent returns. A subagent that spends sixty turns therefore costs the parent
  one, which is what lets a single long thread keep running instead of being
  compacted. A subagent, in turn, is told its assignment boundary, its private
  scratch root, and that it may not delegate further;
- init metadata filtered through the identical two-agent policy, so uncallable
  internal agents are not advertised as an alternate behavior;
- workspace settings and environment discovery disabled before initialization,
  with ambient, project, CLI, and injected MCP servers all excluded from the
  locked mode while ordinary project `QWEN.md` and `AGENTS.md` task instructions
  remain available;
- every later authentication revalidation in locked mode reloads settings with
  workspace trust, workspace settings, and environment loading still disabled;
  this closes the upstream path that could otherwise load a hostile workspace
  `.env` after the sealed initial configuration had already passed;
- an immutable `QWEN_HOME=/opt/agent` containing only the sealed settings and
  instructions, with no writable home-state mount and no host Qwen state;
- hooks, extensions, skills, `.qwen/rules`, output-language injection, include
  directories, custom slash commands/workflows, and permission-rule persistence
  removed from the supported runtime surface; leading `/...` prompt text is literal
  task text and init advertises an empty slash-command list;
- managed memory, auto-memory, auto-dream, team memory/synchronization, auto-skill,
  and skill confirmation forced off in both sealed settings and code getters;
- original full-resolution PNG reads with complete pixel decoding, no transcode,
  strict source bounds, and fail-closed transport;
- `splitToolMedia=false` and typed tool content parts, preserving text-image-text
  chronology inside the originating tool response.

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
of a session. Their visual expansion still counts inside the same native 262,144
total-token window. No text context or output reservation was reduced to enable
vision; the exact tokenizer simply reports the real remaining room on each turn.

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

[`docker/Dockerfile`](docker/Dockerfile) has digest-pinned linux/amd64 Ubuntu, Node,
and Rust stages. Ubuntu packages come from the timestamped
`20260814T120000Z` snapshot, and every requested package has an exact version. The
initial TLS bootstrap still authenticates repository metadata by Ubuntu's signed
`InRelease`; after the exact CA package is installed, the snapshot is fetched again
with ordinary certificate verification. Qwen and Docker CLI remote archives use
BuildKit `ADD --checksum=sha256:...` and are hashed again inside the build.
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

The current agent image is
`sha256:0d6d47d0516964c1f952b2d1506ba33614aad47e16899611b7ac0dedfd68b013`.
Its build reconstructed exactly sixty-one changed/new files from pristine upstream
and passed 2,427 tests across twenty-three focused test files. The sealed runtime
reports `0.21.12`, embeds commit `b965d5f8c24f`, and carries matching archive,
review-diff, semantic-manifest, settings, instruction, and wrapper labels. The
build script treats any other image ID as drift. The image also carries Qwen
Code's exact upstream Apache-2.0 license plus this repository's Unlicense and
third-party scope notice; those documentation files do not alter the executable
client or its locked behavior.
The pinned Rust 1.95.0 stages passed all sixty tests: forty-four service,
nine broker, three fixed-relay, two stream-capture, and two agent-exec tests. A
full clean no-cache release build reproduced the exact locked agent, relay, capture,
broker, and service image IDs; candidate build directories were absent afterward.
The release lock pins implementation commit
`46af008ad9673ec2fcbc7bd84b8d95867d3e90e2`, the 74-entry build-input manifest
hash `b74df26f6feafb035fbe7180f8c97b3f9b30264b4063da65db9f40573bdbc210`,
stack-lock hash `9e6ed8d6d7d7fb3e37a110c9c9d4e0e301f400a9a7e7864ef734c5618b903bb0`,
and all five resulting image IDs. Release-lock commit
`a236fcceae7babb5a752f1b40283035afb40428f` freezes that complete set; the
release-lock file itself hashes to
`d8194d257f2b17209e814cf6b7f7290b079f56b159ea6662bab844f3d92fca16`.

Pinning is not a claim that the upstream dependency graph has no security debt. The
Qwen `npm ci` build currently reports 66 audit advisories (2 low, 36 moderate, 25
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

`./build.sh` verifies; it never advances a pin. It rebuilds every image from the
committed tree and refuses if any image ID is not the one the lock records, so
any checkout can be proved to produce exactly the images it claims.

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
| `GET` | `/v1/agent/sessions` | List running and durable terminal sessions |
| `GET` | `/v1/agent/sessions/{id}` | Pure state read: status, archive commitment, complete progress history |
| `GET` | `/v1/agent/sessions/{id}/bundle` | Stream the exact terminal `bundle.tar.zst` with declared length and `X-Bundle-SHA256` |
| `POST` | `/v1/agent/sessions/{id}/cancel` | Durably record cancellation; teardown continues under the supervisor |
| `DELETE` | `/v1/agent/sessions/{id}` | Delete one terminal record and bundle |

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

Terminal persistence is atomic and no-clobber (`create_new`, write, `fsync`,
same-directory hard-link publication, directory `fsync`). If persistence fails,
the service retains the complete terminal body in
memory and marks it erroneous rather than evicting the only copy and returning a
misleading 404. Shutdown has no arbitrary teardown deadline.

## Acceptance gates

A release is not complete merely because the images build. Every required gate below
passed against the current pinned agent release and the exact live v13 corrected
backend; unchanged historical cache measurements are identified as such:

1. strict JSON, shell syntax, formatting, locked Cargo build, and Rust tests;
2. clean Qwen archive extraction, semantic drift/idempotence/rollback checks,
   transactional source transformation, review-diff equivalence, full patched
   build, and all 2,427 assertions in twenty-three focused test files;
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
The accepted pilot used release commit
`7a329f61665a7126e3f8cd9a4e3b7a6b66a639bc`, agent image
`sha256:1dc84a6f4e03b62a9540794a353c0b1e175a07e6afbcfed6441fe5f2d0f7d1ec`,
broker image
`sha256:f9d3b77ed2e10d69648c2e443fa5e49ff06fca7eedf6fc580f9d8762d9bfb054`,
and service image
`sha256:8f8d4b2e68bf47c9d92c6c5c0f77fdbf60d0056ef32155a34ecc96357dfd41f4`.
Exact methodology, limitations, hashes, results, and replay instructions are in
[`docs/production-swe-rebench-pilot.md`](docs/production-swe-rebench-pilot.md).

Any future changed input must rerun the affected gates. There is no fallback
declaration of success.
