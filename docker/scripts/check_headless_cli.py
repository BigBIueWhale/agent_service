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

WORKSPACE = Path("/workspace")
# The client's own system-scope settings files. Production names no override for them and
# the image carries neither, so the sealed settings are the one source; a host that had
# either would qualify another configuration, and is refused instead.
SYSTEM_SETTINGS_FILES = (
    Path("/etc/qwen-code/settings.json"),
    Path("/etc/qwen-code/system-defaults.json"),
)
# The two-turn cycle's requests, in order: the startup proof's six counts (the preamble,
# the preamble with its startup context, the compaction frame -- the snapshot's trailer,
# the acknowledgement and a retained input's header -- and the todo reminder, the framing
# text alone, and the message, turn and tool-result probes), then for each turn the request with its
# pending message counted, the request about to be issued counted, and the generation. A
# tool result is bounded where it is made, so no turn counts a baseline without it.
EXPECTED_ROUTES = ["/tokenize"] * 8 + ["/v1/chat/completions"] + ["/tokenize"] * 2 + ["/v1/chat/completions"]
EXPECTED_ROLES = [
    ["system"],
    ["system", "user", "user", "assistant", "user", "user"],
    None,
    ["system", "user"],
    ["system", "user", "assistant"],
    ["system", "user", "assistant", "tool"],
    ["system", "user"],
    ["system", "user"],
    ["system", "user"],
    ["system", "user", "assistant", "tool"],
    ["system", "user", "assistant", "tool"],
    ["system", "user", "assistant", "tool"],
]


class SmokeFailure(RuntimeError):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise SmokeFailure(message)


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
CONTEXT_END = "\n\nCLI invocation started: "


def require_quantities_stated_once(system: str, tools: list) -> None:
    """Every budget the preamble states in bytes, tokens or turns is stated once, in the
    ## Context section the client renders from the partition. The sealed instructions and
    the tool declarations name those budgets rather than restate them, so no two parts of
    the preamble can give two values for one."""
    require(system.count(CONTEXT_HEADING) == 1, "the system message does not carry one ## Context section")
    start = system.index(CONTEXT_HEADING)
    require(CONTEXT_END in system[start:], "the ## Context section is not followed by the invocation line")
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


def require_production_requests(requests: list[dict], settings: dict, instructions: str,
                                instructions_label: str, strict_tools: list[str], turn_budget: int) -> None:
    """The requests are the ones production sends, in shape and in their sealed inputs."""
    routes = [r["path"] for r in requests]
    require(routes == EXPECTED_ROUTES, f"the two-turn cycle issued another request sequence: {routes}")
    model = settings["model"]["name"]
    config = settings["modelProviders"]["openai"][0]["generationConfig"]
    require(requests[2]["body"] == {"model": model, "prompt": "framing", "add_special_tokens": False},
            f"the framing probe drifted: {requests[2]['body']!r}")
    block = (f"--- Context from: {instructions_label} ---\n{instructions.strip()}\n"
             f"--- End of Context from: {instructions_label} ---")
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
        require_quantities_stated_once(system, body["tools"])
        require(body["chat_template_kwargs"] == config["extra_body"]["chat_template_kwargs"],
                f"request {index}'s template arguments are not the sealed ones")
        tools = tools if tools is not None else body["tools"]
        require(body["tools"] == tools, f"request {index} declares other tools than the first")
        # Every turn states the budget a session that names none runs under; the startup
        # proof counts its line with the widest number left out.
        if index >= 6:
            require(f"- This session may run at most {turn_budget} turns." in system,
                    f"request {index} does not state the sealed {turn_budget}-turn budget")
    require(sorted(t["function"]["name"] for t in tools) == sorted(strict_tools),
            "the declared tools are not the launcher's strict tools")
    for index in (8, 11):
        body = requests[index]["body"]
        for key, value in config["samplingParams"].items():
            require(body[key] == value, f"generation {index} sends {key}={body[key]!r}, not {value!r}")
        for key, value in config["extra_body"].items():
            require(body[key] == value, f"generation {index} sends {key}={body[key]!r}, not {value!r}")


