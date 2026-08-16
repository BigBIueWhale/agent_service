#!/usr/bin/env python3

from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import stat
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "verify_toolchain.py"
SPEC = importlib.util.spec_from_file_location("verify_toolchain", SCRIPT)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class VerifyToolchainTests(unittest.TestCase):
    def fixture(self, root: Path) -> tuple[Path, Path]:
        apt = root / "apt.lock"
        apt.write_text("exact-package=1\n", encoding="utf-8")
        command = root / "probe"
        command.write_text("#!/bin/sh\nprintf 'probe 1\\n'\n", encoding="utf-8")
        command.chmod(0o755)
        manifest = root / "manifest.json"
        payload = {
            "schema_version": 1,
            "apt_lock_sha256": hashlib.sha256(apt.read_bytes()).hexdigest(),
            "commands": {"probe": str(command)},
            "version_probes": [
                {
                    "name": "probe",
                    "argv": [str(command)],
                    "first_line": "probe 1",
                }
            ],
        }
        manifest.write_text(json.dumps(payload) + "\n", encoding="utf-8")
        return manifest, apt

    def test_accepts_exact_manifest(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifest, apt = self.fixture(root)
            old_path = os.environ.get("PATH")
            os.environ["PATH"] = str(root)
            try:
                MODULE.verify(manifest, apt)
            finally:
                if old_path is None:
                    os.environ.pop("PATH", None)
                else:
                    os.environ["PATH"] = old_path

    def test_rejects_apt_lock_drift(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifest, apt = self.fixture(root)
            apt.write_text("changed\n", encoding="utf-8")
            with self.assertRaisesRegex(MODULE.ContractError, "apt lock drift"):
                MODULE.verify(manifest, apt)

    def test_rejects_command_path_drift(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifest, apt = self.fixture(root)
            old_path = os.environ.get("PATH")
            os.environ["PATH"] = "/usr/bin"
            try:
                with self.assertRaisesRegex(MODULE.ContractError, "path drift"):
                    MODULE.verify(manifest, apt)
            finally:
                if old_path is None:
                    os.environ.pop("PATH", None)
                else:
                    os.environ["PATH"] = old_path

    def test_rejects_symlinked_manifest(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifest, _ = self.fixture(root)
            link = root / "manifest-link.json"
            link.symlink_to(manifest)
            with self.assertRaisesRegex(MODULE.ContractError, "non-symlink"):
                MODULE.load_manifest(link)

    def test_rejects_non_executable_promised_command(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifest, apt = self.fixture(root)
            command = root / "probe"
            command.chmod(stat.S_IRUSR | stat.S_IWUSR)
            old_path = os.environ.get("PATH")
            os.environ["PATH"] = str(root)
            try:
                with self.assertRaises(MODULE.ContractError):
                    MODULE.verify(manifest, apt)
            finally:
                if old_path is None:
                    os.environ.pop("PATH", None)
                else:
                    os.environ["PATH"] = old_path


if __name__ == "__main__":
    unittest.main()
