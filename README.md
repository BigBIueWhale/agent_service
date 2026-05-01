# `agent_service` — singleton agentic Qwen3.6 runner with a sealed Docker session

A small, opinionated Rust HTTP service that runs **one** Qwen Code agent at
a time inside a network-sealed Docker container, mirrors the live session
to a browser-watchable ttyd, and returns the final answer. Designed for a
host that already runs a Qwen3.6 vLLM endpoint on `127.0.0.1` (e.g. the
`qwen36-agent-setup` deployment one directory up).

The whole API is one streaming endpoint. `POST /v1/agent/run` with two
required fields. Two NDJSON events back. That's it.

## Tool palette inside the agent container

The agent image is large (~14 GiB) on purpose: it carries a comprehensive
Linux dev / security / RE / forensics toolset so the agent can actually
do the work. The full catalog is enumerated for the model at
`/home/agent/.qwen/QWEN.md` (see `docker/config/QWEN.md` in the source
tree). Highlights:

- **Languages**: Python 3.12 (with hundreds of preinstalled packages),
  Node.js 22 LTS, Bun, Go, Rust, Ruby, Perl, Lua 5.4, R.
- **Compilers / build / debug**: full GCC + Clang, ARM/AArch64 cross,
  cmake, meson, ninja, gdb, valgrind, strace, ltrace.
- **Reverse engineering / binary analysis**: radare2, binwalk, hexedit,
  gdb-multiarch, capstone, unicorn, keystone, pwntools, lief, ROPgadget,
  ropper, z3-solver, full binutils.
- **Security / pentest (offline)**: sqlmap, nikto, gobuster, dirb, wfuzz,
  john (the ripper), hydra, aircrack-ng, yara, clamav.
- **Forensics**: sleuthkit, volatility3, foremost, scalpel, bulk-extractor,
  exiftool.
- **Network / packet analysis**: nmap, tcpdump, tshark, termshark,
  suricata, scapy, dpkt, pyshark, impacket.
- **Browser automation (offline)**: Playwright (Chromium + Firefox
  preinstalled), Puppeteer, Selenium.
- **Documents**: pandoc, ghostscript, qpdf, poppler-utils, libreoffice
  writer + calc, plantuml, mermaid CLI, weasyprint, wkhtmltopdf.
- **Image / video / audio / OCR**: ImageMagick, ffmpeg, sox, OpenCV,
  Pillow, tesseract (eng/heb/ara/rus/fra/deu/spa), easyocr, moviepy,
  librosa.
- **Data science / ML / NLP**: numpy, scipy, pandas, polars, scikit-learn,
  xgboost, torch (CPU), transformers, sentence-transformers, faiss,
  spacy + 2 English models, nltk + corpora.

The agent has **no internet access at runtime**. The agent image is
sealed: outbound DNS, internet, package mirrors, GitHub, etc. are all
unreachable. `apt install` / `pip install` / `npm install` / `git clone`
of a remote / `curl` / `wget` will all fail. This is by design. The agent
is told this explicitly and instructed not to retry.

## What it does

You hand the service a prompt and a path to a folder. It:

1. Validates both inputs strictly — no symlinks, size caps, absolute path,
   no system roots.
