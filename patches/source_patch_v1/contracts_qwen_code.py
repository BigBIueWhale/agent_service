"""Semantic contracts for the pinned Qwen Code source transformation.

The generated landmark module proves exact byte identity with the reviewed diff.
This module independently records *why* each part of that diff exists and checks
the source relationships that make the locked agent-service behavior true.  A
future upstream update is therefore not accepted merely because a diff can be
made to apply: each concern must either still satisfy its precondition or meet
its documented removal condition and be deliberately redesigned.

Qwen Code is TypeScript.  The immutable build subsequently runs the upstream
TypeScript compiler and focused Vitest suites; these pre-write validators stay
dependency-free so they can run before ``npm ci`` and refuse without mutating
the disposable source tree.
"""

from __future__ import annotations

from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass

import re

from .framework import PatchRefusedError, forbid_text, require_text

State = Mapping[str, str]
Validator = Callable[[State], None]


@dataclass(frozen=True)
class SemanticConcern:
    """One independently reviewable defect and its retirement condition."""

    name: str
    rationale: str
    removal_condition: str
    validate_before: Validator
    validate_after: Validator


@dataclass(frozen=True)
class SemanticContract:
    rationale: str
    removal_condition: str
    validate_before: Validator
    validate_after: Validator


def _require(condition: object, message: str) -> None:
    if not condition:
        raise PatchRefusedError(message)


def _source(state: State, path: str, *, label: str) -> str:
    _require(path in state, f"{label}: missing {path}")
    return state[path]


def _require_all(
    state: State,
    path: str,
    needles: Sequence[str],
    *,
    label: str,
) -> str:
    source = _source(state, path, label=label)
    for needle in needles:
        _require(
            needle in source,
            f"{label}: {path} lacks required construct {needle!r}",
        )
    return source


def _require_ordered(
    source: str,
    needles: Sequence[str],
    *,
    label: str,
    location: str,
) -> None:
    cursor = 0
    for needle in needles:
        found = source.find(needle, cursor)
        _require(
            found >= 0,
            f"{label}: {location} lacks ordered construct {needle!r}",
        )
        cursor = found + len(needle)


def _validate_locked_boundary_before(state: State) -> None:
    label = "locked configuration boundary precondition"
    cli = "packages/cli/src/config/config.ts"
    auth = "packages/cli/src/config/auth.ts"
    auth_test = "packages/cli/src/config/auth.test.ts"
    core = "packages/core/src/config/config.ts"
    metadata = "scripts/generate-git-commit-info.js"
    _require_all(
        state,
        cli,
        (
            ".option('core-tools'",
            "assembleMcpServers(settings.mcpServers",
            "onPersistPermissionRule: async",
        ),
        label=label,
    )
    forbid_text(state, cli, ".option('strict-tools'", label=label)
    forbid_text(state, cli, "lockedAgentServiceMode", label=label)
    _require_all(
        state,
        auth,
        (
            "const settings = loadSettings(process.cwd(), false);",
            "loadEnvironment(settings.merged);",
        ),
        label=label,
    )
    forbid_text(state, auth, "lockedAgentServiceMode", label=label)
    _require_all(
        state,
        auth_test,
        ("vi.mock('./settings.js'", "describe('validateAuthMethod'"),
        label=label,
    )
    forbid_text(
        state,
        auth_test,
        "keeps locked auth validation inside the sealed settings and environment boundary",
        label=label,
    )
    forbid_text(state, core, "getForegroundAgentsOnly()", label=label)
    _require_all(
        state,
        metadata,
        ("execSync('git rev-parse --short HEAD'", "let gitCommitInfo = 'N/A'"),
        label=label,
    )


def _validate_locked_boundary_after(state: State) -> None:
    label = "locked configuration boundary result"
    cli = "packages/cli/src/config/config.ts"
    auth = "packages/cli/src/config/auth.ts"
    auth_test = "packages/cli/src/config/auth.test.ts"
    entry = "packages/cli/src/gemini.tsx"
    core = "packages/core/src/config/config.ts"
    helpers = "packages/cli/src/utils/nonInteractiveHelpers.ts"
    metadata = "scripts/generate-git-commit-info.js"
    _require_all(
        state,
        cli,
        (
            ".option('strict-tools'",
            ".option('foreground-agents-only'",
            "const lockedAgentServiceMode = argv.foregroundAgentsOnly === true;",
            "contextRuleExcludes: lockedAgentServiceMode ? ['**'] : []",
            "cliMcpServers = lockedAgentServiceMode",
            "onPersistPermissionRule: lockedAgentServiceMode",
            "enableManagedAutoMemory:",
            "enableManagedAutoDream:",
            "enableTeamMemory:",
            "enableTeamMemorySync:",
            "enableAutoSkill:",
            "autoSkillConfirm:",
        ),
        label=label,
    )
    _require_all(
        state,
        entry,
        (
            "argv.foregroundAgentsOnly",
            "skipLoadEnvironment: true",
            "skipWorkspaceSettings: true",
            "workspaceTrusted: false",
            "!isBareMode(argv.bare) && !argv.foregroundAgentsOnly",
        ),
        label=label,
    )
    auth_source = _require_all(
        state,
        auth,
        (
            "process.env['QWEN38_AGENT_SERVICE_LOCKED'] === '1'",
            "skipLoadEnvironment: true",
            "skipWorkspaceSettings: true",
            "workspaceTrusted: false",
            "if (!lockedAgentServiceMode) {",
            "loadEnvironment(settings.merged);",
        ),
        label=label,
    )
    _require_ordered(
        auth_source,
        (
            "const lockedAgentServiceMode =",
            "const settings = loadSettings(",
            "skipLoadEnvironment: true",
            "skipWorkspaceSettings: true",
            "workspaceTrusted: false",
            "if (!lockedAgentServiceMode) {",
            "loadEnvironment(settings.merged);",
        ),
        label=label,
        location=auth,
    )
    _require_all(
        state,
        auth_test,
        (
            "keeps locked auth validation inside the sealed settings and environment boundary",
            "process.env['QWEN38_AGENT_SERVICE_LOCKED'] = '1';",
            "skipLoadEnvironment: true",
            "skipWorkspaceSettings: true",
            "workspaceTrusted: false",
            "expect(settings.loadEnvironment).not.toHaveBeenCalled();",
            "retains ordinary auth environment loading outside locked mode",
            "expect(settings.loadEnvironment).toHaveBeenCalledWith({});",
        ),
        label=label,
    )
    core_source = _require_all(
        state,
        core,
        (
            "private readonly strictTools: string[] | undefined;",
            "private readonly foregroundAgentsOnly: boolean;",
            "getStrictTools(): string[] | undefined",
            "getForegroundAgentsOnly(): boolean",
            "!this.getForegroundAgentsOnly()",
            "!options?.skipSkillManager && !this.getForegroundAgentsOnly()",
        ),
        label=label,
    )
    _require(
        core_source.count("getForegroundAgentsOnly()") >= 13,
        f"{label}: the mode no longer dominates every initialization/getter gate",
    )
    _require_all(
        state,
        helpers,
        (
            "export function shouldInterpretSlashCommands(config: Config): boolean",
            "return !config.getForegroundAgentsOnly();",
            "const slashCommands = config.getForegroundAgentsOnly()",
            "name === 'general-purpose' || name === 'Explore'",
        ),
        label=label,
    )
    _require_all(
        state,
        metadata,
        (
            "PINNED_QWEN_CODE_COMMIT = 'b965d5f8c24f48e65fb0b17c7d45f34ca4ce8f38'",
            "PINNED_QWEN_CODE_VERSION = '0.21.12'",
            "PINNED_SOURCE_DATE_EPOCH = '1786725153'",
            "process.env['SOURCE_DATE_EPOCH'] !== PINNED_SOURCE_DATE_EPOCH",
            "cliVersion !== PINNED_QWEN_CODE_VERSION",
            "PINNED_QWEN_CODE_COMMIT.slice(0, 12)",
            ").getUTCFullYear()",
        ),
        label=label,
    )


def _validate_exact_tokens_before(state: State) -> None:
    label = "server-authoritative token count precondition"
    limits = "packages/core/src/core/tokenLimits.ts"
    pipeline = "packages/core/src/core/openaiContentGenerator/pipeline.ts"
    chat = "packages/core/src/core/geminiChat.ts"
    _require_all(
        state,
        limits,
        ("export function clampOutputTokensToWindow(", "MIN_CLAMPED_OUTPUT_TOKENS"),
        label=label,
    )
    forbid_text(state, pipeline, "deriveVllmTokenizeUrl", label=label)
    _require_all(
        state,
        chat,
        ("estimatePromptTokens(", "ESTIMATE_CLAMP_OVERHEAD_PAD"),
        label=label,
    )


def _validate_exact_tokens_after(state: State) -> None:
    require_text(
        state,
        "packages/core/src/core/generation-context.ts",
        "{ ...request, generationContext: this.context }",
        count=3,
        label="captured generation owner precedence",
    )
    label = "owned server-authoritative generation result"
    context = "packages/core/src/core/generation-context.ts"
    pipeline = "packages/core/src/core/openaiContentGenerator/pipeline.ts"
    tokenizer = "packages/core/src/core/exact-request-tokenizer.ts"
    chat = "packages/core/src/core/geminiChat.ts"
    content = "packages/core/src/core/contentGenerator.ts"
    _require_all(
        state,
        context,
        (
            "Object.freeze(this)",
            "readonly generationContext: GenerationContext",
            "generateContent(request: GenerateContentParameters",
            "generateContentStream(request: GenerateContentParameters",
            "countRequestTokens(request: GenerateContentParameters",
        ),
        label=label,
    )
    require_text(
        state, context, "generationContext: this.context", count=3, label=label
    )
    _require_all(
        state,
        tokenizer,
        (
            "export function deriveVllmTokenizeUrl(",
            "normalizedPath.endsWith('/v1')",
            "export function validateVllmTokenizeResponse(",
            "refusing to estimate",
            "response.max_model_len",
        ),
        label=label,
    )
    pipeline_source = _require_all(
        state,
        pipeline,
        (
            "async countRequestTokens(",
            "model: wireRequest.model",
            "add_generation_prompt: true",
            "typed['kv_scope'] = request.generationContext.kvScope;",
        ),
        label=label,
    )
    _require_ordered(
        pipeline_source,
        (
            "const wireRequest = await this.buildRequest(",
            "const tokenizeBody: Record<string, unknown>",
            "messages: wireRequest.messages",
            "tokenizeBody['tools'] = wireRequest.tools",
            "tokenizeBody['chat_template_kwargs']",
            "this.client.post<VllmTokenizeResponse>",
            "validateVllmTokenizeResponse(",
        ),
        label=label,
        location=pipeline,
    )
    _require_ordered(
        pipeline_source,
        (
            "this.config.provider.buildRequest(",
            "typed['kv_scope'] = request.generationContext.kvScope;",
        ),
        label=label,
        location=pipeline,
    )
    _require_all(
        state,
        content,
        (
            "OwnedGenerationRequest as GenerateContentParameters",
            "countRequestTokens?(",
            "config.authType !== AuthType.USE_OPENAI",
            "config.exactTokenCounting !== 'vllm'",
            "config.strictToolCalling !== true",
            "Number.isSafeInteger(config.contextWindowSize)",
            "deriveVllmTokenizeUrl(config.baseUrl)",
        ),
        label=label,
    )
    chat_source = _require_all(
        state,
        chat,
        (
            "readonly generationContext: GenerationContext",
            "this.generationContext.bind(",
            "result.maxModelLen !== partition.window",
            "this.getRequestHistoryWithPendingForRoute(",
            "promptTokensForClamp = await countExactRequestTokens(requestContents);",
        ),
        label=label,
    )
    _require(
        chat_source.count("await countExactRequestTokens(") >= 2,
        f"{label}: generation and compaction must use exact rendered requests",
    )
    for path in (chat, "packages/core/src/services/chatCompressionService.ts"):
        for symbol in (
            "estimatePromptTokens(",
            "ESTIMATE_CLAMP_OVERHEAD_PAD",
            "newTokenCountIsEstimated",
        ):
            forbid_text(state, path, symbol, label=label)
    _require_all(
        state,
        "packages/core/src/models/modelsConfig.ts",
        (
            "withSelectionTransaction",
            "rollbackSnapshot",
            "syncAfterAuthRefresh",
        ),
        label=label,
    )


def _validate_stream_commit_before(state: State) -> None:
    label = "strict stream commit barrier precondition"
    chat = "packages/core/src/core/geminiChat.ts"
    converter = "packages/core/src/core/openaiContentGenerator/converter.ts"
    _require_all(
        state,
        chat,
        (
            "TRANSPORT_STREAM_RETRY_CONFIG.maxRetries",
            "maxContinuationRetries",
            "recover these so the agent loop is not broken",
        ),
        label=label,
    )
    forbid_text(state, chat, "const strictToolCalling", label=label)
    forbid_text(state, converter, "requestContext.strictToolCalling", label=label)


def _validate_stream_commit_after(state: State) -> None:
    label = "durable stream commit result"
    chat = "packages/core/src/core/geminiChat.ts"
    converter = "packages/core/src/core/openaiContentGenerator/converter.ts"
    pipeline = "packages/core/src/core/openaiContentGenerator/pipeline.ts"
    source = _require_all(
        state,
        chat,
        (
            "FRESH_RESAMPLE_MAX_RETRIES = 1",
            "let deliveredContent = false;",
            "private readonly chatRecordingService: ChatCommitRecorder",
            "requireServedUsage(usageMetadata, 'completed chat stream')",
        ),
        label=label,
    )
    _require_ordered(
        source,
        (
            "await this.chatRecordingService.recordAssistantTurn({",
            "this.history.push({",
            "committed = true;",
            "syncFunctionCallsField(terminal, committedCalls)",
            "yield terminal;",
        ),
        label=label,
        location=chat,
    )
    for symbol in (
        "maxContinuationRetries",
        "transportContinuationPrefix",
        "const strictToolCalling",
        "extractToolCallsFromText",
    ):
        forbid_text(state, chat, symbol, label=label)
    _require_all(
        state,
        converter,
        (
            "choice.finish_reason !== 'tool_calls'",
            "choice.finish_reason !== 'length'",
            "JSON.parse(toolCall.function.arguments)",
            "tool arguments are not an object",
            "typeof toolCall.function.arguments !== 'string'",
            "Model response completed the tool branch without a tool call.",
            "toolCallParser.hasInvalidToolCallIndex()",
            "toolCallParser.hasConflictingToolCallIdentity()",
            "toolCallParser.hasInvalidToolCallArguments()",
            "parsedToolCalls.every((toolCall) => Boolean(toolCall.id))",
        ),
        label=label,
    )
    _require_all(
        state,
        pipeline,
        (
            "let terminal: GenerateContentResponse | undefined;",
            "INVALID_RESPONSE_SEQUENCE",
            "ResponseObservationError",
        ),
        label=label,
    )


def _validate_tool_policy_before(state: State) -> None:
    label = "universal tool/delegation policy precondition"
    permission = "packages/core/src/permissions/permission-manager.ts"
    agent = "packages/core/src/tools/agent/agent.ts"
    _require_all(
        state,
        permission,
        ("getCoreTools?()", "Non-core tools bypass coreTools allowlist check"),
        label=label,
    )
    forbid_text(state, permission, "strictToolsAllowList", label=label)
    _require_all(
        state,
        agent,
        ("run_in_background", "fork_turns", "working_dir", "model"),
        label=label,
    )
    forbid_text(state, agent, "foregroundOnlyDescription", label=label)


def _validate_tool_policy_after(state: State) -> None:
    label = "universal tool/delegation policy result"
    permission = "packages/core/src/permissions/permission-manager.ts"
    agent = "packages/core/src/tools/agent/agent.ts"
    permission_source = _require_all(
        state,
        permission,
        (
            "private strictToolsAllowList: Set<string> | null = null;",
            "const rawStrictTools = this.config.getStrictTools?.();",
            "rawStrictTools.map((t) => parseRule(t).toolName)",
            "!this.strictToolsAllowList.has(canonicalName)",
            "Non-core tools bypass coreTools allowlist check",
        ),
        label=label,
    )
    _require_ordered(
        permission_source,
        (
            "const canonicalName = resolveToolName(toolName);",
            "!this.strictToolsAllowList.has(canonicalName)",
            "return false;",
            "Non-core tools bypass coreTools allowlist check",
        ),
        label=label,
        location=permission,
    )
    agent_source = _require_all(
        state,
        agent,
        (
            "foregroundOnlyDescription",
            "enum: ['general-purpose', 'Explore']",
            "Background agents are disabled",
            'Parameter "${unsupported[0]}" is disabled',
            "const backgroundRequested =\n        !this.config.getForegroundAgentsOnly() &&",
        ),
        label=label,
    )
    for forbidden_parameter in (
        "fork_turns",
        "fork_tools",
        "fork_profile",
        "run_in_background",
        "isolation",
        "working_dir",
        "model",
        "name",
        "plan_mode_required",
        "read_only",
    ):
        _require(
            f"'{forbidden_parameter}'" in agent_source,
            f"{label}: dispatch no longer rejects {forbidden_parameter}",
        )


def _validate_deployment_prompt_scratch_before(state: State) -> None:
    label = "deployment prompt, scratch, and effect journal precondition"
    builtin = "packages/core/src/subagents/builtin-agents.ts"
    # The git snapshot is a system-prompt layer built from repository data.
    # Upstream bounds the status listing and nothing else, so the branch name
    # and five commit subjects decide how large that layer is.
    git = "packages/core/src/utils/gitUtils.ts"
    require_text(state, git, "    const MAX_STATUS_CHARS = 2000;", label=label)
    forbid_text(state, git, "cappedRepositoryText", label=label)
    forbid_text(state, git, "MAX_BRANCH_CHARS", label=label)
    forbid_text(state, git, "MAX_LOG_CHARS", label=label)
    _require_all(
        state,
        builtin,
        (
            "CRITICAL: READ-ONLY MODE - NO FILE MODIFICATIONS",
            "Creating temporary files anywhere, including /tmp",
            "ToolNames.WEB_FETCH",
        ),
        label=label,
    )
    for path in (
        "packages/core/src/core/qwen38-deployment-prompt.ts",
        "packages/core/src/tools/agent/qwen38-subagent-scratch.ts",
        "packages/core/src/tools/agent/qwen38-effect-journal.ts",
    ):
        _require(path not in state, f"{label}: {path} unexpectedly exists upstream")


def _validate_deployment_prompt_scratch_after(state: State) -> None:
    label = "deployment prompt, scratch, and effect journal result"
    # Every value the git snapshot renders is repository data someone else
    # sized, and all three go through one capping helper that shows the cut.
    # The layer lands in the system prompt of every request, so the startup
    # proof bounds the values by their caps instead of counting them: no
    # repository can make a deployment refuse to start. The caps are bytes,
    # measured after NFC because the served tokenizer normalizes first, and
    # every value is lines behind `git: ` ending in a newline between fixed
    # lines that end in one, so no token spans a value's edge.
    git = "packages/core/src/utils/gitUtils.ts"
    git_source = _require_all(
        state,
        git,
        (
            "const MAX_BRANCH_BYTES = 128;",
            "const MAX_STATUS_BYTES = 1024;",
            "const MAX_LOG_BYTES = 768;",
            "export const GIT_SNAPSHOT_REPOSITORY_BYTES =",
            "MAX_BRANCH_BYTES + MAX_STATUS_BYTES + MAX_LOG_BYTES;",
            "function repositoryLines(",
            "const lines = tokenizerText(",
            ".map((line) => `git: ${line}\\n`)",
            "if (lines.bytes <= maxBytes) return lines.text;",
            "const kept = cutToTokenizerBytes(",
            "if (rendered.bytes > maxBytes) {",
            # A value over its cap is a defect in the cut, and must not pass
            # for a repository that had no snapshot.
            "  return renderGitSnapshot(values);\n}",
            "value(values?.branch ?? '', MAX_BRANCH_BYTES, 'branch --show-current')",
            "value(values?.status ?? '', MAX_STATUS_BYTES, 'status')",
            "value(values?.log ?? '', MAX_LOG_BYTES, 'log --oneline -n 5')",
            "export const GIT_SNAPSHOT_WITHOUT_REPOSITORY_DATA = renderGitSnapshot();",
        ),
        label=label,
    )
    for uncounted in ("MAX_BRANCH_CHARS", "MAX_STATUS_CHARS", "MAX_LOG_CHARS", "cappedRepositoryText", "utf8Prefix"):
        _require(
            uncounted not in git_source,
            f"{label}: {git} still caps a snapshot value by '{uncounted}'; a "
            f"character cap is no token cap, so every value is capped in bytes.",
        )
    for uncapped in (
        "`Current branch: ${branch}`",
        "`Recent commits:\\n${log}`",
    ):
        _require(
            uncapped not in git_source,
            f"{label}: {git} still renders {uncapped} without a cap; every "
            f"value in the snapshot is repository data and goes through "
            f"cappedRepositoryText.",
        )
    # The cases run against a fixture root with the repository walk stubbed.
    # They asserted rendering while silently depending on whether the suite ran
    # from inside a checkout, so in a tree extracted from an archive every one
    # of them returned null before reaching the command it was testing.
    _require_all(
        state,
        "packages/core/src/utils/gitUtils.test.ts",
        (
            "const { mockExistsSync } = vi.hoisted(() => ({",
            "vi.mock('node:fs', async (importOriginal) => {",
            "const REPO_ROOT = '/repo/fixture';",
        ),
        label=label,
    )
    forbid_text(
        state,
        "packages/core/src/utils/gitUtils.test.ts",
        "getRecentGitStatus(process.cwd())",
        label=label,
    )
    _require_all(
        state,
        "packages/core/src/utils/gitUtils.test.ts",
        (
            "truncates a branch name over 128 bytes",
            "truncates recent commits over 768 bytes",
            "cuts a value on a character boundary, within its byte cap",
            "measures a value in the bytes the served tokenizer sees, after NFC",
            "adds at most its repository byte cap to the snapshot without repository data, for any repository",
        ),
        label=label,
    )
    cli = "packages/cli/src/config/config.ts"
    prompt = "packages/core/src/core/qwen38-deployment-prompt.ts"
    prompts = "packages/core/src/core/prompts.ts"
    core = "packages/core/src/agents/runtime/agent-core.ts"
    context = "packages/core/src/agents/runtime/agent-context.ts"
    shell = "packages/core/src/utils/shellContextEnv.ts"
    builtin = "packages/core/src/subagents/builtin-agents.ts"
    agent = "packages/core/src/tools/agent/agent.ts"
    scratch = "packages/core/src/tools/agent/qwen38-subagent-scratch.ts"
    journal = "packages/core/src/tools/agent/qwen38-effect-journal.ts"

    _require_all(
        state,
        cli,
        (
            "QWEN38_AGENT_SERVICE_LOCKED: '1'",
            "QWEN_SYSTEM_MD: '/opt/agent/system.md'",
            "QWEN_DEPLOYMENT_CONTRACT_MD: '/opt/agent/deployment-contract.md'",
            "Locked agent-service configuration forbids CLI system-prompt overrides",
        ),
        label=label,
    )
    _require_all(
        state,
        prompt,
        (
            "QWEN38_LOCKED_SYSTEM_PROMPT_PATH = '/opt/agent/system.md'",
            "'/opt/agent/deployment-contract.md'",
            "stat.isSymbolicLink()",
            "must be nonempty UTF-8-style LF text with a terminal newline",
            "appendQwen38DeploymentContract",
            "appendQwen38SubagentInvocation",
            "getQwen38EngineeringDiscipline",
            "appendQwen38EngineeringDiscipline",
            "appendQwen38MainSessionFrame",
            "locked agent-service subagent prompt was built outside its invocation frame",
            "Private scratch root:",
        ),
        label=label,
    )
    # A subagent's scratch is under /tmp, so its frame says what the
    # deployment contract says of /tmp, in the same plain words: discarded at
    # session teardown, with anything that must survive belonging in
    # /workspace or /artifacts. The service's "bundled" is not the model's
    # word for either.
    require_text(
        state,
        prompt,
        "- Scratch is not a deliverable: it lives under /tmp, which is discarded at "
        "session teardown, so anything that must survive belongs in /workspace or "
        "/artifacts.",
        label=label,
    )
    forbid_text(state, prompt, "bundled", label=label)
    _require_all(
        state,
        prompts,
        (
            "appendQwen38DeploymentContract",
            "appendQwen38MainSessionFrame(fs.readFileSync(systemMdPath, 'utf8'))",
            "  turnBudget?: Qwen38TurnBudget,\n): string {",
            "      contextPartition,\n      turnBudget,\n    );",
        ),
        label=label,
    )
    _require_all(
        state,
        core,
        (
            "getCurrentQwen38SubagentExecution",
            "appendQwen38EngineeringDiscipline(finalPrompt)",
            "appendQwen38DeploymentContract(",
            "appendQwen38SubagentInvocation(",
        ),
        label=label,
    )
    # Reaching the session turn budget ends the session, so the model is told
    # the number: once, as a fixed line of the `## Context` section, read from
    # the budget the session was started with -- a subagent's is its own run's
    # -- and never counted down. A budget that is no whole number of turns
    # cannot be stated, and a locked contract built without one is refused.
    # The number is what the startup proof leaves out and bounds by the widest
    # a safe integer renders to, so the form it counts must differ from the
    # form sent in the digits alone.
    prompt_source = _require_all(
        state,
        prompt,
        (
            "export const QWEN38_TURN_BUDGET_LEFT_OUT = 'left out';",
            "export const QWEN38_TURN_BUDGET_BYTES = String(Number.MAX_SAFE_INTEGER).length;",
            "  if (turnBudget === QWEN38_TURN_BUDGET_LEFT_OUT) return '';\n  if (!Number.isSafeInteger(turnBudget) || turnBudget < 1) {",
            "`- This session may run at most ${turnBudgetText(turnBudget)} turns. Reaching that number ends the session.`,",
            "'locked agent-service states its turn budget in the deployment contract, and none was supplied',",
            "${qwen38ContextSection(partition, turnBudget)}",
        ),
        label=label,
    )
    _require(
        prompt_source.count("turnBudgetText(") == 2,
        f"{label}: {prompt} states the turn budget somewhere other than its one line",
    )
    _require_ordered(
        _source(state, core, label=label),
        (
            "const deploymentPrompt = appendQwen38DeploymentContract(",
            "appendQwen38EngineeringDiscipline(finalPrompt),",
            "partitionContextWindow(subagentWindow),",
            "this.runConfig.max_turns,",
        ),
        label=label,
        location=core,
    )
    for counted in (
        prompt,
        "packages/core/src/core/client.ts",
        "packages/core/src/core/geminiChat.ts",
        "packages/cli/src/nonInteractiveCli.ts",
        core,
    ):
        for countdown in (
            "turns remaining",
            "Turns remaining",
            "remaining turns",
            "Remaining turns",
            "turns left",
            "Turns left",
            "turnsRemaining",
            "remainingTurns",
        ):
            forbid_text(state, counted, countdown, label=label)
    for case in (
        "states the turn budget the session was given, once, and that reaching it ends the session",
        "leaves out only the budget's digits in the form the startup proof counts",
        "refuses a turn budget that is not a whole number of turns",
    ):
        require_text(
            state, "packages/core/src/core/qwen38-deployment-prompt.test.ts", case, label=label
        )
    _require_all(
        state,
        context,
        (
            "export interface Qwen38SubagentExecutionContext",
            "readonly scratchDir: string;",
            "readonly subagentType: 'general-purpose' | 'Explore';",
            "getCurrentQwen38SubagentExecution",
        ),
        label=label,
    )
    _require_all(
        state,
        shell,
        (
            "env['QWEN_SUBAGENT_SCRATCH'] = scratch;",
            "env['TMPDIR'] = scratch;",
            "env['XDG_CACHE_HOME'] = `${scratch}/cache`;",
            "env['PIP_CACHE_DIR'] = `${scratch}/pip`;",
            "env['NPM_CONFIG_CACHE'] = `${scratch}/npm`;",
            "env['CARGO_HOME'] = `${scratch}/cargo`;",
            "env['GOPATH'] = `${scratch}/go`;",
        ),
        label=label,
    )
    scratch_source = _require_all(
        state,
        scratch,
        (
            "QWEN38_SUBAGENT_SCRATCH_ROOT = '/tmp/qwen-subagents'",
            "fs.mkdtempSync(path.join(scratchRoot, `${subagentType}-`))",
            "stat.isSymbolicLink()",
            "must have mode 0700",
            "fs.realpathSync(directory) !== path.resolve(directory)",
        ),
        label=label,
    )
    _require(
        scratch_source.count("createPrivateDirectory(directory)") == 1,
        f"{label}: language-specific scratch children are no longer created uniformly",
    )
    _require_all(
        state,
        journal,
        (
            "QWEN38_EFFECT_JOURNAL_ROOT = '/qwen-runtime/effects'",
            "constants.O_RDONLY | constants.O_NOFOLLOW",
            "createHash('sha256')",
            "regular file changed while it was being hashed",
            "snapshotRoot('workspace'",
            "snapshotRoot('artifacts'",
            "contentModified",
            "metadataModified",
            "QWEN38_TRUSTED_EXPLORE_EFFECT_JOURNAL_V1",
            "path_list_truncated=true",
            "Read the exact hashed manifest before relying on omitted path details.",
        ),
        label=label,
    )
    builtin_source = _require_all(
        state,
        builtin,
        (
            "File operations can affect the real workspace",
            "ToolNames.WRITE_FILE",
            "ToolNames.EDIT",
            "ToolNames.NOTEBOOK_EDIT",
            "a report does not establish exclusive attribution of concurrent or external effects",
        ),
        label=label,
    )
    _require(
        "CRITICAL: READ-ONLY MODE" not in builtin_source
        and "ToolNames.WEB_FETCH," not in builtin_source
        and "ToolNames.SKILL," not in builtin_source,
        f"{label}: Explore retained the obsolete read-only/network/plugin surface",
    )
    agent_source = _require_all(
        state,
        agent,
        (
            "createQwen38SubagentScratch(subagentConfig.name)",
            "beginQwen38EffectJournal()",
            "await finishQwen38EffectJournal(effectJournal)",
            "subagent execution and mandatory Explore effect journaling both failed",
            "qwen38EffectSummary",
            "let runFailure: { error: unknown } | undefined;",
            "runFailure = { error };",
            "[runFailure.error, journalError]",
        ),
        label=label,
    )
    _require_ordered(
        agent_source,
        (
            "const effectJournal =",
            "await beginQwen38EffectJournal()",
            "stopHookWarning = await runFramed();",
            "await finishQwen38EffectJournal(effectJournal)",
            "if (runFailure) throw runFailure.error;",
        ),
        label=label,
        location=agent,
    )


def _validate_image_before(state: State) -> None:
    label = "full-quality chronological image precondition"
    image = "packages/core/src/utils/image-view.ts"
    files = "packages/core/src/utils/fileUtils.ts"
    converter = "packages/core/src/core/openaiContentGenerator/converter.ts"
    _require_all(
        state,
        image,
        ("mimeType: 'image/jpeg'", "boundedSize(sourceWidth, sourceHeight, 1)"),
        label=label,
    )
    _require_all(
        state,
        files,
        ("willRenderPdfImages", "fall through to the legacy"),
        label=label,
    )
    forbid_text(state, converter, "accepts only inline image/png", label=label)


def _validate_image_after(state: State) -> None:
    label = "full-quality chronological image result"
    image = "packages/core/src/utils/image-view.ts"
    files = "packages/core/src/utils/fileUtils.ts"
    converter = "packages/core/src/core/openaiContentGenerator/converter.ts"
    image_source = _require_all(
        state,
        image,
        (
            "QWEN38_IMAGE_MAX_PIXELS = 16_777_216",
            "QWEN38_IMAGE_MAX_ASPECT_RATIO = 30",
            "const PNG_SIGNATURE = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10])",
            "function inspectStrictPng(",
            "IHDR is not the first chunk",
            "source pixels are not 8-bit RGB or RGBA",
            "type === 'acTL' || type === 'fcTL' || type === 'fdAT'",
            "tRNS transparency is forbidden",
            "missing IDAT",
            "bytes follow IEND",
            "limitInputPixels: QWEN38_IMAGE_MAX_PIXELS",
            ".raw()",
            "metadata.orientation !== 1",
            "const original = await prepareStrictOriginalPng(filePath, signal);",
            "bytes: original.bytes",
            "mimeType: 'image/png'",
        ),
        label=label,
    )
    _require_ordered(
        image_source,
        (
            "const bytes = await fs.readFile(filePath, { signal });",
            "const header = inspectStrictPng(bytes, filePath);",
            "const pixels = header.width * header.height;",
            "const decoderOptions =",
            ".raw()",
            "return { bytes, width: header.width, height: header.height };",
        ),
        label=label,
        location=image,
    )
    files_source = _require_all(
        state,
        files,
        (
            "const shouldRenderImageOverview = fileType === 'image';",
            "Raster image did not enter the required PNG validator.",
            "const view = await renderImageOverview(",
            "data: view.bytes.toString('base64')",
            "mimeType: view.mimeType",
            "if (!(error instanceof ImageViewError)) throw error;",
        ),
        label=label,
    )
    image_case_start = files_source.index("case 'image':")
    image_case_end = files_source.index("case 'audio':", image_case_start)
    image_case = files_source[image_case_start:image_case_end]
    _require(
        "base64SizeInMB" not in image_case
        and "fs.promises.readFile" not in image_case
        and "mediaMimeType" not in image_case,
        f"{label}: legacy forward-verbatim image fallback remains reachable",
    )
    converter_source = _require_all(
        state,
        converter,
        (
            "mimeType !== 'image/png'",
            "accepts only inline image/png data",
            "forbids file/remote image references",
        ),
        label=label,
    )
    _require(
        converter_source.count("forbids file/remote image references") == 1,
        f"{label}: image transport refusal is missing or ambiguously duplicated",
    )


def _validate_model_config_before(state: State) -> None:
    label = "model semantic-field propagation precondition"
    constants = "packages/core/src/models/constants.ts"
    config = "packages/core/src/models/content-generator-config.ts"
    forbid_text(state, constants, "'strictToolCalling'", label=label)
    forbid_text(state, constants, "'exactTokenCounting'", label=label)
    forbid_text(state, config, "field === 'strictToolCalling'", label=label)


def _validate_model_config_after(state: State) -> None:
    label = "model semantic-field propagation result"
    constants = "packages/core/src/models/constants.ts"
    config = "packages/core/src/models/content-generator-config.ts"
    types = "packages/core/src/models/types.ts"
    _require_all(
        state,
        constants,
        ("'strictToolCalling'", "'exactTokenCounting'"),
        label=label,
    )
    config_source = _require_all(
        state,
        config,
        (
            "nextConfig.strictToolCalling = undefined;",
            "nextConfig.exactTokenCounting = undefined;",
            "field === 'strictToolCalling'",
            "field === 'exactTokenCounting'",
        ),
        label=label,
    )
    _require_ordered(
        config_source,
        (
            "if (modelId && modelId !== parentConfig.model)",
            "nextConfig.thinkingMandatory = undefined;",
            "nextConfig.strictToolCalling = undefined;",
            "nextConfig.exactTokenCounting = undefined;",
        ),
        label=label,
        location=config,
    )
    _require_all(
        state,
        types,
        ("| 'strictToolCalling'", "| 'exactTokenCounting'"),
        label=label,
    )


def _validate_behavioral_evidence_before(state: State) -> None:
    label = "focused behavioral evidence precondition"
    _require(
        "packages/core/src/config/qwen38-agent-service-contract.test.ts" not in state,
        f"{label}: locked contract test unexpectedly exists upstream",
    )
    _require(
        "packages/core/src/core/openaiContentGenerator/pipeline.tokenize.test.ts"
        not in state,
        f"{label}: exact tokenizer test unexpectedly exists upstream",
    )
    _require(
        "packages/core/src/utils/qwen38-image-contract.test.ts" not in state,
        f"{label}: strict image contract test unexpectedly exists upstream",
    )