def qualify(stdout: bytes, runtime: Path, nonce: str, requests: list[dict], certifier: Path) -> dict:
    require(bool(stdout.strip()), "CLI emitted no events")
    require(stdout.endswith(b"\n"), "CLI emitted an unterminated event")
    certificate = certify(stdout, certifier, runtime / "events.jsonl")
    events = [json.loads(line) for line in stdout.splitlines()]
    init = [e for e in events if e.get("type") == "system" and e.get("subtype") == "init"]
    require(len(init) == 1, "CLI must emit exactly one init event")
    session = init[0]["session_id"]
    require(bool(session), "init has no session identity")
    require("read_file" in init[0]["tools"], "read_file was not initialized")
    require(init[0]["permission_mode"] == "yolo", "launcher approval policy drifted")
    require(all(e["session_id"] == session for e in events), "event ownership drifted")
    result = events[-1]
    require(result["type"] == "result" and result["subtype"] == "success" and result["is_error"] is False,
            "CLI did not finish successfully")
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
    require(replies[0]["tool_use_id"] == "smoke_read" and replies[0]["is_error"] is False
            and replies[0]["content"] == nonce + "\n", "tool result did not carry the file content")
    generations = [r["body"] for r in requests if r["path"] == "/v1/chat/completions"]
    require(len(generations) == 2, "stub did not serve both generations")
    require(all(g["kv_scope"] == session for g in generations), "provider requests lost their session owner")
    transcripts = list(runtime.rglob(f"chats/{session}.jsonl"))
    require(len(transcripts) == 1, "canonical session transcript is missing or ambiguous")
    raw = transcripts[0].read_bytes()
    require(bool(raw) and raw.endswith(b"\n"), "canonical transcript is empty or torn")
    records = [json.loads(line) for line in raw.splitlines()]
    require(all(r["sessionId"] == session for r in records), "transcript ownership drifted")
    require(records[0]["parentUuid"] is None and
            all(r["parentUuid"] == p["uuid"] for p, r in zip(records, records[1:])),
            "canonical transcript chain is incomplete")
    telemetry = [r["systemPayload"]["uiEvent"] for r in records if r.get("subtype") == "ui_telemetry"]
    dispatches = [e for e in telemetry if e["event.name"] == "qwen-code.api_dispatch"]
    usages = [e for e in telemetry if e["event.name"] == "qwen-code.api_usage"]
    require(len(dispatches) == len(usages) == 2, "recorder did not retain both request/usage pairs")
    require({e["request_id"] for e in dispatches} == {e["request_id"] for e in usages}
            and len({e["request_id"] for e in dispatches}) == 2, "durable request pairing drifted")
    require(all(e["kv_scope"] == session for e in dispatches + usages), "durable generation ownership drifted")
    require(any(r.get("type") == "tool_result" and nonce in json.dumps(r) for r in records),
            "tool result was not recorded")
    require(records[-1]["type"] == "assistant" and records[-1]["message"]["parts"] ==
            [{"text": "HEADLESS_SMOKE_OK " + nonce}], "final response was not recorded before exit")
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


def check(entry: Path, settings_path: Path, launcher_source: Path, certifier: Path) -> dict:
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
                self.wfile.write(body)

            def do_POST(self):
                nonlocal generation_count
                try:
                    size = int(self.headers["Content-Length"])
                    require(0 < size < 2 * 1024 * 1024, "unexpected request size")
                    body = json.loads(self.rfile.read(size))
                    requests.append({"path": self.path, "body": body})
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
                    generation_count += 1
                    if generation_count == 1:
                        require(nonce not in json.dumps(body), "fixture content leaked into the initial prompt")
                        delta = {"tool_calls": [{"index": 0, "id": "smoke_read", "type": "function",
                            "function": {"name": "read_file", "arguments": json.dumps({"file_path": str(fixture), "offset": 0})}}]}
                        finish = "tool_calls"
                    elif generation_count == 2:
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
            # substitution: the image has no /qwen-runtime tmpfs, and nothing the model is sent
            # names it.
            env = {"PATH": os.environ["PATH"], "HOME": str(home), "LANG": "C.UTF-8", "LC_ALL": "C.UTF-8",
                "QWEN_RUNTIME_DIR": str(runtime), "QWEN_HOME": str(sealed_home),
                "QWEN38_AGENT_SERVICE_LOCKED": "1", "QWEN_SYSTEM_MD": str(sealed_home / "system.md"),
                "QWEN_DEPLOYMENT_CONTRACT_MD": str(sealed_home / "deployment-contract.md"),
                credential_key: credential, "NO_COLOR": "1", "QWEN_TELEMETRY_ENABLED": "false",
                "XDG_CACHE_HOME": str(runtime / "cache"), "NPM_CONFIG_CACHE": str(runtime / "npm"),
                "PIP_CACHE_DIR": str(runtime / "pip"), "CARGO_HOME": str(runtime / "cargo"),
                "GOPATH": str(runtime / "go"),
                "QWEN_STREAM_CONTRACT_SHA256": expected_contract}
            # The budget a session that names none is launched with.
            command = [node, "--expose-gc", str(entry), *arguments, f"--max-session-turns={turn_budget}"]
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
            require(process.returncode == 0, f"CLI exited {process.returncode}; stdout={stdout!r}; stderr={stderr!r}")
            require(not failures, f"provider protocol failed: {failures}")
            require(sealed_digests(sealed_home) == sealed_before, "the run changed a file in the sealed home")
            result = qualify(stdout, runtime, nonce, requests, certifier)
            require_production_requests(requests, settings, instructions,
                                        os.path.relpath(instructions_path, workspace), strict_tools[0].split(","),
                                        turn_budget)
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
    print(json.dumps(check(args.entry, args.settings, args.launcher_source, args.certifier), sort_keys=True))


if __name__ == "__main__":
    main()
