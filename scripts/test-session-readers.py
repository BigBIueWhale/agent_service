#!/usr/bin/env python3
"""Offline public-reader checks; no service, model, build or container is contacted."""
import copy
import json
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OBSERVED = ["observed_usage", "observed_subagent_scope_count", "observed_unaccounted_records"]
EMPTY_USAGE = {
    "requests": 0, "usageReports": 0, "unfinalizedRequests": 0,
    "unreportedUsageRequests": 0, "usage": None,
}
MAIN_USAGE = {
    "requests": 1, "usageReports": 1, "unfinalizedRequests": 0,
    "unreportedUsageRequests": 0, "usage": {
        "promptTokenCount": 12, "candidatesTokenCount": 7,
        "thoughtsTokenCount": 2, "cachedContentTokenCount": 4, "totalTokenCount": 19,
    },
}
INTERNAL_USAGE = {
    "requests": 2, "usageReports": 1, "unfinalizedRequests": 0,
    "unreportedUsageRequests": 1, "usage": {
        "promptTokenCount": 8, "candidatesTokenCount": 5,
        "thoughtsTokenCount": 2, "cachedContentTokenCount": 0, "totalTokenCount": 13,
    },
}
TOTAL_USAGE = {
    "requests": 3, "usageReports": 2, "unfinalizedRequests": 0,
    "unreportedUsageRequests": 1, "usage": {
        "promptTokenCount": 20, "candidatesTokenCount": 12,
        "thoughtsTokenCount": 4, "cachedContentTokenCount": 4, "totalTokenCount": 32,
    },
}


def terminal_body():
    return {
        "status": "ended", "num_turns": None, **dict.fromkeys(OBSERVED), "deliverables": [],
        "terminal": {
            "agent_result": None, "bundle": None, "is_process_error": True,
            "response": "", "raw_session_tree_retained": False, "teardown_diagnostics": [],
        },
    }


def certified_body():
    body = terminal_body()
    body.update(num_turns=2, observed_usage=copy.deepcopy(TOTAL_USAGE),
                observed_subagent_scope_count=0, observed_unaccounted_records=0)
    body["terminal"]["agent_result"] = {
        "num_turns": 2, "main_kv_scope": "main", "usage": copy.deepcopy(TOTAL_USAGE),
        "request_scopes": [
            {"kv_scope": "main", "usage": copy.deepcopy(MAIN_USAGE)},
            {"kv_scope": "internal", "usage": copy.deepcopy(INTERNAL_USAGE)},
        ],
        "subagent_scopes": [], "subagent_scope_count": 0, "subagent_error_count": 0,
        "missing_deliverables": None,
    }
    return body