def _validate_behavioral_evidence_after(state: State) -> None:
    label = "focused behavioral evidence result"
    required_evidence = {
        "packages/core/src/config/qwen38-agent-service-contract.test.ts": (
            "disables ambient extension, hook, and skill initialization",
            "getForegroundAgentsOnly",
        ),
        "packages/core/src/core/openaiContentGenerator/pipeline.tokenize.test.ts": (
            "derives the root tokenizer endpoint from a /v1 API base",
            "fails closed on malformed or mismatched responses",
        ),
        "packages/core/src/core/geminiChat.test.ts": (
            "gives every turn the same room, whatever its prompt",
            "refuses when the request cannot be counted at all",
            "resamples one invalid pre-content stream",
            "never resamples an invalid stream after visible output escaped",
            "publishes MAX_TOKENS without another request",
            "literal response preservation",
            "maps maxRetries zero to one outer establishment attempt",
            "literal-with-structure",
        ),
        "packages/core/src/services/chatCompressionService.test.ts": (
            "sends the last issued prompt, and nothing after it, to one cache-preserving main-model request",
            "rejects unusable summary output",
            "name: STATE_SNAPSHOT_FUNCTION_NAME,",
            "requires an exact shrinking candidate that leaves a turn issuable",
        ),
        "packages/core/src/core/baseLlmClient.test.ts": (
            "uses the authoritative tokenizer and forwards the identical rendered-request options",
            "fails closed when exact counting is required but the generator lacks it",
            "returns the terminal streaming finish reason",
        ),
        "packages/core/src/core/openaiContentGenerator/converter.test.ts": (
            "preserves text-image-text chronology inside the originating tool result",
            "suppresses diagnostic tool prefixes on a length terminal in strict mode",
            "requires a tool_calls terminal in strict mode",
            "exposes a completed identified call in strict mode",
        ),
        "packages/core/src/permissions/permission-manager.test.ts": (
            "strictTools universally gates core, dynamic, and synthetic tools",
        ),
        "packages/core/src/tools/agent/agent.test.ts": (
            "exposes only sequential built-in delegation in foreground-agents-only mode",
        ),
        "packages/core/src/core/qwen38-deployment-prompt.test.ts": (
            "requires both immutable paths in the locked runtime",
            "requires and appends the unique invocation scratch in locked subagents",
        ),
        "packages/core/src/tools/agent/qwen38-subagent-scratch.test.ts": (
            "creates a unique private tree for each invocation",
            "refuses a symlinked scratch root",
        ),
        "packages/core/src/tools/agent/qwen38-effect-journal.test.ts": (
            "reports content, metadata, creation, removal, symlink, and artifact effects",
            "does not treat scratch-only writes as project effects",
            "refuses a symlinked project root instead of following it",
        ),
        "packages/core/src/subagents/builtin-agents.test.ts": (
            "gives Explore writable local investigation tools without control-plane tools",
        ),
        "packages/core/src/utils/shellContextEnv.test.ts": (
            "routes every subagent scratch/cache variable through its invocation tree",
        ),
        "packages/core/src/utils/qwen38-image-contract.test.ts": (
            "emits the exact original PNG bytes at their full dimensions",
            "fails closed for JPEG instead of transcoding or forwarding it",
        ),
    }
    for path, needles in required_evidence.items():
        _require_all(state, path, needles, label=label)


def _validate_session_time_before(state: State) -> None:
    label = "session time-anchor precondition"
    prompt = "packages/core/src/core/qwen38-deployment-prompt.ts"
    # The deployment-prompt module is created by this patch set; in the
    # pristine tree there is nothing to check.
    if prompt in state:
        forbid_text(state, prompt, "QWEN38_SESSION_STARTED_AT_UTC", label=label)


def _validate_session_time_after(state: State) -> None:
    label = "CLI invocation time anchor result"
    prompt = "packages/core/src/core/qwen38-deployment-prompt.ts"
    require_text(
        state,
        prompt,
        "const QWEN38_INVOCATION_STARTED_AT_UTC = new Date().toISOString();",
        label=label,
    )
    require_text(
        state,
        prompt,
        "CLI invocation started: ${QWEN38_INVOCATION_STARTED_AT_UTC}",
        label=label,
    )


def _validate_stream_evidence_before(state: State) -> None:
    label = "headless stream-evidence precondition"
    adapter = "packages/cli/src/nonInteractive/io/BaseJsonOutputAdapter.ts"
    forbid_text(
        state, adapter, "must carry what the model actually received", label=label
    )


def _validate_stream_evidence_after(state: State) -> None:
    label = "headless stream-evidence result"
    adapter = "packages/cli/src/nonInteractive/io/BaseJsonOutputAdapter.ts"
    adapter_test = "packages/cli/src/nonInteractive/io/BaseJsonOutputAdapter.test.ts"
    # The emitted stream is the session's evidentiary record: the
    # tool_result content must be the model-facing responseParts, with the
    # short human-facing display string only as a fallback.
    adapter_source = _source(state, adapter, label=label)
    _require_ordered(
        adapter_source,
        (
            "must carry what the model actually received",
            "return functionResponsePartsToString(response.responseParts);",
            "return response.resultDisplay;",
        ),
        label=label,
        location=adapter,
    )
    require_text(
        state,
        adapter_test,
        "[stream-evidence] prefers model-facing parts over the display banner",
        label=label,
    )
    require_text(
        state,
        adapter_test,
        "[stream-evidence] falls back to the display when no parts exist",
        label=label,
    )


def _validate_compaction_event_before(state: State) -> None:
    label = "headless compaction event precondition"
    chat = "packages/core/src/core/geminiChat.ts"
    turn = "packages/core/src/core/turn.ts"
    agent_core = "packages/core/src/agents/runtime/agent-core.ts"
    adapter = "packages/cli/src/nonInteractive/io/BaseJsonOutputAdapter.ts"
    helpers = "packages/cli/src/utils/nonInteractiveHelpers.ts"
    # Upstream reports only the successful half of a compaction, and only as
    # an interactive signal; a refused or failed attempt yields nothing.
    require_text(
        state,
        chat,
        "Failed/skipped compaction attempts are silent.",
        label=label,
    )
    # A subagent's compaction reaches nothing but the debug log.
    require_text(state, agent_core, "[AGENT-COMPACT]", label=label)
    forbid_text(state, turn, "ChatCompaction", label=label)
    forbid_text(state, adapter, "'compaction'", label=label)
    forbid_text(state, helpers, "'compaction'", label=label)


def _validate_compaction_event_after(state: State) -> None:
    label = "headless compaction event result"
    chat = "packages/core/src/core/geminiChat.ts"
    turn = "packages/core/src/core/turn.ts"
    agent_core = "packages/core/src/agents/runtime/agent-core.ts"
    agent_tool = "packages/core/src/tools/agent/agent.ts"
    tools = "packages/core/src/tools/tools.ts"
    adapter = "packages/cli/src/nonInteractive/io/BaseJsonOutputAdapter.ts"
    helpers = "packages/cli/src/utils/nonInteractiveHelpers.ts"
    chat_test = "packages/core/src/core/geminiChat.test.ts"
    agent_tool_test = "packages/core/src/tools/agent/agent.test.ts"
    adapter_test = "packages/cli/src/nonInteractive/io/BaseJsonOutputAdapter.test.ts"
    helpers_test = "packages/cli/src/utils/nonInteractiveHelpers.test.ts"

    # One projection builds the emitted record, so the main session and every
    # subagent carry the same field set, and `succeeded` is derived from the
    # status rather than assumed.
    _require_all(
        state,
        turn,
        (
            "ChatCompaction = 'chat_compaction'",
            "export interface CompactionRecord {",
            "export function toCompactionRecord(",
            "succeeded: info.compressionStatus === CompressionStatus.COMPRESSED,",
            "ServerGeminiChatCompactionEvent",
        ),
        label=label,
    )
    require_text(
        state,
        turn,
        "? info.newTokenCount\n        : info.originalTokenCount",
        label=label,
    )
    _require_all(
        state,
        "packages/cli/src/ui/hooks/useGeminiStream.ts",
        (
            "case ServerGeminiEventType.ChatCompaction:",
            "!event.value.succeeded && event.value.status !== 'NOOP'",
            "the original context was preserved.",
        ),
        label=label,
    )
    # The bridge in turn.ts must forward the record itself.
    require_text(
        state,
        turn,
        "type: GeminiEventType.ChatCompaction,",
        label=label,
    )

    # Emission is gated on "an attempt completed", not on success, so a
    # refusal cannot be silent. NOOP means no attempt ran and stays silent.
    chat_source = _require_all(
        state,
        chat,
        (
            "COMPACTION = 'compaction'",
            "type: StreamEventType.COMPACTION;",
        ),
        label=label,
    )
    forbid_text(
        state,
        chat,
        "Failed/skipped compaction attempts are silent.",
        label=label,
    )
    _require(
        chat_source.count(
            "if (compressionInfo.compressionStatus !== CompressionStatus.NOOP) {"
        )
        == 1,
        f"{label}: {chat} no longer reports failed pre-stream compactions",
    )
    _require(
        chat_source.count("type: StreamEventType.COMPACTION,") == 1,
        f"{label}: one pre-stream owning seam must emit the compaction attempt",
    )
    _require_ordered(
        chat_source,
        (
            "await this.chatRecordingService.recordChatCompression({",
            "await this.chatRecordingService.flush()",
        ),
        label=label,
        location=chat,
    )

    # Subagent attribution: the agent event carries the record to the agent
    # tool's display, and the headless bridge stamps the owning tool-call id.
    require_text(
        state,
        agent_core,
        "this.eventEmitter?.emit(AgentEventType.COMPACTION, {",
        label=label,
    )
    require_text(state, tools, "compactions?: CompactionRecord[];", label=label)
    require_text(
        state,
        agent_tool,
        "this.currentCompactions.push(event.compaction);",
        label=label,
    )
    require_text(
        state,
        helpers,
        "adapter.emitSystemMessage('compaction', compaction, agentToolCallId);",
        label=label,
    )

    # emitSystemMessage must be able to carry a parent id at all — without
    # that, every subagent compaction would be silently reattributed to the
    # parent thread — and it must default to null for the main session.
    require_text(
        state,
        adapter,
        "  emitSystemMessage(\n"
        "    subtype: string,\n"
        "    data?: unknown,\n"
        "    parentToolUseId?: string | null,\n"
        "  ): void;",
        label=label,
    )
    require_text(
        state,
        adapter,
        "  emitSystemMessage(\n"
        "    subtype: string,\n"
        "    data?: unknown,\n"
        "    parentToolUseId: string | null = null,\n"
        "  ): void {",
        label=label,
    )
    require_text(
        state,
        adapter,
        "      session_id: this.getSessionId(),\n"
        "      parent_tool_use_id: parentToolUseId,\n"
        "      data,",
        label=label,
    )
    require_text(state, adapter, "case GeminiEventType.ChatCompaction:", label=label)
    require_text(
        state,
        adapter,
        "this.emitSystemMessage('compaction', event.value, null);",
        label=label,
    )

    require_text(
        state,
        chat_test,
        "yields a COMPACTION record and no COMPRESSED event when the attempt fails",
        label=label,
    )
    require_text(
        state,
        adapter_test,
        "[compaction-event] emits a main-session compaction record with before/after tokens",
        label=label,
    )
    require_text(
        state,
        adapter_test,
        "[compaction-event] records a refused attempt as a failure rather than staying silent",
        label=label,
    )
    require_text(
        state,
        adapter_test,
        "[compaction-event] attributes a system message to a subagent when given its tool-call id",
        label=label,
    )
    require_text(
        state,
        helpers_test,
        "[compaction-event] emits each new subagent compaction exactly once, "
        "attributed to the agent tool call",
        label=label,
    )
    require_text(
        state,
        agent_tool_test,
        "[compaction-event] accumulates every subagent compaction attempt on "
        "the display, oldest first",
        label=label,
    )


def _validate_subagent_result_scope_before(state: State) -> None:
    label = "subagent result scope precondition"
    types = "packages/cli/src/nonInteractive/types.ts"
    adapter = "packages/cli/src/nonInteractive/io/BaseJsonOutputAdapter.ts"
    helpers = "packages/cli/src/utils/nonInteractiveHelpers.ts"
    agent_tool = "packages/core/src/tools/agent/agent.ts"
    tools = "packages/core/src/tools/tools.ts"
    # A result message carries no scope at all, so a subagent's own terminal
    # record is indistinguishable from the session's.
    require_text(
        state,
        types,
        "  uuid: string;\n  session_id: string;\n  is_error: false;",
        label=label,
    )
    require_text(
        state,
        types,
        "  uuid: string;\n  session_id: string;\n  is_error: true;",
        label=label,
    )
    # The owning id is in scope at the emit site and dropped one line later.
    require_text(
        state,
        adapter,
        "const errorResult = this.buildSubagentErrorResult(errorMessage, numTurns);",
        label=label,
    )
    # The subagent's turn count is hardcoded to zero on the emitted record...
    require_text(
        state,
        helpers,
        "adapter.emitSubagentErrorResult(errorMessage, 0, agentToolCallId);",
        label=label,
    )
    # ...and the display it would have to come from does not carry one.
    forbid_text(state, tools, "turnsUsed?: number;", label=label)
    # The model-visible construction on the foreground path takes no count.
    require_text(
        state,
        agent_tool,
        "toModelVisibleSubagentResult(subagent.getFinalText(), terminateMode),",
        label=label,
    )


def _validate_subagent_result_scope_after(state: State) -> None:
    label = "subagent result scope result"
    types = "packages/cli/src/nonInteractive/types.ts"
    adapter = "packages/cli/src/nonInteractive/io/BaseJsonOutputAdapter.ts"
    adapter_test = "packages/cli/src/nonInteractive/io/BaseJsonOutputAdapter.test.ts"
    helpers = "packages/cli/src/utils/nonInteractiveHelpers.ts"
    helpers_test = "packages/cli/src/utils/nonInteractiveHelpers.test.ts"
    agent_tool = "packages/core/src/tools/agent/agent.ts"
    agent_tool_test = "packages/core/src/tools/agent/agent.test.ts"
    tools = "packages/core/src/tools/tools.ts"

    # Both result shapes declare the scope, so no result can be emitted
    # without saying which thread it ends.
    require_text(
        state,
        types,
        "  parent_tool_use_id: string | null;\n  is_error: false;",
        label=label,
    )
    require_text(
        state,
        types,
        "  parent_tool_use_id: string | null;\n  is_error: true;",
        label=label,
    )

    # The owning id is required by the builder's signature, threaded from the
    # emit site, and stamped on the record.
    require_text(
        state,
        adapter,
        "  protected buildSubagentErrorResult(\n"
        "    terminateMode: SubagentStopState,\n"
        "    errorMessage: string,\n"
        "    numTurns: number,\n"
        "    parentToolUseId: string,\n"
        "  ): CLIResultMessageError {",
        label=label,
    )
    require_text(
        state,
        adapter,
        "    const errorResult = this.buildSubagentErrorResult(\n"
        "      terminateMode,\n"
        "      errorMessage,\n"
        "      numTurns,\n"
        "      parentToolUseId,\n"
        "    );",
        label=label,
    )
    require_text(
        state,
        adapter,
        "      parent_tool_use_id: parentToolUseId,\n      is_error: true,",
        label=label,
    )
    forbid_text(
        state,
        adapter,
        "this.buildSubagentErrorResult(errorMessage, numTurns)",
        label=label,
    )
    # Both branches of the session's own result stamp null, so "main session"
    # is stated rather than inferred from an absent field.
    require_text(
        state,
        adapter,
        "        parent_tool_use_id: null,\n",
        count=2,
        label=label,
    )

    # The subagent's turn count reaches the emitted record through the same
    # display channel every other subagent event already uses, published in
    # the update that carries the terminal status.
    require_text(state, tools, "  turnsUsed?: number;", label=label)
    require_text(
        state,
        agent_tool,
        "      const turnsUsed = subagent.getTurnsUsed();",
        label=label,
    )
    require_text(
        state,
        agent_tool,
        "            turnsUsed,\n          },",
        count=2,
        label=label,
    )
    require_text(
        state,
        helpers,
        "          adapter.emitSubagentErrorResult(\n"
        "            stoppedSubagentState(taskDisplay.terminateMode),\n"
        "            errorMessage,\n"
        "            taskDisplay.turnsUsed ?? 0,\n"
        "            agentToolCallId,\n"
        "          );",
        label=label,
    )
    forbid_text(
        state,
        helpers,
        "adapter.emitSubagentErrorResult(errorMessage, 0, agentToolCallId);",
        label=label,
    )

    # Both reachable model-visible constructions carry the turn count. The one
    # inside runSubagentWithHooks only feeds the display; the foreground body
    # builds the text the parent model actually reads, and it is the one that
    # was losing the count.
    require_text(
        state,
        agent_tool,
        "        toModelVisibleSubagentResult(\n"
        "          subagentRawText,\n"
        "          terminateMode,\n"
        "          subagent.getTurnsUsed(),\n"
        "          subagent.getLoopType(),\n"
        "          subagent.getFinalMessageSlip(),\n"
        "        ),",
        label=label,
    )
    require_text(
        state,
        agent_tool,
        "              toModelVisibleSubagentResult(\n"
        "                subagent.getFinalText(),\n"
        "                terminateMode,\n"
        "                subagent.getTurnsUsed(),\n"
        "                subagent.getLoopType(),\n"
        "                subagent.getFinalMessageSlip(),\n"
        "              ),",
        label=label,
    )
    forbid_text(
        state,
        agent_tool,
        "toModelVisibleSubagentResult(subagent.getFinalText(), terminateMode)",
        label=label,
    )

    require_text(
        state,
        adapter_test,
        "[subagent-scope] scopes a subagent error result to the agent tool "
        "call that owns it",
        label=label,
    )
    require_text(
        state,
        adapter_test,
        "[subagent-scope] leaves the session terminal result unscoped",
        label=label,
    )
    require_text(
        state,
        adapter_test,
        "[subagent-scope] leaves a session error result unscoped too",
        label=label,
    )
    require_text(
        state,
        helpers_test,
        "[subagent-scope] reports the turn count the stopped subagent reached",
        label=label,
    )
    require_text(
        state,
        agent_tool_test,
        "[subagent-scope] tells the parent how far an exhausted subagent got",
        label=label,
    )
    require_text(
        state,
        agent_tool_test,
        "[subagent-scope] renders a single completed turn in the singular",
        label=label,
    )
    require_text(
        state,
        agent_tool_test,
        "[subagent-scope] publishes the turn count with the terminal display status",
        label=label,
    )

    # A subagent that stopped before its goal is a failed call, whatever
    # stopped it, and so is one that could not be run: one error type, and a
    # message the text the parent reads already carries, so the scheduler's
    # failure path sends the model that text unchanged. The labelled report
    # opens with the summary the failed call carries; only GOAL is a success.
    subagent_result = "packages/core/src/agents/subagent-result.ts"
    _require_all(
        state,
        subagent_result,
        (
            "export function describeUnfinishedSubagent(\n"
            "  terminateMode: Exclude<AgentTerminateMode, AgentTerminateMode.GOAL>,\n",
            "  return `subagent ${reason}`;\n",
            "    const summary = describeUnfinishedSubagent(\n"
            "      terminateMode,\n"
            "      turnsUsed,\n"
            "      loopType,\n"
            "      finalMessageSlip,\n"
            "    );\n",
            "      ? `[${summary}; its assignment is unfinished and the report below is partial]\\n\\n${partial}`\n"
            "      : `[${summary} and produced no report; its assignment is unfinished]`;",
        ),
        label=label,
    )
    _require_ordered(
        _source(state, agent_tool, label=label),
        (
            "          if (terminateMode === AgentTerminateMode.GOAL) {\n"
            "            const visibleFinalText =\n"
            "              finalText || '(subagent produced no model-visible output)';\n"
            "            return {\n"
            "              llmContent: [{ text: visibleFinalText + wtSuffix }],\n"
            "              returnDisplay: this.currentDisplay!,\n"
            "            };\n"
            "          }\n",
            "          const unfinished = {\n"
            "            type: ToolErrorType.EXECUTION_FAILED,\n"
            "            message: describeUnfinishedSubagent(\n"
            "              terminateMode,\n"
            "              subagent.getTurnsUsed(),\n"
            "              subagent.getLoopType(),\n"
            "              subagent.getFinalMessageSlip(),\n"
            "            ),\n"
            "          };\n",
            "                  text: `Agent was cancelled by the user. Partial result follows:\\n\\n${finalText}${wtSuffix}`,\n"
            "                },\n"
            "              ],\n"
            "              returnDisplay: this.currentDisplay!,\n"
            "              error: unfinished,\n"
            "            };\n",
            "            llmContent: [{ text: finalText + wtSuffix }],\n"
            "            returnDisplay: this.currentDisplay!,\n"
            "            error: unfinished,\n"
            "          };\n",
            "      const failedToRun = `Failed to run subagent: ${errorMessage}`;\n"
            "      return {\n"
            "        llmContent: `${failedToRun}${wtSuffix}${effectSuffix}`,\n"
            "        returnDisplay: this.currentDisplay!,\n"
            "        error: { type: ToolErrorType.EXECUTION_FAILED, message: failedToRun },\n"
            "      };\n",
        ),
        label=label,
        location=agent_tool,
    )
    # The retired shape: an unfinished subagent returned as a success, with
    # fallbacks for a report the label means is never empty.
    for retired in (
        "if (terminateMode === AgentTerminateMode.ERROR) {",
        "'Subagent execution failed.'",
        "'(no partial result captured)'",
    ):
        forbid_text(state, agent_tool, retired, label=label)
    for path, case in (
        (agent_tool_test,
         "a subagent that stopped as %s is a failed call, and the parent still reads its labelled report"),
        (agent_tool_test, "a subagent that reached its goal is not a failed call"),
        ("packages/core/src/agents/subagent-result.test.ts",
         "opens the report of a subagent that stopped as %s with the summary its failed call carries"),
    ):
        require_text(state, path, case, label=label)


def _validate_pages_affordance_before(state: State) -> None:
    label = "read_file range-mechanism precondition"
    tool = "packages/core/src/tools/read-file.ts"
    files = "packages/core/src/utils/fileUtils.ts"
    pdf = "packages/core/src/utils/pdf.ts"
    # Upstream advertises the PDF-only page range on every file type.
    require_text(state, tool, "  pages?: string;", label=label)
    require_text(
        state,
        tool,
        "          pages: {\n            description: `Optional: For PDF files,",
        label=label,
    )
    require_text(state, tool, "        pages: this.params.pages,", label=label)
    # And every remediation message points back at that parameter.
    require_text(
        state,
        pdf,
        "Use the 'pages' parameter to read a specific page range",
        label=label,
    )
    forbid_text(state, pdf, "PDF_PAGE_RANGE_REMEDY", label=label)
    forbid_text(state, files, "applies only to PDF files", label=label)
    require_text(
        state,
        files,
        "if (fileType === 'pdf' && normalizedPages !== undefined) {",
        label=label,
    )


def _validate_pages_affordance_after(state: State) -> None:
    label = "read_file range-mechanism result"
    tool = "packages/core/src/tools/read-file.ts"
    files = "packages/core/src/utils/fileUtils.ts"
    pdf = "packages/core/src/utils/pdf.ts"
    tool_test = "packages/core/src/tools/read-file.test.ts"

    # The parameter is gone from the tool's type, its advertised schema, and
    # everything it forwards: the model is never offered a parameter whose
    # applicability it cannot evaluate from the schema.
    tool_source = _source(state, tool, label=label)
    forbid_text(state, tool, "  pages?: string;", label=label)
    forbid_text(state, tool, "          pages: {", label=label)
    forbid_text(state, tool, "this.params.pages", label=label)
    forbid_text(state, tool, "parsePDFPageRange", label=label)
    _require(
        tool_source.count("            type: 'integer',") == 2
        and tool_source.count("          file_path: {") == 1,
        f"{label}: {tool} must advertise exactly file_path, offset, and limit",
    )
    # A supplied argument is refused rather than silently dropped. The refusal
    # is no longer a special case for one name: the schema is closed, so
    # `pages` is one instance of the general rule that a tool does not accept
    # a parameter it does not declare.
    require_text(state, tool, "        additionalProperties: false,", label=label)
    require_text(
        state,
        "packages/core/src/tools/tools.ts",
        "export function undeclaredToolParamsError(",
        label=label,
    )

    # One remedy, declared once and used by every message that has to send
    # the model somewhere, so no guidance names a parameter that is gone --
    # and rendered for the file the caller actually named, so the command can
    # be run as written.
    require_text(state, pdf, "export function pdfPageRangeRemedy(", label=label)
    require_text(
        state, pdf, "export const PDF_PAGE_RANGE_REMEDY_TEMPLATE =", label=label
    )
    forbid_text(state, pdf, "export const PDF_PAGE_RANGE_REMEDY =", label=label)
    forbid_text(state, pdf, "'pages' parameter", label=label)
    forbid_text(state, files, "'pages' parameter to", label=label)
    _require(
        _source(state, pdf, label=label).count("pdfPageRangeRemedy(") == 6
        and _source(state, files, label=label).count("pdfPageRangeRemedy(") == 1,
        f"{label}: every large/truncated PDF message must name the one remedy",
    )
    # No message may send the model to a read_file page range that read_file
    # refuses. This one did, and it was reachable for any text-only model.
    forbid_text(state, files, "read_file on the original PDF with pages", label=label)
    forbid_text(
        state,
        "packages/core/src/services/visionBridge/vision-bridge-service.ts",
        "read_file on the original PDF with a later page range",
        label=label,
    )
    # The shared consumption layer no longer has a `pages` option to guard.
    # A runtime guard was the weaker form of this: it left the parameter
    # representable and relied on a check to refuse it. The option is gone, so
    # a caller cannot express a page range at all.
    forbid_text(state, files, "normalizedPages", label=label)
    forbid_text(state, files, "  pages?: string;", label=label)
    forbid_text(state, files, "parsePDFPageRange", label=label)
    forbid_text(state, pdf, "export function parsePDFPageRange(", label=label)
    require_text(
        state,
        files,
        "errorType: ToolErrorType.INVALID_TOOL_PARAMS,",
        count=1,
        label=label,
    )
    require_text(
        state,
        tool_test,
        "[pages-contract] advertises exactly one range mechanism and no pages parameter",
        label=label,
    )
    require_text(
        state,
        tool_test,
        "[pages-contract] refuses an undeclared pages argument on %s",
        label=label,
    )
    require_text(
        state,
        tool_test,
        "[pages-contract] reads a PDF whole, with no page parameter to supply",
        label=label,
    )


# Nine of the ten native tools; notebook_edit already declared its schema
# closed upstream, so this transformation does not touch it and it is not in
# the validator's view of the tree. The [param-contract] case in
# qwen38-agent-service-contract.test.ts reads all ten from the built source
# and is the check that covers it.
_NATIVE_TOOL_SOURCES: tuple[str, ...] = (
    "packages/core/src/tools/agent/agent.ts",
    "packages/core/src/tools/edit.ts",
    "packages/core/src/tools/glob.ts",
    "packages/core/src/tools/ls.ts",
    "packages/core/src/tools/read-file.ts",
    "packages/core/src/tools/ripGrep.ts",
    "packages/core/src/tools/shell.ts",
    "packages/core/src/tools/todoWrite.ts",
    "packages/core/src/tools/write-file.ts",
)


# The keywords a JSON-Schema string constraint is spelled with. The served
# engine compiles a declared tool schema into the grammar it constrains
# generation with, and a length- or pattern-bounded string there cannot also
# carry the `<parameter=` / `</parameter>` exclusions that keep a malformed
# tool call ungenerable; it refuses such a parameter rather than dropping
# them. `pattern` and `format` are matched only where a string value follows,
# because `pattern` is also the name of a parameter two of these tools
# declare.
_STRING_CONSTRAINT = re.compile(
    r"\bminLength\s*:|\bmaxLength\s*:|\bpattern\s*:\s*['\"`]|\bformat\s*:\s*['\"`]"
)


def _require_no_string_constraint(state: State, path: str, *, label: str) -> None:
    match = _STRING_CONSTRAINT.search(_source(state, path, label=label))
    _require(
        match is None,
        f"{label}: {path} declares the JSON-Schema string constraint "
        f"{match.group(0)!r} the served engine refuses. Delete the keyword "
        f"from the declaration and enforce the bound in the tool's "
        f"validateToolParams, where the refusal can name the parameter and "
        f"the limit."
        if match
        else "",
    )


def _validate_param_contract_before(state: State) -> None:
    label = "closed tool-parameter schema precondition"
    tools = "packages/core/src/tools/tools.ts"
    read_file = "packages/core/src/tools/read-file.ts"
    files = "packages/core/src/utils/fileUtils.ts"
    glob = "packages/core/src/tools/glob.ts"
    shell = "packages/core/src/tools/shell.ts"
    # Upstream validates against the schema and nothing more, so a name the
    # schema does not declare is accepted and then dropped.
    forbid_text(state, tools, "undeclaredToolParamsError", label=label)
    require_text(
        state,
        tools,
        "    const errors = SchemaValidator.validate(\n"
        "      this.schema.parametersJsonSchema,\n"
        "      params,\n"
        "    );",
        label=label,
    )
    # Seven of the ten native tools leave their schema open.
    for path in (
        read_file,
        "packages/core/src/tools/edit.ts",
        "packages/core/src/tools/write-file.ts",
        glob,
        "packages/core/src/tools/ripGrep.ts",
        "packages/core/src/tools/ls.ts",
        shell,
    ):
        forbid_text(state, path, "additionalProperties: false", label=label)
    # `offset`/`limit` are read only by the text branch; every other branch
    # drops them without a word, and only `.ipynb` is refused, by guessing
    # from the extension before the file type is known.
    require_text(
        state,
        read_file,
        "offset and limit are not supported for Jupyter notebook (.ipynb) files.",
        label=label,
    )
    require_text(state, read_file, "Requires 'limit' to be set.", label=label)
    forbid_text(
        state,
        files,
        "if (fileType !== 'text' && (offset !== undefined || limit !== undefined)) {",
        label=label,
    )
    # Two live tool descriptions instruct the model to batch parallel calls.
    require_text(state, glob, "call multiple tools in a single response", label=label)
    require_text(state, shell, "run_shell_command tool calls in parallel.", label=label)
    # Upstream spends JSON-Schema string constraints the served grammar cannot
    # express: the agent tool bounds `todo_id` and `fork_profile` by length and
    # `fork_turns` by pattern, and todo_write bounds every todo id.
    agent = "packages/core/src/tools/agent/agent.ts"
    todo = "packages/core/src/tools/todoWrite.ts"
    require_text(state, agent, "          maxLength: 500,", label=label)
    require_text(state, agent, "              pattern: '^[1-9][0-9]*$',", label=label)
    require_text(state, agent, "          minLength: 2,", label=label)
    require_text(state, agent, "          maxLength: 50,", label=label)
    require_text(state, todo, "              minLength: 1,", label=label)
    require_text(
        state, todo, "              items: { type: 'string', maxLength: 500 },", label=label
    )


def _validate_param_contract_after(state: State) -> None:
    label = "closed tool-parameter schema result"
    tools = "packages/core/src/tools/tools.ts"
    read_file = "packages/core/src/tools/read-file.ts"
    files = "packages/core/src/utils/fileUtils.ts"
    glob = "packages/core/src/tools/glob.ts"
    shell = "packages/core/src/tools/shell.ts"
    contract_test = "packages/core/src/config/qwen38-agent-service-contract.test.ts"
    tool_test = "packages/core/src/tools/read-file.test.ts"
    files_test = "packages/core/src/utils/fileUtils.test.ts"

    # One shared rule, applied before schema validation so the refusal names
    # the offending key and the accepted set rather than Ajv's anonymous
    # "must NOT have additional properties".
    source = _require_all(
        state,
        tools,
        (
            "export function undeclaredToolParamsError(",
            "const undeclared = undeclaredToolParamsError(",
            "It accepts exactly: ",
            "Re-send the call using only those parameters.",
        ),
        label=label,
    )
    _require_ordered(
        source,
        (
            "const undeclared = undeclaredToolParamsError(",
            "const errors = SchemaValidator.validate(",
        ),
        label=label,
        location=tools,
    )

    # Every one of the ten declares its schema closed. The schema is the
    # surface that teaches, because it is re-rendered into every request.
    for path in _NATIVE_TOOL_SOURCES:
        _require(
            "additionalProperties: false" in _source(state, path, label=label),
            f"{label}: {path} must declare its parameter schema closed",
        )

    _require_all(
        state,
        tools,
        (
            "schema.additionalProperties !== false",
            "schema.patternProperties !== undefined",
        ),
        label=label,
    )
    _require_all(
        state,
        "packages/core/src/tools/tool-modification.ts",
        (
            "new WeakMap",
            "getToolModification",
            "cloneToolParams",
        ),
        label=label,
    )
    require_text(
        state,
        "packages/core/src/core/coreToolScheduler.ts",
        "cloneToolParams(args)",
        label=label,
    )
    for path in _NATIVE_TOOL_SOURCES:
        forbid_text(state, path, "runtimeOnlyParams", label=label)

    # No native tool declares a string constraint the served grammar cannot
    # express. The bound moves into the tool, which can refuse the call and
    # name the parameter and the limit; a grammar can only reshape what the
    # model was about to write, and say nothing. `fork_turns` loses the whole
    # `oneOf` rather than just the `pattern` it carried: with the pattern gone
    # both branches match "all", which `oneOf` rejects.
    for path in _NATIVE_TOOL_SOURCES:
        _require_no_string_constraint(state, path, label=label)
    require_text(
        state,
        "packages/core/src/tools/agent/agent.ts",
        "        fork_turns: {\n          type: 'string',\n",
        label=label,
    )

    # The two tools that replace validateToolParams wholesale never run Ajv,
    # so they have to apply the shared rule themselves or they stay the only
    # tools that drop an undeclared name.
    for path in (
        "packages/core/src/tools/agent/agent.ts",
        "packages/core/src/tools/todoWrite.ts",
    ):
        require_text(state, path, "undeclaredToolParamsError(", label=label)

    # One range rule for every non-text type, decided where the type is known.
    require_text(
        state,
        files,
        "if (fileType !== 'text' && (offsetSelectsRange || limit !== undefined)) {",
        label=label,
    )
    require_text(
        state,
        files,
        "a range of lines, and a line range applies to text ",
        label=label,
    )
    forbid_text(
        state,
        read_file,
        "offset and limit are not supported for Jupyter notebook (.ipynb) files.",
        label=label,
    )
    # The schema no longer requires a `limit` the code never required.
    forbid_text(state, read_file, "Requires 'limit' to be set.", label=label)
    require_text(state, read_file, "Optional: text files only.", count=1, label=label)

    # A truncated read carries the call that continues it, with the caller's
    # own path and the exact resume line, rather than naming two parameters
    # and leaving the arithmetic to the reader. The sentence around it is the
    # shared one in tools.ts; what this tool owes is the values.
    require_text(
        state,
        read_file,
        "continuation: `read_file with file_path: ${JSON.stringify(",
        label=label,
    )
    require_text(
        state, files, "nextRead?: { offset: number; limit: number };", label=label
    )

    # No tool description advertises a dispatch this build does not have.
    for path in (glob, shell):
        require_text(
            state,
            path,
            "Tool calls run one at a time; there is no parallel dispatch.",
            label=label,
        )
    forbid_text(state, glob, "call multiple tools in a single response", label=label)
    forbid_text(state, shell, "run_shell_command tool calls in parallel.", label=label)

    # Executable evidence, in the suites the image runs.
    require_text(
        state,
        contract_test,
        "[param-contract] %s declares its parameter schema closed",
        label=label,
    )
    require_text(
        state,
        contract_test,
        "[param-contract] %s bounds its strings in code, not in the grammar",
        label=label,
    )
    require_text(
        state,
        contract_test,
        "[param-contract] the shared closed-schema refusal precedes validation while preserving external schema rules",
        label=label,
    )
    require_text(
        state,
        contract_test,
        "[param-contract] %s does not advertise a parallel dispatch this build does not have",
        label=label,
    )
    require_text(
        state,
        tool_test,
        "[param-contract] refuses %s instead of dropping it",
        label=label,
    )
    require_text(
        state,
        tool_test,
        "[param-contract] a truncated read carries the exact call that continues it",
        label=label,
    )
    require_text(
        state,
        files_test,
        "[param-contract] refuses a line range on %s instead of ignoring it",
        label=label,
    )


def _validate_required_read_offset_before(state: State) -> None:
    label = "required read offset precondition"
    tool = "packages/core/src/tools/read-file.ts"
    # Upstream advertises `offset` as optional, so a serialization that drops
    # it is schema-legal and executes as a silent restart from line 1.
    require_text(state, tool, "  offset?: number;", label=label)
    require_text(state, tool, "        required: ['file_path'],", label=label)
    require_text(
        state,
        tool,
        "Optional: For text files, the 0-based line number to start reading",
        label=label,
    )


def _validate_required_read_offset_after(state: State) -> None:
    label = "required read offset result"
    tool = "packages/core/src/tools/read-file.ts"
    files = "packages/core/src/utils/fileUtils.ts"
    tool_test = "packages/core/src/tools/read-file.test.ts"
    contract_test = "packages/core/src/config/qwen38-agent-service-contract.test.ts"

    # The schema is the enforcement surface: with `offset` in `required`, the
    # armed decode-time grammar refuses the omission in-span, and Ajv refuses
    # it for any call that reaches the harness another way. 0 is the explicit
    # spelling of "from the beginning".
    require_text(state, tool, "        required: ['file_path', 'offset'],", label=label)
    require_text(state, tool, "  offset: number;", label=label)
    forbid_text(state, tool, "offset?: number;", label=label)
    require_text(state, tool, "pass 0 to start at the beginning", label=label)
    require_text(
        state,
        tool,
        "every read states where it starts: 'offset' is required",
        label=label,
    )
    # A whole-file read is offset 0 with no limit: the cache fast path and
    # the non-text range refusal both key on that spelling, so an explicit 0
    # stays exactly as acceptable as the absence used to be.
    require_text(
        state,
        tool,
        "const isFullRead =\n      this.params.offset === 0 && this.params.limit === undefined;",
        label=label,
    )
    require_text(
        state,
        files,
        "const offsetSelectsRange = offset !== undefined && offset !== 0;",
        label=label,
    )
    require_text(
        state, files, "Re-send the call with offset: 0 and no limit.", label=label
    )
    forbid_text(state, files, "Re-send the call with only 'file_path'.", label=label)
    # Executable evidence, in the suites the image runs.
    require_text(
        state,
        tool_test,
        "[param-contract] requires offset so an omitted range cannot execute as line 1",
        label=label,
    )
    require_text(
        state,
        tool_test,
        "[param-contract] offset 0 is the whole-file read and negatives stay refused",
        label=label,
    )
    require_text(
        state,
        contract_test,
        "[param-contract] read_file requires offset so an omitted range cannot execute as line 1",
        label=label,
    )


