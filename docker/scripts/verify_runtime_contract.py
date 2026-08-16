#!/usr/bin/env python3
"""Validate every sealed agent surface against the canonical runtime contract."""

from __future__ import annotations

import hashlib
import json
import re
import stat
import sys
from pathlib import Path
from typing import Any


EXPECTED_TOP_LEVEL = {
    "schema_version",
    "contract_id",
    "profile",
    "sealed_environment",
    "filesystem",
    "network",
    "model",
    "generation",
    "vision",
    "native_tools",
    "subagents",
    "execution",
    "toolchain",
    "components",
}
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")


class ContractError(RuntimeError):
    pass


def require_regular_file(path: Path, label: str) -> bytes:
    metadata = path.lstat()
    if not stat.S_ISREG(metadata.st_mode) or path.is_symlink():
        raise ContractError(f"{label} must be a regular, non-symlink file: {path}")
    raw = path.read_bytes()
    if not raw or not raw.endswith(b"\n") or b"\r" in raw:
        raise ContractError(f"{label} must be nonempty terminal-LF data without CR bytes")
    return raw


def load_json(path: Path, label: str) -> dict[str, Any]:
    raw = require_regular_file(path, label)
    try:
        value = json.loads(raw)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ContractError(f"{label} is invalid UTF-8 JSON: {error}") from error
    if not isinstance(value, dict):
        raise ContractError(f"{label} must be a JSON object")
    return value


def sha256(raw: bytes) -> str:
    return hashlib.sha256(raw).hexdigest()


def require_equal(label: str, observed: Any, expected: Any) -> None:
    if observed != expected:
        raise ContractError(f"{label} drift: expected {expected!r}, observed {observed!r}")


def require_false_map(label: str, value: Any, expected_keys: set[str]) -> None:
    if not isinstance(value, dict) or set(value) != expected_keys:
        raise ContractError(f"{label} fields differ from the canonical contract")
    for key in sorted(expected_keys):
        require_equal(f"{label}.{key}", value[key], False)


def require_fragments(label: str, text: str, fragments: list[str]) -> None:
    normalized_text = " ".join(text.split())
    for fragment in fragments:
        if " ".join(fragment.split()) not in normalized_text:
            raise ContractError(f"{label} is missing canonical fragment: {fragment!r}")


def verify_settings(contract: dict[str, Any], settings: dict[str, Any]) -> None:
    model = contract["model"]
    generation = contract["generation"]
    vision = contract["vision"]
    execution = contract["execution"]
    network = contract["network"]

    require_equal("settings model name", settings["model"]["name"], model["served_name"])
    require_equal(
        "settings model base URL", settings["model"]["baseUrl"], network["model_base_url"]
    )
    require_equal(
        "settings reasoning effort",
        settings["model"]["reasoningEffort"],
        generation["reasoning_effort"],
    )
    require_equal(
        "settings max session turns",
        settings["model"]["maxSessionTurns"],
        execution["max_session_turns"],
    )
    require_equal(
        "settings max wall time",
        settings["model"]["maxWallTimeSeconds"],
        execution["max_wall_time_seconds"],
    )
    require_equal(
        "settings max cumulative tool calls",
        settings["model"]["maxToolCalls"],
        execution["max_cumulative_tool_calls"],
    )
    require_equal(
        "settings per-turn tool-call circuit breaker",
        settings["model"]["maxToolCallsPerTurn"],
        execution["max_tool_calls_per_turn"],
    )
    require_equal("settings max subagent depth", settings["model"]["maxSubagentDepth"], 1)
    require_equal(
        "settings retained compacted images",
        settings["model"]["chatCompression"]["maxRecentImagesToRetain"],
        0,
    )
    for key in (
        "modelFallbacks",
        "fastModel",
        "visionModel",
        "compactionModel",
        "imageModel",
        "voiceModel",
    ):
        require_equal(f"settings {key}", settings[key], "")
    require_equal("settings chat recording", settings["general"]["chatRecording"], False)
    require_equal("settings sandbox", settings["tools"]["sandbox"], False)
    require_equal("settings auto compact threshold", settings["context"]["autoCompactThreshold"], 1.0)
    require_false_map(
        "settings memory",
        settings["memory"],
        {
            "enableManagedAutoMemory",
            "enableManagedAutoDream",
            "enableAutoSkill",
            "autoSkillConfirm",
            "enableTeamMemory",
            "enableTeamMemorySync",
        },
    )

    providers = settings["modelProviders"]
    if set(providers) != {"openai"} or not isinstance(providers["openai"], list) or len(providers["openai"]) != 1:
        raise ContractError("settings must expose exactly one OpenAI-compatible provider")
    provider = providers["openai"][0]
    require_equal("provider id", provider["id"], model["served_name"])
    require_equal("provider base URL", provider["baseUrl"], network["model_base_url"])
    require_equal("provider agent capability", provider["capabilities"]["agent"], True)
    require_equal("provider vision capability", provider["capabilities"]["vision"], True)
    config = provider["generationConfig"]
    require_equal("request retries", config["maxRetries"], generation["request_retries"])
    require_equal("mandatory thinking", config["thinkingMandatory"], True)
    require_equal("strict tool calling", config["strictToolCalling"], True)
    require_equal("token counter", config["exactTokenCounting"], "vllm")
    require_equal("split tool media", config["splitToolMedia"], vision["split_tool_media"])
    require_equal("tool result format", config["toolResultContentFormat"], "parts")
    require_equal("context window", config["contextWindowSize"], model["context_window_tokens"])
    require_equal(
        "modalities",
        config["modalities"],
        {"image": True, "pdf": False, "audio": False, "video": False},
    )
    require_equal(
        "sampling parameters",
        config["samplingParams"],
        {
            "temperature": generation["temperature"],
            "top_p": generation["top_p"],
            "top_k": generation["top_k"],
            "min_p": generation["min_p"],
            "presence_penalty": generation["presence_penalty"],
            "repetition_penalty": generation["repetition_penalty"],
            "max_tokens": generation["thinking_token_budget"],
        },
    )
    require_equal(
        "generation extra body",
        config["extra_body"],
        {
            "parallel_tool_calls": generation["parallel_tool_calls"],
            "reasoning_effort": generation["reasoning_effort"],
            "thinking_token_budget": generation["thinking_token_budget"],
            "final_response_token_budget": generation["final_response_token_budget"],
            "chat_template_kwargs": {
                "enable_thinking": generation["thinking_enabled"],
                "preserve_thinking": generation["preserve_thinking"],
                "reasoning_effort": generation["reasoning_effort"],
                "add_vision_id": False,
            },
        },
    )


