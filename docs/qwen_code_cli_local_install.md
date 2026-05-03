# Installing Qwen Code CLI on a local dev machine

This documents the exact install performed on the dev box at
`/home/user/Desktop/agent_service` so a developer can run `qwen`
interactively against the same vLLM endpoint the agent_service
orchestrates inside containers.

The qwen-code version, sampling block, modalities, reliability
safeguards, and telemetry-off discipline match the project's
container setup (`docker/Dockerfile`, `docker/config/settings.json`).
The only intentional differences are:

- **`approvalMode: "default"`** instead of `"yolo"` (interactive use —
  qwen-code prompts before each tool call rather than auto-approving).
- **`general.checkpointing.enabled: true`** instead of `false`
  (resume across crashes; off in the container because the container
  is ephemeral).
- **No `~/.qwen/QWEN.md`.** qwen-code's default built-in system
  prompt is kept; the container's custom QWEN.md describes a sealed
  environment that doesn't exist here.
- **No `NO_COLOR=1` env var.** That's for clean JSONL stream parsing
  in headless mode; the interactive TUI wants colors.

## Prerequisites

The parent `qwen_36_agent_setup` project must be running and serving
the patched vLLM on `127.0.0.1:8001`. Verify:

```bash
curl -fsS http://127.0.0.1:8001/v1/models | jq -r '.data[].id'
# → Qwen3.6-27B-AWQ
```

All 12 server-side patches (parent README §7) are live in that
process; this dev install does not need any of them client-side.

## Step 1 — install nvm per-user (no sudo)

```bash
curl -fsSL https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.3/install.sh | bash
```

Writes `~/.nvm/`, appends nvm-init lines to `~/.bashrc`, and exits
without touching anything system-wide.

## Step 2 — install Node 22 via nvm

```bash
export NVM_DIR="$HOME/.nvm" && . "$NVM_DIR/nvm.sh"
nvm install 22
# → Node v22.22.2, npm v10.9.7 (matches the NodeSource 22.x line the
#   project's Dockerfile uses)
```

qwen-code 0.15.6 hard-rejects Node <20.

## Step 3 — install pinned qwen-code 0.15.6

```bash
npm install -g @qwen-code/qwen-code@0.15.6
qwen --version    # → 0.15.6
```

Pin matches the project's pinned version
(`agent_service/src/config.rs:13` → `QWEN_CODE_VERSION = "0.15.6"`).
Auto-update is disabled separately via the env var below so the pin
holds across `qwen` invocations.

## Step 4 — write `~/.qwen/settings.json`

Save the following at `/home/user/.qwen/settings.json`:

```json
{
  "modelProviders": {
    "openai": [
      {
        "id": "Qwen3.6-27B-AWQ",
        "name": "Qwen3.6-27B-AWQ",
        "baseUrl": "http://127.0.0.1:8001/v1",
        "envKey": "OPENAI_API_KEY"
      }
    ]
  },
  "env": {
    "OPENAI_API_KEY": "dummy-not-checked-by-vllm-but-required-by-client"
  },
  "security": {
    "auth": { "selectedType": "openai" }
  },
  "model": {
    "name": "Qwen3.6-27B-AWQ",
    "skipLoopDetection": true,
    "skipNextSpeakerCheck": true,
    "maxSessionTurns": 200,
    "sessionTokenLimit": 262144,
    "generationConfig": {
      "modalities": { "image": true, "video": true },
      "splitToolMedia": true,
      "contextWindowSize": 152000,
      "samplingParams": {
        "temperature": 0.6,
        "top_p": 0.95,
        "top_k": 20,
        "min_p": 0.0,
        "presence_penalty": 0.0,
        "repetition_penalty": 1.0,
        "max_tokens": 32768
      }
    }
  },
  "tools": {
    "approvalMode": "default",
    "sandboxImage": ""
  },
  "general": { "checkpointing": { "enabled": true } },
  "telemetry": { "enabled": false }
}
```

### Why `modelProviders.openai[0].id` must equal `model.name`

This is the subtle gotcha. qwen-code's provider resolution
(`cli.js:464329`) does:

```js
modelProvider = providers.find(p => p.id === resolvedModel);
```

