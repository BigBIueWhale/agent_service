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
            "gives every turn the window share, whatever the prompt costs",
            "refuses when the request cannot be counted at all",
            "resamples one invalid pre-content stream",
            "never resamples an invalid stream after visible output escaped",
            "publishes MAX_TOKENS without another request",
            "literal response preservation",
            "maps maxRetries zero to one outer establishment attempt",
            "literal-with-structure",
        ),
        "packages/core/src/services/chatCompressionService.test.ts": (
            "sends the ENTIRE history to one cache-preserving main-model request",
            "rejects unusable summary output",
            "summaryResult({ hadToolCall: true })",
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
    # and leaving the arithmetic to the reader.
    require_text(
        state,
        read_file,
        "To continue from where this read stopped, call read_file with file_path: ",
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

    # A page is a contiguous slice of the file: lines verbatim, and the
    # newline that terminates the last one whenever the file continues past
    # it, so the pages a caller is told to read concatenate to the file.
    _require_all(
        state,
        files,
        (
            "        const rangeReachedEof =",
            "        const linesIncluded = selectedLines.length;",
            "            index === selectedLines.length - 1 && rangeReachedEof",
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
        "            turnsUsed,\n          },",
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
        "                subagent.getTurnsUsed(),\n                subagent.getLoopType(),\n              ),",
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


# The window divides into this many shares, and these are the shares each
# budget takes. They are read out of the post-patch source rather than
# restated here, so the arithmetic below is a check on the tree and not a
# copy of it.
_PARTITION_SHARE_NAMES = (
    ("WINDOW_SHARES", None),
    ("SUMMARY_RESERVE_SHARES", "summaryReserve"),
    ("TURN_OUTPUT_SHARES", "turnOutput"),
    ("TOOL_RESULT_SHARES", "toolResult"),
    ("DIRECTIVE_RESERVE_SHARES", "directiveReserve"),
)

# The served deployment, and the budgets its window must yield.
_SERVED_WINDOW = 262_144
_SERVED_PARTITION = {
    "summaryReserve": 49_152,
    "turnOutput": 32_768,
    "toolResult": 16_384,
    "directiveReserve": 2_048,
    "compactionTrigger": 161_792,
}


def _read_partition_shares(source: str, *, label: str) -> dict[str, int]:
    shares: dict[str, int] = {}
    for name, _ in _PARTITION_SHARE_NAMES:
        match = re.search(rf"^const {name} = (\d+);$", source, re.MULTILINE)
        _require(
            match is not None,
            f"{label}: tokenLimits.ts does not declare {name} as an integer share",
        )
        shares[name] = int(match.group(1))
    return shares


def _partition(window: int, shares: dict[str, int]) -> dict[str, int]:
    total = shares["WINDOW_SHARES"]
    summary = (window * shares["SUMMARY_RESERVE_SHARES"]) // total
    turn = (window * shares["TURN_OUTPUT_SHARES"]) // total
    tool = (window * shares["TOOL_RESULT_SHARES"]) // total
    directive = (window * shares["DIRECTIVE_RESERVE_SHARES"]) // total
    return {
        "summaryReserve": summary,
        "turnOutput": turn,
        "toolResult": tool,
        "directiveReserve": directive,
        "compactionTrigger": window - summary - turn - tool - directive,
    }


def _validate_context_partition(state: State, *, label: str) -> None:
    """Every context budget is a share of the served window, and the shares
    spend the window exactly.

    This is the whole safety argument, checked here rather than at run time:
    a turn issued below the compaction trigger, given the turn-output share,
    appending at most the tool-result share, produces a history no larger
    than the window less the summary reserve — the largest history a
    whole-history summary request can still be issued for.
    """

    limits = "packages/core/src/core/tokenLimits.ts"
    source = _source(state, limits, label=label)
    shares = _read_partition_shares(source, label=label)
    reserved = sum(
        shares[name] for name, budget in _PARTITION_SHARE_NAMES if budget is not None
    )
    _require(
        all(value > 0 for value in shares.values()),
        f"{label}: every share must be a positive integer number of window shares",
    )
    _require(
        reserved < shares["WINDOW_SHARES"],
        f"{label}: the reserved shares leave no window for the compaction trigger",
    )
    for window in (
        256,
        4_096,
        131_072,
        200_000,
        202_752,
        _SERVED_WINDOW,
        _SERVED_WINDOW + 1,
        1_048_576,
    ):
        part = _partition(window, shares)
        _require(
            all(value > 0 for value in part.values()),
            f"{label}: a {window}-token window yields a non-positive budget {part!r}",
        )
        _require(
            sum(part.values()) == window,
            f"{label}: the budgets do not spend a {window}-token window exactly",
        )
        _require(
            part["compactionTrigger"]
            + part["turnOutput"]
            + part["toolResult"]
            + part["directiveReserve"]
            == window - part["summaryReserve"],
            f"{label}: a turn at a {window}-token window can outgrow its own summary",
        )
    served = _partition(_SERVED_WINDOW, shares)
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
    section_match = re.search(r"const SECTIONS = \[(.*?)\] as const;", snapshot, re.S)
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
    _require_all(
        state,
        snapshot_path,
        (
            "if (!text.trim()) refuse();",
            "if (section !== SECTIONS.length) refuse();",
            "return complete && depth === 0;",
        ),
        label="complete state snapshot",
    )
    label = "context-partition result"
    limits = "packages/core/src/core/tokenLimits.ts"
    limits_test = "packages/core/src/core/tokenLimits.test.ts"
    chat = "packages/core/src/core/geminiChat.ts"
    chat_test = "packages/core/src/core/geminiChat.test.ts"
    service = "packages/core/src/services/chatCompressionService.ts"
    service_test = "packages/core/src/services/chatCompressionService.test.ts"
    pipeline = "packages/core/src/core/openaiContentGenerator/pipeline.ts"
    generator = "packages/core/src/core/contentGenerator.ts"
    prompts = "packages/core/src/core/prompts.ts"
    prompts_test = "packages/core/src/core/prompts.test.ts"

    # The arithmetic itself, evaluated against the shares the tree declares.
    _validate_context_partition(state, label=label)

    # One derivation, in one place, with no tuned constant beside it.
    limits_source = _require_all(
        state,
        limits,
        (
            "export interface ContextPartition {",
            "export function partitionContextWindow(",
            "export function turnOutputBudget(",
            "      directiveReserve,",
            "return Math.min(outputCeiling, partition.turnOutput);",
        ),
        label=label,
    )
    budget_start = limits_source.index("export function turnOutputBudget(")
    budget_body = limits_source[budget_start:]
    _require(
        "prompt" not in budget_body.lower().split("}")[0],
        f"{label}: a turn's output budget depends on the prompt again",
    )
    for absent in (
        "MIN_CLAMPED_OUTPUT_TOKENS",
        "outputClampMargin",
        "compactionRoom",
        "clampOutputTokensToWindow",
        "clampExactOutputTokensToWindow",
    ):
        _require(
            absent not in limits_source,
            f"{label}: {limits} still carries the run-time budget term '{absent}'",
        )

    # A turn is issued below the compaction trigger or it is not issued.
    _require_ordered(
        _source(state, chat, label=label),
        (
            "promptTokensForClamp = await countExactRequestTokens(requestContents);",
            "if (promptTokensForClamp >= partition.compactionTrigger) {",
            "throw new Error(",
            "maxOutputTokens: turnOutputBudget(outputCeiling, partition),",
        ),
        label=label,
        location=chat,
    )
    for case in (
        "refuses when the directive outgrows the share reserved for it",
        "holds the request and the whole reserve inside the window at the largest history a turn can produce",
    ):
        require_text(state, service_test, case, label=label)
    for case in (
        "gives every turn the window share, whatever the prompt costs",
        "issues no turn once the rendered prompt reaches the compaction trigger",
        "refuses when the tokenizer reports another window",
        "refuses when the provider declares no context window",
    ):
        require_text(state, chat_test, case, label=label)
    for case in (
        "spends the served window on five shares and nothing else",
        "sums to the window exactly at every window size",
        "leaves a turn issued below the trigger inside the summarizable size",
        "re-derives every budget from a larger window with no code change",
        "refuses a window it cannot partition",
    ):
        require_text(state, limits_test, case, label=label)

    # The reserve covers the directive and the snapshot together, sized from
    # the request that was counted rather than from a margin.
    service_source = _require_all(
        state,
        service,
        (
            "const partition = partitionContextWindow(contextLimit);",
            "compactionOutputBudget = contextLimit - summaryRequestTokenCount;",
            "if (compactionOutputBudget < partition.summaryReserve) {",
            "if (directiveTokens > partition.directiveReserve) {",
            "      if (originalTokenCount < partition.compactionTrigger) {",
            "    if (newTokenCount >= partition.compactionTrigger) {",
            "          maxOutputTokens: compactionOutputBudget,",
        ),
        label=label,
    )
    # The snapshot is issued at the room the window actually has, and that room
    # is never less than the reserve: a turn is only issued while the
    # conversation is below the trigger, so the largest this request can become
    # is A + C + T + D, which is W - S. The floor is asserted in the service so
    # a broken partition fails loudly, rather than clamped so it silently
    # shrinks the snapshot -- a clamp is what hands a summary a few thousand
    # tokens and truncates it.
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

    # The compaction request is an ordinary one. Nothing splits the output
    # budget into phases, nothing shrinks or splits the input, and nothing
    # converges a failed attempt into a smaller retry.
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
    # The per-request phase-budget apparatus is gone from the layers that
    # carried it, so no caller can reintroduce a split budget.
    for path in (pipeline, generator):
        forbid_text(state, path, "phaseBudgetOverrides", label=label)
    # The request stays a pure extension of the conversation already in the
    # provider's prefix cache: same contents, same order, one appended
    # directive. Reshaping it would re-prefill the whole history.
    _require_all(
        state,
        service,
        (
            "      contents: [...sideQueryHistory, directiveContent],",
            "      promptCacheSharing: true,",
        ),
        label=label,
    )
    require_text(
        state,
        service_test,
        "sends the ENTIRE history to one cache-preserving main-model request",
        label=label,
    )
    # A truncated snapshot is refused before its content is examined, so one
    # that superficially parses complete can never be committed.
    _require_ordered(
        service_source,
        (
            "if (summaryResult.finishReason === FinishReason.MAX_TOKENS) {",
            "COMPRESSION_FAILED_OUTPUT_TRUNCATED",
            "if (!processedSummary) {",
        ),
        label=label,
        location=service,
    )
    require_text(
        state,
        service_test,
        "discards a truncated response even when its text parses as a complete snapshot",
        label=label,
    )
    # The snapshot schema gives each kind of fact one home; the retired
    # nine-section schema gave one code snippet four.
    _require_all(
        state,
        prompts,
        (
            "<intent>",
            "<environment>",
            "<completed>",
            "<in_progress>",
            "<learnings>",
            "<next_step>",
            "exactly once",
        ),
        label=label,
    )
    forbid_text(state, prompts, "<all_user_messages>", label=label)
    forbid_text(state, prompts, "<files_and_code_sections>", label=label)
    require_text(
        state,
        prompts_test,
        "states the writing contract: dense, deduplicated by structure",
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
    _require_all(
        state,
        "packages/core/src/services/postCompactAttachments.ts",
        (
            "const authoredParts = retainedInstructionParts(history)",
            "...authoredParts",
        ),
        label=label,
    )
    _require_all(
        state,
        "packages/core/src/services/state-snapshot.ts",
        (
            "new SaxesParser",
            "parser.on('doctype', refuse)",
            "parser.on('comment', refuse)",
            "tag.name !== SECTIONS[section]",
            "section !== SECTIONS.length",
        ),
        label=label,
    )
    require_text(state, service, "!isValidStateSnapshot(processedSummary)", label=label)


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
    source = _require_all(
        state,
        service,
        (
            "error instanceof GenerationTextFailure",
            "error.partial.requestAttempts > 0",
            "summary: error.partial.text",
            "reasoning: error.partial.thoughtText",
            "finishReason: error.partial.finishReason ?? null",
            "requestAttempts: error.partial.requestAttempts",
            "const outputAccounting: CompactionOutputAccounting = {",
            "usage: summaryUsage",
            "reasoning: summaryResult.thoughtText",
            "requestAttempts: summaryResult.requestAttempts",
        ),
        label=label,
    )
    _require(
        source.count("const outputAccounting") == 1
        and source.count("output: outputAccounting,") == 11,
        f"{label}: all post-generation outcomes must retain the same served evidence",
    )
    _require_all(
        state,
        "packages/core/src/services/chatCompressionService.test.ts",
        (
            "[compaction-event] records where a truncated attempt spent its output budget",
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
            "const incompleteGeneration = describeIncompleteGeneration(",
            "terminateMode: AgentTerminateMode.INCOMPLETE_GENERATION,",
        ),
        label=label,
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
    require_text(
        state,
        types,
        "| 'error_incomplete_generation'",
        label=label,
    )
    require_text(
        state,
        types,
        "    subtype: 'error_incomplete_generation',",
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
_DECLARED_SUCCESS = re.compile(
    r"export interface CLIResultMessageSuccess \{\n  type: 'result';\n"
    r"  subtype: '([a-z_]+)';"
)
_DECLARED_ERRORS = re.compile(r"\n  subtype:\n((?:    \| '[a-z_]+'(?:;|\n))+)")
_NAMED = re.compile(
    r"\n  \[AgentTerminateMode\.([A-Z_]+)\]: \{\n"
    r"    subtype: '([a-z_]+)',\n"
    r"    isError: (true|false),\n"
    r"    exitCode: (\d+),\n"
    r"  \},"
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
    # and builds; nothing else in this pipeline can see it. The four sets
    # below are read out of the post-patch source and required to agree: the
    # states an agentic run can end in, the states some statement in the tree
    # constructs, the states the table names, and the names the protocol
    # declares.
    #
    # The assertion is production, not reachability. Whether a given
    # deployment's argv can reach a producing statement is a path-sensitive
    # property of a call graph that no text-level check can decide, and the
    # state set is instead kept equal to the states this build can reach, so
    # the two coincide -- a property a reader checks by eye on an eight-member
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

    types_source = _source(state, types, label=label)
    success_match = _DECLARED_SUCCESS.search(types_source)
    _require(
        success_match is not None,
        f"{label}: {types} declares no success subtype",
    )
    declared_success = {success_match.group(1)}
    errors_match = _DECLARED_ERRORS.search(types_source)
    _require(
        errors_match is not None,
        f"{label}: {types} declares no error subtype union",
    )
    declared_errors = set(re.findall(r"'([a-z_]+)'", errors_match.group(1)))
    _require(
        len(declared_errors) == errors_match.group(1).count("|"),
        f"{label}: {types} declares a duplicate error subtype",
    )

    named = _NAMED.findall(
        types_source.split("export const TERMINAL_RESULT_BY_STATE = {", 1)[1].split(
            "} as const satisfies Record<AgentTerminateMode, TerminalResult>;", 1
        )[0]
    )
    _require(named, f"{label}: {types} maps no state to a terminal record")
    named_states = {entry[0] for entry in named}
    _require(
        named_states == state_names,
        f"{label}: the terminal-record table is not total over the states a run "
        f"can end in; states without a record: {sorted(state_names - named_states)!r}, "
        f"records without a state: {sorted(named_states - state_names)!r}",
    )
    named_success = {name for _, name, is_error, _ in named if is_error == "false"}
    named_errors = {name for _, name, is_error, _ in named if is_error == "true"}
    _require(
        named_success == declared_success,
        f"{label}: the success name the table gives a state and the one {types} "
        f"declares differ: named {sorted(named_success)!r}, "
        f"declared {sorted(declared_success)!r}",
    )
    _require(
        named_errors == declared_errors,
        f"{label}: the error names the table gives states and the ones {types} "
        f"declares differ; declared with no row in the table: "
        f"{sorted(declared_errors - named_errors)!r}, in the table but not "
        f"declared: {sorted(named_errors - declared_errors)!r}",
    )
    for _, name, is_error, _ in named:
        _require(
            name.startswith("error_") == (is_error == "true"),
            f"{label}: {types} pairs the name {name!r} with isError {is_error}",
        )

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
        for name in sorted(declared_errors):
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
    require_text(
        state,
        types_test,
        "names a scope that stopped in %s as %s",
        label=label,
    )
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
        "packages/core/src/utils/truncation.ts",
        (
            "truncateAndSaveToFile",
            "truncateToolOutput",
            "truncateLlmContent",
            "TOOL_OUTPUT_TRUNCATED_PREFIX",
            "COMBINED_PASS_TOLERANCE_FACTOR",
            "TRUNCATION_FALLBACK_ENVELOPE_SLACK",
        ),
    ),
    (
        "packages/core/src/utils/tool-response-finalizer.ts",
        (
            "enforceFunctionResponseBudget",
            "finalizeToolResponses",
            "allocateTextBudget",
            "fitText",
        ),
    ),
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


def _validate_tool_result_bound_after(state: State) -> None:
    store_path = "packages/core/src/utils/session-artifacts.ts"
    store = _source(state, store_path, label="durable artifact publication")
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
            "if (used + bytes.byteLength > MAX_SESSION_ARTIFACT_BYTES) {",
            "const file = await fs.open(filepath, 'wx', 0o400);",
            "await file.writeFile(bytes);",
            "await file.sync();",
            "await syncDirectoryAncestors(directory);",
        ),
        label="durable artifact publication",
        location=store_path,
    )
    label = "tool-result-bound result"
    chat = "packages/core/src/core/geminiChat.ts"
    chat_test = "packages/core/src/core/geminiChat.test.ts"
    truncation = "packages/core/src/utils/truncation.ts"
    truncation_test = "packages/core/src/utils/truncation.test.ts"
    finalizer = "packages/core/src/utils/tool-response-finalizer.ts"
    finalizer_test = "packages/core/src/utils/tool-response-finalizer.test.ts"

    # What a batch costs is the difference it makes to the request that will
    # be sent, counted twice by the tokenizer that will count the send.
    chat_source = _source(state, chat, label=label)
    _require_ordered(
        chat_source,
        (
            "private async boundPendingToolResults(",
            "let requestTokens = await countExactRequestTokens(",
            "const pending = pendingToolResults(userContent.parts ?? []);",
            "if (pending.length === 0) {",
            "const historyTokens = await countExactRequestTokens(",
            "this.getRequestHistoryForRoute(undefined, supportedModalities),",
            "for (const result of pending) {",
            "if (requestTokens - historyTokens <= partition.toolResult) break;",
            "parts: await referencePendingToolResult(",
            "requestTokens = await countExactRequestTokens(",
            "const toolResultTokens = requestTokens - historyTokens;",
            "if (toolResultTokens > partition.toolResult) {",
            "throw new Error(",
        ),
        label=label,
        location=chat,
    )
    # The bound runs before the compaction gate, and its count is the count
    # the gate uses, so the batch is never counted twice for two purposes.
    _require_ordered(
        chat_source,
        (
            "const pendingBatch = await this.boundPendingToolResults(",
            "userContent = pendingBatch.userContent;",
            "const effectiveTokens = pendingBatch.requestTokens;",
            "compressionInfo = await this.tryCompress(",
        ),
        label=label,
        location=chat,
    )

    # Displacement writes the result whole, largest first, and skips a result
    # that already stands for a file.
    _require_all(
        state,
        finalizer,
        (
            "export function pendingToolResults(parts: Part[]): PendingToolResult[] {",
            "if (isPersistedToolResult(value)) continue;",
            "return results.sort((a, b) => b.text.length - a.text.length);",
            "export async function referencePendingToolResult(",
            "const persisted = await persistToolResult(",
            "        [result.field]: result.protectedPrefix",
            "          ? `${result.protectedPrefix}${persisted.reference}`",
            "          : persisted.reference,",
        ),
        label=label,
    )
    # A reference that named no file, or a file holding part of the result,
    # would promise the model something it cannot read back.
    require_text(
        state,
        truncation,
        "await persistSessionArtifact(config, Buffer.from(content, 'utf8'), 'txt')",
        label=label,
    )
    store = "packages/core/src/utils/session-artifacts.ts"
    _require_all(
        state,
        store,
        (
            "MAX_SESSION_ARTIFACT_BYTES = 500 * 1024 * 1024",
            "await fs.readdir(directory)",
            "used + bytes.byteLength > MAX_SESSION_ARTIFACT_BYTES",
            "stale: Number.MAX_SAFE_INTEGER",
            "await fs.open(filepath, 'wx', 0o400)",
            "await file.sync()",
            "await syncDirectoryAncestors(directory)",
            "return withCleanup(",
            "createHash('sha256')",
        ),
        label=label,
    )
    for path in ("packages/core/src/utils/tool-output-cleanup.ts",):
        _require(
            path not in state, f"{label}: age-based artifact cleanup remains at {path}"
        )
    for symbol in ("trackToolResultBytes", "unusedResultPath", "cleanupOldToolOutputs"):
        forbid_text(state, truncation, symbol, label=label)
    require_text(
        state,
        "packages/core/src/tools/web-fetch.ts",
        "persistSessionArtifact(",
        label=label,
    )

    # One bound on one quantity: every character bound this replaces is gone
    # from the tree, including the per-tool budgets and the batch's
    # character water-filling.
    for path, symbols in _CHARACTER_BOUND_SITES:
        for symbol in symbols:
            forbid_text(state, path, symbol, label=label)

    # Executed in the build.
    for case in (
        "sends a batch inside its share untouched",
        "displaces the largest result to disk when the batch is over its share",
        "displaces only as many results as the share requires",
        "issues no turn when a batch of references is still over its share",
        "takes no second count for a turn that appends no tool result",
    ):
        require_text(state, chat_test, case, label=label)
    for case in (
        "writes the result whole and returns a reference naming the file",
        "refuses rather than dropping the result when the call id is unusable",
        "refuses when the write fails",
    ):
        require_text(state, truncation_test, case, label=label)
    for case in (
        "orders by size so the first displacement buys the most room",
        "skips a result that already stands for a file on disk",
        "keeps a protected prefix inline in front of the reference",
    ):
        require_text(state, finalizer_test, case, label=label)


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
    initialization = config.split("private async initializeOnce(", 1)[1].split(
        "private async initializeInternal(", 1
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
            "export interface GenerationUsageSummary",
            "usage: ServedUsage | null;",
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
        "createCompressionHistoryItem(outcome)", "await chatRecorder?.flush()",
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


CONCERNS: tuple[SemanticConcern, ...] = (
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
            "Native tool schemas explicitly close their parameter objects; external schemas retain "
            "their declared openness and pattern rules. Authenticated editor changes travel through "
            "private object provenance preserved by scheduler cloning, never hidden model parameter "
            "names."
        ),
        removal_condition=(
            "Upstream validates each schema as written, preserves trusted editor provenance outside "
            "model JSON, and refuses unsupported file ranges."
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
        name="subagent-result-scope-and-turn-count",
        rationale=(
            "Each child result carries its spawning tool-call scope and actual execution-round count. "
            "Root and child results cannot absorb each other’s state or usage."
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
            "All request budgets derive from shares of the declared served window. Exact before/after "
            "sizing governs compaction and tool-result displacement. Original authored inputs are "
            "retained independently of model summaries; summaries must satisfy the six-section "
            "structural contract. No phase budget forces reasoning to stop."
        ),
        removal_condition=(
            "Upstream provides the same partition arithmetic, exact request sizing, durable "
            "authored-input retention, and structural summary validation."
        ),
        validate_before=_validate_compaction_budget_before,
        validate_after=_validate_compaction_budget_after,
    ),
    SemanticConcern(
        name="tool-result-bound",
        rationale=(
            "The measured contribution of pending tool results fits one window share. Oversized results "
            "become references to complete immutable session artifacts. Text and binary producers share "
            "durable quota, exclusive publication, and ownership retention across Config recreation, "
            "resume, fork, and elapsed time."
        ),
        removal_condition=(
            "Upstream bounds actual rendered contribution and preserves full artifacts with durable "
            "concurrent accounting and reference-aware lifetime."
        ),
        validate_before=_validate_tool_result_bound_before,
        validate_after=_validate_tool_result_bound_after,
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
        name="terminal-state-is-a-value",
        rationale=(
            "A closed terminal-state value maps once to wire name, error classification, and exit code. "
            "The declared, constructed, and mapped state sets agree. Budget admission precedes "
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
