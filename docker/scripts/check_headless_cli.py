#!/usr/bin/env python3
"""Qualify the shipped CLI through production certification and an owned protocol fixture.

The CLI runs as the launcher runs it: its home is the sealed directory that holds the
settings, read in place, so the instructions production sends are the ones qualified,
and the fixture serves the model endpoint those settings name rather than a rewritten
copy of them.
"""
from __future__ import annotations

import argparse
import hashlib
from http.server import BaseHTTPRequestHandler, HTTPServer
import base64
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import signal
import subprocess
import tempfile
import threading
import urllib.parse

from verify_runtime_contract import cli_arguments
from request_evidence import require_request_evidence, require_response_evidence

# The canonical recording format this release's client writes
# (`CHAT_RECORDING_VERSION` in its transcript records); a recording of any
# other version is one this client refuses to resume.
CHAT_RECORDING_VERSION = 22
WORKSPACE = Path("/workspace")
# The client's own system-scope settings files. Production names no override for them and
# the image carries neither, so the sealed settings are the one source; a host that had
# either would qualify another configuration, and is refused instead.
SYSTEM_SETTINGS_FILES = (
    Path("/etc/qwen-code/settings.json"),
    Path("/etc/qwen-code/system-defaults.json"),
)
# The two-turn cycle's requests, in order: the startup proof's seven counts (the preamble,
# the preamble with its startup context, the compaction frame -- the snapshot's trailer --
# and the todo reminder, the framing
# text alone, the message, turn and tool-result probes, and the preamble in the shape a
# compaction request carries it), then for each turn the request it is about to issue,
# counted once because compaction leaves it as it was, and the generation. A tool result is
# bounded where it is made, so no turn counts a baseline without it.
EXPECTED_ROUTES = ["/tokenize"] * 8 + ["/v1/chat/completions"] + ["/tokenize"] + ["/v1/chat/completions"]
EXPECTED_ROLES = [
    ["system"],
    ["system", "user", "user", "user"],
    None,
    ["system", "user"],
    ["system", "user", "assistant"],
    ["system", "user", "assistant", "tool"],
    ["system", "user"],
    ["system", "user"],
    ["system", "user"],
    ["system", "user", "assistant", "tool"],
    ["system", "user", "assistant", "tool"],
]
# The proof's count of what a compaction request adds: the turn's tools with the snapshot's
# declaration after them, and the message that closes the request, stating the widest
# ceiling a draw can be issued with, the served window.
COMPACTION_PROBE = 6
SNAPSHOT_FUNCTION = "state_snapshot"


class SmokeFailure(RuntimeError):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise SmokeFailure(message)


def smoke_assistant_content(record: dict) -> dict:
    """Project this fixture's accepted text and call from its raw generation."""
    envelope = json.loads(record["generation"]["generation_json"])
    observed_parts = []
    for observation in envelope["observations"]:
        calls = {entry["part_index"]: entry["normalized_id"] for entry in observation["call_ids"]}
        candidates = observation["response"].get("candidates") or []
        primary = candidates[0] if candidates else {}
        for index, part in enumerate(primary.get("content", {}).get("parts", [])):
            if "functionCall" in part:
                require(index in calls, "canonical generation omitted a call identity")
                if calls[index] is None:
                    continue
                call = dict(part["functionCall"])
                call["id"] = calls[index]
                observed_parts.append({**part, "functionCall": call})
            else:
                observed_parts.append(dict(part))
    parts = []
    thoughts = [part for part in observed_parts if part.get("thought")]
    thought_text = "".join(part.get("text", "") for part in thoughts)
    if thought_text:
        signature = next((part["thoughtSignature"] for part in thoughts
                          if part.get("thoughtSignature")), None)
        parts.append({"text": thought_text, "thought": True,
                      **({"thoughtSignature": signature} if signature else {})})
    for part in observed_parts:
        if part.get("thought") or part.get("text") == "":
            continue
        if (parts and set(parts[-1]) == {"text"} and set(part) == {"text"}):
            parts[-1]["text"] += part["text"]
        else:
            require("text" in part or "functionCall" in part,
                    "smoke fixture produced an unexpected model part")
            parts.append(part)
    return {"role": "model", "parts": parts}


