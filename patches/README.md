# Pinned upstream patches

This directory is part of the reproducible build input. Patches are applied only to the exact upstream revision recorded here; the build fails if a hunk does not apply.

## Qwen Code 0.21.12

- Repository: `https://github.com/QwenLM/qwen-code`
- Tag: `v0.21.12`
- Commit: `b965d5f8c24f48e65fb0b17c7d45f34ca4ce8f38`
- Commit archive: `https://codeload.github.com/QwenLM/qwen-code/tar.gz/b965d5f8c24f48e65fb0b17c7d45f34ca4ce8f38`
- Commit archive SHA-256: `61beddff8bde1dd2654c8714f927b46ab7cf9822b8561d11e3a2b8e085b5e745`
- Patch: `qwen-code-0.21.12-agent-service.patch`
- Patch SHA-256: `0c621cd8000bb61af633614ee775950b454b02b05a7bb54c9b62824587138181`
- Official npm package integrity: `sha512-jN1OahOckJkrc8mnT/uqLbarYLKLmlc8gttmcHOg2WXYItu7S0sBzP+0dwBUoi/zBvywu5Sq1ilj6Eh/k0r07Q==`
- Official npm package SHA-1: `ec637654144c77505da331162a5915f50c416557`
- Pinned Node build/runtime image (linux/amd64 manifest): `node@sha256:d649c27dae7ba0137b3cef5dd75baa422c08dc3d9e3fc0c23dfb172dc3cc6436`

The patch adds the requirements that upstream 0.21.12 does not provide as one fail-closed mode:

- exact rendered-request token counts from vLLM's real `/tokenize` endpoint;
- an exact `max_tokens = min(configured_output_ceiling, context_window - rendered_prompt_tokens)` clamp, with no heuristic margin, padding, minimum, or fallback estimate;
- strict native tool-call parsing and a successful-tool-call terminal invariant;
- no XML recovery, automatic continuation, semantic-changing retry, or partial tool execution after a length stop;
- one universal tool allowlist covering core, dynamic, MCP, skill, and synthetic tools;
- an explicit foreground-agents-only mode that exposes only `general-purpose` and `Explore`, returns results inline, and rejects forks, background work, teams, worktrees, custom agent types, and model overrides.
- init metadata filtered through that same policy, so it does not advertise internal or uncallable agent types.

Verification performed in the pinned Node image:

- the complete patched TypeScript/CLI build passed;
- six focused core files passed: 1,235 tests, including the complete 259-test Agent suite with pinned Git 2.39.5;
- the CLI configuration suite passed: 336 tests;
- the CLI non-interactive helper suite passed: 53 tests;
- total focused tests: 1,624 passed, zero failed;
- a sealed runtime capture observed two `/tokenize` calls followed by one streaming `/v1/chat/completions` call;
- the three requests carried identical messages, tools, and chat-template kwargs;
- a synthetic exact prompt count of 12,345 produced `max_tokens: 249799` for the 262,144-token deployed context;
- the captured chat request used only the ten approved tools and the foreground Agent schema had only `description`, `prompt`, `todo_id`, and `subagent_type` (`general-purpose` or `Explore`).

The production Docker build repeats patch application, compilation, and contract tests from clean upstream source. The temporary research clone and research images are not build inputs and are removed during final cleanup.
