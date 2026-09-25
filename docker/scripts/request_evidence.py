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
        representation = request["body"]
        if representation["kind"] == "full":
            body = representation["json"]
        elif representation["kind"] == "delta":
            previous_id, previous_segment, previous_messages = scopes[request["kv_scope"]]
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
              "sequence": 0, "http": False, "ended": False, "bytes": bytearray()}
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
        if response["request_id"] not in admitted or state is None or state["ended"] or state["journal"] != response["journal_id"] or response["sequence"] != state["sequence"] + 1:
            raise ValueError("response identity or sequence is incomplete")
        event = response["event"]
        actual = state["actual"]
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
        else:
            raise ValueError("unknown response event; use the matching harness")
        state["sequence"] += 1
    if any(not state["ended"] for state in states.values()):
        raise ValueError("response completion is missing; inspect the original stream")
    return responses
