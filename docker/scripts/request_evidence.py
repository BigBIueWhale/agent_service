"""Replay exact client request evidence and compare it to bytes received by a provider."""
from __future__ import annotations
import base64
import hashlib
import json


def message_bytes(body: str) -> list[str]:
    decoder = json.JSONDecoder()
    at = 1
    while at < len(body) - 1:
        key, at = decoder.raw_decode(body, at)
        if body[at] != ":":
            raise ValueError("request body has invalid object framing")
        at += 1
        if key == "messages":
            if body[at] != "[":
                raise ValueError("request messages is not an array")
            at += 1
            messages = []
            while body[at] != "]":
                value, end = decoder.raw_decode(body, at)
                if not isinstance(value, dict):
                    raise ValueError("request message is not an object")
                messages.append(body[at:end])
                at = end
                if body[at] == ",":
                    at += 1
            return messages
        _, at = decoder.raw_decode(body, at)
        if body[at] == ",":
            at += 1
    raise ValueError("request body has no top-level messages")


def require_request_evidence(events: list[dict], received: list[str]) -> list[dict]:
    if not events or events[0].get("type") != "system" or events[0].get("subtype") != "stream_start":
        raise ValueError("request evidence has no stream_start header; retain the complete stdout")
    requests = [event["request"] for event in events if event.get("type") == "model_request"]
    terminal = events[-1]["request_evidence"]
    origin = events[0]["request_evidence_origin"]
    if origin != {"journal_id": terminal["journal_id"], "first_sequence": terminal["first_sequence"]}:
        raise ValueError("request capture origin differs from terminal accounting")
    if terminal["request_count"] != len(requests) or len(requests) != len(received):
        raise ValueError("request capture omitted an actual provider call; inspect pipeline admission")
    scopes: dict[str, tuple[str, str, list[str]]] = {}
    seen = set()
    for offset, (request, actual) in enumerate(zip(requests, received)):
        if request["journal_id"] != terminal["journal_id"] or request["sequence"] != terminal["first_sequence"] + offset or request["request_id"] in seen:
            raise ValueError("request capture identity or sequence is incomplete")
        seen.add(request["request_id"])
        owner = request["owner"]
        if owner != {"kind": "utility"} and not (set(owner) == {"kind", "attempt_id"} and owner["kind"] == "chat" and isinstance(owner["attempt_id"], str) and owner["attempt_id"]):
            raise ValueError("request ownership is unknown; inspect the matching client")
        representation = request["body"]
        if representation["kind"] == "full":
            body = representation["json"]
        elif representation["kind"] == "delta":
            previous = scopes.get(request["kv_scope"])
            if previous is None:
                raise ValueError("request delta has no invocation base; retain the complete request evidence from its stream_start header")
            previous_id, previous_segment, previous_messages = previous
            retained = representation["retain_messages"]
            if previous_id != representation["base_request_id"] or previous_segment != request["segment_id"] or type(retained) is not int or not 0 <= retained <= len(previous_messages):
                raise ValueError("request delta references unavailable messages")
            body = representation["prefix"] + ",".join(previous_messages[:retained] + representation["added_messages"]) + representation["suffix"]
        else:
            raise ValueError("unknown request representation; use the matching harness")
        encoded = body.encode("utf-8")
        if body != actual or len(encoded) != request["body_bytes"] or hashlib.sha256(encoded).hexdigest() != request["body_sha256"]:
            raise ValueError("replayed request differs from bytes received by provider")
        if json.loads(body)["kv_scope"] != request["kv_scope"]:
            raise ValueError("request body and evidence name different invocations")
        scopes[request["kv_scope"]] = request["request_id"], request["segment_id"], message_bytes(body)
    return requests


