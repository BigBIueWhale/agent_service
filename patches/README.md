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
- Review-diff SHA-256: `c45e841bb21c35c830e4fbb66d868fce6e33c71074aaccddf5cf1ccdc5cfe412`
- Semantic transformer: `source_patch_v1/`
- Transformer-manifest SHA-256: `bc464d1451f1e5a4ce655371127c3c4adaf73b8c039b9c277c98cb0a12f1e9fd`
- Official npm package integrity: `sha512-jN1OahOckJkrc8mnT/uqLbarYLKLmlc8gttmcHOg2WXYItu7S0sBzP+0dwBUoi/zBvywu5Sq1ilj6Eh/k0r07Q==`
- Official npm package SHA-1: `ec637654144c77505da331162a5915f50c416557`
- Pinned Node build/runtime image (linux/amd64 manifest): `node@sha256:d649c27dae7ba0137b3cef5dd75baa422c08dc3d9e3fc0c23dfb172dc3cc6436`

The semantic transformer adds the requirements that upstream 0.21.12 does not
provide as one fail-closed mode:

- exact rendered-request token counts from vLLM's real `/tokenize` endpoint;
- an exact per-turn output budget: the smallest of the configured output ceiling, the context window less the rendered prompt, and the auto-compaction threshold less the rendered prompt, the window term carrying no heuristic margin, padding, or fallback estimate;
- exact rendered-token automatic-compaction gating before generation, including
  the actual image expansion, template, and tool schemas;
- strict native tool-call parsing and a successful-tool-call terminal invariant;
- no XML recovery, length-stop/transport continuation, model fallback, or partial
  tool execution after a length stop;
- at most one fresh resample for malformed output before any visible/non-thinking
  content, never after visible output or a potentially executable side effect;
- exact, same-model compaction whose input and candidate output are both counted by
  vLLM and whose summary must end normally without tool calls;
- a conversation that stays summarizable at every size it can reach. Two
  bounds hold it there and they are the same arithmetic seen from each end.
  A turn's output request stops at the auto-compaction threshold, so no
  single turn can carry the history past the point where compaction falls
  due; the buffer between that threshold and the effective window absorbs
  the turn's tool result, whose size the tokenizer cannot know in advance.
  The compaction request is then sized to the room the window actually
  leaves beside it, counted by `/tokenize` on the request that will be sent,
  so a summary request can be issued whatever the history costs. The ceiling
  on that budget is 49,152, and it is one number because it is two things at
  once: the request's `max_tokens` and the reserve the thresholds hold free,
  which is what keeps the budget at its full value in every state the
  thresholds admit. It costs working window — the automatic trigger sits at
  199,992 rather than upstream's 229,144. The request is otherwise ordinary:
  it declares no per-request phase budgets, so the model thinks to a natural
  stop and writes the snapshot under the pinned `extra_body` ceilings exactly
  as on any turn, and the reasoning-end marker is never forced. The snapshot
  schema gives every fact exactly one home, so nothing in it has two places
  to grow;
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
- one closed parameter contract for every advertised tool. A tool schema is
  the model's only map of what is callable and it is re-rendered into the
  tools block of every request, so it is the surface that teaches; a single
  error message is one line of history competing with it. JSON Schema permits
  an undeclared property by default, so an invented, misremembered, or stale
  parameter name passed validation and was then dropped -- the tool did
  something other than what was asked and said nothing. `read_file` refused
  exactly one such name, `pages`, as a special case written after the fact,
  and every other tool dropped every other name. All ten native schemas now
  declare `additionalProperties: false`, and one shared rule refuses an
  undeclared name before schema validation, naming both the offending key and
  the complete accepted set: `read_file has no parameter 'pages'. It accepts
  exactly: 'file_path', 'offset', 'limit'. Re-send the call using only those
  parameters.` `agent` and `todo_write` replace `validateToolParams`
  wholesale and so never run Ajv at all; both now apply the same rule, in
  `agent`'s case after its specific refusals so that "this parameter exists
  upstream and is disabled here" keeps precedence over "no such parameter".
  The `pages` special case is gone: it was one instance of the general rule.
