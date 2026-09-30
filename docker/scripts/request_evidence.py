"""Replay exact client request evidence and compare it to bytes received by a provider."""
from __future__ import annotations
import base64
import hashlib
import json
import math
import struct
from urllib.parse import urlsplit


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


def require_request_evidence(events: list[dict], received: list[dict]) -> list[dict]:
    if not events or events[0].get("type") != "system" or events[0].get("subtype") != "stream_start":
        raise ValueError("request evidence has no stream_start header; retain the complete stdout")
    requests = [event["request"] for event in events if event.get("type") == "model_request"]
    physical = [event for event in events if event.get("type") in ("model_request", "model_utility_request")]
    terminal = events[-1]["request_evidence"]
    origin = events[0]["request_evidence_origin"]
    if origin != {"journal_id": terminal["journal_id"], "first_sequence": terminal["first_sequence"]}:
        raise ValueError("request capture origin differs from terminal accounting")
    if terminal["request_count"] != len(physical) or len(physical) != len(received):
        raise ValueError("request capture omitted an actual provider call; inspect pipeline admission")
    scopes: dict[str, tuple[str, str, list[str]]] = {}
    operations: dict[str, dict] = {}
    seen = set()
    for offset, (envelope, actual) in enumerate(zip(physical, received)):
        request = envelope["request"] if envelope["type"] == "model_request" else envelope["utility_request"]
        if request["journal_id"] != terminal["journal_id"] or request["sequence"] != terminal["first_sequence"] + offset or request["request_id"] in seen:
            raise ValueError("request capture identity or sequence is incomplete")
        seen.add(request["request_id"])
        if envelope["type"] == "model_utility_request":
            body = request["body_json"]
            encoded = body.encode("utf-8")
            url = urlsplit(request["request_url"])
            kind = request["kind"]
            if (actual.get("path") != url.path or url.scheme not in ("http", "https") or not url.netloc
                    or body != actual.get("raw_body") or len(encoded) != request["body_bytes"]
                    or hashlib.sha256(encoded).hexdigest() != request["body_sha256"]):
                raise ValueError("utility request differs from bytes received by provider")
            parsed = json.loads(body)
            if not isinstance(parsed, dict) or parsed != actual.get("body") or parsed.get("model") != request["requested_model"]:
                raise ValueError("utility request body changes the selected model or provider input")
            if kind == "embedding":
                inputs = parsed.get("input")
                count = 1 if isinstance(inputs, str) else len(inputs) if isinstance(inputs, list) and all(isinstance(item, str) for item in inputs) else 0
                if (not url.path.endswith("/embeddings") or parsed.get("encoding_format") != "base64"
                        or request["expected_max_model_len"] is not None
                        or type(request["requested_input_count"]) is not int
                        or request["requested_input_count"] != count or count == 0):
                    raise ValueError("embedding request lost its separate text inputs")
            elif kind == "tokenize_chat":
                if (not url.path.endswith("/tokenize") or not isinstance(parsed.get("messages"), list)
                        or parsed.get("add_generation_prompt") is not True or "prompt" in parsed
                        or request["requested_input_count"] is not None
                        or type(request["expected_max_model_len"]) is not int
                        or request["expected_max_model_len"] <= 0):
                    raise ValueError("chat tokenizer request lost its rendered input or selected context limit")
            elif kind == "tokenize_text":
                if (not url.path.endswith("/tokenize") or not isinstance(parsed.get("prompt"), str)
                        or parsed.get("add_special_tokens") is not False or "messages" in parsed
                        or request["requested_input_count"] is not None
                        or type(request["expected_max_model_len"]) is not int
                        or request["expected_max_model_len"] <= 0):
                    raise ValueError("text tokenizer request lost its rendered input or selected context limit")
            else:
                raise ValueError("unknown physical utility request kind; use the matching harness")
            identity = {key: value for key, value in request.items() if key not in ("sequence", "request_id")}
            old = operations.setdefault(request["operation_id"], identity)
            if old != identity:
                raise ValueError("utility retry changed its serialized request or owner")
            continue
        if actual.get("path") != "/v1/chat/completions":
            raise ValueError("Chat request reached another provider route")
        actual_body = actual.get("raw_body")
        owner = request["owner"]
        if not ((set(owner) == {"kind", "operation_id", "purpose"} and owner["kind"] == "utility" and isinstance(owner["operation_id"], str) and owner["operation_id"]
                 and owner["purpose"] in ("compaction", "other"))
                or (set(owner) == {"kind", "attempt_id"} and owner["kind"] == "chat" and isinstance(owner["attempt_id"], str) and owner["attempt_id"])):
            raise ValueError("request ownership is unknown; inspect the matching client")
        representation = request["body"]
        previous = scopes.get(request["kv_scope"])
        if representation["kind"] == "full":
            if previous is not None and previous[1] == request["segment_id"]:
                raise ValueError("full request body repeats an active invocation segment; inspect the original request evidence with the matching client")
            body = representation["json"]
        elif representation["kind"] == "delta":
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
        if body != actual_body or len(encoded) != request["body_bytes"] or hashlib.sha256(encoded).hexdigest() != request["body_sha256"]:
            raise ValueError("replayed request differs from bytes received by provider")
        parsed = json.loads(body)
        if parsed["kv_scope"] != request["kv_scope"]:
            raise ValueError("request body and evidence name different invocations")
        policy = request["decode_policy"]
        if (policy.get("mode") not in ("stream", "nonstream")
                or not isinstance(policy.get("model"), str) or not policy["model"].strip()
                or type(parsed.get("stream")) is not bool
                or parsed["stream"] != (policy["mode"] == "stream")
                or parsed.get("model") != policy["model"]):
            raise ValueError("selected decoder contradicts the dispatched request; inspect the original request and matching client")
        scopes[request["kv_scope"]] = request["request_id"], request["segment_id"], message_bytes(body)
    return requests