def certify(stdout: bytes, certifier: Path, events_path: Path) -> dict:
    with events_path.open("xb") as stream:
        os.fchmod(stream.fileno(), 0o600)
        stream.write(stdout)
        stream.flush()
        os.fsync(stream.fileno())
    result = subprocess.run([str(certifier), str(events_path)], capture_output=True, timeout=45)
    require(result.returncode == 0,
            f"production stream certification refused: {result.stdout!r}; {result.stderr!r}")
    certificate = json.loads(result.stdout)
    require(certificate["certification"]["state"] == "certified",
            "production reader did not supply a certificate")
    return certificate


def stub_address(settings: dict) -> tuple[str, int]:
    """The loopback address the sealed settings send the model's requests to."""
    base = urllib.parse.urlsplit(settings["model"]["baseUrl"])
    require(base.scheme == "http" and base.hostname == "127.0.0.1" and base.port is not None
            and base.path == "/v1", "the sealed model endpoint is not a loopback /v1 base URL")
    return base.hostname, base.port


# A number of bytes, tokens or turns, as the preamble writes one.
STATED_QUANTITY = re.compile(r"\b(\d[\d,]*) (bytes|tokens|turns)\b")
CONTEXT_HEADING = "\n## Context\n"
# The section closes the deployment contract; the next part of the system message
# begins after the separator the client puts between parts.
CONTEXT_END = "\n\n---\n\n"


def require_quantities_stated_once(system: str, tools: list) -> None:
    """Every budget the preamble states in bytes, tokens or turns is stated once, in the
    ## Context section the client renders from the partition. The sealed instructions and
    the tool declarations name those budgets rather than restate them, so no two parts of
    the preamble can give two values for one."""
    require(system.count(CONTEXT_HEADING) == 1, "the system message does not carry one ## Context section")
    start = system.index(CONTEXT_HEADING)
    require(CONTEXT_END in system[start:], "the ## Context section is not followed by the next part of the system message")
    end = system.index(CONTEXT_END, start)
    stated = []
    for match in STATED_QUANTITY.finditer(system):
        quantity = f"{match.group(1).replace(',', '')} {match.group(2)}"
        require(start <= match.start() < end,
                f"the system message states {quantity} outside its ## Context section")
        stated.append(quantity)
    require(len(stated) == len(set(stated)), f"the ## Context section states a budget twice: {stated}")
    declared = STATED_QUANTITY.search(json.dumps(tools, ensure_ascii=False))
    require(declared is None, f"a tool declaration states {declared and declared.group(0)!r}; "
            "it names the budget instead, which the ## Context section states once")


def sealed_digests(home: Path) -> dict[str, str]:
    return {str(path.relative_to(home)): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in sorted(home.rglob("*")) if path.is_file()}


def context_block(label: str, text: str) -> str:
    """A sealed file as the client carries it in the system message."""
    return f"--- Context from: {label} ---\n{text.strip()}\n--- End of Context from: {label} ---"


def require_production_requests(requests: list[dict], settings: dict, instructions: str,
                                instructions_label: str, output_language: str,
                                output_language_label: str, strict_tools: list[str],
                                turn_budget: int) -> None:
    """The requests are the ones production sends, in shape and in their sealed inputs."""
    routes = [r["path"] for r in requests]
    require(routes == EXPECTED_ROUTES, f"the two-turn cycle issued another request sequence: {routes}")
    model = settings["model"]["name"]
    config = settings["modelProviders"]["openai"][0]["generationConfig"]
    require(requests[2]["body"] == {"model": model, "prompt": "framing", "add_special_tokens": False},
            f"the framing probe drifted: {requests[2]['body']!r}")
    block = context_block(instructions_label, instructions)
    rule = context_block(output_language_label, output_language)
    tools = None
    for index, (request, roles) in enumerate(zip(requests, EXPECTED_ROLES)):
        body = request["body"]
        if roles is None:
            continue
        observed = [m.get("role") for m in body["messages"]]
        require(observed == roles, f"request {index} carries roles {observed}, not {roles}")
        system = body["messages"][0]["content"]
        # The sealed instructions travel in every system message, whole, once.
        require(isinstance(system, str) and system.count(block) == 1,
                f"request {index}'s system message does not carry the sealed QWEN.md once")
        # Upstream's output-language rule follows it, once, as the next context file.
        require(system.count(rule) == 1 and f"{block}\n\n{rule}" in system,
                f"request {index}'s system message does not carry the sealed output-language rule "
                "once, after the sealed QWEN.md")
        declared = body["tools"]
        if index == COMPACTION_PROBE:
            require(declared[-1]["function"]["name"] == SNAPSHOT_FUNCTION,
                    "the proof's compaction count does not declare the snapshot after the turn's tools")
            declared = declared[:-1]
            closing = body["messages"][-1]["content"]
            closing = closing if isinstance(closing, str) else "".join(
                part.get("text", "") for part in closing if part.get("type") == "text")
            require("your answer may generate at most 262144 tokens" in closing,
                    "the proof's compaction count does not state the widest ceiling a draw has")
        require_quantities_stated_once(system, declared)
        require(body["chat_template_kwargs"] == config["extra_body"]["chat_template_kwargs"],
                f"request {index}'s template arguments are not the sealed ones")
        tools = tools if tools is not None else declared
        require(declared == tools, f"request {index} declares other tools than the first")
        # Every turn states the budget a session that names none runs under; the startup
        # proof counts its line with the widest number left out.
        if index > COMPACTION_PROBE:
            require(f"- This session may run at most {turn_budget} turns." in system,
                    f"request {index} does not state the sealed {turn_budget}-turn budget")
    require(sorted(t["function"]["name"] for t in tools) == sorted(strict_tools),
            "the declared tools are not the launcher's strict tools")
    for index in (8, 10):
        body = requests[index]["body"]
        for key, value in config["samplingParams"].items():
            require(body[key] == value, f"generation {index} sends {key}={body[key]!r}, not {value!r}")
        for key, value in config["extra_body"].items():
            require(body[key] == value, f"generation {index} sends {key}={body[key]!r}, not {value!r}")