def verify_prompts(
    contract: dict[str, Any], instructions: str, system: str, deployment: str
) -> None:
    model = contract["model"]
    generation = contract["generation"]
    vision = contract["vision"]
    require_equal(
        "deployment contract marker",
        deployment.splitlines()[0],
        "# QWEN38_DEPLOYMENT_CONTRACT_V1",
    )
    require_fragments(
        "system prompt",
        system,
        [
            "one long-lived, non-interactive local engineering session",
            "Tool calls are sequential.",
            "foreground `general-purpose` or `Explore` subagent",
            "deployment-contract section",
        ],
    )
    require_fragments(
        "deployment contract",
        deployment,
        [
            "disposable Docker agent, not the operator's host",
            "The agent has `--network none`",
            contract["network"]["model_base_url"],
            model["served_name"],
            model["checkpoint_repository"],
            model["checkpoint_revision"],
            "TurboQuant K8V4: FP8 keys and packed 4-bit values",
            f"{model['context_window_tokens']:,} total tokens",
            f"Thinking is always enabled at `{generation['reasoning_effort']}`",
            "`preserve_thinking=false`",
            f"Reasoning ceiling is {generation['thinking_token_budget']:,} tokens",
            f"final-response ceiling is {generation['final_response_token_budget']:,}",
            f"at most {vision['max_source_pixels_per_image']:,} pixels",
            f"aspect ratio at most {vision['max_aspect_ratio']}:1",
            "Explore is investigative in purpose, not mechanically read-only.",
            "Journal failure makes the tool call fail; changes are never silently reverted.",
            "PDF handling is local computation, not direct PDF vision.",
        ],
    )
    tool_section_match = re.search(
        r"- The only native protocol tools are:(.*?)\n- Local programs",
        deployment,
        flags=re.DOTALL,
    )
    if not tool_section_match:
        raise ContractError("deployment contract native-tool section is missing")
    observed_tools = re.findall(r"`([^`]+)`", tool_section_match.group(1))
    require_equal("deployment native tools", observed_tools, contract["native_tools"])
    require_fragments(
        "QWEN instructions",
        instructions,
        [
            "Explore is investigative in purpose, not mechanically read-only.",
            "There is no character estimate, byte division, padding margin, or token-count fallback.",
            "Completed historical thinking is intentionally omitted",
            "PDF is handled with deliberate offline computation, not direct PDF transport.",
            "Journal failure makes the tool call fail; useful changes are not",
        ],
    )