def require_response_evidence(events: list[dict], served: list[dict]) -> list[dict]:
    """Verify every observed byte against the fake provider, including failed prefixes."""
    physical = [event for event in events if event.get("type") in ("model_request", "model_utility_request")]
    requests = [event["request"] if event["type"] == "model_request" else event["utility_request"]
                for event in physical]
    if len(requests) != len(served) or events[-1]["request_evidence"]["open_response_ids"] != []:
        raise ValueError("response recording is incomplete; inspect the provider and recorder")
    states = {request["request_id"]: {"journal": request["journal_id"], "actual": actual,
              "sequence": 0, "http": False, "termination": None, "ended": False, "outcome": None, "closed": False,
              "owner": request["owner"] if envelope["type"] == "model_request" else {"kind": "physical_utility"}, "bytes": bytearray(),
              "decoded_outputs": [], "decoded_failure": None, "decoded_pending": None}
              for envelope, request, actual in zip(physical, requests, served)}
    if len(states) != len(requests):
        raise ValueError("response requests reuse an identity; inspect the original stream")
    admitted = set()
    responses = []
    for envelope in events:
        if envelope.get("type") in ("model_request", "model_utility_request"):
            admitted.add((envelope["request"] if envelope["type"] == "model_request" else envelope["utility_request"])["request_id"])
        if envelope.get("type") != "model_response":
            continue
        response = envelope["response"]
        state = states.get(response["request_id"])
        if response["request_id"] not in admitted or state is None or state["closed"] or state["journal"] != response["journal_id"] or response["sequence"] != state["sequence"] + 1:
            raise ValueError("response identity or sequence is incomplete")
        event = response["event"]
        actual = state["actual"]
        kind = event["kind"]
        if state["owner"]["kind"] != "physical_utility":
            responses.append(response)
        if ((kind in ("history", "delivery")) != (state["outcome"] is not None)
                or (kind == "outcome" and not state["ended"])
                or (kind in ("http", "body", "end") and state["ended"])):
            raise ValueError("response outcome must follow transport completion; inspect the original stream")
        if kind == "http":
            if state["http"] or state["sequence"] or event["status"] != actual["response_status"] or event["content_type"] != actual["response_content_type"]:
                raise ValueError("recorded HTTP response differs from provider")
            state["http"] = True
        elif kind == "body":
            decoded = base64.b64decode(event["base64"], validate=True)
            if not state["http"] or not decoded or base64.b64encode(decoded).decode() != event["base64"] or event["offset"] != len(state["bytes"]):
                raise ValueError("response bytes are malformed or out of order")
            state["bytes"].extend(decoded)
        elif kind == "end":
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
        elif kind in ("decoded_body", "decoded_end"):
            role = event.get("role")
            index = event.get("index")
            expected_index = len(state["decoded_outputs"]) if role == "utility" else 0
            if (not state["http"] or role not in ("utility", "failure")
                    or (role == "utility" and state["owner"]["kind"] != "utility")
                    or (role == "failure" and state["decoded_failure"] is not None)
                    or type(index) is not int or index != expected_index):
                raise ValueError("decoded response has no matching owner or index")
            pending = state["decoded_pending"]
            if kind == "decoded_body":
                if set(event) != {"kind", "role", "index", "offset", "base64"}:
                    raise ValueError("decoded response has unknown byte fields")
                if pending is None:
                    if event["offset"] != 0:
                        raise ValueError("decoded response starts after a byte gap")
                    pending = {"role": role, "index": index, "bytes": bytearray()}
                    state["decoded_pending"] = pending
                decoded = base64.b64decode(event["base64"], validate=True)
                if (pending["role"] != role or pending["index"] != index
                        or type(event["offset"]) is not int or event["offset"] != len(pending["bytes"])
                        or not decoded or base64.b64encode(decoded).decode() != event["base64"]):
                    raise ValueError("decoded response has an invalid or missing byte chunk")
                pending["bytes"].extend(decoded)
            else:
                if set(event) != {"kind", "role", "index", "body_bytes", "body_sha256"}:
                    raise ValueError("decoded response has unknown completion fields")
                if (pending is None or pending["role"] != role or pending["index"] != index
                        or event["body_bytes"] != len(pending["bytes"])
                        or event["body_sha256"] != hashlib.sha256(pending["bytes"]).hexdigest()):
                    raise ValueError("decoded response end differs from its exact bytes")
                def unique(pairs):
                    result = {}
                    for key, value in pairs:
                        if key in result:
                            raise ValueError("decoded response repeats a JSON key")
                        result[key] = value
                    return result
                decoded = json.loads(bytes(pending["bytes"]).decode("utf-8"), object_pairs_hook=unique)
                if (not isinstance(decoded, dict)
                        or set(decoded) != {"response", "incomplete_tool_calls", "tool_call_preparations"}
                        or not isinstance(decoded["response"], dict)
                        or not isinstance(decoded["incomplete_tool_calls"], list)
                        or not isinstance(decoded["tool_call_preparations"], list)):
                    raise ValueError("decoded response has an unknown value shape")
                if role == "utility":
                    state["decoded_outputs"].append(decoded)
                else:
                    state["decoded_failure"] = decoded
                state["decoded_pending"] = None
        elif kind == "outcome":
            if event["status"] == "completed" and state["decoded_failure"] is not None:
                raise ValueError("completed response contains a failed decode observation")
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
            if (state["decoded_pending"] is not None
                    or len(state["decoded_outputs"]) != (progress[1] if state["owner"]["kind"] == "utility" else 0)):
                raise ValueError("response outcome omits decoded outputs; inspect the original stream")
            if state["owner"]["kind"] == "physical_utility" and (usage is not None or progress[1] != 0
                    or (event["status"] == "completed" and progress[0] != 1)):
                raise ValueError("physical utility processing claims Chat usage or decoded output")
            state["outcome"] = event["status"]
            state["pipeline_outputs"] = progress[1]
        elif kind == "delivery":
            delivered = event.get("outputs_delivered")
            if (set(event) != {"kind", "outputs_delivered"}
                    or state["owner"]["kind"] not in ("utility", "physical_utility")
                    or type(delivered) is not int or not 0 <= delivered <= state["pipeline_outputs"]
                    or (state["owner"]["kind"] == "physical_utility" and delivered != 0)):
                raise ValueError("utility delivery has no matching physical output prefix")
            state["closed"] = True
        elif kind == "history":
            if set(event) != {"kind", "disposition"} or state["owner"]["kind"] != "chat" or event["disposition"] not in ("accepted", "abandoned") or (event["disposition"] == "accepted" and state["outcome"] != "completed"):
                raise ValueError("chat history decision has no valid processing owner")
            state["closed"] = True
        else:
            raise ValueError("unknown response event; use the matching harness")
        state["sequence"] += 1
    if any(not state["closed"] for state in states.values()):
        raise ValueError("response completion is missing; inspect the original stream")
    require_utility_completions(events, states)
    return responses