- the same defect one layer down. `offset` and `limit` were read only by the
  text branch and were silently ignored on PDFs, images, audio, video, SVGs,
  notebooks, and binaries -- a plausible wrong answer rather than an error,
  and worse than the `pages` case because nothing downstream could observe
  it. Only `.ipynb` was refused, and only by guessing from the extension at
  the tool layer before the file type was known. One rule now covers every
  non-text type and fires at the first point in the read that knows what the
  file is, so the refusal can name it: `'offset' and 'limit' select a range
  of lines, and a line range applies to text files only; docs/spec.pdf is a
  pdf file, which is read whole. Re-send the call with offset: 0 and no
  limit.` Because `offset` is required (below), 0 is how a non-text read is
  spelled: it selects no range and passes, so only a range-selecting nonzero
  offset or a `limit` is refused. The extension guess is deleted, and the
  schema was corrected to match what the code actually does: `offset` no
  longer claims to require `limit` (it never did -- a missing `limit` takes
  the default window), and a line range is stated to be text-only and
  refused elsewhere.
- a required `offset`, so an omitted range cannot execute as line 1.
  Benchmark forensics over five event logs (1,277 tool calls) found 184
  `read_file` calls that executed without `offset` while the model's own
  reasoning named the resume line it intended: the sampler occasionally
  discharges a pending parameter into the prose channel just before the
  `<tool_call>` trigger fires, and a call omitting an *optional* parameter
  is a perfectly grammatical serialization of a wrong request -- the armed
  decode grammar has nothing to refuse, the read silently restarts at line
  1, and the loop guard eventually kills the session on the byte-identical
  re-reads. Requiring the parameter turns that omission from a legal wrong
  call into an impossible one: the grammar refuses it in-span (verified
  against the pinned runtime image -- rejection lands on the first token
  that would skip the parameter, and a duplicated parameter name is
  likewise unserializable), Ajv refuses it with `params must have required
  property 'offset'` for any call that arrives another way, and 0 is the
  explicit spelling of "from the beginning" for every file kind, so
  non-text files stay readable. It is also with the training grain: 660 of
  851 observed reads already carried an explicit offset. `limit` stays
  optional deliberately -- its omission is fail-visible, because the
  truncation notice carries the exact continuation call, while an omitted
  offset was fail-silent; and every historical exemplar re-rendered into
  later prompts now shows the offset, so context teaches the required
  shape instead of the wrong one. Three `[param-contract]` cases execute
  the required schema, the Ajv refusal, the offset-0 whole read, and the
  negative-value refusal in the build.
- quarantine for tool-call syntax discharged into prose. The visible half
  of the same failure: in 4 of 3,278 events an assistant text block
  carried a fragment of call serialization -- foreign and Qwen closers
  first (`</invoke>`, `</function_calls>`, `</parameter>`), sometimes with
  the very `<parameter=offset>` value the adjacent call then omitted.
  History re-renders assistant text verbatim into every later prompt, so
  the fragment became a clean-looking exemplar of exactly the malformed
  shape it came from, rendered immediately before that turn's re-rendered
  tool calls -- one bad serialization teaching the next. A structural
  detector now strips the trailing fragment from what history and the
  durable JSONL record keep, on tool-call turns only: a maximal trailing
  run of closer/opener/value lines that opens and closes with a closing
  tag. Execution is never touched -- the calls in such turns are complete
  and the strict resample window has already closed by the time prose is
  visible, so a refusal here would spend a turn on cosmetic noise, which
  is the loop-guard mistake one layer down. The live event stream carries
  the raw text before the strip, so captured sessions keep the forensic
  ground truth. Against the full corpus the detector strips all four
  fragments and nothing else; inline prose mentions, fenced quotes, a lone
  quoted closer, and opener-first examples are all structurally excluded,
  and a false positive would only shorten one message's re-rendered copy.
  Eleven `[residue-quarantine]` cases -- the four real fragments verbatim,
  four false-positive controls, plain and empty text, the history seam
  keeping the prose beside the executed call, and the never-rewrite rule
  for turns without calls -- execute this in the build.
