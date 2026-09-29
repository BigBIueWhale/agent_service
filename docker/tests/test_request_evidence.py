"""Source-only checks for the shared fake-provider request verifier."""
import copy
import hashlib
import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
from request_evidence import require_request_evidence


class RequestEvidenceTests(unittest.TestCase):
    def check(self, events, bodies):
        receipts = [{"path": "/v1/chat/completions", "raw_body": body} for body in bodies]
        return require_request_evidence(events, receipts)

    def fixture(self):
        messages = [{"role": "user", "content": "שלום 🌈 \n \""}]
        serialize = lambda value: json.dumps(value, ensure_ascii=False, separators=(",", ":"))
        first = serialize({"messages": messages, "model": "fixture-model", "stream": False, "kv_scope": "root"})
        added = {"role": "user", "content": "new reminder"}
        second = serialize({"messages": messages + [added], "model": "fixture-model", "stream": False, "kv_scope": "root"})
        bodies = [first, second]
        origin = {"journal_id": "journal", "first_sequence": 3}
        events = [{"type": "system", "subtype": "stream_start", "request_evidence_origin": origin}]
        for sequence, body in enumerate(bodies, 3):
            representation = {"kind": "full", "json": body} if sequence == 3 else {
                "kind": "delta", "base_request_id": "r3", "retain_messages": 1,
                "added_messages": [serialize(added)], "prefix": '{"messages":[',
                "suffix": '],"model":"fixture-model","stream":false,"kv_scope":"root"}',
            }
            events.append({"type": "model_request", "request": {
                "journal_id": "journal", "sequence": sequence,
                "request_id": f"r{sequence}", "kv_scope": "root", "segment_id": "s",
                "prompt_id": "prompt", "owner": {"kind": "utility", "operation_id": "utility-operation"}, "body": representation,
                "decode_policy": {"mode": "nonstream", "model": "fixture-model"},
                "body_bytes": len(body.encode()), "body_sha256": hashlib.sha256(body.encode()).hexdigest(),
            }})
        events.append({"type": "result", "request_evidence": {**origin, "request_count": 2}})
        return events, bodies

    def test_replays_exact_received_bytes_with_unicode_and_delta(self):
        events, bodies = self.fixture()
        self.assertEqual(len(self.check(events, bodies)), 2)

    def test_full_body_requires_a_new_segment_for_its_scope(self):
        events, bodies = self.fixture()
        second = events[2]["request"]
        second["body"] = {"kind": "full", "json": bodies[1]}
        with self.assertRaisesRegex(ValueError, "full request body repeats an active invocation segment"):
            self.check(events, bodies)
        second["segment_id"] = "next-segment"
        self.assertEqual(len(self.check(events, bodies)), 2)

    def test_selected_decoder_uses_the_dispatched_model(self):
        for mode, model in (("nonstream", "another-model"), ("unknown", "fixture-model")):
            with self.subTest(mode=mode, model=model):
                events, bodies = self.fixture()
                events[1]["request"]["decode_policy"].update(mode=mode, model=model)
                with self.assertRaisesRegex(ValueError, "selected decoder contradicts the dispatched request"):
                    self.check(events, bodies)

    def test_zero_retained_messages_replace_or_remove_the_whole_message_list(self):
        for messages in ([], [{"role": "user", "content": "replacement שלום 🧪\n"}]):
            with self.subTest(messages=messages):
                events, bodies = self.fixture()
                serialize = lambda value: json.dumps(value, ensure_ascii=False, separators=(",", ":"))
                bodies[1] = serialize({"messages": messages, "model": "fixture-model", "stream": False, "kv_scope": "root"})
                request = events[2]["request"]
                request["body"].update(retain_messages=0, added_messages=[serialize(message) for message in messages])
                request.update(body_bytes=len(bodies[1].encode()), body_sha256=hashlib.sha256(bodies[1].encode()).hexdigest())
                self.assertEqual(len(self.check(events, bodies)), 2)
                for defect in ("base", "segment", "scope"):
                    broken = copy.deepcopy(events)
                    if defect == "base": broken[2]["request"]["body"]["base_request_id"] = "unissued"
                    if defect == "segment": broken[2]["request"]["segment_id"] = "other"
                    if defect == "scope": broken[2]["request"]["kv_scope"] = "child"
                    with self.assertRaisesRegex(ValueError, "delta"):
                        self.check(broken, bodies)

    def test_refuses_missing_changed_or_foreign_evidence(self):
        for defect in ["omission", "hash", "bytes", "origin", "segment", "kind", "header"]:
            with self.subTest(defect=defect):
                events, bodies = self.fixture()
                events = copy.deepcopy(events)
                if defect == "omission":
                    del events[1]
                elif defect == "header":
                    events[0]["subtype"] = "init"
                elif defect == "hash":
                    events[1]["request"]["body_sha256"] = "0" * 64
                elif defect == "bytes":
                    bodies[0] += " "
                elif defect == "origin":
                    events[0]["request_evidence_origin"]["first_sequence"] = 4
                elif defect == "segment":
                    events[2]["request"]["segment_id"] = "other"
                elif defect == "kind":
                    events[2]["request"]["body"]["kind"] = "unknown"
                with self.assertRaises(ValueError):
                    self.check(events, bodies)


