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
- Review-diff SHA-256: `5d6cb03b60bdc7d34086b4be076c09dc0b291aadd90771b7ec4b66471b2de8d6`
- Semantic transformer: `source_patch_v1/`
- Transformer-manifest SHA-256: `59acd4e03989bdd3dbbac308d5aa6d8690b26d2d15939566511037ba522a2e75`
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
- one sealed engineering discipline, delivered byte-identically to the main
  session and to every foreground subagent, because honest verification, scope,
  and tool-result integrity are properties of this runtime rather than of one
  role; only genuinely role-specific framing is layered on top of it;
- a main-session frame that states why delegation is the intended way to work
  here: context length is the scarcest resource, a subagent's reading never
  enters the parent context, and the parent's KV cache is retained in host RAM
  for the duration of the subagent, so nothing already said is re-ingested when
  the subagent returns. A long single thread therefore keeps running instead of
  being compacted. Six assertions in the build pin the frame: three for its
  presence and three for its reasoning, so it cannot be silently reduced to a
  bare instruction;
- an API or transport error ends the run in every output format. Upstream raises
  it only under `--output-format text`; under `json` and `stream-json` the same
  error was appended to the assistant message as ordinary text and the session
  then reported `subtype: success`, `is_error: false`, exit 0 — a failed turn
  presented to the caller as a finished answer. Production forensics found this
  on a session whose last turn died on the stream cap: the recorded final
  response was a mid-thought preamble followed by the error string, and the only
  reason it was ever noticed is that the turn carried no usage, which broke the
  service's independent `num_turns` cross-check;
- locked settings loading before initialization and during later auth validation:
  no workspace settings or `.env`, no ambient/project/CLI/session/injected MCP,
  and no include-directory override;
- immutable `QWEN_HOME=/opt/agent`, with hooks, extensions, skills, output-language injection, `.qwen/rules`, permission persistence, managed/automatic/team memory, auto-dream, auto-skill, custom slash commands, and workflows disabled;
- leading slash prompts treated as literal task text and an exactly empty advertised slash-command list, while ordinary project `QWEN.md` and `AGENTS.md` guidance remains available;
- complete source-PNG validation and decoding with static 8-bit RGB/RGBA,
  16,777,216-pixel, 30:1, and 100-MiB limits; no resize/transcode fallback;
- chronological text-image-text tool results with `splitToolMedia=false`, plus
  fail-closed rejection of non-PNG and file/remote image transports;
- rejection of the PDF-only `pages` parameter on non-PDF files at both the
  validation and consumption layers, with an error that names `offset`/`limit`
  as the text-file remedy — production forensics showed upstream's
  syntax-first errors and silent ignore steering agents into identical-call
  loops (three `[pages-contract]` tests execute this boundary in the build);
- evidentiary stream-json tool results: the emitted record prefers the
  model-facing responseParts over the short human-facing display string, so
  captured event streams carry what the model actually received (two
  `[stream-evidence]` tests plus the full output-adapter suite execute this
  in the build);
- a session-start time anchor: the deployment contract ends with one
  timestamp computed once at process start, keeping the system prompt
  byte-stable for the session (prefix-cache safe) while giving the model
  absolute time; live time stays observable via `date`.

Verification performed in the pinned Node image:

- seven transformer-framework tests cover pristine application, byte-identical
  idempotence, new-file handling, source/output drift, intermediate-state refusal,
  review-diff drift, time-of-check/time-of-use mutation, and transactional rollback;
- the complete patched TypeScript/CLI build passed;
- twenty-one focused core files passed, including base-client finish reasons,
  exact auto-compaction, scheduler recovery, the complete Agent suite, strict
  PNG/container/decode tests, chronological media, runtime isolation, and the
  deployment-prompt contract;
- all five focused CLI suites passed, covering locked auth revalidation,
  configuration, initialization metadata, literal leading-slash prompts, the
  stream-json output adapter, and the non-interactive path;
- total focused behavioral tests: 2,543 passed across twenty-six suites, zero
  failed, with 259 environment-gated cases skipped;
- a sealed runtime capture observed two `/tokenize` calls followed by one streaming `/v1/chat/completions` call;
- the three requests carried identical messages, tools, and chat-template kwargs;
- a synthetic exact prompt count of 12,345 produced `max_tokens: 249799` for the 262,144-token deployed context;
- the captured chat request used only the ten approved tools and the foreground Agent schema had only `description`, `prompt`, `todo_id`, and `subagent_type` (`general-purpose` or `Explore`).

The production Docker build repeats semantic transformation, review-diff identity,
compilation, and contract tests from clean upstream source. The review diff is human
evidence, not an alternate `git apply` path. The temporary research clone and
research images are not build inputs and are removed during final cleanup.
