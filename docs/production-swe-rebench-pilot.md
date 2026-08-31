# Production agent-service SWE-rebench pilot

## Result

The accepted `agent_service` release solved the pinned SWE-rebench pilot task.
This was an end-to-end production-agent test, not a direct model benchmark: every
model-bearing turn ran inside the pinned Qwen Code agent image after a real
`POST /v1/agent/sessions`, and completion was obtained from the production
`/wait` endpoint. Harbor supplied the task and the immutable post-session
evaluator only. It was never configured as a model adapter and never called vLLM.

| Measurement | Production session |
|---|---:|
| Production session | `s-133dfd0c3b2643fa8291ab45c9508116` |
| Terminal status | completed |
| Evaluator reward | 1 |
| Evaluator tests | 11 passed, 0 failed/error/skipped |
| Model turns | 61 |
| Native tool calls/results | 60 / 60 |
| Tool-result errors handled in-thread | 5 |
| Wall duration | 1,090,658 ms |
| Agent duration | 1,084,315 ms |
| Qwen Code aggregate input tokens | 3,407,979 |
| Qwen Code aggregate output tokens | 38,461 |
| Qwen Code aggregate total tokens | 3,446,440 |
| Candidate patch bytes | 5,301 |
| Candidate patch SHA-256 | `d4e69c838e9349ff20189893def6e5c1d3e5301efd07c822cacbf74dac08fe36` |
| Production bundle SHA-256 | `4ac6f5729017a111cb5dde1c30110a7d5520af1059c7e529234073d42c519f3a` |

One completed task is a lifecycle proof for the production pair — real session
creation, real turns, a clean terminal bundle, and an independent evaluator
pass — not a benchmark-suite score and not a population-level quality or
latency claim.

The Qwen Code result object's `cache_read_input_tokens` compatibility field was
zero. This deployment already treats that frontend field as non-authoritative;
the backend's own metrics are the authority for physical prefix-cache hits. The
aggregate input figure above describes rendered request traffic and must not be
misreported as the amount of attention computation or cache misses.

## Immutable inputs and production release

- Dataset: `ibragim-badertdinov/swe-rebench-07-2026@2026-07`
- Dataset OCI digest:
  `sha256:e2e357045bf03e4900d2506c36562f6eaff7acd37f63780600967ea3aecdcd79`
- Harbor: `0.21.0`, commit
  `64afbbcb62165950301e1a6407c729aa26d844ff`
- Task: `Gentleman-Programming__gentle-ai-595`
- Task base commit: `36051a1b41d879b1bf76f4aa9aa984d74e54c26d`
- Task base image:
  `sha256:4714a9461b2e40cfb122afa32c2c7ad6b154f59e0f9239ae1610018e05fa2029`
- Realized evaluator image:
  `sha256:8abec1b1eb1f496fe1762f74f7b0cc535ac287683b8b81316c1e35ef34c819c2`
- Preserved evaluator archive SHA-256:
  `2dd8689b6568f5b8742a41dbf050f8dd2aeb48ff40b42c6624c98e985a85e069`
- Agent-service release commit:
  `7a329f61665a7126e3f8cd9a4e3b7a6b66a639bc`
- Release implementation commit:
  `bc67dae720894cbbcd62122a2a9ff6b56b042168`
- Release-lock SHA-256:
  `a43ffd0738749771fda13ce4d4b491e58356e2f0be430880334747ac5761f5d4`
- Stack-lock SHA-256:
  `de1307bd8598cd928191b1a0947c086fcb9af2cc91c17c4488f70d06ca528de3`
- vLLM image:
  `sha256:587e8710c6630edd249f19b46837c12ebe5b5dcdc98486e215ac48a66644dc7f`
- Agent image:
  `sha256:1dc84a6f4e03b62a9540794a353c0b1e175a07e6afbcfed6441fe5f2d0f7d1ec`
- Broker image:
  `sha256:f9d3b77ed2e10d69648c2e443fa5e49ff06fca7eedf6fc580f9d8762d9bfb054`
- Service image:
  `sha256:8f8d4b2e68bf47c9d92c6c5c0f77fdbf60d0056ef32155a34ecc96357dfd41f4`

