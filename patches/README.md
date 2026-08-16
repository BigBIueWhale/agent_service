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
- Review-diff SHA-256: `77e137c098e29a1b3b28da112fc323777a588e0c23caab8532caeb03eb5c2b79`
- Semantic transformer: `source_patch_v1/`
- Transformer-manifest SHA-256: `90102ad5fef4531b4a007eac3715bb1e001f9bfa5e090204aa0c5d3a6d50ecc0`
- Official npm package integrity: `sha512-jN1OahOckJkrc8mnT/uqLbarYLKLmlc8gttmcHOg2WXYItu7S0sBzP+0dwBUoi/zBvywu5Sq1ilj6Eh/k0r07Q==`
- Official npm package SHA-1: `ec637654144c77505da331162a5915f50c416557`
- Pinned Node build/runtime image (linux/amd64 manifest): `node@sha256:d649c27dae7ba0137b3cef5dd75baa422c08dc3d9e3fc0c23dfb172dc3cc6436`

The semantic transformer adds the requirements that upstream 0.21.12 does not
provide as one fail-closed mode:

- exact rendered-request token counts from vLLM's real `/tokenize` endpoint;
- an exact `max_tokens = min(configured_output_ceiling, context_window - rendered_prompt_tokens)` clamp, with no heuristic margin, padding, minimum, or fallback estimate;
- exact rendered-token automatic-compaction gating before generation, including
  the actual image expansion, template, and tool schemas;
- strict native tool-call parsing and a successful-tool-call terminal invariant;
- no XML recovery, length-stop/transport continuation, model fallback, or partial
  tool execution after a length stop;
- at most one fresh resample for malformed output before any visible/non-thinking
  content, never after visible output or a potentially executable side effect;
- exact, same-model compaction whose input and candidate output are both counted by
  vLLM and whose summary must end normally without tool calls;
- one universal tool allowlist covering core, dynamic, MCP, skill, and synthetic tools;
- an explicit foreground-agents-only mode that exposes only `general-purpose` and `Explore`, returns results inline, and rejects forks, background work, teams, worktrees, custom agent types, and model overrides.
- init metadata filtered through that same policy, so it does not advertise internal or uncallable agent types.
- locked settings loading before initialization: no workspace settings or `.env`, no ambient/project/CLI/session/injected MCP, and no include-directory override;
- immutable `QWEN_HOME=/opt/agent`, with hooks, extensions, skills, output-language injection, `.qwen/rules`, permission persistence, managed/automatic/team memory, auto-dream, auto-skill, custom slash commands, and workflows disabled;
- leading slash prompts treated as literal task text and an exactly empty advertised slash-command list, while ordinary project `QWEN.md` and `AGENTS.md` guidance remains available;
- complete source-PNG validation and decoding with static 8-bit RGB/RGBA,
  16,777,216-pixel, 30:1, and 100-MiB limits; no resize/transcode fallback;
- chronological text-image-text tool results with `splitToolMedia=false`, plus
  fail-closed rejection of non-PNG and file/remote image transports.

Verification performed in the pinned Node image:

- seven transformer-framework tests cover pristine application, byte-identical
  idempotence, new-file handling, source/output drift, intermediate-state refusal,
  review-diff drift, time-of-check/time-of-use mutation, and transactional rollback;
- the complete patched TypeScript/CLI build passed;
- thirteen focused core files passed, including base-client finish reasons, exact
  auto-compaction, scheduler recovery, the complete Agent suite, strict
  PNG/container/decode tests, chronological media, and runtime isolation;
- all three focused CLI suites passed, covering configuration, initialization
  metadata, literal leading-slash prompts, and the non-interactive path;
- total focused behavioral tests: 2,326 passed across sixteen suites, zero failed;
- a sealed runtime capture observed two `/tokenize` calls followed by one streaming `/v1/chat/completions` call;
- the three requests carried identical messages, tools, and chat-template kwargs;
- a synthetic exact prompt count of 12,345 produced `max_tokens: 249799` for the 262,144-token deployed context;
- the captured chat request used only the ten approved tools and the foreground Agent schema had only `description`, `prompt`, `todo_id`, and `subagent_type` (`general-purpose` or `Explore`).

The production Docker build repeats semantic transformation, review-diff identity,
compilation, and contract tests from clean upstream source. The review diff is human
evidence, not an alternate `git apply` path. The temporary research clone and
research images are not build inputs and are removed during final cleanup.