2. Acquires a strict singleton: refuses concurrent runs with HTTP 409.
3. Copies the folder (perms-normalised) to a per-session staging tree.
4. Builds a per-session network sandbox: an outer `socat` container
   (`--network=host`, byte-forwards a Unix socket to the host's vLLM port),
   a `--internal` Docker network with no NAT to anywhere, and an inner
   `socat` container on that network (TCP listener that forwards to the
   bind-mounted Unix socket).
5. Spawns the agent container — Qwen Code 0.15.6 inside the big tooling
   image — on the internal network, with DNS pointed at a no-listener
   loopback and `OPENAI_BASE_URL` set to the inner proxy's IP literal.
6. Waits for ttyd to bind, streams `{"event":"started",...,"ttyd_url":"..."}`
   to the HTTP client so the human can open the URL in a browser.
7. Waits for the agent to finish, parses its `result` event from the
   bind-mounted JSONL stream, and streams `{"event":"finished",...,"response":"..."}`.
8. Tears down inner proxy, internal network, outer proxy, staging tree —
   in that order.

The agent inside the container has full shell access, the entire
vibe-web-terminal toolset, and Qwen Code's built-in subagent dispatch —
encouraged to delegate any non-trivial sub-task sequentially. **No GPU
access** for the agent container by design (the host's vLLM is the only
model surface).

---

## Architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│ HOST (public-internet-exposed)                                       │
│                                                                      │
│   vLLM listening on 127.0.0.1:8001                                   │
│       ▲                                                              │
│       │ TCP                                                          │
│   ┌───┴──────────────────────────┐                                   │
│   │ OUTER PROXY CONTAINER        │  --network=host                   │
│   │  socat                       │  socat                            │
│   │  qwen-agent-template         │   UNIX-LISTEN:/sock/vllm.sock     │
│   │  --user 1000 --read-only     │   TCP:127.0.0.1:<vllm_port>       │
│   │  --cap-drop ALL --memory 64m │                                   │
│   └───┬──────────────────────────┘                                   │
│       │ Unix socket  (bind-mounted into both proxies)                │
│       │                                                              │
│   ┌───┴──────────────────────────┐                                   │
│   │ INNER PROXY CONTAINER        │  --network=agent-net-<id>         │
│   │  socat                       │  socat                            │
│   │  qwen-agent-template         │   TCP-LISTEN:8001                 │
│   │  --dns 127.0.0.1             │   UNIX-CONNECT:/sock/vllm.sock    │
│   │  --read-only --cap-drop ALL  │                                   │
│   └───┬──────────────────────────┘                                   │
│       │ TCP                                                          │
│       ▼                                                              │
│   ┌──────────────────────────────────┐                               │
│   │ DOCKER NETWORK: agent-net-<id>   │  --internal                   │
│   │   no NAT, no internet, no DNS    │                               │
│   └───┬──────────────────────────────┘                               │
│       │                                                              │
│   ┌───┴─────────────────────────┐                                    │
│   │ AGENT CONTAINER             │  qwen → http://<inner_ip>:8001/v1 │
│   │  qwen-agent-template        │  ttyd 7681 → 127.0.0.1:<eph>       │
│   │  --dns 127.0.0.1            │                                    │
│   │  --user 1000 --cap-drop ALL │                                    │
│   │  no --gpus, no --privileged │                                    │
│   └─────────────────────────────┘                                    │
│                                  ▲                                   │
│                                  │ http (browser, read-only)         │
│                                                                      │
└──────────────────────────────────────────────────────────────────────┘
                                   │
                                Browser
```

### Data plane: `socat` only, in containers

Every byte from the agent to vLLM and back traverses two `socat` processes,
each running inside a hardened container. socat is byte-level — it doesn't
parse content — so streaming chat completions (SSE / chunked transfer),
keep-alive connections, and any future protocol upgrade (HTTP/2,
WebSocket) all pass through transparently. The orchestrator is **not** in
the data plane; if its process is briefly unhealthy, in-flight chat
completions keep flowing.

### Why two hops, both as containers

- **The agent is on a `--internal` Docker network.** That flag drops every
  packet whose destination isn't on the same network — no NAT, no host
  reachability, no internet. The only peer the agent can reach is the
  inner proxy.
- **The host's vLLM is published with `-p 127.0.0.1:8001:8001`** (a
  deliberate property of the parent project — vLLM must not be reachable
  from anywhere else on the host). Containers cannot reach a 127.0.0.1-
  published port through `host.docker.internal:host-gateway`; the only way
  to reach it is from a process that shares the host's network namespace.
- **So the outer hop runs `--network=host`** and uses socat to bridge
  *the host's* `127.0.0.1:<vllm_port>` to a Unix socket on the host
  filesystem.
- **The inner hop runs on the agent's internal network**, accepts TCP from
  the agent, and forwards to the bind-mounted Unix socket — so it never
  needs any host network access of its own.
- **Both hops are containers (not host processes, not Rust code).** That's
  the docker-native answer: container-level security primitives apply
  uniformly (`--read-only`, `--cap-drop ALL`, `--user 1000`,
  `--security-opt no-new-privileges`, `--memory`, `--pids-limit`),
  `docker ps` / `docker logs` are the operator interface, and the
  Rust orchestrator stays purely in control of *lifecycle*, never *bytes*.

### Singleton, not a pool

For now, **exactly one** agent run can be in flight at a time. A second
`POST /v1/agent/run` while the first is running gets HTTP 409 with the
running session's id. The architecture is structured (mutex over
`Option<RunningSession>`) so growing into a small bounded pool is a
mechanical change once you have more GPUs — replace the mutex with a
semaphore + `Vec<RunningSession>`, generate per-session network names
(already done; they're `agent-net-<uuid>`), and route each session at a
distinct vLLM endpoint.

### Why ttyd + tmux

ttyd's default behaviour spawns the command on each new browser
connection — we don't want that. The agent should run regardless of
whether anyone is watching. So inside the container, `agent_init.sh`
starts the agent in a detached `tmux` session and foregrounds ttyd
attached to that session in read-only mode. Multiple browsers can attach
concurrently; none of them can type into the agent. The browser-to-ttyd
channel is WebSocket and is not in the agent → vLLM proxy chain.

### Capturing the response

Qwen Code 0.15.6's `--json-file` flag is **TUI-only** and is silently
ignored under headless `-p` mode (verified against
`packages/cli/src/gemini.tsx:185-204` at tag `v0.15.6`). The clean
solution: **don't let ttyd run `qwen` directly**. The container's wrapper
pipes `qwen --output-format stream-json | tee /output/events.jsonl`, so
ttyd renders the JSONL stream live for the human watcher AND a
host-visible bind mount accumulates the events for programmatic parse.
The host reads the last line whose `type == "result"` and pulls
`.result` (success) or `.error.message` (failure) out of it.

---

## Pinned versions

| Component | Version | Pin location |
|---|---|---|
| Qwen Code CLI | `@qwen-code/qwen-code@0.15.6` | `docker/Dockerfile` |
| Node.js | 22.x LTS via NodeSource | `docker/Dockerfile` |
| Ubuntu base | `ubuntu:24.04` | `docker/Dockerfile` |
| ttyd | commit `9c87671ccae9eefa3c01b08169272c0922e7cdff` | `docker/Dockerfile` |
| libwebsockets | `v4.3.6` | `docker/Dockerfile` |
| PlantUML | `v1.2025.0` | `docker/Dockerfile` |
| glow | `v2.0.0` | `docker/Dockerfile` |
| socat | from `qwen-agent-template:0.1.0` (Ubuntu's apt `socat`) | `docker/Dockerfile` |
| axum | `=0.8.9` | `Cargo.toml` |
| tokio | `=1.52.1` | `Cargo.toml` |
| All other Rust deps | strict `=` pin | `Cargo.toml` |

Floating tags (`latest`, `main`) are not used anywhere.

---

## Reliability configuration

Qwen Code 0.15.6 ships several built-in autonomous-run safeguards that
are **off by default** but matter a lot for unattended runs. The image's
`~/.qwen/settings.json` enables them all:

| Key | Value | What it does |
|---|---|---|
| `model.skipLoopDetection` | `false` | Activates **five** loop detectors (`packages/core/src/services/loopDetectionService.ts`): identical-tool-call repeat (×5), 50-char content chunk repeat (×10), repetitive structured thoughts (×3), excessive read-like tools (≥8 in last 15), same-tool name with varying args (×8). On detection the run aborts cleanly via `GeminiEventType.LoopDetected`. **Critical** for unattended runs — Qwen Code's default is `true` (loop detection disabled). |
| `model.skipNextSpeakerCheck` | `true` | Prevents the CLI from auto-injecting `"Please continue."` after empty turns. Auto-prodding is a footgun for Qwen3 — pinned explicitly to defend against future Qwen Code version changes. |
| `model.maxSessionTurns` | `200` | Last-resort turn cap. Run aborts with exit code 53 (`MAX_TURNS_EXCEEDED`) if the outer session-turn count exceeds it. Read raw at `client.ts:709-710` with no internal clamp. CLI flag `--max-session-turns` (driven by `AGENT_SERVICE_MAX_TURNS`) overrides the settings file value when present. The `MAX_TURNS = 100` constant at `client.ts:96` is unrelated — it's a recursion-depth bound on `sendMessageStream`, not a cap on session turns. **Why 200 and not lower**: the five loop detectors above + `sessionTokenLimit` + the orchestrator's wall-clock catch every stuck-mode failure before this fires; the turn cap is just the "the model is making progress but we're way past any plausible legitimate run length" backstop. |
| `model.sessionTokenLimit` | `262144` | A **most-recent-prompt-token** cap, not cumulative. Compared at `client.ts:731-747` against `lastPromptTokenCount` (from `uiTelemetry.ts:147,180-186` — `totalTokenCount` of the most recent API response, including cached tokens). Acts as a "this request would have OOM'd or hit max-model-len" backstop; aborts the session cleanly with the run's `result` event still emitted. 262144 is Qwen3.6's `max_position_embeddings`; in practice the parent project's vLLM is configured to 152000, so this trips effectively never (vLLM rejects with HTTP 400 first). Qwen Code's default is `-1` (disabled). |

These are independent of the sampling configuration below — none of them
require changing `presence_penalty` away from the AWQ-recipe-mandated
`0.0`.

What is **not** available in v0.15.6 (in case you're searching for it):
no `--strict` / `--bail-on-error` flag; no time-based "stuck thinking"
detector; no tunable thresholds for the loop heuristics; no LLM-based
loop check (only the streaming heuristics above).

## Sampling configuration

The image's `~/.qwen/settings.json` bakes in the official
QuantTrio/Qwen3.6-27B-AWQ "Best Practices" thinking-mode parameters:

```json
{
  "temperature": 0.6,
  "top_p": 0.95,
  "top_k": 20,
  "min_p": 0.0,
  "presence_penalty": 0.0,
  "repetition_penalty": 1.0,
  "max_tokens": 81920
}
```

These are the **higher-quality** values from the model card — they accept
slightly higher infinite-loop risk in exchange for noticeably better
output on math and code (`presence_penalty=0.0` per Alibaba's published
recommendation; the `linear_attn.in_proj_a/b` layers in the AWQ recipe
are kept at BF16 specifically to mitigate the loop pathology, see the
parent project's README §3.1).

Vision is enabled (`modalities.image=true, modalities.video=true`) and
`splitToolMedia=true` is set as documented in the parent project's §5.8.
Reasoning is on, defaulted to `enable_thinking=true` server-side; the
client emits no `chat_template_kwargs` (verified by source grep against
v0.15.6) so the server defaults always land.

---

## Security properties

- **Listens only on loopback.** The service refuses to bind anywhere else.
- **Network isolation in depth.** The agent container is on a `--internal`
  Docker network; the inner proxy is the only peer it can reach; the inner
  proxy can only forward to the bind-mounted Unix socket; the outer proxy
  is the only thing on that socket and only forwards to one fixed TCP
  destination on the host (`127.0.0.1:<vllm_port>`). socat is byte-stupid
  — it doesn't parse, it can't be redirected by client traffic.
- **DNS exfiltration is closed.** `--internal` networks alone are not
  sufficient — Docker's embedded resolver at `127.0.0.11` forwards queries
  via the daemon namespace and *does* reach external DNS. Every container
  in the chain (agent, inner proxy, outer proxy) is started with
  `--dns 127.0.0.1 --dns-search .` — resolv.conf points at a non-listening
  loopback, so every DNS lookup fails immediately. The agent reaches the
  inner proxy by IP literal, never by name.
- **No GPU access** for the agent container. Verified by inspection of
  every `run_detached` call site in `src/network.rs` and `src/session.rs`.
- **No `--privileged` and no `CAP_*` adds anywhere** beyond the agent's
  unused `NET_BIND_SERVICE` (kept for the case where a future operator
  lowers the ttyd port below 1024).
- **Both proxies and the agent are `--cap-drop ALL --user 1000:1000
  --read-only --security-opt no-new-privileges`.** Each proxy is
  additionally `--memory 64m --pids-limit 32` (small because they run
  one socat process and nothing else); the agent gets **`--memory 32g
  --memory-swap 32g`** (no swap) and **`--storage-opt size=128g`**
  (writable-layer cap), with `--pids-limit 4096`. Both limits are
  configurable via `AGENT_SERVICE_MEMORY` and
  `AGENT_SERVICE_STORAGE_QUOTA`.
- **The user's source folder is *copied***, not bind-mounted, into a
  per-session staging tree (mode-normalised to 0o755 dirs / 0o644 files)
  before the agent sees it. The agent cannot reach back into the user's
  actual working tree.
- **Symlinks inside the source folder are rejected outright** at
  validation time.
- **Orphan sweep on startup.** Every Docker object the service creates is
  labelled `agent_service.session=<uuid>`. If the orchestrator crashes
  mid-session, the next start `docker rm`s every container and `docker
  network rm`s every network bearing that label, then begins serving.

---

## Build

### 1. Build the agent template image

The image is large (~14 GiB) because it carries the full toolset the agent
might want to call — full Python + Node + every common Unix dev tool. Build
once, run forever. The same image is reused for both proxies (with
`--entrypoint socat`), so this is the only image to build or pull.

```bash
cd /home/user/Desktop/agent_service
docker build \
    -t qwen-agent-template:0.1.0 \
    -f docker/Dockerfile \
    docker/
```

The build takes ~30–60 minutes the first time depending on disk and
network. Subsequent rebuilds use the layer cache.

Sanity-check the image (the agent CLI, the proxy binary, and ttyd are all
expected to be present):

```bash
docker run --rm --entrypoint qwen   qwen-agent-template:0.1.0 --version  # 0.15.6
docker run --rm --entrypoint socat  qwen-agent-template:0.1.0 -V         # any version line
docker run --rm --entrypoint ttyd   qwen-agent-template:0.1.0 --version  # any version line
```

### 2. Build the Rust binary

```bash
cd /home/user/Desktop/agent_service
cargo build --release
# binary lands at ./target/release/agent_service
```

---

## Run

The service listens on `127.0.0.1:8090` by default. Every configurable
knob is an environment variable; the defaults match the parent project's
vLLM deployment.

```bash
# Defaults; uncomment / change as needed.
# export AGENT_SERVICE_LISTEN_ADDR=127.0.0.1:8090
# export AGENT_SERVICE_VLLM_HOST=127.0.0.1
# export AGENT_SERVICE_VLLM_PORT=8001
# export AGENT_SERVICE_MODEL_NAME=Qwen3.6-27B-AWQ
# export AGENT_SERVICE_IMAGE=qwen-agent-template:0.1.0
# export AGENT_SERVICE_MEMORY=32g                        # agent RAM ceiling
# export AGENT_SERVICE_MEMORY_SWAP=32g                   # = MEMORY → no swap
# export AGENT_SERVICE_STORAGE_QUOTA=128g                # writable-storage cap; empty disables
# export AGENT_SERVICE_STATE_DIR="$HOME/.local/state/agent_service"
# export AGENT_SERVICE_RESULTS_DIR="$HOME/.local/state/agent_service/results"
# export AGENT_SERVICE_RESULTS_RETAIN=20                 # past bundles to keep; 0 = unlimited
# export AGENT_SERVICE_TIMEOUT_SECS=7200                 # 2h wall-clock cap
# export AGENT_SERVICE_MAX_TURNS=200                     # last-resort cap; loop detectors fire much earlier

./target/release/agent_service
```

Pre-flight verifies:

- The Docker daemon is reachable as the running user (no root needed —
  the user must be in the `docker` group).
- `AGENT_SERVICE_IMAGE` is present locally (both proxies and the agent
  reuse it).
- The host has `tar` and `zstd` on PATH (used to build the per-session
  result bundle).
- `AGENT_SERVICE_STORAGE_QUOTA` is honoured by the local Docker storage
  driver, **if** a quota was requested. Probed at startup by running a
  no-op container with `--storage-opt size=…`; if the daemon rejects it,
  the service refuses to start with a message pointing at the two ways
  forward (configure the daemon for per-container quotas, or set
  `AGENT_SERVICE_STORAGE_QUOTA=` empty to disable). This is opinionated
  on purpose — silently dropping the cap on systems that can't enforce
  it would defeat the point of asking for one.
- `AGENT_SERVICE_STATE_DIR` and `AGENT_SERVICE_RESULTS_DIR` exist or can
  be created.
- Any orphan containers / networks left by a previous crash (matched by
  the `agent_service.session=*` label) are swept; any stale per-session
  staging directories under `<state_dir>/sessions/` are removed. (The
  results directory is **not** swept — it's the persistent home for
  past-session bundles.)
- Past-session bundles are pruned to `AGENT_SERVICE_RESULTS_RETAIN` by
  oldest mtime (in case the operator just shrank the retain count).
- `AGENT_SERVICE_LISTEN_ADDR` resolves to a loopback address — the
  service refuses to bind anywhere else, full stop.

The service exits non-zero with a single human-readable line on any
pre-flight failure. Exit codes: `2` config, `10` daemon unreachable,
`11` image missing, `12` other internal, `1` bind / runtime.

---

## API

Listening on `127.0.0.1:8090` only. No TLS — loopback-only; if you tunnel
this, that's the caller's job. One way to invoke, one way to consume.

### `POST /v1/agent/run`

Request body — both fields **required**:

```json
{
  "prompt": "Reproduce the bug in tests/regress.test.ts and fix it.",
  "folder": "/home/user/projects/widget-server"
}
```

Validation:

- `prompt` is non-empty after trimming, ≤ 100 KiB, contains no NUL byte.
- `folder` is an absolute path to an existing directory, with no
  symlinks anywhere in the tree, ≤ 4 GiB total, ≤ 200 000 files. System
  paths (`/`, `/etc`, `/proc`, `/sys`, `/dev`, `/boot`, `/var/run`,
  `/run`) are rejected.

Successful response: `200 OK`, `Content-Type: application/x-ndjson`.
**Exactly two newline-delimited JSON objects** in the stream:

```jsonc
// Line 1 — emitted as soon as ttyd is reachable.
{
  "event": "started",
  "session_id": "s-1f0e7b...",
  "ttyd_url": "http://127.0.0.1:51234/",
  "started_at_unix": 1746115234,
  "agent_image": "qwen-agent-template:0.1.0",
  "model_name": "Qwen3.6-27B-AWQ"
}

// Line 2 — emitted when the container exits, events.jsonl is parsed,
//          and the result bundle has been written.
{
  "event": "finished",
  "session_id": "s-1f0e7b...",
  "finished_at_unix": 1746116012,
  "duration_wall_ms": 778431,
  "container_exit_code": 0,
  "is_error": false,
  "response": "<the agent's final answer text>",
  "agent_num_turns": 17,
  "agent_duration_ms": 776543,
  "bundle_archive_path": "/home/user/.local/state/agent_service/results/s-1f0e7b/bundle.tar.zst",
  "bundle_compressed_bytes": 1284412,
  "bundle_uncompressed_bytes": 5938204,
  "bundle_file_count": 14,
  "bundle_artifacts_file_count": 12,
  "teardown_diagnostics": []
}
```

**The bundle archive is the agent's primary output channel for files.**
Inside `bundle.tar.zst`:

```
artifacts/                      ← whatever the agent wrote to /artifacts/
   (structure determined entirely by the operator's prompt — the agent
    is told that /artifacts is empty when it starts and that anything it
    writes there is returned)
output/events.jsonl             ← the agent's full structured event stream
                                  (forensics; the same stream that ttyd
                                   shows live during the run)
output/qwen-exit-code           ← the qwen process's numeric exit code
```

`bundle_archive_path` is **always present** (empty string only on a
bundle creation failure, in which case `teardown_diagnostics` explains
why). `bundle_artifacts_file_count` separates the agent's intentional
output from the forensics sidecars, so callers can quickly spot a run
that produced no artefacts.

Past bundles persist under `AGENT_SERVICE_RESULTS_DIR` (default
`<state_dir>/results/<session_id>/`). After each successful run the
service prunes oldest-by-mtime bundles down to
`AGENT_SERVICE_RESULTS_RETAIN` (default 20; set to 0 to disable
pruning). The session's *staging* directory (under
`<state_dir>/sessions/<session_id>/`) is removed at the end of the run
regardless — only the bundle persists.

If something fails after `started` but before `finished` is emittable
(e.g. `docker wait` itself errors), a third event shape is emitted and
the stream closes:

```json
{"event":"error","kind":"docker_command_failed","error":"<message>","session_id":"s-..."}
```

Pre-stream failures (validation, busy, docker unavailable, image missing)
return a non-streaming JSON body with the error shape from `error.rs`:

```json
{ "error": "<message>", "kind": "<machine-readable kind>", "running_session_id": "" }
```

| HTTP | `kind` |
|---|---|
| 400 | `invalid_request` |
| 409 | `busy` (`running_session_id` non-empty) |
| 502 | `docker_command_failed` / `agent_output_missing` |
| 503 | `docker_unavailable` / `image_missing` |
| 504 | `timeout` |
| 500 | `staging_failed` / `internal` |

### `GET /v1/agent/current`

Returns the currently-running session as JSON, or `404` with
`{"running":false,"session":null}` when idle.

```json
{
  "session_id": "s-1f0e7b...",
  "ttyd_url": "http://127.0.0.1:51234/",
  "started_at_unix": 1746115234,
  "prompt_preview": "Reproduce the bug in tests/regress.test.ts and fix it."
}
```

### `GET /healthz`

`200 OK` plain-text `ok`. For supervisor probes.

---

## Example session

```bash
# Terminal 1 — start the service
./target/release/agent_service

# Terminal 2 — kick off a run.
curl --no-buffer -X POST \
  -H 'Content-Type: application/json' \
  --data '{
    "prompt":"Find out why tests/test_login.py::test_session_expiry fails and fix it. Use subagents to investigate independent code paths sequentially. Final answer: a unified diff of the fix and a one-paragraph root-cause explanation.",
    "folder":"/home/user/projects/myapp"
  }' \
  http://127.0.0.1:8090/v1/agent/run