def require_utility_completions(events: list[dict], states: dict[str, dict]) -> None:
    """Bind every utility result to all attempts and the final observed provider bytes."""
    if not any(envelope.get("type") in ("model_utility_request", "model_utility_completion")
               for envelope in events):
        return
    operations: dict[str, dict] = {}
    completed: set[str] = set()
    journal = events[0]["request_evidence_origin"]["journal_id"]
    fields = ("journal_id", "operation_id", "kv_scope", "kind", "requested_model",
              "requested_input_count", "expected_max_model_len")
    for envelope in events:
        if envelope.get("type") == "model_utility_request":
            request = envelope["utility_request"]
            operation_id = request["operation_id"]
            if operation_id in completed:
                raise ValueError("physical utility request follows its operation completion")
            identity = {field: request[field] for field in fields}
            operation = operations.setdefault(operation_id, {"identity": identity, "ids": []})
            if operation["identity"] != identity:
                raise ValueError("physical utility retry changes operation identity")
            operation["ids"].append(request["request_id"])
        elif envelope.get("type") == "model_utility_completion":
            completion = envelope["utility_completion"]
            operation_id = completion["operation_id"]
            if operation_id in completed or completion["journal_id"] != journal:
                raise ValueError("physical utility operation repeats its completion")
            operation = operations.get(operation_id)
            request_ids = completion["request_ids"]
            result = completion["result"]
            error = completion["error"]
            if not ((result is None and isinstance(error, str) and error)
                    or (isinstance(result, dict) and error is None and request_ids)):
                raise ValueError("physical utility completion has no unique result or failure")
            if operation is None:
                if request_ids or result is not None:
                    raise ValueError("physical utility completion names unrecorded attempts")
                completed.add(operation_id)
                continue
            if ({field: completion[field] for field in fields} != operation["identity"]
                    or request_ids != operation["ids"]
                    or any(request_id not in states or not states[request_id]["closed"]
                           for request_id in request_ids)
                    or any(states[request_id]["outcome"] == "completed" for request_id in request_ids[:-1])):
                raise ValueError("physical utility completion omits or misattributes a retry")
            if result is not None:
                final = states[request_ids[-1]]
                actual = final["actual"]
                media_type = (actual.get("response_content_type") or "").split(";", 1)[0].strip().lower()
                if (final["outcome"] != "completed" or final["termination"] not in ("eof", "cancelled")
                        or not 200 <= actual["response_status"] < 300
                        or not (media_type == "application/json" or media_type.endswith("+json"))):
                    raise ValueError("physical utility result has no complete JSON response")
                try:
                    physical = json.loads(bytes(final["bytes"]).decode("utf-8-sig"))
                except (UnicodeDecodeError, ValueError) as cause:
                    raise ValueError("physical utility result has no UTF-8 JSON response") from cause
                if not isinstance(physical, dict):
                    raise ValueError("physical utility result has no JSON object response")
                if completion["kind"] == "embedding":
                    data = physical.get("data")
                    vectors = result.get("vectors")
                    if (result.get("kind") != "embedding" or not isinstance(data, list)
                            or not isinstance(vectors, list)
                            or len(data) != len(vectors) or len(data) != completion["requested_input_count"]):
                        raise ValueError("embedding result does not match separate provider inputs")
                    try:
                        ordered = sorted(data, key=lambda item: item["index"])
                        for index, item in enumerate(ordered):
                            encoded = item["embedding"]
                            raw = base64.b64decode(encoded, validate=True)
                            if (type(item["index"]) is not int or item["index"] != index
                                    or not raw or len(raw) % 4 or base64.b64encode(raw).decode() != encoded):
                                raise ValueError("embedding response has invalid ordered float32 bytes")
                            decoded = [value[0] for value in struct.iter_unpack("<f", raw)]
                            if not all(math.isfinite(value) for value in decoded) or decoded != vectors[index]:
                                raise ValueError("embedding result differs from provider float32 bytes")
                    except (KeyError, TypeError, IndexError) as cause:
                        raise ValueError("embedding response has invalid vector fields") from cause
                else:
                    count = physical.get("count")
                    maximum = physical.get("max_model_len")
                    if (result.get("kind") != "token_count" or type(count) is not int
                            or type(maximum) is not int or not 0 <= count <= 2**53 - 1
                            or maximum != completion["expected_max_model_len"]
                            or result.get("total_tokens") != count or result.get("max_model_len") != maximum):
                        raise ValueError("tokenizer result differs from provider bytes or selected context limit")
            del operations[operation_id]
            completed.add(operation_id)
    if operations:
        raise ValueError("physical utility operation has no completion; retain the complete stream")
