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

    def test_version_probes_ignore_callers_hostile_working_directory(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            sealed = root / "sealed"
            hostile = root / "submitted-project"
            sealed.mkdir()
            hostile.mkdir()
            manifest, apt = self.fixture(sealed)
            probe = sealed / "probe"
            probe.write_text(
                "#!/bin/sh\n"
                "if test -e ./go.mod; then\n"
                "  printf 'hostile workspace affected version probe\\n'\n"
                "  exit 86\n"
                "fi\n"
                "printf 'probe 1\\n'\n",
                encoding="utf-8",
            )
            probe.chmod(0o755)
            (hostile / "go.mod").write_text(
                "module hostile.example/probe\n\ngo 1.99.0\n",
                encoding="utf-8",
            )
            old_path = os.environ.get("PATH")
            old_cwd = Path.cwd()
            os.environ["PATH"] = str(sealed)
            os.chdir(hostile)
            try:
                MODULE.verify(manifest, apt)
            finally:
                os.chdir(old_cwd)
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

    def test_rejects_manifest_reached_through_symlinked_parent(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            sealed = root / "sealed"
            sealed.mkdir()
            manifest, apt = self.fixture(sealed)
            alias = root / "alias"
            alias.symlink_to(sealed, target_is_directory=True)
            with self.assertRaisesRegex(MODULE.ContractError, "toolchain probe directory"):
                MODULE.verify(alias / manifest.name, apt)

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