def _validate_text_read_fidelity_before(state: State) -> None:
    label = "text read fidelity precondition"
    files = "packages/core/src/utils/fileUtils.ts"
    sync = "packages/core/src/utils/sync-file-encoding.ts"
    ranges = "packages/core/src/utils/read-text-range.ts"

    # Upstream infers an encoding from the shape of the bytes and, failing
    # that, decodes as UTF-8 with replacement characters, so a file that is
    # not what it is read as comes back as plausible text.
    require_text(
        state, files, "  const detected = detectEncodingFromBuffer(full);", label=label
    )
    require_text(
        state, files, "        content: iconvLite.decode(full, detected),", label=label
    )
    require_text(
        state,
        files,
        "  // Final fallback: UTF-8 with replacement characters",
        label=label,
    )
    require_text(
        state, sync, "          content: iconvDecode(full, detected),", label=label
    )
    # The BOM decoders are lossy in the same way: an ill-formed code unit
    # becomes U+FFFD rather than an error.
    require_text(state, files, "      out += '\\uFFFD';", label=label)
    # A sampled guess and an exact decode answer different questions, and only
    # the second one decides what is returned.
    require_text(
        state, files, "    const sampleSize = Math.min(8192, stats.size);", label=label
    )
    # The refusal the streamed reader already has is named for the size of the
    # file rather than for the property that fails.
    require_text(
        state, ranges, "export class LargeNonUtf8TextError extends Error {", label=label
    )
    require_text(state, ranges, "    readonly reason?: 'invalid-utf8',", label=label)
    # A page drops trailing whitespace from every line it returns, and ends a
    # line short with a marker of its own rather than at a line boundary.
    require_text(
        state,
        files,
        "        const selectedLines = content.split('\\n').map((line) => line.trimEnd());",
        label=label,
    )
    require_text(
        state,
        files,
        "                line.substring(0, remaining) + '... [truncated]',",
        label=label,
    )


def _validate_text_read_fidelity_after(state: State) -> None:
    label = "text read fidelity result"
    files = "packages/core/src/utils/fileUtils.ts"
    sync = "packages/core/src/utils/sync-file-encoding.ts"
    ranges = "packages/core/src/utils/read-text-range.ts"
    files_test = "packages/core/src/utils/fileUtils.test.ts"
    tool_test = "packages/core/src/tools/read-file.test.ts"

    # One decode, and it is the encoding the file declares. `fatal: true` is
    # the whole rule, so a sequence that encoding does not define raises rather
    # than resolving to a character the file does not contain.
    _require_all(
        state,
        files,
        (
            "export function decodeBufferWithEncodingInfo(full: Buffer): FileReadResult {",
            "function decodeUnicode(",
            "new TextDecoder(encoding, { fatal: true, ignoreBOM: true })",
            "throw new NonUtf8TextError(declared, 'undecodable');",
        ),
        label=label,
    )
    # Nothing in the read path infers an encoding, and nothing substitutes a
    # replacement character for one it could not read.
    for path in (files, sync):
        for absent in (
            "detectEncodingFromBuffer",
            "isUtf8CompatibleEncoding",
            "\\uFFFD",
        ):
            forbid_text(state, path, absent, label=label)
    forbid_text(state, files, "loadIconvLite", label=label)
    forbid_text(state, sync, "iconvDecode(full", label=label)
    # There is one buffer decoder, and it is synchronous because nothing it
    # does is asynchronous.
    forbid_text(state, files, "decodeBufferWithEncodingInfoAsync", label=label)
    forbid_text(state, sync, "decodeBufferWithEncodingInfo", label=label)
    # The declaration is read from the BOM window and settles nothing else.
    _require_all(
        state,
        files,
        (
            "const MAX_BOM_BYTES = 4;",
            "    const windowSize = Math.min(MAX_BOM_BYTES, stats.size);",
            "    return bom ? bomEncodingToName(bom.encoding) : 'utf-8';",
        ),
        label=label,
    )
    # One refusal, named for the property that fails rather than for a file
    # size, carrying the conversion the caller's next move needs.
    _require_all(
        state,
        ranges,
        (
            "export class NonUtf8TextError extends Error {",
            "    readonly reason: 'undecodable' | 'not-streamable',",
            "iconv -f SOURCE_ENCODING -t UTF-8",
        ),
        label=label,
    )
    forbid_text(state, ranges, "LargeNonUtf8TextError", label=label)

    # A page is a contiguous slice of the file: its lines as the file spells
    # them, in the NFC form the tokenizer reads, and the newline that
    # terminates the last one whenever the file continues past it, so the
    # pages a caller is told to read concatenate to the file in that form.
    _require_all(
        state,
        files,
        (
            "        const rangeReachedEof =",
            "        const linesIncluded = pageLines.length;",
            "            index === pageLines.length - 1 && rangeReachedEof",
            "              : `${line}\\n`,",
            "                  offset: actualEndLine,",
        ),
        label=label,
    )
    forbid_text(state, files, "trimEnd()", label=label)
    forbid_text(state, files, "... [truncated]", label=label)
    # A line the result cannot carry whole is refused, with the two moves that
    # get past it, rather than halved into a fragment that reads like the line.
    _require_all(
        state,
        files,
        (
            "          const overlongLine =",
            "            `no read starting there can return it. Read it in slices with a ` +",
            "            errorType: ToolErrorType.FILE_TOO_LARGE,",
        ),
        label=label,
    )

    # Executable evidence, in the suites the image runs.
    require_text(
        state,
        files_test,
        "[encoding-contract] refuses a multi-byte sequence the file cuts short",
        label=label,
    )
    require_text(
        state,
        files_test,
        "[read-contract] refuses a line wider than one page instead of halving it",
        label=label,
    )
    require_text(
        state,
        tool_test,
        "[read-contract] pages a file back exactly, following the call each page carries",
        label=label,
    )
    require_text(
        state,
        tool_test,
        "[encoding-contract] refuses bytes that are not valid UTF-8 instead of decoding them as something else",
        label=label,
    )


def _validate_literal_response_before(state: State) -> None:
    label = "literal response fidelity precondition"
    chat = "packages/core/src/core/geminiChat.ts"
    # Upstream commits the streamed text parts to history verbatim; nothing
    # between the stream and the push inspects prose for call syntax.
    require_text(
        state, chat, "const consolidatedHistoryParts: Part[] = [];", label=label
    )
    forbid_text(state, chat, "stripTrailingToolCallResidue", label=label)


# The write-argument displacement, retired. A completed write_file's `content`
# argument was replaced in history with a pointer to the file, through one
# history-rewrite primitive; the model copied the pointer into later writes.
_RETIRED_ARGUMENT_DISPLACEMENT = (
    "redactCompletedCallArgument",
    "writtenContentRedactionText",
    "Content written to",
    "written-content displacement",
)


def _validate_literal_response_after(state: State) -> None:
    label = "literal response fidelity result"
    chat = "packages/core/src/core/geminiChat.ts"
    for path in (
        chat,
        "packages/core/src/core/openaiContentGenerator/converter.ts",
        "packages/core/src/core/openaiContentGenerator/pipeline.ts",
    ):
        for symbol in (
            "stripTrailingToolCallResidue",
            "LeadingProtocolTagLeakDetector",
            "protocolTagSanitized",
        ):
            forbid_text(state, path, symbol, label=label)
    for path in (
        "packages/core/src/utils/toolCallResidue.ts",
        "packages/core/src/utils/toolCallResidue.test.ts",
        "packages/core/src/core/toolCallRecovery.ts",
    ):
        _require(
            path not in state,
            f"{label}: deleted text-repair mechanism remains at {path}",
        )
    _require_all(
        state,
        chat,
        (
            "const consolidatedHistoryParts: Part[] = [];",
            "await this.chatRecordingService.recordAssistantTurn({",
            "message: message",
            "...consolidatedHistoryParts",
        ),
        label=label,
    )
    _require_all(
        state,
        "packages/core/src/core/geminiChat.test.ts",
        (
            "literal response preservation",
            "streams and commits the exact literal %j once",
            "literal-with-structure",
        ),
        label=label,
    )

    # This patch rewrites nothing the model wrote. The model learns in context
    # from its own history, so a past call whose argument was swapped for a
    # pointer is a demonstration of calling the tool with that pointer, and no
    # wording of the pointer changes that. Swapping a completed write's
    # `content` for "[Content written to ...]" was exactly such a
    # demonstration: in the probe that ran with it, 20 of 26 note writes sent
    # the placeholder itself as the file's content, and the tool wrote it. The
    # mechanism is retired by name in every file the patch touches, so it
    # cannot come back under another caller. Upstream's approved-plan redaction
    # is upstream's own and is left exactly as upstream has it.
    for path in state:
        for retired in _RETIRED_ARGUMENT_DISPLACEMENT:
            forbid_text(state, path, retired, label=label)


def _validate_no_repair_validation_before(state: State) -> None:
    label = "schema validation repair precondition"
    validator = "packages/core/src/utils/schemaValidator.ts"
    # Four coercion passes rewrite the caller's arguments and re-validate, so
    # a call that violated the schema still executes.
    _require_all(
        state,
        validator,
        (
            "// --- Four-pass coercion ---",
            "fixBooleanValues(",
            "fixStringValues(",
            "fixStringifiedJsonValues(",
            "fixNumericValues(",
            "valid = validate(data);",
        ),
        label=label,
    )
    # And a schema that will not compile disables validation entirely, with
    # the only notice going to a debug logger this deployment never enables.
    require_text(
        state,
        validator,
        "      // Skip validation rather than blocking tool usage.",
        label=label,
    )
    require_text(state, validator, "'Skipping parameter validation.',", label=label)


def _validate_no_repair_validation_after(state: State) -> None:
    label = "schema validation repair result"
    validator = "packages/core/src/utils/schemaValidator.ts"
    validator_test = "packages/core/src/utils/schemaValidator.test.ts"
    shell_test = "packages/core/src/tools/shell.test.ts"

    # The repair passes and every helper that existed only to serve them are
    # gone: a wrong call fails, and the message names the parameter.
    for needle in (
        "fixBooleanValues",
        "fixStringValues",
        "fixStringifiedJsonValues",
        "fixNumericValues",
        "getAcceptedTypes",
        "getEffectiveProperties",
        "typeIsAccepted",
        "resolveRef",
        "Four-pass coercion",
    ):
        forbid_text(state, validator, needle, label=label)
    # A schema that cannot be compiled cannot be checked, and an unchecked
    # call is not a safe call. The guard no longer switches itself off.
    forbid_text(state, validator, "Skipping parameter validation.", label=label)
    forbid_text(state, validator, "debugLogger", label=label)
    require_text(
        state,
        validator,
        "Parameters cannot be validated, so the call is refused.",
        label=label,
    )
    require_text(
        state,
        validator_test,
        "[param-contract] reports violations without repairing them",
        label=label,
    )
    require_text(
        state,
        validator_test,
        "[param-contract] refuses a schema version it cannot compile",
        label=label,
    )
    require_text(
        state,
        shell_test,
        "[param-contract] is_background is not coerced",
        label=label,
    )


# How a failed call's model-facing error field is built, tolerant of whichever
# line shape prettier picks for it.
_MODEL_FACING_ERROR_FIELD = re.compile(
    r"response:\s*\{\s*(?:\.\.\.[A-Za-z_$][\w$]*,\s*)?error"
)
# Every module in this transformation's own file set that builds that field.
_MODEL_FACING_ERROR_PRODUCERS = (
    "packages/core/src/agents/runtime/agent-core.ts",
    "packages/core/src/core/coreToolScheduler.ts",
    "packages/core/src/core/geminiChat.ts",
    "packages/core/src/core/turn.ts",
    "packages/core/src/followup/speculation.ts",
    "packages/core/src/services/chatCompressionService.ts",
)


def _validate_model_facing_failure_before(state: State) -> None:
    label = "model-facing failure text precondition"
    scheduler = "packages/core/src/core/coreToolScheduler.ts"
    # The failure path forwards only the operational summary; the half named
    # for the model is read for images and otherwise discarded.
    require_text(
        state,
        scheduler,
        "        const operationalErrorMessage = toolResult.error.message;\n"
        "        let errorMessage = operationalErrorMessage;",
        label=label,
    )
    forbid_text(state, scheduler, "mergeModelFacingFailureText", label=label)
    # Two tools carry in-source warnings that exist only because of it.
    require_text(
        state,
        "packages/core/src/tools/shell.ts",
        "model-facing functionResponse from `error.message`, NOT from",
        label=label,
    )
    # Nothing holds the model's copy of a failed call to any size: the
    # response forwards whatever string it was handed, however long.
    forbid_text(state, "packages/core/src/tools/tools.ts", "boundedFailureText", label=label)
    forbid_text(state, scheduler, "boundedFailureText", label=label)
    # A stat failure that may be permanent is answered with a transient
    # failure's remedy. Three tools reach the shared branch -- edit,
    # write_file and shell's sed path -- and the notebook tool carries its
    # own copy of the same mistake.
    require_text(
        state,
        "packages/core/src/tools/priorReadEnforcement.ts",
        "then retry ${verb} it.",
        label=label,
    )
    require_text(
        state,
        "packages/core/src/tools/notebook-edit.ts",
        "Re-read it with the ${ToolNames.READ_FILE} tool before editing it.",
        count=3,
        label=label,
    )


def _validate_model_facing_failure_after(state: State) -> None:
    label = "model-facing failure text result"
    scheduler = "packages/core/src/core/coreToolScheduler.ts"
    scheduler_test = "packages/core/src/core/coreToolScheduler.test.ts"

    source = _require_all(
        state,
        scheduler,
        (
            "export function mergeModelFacingFailureText(",
            "        const modelFacingText = mergeModelFacingFailureText(\n"
            "          toolResult.llmContent,\n"
            "          errorMessage,\n"
            "        );",
            # The merged text reaches the model and nothing else. Folding it
            # into `errorMessage` instead leaked tool output into the
            # scrollback, the PostToolUseFailure hook and the sanitized
            # telemetry span -- three readers that want the operational
            # summary, not the model's copy.
            "  modelFacingText?: string,",
            "  const modelText = modelFacingText ?? error.message;",
            "        response: { error: modelText },",
            "    resultDisplay: resultDisplay ?? error.message,",
        ),
        label=label,
    )
    # One seam, before everything that reads the message, so the guarantee is
    # general rather than a habit each tool has to remember.
    _require_ordered(
        source,
        (
            "const modelFacingText = mergeModelFacingFailureText(",
            "const error = new Error(errorMessage);",
            "          modelFacingText,",
        ),
        label=label,
        location=scheduler,
    )
    # The operational message keeps its own identity at the seam.
    require_text(
        state,
        scheduler,
        "        const operationalErrorMessage = toolResult.error.message;\n"
        "        let errorMessage = operationalErrorMessage;",
        label=label,
    )
    # The per-tool warnings that documented the discard are retired with it.
    forbid_text(
        state,
        "packages/core/src/tools/shell.ts",
        "model-facing functionResponse from `error.message`, NOT from",
        label=label,
    )
    forbid_text(
        state,
        "packages/core/src/tools/agent/agent.ts",
        "(`llmContent` is discarded there)",
        label=label,
    )
    require_text(
        state,
        scheduler_test,
        "[error-envelope] mergeModelFacingFailureText",
        label=label,
    )

    # ── The tool's words, then the summary; the one bound holds the copy ─
    #
    # A failure message is the one place a tool writes model-supplied input
    # back out -- the path it could not open, the pattern it could not
    # compile, the tool name it did not recognise -- and an argument that
    # arrived merged makes that copy as large as the argument itself. It is
    # held to one inline block by the same step that holds every tool
    # result, where the model's copy is made (see tool-result-bound), and
    # that step keeps the start and the end. So the operational summary, the
    # short half that says what failed, follows the tool's own words, and the
    # end a bound keeps holds it.
    #
    # `error.message` is deliberately left whole. The scrollback, the
    # PostToolUseFailure hook and the sanitized telemetry span read it and
    # want the operational summary in full.
    merge = source.split("export function mergeModelFacingFailureText(", 1)[1].split(
        "\n}\n", 1
    )[0]
    _require(
        "return `${modelFacing}\\n${operational}`;" in merge
        and "return `${operational}\\n${modelFacing}`;" not in merge,
        f"{label}: the operational summary no longer follows the model-facing "
        f"half of a merged failure, where the end a bound keeps holds it.",
    )
    # No failure is bounded by a rule of its own any more; a second rule for
    # failures was one more place for a result to be cut differently.
    for path in (
        "packages/core/src/tools/tools.ts",
        scheduler,
        "packages/core/src/agents/runtime/agent-core.ts",
        "packages/core/src/followup/speculation.ts",
        "packages/cli/src/acp-integration/session/Session.ts",
    ):
        forbid_text(state, path, "boundedFailureText", label=label)

    # A seventh producer would be a seventh place a failure could reach the
    # model without its copy being finished. Every module in this
    # transformation's file set that builds the field is named, so a new one
    # has to join the list deliberately rather than appear quietly beside
    # them. The set is not the whole tree:
    # packages/core/src/core/turn-interruption.ts builds the same field and is
    # not part of this transformation, so no state-based check can see it; the
    # value it builds is a cancellation reason this process wrote, not text
    # the model chose.
    producers = sorted(
        path
        for path, text in state.items()
        if path.startswith("packages/core/src/")
        and path.endswith(".ts")
        and not path.endswith(".test.ts")
        and _MODEL_FACING_ERROR_FIELD.search(text)
    )
    _require(
        producers == list(_MODEL_FACING_ERROR_PRODUCERS),
        f"{label}: the modules building a failed call's model-facing error "
        f"field changed to {producers}. A new one must either finish its copy "
        f"with boundToolResponseParts or carry only text this process wrote.",
    )

    # ── An error invites nothing it cannot deliver ──────────────────────
    #
    # A stat failure may be permanent: ENAMETOOLONG, ENOTDIR and ELOOP are
    # properties of the path itself and EACCES a property of its permissions.
    # `read_file` performs the same stat on the same path, so "re-read, then
    # retry" asks for an identical call with an identical outcome -- the
    # read/edit retry loop TARGET_NOT_REGULAR_FILE already exists to avoid,
    # and the shape this deployment's own history records as having ended a
    # run. Both stat-failure remedies now name the one route that does not
    # repeat the syscall that just failed.
    #
    # No branch distinguishes a transient code from a permanent one, because
    # re-reading is not the remedy for either and a second wording would be a
    # second way to be wrong.
    prior_read = "packages/core/src/tools/priorReadEnforcement.ts"
    notebook = "packages/core/src/tools/notebook-edit.ts"
    forbid_text(state, prior_read, "then retry", label=label)
    require_text(
        state, prior_read, "stat failed before ${verbDisplay}.", label=label
    )
    require_text(
        state, notebook, "stat failed before editing this notebook.", label=label
    )
    # The other two re-read remedies stay: a notebook that disappeared or
    # changed since it was read is a stale-state failure, and re-reading is
    # exactly the remedy for that. Retiring those with the wrong one would
    # take away a remedy that works.
    require_text(
        state,
        notebook,
        "Re-read it with the ${ToolNames.READ_FILE} tool before editing it.",
        count=2,
        label=label,
    )
    for path in (
        "packages/core/src/tools/edit.test.ts",
        "packages/core/src/tools/write-file.test.ts",
    ):
        require_text(
            state,
            path,
            "expect(result.error?.message).not.toMatch(/retry/i);",
            label=label,
        )
    require_text(
        state,
        "packages/core/src/tools/notebook-edit.test.ts",
        "expect(String(result?.llmContent)).not.toMatch(/retry/i);",
        label=label,
    )

    # Executed in the build.
    for path, case in (
        (
            "packages/core/src/core/toolResultBound.test.ts",
            "bounds a failure by the same rule, keeping it a failure",
        ),
        (
            "packages/core/src/core/coreToolScheduler.test.ts",
            "keeps an operational reason that exists only in error.message, after the output",
        ),
    ):
        require_text(state, path, case, label=label)


def _validate_pdf_text_only_before(state: State) -> None:
    label = "PDF text-only precondition"
    files = "packages/core/src/utils/fileUtils.ts"
    pdf = "packages/core/src/utils/pdf.ts"
    bridge = "packages/core/src/services/visionBridge/vision-bridge-service.ts"
    # A page renderer exists and three separate paths fall back to it.
    require_text(
        state, pdf, "export async function renderPDFPagesToImages(", label=label
    )
    require_text(state, pdf, "export async function isPdftoppmAvailable(", label=label)
    _require(
        _source(state, files, label=label).count("renderPDFPagesToImages") == 3,
        f"{label}: {files} must import and call the page renderer",
    )
    require_text(
        state,
        bridge,
        "export interface VisionBridgePdfSourceContext {",
        label=label,
    )


def _validate_pdf_text_only_after(state: State) -> None:
    label = "PDF text-only result"
    files = "packages/core/src/utils/fileUtils.ts"
    pdf = "packages/core/src/utils/pdf.ts"
    read_file = "packages/core/src/tools/read-file.ts"
    bridge = "packages/core/src/services/visionBridge/vision-bridge-service.ts"

    # Neutralising the renderer behind a `false` constant left forty lines of
    # unreachable fallback that the compiler cannot flag. It is deleted.
    for path, needles in (
        (
            pdf,
            (
                "renderPDFPagesToImages",
                "isPdftoppmAvailable",
                "resetPdftoppmCache",
                "PDFRenderedImage",
                "PDF_RENDER_",
                "PDF_MAX_PAGES_PER_READ",
            ),
        ),
        (
            files,
            (
                "renderPDFPagesToImages",
                "willRenderPdfImages",
                "renderForBridge",
                "preparePdfForVisionBridge",
                "PDFVisionBridgeCandidate",
                "pdfVisionBridgeCandidate",
                "VISION_BRIDGE_MAX_IMAGES",
            ),
        ),
        (
            read_file,
            (
                "transcribePdfCandidate",
                "restorePdfFallback",
                "pdfVisionBridgeNotice",
                "runVisionBridge",
            ),
        ),
        (bridge, ("VisionBridgePdfSourceContext", "inferPdfSourceContext")),
    ):
        for needle in needles:
            forbid_text(state, path, needle, label=label)

    # The boundary is stated where the renderer used to be, so the next reader
    # is told why there is no fallback rather than inferring it from absence.
    require_text(state, pdf, "// PDF support ends at text extraction.", label=label)
    # The image bridge itself survives: it serves ordinary images, and only
    # its PDF-specific half is removed.
    require_text(state, bridge, "export async function runVisionBridge(", label=label)
    _require(
        _source(state, files, label=label).count("preserveUnsupportedImage") == 3,
        f"{label}: {files} must keep the image bridge's option intact",
    )


def _validate_turn_count_owner_before(state: State) -> None:
    label = "subagent turn-count ownership precondition"
    core = "packages/core/src/agents/runtime/agent-core.ts"
    headless = "packages/core/src/agents/runtime/agent-headless.ts"
    # Upstream keeps the count in the reasoning loop's activation record and
    # nowhere else: no field on the core, and no accessor for a caller that
    # needs it after the loop has unwound.
    require_text(state, core, "    let turnCounter = 0;", label=label)
    forbid_text(state, core, "reasoningTurnsUsed", label=label)
    forbid_text(state, headless, "getReasoningTurnsUsed", label=label)


def _validate_turn_count_owner_after(state: State) -> None:
    label = "subagent turn-count ownership result"
    core = "packages/core/src/agents/runtime/agent-core.ts"
    headless = "packages/core/src/agents/runtime/agent-headless.ts"
    headless_test = "packages/core/src/agents/runtime/agent-headless.test.ts"

    # The counter belongs to the object that outlives the throw, and there is
    # exactly one of it: the mirror that could disagree is gone.
    require_text(state, core, "  private reasoningTurnsUsed = 0;", label=label)
    require_text(state, core, "  getReasoningTurnsUsed(): number {", label=label)
    require_text(state, core, "  resetReasoningTurns(): void {", label=label)
    forbid_text(state, core, "turnCounter", label=label)
    forbid_text(state, headless, "private turnsUsed", label=label)
    forbid_text(state, headless, "this.turnsUsed", label=label)
    require_text(
        state,
        headless,
        "    return this.core.getReasoningTurnsUsed();",
        label=label,
    )
    # Reset where a logical turn begins, not inside the loop: a throw from
    # chat creation or tool preparation never reaches the loop.
    require_text(state, headless, "    this.core.resetReasoningTurns();", label=label)
    require_text(
        state,
        headless_test,
        "[subagent-scope] reports the turns a thrown run had started",
        label=label,
    )


def _validate_subagent_progress_before(state: State) -> None:
    label = "subagent terminal-progress precondition"
    events = "packages/core/src/agents/runtime/agent-events.ts"
    headless = "packages/core/src/agents/runtime/agent-headless.ts"
    core = "packages/core/src/agents/runtime/agent-core.ts"
    agent_tool = "packages/core/src/tools/agent/agent.ts"
    cli = "packages/cli/src/nonInteractiveCli.ts"
    detector = "packages/core/src/services/loopDetectionService.ts"
    subagent_result = "packages/core/src/agents/subagent-result.ts"
    # The terminal event announces the status without the count that decides
    # what to do about it, and without which of nine rules stopped the run.
    require_text(
        state,
        events,
        "export interface AgentFinishEvent {\n  subagentId: string;\n"
        "  terminateReason: string;\n  timestamp: number;",
        label=label,
    )
    forbid_text(state, events, "loopType", label=label)
    forbid_text(state, headless, "getLoopType", label=label)
    forbid_text(state, core, "loopType", label=label)
    forbid_text(state, agent_tool, "describeSubagentTerminateReason", label=label)
    forbid_text(state, subagent_result, "describeLoopType", label=label)
    # The rule labels exist only inside the CLI, out of reach of the core
    # subagent path, which is why only the main session ever names a rule.
    require_text(
        state, cli, "const LOOP_TYPE_LABELS: Record<LoopType, string> = {", label=label
    )
    forbid_text(state, detector, "LOOP_TYPE_LABELS", label=label)


def _validate_subagent_progress_after(state: State) -> None:
    label = "subagent terminal-progress result"
    events = "packages/core/src/agents/runtime/agent-events.ts"
    headless = "packages/core/src/agents/runtime/agent-headless.ts"
    core = "packages/core/src/agents/runtime/agent-core.ts"
    agent_tool = "packages/core/src/tools/agent/agent.ts"
    agent_tool_test = "packages/core/src/tools/agent/agent.test.ts"
    cli = "packages/cli/src/nonInteractiveCli.ts"
    detector = "packages/core/src/services/loopDetectionService.ts"
    index = "packages/core/src/index.ts"
    subagent_result = "packages/core/src/agents/subagent-result.ts"
    subagent_result_test = "packages/core/src/agents/subagent-result.test.ts"

    # The terminal event carries both terminal facts, and both are required
    # rather than optional, so no emit site can omit them.
    _require_all(
        state,
        events,
        (
            "  turnsUsed: number;",
            "  loopType: LoopType | null;",
        ),
        label=label,
    )
    require_text(
        state,
        headless,
        "          turnsUsed: this.core.getReasoningTurnsUsed(),\n"
        "          loopType: this.loopType,",
        label=label,
    )
    require_text(state, headless, "  getLoopType(): LoopType | null {", label=label)
    # The reasoning loop reports which rule fired, and only when one did.
    require_text(
        state,
        core,
        "      loopType:\n        terminateMode === AgentTerminateMode.LOOP_DETECTED\n"
        "          ? loopDetector.getLastLoopType()\n          : null,",
        label=label,
    )
    # The listener that wins the running -> failed transition publishes the
    # complete terminal snapshot; a later, richer update is never read.
    require_text(
        state,
        agent_tool,
        "          terminateReason: describeSubagentTerminateReason(\n"
        "            event.terminateReason as AgentTerminateMode,\n"
        "            event.loopType,\n            event.finalMessageSlip,\n          ),\n"
        "          turnsUsed: event.turnsUsed,",
        label=label,
    )
    # Both writers of the terminal display say the same thing the same way.
    require_text(
        state,
        agent_tool,
        "            terminateReason: describeSubagentTerminateReason(\n"
        "              terminateMode,\n              loopType,\n"
        "              finalMessageSlip,\n            ),",
        label=label,
    )
    require_text(
        state,
        agent_tool,
        "            turnsUsed,\n          },",
        count=2,
        label=label,
    )
    # Both model-visible constructions carry the count, the rule and the
    # shape of a slipped final message.
    require_text(
        state,
        agent_tool,
        "          subagent.getTurnsUsed(),\n          subagent.getLoopType(),\n"
        "          subagent.getFinalMessageSlip(),\n        ),",
        label=label,
    )
    require_text(
        state,
        agent_tool,
        "                subagent.getTurnsUsed(),\n                subagent.getLoopType(),\n"
        "                subagent.getFinalMessageSlip(),\n              ),",
        label=label,
    )
    # One vocabulary for the rules, beside the detector, used by the headless
    # session and the subagent path alike.
    _require_all(
        state,
        detector,
        (
            "export const LOOP_TYPE_LABELS: Record<LoopType, string> = {",
            "export function describeLoopType(",
        ),
        label=label,
    )
    require_text(state, index, "  describeLoopType,", label=label)
    forbid_text(state, cli, "const LOOP_TYPE_LABELS", label=label)
    require_text(
        state, cli, "const described = describeLoopType(loopType);", label=label
    )
    require_text(
        state,
        subagent_result,
        "export function describeSubagentTerminateReason(",
        label=label,
    )
    require_text(
        state,
        subagent_result,
        "    reason = `stopped as ${String(terminateMode).toLowerCase()}${detail}${turns}`;",
        label=label,
    )
    require_text(
        state,
        agent_tool_test,
        "[subagent-scope] publishes the turn count with the terminal display status",
        label=label,
    )
    require_text(
        state,
        agent_tool_test,
        "[loop-attribution] names the rule that halted a looping subagent",
        label=label,
    )
    require_text(
        state,
        subagent_result_test,
        "[loop-attribution] names the loop rule in the text the parent reads",
        label=label,
    )
    require_text(
        state,
        subagent_result_test,
        "[loop-attribution] distinguishes a budget halt from a repetition halt",
        label=label,
    )
    require_text(
        state,
        subagent_result_test,
        "[loop-attribution] leaves a non-loop terminate reason exactly as it was",
        label=label,
    )


def _validate_compaction_budget_before(state: State) -> None:
    label = "context-partition precondition"
    limits = "packages/core/src/core/tokenLimits.ts"
    service = "packages/core/src/services/chatCompressionService.ts"
    prompts = "packages/core/src/core/prompts.ts"
    # Upstream spends the window through a tuned ladder of independent
    # constants and writes the snapshot into nine overlapping sections.
    _require_all(
        state,
        service,
        (
            "export const COMPACT_MAX_OUTPUT_TOKENS = 20_000;",
            "export const AUTOCOMPACT_BUFFER = 13_000;",
            "export const WARN_BUFFER = 20_000;",
            "export const HARD_BUFFER = 3_000;",
            "export const DEFAULT_PCT = 0.85;",
            "export function computeThresholds(",
        ),
        label=label,
    )
    forbid_text(state, limits, "partitionContextWindow", label=label)
    # The trigger is a configured fraction of the window, tunable per session.
    require_text(
        state,
        "packages/core/src/config/config.ts",
        "  getAutoCompactThreshold(): number | undefined {",
        label=label,
    )
    require_text(state, prompts, "<all_user_messages>", label=label)


# The three quantities the partition declares. `D` and `M` are shares of the
# window; `F` is a byte count that belongs to the served chat template and
# does not scale with the window. They are read out of the post-patch source
# rather than restated here, so the arithmetic below is a check on the tree
# and not a copy of it.
_PARTITION_DECLARED_NAMES = (
    "WINDOW_SHARES",
    "STATIC_PREAMBLE_SHARES",
    "INLINE_BLOCK_SHARES",
    "MESSAGE_FRAMING_BYTES",
)

# The served deployment, and the partition its window must yield.
_SERVED_WINDOW = 262_144
_SERVED_PARTITION = {
    "staticPreamble": 12_288,
    "inlineBlockBytes": 32_768,
    "messageFraming": 61,
    "turnGeneration": 69_509,
    "compactionTrigger": 180_347,
}


def _read_partition_declarations(source: str, *, label: str) -> dict[str, int]:
    declared: dict[str, int] = {}
    for name in _PARTITION_DECLARED_NAMES:
        match = re.search(rf"^const {name} = (\d+);$", source, re.MULTILINE)
        _require(
            match is not None,
            f"{label}: tokenLimits.ts does not declare {name} as an integer",
        )
        declared[name] = int(match.group(1))
    return declared


def _surviving(window: int, declared: dict[str, int], turn: int) -> int:
    """What a compaction leaves standing if a turn were given `turn` tokens.

    The static preamble, then the snapshot, the authored input and one result
    -- each at most one framed inline block -- and the carried turn, framed
    like any other message.
    """
    total = declared["WINDOW_SHARES"]
    preamble = (window * declared["STATIC_PREAMBLE_SHARES"]) // total
    block = (window * declared["INLINE_BLOCK_SHARES"]) // total + declared[
        "MESSAGE_FRAMING_BYTES"
    ]
    return preamble + 3 * block + (turn + declared["MESSAGE_FRAMING_BYTES"])


def _fits(window: int, declared: dict[str, int], turn: int) -> bool:
    """Whether a turn of `turn` tokens leaves that standing below its trigger."""
    total = declared["WINDOW_SHARES"]
    preamble = (window * declared["STATIC_PREAMBLE_SHARES"]) // total
    return _surviving(window, declared, turn) <= window - turn - preamble - 1


def _partition(window: int, declared: dict[str, int]) -> dict[str, int]:
    """`C` by search, so this checks the definition rather than the formula.

    `C` is the largest room a turn may be given while a compaction can still
    leave its result standing below the trigger. The source solves that in
    closed form; here it is found by bisection from the property itself, and
    the two are then required to agree.
    """
    total = declared["WINDOW_SHARES"]
    preamble = (window * declared["STATIC_PREAMBLE_SHARES"]) // total
    low, high = 0, window
    if not _fits(window, declared, 1):
        turn = 0
    else:
        while low + 1 < high:
            mid = (low + high) // 2
            if _fits(window, declared, mid):
                low = mid
            else:
                high = mid
        turn = low
    return {
        "staticPreamble": preamble,
        "inlineBlockBytes": (window * declared["INLINE_BLOCK_SHARES"]) // total,
        "messageFraming": declared["MESSAGE_FRAMING_BYTES"],
        "turnGeneration": turn,
        "compactionTrigger": window - turn - preamble,
    }


def _validate_context_partition(state: State, *, label: str) -> None:
    """Every context budget follows from three declared quantities, and the
    two that are derived are the largest values the fit admits.

    The safety argument: what stands in the window after a compaction is the
    static preamble plus a snapshot, the authored input, one tool result and
    the turn carried behind them. Three of those four are bounded in bytes
    before they exist, and a text's tokens are at most the UTF-8 bytes of its
    NFC form -- the served tokenizer normalizes to NFC and then spends at
    least a byte per token -- so the whole of it is known before any of it is
    generated.
    `C` is the largest room a turn may be given while that still stands below
    the trigger, and `T` is what the window has left. This checks the
    definition rather than the formula: `C` is found here by bisection on the
    fit and required to be what the source computes, `C + 1` is required not
    to fit, and the two consequences -- that a turn issued at the largest
    admitted prompt cannot reach the end of the window, and that the request
    which later summarises that prompt is issued with at least `C + 1` -- are
    asserted at every window the deployment can be given.
    """

    limits = "packages/core/src/core/tokenLimits.ts"
    source = _source(state, limits, label=label)
    declared = _read_partition_declarations(source, label=label)
    _require(
        all(value > 0 for value in declared.values()),
        f"{label}: every declared quantity must be a positive integer",
    )
    _require(
        declared["STATIC_PREAMBLE_SHARES"] + declared["INLINE_BLOCK_SHARES"]
        < declared["WINDOW_SHARES"],
        f"{label}: the declared shares leave no window for a turn or a trigger",
    )
    # `T` is what the window has left once `C` and `D` are held back, so the
    # three terms spend the window exactly and no fourth budget exists.
    require_text(
        state,
        limits,
        "  const compactionTrigger = contextWindowSize - turnGeneration - staticPreamble;",
        label=label,
    )
    # Every window this partition can be asked to describe: the served one,
    # 524,288 as the next served tier, the tiers the model table can select,
    # and two off a share boundary so the rounding is covered.
    for window in (
        4_096,
        131_072,
        196_608,
        200_000,
        202_752,
        _SERVED_WINDOW,
        _SERVED_WINDOW + 1,
        272_000,
        524_288,
        1_000_000,
        1_048_576,
    ):
        part = _partition(window, declared)
        _require(
            all(value > 0 for value in part.values()),
            f"{label}: a {window}-token window yields a non-positive budget {part!r}",
        )
        _require(
            part["staticPreamble"] + part["turnGeneration"] + part["compactionTrigger"]
            == window,
            f"{label}: a {window}-token window is not spent exactly by {part!r}",
        )
        _require(
            _fits(window, declared, part["turnGeneration"])
            and not _fits(window, declared, part["turnGeneration"] + 1),
            f"{label}: at a {window}-token window {part['turnGeneration']} is not "
            f"the largest room a turn can be given while a compaction still "
            f"leaves {_surviving(window, declared, part['turnGeneration'])} "
            f"tokens standing below the trigger",
        )
        _require(
            part["compactionTrigger"] - 1 + part["turnGeneration"] < window,
            f"{label}: at a {window}-token window a turn issued at the largest "
            f"admitted prompt ({part['compactionTrigger'] - 1}) with its "
            f"{part['turnGeneration']}-token room reaches past the window",
        )
        _require(
            window - (part["compactionTrigger"] - 1)
            == part["turnGeneration"] + part["staticPreamble"] + 1,
            f"{label}: at a {window}-token window the request that summarises "
            f"the largest admitted prompt is not issued with a whole turn's "
            f"room plus the preamble",
        )
    # A window too small to leave a turn any room is refused rather than
    # partitioned into something unusable.
    for tiny in (256, 257, 459):
        _require(
            _partition(tiny, declared)["turnGeneration"] < 1,
            f"{label}: a {tiny}-token window must leave a turn no room, so the "
            f"partition refuses it instead of deriving one",
        )
    served = _partition(_SERVED_WINDOW, declared)
    _require(
        served == _SERVED_PARTITION,
        f"{label}: the served {_SERVED_WINDOW}-token window yields {served!r}, "
        f"not the reviewed {_SERVED_PARTITION!r}",
    )


