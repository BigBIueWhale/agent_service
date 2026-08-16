# Qwen3.8 local agent service

This repository is the one supported local-agent experience for this workstation.
It runs a pinned, patched Qwen Code client in an offline Docker container against
the already deployed, corrected Qwen3.8-27B NVFP4 backend. It is intentionally a
singleton: one long-lived main agent thread, optional sequential foreground
subagents, one model, one sampling policy, one context policy, one network path,
and one set of scripts.

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
| vLLM runtime | `0.27.2rc1.dev106+g9df9b0b0a`, immutable-root v12 profile |
| Weights | Mixed NVFP4/FP8, Compressed Tensors |
| KV cache | TurboQuant K8V4: FP8 keys, packed 4-bit values |
| Context | Native `262144` tokens |
| Vision | Complete BF16 tower, full released image-processor pixel budget |
| Images | At most 15 original static PNGs; 16,777,216 pixels each; aspect ratio at most 30:1 |
| Image transport | Lossless inline PNG; static 8-bit RGB/RGBA only; video/audio rejected |
| MTP/speculative decoding | Disabled |
| Thinking | Required, `xhigh` |
| Historical thinking | `preserve_thinking=false` |
| Qwen Code | `0.21.12`, commit `b965d5f8c24f48e65fb0b17c7d45f34ca4ce8f38` |
| Agent image | `sha256:cc916c63598c5953810482e2e5f614eaa1e96695f5c07bfb2c3f2f894e9aa323` |
| Service | Rust, one session at a time, Docker-only |
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
labels. It also requires a read-only backend root, the exact bounded `/root`, `/tmp`,
and `/run` tmpfs contracts, exactly one labelled v12 vLLM-cache volume, and no other
mount. Runtime JIT state therefore cannot mutate the container layer. The backend's
own status performs the complete file-manifest verification.
An uncorrected Unsloth mount cannot satisfy this lock.

The backend repository remains authoritative for the server patches, model-file
manifest, VRAM accounting, native-context proof, prompt template, parser tests,
and cache benchmarks: [`../Qwen_best_model_ever/README.md`](../Qwen_best_model_ever/README.md).

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

Completed hidden reasoning is deliberately not replayed into every later request:
`preserve_thinking=false`. Visible answers, native tool calls, typed tool results,
and the current turn's reasoning remain. This is the selected long-horizon policy:
it spends the scarce context window on the continuing task rather than repeatedly
paying for every old hidden trace. The setting does not disable thinking.

These are defaults at both layers, not suggestions in prose. vLLM defaults omitted
request fields to thinking enabled, xhigh, unpreserved historical thinking, and the
exact tuple above. The pinned Qwen Code settings send the same values explicitly,
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
`min(262144, 262144 - rendered_prompt_tokens)`. There is no character division,
`target // 8`, image-token guess, padding margin, minimum-output fabrication, local
tokenizer, or tokenizer fallback in the compaction trigger or outbound clamp. If
the tokenizer is missing, malformed, or reports another model window, the turn
fails before generation.

Session turns and cumulative tool calls have no arbitrary cutoff. One model turn
has a 1,000-tool-call circuit breaker for degenerate output. Auto-compaction is
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
obsolete implementation: singleton ownership, copied disposable workspaces,
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
`77e137c098e29a1b3b28da112fc323777a588e0c23caab8532caeb03eb5c2b79`.
The transformer's manifest SHA-256 is
`90102ad5fef4531b4a007eac3715bb1e001f9bfa5e090204aa0c5d3a6d50ecc0`.
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
- exact `max_tokens = min(configured ceiling, context window - rendered tokens)`;
- no character estimate, byte division, padding margin, token safety margin,
  minimum-output fabrication, or tokenizer fallback;
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
- init metadata filtered through the identical two-agent policy, so uncallable
  internal agents are not advertised as an alternate behavior;
- workspace settings and environment discovery disabled before initialization,
  with ambient, project, CLI, and injected MCP servers all excluded from the
  locked mode while ordinary project `QWEN.md` and `AGENTS.md` task instructions
  remain available;
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

Images enter the agent only through `read_file` in the copied `/workspace`. The
creation API remains deliberately small—folder plus text prompt—so there is no
second upload protocol or alternate history renderer. The model calls `read_file`
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
turn. The visible summary can preserve findings, while completed hidden thinking is
also omitted. This is the selected long-thread policy.

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
metadata exactly matches the deployed contract, a stable session ID, exactly one
terminal result as the final event, an internally consistent main-turn count, and
the complete success/error envelope. Zero-usage streaming fragments are not
mistaken for additional turns. Duplicate results, post-result output, malformed
lines, missing fields, an empty successful result, and a missing result are hard
errors. It never chooses a convenient-looking “last result.”

## Prefix caching evidence