```

You'll see the two NDJSON lines stream out. As soon as line 1 arrives,
open the `ttyd_url` in a browser to watch the agent reason. Line 2
arrives when it's done, with the final answer in `response`.

---

## Repository layout

```
agent_service/
├── Cargo.toml               # strict-= pinned deps
├── README.md                # this file
├── docker/
│   ├── Dockerfile           # qwen-agent-template:0.1.0 (agent + socat + ttyd + tools)
│   └── config/
│       ├── settings.json    # ~/.qwen/settings.json (sampling, modalities)
│       ├── QWEN.md          # ~/.qwen/QWEN.md (operating instructions)
│       ├── agent_init.sh    # CMD: tmux + ttyd
│       └── run_agent.sh     # in-tmux: qwen | tee /output/events.jsonl
└── src/
    ├── main.rs              # bootstrap, listener, signals
    ├── api.rs               # axum routes + NDJSON streaming + pre-flight
    ├── bundle.rs            # tar.zst result bundle + retention pruning
    ├── config.rs            # env-driven config (loopback-only enforced)
    ├── docker_ops.rs        # subprocess wrappers around `docker`
    ├── error.rs             # ServiceError + IntoResponse
    ├── network.rs           # IsolatedNetwork (outer + inner socat + internal net)
    ├── result_parse.rs      # parse events.jsonl → AgentResult
    ├── session.rs           # singleton manager + lifecycle
    ├── staging.rs           # per-session paths + copy-with-perms
    └── validation.rs        # prompt + folder validation
```

---

## Multi-GPU growth path (deliberate forward-compat)

The singleton today is `Mutex<Option<RunningSession>>`. To grow into a
bounded pool when more GPUs come online:

1. Change `state: Mutex<Option<RunningSession>>` to
   `(Semaphore, Mutex<Vec<RunningSession>>)` in `session.rs`.
2. Permit the caller to pass a target vLLM endpoint per request, OR run
   one `agent_service` per GPU and put a tiny round-robin in front.
3. The network module already builds per-session networks named
   `agent-net-<uuid>` and per-session container names `agent-{outproxy,inproxy,agent}-<uuid>`
   — they don't collide.

The Rust side took 5 minutes to design with this in mind. The Docker side
already supports it because every per-session object carries a unique
session id.

---

## License

Same as the parent `qwen_36_agent_setup` project.
