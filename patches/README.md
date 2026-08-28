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
- Review-diff SHA-256: `85b6bbf74ea0070af4a00fc23534fbb6786acd95c773f0030379ca3f4fe7892a`
- Semantic transformer: `source_patch_v1/`
- Transformer-manifest SHA-256: `b70de96802a4a9a8bef5cff320c899b42fc78af7c968b3896f02dadc221eae48`
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
- a per-request maintenance phase budget that cannot be silently lost.
  Compaction runs inside a fixed 20,000-token output ceiling and splits it
  deliberately — a bounded thinking phase, and a reserved final-response
  phase so the state snapshot has room to be written. The override was
  applied to the per-request sampling parameters, but this deployment pins
  both phase budgets in `extra_body`, which the provider hook merges over
  the request, so the split never reached the wire: the summarizer ran at
  the pinned 262,144-token thinking budget inside a 20,000-token cap.
  Thought parts are filtered out of the response, so two consecutive
  attempts each spent about 19,900 output tokens and produced no summary at
  all; the session then died refusing to send 240,122 tokens against a
  239,144-token limit. The existing ceiling guard could not have caught it:
  it compared against the sampling-parameter layer, which never declares
  those keys, so it read `undefined` and always passed. The budget is now
  written onto the fully merged wire request, after every configuration
  layer, and validated against the exact value it replaces — so the same
  guard that could never fire now reads the pinned 262,144 and refuses
  anything above it (two `[phase-budget]` tests execute both halves in the
  build);
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
- exactly one range mechanism on `read_file`. Upstream advertised the
  PDF-only `pages` parameter on every file type. A tool schema is the
  model's only map of what is callable, and a parameter whose applicability
  depends on the value of another parameter cannot be read off that map, so
  the model used it as a line range on source files. Making the refusal
  accurate at both the validation and consumption layers, naming
  `offset`/`limit` as the remedy, did not change the behaviour: it was
  measured again afterwards, 106 refusals across 7 of 18 subagent scopes in
  a single run, one subagent issuing 55 of them, and three subagents
  repeating a byte-identical failing call until loop detection killed them.
  The affordance itself was the defect. `read_file` now reads whole files
  and advertises exactly `file_path`, `offset`, and `limit`; a `pages`
  argument supplied anyway is refused whatever the file type — an
  undeclared key survives JSON-schema validation, and a dropped page range
  would leave the model believing it had asked for one — and the shared
  consumption layer keeps its own fail-closed guard for callers that bypass
  the tool. PDF page selection happens where it is unambiguous and where
  this deployment's sealed contract already puts it, a page-ranged
  `pdftotext` run in the shell, and one exported remedy string is what
  every large-, truncated-, or overflowing-PDF message names, so no
  guidance can point at a parameter that no longer exists (six
  `[pages-contract]` cases execute this boundary in the build);
- evidentiary stream-json tool results: the emitted record prefers the
  model-facing responseParts over the short human-facing display string, so
  captured event streams carry what the model actually received (two
  `[stream-evidence]` tests plus the full output-adapter suite execute this
  in the build);
- a session-start time anchor: the deployment contract ends with one
  timestamp computed once at process start, keeping the system prompt
  byte-stable for the session (prefix-cache safe) while giving the model
  absolute time; live time stays observable via `date`;
- natively emitted conversation compaction. Upstream surfaces compaction only
  as an interactive UI signal and only when it succeeds, so the captured
  `output/events.jsonl` carried no trace of it: the sole way to tell that a
  session compacted was to infer it from a drop in `usage.input_tokens`
  between consecutive billed assistant events — a reconstruction from a side
  effect that cannot distinguish an attempt that was refused or failed from
  one that never ran, and that shows nothing at all for a subagent, which
  compacts its own chat outside the parent's history. Every completed
  attempt is now emitted as a `system` event with `subtype: "compaction"`,
  carrying the rendered prompt token count before and after, the
  `CompressionStatus` spelling, and a `succeeded` flag, under the same
  `parent_tool_use_id` convention as every other message (`null` for the main
  session, the `agent` tool-call id for a subagent). A NOOP — no attempt was
  made — stays silent, and the narrower `ChatCompressed` event keeps its
  exact meaning of "the history was replaced", so startup-context restoration
  and the interactive notice still fire only on real successes (seven
  `[compaction-event]` and compaction-ordering tests execute this in the
  build);
