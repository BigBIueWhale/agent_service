#!/usr/bin/env python3
"""Fail-closed verifier for the immutable agent toolchain capability manifest."""

from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import stat
import subprocess
import sys
from pathlib import Path

EXPECTED_TOP_LEVEL = {
    "schema_version",
    "apt_lock_sha256",
    "commands",
    "version_probes",
}
COMMAND_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9+._-]*$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")


class ContractError(RuntimeError):
    pass


def require_regular_file(path: Path, label: str) -> None:
    metadata = path.lstat()
    if not stat.S_ISREG(metadata.st_mode) or path.is_symlink():
        raise ContractError(f"{label} must be a regular, non-symlink file: {path}")


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def load_manifest(path: Path) -> dict[str, object]:
    require_regular_file(path, "toolchain manifest")
    try:
        raw = path.read_bytes()
        if not raw.endswith(b"\n") or b"\r" in raw:
            raise ContractError("toolchain manifest must be terminal-LF JSON")
        value = json.loads(raw)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ContractError(f"toolchain manifest is invalid UTF-8 JSON: {error}") from error
    if not isinstance(value, dict) or set(value) != EXPECTED_TOP_LEVEL:
        raise ContractError(
            f"toolchain manifest fields differ: expected {sorted(EXPECTED_TOP_LEVEL)}, "
            f"observed {sorted(value) if isinstance(value, dict) else type(value).__name__}"
        )
    return value


def verify(manifest_path: Path, apt_lock_path: Path) -> None:
    manifest = load_manifest(manifest_path)
    if manifest["schema_version"] != 1:
        raise ContractError("toolchain manifest schema_version must equal 1")
    expected_apt = manifest["apt_lock_sha256"]
    if not isinstance(expected_apt, str) or not SHA256_RE.fullmatch(expected_apt):
        raise ContractError("toolchain manifest apt_lock_sha256 is malformed")
    require_regular_file(apt_lock_path, "agent apt lock")
    actual_apt = sha256_file(apt_lock_path)
    if actual_apt != expected_apt:
        raise ContractError(
            f"agent apt lock drift: expected {expected_apt}, observed {actual_apt}"
        )

    commands = manifest["commands"]
    if not isinstance(commands, dict) or not commands:
        raise ContractError("toolchain commands must be a nonempty object")
    for name, expected_path in sorted(commands.items()):
        if not isinstance(name, str) or not COMMAND_RE.fullmatch(name):
            raise ContractError(f"invalid toolchain command name: {name!r}")
        if (
            not isinstance(expected_path, str)
            or not expected_path.startswith("/")
            or ".." in Path(expected_path).parts
        ):
            raise ContractError(f"invalid path for {name}: {expected_path!r}")
        observed = shutil.which(name, path=os.environ.get("PATH"))
        if observed != expected_path:
            raise ContractError(
                f"toolchain path drift for {name}: expected {expected_path}, "
                f"observed {observed!r}"
            )
        if not os.access(expected_path, os.X_OK):
            raise ContractError(f"promised tool is not executable: {expected_path}")

    probes = manifest["version_probes"]
    if not isinstance(probes, list) or not probes:
        raise ContractError("version_probes must be a nonempty array")
    names: set[str] = set()
    for probe in probes:
        if not isinstance(probe, dict) or set(probe) != {
            "name",
            "argv",
            "first_line",
        }:
            raise ContractError(f"invalid version probe shape: {probe!r}")
        name = probe["name"]
        argv = probe["argv"]
        expected = probe["first_line"]
        if (
            not isinstance(name, str)
            or not COMMAND_RE.fullmatch(name)
            or name in names
        ):
            raise ContractError(f"invalid or duplicate probe name: {name!r}")
        names.add(name)
        if (
            not isinstance(argv, list)
            or not argv
            or not all(isinstance(item, str) and "\x00" not in item for item in argv)
            or not str(argv[0]).startswith("/")
            or not isinstance(expected, str)
            or "\n" in expected
            or "\r" in expected
        ):
            raise ContractError(f"invalid version probe {name}: {probe!r}")
        try:
            completed = subprocess.run(
                argv,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                check=False,
                timeout=10,
                env={"LANG": "C.UTF-8", "LC_ALL": "C.UTF-8", "PATH": os.environ["PATH"]},
            )
        except (OSError, subprocess.TimeoutExpired) as error:
            raise ContractError(f"version probe {name} failed to run: {error}") from error
        lines = completed.stdout.splitlines()
        observed = lines[0] if lines else ""
        if completed.returncode != 0 or observed != expected:
            raise ContractError(
                f"version probe {name} drift: expected rc=0 first_line={expected!r}; "
                f"observed rc={completed.returncode} first_line={observed!r}"
            )


def main() -> int:
    if len(sys.argv) != 3:
        print(
            "usage: verify_toolchain.py /opt/agent/toolchain-manifest.json "
            "/opt/locks/agent-apt-packages.lock",
            file=sys.stderr,
        )
        return 2
    try:
        verify(Path(sys.argv[1]), Path(sys.argv[2]))
    except (ContractError, OSError) as error:
        print(f"TOOLCHAIN CONTRACT FAILURE: {error}", file=sys.stderr)
        return 1
    print("TOOLCHAIN_CONTRACT_OK schema=1")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
