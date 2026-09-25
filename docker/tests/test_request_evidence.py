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
    def fixture(self):
        messages = [{"role": "user", "content": "שלום 🌈 \n \""}]
        serialize = lambda value: json.dumps(value, ensure_ascii=False, separators=(",", ":"))
        first = serialize({"messages": messages, "kv_scope": "root"})
        added = {"role": "user", "content": "new reminder"}
        second = serialize({"messages": messages + [added], "kv_scope": "root"})
        bodies = [first, second]
        origin = {"journal_id": "journal", "first_sequence": 3}
        events = [{"type": "system", "subtype": "init", "request_evidence_origin": origin}]
        for sequence, body in enumerate(bodies, 3):
            representation = {"kind": "full", "json": body} if sequence == 3 else {
                "kind": "delta", "base_request_id": "r3", "retain_messages": 1,
                "added_messages": [serialize(added)], "prefix": '{"messages":[',
                "suffix": '],"kv_scope":"root"}',
            }
            events.append({"type": "model_request", "request": {
                "journal_id": "journal", "sequence": sequence,
                "request_id": f"r{sequence}", "kv_scope": "root", "segment_id": "s",
                "prompt_id": "prompt", "body": representation,
                "body_bytes": len(body.encode()), "body_sha256": hashlib.sha256(body.encode()).hexdigest(),
            }})
        events.append({"type": "result", "request_evidence": {**origin, "request_count": 2}})
        return events, bodies

    def test_replays_exact_received_bytes_with_unicode_and_delta(self):
        events, bodies = self.fixture()
        self.assertEqual(len(require_request_evidence(events, bodies)), 2)

    def test_refuses_missing_changed_or_foreign_evidence(self):
        for defect in ["omission", "hash", "bytes", "origin", "segment", "kind"]:
            with self.subTest(defect=defect):
                events, bodies = self.fixture()
                events = copy.deepcopy(events)
                if defect == "omission":
                    del events[1]
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
                    require_request_evidence(events, bodies)


class ResponseEvidenceTests(unittest.TestCase):
    def fixture(self, termination="eof"):
        import base64
        from request_evidence import require_response_evidence
        body = b'data: {broken \xff\n\n'
        request = {"journal_id": "j", "request_id": "r"}
        envelope = lambda sequence, event: {"type": "model_response", "response": {**request, "sequence": sequence, "event": event}}
        events = [
            {"type": "model_request", "request": request},
            envelope(1, {"kind": "http", "status": 200, "content_type": "text/event-stream"}),
            envelope(2, {"kind": "body", "offset": 0, "base64": base64.b64encode(body).decode()}),
            envelope(3, {"kind": "end", "termination": termination, "body_bytes": len(body), "body_sha256": hashlib.sha256(body).hexdigest(), "error": None}),
            envelope(4, {"kind": "outcome", "status": "failed", "error": "malformed JSON"}),
            {"type": "result", "request_evidence": {"open_response_ids": []}},
        ]
        served = [{"response_status": 200, "response_content_type": "text/event-stream", "response_chunks": [base64.b64encode(body + (b"unobserved" if termination == "cancelled" else b"")).decode()]}]
        return events, served

    def test_complete_wire_and_explicit_cancelled_prefix(self):
        from request_evidence import require_response_evidence
        for termination in ["eof", "cancelled"]:
            events, served = self.fixture(termination)
            self.assertEqual(len(require_response_evidence(events, served)), 4)

    def test_missing_or_changed_wire_is_refused(self):
        from request_evidence import require_response_evidence
        for defect in ["missing", "unknown", "gap", "hash", "size", "provider", "open", "headers", "early", "outcome_status", "outcome_error", "missing_end"]:
            with self.subTest(defect=defect):
                events, served = self.fixture()
                if defect == "missing": del events[-2]
                if defect == "missing_end": del events[-3]
                if defect == "outcome_status": events[-2]["response"]["event"]["status"] = "unknown"
                if defect == "outcome_error": events[-2]["response"]["event"]["error"] = None
                if defect == "early": events[0], events[1] = events[1], events[0]
                if defect == "unknown": events[2]["response"]["event"]["kind"] = "unknown"
                if defect == "gap": events[2]["response"]["event"]["offset"] = 1
                if defect == "hash": events[-3]["response"]["event"]["body_sha256"] = "0" * 64
                if defect == "size": events[-3]["response"]["event"]["body_bytes"] = 0
                if defect == "provider": served[0]["response_chunks"] = ["e30="]
                if defect == "open": events[-1]["request_evidence"]["open_response_ids"] = ["r"]
                if defect == "headers": served[0]["response_status"] = 500
                with self.assertRaises(ValueError): require_response_evidence(events, served)


if __name__ == "__main__":
    unittest.main()