class PhysicalUtilityEvidenceTests(unittest.TestCase):
    def fixture(self):
        import base64
        body_value = {"model": "fixture-model", "prompt": "hello", "add_special_tokens": False}
        body = json.dumps(body_value, separators=(",", ":"))
        origin = {"journal_id": "journal", "first_sequence": 1}
        events = [{"type": "system", "subtype": "stream_start", "request_evidence_origin": origin}]
        receipts = []
        for index, (status, raw) in enumerate(((500, b""), (200, b'{"count":5,"max_model_len":100}')), 1):
            request_id = f"tokenize-{index}"
            request = {"journal_id": "journal", "sequence": index, "request_id": request_id,
                       "operation_id": "count-text", "kv_scope": "session", "kind": "tokenize_text",
                       "requested_model": "fixture-model", "requested_input_count": None,
                       "expected_max_model_len": 100, "request_url": "http://fixture.invalid/tokenize",
                       "body_json": body, "body_bytes": len(body.encode()),
                       "body_sha256": hashlib.sha256(body.encode()).hexdigest()}
            events.append({"type": "model_utility_request", "utility_request": request})
            receipt = {"path": "/tokenize", "raw_body": body, "body": body_value,
                       "response_status": status, "response_content_type": "application/json",
                       "response_chunks": [base64.b64encode(raw).decode()] if raw else []}
            receipts.append(receipt)
            response = lambda sequence, event: {"type": "model_response", "response": {
                "journal_id": "journal", "request_id": request_id, "sequence": sequence, "event": event}}
            events.append(response(1, {"kind": "http", "status": status,
                                       "content_type": "application/json"}))
            next_sequence = 2
            if raw:
                events.append(response(next_sequence, {"kind": "body", "offset": 0,
                                                       "base64": base64.b64encode(raw).decode()}))
                next_sequence += 1
            events.append(response(next_sequence, {"kind": "end", "termination": "eof",
                                                   "body_bytes": len(raw),
                                                   "body_sha256": hashlib.sha256(raw).hexdigest(),
                                                   "error": None}))
            events.append(response(next_sequence + 1, {"kind": "outcome",
                                                       "status": "completed" if raw else "failed",
                                                       "error": None if raw else "HTTP 500",
                                                       "served_usage": None,
                                                       "sdk_values_seen": 1 if raw else 0,
                                                       "pipeline_outputs_delivered": 0}))
            events.append(response(next_sequence + 2, {"kind": "delivery", "outputs_delivered": 0}))
        events.append({"type": "model_utility_completion", "utility_completion": {
            "journal_id": "journal", "operation_id": "count-text", "kv_scope": "session",
            "kind": "tokenize_text", "requested_model": "fixture-model",
            "requested_input_count": None, "expected_max_model_len": 100,
            "request_ids": ["tokenize-1", "tokenize-2"],
            "result": {"kind": "token_count", "total_tokens": 5, "max_model_len": 100},
            "error": None}})
        events.append({"type": "result", "request_evidence": {**origin,
                                                                   "request_count": 2,
                                                                   "open_response_ids": []}})
        return events, receipts

    def test_retry_request_and_response_bytes_are_all_accounted(self):
        from request_evidence import require_response_evidence
        events, receipts = self.fixture()
        self.assertEqual(require_request_evidence(events, receipts), [])
        self.assertEqual(require_response_evidence(events, receipts), [])
        omitted = copy.deepcopy(events)
        del omitted[1]
        with self.assertRaises(ValueError):
            require_request_evidence(omitted, receipts)
        changed = copy.deepcopy(receipts)
        changed[1]["raw_body"] += " "
        with self.assertRaisesRegex(ValueError, "utility request differs"):
            require_request_evidence(events, changed)
        missing_bytes = copy.deepcopy(events)
        del missing_bytes[8]
        with self.assertRaises(ValueError):
            require_response_evidence(missing_bytes, receipts)
        false_output = copy.deepcopy(events)
        outcome = next(event["response"]["event"] for event in false_output
                       if event.get("type") == "model_response"
                       and event["response"]["request_id"] == "tokenize-2"
                       and event["response"]["event"]["kind"] == "outcome")
        outcome["pipeline_outputs_delivered"] = 1
        with self.assertRaises(ValueError):
            require_response_evidence(false_output, receipts)
        missing_completion = copy.deepcopy(events)
        del missing_completion[-2]
        with self.assertRaisesRegex(ValueError, "no completion"):
            require_response_evidence(missing_completion, receipts)
        wrong_result = copy.deepcopy(events)
        wrong_result[-2]["utility_completion"]["result"]["total_tokens"] = 6
        with self.assertRaisesRegex(ValueError, "differs from provider bytes"):
            require_response_evidence(wrong_result, receipts)

    def test_chat_and_utility_requests_share_one_physical_sequence(self):
        events, receipts = self.fixture()
        chat_events, bodies = RequestEvidenceTests().fixture()
        chat = copy.deepcopy(chat_events[1])
        chat["request"].update(journal_id="journal", sequence=3)
        events.insert(-1, chat)
        events[-1]["request_evidence"]["request_count"] = 3
        receipts.append({"path": "/v1/chat/completions", "raw_body": bodies[0]})
        self.assertEqual(require_request_evidence(events, receipts), [chat["request"]])
        repeated = copy.deepcopy(events)
        repeated[-2]["request"]["sequence"] = 2
        with self.assertRaisesRegex(ValueError, "identity or sequence"):
            require_request_evidence(repeated, receipts)


