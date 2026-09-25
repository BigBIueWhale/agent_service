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


if __name__ == "__main__":
    unittest.main()
