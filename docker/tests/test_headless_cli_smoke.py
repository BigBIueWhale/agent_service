#!/usr/bin/env python3
"""Negative controls for the image's real-entry smoke gate."""
import signal
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "docker/scripts"))
from check_headless_cli import SmokeFailure, check


class HeadlessSmokeTests(unittest.TestCase):
    def run_entry(self, source):
        with tempfile.TemporaryDirectory(prefix="headless-gate-negative-") as temporary:
            entry = Path(temporary) / "entry.mjs"
            entry.write_text(source)
            return check(entry, ROOT / "docker/config/settings.json", ROOT / "src/bin/agent_exec.rs")

    def test_rejects_zero_exit_without_events(self):
        with self.assertRaisesRegex(SmokeFailure, "CLI emitted no events"):
            self.run_entry("process.stdin.resume();\n")

    def test_rejects_nonzero_exit_even_with_output(self):
        with self.assertRaisesRegex(SmokeFailure, "CLI exited 7"):
            self.run_entry('console.log(\'{"type":"result","subtype":"success"}\'); process.exit(7);\n')

    def test_joins_child_and_closes_pipes_when_communication_fails(self):
        children = []
        failure = RuntimeError("injected communication failure")

        def refuse_communication(process, *_args, **_kwargs):
            children.append(process)
            raise failure

        with patch.object(subprocess.Popen, "communicate", refuse_communication):
            with self.assertRaises(RuntimeError) as caught:
                self.run_entry("setInterval(() => {}, 1000);\n")
        self.assertIs(caught.exception, failure)
        self.assertEqual(len(children), 1)
        child = children[0]
        self.assertEqual(child.returncode, -signal.SIGKILL)
        self.assertTrue(all(pipe.closed for pipe in (child.stdin, child.stdout, child.stderr)))

    def test_joins_child_after_interrupted_communication(self):
        children = []
        failure = KeyboardInterrupt()

        def interrupt_communication(process, *_args, **_kwargs):
            children.append(process)
            # communicate() has consumed Popen's interrupt wait allowance.
            process._sigint_wait_secs = 0
            raise failure

        with patch.object(subprocess.Popen, "communicate", interrupt_communication):
            with self.assertRaises(KeyboardInterrupt) as caught:
                self.run_entry("setInterval(() => {}, 1000);\n")
        self.assertIs(caught.exception, failure)
        self.assertEqual(len(children), 1)
        self.assertEqual(children[0].returncode, -signal.SIGKILL)
        self.assertTrue(all(pipe.closed for pipe in (children[0].stdin, children[0].stdout, children[0].stderr)))

    def test_joins_child_and_reports_timeout(self):
        children = []

        def expire_communication(process, *_args, **_kwargs):
            children.append(process)
            raise subprocess.TimeoutExpired(process.args, 45, output=b"partial events")

        with patch.object(subprocess.Popen, "communicate", expire_communication):
            with self.assertRaisesRegex(SmokeFailure, "CLI exceeded 45 seconds"):
                self.run_entry("setInterval(() => {}, 1000);\n")
        self.assertEqual(len(children), 1)
        self.assertEqual(children[0].returncode, -signal.SIGKILL)
        self.assertTrue(all(pipe.closed for pipe in (children[0].stdin, children[0].stdout, children[0].stderr)))


if __name__ == "__main__":
    unittest.main()