def _validate_compaction_budget_after(state: State) -> None:
    authored_path = "packages/core/src/core/authored-instructions.ts"
    authored = _source(state, authored_path, label="complete authored retention")
    retained = authored.split("export function retainedInstructionParts(", 1)[1].split(
        "function collectInstructions(", 1
    )[0]
    _require_ordered(
        retained,
        (
            "const instructions = collectInstructions(history);",
            "for (const instruction of instructions)",
            "...attachInstructions(structuredClone(instruction.parts), [instruction])",
            "return parts;",
        ),
        label="complete authored retention",
        location=authored_path,
    )
    snapshot_path = "packages/core/src/services/state-snapshot.ts"
    snapshot = _source(state, snapshot_path, label="complete state snapshot")
    section_match = re.search(
        r"export const STATE_SNAPSHOT_SECTIONS = \[(.*?)\] as const;", snapshot, re.S
    )
    _require(section_match is not None, "state snapshot section declaration missing")
    _require(
        re.findall(r"'([^']+)'", section_match[1])
        == [
            "intent",
            "environment",
            "completed",
            "in_progress",
            "learnings",
            "next_step",
        ],
        "state snapshot requires exactly the ordered six sections",
    )
    # The sections are a declared closed schema, not a shape the model is
    # asked to type. Presence, uniqueness and ordering are properties of the
    # declaration; the client re-checks the answer against the question it
    # asked, because it cannot observe whether the engine applied it.
    _require_all(
        state,
        snapshot_path,
        (
            "required: [...STATE_SNAPSHOT_SECTIONS],",
            "additionalProperties: false,",
            # The bound travels with the declaration, so the model is told what
            # one inline block may carry before it composes a snapshot, and
            # acceptance -- not decoding -- is where a longer draw is refused
            # and redrawn whole.
            "export function stateSnapshotTool(maxBytes: number): Tool {",
            "`${maxBytes} bytes, including their headings; a longer snapshot is refused and redrawn.`,",
            "export function acceptStateSnapshot(\n  calls: readonly FunctionCall[],\n  maxBytes: number,\n): StateSnapshotAcceptance {",
            "const rendered = tokenizerText(renderStateSnapshot(snapshot));",
            "if (rendered.bytes > maxBytes) {",
            "  return tokenizerText(\n    [\n      '# state_snapshot',",
            "SchemaValidator.validate(STATE_SNAPSHOT_PARAMETERS, args)",
            "(section) => !String(record[section]).trim(),",
            # A draw is refused for one of two things, and says which: it
            # declared no complete snapshot, or it declared one past the bound,
            # which it keeps, rendered, because that is what the draw produced.
            "readonly refused: 'incomplete';",
            "readonly refused: 'over_bound';",
            "      refused: 'over_bound',",
            "      rendered: rendered.text,",
        ),
        label="complete state snapshot",
    )
    forbid_text(state, snapshot_path, "SaxesParser", label="complete state snapshot")
    label = "context-partition result"
    limits = "packages/core/src/core/tokenLimits.ts"
    limits_test = "packages/core/src/core/tokenLimits.test.ts"
    chat = "packages/core/src/core/geminiChat.ts"
    chat_test = "packages/core/src/core/geminiChat.test.ts"
    service = "packages/core/src/services/chatCompressionService.ts"
    service_test = "packages/core/src/services/chatCompressionService.test.ts"
    attachments = "packages/core/src/services/postCompactAttachments.ts"
    attachments_test = "packages/core/src/services/postCompactAttachments.test.ts"
    pipeline = "packages/core/src/core/openaiContentGenerator/pipeline.ts"
    generator = "packages/core/src/core/contentGenerator.ts"
    generator_test = "packages/core/src/core/contentGenerator.test.ts"
    prompts = "packages/core/src/core/prompts.ts"
    prompts_test = "packages/core/src/core/prompts.test.ts"

    # The arithmetic itself, evaluated against the shares the tree declares.
    _validate_context_partition(state, label=label)

    # One derivation, in one place, from three declared quantities. A turn's
    # limit is the room the fit leaves and nothing else: not the prompt, not a
    # model ceiling, not a clamp margin, not a floor. It is the same number a
    # compaction's snapshot is issued with, because what a compaction has to
    # carry is that quantity repeated.
    limits_source = _require_all(
        state,
        limits,
        (
            "export interface ContextPartition {",
            "export function partitionContextWindow(",
            "export function turnOutputLimit(",
            "    turnGeneration,\n    compactionTrigger,\n  };",
            "return partition.turnGeneration;",
            # The one place bytes stand in for tokens: a framed block, `M`
            # bytes of NFC plus the template's `F`, spent once in the fit.
            "  const block = inlineBlockBytes + messageFraming;",
            # A text's tokens are at most the UTF-8 bytes of its NFC form, not
            # of the text as written, and this is the one place either is
            # measured: every byte bound that stands in for tokens takes its
            # measure here and hands on the form it measured.
            "export function tokenizerText(text: string): TokenizerText {",
            "const normalized = text.normalize('NFC');",
            "export function cutToTokenizerBytes(",
            "const head = tokenizerText(encoded.subarray(0, end).toString('utf8'));",
            "if (head.bytes > maxBytes) {",
        ),
        label=label,
    )
    # A second conversion site, even one that only restates the first, is a
    # second place the units could be converted differently.
    _require(
        "inlineBlockTokenBound" not in limits_source,
        f"{label}: {limits} converts bytes to tokens outside the partition's fit",
    )
    for false_theorem in (
        "no byte sequence becomes more tokens than it has",
        "this tokenizer is byte-level, so no block",
    ):
        _require(
            false_theorem not in limits_source,
            f"{label}: {limits} still states that bytes as written bound tokens "
            f"('{false_theorem}'); NFC can make a text three times longer, so "
            f"only the bytes of its NFC form do",
        )
    measuring = sorted(
        path
        for path, text in state.items()
        if path.startswith("packages/")
        and path.endswith((".ts", ".tsx"))
        and ".test." not in path
        and ("normalize('NF" in text or 'normalize("NF' in text)
    )
    _require(
        measuring == [limits],
        f"{label}: text is normalized for measuring outside {limits}: "
        f"{', '.join(p for p in measuring if p != limits) or 'nowhere, not even there'}. "
        f"One helper measures, tokenizerText, and everything uses it.",
    )
    # The counterexample that refutes the old statement is kept as a test
    # wherever a bound is taken.
    for test_path, name in (
        (limits_test, "measures U+1D1C0 at the twelve bytes of its NFC form, not the four it is written in"),
        (limits_test, "bounds what U+1D1C0 normalizes to, not what it is written in"),
        ("packages/core/src/core/toolResultBound.test.ts", "bounds U+1D1C0 by the twelve bytes it normalizes to, not the four it is written in"),
        ("packages/core/src/core/toolResultBound.test.ts", "hands a result inside the bound on whole, in the NFC form it was measured in"),
        ("packages/core/src/tools/read-file.test.ts", "pages by the bytes a line normalizes to, not the bytes it is written in"),
        ("packages/core/src/services/state-snapshot.test.ts", "measures the rendered snapshot in the NFC bytes the tokenizer reads"),
    ):
        require_text(state, test_path, name, label=label)
    limit_body = limits_source.split("export function turnOutputLimit(", 1)[1].split(
        "\n}\n", 1
    )[0]
    _require(
        "Math.min(" not in limit_body and "eiling" not in limit_body,
        f"{label}: a turn's output limit is bounded by something other than the window",
    )
    for absent in (
        "TOOL_RESULT_SHARES",
        "readonly toolResult",
        "TURN_OUTPUT_SHARES",
        "SUMMARY_RESERVE_SHARES",
        "turnOutputBudget",
        "summaryReserve",
        "readonly turnOutput",
        "MIN_CLAMPED_OUTPUT_TOKENS",
        "outputClampMargin",
        "compactionRoom",
        "clampOutputTokensToWindow",
        "clampExactOutputTokensToWindow",
        # The reserve pair this partition replaced. A reserve was a magnitude
        # held back by choice; every term here is now derived from the fit or
        # declared with its count, so neither name may return under either
        # spelling.
        "GENERATION_RESERVE_SHARES",
        "DIRECTIVE_RESERVE_SHARES",
        "generationReserve",
        "directiveReserve",
        "GENERATION_RESERVE",
        "DIRECTIVE_RESERVE",
    ):
        _require(
            absent not in limits_source,
            f"{label}: {limits} still carries the retired budget term '{absent}'",
        )

    # A turn is issued below the compaction trigger or it is not issued, and
    # what it is issued with is the remainder. Nothing in the send path
    # consults a ceiling: not a configured one, not the model's, not an
    # environment variable.
    chat_source = _source(state, chat, label=label)
    _require_ordered(
        chat_source,
        (
            # `D` and `F` are declared rather than derived, so they are proved
            # against the served tokenizer before this chat issues anything --
            # through the turn's own counter, so the route that will serve the
            # turn is the route they are proved against. Each framing is the
            # request with one more message, less the request without it, less
            # that message's content counted alone.
            "private async verifyDeclarations(",
            "const declared = this.preambleDeclaration;",
            "count(contents, declared?.systemInstruction);",
            "const bare = await counted([]);",
            "const withStartup = startup ? await counted([startup.counted]) : bare;",
            "const repositoryData = declared?.repositoryDataBytes ?? 0;",
            "const turnBudget = declared?.turnBudgetBytes ?? 0;",
            "const workspaceData = startup?.workspaceDataBytes ?? 0;",
            "const preamble = withStartup + repositoryData + turnBudget + workspaceData;",
            "if (preamble > partition.staticPreamble) {",
            "`room for ${turnBudget} bytes of turn budget`",
            "const probe = await countText(FRAMING_PROBE_TEXT);",
            "const withMessage = await counted([message]);",
            "const withTurn = await counted([message, turn]);",
            "const withResult = await counted([message, call, result]);",
            "['user message', withMessage - bare - probe],",
            "['assistant turn', withTurn - withMessage - 2 * probe],",
            "['tool result, with the call it answers', withResult - withTurn - probe],",
            "if (framing > partition.messageFraming) {",
            "await this.verifyDeclarations(",
            "countExactTextTokens,",
            "promptTokensForClamp = await countExactRequestTokens(requestContents);",
            "if (promptTokensForClamp >= partition.compactionTrigger) {",
            "throw new Error(",
            "maxOutputTokens: turnOutputLimit(partition),",
        ),
        label=label,
        location=chat,
    )
    # Both refusals name a move the operator has. A preamble already written
    # into history cannot be shortened after the fact, which is why every
    # value the preamble renders is bounded before it is counted.
    _require_all(
        state,
        chat,
        (
            "Shorten the system prompt or a tool description, or deploy on a larger window.",
            "Raise the declared framing to match the template, or serve the template the partition was written for.",
        ),
        label=label,
    )
    # The framing proof once counted an empty message, which the converter
    # drops before the request is sent, so it measured nothing and could not
    # fail. The probe carries content, the text count takes it back out, and
    # the tests prove a template wider than `F` is refused on the wire.
    require_text(state, chat, "const FRAMING_PROBE_TEXT = 'framing';", label=label)
    # The main session's instruction carries repository data and states the
    # turn budget, so its chat is declared with that instruction's fixed text
    # -- the snapshot without its values, the budget's line without its
    # number -- and the byte bound of each; replacing the instruction clears
    # the declaration it no longer matches.
    _require_all(
        state,
        chat,
        (
            "export interface PreambleDeclaration {\n  readonly systemInstruction: string;\n  readonly repositoryDataBytes: number;\n  readonly turnBudgetBytes: number;\n}",
            "    this.preambleDeclaration = undefined;",
            "  declarePreamble(declaration: PreambleDeclaration): void {",
        ),
        label=label,
    )
    _require_all(
        state,
        "packages/core/src/core/client.ts",
        (
            "gitStatus: GIT_SNAPSHOT_WITHOUT_REPOSITORY_DATA,",
            "repositoryDataBytes: GIT_SNAPSHOT_REPOSITORY_BYTES,",
            "const base = baseStating(this.config.getMaxSessionTurns());",
            "const declaredBase = baseStating(QWEN38_TURN_BUDGET_LEFT_OUT);",
            "          base: declaredBase,\n          gitStatus: GIT_SNAPSHOT_WITHOUT_REPOSITORY_DATA,",
            "turnBudgetBytes: declaredBase === base ? 0 : QWEN38_TURN_BUDGET_BYTES,",
            "chat.declarePreamble(declaration);",
            "this.chat.declarePreamble(preamble.declaration);",
        ),
        label=label,
    )
    forbid_text(state, chat, "const framing = framed - preamble;", label=label)
    # The startup context opens every history, so it is part of the static
    # preamble: counted with its two runs of workspace data left out and those
    # runs bounded by their caps in the NFC bytes the tokenizer reads. Every
    # chat that opens with it declares it, the main session's and each
    # agent's, and a declaration is proved again before the next turn.
    environment = "packages/core/src/utils/environmentContext.ts"
    _require_all(
        state,
        environment,
        (
            "export const STARTUP_ENVIRONMENT_BYTES = 256;",
            "export const STARTUP_LISTING_BYTES = 1024;",
            "export const STARTUP_CONTEXT_WORKSPACE_BYTES =",
            "export const STARTUP_CONTEXT_WITHOUT_WORKSPACE_DATA = `${STARTUP_CONTEXT_HEAD}${STARTUP_CONTEXT_FOLDERS}${SYSTEM_REMINDER_CLOSE}`;",
            "return `${STARTUP_CONTEXT_HEAD}${environment}${STARTUP_CONTEXT_FOLDERS}${listing}${SYSTEM_REMINDER_CLOSE}`;",
            "if (lines.bytes > STARTUP_ENVIRONMENT_BYTES) {",
            "if (listing.bytes <= STARTUP_LISTING_BYTES) return listing.text;",
            "if (cut.bytes > STARTUP_LISTING_BYTES) {",
            "export function declareStartupContext(",
            "return { ...part, text: STARTUP_CONTEXT_WITHOUT_WORKSPACE_DATA };",
        ),
        label=label,
    )
    _require_all(
        state,
        chat,
        (
            "  declareStartupContext(\n    declaration: StartupContextDeclaration | undefined,\n  ): void {",
            "JSON.stringify(this.history[0]) !== JSON.stringify(declaration.content)",
            "  getStartupContext(): Content | undefined {",
            "    // A new declaration is a new preamble, proved before the next turn.\n    this.preambleDeclaration = declaration;",
        ),
        label=label,
    )
    # Tool declarations change after startup -- the agent tool re-declares
    # itself when its subagent list finishes loading, and discovered or
    # deferred tools are declared later -- so a proof is held to the preamble
    # it measured, as the counter rendered it, and a turn whose preamble
    # differs in any part is proved again before it is issued. A flag that a
    # proof once ran says nothing about the declarations a chat sends later.
    _require_ordered(
        chat_source,
        (
            "private provenPreamble: string | undefined;",
            "rendered: Pick<ChatGenerationConfig, 'tools' | 'systemInstruction'>,",
            "const preambleProved = JSON.stringify({",
            "tools: rendered.tools,",
            "declared?.systemInstruction ?? rendered.systemInstruction,",
            "repositoryDataBytes: declared?.repositoryDataBytes ?? 0,",
            "turnBudgetBytes: declared?.turnBudgetBytes ?? 0,",
            "if (this.provenPreamble === preambleProved) return;",
            "const bare = await counted([]);",
            "this.provenPreamble = preambleProved;",
            "        countExactTextTokens,\n        { ...this.generationConfig, ...params.config },\n      );",
        ),
        label=label,
        location=chat,
    )
    forbid_text(state, chat, "declarationsVerified", label=label)
    for case in (
        "proves an unchanged preamble once, however many turns send it",
        "re-proves the preamble before the next turn when its tool declarations change",
        "refuses the turn that would send re-declared tools past the share",
    ):
        require_text(state, chat_test, case, label=label)
    require_text(
        state,
        "packages/core/src/core/client.ts",
        "chat.declareStartupContext(declareStartupContext(history));",
        label=label,
    )
    require_text(
        state,
        "packages/core/src/agents/runtime/agent-core.ts",
        "chat.declareStartupContext(declareStartupContext(startHistory));",
        label=label,
    )
    # A compaction keeps the startup context whole at the head of the history
    # it counts and commits, so nothing rebuilds it afterwards and nothing can
    # fail to. The rebuild, and the catch that swallowed its failures, are
    # gone.
    _require_ordered(
        _source(state, "packages/core/src/services/chatCompressionService.ts", label=label),
        (
            "const startupContext = chat.getStartupContext();",
            "...(startupContext ? [startupContext] : []),",
            "...composePostCompactHistory(historyBeforeCompaction, summary, {",
            "const candidateCount = await chat.countRequestTokensForCandidateHistory(",
        ),
        label=label,
        location="packages/core/src/services/chatCompressionService.ts",
    )
    for rebuilt in (
        "restoreStartupContextAfterCompaction",
        "Failed to restore startup context after compaction",
    ):
        forbid_text(state, "packages/core/src/core/client.ts", rebuilt, label=label)
    require_text(
        state,
        "packages/core/src/core/client.ts",
        "extraHistory && stripStartupContext(extraHistory),",
        label=label,
    )
    for test_path, name in (
        ("packages/core/src/utils/environmentContext.test.ts", "renders the context as upstream does when its workspace data fits"),
        ("packages/core/src/utils/environmentContext.test.ts", "cuts a listing past its cap to whole lines, ending in the upstream indicator"),
        ("packages/core/src/utils/environmentContext.test.ts", "measures the listing in the NFC bytes the tokenizer reads"),
        ("packages/core/src/utils/environmentContext.test.ts", "refuses environment lines past their cap rather than cut a directory short"),
        ("packages/core/src/utils/environmentContext.test.ts", "refuses to declare a context the proof could not bound"),
        (chat_test, "counts the startup context without its workspace data and bounds that data by bytes"),
        (chat_test, "refuses a startup context whose workspace bound passes the share, naming that room"),
        (service_test, "keeps the startup context whole at the head of the history it counts and commits"),
        ("packages/core/src/core/client.test.ts", "leaves the startup prelude to the compaction that kept it"),
    ):
        require_text(state, test_path, name, label=label)
    _require_all(
        state,
        "packages/core/src/core/openaiContentGenerator/pipeline.ts",
        ("async countTextTokens(", "prompt: request.text,", "add_special_tokens: false,"),
        label=label,
    )
    _require_all(
        state,
        "packages/core/src/core/geminiChat.test.ts",
        (
            "'refuses a template that frames one %s past the declared framing',",
            "'shows why an empty probe measured nothing: the converter never sends it'",
            "'measures every framing from the wire requests the converter really sends'",
            "'counts the declared instruction and bounds its repository data by bytes'",
            "'counts the instruction with its turn budget left out and bounds the budget by its widest rendering'",
            "'refuses a declared preamble whose turn budget bound passes its share, naming that room'",
            "'drops a declaration when the instruction it describes is replaced'",
        ),
        label=label,
    )
    require_text(
        state,
        "packages/core/src/core/client.test.ts",
        "declares the turn budget's place left empty and its widest rendering, whatever budget the session was given",
        label=label,
    )
    for absent in (
        "turnOutputBudget",
        "defaultOutputCeiling",
        "explicitOutputCeiling",
        "outputCeiling",
        "QWEN_CODE_MAX_OUTPUT_TOKENS",
        "OUTPUT_TOKEN_CEILING",
        "samplingParams?.max_tokens",
        "summaryReserve",
        "generationReserve",
        "directiveReserve",
    ):
        forbid_text(state, chat, absent, label=label)
    # The route refuses a configured ceiling at configuration, so the one
    # limit a turn has is the one the send path derives.
    _require_all(
        state,
        generator,
        (
            "PROVIDER_OUTPUT_BUDGET_KEYS",
            "OUTPUT_CEILING_ENV",
            "would be a second bound on the same quantity and is refused.",
        ),
        label=label,
    )
    _require_all(
        state,
        limits,
        (
            "export const PROVIDER_OUTPUT_BUDGET_KEYS = [",
            "export const OUTPUT_CEILING_ENV = 'QWEN_CODE_MAX_OUTPUT_TOKENS';",
        ),
        label=label,
    )
    require_text(state, pipeline, "PROVIDER_OUTPUT_BUDGET_KEYS,", label=label)
    forbid_text(
        state,
        pipeline,
        "const PROVIDER_OUTPUT_BUDGET_KEYS = [",
        label=label,
    )

    # The compaction unit is the prompt the last turn was issued against,
    # split off the history by the chat that issued it and rendered as that
    # request was; the turn after it is kept whole. The rule is one function
    # and it walks back over the model content the turn recorded.
    _require_all(
        state,
        chat,
        (
            "export function splitIssuedTurn(",
            "while (index >= 0 && history[index].role === 'model') index--;",
            "if (index < 0 || index === history.length - 1) return undefined;",
            "renderIssuedTurnSplit(",
            "this.getRequestHistoryFrom(split.prompt, issuedUserContent),",
            "turn: extractCuratedHistory(split.turn).map(copyContentContainer),",
        ),
        label=label,
    )
    for case in (
        "refuses when the directive outgrows the static preamble",
        "gives the snapshot at least a turn's room at the largest prompt a turn can be issued against",
        "sends the last issued prompt, and nothing after it, to one cache-preserving main-model request",
        "keeps the pending tool result out of the summary and in the turn's commit counts",
        "refuses to compact before any turn was issued",
    ):
        require_text(state, service_test, case, label=label)
    for case in (
        "gives every turn the same room, whatever its prompt",
        "sends no output limit but the turn's room, whatever the request asked for",
        "issues no turn once the rendered prompt reaches the compaction trigger",
        "refuses when the tokenizer reports another window",
        "refuses when the provider declares no context window",
    ):
        require_text(state, chat_test, case, label=label)
    for case in (
        "declares D and M as shares of the window and F as a constant",
        "gives a turn the largest room the fit allows, at every window",
        "spends the whole window and nothing more",
        "cannot overrun the window from the largest admitted prompt",
        "issues the compaction of any admitted prompt with a whole turn of room",
        "leaves the trigger independent of the static preamble",
        "names the served partition",
        "refuses a window too small to leave a turn any room",
        "refuses a window that is not a count of tokens",
        "is the turn generation room and nothing else",
        "gives a turn the same room whatever its prompt",
        "spends the byte-to-token theorem once, as M plus the framing",
    ):
        require_text(state, limits_test, case, label=label)
    # The retired vocabulary cannot come back through a test either: a case
    # that still names a reserve is describing a partition this deployment no
    # longer has.
    for retired in ("generationReserve", "directiveReserve"):
        for path in (limits_test, chat_test, service_test):
            forbid_text(state, path, retired, label=label)
    for case in (
        "refuses a configured output ceiling: samplingParams.%s",
        "refuses the output-ceiling environment variable",
    ):
        require_text(state, generator_test, case, label=label)

    # The snapshot's room is sized from the request that was counted rather
    # than from a margin: the issued prompt plus the directive, less the
    # widest notice a redraw appends. The directive and that notice, framed,
    # are held to the static preamble's share before the first draw, so every
    # draw -- the first and each redraw -- is issued under one ceiling that is
    # never below a turn's room.
    service_source = _require_all(
        state,
        service,
        (
            "const partition = partitionContextWindow(contextLimit);",
            "const split = chat.renderIssuedTurnSplit(",
            "const issuedPrompt = split.prompt;",
            "const redrawNoticeTokens =\n        partition.messageFraming + DRAW_REFUSAL_NOTICE_MAX_BYTES;",
            "if (directiveTokens + redrawNoticeTokens > partition.staticPreamble) {",
            "compactionOutputBudget =\n        contextLimit - summaryRequestTokenCount - redrawNoticeTokens;",
            "if (compactionOutputBudget < partition.turnGeneration) {",
            "      if (originalTokenCount < partition.compactionTrigger) {",
            "    if (newTokenCount >= partition.compactionTrigger) {",
            "          contents,\n          config: {\n            ...sideQueryOptions.config,\n            maxOutputTokens: compactionOutputBudget,",
            # The widest notice is derived from the notices, every kind of
            # refusal at its widest, keyed so a kind cannot go unheld.
            "export const DRAW_REFUSAL_NOTICE_MAX_BYTES = Math.max(",
            "    .map((refusal) => tokenizerText(drawRefusalNotice(refusal)).bytes),",
            "const WIDEST_DRAW_REFUSALS: Record<\n  DrawRefusal['kind'],\n  readonly DrawRefusal[]\n> = {",
            "} satisfies Record<StateSnapshotLack['kind'], StateSnapshotLack>).map(",
        ),
        label=label,
    )
    for absent in (
        "summaryReserve",
        "pendingToolResult",
        "sideQueryHistory",
        "generationReserve",
        "directiveReserve",
    ):
        forbid_text(state, service, absent, label=label)
    # The snapshot is issued at the room the window actually has, less the
    # widest redraw notice, and that room is never less than a turn's: the
    # prompt this request extends was issued below the trigger, so the request
    # and a redraw's notice are at most T - 1 + D, leaving at least C + 1. The
    # floor is asserted in the service so a broken partition fails loudly,
    # rather than clamped so it silently shrinks the snapshot -- a clamp is
    # what hands a summary a few thousand tokens and truncates it.
    _require(
        "Math.min(" not in service_source.split("compactionOutputBudget =")[1][:400],
        f"{label}: {service} clamps the summary instead of asserting its floor",
    )
    for absent in (
        "COMPACT_MAX_OUTPUT_TOKENS",
        "SUMMARY_RESERVE",
        "AUTOCOMPACT_BUFFER",
        "WARN_BUFFER",
        "HARD_BUFFER",
        "DEFAULT_PCT",
        "COMPACTION_BUDGET_SAFETY_MARGIN",
        "computeCompactionOutputBudget",
        "computeThresholds",
        "effectiveWindow",
    ):
        _require(
            absent not in service_source,
            f"{label}: {service} still carries the tuned ladder term '{absent}'",
        )
    # A share of the window is not a setting: nothing reads a configured
    # fraction to decide when compaction is due.
    for path in (
        "packages/core/src/config/config.ts",
        "packages/cli/src/config/config.ts",
        "packages/cli/src/config/settingsSchema.ts",
    ):
        for absent in ("autoCompactThreshold", "getAutoCompactThreshold"):
            forbid_text(state, path, absent, label=label)

    # The compaction request is an ordinary one, and every attempt is the same
    # request under the same ceiling. A compaction may draw more than one
    # candidate, because a candidate the model itself malformed says nothing
    # about the request that produced it and another draw is judged by the
    # same rule. A redraw adds one message and nothing else: why the draw
    # before it was refused, and what to do instead, so it does not start over
    # blind. What is forbidden is a second attempt that asks for less than the
    # first: a smaller budget, a split output, a shrunken input, a converging
    # backoff. Those accept through a weaker route, which is a fallback wearing
    # a retry's name, and the terms below are how that has been reintroduced
    # before.
    for absent in (
        "COMPACT_THINKING_TOKEN_BUDGET",
        "COMPACT_FINAL_RESPONSE_TOKEN_BUDGET",
        "COMPACT_PHASE_DELIMITER_ALLOWANCE",
        "MAX_TRUNCATION_BACKOFF",
        "MAX_CONSECUTIVE_FAILURES",
        "summaryRequestBudget",
        "cleanSplitIndices",
        "carriedTail",
        "phaseBudgetOverrides",
    ):
        _require(
            absent not in service_source,
            f"{label}: {service} splits or converges the compaction budget via '{absent}'",
        )
    # Redrawing is bounded, and the bound is counted in candidates rather than
    # in elapsed time or consecutive faults, so the cost of a transition is
    # knowable before it runs.
    _require_all(
        state,
        service,
        (
            "export const MAX_COMPACTION_CANDIDATE_ATTEMPTS = 4;",
            "      outcome.kind === 'resampleable' &&",
            "      rejectedAttempts.length + 1 < MAX_COMPACTION_CANDIDATE_ATTEMPTS",
        ),
        label=label,
    )
    # The first draw is the counted request; each redraw is that request and
    # one notice about the draw just before it, never an accumulation and
    # never the refused draw itself, which would not fit the proved room.
    _require_ordered(
        service_source,
        (
            "let outcome = await drawCandidate(sideQueryOptions.contents);",
            "const notice = drawRefusalNotice(outcome.refusal);",
            "rejectedAttempts.push({",
            "outcome = await drawCandidate([\n        ...sideQueryOptions.contents,\n        { role: 'user', parts: [{ text: notice }] },\n      ]);",
        ),
        label=label,
        location=service,
    )
    # The rule this replaced -- every draw the same frozen request, adding
    # nothing -- no longer describes the code, so its wording may not return.
    for retired in ("same frozen options", "adds nothing to them"):
        forbid_text(state, service, retired, label=label)
    forbid_text(state, service_test, "The request is frozen across draws", label=label)
    # A notice is built only from what the service measured and closed names:
    # no refusal field is a string the model could have written, so every
    # notice has the widest rendering the preflight holds.
    for path, start in (
        (service, "export type DrawRefusal ="),
        ("packages/core/src/services/state-snapshot.ts", "export type StateSnapshotLack ="),
    ):
        declared = _source(state, path, label=label)
        _require(start in declared, f"{label}: {path} lacks '{start}'")
        block = declared.split(start, 1)[1].split(";\n\n", 1)[0]
        _require(
            ": string" not in block,
            f"{label}: {path} lets a refusal carry text, so a notice has no widest rendering",
        )
    _require_all(
        state,
        service,
        (
            "          refusal: DrawRefusal;",
            "          refusal: { kind: 'truncated', limit: compactionOutputBudget },",
            "          refusal: { kind: 'unfinished' },",
            "              : { kind: 'incomplete', lack: acceptance.lack },",
            "            kind: 'not_smaller',",
            "            kind: 'no_room',",
        ),
        label=label,
    )
    _require_all(
        state,
        "packages/core/src/services/state-snapshot.ts",
        (
            "      lack: { kind: 'call_count', calls: calls.length },",
            "      lack: { kind: 'other_function' },",
            "      lack: { kind: 'no_arguments' },",
            "      lack: { kind: 'schema' },",
            "      lack: { kind: 'empty_sections', sections: empty },",
            "      bytes: rendered.bytes,",
        ),
        label=label,
    )
    for case in (
        "tells the next draw why %s was refused, and what to do instead",
        "does not show the next draw the draw that was refused",
        "tells each redraw only why the draw just before it was refused",
        "holds the widest redraw notice inside the directive's share of the static preamble",
        "derives the widest notice from the notices themselves",
    ):
        require_text(state, service_test, case, label=label)
    require_text(
        state,
        "packages/core/src/services/state-snapshot.test.ts",
        "says what a draw lacked without repeating anything the model wrote",
        label=label,
    )
    # Only the model's own output earns another draw. A cause that is fixed
    # given the frozen request -- an unsupported configuration, a transport or
    # protocol fault, a workspace read, a counter -- refuses identically however
    # often it is asked, so it is named rather than retried. The distinction is
    # carried in the outcome's shape, not recovered from its status.
    _require_all(
        state,
        service,
        (
            "          kind: 'accepted';",
            "          kind: 'resampleable';",
            "          kind: 'terminal';",
        ),
        label=label,
    )
    # Every draw is billed, so every draw is reported. The candidates a
    # transition refused are retained beside the one that settled it, and the
    # list is always present: "nothing was refused" and "refusals were not
    # recorded" are different facts and must not share a representation.
    _require_all(
        state,
        service,
        ("    const rejectedAttempts: CompactionRejectedAttempt[] = [];",),
        label=label,
    )
    _require_all(
        state,
        "packages/core/src/core/turn.ts",
        (
            "  rejectedAttempts: CompactionRejectedAttempt[];",
            "  rejectedAttempts: CompactionRejectedAttemptRecord[];",
        ),
        label=label,
    )
    # The per-request phase-budget apparatus is gone from the layers that
    # carried it, so no caller can reintroduce a split budget.
    for path in (pipeline, generator):
        forbid_text(state, path, "phaseBudgetOverrides", label=label)
    # The request is the issued prompt as the provider already holds it: the
    # same contents, in the same order, one appended directive, and nothing
    # the turn recorded after that prompt. Reshaping it would re-prefill the
    # prompt for no reason; carrying the turn would make the turn's size the
    # request's problem, which is the reservation this design removed.
    _require_all(
        state,
        service,
        (
            "      contents: [...issuedPrompt, directiveContent],",
            "      promptCacheSharing: true,",
            "            turn,\n          }),",
        ),
        label=label,
    )
    # A truncated snapshot is refused before its content is examined, so one
    # that superficially parses complete can never be committed.
    _require_ordered(
        service_source,
        (
            "if (summaryResult.finishReason === FinishReason.MAX_TOKENS) {",
            "COMPRESSION_FAILED_OUTPUT_TRUNCATED",
            "if (!acceptance.snapshot) {",
        ),
        label=label,
        location=service,
    )
    require_text(
        state,
        service_test,
        "discards a truncated response even when it carries a complete snapshot",
        label=label,
    )
    # The snapshot schema gives each kind of fact one home; the retired
    # nine-section schema gave one code snippet four. The sections are
    # described where they are declared, so the prompt shows no skeleton and
    # spends its instructions on faithfulness, which is the only part of this
    # the model can be held to. It also says where the conversation it is
    # shown ends: at the issued prompt, with the turn following the snapshot.
    _require_all(
        state,
        prompts,
        (
            "record the snapshot by calling the one function this request declares",
            "there is no markup to produce and nothing to escape",
            "exactly once",
            "ends with the prompt the latest turn was issued against",
            "follow your snapshot verbatim",
            "make next_step what that prompt called for",
        ),
        label=label,
    )
    for retired in (
        "<state_snapshot>",
        "<intent>",
        "<all_user_messages>",
        "<files_and_code_sections>",
        "CDATA",
        "<analysis>",
    ):
        forbid_text(state, prompts, retired, label=label)
    require_text(
        state,
        prompts_test,
        "states the writing contract: dense, deduplicated by structure",
        label=label,
    )
    require_text(
        state,
        prompts_test,
        "asks for the declared call and describes no markup at all",
        label=label,
    )
    require_text(
        state,
        prompts_test,
        "ends the snapshot at the prompt the latest turn was issued against",
        label=label,
    )

    _require_all(
        state,
        "packages/core/src/core/authored-instructions.ts",
        (
            "retainUserInput",
            "retainDelegatedTask",
            "retainedInstructionParts",
            "carryHistoryInstructions",
            "Conflicting authored instruction",
            "structuredClone",
        ),
        label=label,
    )
    # The post-compact history is one user content the resuming agent reads,
    # then the turn, verbatim. No synthetic model content stands between them:
    # a model turn with no reasoning renders with an empty thinking block,
    # which tells the model that turn thought nothing.
    _require_all(
        state,
        attachments,
        (
            "export function composePostCompactHistory(",
            "): Content[] {",
            "const { planModeActive, runningSubagents, turn = [] } = options;",
            "const authoredParts = retainedInstructionParts(history)",
            "...authoredParts",
            "  turn?: Content[];",
            "return [{ role: 'user', parts }, ...turn.map((content) => ({ ...content }))];",
        ),
        label=label,
    )
    for absent in (
        "Got it. Thanks for the additional context!",
        "trailingFunctionCallContent",
        "ackParts",
        "postAckParts",
    ):
        forbid_text(state, attachments, absent, label=label)
    # Nothing is restored behind the snapshot: no file is read back off disk
    # by a guessed size, and no earlier image is re-embedded. A file the
    # session read is one read_file away afterwards exactly as before, and
    # the composer reads nothing, so it is synchronous and cannot fail on the
    # workspace.
    for absent in (
        "export async function composePostCompactHistory(",
        "node:fs",
        "CHARS_PER_TOKEN",
        "POST_COMPACT_",
        "extractRecentFilePaths",
        "extractRecentImages",
        "countToolResponseImages",
        "readFileSizeAdaptive",
        "buildFileRestorationBlocks",
        "buildImageRestorationBlock",
        "workspaceRoot",
        "maxFiles",
        "maxImages",
    ):
        forbid_text(state, attachments, absent, label=label)
    for absent in (
        "countToolResponseImages",
        "enableScreenshotTrigger",
        "screenshotTriggerThreshold",
        "image_overflow",
        "maxRecentFiles",
        "resolveCompactionTuning",
    ):
        forbid_text(state, service, absent, label=label)
    slimming = "packages/core/src/services/compactionInputSlimming.ts"
    for absent in (
        "maxRecentFiles",
        "DEFAULT_MAX_RECENT_FILES",
        "ScreenshotTrigger",
        "screenshotTrigger",
        "QWEN_COMPACT_MAX_RECENT_FILES",
        "QWEN_COMPACT_SCREENSHOT",
    ):
        forbid_text(state, slimming, absent, label=label)
    for path in (
        "packages/core/src/config/config.ts",
        "packages/core/src/core/turn.ts",
        "packages/cli/src/ui/hooks/useGeminiStream.ts",
        "packages/cli/src/acp-integration/session/Session.ts",
    ):
        for absent in (
            "maxRecentFilesToRetain",
            "enableScreenshotTrigger",
            "screenshotTriggerThreshold",
            "image_overflow",
        ):
            forbid_text(state, path, absent, label=label)
    forbid_text(state, prompts, "may also be restored", label=label)
    for case in (
        "carries the turn verbatim behind the snapshot, reasoning included",
        "ends with the turn so a pending functionResponse has its match",
        "restores nothing from the workspace or the earlier history",
    ):
        require_text(state, attachments_test, case, label=label)
    for path, case in (
        (
            "packages/core/src/core/authored-instructions.test.ts",
            "carries an authored image as retained input and restores no other image",
        ),
        (
            "packages/core/src/services/compactionInputSlimming.test.ts",
            "resolves only the image-payload knobs, now that compaction restores nothing",
        ),
    ):
        require_text(state, path, case, label=label)
    # The compaction request declares the snapshot function and forces the
    # call, so the artifact is constrained where it is generated rather than
    # judged after the model has hand-written it.
    _require_all(
        state,
        service,
        (
            "tools: [stateSnapshotTool(partition.inlineBlockBytes)],",
            "mode: FunctionCallingConfigMode.ANY,",
            "acceptStateSnapshot(\n        summaryResult.functionCalls,\n        partition.inlineBlockBytes,\n      )",
            "if (!acceptance.snapshot) {",
            # A snapshot refused for its length was not empty. It is recorded
            # under its own status, with the snapshot it declared, and never
            # as a draw that produced no summary.
            "  incomplete: CompressionStatus.COMPRESSION_FAILED_EMPTY_SUMMARY,",
            "  over_bound: CompressionStatus.COMPRESSION_FAILED_SUMMARY_OVER_BOUND,",
            "status: SNAPSHOT_REFUSAL_STATUS[acceptance.refused],",
            "        : acceptance.refused === 'over_bound'\n          ? acceptance.rendered",
        ),
        label=label,
    )
    forbid_text(state, service, "isValidStateSnapshot", label=label)
    forbid_text(
        state,
        service,
        "status: CompressionStatus.COMPRESSION_FAILED_EMPTY_SUMMARY,",
        label=label,
    )
    for path, case in (
        (service_test, "records a snapshot refused for its length as over the bound, with what the model wrote"),
        (service_test, "'a snapshot past one inline block'"),
        ("packages/core/src/services/state-snapshot.test.ts", "refuses %s as declaring no complete snapshot"),
        ("packages/cli/src/utils/compression-result.test.ts", "COMPRESSION_FAILED_SUMMARY_OVER_BOUND, 'error', 'longer than its bound'"),
    ):
        require_text(state, path, case, label=label)