def verify_wrapper(contract: dict[str, Any], wrapper: str) -> None:
    sealed = contract["sealed_environment"]
    filesystem = contract["filesystem"]
    network = contract["network"]
    execution = contract["execution"]
    expected_network_fields = {
        "agent_network_mode",
        "model_base_url",
        "model_proxy_port",
        "model_relay_ready_event",
        "start_gate_file",
        "interfaces",
        "ipv4_addresses",
        "ipv6_addresses",
        "ipv4_routes",
        "ipv6_routes",
        "default_route",
        "dns",
        "internet",
        "host_network",
        "docker_socket",
        "gpu",
    }
    if set(network) != expected_network_fields:
        raise ContractError("network fields differ from the canonical contract")
    require_equal("agent network mode", network["agent_network_mode"], "none")
    require_equal("agent interfaces", network["interfaces"], ["lo"])
    require_equal("agent IPv4 addresses", network["ipv4_addresses"], ["127.0.0.1/8"])
    require_equal("agent IPv6 addresses", network["ipv6_addresses"], [])
    require_equal("agent IPv4 routes", network["ipv4_routes"], [])
    require_equal("agent IPv6 routes", network["ipv6_routes"], [])
    for denied_authority in (
        "default_route",
        "dns",
        "internet",
        "host_network",
        "docker_socket",
        "gpu",
    ):
        require_equal(f"network.{denied_authority}", network[denied_authority], False)
    require_equal("model preflight retry policy", execution["model_preflight_retries"], 0)
    require_equal("model preflight request count", execution["model_preflight_requests"], 1)
    expected_filesystem_fields = {
        "workspace",
        "artifacts",
        "streams",
        "output",
        "output_mounted_in_agent",
        "output_owner_component",
        "runtime",
        "effect_manifest_root",
        "subagent_scratch_root",
        "tmpfs_tmp",
        "tmpfs_runtime",
        "operator_source_writable",
        "agent_exec_sandbox",
    }
    if set(filesystem) != expected_filesystem_fields:
        raise ContractError("filesystem fields differ from the canonical contract")
    require_equal("agent stream mount", filesystem["streams"], "/streams")
    require_equal("agent output mount absence", filesystem["output_mounted_in_agent"], False)
    require_equal("output owner", filesystem["output_owner_component"], "session-capture")
    require_equal(
        "agent exec sandbox",
        filesystem["agent_exec_sandbox"],
        "landlock-fs-v4-write-roots-v1+output-unmounted-v1",
    )
    require_fragments(
        "agent wrapper",
        wrapper,
        [
            f"readonly SYSTEM_PROMPT_SOURCE={sealed['system_prompt_path']}",
            f"readonly DEPLOYMENT_CONTRACT_SOURCE={sealed['deployment_contract_path']}",
            f"readonly QWEN_HOME={sealed['qwen_home']}",
            f"readonly MODEL_BASE={network['model_base_url'].removesuffix('/v1')}",
            f"readonly EXPECTED_INTERFACE={network['interfaces'][0]}",
            f"readonly EXPECTED_IPV4_ADDRESS={network['ipv4_addresses'][0]}",
            f"readonly START_GATE_FILE={network['start_gate_file']}",
            f"export {sealed['marker_name']}={sealed['marker_value']}",
            "python3 \"${RUNTIME_CONTRACT_VERIFIER_SOURCE}\"",
            "python3 \"${TOOLCHAIN_VERIFIER_SOURCE}\"",
            "find /sys/class/net -mindepth 1 -maxdepth 1",
            "ip -o -4 address show",
            "ip -o -6 address show",
            "ip -4 route show",
            "ip -6 route show",
            "flock --exclusive \"${START_GATE_FILE}\" true",
            "--connect-timeout 2 --max-time 10",
            "readonly STREAMS_DIR=/streams",
            "readonly AGENT_EXEC=/opt/agent/agent_exec",
            "[[ ! -e /output ]]",
            "setsid \"${AGENT_EXEC}\"",
            "AGENT_EXEC_READY sandbox=${AGENT_EXEC_SANDBOX}",
            "printf 'EXEC\\n'",
        ],
    )
    if "--retry" in wrapper or "retry-connrefused" in wrapper:
        raise ContractError("agent wrapper must not retry its broker-gated model preflight")
    if re.search(r"/output/(events|qwen|ready|response)|setsid node|\btee\b", wrapper):
        raise ContractError("agent wrapper must not own or write service output files")