- a truncated read that carries the call which continues it. The `read_file`
  description promised that a truncated response "will explain how to
  continue with 'offset' and 'limit'"; the notice said only `Showing lines
  1-2000 of 40000 total lines.` Naming the parameters is not explaining how
  to continue -- the reader still has to derive a 0-based resume line from a
  1-based display range, and a line the character budget cut short makes that
  arithmetic wrong in a way that silently skips content. The resume point is
  computed where that is knowable, and the notice carries the actual next
  call: `To continue from where this read stopped, call read_file with
  file_path: "/workspace/src/main.rs", offset: 2000, limit: 2000.` A read
  that reached the end of the file carries nothing.
- PDF page selection named for the file that was asked for. One exported
  remedy already sent every large-, truncated-, or overflowing-PDF message to
  the same place, but it named `<absolute path>`, a value the message already
  had. It is now a function of the path, so the command can be run as
  written; `FIRST` and `LAST` stay placeholders because they are the caller's
  decision, and a template form derived from the same function serves the
  tool description, which is fed to `/tokenize` with the rest of the tools
  block and must stay byte-stable. The 100 MB full-text gate put that remedy
  in `llmContent`, which the scheduler discards on a result carrying `error`:
  the model saw only the size complaint. It is in both halves now. Two
  continuation messages on the text-only vision-bridge path told the model to
  call `read_file` on the original PDF "with pages" or "with a later page
  range" -- a call `read_file` refuses, so a guaranteed dead end for any
  caller running a text-only model. They now give the `pdftotext` command in
  full.
- no tool description that advertises a dispatch this build does not have.
  `parallel_tool_calls` is false and both sealed prompts say tool calls are
  sequential, but `glob` told the model "You have the capability to call
  multiple tools in a single response. It is always better to speculatively
  perform multiple searches as a batch", and `run_shell_command` told it to
  "send a single message with two run_shell_command tool calls in parallel".
  Those descriptions are re-rendered into every request; the sealed prompt
  says the opposite once. Both now state the runtime fact. Nine
  `[param-contract]` cases execute this boundary in the build, over the
  closed schemas of all ten tools, the ordering of the shared refusal against
  schema validation, the non-text range refusal, the truncation
  continuation, and the two descriptions.
- tool-parameter validation that reports rather than repairs. Every tool call
  passed through a validator that ran four coercion passes over the caller's
  arguments -- `"true"`/`"false"` to boolean, a number to a string, a
  JSON-looking string to an array or object, `"3"` to `3` -- and re-validated
  the mutated object, so a call that violated the advertised schema still
  executed. That is the parameter-ignoring defect one layer earlier, and it
  never surfaced as a failure precisely because it made wrong calls succeed.
  The same function also disabled itself: a schema Ajv could not compile
  skipped validation entirely and warned to a debug logger this deployment
  does not enable, so the guard failed open in silence. Both are gone, with
  the ~1,100 lines of coercion machinery they needed. A violation is reported
  as `params/<path> must <constraint>`, and an uncompilable schema refuses the
  call. The modification flow, which re-enters the tool with the runtime's own
  bookkeeping alongside the edited parameters, routes those two names through
  a channel declared on the tool and removed before the schema check, so they
  are never advertised and every other undeclared name is still refused
  (`notebook_edit` needs no channel: it already keeps its bookkeeping in a
  side map, which is the shape that retires the channel entirely).
- one model-facing field for a failed tool call. A `ToolResult` carries
  `llmContent`, written for the model, and `error.message`, an operational
  summary for telemetry and the scrollback; on the failure path the scheduler
  forwarded only `error.message` and read `llmContent` for images alone. Any
  remedy written into the half named for the model was discarded before the
  model saw it -- the `agent` tool's "Use TeamCreate to start a team first",
  the teammate name in its spawn failures, the `pdftotext` command on an
  oversized PDF. Tools worked around it one at a time by copying `llmContent`
  into `error.message`, and two in-source comments existed only to warn the
  next author. Neither half can simply win: `llmContent` usually carries the
  remedy but often omits the path, while `error.message` is sometimes the only
  operational fact there is, such as a shell timeout summary against unrelated
  output. Both are now merged, once, into the value that reaches the model and
  only that value: `error.message` keeps its own readers -- the scrollback,
  the PostToolUseFailure hook and the sanitized telemetry span -- because
  folding the model's copy into it leaked tool output into all three. A tool
  added later cannot reintroduce the loss whichever half it writes, and the
  per-tool workarounds are retired.
- PDF reading with no image fallback at all. Three paths rendered pages to
  JPEGs: for a vision-capable model on text overflow, again on extraction
  failure, and again to feed a text-only model's vision bridge. A rendered
  page is a lossy image substituted for the document that was asked for and an
  unannounced modality change, and it cannot satisfy the static original-PNG
  contract this deployment pins. Pinning `willRenderPdfImages = false`
  neutralised the first path but left the code: a typed boolean constant the
  compiler cannot flag, forty lines of unreachable fallback, a bridge path
  still reachable for any caller running a text-only model, and continuation
  guidance inside it that told the model to reopen the PDF with a `pages`
  argument `read_file` refuses -- a guaranteed dead end. The renderer, the
  bridge's PDF half, the page-range option on the shared read utility, and the
  parser that fed them are deleted rather than disabled, together with their
  tests; the image bridge itself is untouched. `pdf.ts` states the boundary
  where the renderer used to be, so the next reader is told why there is no
  fallback instead of inferring it from absence.
- a subagent turn count that survives a throw. The reasoning loop's counter
  was a stack local and the loop has no `catch`. Every terminate reason
  reported the true count because each of them returns; a throw -- from the
  model stream, a tool call, or a synchronous event listener -- unwound past
  the single line that copied the count onto the headless wrapper, and the
  terminal event emitted the zero that wrapper had initialised. A subagent
  that died mid-run was recorded as having taken no turns at all, which is the
  case where the count matters most. The counter now belongs to the core,
  which outlives the throw, and the wrapper's mirror -- a second writer that
  could disagree -- is removed rather than kept in step. Reading the count out
  of the exception path was rejected: it is a special case each new throw site
  would have to remember.
- a build that runs the tests it claims to. The image executed a fixed list of
  24 files plus two `-t` filters, so `fileUtils.test.ts`, `read-file.test.ts`
  and `subagent-result.test.ts` never ran in full and `agent-core.test.ts`,
  `web-fetch.test.ts` and `readManyFiles.test.ts` never ran at all -- which is
  why roughly thirty assertions pinning the deleted PDF renderer sat broken
  and invisible. The list is now derived at build time from the hashed patch
  itself: every test file the patch modifies must exist and is executed in
  full, and so is the test file adjacent to every source file the patch
  modifies, when one exists. A patched test file that is missing fails the
  build. There are no `-t` filters left, so a test the build does not run is
  no longer a reachable state; coverage rises from 28 files to 54.
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
- a compaction attempt that diagnoses itself from the standard bundle. The
  compaction diagnostics go to the debug logger, and this deployment enables
  no debug logging, so none of them reach `qwen.stderr`; a discarded summary
  is never persisted, and a side query's usage never reaches the billed event
  stream. Turning on debug logging would be a second mode and would pollute
  the captured stream, so the compaction event carries the accounting
  instead: the budget the attempt ran under, the output tokens produced, how
  many of them were reasoning, how much summary survived, and the provider's
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
- the full build-derived suite passed: all 58 derived test files (5,318
  tests: 5,297 passed and 1 environment-skip in the bare pinned image, the
  20 git-dependent cases with git present as the production image provides
  it) -- zero failures;
- the same 20 environment-gated failures reproduce byte-identically on the
  unmodified reviewed tree in the bare image, so they are properties of the
  runner, not of any change;
- a sealed runtime capture observed two `/tokenize` calls followed by one streaming `/v1/chat/completions` call;
- the three requests carried identical messages, tools, and chat-template kwargs;
- a synthetic exact prompt count of 12,345 produced `max_tokens: 249799` for the 262,144-token deployed context;
- the captured chat request used only the ten approved tools and the foreground Agent schema had only `description`, `prompt`, `todo_id`, and `subagent_type` (`general-purpose` or `Explore`).

The production Docker build repeats semantic transformation, review-diff identity,
compilation, and contract tests from clean upstream source. The review diff is human
evidence, not an alternate `git apply` path. The temporary research clone and
research images are not build inputs and are removed during final cleanup.
