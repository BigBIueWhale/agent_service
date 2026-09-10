#!/usr/bin/env python3
"""Negative controls for the image's real-entry smoke gate."""
import signal
import json
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
    def test_refuses_missing_malformed_or_mismatched_pairing_before_starting_actors(self):
        for body in [None, b'{', b'null', b'{}', json.dumps({"schema_sha256": "1" * 64}).encode()]:
            with self.subTest(body=body), tempfile.TemporaryDirectory(prefix="headless-gate-pairing-") as temporary:
                entry = Path(temporary) / "entry.mjs"
                if body is not None:
                    (entry.parent / "stream-binding-manifest.json").write_bytes(body)
                with patch("check_headless_cli.contract_identity", return_value="0" * 64), \
                     patch("check_headless_cli.HTTPServer") as server, \
                     patch("check_headless_cli.subprocess.Popen") as child:
                    with self.assertRaisesRegex(SmokeFailure, "before provider admission"):
                        check(entry, ROOT / "docker/config/settings.json", ROOT / "src/bin/agent_exec.rs",
                              Path(temporary) / "unreachable-certifier")
                    server.assert_not_called()
                    child.assert_not_called()

    def run_entry(self, source):
        with tempfile.TemporaryDirectory(prefix="headless-gate-negative-") as temporary:
            entry = Path(temporary) / "entry.mjs"
            entry.write_text(source)
            (entry.parent / "stream-binding-manifest.json").write_text(
                json.dumps({"schema_sha256": "0" * 64}))
            with patch("check_headless_cli.WORKSPACE", Path(temporary)), \
                 patch("check_headless_cli.contract_identity", return_value="0" * 64):
                return check(entry, ROOT / "docker/config/settings.json", ROOT / "src/bin/agent_exec.rs",
                             Path(temporary) / "unreachable-certifier")

    def test_rejects_zero_exit_without_events(self):
        with self.assertRaisesRegex(SmokeFailure, "CLI emitted no events"):
            self.run_entry("process.stdin.resume();\n")

    def test_passes_the_verified_certifier_identity_to_cli_startup(self):
        with self.assertRaisesRegex(SmokeFailure, "CLI emitted no events"):
            self.run_entry("if (process.env.QWEN_STREAM_CONTRACT_SHA256 !== '0'.repeat(64)) process.exit(23); process.stdin.resume();\n")

    def test_fresh_proof_is_absent_from_the_prompt_and_file_name(self):
        with self.assertRaisesRegex(SmokeFailure, "CLI emitted no events"):
            self.run_entry('''import { readFileSync } from 'node:fs';
let prompt = '';
process.stdin.setEncoding('utf8');
process.stdin.on('data', (chunk) => { prompt += chunk; });
process.stdin.on('end', () => {
  const path = /^Read (.+) with read_file/.exec(prompt)?.[1];
  if (!path) process.exit(18);
  const proof = readFileSync(path, 'utf8').trim();
  if (!/^[a-f0-9]{32}$/.test(proof) || prompt.includes(proof) || path.includes(proof)) process.exit(19);
});
''')

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