def _validate_compaction_accounting_before(state: State) -> None:
    label = "compaction output-accounting precondition"
    turn = "packages/core/src/core/turn.ts"
    service = "packages/core/src/services/chatCompressionService.ts"
    # Nothing records how the fixed maintenance budget was spent; the only
    # trace of a truncated attempt is a debug warning that this deployment
    # never enables.
    forbid_text(state, turn, "CompactionOutputAccounting", label=label)
    forbid_text(state, service, "outputAccounting", label=label)
    require_text(
        state,
        service,
        "export const COMPACT_MAX_OUTPUT_TOKENS = 20_000;",
        label=label,
    )


def _validate_compaction_accounting_after(state: State) -> None:
    label = "compaction output-accounting result"
    _require_all(
        state,
        "packages/core/src/core/turn.ts",
        (
            "export interface CompactionOutputAccounting {",
            "maxOutputTokens: number;",
            "usage: ServedUsage | null;",
            "summary: string;",
            "reasoning: string;",
            "finishReason: string | null;",
            "requestAttempts: number;",
            "output: CompactionOutputAccounting | null;",
            "output: info.output ?? null,",
        ),
        label=label,
    )
    service = "packages/core/src/services/chatCompressionService.ts"
    # A request that was issued and then failed still spent the budget, and its
    # prefix is the only evidence of where. `GenerationTextFailure` does not by
    # itself prove a request left -- generateText wraps every error it caught in
    # one, including failures from before the call -- so the attempt count is
    # what separates a draw that spent something from one that never started.
    source = _require_all(
        state,
        service,
        (
            "error instanceof GenerationTextFailure",
            "error.partial.requestAttempts > 0",
            "summary: partial.text",
            "reasoning: partial.thoughtText",
            "finishReason: partial.finishReason ?? null",
            "requestAttempts: partial.requestAttempts",
            "    ): CompactionOutputAccounting => ({",
            "usage: summaryUsage",
            "reasoning: summaryResult.thoughtText",
            "requestAttempts: summaryResult.requestAttempts",
        ),
        label=label,
    )
    # One builder assembles the accounting, and every outcome reached after a
    # draw carries what that draw spent: the terminal one carries it exactly
    # when a request was issued, and each post-acceptance outcome carries the
    # accepted draw's. None of them may report an attempt without its cost.
    _require(
        source.count("const accountingFor = (") == 1
        and source.count("const outputAccounting = accountingFor(outcome.accounting);") == 1
        and source.count("? { output: accountingFor(outcome.accounting) }") == 1
        and source.count("output: outputAccounting,") == 3,
        f"{label}: all post-generation outcomes must retain the same served evidence",
    )
    _require_all(
        state,
        "packages/core/src/services/chatCompressionService.test.ts",
        (
            "[compaction-event] records where every truncated draw spent its output budget",
            "[compaction-event] leaves the accounting null when no generation ran",
        ),
        label=label,
    )
    require_text(
        state,
        "packages/cli/src/nonInteractive/io/BaseJsonOutputAdapter.test.ts",
        "[compaction-event] carries the failed attempt output accounting to the stream",
        label=label,
    )


def _validate_incomplete_generation_before(state: State) -> None:
    label = "incomplete-generation precondition"
    turn = "packages/core/src/core/turn.ts"
    cli = "packages/cli/src/nonInteractiveCli.ts"
    types = "packages/cli/src/nonInteractive/types.ts"
    adapter = "packages/cli/src/nonInteractive/io/BaseJsonOutputAdapter.ts"
    agent_core = "packages/core/src/agents/runtime/agent-core.ts"
    agent_types = "packages/core/src/agents/runtime/agent-types.ts"
    # The provider's terminal reason reaches the headless adapter, which reads
    # the usage half of the event and drops the reason, so nothing downstream
    # of the stream can separate a completed message from a severed one.
    require_text(
        state,
        adapter,
        "case GeminiEventType.Finished:\n        if (event.value?.usageMetadata) {",
        label=label,
    )
    # Both reasoning loops equate "this turn requested no tools" with an
    # answer, and neither consults the reason the generation ended.
    require_text(
        state,
        cli,
        "let shouldFinalizeTurn = toolCallRequests.length === 0;",
        label=label,
    )
    require_text(
        state,
        agent_core,
        "// No tool calls \u2014 treat this as the model's final answer.",
        label=label,
    )
    for path in (turn, cli, agent_core):
        forbid_text(state, path, "describeIncompleteGeneration", label=label)
    for path in (cli, types):
        forbid_text(state, path, "error_incomplete_generation", label=label)
    forbid_text(state, agent_types, "INCOMPLETE_GENERATION", label=label)


def _validate_incomplete_generation_after(state: State) -> None:
    label = "incomplete-generation result"
    turn = "packages/core/src/core/turn.ts"
    turn_test = "packages/core/src/core/turn.test.ts"
    cli = "packages/cli/src/nonInteractiveCli.ts"
    cli_test = "packages/cli/src/nonInteractiveCli.test.ts"
    types = "packages/cli/src/nonInteractive/types.ts"
    agent_core = "packages/core/src/agents/runtime/agent-core.ts"
    agent_types = "packages/core/src/agents/runtime/agent-types.ts"
    subagent_result = "packages/core/src/agents/subagent-result.ts"

    # One predicate decides it, in core, so the main line and every subagent
    # answer the question the same way. STOP is the only self-ended terminal.
    turn_source = _require_all(
        state,
        turn,
        (
            "export function describeIncompleteGeneration(",
            "reason: FinishReason | undefined,",
            "issued?: IssuedGeneration,",
            "export function issuedGeneration(",
            # The limit the model is told about is read from the rule that
            # sets it, not restated as arithmetic beside it, so the sentence
            # cannot drift from the partition.
            "turnOutputLimit(partitionContextWindow(issued.window))",
        ),
        label=label,
    )
    _require_ordered(
        turn_source,
        (
            "export function describeIncompleteGeneration(",
            "if (reason === FinishReason.STOP) {",
            "return null;",
            "reason ?? 'no terminal reason'",
        ),
        label=label,
        location=turn,
    )

    # The session's success path is gated on it, and the reason it carries is
    # the one the last generation reported, in whichever loop produced it.
    cli_source = _require_all(
        state,
        cli,
        (
            "let lastGenerationFinishReason: GeminiFinishedEventValue['reason'];",
            "let lastGenerationUsage: GeminiFinishedEventValue['usageMetadata'];",
            "const incompleteGeneration = describeIncompleteGeneration(",
            "                issuedGeneration(\n                  lastGenerationUsage,",
            "terminateMode: AgentTerminateMode.INCOMPLETE_GENERATION,",
        ),
        label=label,
    )
    _require(
        cli_source.count("lastGenerationUsage = event.value.usageMetadata;") == 2,
        f"{label}: the main-turn and drain loops do not both record the served usage the terminal reason belongs to",
    )
    _require(
        cli_source.count("lastGenerationFinishReason = event.value.reason;") == 2,
        f"{label}: the main-turn and drain loops do not both record the terminal reason",
    )
    _require(
        cli_source.count("lastGenerationFinishReason = undefined;") == 2,
        f"{label}: a turn head can inherit a stale terminal reason",
    )
    _require_ordered(
        cli_source,
        (
            "const incompleteGeneration = describeIncompleteGeneration(",
            "terminateMode: AgentTerminateMode.INCOMPLETE_GENERATION,",
            "if (config.getJsonSchema()) {",
            "return finish({ terminateMode: AgentTerminateMode.GOAL });",
        ),
        label=label,
        location=cli,
    )
    # The state's record carries the name the stream contract's terminal table
    # defines for it; the client's build refuses a name the table does not.
    require_text(
        state,
        types,
        "  [AgentTerminateMode.INCOMPLETE_GENERATION]: terminalResult(\n"
        "    'error_incomplete_generation',\n"
        "  ),",
        label=label,
    )

    # The same rule inside a subagent, whose report the parent would otherwise
    # integrate as a conclusion.
    agent_core_source = _require_all(
        state,
        agent_core,
        (
            "let roundFinishReason: FinishReason | undefined;",
            "roundFinishReason = chunkFinishReason;",
            "terminateMode = AgentTerminateMode.INCOMPLETE_GENERATION;",
            "// No tool calls and a self-ended generation \u2014 this is the",
        ),
        label=label,
    )
    _require_ordered(
        agent_core_source,
        (
            "roundFinishReason = undefined;",
            "roundFinishReason = chunkFinishReason;",
            "describeIncompleteGeneration(",
            "roundFinishReason,",
            "this.reasoningTurnsUsed,",
            "issuedGeneration(",
            "lastUsage,",
            "terminateMode = AgentTerminateMode.INCOMPLETE_GENERATION;",
        ),
        label=label,
        location=agent_core,
    )
    require_text(
        state,
        agent_types,
        "INCOMPLETE_GENERATION = 'INCOMPLETE_GENERATION',",
        label=label,
    )
    require_text(
        state,
        subagent_result,
        "reason = `was cut off mid-generation${turns}`;",
        label=label,
    )

    # Executed in the build: the predicate itself, the session's two terminal
    # states, and which generation the session reads.
    require_text(
        state,
        turn_test,
        "describe('describeIncompleteGeneration'",
        label=label,
    )
    require_text(
        state,
        turn_test,
        "names %s as a generation stopped from outside",
        label=label,
    )
    require_text(
        state,
        turn_test,
        "names the prompt, the limit and the output when a generation reached its limit",
        label=label,
    )
    require_text(
        state,
        cli_test,
        "reports a run whose last generation was stopped by $name as incomplete",
        label=label,
    )
    require_text(
        state,
        cli_test,
        "reports a run whose last generation ended on its own as a success",
        label=label,
    )
    require_text(
        state,
        cli_test,
        "reads the last generation of the run, not an earlier completed one",
        label=label,
    )
    # A fixture that leaves the terminal reason unstated would exercise the
    # incomplete branch by accident, so none may.
    forbid_text(state, cli_test, "reason: undefined", label=label)


_FINAL_MESSAGE_SLIP_MODULE = "packages/core/src/core/final-message-slip.ts"
_FINAL_MESSAGE_SLIP_MARKUP = (
    "'<tool_call>',",
    "'</tool_call>',",
    "'<function=',",
    "'</function>',",
    "'<parameter=',",
    "'</parameter>',",
)
_OLD_FINAL_RESULT_NUDGE = "'Please provide the final result now and stop calling tools.'"
# Upstream asks a model, after every turn that ends without a tool call, whether it
# should keep talking, and answers "Please continue." when it says yes. A setting
# only switched it off; the check, its telemetry event and the setting are gone.
_NEXT_SPEAKER_MODULE = "packages/core/src/utils/nextSpeakerChecker.ts"
_NEXT_SPEAKER_TEST = "packages/core/src/utils/nextSpeakerChecker.test.ts"
_RETIRED_NEXT_SPEAKER_NAMES = (
    "checkNextSpeaker",
    "nextSpeakerChecker",
    "NextSpeakerCheckEvent",
    "logNextSpeakerCheck",
    "EVENT_NEXT_SPEAKER_CHECK",
    "next_speaker_check",
    "skipNextSpeakerCheck",
    "SkipNextSpeakerCheck",
    "Skip Next Speaker Check",
)


def _validate_final_message_slip_before(state: State) -> None:
    label = "final-message-slip precondition"
    cli = "packages/cli/src/nonInteractiveCli.ts"
    types = "packages/cli/src/nonInteractive/types.ts"
    agent_core = "packages/core/src/agents/runtime/agent-core.ts"
    agent_types = "packages/core/src/agents/runtime/agent-types.ts"

    # No shared rule exists: the session takes any self-ended turn without a
    # tool call as its answer, and a subagent nudges once on empty text with
    # a request the model may keep ignoring.
    _require(
        _FINAL_MESSAGE_SLIP_MODULE not in state,
        f"{label}: {_FINAL_MESSAGE_SLIP_MODULE} already exists",
    )
    require_text(state, agent_core, _OLD_FINAL_RESULT_NUDGE, count=2, label=label)
    for path in (cli, agent_core):
        forbid_text(state, path, "describeFinalMessageSlip", label=label)
        forbid_text(state, path, "finalMessageSlip", label=label)
    forbid_text(state, agent_types, "SLIPPED_FINAL_MESSAGE", label=label)
    forbid_text(state, types, "error_slipped_final_message", label=label)
    # The next-speaker check ships, switched off by a setting that defaults to off.
    _require_all(
        state,
        _NEXT_SPEAKER_MODULE,
        ("export async function checkNextSpeaker(",),
        label=label,
    )
    _require_all(
        state,
        "packages/core/src/core/client.ts",
        (
            "        if (this.config.getSkipNextSpeakerCheck()) {",
            "        const nextSpeakerCheck = await checkNextSpeaker(",
            "            : [{ text: 'Please continue.' }];",
        ),
        label=label,
    )


def _validate_final_message_slip_after(state: State) -> None:
    label = "final-message-slip result"
    module = _FINAL_MESSAGE_SLIP_MODULE
    module_test = "packages/core/src/core/final-message-slip.test.ts"
    index = "packages/core/src/index.ts"
    cli = "packages/cli/src/nonInteractiveCli.ts"
    cli_test = "packages/cli/src/nonInteractiveCli.test.ts"
    types = "packages/cli/src/nonInteractive/types.ts"
    adapter_test = "packages/cli/src/nonInteractive/io/BaseJsonOutputAdapter.test.ts"
    agent_core = "packages/core/src/agents/runtime/agent-core.ts"
    agent_core_test = "packages/core/src/agents/runtime/agent-core.test.ts"
    agent_types = "packages/core/src/agents/runtime/agent-types.ts"
    events = "packages/core/src/agents/runtime/agent-events.ts"
    headless = "packages/core/src/agents/runtime/agent-headless.ts"
    interactive = "packages/core/src/agents/runtime/agent-interactive.ts"
    agent_tool = "packages/core/src/tools/agent/agent.ts"
    subagent_result = "packages/core/src/agents/subagent-result.ts"
    subagent_result_test = "packages/core/src/agents/subagent-result.test.ts"
    recording = "packages/core/src/services/chatRecordingService.ts"
    branches = "packages/core/src/utils/conversation-branches.ts"
    sdk_ts = "packages/sdk-typescript/src/types/protocol.ts"
    sdk_python = "packages/sdk-python/src/qwen_code_sdk/protocol.py"

    # ── One rule, in core, used by both loops ────────────────────────────
    #
    # Detection is an exact string test on the turn's visible text: empty or
    # whitespace-only, or any of the served template's six tool-call markers.
    # It judges nothing about whether the task is done, and the next-speaker
    # judgment this deployment removed does not return through it.
    module_source = _require_all(
        state,
        module,
        (
            "export const FINAL_MESSAGE_SLIP_MARKUP = [",
            *_FINAL_MESSAGE_SLIP_MARKUP,
            "export type FinalMessageSlipKind = 'markup' | 'empty';",
            "export const FINAL_MESSAGE_SLIP_LIMIT = 3;",
            "export function describeFinalMessageSlip(",
            "export function finalMessageSlipNotice(",
            "export function describeFinalMessageSlipKind(",
            "export function describeFinalMessageSlipRun(",
            "export function describeSlippedFinalMessage(",
        ),
        label=label,
    )
    _require_ordered(
        module_source,
        (
            "export function describeFinalMessageSlip(",
            "if (text.trim().length === 0) {",
            "return 'empty';",
            "FINAL_MESSAGE_SLIP_MARKUP.some((markup) => text.includes(markup))",
            "return 'markup';",
            "return null;",
        ),
        label=label,
        location=module,
    )
    _require(
        module_source.count("'<") == len(_FINAL_MESSAGE_SLIP_MARKUP),
        f"{label}: {module} tests markers other than the six",
    )
    for needle in (
        "Your previous message ended the turn without a tool call, and it ",
        "'contained tool-call markup outside a structured call. Nothing was '",
        "'executed and nothing changed. If you meant to call a tool, emit it '",
        "'as a proper tool call; if you were quoting the syntax, restate your '",
        "'answer without the literal markup; otherwise give your final answer '",
        "'Your previous message ended the turn without a tool call and without '",
        "'any visible text. Nothing was executed and nothing changed. If you '",
        "'meant to call a tool, emit it as a proper tool call; otherwise give '",
        "a third message like it ends the run.",
        "'tool-call markup outside a structured call'",
        "'no visible text'",
        "three consecutive turns with a message that was not a final ",
        "after being told twice",
        "this run carries no final answer.",
    ):
        require_text(state, module, needle, label=label)
    for path in (module, cli, agent_core):
        forbid_text(state, path, "nextSpeaker", label=label)
        forbid_text(state, path, "checkNextSpeaker", label=label)
    require_text(state, index, "} from './core/final-message-slip.js';", label=label)
    # Nor anywhere else: the check, its test, its telemetry event and its setting are
    # gone from the shipped tree, and no file the transformation writes names them, so
    # nothing can switch the judgment back on. A turn that ends without a tool call
    # takes the completion every turn nobody continues takes.
    for absent in (_NEXT_SPEAKER_MODULE, _NEXT_SPEAKER_TEST):
        _require(absent not in state, f"{label}: {absent} still ships")
    for path, source in sorted(state.items()):
        for retired in _RETIRED_NEXT_SPEAKER_NAMES:
            _require(
                retired not in source,
                f"{label}: {path} still names the retired next-speaker {retired!r}",
            )
    forbid_text(
        state, "packages/core/src/core/client.ts", "[{ text: 'Please continue.' }]", label=label
    )

    # ── The session: notice, continuation, and the third slip's terminal ─
    #
    # Only a turn the model ended itself is read for a slip; a severed
    # generation keeps its own terminal. Both reasoning loops share one
    # counter and one answer, the notice goes to the model as the next turn
    # and to the stream and the recording as the user-role content it is,
    # and the third consecutive slip is raised as the one terminal shape.
    cli_source = _require_all(
        state,
        cli,
        (
            "let consecutiveFinalMessageSlips = 0;",
            "const noticeForFinalMessageSlip = async (",
            "if (consecutiveFinalMessageSlips >= FINAL_MESSAGE_SLIP_LIMIT) {",
            "terminateMode: AgentTerminateMode.SLIPPED_FINAL_MESSAGE,",
            "message: describeSlippedFinalMessage(kind),",
            "adapter.emitUserMessage(notice);",
            "recordMidTurnUserMessage(notice)",
            "? describeFinalMessageSlip(turnText)",
            "? describeFinalMessageSlip(itemText)",
        ),
        label=label,
    )
    _require(
        cli_source.count("lastGenerationFinishReason === FinishReason.STOP") == 2,
        f"{label}: both reasoning loops must read a slip only from a turn the "
        "model ended itself",
    )
    _require(
        cli_source.count("await noticeForFinalMessageSlip(") == 2,
        f"{label}: both reasoning loops must answer a slip through the one notice",
    )
    _require(
        cli_source.count("let consecutiveFinalMessageSlips = 0;") == 1
        and cli_source.count("consecutiveFinalMessageSlips = 0;") == 3,
        f"{label}: one count, declared once and reset by both reasoning loops "
        "on a turn that did not slip",
    )
    _require_ordered(
        cli_source,
        (
            "return emitLoopDetectedResult();",
            "lastGenerationFinishReason === FinishReason.STOP",
            "? describeFinalMessageSlip(turnText)",
            "await noticeForFinalMessageSlip(",
            "hasUnsentContinuation = true;",
            "continue;",
            "consecutiveFinalMessageSlips = 0;",
            "let shouldFinalizeTurn = toolCallRequests.length === 0;",
        ),
        label=label,
        location=cli,
    )
    forbid_text(state, cli, _OLD_FINAL_RESULT_NUDGE, label=label)
    drain_source = cli_source.split("const drainBatch = async () => {", 1)[1]
    _require_ordered(
        drain_source,
        (
            "const itemPromptId = `${prompt_id}/automatic/${turnCount + 1}`;",
            "while (true) {",
            "admitBudgetedTurn();",
            "turnCount++;",
            "const itemStream = geminiClient.sendMessageStream(",
            "await noticeForFinalMessageSlip(itemFinalMessageSlip)",
        ),
        label=label,
        location=cli,
    )
    forbid_text(state, cli, "void p.finally(", label=label)
    require_text(
        state, cli,
        "})().finally(() => {\n"
        "                  if (drainPromise === p) drainPromise = null;\n"
        "                });\n"
        "                drainPromise = p;\n"
        "                return p;",
        label=label,
    )
    # The state's record carries the name the stream contract's terminal table
    # defines for it; the client's build refuses a name the table does not.
    require_text(
        state,
        types,
        "  [AgentTerminateMode.SLIPPED_FINAL_MESSAGE]: terminalResult(\n"
        "    'error_slipped_final_message',\n"
        "  ),",
        label=label,
    )
    require_text(
        state,
        agent_types,
        "SLIPPED_FINAL_MESSAGE = 'SLIPPED_FINAL_MESSAGE',",
        label=label,
    )

    # ── The subagent: the same rule, after the incomplete check ──────────
    #
    # The nudge is gone. A slip is answered with the same notice, recorded
    # in the child transcript under its own kind, and the third in a row is
    # the same terminal state, carried to the parent with its shape.
    agent_core_source = _require_all(
        state,
        agent_core,
        (
            "let consecutiveFinalMessageSlips = 0;",
            "let finalMessageSlip: FinalMessageSlipKind | null = null;",
            "  private finalMessageSlipNotices = 0;",
            "  getFinalMessageSlipNotices(): number {",
            "const slip = describeFinalMessageSlip(roundText);",
            "if (consecutiveFinalMessageSlips >= FINAL_MESSAGE_SLIP_LIMIT) {",
            "terminateMode = AgentTerminateMode.SLIPPED_FINAL_MESSAGE;",
            "finalMessageSlip = slip;",
            "this.finalMessageSlipNotices += 1;",
            "kind: 'final_message_slip',",
        ),
        label=label,
    )
    _require_ordered(
        agent_core_source,
        (
            "terminateMode = AgentTerminateMode.INCOMPLETE_GENERATION;",
            "const slip = describeFinalMessageSlip(roundText);",
            "terminateMode = AgentTerminateMode.SLIPPED_FINAL_MESSAGE;",
            "finalMessageSlipNotice(",
            "kind: 'final_message_slip',",
            "consecutiveFinalMessageSlips = 0;",
            "// No tool calls and a self-ended generation — this is the",
        ),
        label=label,
        location=agent_core,
    )
    _require(
        agent_core_source.count("finalMessageSlipNotices: this.finalMessageSlipNotices,") == 3,
        f"{label}: every exit of the subagent loop must report its notices",
    )
    forbid_text(state, agent_core, _OLD_FINAL_RESULT_NUDGE, label=label)
    forbid_text(state, agent_core, "Please provide the final result", label=label)
    _require_all(
        state,
        events,
        (
            "  finalMessageSlip: FinalMessageSlipKind | null;",
            "  finalMessageSlipNotices: number;",
            "kind?: 'message' | 'notification' | 'final_message_slip';",
        ),
        label=label,
    )
    _require_all(
        state,
        headless,
        (
            "  getFinalMessageSlip(): FinalMessageSlipKind | null {",
            "          finalMessageSlip: this.finalMessageSlip,\n"
            "          finalMessageSlipNotices: this.core.getFinalMessageSlipNotices(),",
        ),
        label=label,
    )
    require_text(
        state,
        interactive,
        "case AgentTerminateMode.SLIPPED_FINAL_MESSAGE:",
        label=label,
    )
    _require_all(
        state,
        subagent_result,
        (
            "finalMessageSlip?: FinalMessageSlipKind | null,",
            "terminateMode === AgentTerminateMode.SLIPPED_FINAL_MESSAGE",
            "having ended ${describeFinalMessageSlipRun(finalMessageSlip)}",
        ),
        label=label,
    )
    _require(
        _source(state, agent_tool, label=label).count("subagent.getFinalMessageSlip()") == 4,
        f"{label}: {agent_tool} does not carry the shape of the slip to the "
        "parent, its failed call and the terminal display",
    )
    require_text(state, agent_tool, "event.finalMessageSlip,", label=label)
    require_text(
        state,
        recording,
        "externalInputKind?: 'message' | 'notification' | 'final_message_slip';",
        label=label,
    )
    require_text(
        state,
        branches,
        "record.externalInputKind === 'final_message_slip' ||",
        label=label,
    )
    require_text(state, sdk_ts, "| 'error_slipped_final_message'", label=label)
    require_text(state, sdk_python, '"error_slipped_final_message",', label=label)

    # ── Executed in the build ────────────────────────────────────────────
    require_text(state, module_test, "describe('describeFinalMessageSlip'", label=label)
    require_text(
        state,
        module_test,
        "is an exact string test on the six markers, not a judgment of the text",
        label=label,
    )
    for name in (
        "puts the first notice to the model as the next user message and continues the run",
        "numbers the second consecutive notice by the slip it answers",
        "ends the run on the third consecutive slip as error_slipped_final_message",
        "reads a turn with no visible text the same way, in the empty wording",
        "takes a clean message as the final answer without a notice, whatever it says",
        "starts the count again after a turn that called a tool",
        "reads the whole message, so markup split across streamed chunks is the same slip as markup delivered in one piece",
        "does not read a generation stopped from outside for a slip: the cut is the terminal",
        "counts every drain-slip generation through the third-slip terminal in one interaction",
        "refuses the fourth generation when drain-slip notices exhaust the turn budget",
    ):
        require_text(state, cli_test, name, label=label)
    for name in (
        "puts the first notice to the model and takes the next clean round as the report",
        "ends the run on the third consecutive slip, and the parent is told what happened",
        "treats three silent rounds the same way, and says the subagent produced no report",
        "starts the count again after a round that called a tool",
    ):
        require_text(state, agent_core_test, name, label=label)
    require_text(
        state,
        subagent_result_test,
        "[final-message-slip] tells the parent a subagent ended three turns without a final answer, naming the shape",
        label=label,
    )
    require_text(
        state,
        adapter_test,
        "[AgentTerminateMode.SLIPPED_FINAL_MESSAGE, 'error_slipped_final_message'],",
        label=label,
    )


def _validate_terminal_state_before(state: State) -> None:
    label = "terminal-state precondition"
    types = "packages/cli/src/nonInteractive/types.ts"
    adapter = "packages/cli/src/nonInteractive/io/BaseJsonOutputAdapter.ts"
    cli = "packages/cli/src/nonInteractiveCli.ts"
    errors = "packages/cli/src/utils/errors.ts"
    agent_types = "packages/core/src/agents/runtime/agent-types.ts"
    client = "packages/core/src/core/client.ts"
    helpers = "packages/cli/src/utils/nonInteractiveHelpers.ts"

    # The wire vocabulary is a hand-written list beside the record, with no
    # table deciding which state carries which name.
    require_text(
        state,
        types,
        "subtype: 'error_max_turns' | 'error_during_execution';",
        label=label,
    )
    forbid_text(state, types, "TERMINAL_RESULT_BY_STATE", label=label)

    # A free-form subtype with a default is what lets a state be reported
    # under a neighbouring state's name.
    require_text(state, adapter, "readonly subtype?: string;", label=label)
    require_text(state, adapter, "          'error_during_execution',", label=label)
    require_text(state, adapter, "?? 'success',", label=label)
    # Every subagent that stops is reported under one hard-coded name.
    require_text(state, adapter, "subtype: 'error_during_execution',", label=label)

    # Two terminal paths leave the process without emitting a record at all.
    for helper in (
        "export async function handleCancellationError(",
        "export async function handleMaxTurnsExceededError(",
        "export async function handleBudgetExceededError(",
    ):
        require_text(state, errors, helper, label=label)

    # The turn budget is counted twice, checked by two spellings of one
    # predicate, and charged to the turn before the turn is admitted.
    require_text(state, cli, "let limitedTurnCount = 0;", label=label)
    require_text(
        state,
        cli,
        "if (maxSessionTurns >= 0 && limitedTurnCount > maxSessionTurns) {",
        label=label,
    )
    require_text(
        state,
        client,
        "this.config.getMaxSessionTurns() > 0 &&",
        label=label,
    )

    # The subagent's own enumeration exists but is discarded before the wire.
    require_text(state, agent_types, "export enum AgentTerminateMode {", label=label)
    forbid_text(state, agent_types, "MAX_TOOL_CALLS", label=label)
    forbid_text(state, helpers, "terminateMode", label=label)


_TERMINAL_ENUM = re.compile(r"\n  ([A-Z_]+) = '([A-Z_]+)',")
_NAMED = re.compile(
    r"\n  \[AgentTerminateMode\.([A-Z_]+)\]: terminalResult\(\s*'([a-z_]+)',?\s*\),"
)
# The two shapes that put a terminal state into a value: an ending carrying it
# as `terminateMode`, and the table naming the state each run budget's overrun
# ends the run in. A comparison against a state is not one of these -- reading a
# state is not producing it, and the distinction is the whole point.
_STATE_PRODUCER = re.compile(r"terminateMode\s*[:=]\s*AgentTerminateMode\.([A-Z_]+)")
_BUDGET_STATE_TABLE = re.compile(
    r"const BUDGET_STATE: Record<BudgetKind, AgentTerminateMode> = \{\n"
    r"((?:  '[a-z-]+': AgentTerminateMode\.[A-Z_]+,\n)+)\};"
)