class ResponseEvidenceTests(unittest.TestCase):
    def fixture(self, termination="eof"):
        import base64
        from request_evidence import require_response_evidence
        body = b'data: {broken \xff\n\n'
        request = {"journal_id": "j", "request_id": "r"}
        envelope = lambda sequence, event: {"type": "model_response", "response": {**request, "sequence": sequence, "event": event}}
        events = [
            {"type": "model_request", "request": {**request, "owner": {"kind": "utility", "operation_id": "utility-operation"}}},
            envelope(1, {"kind": "http", "status": 200, "content_type": "text/event-stream"}),
            envelope(2, {"kind": "body", "offset": 0, "base64": base64.b64encode(body).decode()}),
            envelope(3, {"kind": "end", "termination": termination, "body_bytes": len(body), "body_sha256": hashlib.sha256(body).hexdigest(), "error": None}),
            envelope(4, {"kind": "outcome", "served_usage": None, "status": "failed", "error": "malformed JSON", "sdk_values_seen": 0, "pipeline_outputs_delivered": 0}),
            envelope(5, {"kind": "delivery", "outputs_delivered": 0}),
            {"type": "result", "request_evidence": {"open_response_ids": []}},
        ]
        served = [{"response_status": 200, "response_content_type": "text/event-stream", "response_chunks": [base64.b64encode(body + (b"unobserved" if termination == "cancelled" else b"")).decode()]}]
        return events, served

    def test_complete_wire_and_explicit_cancelled_prefix(self):
        from request_evidence import require_response_evidence
        for termination in ["eof", "cancelled"]:
            events, served = self.fixture(termination)
            self.assertEqual(len(require_response_evidence(events, served)), 5)

    def test_utility_output_requires_complete_decoded_bytes(self):
        import base64
        from request_evidence import require_response_evidence
        events, served = self.fixture()
        provider = b'data: {"choices":[{"delta":{"content":"x"},"finish_reason":"stop"}]}\n\n'
        events[2]["response"]["event"]["base64"] = base64.b64encode(provider).decode()
        events[3]["response"]["event"].update(
            body_bytes=len(provider), body_sha256=hashlib.sha256(provider).hexdigest())
        served[0]["response_chunks"] = [base64.b64encode(provider).decode()]
        decoded = json.dumps({"response": {"candidates": [{
                                  "content": {"parts": [{"text": "x"}], "role": "model"},
                                  "index": 0, "safetyRatings": [], "finishReason": "STOP"}]},
                              "incomplete_tool_calls": [], "tool_call_preparations": []},
                             separators=(",", ":")).encode()
        body = {"kind": "decoded_body", "role": "utility", "index": 0,
                "offset": 0, "base64": base64.b64encode(decoded).decode()}
        end = {"kind": "decoded_end", "role": "utility", "index": 0,
               "body_bytes": len(decoded), "body_sha256": hashlib.sha256(decoded).hexdigest()}
        for offset, event in enumerate((body, end)):
            row = copy.deepcopy(events[4])
            row["response"]["sequence"] = 4 + offset
            row["response"]["event"] = event
            events.insert(4 + offset, row)
        events[6]["response"]["sequence"] = 6
        events[6]["response"]["event"].update(status="completed", error=None, sdk_values_seen=1)
        events[6]["response"]["event"]["pipeline_outputs_delivered"] = 1
        events[7]["response"]["sequence"] = 7
        events[7]["response"]["event"]["outputs_delivered"] = 1
        self.assertEqual(len(require_response_evidence(events, served)), 7)
        forged = copy.deepcopy(events)
        for offset, original in enumerate((body, end)):
            row = copy.deepcopy(forged[6])
            row["response"]["sequence"] = 6 + offset
            row["response"]["event"] = {**original, "role": "failure"}
            forged.insert(6 + offset, row)
        forged[8]["response"]["sequence"] = 8
        forged[9]["response"]["sequence"] = 9
        with self.assertRaisesRegex(ValueError, "completed response contains a failed decode"):
            require_response_evidence(forged, served)
        omitted = copy.deepcopy(events)
        del omitted[4:6]
        for index in (4, 5):
            omitted[index]["response"]["sequence"] -= 2
        with self.assertRaisesRegex(ValueError, "omits decoded outputs"):
            require_response_evidence(omitted, served)
        events[5]["response"]["event"]["body_sha256"] = "0" * 64
        with self.assertRaisesRegex(ValueError, "decoded response end differs"):
            require_response_evidence(events, served)

    def test_processing_progress_requires_an_observed_sdk_value(self):
        from request_evidence import require_response_evidence
        for defect in ("missing", "unseen_sdk", "unsafe_count"):
            with self.subTest(defect=defect):
                events, served = self.fixture()
                outcome = events[-3]["response"]["event"]
                if defect == "missing":
                    del outcome["sdk_values_seen"]
                elif defect == "unseen_sdk":
                    outcome["pipeline_outputs_delivered"] = 1
                else:
                    outcome["sdk_values_seen"] = 2**53
                with self.assertRaises(ValueError):
                    require_response_evidence(events, served)

    def test_processing_outcome_matches_the_observed_transport(self):
        from request_evidence import require_response_evidence
        for http_status in (200, 299, 300, 503):
            for termination in ("eof", "cancelled", "failed"):
                for outcome in ("completed", "failed", "cancelled"):
                    with self.subTest(http_status=http_status, termination=termination, outcome=outcome):
                        events, served = self.fixture(termination)
                        events[-4]["response"]["event"]["error"] = "transport failed" if termination == "failed" else None
                        events[1]["response"]["event"]["status"] = http_status
                        served[0]["response_status"] = http_status
                        events[-3]["response"]["event"].update(status=outcome, error="processing failed" if outcome == "failed" else None)
                        if outcome == "completed" and (not 200 <= http_status < 300 or termination == "failed"):
                            with self.assertRaisesRegex(ValueError, "successful HTTP transport"):
                                require_response_evidence(events, served)
                        else:
                            self.assertEqual(len(require_response_evidence(events, served)), 5)

    def test_physical_usage_is_explicit_and_valid(self):
        from request_evidence import require_response_evidence
        for status in ("completed", "failed", "cancelled"):
            for defect in ("valid", "null", "zero", "missing", "extra", "fraction", "negative", "unsafe", "boolean", "total", "cached", "thoughts", "provider"):
                with self.subTest(status=status, defect=defect):
                    events, served = self.fixture()
                    usage = dict(promptTokenCount=5, candidatesTokenCount=3, totalTokenCount=8,
                                 cachedContentTokenCount=1, thoughtsTokenCount=2)
                    event = events[-3]["response"]["event"]
                    event.update(status=status, error="processing failed" if status == "failed" else None, served_usage=usage)
                    if defect == "null": event["served_usage"] = None
                    if defect == "zero": event["served_usage"] = dict.fromkeys(usage, 0)
                    if defect == "missing": del event["served_usage"]
                    if defect == "extra": usage["estimated"] = True
                    if defect == "fraction": usage["thoughtsTokenCount"] = 0.5
                    if defect == "negative": usage["thoughtsTokenCount"] = -1
                    if defect == "unsafe": usage["totalTokenCount"] = 2**53
                    if defect == "boolean": usage["thoughtsTokenCount"] = False
                    if defect == "total": usage["totalTokenCount"] = 9
                    if defect == "cached": usage["cachedContentTokenCount"] = 6
                    if defect == "thoughts": usage["thoughtsTokenCount"] = 4
                    if defect == "provider": served[0]["served_usage"] = dict.fromkeys(usage, 0)
                    valid = defect in ("valid", "null", "zero") or (defect == "provider" and status != "completed")
                    if valid:
                        self.assertEqual(len(require_response_evidence(events, served)), 5)
                    else:
                        with self.assertRaises(ValueError): require_response_evidence(events, served)

    def test_missing_or_changed_wire_is_refused(self):
        from request_evidence import require_response_evidence
        for defect in ["missing", "missing_delivery", "excess_delivery", "unknown", "gap", "hash", "size", "provider", "open", "headers", "early", "outcome_status", "outcome_error", "missing_end"]:
            with self.subTest(defect=defect):
                events, served = self.fixture()
                if defect == "missing": del events[-3]
                if defect == "missing_delivery": del events[-2]
                if defect == "excess_delivery": events[-2]["response"]["event"]["outputs_delivered"] = 1
                if defect == "missing_end": del events[-4]
                if defect == "outcome_status": events[-3]["response"]["event"]["status"] = "unknown"
                if defect == "outcome_error": events[-3]["response"]["event"]["error"] = None
                if defect == "early": events[0], events[1] = events[1], events[0]
                if defect == "unknown": events[2]["response"]["event"]["kind"] = "unknown"
                if defect == "gap": events[2]["response"]["event"]["offset"] = 1
                if defect == "hash": events[-4]["response"]["event"]["body_sha256"] = "0" * 64
                if defect == "size": events[-4]["response"]["event"]["body_bytes"] = 0
                if defect == "provider": served[0]["response_chunks"] = ["e30="]
                if defect == "open": events[-1]["request_evidence"]["open_response_ids"] = ["r"]
                if defect == "headers": served[0]["response_status"] = 500
                with self.assertRaises(ValueError): require_response_evidence(events, served)

    def test_chat_processing_requires_its_history_decision(self):
        from request_evidence import require_response_evidence
        events, served = self.fixture()
        events[0]["request"]["owner"] = {"kind": "chat", "attempt_id": "attempt"}
        del events[-2]
        with self.assertRaises(ValueError): require_response_evidence(events, served)
        history = {"type": "model_response", "response": {"journal_id": "j", "request_id": "r", "sequence": 5, "event": {"kind": "history", "disposition": "abandoned"}}}
        events.insert(-1, history)
        self.assertEqual(len(require_response_evidence(events, served)), 5)
        history["response"]["event"]["disposition"] = "accepted"
        with self.assertRaises(ValueError): require_response_evidence(events, served)
        events[-3]["response"]["event"] = {"kind": "outcome", "served_usage": None, "status": "completed", "error": None, "sdk_values_seen": 0, "pipeline_outputs_delivered": 0}
        self.assertEqual(len(require_response_evidence(events, served)), 5)

    def test_output_cannot_claim_an_unissued_or_foreign_attempt(self):
        from request_evidence import require_output_ownership
        events, _ = self.fixture()
        events[0]["request"].update(owner={"kind": "chat", "attempt_id": "attempt"}, kv_scope="session")
        assistant = {"type": "assistant", "session_id": "session", "parent_tool_use_id": None, "origin": {"kind": "model", "attempt_id": "attempt", "kv_scope": "session"}}
        events.insert(1, assistant)
        require_output_ownership(events)
        for defect in ("unknown", "foreign"):
            broken = copy.deepcopy(events)
            if defect == "unknown": broken[1]["origin"]["attempt_id"] = "unissued"
            else: broken[1]["parent_tool_use_id"] = "child"
            with self.assertRaises(ValueError): require_output_ownership(broken)


if __name__ == "__main__":
    unittest.main()
