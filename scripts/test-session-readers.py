#!/usr/bin/env python3
"""Offline contract checks; no service, model, or container is contacted."""
import copy
import json
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
LIVE = ["observed_output_tokens", "observed_reasoning_tokens",
        "observed_subagent_scope_count", "observed_unaccounted_records"]


class SessionReaderTests(unittest.TestCase):
    def validate(self, body, valid):
        result = subprocess.run(
            ["jq", "-e", "-f", str(ROOT / "scripts/session-body.jq")],
            input=json.dumps(body), text=True, capture_output=True, check=False,
        )
        self.assertEqual(result.returncode == 0, valid, result.stderr)
        if valid:
            self.assertEqual(json.loads(result.stdout), body)

    def test_observation_and_terminal_evidence_are_exclusive(self):
        running = {"status": "running", "terminal": None, **dict.fromkeys(LIVE, 0)}
        self.validate(running, True)
        terminal = {
            "status": "completed", **dict.fromkeys(LIVE),
            "terminal": {"agent_result": None, "bundle": None,
                         "is_process_error": True, "response": "",
                         "raw_session_tree_retained": False, "teardown_diagnostics": []},
        }
        self.validate(terminal, True)
        for field in LIVE:
            invalid = copy.deepcopy(terminal)
            invalid[field] = 0
            self.validate(invalid, False)
            invalid = copy.deepcopy(running)
            invalid.pop(field)
            self.validate(invalid, False)
        invalid = copy.deepcopy(running)
        invalid["terminal"] = terminal["terminal"]
        self.validate(invalid, False)
        terminal["terminal"] = None
        self.validate(terminal, False)

    def test_no_ending_and_no_bundle_are_distinct_from_zero_artifacts(self):
        body = {"status": "cancelled", **dict.fromkeys(LIVE),
                "terminal": {"agent_result": None, "bundle": {
                    "sha256": "1" * 64, "compressed_bytes": 100,
                    "uncompressed_bytes": 0, "file_count": 0, "artifacts_file_count": 0},
                    "is_process_error": False, "response": "",
                    "raw_session_tree_retained": False, "teardown_diagnostics": []}}
        self.validate(body, True)
        result = subprocess.run(["jq", "-er", ".terminal.bundle.artifacts_file_count"],
                                input=json.dumps(body), text=True, capture_output=True, check=True)
        self.assertEqual(result.stdout.strip(), "0")


if __name__ == "__main__":
    unittest.main()