The complete per-file task hashes, exact evaluator materialization observation,
timeouts, CPU/memory limits, and invariants are in the tracked benchmark
lock. The evaluator image is the rerun authority: its Dockerfile downloaded the
`uv` installer over HTTPS, and that installer explicitly reported that it had no
checksums to verify. Repeating that mutable network fetch would not reproduce the
accepted environment as strongly as loading the preserved, hashed image archive.

## Execution and evidence boundary

The executed harness performs these steps and fails on any mismatch:

1. Require the clean, exact release commit and validate the release lock, stack
   lock, image IDs, live production status, task hashes, and absence of a running
   session.
2. Copy the exact base-commit checkout into a fresh source directory for the run.
3. Submit `{folder, prompt}` to production `POST /v1/agent/sessions` and verify
   the returned model and context.
4. Wait through production `/v1/agent/sessions/{id}/wait`. A transport deadline may
   request production cancellation, but it cannot reinterpret a partial run as a
   model result.
5. Require a clean completed terminal record, zero process error, exit code zero,
   zero teardown diagnostics, and a complete bundle.
6. Copy and hash that exact production bundle, extract it without following
   symlinks, calculate the candidate patch from its staged workspace, and only then
   run the immutable evaluator with `--network none`.
7. Remove the evaluator container and require its structured report and reward.

The copied benchmark bundle was byte-identical to the durable archive under
`.runtime/results/<session-id>/`. The production session left no agent, model
relay, or capture container. The only project TCP listeners remained the two fixed
loopback relays on `127.0.0.1:8000` and `127.0.0.1:8090`.

The task required Go 1.25.10 while the deliberately sealed production agent image
contains Go 1.22.2 and no network/module cache. The agent correctly disclosed
that limitation rather than claiming to have run unavailable tests. The pinned
post-session evaluator contains the task's intended toolchain and proved the
candidate behavior with 11 passing tests. This is part of the production-agent
measurement; no hidden toolchain or network exception was injected into the agent.

## Infrastructure failures are not model scores

Two earlier attempts are retained as failure evidence and excluded from scoring:

- Attempt 01, session `s-820ce32b2ecd4b66a037393cd1e5416d`, failed before
  model turns because the agent exited during topology creation and the capture
  sidecar could not join its namespace. It has zero turns and is an infrastructure
  failure.
- Attempt 02, session `s-2ca24eed566c4aa99668d1cabc1f655e`, completed the
  agent process but exposed a broker race. The broker subscribed to capture logs
  with Docker `--since 0s`; Docker interprets that as relative "from now", so a
  `CAPTURE_COMPLETE` emitted before attachment could be missed forever. The service
  deliberately refused to promote unproved capture output and retained a failure
  bundle. It is an infrastructure failure, not a failed model task.

Commit `1f5a2267497b7cf8e39f18208dcf7207bd3a165e` repaired the race by replaying
Docker logs from exact Unix epoch `0`, with a unit test that freezes the argument
contract. The pinned Rust build then passed 44 service, 9 broker, 3 relay, 2 capture,
and 2 agent-exec tests. A complete clean release build reproduced every locked image.
A real five-turn production smoke session (`s-b8f334fc047a474dac79d7b9a28a607c`)
proved that a late capture-complete event now becomes a clean terminal bundle before
the benchmark was accepted.

## Tracked and local evidence

The exact harness that ran, its benchmark lock, and its structured result are
tracked under `artifacts/swe-rebench-2026-07-production-service/`. Large production
bundles, evaluator archives, materialized workspaces, logs, and older failure bundles
remain in that ignored local evidence root and in `.runtime/results/`; they are not
silently deleted or added to Git.

- Executed harness SHA-256:
  `eae987cf91a7ddd37a170d6caea88b9121e55d753811ed981aab74b6e6478ba6`
- Benchmark-lock file SHA-256:
  `12989a928eb17473b9a03f666c4a15bd8cdea201b9b4e85b590483d3c34c2887`
- Result file SHA-256:
  `0bb5d56b4b6e0e33f3b790c56c9fccb255c21b1111126a838f1be63f30cf49b1`

To rerun on this exact workstation after verifying the live release:

```bash
cd /home/user/Desktop/agent_service
./status.sh
./artifacts/swe-rebench-2026-07-production-service/pilot-production-service.sh
```

The harness intentionally refuses existing final run directories. Preserving the
accepted result and rerunning therefore requires an explicitly named new run or
an operator-authorized archival decision; it never overwrites evidence.
