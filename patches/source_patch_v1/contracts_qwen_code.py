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
            "fs.readFileSync(systemMdPath, 'utf8')",
        ),
        label=label,
    )
    _require_all(
        state,
        core,
        (
            "getCurrentQwen38SubagentExecution",
            "appendQwen38DeploymentContract(finalPrompt)",
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


def _validate_nonpdf_pages_before(state: State) -> None:
    label = "non-PDF pages rejection precondition"
    tool = "packages/core/src/tools/read-file.ts"
    files = "packages/core/src/utils/fileUtils.ts"
    forbid_text(
        state, tool, "applies only to PDF files", label=label
    )
    forbid_text(
        state, files, "applies only to PDF files", label=label
    )
    require_text(
        state,
        files,
        "if (fileType === 'pdf' && normalizedPages !== undefined) {",
        label=label,
    )


def _validate_nonpdf_pages_after(state: State) -> None:
    label = "non-PDF pages rejection result"
    tool = "packages/core/src/tools/read-file.ts"
    files = "packages/core/src/utils/fileUtils.ts"
    tool_test = "packages/core/src/tools/read-file.test.ts"
    files_test = "packages/core/src/utils/fileUtils.test.ts"
    # The file-type decision precedes every pages syntax check, names the
    # real remedy, and exists at both the validation and consumption layers
    # so no caller can silently drop a supplied pages parameter.
    require_text(
        state,
        tool,
        "if (params.pages !== undefined && ext !== '.pdf') {",
        label=label,
    )
    require_text(
        state,
        tool,
        "applies only to PDF files; '${path.basename(filePath)}' is not a PDF."
        " For text files, use 'offset' and 'limit' instead.",
        label=label,
    )
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
    # PDF behavior is byte-for-byte untouched.
    require_text(
        state,
        files,
        "if (fileType === 'pdf' && normalizedPages !== undefined) {",
        label=label,
    )
    require_text(
        state,
        tool_test,
        "[pages-contract] rejects pages on a non-PDF file before any syntax validation",
        label=label,
    )
    require_text(
        state,
        tool_test,
        "[pages-contract] still accepts a bounded pages range on a PDF",
        label=label,
    )
    require_text(
        state,
        files_test,
        "[pages-contract] rejects a pages parameter on a non-PDF file instead of ignoring it",
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
        name="non-pdf-pages-rejection",
        rationale=(
            "Production benchmark forensics showed three agent runs dying in "
            "identical read_file loops: the PDF-only pages parameter used as a "
            "line range on source files. Upstream validated pages syntax before "
            "knowing the file type, reported a PDF capacity error on text files, "
            "and silently ignored pages small enough to pass, so the model's "
            "only feedback pointed away from the real fix. The file-type "
            "decision must come first, the error must name the remedy "
            "(offset/limit), and a supplied parameter must never be silently "
            "dropped at the consumption layer."
        ),
        removal_condition=(
            "Remove only when upstream read_file rejects pages on non-PDF "
            "inputs at both the validation and consumption layers with an "
            "error that names offset/limit as the text-file remedy."
        ),
        validate_before=_validate_nonpdf_pages_before,
        validate_after=_validate_nonpdf_pages_after,
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