def verify_agent_exec(contract: dict[str, Any], source: str) -> None:
    filesystem = contract["filesystem"]
    require_fragments(
        "agent_exec source",
        source,
        [
            f'const SANDBOX_ID: &str = "{filesystem["agent_exec_sandbox"]}";',
            'const EVENTS_SOCKET: &str = "/streams/events.sock";',
            'const STDERR_SOCKET: &str = "/streams/stderr.sock";',
            'const NODE: &str = "/usr/local/bin/node";',
            'const CLI: &str = "/opt/qwen-code/scripts/cli-entry.js";',
            'std::path::Path::new("/output").exists()',
            '("/workspace", DIRECTORY_WRITE_ACCESS)',
            '("/artifacts", DIRECTORY_WRITE_ACCESS)',
            '("/tmp", DIRECTORY_WRITE_ACCESS)',
            '("/qwen-runtime", DIRECTORY_WRITE_ACCESS)',
            'libc::SYS_landlock_restrict_self',
            'libc::close_range(3, u32::MAX, 0)',
            '.arg("--foreground-agents-only")',
            '.arg("--max-subagent-depth=1")',
            '.arg("--max-session-turns=-1")',
            '.arg("--max-tool-calls=-1")',
        ],
    )
    match = re.search(r'const STRICT_TOOLS: &str =\s*"([^"]+)";', source)
    if not match:
        raise ContractError("agent_exec strict-tool declaration is missing")
    require_equal("agent_exec strict tools", match.group(1).split(","), contract["native_tools"])


def verify(paths: list[Path]) -> None:
    (
        contract_path,
        settings_path,
        instructions_path,
        system_path,
        deployment_path,
        toolchain_path,
        wrapper_path,
        agent_exec_source_path,
    ) = paths
    contract = load_json(contract_path, "runtime contract")
    if set(contract) != EXPECTED_TOP_LEVEL:
        raise ContractError(
            f"runtime contract fields differ: expected {sorted(EXPECTED_TOP_LEVEL)}, "
            f"observed {sorted(contract)}"
        )
    require_equal("runtime contract schema", contract["schema_version"], 1)
    require_equal("runtime contract id", contract["contract_id"], "qwen38-agent-runtime-v1")
    require_equal("runtime profile", contract["profile"], "qwen38-agent-service-v3")

    settings_raw = require_regular_file(settings_path, "settings")
    instructions_raw = require_regular_file(instructions_path, "instructions")
    system_raw = require_regular_file(system_path, "system prompt")
    deployment_raw = require_regular_file(deployment_path, "deployment contract")
    toolchain_raw = require_regular_file(toolchain_path, "toolchain manifest")
    wrapper_raw = require_regular_file(wrapper_path, "agent wrapper")
    agent_exec_source_raw = require_regular_file(agent_exec_source_path, "agent_exec source")
    components = contract["components"]
    for key, raw in (
        ("settings_sha256", settings_raw),
        ("instructions_sha256", instructions_raw),
        ("system_prompt_sha256", system_raw),
        ("deployment_contract_sha256", deployment_raw),
        ("toolchain_manifest_sha256", toolchain_raw),
        ("wrapper_sha256", wrapper_raw),
        ("agent_exec_source_sha256", agent_exec_source_raw),
    ):
        expected = components[key]
        if not isinstance(expected, str) or not SHA256_RE.fullmatch(expected):
            raise ContractError(f"runtime contract component hash is malformed: {key}")
        require_equal(key, sha256(raw), expected)

    settings = json.loads(settings_raw)
    toolchain = json.loads(toolchain_raw)
    require_equal(
        "toolchain apt lock identity",
        toolchain["apt_lock_sha256"],
        contract["toolchain"]["apt_lock_sha256"],
    )
    verify_settings(contract, settings)
    verify_prompts(
        contract,
        instructions_raw.decode("utf-8"),
        system_raw.decode("utf-8"),
        deployment_raw.decode("utf-8"),
    )
    verify_wrapper(contract, wrapper_raw.decode("utf-8"))
    verify_agent_exec(contract, agent_exec_source_raw.decode("utf-8"))


def main() -> int:
    if len(sys.argv) != 9:
        print(
            "usage: verify_runtime_contract.py CONTRACT SETTINGS INSTRUCTIONS "
            "SYSTEM DEPLOYMENT TOOLCHAIN WRAPPER AGENT_EXEC_SOURCE",
            file=sys.stderr,
        )
        return 2
    try:
        verify([Path(value) for value in sys.argv[1:]])
    except (ContractError, KeyError, OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        print(f"RUNTIME CONTRACT FAILURE: {error}", file=sys.stderr)
        return 1
    print("RUNTIME_CONTRACT_OK schema=1 id=qwen38-agent-runtime-v1")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