def _validate_terminal_state_after(state: State) -> None:
    writer_path = "packages/cli/src/utils/output-writer.ts"
    _require_all(
        state,
        writer_path,
        (
            "this.failure ??= { error };",
            "this.pending++;",
            "this.pending--;",
            "while (this.pending > 0 && !this.failure)",
            "if (this.pending === 0 || this.failure)",
        ),
        label="callback-owned output settlement",
    )
    require_text(
        state,
        writer_path,
        "if (this.failure) throw this.failure.error;",
        count=2,
        label="falsy output failure retention",
    )
    _require_all(
        state,
        "packages/cli/src/nonInteractive/io/StreamJsonOutputAdapter.ts",
        (
            "this.writer = outputWriter(outputStream ?? process.stdout);",
            "return this.writer.flush();",
        ),
        label="adapter joins its own output writer",
    )
    label = "terminal-state result"
    types = "packages/cli/src/nonInteractive/types.ts"
    types_test = "packages/cli/src/nonInteractive/io/BaseJsonOutputAdapter.test.ts"
    adapter = "packages/cli/src/nonInteractive/io/BaseJsonOutputAdapter.ts"
    cli = "packages/cli/src/nonInteractiveCli.ts"
    cli_test = "packages/cli/src/nonInteractiveCli.test.ts"
    bootstrap = "packages/cli/src/cli.ts"
    bootstrap_test = "packages/cli/src/cli.test.ts"
    errors = "packages/cli/src/utils/errors.ts"
    agent_types = "packages/core/src/agents/runtime/agent-types.ts"
    agent = "packages/core/src/tools/agent/agent.ts"
    budget = "packages/core/src/config/session-turn-budget.ts"
    budget_test = "packages/core/src/config/session-turn-budget.test.ts"
    client = "packages/core/src/core/client.ts"
    helpers = "packages/cli/src/utils/nonInteractiveHelpers.ts"
    helpers_test = "packages/cli/src/utils/nonInteractiveHelpers.test.ts"

    # ── The closed set, asserted in every direction ──────────────────────
    #
    # A declared name with no producer is byte-identical, type-checks, parses
    # and builds; nothing else in this pipeline can see it. The sets below
    # are read out of the post-patch source and required to agree: the states
    # an agentic run can end in, the states some statement in the tree
    # constructs, and the states the table names. The names the protocol
    # declares are the stream contract's terminal table, which the client
    # reads from its generated binding: the TypeScript build refuses a name the
    # table does not define, and one it defines that no state produces.
    #
    # The assertion is production, not reachability. Whether a given
    # deployment's argv can reach a producing statement is a path-sensitive
    # property of a call graph that no text-level check can decide, and the
    # state set is instead kept equal to the states this build can reach, so
    # the two coincide -- a property a reader checks by eye on a nine-member
    # enum.
    enum_source = _source(state, agent_types, label=label)
    enum_body = enum_source.split("export enum AgentTerminateMode {", 1)[1].split(
        "\n}", 1
    )[0]
    states = _TERMINAL_ENUM.findall(enum_body)
    _require(states, f"{label}: no terminal states declared in {agent_types}")
    for name, value in states:
        _require(
            name == value,
            f"{label}: {agent_types} state {name!r} does not spell itself {value!r}",
        )
    state_names = {name for name, _ in states}

    types_source = _require_all(
        state,
        types,
        (
            "import {\n  AgentTerminateMode,\n  TERMINAL_OUTCOMES,\n} from '@qwen-code/qwen-code-core';",
            "type TerminalOutcomes = typeof TERMINAL_OUTCOMES;",
            "export type TerminalSubtype = keyof TerminalOutcomes;",
            "  subtype: TerminalSubtypeWhere<false>;",
            "  subtype: TerminalSubtypeWhere<true>;",
            "  [S in TerminalSubtype]: { readonly subtype: S } & TerminalOutcomes[S];",
            "  return { subtype, ...TERMINAL_OUTCOMES[subtype] };",
            "type RequireNever<T extends never> = T;",
            "export type TerminalSubtypeWithoutAState = RequireNever<\n"
            "  Exclude<\n"
            "    TerminalSubtype,\n"
            "    (typeof TERMINAL_RESULT_BY_STATE)[AgentTerminateMode]['subtype']\n"
            "  >\n"
            ">;",
        ),
        label=label,
    )
    # Neither the vocabulary, nor whether a name is an error, nor the exit code
    # it leaves is restated in the client: all three are the contract's.
    for retired in (
        "  type: 'result';\n  subtype: 'success';",
        "  subtype:\n    | 'error_",
        "    isError: true,\n",
        "    isError: false,\n",
        "    exitCode: ",
        "readonly isError: false;",
        "readonly isError: true;",
    ):
        forbid_text(state, types, retired, label=label)

    named = _NAMED.findall(
        types_source.split("export const TERMINAL_RESULT_BY_STATE = {", 1)[1].split(
            "} as const satisfies Record<AgentTerminateMode, TerminalResult>;", 1
        )[0]
    )
    _require(named, f"{label}: {types} maps no state to a terminal record")
    named_states = {entry[0] for entry in named}
    _require(
        len(named_states) == len(named),
        f"{label}: {types} gives one state more than one terminal record",
    )
    _require(
        named_states == state_names,
        f"{label}: the terminal-record table is not total over the states a run "
        f"can end in; states without a record: {sorted(state_names - named_states)!r}, "
        f"records without a state: {sorted(named_states - state_names)!r}",
    )
    names = {name for _, name in named}
    _require(
        len(names) == len(named),
        f"{label}: {types} reports two states under one name",
    )
    success_names = {name for state_name, name in named if state_name == "GOAL"}

    # The producer half: a state earns its place in the enum -- and therefore
    # its wire name -- when some statement in the tree constructs it. The scan
    # covers every source file this patch touches, excluding the tests and the
    # mapping table itself, so a state left in the enum with nothing to reach
    # it is refused before a byte is written.
    produced_states: set[str] = set()
    for path, source in state.items():
        if not path.endswith((".ts", ".tsx")) or path.endswith(
            (".test.ts", ".test.tsx")
        ):
            continue
        if path == types:
            continue
        produced_states.update(_STATE_PRODUCER.findall(source))
        for rows in _BUDGET_STATE_TABLE.findall(source):
            produced_states.update(re.findall(r"AgentTerminateMode\.([A-Z_]+)", rows))
    _require(
        produced_states == state_names,
        f"{label}: the states a run can end in and the states this build "
        f"constructs differ; declared with no producing statement: "
        f"{sorted(state_names - produced_states)!r}, constructed but not "
        f"declared: {sorted(produced_states - state_names)!r}",
    )

    # The table is the only producer: no terminal name is written as a literal
    # where a record is built, and no cast reintroduces one.
    for path in (adapter, cli, helpers):
        for name in sorted(names - success_names):
            forbid_text(state, path, f"'{name}'", label=label)
        forbid_text(state, path, "CLIResultMessageError['subtype']", label=label)
        forbid_text(state, path, "CLIResultMessageSuccess['subtype']", label=label)
    require_text(
        state, adapter, "TERMINAL_RESULT_BY_STATE[options.terminateMode]", label=label
    )
    require_text(state, adapter, "TERMINAL_RESULT_BY_STATE[terminateMode]", label=label)
    require_text(state, adapter, "subtype: terminal.subtype,", count=3, label=label)
    forbid_text(state, adapter, "readonly subtype?: string;", label=label)
    forbid_text(state, adapter, "readonly isError: boolean;", label=label)

    # ── One emitter, and no terminal path that leaves without speaking ───
    for helper in (
        "handleCancellationError",
        "handleMaxTurnsExceededError",
        "handleBudgetExceededError",
    ):
        forbid_text(state, errors, helper, label=label)
        forbid_text(state, cli, helper, label=label)
    forbid_text(state, errors, "process.exit", label=label)
    # SessionEnded is the one shape a terminal state is raised in. A second
    # marker class carrying an ending's properties beside it is a second
    # vocabulary for the same thing, which is how the two drift apart.
    for path in (errors, cli, bootstrap):
        forbid_text(state, path, "AlreadyReportedError", label=label)
    cli_source = _require_all(
        state,
        cli,
        (
            "interface SessionEnding {",
            "class SessionEnded extends Error {",
            "const finish = async (ending: SessionEnding): Promise<number> => {",
            "const terminal = TERMINAL_RESULT_BY_STATE[ending.terminateMode];",
            "const exitCode = ending.exitCode ?? terminal.exitCode;",
        ),
        label=label,
    )
    _require(
        cli_source.count("await emitResult({") == 1,
        f"{label}: {cli} emits a terminal record from somewhere other than its "
        "single emitter",
    )
    _require(
        cli_source.count("return finish({")
        + cli_source.count("return finish(error.ending)")
        >= 10,
        f"{label}: {cli} has fewer terminal paths returning a state than the "
        "run has endings",
    )

    # ── The budget: one counter's question, asked before the turn ────────
    _require_all(
        state,
        budget,
        (
            "export function validateMaxSessionTurns(",
            "export function sessionTurnBudgetReached(",
            "export function describeSessionTurnBudget(",
            "return maxSessionTurns >= 1 && startedBudgetedTurns >= maxSessionTurns;",
        ),
        label=label,
    )
    require_text(state, cli, "sessionTurnBudgetReached(", label=label)
    require_text(state, client, "sessionTurnBudgetReached(", count=2, label=label)
    for path in (cli, client):
        forbid_text(state, path, "getMaxSessionTurns() > 0", label=label)
    forbid_text(state, cli, "limitedTurnCount", label=label)
    forbid_text(state, cli, "budgetedTurnCount", label=label)
    _require(
        len(re.findall(r"admitBudgetedTurn\(\);\s+turnCount\+\+;", cli_source)) == 2,
        f"{label}: both reasoning loops must admit before charging their turn",
    )
    # Core's own crossing of the same bound is read on the headless path
    # rather than falling through the adapter's default case.
    _require(
        cli_source.count("if (event.type === GeminiEventType.MaxSessionTurns) {") == 2,
        f"{label}: {cli} does not read the session-turn-budget event in both "
        "of its reasoning loops",
    )

    # ── A subagent's state survives to its own record ────────────────────
    require_text(
        state,
        "packages/core/src/tools/tools.ts",
        "terminateMode?: AgentTerminateMode;",
        label=label,
    )
    require_text(state, agent_types, "MAX_TOOL_CALLS = 'MAX_TOOL_CALLS',", label=label)
    _require(
        _source(state, agent, label=label).count("terminateMode:") >= 5,
        f"{label}: {agent} publishes a terminal display that names no state",
    )
    _require_ordered(
        _source(state, helpers, label=label),
        (
            "adapter.emitSubagentErrorResult(",
            "stoppedSubagentState(taskDisplay.terminateMode),",
        ),
        label=label,
        location=helpers,
    )
    require_text(state, types, "export function stoppedSubagentState(", label=label)
    # A scope whose first observed update already reports it stopped still
    # owes its scope a record.
    forbid_text(state, helpers, "previousStatus !== undefined", label=label)

    # ── Executed in the build ────────────────────────────────────────────
    for case in (
        "names a scope that stopped in %s as %s",
        "reports %s with the error flag and exit code the contract gives its name",
        "names every subtype the contract defines as the ending of some state",
    ):
        require_text(state, types_test, case, label=label)
    require_text(
        state,
        cli_test,
        "ends a run that reached the session turn budget as error_max_turns",
        label=label,
    )
    require_text(
        state,
        cli_test,
        "names each run budget on the record of the run it stopped",
        label=label,
    )
    require_text(
        state,
        cli_test,
        "charges a runtime Goal continuation to the session turn budget",
        label=label,
    )
    require_text(
        state,
        helpers_test,
        "reports a scope whose first update already reports it stopped",
        label=label,
    )
    require_text(
        state,
        budget_test,
        "admits exactly as many budgeted turns as the budget names",
        label=label,
    )
    # The build derives the tests it runs from the files this patch touches,
    # so the bootstrap suite covers the one classified shape a run can throw
    # past its emitter. It locates the sources it asserts on from its own
    # file: a path resolved against the working directory reads nothing under
    # the root config the build runs, and a suite that cannot read what it
    # asserts on cannot refuse anything.
    require_text(
        state,
        bootstrap_test,
        "path.dirname(fileURLToPath(import.meta.url))",
        label=label,
    )
    for cwd_relative in ("readFileSync('", "copyFileSync('"):
        forbid_text(state, bootstrap_test, cwd_relative, label=label)
    _require_all(
        state,
        "packages/cli/src/utils/output-writer.ts",
        (
            "new WeakMap<NodeJS.WritableStream, OutputWriter>()",
            "this.stream.write(bytes, complete)",
            "while (this.pending > 0 && !this.failure)",
            "await writer.flush()",
        ),
        label=label,
    )
    _require_ordered(
        cli_source,
        (
            "adapter.emitResult(result)",
            "await adapter.flush()",
            "options.onResultEmitted?.()",
        ),
        label=label,
        location=cli,
    )
    _require_all(
        state,
        "packages/cli/src/nonInteractive/session.ts",
        (
            "private async runTurn(",
            "let resultDelivered = false",
            "if (resultDelivered)",
            "'turn_cleanup_failed'",
            "this.shutdownPromise ??= runCleanupSteps",
        ),
        label=label,
    )


# Every tool that spilled its own output to disk, or declared its own
# character budget for the scheduler to spill it at, and every layer that
# bounded a batch of results in characters.
_CHARACTER_BOUND_SITES: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        "packages/core/src/core/coreToolScheduler.ts",
        (
            "GATE_HEADROOM",
            "GATE_EXEMPT_TOOLS",
            "maybePersistLargeToolResult",
            "applyBatchOutputBudget",
            "truncateLlmContent",
            "getTruncateToolOutputThreshold",
        ),
    ),
    (
        "packages/core/src/config/config.ts",
        (
            "DEFAULT_TRUNCATE_TOOL_OUTPUT_THRESHOLD",
            "DEFAULT_TOOL_OUTPUT_BATCH_BUDGET",
            "getTruncateToolOutputThreshold",
            "getToolOutputBatchBudget",
        ),
    ),
    (
        "packages/cli/src/config/settingsSchema.ts",
        ("truncateToolOutputThreshold", "toolOutputBatchBudget"),
    ),
    (
        "packages/cli/src/config/config.ts",
        ("truncateToolOutputThreshold", "toolOutputBatchBudget"),
    ),
    (
        "packages/core/src/utils/fileUtils.ts",
        ("getTruncateToolOutputThreshold", "getRangeReadByteLimit"),
    ),
    ("packages/core/src/tools/tools.ts", ("maxOutputChars", "truncateKeep")),
    ("packages/core/src/tools/shell.ts", ("maxOutputChars", "truncateToolOutput")),
    ("packages/core/src/tools/mcp-tool.ts", ("maxOutputChars", "truncateToolOutput")),
    ("packages/core/src/tools/grep.ts", ("maxOutputChars",)),
    ("packages/core/src/tools/ripGrep.ts", ("maxOutputChars",)),
    ("packages/core/src/tools/read-file.ts", ("maxOutputChars",)),
    ("packages/core/src/tools/read-mcp-resource.ts", ("maxOutputChars",)),
    ("packages/core/src/tools/enterPlanMode.ts", ("maxOutputChars",)),
    ("packages/core/src/tools/web-search.ts", ("maxOutputChars",)),
    ("packages/core/src/tools/agent/agent.ts", ("maxOutputChars", "truncateKeep")),
    ("packages/core/src/test-utils/mock-tool.ts", ("maxOutputChars", "truncateKeep")),
    ("packages/core/src/core/geminiChat.ts", ("enforceFunctionResponseBudget",)),
    ("packages/core/src/agents/runtime/agent-core.ts", ("finalizeToolResponses",)),
    ("packages/core/src/followup/speculation.ts", ("finalizeToolResponses",)),
    ("packages/cli/src/nonInteractiveCli.ts", ("finalizeToolResponses",)),
    ("packages/cli/src/ui/hooks/useGeminiStream.ts", ("finalizeToolResponses",)),
    ("packages/cli/src/acp-integration/session/Session.ts", ("finalizeToolResponses",)),
    ("packages/core/src/index.ts", ("finalizeToolResponses",)),
)


def _validate_tool_result_bound_before(state: State) -> None:
    label = "tool-result-bound precondition"
    chat = "packages/core/src/core/geminiChat.ts"
    truncation = "packages/core/src/utils/truncation.ts"
    finalizer = "packages/core/src/utils/tool-response-finalizer.ts"
    scheduler = "packages/core/src/core/coreToolScheduler.ts"
    config = "packages/core/src/config/config.ts"
    files = "packages/core/src/utils/fileUtils.ts"
    tools = "packages/core/src/tools/tools.ts"

    # Upstream bounds a tool result in characters, in six places that neither
    # agree with one another nor with any token the model will actually read.
    _require_all(
        state,
        truncation,
        (
            "export async function truncateToolOutput(",
            "export async function truncateLlmContent(",
            "export const TOOL_OUTPUT_TRUNCATED_PREFIX =",
            "export const COMBINED_PASS_TOLERANCE_FACTOR = 2;",
        ),
        label=label,
    )
    _require_all(
        state,
        finalizer,
        (
            "export function enforceFunctionResponseBudget(",
            "export async function finalizeToolResponses(",
            "function allocateTextBudget(",
        ),
        label=label,
    )
    _require_all(
        state,
        scheduler,
        ("const GATE_HEADROOM = 3000;", "private async applyBatchOutputBudget("),
        label=label,
    )
    _require_all(
        state,
        config,
        (
            "export const DEFAULT_TRUNCATE_TOOL_OUTPUT_THRESHOLD = 25_000;",
            "export const DEFAULT_TOOL_OUTPUT_BATCH_BUDGET = 200_000;",
        ),
        label=label,
    )
    require_text(
        state, tools, "  get maxOutputChars(): number | undefined {", label=label
    )
    require_text(
        state,
        files,
        "        const configCharLimit = config.getTruncateToolOutputThreshold();",
        label=label,
    )
    # Nothing measures what a batch costs the request it is about to join.
    forbid_text(state, chat, "boundPendingToolResults", label=label)
    forbid_text(state, truncation, "persistToolResult(", label=label)


# Modules whose only purpose was bounding a tool result by a rule of their
# own. They are deleted, so each is checked by its absence from the tree.
_RETIRED_BOUND_MODULES = (
    "packages/core/src/utils/truncation.ts",
    "packages/core/src/utils/truncation.test.ts",
    "packages/core/src/utils/tool-response-finalizer.ts",
    "packages/core/src/utils/tool-response-finalizer.test.ts",
    "packages/core/src/utils/tool-response-finalizer.integration.test.ts",
)

# Tools that produce text whose size they do not decide. None bounds it: each
# returns its whole output, laid out as upstream lays it out, and the one bound
# is taken where the model's copy is made.
_UNBOUNDED_PRODUCERS = (
    "packages/core/src/tools/shell.ts",
    "packages/core/src/tools/mcp-tool.ts",
    "packages/core/src/tools/read-mcp-resource.ts",
    "packages/core/src/tools/web-search.ts",
    "packages/core/src/tools/web-fetch.ts",
    "packages/core/src/tools/write-file.ts",
    "packages/core/src/tools/agent/agent.ts",
    "packages/core/src/tools/monitor.ts",
    "packages/core/src/tools/notebook-edit.ts",
    "packages/core/src/tools/todoWrite.ts",
)


def _validate_tool_result_bound_after(state: State) -> None:
    store_path = "packages/core/src/utils/session-artifacts.ts"
    store = _source(state, store_path, label="durable artifact publication")
    # Publication is durable and exclusive, and the capacity is charged
    # against what the store actually holds, under its writer lock. Reaching
    # the capacity is an outcome with a next action, so it is returned; a
    # corrupt entry, a lock another writer holds or a failed write is a
    # defect, and those are thrown.
    _require_ordered(
        store,
        (
            "const release = await lockfile.lock(directory,",
            "for (const entry of await fs.readdir(directory))",
            "used += stat.size;",
            "if (exists) {",
            "if (!Buffer.from(bytes).equals(await file.readFile()))",
            "await file.sync();",
            "await syncDirectoryAncestors(directory);",
            # Reaching the capacity returns what the store holds and what was
            # asked of it, and nothing between the test and the return can
            # throw instead.
            "    if (used + bytes.byteLength > SESSION_ARTIFACT_CAPACITY_BYTES) {\n"
            "      return {\n"
            "        retained: false,\n"
            "        heldBytes: used,\n"
            "        requestedBytes: bytes.byteLength,\n"
            "        capacityBytes: SESSION_ARTIFACT_CAPACITY_BYTES,\n"
            "      };\n"
            "    }",
            "const file = await fs.open(filepath, 'wx', 0o400);",
            "await file.writeFile(bytes);",
            "await file.sync();",
            "await syncDirectoryAncestors(directory);",
        ),
        label="durable artifact publication",
        location=store_path,
    )
    label = "tool-result-bound result"
    # The capacity is declared, stated once, and named as the policy it is.
    _require_all(
        state,
        store_path,
        (
            "export const SESSION_ARTIFACT_CAPACITY_BYTES = 500 * 1024 * 1024;",
            "It is a declared capacity, not a derivation",
            "export function sessionArtifactPath(",
            "export type SessionArtifactRetention =",
            "): Promise<SessionArtifactRetention> {",
            "stale: Number.MAX_SAFE_INTEGER",
            "return withCleanup(",
            "createHash('sha256')",
            "Session artifact integrity failure",
        ),
        label=label,
    )
    for retired in ("MAX_SESSION_ARTIFACT_BYTES", "Session artifact disk budget exhausted"):
        forbid_text(state, store_path, retired, label=label)
    for path in ("packages/core/src/utils/tool-output-cleanup.ts",):
        _require(
            path not in state, f"{label}: age-based artifact cleanup remains at {path}"
        )

    # ── One bound, where the model's copy is made ────────────────────────
    #
    # What the model reads of a call is one text: the output or the failure,
    # with every hook, rule and skill reminder that joined it. That text is
    # held to one inline block in the bytes of its NFC form, whole, once,
    # after everything has joined it. A text past it keeps its head, which is
    # cut before anything is retained, so retention decides nothing about it;
    # the whole is retained and one notice leads the head.
    seam_path = "packages/core/src/core/toolResultBound.ts"
    seam = _require_all(
        state,
        seam_path,
        (
            "export async function boundToolResponseParts(",
            "const maxBytes = config.getInlineBlockBytes();",
            "function withNestedTextJoined(",
            "export function assertToolResponsesBounded(",
            "  cutTailToTokenizerBytes,\n  cutToTokenizerBytes,\n  tokenizerText,\n} from './tokenLimits.js';",
            "import { middleCutContent, type OutputBound } from '../tools/tools.js';",
            "SESSION_ARTIFACT_CAPACITY_BYTES,",
        ),
        label=label,
    )
    _require_ordered(
        seam,
        (
            "async function boundedText(",
            "const filepath = sessionArtifactPath(config, bytes, 'txt');",
            "const widest = Math.max(",
            "if (widest > maxBytes) {",
            # Upstream's split: one fifth of the room to the start, and the
            # rest to the end, so what a result says last is what it keeps.
            "const room = maxBytes - widest;",
            "const head = cutToTokenizerBytes(whole.text, Math.floor(room / 5));",
            "const tail = cutTailToTokenizerBytes(whole.text, room - head.bytes);",
            "const retention = await persistSessionArtifact(config, bytes, 'txt');",
            "const firstCut = lineBreaks(head.text);",
            "tailFrom > 0 && whole.text[tailFrom - 1] === '\\n' ? tailLine - 1 : tailLine;",
            "middleCutContent(\n      head.text,",
            "? readBack(firstCut, lastCut - firstCut + 1)",
            "if (emitted.bytes > maxBytes) {",
            "async function boundedPart(",
            "const functionResponse = withNestedTextJoined(part.functionResponse);",
            "const whole = tokenizerText(text);",
            "if (whole.bytes <= maxBytes) {",
            "handedOn = await boundedText(config, maxBytes, text, whole);",
        ),
        label=label,
        location=seam_path,
    )
    # Nothing in the seam converts bytes into tokens or keeps a second store.
    for absent in ("inlineBlockTokenBound", "500 * 1024 * 1024", "normalize('NF", "boundedContent("):
        forbid_text(state, seam_path, absent, label=label)
    # The one notice's two placements live beside it: leading what a tool
    # returned the first part of, and standing in the cut of a result whose
    # middle was cut.
    _require_all(
        state,
        "packages/core/src/tools/tools.ts",
        (
            "export function middleCutContent(",
            "  return `${head}\\n\\n---\\n${MIDDLE_CUT_LEAD}${formatOutputBound(bound)}\\n---\\n\\n${tail}`;",
            "'The middle of this result is cut here: above is its start, below is its end. ';",
        ),
        label=label,
    )
    _require_all(
        state,
        "packages/core/src/core/tokenLimits.ts",
        (
            "export function cutTailToTokenizerBytes(",
            "if (tail.text !== cut) {",
            "if (tail.bytes > maxBytes) {",
        ),
        label=label,
    )

    # The scheduler finishes every call's copy after the batch hook has joined
    # it and before anything logs, records or delivers it. A copy that cannot
    # be finished fails that call with its cause: a throw here would be
    # swallowed by the completion notice and leave the batch unanswered.
    scheduler = "packages/core/src/core/coreToolScheduler.ts"
    scheduler_source = _source(state, scheduler, label=label)
    _require_ordered(
        scheduler_source,
        (
            "completedCalls = withPostToolBatchAdditionalContext(",
            "completedCalls = withPostToolBatchArtifacts(",
            "completedCalls = await this.finishModelCopies(completedCalls);",
            "logToolCall(this.config, new ToolCallEvent(call));",
            "this.recordToolResults(completedCalls);",
            "await this.onAllToolCallsComplete(completedCalls);",
        ),
        label=label,
        location=scheduler,
    )
    _require_ordered(
        scheduler_source,
        (
            "private async finishModelCopies(",
            "const responseParts = await boundToolResponseParts(",
            "The result of this call could not be delivered: ",
            "ToolErrorType.UNHANDLED_EXCEPTION,",
        ),
        label=label,
        location=scheduler,
    )
    # Every other runtime that executes a call itself finishes its copy the
    # same way.
    for path, needle, count in (
        ("packages/cli/src/acp-integration/session/Session.ts", "await boundToolResponseParts(", 2),
        ("packages/core/src/followup/speculation.ts", "const boundedResponses = await boundToolResponseParts(", 1),
        ("packages/core/src/agents/runtime/agent-core.ts", "const responseParts = await boundToolResponseParts(", 1),
    ):
        require_text(state, path, needle, count=count, label=label)

    # What a turn appends is held to what the bound makes true where it
    # arrives, and a result past it is refused as a defect of the path that
    # made it, not repaired: at the send, before the one count the send
    # takes, and where a speculated branch is adopted without passing the
    # send.
    chat = "packages/core/src/core/geminiChat.ts"
    chat_source = _source(state, chat, label=label)
    _require_ordered(
        chat_source,
        (
            "assertToolResponsesBounded(\n          userContent.parts ?? [],\n          partition.inlineBlockBytes,\n        );",
            "const effectiveTokens = await countExactRequestTokens(",
            "compressionInfo = await this.tryCompress(",
        ),
        label=label,
        location=chat,
    )
    # The fit's "one result a turn" rests on one call a turn. The client asks
    # for it itself: its request builder writes parallel_tool_calls: false
    # into every request, once, after provider and extra-body decoration, so
    # no setting carries it and none can replace it. extra_body is a closed
    # allowlist: the model configuration admits only the reasoning switches
    # this deployment sends -- reasoning_effort, and chat_template_kwargs with
    # the three keys the served template reads -- and refuses every other field
    # or template argument by name, the client's own fields among them. The backend's grammar stops after
    # the first call, and the client checks it where a turn is assembled: a
    # turn that carries more is refused whole, before any of it is recorded,
    # committed or run, as a defect of the serving side, naming the backend
    # and its grammar. It is never cut down to one call.
    pipeline = "packages/core/src/core/openaiContentGenerator/pipeline.ts"
    _require_ordered(
        _source(state, pipeline, label=label),
        (
            "let providerRequest = this.config.provider.buildRequest(",
            "    typed['parallel_tool_calls'] = false;\n"
            "    // Invocation ownership is authoritative after provider and extra-body decoration.\n"
            "    typed['kv_scope'] = request.generationContext.kvScope;\n"
            "    return providerRequest;",
        ),
        label=label,
        location=pipeline,
    )
    require_text(state, pipeline, "parallel_tool_calls", count=1, label=label)
    constants = "packages/core/src/core/openaiContentGenerator/constants.ts"
    content_generator = "packages/core/src/core/contentGenerator.ts"
    _require_all(
        state,
        constants,
        (
            "export const EXTRA_BODY_FIELDS = [\n"
            "  'reasoning_effort',\n"
            "  'chat_template_kwargs',\n"
            "] as const;",
            "export const CHAT_TEMPLATE_KWARGS_FIELDS = [\n"
            "  'enable_thinking',\n"
            "  'reasoning_effort',\n"
            "  'add_vision_id',\n"
            "] as const;",
        ),
        label=label,
    )
    _require_all(
        state,
        content_generator,
        (
            "import {\n"
            "  CHAT_TEMPLATE_KWARGS_FIELDS,\n"
            "  EXTRA_BODY_FIELDS,\n"
            "} from './openaiContentGenerator/constants.js';",
            "  const refused = Object.keys(config.extra_body ?? {})\n"
            "    .filter(\n"
            "      (field) => !(EXTRA_BODY_FIELDS as readonly string[]).includes(field),\n"
            "    )\n"
            "    .map((field) => `extra_body.${field}`);",
            "      refused.push('extra_body.chat_template_kwargs (not an object)');",
            "              !(CHAT_TEMPLATE_KWARGS_FIELDS as readonly string[]).includes(key),",
            "          .map((key) => `extra_body.chat_template_kwargs.${key}`),",
            "  if (refused.length > 0) {\n"
            "    errors.push(\n"
            "      new Error(\n"
            "        `extra_body carries ${refused.join(', ')}.",
        ),
        label=label,
    )
    # The denylist it replaces, which admitted any field the client did not
    # write, does not return beside it.
    for path in (constants, content_generator):
        forbid_text(state, path, "CLIENT_OWNED_REQUEST_FIELDS", label=label)
    forbid_text(
        state,
        "packages/core/src/core/contentGenerator.test.ts",
        "admits extra_body fields the client does not write",
        label=label,
    )
    for path, case in (
        ("packages/core/src/core/openaiContentGenerator/pipeline.test.ts",
         "asks every request for one call per turn, whatever decorates it"),
        ("packages/core/src/core/contentGenerator.test.ts",
         "refuses extra_body that carries a field the client writes: %s"),
        ("packages/core/src/core/contentGenerator.test.ts",
         "refuses extra_body that carries a field this deployment does not send: %s"),
        ("packages/core/src/core/contentGenerator.test.ts",
         "refuses a template argument the served template does not read: %s"),
        ("packages/core/src/core/contentGenerator.test.ts",
         "admits exactly the reasoning switches this deployment sends"),
    ):
        require_text(state, path, case, label=label)
    # One call a turn was the deployment's setting; it is the client's own.
    forbid_text(state, "packages/core/src/core/tool-call-count.ts",
                "the deployment's guarantee, not this client's choice", label=label)
    forbid_text(state, "packages/core/src/core/geminiChat.ts",
                "which the deployment decides and this client checks", label=label)
    count_path = "packages/core/src/core/tool-call-count.ts"
    _require_all(
        state,
        count_path,
        (
            "export class ToolCallCountDefectError extends Error {",
            "export function assertOneToolCallPerTurn(parts: readonly Part[]): void {",
            "if (calls.length <= 1) return;",
            "throw new ToolCallCountDefectError(",
            "\"parallel_tool_calls: false, and the vLLM backend's Qwen structural-tag \" +",
            "'model slip; none of these calls was run or added to the conversation. ' +",
        ),
        label=label,
    )
    for absent in ("calls.slice(0, 1)", "calls[0]", ".slice(0, 1)"):
        forbid_text(state, count_path, absent, label=label)
    _require_ordered(
        chat_source,
        (
            "if (streamError) throw streamError.error;",
            "assertOneToolCallPerTurn(consolidatedHistoryParts);",
            "await this.chatRecordingService.recordAssistantTurn({",
            "      this.history.push({",
            "const committedCalls = consolidatedHistoryParts.filter(",
        ),
        label=label,
        location=chat,
    )
    require_text(state, "packages/core/src/index.ts", "export * from './core/tool-call-count.js';", label=label)
    for path, case in (
        ("packages/core/src/core/geminiChat.test.ts", "refuses a turn that carries two tool calls, as the deployment defect it is"),
        ("packages/core/src/core/tool-call-count.test.ts", "refuses a turn with two calls as a deployment defect, naming the backend and its grammar"),
    ):
        require_text(state, path, case, label=label)
    client = _source(state, "packages/core/src/core/client.ts", label=label)
    adopt = client.split("async adoptHistory(contents: readonly Content[]): Promise<void> {", 1)
    _require(len(adopt) == 2, f"{label}: adoptHistory is missing from client.ts")
    _require_ordered(
        adopt[1],
        (
            "assertToolResponsesBounded(content.parts ?? [], maxBytes);",
            "await recorder.recordAdoptedMessage(snapshot);",
        ),
        label=label,
        location="packages/core/src/core/client.ts adoptHistory",
    )

    # The batch bound is gone: nothing measures, displaces or refuses a batch
    # of results, and no reference stands in for a result.
    for absent in (
        "boundPendingToolResults",
        "pendingToolResults",
        "referencePendingToolResult",
        "emptyPendingToolResults",
        "inlineBlockTokenBound",
        "issue fewer tool calls in one turn",
        "tool-response-finalizer",
        "this.getRequestHistoryForRoute(undefined, supportedModalities),",
    ):
        forbid_text(state, chat, absent, label=label)
    for path in _RETIRED_BOUND_MODULES:
        _require(path not in state, f"{label}: the retired bound module {path} remains")
    for path in ("packages/core/src/core/prompts.ts", "packages/core/src/core/__snapshots__/prompts.test.ts.snap"):
        forbid_text(state, path, "persisted-output", label=label)
    forbid_text(state, "packages/core/src/index.ts", "tool-response-finalizer", label=label)
    require_text(state, "packages/core/src/index.ts", "export * from './core/toolResultBound.js';", label=label)

    # Tools do not bound themselves. A producer asks the session for no
    # inline block and retains nothing of its own output; web_fetch retains
    # the original response bytes it fetched, which is what it reads a PDF
    # from, and says where they are.
    for path in _UNBOUNDED_PRODUCERS:
        forbid_text(state, path, "getInlineBlockBytes", label=label)
        for retired in ("boundedTokenizerText", "boundedEchoText", "boundedFailureText", "requireSessionConfig"):
            forbid_text(state, path, retired, label=label)
        if path != "packages/core/src/tools/web-fetch.ts":
            forbid_text(state, path, "persistSessionArtifact(", label=label)
    require_text(
        state, "packages/core/src/tools/web-fetch.ts", "persistSessionArtifact(", label=label
    )
    tools = "packages/core/src/tools/tools.ts"
    for retired in (
        "boundedTokenizerText",
        "boundedEchoText",
        "boundedFailureText",
        "partInTokenizerForm",
        "requireSessionConfig",
        "cut at this tool's limit",
        "refused?:",
    ):
        forbid_text(state, tools, retired, label=label)
    # A count cap the deployment did not choose is not a bound either: the
    # truncateToolOutputLines setting and its default are gone from every
    # layer that carried them.
    for path in (
        "packages/core/src/config/config.ts",
        "packages/cli/src/config/config.ts",
        "packages/cli/src/config/settingsSchema.ts",
        "packages/core/src/utils/fileUtils.ts",
        "packages/core/src/telemetry/types.ts",
        "packages/core/src/telemetry/loggers.ts",
        "packages/core/src/telemetry/qwen-logger/qwen-logger.ts",
    ):
        for retired in ("getTruncateToolOutputLines", "truncateToolOutputLines", "truncate_tool_output_lines"):
            forbid_text(state, path, retired, label=label)
    forbid_text(state, "packages/core/src/config/config.ts", "DEFAULT_TRUNCATE_TOOL_OUTPUT_LINES", label=label)

    # One bound on one quantity: every character bound this replaces is gone
    # from the tree, including the per-tool budgets and the batch's
    # character water-filling.
    for path, symbols in _CHARACTER_BOUND_SITES:
        for symbol in symbols:
            forbid_text(state, path, symbol, label=label)

    # Executed in the build.
    for path, case in (
        ("packages/core/src/core/toolResultBound.test.ts", "keeps the start and the end of a result past the bound, split as upstream split them, with the notice in the cut"),
        ("packages/core/src/core/toolResultBound.test.ts", "reads back from the line the start was cut in through the line the end begins in"),
        ("packages/core/src/core/toolResultBound.test.ts", "never splits a multi-byte code point at either boundary"),
        ("packages/core/src/core/tokenLimits.test.ts", "keeps the end of the normalized text, cut on a code-point boundary within the bound"),
        ("packages/core/src/core/tokenLimits.test.ts", "keeps the tail of what U+1D1C0 normalizes to, and that tail is normalized too"),
        ("packages/core/src/tools/tools.test.ts", "stands in the cut between the start and the end of a result cut in the middle"),
        ("packages/core/src/core/toolResultBound.test.ts", "bounds the joined text, not the tool output alone"),
        ("packages/core/src/core/toolResultBound.test.ts", "says the rest was not retained when the store is full, with what to do instead, and does not throw"),
        ("packages/core/src/core/toolResultBound.test.ts", "throws when the store fails its integrity check, because that is a defect and not a capacity"),
        ("packages/core/src/core/toolResultBound.test.ts", "joins text nested beside media to the response text, and bounds the joined text"),
        ("packages/core/src/core/toolResultBound.test.ts", "refuses a result that skipped the bound, naming the call, in the bytes the tokenizer reads"),
        ("packages/core/src/core/toolResultBound.test.ts", "keeps the start and the end of a long result and retains the whole"),
        ("packages/core/src/core/toolResultBound.test.ts", "bounds the text a hook joined to the output, not the output alone"),
        ("packages/core/src/core/toolResultBound.test.ts", "bounds the text a batch hook joined after every call finished"),
        ("packages/core/src/core/toolResultBound.test.ts", "fails a call whose copy cannot be finished, naming the cause, rather than hanging the batch"),
        ("packages/core/src/core/geminiChat.test.ts", "refuses a result that skipped the bound, naming the call, before anything is sent"),
        ("packages/core/src/core/geminiChat.test.ts", "counts a turn that appends a tool result like any other turn"),
        ("packages/core/src/core/geminiChat.test.ts", "takes no second count for a turn that appends no tool result"),
        ("packages/core/src/core/client.test.ts", "refuses an adopted tool result past one inline block before recording any of it"),
        ("packages/core/src/core/coreToolScheduler.test.ts", "joins the placeholder for an image too large to send to the result text"),
        ("packages/core/src/utils/session-artifacts.test.ts", "answers a full store with a value naming its capacity, charging existing bytes regardless of age"),
        ("packages/core/src/utils/session-artifacts.test.ts", "names the path an artifact is kept at before it is written"),
    ):
        require_text(state, path, case, label=label)


def _validate_served_accounting_before(state: State) -> None:
    label = "served-accounting precondition"
    converter = "packages/core/src/core/openaiContentGenerator/converter.ts"
    chat = "packages/core/src/core/geminiChat.ts"
    estimation = "packages/core/src/services/tokenEstimation.ts"
    resume = "packages/core/src/services/session-resume-token-counts.ts"
    parts = "packages/core/src/utils/partUtils.ts"
    base = "packages/core/src/core/baseLlmClient.ts"
    turn = "packages/core/src/core/turn.ts"
    tools = "packages/core/src/tools/tools.ts"
    adapter = "packages/cli/src/nonInteractive/io/BaseJsonOutputAdapter.ts"
    helpers = "packages/cli/src/utils/nonInteractiveHelpers.ts"
    wire = "packages/cli/src/nonInteractive/types.ts"

    # Upstream approximates what the backend serves: a reasoning count from
    # the reasoning text, clamped to the completion count, and a cached-prompt
    # count of zero whenever none was reported.
    _require_all(
        state,
        converter,
        (
            "estimateTextTokens(reasoningText ?? '')",
            "TOKEN_ESTIMATE_UNITS_PER_TOKEN",
            "reasoning_tokens absent; estimated",
            "extendedUsage.cached_tokens ??\n      0;",
        ),
        label=label,
    )
    # The previous-response output counter and the arithmetic behind it.
    require_text(state, chat, "private lastOutputTokenCount = 0;", label=label)
    require_text(
        state,
        estimation,
        "export function getUsageOutputTokenCountForPromptEstimate(",
        label=label,
    )
    require_text(
        state,
        resume,
        "getUsageOutputTokenCountForPromptEstimate(usage)",
        label=label,
    )
    # Nothing reads a side query's reasoning, a subagent's rounds, or the
    # reasoning share of a turn.
    forbid_text(state, converter, "readServedUsageDetails", label=label)
    forbid_text(state, parts, "getResponseThoughtText", label=label)
    forbid_text(state, base, "requireServedUsage", label=label)
    forbid_text(state, turn, "reasoning: string;", label=label)
    forbid_text(state, tools, "SubagentRoundRecord", label=label)
    forbid_text(state, adapter, "emitSubagentRound", label=label)
    forbid_text(state, helpers, "emittedRoundCounts", label=label)
    forbid_text(state, wire, "reasoning_output_tokens", label=label)