`resolvedModel` comes from `settings.model.name`. Only when the id
matches does `modelProvider.baseUrl` get pushed onto the resolution
layer stack (`cli.js:279849-279858`'s `baseUrlLayers`), where it
becomes layer #1 ahead of CLI flag, env var (`OPENAI_BASE_URL`), and
`security.auth.baseUrl`. `resolveOptionalField` returns the first
present value, so layer #1 wins.

If id and `model.name` disagree, the modelProvider entry is silently
unused and qwen-code falls through to whatever the OpenAI SDK
resolves on its own — `https://api.openai.com/v1` by default — which
is what produced the `401` from `dashscope.aliyuncs.com` we saw
during this install (the SDK's default chain landed on Alibaba's
endpoint).

The agent_service container's `docker/config/settings.json` happens
to use `id: "qwen36-vllm"` ≠ `model.name: "Qwen3.6-27B-AWQ"` and
"works" only because the orchestrator injects `OPENAI_BASE_URL` at
container start (`src/session.rs:168`), which fires the env layer
(#3) instead. On bare metal we don't want a global `OPENAI_BASE_URL`
in our shell — too general a name, would shadow the OpenAI SDK in
every other tool. So we use the modelProviders match the way it was
designed.

### Knob-by-knob rationale

| Setting | Why |
|---|---|
| `model.name = "Qwen3.6-27B-AWQ"` | matches `/v1/models` advertised id |
| `generationConfig.contextWindowSize = 152000` | matches vLLM `--max-model-len` (parent README §5.2) |
| `generationConfig.modalities.image/video = true` | bypass qwen-code's `defaultModalities()` text-only fallback that would otherwise replace every `image_url` with `[Unsupported image file: …]` before the request leaves the client (parent README §5.8) |
| `generationConfig.splitToolMedia = true` | redundant with the parent project's vLLM patch §7.9 (tool-role media preserve), kept defensively |
| `generationConfig.samplingParams` | QuantTrio AWQ thinking-mode "Best Practices" recipe; explicit values bypass the parent project's server-side patch §7.6 defaults (Pydantic `model_fields_set`) |
| `samplingParams.max_tokens = 32768` | Alibaba's "general tasks" recommendation (the server's patch §7.6 default for unset is `81920` — Alibaba's "competition" value) |
| `model.skipLoopDetection`, `skipNextSpeakerCheck`, `maxSessionTurns`, `sessionTokenLimit` | qwen-code-internal reliability safeguards (agent_service README §"Reliability configuration") |
| `tools.approvalMode = "default"` | interactive — CLI prompts before each tool call. Set to `"yolo"` if you want the container's auto-approve behaviour |
| `tools.sandboxImage = ""` | no nested sandbox container |
| `general.checkpointing.enabled = true` | resume across crashes (off in the container since it's ephemeral) |
| `telemetry.enabled = false` | no usage data sent |
| `env.OPENAI_API_KEY = "dummy-..."` | injected into `process.env` by qwen-code at startup; vLLM doesn't actually check it (no `--api-key` on the parent project's launch line), but the OpenAI SDK refuses to construct without one |

## Step 5 — add three Qwen-namespaced env vars to `~/.bashrc`

These are intentionally `QWEN_*`-prefixed (scoped to qwen-code by
name; do not shadow other tools):

```bash
# qwen-code: pinned to 0.15.6 (no auto-update), telemetry off
export QWEN_TELEMETRY_ENABLED=false
export QWEN_DISABLE_AUTOUPDATE=true
export QWEN_SANDBOX=false
```

`QWEN_TELEMETRY_ENABLED=false` is belt-and-braces with
`telemetry.enabled: false` in settings.json.
`QWEN_DISABLE_AUTOUPDATE=true` is what holds the 0.15.6 pin —
without it qwen will silently upgrade itself. `QWEN_SANDBOX=false`
disables qwen-code's nested-container sandbox (we don't have a
sandbox image configured).

## Step 6 — verify

Open a new shell (so nvm + the env vars load) and run:

```bash
qwen --version                              # → 0.15.6
qwen -p "Reply with literally just OK"      # → OK
```

If the second command returns `API Error: 401 Incorrect API key`
with a link to `help.aliyun.com/zh/model-studio/error-code`, the
`modelProviders.openai[0].id` is not equal to `model.name` — see
"Why `modelProviders.openai[0].id` must equal `model.name`" above.

## Then `qwen` interactively

```bash
cd /path/to/some/project
qwen
```

Launches the TUI. Tool calls prompt for confirmation
(`approvalMode: "default"`). All requests flow to
`http://127.0.0.1:8001/v1` against the patched vLLM.
