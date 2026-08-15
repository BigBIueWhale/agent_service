# Qwen3.8 local agent service

This repository is the one supported local-agent experience for this workstation.
It runs a pinned, patched Qwen Code client in an offline Docker container against
the already deployed Qwen3.8-27B Unsloth NVFP4 backend. It is intentionally a
singleton: one long-lived main agent thread, optional sequential foreground
subagents, one model, one sampling policy, one context policy, one network path,
and one set of scripts.

There is no Claude mode, Codex mode, Qwen3.6 mode, vision mode, reduced-context
mode, alternate port, retry downgrade, heuristic tokenizer, XML tool recovery,
host client installation, or compatibility fallback. A contradiction is an error
with evidence, not an invitation to try something else.

The application itself runs in Docker. The host scripts perform only explicit
diagnostics and Docker lifecycle control. They do not install packages or modify
the host's Claude, Codex, Qwen, npm, Python, Rust, or shell configuration.

## Bottom line

The fixed stack is:

| Component | Locked value |
|---|---|
| Model | `unsloth/Qwen3.8-27B-NVFP4` |
| Model revision | `16b6615af3548b88e2d8e382457bc705b00479cf` |
| Served name | `qwen3.8-27b-nvfp4-k8v4` |
| vLLM source | `9df9b0b0a1816b6d0d0f6ecd0da563cc37fd72f5` |
| vLLM runtime | `0.27.2rc1.dev106+g9df9b0b0a` |
| Weights | Mixed NVFP4/FP8, Compressed Tensors |
| KV cache | TurboQuant K8V4: FP8 keys, packed 4-bit values |
| Context | Native `262144` tokens |
| Vision | Disabled |
| MTP/speculative decoding | Disabled |
| Thinking | Required, `xhigh` |
| Historical thinking | `preserve_thinking=false` |
| Qwen Code | `0.21.12`, commit `b965d5f8c24f48e65fb0b17c7d45f34ca4ce8f38` |
| Service | Rust, one session at a time, Docker-only |
| Service listener | `127.0.0.1:8090` only |
| Model listener | `127.0.0.1:8000` only |

Every longer pin—including base-image digests, package snapshot and versions,
source archive and patch hashes, Docker/BuildKit versions, image identity, live
backend command, model manifest, driver, GPU, and configuration hashes—is in
[`config/stack.lock.json`](config/stack.lock.json). The lock is compiled into the
service binary. At startup, the mounted copy must match the compiled copy
byte-for-byte, the agent image ID and labels must match it, and the live backend
container and HTTP endpoints must match it field-for-field.

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
text-only model loads about 20.47 GiB of weights on an RTX 5090 with 32,607 MiB.
After hybrid-state paging and alignment, the measured 6.45 GiB cache reservation
provides 264,115 cache tokens, only 1,971 above the native limit. The final cold
acceptance used 30,287 MiB and left 1,824 MiB free.

Accordingly, the only honest quality-first maximum on this GPU is native 262,144.
The backend proved a 262,143-token prompt plus one output token and correctly
rejected a request that exceeded the physical model window. A static-YaRN startup
was experimentally bracketed around 336K before OOM, but it lacks both quality
validation and operational margin and is not a supported mode.

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
- old multimodal workarounds are unreachable because this profile is truly text-only;
- several reasoning-ingress and parser reconstruction issues are fixed or superseded
  upstream;
- current code nevertheless had new tool-grammar, truncation, TurboQuant workspace,
  phase-budget, and protocol-validation defects, which received narrow reviewed
  patches and direct tests in the backend repository.

The old `agent_service` design also contained good outer-envelope ideas alongside
obsolete implementation: singleton ownership, copied disposable workspaces,
offline/no-GPU agents, explicit cancellation, durable JSONL and bundles, labels,
orphan recovery, and ordered teardown were kept. Qwen Code 0.15.6, Qwen3.6 AWQ,
port 8001, ttyd, wildcard bridge plumbing, vision, 152K context, host installation,
warning-only probes, polling waits, and mutable package installation were removed.

## The pinned Qwen Code patch

Upstream Qwen Code 0.21.12 is downloaded from the exact commit archive and then
patched by [`patches/qwen-code-0.21.12-agent-service.patch`](patches/qwen-code-0.21.12-agent-service.patch).
The pin was rechecked on 2026-08-15: GitHub's latest stable release was
`v0.21.12`, and npm's `latest` metadata reported both version `0.21.12` and
`gitHead=b965d5f8c24f48e65fb0b17c7d45f34ca4ce8f38`. The npm tarball is not an
unused second execution path; the reviewed commit archive is the sole source.
The build first verifies both hashes and that every hunk applies cleanly. It then
runs `npm ci` against the upstream lock file, applies upstream's own `patch-package`
set, builds the CLI, bundles it, and runs the seven focused suites covering all
modified areas. No published npm package or mutable `latest` tag is executed.

The local patch provides:

- vLLM `/tokenize` calls over the same rendered messages, tools, and template kwargs
  as the subsequent completion request;
- exact `max_tokens = min(configured ceiling, context window - rendered tokens)`;
- no character estimate, byte division, padding margin, token safety margin,
  minimum-output fabrication, or tokenizer fallback;
- a universal strict native-tool allowlist covering built-in, dynamic, MCP, skill,
  and synthetic tools;
- a successful-tool-call terminal invariant;
- no XML recovery, implicit semantic retry, automatic continuation, or executable
  partial call after a length stop;
- foreground-only `general-purpose` and `Explore` agents, with no forks,
  background work, teams, worktrees, custom types, model overrides, or nesting.

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
the backend's timed 65K-token agent continuation reported 64,480 cached prompt
tokens and 0.386-second time-to-first-token versus 12.970 seconds for a zero-cache
control, a measured 33.639× improvement. The exact measurements and test command
are recorded in the backend README. Agent-service acceptance additionally exercises
multi-turn native tool history so request construction cannot accidentally defeat
the cache while a synthetic benchmark still passes.

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

The source folder is copied. The original is never mutated. Executable semantics are
preserved while dangerous mode bits are stripped. The agent modifies `/workspace`;
the final workspace, `/artifacts`, prompt record, ready record, complete events,
stderr, exit code, and final response are placed in a deterministic `bundle.tar.zst`.
Missing bundle entries, symlinks, read races, or tar errors are process failures.
There is no `--ignore-failed-read` path.

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

A release is not complete merely because the images build. The required gates are:

1. strict JSON, shell syntax, formatting, locked Cargo build, and Rust tests;
2. clean Qwen archive extraction, patch check/application, full patched build, and
   all seven focused upstream suites (the researched baseline was 1,571 tests);
3. agent-image label/hash/version checks and network-none route proof;
4. exact live backend container/image/labels/command/listener/version/model/tokenizer;
5. sealed model path from agent loopback, with no route or DNS;
6. real native tool call and typed tool-result continuation through Qwen Code;
7. strict stream-JSON result capture and deterministic complete bundle;
8. malformed/duplicate/post-terminal stream failures;
9. complete and truncated tool-call behavior for streaming and non-streaming server
   paths;
10. repeated agentic turns showing actual cached prompt tokens and materially lower
    time-to-first-token;
11. native-context boundary and K8V4 memory evidence inherited from the exact live
    backend image;
12. graceful cancellation, unlimited-wait shutdown, orphan sweep, listener audit,
    and a final clean Git repository under `Ronen Zyroff <rzyroff@gmail.com>`.

Any unrun or failed gate must be reported as such. There is no fallback declaration
of success.