- a subagent's own terminal record scoped to the subagent that produced it.
  Every emitted event names its scope — `parent_tool_use_id` is `null` for
  the main session and the owning `agent` tool-call id for a subagent — but
  upstream's subagent error result carried no scope at all, even though
  `emitSubagentErrorResult` already holds that id and uses it one line
  earlier to finalize the subagent's pending assistant message. A session run
  with `--max-session-turns=3`, whose subagent was told to loop forever,
  therefore recorded the subagent's `MAX_TURNS` result as the *session's*
  terminal result; the parent then recovered, answered, and finished, so four
  more events followed a record the service had to read as the end of the
  run. The service refused the entire capture as `agent_output_missing`, and
  it was right to: the alternative is choosing a convenient-looking last
  result, which is exactly what that check exists to prevent. The cost was
  that every session in which a subagent exhausted its budget was discarded
  whole, even though the parent had completed successfully. The record now
  carries its owning tool-call id, both branches of the session's own result
  stamp `parent_tool_use_id: null` instead of omitting the field, and the
  service treats only a null-scoped result as terminal — the same rule its
  main-turn count already used. Suppressing the event instead was rejected:
  it would restore precisely the subagent invisibility the compaction event
  above was added to remove, leaving a refused attempt indistinguishable from
  one that never ran;
- how far that subagent got. The same capture reported `num_turns: 0` for a
  subagent that had completed three turns, and the text handed to the parent
  was `[subagent exhausted its turn budget and produced no report; its
  assignment is unfinished]` with no count in it. Nothing had lost the count:
  `AgentHeadless` held 3 across the whole `MAX_TURNS` exit, which returns
  normally rather than throwing. `num_turns` was an upstream literal `0` at
  the emit site, and the model-visible text is built by a *second*
  `toModelVisibleSubagentResult` call in the agent tool's foreground body —
  the call inside `runSubagentWithHooks` that already received the count only
  feeds the display, never the parent. Both report it now. The count reaches
  the emitted record over the same `AgentResultDisplay` channel the
  subagent's tool calls and compactions already ride, published in the update
  that carries the terminal status, so `num_turns` on a subagent result means
  what it means on the session's own: the model round-trips that scope
  completed, which is the number of assistant events the stream carries under
  that tool-call id (seven `[subagent-scope]` tests execute both halves in
  the build);
- how far it got on every terminate reason, and which rule stopped it. The
  same channel still reported `num_turns: 0` for every subagent error
  result — MAX_TURNS, TIMEOUT, ERROR, CANCELLED, and LOOP_DETECTED alike —
  while the real counts in one capture were 11, 7, and 9. Nothing had lost
  the count a second time: two writers flip the subagent display from
  running to failed, and the one that wins is the terminal event fired from
  the reasoning loop's own exit, which announced the status and the reason
  but carried no count. The emitted record is built on the first
  running-to-failed transition, so the later, complete update from the tool
  body was never read and the emitter stamped its `?? 0` fallback. That
  event now carries the terminal facts in full, so whichever writer is read
  first is already right. It also carries which rule fired: `LOOP_DETECTED`
  covers nine rules across two tiers, and the subagent path never consulted
  the detector's last loop type, so a five-identical-call halt and an
  exhausted per-turn tool-call budget reached both the operator and the
  parent model as the same bare word — which is why diagnosing one of them
  took a five-hour telemetry reconstruction. The rule labels moved out of
  the CLI to sit beside the detector, so the headless session's message and
  a subagent's record and report name the same cause in the same words (one
  `[subagent-scope]` and four `[loop-attribution]` tests execute this in the
  build);
- a failed compaction that diagnoses itself from the standard bundle. The
  compaction diagnostics — `[chat-compression] summary terminated with
  MAX_TOKENS` and its siblings — go to the debug logger, and this deployment
  enables no debug logging, so none of them reached `qwen.stderr`; the
  truncated summary was discarded unpersisted, and a failed side query's
  usage never reaches the billed event stream. The investigation into the
  session above could therefore establish that the 20,000-token maintenance
  budget had been exhausted but not how it split between hidden thinking and
  the final response, which is the one fact that identifies the failure.
  Turning on debug logging would be a second mode and would pollute the
  captured stream, so the compaction event carries the accounting instead:
  the ceiling, both phase budgets, the output tokens produced, how many of
  them were reasoning, how much summary survived, and the provider's
  terminal reason. It is attached to every outcome reachable after the
  generation and left empty when the attempt was refused before it, so a
  budget consumed entirely by reasoning stays distinguishable from a request
  that never ran (three more `[compaction-event]` tests execute this in the
  build).

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
- total focused behavioral tests: 2,550 passed across twenty-six suites, zero
  failed, with 259 environment-gated cases skipped;
- a sealed runtime capture observed two `/tokenize` calls followed by one streaming `/v1/chat/completions` call;
- the three requests carried identical messages, tools, and chat-template kwargs;
- a synthetic exact prompt count of 12,345 produced `max_tokens: 249799` for the 262,144-token deployed context;
- the captured chat request used only the ten approved tools and the foreground Agent schema had only `description`, `prompt`, `todo_id`, and `subagent_type` (`general-purpose` or `Explore`).

The production Docker build repeats semantic transformation, review-diff identity,
compilation, and contract tests from clean upstream source. The review diff is human
evidence, not an alternate `git apply` path. The temporary research clone and
research images are not build inputs and are removed during final cleanup.