def _validate_served_accounting_after(state: State) -> None:
    usage_path = "packages/core/src/core/served-usage.ts"
    usage = _source(state, usage_path, label="complete served usage validation")
    fields = re.search(r"const SERVED_USAGE_FIELDS = \[(.*?)\] as const;", usage, re.S)
    _require(fields is not None, "served usage field declaration missing")
    _require(
        set(re.findall(r"'([^']+)'", fields[1]))
        == {
            "totalTokenCount",
            "promptTokenCount",
            "candidatesTokenCount",
            "thoughtsTokenCount",
            "cachedContentTokenCount",
        },
        "served usage must validate all five reported counts",
    )
    _require(
        re.search(
            r"export function isTokenCount\(value: unknown\): value is number\s*\{\s*"
            r"return Number\.isSafeInteger\(value\) && \(value as number\) >= 0;\s*\}",
            usage,
        )
        is not None,
        "served usage must refuse every non-integer or negative count",
    )
    _require_all(
        state,
        usage_path,
        (
            "const invalidField = SERVED_USAGE_FIELDS.find(",
            "!isTokenCount(",
            "usage !== null && typeof usage === 'object' ? counts[field] : undefined",
            "invalidField !== undefined ||",
            "invalid served usage (${invalidField ?? 'relationships'})",
        ),
        label="complete served usage validation",
    )
    _require_all(
        state,
        "packages/sdk-typescript/src/daemon/ui/usage.ts",
        (
            "import { isTokenCount } from '@qwen-code/qwen-code-core/servedUsage';",
            "!isTokenCount(usage[key]) || !isTokenCount(sum)",
        ),
        label="shared safe count validation for displayed usage",
    )
    logging_path = (
        "packages/core/src/core/loggingContentGenerator/loggingContentGenerator.ts"
    )
    logging = _source(state, logging_path, label="durable generation dispatch")
    _require_ordered(
        logging,
        (
            "await observation.begin();",
            "this.wrapped.generateContent(req, userPromptId)",
            "await observation.begin();",
            "this.wrapped.generateContentStream(",
        ),
        label="durable generation dispatch",
        location=logging_path,
    )
    config_path = "packages/core/src/config/config.ts"
    config = _source(state, config_path, label="initial recording owner")
    initialization = config.split("async initialize(options?", 1)[1].split(
        "async initializeWorkspace(", 1
    )[0]
    _require_ordered(
        initialization,
        (
            "await this.activateSessionWriter();",
            "await this.initializeInternal(options);",
        ),
        label="initial recording owner",
        location=config_path,
    )
    _require_all(
        state,
        config_path,
        (
            "private chatRecordingService: ChatRecordingService;",
            "getChatRecordingService(): ChatRecordingService {",
            "return this.initializeConfig(async () => {",
            "private async initializeConfig(start: () => Promise<void>)",
            "const initialization = this.initializeOnce(start);",
            "private async initializeOnce(start: () => Promise<void>)",
            "await start();",
        ),
        label="mandatory recorder and shared initialization lifetime",
    )
    workspace = config.split("async initializeWorkspace(", 1)[1].split(
        "private async initializeConfig(", 1
    )[0]
    _require(
        "return this.initializeConfig(() =>" in workspace
        and "this.initializeInternal({ ...options, skipGeminiInitialization: true })" in workspace
        and "activateSessionWriter" not in workspace,
        "workspace preparation must share initialization settlement without a session writer",
    )
    recorder_path = "packages/core/src/services/chatRecordingService.ts"
    recorder = _source(state, recorder_path, label="owned canonical append")
    _require_ordered(
        recorder,
        (
            "const lease = this.binding?.lease;",
            "if (!lease) throw new SessionWriterUnavailableError();",
            "await lease.appendJsonLine(record);",
        ),
        label="owned canonical append",
        location=recorder_path,
    )
    label = "served usage and durable observation result"
    core = "packages/core/src/"
    cli = "packages/cli/src/"
    _require_all(
        state,
        core + "core/served-usage.ts",
        (
            "export function requireServedUsage(",
            "Number.isSafeInteger(value)",
            "counts.totalTokenCount !==\n      counts.promptTokenCount + counts.candidatesTokenCount",
            "counts.thoughtsTokenCount > counts.candidatesTokenCount",
            "counts.cachedContentTokenCount > counts.promptTokenCount",
        ),
        label=label,
    )
    _require_all(
        state,
        core + "core/openaiContentGenerator/converter.ts",
        (
            "function readServedUsage(",
            "requireServedUsage(mapped, 'OpenAI usage')",
            "cachedContentTokenCount: usage.prompt_tokens_details?.cached_tokens",
            "thoughtsTokenCount: usage.completion_tokens_details?.reasoning_tokens",
        ),
        label=label,
    )
    for symbol in (
        "estimateTextTokens",
        "TOKEN_ESTIMATE_UNITS_PER_TOKEN",
        "reasoning_tokens absent",
    ):
        forbid_text(
            state,
            core + "core/openaiContentGenerator/converter.ts",
            symbol,
            label=label,
        )
    _require_all(
        state,
        core + "core/baseLlmClient.ts",
        (
            "GenerationTextFailure",
            "getResponseThoughtText",
            "requireServedUsage",
            "requestAttempts",
        ),
        label=label,
    )
    _require_all(
        state,
        core + "core/loggingContentGenerator/loggingContentGenerator.ts",
        (
            "createUsageObserver(",
            "begin: () => logApiDispatch(this.config, dispatch, recorder)",
            "await observation.begin()",
            "observation.finish",
            "requestSessionId",
        ),
        label=label,
    )
    observations = core + "telemetry/generation-observations.ts"
    _require_all(
        state,
        observations,
        (
            "export class GenerationObservations",
            "request_id",
            "kv_scope",
            "served_usage",
            "requireServedUsage",
            "EVENT_API_DISPATCH",
            "EVENT_API_USAGE",
        ),
        label=label,
    )
    _require_all(
        state,
        core + "telemetry/generation-usage.ts",
        (
            "export type { GenerationUsageSummary } from './generation-usage-types.js';",
            "unfinalizedRequests",
            "unreportedUsageRequests",
            "usageReports",
            "summarizeGenerationUsage",
            "requireGenerationUsage",
            "requireServedUsage(summary.usage",
        ),
        label=label,
    )
    _require_all(
        state,
        core + "telemetry/generation-usage-types.ts",
        ("export interface GenerationUsageSummary", "usage: GenerationUsageCounts | null;"),
        label=label,
    )
    _require_all(
        state,
        core + "telemetry/uiTelemetry.ts",
        (
            "new GenerationObservations()",
            "getGenerationUsage(",
            "replaceSessionEvents(",
        ),
        label=label,
    )
    _require_all(
        state,
        core + "services/recorded-generation-observations.ts",
        (
            "new GenerationObservations()",
            "getRecordedUiEvent(record)",
            "getRecordedGenerationObservations(",
            "return observations.values();",
        ),
        label=label,
    )
    for path in (
        "packages/cli/src/commands/review/cost-ledger.ts",
        "packages/cli/src/ui/utils/export/collect.ts",
    ):
        _require_all(
            state,
            path,
            ("getRecordedGenerationObservations", "summarizeGenerationUsage"),
            label=label,
        )
    _require_all(
        state,
        core + "agents/runtime/agent-statistics.ts",
        (
            "constructor(private readonly readOwnerUsage: () => GenerationUsageSummary)",
            "ownerUsage: this.readOwnerUsage()",
            "formatGenerationUsage(stats.ownerUsage)",
        ),
        label=label,
    )
    _require_all(
        state,
        core + "tools/agent/agent.ts",
        (
            "updates: Partial<Omit<AgentResultDisplay, 'executionSummary'>>",
            "this.currentDisplay = {\n      ...this.currentDisplay,\n      ...updates,\n"
            "      executionSummary: this.subagent?.getExecutionSummary(),\n    };",
        ),
        label=label,
    )
    forbid_text(state, core + "tools/agent/agent.ts", "isInteractive()", label=label)
    _require_all(
        state,
        core + "services/session-resume-usage.ts",
        ("requireServedUsage",),
        label=label,
    )
    for path in (core + "services/session-resume-token-counts.ts",):
        _require(
            path not in state,
            f"{label}: obsolete estimated usage module remains at {path}",
        )
    _require_all(
        state,
        cli + "nonInteractive/types.ts",
        (
            "export interface Usage",
            "export type { GenerationUsageSummary } from '@qwen-code/qwen-code-core/generationUsage';",
            "usage: GenerationUsageSummary | null;",
            "reasoning_output_tokens: number",
        ),
        label=label,
    )
    _require_all(
        state,
        cli + "ui/status-line-data.ts",
        (
            "runOutsideAgentContext",
            "uiTelemetryService.getGenerationUsage(stats.sessionId)",
        ),
        label=label,
    )
    _require_all(
        state,
        core + "agents/agent-transcript.ts",
        (
            "ChatCommitRecorder",
            "generation_failure",
            "recordChatCompression",
            "flush",
        ),
        label=label,
    )
    _require_all(
        state,
        cli + "utils/nonInteractiveHelpers.ts",
        (
            "adapter.emitSubagentRound(round, agentToolCallId)",
            "emittedRoundCounts",
        ),
        label=label,
    )
    _require_all(
        state,
        core + "config/config.ts",
        (
            "await this.activateSessionWriter()",
            "private async activateChatRecording()",
            "recorder.activate(",
            "async startNewSession(",
        ),
        label=label,
    )
    _require_all(
        state,
        core + "services/sessionService.ts",
        (
            "private async withMutationLease<T>",
            "await jsonl.readStrict<ChatRecord>(filePath)",
        ),
        label=label,
    )
    for path in (core + "config/config.ts", core + "services/chatRecordingService.ts"):
        for symbol in ("sessionWriterLeaseEnabled", "writerLeaseRequired"):
            forbid_text(state, path, symbol, label=label)
    # The lease is mandatory here, so the election it runs is this patch's to
    # get right. Upstream's election can leave a sealed session with no
    # writer: a contender links its record into the primary path in the gap
    # after another has renamed the sealed primary away, the other's link
    # fails with EEXIST, the contender sees the other's claim and removes its
    # record, and the inspection that follows finds the path missing -- which
    # upstream treats as unavailable, so both lose. An EEXIST link whose path
    # is missing by the time it is inspected is that removed record, and the
    # link is tried again under the same claim, within the same budget; a
    # missing path after any other failure still fails.
    lease = core + "services/session-writer-lease.ts"
    link = _source(state, lease, label=label).split(
        "async function linkClaimedPrimary(", 1
    )[1].split("\nasync function removeClaimedPrimary(", 1)[0]
    _require_ordered(
        link,
        (
            "await assertExactTransitionClaim(claimPath, claimRaw);",
            "await fs.link(sourcePath, lockPath);",
            "state === 'missing' &&\n        (error as NodeJS.ErrnoException).code === 'EEXIST'",
            "await waitForClaimedPrimaryCandidate(++candidateWaitAttempts);\n        continue;",
            "if (state === 'other') throw new SessionWriterLostError();",
            "throw new SessionWriterUnavailableError({",
        ),
        label=label,
        location=lease,
    )
    for case in (
        "takes over a sealed session when a racing candidate leaves the path between the failed link and the inspection",
        "seals a session when a racing candidate leaves the path between the failed link and the inspection",
        "fails closed when the primary link fails for any other reason, even with the path free",
        "expected exactly one certified replacement; the contenders answered",
    ):
        require_text(state, core + "services/session-writer-lease.test.ts", case, label=label)


def _validate_manual_compaction_before(state: State) -> None:
    label = "manual compaction precondition"
    _require_all(state, "packages/cli/src/ui/commands/compressCommand.ts", (
        "context.executionMode ?? 'interactive'",
        "MAX_COMPRESS_INSTRUCTIONS_CHARS",
        "originalTokenCount: null",
    ), label=label)
    require_text(state, "packages/cli/src/ui/hooks/slashCommandProcessor.ts",
                 "Promise.race", label=label)
    require_text(state, "packages/cli/src/ui/components/messages/CompressionMessage.tsx",
                 "originalTokenCount ?? 0", label=label)


def _validate_manual_compaction_after(state: State) -> None:
    label = "manual compaction ownership and outcome"
    cli = "packages/cli/src/"
    core = "packages/core/src/"
    for command in ("compressCommand.ts", "compressFastCommand.ts"):
        path = cli + "ui/commands/" + command
        require_text(state, path, "runCompressionCommand(", label=label)
        for absent in ("executionMode", "setPendingItem", "MAX_COMPRESS_INSTRUCTIONS_CHARS"):
            forbid_text(state, path, absent, label=label)
    outcome_path = cli + "utils/compression-result.ts"
    outcome = _require_all(state, outcome_path, (
        "Number.isSafeInteger(count)", "const unhandled: never = status;",
        "createCompressionHistoryItem(", "text: formatCompressionResult(compression).content",
        "error instanceof CompactionFinalizationError", "finalizationFailure: error.message",
    ), label=label)
    forbid_text(state, outcome_path, "?? 0", label=label)
    turn = _source(state, core + "core/turn.ts", label=label)
    statuses = set(re.findall(r"^\s+(COMPRESSED|NOOP|COMPRESSION_FAILED_[A-Z_]+)\b", turn, re.M))
    _require("COMPRESSION_FAILED_INSUFFICIENT_ROOM" in statuses, label + ": missing room rejection")
    for status in statuses:
        _require("case CompressionStatus." + status + ":" in outcome,
                 label + ": missing presentation for " + status)
    processor = _source(state, cli + "ui/hooks/slashCommandProcessor.ts", label=label)
    cancel = processor.split("const cancelSlashCommand =", 1)[1].split("useKeypress(", 1)[0]
    _require("controller.abort()" in cancel and "setIsProcessing(false)" not in cancel,
             label + ": cancellation releases the owner")
    action = processor.split("const handleSlashCommand =", 1)[1]
    _require("Promise.race" not in action, label + ": action settlement is raced")
    _require_ordered(action, (
        "await commandToExecute.action(", "case 'compression':",
        "createCompressionHistoryItem(outcome)", "await chatRecorder.flush()",
        "setIsProcessing(false)",
    ), label=label, location="terminal command owner")
    _require_all(state, cli + "nonInteractiveCliCommands.ts", (
        "abortSignal: abortController.signal", "case 'compression':",
    ), label=label)
    headless = _source(state, cli + "nonInteractiveCli.ts", label=label)
    manual = headless.split("case 'compression':", 1)[1].split("case 'submit_prompt':", 1)[0]
    _require_ordered(manual, ("adapter.emitSystemMessage(", "'compaction'",
        "toCompactionRecord(slashCommandResult.info)", "emitFinalAssistantMessage"),
        label=label, location="headless command event")
    _require_all(state, cli + "acp-integration/session/Session.ts", (
        "case 'compression':", "createCompressionHistoryItem(outcome)", "await recorder.flush()",
    ), label=label)
    _require_all(state, core + "core/client.ts", (
        "private async runCompaction(", "this.compactionFinalizationError = error",
        "{ cause: this.compactionFinalizationError }",
    ), label=label)
    chat = _source(state, core + "core/geminiChat.ts", label=label)
    fast = chat.split("async compressFast()", 1)[1].split("setSystemInstruction(", 1)[0]
    _require_ordered(fast, ("recordChatCompression(", "chatRecordingService.flush()",
        "this.setHistory(newHistory)", "throw new CompactionFinalizationError(info, error)"),
        label=label, location="fast durable checkpoint")
    recorder = _source(state, core + "services/chatRecordingService.ts", label=label)
    admission = recorder.split("recordSlashCommand(payload:", 1)[1].split("async recordAdoptedMessage", 1)[0]
    _require("SessionWriterUnavailableError" in admission and "this.enterWriteFailure(error, this.getSessionId())" in admission,
             label + ": command admission failure is not owned by the recorder")
    forbid_text(state, core + "services/chatCompressionService.ts", "MAX_HOOK_INSTRUCTIONS_CHARS", label=label)


def _validate_correctable_repetition_before(state: State) -> None:
    label = "repetition-halt precondition"
    detector = "packages/core/src/services/loopDetectionService.ts"
    # The guard has one answer: the streak reaches the threshold and the run
    # ends. Nothing goes back to the model, so a slip that is one wrong
    # argument away from correct is indistinguishable from a model that cannot
    # be told anything.
    require_text(
        state,
        detector,
        "    if (this.toolCallRepetitionCount >= TOOL_CALL_LOOP_THRESHOLD) {\n"
        "      this.lastLoopType = LoopType.CONSECUTIVE_IDENTICAL_TOOL_CALLS;",
        label=label,
    )
    forbid_text(state, detector, "takeToolCallRefusal", label=label)
    forbid_text(state, "packages/core/src/core/turn.ts",
                "ToolCallRepetitionRefusal", label=label)
    forbid_text(state, "packages/core/src/core/coreToolScheduler.ts",
                "repetitionRefusal", label=label)


def _validate_correctable_repetition_after(state: State) -> None:
    label = "a fired repetition rule is correctable"
    core = "packages/core/src/"
    detector = core + "services/loopDetectionService.ts"

    # The halt fires exactly where it always did, on the same comparison, and
    # the correction is delivered on the call before it -- inside the existing
    # tolerance, so the ceiling of identical calls does not move. Only the halt
    # logs a detected loop; a refusal did not end the run and must not be
    # recorded as though it had.
    require_text(
        state,
        detector,
        "const TOOL_CALL_LOOP_CORRECTION_POINT = TOOL_CALL_LOOP_THRESHOLD - 1;",
        label=label,
    )
    rule = _source(state, detector, label=label)
    body = rule.split("private checkToolCallLoop(", 1)[1].split(
        "takeToolCallRefusal(", 1
    )[0]
    _require_ordered(body, (
        "if (this.toolCallRepetitionCount >= TOOL_CALL_LOOP_THRESHOLD) {",
        "this.lastLoopType = LoopType.CONSECUTIVE_IDENTICAL_TOOL_CALLS;",
        "logLoopDetected(",
        "return true;",
        "if (this.toolCallRepetitionCount === TOOL_CALL_LOOP_CORRECTION_POINT) {",
        "this.pendingToolCallRefusal = {",
        "repetitionCount: this.toolCallRepetitionCount,",
        "droppedArgumentNames:",
    ), label=label, location="consecutive-identical guard")
    _require(
        "logLoopDetected(" not in body.split(
            "if (this.toolCallRepetitionCount === TOOL_CALL_LOOP_CORRECTION_POINT) {",
            1,
        )[1],
        label + ": the refusal is logged as a detected loop",
    )
    # Refusals are taken by the one caller that owns the request, never left
    # to expire silently.
    _require_all(state, detector, (
        "takeToolCallRefusal(): ToolCallRepetitionRefusal | null {",
        "this.pendingToolCallRefusal = null;",
    ), label=label)

    # The refusal states counted facts only: how many times, and which
    # arguments an earlier differing call to the same tool proved were
    # dropped. With no such call it claims nothing.
    _require_all(state, core + "core/turn.ts", (
        "export interface ToolCallRepetitionRefusal {",
        "  repetitionCount: number;",
        "  droppedArgumentNames: string[];",
        "  repetitionRefusal?: ToolCallRepetitionRefusal;",
        "export function repeatedToolCallMessage(",
    ), label=label)
    message = _source(state, core + "core/turn.ts", label=label)
    message = message.split("export function repeatedToolCallMessage(", 1)[1]
    _require_ordered(message, (
        "refusal.droppedArgumentNames.length > 0",
        "that differed passed",
        "Issuing this exact call again ends the run.",
    ), label=label, location="corrective message")

    # One delivery point. The interactive UI, the headless runner and a
    # subagent's own loop all schedule through it, so none of them carries a
    # policy of its own.
    scheduler = _source(state, core + "core/coreToolScheduler.ts", label=label)
    _require_ordered(scheduler, (
        "if (reqInfo.repetitionRefusal) {",
        "repeatedToolCallMessage(",
        "ToolErrorType.EXECUTION_DENIED,",
    ), label=label, location="scheduler refusal")

    # Both reasoning loops attach it to the request they are about to have
    # executed: the main session before it yields the call to its consumer,
    # the subagent when it turns the round's calls into scheduled requests.
    _require_all(state, core + "core/client.ts", (
        "const repetitionRefusal = this.loopDetector.takeToolCallRefusal();",
        "event.value.repetitionRefusal = repetitionRefusal;",
    ), label=label)
    _require_all(state, core + "agents/runtime/agent-core.ts", (
        "const repetitionRefusals = new Map<FunctionCall, ToolCallRepetitionRefusal>();",
        "repetitionRefusals.set(functionCall, refusal);",
        "const repetitionRefusal = repetitionRefusals.get(fc);",
        "...(repetitionRefusal ? { repetitionRefusal } : {}),",
    ), label=label)


def _validate_stream_admission_before(state: State) -> None:
    forbid_text(
        state, "packages/core/src/config/config.ts", "RuntimeContractAdmission",
        label="shared stream admission precondition",
    )


def _validate_stream_admission_after(state: State) -> None:
    label = "shared stream admission and public usage types"
    core = "packages/core/src/"
    _require_all(state, core + "config/config.ts", (
        "private readonly runtimeContractAdmission = new RuntimeContractAdmission();",
        "admitRuntimeStreamRecord(record: unknown)",
        "admitRuntimeControlRecord(record: unknown)",
        "this.assertRuntimeContractAvailable();",
    ), label=label)
    goal = _require_all(state, core + "goals/goal-runtime.ts", (
        "admission: RuntimeContractAdmission;",
        "options.admission.admitGoalSnapshot(payload.snapshot);",
        "return options.journal.recordGoalState(uuid, payload);",
    ), label=label)
    _require(goal.count("options.journal.recordGoalState(") == 1,
             f"{label}: Goal journal admission can be bypassed")
    _require_ordered(goal, (
        "options.admission.admitGoalSnapshot(payload.snapshot);",
        "return options.journal.recordGoalState(uuid, payload);",
    ), label=label, location="Goal journal admission")
    for name in ("StreamJsonOutputAdapter", "JsonOutputAdapter"):
        _require_all(state, f"packages/cli/src/nonInteractive/io/{name}.ts", (
            "this.config.admitRuntimeStreamRecord(message)",
        ), label=label)
    _require_all(state, core + "utils/runtime-stream-contract.ts", (
        "../generated/stream-record-validator.js",
        "../generated/goal-snapshot-validator.js",
        "../generated/outbound-control-validator.js",
    ), label=label)
    logging = _source(state, core + "core/loggingContentGenerator/loggingContentGenerator.ts", label=label)
    _require(logging.count("this.config.assertRuntimeContractAvailable();") == 4,
             f"{label}: both generation entrypoints must refuse retained contract failure before and after intent admission")
    for path in (core + "telemetry/generation-usage-types.ts",
                 "packages/acp-bridge/src/context-usage-types.ts"):
        contents = _source(state, path, label=label)
        _require("@google/genai" not in contents and "../core/" not in contents,
                 f"{label}: public wire types leak provider request internals")


# The unit a result is cut by, spelled the way a tool spells it. A cap named
# for what it bounds is the only thing that distinguishes cutting a result
# from slicing a display label, and the difference is what this concern is
# about: one is a fact withheld from the model, the other is cosmetics.
_OUTPUT_CAP = re.compile(
    r"\.slice\(\s*0,\s*[\w$.]*(?:[Ll]imit|[Mm]axResults|[Mm]axShown|[Cc]ap)\b"
)
_BOUND_HELPER = re.compile(r"\b(?:boundedContent|formatOutputBound)\(")

# Phrasings the tools used before there was one notice. They are forbidden by
# name so a tool cannot quietly grow its own vocabulary again: to say a result
# was cut you must call the helper, and the helper is the only thing that
# knows to state the bound, its unit, the total or why it is unknown, and the
# call that continues.
_RETIRED_NOTICES = (
    "truncated] ...",
    "Truncated by max_results",
    "Search did not complete",
    "No valid matches were returned",
    "Showing lines ",
    "Output exceeded the maximum captured size",
)

# Modules outside `src/tools` that put text in front of the model. The one
# notice belongs to them too, and `_tool_sources` cannot see them: naming them
# here is what keeps that rule enforced rather than merely true because
# whoever last edited them happened to write it correctly.
_MODEL_FACING_NON_TOOL_SOURCES = (
    "packages/core/src/services/monitorRegistry.ts",
    "packages/core/src/services/shellExecutionService.ts",
)



def _tool_sources(state: State) -> dict[str, str]:
    prefix = "packages/core/src/tools/"
    return {
        path: text
        for path, text in state.items()
        if path.startswith(prefix)
        and path.endswith(".ts")
        and not path.endswith(".test.ts")
        and "/" not in path[len(prefix):]
    }


def _validate_bounded_output_before(state: State) -> None:
    label = "bounded tool output precondition"
    # Before this concern each tool phrased its own notice, so the retired
    # wordings are exactly what the tree should still contain.
    forbid_text(
        state, "packages/core/src/tools/tools.ts", "formatOutputBound", label=label
    )
    forbid_text(
        state, "packages/core/src/tools/tools.ts", "OutputBound", label=label
    )
    forbid_text(
        state, "packages/core/src/lsp/types.ts", "LspBounded", label=label
    )
    # web-fetch cut its own text at a character cap it never declared, phrased
    # in its own words: a result short by a rule the model is never told.
    require_text(
        state,
        "packages/core/src/tools/web-fetch.ts",
        "const MAX_CONTENT_CHARS = 100_000;",
        label=label,
    )
    # Nothing upstream knows what one inline block may be, which is the
    # quantity the one bound on a tool result, and read_file's page, are
    # sized by.
    forbid_text(
        state,
        "packages/core/src/config/config.ts",
        "getInlineBlockBytes",
        label=label,
    )
    # Listings are cut to counts the caller never asked for, in the tools'
    # own words or in none.
    for path, cap in (
        ("packages/core/src/tools/glob.ts", "MAX_FILE_COUNT"),
        ("packages/core/src/tools/ls.ts", "MAX_ENTRY_COUNT"),
    ):
        _require_all(state, path, (cap,), label=label)
    # One layer below the tools, the monitor registry cuts the event line the
    # model actually reads and words that cut itself.
    require_text(
        state,
        "packages/core/src/services/monitorRegistry.ts",
        "'...[truncated]'",
        label=label,
    )
    # The shell service words its own capture-limit notice, one layer below the
    # tools and out of the one-notice detector's reach, and keeps the fact that
    # it truncated inside that prose rather than on the result.
    require_text(
        state,
        "packages/core/src/services/shellExecutionService.ts",
        "Output exceeded the maximum captured size",
        label=label,
    )
    forbid_text(
        state,
        "packages/core/src/services/shellExecutionService.ts",
        "outputCaptureLimitExceeded?:",
        label=label,
    )


def _validate_bounded_output_after(state: State) -> None:
    label = "bounded tool output result"
    tools_ts = "packages/core/src/tools/tools.ts"
    # One notice, and it carries every fact a cut result owes its reader: what
    # was asked for, what came back, the limit that cut it and its unit when a
    # limit did, the true total or why it is unknown, and the call that
    # continues -- or, when the rest was not kept, why not and what to do.
    _require_all(
        state,
        tools_ts,
        (
            "export type OutputBound = OutputBoundFacts &",
            "        limit: number;",
            "        limitUnit: string;",
            "        limit?: undefined;",
            "        limitUnit?: undefined;",
            "interface OutputBoundFacts {",
            "  requested?: number;",
            "  returned: number;",
            "  total: number | { readonly unknown: string };",
            "  continuation?: string | { readonly unretained: string };",
            "  coverageUnknown?: boolean;",
            "export function formatOutputBound(bound: OutputBound): string {",
            ": ` The rest was not retained: ${bound.continuation.unretained}`;",
            ": `, cut at a limit of ${bound.limit} ${bound.limitUnit}`;",
            "export function boundedContent(content: string, bound: OutputBound): string {",
            "  return `${formatOutputBound(bound)}\\n\\n---\\n\\n${content}`;",
        ),
        label=label,
    )
    # A notice that returned nothing is worded like any other, and none says
    # a tool's own limit cut a result: the limit it names is the one that did.
    for retired in (
        "refused?: boolean;",
        "bound.refused",
        "cut at this tool's limit",
        "export function requireSessionConfig<",
        "export function boundedTokenizerText(",
        "export function boundedEchoText(",
        "export function boundedFailureText(",
        "export function partInTokenizerForm(",
    ):
        forbid_text(state, tools_ts, retired, label=label)

    # `M` is a share of the served window, so the session is what knows it.
    # The window is the one place it is derived, and a provider that declares
    # none has no share to take and is refused instead of given a default.
    _require_all(
        state,
        "packages/core/src/config/config.ts",
        (
            "  getInlineBlockBytes(): number {",
            "return partitionContextWindow(contextWindowSize).inlineBlockBytes;",
            "The inline-block bound is a share of the served context window, and this provider declares none.",
        ),
        label=label,
    )

    sources = _tool_sources(state)
    _require(
        len(sources) > 20,
        f"{label}: expected the tool directory in the final state, found {len(sources)} files",
    )
    # Of the tools, only read_file asks for the inline block: it sizes a page
    # by it. Every other tool returns its whole result, and the one bound is
    # taken where the model's copy is made.
    askers = sorted(
        path for path, text in sources.items() if "getInlineBlockBytes" in text
    )
    _require(
        askers == ["packages/core/src/tools/read-file.ts"],
        f"{label}: tools other than read_file ask for the inline block: "
        f"{', '.join(p for p in askers if not p.endswith('/read-file.ts'))}. "
        f"A tool does not bound its own result; the bound is applied where the "
        f"model's copy is made.",
    )

    # A tool that cuts a result must say so through the one notice.
    offenders = sorted(
        path
        for path, text in sources.items()
        if _OUTPUT_CAP.search(text) and not _BOUND_HELPER.search(text)
    )
    _require(
        not offenders,
        f"{label}: these tools cut a result to a cap without declaring it: "
        f"{', '.join(offenders)}. A result that is quietly short is one the "
        f"model cannot tell from a complete one, so it spends the context and "
        f"misreports coverage at the same time. Return the cut content through "
        f"boundedContent() in packages/core/src/tools/tools.ts, which states "
        f"what was asked for, what came back, the bound and its unit, the true "
        f"total or why the tool cannot know it, and the exact call that "
        f"continues. Do not phrase a notice here: there is one, so that every "
        f"tool says it the same way and a reader learns the shape once.",
    )

    # read_file pages by the room one inline block leaves what leads a page,
    # and only two things can end a page: its bytes, or the caller's own
    # `limit`. The notice names whichever did -- blaming the bytes when the
    # caller's smaller `limit` stopped the page is exactly the plausible wrong
    # number this notice exists to prevent. The page the file ends with carries
    # no notice, because nothing cut it; it says instead which lines it holds
    # and that the file ends with them, counted in lines rather than in the
    # segments a final newline adds one to, so a reader that paged there knows
    # it reached the end.
    _require_all(
        state,
        "packages/core/src/tools/read-file.ts",
        (
            "function readPageBytes(",
            "if (result.nextRead && result.linesShown && trueTotal !== undefined) {",
            "result.truncatedByBytes === true || this.params.limit === undefined;",
            "limit: cutByBytes ? pageBytes : this.params.limit!,",
            "limitUnit: cutByBytes ? 'bytes of content' : 'lines',",
            "result.originalLineCount - (result.endsWithNewline === true ? 1 : 0);",
            "  return page.split('\\n').length - (page === '' || page.endsWith('\\n') ? 1 : 0);",
            "      returned: pageLineCount(page),",
            "`Lines ${first}-${first + returned - 1} of ${total} in total: the file ends here.`",
            "llmContent = `${pageEnd(this.params.offset + 1, pageLineCount(page), trueTotal)}${PAGE_STATEMENT_BREAK}${page}`;",
            "tokenizerText(`${pageEnd(most, most, most)}${PAGE_STATEMENT_BREAK}`).bytes,",
        ),
        label=label,
    )
    # A sentence that says "lines" means lines. `originalLineCount` counts the
    # segments a split on "\n" yields, which is one more than the lines
    # whenever the file ends with a newline; every arithmetic use of it counts
    # segments and stays as it is, so the fact travels instead of the count
    # changing under the paging that depends on it.
    require_text(
        state,
        "packages/core/src/utils/read-text-range.ts",
        "    endsWithNewline: content.endsWith('\\n'),",
        label=label,
    )
    # The fact has to survive the seam it crosses: the file-system response
    # declares it too, or fileUtils reads a property the type does not have.
    _require_all(
        state,
        "packages/core/src/services/fileSystemService.ts",
        (
            "    endsWithNewline?: boolean;",
            "      ...(readResult.endsWithNewline !== undefined",
        ),
        label=label,
    )
    _require_all(
        state,
        "packages/core/src/utils/fileUtils.ts",
        (
            "  endsWithNewline?: boolean;",
            "          endsWithNewline: _meta?.endsWithNewline,",
            "          truncatedByBytes,",
            # A page is budgeted in the NFC form it is handed on in, which the
            # range reader's count of the file's own bytes is not; its size is
            # the caller's, and no line count the deployment never chose
            # shortens it.
            "const maxOutputBytes = options.pageBytes ?? DEFAULT_RANGE_READ_BYTES;",
            "const normalized = tokenizerText(line);",
            "if (pageBytes + separator + normalized.bytes > maxOutputBytes) break;",
            "const page = tokenizerText(pageLines.join('\\n'));",
        ),
        label=label,
    )
    _require_all(
        state,
        "packages/core/src/tools/read-file.ts",
        (
            "if (typeof llmContent === 'string' && result.linesShown !== undefined) {",
            "const block = tokenizerText(llmContent);",
            "const maxBytes = this.config.getInlineBlockBytes();",
        ),
        label=label,
    )
    require_text(
        state,
        "packages/core/src/utils/readManyFiles.ts",
        "          (fileReadResult.endsWithNewline === true ? 1 : 0);",
        label=label,
    )
    for case in (
        "reports the true line count and names what actually bound",
        "says the last page of a file read in pages is its end, counting its lines, not its final newline",
        "says a read that starts past the end of a file has no lines, and where the file ends",
        "counts the lines of a page by the page",
    ):
        require_text(state, "packages/core/src/tools/read-file.test.ts", case, label=label)

    # The raw byte cut is gone: a cut is made in the tokenizer's form or not
    # at all.
    for path in list(sources) + ["packages/core/src/tools/agent/agent.ts"]:
        _require(
            "cutToUtf8Bytes" not in _source(state, path, label=label),
            f"{label}: {path} still cuts by raw UTF-8 bytes; the bound is the NFC "
            f"form's bytes, cut by cutToTokenizerBytes",
        )

    # Nothing may re-grow a private notice beside it.
    for path, text in sources.items():
        if path == tools_ts:
            continue
        for retired in _RETIRED_NOTICES:
            _require(
                retired not in text,
                f"{label}: {path} contains the retired truncation wording "
                f"{retired!r}. These were replaced by the single notice in "
                f"{tools_ts}; a tool that phrases its own is how a bound "
                f"became invisible in the first place.",
            )

    # The known capping tools are named so the detector cannot be satisfied by
    # a tree that simply stopped capping anything. Each cap they keep is the
    # caller's own or a scan's, and each is stated through boundedContent.
    for path in (
        "packages/core/src/tools/glob.ts",
        "packages/core/src/tools/grep.ts",
        "packages/core/src/tools/lsp.ts",
        "packages/core/src/tools/ripGrep.ts",
        "packages/core/src/tools/tool-search.ts",
    ):
        require_text(state, path, "boundedContent(", count=1, label=label)
    # A listing is not cut to a count nobody asked for: glob keeps only its
    # scan ceiling, and ls returns every entry.
    for path, retired in (
        ("packages/core/src/tools/glob.ts", "MAX_FILE_COUNT"),
        ("packages/core/src/tools/ls.ts", "MAX_ENTRY_COUNT"),
        ("packages/core/src/tools/ls.ts", "boundedContent("),
    ):
        forbid_text(state, path, retired, label=label)
    require_text(
        state, "packages/core/src/tools/glob.ts", "MAX_GLOB_COLLECTED_ENTRIES = 1000", label=label
    )
    # The todo reminder that cut its own copy of the list is gone with the
    # reminder itself: todo_write states the list it wrote and nothing more.
    for retired in ("formatOutputBound(", "setActiveTodoReminder", "MAX_ACTIVE_TODO_CONTEXT_CHARS"):
        forbid_text(state, "packages/core/src/tools/todoWrite.ts", retired, label=label)
    # Nor does a copy of the list come back into later turns: the periodic
    # reminder is deleted everywhere it was wired, and the work-chain owners
    # the scheduler scopes todos by are what remains.
    for path in (
        "packages/core/src/config/config.ts",
        "packages/core/src/core/client.ts",
        "packages/cli/src/acp-integration/session/Session.ts",
    ):
        for retired in ("ActiveTodoReminder", "activeTodoReminder", "ACTIVE_TODO_REMINDER_REFRESH_TURNS"):
            forbid_text(state, path, retired, label=label)
    require_text(
        state,
        "packages/core/src/config/config.ts",
        "  getActiveTodoWorkChainOwner(",
        label=label,
    )
    forbid_text(
        state,
        "packages/core/src/services/monitorRegistry.ts",
        "'...[truncated]'",
        label=label,
    )

    # A layer below the tools phrases model-facing text too. It states the
    # capture limit through the same helper, so a single result cannot carry
    # two vocabularies for the same fact, and it reports the truncation as a
    # field so a caller can tell a retained prefix from a complete capture
    # without reading the prose.
    for path in _MODEL_FACING_NON_TOOL_SOURCES:
        text = _source(state, path, label=label)
        for retired in _RETIRED_NOTICES:
            _require(
                retired not in text,
                f"{label}: {path} contains the retired truncation wording "
                f"{retired!r}. It is below `src/tools`, so the detector that "
                f"holds tools to the single notice cannot see it; the rule "
                f"applies here by name instead.",
            )
        require_text(state, path, "formatOutputBound(", count=1, label=label)
    # The capture notice follows the output it describes, as upstream placed
    # it.
    _require_all(
        state,
        "packages/core/src/services/shellExecutionService.ts",
        (
            "outputCaptureLimitExceeded?: boolean;",
            "unit: 'bytes of output',",
            "total: totalBytesReceived,",
            "return output ? `${output}\\n\\n${notice}` : notice;",
        ),
        label=label,
    )

    # Upstream's layouts, kept. Nothing moves the facts a result closes with
    # ahead of what they describe to survive a cut that keeps only the start:
    # the cut keeps the end, so an exit status, an error or a note on where
    # the bytes went stays where upstream put it and where the model was
    # trained to read it.
    for path, ordered in (
        (
            "packages/core/src/tools/shell.ts",
            (
                "`Output: ${result.output || '(empty)'}`,",
                "`Error: ${finalError}`,",
                "`Exit Code: ${result.exitCode ?? '(none)'}`,",
                "`Signal: ${result.signal ?? '(none)'}`,",
                "`Process Group PGID: ${result.pid ?? '(none)'}`,",
            ),
        ),
        (
            "packages/core/src/tools/web-search.ts",
            (
                "      sections.push(answerText);",
                "'Opened evidence pages (read in full by the search agent):\\n' +",
                "formatSearchBody(this.params.query, data, partialNote) +",
                "      CITATION_POLICY +",
                "      SAFETY_FOOTER;",
            ),
        ),
        (
            "packages/core/src/tools/write-file.ts",
            (
                "`User modified the \\`content\\` to be: ${content}`,",
                "formatRecordArtifactReminder(artifact.workspacePath),",
            ),
        ),
        (
            "packages/core/src/tools/ls.ts",
            (
                "let resultMessage = `Listed ${totalEntryCount} item(s) in ${this.params.path}:\\n---\\n${directoryContent}`;",
                "resultMessage += `\\n\\n(${ignoredMessages.join(', ')})`;",
            ),
        ),
    ):
        _require_ordered(
            _source(state, path, label=label), ordered, label=label, location=path
        )
    for path, needle in (
        ("packages/core/src/tools/shell.ts", "llmContent += `\\n\\n${longRunHint}`;"),
        ("packages/core/src/tools/shell.ts", "llmContent += `\\n\\n${attributionWarning}`;"),
        (
            "packages/core/src/tools/web-fetch.ts",
            "llmContent: `${header}\\n\\n${entry.content}${sourceNote}`,",
        ),
        ("packages/core/src/tools/agent/agent.ts", "            ) + effectSuffix;"),
    ):
        require_text(state, path, needle, label=label)
    require_text(
        state,
        "packages/core/src/tools/web-search.ts",
        "listing the relevant URLs from above as markdown links.",
        label=label,
    )
    # create_sub_session is upstream's own again: upstream returns a first
    # turn whole, its link warning after it, and bounds nothing itself.
    _require(
        "packages/core/src/tools/create-sub-session.ts" not in state,
        f"{label}: packages/core/src/tools/create-sub-session.ts is patched "
        f"again; upstream's returns a first turn whole, its link warning "
        f"after it, and bounds nothing itself.",
    )

    # A tool returns its whole result, laid out as upstream lays it out, and
    # the one bound keeps its start and its end, so what a result says last
    # survives in the place the model was trained to find it. Executed in the
    # build.
    for path, case in (
        ("packages/core/src/tools/tools.test.ts", "names the limit that cut a result and the call that continues"),
        ("packages/core/src/tools/tools.test.ts", "names no cut for a result no limit cut, only the coverage it could not reach"),
        ("packages/core/src/tools/tools.test.ts", "says the rest was not retained, and why, instead of naming a read that cannot succeed"),
        ("packages/core/src/tools/tools.test.ts", "leads the content it bounds"),
        ("packages/core/src/tools/shell.test.ts", "returns command output whole, in upstream field order with the exit status after it"),
        ("packages/core/src/services/shellExecutionService.test.ts", "bounds buffered PTY output before building the final string"),
        ("packages/core/src/tools/web-fetch.test.ts", "returns preapproved markdown whole for the one bound on a tool result to hold"),
        ("packages/core/src/tools/web-fetch.test.ts", "says a binary body cannot be kept when the artifact store is full, with what to do instead"),
        ("packages/core/src/tools/mcp-tool.test.ts", "returns a reply whose parts together pass one inline block whole, for the one bound on a tool result to hold"),
        ("packages/core/src/tools/web-search.test.ts", "keeps a long answer before its evidence and guidance, and the cut keeps both ends"),
        ("packages/core/src/tools/read-mcp-resource.test.ts", "returns a resource whole however large, leaving its bound to where the model copy is made"),
        ("packages/core/src/tools/create-sub-session.test.ts", "returns a long first turn whole, its link warning after it as upstream lays it out"),
        ("packages/core/src/tools/write-file.test.ts", "quotes user-modified content whole, where upstream puts it, leaving its bound to where the model copy is made"),
        ("packages/core/src/tools/ls.test.ts", "says what it left out after the entries, as upstream lays it out"),
        ("packages/core/src/tools/glob.test.ts", "returns every match below the scan ceiling, leaving the inline bound to the one bound on a tool result"),
        ("packages/core/src/tools/ls.test.ts", "lists every entry, leaving the inline bound to the one bound on a tool result"),
        ("packages/core/src/services/monitorRegistry.test.ts", "bounds a long event line through the one notice"),
    ):
        require_text(state, path, case, label=label)

    # A cap the service applied and then forgot cannot be declared by anyone,
    # so the fact travels with the items.
    _require_all(
        state,
        "packages/core/src/lsp/types.ts",
        ("export interface LspBounded<T> {", "readonly hadMore: boolean;"),
        label=label,
    )
    forbid_text(
        state,
        "packages/core/src/tools/lsp.ts",
        "Found ${Math.min(symbols.length, limit)}",
        label=label,
    )


