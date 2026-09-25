"""Replay exact client request evidence and compare it to bytes received by a provider."""
from __future__ import annotations
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