def qualify(stdout: bytes, runtime: Path, nonce: str, requests: list[dict], certifier: Path,
            deliverables: list[str]) -> dict:
    require(bool(stdout.strip()), "CLI emitted no events")
    require(stdout.endswith(b"\n"), "CLI emitted an unterminated event")
    certificate = certify(stdout, certifier, runtime / "events.jsonl")
    events = [json.loads(line) for line in stdout.splitlines()]
    require(not any(event.get("type", "").startswith("control_") for event in events),
            "SDK control records entered the non-SDK evidence stream; inspect stdout routing")
    request_evidence = require_request_evidence(events, requests)
    normalization_seeds = [event["normalization_seed"] for event in events
                           if event["type"] == "model_normalization_seed"]
    require(len(normalization_seeds) == 2 and
            [seed["request_id"] for seed in normalization_seeds] ==
            [request["request_id"] for request in request_evidence] and
            normalization_seeds[0]["history_call_ids"] == [] and
            "smoke_read" in normalization_seeds[1]["history_call_ids"],
            "the two Chat generations lost their request-bound history seeds")
    response_evidence = require_response_evidence(events, requests)
    require(all(response["event"]["status"] == "completed" for response in response_evidence if response["event"]["kind"] == "outcome"),
            "ordinary provider responses did not complete decoding; inspect the processing outcomes")
    require(not any(e.get("subtype") == "compaction" for e in events),
            "the two-generation fixture issued no compaction draw; inspect unexpected compaction evidence")
    init = [e for e in events if e.get("type") == "system" and e.get("subtype") == "init"]
    require(len(init) == 1, "CLI must emit exactly one init event")
    session = init[0]["session_id"]
    require(bool(session), "init has no session identity")
    require("read_file" in init[0]["tools"], "read_file was not initialized")
    require(init[0]["permission_mode"] == "yolo", "launcher approval policy drifted")
    require(all(e["session_id"] == session for e in events), "event ownership drifted")
    result = events[-1]
    if deliverables:
        # The model wrote its final message, and nothing wrote the declared paths: the run is not
        # a success, and its record names every path it owed, in declared order, as the
        # production certificate does.
        require(result["type"] == "result" and result["subtype"] == "error_missing_deliverables"
                and result["is_error"] is True and result["missing_deliverables"] == deliverables
                and all(path in result["error"]["message"] for path in deliverables),
                "a final message that left its declared deliverables unwritten was not reported as "
                "error_missing_deliverables naming each of them")
        certified = certificate["certification"]["result"]
        require(certified["subtype"] == "error_missing_deliverables"
                and certified["missing_deliverables"] == deliverables,
                "the production certificate lost the missing deliverables")
    else:
        require(result["type"] == "result" and result["subtype"] == "success" and result["is_error"] is False
                and "missing_deliverables" not in result, "CLI did not finish successfully")
        require(result["result"] == "HEADLESS_SMOKE_OK " + nonce, "final response lost the tool's fresh content")
    require(result["num_turns"] == 2, "CLI did not complete the two-turn tool cycle")
    require(result["usage"] == {
        "requests": 2, "usageReports": 2, "unfinalizedRequests": 0, "unreportedUsageRequests": 0,
        "usage": {"promptTokenCount": 64, "candidatesTokenCount": 16, "thoughtsTokenCount": 0,
                  "cachedContentTokenCount": 0, "totalTokenCount": 80},
    }, "terminal usage does not certify both served requests")
    blocks = [b for e in events if e.get("type") in ("assistant", "user")
              for b in e["message"]["content"]]
    uses = [b for b in blocks if b["type"] == "tool_use"]
    replies = [b for b in blocks if b["type"] == "tool_result"]
    require(len(uses) == len(replies) == 1, "expected one actual tool invocation and result")
    require(uses[0]["id"] == "smoke_read" and uses[0]["name"] == "read_file", "wrong tool invocation")
    require(replies[0] == {"type": "tool_result", "tool_use_id": "smoke_read", "is_error": False},
            "the stream did not report the call's return, or carried a copy of its result")
    generations = [r["body"] for r in requests if r["path"] == "/v1/chat/completions"]
    require(len(generations) == 2, "stub did not serve both generations")
    received = [message for message in generations[1]["messages"] if message.get("role") == "tool"]
    require(len(received) == 1 and received[0]["content"] ==
            [{"type": "text", "text": nonce + "\n"}],
            "the next request did not give the model the file content; inspect result serialization")
    carried_calls = [call for message in generations[1]["messages"]
                     if message.get("role") == "assistant" for call in message.get("tool_calls", [])]
    require(len(carried_calls) == 1 and carried_calls[0]["id"] == uses[0]["id"] and
            carried_calls[0]["function"]["name"] == uses[0]["name"] and
            json.loads(carried_calls[0]["function"]["arguments"]) == uses[0]["input"],
            "the next request rewrote the model's call; inspect canonical model-part commitment")
    require(all(g["kv_scope"] == session for g in generations), "provider requests lost their session owner")
    transcripts = list(runtime.rglob(f"chats/{session}.jsonl"))
    require(len(transcripts) == 1, "canonical session transcript is missing or ambiguous")
    raw = transcripts[0].read_bytes()
    require(bool(raw) and raw.endswith(b"\n"), "canonical transcript is empty or torn")
    records = [json.loads(line) for line in raw.splitlines()]
    require(all(type(r.get("recordingVersion")) is int and r["recordingVersion"] == CHAT_RECORDING_VERSION for r in records),
            "canonical recording version is missing or unknown; inspect the runtime writer before testing resume")
    require(all(all(field not in event for field in
                    ("recordingVersion", "checkpointVersion", "historyRevision", "afterCommit"))
                and event.get("subtype") != "runtime_history" for event in events),
            "canonical history entered stdout evidence; inspect the two recording paths")
    require(all(r["sessionId"] == session for r in records), "transcript ownership drifted")
    expected_origins = [{"kind": "model", "attempt_id": evidence["owner"]["attempt_id"],
                         "kv_scope": evidence["kv_scope"]}
                        for evidence in request_evidence if evidence["owner"]["kind"] == "chat"]
    fresh_assistants = [record for record in records
                        if record["type"] == "assistant" and record.get("subtype") is None]
    require([json.loads(record["generation"]["generation_json"])["origin"]
             for record in fresh_assistants] == expected_origins,
            "canonical assistant commits lost their exact producing attempts; inspect GeminiChat and the shared recorder")
    history = None
    edits = 0
    for record in records:
        if record.get("subtype") == "runtime_history":
            require("message" not in record, "history edits also claim display-message ownership")
            change = record["systemPayload"]
            if change["kind"] == "checkpoint":
                require(change["state"]["imagePayloads"] == [], "text fixture unexpectedly acquired images")
                history = list(change["state"]["history"])
            else:
                require(change["kind"] == "splice" and history is not None,
                        "runtime history omits initialization or contains an unknown edit")
                index, count = change["index"], change["deleteCount"]
                require(change["beforeLength"] == len(history) and 0 <= index <= len(history)
                        and 0 <= count <= len(history) - index and change["imagePayloads"] == [],
                        "runtime history edit does not address its recorded state")
                history[index:index + count] = change["insert"]
                edits += 1
        elif record["type"] == "assistant" and record.get("subtype") is None:
            require(history is not None and record.get("historyLength") == len(history),
                    "assistant commit does not address its recorded history position")
            history.append(smoke_assistant_content(record))
        elif record["type"] == "model_normalization_seed":
            require(history is not None, "normalization seed precedes the history checkpoint")
            identities = list(dict.fromkeys(
                part[owner]["id"] for content in history for part in content.get("parts", [])
                for owner in ("functionCall", "functionResponse") if owner in part and part[owner].get("id")
            ))
            require(record["normalizationSeed"]["history_call_ids"] == identities,
                    "normalization seed differs from the active canonical history")
    require(history is not None and edits >= 2, "both user and tool input need explicit history admission")
    require(history[-1] == smoke_assistant_content(fresh_assistants[-1]),
            "restored history lost the final assistant")
    restored_calls = [part["functionCall"] for content in history for part in content.get("parts", [])
                      if "functionCall" in part]
    restored_results = [part["functionResponse"] for content in history for part in content.get("parts", [])
                        if "functionResponse" in part]
    require(len(restored_calls) == len(restored_results) == 1
            and restored_calls[0]["id"] == restored_results[0]["id"] == uses[0]["id"]
            and nonce in json.dumps(restored_results[0], ensure_ascii=False),
            "explicit runtime history lost or duplicated the actual tool cycle")
    require(not any(record.get("subtype") == "generation_failure" for record in records),
            "the successful two-generation fixture recorded an abandoned attempt; inspect canonical disposition")
    require([r["modelRequest"] for r in records if r["type"] == "model_request"] == request_evidence,
            "canonical and stdout request evidence differ")
    require([r["modelUtilityRequest"] for r in records if r["type"] == "model_utility_request"] ==
            [e["utility_request"] for e in events if e["type"] == "model_utility_request"],
            "canonical and stdout physical utility requests differ")
    require([r["modelUtilityCompletion"] for r in records if r["type"] == "model_utility_completion"] ==
            [e["utility_completion"] for e in events if e["type"] == "model_utility_completion"],
            "canonical and stdout physical utility completions differ")
    require([r["modelResponse"] for r in records if r["type"] == "model_response"] ==
            [e["response"] for e in events if e["type"] == "model_response"],
            "canonical and stdout response evidence differ")
    require([r["normalizationSeed"] for r in records if r["type"] == "model_normalization_seed"] ==
            [e["normalization_seed"] for e in events if e["type"] == "model_normalization_seed"],
            "canonical and stdout normalization seeds differ")
    renders = {}
    bindings = {}
    for record in records:
        if record["type"] == "model_history_assertion":
            assertion = record["runtimeHistoryAssertion"]
            render_id = assertion["renderId"]
            require(isinstance(render_id, str) and render_id and render_id not in renders and
                    isinstance(assertion["stateSha256"], str) and
                    len(assertion["stateSha256"]) == 64 and
                    all(character in "0123456789abcdef" for character in assertion["stateSha256"]),
                    "canonical render assertion has no unique identity or state digest")
            renders[render_id] = assertion
        elif record["type"] == "model_history_binding":
            binding = record["runtimeHistoryBinding"]
            attempt_id = binding["attemptId"]
            require(isinstance(attempt_id, str) and attempt_id and
                    attempt_id not in bindings and binding["renderId"] in renders,
                    "canonical Chat attempt has no unique prior render assertion")
            bindings[attempt_id] = binding
        elif record["type"] == "model_request" and record["modelRequest"]["owner"]["kind"] == "chat":
            request = record["modelRequest"]
            binding = bindings.get(request["owner"]["attempt_id"])
            assertion = renders.get(binding["renderId"]) if binding else None
            require(assertion is not None and assertion["kvScope"] == request["kv_scope"] and
                    assertion["promptId"] == request["prompt_id"],
                    "canonical physical Chat request has no matching attempt-bound render")
    parent = None
    for record in records:
        require(record["parentUuid"] == parent, "canonical transcript chain is incomplete")
        if record["type"] in ("model_request", "model_utility_request", "model_response",
                              "model_utility_completion", "model_normalization_seed",
                              "model_generation", "model_attempt_completion",
                              "model_history_assertion", "model_history_binding"):
            require("message" not in record, "request evidence entered replayable history")
        else:
            parent = record["uuid"]
    telemetry = [r["systemPayload"]["uiEvent"] for r in records if r.get("subtype") == "ui_telemetry"]
    dispatches = [e for e in telemetry if e["event.name"] == "qwen-code.api_dispatch"]
    usages = [e for e in telemetry if e["event.name"] == "qwen-code.api_usage"]
    require(len(dispatches) == len(usages) == 2, "recorder did not retain both request/usage pairs")
    require({e["request_id"] for e in dispatches} == {e["request_id"] for e in usages}
            and len({e["request_id"] for e in dispatches}) == 2, "durable request pairing drifted")
    require(all(e["kv_scope"] == session for e in dispatches + usages), "durable generation ownership drifted")
    require(any(r.get("type") == "tool_result" and nonce in json.dumps(r) for r in records),
            "tool result was not recorded")
    recorded_calls = [part["functionCall"] for record in fresh_assistants
                      for part in smoke_assistant_content(record)["parts"] if "functionCall" in part]
    require(recorded_calls == [{"id": uses[0]["id"], "name": uses[0]["name"], "args": uses[0]["input"]}],
            "canonical history rewrote the model's call; inspect assistant recording before testing resume")
    # The final turn's completion evidence follows its commit; the last turn of
    # the conversation is the final answer.
    turns = [r for r in records if r["type"] in ("user", "assistant", "tool_result", "system")]
    require(turns[-1]["type"] == "assistant" and smoke_assistant_content(turns[-1])["parts"] ==
            [{"text": "HEADLESS_SMOKE_OK " + nonce}], "final response was not recorded before exit")
    require(records[-1]["type"] == "model_attempt_completion" and
            records[-1]["completion"]["disposition"] == "accepted",
            "the final turn's completion was not recorded before exit")
    return {"check": "headless_cli", "status": "passed", "events": len(events),
            "transcript_records": len(records), "served_requests": 2, "real_tool_calls": 1,
            "production_certificate": certificate}


