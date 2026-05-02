# `agent_service` — singleton agentic Qwen3.6 runner with a sealed Docker session

A small, opinionated Rust HTTP service that runs **one** Qwen Code agent at
a time inside a network-sealed Docker container, mirrors the live session
to a browser-watchable ttyd, and returns the final answer. Designed for a
host that already runs a Qwen3.6 vLLM endpoint on `127.0.0.1` (e.g. the
`qwen36-agent-setup` deployment one directory up).

The API is a small lifecycle-explicit CRUD over a `session` resource:
`POST /v1/agent/sessions` to create, `GET` to read or list,
`POST .../cancel` to interrupt, `DELETE` to discard. One JSON shape
(`SessionBody`) for every read, with a `status` discriminator
(`running` | `completed` | `cancelled`). Reads are pure, writes are
idempotent, and there is no time- or count-based eviction anywhere —
sessions live until the operator deletes them.

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
- **Forensics**: sleuthkit, volatility3, foremost, scalpel, exiftool.
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

You hand the service a prompt and a path to a folder via
`POST /v1/agent/sessions`. It:

1. Validates both inputs strictly — no symlinks, size caps, absolute path,
   no system roots.
2. Acquires a strict singleton: refuses concurrent submissions with
   HTTP 409 (`busy`, with the running session's id in the envelope).
3. Copies the folder (perms-normalised) to a per-session staging tree.
4. Builds a per-session network sandbox: an outer `socat` container
   (`--network=host`, byte-forwards a Unix socket to the host's vLLM
   port); a `--internal` +
   `com.docker.network.bridge.gateway_mode_ipv4=isolated` Docker network
   with no NAT, no host loopback, and no bridge gateway; an inner `socat`
   container on that network (TCP listener that forwards to the
   bind-mounted Unix socket); and a per-session ttyd-publishing sidecar
   on a separate non-internal bridge.
5. Spawns the agent container — Qwen Code 0.15.6 inside the big tooling
   image — on the internal network, with DNS pointed at a no-listener
   loopback and `OPENAI_BASE_URL` set to the inner proxy's IP literal.
6. Blocks the `POST` until ttyd is reachable through the sidecar, then
   returns `201 Created` with the `running` `SessionBody` (including the
   `ttyd_url` the operator opens in a browser).
7. The agent runs in the background. Its progress is observable through
   `GET /v1/agent/sessions/<id>` (live `num_turns` and
   `last_event_at_unix` fields, recomputed on each read from the
   in-flight `events.jsonl`) or by attaching to ttyd in a browser.
8. On agent exit, the orchestrator parses the final `result` event,
   builds the result bundle (artifacts + events.jsonl + qwen-exit-code +
   qwen.stderr), persists `finished.json`, and tears down the sandbox
   (ttyd sidecar, inner proxy, both networks, outer proxy, staging tree
   — in that order). The session transitions to `completed` (or
   `cancelled` if the operator hit the cancel endpoint) and is read out
   of the on-disk record from then on.
9. The bundle and `finished.json` persist until `DELETE
   /v1/agent/sessions/<id>` removes them. There is no automatic
   pruning.

The agent inside the container has full shell access, the entire
vibe-web-terminal toolset, and Qwen Code's built-in subagent dispatch —
encouraged to delegate any non-trivial sub-task sequentially. **No GPU
access** for the agent container by design (the host's vLLM is the only
model surface).

---

## Architecture

```
┌──────────────────────────────────────────────────────────────────────────┐
│ HOST (public-internet-exposed)                                           │
│                                                                          │
│   vLLM 127.0.0.1:8001               ttyd publish 127.0.0.1:<eph>         │
│       ▲                                  ▲                               │
│       │ TCP                              │ TCP   (-p, browser)           │
│   ┌───┴──────────────────────────┐   ┌───┴────────────────────────────┐  │
│   │ OUTER PROXY CONTAINER        │   │ TTYD-SIDECAR CONTAINER         │  │
│   │  --network=host              │   │  --entrypoint socat            │  │
│   │  --entrypoint socat          │   │  primary: agent-pub-<id>       │  │
│   │  qwen-agent-template         │   │  also:    agent-net-<id>       │  │
│   │   UNIX-LISTEN:/sock/...      │   │   TCP-LISTEN:7681              │  │
│   │   → TCP:127.0.0.1:<vllm>     │   │   → TCP:<agent_ip>:7681        │  │
│   │  --user 1000 --read-only     │   │  --read-only --cap-drop ALL    │  │
│   │  --cap-drop ALL --memory 64m │   │  --user 1000 --memory 64m      │  │
│   └───┬──────────────────────────┘   └───┬────────────────────────────┘  │
│       │ Unix socket  (bind-mounted        │ on TWO networks (per session) │
│       │ into both proxies as /sock)       │                              │
│       │                          ┌────────┘                              │
│   ┌───┴──────────────────────────┴┐                                      │
│   │ INNER PROXY CONTAINER          │  --network=agent-net-<id>           │
│   │  --entrypoint socat            │   TCP-LISTEN:8001                   │
│   │   UNIX-CONNECT:/sock/...       │   → host's vLLM via Unix sock       │
│   │  --user 1000 --dns 127.0.0.1   │                                     │
│   │  --read-only --cap-drop ALL    │                                     │
│   └───┬────────────────────────────┘                                     │
│       │ TCP                                                              │
│       ▼                                                                  │
│   ┌──────────────────────────────────────────────────┐                   │
│   │ DOCKER NETWORK: agent-net-<id>                   │                   │
│   │   --internal                                     │                   │
│   │   gateway_mode_ipv4=isolated  (NO host iface IP) │                   │
│   │   no NAT, no internet, no host, no gateway       │                   │
│   └───┬──────────────────────────────────────────────┘                   │
│       │                                                                  │
│   ┌───┴─────────────────────────┐                                        │
│   │ AGENT CONTAINER             │  qwen → http://<inner_ip>:8001/v1     │
│   │  qwen-agent-template        │  ttyd 7681 (container-local; no `-p`) │
│   │  --dns 127.0.0.1            │  on agent-net-<id> ONLY                │
│   │  --user 1000 --cap-drop ALL │  → sidecar bridges to host loopback   │
│   │  no --gpus, no --privileged │                                        │
│   └─────────────────────────────┘                                        │
│                                                                          │
│   DOCKER NETWORK: agent-pub-<id>  (sidecar's `-p` lives here.            │
│     `--internal` is incompatible with `-p`, so this bridge is non-       │
│     internal but has enable_ip_masquerade=false to block outbound NAT    │
│     for the sidecar. The agent never touches it.)                        │
│                                                                          │
└──────────────────────────────────────────────────────────────────────────┘
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

### Why two hops to vLLM, both as containers

- **The agent is on a `--internal` + `gateway_mode_ipv4=isolated` Docker
  network.** `--internal` drops every packet whose destination isn't on
  the same network (no NAT to internet); `gateway_mode_ipv4=isolated`
  (Docker ≥ 27.1) additionally suppresses the host-side bridge IP, so the
  bridge has no gateway address that the agent could route to. Together:
  no NAT, no internet, no host loopback, no host services on `0.0.0.0`,
  no other Docker networks. The agent's `ip route` shows exactly one
  link-scope subnet entry — no `default via …`. **The only peer the
  agent can reach is the inner proxy** (and, after attach, the ttyd
  sidecar — see below). Pre-flight verifies the daemon honours the
  isolated-gateway flag; per-session post-create asserts the agent's
  routing table actually has no default route, catching any silent
  Docker-semantic regression at iteration 1, not iteration 50.
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

### Why ttyd publishes via a sidecar, not the agent's own `-p`

Docker silently drops `-p` port publishing on `--internal` networks.
This is a fundamental incompatibility, not a bug: an internal network has
no path to the host bridge by definition, so port-forwarding rules
cannot be installed. The original design tried to publish ttyd directly
from the agent container; the silent drop made `wait_for_ttyd` time out
on every run.

The fix is a per-session **socat sidecar dual-attached to two networks**:

- **`agent-net-<id>`** — the internal-isolated network where the agent
  lives. Sidecar joins this so it can reach the agent's port 7681.
- **`agent-pub-<id>`** — a per-session non-internal bridge with
  `enable_ip_masquerade=false`. Sidecar's primary interface; this is
  where `-p 127.0.0.1::7681` lives. Masquerade-off keeps NAT outbound
  shut even though the bridge isn't `--internal`.

Attach order is load-bearing: sidecar's primary network MUST be the
publish bridge (else `-p` is silently dropped); the agent network is
joined via `docker network connect` after `docker run`. The sidecar's
socat is fixed at `TCP-LISTEN:7681,fork → TCP:<agent_ip>:7681` — it can
only forward to the agent's ttyd, nowhere else, target frozen at process
start.

The agent itself is **only** on `agent-net-<id>` and never has any
interface on the publish bridge. Its `ip route` confirms this: one
link-scope entry, no default. So even if the sidecar's socat were
compromised, the agent has no path to follow it onto the publish bridge.

### Singleton, not a pool

For now, **exactly one** agent run can be in flight at a time. A second
`POST /v1/agent/sessions` while the first is running gets HTTP 409
(`busy`, with the running session's id in the envelope). The
architecture is structured (`tokio::sync::Semaphore` with one permit
inside `runtime::Manager`, `HashMap<id, RunningEntry>` already capable
of holding more than one entry, per-session cancel tokens already
threaded through) so growing into a small bounded pool is a mechanical
change once more GPUs come online. See the
[Multi-GPU growth path](#multi-gpu-growth-path-deliberate-forward-compat)
section below for the concrete steps.

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

Qwen Code 0.15.6 ships several built-in autonomous-run safeguards. The
image's `~/.qwen/settings.json` configures them as follows:

| Key | Value | What it does |
|---|---|---|
| `model.skipLoopDetection` | `true` | **Loop detection disabled.** Five streaming heuristics live in `packages/core/src/services/loopDetectionService.ts` (identical-tool-call repeat ×5, 50-char content chunk repeat ×10, repetitive structured thoughts ×3, excessive read-likes ≥8 in last 15, same-tool name with varying args ×8). They false-positive heavily on legitimate deep-codebase exploration where the agent calls `read`/`grep` repeatedly across many files in similar shapes — exactly our workload. We rely on the other backstops below (`maxSessionTurns`, `sessionTokenLimit`, the orchestrator's wall-clock cap) to catch genuinely runaway sessions. Matches Qwen Code's own default. |
| `model.skipNextSpeakerCheck` | `true` | Prevents the CLI from auto-injecting `"Please continue."` after empty turns. Auto-prodding is a footgun for Qwen3 — pinned explicitly to defend against future Qwen Code version changes. |
| `model.maxSessionTurns` | `200` | Last-resort turn cap. Run aborts with exit code 53 (`MAX_TURNS_EXCEEDED`) if the outer session-turn count exceeds it. Read raw at `client.ts:709-710` with no internal clamp. CLI flag `--max-session-turns` (driven by `AGENT_SERVICE_MAX_TURNS`) overrides the settings-file value when present. The `MAX_TURNS = 100` constant at `client.ts:96` is unrelated — it's a recursion-depth bound on `sendMessageStream`, not a cap on session turns. **Why 200 and not lower**: `sessionTokenLimit` + the orchestrator's wall-clock catch the common stuck-mode failures earlier; the turn cap is just the "the model is making progress but we're way past any plausible legitimate run length" backstop. |
| `model.sessionTokenLimit` | `262144` | A **most-recent-prompt-token** cap, not cumulative. Compared at `client.ts:731-747` against `lastPromptTokenCount` (from `uiTelemetry.ts:147,180-186` — `totalTokenCount` of the most recent API response, including cached tokens). Acts as a "this request would have OOM'd or hit max-model-len" backstop; aborts the session cleanly with the run's `result` event still emitted. 262144 is Qwen3.6's `max_position_embeddings`; in practice the parent project's vLLM is configured to 152000, so vLLM rejects with HTTP 400 first and this almost never fires directly. Qwen Code's default is `-1` (disabled). |

These are independent of the sampling configuration below — none of them
require changing `presence_penalty` away from the AWQ-recipe-mandated
`0.0`.

The orchestrator-side backstops layered on top of these:

- **Wall-clock cap** (`AGENT_SERVICE_TIMEOUT_SECS`, default 7200 s = 2 h):
  the outer-most "kill the container regardless" timer.
- **Singleton + cancellation:** the operator can `POST .../cancel` at
  any time; the cancel races against `docker wait`, then `docker stop`
  + re-await for the exit code, then teardown.
- **No `events.jsonl` mtime advance** is observable on the running
  `SessionBody` (`last_event_at_unix`); operators can spot a wedged
  agent without waiting for the wall-clock cap.

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
  "max_tokens": 32768
}
```

The temperature / top-p / top-k / min-p / penalty values are the
thinking-mode "higher-quality" recipe from the QuantTrio/Qwen3.6-27B-AWQ
model card — they accept slightly higher infinite-loop risk in exchange
for noticeably better output on math and code (`presence_penalty=0.0`
per Alibaba's published recommendation; the `linear_attn.in_proj_a/b`
layers in the AWQ recipe are kept at BF16 specifically to mitigate the
loop pathology, see the parent project's README §3.1).

`max_tokens = 32,768` is **Alibaba's general-tasks recommendation** for
this model (from the card's "Best Practices" block). The card publishes
two values: `32,768` for general tasks (our regime — agentic coding
with thinking on), and `81,920` for "highly complex math / programming
**competition** outputs". The 81,920 value is also the parent project's
server-side default via patch §7.6 for clients that send no
`max_tokens`; we send it explicitly here, so the agent operates at the
general-tasks value rather than the benchmarking-grade one. Lowering it
further would clip the model's `<think>` blocks mid-thought, which
corrupts agent loops — the model's training expects up to ~32K of
thinking-plus-answer per turn and we honour that.

`contextWindowSize = 120,000` is **deliberately aligned to the
deployment's effective prompt budget**, namely
`max_model_len − max_tokens = 152,000 − 32,768 ≈ 119,232 → 120,000`.
The parent project's `max_model_len` is hardware-bound at 152,000
(parent README §5.2 — 9.7 GiB KV pool at gmu=0.97 → 158,368 boot KV
tokens, 1.04× concurrency at full max-len). With qwen-code's default
`chatCompression.contextPercentageThreshold=0.7`, history compaction
fires at 84,000 prompt tokens — well below vLLM's hard 120K cap, so
qwen-code's client-side machinery has room to react before vLLM has to
return HTTP 400. Setting `contextWindowSize` higher than the effective
prompt budget (e.g., qwen-code's docs example of `131,072`) lets the
client send prompts that vLLM will then reject — which manifests as a
terminal `completed` session whose `response` is the vLLM error string.
Don't do that.

Note: Alibaba's model card recommends "**at least 128K tokens** to
preserve thinking capabilities". Our effective prompt budget is 120K,
**8K below that floor** — a structural compromise from running 27B AWQ
+ full-fidelity vision + BF16 KV on a 32 GB card. Pushing
`max_model_len` higher costs other invariants the parent project has
chosen to defend (concurrency slack, cold-path 4-MP image safety,
§5.2). The deficit is hardware-bound, not tunable here.

Vision is enabled (`modalities.image=true, modalities.video=true`) and
`splitToolMedia=true` is set as documented in the parent project's §5.8.
Reasoning is on, defaulted to `enable_thinking=true` server-side; the
client emits no `chat_template_kwargs` (verified by source grep against
v0.15.6) so the server defaults always land.

---

## Security properties

- **Listens only on loopback.** The service refuses to bind anywhere
  else. Verified at startup by `config::parse_listen_addr`.
- **Network isolation in depth — strictly stronger than `--internal`
  alone.** The agent container is on a `--internal` +
  `com.docker.network.bridge.gateway_mode_ipv4=isolated` Docker network.
  `--internal` blocks NAT outbound; the isolated-gateway flag (Docker ≥
  27.1) suppresses the host-side bridge IP entirely, so the bridge has
  **no gateway address** for the agent to route to. Verified live: the
  agent's `ip route` shows exactly one link-scope subnet entry, no
  `default via …`. As a consequence, host services bound on `0.0.0.0`
  (e.g. SSHd) — which were reachable on a pristine `--internal` bridge
  via the bridge gateway — are now unreachable from the agent. The only
  peer the agent can reach is the inner proxy (and, after sidecar
  attach, the ttyd sidecar — see "Why ttyd publishes via a sidecar"
  above). The inner proxy can only forward to the bind-mounted Unix
  socket; the outer proxy is the only thing on that socket and only
  forwards to one fixed TCP destination on the host
  (`127.0.0.1:<vllm_port>`). socat is byte-stupid — it doesn't parse,
  it can't be redirected by client traffic.
- **Per-session post-create assertion.** Immediately after `docker run`
  of the agent, the orchestrator runs `ip -4 route show` inside the
  container and refuses to proceed if it sees any `default via …` line.
  This catches a Docker-semantic regression (e.g. an upgrade that
  silently changes `gateway_mode_ipv4=isolated` behaviour) on iteration
  1, not iteration 50.
- **Pre-flight isolated-gateway probe at boot.** Before binding the
  listener, the orchestrator creates a throwaway labelled network with
  `--internal -o com.docker.network.bridge.gateway_mode_ipv4=isolated`
  and immediately removes it. Refuses to start if the daemon doesn't
  honour the flag — the design's primary isolation primitive is
  fail-loud, not silently degraded.
- **DNS exfiltration is closed.** `--internal` networks alone are not
  sufficient — Docker's embedded resolver at `127.0.0.11` forwards
  queries via the daemon namespace and *does* reach external DNS on
  non-internal networks. Every container in the chain (agent, inner
  proxy, outer proxy, ttyd sidecar) is started with `--dns 127.0.0.1
  --dns-search .` — resolv.conf points at a non-listening loopback, so
  every DNS lookup fails immediately. The agent reaches the inner proxy
  by IP literal, never by name.
- **No GPU access** for any per-session container. Verified by
  inspection of every `run_detached` call site in `src/network.rs` and
  `src/session.rs`.
- **No `--privileged` and no `CAP_*` adds anywhere** beyond the agent's
  unused `NET_BIND_SERVICE` (kept for the case where a future operator
  lowers the ttyd port below 1024).
- **All proxies, the ttyd sidecar, and the agent are `--cap-drop ALL
  --user 1000:1000 --read-only --security-opt no-new-privileges`.**
  Each proxy + the sidecar are additionally `--memory 64m --pids-limit
  32` (small because each runs one socat process and nothing else); the
  agent gets **`--memory 32g --memory-swap 32g`** (no swap) and
  **`--storage-opt size=128g`** (writable-layer cap), with
  `--pids-limit 4096`. Both limits are configurable via
  `AGENT_SERVICE_MEMORY` and `AGENT_SERVICE_STORAGE_QUOTA`.
- **The ttyd sidecar's socat target is fixed at process start** —
  `TCP-LISTEN:7681,fork → TCP:<agent_ip>:7681`. It cannot be redirected
  to a different destination by client traffic. The sidecar's socat
  argv is fully constructed by the orchestrator from a parsed
  `IpAddr` (rejected if not an IPv4 literal), never from any
  user-controlled string.
- **The user's source folder is *copied***, not bind-mounted, into a
  per-session staging tree (mode-normalised to 0o755 dirs / 0o644
  files) before the agent sees it. The agent cannot reach back into
  the user's actual working tree.
- **Symlinks inside the source folder are rejected outright** at
  validation time.
- **Orphan sweep on startup.** Every Docker object the service creates
  is labelled `agent_service.session=<uuid>`. If the orchestrator
  crashes mid-session, the next start `docker rm`s every container
  (including the ttyd sidecar) and `docker network rm`s every network
  (both the agent network and the per-session publish bridge) bearing
  that label, then begins serving. Mid-creation crash recovery is
  walked through end-to-end in `src/network.rs`.

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
# export AGENT_SERVICE_TIMEOUT_SECS=7200                 # 2h wall-clock cap
# export AGENT_SERVICE_MAX_TURNS=200                     # last-resort turn cap (1..=1024)

./target/release/agent_service
```

Pre-flight verifies:

- The Docker daemon is reachable as the running user (no root needed —
  the user must be in the `docker` group).
- `AGENT_SERVICE_IMAGE` is present locally (both proxies, the ttyd
  sidecar, and the agent reuse it).
- The Docker daemon honours
  `com.docker.network.bridge.gateway_mode_ipv4=isolated` (Docker ≥
  27.1). Probed by creating a throwaway labelled network with that
  option and removing it; refuses to start otherwise — this is the
  agent network's primary isolation primitive and we will not silently
  degrade to a weaker sandbox.
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
  staging directories under `<state_dir>/sessions/` are removed; any
  per-session result directory under `<results_dir>/` that is missing
  `finished.json` (a partially-written record from a crashed server) is
  removed. **Completed result bundles are not swept** — the lifecycle
  is explicit, so a session lives until `DELETE`.
- `AGENT_SERVICE_LISTEN_ADDR` resolves to a loopback address — the
  service refuses to bind anywhere else, full stop.

The service exits non-zero with a single human-readable line on any
pre-flight failure. Exit codes: `2` config, `10` daemon unreachable,
`11` image missing, `12` other internal, `1` bind / runtime.

---

## API

Listening on `127.0.0.1:8090` only. No TLS — loopback-only; if you tunnel
this, that's the caller's job.

The resource is a `session`. Every session-related endpoint returns the
same `SessionBody` shape, discriminated by `status`
(`running` | `completed` | `cancelled`). Required-field discipline:
every field is always serialised, with running-only fields zeroed for
terminal states and terminal-only fields zeroed while running, so
clients have one parser.

| Method | Path | What it does | Success status |
|---|---|---|---|
| `POST`   | `/v1/agent/sessions`               | Create. Body `{prompt, folder}`. Blocks until ttyd is reachable. | `201 Created` |
| `GET`    | `/v1/agent/sessions`               | List. Combines in-memory running session (≤1) with on-disk terminal records. Sorted by `started_at_unix`. | `200 OK` |
| `GET`    | `/v1/agent/sessions/{id}`          | Pure read; idempotent. | `200 OK` / `404` |
| `POST`   | `/v1/agent/sessions/{id}/cancel`   | Cancel; idempotent. Awaits teardown so the returned body reflects the final state. | `200 OK` |
| `DELETE` | `/v1/agent/sessions/{id}`          | Remove a terminal session's bundle + record from disk. Refuses while running (`409`). | `204 No Content` / `404` / `409` |
| `GET`    | `/healthz`                         | Plain-text `ok`. For supervisor probes. | `200 OK` |

There is **no** time-, count-, or read-based eviction anywhere. Reads
never mutate. Writes (cancel, delete) are idempotent.

### `POST /v1/agent/sessions` — create

Request body — both fields **required**:

```json
{
  "prompt": "Reproduce the bug in tests/regress.test.ts and fix it.",
  "folder": "/home/user/projects/widget-server"
}
```

Validation:

- `prompt` is non-empty after trimming, contains no NUL byte. The
  effective size cap is the request-body limit (`256 KiB`) imposed
  before validation runs.
- `folder` is an absolute path to an existing directory, with no
  symlinks anywhere in the tree, ≤ 4 GiB total, ≤ 200 000 files. System
  paths (`/`, `/etc`, `/proc`, `/sys`, `/dev`, `/boot`, `/var/run`,
  `/run`) are rejected.

The call **blocks** until the agent's ttyd listener is reachable through
the per-session sidecar (typically a few seconds). On success: `201
Created` with the running `SessionBody` (see schema below); the
`ttyd_url` field carries the URL the operator opens in a browser. Until
the agent finishes, the same body is observable through `GET .../{id}`
with live `num_turns` and `last_event_at_unix` updated on each read.

### `GET /v1/agent/sessions/{id}` — read one

Pure read of one session. Falls back to disk if the session is no longer
running. `404` for unknown ids (never submitted, already DELETE'd, or
lost in a server crash — running sessions do not survive process
restart by design).

### `GET /v1/agent/sessions` — list

```json
{
  "sessions": [
    { /* SessionBody for s-aaa... */ },
    { /* SessionBody for s-bbb... */ }
  ]
}
```

Combines the in-memory running session (at most one, by singleton) with
every on-disk terminal record under `<results_dir>/`. Sorted by
`started_at_unix` ascending. Skips half-written records that are
missing `finished.json` (they appear once the rename lands).

### `POST /v1/agent/sessions/{id}/cancel` — cancel

Idempotent. Cancels the per-session token; the run task observes it,
issues `docker stop` (10 s grace) + re-await for the exit code, runs
teardown (uninterruptible), and persists `finished.json`. The HTTP call
**awaits** all of that, so the returned body reflects the final state
— `status: "cancelled"` for a successful cancel; `status: "completed"`
if the session had already terminated by the time the cancel arrived.
A cancel on a session id that doesn't exist returns `404`.

### `DELETE /v1/agent/sessions/{id}` — delete a terminal record

Removes `<results_dir>/<id>/` (the persisted body and bundle).
Returns:

- `204 No Content` on success.
- `404 Not Found` if the id is unknown.
- `409 Conflict` (`kind: session_running`) if the session is still
  running. The lifecycle is explicit: cancel first, then delete.

### `SessionBody` schema

Every session-related read returns this shape (running-only fields
zeroed for terminal records and vice versa). Field semantics:

```jsonc
{
  // Always populated
  "session_id":         "s-1f0e7b...",
  "status":             "running" | "completed" | "cancelled",
  "started_at_unix":    1746115234,
  "ttyd_url":           "http://127.0.0.1:51234/",   // empty string for sessions that
                                                      // failed before ttyd-up
  "prompt_preview":     "Reproduce the bug in ...",  // first 200 chars of the submitted prompt

  // Live progress (running) / frozen-at-end (terminal)
  "num_turns":          17,          // count of distinct LLM invocations: contiguous runs
                                     //   of `"type":"assistant"` lines in events.jsonl
                                     //   collapse to one (each invocation emits multiple
                                     //   assistant lines, one per content block).
  "last_event_at_unix": 1746115512,  // mtime of events.jsonl. Stops advancing if the
                                     //   agent is wedged.

  // Populated only on terminal status; zero/empty while running
  "finished_at_unix":            1746116012,
  "duration_wall_ms":            778431,
  "container_exit_code":         0,    // `docker wait` on the agent container — usually 0
                                       //   even when qwen failed (the wrapper exits cleanly
                                       //   through ttyd's SIGTERM)
  "agent_exit_code":             0,    // qwen-code's actual exit, read from
                                       //   output/qwen-exit-code:
                                       //   0   normal completion
                                       //   53  hit max-session-turns
                                       //   137 SIGKILL'd by docker stop after a cancel
                                       //   96  wrapper: PROMPT env was empty
                                       //   97  init:    no PROMPT or RUN_AGENT script
                                       //   -1  setup failed before the wrapper ran
  "is_process_error":            false,// qwen-code itself errored (structured error
                                       //   envelope, mid-run crash, or pre-ttyd setup
                                       //   failure). Does NOT mean "the response is
                                       //   useful": a vLLM 400 that becomes the agent's
                                       //   final answer leaves this false. Inspect
                                       //   `response` for wire-error envelopes if needed.
  "response":                    "<the agent's final answer text>",
  "agent_duration_ms":           776543,
  "bundle_archive_path":         "/home/user/.local/state/agent_service/results/s-1f0e7b/bundle.tar.zst",
  "bundle_compressed_bytes":     1284412,
  "bundle_uncompressed_bytes":   5938204,
  "bundle_file_count":           14,
  "bundle_artifacts_file_count": 12,
  "teardown_diagnostics":        []    // human-readable strings; non-empty on partial failure
}
```

Note that `status: "completed"` does NOT mean "succeeded" — it means
the run reached a terminal state through normal flow (as opposed to
`cancelled` via the cancel endpoint). `is_process_error` distinguishes
qwen-code-process-level failures (crash, structured-error envelope,
pre-ttyd setup failure — these produce a `completed` body with
`is_process_error: true` and `agent_exit_code: -1`, with the
`response` carrying a human-readable explanation) from clean process
exits. It does **not** detect the case where qwen-code ran cleanly
but the agent's final answer is itself an error string (e.g., a vLLM
HTTP 400 echoed back by the model as its last assistant turn) — for
that, inspect `response` directly.

### Result bundle

**The bundle archive is the agent's primary output channel for files.**
It lives at `bundle_archive_path` (always populated on a terminal
record; empty string only if bundle creation failed, in which case
`teardown_diagnostics` explains why). Inside `bundle.tar.zst`:

```
artifacts/             ← whatever the agent wrote to /artifacts/. The
                         agent is told this directory is empty at start
                         and that anything written there is returned.
output/events.jsonl    ← full structured event stream (the same stream
                         ttyd renders live during the run); forensics
output/qwen-exit-code  ← qwen-code's numeric exit code (one line)
output/qwen.stderr     ← qwen-code's stderr; usually empty, populated
                         on internal qwen errors
```

`bundle_artifacts_file_count` separates the agent's intentional output
from the forensics sidecars, so callers can quickly spot a run that
produced no artefacts.

The session's *staging* directory (under
`<state_dir>/sessions/<session_id>/`) is removed at the end of the run
regardless — only the bundle persists, until `DELETE`.

### Error envelope

Every failure response carries the same shape:

```json
{ "error": "<message>", "kind": "<machine-readable kind>", "session_id": "" }
```

| HTTP | `kind` | When |
|---|---|---|
| 400 | `invalid_request`        | Body validation failed (prompt, folder, …). |
| 404 | `not_found`              | Unknown session id. |
| 409 | `busy`                   | Singleton already held; `session_id` carries the running id. |
| 409 | `session_running`        | `DELETE` against a still-running session; `session_id` carries the offending id. Cancel first. |
| 502 | `docker_command_failed`  | A `docker` subprocess returned non-zero. |
| 502 | `agent_output_missing`   | The agent ran but produced no parseable `result` event. |
| 503 | `docker_unavailable`     | Daemon not reachable as the running user. |
| 503 | `image_missing`          | `AGENT_SERVICE_IMAGE` is not present locally. |
| 504 | `timeout`                | A wall-clock cap was hit. |
| 500 | `staging_failed`         | Host-side filesystem failure during staging / bundle. |
| 500 | `internal`               | Anything else. |

The `session_id` field is always present (empty string when not
applicable) — clients have one parser for every error response.

---

## Example session

```bash
# Terminal 1 — start the service.
./target/release/agent_service

# Terminal 2 — submit a session. The POST blocks for a few seconds
# until ttyd is reachable, then returns 201 with the running body.
SID=$(curl -sS -X POST -H 'Content-Type: application/json' \
  --data '{
    "prompt":"Find out why tests/test_login.py::test_session_expiry fails and fix it. Use subagents to investigate independent code paths sequentially. Final answer: a unified diff of the fix and a one-paragraph root-cause explanation.",
    "folder":"/home/user/projects/myapp"
  }' \
  http://127.0.0.1:8090/v1/agent/sessions \
  | tee /dev/stderr | jq -r '.session_id')

# Open the ttyd_url field from that JSON in a browser to watch.

# Poll for completion. Live `num_turns` and `last_event_at_unix`
# advance while the agent runs; `status` flips to "completed" or
# "cancelled" at the end.
while :; do
  BODY=$(curl -sS http://127.0.0.1:8090/v1/agent/sessions/"$SID")
  STATUS=$(echo "$BODY" | jq -r .status)
  echo "$(date +%H:%M:%S) status=$STATUS turn=$(echo "$BODY" | jq -r .num_turns)"
  [ "$STATUS" != "running" ] && break
  sleep 30
done
echo "$BODY" | jq

# Cancel mid-run if needed:
#   curl -sS -X POST http://127.0.0.1:8090/v1/agent/sessions/"$SID"/cancel | jq

# Once you're done with the bundle, delete it:
#   curl -sS -X DELETE http://127.0.0.1:8090/v1/agent/sessions/"$SID" -o /dev/null -w '%{http_code}\n'
```

The agent runs to completion regardless of whether you keep polling,
hold the ttyd browser tab open, or even disconnect entirely — the run
task is detached from the HTTP request. The singleton enforces "one
agent at a time", and `DELETE` is the only thing that removes a
record.

---

## Repository layout

```
agent_service/
├── Cargo.toml               # strict-= pinned deps
├── README.md                # this file
├── docker/
│   ├── Dockerfile           # qwen-agent-template:0.1.0 (agent + socat + ttyd + tools)
│   └── config/
│       ├── settings.json    # ~/.qwen/settings.json (sampling, modalities, safeguards)
│       ├── QWEN.md          # ~/.qwen/QWEN.md (operating instructions)
│       ├── agent_init.sh    # container CMD: tmux + ttyd in read-only attach mode
│       └── run_agent.sh     # in-tmux: qwen | tee /output/events.jsonl
└── src/
    ├── main.rs              # bootstrap, listener, signals, graceful shutdown
    ├── api.rs               # axum routes (lifecycle CRUD) + pre-flight
    ├── runtime.rs           # Manager: singleton, in-memory map of running sessions,
    │                        #   on-disk persistence of terminal records, cancellation
    │                        #   token tree, SessionBody wire shape
    ├── session.rs           # run_one: per-session orchestration
    │                        #   (validate → stage → network → spawn → wait → bundle → tear down)
    ├── network.rs           # IsolatedNetwork: agent-net (--internal + isolated gateway),
    │                        #   agent-pub (sidecar publish bridge), outer/inner socat,
    │                        #   ttyd sidecar, post-create no-default-route assertion
    ├── docker_ops.rs        # subprocess wrappers around `docker` (ping, image, run, …)
    ├── bundle.rs            # tar.zst result bundle (artifacts + events.jsonl
    │                        #   + qwen-exit-code + qwen.stderr)
    ├── result_parse.rs      # parse events.jsonl → AgentResult
    ├── staging.rs           # per-session paths + copy-with-perms
    ├── validation.rs        # prompt + folder validation
    ├── config.rs            # env-driven config (loopback-only enforced)
    └── error.rs             # ServiceError + IntoResponse + WireError
```

---

## Multi-GPU growth path (deliberate forward-compat)

The singleton today is a `tokio::sync::Semaphore` with one permit
inside `runtime::Manager`; the in-memory map is already a
`HashMap<String, Arc<RunningEntry>>` capable of holding more than one
running entry, and the cancellation tree already gives each session
its own child token. To grow into a bounded pool when more GPUs come
online:

1. Bump `Semaphore::new(1)` to `Semaphore::new(N)` in
   `runtime::Manager::new`. The `try_acquire_owned` call in `submit`
   already returns `Busy` only when *no* permit is available; everything
   else just works.
2. Route per session to a target vLLM endpoint — either let the caller
   pass it in `CreateRequest`, or run one `agent_service` per GPU and
   put a tiny round-robin in front.
3. No network or container-name changes are needed. Per-session names
   are already `{agent-net,agent-pub,agent-{outproxy,inproxy,sidecar},agent}-<uuid>`,
   so they don't collide across concurrent sessions.

The Rust side took 5 minutes to design with this in mind. The Docker
side already supports it because every per-session object carries a
unique session id.

---

## License

Same as the parent `qwen_36_agent_setup` project.