Prefix caching is enabled in the sole backend command. It is not accepted on faith:
the accepted v12 corrected backend's final chronological image-history probe measured
a 6.0440-second cold TTFT with zero prefix/multimodal hits, then a 0.3331-second warm
Anthropic TTFT with 14,560 prefix-hit tokens and a multimodal-cache hit—an 18.146×
improvement.
OpenAI and Anthropic histories rendered to identical token IDs. Changed image bytes
missed the multimodal cache; moving identical bytes hit the multimodal cache but
missed the prefix cache, proving the two mechanisms are not being conflated.

The real Qwen Code path was also measured from authoritative vLLM counters. Session
`s-a0def9ef00b8444c82ec0069d5cd3dce` completed four native model turns—text read,
original-PNG read, shell, and final response—and added 53,867 prompt tokens, of which
35,360 were prefix-cache hits. Its two image-history queries produced one
multimodal-cache hit: the first request after the image tool result encoded it, and
the later request reused the exact image while it remained in its chronological tool
position.

The identical task was then run as session
`s-18eb0b39b0f54c13b03c3ba103233859`. All four native turns passed again. It added
53,592 prompt tokens, of which 45,760 were prefix-cache hits, and both multimodal
queries were hits. Mean backend TTFT across the four requests fell from 1.129958153
seconds to 0.279406309 seconds, a 4.044140-times improvement. Qwen process time fell
from 30.552 seconds to 18.774 seconds. These are sampled agent runs, so the claim is
cache reuse and measured timing separation—not deterministic output identity.

Qwen Code's compatibility usage field reported zero cache reads in both runs even
though vLLM's counters proved the hits. The release therefore uses backend counters,
not that frontend compatibility field, as its cache authority.

## Network and filesystem isolation

The host is assumed to be reachable from the public Internet on every non-loopback
interface. Neither project publishes a port. The only TCP listeners are validated as
exactly `127.0.0.1:8000` and `127.0.0.1:8090`.

```text
agent container: --network none
  Qwen -> 127.0.0.1:18000
              |
inner socat container: --network container:<agent>
              |
       /sock/vllm.sock
              |
outer socat container: --network host
              |
       127.0.0.1:8000 vLLM
```

The agent literally has no interface, IPv4 route, DNS, bridge gateway, published
port, host network, or GPU device. The inner proxy shares only the agent's network
namespace and binds the agent's own loopback. The outer proxy alone shares the host
network and connects only to the pinned vLLM loopback listener. They meet through a
private per-session Unix socket. Both proxies are read-only, uid 1000, cap-drop-all,
no-new-privileges containers with small memory/PID limits.

The service container itself uses host networking because it must bind the host
loopback and control Docker. It has no published ports, is read-only, runs as uid
1000 with only the Docker-socket group added, and mounts `/home/user` read-only. The
single project `.runtime` subtree is over-mounted read-write for staging and durable
results. Every requested source folder must be a strict descendant of the pinned
`/home/user` input root and may not overlap service state. Symlinks, special files,
system roots, more than 200,000 files, or more than 4 GiB are rejected before the
agent starts.

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

The source folder is copied. The original is never mutated. Executable semantics are
preserved while dangerous mode bits are stripped. The agent modifies `/workspace`;
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

## Reproducibility and real version pinning

[`docker/Dockerfile`](docker/Dockerfile) has digest-pinned linux/amd64 Ubuntu, Node,
and Rust stages. Ubuntu packages come from the timestamped
`20260814T120000Z` snapshot, and every requested package has an exact version. The
initial TLS bootstrap still authenticates repository metadata by Ubuntu's signed
`InRelease`; after the exact CA package is installed, the snapshot is fetched again
with ordinary certificate verification. Qwen and Docker CLI remote archives use
BuildKit `ADD --checksum=sha256:...` and are hashed again inside the build.
The Dockerfile frontend itself is digest-pinned, and BuildKit receives
`SOURCE_DATE_EPOCH=1786725153`, the exact upstream Qwen commit timestamp, as a
build argument so image creation and layer timestamps do not depend on rebuild
time.

The service image contains Docker CLI 29.7.2 from the exact official static archive,
not the host binary. The Rust binary is built by the pinned Rust 1.95.0 image with
`cargo test --locked` and `cargo build --locked --release`. The agent image is sealed
by exact image ID plus labels for the upstream version/commit/archive/patch and all
three configuration files. The service image is sealed by committed source, stack
lock, and Cargo lock labels.