def contract_identity(certifier: Path) -> str:
    result = subprocess.run([str(certifier), "--contract-identity"],
                            capture_output=True, check=True, timeout=45)
    identity = result.stdout.decode("ascii").removesuffix("\n")
    require(len(identity) == 64 and all(c in "0123456789abcdef" for c in identity),
            "production certifier returned an invalid contract identity")
    return identity


def check(entry: Path, settings_path: Path, launcher_source: Path, certifier: Path,
          deliverables: list[str]) -> dict:
    expected_contract = contract_identity(certifier)
    manifest_path = entry.parent / "stream-binding-manifest.json"
    try:
        manifest = json.loads(manifest_path.read_bytes())
    except (OSError, UnicodeError, ValueError) as cause:
        raise SmokeFailure(f"CLI contract identity cannot be read before provider admission: {cause}") from cause
    require(isinstance(manifest, dict) and manifest.get("schema_sha256") == expected_contract,
            "CLI and native certifier contract identities differ before provider admission")
    arguments = cli_arguments(launcher_source.read_text())
    strict_tools = [a.removeprefix("--strict-tools=") for a in arguments if a.startswith("--strict-tools=")]
    require(len(strict_tools) == 1, "the launcher names its strict tools once")
    settings = json.loads(settings_path.read_text())
    model = settings["model"]["name"]
    providers = settings["modelProviders"]["openai"]
    require(len(providers) == 1 and providers[0]["id"] == model, "shipping model declaration drifted")
    require(providers[0]["baseUrl"] == settings["model"]["baseUrl"], "the sealed provider names another endpoint")
    credential_key = providers[0]["envKey"]
    credential = settings["env"][credential_key]
    # QWEN_HOME is the sealed directory that holds these settings, as run_agent.sh sets it,
    # so the CLI reads the settings and the instructions beside them in place.
    require(settings_path.name == "settings.json", "the sealed settings are not a QWEN_HOME settings.json")
    sealed_home = settings_path.parent
    instructions_path = sealed_home / "QWEN.md"
    instructions = instructions_path.read_text()
    output_language_path = sealed_home / "output-language.md"
    output_language = output_language_path.read_text()
    for present in SYSTEM_SETTINGS_FILES:
        require(not present.exists(), f"{present} exists; the sealed settings would not be the only source")
    turn_budget = settings["model"]["maxSessionTurns"]
    sealed_before = sealed_digests(sealed_home)
    node = shutil.which("node")
    require(node is not None, "Node is missing")
    requests: list[dict] = []
    failures: list[str] = []
    generation_count = 0
    with tempfile.TemporaryDirectory(prefix="qwen-headless-smoke-") as temporary:
        root = Path(temporary)
        home, runtime = [root / name for name in ("home", "runtime")]
        workspace = WORKSPACE
        for directory in (home, runtime):
            directory.mkdir(mode=0o700)
        nonce = secrets.token_hex(16)
        fixture_fd, fixture_name = tempfile.mkstemp(prefix="headless-probe-", suffix=".txt", dir=workspace)
        fixture = Path(fixture_name)
        with os.fdopen(fixture_fd, "w") as contents:
            contents.write(nonce + "\n")

        class Stub(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def setup(self):
                super().setup()
                self.connection.settimeout(10)

            def reply(self, status, body, content_type="application/json"):
                self.send_response(status)
                self.send_header("Content-Type", content_type)
                self.send_header("Content-Length", str(len(body)))
                self.send_header("Connection", "close")
                self.end_headers()
                if hasattr(self, "response_record"):
                    self.response_record.update(response_status=status, response_content_type=content_type)
                    self.response_record["response_chunks"].append(base64.b64encode(body).decode())
                self.wfile.write(body)

            def do_POST(self):
                nonlocal generation_count
                try:
                    size = int(self.headers["Content-Length"])
                    require(0 < size < 2 * 1024 * 1024, "unexpected request size")
                    raw_body = self.rfile.read(size).decode("utf-8")
                    body = json.loads(raw_body)
                    self.response_record = {"path": self.path, "body": body, "raw_body": raw_body, "response_chunks": []}
                    requests.append(self.response_record)
                    require(len(requests) <= len(EXPECTED_ROUTES), "CLI exceeded the smoke request bound")
                    require(body["model"] == model, "CLI selected a different model")
                    if self.path == "/tokenize":
                        # Both forms the served /tokenize accepts: a rendered chat request, and a text
                        # alone, with no template and special tokens off.
                        if "messages" in body:
                            require(isinstance(body["messages"], list), "sizing lacks messages")
                        else:
                            require(isinstance(body.get("prompt"), str) and body.get("add_special_tokens") is False,
                                    "text sizing is not a template-free prompt with special tokens off")
                        self.reply(200, json.dumps({"count": 1024, "max_model_len": 262144}).encode())
                        return
                    require(self.path == "/v1/chat/completions", "unexpected provider route")
                    require(self.headers["Authorization"] == "Bearer " + credential, "CLI did not use its configured credential")
                    require(body["stream"] is True, "CLI did not use the streaming provider")
                    require(isinstance(body["kv_scope"], str) and bool(body["kv_scope"]), "request lacks an owner")
                    require(any(t.get("function", {}).get("name") == "read_file" for t in body["tools"]),
                            "provider tool schema lacks read_file")
                    # One result a turn rests on one call a turn, which the client's request builder
                    # asks for itself: no setting carries it, so the bundle is what must send it.
                    require(body.get("parallel_tool_calls") is False, "CLI did not ask for one call per turn")
                    # The model's own tokens travel in the recorded response only when the request
                    # asks for them, which the request builder does for every request.
                    require(body.get("return_token_ids") is True, "CLI did not ask for the generated token ids")
                    generation_count += 1
                    if generation_count == 1:
                        require(nonce not in json.dumps(body), "fixture content leaked into the initial prompt")
                        delta = {"tool_calls": [{"index": 0, "id": "smoke_read", "type": "function",
                            "function": {"name": "read_file", "arguments": json.dumps({"file_path": str(fixture), "offset": 0})}}]}
                        finish = "tool_calls"
                    elif generation_count == 2:
                        carried = [m for m in body["messages"] if m.get("role") == "assistant"]
                        require(len(carried) == 1 and carried[0].get("content") is None,
                                "carried model call acquired text the provider never produced; inspect history composition")
                        responses = [m for m in body["messages"] if m.get("role") == "tool"
                                     and m.get("tool_call_id") == "smoke_read"]
                        require(len(responses) == 1 and nonce in json.dumps(responses[0]),
                                "CLI did not execute and return the actual file read")
                        delta = {"content": "HEADLESS_SMOKE_OK " + nonce}
                        finish = "stop"
                    else:
                        raise SmokeFailure("unexpected extra generation")
                    chunk = {"id": f"smoke_{generation_count}", "object": "chat.completion.chunk", "created": 0,
                        "model": model, "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
                        "usage": {"prompt_tokens": 32, "completion_tokens": 8, "total_tokens": 40,
                                  "prompt_tokens_details": {"cached_tokens": 0},
                                  "completion_tokens_details": {"reasoning_tokens": 0}}}
                    self.response_record["served_usage"] = {
                        "promptTokenCount": chunk["usage"]["prompt_tokens"],
                        "candidatesTokenCount": chunk["usage"]["completion_tokens"],
                        "totalTokenCount": chunk["usage"]["total_tokens"],
                        "thoughtsTokenCount": chunk["usage"]["completion_tokens_details"]["reasoning_tokens"],
                        "cachedContentTokenCount": chunk["usage"]["prompt_tokens_details"]["cached_tokens"],
                    }
                    self.reply(200, ("data: " + json.dumps(chunk) + "\n\ndata: [DONE]\n\n").encode(), "text/event-stream")
                except Exception as error:
                    failures.append(str(error))
                    self.reply(500, json.dumps({"error": str(error)}).encode())

            def do_GET(self):
                failures.append("unexpected GET " + self.path)
                self.reply(500, b'{"error":"unexpected GET"}')

            def log_message(self, *_args):
                pass

        server = HTTPServer(stub_address(settings), Stub)
        worker = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.05})
        worker.start()
        try:
            # run_agent.sh's exports and agent_exec's one addition. The runtime root is the one
            # substitution: the image carries no session's runtime directory, and nothing the
            # model is sent names it.
            env = {"PATH": os.environ["PATH"], "HOME": str(home), "LANG": "C.UTF-8", "LC_ALL": "C.UTF-8",
                "QWEN_RUNTIME_DIR": str(runtime), "QWEN_HOME": str(sealed_home),
                "QWEN38_AGENT_SERVICE_LOCKED": "1", "QWEN_SYSTEM_MD": str(sealed_home / "system.md"),
                "QWEN_DEPLOYMENT_CONTRACT_MD": str(sealed_home / "deployment-contract.md"),
                credential_key: credential, "NO_COLOR": "1", "QWEN_TELEMETRY_ENABLED": "false",
                "XDG_CACHE_HOME": str(runtime / "cache"), "NPM_CONFIG_CACHE": str(runtime / "npm"),
                "PIP_CACHE_DIR": str(runtime / "pip"), "CARGO_HOME": str(runtime / "cargo"),
                "GOPATH": str(runtime / "go"),
                "QWEN_STREAM_CONTRACT_SHA256": expected_contract}
            # The budget a session that names none is launched with, and the deliverables list in
            # the one compact form the launcher passes it.
            command = [node, "--expose-gc", str(entry), *arguments, f"--max-session-turns={turn_budget}",
                       "--deliverables=" + json.dumps(deliverables, ensure_ascii=False, separators=(",", ":"))]
            prompt = f"Read {fixture} with read_file using offset 0, then reply HEADLESS_SMOKE_OK followed by its exact content.\n"
            try:
                with subprocess.Popen(
                    command, cwd=workspace, env=env, start_new_session=True, umask=0o077,
                    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                ) as process:
                    try:
                        stdout, stderr = process.communicate(prompt.encode(), timeout=45)
                    except BaseException:
                        try:
                            os.killpg(process.pid, signal.SIGKILL)
                        except ProcessLookupError:
                            pass  # The owned process group has already exited.
                        process.wait()
                        raise
            except subprocess.TimeoutExpired as error:
                raise SmokeFailure(
                    f"CLI exceeded 45 seconds; stdout={error.output!r}; stderr={error.stderr!r}"
                ) from error
            expected_exit = 1 if deliverables else 0
            require(process.returncode == expected_exit,
                    f"CLI exited {process.returncode}, not {expected_exit}; stdout={stdout!r}; stderr={stderr!r}")
            require(not failures, f"provider protocol failed: {failures}")
            require(sealed_digests(sealed_home) == sealed_before, "the run changed a file in the sealed home")
            result = qualify(stdout, runtime, nonce, requests, certifier, deliverables)
            require_production_requests(requests, settings, instructions,
                                        os.path.relpath(instructions_path, workspace), output_language,
                                        os.path.relpath(output_language_path, workspace),
                                        strict_tools[0].split(","), turn_budget)
            return result
        finally:
            server.shutdown()
            worker.join()
            server.server_close()
            fixture.unlink()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("entry", type=Path)
    parser.add_argument("settings", type=Path)
    parser.add_argument("launcher_source", type=Path)
    parser.add_argument("certifier", type=Path)
    args = parser.parse_args()
    # The same two-turn run twice: owing nothing, it succeeds; owing a file nothing writes, it ends
    # as error_missing_deliverables naming it, and the production certifier agrees.
    print(json.dumps({
        "declared_none": check(args.entry, args.settings, args.launcher_source, args.certifier, []),
        "declared_missing": check(args.entry, args.settings, args.launcher_source, args.certifier,
                                  ["headless-smoke/owed.md"]),
    }, sort_keys=True))


if __name__ == "__main__":
    main()
