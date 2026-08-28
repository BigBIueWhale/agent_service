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
    forbid_text(state, limits, "clampExactOutputTokensToWindow", label=label)
    forbid_text(state, pipeline, "deriveVllmTokenizeUrl", label=label)
    _require_all(
        state,
        chat,
        ("estimatePromptTokens(", "ESTIMATE_CLAMP_OVERHEAD_PAD"),
        label=label,
    )


def _validate_exact_tokens_after(state: State) -> None:
    label = "server-authoritative token count result"
    limits = "packages/core/src/core/tokenLimits.ts"
    pipeline = "packages/core/src/core/openaiContentGenerator/pipeline.ts"
    chat = "packages/core/src/core/geminiChat.ts"
    content = "packages/core/src/core/contentGenerator.ts"
    compression = "packages/core/src/services/chatCompressionService.ts"
    base_client = "packages/core/src/core/baseLlmClient.ts"
    limit_source = _require_all(
        state,
        limits,
        (
            "export function clampExactOutputTokensToWindow(",
            "const room = contextWindowSize - exactPromptTokens;",
            "if (room <= 0)",
            "return Math.min(outputCeiling, room);",
        ),
        label=label,
    )
    exact_start = limit_source.index("export function clampExactOutputTokensToWindow(")
    exact_end = limit_source.index("\nexport function ", exact_start + 10)
    exact_body = limit_source[exact_start:exact_end]
    _require(
        "MIN_CLAMPED_OUTPUT_TOKENS" not in exact_body
        and "Math.max(" not in exact_body,
        f"{label}: exact clamp contains a fabricated minimum or heuristic floor",
    )
    pipeline_source = _require_all(
        state,
        pipeline,
        (
            "export function deriveVllmTokenizeUrl(",
            "normalizedPath.endsWith('/v1')",
            "export function validateVllmTokenizeResponse(",
            "refusing to estimate",
            "async countRequestTokens(",
            "const wireRequest = await this.buildRequest(",
            "model: wireRequest.model",
            "messages: wireRequest.messages",
            "add_generation_prompt: true",
            "tokenizeBody['tools'] = wireRequest.tools",
            "tokenizeBody['chat_template_kwargs']",
            "deriveVllmTokenizeUrl(this.contentGeneratorConfig.baseUrl)",
            "response.max_model_len",
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
    chat_source = _require_all(
        state,
        chat,
        (
            "cgConfigForThresholds?.exactTokenCounting === 'vllm'",
            "cannot count rendered requests",
            "result.maxModelLen !== contextWindowForClamp",
            "this.getRequestHistoryWithPendingForRoute(",
            "promptTokensForClamp = exactTokenCounting",
            "clampExactOutputTokensToWindow(",
            "if (!exactTokenCounting && this.promptCountIsEstimateDerived())",
        ),
        label=label,
    )
    _require(
        chat_source.count("await countExactRequestTokens(") >= 3,
        f"{label}: compaction and generation no longer share the exact renderer count",
    )
    _require_all(
        state,
        content,
        (
            "countRequestTokens?(",
            "Promise<ExactRequestTokenCount>",
            "exactTokenCounting?: 'vllm'",
        ),
        label=label,
    )
    compression_source = _require_all(
        state,
        compression,
        (
            "`${promptId}:auto-threshold`",
            "await chat.countRequestTokensForCandidateHistory(",
            "COMPRESSION_FAILED_TOKEN_COUNT_ERROR",
            "COMPACT_THINKING_TOKEN_BUDGET = 12_000",
            "COMPACT_FINAL_RESPONSE_TOKEN_BUDGET = 8_000",
            "summaryResult.finishReason === FinishReason.MAX_TOKENS",
            "summaryResult.finishReason !== FinishReason.STOP",
            "newTokenCountIsEstimated: false",
        ),
        label=label,
    )
    _require(
        "estimatePromptTokens(" not in compression_source,
        f"{label}: compaction threshold silently regained a heuristic tokenizer",
    )
    _require_all(
        state,
        base_client,
        (
            "async countRequestTokens(",
            "contentGeneratorConfig.exactTokenCounting !== 'vllm'",
            "finishReason: result.finishReason",
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
    label = "strict stream commit barrier result"
    chat = "packages/core/src/core/geminiChat.ts"
    converter = "packages/core/src/core/openaiContentGenerator/converter.ts"
    types = "packages/core/src/core/openaiContentGenerator/types.ts"
    chat_source = _require_all(
        state,
        chat,
        (
            "const strictToolCalling = cgConfig?.strictToolCalling === true;",
            "STRICT_FRESH_RESAMPLE_MAX_RETRIES = 1",
            "!streamYieldedContentChunk",
            "const maxContinuationRetries = strictToolCalling\n                ? 0",
            "overrides || strictToolCalling ? false : isUnattendedMode()",
            "maxAttempts: Math.max(1, Math.floor(cgConfig.maxRetries) + 1)",
            "!strictToolCalling &&",
            "exactRoute || strictToolCalling",
        ),
        label=label,
    )
    _require(
        chat_source.count("strictToolCalling") >= 15,
        f"{label}: strict mode no longer gates every retry/recovery boundary",
    )
    converter_source = _require_all(
        state,
        converter,
        (
            "choice.finish_reason !== 'tool_calls'",
            "choice.finish_reason !== 'length'",
            "choice.finish_reason === 'tool_calls'",
            "JSON.parse(toolCall.function.arguments)",
            "tool arguments are not an object",
            "Model response contained an unidentified tool call.",
            "toolCallParser.hasInvalidToolCallIndex()",
            "toolCallParser.hasConflictingToolCallIdentity()",
            "toolCallParser.hasInvalidToolCallArguments()",
            "parsedToolCalls.every((toolCall) => Boolean(toolCall.id))",
        ),
        label=label,
    )
    _require(
        converter_source.count("requestContext.strictToolCalling") >= 7,
        f"{label}: batch and stream converters are not both under the terminal gate",
    )
    require_text(
        state,
        types,
        "strictToolCalling?: boolean;",
        count=1,
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
            "Parameter \"${unsupported[0]}\" is disabled",
            "this.config.getForegroundAgentsOnly()\n        ? false",
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
    _require_all(
        state,
        prompts,
        (
            "appendQwen38DeploymentContract",
            "appendQwen38MainSessionFrame(fs.readFileSync(systemMdPath, 'utf8'))",
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
            "Exploration is a role, not a mechanical read-only permission profile",
            "ToolNames.WRITE_FILE",
            "ToolNames.EDIT",
            "ToolNames.NOTEBOOK_EDIT",
            "hashed before and after your run",
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
            "if (runError) throw runError;",
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
            "const willRenderPdfImages = false;",
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
            "uses vLLM exact rendered-prompt counts with no heuristic pad or margin",
            "fails closed when exact request token counting is unavailable",
            "resamples one invalid pre-content stream in strict tool-calling mode",
            "never resamples an invalid strict stream after visible output escaped",
            "does not synthesize an output continuation after MAX_TOKENS in strict mode",
            "never enters a configured model fallback chain in strict mode",
            "maps maxRetries zero to one outer establishment attempt",
            "keeps XML text non-executable in strict tool-calling mode",
        ),
        "packages/core/src/services/chatCompressionService.test.ts": (
            "uses one cache-preserving main-model request with explicit phase budgets",
            "rejects unusable summary output",
            "summaryResult({ hadToolCall: true })",
            "requires an exact shrinking candidate with 20K of next-turn room",
        ),
        "packages/core/src/core/baseLlmClient.test.ts": (
            "uses the authoritative tokenizer and forwards the identical rendered-request options",
            "fails closed when exact counting is required but the generator lacks it",
            "returns the terminal streaming finish reason",
        ),
        "packages/core/src/core/openaiContentGenerator/pipeline.test.ts": (
            "applies bounded maintenance phase budgets to the OpenAI-compatible wire request",
            "combined phases above the request output ceiling",
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
    label = "session time-anchor result"
    prompt = "packages/core/src/core/qwen38-deployment-prompt.ts"
    # One timestamp computed at process start keeps the system prompt
    # byte-stable for the session (prefix-cache friendly) while giving the
    # model an absolute time anchor.
    require_text(
        state,
        prompt,
        "const QWEN38_SESSION_STARTED_AT_UTC = new Date().toISOString();",
        label=label,
    )
    require_text(
        state,
        prompt,
        "Session started: ${QWEN38_SESSION_STARTED_AT_UTC}",
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
        chat_source.count(
            "reactiveInfo.compressionStatus !== CompressionStatus.NOOP"
        )
        == 1,
        f"{label}: {chat} no longer reports the reactive overflow rescue",
    )
    _require(
        chat_source.count("type: StreamEventType.COMPACTION,") == 2,
        f"{label}: {chat} must report both compaction sites",
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
    require_text(
        state, adapter, "case GeminiEventType.ChatCompaction:", label=label
    )
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
        "            executionSummary,\n            turnsUsed,\n          },",
        count=2,
        label=label,
    )
    require_text(
        state,
        helpers,
        "          adapter.emitSubagentErrorResult(\n"
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
        "        ),",
        label=label,
    )
    require_text(
        state,
        agent_tool,
        "            toModelVisibleSubagentResult(\n"
        "              subagent.getFinalText(),\n"
        "              terminateMode,\n"
        "              subagent.getTurnsUsed(),\n"
        "              subagent.getLoopType(),\n"
        "            ),",
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
        "[subagent-scope] publishes the turn count with the terminal display "
        "status",
        label=label,
    )


def _validate_phase_budget_before(state: State) -> None:
    label = "maintenance phase-budget precondition"
    pipeline = "packages/core/src/core/openaiContentGenerator/pipeline.ts"
    # There is no per-request phase budget at all. The layer one would have to
    # survive is the provider hook called below, which merges the pinned
    # `extra_body` over whatever this file built.
    forbid_text(state, pipeline, "phaseBudgetOverrides", label=label)
    require_text(
        state,
        pipeline,
        "    let providerRequest = this.config.provider.buildRequest(",
        label=label,
    )
    # The single exit of the sampling-parameter branch, which the budget must
    # not be attached to, and the end of the wire request, where it must.
    require_text(
        state,
        pipeline,
        "      return clampProviderOutputBudgetKeys(",
        label=label,
    )
    require_text(state, pipeline, "    return providerRequest;", label=label)


def _validate_phase_budget_after(state: State) -> None:
    label = "maintenance phase-budget result"
    pipeline = "packages/core/src/core/openaiContentGenerator/pipeline.ts"
    pipeline_test = "packages/core/src/core/openaiContentGenerator/pipeline.test.ts"

    source = _require_all(
        state,
        pipeline,
        (
            "function applyPhaseBudgetOverrides(\n  wireRequest: Record<string, unknown>,",
            "  const configured = wireRequest[wireKey];",
            "  wireRequest['thinking_token_budget'] = overrides.thinkingTokenBudget;",
            "  wireRequest['final_response_token_budget'] =",
        ),
        label=label,
    )
    # Exactly one application, and it is the last thing done to the request,
    # after the provider hook has merged every pinned configuration layer.
    _require(
        source.count("applyPhaseBudgetOverrides(") == 2,
        f"{label}: {pipeline} must declare and apply the phase budget exactly once",
    )
    _require_ordered(
        source,
        (
            "let providerRequest = this.config.provider.buildRequest(",
            "applyPhaseBudgetOverrides(\n      typed,\n      request.phaseBudgetOverrides,\n"
            "      request.config?.maxOutputTokens,\n    );",
            "return providerRequest;",
        ),
        label=label,
        location=pipeline,
    )
    # The sampling-parameter layer no longer writes it, so it cannot be
    # overwritten downstream.
    forbid_text(
        state,
        pipeline,
        "(request as PromptCacheSharingParameters).phaseBudgetOverrides",
        label=label,
    )
    # The ceiling guard is unchanged in wording and now reads the merged wire
    # value, so a per-request budget above the pinned one is refused.
    require_text(
        state,
        pipeline,
        "budget must not exceed the pinned provider ceiling",
        label=label,
    )
    require_text(
        state,
        pipeline_test,
        "[phase-budget] overrides a phase budget pinned in extra_body, "
        "which the provider merges over the request",
        label=label,
    )
    require_text(
        state,
        pipeline_test,
        "[phase-budget] refuses a phase budget above a ceiling pinned in extra_body",
        label=label,
    )


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
        state, pdf, "Use the 'pages' parameter to read a specific page range", label=label
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
    files_test = "packages/core/src/utils/fileUtils.test.ts"

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
    # A supplied argument is refused rather than silently dropped, and the
    # refusal does not depend on the file type, so it cannot be evaded by
    # renaming the file.
    require_text(
        state,
        tool,
        "if ((params as { pages?: unknown }).pages !== undefined) {",
        label=label,
    )
    require_text(
        state,
        tool,
        "read_file has no 'pages' parameter and reads whole files.",
        label=label,
    )

    # One remedy, declared once and used by every message that has to send
    # the model somewhere, so no guidance names a parameter that is gone.
    require_text(state, pdf, "export const PDF_PAGE_RANGE_REMEDY =", label=label)
    forbid_text(state, pdf, "'pages' parameter", label=label)
    forbid_text(state, files, "'pages' parameter to", label=label)
    _require(
        _source(state, pdf, label=label).count("${PDF_PAGE_RANGE_REMEDY}") == 5
        and _source(state, files, label=label).count("${PDF_PAGE_RANGE_REMEDY}") == 3,
        f"{label}: every large/truncated PDF message must name the one remedy",
    )
    # The consumption layer keeps its fail-closed guard: a `pages` option
    # supplied to the shared utility on a non-PDF is refused, never ignored.
    require_text(
        state,
        files,
        "if (normalizedPages !== undefined && fileType !== 'pdf') {",
        label=label,
    )
    require_text(
        state,
        files,
        "errorType: ToolErrorType.INVALID_TOOL_PARAMS,",
        count=2,
        label=label,
    )
    # PDF page extraction itself is byte-for-byte untouched.
    require_text(
        state,
        files,
        "if (fileType === 'pdf' && normalizedPages !== undefined) {",
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
    require_text(
        state,
        files_test,
        "[pages-contract] rejects a pages parameter on a non-PDF file instead of ignoring it",
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
        "          turnsUsed: this.turnsUsed,\n          loopType: this.loopType,",
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
        "            event.loopType,\n          ),\n"
        "          turnsUsed: event.turnsUsed,",
        label=label,
    )
    # Both writers of the terminal display say the same thing the same way.
    require_text(
        state,
        agent_tool,
        "            terminateReason: describeSubagentTerminateReason(\n"
        "              terminateMode,\n              loopType,\n            ),",
        label=label,
    )
    require_text(
        state,
        agent_tool,
        "            executionSummary,\n            turnsUsed,\n          },",
        count=2,
        label=label,
    )
    # Both model-visible constructions carry the count and the rule.
    require_text(
        state,
        agent_tool,
        "          subagent.getTurnsUsed(),\n          subagent.getLoopType(),\n        ),",
        label=label,
    )
    require_text(
        state,
        agent_tool,
        "              subagent.getTurnsUsed(),\n              subagent.getLoopType(),\n            ),",
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
    require_text(state, cli, "const described = describeLoopType(loopType);", label=label)
    require_text(
        state,
        subagent_result,
        "export function describeSubagentTerminateReason(",
        label=label,
    )
    require_text(
        state,
        subagent_result,
        "      reason = `stopped as ${String(terminateMode).toLowerCase()}${detail}${turns}`;",
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
    turn = "packages/core/src/core/turn.ts"
    service = "packages/core/src/services/chatCompressionService.ts"
    service_test = "packages/core/src/services/chatCompressionService.test.ts"
    adapter_test = "packages/cli/src/nonInteractive/io/BaseJsonOutputAdapter.test.ts"

    # The record answers "where did the output budget go": the ceiling, both
    # phase budgets, what was produced, how much of it was hidden reasoning,
    # how much summary survived, and why generation stopped.
    _require_all(
        state,
        turn,
        (
            "export interface CompactionOutputAccounting {",
            "  maxOutputTokens: number;",
            "  thinkingTokenBudget: number;",
            "  finalResponseTokenBudget: number;",
            "  outputTokens: number;",
            "  thinkingTokens: number;",
            "  summaryChars: number;",
            "  finishReason: string | null;",
            "  output: CompactionOutputAccounting | null;",
            "    output: info.output ?? null,",
        ),
        label=label,
    )
    # Populated once from the single generation, and attached to every
    # outcome reachable after it, so no post-generation status can be silent
    # about the budget. A refusal before generation stays empty.
    service_source = _require_all(
        state,
        service,
        (
            "    const outputAccounting: CompactionOutputAccounting = {",
            "      thinkingTokens: summaryResult.usage?.thoughtsTokenCount ?? 0,",
            "      summaryChars: processedSummary.length,",
        ),
        label=label,
    )
    _require(
        service_source.count("const outputAccounting") == 1,
        f"{label}: {service} must derive the accounting exactly once",
    )
    _require(
        service_source.count("output: outputAccounting,") == 10,
        f"{label}: {service} must attach the accounting to every "
        "post-generation outcome",
    )
    require_text(
        state,
        service_test,
        "[compaction-event] records where a truncated attempt spent its output budget",
        label=label,
    )
    require_text(
        state,
        service_test,
        "[compaction-event] leaves the accounting null when no generation ran",
        label=label,
    )
    require_text(
        state,
        adapter_test,
        "[compaction-event] carries the failed attempt output accounting to the stream",
        label=label,
    )


CONCERNS: tuple[SemanticConcern, ...] = (
    SemanticConcern(
        name="locked-config-and-literal-cli",
        rationale=(
            "Upstream Qwen Code loads workspace settings, environment files, MCP "
            "servers, hooks, extensions, skills, rules, memory automation, output "
            "language, slash commands, and dynamic agent advertisements. In an "
            "unattended copied workspace those are alternate executable policy "
            "surfaces. The locked mode must keep ordinary QWEN.md/AGENTS.md task "
            "guidance while making every configuration/plugin surface inert and "
            "treating the submitted prompt literally."
        ),
        removal_condition=(
            "Remove only when upstream exposes one documented immutable mode that "
            "provably skips all listed sources before initialization, advertises no "
            "uncallable commands/agents, and has equivalent adversarial tests."
        ),
        validate_before=_validate_locked_boundary_before,
        validate_after=_validate_locked_boundary_after,
    ),
    SemanticConcern(
        name="exact-rendered-request-tokenization",
        rationale=(
            "Character ratios, cached provider usage, and image-token estimates do "
            "not describe the next fully rendered multimodal tool request. Both late "
            "compaction and max_tokens must use vLLM /tokenize with the same messages, "
            "tools, and template kwargs, require the same max_model_len, and use every "
            "real remaining token without a fabricated minimum or padding fallback."
        ),
        removal_condition=(
            "Remove only when upstream has a server-authoritative full-wire request "
            "tokenizer shared by compaction and generation, including tools/images and "
            "template kwargs, with mismatch/unavailability as hard errors and no "
            "heuristic branch for this provider."
        ),
        validate_before=_validate_exact_tokens_before,
        validate_after=_validate_exact_tokens_after,
    ),
    SemanticConcern(
        name="strict-stream-terminal-commit",
        rationale=(
            "A streaming prefix is diagnostic until the provider supplies a complete, "
            "identified, object-valued structured call with a tool_calls terminal. "
            "Upstream retry, continuation, XML recovery, and permissive batch/stream "
            "conversion can otherwise replay a turn or promote degenerate output into "
            "a local action. Strict mode therefore gets one establishment attempt and "
            "no semantic recovery; length-stopped prefixes stay non-executable."
        ),
        removal_condition=(
            "Remove only when upstream offers an equivalent end-to-end commit barrier "
            "covering HTTP establishment, invalid-stream replay, transport continuation, "
            "XML recovery, batch conversion, streaming conversion, and terminal reason."
        ),
        validate_before=_validate_stream_commit_before,
        validate_after=_validate_stream_commit_after,
    ),
    SemanticConcern(
        name="universal-tools-and-foreground-delegation",
        rationale=(
            "Upstream coreTools does not gate dynamic, MCP, skill, or synthetic tools, "
            "and Agent can create background/fork/team/worktree/model variants. The one "
            "service needs one canonical allowlist before every registry branch and only "
            "awaited general-purpose/Explore children, enforced in schema, validation, "
            "advertisement, and dispatch rather than by prompt advice."
        ),
        removal_condition=(
            "Remove only when upstream supplies a universal canonical-name allowlist "
            "and a schema/validator/dispatcher-enforced sequential foreground agent mode "
            "with no alternate creation parameters."
        ),
        validate_before=_validate_tool_policy_before,
        validate_after=_validate_tool_policy_after,
    ),
    SemanticConcern(
        name="deployment-prompts-subagent-scratch-and-effect-journal",
        rationale=(
            "The generic upstream main prompt advertises unavailable host, parallel, "
            "background, skill, and fallback behaviors, while upstream Explore is "
            "mechanically read-only and even forbids /tmp. The sealed deployment must "
            "replace that base with one immutable prompt contract, give both foreground "
            "roles unique private scratch, keep Explore computationally writable, and "
            "hash/journal every Explore workspace or artifact effect for the parent."
        ),
        removal_condition=(
            "Remove only when upstream can require an immutable distributor prompt and "
            "shared subagent contract, attach per-invocation scratch to every shell spawn, "
            "and provide Git-independent content-hash effect journaling without making "
            "Explore read-only or following untrusted symlinks."
        ),
        validate_before=_validate_deployment_prompt_scratch_before,
        validate_after=_validate_deployment_prompt_scratch_after,
    ),
    SemanticConcern(
        name="original-png-and-image-chronology",
        rationale=(
            "Upstream resizes/transcodes image overviews, accepts multiple/remote media "
            "forms, can fall through after decoder failures, and can render PDFs as lossy "
            "images. The backend accepts one full-quality source contract. Qwen Code must "
            "fully decode and validate a static 8-bit RGB/RGBA PNG, retain its exact bytes, "
            "reject every fallback transport, and keep text-image-text in the originating "
            "tool message so template chronology matches training-time semantics."
        ),
        removal_condition=(
            "Remove only when upstream can express and test the identical original-byte "
            "PNG bounds/decoder policy and non-splitting typed tool-result chronology, "
            "with no resize, transcode, remote/file media, or PDF-image fallback."
        ),
        validate_before=_validate_image_before,
        validate_after=_validate_image_after,
    ),
    SemanticConcern(
        name="model-field-isolation",
        rationale=(
            "Strict tool semantics and exact tokenization are model/provider capabilities, "
            "not ambient defaults. They must survive registry resolution for the pinned "
            "model yet be explicitly cleared when a subagent changes model identity, or a "
            "different provider could inherit an unsupported safety contract."
        ),
        removal_condition=(
            "Remove when upstream types and propagates equivalent provider-scoped fields "
            "and proves they cannot leak across a model override."
        ),
        validate_before=_validate_model_config_before,
        validate_after=_validate_model_config_after,
    ),
    SemanticConcern(
        name="behavioral-regression-evidence",
        rationale=(
            "Source landmarks establish intent but not behavior. Every safety boundary "
            "needs an executable broken/fixed or fail-closed probe in the exact compiled "
            "tree, including tokenizer mismatch, retries, XML, truncation, chronology, "
            "universal tool names, foreground agents, and original image bytes."
        ),
        removal_condition=(
            "Retire individual local tests only after equivalent upstream tests execute "
            "the same failure boundary in the pinned build; never remove the evidence "
            "merely because the implementation moved."
        ),
        validate_before=_validate_behavioral_evidence_before,
        validate_after=_validate_behavioral_evidence_after,
    ),
    SemanticConcern(
        name="wire-level-maintenance-phase-budget",
        rationale=(
            "Automatic compaction runs inside a fixed 20,000-token output "
            "ceiling and splits it deliberately: a bounded thinking phase "
            "and a reserved final-response phase, so the state snapshot has "
            "room to be written. The override was applied to the "
            "per-request sampling parameters, but the pinned deployment "
            "declares both phase budgets in extra_body, which the provider "
            "hook merges over the request. The split therefore never "
            "reached the wire: the summarizer ran at the pinned 262,144-"
            "token thinking budget inside a 20,000-token cap, and since "
            "thought parts are filtered out of the response, two "
            "consecutive attempts each spent ~19,900 output tokens and "
            "produced no summary at all. The session then died refusing to "
            "send 240,122 tokens against a 239,144-token limit. The "
            "existing ceiling guard could not catch it either: it compared "
            "against the sampling-parameter layer, which never declares "
            "these keys, so it read undefined and always passed. A "
            "per-request budget must be written where every configuration "
            "layer has already merged, and validated against the exact "
            "value it replaces."
        ),
        removal_condition=(
            "Remove only when upstream applies per-request phase budgets to "
            "the final wire request and validates them against the merged "
            "configured value, so no provider hook can silently restore the "
            "pinned budget."
        ),
        validate_before=_validate_phase_budget_before,
        validate_after=_validate_phase_budget_after,
    ),
    SemanticConcern(
        name="read-file-single-range-mechanism",
        rationale=(
            "read_file advertised a PDF-only `pages` parameter on every file "
            "type. A tool schema is the model's only map of what is "
            "callable, and a parameter whose applicability depends on the "
            "value of another parameter cannot be evaluated from that map, "
            "so the model used it as a line range on source files. Making "
            "the error message accurate did not change the behaviour: it "
            "was measured again afterwards, 106 times across 7 of 18 "
            "subagent scopes in one run, one subagent issuing 55 such "
            "calls, and three subagents repeating a byte-identical failing "
            "call until loop detection killed them. The affordance itself "
            "is the defect. read_file now reads whole files with exactly "
            "one range mechanism, offset/limit; page selection happens "
            "where it is unambiguous, a page-ranged pdftotext run in the "
            "shell, which is already this deployment's sealed PDF doctrine "
            "and is named by every message that has to send the model "
            "somewhere. A `pages` argument supplied anyway is refused at "
            "both the tool and the shared consumption layer, never dropped."
        ),
        removal_condition=(
            "Remove only when upstream read_file advertises no parameter "
            "that is valid for a subset of the file types it accepts, and "
            "refuses rather than ignores one that is supplied anyway."
        ),
        validate_before=_validate_pages_affordance_before,
        validate_after=_validate_pages_affordance_after,
    ),
    SemanticConcern(
        name="headless-stream-evidence",
        rationale=(
            "The headless stream-json output is captured as the session's "
            "evidentiary record. Upstream serialized tool results from the "
            "human-facing display string, so ranged file reads were recorded "
            "as a bare 'Read lines X-Y of Z' banner while the model received "
            "the full content — forensic analysis of production sessions "
            "misattributed model failures to information blackouts. The "
            "record must prefer the model-facing responseParts and fall back "
            "to the display only when no parts exist."
        ),
        removal_condition=(
            "Remove only when upstream stream-json emits the model-facing "
            "tool-result content for every tool by default."
        ),
        validate_before=_validate_stream_evidence_before,
        validate_after=_validate_stream_evidence_after,
    ),
    SemanticConcern(
        name="headless-compaction-event",
        rationale=(
            "Conversation compaction was invisible in the captured event "
            "stream. Upstream surfaces it only as an interactive UI signal "
            "and only when it succeeds, so the sole way to tell that a "
            "session compacted was to infer it from a drop in "
            "usage.input_tokens between consecutive billed assistant "
            "events — a reconstruction from a side effect that cannot "
            "distinguish an attempt that was refused or failed from one "
            "that never ran, and that shows nothing at all for a subagent, "
            "which compacts its own chat outside the parent's history. "
            "Every completed attempt is now emitted as a first-class "
            "system/compaction event carrying the rendered prompt token "
            "count before and after, the outcome, and the same "
            "parent_tool_use_id convention as every other message, so the "
            "main session and each subagent are separable from the "
            "standard bundle alone."
        ),
        removal_condition=(
            "Remove only when upstream emits a compaction event in "
            "stream-json for every attempt — successful, refused, and "
            "failed alike — attributed to the main session or the owning "
            "subagent tool call, and carrying the before/after rendered "
            "prompt token counts."
        ),
        validate_before=_validate_compaction_event_before,
        validate_after=_validate_compaction_event_after,
    ),
    SemanticConcern(
        name="subagent-result-scope-and-turn-count",
        rationale=(
            "A subagent that exhausted its inherited turn budget emitted a "
            "terminal result with no parent_tool_use_id and num_turns 0. "
            "Every other emitted event names its scope -- null for the main "
            "session, the agent tool-call id for a subagent -- so an unscoped "
            "result is read as the session's own: the subagent's failure was "
            "attributed to the parent, and the parent's subsequent recovery "
            "and real answer became output after the end of the session. The "
            "service correctly refused the whole capture, so every session in "
            "which a subagent ran out of turns was discarded even though the "
            "parent had finished. The turn count was lost twice over: the "
            "record said the subagent took no turns at all, and the "
            "model-visible text the parent reads was built by the foreground "
            "body from a second call that never received the count, while "
            "only the display-facing call inside runSubagentWithHooks had "
            "it. A subagent failure has to stay visible in the stream and "
            "scoped to the subagent, carrying how far it got."
        ),
        removal_condition=(
            "Remove only when upstream scopes every result message to the "
            "thread it ends -- null for the main session, the owning agent "
            "tool-call id for a subagent -- and reports that subagent's own "
            "completed turn count both on the emitted record and in the text "
            "the parent model receives."
        ),
        validate_before=_validate_subagent_result_scope_before,
        validate_after=_validate_subagent_result_scope_after,
    ),
    SemanticConcern(
        name="subagent-terminal-progress-and-loop-attribution",
        rationale=(
            "Every subagent error result recorded num_turns 0, whatever "
            "stopped it -- MAX_TURNS, TIMEOUT, ERROR, CANCELLED, "
            "LOOP_DETECTED alike -- while the real counts in the same "
            "capture were 11, 7 and 9. Nothing had lost the count: two "
            "writers flip the subagent display from running to failed, and "
            "the one that wins is the terminal event from the reasoning "
            "loop's own exit, which announced the status without it. The "
            "emitted record is built on the first running-to-failed "
            "transition, so the later, complete update from the tool body "
            "was never read. The terminal event now carries the terminal "
            "facts in full. It also carries which rule fired: LOOP_DETECTED "
            "covers nine rules across two tiers, and the subagent path "
            "never read the detector's last loop type, so a five-identical-"
            "call halt and an exhausted per-turn tool-call budget reached "
            "the operator and the parent model as the same word -- which is "
            "why diagnosing one took a five-hour telemetry reconstruction. "
            "The rule labels moved beside the detector so the headless "
            "session's message and a subagent's record name the same cause "
            "in the same words."
        ),
        removal_condition=(
            "Remove only when upstream's subagent terminal event carries "
            "the run's completed turn count and the loop rule that ended "
            "it, and both reach the emitted record and the text the parent "
            "model reads."
        ),
        validate_before=_validate_subagent_progress_before,
        validate_after=_validate_subagent_progress_after,
    ),
    SemanticConcern(
        name="compaction-output-accounting",
        rationale=(
            "A failed compaction was undiagnosable. The service captures "
            "the stream-json event file and enables no debug logging, so "
            "the '[chat-compression] summary terminated with MAX_TOKENS' "
            "warning and its siblings went nowhere, and the truncated "
            "summary was discarded unpersisted. Forensics on the session "
            "that died could establish that the 20,000-token maintenance "
            "budget had been exhausted but not how it split between the "
            "hidden thinking phase and the final response -- the one fact "
            "that identifies the failure. Turning on debug logging would be "
            "a second mode and would pollute the captured stream, so the "
            "native compaction event carries it instead: the ceiling, both "
            "phase budgets, the output tokens produced, how many of them "
            "were reasoning, how much summary survived, and the provider's "
            "terminal reason. It is attached to every outcome reachable "
            "after the generation and left empty when the attempt was "
            "refused before it, so a budget consumed entirely by reasoning "
            "stays distinguishable from a request that never ran."
        ),
        removal_condition=(
            "Remove only when upstream reports a compaction attempt's "
            "output-budget accounting -- at minimum the thinking/visible "
            "split and the terminal reason -- in the captured event stream "
            "without enabling debug logging."
        ),
        validate_before=_validate_compaction_accounting_before,
        validate_after=_validate_compaction_accounting_after,
    ),
    SemanticConcern(
        name="session-time-anchor",
        rationale=(
            "Benchmark forensics found zero time awareness across every "
            "session: agents never knew when they started. The deployment "
            "contract now ends with one session-start timestamp computed "
            "once at process start — an absolute anchor that keeps the "
            "system prompt byte-stable for the whole session, preserving "
            "prefix caching; live time remains observable via `date`."
        ),
        removal_condition=(
            "Remove only when upstream injects an equivalent stable "
            "session-start time into the system prompt by default."
        ),
        validate_before=_validate_session_time_before,
        validate_after=_validate_session_time_after,
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