def _validate_compaction_read_evidence_before(state: State) -> None:
    label = "compaction read-evidence precondition"
    cache = "packages/core/src/services/fileReadCache.ts"
    chat = "packages/core/src/core/geminiChat.ts"
    client = "packages/core/src/core/client.ts"
    # Upstream has the per-file disarm microcompaction uses and nothing that
    # disarms every entry, so a compaction that replaces the whole history
    # reaches for clear() -- which also discards authorship.
    require_text(
        state,
        cache,
        "  markReadEvictedFromHistory(stats: Stats): boolean {",
        label=label,
    )
    forbid_text(state, cache, "markAllReadsEvictedFromHistory", label=label)
    require_text(
        state, chat, "      this.config.getFileReadCache().clear();", label=label
    )
    require_text(
        state,
        client,
        "      debugLogger.debug('[FILE_READ_CACHE] clear after tryCompressChat');",
        label=label,
    )


def _validate_compaction_read_evidence_after(state: State) -> None:
    """Compaction disarms quotability and leaves authorship alone.

    recordWrite deliberately seeds read evidence, because the model composed
    the bytes a tool just wrote and has therefore seen them. clear() discards
    that along with residency, so a session was refused permission to
    overwrite a file it had written itself. The two facts are different and
    the entry already separates them: readResidentInHistory says the content
    is still quotable from history, lastReadAt/lastWriteAt say the model has
    seen it. Compaction invalidates only the first.
    """

    label = "compaction read-evidence result"
    cache = "packages/core/src/services/fileReadCache.ts"
    cache_test = "packages/core/src/services/fileReadCache.integration.test.ts"
    chat = "packages/core/src/core/geminiChat.ts"
    client = "packages/core/src/core/client.ts"

    source = _require_all(
        state,
        cache,
        (
            "  markAllReadsEvictedFromHistory(): number {",
            "  markReadEvictedFromHistory(stats: Stats): boolean {",
            "  clear(): void {",
        ),
        label=label,
    )
    # The all-entry disarm touches residency and nothing else. Naming the
    # authorship fields here is what keeps a later edit from folding clear()
    # back into it.
    body = source.split("  markAllReadsEvictedFromHistory(): number {", 1)[1]
    body = body.split("\n  }\n", 1)[0]
    _require(
        "entry.readResidentInHistory = false;" in body,
        f"{label}: the all-entry disarm does not disarm history residency",
    )
    for absent in (
        "lastReadAt",
        "lastWriteAt",
        "lastReadWasFull",
        "lastReadCacheable",
        "delete",
        "clear(",
    ):
        _require(
            absent not in body,
            f"{label}: the all-entry disarm touches {absent!r}, which is "
            f"authorship evidence rather than quotability. A file the model "
            f"wrote is one it has seen; discarding that refuses its next "
            f"overwrite of bytes it authored",
        )

    # The compaction that replaces history disarms; it does not wipe. The
    # client wrapper does not repeat it: the chat already disarmed before
    # startChat, and the cache is owned by Config rather than by the chat.
    require_text(state, chat, ".markAllReadsEvictedFromHistory();", label=label)
    forbid_text(state, chat, "getFileReadCache().clear()", label=label)
    forbid_text(
        state, client, "[FILE_READ_CACHE] clear after tryCompressChat", label=label
    )

    # Executed in the build.
    require_text(
        state,
        cache_test,
        "disarms history residency without discarding the write that authored the file",
        label=label,
    )


def _validate_stream_bounds_before(state: State) -> None:
    label = "stream-bounds precondition"
    constants = "packages/core/src/core/openaiContentGenerator/constants.ts"
    pipeline = "packages/core/src/core/openaiContentGenerator/pipeline.ts"
    # Upstream arms the idle watchdog when the stream is wrapped, sizes both
    # guards from environment knobs and defaults, and lets 0 disable each.
    _require_all(
        state,
        constants,
        (
            "export const QWEN_STREAM_IDLE_TIMEOUT_MS_ENV = 'QWEN_STREAM_IDLE_TIMEOUT_MS';",
            "export const DEFAULT_STREAM_MAX_LIFETIME_MS = 900000;",
        ),
        label=label,
    )
    _require_all(
        state,
        pipeline,
        (
            "function resolveStreamGuardMs(",
            "async function* withStreamGuards(",
        ),
        label=label,
    )


def _validate_stream_bounds_after(state: State) -> None:
    label = "stream-bounds result"
    constants = "packages/core/src/core/openaiContentGenerator/constants.ts"
    pipeline = "packages/core/src/core/openaiContentGenerator/pipeline.ts"
    content = "packages/core/src/core/contentGenerator.ts"
    pipeline_test = "packages/core/src/core/openaiContentGenerator/pipeline.test.ts"
    bound_test = "packages/core/src/core/openaiContentGenerator/pipeline-guard-config.test.ts"
    settings_doc = "docs/users/configuration/settings.md"
    # The decode floor is declared once, as policy, with its consequence; the
    # idle bound is declared once with its reason.
    _require_all(
        state,
        constants,
        (
            "export const DECODE_FLOOR_TPS = 12;",
            " * refused as a broken engine, loudly, rather than waited on. The value is\n"
            " * declared policy, not a measurement, and it is deliberately pessimistic, so\n",
            "export const STREAM_IDLE_TIMEOUT_MS = 240000;",
            " * The longest silence between the chunks of a started stream. It arms at the\n"
            " * first chunk.",
        ),
        label=label,
    )
    source = _require_all(
        state,
        pipeline,
        (
            "export function streamGenerationBoundMs(maxTokens: unknown): number {",
            "  const boundMs = Math.ceil((maxTokens * 1000) / DECODE_FLOOR_TPS);",
            "export class StreamStartTimeoutError extends Error {",
            "  const startDeadline = bounds.dispatchedAt + bounds.requestTimeoutMs;",
            "        : startDeadline - performance.now();",
            "        const idleIn = started ? idleMs : Number.POSITIVE_INFINITY;",
            "      if (started) upstreamMs += performance.now() - awaitedAt;",
            "      if (error instanceof StreamStartTimeoutError) {",
        ),
        label=label,
    )
    # The generation's bound comes from the request's own max_tokens, and the
    # start deadline from the request timeout the SDK client was built with,
    # counted from the moment the request is dispatched.
    _require_ordered(
        source,
        (
            "        const generationMs = streamGenerationBoundMs(openaiRequest.max_tokens);",
            "        const requestTimeoutMs = resolveRequestTimeout(\n"
            "          this.contentGeneratorConfig.timeout,\n"
            "        );",
            "        const dispatchedAt = performance.now();",
            "          const createPromise = this.client.chat.completions.create(",
            "          { dispatchedAt, requestTimeoutMs, generationMs },",
        ),
        label=label,
        location=pipeline,
    )
    # One mode: no knob, no default beside the derived bound, nothing that
    # disables a bound, and the retired names do not return.
    for path in (constants, pipeline, content):
        for retired in (
            "QWEN_STREAM_IDLE_TIMEOUT_MS",
            "QWEN_STREAM_MAX_LIFETIME_MS",
            "DEFAULT_STREAM_IDLE_TIMEOUT_MS",
            "DEFAULT_STREAM_MAX_LIFETIME_MS",
            "streamIdleTimeoutMs",
            "streamMaxLifetimeMs",
            "resolveStreamGuardMs",
            "0 to disable",
            "idleMs > 0 || maxLifetimeMs > 0",
        ):
            forbid_text(state, path, retired, label=label)
    for path in ("packages/core/src/tools/web-fetch.ts", settings_doc):
        for retired in ("QWEN_STREAM_IDLE_TIMEOUT_MS", "QWEN_STREAM_MAX_LIFETIME_MS"):
            forbid_text(state, path, retired, label=label)
    require_text(
        state,
        settings_doc,
        "at a declared decode floor of 12 tokens per second",
        label=label,
    )
    for path, case in (
        (pipeline_test, "bounds the wait for the first chunk by the request timeout, not the idle guard"),
        (pipeline_test, "counts the start deadline from dispatch, not from the response headers"),
        (pipeline_test, "arms the idle guard at the first chunk"),
        (pipeline_test, "bounds the generation from its first chunk by max_tokens at the decode floor (issue #8597)"),
        (pipeline_test, "completes a generation that fits its bound however long it queued"),
        (pipeline_test, "refuses a streaming request that states no max_tokens before sending it"),
        (bound_test, "is the most tokens the request may generate at the decode floor"),
        (bound_test, "refuses a streaming request whose max_tokens is $label"),
    ):
        require_text(state, path, case, label=label)


CONCERNS: tuple[SemanticConcern, ...] = (
    SemanticConcern(
        name="shared-stream-admission",
        rationale=(
            "Core Goal state and serialized CLI records enter one required admission owner. "
            "A retained violation blocks further provider admission. The generated validators "
            "are compiled from the paired canonical schema with pinned dependencies and "
            "verified with their output receipt before build. Pure public usage "
            "types keep required SDK declaration bundles independent of provider internals. "
            "These source checks do not replace final-image certification and lifecycle tests."
        ),
        removal_condition=(
            "Upstream provides the same shared contract, mandatory common admission, "
            "failure retention and independently consumable public declarations."
        ),
        validate_before=_validate_stream_admission_before,
        validate_after=_validate_stream_admission_after,
    ),
    SemanticConcern(
        name="manual-compaction-ownership-and-outcome",
        rationale=(
            "Manual compaction joins its action, presentation and recording; cancellation cannot "
            "admit a successor or discard a completed checkpoint. Every renderer receives the same "
            "named outcome with exact counts and separately reported finalization failure."
        ),
        removal_condition=(
            "Upstream provides joined command ownership, complete outcome projection and replay, "
            "durable checkpoint failure ownership, and exact admission of whole directives."
        ),
        validate_before=_validate_manual_compaction_before,
        validate_after=_validate_manual_compaction_after,
    ),
    SemanticConcern(
        name="locked-config-and-literal-cli",
        rationale=(
            "A sealed deployment loads its explicit settings, prompt, tools, and local task guidance "
            "without admitting workspace executable policy or interpreting the submitted task as a CLI "
            "command."
        ),
        removal_condition=(
            "Upstream provides the same sealed configuration boundary and adversarial initialization "
            "tests."
        ),
        validate_before=_validate_locked_boundary_before,
        validate_after=_validate_locked_boundary_after,
    ),
    SemanticConcern(
        name="exact-rendered-request-tokenization",
        rationale=(
            "Every generation and exact count requires a captured invocation owner at the raw provider "
            "seam. The final request writes that owner after provider decoration. Configuration admits "
            "only an explicit OpenAI-compatible vLLM route, strict calls, exact sizing, and a positive "
            "declared window; selection failure rolls back the route."
        ),
        removal_condition=(
            "Upstream requires owned requests, counts the identical rendered wire payload, and proves "
            "route isolation and transactional activation."
        ),
        validate_before=_validate_exact_tokens_before,
        validate_after=_validate_exact_tokens_after,
    ),
    SemanticConcern(
        name="strict-stream-terminal-commit",
        rationale=(
            "One generation contract withholds structured calls and the terminal until EOF validation "
            "and canonical assistant recording succeed. Invalid pre-content requests have a bounded "
            "identical resample; delivered answers cannot be replayed. Literal text never supplies "
            "executable calls."
        ),
        removal_condition=(
            "Upstream provides equivalent batch/stream call grammar, terminal and durable commit "
            "barriers, with no heuristic recovery path."
        ),
        validate_before=_validate_stream_commit_before,
        validate_after=_validate_stream_commit_after,
    ),
    SemanticConcern(
        name="universal-tools-and-foreground-delegation",
        rationale=(
            "The declared tool allowlist gates native, dynamic, and synthetic invocations at the "
            "permission seam. Explicit foreground delegation exposes only the callable sequential "
            "built-in surface. An empty allowlist grants nothing."
        ),
        removal_condition=(
            "Upstream applies the same universal permission rule and exposes only delegation its "
            "configured runtime can perform."
        ),
        validate_before=_validate_tool_policy_before,
        validate_after=_validate_tool_policy_after,
    ),
    SemanticConcern(
        name="deployment-prompt-and-invocation-facts",
        rationale=(
            "Deployment-specific scratch and effect journals are described only inside the invocation "
            "that provides them. Ordinary host subagent prompts accurately describe real workspace "
            "effects and report limitations. Sealed prompt files and invocation frames remain explicit "
            "immutable inputs."
        ),
        removal_condition=(
            "Upstream binds prompt promises to actual invocation facilities and verifies sealed prompt, "
            "scratch, journal, and host behavior."
        ),
        validate_before=_validate_deployment_prompt_scratch_before,
        validate_after=_validate_deployment_prompt_scratch_after,
    ),
    SemanticConcern(
        name="original-png-and-image-chronology",
        rationale=(
            "Image content preserves the original supported PNG bytes and dimensions. Decoder bounds "
            "refuse unsupported or oversized input. Ordered text and images remain in their originating "
            "tool response."
        ),
        removal_condition=(
            "Upstream enforces identical original-byte PNG policy and typed non-splitting tool-result "
            "chronology."
        ),
        validate_before=_validate_image_before,
        validate_after=_validate_image_after,
    ),
    SemanticConcern(
        name="model-field-isolation",
        rationale=(
            "Exact sizing, strict calls, window, endpoint, and reasoning settings belong to a resolved "
            "provider route. A different selected model cannot inherit another route’s capabilities or "
            "identity."
        ),
        removal_condition=(
            "Upstream carries provider-scoped snapshots through model selection and independently tests "
            "activation rollback and child overrides."
        ),
        validate_before=_validate_model_config_before,
        validate_after=_validate_model_config_after,
    ),
    SemanticConcern(
        name="behavioral-regression-evidence",
        rationale=(
            "Exact source identity and semantic relationships are supplemented by executable tests of "
            "ownership, provider validation, durable recording, usage, retries, text, tool schemas, and "
            "artifacts. Each test must exercise its stated boundary."
        ),
        removal_condition=(
            "Equivalent upstream tests execute the same failure and positive-control boundaries in the "
            "pinned source."
        ),
        validate_before=_validate_behavioral_evidence_before,
        validate_after=_validate_behavioral_evidence_after,
    ),
    SemanticConcern(
        name="read-file-single-range-mechanism",
        rationale=(
            "read_file offers offset/limit as its one range mechanism. PDF page selection belongs to an "
            "explicit pdftotext command rendered with the requested filename. Unsupported arguments and "
            "non-text line ranges refuse instead of being ignored."
        ),
        removal_condition=(
            "Upstream exposes unambiguous file range parameters and refuses unsupported selections "
            "without losing remediation guidance."
        ),
        validate_before=_validate_pages_affordance_before,
        validate_after=_validate_pages_affordance_after,
    ),
    SemanticConcern(
        name="text-read-fidelity",
        rationale=(
            "Text uses its declared Unicode BOM encoding or fatal UTF-8. Pages preserve line content "
            "and terminators so their concatenation reproduces the file. An unpageable line refuses "
            "with an explicit alternative and continuation."
        ),
        removal_condition=(
            "Upstream guarantees strict decoding and exact contiguous text paging at every file size."
        ),
        validate_before=_validate_text_read_fidelity_before,
        validate_after=_validate_text_read_fidelity_after,
    ),
    SemanticConcern(
        name="schema-faithful-tool-parameters",
        rationale=(
            "Native tool schemas explicitly close their parameter objects and declare no "
            "JSON-Schema string constraint, because the served engine compiles the declaration "
            "into its generation grammar and refuses a bounded string there; each bound is "
            "enforced in the tool, which names the parameter and the limit when it refuses. "
            "External schemas retain their declared openness and pattern rules. Authenticated "
            "editor changes travel through private object provenance preserved by scheduler "
            "cloning, never hidden model parameter names."
        ),
        removal_condition=(
            "Upstream validates each schema as written, declares no string constraint the served "
            "grammar cannot express, preserves trusted editor provenance outside model JSON, and "
            "refuses unsupported file ranges."
        ),
        validate_before=_validate_param_contract_before,
        validate_after=_validate_param_contract_after,
    ),
    SemanticConcern(
        name="validation-refuses-rather-than-repairs",
        rationale=(
            "Schema validation leaves supplied arguments unchanged. A constraint violation or "
            "uncompilable schema refuses execution and identifies the error."
        ),
        removal_condition=(
            "Upstream refuses invalid and uncompilable schemas without coercion or skipped validation."
        ),
        validate_before=_validate_no_repair_validation_before,
        validate_after=_validate_no_repair_validation_after,
    ),
    SemanticConcern(
        name="model-facing-failure-text",
        rationale=(
            "Tool failure output preserves both the model-facing remedy and distinct operational error "
            "information, without dropping or duplicating either."
        ),
        removal_condition=(
            "Upstream forwards the complete model-facing failure and any distinct operational details "
            "through the shared scheduler."
        ),
        validate_before=_validate_model_facing_failure_before,
        validate_after=_validate_model_facing_failure_after,
    ),
    SemanticConcern(
        name="pdf-text-extraction-without-a-renderer",
        rationale=(
            "PDF input follows the sealed text-extraction contract. Missing tools, malformed or "
            "encrypted PDFs, timeout, and extraction overflow remain explicit failures with actionable "
            "page-range guidance."
        ),
        removal_condition=(
            "Upstream enforces the same text-only extraction surface and preserves complete extracted "
            "content or an explicit refusal."
        ),
        validate_before=_validate_pdf_text_only_before,
        validate_after=_validate_pdf_text_only_after,
    ),
    SemanticConcern(
        name="subagent-turn-count-ownership",
        rationale=(
            "A child reasoning round count belongs to its execution loop and survives completion, "
            "failure, cancellation, background continuation, and final display. API request counts are "
            "a separate fact."
        ),
        removal_condition=(
            "Upstream retains exact child reasoning-round ownership through every completion and "
            "continuation path."
        ),
        validate_before=_validate_turn_count_owner_before,
        validate_after=_validate_turn_count_owner_after,
    ),
    SemanticConcern(
        name="generation-stream-evidence",
        rationale=(
            "Streaming and batch renderers preserve generation evidence and provider termination under "
            "the correct root or spawning-call identity. Output drains before successful delivery is "
            "acknowledged."
        ),
        removal_condition=(
            "Upstream emits equivalent scoped generation evidence through callback-settled output "
            "adapters."
        ),
        validate_before=_validate_stream_evidence_before,
        validate_after=_validate_stream_evidence_after,
    ),
    SemanticConcern(
        name="compaction-attempt-evidence",
        rationale=(
            "Every attempted compaction is recorded and flushed at the shared chat seam. Success, "
            "refusal, cancellation, and failure preserve the attempt’s original history and output "
            "facts; only a successful replacement publishes a compressed-history event."
        ),
        removal_condition=(
            "Upstream records all attempted compactions through one shared root/child recorder and "
            "scoped renderer projection."
        ),
        validate_before=_validate_compaction_event_before,
        validate_after=_validate_compaction_event_after,
    ),
    SemanticConcern(
        name="correctable-tool-call-repetition",
        rationale=(
            "A repeated identical tool call is refused and answered before it is fatal. The model is "
            "told the repetition count, that the previous result cannot have changed, and which "
            "argument an earlier differing call to the same tool proved was dropped. Only a repeat "
            "after being told ends the run, on the same comparison and at the same count as before: "
            "the correction is delivered inside the existing tolerance, so the ceiling of "
            "identical calls does not move and the other rules are untouched."
        ),
        removal_condition=(
            "Upstream answers a detected repetition with a tool result the model can act on before it "
            "terminates the run."
        ),
        validate_before=_validate_correctable_repetition_before,
        validate_after=_validate_correctable_repetition_after,
    ),
    SemanticConcern(
        name="subagent-result-scope-and-turn-count",
        rationale=(
            "Each child result carries its spawning tool-call scope and actual execution-round count. "
            "Root and child results cannot absorb each other’s state or usage. A child that stopped "
            "before its goal, or could not be run, is a failed call whose message the report the "
            "parent reads already carries."
        ),
        removal_condition=(
            "Upstream emits distinct correctly owned root and child results with preserved turn counts."
        ),
        validate_before=_validate_subagent_result_scope_before,
        validate_after=_validate_subagent_result_scope_after,
    ),
    SemanticConcern(
        name="subagent-terminal-progress-and-loop-attribution",
        rationale=(
            "Child rounds, compactions, failures, and cancellations retain inspectable transcript "
            "evidence. Terminal and web renderers project bounded status from owner usage and execution "
            "state without repeatedly copying raw reasoning. Loop failures identify the scope they "
            "stopped."
        ),
        removal_condition=(
            "Upstream provides bounded child status and recoverable scoped evidence with exact loop "
            "attribution."
        ),
        validate_before=_validate_subagent_progress_before,
        validate_after=_validate_subagent_progress_after,
    ),
    SemanticConcern(
        name="compaction-output-accounting",
        rationale=(
            "A compaction attempt retains its observed text, reasoning, terminal, request attempts, and "
            "full served usage or explicit absence. Failure after a partial stream cannot erase the "
            "last valid observation. Reasoning remains inline; this contract does not assert reference "
            "persistence."
        ),
        removal_condition=(
            "Upstream preserves complete success and interrupted compaction evidence with served "
            "provenance and explicit preflight absence."
        ),
        validate_before=_validate_compaction_accounting_before,
        validate_after=_validate_compaction_accounting_after,
    ),
    SemanticConcern(
        name="context-window-partition",
        rationale=(
            "The served window is spent from three declared quantities: the static preamble, a "
            "capacity proved against the real preamble before the first turn, with the Git "
            "snapshot's values, the startup context's workspace data and the stated turn budget's "
            "number bounded by their bytes; one inline block; and the per-message framing. A turn's "
            "room and the compaction trigger are derived from the fit; a turn's output limit is "
            "that room and nothing else, and a configured ceiling is refused. Compaction summarises "
            "the prompt the last turn was issued against and carries that turn verbatim behind the "
            "snapshot, so the snapshot always has at least a turn's room. Exact before/after sizing "
            "governs compaction. Original authored inputs are retained independently of model "
            "summaries; summaries must satisfy the six-section structural contract. No phase budget "
            "forces reasoning to stop."
        ),
        removal_condition=(
            "Upstream provides the same derived turn room, a preamble proved against its declared "
            "share, issued-prompt compaction with a verbatim turn, exact request sizing, durable "
            "authored-input retention, and structural summary validation."
        ),
        validate_before=_validate_compaction_budget_before,
        validate_after=_validate_compaction_budget_after,
    ),
    SemanticConcern(
        name="tool-result-bound",
        rationale=(
            "Every tool result is held to one inline block where its model copy is made: the whole "
            "text the model reads, hooks and reminders included, measured once in the bytes of its NFC "
            "form. A longer one keeps its head and a notice naming the read of the complete text, "
            "retained as an immutable session artifact; a store at its declared capacity is an "
            "outcome the notice states, not an error. Tools do not bound themselves, and a result that "
            "arrives past the bound is refused as a defect. Text and binary producers share one store "
            "with exclusive publication and ownership retention across Config recreation, resume, fork, "
            "and elapsed time."
        ),
        removal_condition=(
            "Upstream bounds the complete model-facing text of every result once, where the model's "
            "copy is made, and preserves the full text with durable concurrent accounting and "
            "reference-aware lifetime."
        ),
        validate_before=_validate_tool_result_bound_before,
        validate_after=_validate_tool_result_bound_after,
    ),
    SemanticConcern(
        name="compaction-read-evidence",
        rationale=(
            "Compaction invalidates what the model can quote, not what it has seen. The entry "
            "already separates the two, so compaction disarms history residency for every entry "
            "and leaves the read and write evidence that authorises a later overwrite intact. A "
            "file the session wrote itself stays a file it has seen."
        ),
        removal_condition=(
            "Upstream separates history residency from read/write authorship across every "
            "history-replacing path."
        ),
        validate_before=_validate_compaction_read_evidence_before,
        validate_after=_validate_compaction_read_evidence_after,
    ),
    SemanticConcern(
        name="cli-invocation-time-anchor",
        rationale=(
            "A single process-start timestamp labels the CLI invocation and keeps the deployment prompt "
            "stable. It is not represented as a persisted session start time."
        ),
        removal_condition=(
            "Upstream supplies an equivalent correctly named stable invocation timestamp."
        ),
        validate_before=_validate_session_time_before,
        validate_after=_validate_session_time_after,
    ),
    SemanticConcern(
        name="required-read-offset",
        rationale=(
            "Every read_file call explicitly declares offset, including zero for whole-file reads. "
            "Optional limit carries an exact continuation when paging occurs. Non-text files accept "
            "zero but refuse line ranges."
        ),
        removal_condition=(
            "Upstream requires explicit offset and preserves exact continuation and non-text range "
            "behavior."
        ),
        validate_before=_validate_required_read_offset_before,
        validate_after=_validate_required_read_offset_after,
    ),
    SemanticConcern(
        name="literal-response-fidelity",
        rationale=(
            "Visible text and reasoning retain their original bytes through stream, history, and "
            "recording, including XML-like text beside real structured calls. Execution depends on "
            "validated structured calls rather than text classification."
        ),
        removal_condition=(
            "Upstream preserves arbitrary literal response text with the same strict executable-call "
            "separation."
        ),
        validate_before=_validate_literal_response_before,
        validate_after=_validate_literal_response_after,
    ),
    SemanticConcern(
        name="incomplete-generation-terminal-state",
        rationale=(
            "Only a self-ended STOP can satisfy the completed-generation predicate. Missing or "
            "externally stopped terminals retain an explicit incomplete state in root and child "
            "reasoning loops and UI."
        ),
        removal_condition=(
            "Upstream distinguishes self-ended generation from a severed response and preserves that "
            "fact in every renderer."
        ),
        validate_before=_validate_incomplete_generation_before,
        validate_after=_validate_incomplete_generation_after,
    ),
    SemanticConcern(
        name="final-message-slip-notice",
        rationale=(
            "A turn the model ended itself with no tool call is its final answer only when its "
            "visible text is one. The model is a stochastic entity: a message with no visible text, "
            "or with the served template's tool-call markup outside a structured call, is a slip "
            "that is represented to the model as a user-role notice, never executed, and never used "
            "to end the run until it repeats. The third consecutive slip is the SLIPPED_FINAL_MESSAGE "
            "state, error_slipped_final_message on the wire, in the headless session and every "
            "subagent alike, after the incomplete-generation check and never in its place. Detection "
            "is an exact string test on the turn's visible text; no next-speaker judgment returns: "
            "the check, its telemetry event and the setting that switched it off are gone."
        ),
        removal_condition=(
            "Upstream answers an empty or markup-carrying self-ended turn with the same notice in "
            "both reasoning loops, bounds it at three consecutive slips, and names the ending as its "
            "own terminal state carried to the parent with its shape."
        ),
        validate_before=_validate_final_message_slip_before,
        validate_after=_validate_final_message_slip_after,
    ),
    SemanticConcern(
        name="bounded-tool-output",
        rationale=(
            "A tool result says whether it is complete. One notice states what was asked for, what "
            "came back, the limit that cut it and its unit, the real total or why the tool cannot "
            "know it, and the call that continues or why the rest was not kept; tools do not phrase "
            "their own. A tool that cuts a result to a cap without declaring it refuses the build, "
            "no listing is cut to a count its caller did not ask for, and a cap a service applied is "
            "reported with the items rather than discarded, because a layer cannot declare what it "
            "was never told."
        ),
        removal_condition=(
            "Upstream makes a truncating tool result state its bound, its unit, its total and its "
            "continuation through one shared implementation, and refuses a tool that cuts silently."
        ),
        validate_before=_validate_bounded_output_before,
        validate_after=_validate_bounded_output_after,
    ),
    SemanticConcern(
        name="terminal-state-is-a-value",
        rationale=(
            "A closed terminal-state value maps once to a wire name, whose error classification and exit "
            "code are the stream contract's terminal table, read from its generated binding. The "
            "constructed and mapped state sets agree, and every name the contract defines is one a "
            "state produces. Budget admission precedes "
            "charging. One queued-turn lifetime tracks result delivery, so failed cleanup cannot mint a "
            "second terminal; cleanup and output callbacks are awaited."
        ),
        removal_condition=(
            "Upstream provides total state mapping, exact budget ownership, one result delivery, and "
            "error-preserving cleanup/output settlement."
        ),
        validate_before=_validate_terminal_state_before,
        validate_after=_validate_terminal_state_after,
    ),
    SemanticConcern(
        name="served-accounting",
        rationale=(
            "One validated five-count report describes served prompt, output, reasoning, cache, and "
            "total. Durable dispatch and finalization share request identity; no request, reported "
            "zero, missing usage, and absent finalization remain distinct. The same accumulator "
            "supplies owner-scoped live, resumed, child, ledger, export, and UI projections. All "
            "generating chats require canonical recording under shared session write ownership."
        ),
        removal_condition=(
            "Upstream supplies durable owned observations, required canonical writers, strict replay, "
            "and equivalent provenance-preserving consumer projections."
        ),
        validate_before=_validate_served_accounting_before,
        validate_after=_validate_served_accounting_after,
    ),
    SemanticConcern(
        name="stream-bounds",
        rationale=(
            "A streaming response is held to bounds the client derives, not settings: until its "
            "first chunk, the request timeout counted from dispatch, since a request queued behind "
            "another generation sends nothing by design; from the first chunk, a declared idle bound "
            "between chunks and the generation's own bound, its max_tokens at a declared decode "
            "floor, below which the engine is treated as broken. No bound can be disabled, and a "
            "streaming request that states no max_tokens is refused."
        ),
        removal_condition=(
            "Upstream arms its stall detection at the first chunk, bounds the wait to start by the "
            "request timeout, and derives the generation bound from the request's output limit."
        ),
        validate_before=_validate_stream_bounds_before,
        validate_after=_validate_stream_bounds_after,
    ),
)


def _validate_before(state: State) -> None:
    for concern in CONCERNS:
        concern.validate_before(state)


def _validate_after(state: State) -> None:
    for concern in CONCERNS:
        concern.validate_after(state)


def validate_final(state: State) -> None:
    """Re-run every independent concern against the complete final tree."""

    _validate_after(state)



CONTRACTS: Mapping[str, SemanticContract] = {
    "qwen-code-agent-service": SemanticContract(
        rationale="\n\n".join(
            f"[{concern.name}] {concern.rationale}" for concern in CONCERNS
        ),
        removal_condition="\n\n".join(
            f"[{concern.name}] {concern.removal_condition}" for concern in CONCERNS
        ),
        validate_before=_validate_before,
        validate_after=_validate_after,
    )
}