The current agent image is
`sha256:cc916c63598c5953810482e2e5f614eaa1e96695f5c07bfb2c3f2f894e9aa323`.
Its build reconstructed forty-six changed/new files from pristine upstream and
passed 2,326 tests in the expanded sixteen-suite focused matrix. The sealed runtime
reports `0.21.12`, embeds commit `b965d5f8c24f`, and carries matching archive,
review-diff, semantic-manifest, settings, instruction, and wrapper labels. The
build script treats any other image ID as drift.
The pinned Rust 1.95.0 service stage also passed all thirteen service tests and a
locked release build, including fail-closed tests for default-policy drift,
semantic-manifest identity, and nonempty slash-command advertisement.

Pinning is not a claim that the upstream dependency graph has no security debt. The
Qwen `npm ci` build currently reports 66 audit advisories (2 low, 36 moderate, 25
high, 3 critical). The build records that fact and does not run `npm audit fix`:
doing so would introduce an unreviewed mutable dependency graph. Remediation means
reviewing a newer exact upstream tree or a narrow explicit patch, then rebuilding
and re-running every gate.

Putting a version string in a README is not considered a pin. The scripts require a
clean repository and validate host tool versions, GPU/driver, file hashes, image IDs,
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
| `POST` | `/v1/agent/sessions` | Validate, stage, start, and return after exact model/tokenizer readiness |
| `GET` | `/v1/agent/sessions` | List running and durable terminal sessions |
| `GET` | `/v1/agent/sessions/{id}` | Pure state read |
| `GET` | `/v1/agent/sessions/{id}/wait` | Wait by notification until terminal; no polling timeout |
| `POST` | `/v1/agent/sessions/{id}/cancel` | Explicit cancellation and awaited teardown |
| `DELETE` | `/v1/agent/sessions/{id}` | Delete one terminal record and bundle |

The creation body is `{ "folder": "/absolute/path", "prompt": "..." }`. The HTTP
body cap is 2 MiB; the prompt cap is 1 MiB. Prompt bytes enter Qwen through text stdin,
not a shell argument, so Linux's per-argument limit does not invalidate the API
contract or expose the prompt in a process listing.

Terminal persistence is atomic and no-clobber (`create_new`, write, `fsync`,
same-directory hard-link publication, directory `fsync`). If persistence fails,
the service retains the complete terminal body in
memory and marks it erroneous rather than evicting the only copy and returning a
misleading 404. Shutdown has no arbitrary teardown deadline.

## Acceptance gates

A release is not complete merely because the images build. Every required gate below
passed against the pinned agent image and the exact live v12 corrected backend:

1. strict JSON, shell syntax, formatting, locked Cargo build, and Rust tests;
2. clean Qwen archive extraction, semantic drift/idempotence/rollback checks,
   transactional source transformation, review-diff equivalence, full patched
   build, and all sixteen focused upstream suites;
3. agent-image label/hash/version checks and network-none route proof;
4. exact live backend container/image/labels/command/listener/version/model/tokenizer;
5. sealed model path from agent loopback, with no route or DNS;
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
13. graceful cancellation, unlimited-wait shutdown, orphan sweep, listener audit,
    including immediate cancellation at the published-readiness boundary, and a
    final clean Git repository under `Ronen Zyroff <rzyroff@gmail.com>`.

The main hostile-workspace acceptance deliberately supplied contradictory `.env`,
`.qwen/settings.json`, MCP, hook, rule, skill, memory, output-language, and custom
slash-command fixtures. Init advertised exactly the ten allowed native tools,
`general-purpose` and `Explore`, no MCP servers, and no slash commands. Ordinary
project `QWEN.md` and `AGENTS.md` guidance remained active. Native text, original-PNG
vision, and shell calls all returned correlated typed results; the model read
`VISION_TOOL_PROOF_7F3C` from the image. No hostile marker was executed or included.

Session `s-db8a64fabf494192b844d174a37ee9d8` then passed a JPEG directly to native
`read_file`. It returned a typed error naming the missing exact PNG signature; no
conversion, resize, retry, or alternate tool path occurred. Session
`s-604472363cb04672a38ae01fc130ac16` invoked exactly one foreground `Explore`
subagent, retained parent/child tool IDs, returned its native `read_file` result, and
was verified in the main thread.

While that subagent session was live, the agent container exposed only `lo`, had an
empty IPv4 route table, failed public DNS lookup, had no NVIDIA devices or published
ports, and reached the sealed model only through `127.0.0.1:18000`. All three session
containers had read-only roots, all capabilities dropped, and no-new-privileges.
After completion they were absent.

Finally, session `s-5639d985a0e7465f93277e393348540e` was cancelled immediately
after readiness while its prompt required a long foreground command. Cancellation
returned in 0.311 seconds, durably recorded exit 143, archived the two-event partial
stream without inventing a success/result event, emitted no forbidden final marker,
and removed all three owned containers. Script argument-rejection probes also proved
that start, status, stop, and build reject an alternate mode before changing state.

Any future changed input must rerun the affected gates. There is no fallback
declaration of success.
