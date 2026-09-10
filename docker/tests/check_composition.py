#!/usr/bin/env python3
"""Exercise candidate images through production launch, capture and publication.

The two mounted test executables replace only deployed-stack bootstrap. The
protocol stub is CPU-only and binds the owned Unix socket behind the real relay.
Every container operation names a registered fixture actor; no global sweep is
available to this harness. Evidence remains in the printed owned directory.
"""
from __future__ import annotations

import argparse
import fcntl
import hashlib
from http.server import BaseHTTPRequestHandler
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import socket
import socketserver
import stat
import subprocess
import tempfile
import threading
import time
import uuid
import zipfile


class GateFailure(RuntimeError):
    pass


def require(condition, message):
    if not condition:
        raise GateFailure(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def save(path, data):
    with path.open("xb") as stream:
        os.fchmod(stream.fileno(), 0o600)
        stream.write(data)
        stream.flush()
        os.fsync(stream.fileno())


def save_json(path, data):
    save(path, (json.dumps(data, indent=2) + "\n").encode())


class StubServer(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = False
    block_on_close = True

    def __init__(self, path, model, credential, nonce, case):
        self.model, self.credential, self.nonce = model, credential, nonce
        self.case = case
        self.lock = threading.Lock()
        self.requests, self.connections, self.failures = [], [], []
        self.generations = 0
        super().__init__(str(path), StubHandler)
        os.chmod(path, 0o660)


class StubHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def setup(self):
        super().setup()
        self.connection.settimeout(15)
        self.observation = {"opened_ns": time.monotonic_ns(), "state": "open"}
        with self.server.lock:
            self.server.connections.append(self.observation)

    def finish(self):
        try:
            super().finish()
        finally:
            with self.server.lock:
                self.observation.update(state="handler_settled", settled_ns=time.monotonic_ns())

    def log_message(self, *_args):
        pass

    def reply(self, status, body, content_type="application/json"):
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)
        self.wfile.flush()
        self.close_connection = True

    def do_GET(self):
        try:
            require(self.path == "/v1/models", f"unexpected GET {self.path}")
            with self.server.lock:
                self.server.requests.append({"method": "GET", "path": self.path})
            self.reply(200, json.dumps({"data": [
                {"id": self.server.model, "max_model_len": 262144}
            ]}).encode())
        except Exception as error:
            self.refuse(error)

    def refuse(self, error):
        with self.server.lock:
            self.server.failures.append(repr(error))
        self.reply(500, json.dumps({"error": str(error)}).encode())

    def do_POST(self):
        try:
            size = int(self.headers["Content-Length"])
            require(0 < size < 2 * 1024 * 1024, "unexpected request size")
            raw = self.rfile.read(size)
            require(len(raw) == size, "request body ended early")
            body = json.loads(raw)
            with self.server.lock:
                self.server.requests.append({"method": "POST", "path": self.path, "body": body})
                require(len(self.server.requests) <= 20, "fixture request bound exceeded")
            require(body["model"] == self.server.model, "model identity changed")
            if self.path == "/tokenize":
                if "prompt" in body:
                    require(body["prompt"] == "agent-service-tokenizer-preflight", "unknown raw prompt")
                    response = {"count": 1, "tokens": [42], "max_model_len": 262144}
                else:
                    require(isinstance(body["messages"], list) and body["messages"], "sizing lacks messages")
                    response = {"count": 1024, "max_model_len": 262144}
                self.reply(200, json.dumps(response).encode())
                return
            require(self.path == "/v1/chat/completions", f"unknown POST {self.path}")
            require(self.headers["Authorization"] == "Bearer " + self.server.credential, "credential mismatch")
            require(body["stream"] is True, "generation must stream")
            require(isinstance(body["kv_scope"], str) and body["kv_scope"], "missing generation owner")
            require(any(tool.get("function", {}).get("name") == "read_file" for tool in body["tools"]),
                    "read_file absent from actual tool schema")
            with self.server.lock:
                self.server.generations += 1
                attempt = self.server.generations
            if self.server.case == "cancel_held_sse":
                require(attempt == 1, "cancelled held generation was resampled")
                chunk = {"id": "held_generation", "object": "chat.completion.chunk", "created": 0,
                         "model": self.server.model, "choices": [{"index": 0,
                         "delta": {"content": "CANCEL_HELD"}, "finish_reason": None}]}
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Transfer-Encoding", "chunked")
                self.end_headers()
                data = ("data: " + json.dumps(chunk) + "\n\n").encode()
                self.wfile.write(f"{len(data):x}\r\n".encode() + data + b"\r\n")
                self.wfile.flush()
                with self.server.lock:
                    self.observation["held_chunk_flushed_ns"] = time.monotonic_ns()
                # The ordinary relay must propagate the actual client close.
                # A timer is a failed fixture, never evidence of closure.
                trailing = self.rfile.read(1)
                require(trailing == b"", "unexpected request bytes while SSE was held")
                with self.server.lock:
                    self.observation["peer_eof_ns"] = time.monotonic_ns()
                self.close_connection = True
                return
            if self.server.case in ("provider_malformed_sse", "provider_disconnected_sse"):
                require(attempt == 1, "established failed generation was resampled")
                chunk = {"id": "failed_generation", "object": "chat.completion.chunk", "created": 0,
                         "model": self.server.model, "choices": [{"index": 0,
                         "delta": {"content": "PROVIDER_FAILURE_PREFIX"}, "finish_reason": None}]}
                prefix = ("data: " + json.dumps(chunk) + "\n\n").encode()
                with self.server.lock:
                    self.observation["fault"] = self.server.case
                    self.observation["served_prefix_sha256"] = digest(prefix)
                if self.server.case == "provider_malformed_sse":
                    self.reply(200, prefix + b"data: {invalid-json}\n\ndata: [DONE]\n\n", "text/event-stream")
                else:
                    self.send_response(200)
                    self.send_header("Content-Type", "text/event-stream")
                    self.send_header("Content-Length", str(len(prefix) + 1024))
                    self.send_header("Connection", "close")
                    self.end_headers()
                    self.wfile.write(prefix)
                    self.wfile.flush()
                    # Close before the declared HTTP body completes. No terminal,
                    # usage report, [DONE], or replacement response is supplied.
                    self.close_connection = True
                return
            nonce = self.server.nonce
            if attempt == 1:
                require(nonce not in raw.decode(), "fresh proof leaked into the prompt")
                delta = {"tool_calls": [{"index": 0, "id": "composition_read", "type": "function",
                    "function": {"name": "read_file", "arguments": json.dumps({
                        "file_path": "/workspace/proof.txt", "offset": 0})}}]}
                finish = "tool_calls"
            elif attempt == 2:
                replies = [message for message in body["messages"] if message.get("role") == "tool"
                           and message.get("tool_call_id") == "composition_read"]
                require(len(replies) == 1 and nonce in json.dumps(replies[0]), "actual read did not return proof")
                delta, finish = {"content": "COMPOSITION_OK " + nonce}, "stop"
            else:
                raise GateFailure("unexpected extra generation")
            chunk = {"id": f"composition_{attempt}", "object": "chat.completion.chunk", "created": 0,
                     "model": self.server.model, "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
                     "usage": {"prompt_tokens": 32, "completion_tokens": 8, "total_tokens": 40,
                               "prompt_tokens_details": {"cached_tokens": 0},
                               "completion_tokens_details": {"reasoning_tokens": 0}}}
            self.reply(200, ("data: " + json.dumps(chunk) + "\n\ndata: [DONE]\n\n").encode(), "text/event-stream")
        except Exception as error:
            self.refuse(error)


def prepare_artifacts(source, artifact_path, destination):
    artifacts = Path(artifact_path).resolve()
    manifest = json.loads((artifacts / "manifest.json").read_bytes())
    require(set(manifest) == {"v", "artifacts", "inputs"} and manifest["v"] == 1 and
            set(manifest["artifacts"]) == {"agent_service", "docker_broker", "session_capture"},
            "test executable manifest is incomplete")
    input_paths = {
        "Cargo.toml", "Cargo.lock", ".dockerignore", "build.rs", "docker/Dockerfile",
        "scripts/generate-protocol-bindings.mjs", "config/stack.lock.json",
        "config/broker-policy-v1.json", "config/agent-runtime-contract-v1.json",
        "docker/config/settings.json",
    }
    for directory in ("src", "protocol", "docker/tests"):
        input_paths.update(str(path.relative_to(source)) for path in (source / directory).rglob("*")
                           if path.is_file() and "__pycache__" not in path.parts and path.suffix not in (".pyc", ".pyo"))
    require(set(manifest["inputs"]) == input_paths, "test executable source inventory differs from this build")
    for path, expected in manifest["inputs"].items():
        require(digest((source / path).read_bytes()) == expected, f"test executable input changed: {path}")
    for name, expected in manifest["artifacts"].items():
        raw = (artifacts / name).read_bytes()
        require(len(raw) == expected["bytes"] and digest(raw) == expected["sha256"], "test executable identity changed")
        shutil.copyfile(artifacts / name, destination / name)
        os.chmod(destination / name, 0o555)
    return manifest


class Harness:
    def __init__(self, args, case):
        self.case = case
        require(os.geteuid() == os.getegid() == 1000, "gate requires the owned uid/gid-1000 environment")
        self.source = Path(__file__).resolve().parents[2]
        self.lock = json.loads((self.source / "config/stack.lock.json").read_bytes())
        self.root = Path(tempfile.mkdtemp(prefix="as-gate-"))
        require(self.root.parent == Path("/tmp") and self.root.resolve() == self.root, "unexpected fixture root")
        os.chmod(self.root, 0o700)
        print(f"Composition gate evidence: {self.root}", flush=True)
        self.deadline = time.monotonic() + 1200
        self.containing = False
        self.actors, self.commands = {}, []
        self.session = "s-" + secrets.token_hex(32)
        self.nonce = secrets.token_hex(32)
        self.images = {name: getattr(args, name + "_image") for name in ("agent", "relay", "capture", "broker", "service")}
        for identity in self.images.values():
            require(len(identity) == 71 and identity.startswith("sha256:") and
                    all(c in "0123456789abcdef" for c in identity[7:]), "candidate must be an immutable image ID")
        for directory in ("state", "state/sessions", "state/spool", "results", "control", "model-socket", "input", "evidence"):
            (self.root / directory).mkdir(mode=0o700)
        self.terminal_plans = []
        if self.case.startswith("terminal_"):
            (self.root / "state/composition-faults").mkdir(mode=0o700)
            initial_site = {
                "terminal_prepare_file_sync": "prepare_file_sync",
                "terminal_prepare_directory_sync": "prepare_directory_sync",
                "terminal_publish_link": "publish_link",
                "terminal_publish_directory_sync": "published_directory_sync",
                "terminal_raw_parent_sync": "raw_parent_sync",
                "terminal_temporary_unlink": "temporary_unlink",
                "terminal_temporary_unlink_directory_sync": "temporary_unlink_directory_sync",
            }[self.case]
            self.arm_terminal_fault(initial_site)
        self.artifact_manifest = prepare_artifacts(self.source, args.artifacts, self.root / "input")
        fixture = {"root": str(self.root), "case": case, "session_ids": [self.session],
                   **{name + "_image": self.images[name] for name in ("agent", "relay", "capture")}}
        save_json(self.root / "input/fixture.json", fixture)
        archive = self.root / "input/workspace.zip"
        with zipfile.ZipFile(archive, "x", compression=zipfile.ZIP_STORED) as output:
            output.writestr("proof.txt", self.nonce + "\n")
        self.request = {"prompt": "Read /workspace/proof.txt with read_file and return its exact proof prefixed COMPOSITION_OK.",
                        "max_session_turns": 4, "archive_bytes": archive.stat().st_size,
                        "archive_sha256": digest(archive.read_bytes())}
        save_json(self.root / "input/request.json", self.request)
        save_json(self.root / "input/conflict.json", {**self.request, "prompt": "Conflicting submission must be refused."})
        self.expected = {}
        for prefix, component, image in (("agent-", "agent", "agent"), ("agent-model-", "session-model-relay", "relay"),
                                         ("agent-capture-", "session-capture", "capture")):
            self.expected[prefix + self.session] = {"component": component, "image": self.images[image], "session": self.session}
        self.tag = self.root.name
        for role in ("service", "broker"):
            self.expected[self.tag + "-" + role] = {"image": self.images[role], "gate": self.tag}
        save_json(self.root / "registered-containers.json", self.expected)
        settings = json.loads((self.source / "docker/config/settings.json").read_bytes())
        provider, = settings["modelProviders"]["openai"]
        self.stub = StubServer(self.root / "model-socket/relay.sock", settings["model"]["name"],
                               settings["env"][provider["envKey"]], self.nonce, case)
        self.worker = threading.Thread(target=self.stub.serve_forever, kwargs={"poll_interval": 0.05})
        self.worker.start()

    def command(self, argv, timeout=45, check=True):
        remaining = timeout if self.containing else self.deadline - time.monotonic()
        require(remaining > 0, "composition watchdog expired")
        started = time.monotonic_ns()
        observation = {"argv": argv, "state": "dispatched", "started_ns": started}
        self.commands.append(observation)
        try:
            result = subprocess.run(argv, capture_output=True, timeout=min(timeout, remaining))
        except subprocess.TimeoutExpired as error:
            observation.update(state="timed_out", timeout=error.timeout, settled_ns=time.monotonic_ns(),
                               direct_child="killed_and_joined_by_subprocess_run", daemon_operation="outcome_unknown")
            for name, raw in (("stdout", error.stdout), ("stderr", error.stderr)):
                if raw is not None:
                    save(self.root / "evidence" / f"command-{len(self.commands)}.{name}", raw)
            raise
        except BaseException as error:
            observation.update(state="failed", failure=repr(error), observed_ns=time.monotonic_ns())
            raise
        observation.update(state="returned", status=result.returncode, settled_ns=time.monotonic_ns())
        if check and result.returncode:
            raise GateFailure(f"command {argv!r} exited {result.returncode}: {result.stderr.decode(errors='replace')}")
        return result

    def inspect_owned(self, name):
        require(name in self.expected, "unregistered container operation")
        result = self.command(["docker", "container", "inspect", name], check=False)
        if result.returncode:
            diagnostic = result.stderr.lower()
            require(b"no such container" in diagnostic or b"no such object" in diagnostic,
                    f"container absence undetermined: {result.stderr!r}")
            previous = self.actors.get(name)
            if previous is not None:
                previous["reconciled_absent_at_ns"] = time.monotonic_ns()
            return None
        value, = json.loads(result.stdout)
        expected = self.expected[name]
        descriptor = value.get("ImageManifestDescriptor", {})
        config_digest = value["Image"]
        require(descriptor.get("mediaType") == "application/vnd.oci.image.manifest.v1+json" and
                descriptor.get("digest") == expected["image"] and
                value["Config"]["Image"] == expected["image"] and value["Name"] == "/" + name,
                "container manifest identity mismatch")
        require(len(config_digest) == 71 and config_digest.startswith("sha256:") and
                all(c in "0123456789abcdef" for c in config_digest[7:]), "invalid observed container config digest")
        labels = value["Config"]["Labels"]
        if "gate" in expected:
            require(labels.get("agent_service.composition_gate") == self.tag, "test bootstrap ownership mismatch")
        else:
            require(labels.get("agent_service.session") == self.session and
                    labels.get("agent_service.component") == expected["component"] and
                    labels.get("agent_service.profile") == self.lock["profile"], "session container ownership mismatch")
            sources = [mount["Source"] for mount in value["Mounts"] if mount["Type"] == "bind"]
            require(sources and all(Path(source).is_relative_to(self.root) for source in sources), "fixture mounts escaped ownership")
        previous = self.actors.get(name)
        require(previous is None or previous["id"] == value["Id"], "registered name was reused")
        self.actors[name] = {"id": value["Id"], "manifest_digest": descriptor["digest"],
                             "config_digest": config_digest, "state": value["State"],
                             "observed_at_ns": time.monotonic_ns()}
        if previous is None:
            with (self.root / "actor-admissions.jsonl").open("ab") as ledger:
                os.fchmod(ledger.fileno(), 0o600)
                ledger.write((json.dumps({"name": name, **self.actors[name]}) + "\n").encode())
                ledger.flush()
                os.fsync(ledger.fileno())
        return value

    def start_driver(self, role, expected_startup_failure=None):
        name = self.tag + "-" + role
        require(self.inspect_owned(name) is None, "driver name already exists")
        if role == "service":
            absent = []
            for actor, expected in self.expected.items():
                if "session" in expected:
                    require(self.inspect_owned(actor) is None,
                            "local startup recovery requires every registered session actor absent")
                    absent.append(actor)
            save_json(self.root / "evidence" / f"startup-absence-{len(self.commands)}.json",
                      {"names": absent, "observed_ns": time.monotonic_ns()})
        limits = self.lock[role]
        argv = ["docker", "run", "-d", "--name", name,
                "--label", "agent_service.composition_gate=" + self.tag,
                "--network", "none", "--restart", "no", "--read-only", "--cap-drop", "ALL",
                "--security-opt", "no-new-privileges:true", "--user", "1000:984" if role == "broker" else "1000:1000",
                "--memory", limits["memory"], "--memory-swap", limits["memory_swap"], "--pids-limit", str(limits["pids_limit"]),
                "--mount", f"type=bind,src={self.root},dst={self.root},readonly",
                "--mount", f"type=bind,src={self.root / 'input'},dst=/gate,readonly"]
        writable = ["control"] if role == "broker" else ["state", "results"]
        for directory in writable:
            argv += ["--mount", f"type=bind,src={self.root / directory},dst={self.root / directory}"]
        if role == "broker":
            argv += ["--mount", "type=bind,src=/var/run/docker.sock,dst=/var/run/docker.sock,readonly"]
        else:
            argv += ["--group-add", "984", "--tmpfs", "/tmp:" + limits["tmpfs_tmp"]]
        binary = "docker_broker" if role == "broker" else "agent_service"
        argv += ["--entrypoint", "/gate/" + binary, self.images[role],
                 "--exact", "composition_gate::final_image_" + role, "--ignored", "--nocapture"]
        created_id = self.command(argv).stdout.decode().strip()
        require(len(created_id) == 64 and all(c in "0123456789abcdef" for c in created_id),
                "Docker run did not return exactly one container identity")
        save_json(self.root / "evidence" / f"created-{role}-{len(self.commands)}.json",
                  {"name": name, "id": created_id, "manifest_digest": self.images[role]})
        self.actors[name] = {"id": created_id}
        self.inspect_owned(name)
        if expected_startup_failure is not None:
            require(role == "service", "only the local recovery bootstrap has an expected refusal")
            result = self.command(["docker", "wait", created_id], timeout=60)
            logs = self.command(["docker", "logs", created_id])
            save(self.root / "evidence" / f"refused-startup-{len(self.commands)}.stdout", logs.stdout)
            save(self.root / "evidence" / f"refused-startup-{len(self.commands)}.stderr", logs.stderr)
            refusals = [json.loads(line.removeprefix(b"COMPOSITION_SERVICE_STARTUP_REFUSED "))
                        for line in logs.stdout.splitlines() if line.startswith(b"COMPOSITION_SERVICE_STARTUP_REFUSED ")]
            require(result.stdout.strip() == b"0" and b"COMPOSITION_SERVICE_READY" not in logs.stdout and
                    len(refusals) == 1 and expected_startup_failure["cause"] in refusals[0]["cause"],
                    "startup did not refuse at the intended recovery barrier before listener admission")
            self.audit_terminal_faults()
            require(any(receipt["plan_id"] == expected_startup_failure["id"] and receipt["injected_errno"] == 5
                        for receipt in self.terminal_receipts()), "startup refusal did not execute its armed fault")
            if self.case == "terminal_raw_parent_sync":
                require(any(receipt["plan_id"] == expected_startup_failure["id"] and receipt["injected_errno"] == 5 and
                            receipt["context"] == "terminalization: sync absent raw session tree"
                            for receipt in self.terminal_receipts()), "recovery did not refuse the already-absent raw branch")
            for actor, expected in self.expected.items():
                if "session" in expected:
                    require(self.inspect_owned(actor) is None, "refused recovery created a session actor")
            self.command(["docker", "container", "rm", created_id])
            del self.actors[name]
            return None
        marker = ("COMPOSITION_" + role.upper() + "_READY").encode()
        self.wait_for(lambda: marker in self.command(["docker", "logs", name]).stdout,
                      f"{role} bootstrap readiness", timeout=60)
        return name

    def wait_for(self, predicate, description, timeout=180):
        end = min(self.deadline, time.monotonic() + timeout)
        while time.monotonic() < end:
            result = predicate()
            if result:
                return result
            time.sleep(0.1)
        raise GateFailure(f"deadline awaiting {description}")

    def http(self, service, method, path, request=None):
        argv = ["docker", "exec", service, "curl", "--silent", "--show-error", "--max-time", "30",
                "--write-out", "\n%{http_code}", "--request", method]
        if request:
            argv += ["--header", "Idempotency-Key: " + self.session,
                     "--form", "request=@/gate/" + request + ";type=application/json",
                     "--form", "archive=@/gate/workspace.zip;type=application/zip"]
        argv += ["http://" + self.lock["service"]["listen"] + path]
        response = self.command(argv).stdout
        body, status = response.rsplit(b"\n", 1)
        return int(status), body

    def stop_driver(self, role):
        name = self.tag + "-" + role
        observed = self.inspect_owned(name)
        require(observed is not None, "driver disappeared before settlement")
        if observed["State"]["Running"]:
            self.command(["docker", "kill", "--signal=TERM", observed["Id"]])
        result = self.command(["docker", "wait", observed["Id"]], timeout=60)
        logs = self.command(["docker", "logs", observed["Id"]])
        save(self.root / "evidence" / f"{role}-{len(self.commands)}.stdout", logs.stdout)
        save(self.root / "evidence" / f"{role}-{len(self.commands)}.stderr", logs.stderr)
        require(result.stdout.strip() == b"0", f"{role} failed: {logs.stdout!r} {logs.stderr!r}")
        require(("COMPOSITION_" + role.upper() + "_SETTLED").encode() in logs.stdout, "driver exited without joining its work")
        self.command(["docker", "container", "rm", observed["Id"]])
        # A later fresh service process intentionally gets a new identity.
        del self.actors[name]

    def wait_terminal(self, service, route):
        def terminal():
            status, raw = self.http(service, "GET", route)
            require(status == 200, "accepted session is unreadable")
            for name, registration in self.expected.items():
                if "session" in registration:
                    self.inspect_owned(name)
            body = json.loads(raw)
            return body if body["terminal"] is not None else None
        body = self.wait_for(terminal, "durable terminal publication")
        save_json(self.root / "evidence/terminal.json", body)
        return body

    def cancel_start_gate(self, service, route):
        self.wait_for(lambda: (self.root / "control/created-held.json").exists(),
                      "real topology creation held before response")
        agent = "agent-" + self.session
        observation_script = """import json
from pathlib import Path
found=[]
for path in Path('/proc').glob('[0-9]*/cmdline'):
 try: words=path.read_bytes().split(b'\\0')
 except (FileNotFoundError, ProcessLookupError): continue
 if words[0] in (b'flock', b'/usr/bin/flock') and b'/run/agent/start-gate.lock' in words:
  found.append({'pid':int(path.parent.name),'argv':[w.decode() for w in words if w]})
print(json.dumps(found))
"""
        def waiting_child():
            self.inspect_owned(agent)
            result = self.command(["docker", "exec", agent, "python3", "-c", observation_script])
            return json.loads(result.stdout)
        child = self.wait_for(waiting_child, "actual wrapper flock child")
        require(len(child) == 1, "expected exactly one blocked prelaunch flock")
        save_json(self.root / "evidence/blocked-wrapper-child.json", child)
        gate = self.root / "state/sessions" / self.session / "control/start-gate.lock"
        with gate.open("rb") as held:
            try:
                fcntl.flock(held, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                pass
            else:
                raise GateFailure("service start gate was not held at cancellation barrier")
            require(not self.stub.requests, "provider was contacted before start-gate release")
            status, raw = self.http(service, "POST", route + "/cancel")
            require(status == 202, f"durable cancellation failed: {raw!r}")
            save_json(self.root / "evidence/cancel-receipt.json", json.loads(raw))
            intent = self.root / "results" / self.session / "cancel-requested.json"
            require(intent.is_file(), "HTTP cancellation lacked its durable intent")
            save(self.root / "evidence/cancel-intent.json", intent.read_bytes())
            body = self.wait_terminal(service, route)
            save(self.root / "control/release-create-response", b"cancelled terminal observed while create response remained held\n")
        end = body["terminal"]
        require(body["status"] == "cancelled" and end["agent_result"] is None and
                body["num_turns"] == 0 and self.stub.generations == 0 and not self.stub.requests,
                "cancelled locked topology advanced model work or invented certification")
        require("cancellation" in end["response"].lower(), "cancellation cause was not retained")
        require(end["is_process_error"] is False and end["teardown_diagnostics"] == [
            "durable cancellation became observable while the broker's create transaction was in flight; ownership-checked removal remained serialized behind that transaction while the agent start gate stayed locked"
        ], "locked-gate cancellation contains unrelated lifecycle failures")
        require(end["raw_session_tree_retained"] is False, "settled setup cancellation retained undisposed raw state")
        persisted = self.root / "results" / self.session / "finished.json"
        require(json.loads(persisted.read_bytes()) == body and
                not persisted.with_name("finished.json.tmp").exists(), "cancellation publication is incomplete")
        for name, registration in self.expected.items():
            if "session" in registration:
                require(self.inspect_owned(name) is None, "container outlived cancelled publication")
        self.stop_driver("service")
        self.stop_driver("broker")
        return {"v": 1, "claim": "final-image execution and service publication", "case": self.case,
                "status": "passed", "images": self.images, "test_executables": self.artifact_manifest,
                "session_id": self.session, "terminal_sha256": digest(persisted.read_bytes()),
                "test_fault": "withhold real successful create response until durable HTTP cancellation, then close it",
                "excluded": ["deployed-stack bootstrap", "real tokenizer correctness", "real model semantics"]}

    def cancel_held_sse(self, service, route):
        def held_connection():
            with self.stub.lock:
                return [dict(c) for c in self.stub.connections if "held_chunk_flushed_ns" in c]
        observed = self.wait_for(held_connection, "actual opened SSE response and flushed partial chunk")
        require(len(observed) == 1 and "peer_eof_ns" not in observed[0], "SSE was not held before cancellation")
        def running_phase():
            status, raw = self.http(service, "GET", route)
            require(status == 200, "running session is unreadable")
            state = json.loads(raw)
            require(state["terminal"] is None, "generation ended before its cancellation barrier")
            return state["progress_phase"] == "running_agent"
        self.wait_for(running_phase, "service consumed readiness and entered the running phase")
        current = held_connection()
        require(len(current) == 1 and current[0]["opened_ns"] == observed[0]["opened_ns"] and
                "peer_eof_ns" not in current[0], "held connection changed or closed before cancellation")
        cancellation_dispatch_ns = time.monotonic_ns()
        status, raw = self.http(service, "POST", route + "/cancel")
        require(status == 202, f"durable cancellation failed: {raw!r}")
        save_json(self.root / "evidence/cancel-receipt.json", json.loads(raw))
        intent = self.root / "results" / self.session / "cancel-requested.json"
        require(intent.is_file(), "cancellation response preceded its durable intent")
        save(self.root / "evidence/cancel-intent.json", intent.read_bytes())
        self.wait_for(lambda: (self.root / "control/remove-held.json").exists(), "real teardown held before removal dispatch")
        def closed_connection():
            connections = held_connection()
            require(len(connections) == 1, "held connection identity became ambiguous")
            return connections if "peer_eof_ns" in connections[0] else None
        closed = self.wait_for(closed_connection, "actual provider EOF before allowing relay teardown", timeout=15)
        require(closed[0]["peer_eof_ns"] >= cancellation_dispatch_ns, "connection closed before cancellation dispatch")
        relay = self.inspect_owned("agent-model-" + self.session)
        require(relay is not None and relay["State"]["Running"], "relay stopped before provider-close observation")
        save_json(self.root / "evidence/provider-closed-before-removal.json",
                  {"cancellation_dispatch_ns": cancellation_dispatch_ns, "connection": closed[0], "relay": relay})
        save(self.root / "control/release-remove", b"provider EOF observed while relay remains running\n")
        body = self.wait_terminal(service, route)
        end = body["terminal"]
        require(body["status"] == "cancelled" and end["is_process_error"] is True and
                end["container_exit_code"] == end["agent_exit_code"] == 130,
                "held generation did not settle with the actual TERM outcome")
        require(self.stub.generations == 1 and not self.stub.failures, "cancelled generation was retried or fixture failed")
        closed = held_connection()
        require(len(closed) == 1 and "peer_eof_ns" in closed[0] and
                closed[0]["peer_eof_ns"] >= closed[0]["held_chunk_flushed_ns"], "actual provider connection closure is unproved")
        require(end["teardown_diagnostics"] == [] and end["agent_result"] is not None and
                end["agent_result"]["agent_result_subtype"] == "error_cancelled" and
                end["raw_session_tree_retained"] is False and
                not (self.root / "state/sessions" / self.session).exists(),
                "cancelled result lacked certification or mandatory evidence/teardown failed")
        persisted = self.root / "results" / self.session / "finished.json"
        require(json.loads(persisted.read_bytes()) == body and
                not persisted.with_name("finished.json.tmp").exists(), "cancelled terminal publication is incomplete")
        status, bundle = self.http(service, "GET", route + "/bundle")
        require(status == 200 and len(bundle) == end["bundle"]["compressed_bytes"] and
                digest(bundle) == end["bundle"]["sha256"], "cancelled bundle differs from its commitment")
        archive = self.root / "evidence/bundle.tar.zst"
        save(archive, bundle)
        contents = {}
        for name in ("output/events.jsonl", "output/qwen.stderr", "control/container-logs.txt"):
            contents[name] = self.command(["tar", "--zstd", "-xOf", str(archive), name]).stdout
        proof = re.findall(rb"CAPTURE_COMPLETE capture=unix-stream-capture-v1 events_bytes=(\d+) stderr_bytes=(\d+)",
                           contents["control/container-logs.txt"])
        require(len(proof) == 1 and tuple(map(int, proof[0])) ==
                (len(contents["output/events.jsonl"]), len(contents["output/qwen.stderr"])),
                "actual capture completion does not prove the cancelled output bytes")
        for name, registration in self.expected.items():
            if "session" in registration:
                require(self.inspect_owned(name) is None, "cancelled actor outlived publication")
        self.stop_driver("service")
        self.stop_driver("broker")
        return {"v": 1, "claim": "final-image execution and service publication", "case": self.case,
                "status": "passed", "images": self.images, "test_executables": self.artifact_manifest,
                "session_id": self.session, "terminal_sha256": digest(persisted.read_bytes()),
                "bundle": end["bundle"], "events_sha256": digest(contents["output/events.jsonl"]),
                "test_fault": "hold a real incomplete SSE response until the ordinary client closes it",
                "excluded": ["deployed-stack bootstrap", "real tokenizer correctness", "real model semantics"]}

    def run(self):
        self.start_driver("broker")
        service = self.start_driver("service")
        route = "/v1/agent/sessions/" + self.session
        status, raw = self.http(service, "POST", "/v1/agent/sessions", "request.json")
        receipt = json.loads(raw)
        save_json(self.root / "evidence/receipt.json", receipt)
        require(status == 202 and receipt["session_id"] == self.session, "actual HTTP acceptance failed")
        if self.case == "cancel_start_gate":
            return self.cancel_start_gate(service, route)
        if self.case == "cancel_held_sse":
            return self.cancel_held_sse(service, route)
        if self.case in ("provider_malformed_sse", "provider_disconnected_sse"):
            return self.provider_failure(service, route)
        if self.case in ("capture_proof_held_cancel", "capture_proof_lost", "capture_byte_mismatch",
                         "capture_mode_mismatch", "wait_observation_lost"):
            return self.observation_fault(service, route)
        if self.case.startswith("terminal_"):
            return self.terminal_recovery(service, route)
        if self.case in ("remove_stopped_capture_failed", "quiescence_observation_lost"):
            return self.removal_fault(service, route)
        body = self.wait_terminal(service, route)
        end = body["terminal"]
        require(body["status"] == "completed" and end["container_exit_code"] == end["agent_exit_code"] == 0,
                f"execution failed: {end}")
        require(end["is_process_error"] is False and end["teardown_diagnostics"] == [], "process/teardown failed")
        require(end["response"] == "COMPOSITION_OK " + self.nonce and end["agent_result"] is not None,
                "actual output lacks production certification or fresh proof")
        require(body["observed_unaccounted_records"] == 0 and body["observed_output_tokens"] == 16 and
                body["observed_reasoning_tokens"] == 0 and body["num_turns"] == 2, "served observation mismatch")
        require(end["raw_session_tree_retained"] is False and not (self.root / "state/sessions" / self.session).exists(),
                "successful terminal did not dispose raw state through its transaction")
        persisted = self.root / "results" / self.session / "finished.json"
        require(json.loads(persisted.read_bytes()) == body, "HTTP terminal differs from published bytes")
        require(not persisted.with_name("finished.json.tmp").exists(), "unpublished draft remains")
        terminal_hash = digest(persisted.read_bytes())
        for name in self.expected:
            if "session" in self.expected[name]:
                require(self.inspect_owned(name) is None, "session container outlived publication")

        status, bundle = self.http(service, "GET", route + "/bundle")
        require(status == 200 and len(bundle) == end["bundle"]["compressed_bytes"] and
                digest(bundle) == end["bundle"]["sha256"], "downloaded bundle commitment differs")
        archive = self.root / "evidence/bundle.tar.zst"
        save(archive, bundle)
        names = self.command(["tar", "--zstd", "-tf", str(archive)]).stdout.decode().splitlines()
        require(all(not name.startswith("/") and ".." not in Path(name).parts for name in names), "bundle paths escaped fixture")
        for name in ("staged/proof.txt", "control/prompt.txt", "control/turn-budget.json", "output/events.jsonl",
                     "output/qwen.stderr", "output/ready.json", "output/qwen-exit-code"):
            require(name in names, f"required bundle evidence missing: {name}")
        events = self.command(["tar", "--zstd", "-xOf", str(archive), "output/events.jsonl"]).stdout
        require(events and events.endswith(b"\n"), "captured stream is empty or torn")
        self.certified_events = events
        records = [json.loads(line) for line in events.splitlines()]
        schema_hash = digest((self.source / "protocol/stream-contract-v1.json").read_bytes())
        require(records[0]["stream_contract_sha256"] == schema_hash, "producer/service source contract pairing changed")
        session_id = records[0]["session_id"]
        require(all(record["session_id"] == session_id for record in records), "captured event ownership changed")
        generations = [request["body"] for request in self.stub.requests if request["path"] == "/v1/chat/completions"]
        require(len(generations) == 2 and all(request["kv_scope"] == session_id for request in generations),
                "provider generation ownership differs from the captured session")
        blocks = [block for record in records if record["type"] in ("assistant", "user")
                  for block in record["message"]["content"]]
        uses = [block for block in blocks if block["type"] == "tool_use"]
        replies = [block for block in blocks if block["type"] == "tool_result"]
        require(len(uses) == len(replies) == 1 and uses[0]["id"] == "composition_read" and
                uses[0]["name"] == "read_file" and replies[0]["tool_use_id"] == "composition_read" and
                replies[0]["is_error"] is False and replies[0]["content"] == self.nonce + "\n",
                "captured tool execution did not return the complete successful proof")
        require(any(record.get("event") == {"type": "goal_state", "goal_state": {"v": 2, "goal": None, "activity": "idle"}}
                    for record in records), "initial idle goal state was not retained")
        require(records[-1]["usage"] == {"requests": 2, "usageReports": 2, "unfinalizedRequests": 0,
                "unreportedUsageRequests": 0, "usage": {"promptTokenCount": 64, "candidatesTokenCount": 16,
                "thoughtsTokenCount": 0, "cachedContentTokenCount": 0, "totalTokenCount": 80}}, "terminal usage differs")

        status, replay = self.http(service, "POST", "/v1/agent/sessions", "request.json")
        require(status == 200 and json.loads(replay) == body, "idempotent replay changed the accepted operation")
        status, _ = self.http(service, "POST", "/v1/agent/sessions", "conflict.json")
        require(status == 409, "conflicting submission was not refused")
        self.stop_driver("service")
        service = self.start_driver("service")
        status, reopened = self.http(service, "GET", route)
        require(status == 200 and json.loads(reopened) == body and digest(persisted.read_bytes()) == terminal_hash,
                "fresh Manager could not read the same durable terminal")
        status, _ = self.http(service, "DELETE", route)
        require(status in (200, 204), "actual terminal deletion failed")
        status, _ = self.http(service, "GET", route)
        require(status == 404, "deleted resource remains visible")
        self.stop_driver("service")
        self.stop_driver("broker")
        require(self.stub.generations == 2 and not self.stub.failures, f"provider fixture failed: {self.stub.failures}")
        require(self.stub.requests[0] == {"method": "GET", "path": "/v1/models"}, "real wrapper preflight did not run first")
        return {"v": 1, "claim": "final-image execution and service publication", "case": self.case, "status": "passed",
                "images": self.images, "test_executables": self.artifact_manifest,
                "schema_sha256": schema_hash, "archive_sha256": self.request["archive_sha256"],
                "session_id": self.session, "terminal_sha256": terminal_hash,
                "bundle": end["bundle"], "events_sha256": digest(events),
                "excluded": ["deployed-stack bootstrap", "real tokenizer correctness", "real model semantics"]}

    def broker_reconciliation(self, operation):
        require(operation in ("remove_session", "prove_session_quiescent"), "unregistered reconciliation operation")
        path = self.root / "control/broker.sock"
        metadata = path.lstat()
        require(path.is_socket() and metadata.st_uid == 1000 and metadata.st_gid == 984 and
                metadata.st_mode & 0o7777 == 0o660, "owned broker socket identity changed")
        request = {"op": operation, "session_id": self.session}
        save_json(self.root / "evidence" / (operation + "-reconciliation-dispatch.json"), request)
        response = bytearray()
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
            connection.settimeout(30)
            connection.connect(str(path))
            connection.sendall((json.dumps(request) + "\n").encode())
            connection.shutdown(socket.SHUT_WR)
            while chunk := connection.recv(65536):
                response.extend(chunk)
                require(len(response) <= 4 * 1024 * 1024, "reconciliation response exceeded production bound")
        raw = bytes(response)
        save(self.root / "evidence" / (operation + "-reconciliation-response.jsonl"), raw)
        require(raw.endswith(b"\n") and b"\r" not in raw and b"\n" not in raw[:-1],
                "reconciliation response violates actual broker framing")
        envelope = json.loads(raw)
        require(set(envelope) == {"ok", "data", "error"} and envelope["ok"] is True and envelope["error"] is None,
                "actual broker reconciliation failed")
        return envelope["data"]

    def removal_fault(self, service, route):
        refused = self.root / "control/remove-refused.json"
        self.wait_for(refused.exists, "actual stopped capture removal refusal")
        receipt = json.loads(refused.read_bytes())
        capture_name = "agent-capture-" + self.session
        require(receipt["site"] == "before_remove_stopped_capture" and receipt["session_id"] == self.session and
                receipt["name"] == capture_name and receipt["injected_errno"] == 5 and
                receipt["observed"]["State"]["Running"] is False, "removal fault lacks actual stopped ownership evidence")
        if self.case == "quiescence_observation_lost":
            held = self.root / "control/quiescence-observation-held.json"
            self.wait_for(held.exists, "real independent quiescence response held after handler join")
            observation = json.loads(held.read_bytes())
            require(observation == {"ok": True, "error": None, "data": {
                "agent_present": False, "agent_running": False, "relay_present": False, "relay_running": False,
                "capture_present": True, "capture_running": False, "quiescent": True,
            }}, "lost proof did not establish the intended stopped physical state")
            status, raw = self.http(service, "GET", route)
            require(status == 200 and json.loads(raw)["terminal"] is None, "quiescence was inferred before its actual proof")
            require(not (self.root / "results" / self.session / "bundle.tar.zst").exists(),
                    "bundle was created while independent proof remained unavailable")
            save(self.root / "control/release-observation", b"true quiescence observed only by the fault harness\n")
        body = self.wait_terminal(service, route)
        end = body["terminal"]
        require(body["status"] == "completed" and end["container_exit_code"] == end["agent_exit_code"] == 0 and
                end["is_process_error"] is True and end["agent_result"]["agent_result_subtype"] == "success" and
                end["raw_session_tree_retained"] is True, "teardown failure lost independent result or retained-state obligation")
        require(end["response"] == "COMPOSITION_OK " + self.nonce,
                "teardown failure lost the independently certified fresh tool proof")
        require(body["observed_output_tokens"] == 16 and body["observed_reasoning_tokens"] == 0 and
                body["observed_unaccounted_records"] == 0 and body["num_turns"] == 2,
                "teardown failure lost served accounting")
        expected_cause = f"composition injected EIO before removing proved-stopped capture {capture_name}: Input/output error (os error 5)"
        require(any(expected_cause in detail for detail in end["teardown_diagnostics"]),
                "actual failed removal cause was not retained")
        capture = self.inspect_owned(capture_name)
        require(capture is not None and capture["State"]["Running"] is False and
                capture["Id"] == receipt["observed"]["Id"], "retained container differs from proved-stopped capture")
        for actor in ("agent-" + self.session, "agent-model-" + self.session):
            require(self.inspect_owned(actor) is None, "failed capture removal prevented another component's removal")
        state = self.root / "state/sessions" / self.session
        marker = (state / "control/raw-evidence-retained.txt").read_text()
        expected_retention = "container-quiescence-unproved" if self.case == "quiescence_observation_lost" else "container-teardown-incomplete"
        require(marker.startswith("RAW_SESSION_TREE_RETAINED\ncause=" + expected_retention + "\n"),
                "retained raw state has the wrong authority cause")
        result_dir = self.root / "results" / self.session
        persisted = result_dir / "finished.json"
        require(json.loads(persisted.read_bytes()) == body and not persisted.with_name("finished.json.tmp").exists(),
                "teardown fault did not publish its exact retained resource")
        if self.case == "quiescence_observation_lost":
            require(end["bundle"] is None and not (result_dir / "bundle.tar.zst").exists(),
                    "service bundled without independently delivered quiescence authority")
            require(any("independent post-teardown quiescence proof failed" in detail and
                        "broker response is not one terminal-LF JSON record without CR bytes" in detail
                        for detail in end["teardown_diagnostics"]), "missing quiescence proof lost its actual transport cause")
            events = (state / "output/events.jsonl").read_bytes()
            save(self.root / "evidence/retained-events.jsonl", events)
        else:
            status, bundle = self.http(service, "GET", route + "/bundle")
            require(status == 200 and digest(bundle) == end["bundle"]["sha256"] and
                    len(bundle) == end["bundle"]["compressed_bytes"], "proved quiescence did not permit the exact forensic bundle")
            archive = self.root / "evidence/bundle.tar.zst"
            save(archive, bundle)
            events = self.command(["tar", "--zstd", "-xOf", str(archive), "output/events.jsonl"]).stdout
            require(events == (state / "output/events.jsonl").read_bytes(), "bundle differs from retained raw output")
        require(self.stub.generations == 2 and not self.stub.failures, "teardown fault reissued completed model work")
        save(self.root / "control/release-remove-fault", b"terminal failure and retained evidence verified\n")
        require(self.broker_reconciliation("remove_session") == {}, "owned cleanup returned unexpected data")
        require(self.broker_reconciliation("prove_session_quiescent") == {
            "agent_present": False, "agent_running": False, "relay_present": False, "relay_running": False,
            "capture_present": False, "capture_running": False, "quiescent": True,
        }, "owned cleanup did not prove all registered actors absent")
        for actor, expected in self.expected.items():
            if "session" in expected:
                require(self.inspect_owned(actor) is None, "explicit teardown reconciliation left an actor")
        self.stop_driver("service")
        self.stop_driver("broker")
        return {"v": 1, "claim": "final-image execution and service publication", "case": self.case,
                "status": "passed", "images": self.images, "test_executables": self.artifact_manifest,
                "session_id": self.session, "terminal_sha256": digest(persisted.read_bytes()),
                "events_sha256": digest(events), "bundle": end["bundle"], "test_fault": self.case,
                "excluded": ["deployed-stack bootstrap", "real tokenizer correctness", "real model semantics"]}

    def arm_terminal_fault(self, site):
        plan = {"v": 1, "id": str(uuid.uuid4()), "points": [] if site is None else [{"site": site, "phase": "before"}]}
        control = self.root / "state/composition-faults"
        temporary = control / (plan["id"] + ".next")
        save_json(temporary, plan)
        os.replace(temporary, control / "terminal-io-plan.json")
        directory = os.open(control, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
        save_json(self.root / "evidence" / ("terminal-plan-" + plan["id"] + ".json"), plan)
        self.terminal_plans.append(plan)
        return plan

    def terminal_receipts(self):
        paths = []
        for path in (self.root / "state/composition-faults").iterdir():
            if path.name == "terminal-io-plan.json":
                continue
            require(re.fullmatch(r"terminal-io-[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}-[0-9]{6}\.json", path.name),
                    "terminal fixture contains an unknown receipt entry")
            paths.append(path)
        paths.sort()
        require(len(paths) <= 4096, "terminal fixture exceeded its receipt bound")
        receipts = [json.loads(path.read_bytes()) for path in paths]
        require(all(receipt["v"] == 1 and receipt["plan_id"] in {plan["id"] for plan in self.terminal_plans}
                    for receipt in receipts), "terminal receipt has unknown plan identity")
        return receipts

    def audit_terminal_faults(self):
        receipts = self.terminal_receipts()
        for plan in self.terminal_plans:
            injected = [receipt for receipt in receipts if receipt["plan_id"] == plan["id"] and
                        receipt["injected_errno"] is not None]
            expected = [(point["site"], point["phase"], 5) for point in plan["points"]]
            actual = [(receipt["site"], receipt["phase"], receipt["injected_errno"]) for receipt in injected]
            require(actual == expected, "armed terminal I/O fault was missed, duplicated, or changed")
        boots = {}
        for receipt in receipts:
            boots.setdefault(receipt["boot"], []).append(receipt)
        for boot, events in boots.items():
            events.sort(key=lambda event: event["sequence"])
            require([event["sequence"] for event in events] == list(range(len(events))),
                    "terminal syscall evidence has a sequence gap or duplicate")
            require(all(event["operation_returned_successfully"] == (event["phase"] == "after") for event in events),
                    "terminal syscall receipt confused effect and attempted operation")
            def prior_success(index, sites):
                return any(event["site"] in sites and event["phase"] == "after" and event["injected_errno"] is None
                           for event in events[:index])
            for index, event in enumerate(events):
                if event["phase"] != "before":
                    continue
                site = event["site"]
                if site == "prepare_file_sync":
                    require(prior_success(index, {"prepare_write"}), "prepare sync preceded the complete draft write")
                elif site in ("prepare_directory_sync", "resume_directory_sync"):
                    required = "prepare_file_sync" if site == "prepare_directory_sync" else "resume_file_sync"
                    require(prior_success(index, {required}), "draft directory sync preceded its file sync")
                elif site in ("raw_remove", "publish_link"):
                    require(prior_success(index, {"prepare_directory_sync", "resume_directory_sync"}),
                            "raw disposal or publication preceded durable draft admission in this boot")
                    if site == "publish_link":
                        require(prior_success(index, {"raw_parent_sync"}), "publication preceded raw-parent durability")
                elif site == "published_directory_sync":
                    require(prior_success(index, {"publish_link"}), "publication directory sync preceded the actual hard link")
                elif site == "temporary_unlink":
                    require(prior_success(index, {"published_directory_sync"}), "temporary unlink preceded durable final publication")
                elif site == "reconcile_directory_sync":
                    require(prior_success(index, {"reconcile_file_sync"}), "reconciliation directory sync preceded validated file sync")
                elif site == "reconcile_unlink":
                    require(prior_success(index, {"reconcile_directory_sync"}), "reconciliation unlinked before establishing final durability")
                elif site in ("temporary_unlink_directory_sync", "reconcile_unlink_directory_sync"):
                    required = "temporary_unlink" if site == "temporary_unlink_directory_sync" else "reconcile_unlink"
                    require(prior_success(index, {required}), "unlink directory sync preceded its actual unlink")
        return boots

    def terminal_recovery(self, service, route):
        body = self.wait_terminal(service, route)
        end = body["terminal"]
        require(body["status"] == "completed" and end["container_exit_code"] == end["agent_exit_code"] == 0 and
                end["is_process_error"] is True and end["agent_result"]["agent_result_subtype"] == "success" and
                any("terminal persistence failed; body retained only in service memory" in detail
                    for detail in end["teardown_diagnostics"]), "live owner did not retain the actual publication failure")
        require(self.stub.generations == 2 and not self.stub.failures, "publication fault changed model execution")
        self.audit_terminal_faults()
        result_dir = self.root / "results" / self.session
        temporary = result_dir / "finished.json.tmp"
        finished = result_dir / "finished.json"
        raw_tree = self.root / "state/sessions" / self.session
        draft_only = self.case in ("terminal_prepare_file_sync", "terminal_prepare_directory_sync",
                                   "terminal_publish_link", "terminal_raw_parent_sync")
        require(finished.exists() is (not draft_only) and
                temporary.exists() is (self.case != "terminal_temporary_unlink_directory_sync"),
                "actual publication fault left an unexpected namespace")
        require(raw_tree.exists() is (self.case in ("terminal_prepare_file_sync", "terminal_prepare_directory_sync")),
                "raw-state disposition does not match the reached transaction barrier")
        if finished.exists() and temporary.exists():
            final_stat, temp_stat = finished.stat(), temporary.stat()
            require((final_stat.st_dev, final_stat.st_ino) == (temp_stat.st_dev, temp_stat.st_ino),
                    "actual publication did not create one hard-linked inode")
        candidate_bytes = (temporary if temporary.exists() else finished).read_bytes()
        candidate = json.loads(candidate_bytes)
        require(candidate["terminal"]["bundle"] == end["bundle"] and
                candidate["terminal"]["agent_result"] == end["agent_result"],
                "private accepted candidate lost its result or bundle identity")
        save(self.root / "evidence/accepted-terminal-candidate.json", candidate_bytes)
        bundle_path = result_dir / "bundle.tar.zst"
        require(bundle_path.is_file() and digest(bundle_path.read_bytes()) == end["bundle"]["sha256"],
                "accepted bundle is unavailable before recovery")
        before_inputs = {name: digest((result_dir / name).read_bytes()) for name in
                         ("accepted.json", "progress.json", "bundle.tar.zst")}
        for actor, expected in self.expected.items():
            if "session" in expected:
                require(self.inspect_owned(actor) is None, "session actor remains before recovery startup")
        self.stop_driver("service")
        recovery_site, cause = {
            "terminal_prepare_file_sync": ("resume_file_sync", "resume terminal: sync prepared draft"),
            "terminal_prepare_directory_sync": ("resume_directory_sync", "resume terminal: sync prepared result directory"),
            "terminal_publish_link": ("publish_link", "commit_prepared_terminal: no-clobber publish"),
            "terminal_publish_directory_sync": ("reconcile_directory_sync", "terminal sweep: sync committed publication"),
            "terminal_raw_parent_sync": ("raw_parent_sync", "not durably proved"),
            "terminal_temporary_unlink": ("reconcile_unlink_directory_sync", "terminal sweep after linked-publication recovery"),
            "terminal_temporary_unlink_directory_sync": ("reconcile_directory_sync", "terminal sweep: sync committed publication"),
        }[self.case]
        fault = self.arm_terminal_fault(recovery_site)
        self.start_driver("service", {"id": fault["id"], "cause": cause})
        require({name: digest((result_dir / name).read_bytes()) for name in before_inputs} == before_inputs,
                "refused recovery changed committed acceptance, progress, or bundle bytes")
        if self.case in ("terminal_prepare_file_sync", "terminal_prepare_directory_sync"):
            require(raw_tree.exists() and temporary.read_bytes() == candidate_bytes and not finished.exists(),
                    "failed prepare reconciliation discarded its draft or raw evidence")
        elif self.case == "terminal_raw_parent_sync":
            require(not raw_tree.exists() and temporary.exists() and not finished.exists(),
                    "failed absent-parent sync published an uncertain deletion")
            updated_bytes = temporary.read_bytes()
            updated = json.loads(updated_bytes)
            previous_diagnostics = candidate["terminal"]["teardown_diagnostics"]
            updated_diagnostics = updated["terminal"]["teardown_diagnostics"]
            require(len(updated_diagnostics) == len(previous_diagnostics) + 2 and
                    updated_diagnostics[:-2] == previous_diagnostics and
                    "terminalization: sync absent raw session tree" in updated_diagnostics[-2] and
                    "Input/output error (os error 5)" in updated_diagnostics[-2] and
                    updated_diagnostics[-1] == "terminal raw-state removal is visible but its durability barrier failed; "
                    "leaving the terminal draft unpublished for restart reconciliation at " + str(raw_tree),
                    "failed absence reconciliation changed or lost its exact diagnostic history")
            without_diagnostics = lambda value: {**value, "terminal": {**value["terminal"], "teardown_diagnostics": []}}
            require(without_diagnostics(updated) == without_diagnostics(candidate),
                    "failed absence reconciliation changed accepted facts beyond its new diagnostics")
            candidate_bytes, candidate = updated_bytes, updated
            save(self.root / "evidence/reconciled-private-diagnostics.json", candidate_bytes)
        elif self.case == "terminal_temporary_unlink":
            require(finished.read_bytes() == candidate_bytes and not temporary.exists(),
                    "real recovery unlink did not preserve the exact final before its sync failure")
        elif self.case == "terminal_publish_directory_sync":
            require(finished.read_bytes() == temporary.read_bytes() == candidate_bytes and
                    (finished.stat().st_dev, finished.stat().st_ino) == (temporary.stat().st_dev, temporary.stat().st_ino),
                    "failed linked-publication sync did not preserve the exact same-inode pair")
        elif self.case == "terminal_publish_link":
            require(not finished.exists() and temporary.read_bytes() == candidate_bytes,
                    "failed hard-link publication did not preserve the exact private draft")
        elif self.case == "terminal_temporary_unlink_directory_sync":
            require(finished.read_bytes() == candidate_bytes and not temporary.exists(),
                    "failed final-only reconciliation changed its exact publication")
        self.arm_terminal_fault(None)
        service = self.start_driver("service")
        status, reopened = self.http(service, "GET", route)
        require(status == 200 and json.loads(reopened) == candidate and finished.read_bytes() == candidate_bytes and
                not temporary.exists() and not raw_tree.exists(), "fresh process did not reconcile the exact accepted terminal")
        status, replayed = self.http(service, "POST", "/v1/agent/sessions", "request.json")
        require(status == 200 and json.loads(replayed) == candidate, "recovered operation was not idempotently retained")
        require({name: digest((result_dir / name).read_bytes()) for name in before_inputs} == before_inputs,
                "successful recovery changed committed inputs")
        status, bundle = self.http(service, "GET", route + "/bundle")
        require(status == 200 and digest(bundle) == end["bundle"]["sha256"], "recovery did not retain the accepted bundle")
        save(self.root / "evidence/bundle.tar.zst", bundle)
        boots = self.audit_terminal_faults()
        require(len(boots) == 3, "publication qualification did not observe three independent service boot identities")
        if self.case in ("terminal_prepare_file_sync", "terminal_prepare_directory_sync"):
            recovery_events = next(events for events in boots.values() if any(
                event["site"] == "raw_remove" and event["phase"] == "before" for event in events))
            def index(site, phase):
                return next(i for i, event in enumerate(recovery_events) if event["site"] == site and
                            event["phase"] == phase and event["injected_errno"] is None)
            require(index("resume_file_sync", "after") < index("resume_directory_sync", "after") <
                    index("raw_remove", "before") < index("raw_parent_sync", "after") < index("publish_link", "before"),
                    "fresh recovery did not establish durability before disposal and publication")
        self.stop_driver("service")
        self.stop_driver("broker")
        require(self.stub.generations == 2 and not self.stub.failures, "recovery resampled already completed work")
        return {"v": 1, "claim": "final-image execution and service publication", "case": self.case,
                "status": "passed", "images": self.images, "test_executables": self.artifact_manifest,
                "session_id": self.session, "terminal_sha256": digest(candidate_bytes), "bundle": end["bundle"],
                "recovery_boots": list(boots), "test_fault": self.case,
                "excluded": ["deployed-stack bootstrap", "real tokenizer correctness", "real model semantics",
                             "physical power-loss and storage-device behavior"]}

    def certify_available(self, service):
        result = self.command(["docker", "exec", service, "/gate/agent_service", "--exact",
                               "composition_gate::certify_available_stream", "--ignored", "--nocapture"])
        matches = [line.removeprefix(b"COMPOSITION_AVAILABLE_CERTIFICATE ")
                   for line in result.stdout.splitlines() if line.startswith(b"COMPOSITION_AVAILABLE_CERTIFICATE ")]
        save(self.root / "evidence" / f"available-certificate-{len(self.commands)}.stdout", result.stdout)
        require(len(matches) == 1, "available output has no unique production certificate")
        certificate = json.loads(matches[0])
        save_json(self.root / "evidence" / f"available-certificate-{len(self.commands)}.json", certificate)
        require(certificate["subtype"] == "success" and
                certificate["response"] == "COMPOSITION_OK " + self.nonce,
                "observation fault would mask an independent stream error")
        return certificate

    def observation_fault(self, service, route):
        lost_wait = self.case == "wait_observation_lost"
        marker = "wait-observation-held.json" if lost_wait else "capture-proof-held.json"
        held = self.root / "control" / marker
        self.wait_for(held.exists, "actual successful broker observation held after handler join")
        observation = json.loads(held.read_bytes())
        require(observation["ok"] is True and observation["error"] is None,
                "fault barrier did not follow the actual successful operation")
        agent = self.inspect_owned("agent-" + self.session)
        capture = self.inspect_owned("agent-capture-" + self.session)
        require(agent is not None and not agent["State"]["Running"] and
                agent["State"]["ExitCode"] == 0 and capture is not None and capture["State"]["Running"],
                "observation barrier is not after the successful producer exit with capture owned")
        result_dir = self.root / "results" / self.session
        output = self.root / "state/sessions" / self.session / "output"
        if lost_wait:
            require(observation["data"] == {"exit_code": 0}, "withheld wait was not the actual zero exit")
            # A wait observation precedes capture proof. Observe that the real
            # capture subsequently completed before checking its available bytes.
            self.wait_for(lambda: b"CAPTURE_COMPLETE " in self.command(
                ["docker", "logs", capture["Id"]]).stdout, "actual capture completion after held wait")
        before = (output / "events.jsonl").read_bytes()
        stderr = (output / "qwen.stderr").read_bytes()
        self.certify_available(service)
        if not lost_wait:
            require(observation["data"] == {"events_bytes": len(before), "stderr_bytes": len(stderr)},
                    "real capture proof does not describe the unmodified output")
        save(self.root / "evidence/events-before-fault.jsonl", before)
        save(self.root / "evidence/stderr-before-fault", stderr)
        if self.case in ("capture_byte_mismatch", "capture_mode_mismatch"):
            path = output / ("events.jsonl" if self.case == "capture_byte_mismatch" else "qwen.stderr")
            fd = os.open(path, os.O_RDWR | os.O_NOFOLLOW | os.O_CLOEXEC)
            try:
                prior = os.fstat(fd)
                require(prior.st_uid == prior.st_gid == 1000 and prior.st_mode & 0o7777 == 0o600 and
                        (prior.st_dev, prior.st_ino) == (path.lstat().st_dev, path.lstat().st_ino),
                        "owned capture output changed before injection")
                if self.case == "capture_byte_mismatch":
                    os.lseek(fd, 0, os.SEEK_END)
                    require(os.write(fd, b"\n") == 1, "byte fault was not written")
                else:
                    os.fchmod(fd, 0o640)
                os.fsync(fd)
                after = os.fstat(fd)
                save_json(self.root / "evidence/output-fault.json", {
                    "path": str(path), "device": after.st_dev, "inode": after.st_ino,
                    "before_bytes": prior.st_size, "after_bytes": after.st_size,
                    "before_mode": prior.st_mode, "after_mode": after.st_mode,
                })
            finally:
                os.close(fd)
            self.certify_available(service)
        if self.case == "capture_proof_held_cancel":
            status, raw = self.http(service, "POST", route + "/cancel")
            require(status == 202, "late cancellation was not accepted")
            save_json(self.root / "evidence/cancel-receipt.json", json.loads(raw))
            intent = result_dir / "cancel-requested.json"
            require(intent.is_file(), "late cancellation lacked durable intent")
            save(self.root / "evidence/cancel-intent.json", intent.read_bytes())
        status, raw = self.http(service, "GET", route)
        pending = json.loads(raw)
        require(status == 200 and pending["terminal"] is None, "held observation did not block terminal publication")
        require(not any((result_dir / name).exists() for name in
                        ("finished.json", "finished.json.tmp", "bundle.tar.zst")),
                "publication or bundling proceeded without mandatory observation")
        save_json(self.root / "evidence/held-resource.json", pending)
        save(self.root / "control/release-observation", b"actual observation and owned output inspected\n")
        body = self.wait_terminal(service, route)
        end = body["terminal"]
        cancelled = self.case == "capture_proof_held_cancel"
        require(body["status"] == ("cancelled" if cancelled else "completed") and
                end["container_exit_code"] == end["agent_exit_code"] == (None if lost_wait else 0),
                "observation fault invented or lost the actual lifecycle facts")
        require(end["is_process_error"] is (not cancelled), "observation fault has wrong process outcome")
        require(body["observed_unaccounted_records"] == 0 and body["observed_output_tokens"] == 16 and
                body["observed_reasoning_tokens"] == 0 and body["num_turns"] == 2,
                "refused certification lost independent served observations")
        if cancelled or lost_wait:
            require(end["agent_result"] is not None and end["agent_result"]["agent_result_subtype"] == "success",
                    "available trusted capture was not certified")
        else:
            require(end["agent_result"] is None, "unproved capture was promoted to complete-result certification")
        diagnostics = end["teardown_diagnostics"]
        if cancelled:
            require(diagnostics == [], "late cancellation introduced an unrelated failure")
        else:
            cause = {"wait_observation_lost": "docker wait failed:",
                     "capture_proof_lost": "trusted stream capture did not prove complete durable output:",
                     "capture_byte_mismatch": "trusted captured events output drift",
                     "capture_mode_mismatch": "trusted captured stderr output drift"}[self.case]
            require(any(cause in detail for detail in diagnostics), "intended observation refusal cause was not retained")
            if lost_wait or self.case == "capture_proof_lost":
                require(any("broker response is not one terminal-LF JSON record without CR bytes" in detail
                            for detail in diagnostics), "lost observation did not name the empty-response protocol refusal")
            elif self.case == "capture_byte_mismatch":
                require(any(f"mode=600 bytes={len(before) + 1} expected_bytes={len(before)}" in detail
                            for detail in diagnostics), "byte refusal did not identify the exact count contradiction")
            else:
                require(any(f"mode=640 bytes={len(stderr)} expected_bytes={len(stderr)}" in detail
                            for detail in diagnostics), "mode refusal also changed captured bytes")
        require(end["raw_session_tree_retained"] is False and
                not (self.root / "state/sessions" / self.session).exists(), "accepted forensic bundle did not own raw disposal")
        persisted = result_dir / "finished.json"
        require(json.loads(persisted.read_bytes()) == body and not persisted.with_name("finished.json.tmp").exists(),
                "observation fault terminal is not durably published")
        status, bundle = self.http(service, "GET", route + "/bundle")
        require(status == 200 and len(bundle) == end["bundle"]["compressed_bytes"] and
                digest(bundle) == end["bundle"]["sha256"], "forensic bundle differs from its commitment")
        archive = self.root / "evidence/bundle.tar.zst"
        save(archive, bundle)
        retained = self.command(["tar", "--zstd", "-xOf", str(archive), "output/events.jsonl"]).stdout
        require(retained == before + (b"\n" if self.case == "capture_byte_mismatch" else b""),
                "forensic bundle did not retain exact actual event bytes")
        retained_stderr = self.command(["tar", "--zstd", "-xOf", str(archive), "output/qwen.stderr"]).stdout
        require(retained_stderr == stderr, "forensic bundle changed captured stderr")
        require(self.stub.generations == 2 and not self.stub.failures, "observation fault resampled completed work")
        for actor, expected in self.expected.items():
            if "session" in expected:
                require(self.inspect_owned(actor) is None, "actor outlived observation-fault publication")
        self.stop_driver("service")
        self.stop_driver("broker")
        return {"v": 1, "claim": "final-image execution and service publication", "case": self.case,
                "status": "passed", "images": self.images, "test_executables": self.artifact_manifest,
                "session_id": self.session, "terminal_sha256": digest(persisted.read_bytes()),
                "bundle": end["bundle"], "events_sha256": digest(retained),
                "test_fault": self.case,
                "excluded": ["deployed-stack bootstrap", "real tokenizer correctness", "real model semantics",
                             "capture-owner EOF and sync fault injection"]}

    def provider_failure(self, service, route):
        body = self.wait_terminal(service, route)
        end = body["terminal"]
        require(body["status"] == "completed" and end["is_process_error"] is True and
                end["container_exit_code"] == end["agent_exit_code"] == 1,
                "provider failure did not stop the actual invocation")
        require(self.stub.generations == 1 and not self.stub.failures,
                "failed established response was resampled or the fixture failed")
        require(end["agent_result"] is not None and
                end["agent_result"]["agent_result_subtype"] == "error_during_execution" and
                end["teardown_diagnostics"] == [],
                "provider failure was confused with capture or teardown failure")
        require(end["raw_session_tree_retained"] is False and
                not (self.root / "state/sessions" / self.session).exists(),
                "settled provider failure did not dispose the committed raw tree")
        persisted = self.root / "results" / self.session / "finished.json"
        require(json.loads(persisted.read_bytes()) == body and
                not persisted.with_name("finished.json.tmp").exists(),
                "failed invocation publication is incomplete")
        status, bundle = self.http(service, "GET", route + "/bundle")
        require(status == 200 and len(bundle) == end["bundle"]["compressed_bytes"] and
                digest(bundle) == end["bundle"]["sha256"], "failed invocation bundle differs from commitment")
        archive = self.root / "evidence/bundle.tar.zst"
        save(archive, bundle)
        contents = {}
        for name in ("output/events.jsonl", "output/qwen.stderr", "control/container-logs.txt"):
            contents[name] = self.command(["tar", "--zstd", "-xOf", str(archive), name]).stdout
        events = contents["output/events.jsonl"]
        require(events.endswith(b"\n") and b"PROVIDER_FAILURE_PREFIX" in events,
                "the refused generation's observed prefix was lost")
        records = [json.loads(line) for line in events.splitlines()]
        terminal = records[-1]
        require(terminal["type"] == "result" and terminal["subtype"] == "error_during_execution" and
                terminal["usage"] == {"requests": 1, "usageReports": 0, "unfinalizedRequests": 0,
                                      "unreportedUsageRequests": 1, "usage": None},
                "provider failure fabricated served zero or lost its dispatched request")
        error_message = terminal["error"]["message"]
        expected_cause = (
            "Expected property name or '}' in JSON at position 1 (line 1 column 2)"
            if self.case == "provider_malformed_sse" else
            "terminated (cause: UND_ERR_RES_CONTENT_LENGTH_MISMATCH: Response body length does not match content-length header)"
        )
        require(expected_cause in error_message and error_message.encode() in contents["output/qwen.stderr"],
                "the actual provider refusal cause was lost or replaced by a later failure")
        require(end["response"] == "agent exited abnormally (container=Some(1), qwen=Some(1)). " + error_message,
                "service publication changed the certified provider failure cause")
        if self.case == "provider_malformed_sse":
            require(b"Could not parse message into JSON: {invalid-json}" in contents["output/qwen.stderr"],
                    "the actual SDK malformed-JSON refusal was not observed")
        require(not any(block["type"] in ("tool_use", "tool_result")
                        for record in records if record["type"] in ("assistant", "user")
                        for block in record["message"]["content"]),
                "failed generation admitted a tool effect")
        proof = re.findall(rb"CAPTURE_COMPLETE capture=unix-stream-capture-v1 events_bytes=(\d+) stderr_bytes=(\d+)",
                           contents["control/container-logs.txt"])
        require(len(proof) == 1 and tuple(map(int, proof[0])) ==
                (len(events), len(contents["output/qwen.stderr"])),
                "capture completion does not prove the failed invocation's bytes")
        for name, registration in self.expected.items():
            if "session" in registration:
                require(self.inspect_owned(name) is None, "failed invocation actor outlived publication")
        self.stop_driver("service")
        self.stop_driver("broker")
        return {"v": 1, "claim": "final-image execution and service publication", "case": self.case,
                "status": "passed", "images": self.images, "test_executables": self.artifact_manifest,
                "session_id": self.session, "terminal_sha256": digest(persisted.read_bytes()),
                "bundle": end["bundle"], "events_sha256": digest(events),
                "test_fault": self.case,
                "excluded": ["deployed-stack bootstrap", "real tokenizer correctness", "real model semantics"]}

    def finish(self, certificate):
        cleanup_failures = []
        # Every containment command retains its finite own timeout. An expired
        # execution watchdog cannot consume the later actors' cleanup allowance.
        self.containing = True
        releases = {"cancel_start_gate": ["release-create-response"], "cancel_held_sse": ["release-remove"],
                    "capture_proof_held_cancel": ["release-observation"], "capture_proof_lost": ["release-observation"],
                    "capture_byte_mismatch": ["release-observation"], "capture_mode_mismatch": ["release-observation"],
                    "wait_observation_lost": ["release-observation"],
                    "remove_stopped_capture_failed": ["release-remove-fault"],
                    "quiescence_observation_lost": ["release-observation", "release-remove-fault"]}
        for filename in releases.get(self.case, []):
            try:
                release = self.root / "control" / filename
                if not release.exists():
                    save(release, b"failed case containment release\n")
            except Exception as error:
                cleanup_failures.append(f"test barrier release failed: {error!r}")
        # Creators settle first. A session name observed absent while its
        # create_session handler still runs is not a cleanup postcondition.
        creators_settled = True
        for role in ("service", "broker"):
            name = self.tag + "-" + role
            try:
                observed = self.inspect_owned(name)
                if observed is not None:
                    try:
                        self.stop_driver(role)
                    except Exception as error:
                        creators_settled = False
                        cleanup_failures.append(f"{name} graceful settlement failed: {error!r}")
                        observed = self.inspect_owned(name)
                        if observed is not None:
                            if observed["State"]["Running"]:
                                creators_settled = False
                                self.command(["docker", "kill", "--signal=KILL", observed["Id"]])
                            self.command(["docker", "wait", observed["Id"]])
                            self.command(["docker", "container", "rm", observed["Id"]])
            except Exception as error:
                creators_settled = False
                cleanup_failures.append(f"{name} creator containment unproved: {error!r}")
        for name, registration in self.expected.items():
            if "session" not in registration:
                continue
            try:
                observed = self.inspect_owned(name)
                if observed is not None:
                    cleanup_failures.append(f"{name} required emergency containment")
                    logs = self.command(["docker", "logs", observed["Id"]], check=False)
                    save(self.root / "evidence" / (name + ".stdout"), logs.stdout)
                    save(self.root / "evidence" / (name + ".stderr"), logs.stderr)
                    if observed["State"]["Running"]:
                        self.command(["docker", "kill", "--signal=KILL", observed["Id"]])
                    self.command(["docker", "wait", observed["Id"]])
                    self.command(["docker", "container", "rm", observed["Id"]])
                    require(self.inspect_owned(name) is None, "contained actor still exists")
            except Exception as error:
                cleanup_failures.append(f"{name}: {error!r}")
        for name in self.expected:
            try:
                require(self.inspect_owned(name) is None, f"registered actor remains after reconciliation: {name}")
            except Exception as error:
                cleanup_failures.append(f"final reconciliation {name}: {error!r}")
        self.stub.shutdown()
        self.worker.join()
        self.stub.server_close()
        certificate["cleanup_failures"] = cleanup_failures
        certificate["creator_settlement"] = "joined" if creators_settled else "unproved_daemon_operation_tail"
        certificate["provider_failures"] = self.stub.failures
        certificate["provider_connections"] = self.stub.connections
        if not creators_settled or cleanup_failures or self.stub.failures or any(c["state"] != "handler_settled" for c in self.stub.connections):
            certificate["status"] = "failed"
        save_json(self.root / "provider-requests.json", self.stub.requests)
        save_json(self.root / "commands.json", self.commands)
        save_json(self.root / "actors.json", self.actors)
        save_json(self.root / "certificate.json", certificate)
        return certificate


class CaptureOwnerHarness:
    command = Harness.command
    inspect_owned = Harness.inspect_owned

    def __init__(self, args, case, events, parent):
        require(os.geteuid() == os.getegid() == 1000, "capture owner gate requires uid/gid 1000")
        self.source = Path(__file__).resolve().parents[2]
        self.lock = json.loads((self.source / "config/stack.lock.json").read_bytes())
        self.case, self.events, self.parent = case, events, parent
        self.root = Path(tempfile.mkdtemp(prefix="as-capture-"))
        require(self.root.parent == Path("/tmp") and self.root.resolve() == self.root, "capture fixture root drift")
        os.chmod(self.root, 0o700)
        print(f"Capture owner evidence: {self.root}", flush=True)
        self.tag = self.root.name
        self.name = self.tag + "-owner"
        self.images = {"capture": args.capture_image}
        self.expected = {self.name: {"image": args.capture_image, "gate": self.tag}}
        self.actors, self.commands = {}, []
        self.deadline, self.containing = time.monotonic() + 240, False
        self.stdout, self.stderr = b"", b""
        for name in ("input", "control", "output", "streams", "evidence"):
            (self.root / name).mkdir(mode=0o700)
        self.artifact_manifest = prepare_artifacts(self.source, args.artifacts, self.root / "input")
        self.copy_failure = case in ("events_copy_failure", "events_copy_failure_and_stderr_sync_eio")
        self.sync_failure = case in ("stderr_sync_eio", "events_copy_failure_and_stderr_sync_eio")
        self.limit = len(events) // 2 if self.copy_failure else None
        self.stderr_bytes = b"stderr-prefix\nstderr-tail:\xcf\x80\n"
        require(not self.copy_failure or len(self.stderr_bytes) < self.limit < len(events),
                "certified events cannot establish the declared file-limit case")
        require(parent["status"] == "passed" and digest(events) == parent["events_sha256"],
                "capture owner input lacks its preceding actual service certification")
        save(self.root / "input/events.jsonl", events)
        save_json(self.root / "input/capture-fixture.json", {"v": 1, "case": case,
                  "events_sha256": digest(events), "events_bytes": len(events), "copy_limit_bytes": self.limit})
        save_json(self.root / "input/parent-certification.json", parent)
        save_json(self.root / "registered-containers.json", self.expected)

    def logs(self):
        value = self.inspect_owned(self.name)
        require(value is not None, "capture owner disappeared before log settlement")
        result = self.command(["docker", "logs", value["Id"]])
        require(result.stdout.startswith(self.stdout) and result.stderr.startswith(self.stderr),
                "capture owner log prefix was lost or rewritten")
        self.stdout, self.stderr = result.stdout, result.stderr
        # A live Docker log snapshot may end partway through a record. Only
        # terminal-LF records are observations; the exact tail remains retained.
        complete = self.stdout[:self.stdout.rfind(b"\n") + 1]
        return complete.splitlines()

    def wait_receipt(self, predicate):
        while True:
            lines = self.logs()
            receipts = [json.loads(line.removeprefix(b"CAPTURE_OWNER_RECEIPT ")) for line in lines
                        if line.startswith(b"CAPTURE_OWNER_RECEIPT ")]
            if any(predicate(receipt) for receipt in receipts):
                return lines
            require(self.inspect_owned(self.name)["State"]["Running"], "capture owner exited before required receipt")
            require(time.monotonic() < self.deadline, "capture receipt watchdog expired")
            time.sleep(0.05)

    def release(self, name):
        path = self.root / "control" / name
        if not path.exists():
            save(path, b"capture owner control released\n")

    def run(self):
        require(self.inspect_owned(self.name) is None, "capture test name already exists")
        limits = self.lock["capture"]
        memory = int(self.command(["numfmt", "--from=iec", limits["memory"].upper()]).stdout)
        memory_swap = int(self.command(["numfmt", "--from=iec", limits["memory_swap"].upper()]).stdout)
        argv = ["docker", "run", "-d", "--name", self.name,
                "--label", "agent_service.composition_gate=" + self.tag,
                "--network", "none", "--restart", "no", "--read-only", "--cap-drop", "ALL",
                "--security-opt", "no-new-privileges:true", "--user", "1000:1000",
                "--memory", limits["memory"], "--memory-swap", limits["memory_swap"],
                "--pids-limit", str(limits["pids_limit"])]
        expected_mounts = {}
        for source, target, writable in (("streams", "/streams", True), ("output", "/output", True),
                                         ("input", "/gate", False), ("control", "/gate-control", False)):
            path = str(self.root / source)
            argv += ["--mount", f"type=bind,src={path},dst={target}" + ("" if writable else ",readonly")]
            expected_mounts[target] = (path, writable)
        argv += ["--entrypoint", "/gate/session_capture", self.images["capture"], "--exact",
                 "capture_owner::final_image_capture_owner", "--ignored", "--nocapture"]
        created = self.command(argv).stdout.decode().strip()
        require(re.fullmatch(r"[0-9a-f]{64}", created) is not None, "capture Docker run identity is invalid")
        self.actors[self.name] = {"id": created}
        value = self.inspect_owned(self.name)
        config = value["HostConfig"]
        require(value["Config"]["User"] == "1000:1000" and config["ReadonlyRootfs"] and
                config["NetworkMode"] == "none" and config["CapDrop"] == ["ALL"] and
                config["SecurityOpt"] == ["no-new-privileges:true"] and config["PidsLimit"] == limits["pids_limit"] and
                config["Memory"] == memory and config["MemorySwap"] == memory_swap and
                config["RestartPolicy"] == {"Name": "no", "MaximumRetryCount": 0} and
                value["Config"]["Entrypoint"] == ["/gate/session_capture"] and
                value["Config"]["Cmd"] == ["--exact", "capture_owner::final_image_capture_owner", "--ignored", "--nocapture"],
                "capture owner runtime constraints drifted")
        mounts = {entry["Destination"]: (entry["Source"], entry["RW"]) for entry in value["Mounts"]}
        require(len(value["Mounts"]) == len(expected_mounts) and mounts == expected_mounts and
                all(entry["Type"] == "bind" for entry in value["Mounts"]), "capture owner bootstrap mounts drifted")
        save_json(self.root / "evidence/started-inspect.json", value)
        if self.case == "delayed_stderr_and_last_sync":
            lines = self.wait_receipt(lambda r: r["site"] == "after_copy" and r["output"] == "events")
            require(not any(line.startswith(b"CAPTURE_COMPLETE ") for line in lines),
                    "capture completed before stderr EOF")
            require((self.root / "output/events.jsonl").read_bytes() == self.events,
                    "events EOF receipt lacks exact completed event bytes")
            self.release("release-stderr")
            lines = self.wait_receipt(lambda r: r["site"] == "before_io" and r["operation"] == "sync" and r["output"] == "stderr")
            require(not any(line.startswith(b"CAPTURE_COMPLETE ") for line in lines),
                    "capture completed before its final output sync")
            require((self.root / "output/qwen.stderr").read_bytes() == self.stderr_bytes,
                    "delayed stderr tail did not reach the actual output")
            self.release("release-sync")
            while not any(line.startswith(b"CAPTURE_COMPLETE ") for line in self.logs()):
                require(self.inspect_owned(self.name)["State"]["Running"], "capture exited before COMPLETE")
                require(time.monotonic() < self.deadline, "capture completion watchdog expired")
                time.sleep(0.05)
            self.command(["docker", "kill", "--signal", "TERM", created])
        waited = self.command(["docker", "wait", created], timeout=60)
        lines = self.logs()
        value = self.inspect_owned(self.name)
        require(waited.stdout.strip() == b"0" and value["State"]["Running"] is False and
                value["State"]["ExitCode"] == 0 and value["State"]["OOMKilled"] is False,
                "capture test bootstrap did not join and audit successfully")
        require(self.stdout.endswith(b"\n"), "settled capture test output has an incomplete tail")
        receipts = [json.loads(line.removeprefix(b"CAPTURE_OWNER_RECEIPT ")) for line in lines
                    if line.startswith(b"CAPTURE_OWNER_RECEIPT ")]
        outcomes = [json.loads(line.removeprefix(b"CAPTURE_OWNER_OUTCOME ")) for line in lines
                    if line.startswith(b"CAPTURE_OWNER_OUTCOME ")]
        require(len(outcomes) == 1 and outcomes[0]["writers"] == "joined" and outcomes[0]["receipts"] == receipts and
                [r["sequence"] for r in receipts] == list(range(1, len(receipts) + 1)),
                "capture body/writer audit differs from its complete actual log")
        succeeded = self.case == "delayed_stderr_and_last_sync"
        require(outcomes[0]["outcome"]["state"] == ("ok" if succeeded else "error"), "capture body outcome lost")
        complete = [i for i, line in enumerate(lines) if line.startswith(b"CAPTURE_COMPLETE ")]
        stopped = [line for line in lines if line.startswith(b"CAPTURE_STOPPED ")]
        require(len(complete) == len(stopped) == int(succeeded) and
                not any(line.startswith(b"CAPTURE_ABORTED ") for line in lines), "capture marker disposition differs")
        if succeeded:
            require(lines[complete[0]] == (f"CAPTURE_COMPLETE capture={limits['capture_id']} events_bytes={len(self.events)} "
                    f"stderr_bytes={len(self.stderr_bytes)}").encode(), "capture completion byte counts differ")
            last_sync = next(i for i, line in enumerate(lines) if line.startswith(b"CAPTURE_OWNER_RECEIPT ") and
                             json.loads(line.removeprefix(b"CAPTURE_OWNER_RECEIPT ")).get("site") == "after_io" and
                             json.loads(line.removeprefix(b"CAPTURE_OWNER_RECEIPT ")).get("operation") == "sync" and
                             json.loads(line.removeprefix(b"CAPTURE_OWNER_RECEIPT ")).get("output") == "stderr")
            require(last_sync < complete[0], "COMPLETE preceded the actual final sync return")
        for name in ("events.jsonl", "qwen.stderr"):
            metadata = (self.root / "output" / name).lstat()
            require(stat.S_ISREG(metadata.st_mode) and metadata.st_nlink == 1 and
                    metadata.st_uid == metadata.st_gid == 1000 and metadata.st_mode & 0o7777 == 0o600,
                    "capture output ownership or mode drifted")
        actual_events = (self.root / "output/events.jsonl").read_bytes()
        require(actual_events == (self.events[:self.limit] if self.copy_failure else self.events),
                "capture file bytes differ from the declared full input or real limited prefix")
        require((self.root / "output/qwen.stderr").read_bytes() == self.stderr_bytes, "captured stderr bytes differ")
        require(not any((self.root / "streams").iterdir()), "capture listener paths survived acceptance")
        save_json(self.root / "evidence/settled-inspect.json", value)
        self.command(["docker", "container", "rm", created])
        require(self.inspect_owned(self.name) is None, "capture owner remained after settled removal")
        return {"v": 1, "case": self.case, "status": "passed", "claim": "fixed capture body EOF and output finalization",
                "images": self.images, "test_executables": self.artifact_manifest, "parent": self.parent,
                "outcome": outcomes[0], "output_events_sha256": digest(actual_events),
                "excluded": ["ordinary broker-created capture bootstrap", "physical power-loss durability",
                             "complete drain after cancellation", "real model semantics"]}

    def finish(self, certificate):
        self.containing = True
        failures = []
        for name in ("release-stderr", "release-sync"):
            try:
                self.release(name)
            except Exception as error:
                failures.append(f"capture barrier containment: {error!r}")
        def observe():
            try:
                value = self.inspect_owned(self.name)
                return {"state": "absent"} if value is None else {"state": "present", "value": value}
            except Exception as error:
                failures.append(f"capture containment observation failed: {error!r}")
                return {"state": "unavailable", "cause": repr(error)}

        def attempt(argv, timeout=45):
            try:
                self.command(argv, timeout=timeout)
                return True
            except Exception as error:
                failures.append(f"capture containment {argv!r} failed: {error!r}")
                return False

        observed = observe()
        if observed["state"] == "unavailable":
            observed = observe()
        if observed["state"] == "present":
            value = observed["value"]
            failures.append("capture owner required unplanned containment")
            identity = value["Id"]
            if value["State"]["Running"]:
                attempt(["docker", "kill", "--signal", "TERM", identity])
            waited = attempt(["docker", "wait", identity], timeout=30)
            if not waited:
                # A failed TERM observation is not authority to skip joining.
                # Force only a container freshly proved still running/owned.
                observed = observe()
                if observed["state"] == "present" and observed["value"]["State"]["Running"]:
                    value = observed["value"]
                    attempt(["docker", "kill", value["Id"]])
                    attempt(["docker", "wait", value["Id"]], timeout=30)
            try:
                self.logs()
            except Exception as error:
                failures.append(f"capture containment logs failed: {error!r}")
            observed = observe()
            if observed["state"] == "present":
                value = observed["value"]
                if value["State"]["Running"]:
                    failures.append("capture remained running after containment")
                else:
                    attempt(["docker", "container", "rm", value["Id"]])
        try:
            require(self.inspect_owned(self.name) is None, "capture owner absence unproved")
        except Exception as error:
            failures.append(f"capture final absence failed: {error!r}")
        if any(command["state"] != "returned" for command in self.commands):
            failures.append("capture Docker operation has an unproved daemon tail")
        if failures:
            certificate["status"] = "failed"
        certificate["cleanup_failures"] = failures
        save(self.root / "evidence/capture.stdout", self.stdout)
        save(self.root / "evidence/capture.stderr", self.stderr)
        save_json(self.root / "commands.json", self.commands)
        save_json(self.root / "actors.json", self.actors)
        save_json(self.root / "certificate.json", certificate)
        return certificate


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifacts", required=True)
    for name in ("agent", "relay", "capture", "broker", "service"):
        parser.add_argument("--" + name + "-image", required=True)
    args = parser.parse_args()
    certified_events = None
    parent_certificate = None
    for case in ("tool_cycle", "cancel_start_gate", "cancel_held_sse",
                 "provider_malformed_sse", "provider_disconnected_sse", "capture_proof_held_cancel",
                 "capture_proof_lost", "capture_byte_mismatch", "capture_mode_mismatch", "wait_observation_lost",
                 "terminal_prepare_file_sync", "terminal_prepare_directory_sync", "terminal_publish_link",
                 "terminal_publish_directory_sync", "terminal_raw_parent_sync", "terminal_temporary_unlink",
                 "terminal_temporary_unlink_directory_sync", "remove_stopped_capture_failed", "quiescence_observation_lost"):
        harness = Harness(args, case)
        try:
            certificate = harness.run()
        except BaseException as error:
            certificate = {"v": 1, "claim": "final-image execution and service publication", "case": case,
                           "status": "failed", "failure": repr(error), "images": harness.images}
        certificate = harness.finish(certificate)
        print(json.dumps(certificate, indent=2), flush=True)
        if certificate["status"] != "passed":
            return 1
        if case == "tool_cycle":
            certified_events = harness.certified_events
            parent_certificate = certificate
    require(certified_events is not None and parent_certificate is not None, "capture stage lacks service-certified input")
    for case in ("delayed_stderr_and_last_sync", "stderr_sync_eio", "events_copy_failure", "events_copy_failure_and_stderr_sync_eio"):
        harness = CaptureOwnerHarness(args, case, certified_events, parent_certificate)
        try:
            certificate = harness.run()
        except BaseException as error:
            certificate = {"v": 1, "case": case, "status": "failed", "failure": repr(error), "images": harness.images}
        certificate = harness.finish(certificate)
        print(json.dumps(certificate, indent=2), flush=True)
        if certificate["status"] != "passed":
            return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