class SessionReaderTests(unittest.TestCase):
    def validate(self, body, valid):
        result = subprocess.run(
            ["jq", "-e", "-f", str(ROOT / "scripts/session-body.jq")],
            input=json.dumps(body), text=True, capture_output=True, check=False,
        )
        self.assertEqual(result.returncode == 0, valid, result.stderr)
        if valid:
            self.assertEqual(json.loads(result.stdout), body)

    def test_observations_survive_terminal_without_certifying_missing_evidence(self):
        running = {
            "status": "running", "num_turns": None, "terminal": None, "deliverables": [],
            "observed_usage": copy.deepcopy(EMPTY_USAGE),
            "observed_subagent_scope_count": 0, "observed_unaccounted_records": 0,
        }
        self.validate(running, True)
        terminal = terminal_body()
        self.validate(terminal, True)
        partial = copy.deepcopy(terminal)
        partial.update(observed_usage=copy.deepcopy(MAIN_USAGE),
                       observed_subagent_scope_count=0, observed_unaccounted_records=1)
        self.validate(partial, True)
        self.assertIsNone(partial["terminal"]["agent_result"])
        for field in OBSERVED:
            with self.subTest(field=field):
                invalid = copy.deepcopy(terminal)
                invalid[field] = copy.deepcopy(EMPTY_USAGE) if field == "observed_usage" else 0
                self.validate(invalid, False)
                invalid = copy.deepcopy(running)
                invalid.pop(field)
                self.validate(invalid, False)
        invalid = copy.deepcopy(running)
        invalid.pop("num_turns")
        self.validate(invalid, False)
        invalid = copy.deepcopy(running)
        invalid["terminal"] = terminal["terminal"]
        self.validate(invalid, False)
        terminal["terminal"] = None
        self.validate(terminal, False)

    def test_physical_scope_totals_include_internal_requests_without_inventing_children(self):
        body = certified_body()
        self.validate(body, True)
        self.assertNotEqual(body["num_turns"], body["observed_usage"]["requests"])
        for defect in ["missing_scope", "duplicate_scope", "wrong_total", "unaccounted",
                       "wrong_turns", "unknown_turns", "stale_progress", "main_identity"]:
            with self.subTest(defect=defect):
                invalid = copy.deepcopy(body)
                result = invalid["terminal"]["agent_result"]
                if defect == "missing_scope": result["request_scopes"].pop()
                elif defect == "duplicate_scope": result["request_scopes"][1]["kv_scope"] = "main"
                elif defect == "wrong_total": result["usage"]["usage"]["totalTokenCount"] += 1
                elif defect == "unaccounted": invalid["observed_unaccounted_records"] = 1
                elif defect == "wrong_turns": invalid["num_turns"] = 3
                elif defect == "unknown_turns": invalid["num_turns"] = None
                elif defect == "stale_progress": invalid["progress_events"] = [{"counters": {"physical_requests": 4}}]
                elif defect == "main_identity": result["main_kv_scope"] = ""
                self.validate(invalid, False)

    def test_partial_observations_cannot_claim_a_complete_result(self):
        body = certified_body()
        body["observed_unaccounted_records"] = 1
        self.validate(body, False)
        body["terminal"]["agent_result"] = None
        self.validate(body, True)
        body["observed_usage"]["usage"]["thoughtsTokenCount"] = 13
        self.validate(body, False)

    def test_unknown_zero_and_pending_usage_remain_distinct(self):
        for summary in [
            EMPTY_USAGE,
            {"requests": 1, "usageReports": 0, "unfinalizedRequests": 0,
             "unreportedUsageRequests": 1, "usage": None},
            {"requests": 1, "usageReports": 0, "unfinalizedRequests": 1,
             "unreportedUsageRequests": 0, "usage": None},
            {"requests": 1, "usageReports": 1, "unfinalizedRequests": 0,
             "unreportedUsageRequests": 0, "usage": dict.fromkeys(MAIN_USAGE["usage"], 0)},
        ]:
            with self.subTest(summary=summary):
                body = terminal_body()
                body.update(observed_usage=copy.deepcopy(summary),
                            observed_subagent_scope_count=0, observed_unaccounted_records=0)
                self.validate(body, True)
                invalid = copy.deepcopy(body)
                invalid["observed_usage"].pop("usage")
                self.validate(invalid, False)
                invalid = copy.deepcopy(body)
                invalid["observed_usage"]["requests"] += 1
                self.validate(invalid, False)
                invalid = copy.deepcopy(body)
                invalid["observed_usage"]["usage"] = (None if summary["usageReports"] else dict.fromkeys(MAIN_USAGE["usage"], 0))
                self.validate(invalid, False)

    def test_usage_refuses_missing_extra_fractional_negative_and_out_of_range_counts(self):
        for field in MAIN_USAGE["usage"]:
            for value in [None, -1, 0.5, 9007199254740992, "0"]:
                with self.subTest(field=field, value=value):
                    body = certified_body()
                    body["observed_usage"]["usage"][field] = value
                    self.validate(body, False)
        for holder in ["summary", "served", "scope"]:
            body = certified_body()
            if holder == "summary": body["observed_usage"]["unexpected"] = 0
            elif holder == "served": body["observed_usage"]["usage"]["unexpected"] = 0
            else: body["terminal"]["agent_result"]["request_scopes"][0]["unexpected"] = 0
            self.validate(body, False)

    def test_child_terminal_claims_do_not_own_internal_usage(self):
        body = certified_body()
        result = body["terminal"]["agent_result"]
        result["subagent_scopes"] = [{
            "tool_use_id": "issued-call", "tool_name": "Agent", "reported_num_turns": None,
            "is_error": None, "subtype": None, "error_message": None,
        }]
        body["observed_subagent_scope_count"] = result["subagent_scope_count"] = 1
        self.validate(body, True)
        for field in result["subagent_scopes"][0]:
            invalid = copy.deepcopy(body)
            invalid["terminal"]["agent_result"]["subagent_scopes"][0].pop(field)
            self.validate(invalid, False)
        result["subagent_scopes"][0]["is_error"] = True
        self.validate(body, False)
        result["subagent_scopes"][0].update(reported_num_turns=0, subtype="error_during_execution", error_message="stopped")
        result["subagent_error_count"] = 1
        self.validate(body, True)

    def test_no_ending_and_no_bundle_are_distinct_from_zero_artifacts(self):
        body = terminal_body()
        body["status"] = "cancelled"
        body["terminal"]["bundle"] = {
            "sha256": "1" * 64, "compressed_bytes": 100,
            "uncompressed_bytes": 0, "file_count": 0, "artifacts_file_count": 0,
        }
        self.validate(body, True)
        result = subprocess.run(["jq", "-er", ".terminal.bundle.artifacts_file_count"],
                                input=json.dumps(body), text=True, capture_output=True, check=True)
        self.assertEqual(result.stdout.strip(), "0")

    def test_missing_deliverables_are_listed_only_from_the_declared_ones(self):
        body = certified_body()
        body["deliverables"] = ["report.md", "out/data.csv", "notes.txt"]
        self.validate(body, True)
        result = body["terminal"]["agent_result"]
        for missing in (["report.md", "notes.txt"], ["out/data.csv"],
                        ["report.md", "out/data.csv", "notes.txt"]):
            with self.subTest(missing=missing):
                result["missing_deliverables"] = missing
                self.validate(body, True)
        for missing in ([], ["other.md"], ["notes.txt", "report.md"],
                        ["report.md", "report.md"], "report.md", [1]):
            with self.subTest(missing=missing):
                invalid = copy.deepcopy(body)
                invalid["terminal"]["agent_result"]["missing_deliverables"] = missing
                self.validate(invalid, False)
        invalid = copy.deepcopy(body)
        invalid["terminal"]["agent_result"].pop("missing_deliverables")
        self.validate(invalid, False)
        # Nothing is missing from a session that declared nothing.
        invalid = copy.deepcopy(body)
        invalid["deliverables"] = []
        invalid["terminal"]["agent_result"]["missing_deliverables"] = ["report.md"]
        self.validate(invalid, False)

    def test_the_accepted_deliverables_are_required_in_every_state(self):
        running = {
            "status": "running", "num_turns": None, "terminal": None,
            "observed_usage": copy.deepcopy(EMPTY_USAGE),
            "observed_subagent_scope_count": 0, "observed_unaccounted_records": 0,
        }
        for body in (running, terminal_body()):
            body["deliverables"] = ["report.md"]
            self.validate(body, True)
            for deliverables in (None, "report.md", [""], ["a.md", "a.md"], [1]):
                with self.subTest(status=body["status"], deliverables=deliverables):
                    invalid = copy.deepcopy(body)
                    invalid["deliverables"] = deliverables
                    self.validate(invalid, False)
            invalid = copy.deepcopy(body)
            invalid.pop("deliverables")
            self.validate(invalid, False)

    def test_status_names_the_lifecycle_and_never_a_success(self):
        body = terminal_body()
        body["terminal"]["response"] = "Generation on turn 1 ended as MAX_TOKENS"
        self.validate(body, True)
        body["status"] = "completed"
        self.validate(body, False)


if __name__ == "__main__":
    unittest.main()