def require_response_evidence(events: list[dict], served: list[dict]) -> list[dict]:
    """Verify every observed byte against the fake provider, including failed prefixes."""
    requests = [event["request"] for event in events if event.get("type") == "model_request"]
    if len(requests) != len(served) or events[-1]["request_evidence"]["open_response_ids"] != []:
        raise ValueError("response recording is incomplete; inspect the provider and recorder")
    states = {request["request_id"]: {"journal": request["journal_id"], "actual": actual,
              "sequence": 0, "http": False, "termination": None, "ended": False, "outcome": None, "closed": False, "owner": request["owner"], "bytes": bytearray()}
              for request, actual in zip(requests, served)}
    if len(states) != len(requests):
        raise ValueError("response requests reuse an identity; inspect the original stream")
    admitted = set()
    responses = []
    for envelope in events:
        if envelope.get("type") == "model_request":
            admitted.add(envelope["request"]["request_id"])
        if envelope.get("type") != "model_response":
            continue
        response = envelope["response"]
        responses.append(response)
        state = states.get(response["request_id"])
        if response["request_id"] not in admitted or state is None or state["closed"] or state["journal"] != response["journal_id"] or response["sequence"] != state["sequence"] + 1:
            raise ValueError("response identity or sequence is incomplete")
        event = response["event"]
        actual = state["actual"]
        if (event["kind"] == "history") != (state["outcome"] is not None) or (event["kind"] != "history" and (event["kind"] == "outcome") != state["ended"]):
            raise ValueError("response outcome must follow transport completion; inspect the original stream")
        if event["kind"] == "http":
            if state["http"] or state["sequence"] or event["status"] != actual["response_status"] or event["content_type"] != actual["response_content_type"]:
                raise ValueError("recorded HTTP response differs from provider")
            state["http"] = True
        elif event["kind"] == "body":
            decoded = base64.b64decode(event["base64"], validate=True)
            if not state["http"] or not decoded or base64.b64encode(decoded).decode() != event["base64"] or event["offset"] != len(state["bytes"]):
                raise ValueError("response bytes are malformed or out of order")
            state["bytes"].extend(decoded)
        elif event["kind"] == "end":
            observed = bytes(state["bytes"])
            expected = b"".join(base64.b64decode(part, validate=True) for part in actual["response_chunks"])
            termination = event["termination"]
            if event["body_bytes"] != len(observed) or event["body_sha256"] != hashlib.sha256(observed).hexdigest():
                raise ValueError("response completion omits or changes observed bytes")
            if termination == "eof":
                if not state["http"] or event["error"] is not None or observed != expected:
                    raise ValueError("complete response differs from provider bytes")
            elif termination in ("failed", "cancelled"):
                if not expected.startswith(observed):
                    raise ValueError("failed response prefix differs from provider bytes")
            else:
                raise ValueError("served response was incorrectly called undispatched")
            state["ended"] = True
            state["termination"] = termination
        elif event["kind"] == "outcome":
            progress = (event.get("sdk_values_seen"), event.get("pipeline_outputs_delivered"))
            if (set(event) != {"kind", "status", "error", "served_usage", "sdk_values_seen", "pipeline_outputs_delivered"}
                    or any(type(value) is not int or not 0 <= value <= 2**53 - 1 for value in progress)
                    or (progress[0] > 0 and not state["http"])
                    or (progress[1] > 0 and progress[0] == 0)
                    or event["status"] not in ("completed", "failed", "cancelled")
                    or not (event["error"] is None or isinstance(event["error"], str))
                    or (event["status"] == "completed" and event["error"] is not None)
                    or (event["status"] == "failed" and not isinstance(event["error"], str))):
                raise ValueError("invalid response processing outcome; inspect the original stream")
            if event["status"] == "completed" and (not state["http"] or not 200 <= actual["response_status"] < 300 or state["termination"] not in ("eof", "cancelled")):
                raise ValueError("completed processing has no successful HTTP transport completion; inspect the original stream")
            usage = event["served_usage"]
            fields = {"promptTokenCount", "candidatesTokenCount", "totalTokenCount", "thoughtsTokenCount", "cachedContentTokenCount"}
            if usage is not None and (not isinstance(usage, dict) or set(usage) != fields
                    or any(type(value) is not int or not 0 <= value <= 2**53 - 1 for value in usage.values())
                    or usage["totalTokenCount"] != usage["promptTokenCount"] + usage["candidatesTokenCount"]
                    or usage["thoughtsTokenCount"] > usage["candidatesTokenCount"]
                    or usage["cachedContentTokenCount"] > usage["promptTokenCount"]):
                raise ValueError("invalid response served usage; inspect the original stream")
            if event["status"] == "completed" and "served_usage" in actual and usage != actual["served_usage"]:
                raise ValueError("completed response usage differs from provider; inspect the original stream")
            state["outcome"] = event["status"]
            state["closed"] = state["owner"]["kind"] == "utility"
        elif event["kind"] == "history":
            if set(event) != {"kind", "disposition"} or state["owner"]["kind"] != "chat" or event["disposition"] not in ("accepted", "abandoned") or (event["disposition"] == "accepted" and state["outcome"] != "completed"):
                raise ValueError("chat history decision has no valid processing owner")
            state["closed"] = True
        else:
            raise ValueError("unknown response event; use the matching harness")
        state["sequence"] += 1
    if any(not state["closed"] for state in states.values()):
        raise ValueError("response completion is missing; inspect the original stream")
    return responses


def require_output_ownership(events: list[dict]) -> None:
    """Every model output fragment resolves to one chat attempt and decision."""
    attempts = {}
    requests = {}
    for envelope in events:
        if envelope.get("type") == "model_request":
            request = envelope["request"]
            requests[request["request_id"]] = request
            owner = request["owner"]
            if owner["kind"] == "chat":
                previous = attempts.setdefault(owner["attempt_id"], {"scope": request["kv_scope"], "accepted": False, "output_scope": []})
                if previous["scope"] != request["kv_scope"] or previous["accepted"]:
                    raise ValueError("chat attempt changes scope or continues after acceptance")
        if envelope.get("type") == "model_response" and envelope["response"]["event"]["kind"] == "history":
            response = envelope["response"]
            attempt = attempts[requests[response["request_id"]]["owner"]["attempt_id"]]
            if response["event"]["disposition"] == "accepted":
                if attempt["accepted"]:
                    raise ValueError("chat attempt accepts multiple physical responses")
                attempt["accepted"] = True
        content = envelope.get("type") == "assistant" or (envelope.get("type") == "stream_event" and envelope["event"]["type"] not in ("goal_state", "active_goal", "tool_progress"))
        if not content:
            continue
        origin = envelope["origin"]
        if origin == {"kind": "runtime"}:
            continue
        if set(origin) != {"kind", "attempt_id", "kv_scope"} or origin["kind"] != "model":
            raise ValueError("assistant output has unknown ownership")
        attempt = attempts.get(origin["attempt_id"])
        scope = envelope.get("parent_tool_use_id")
        if origin["kv_scope"] != (scope or envelope["session_id"]) or attempt is None or attempt["scope"] != origin["kv_scope"] or (attempt["output_scope"] and attempt["output_scope"] != [scope]):
            raise ValueError("assistant output has no matching request owner")
        attempt["output_scope"] = [scope]
