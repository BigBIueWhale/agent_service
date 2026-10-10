#!/usr/bin/env python3
"""Run the image's unit test selection and state its one verdict.

The selection is derived, never listed: every test file the reviewed patch
leaves in the tree, and the test beside every source file it leaves, each run
with Vitest in the package that owns it. Desktop's suites run under Bun
(`bun:test`), which the image does not carry, and `integration-tests/` needs a
provider, so neither is selected and nothing in the build runs them.

Every selected package is run even after one fails, every selected file must
come back with a result, and the run ends in exactly one of two statements:
every selected file passed, or these files did not. There is no list of known
failures and no outcome between the two, so nothing can call the selection
passing beside a test that failed or never ran. This file is the build's step,
not a description of it: a replay outside the image runs this file, as the
image's root user, in the pinned toolchain image, on a copy of the tree that has
been through the same build steps.

Usage: run_test_selection.py ARTIFACT_ROOT SOURCE_ROOT
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
import tempfile
from pathlib import Path

TEST_FILE = re.compile(r"\.test\.tsx?$")
SOURCE_FILE = re.compile(r"\.tsx?$")
UNSELECTED = ("integration-tests/", "packages/desktop/")


def selection(artifacts: Path, source: Path) -> list[str]:
    sys.path.insert(0, str(artifacts))
    from patches.source_patch_v1.generated_qwen_code_stage import FINAL_FILES

    retained = sorted(path for path, digest in FINAL_FILES.items() if digest is not None)
    patched_tests = [path for path in retained if TEST_FILE.search(path)]
    missing = [path for path in patched_tests if not (source / path).is_file()]
    if missing:
        raise SystemExit(f"patched test files are missing from the tree: {missing}")
    adjacent = [
        SOURCE_FILE.sub(lambda match: ".test" + match.group(0), path)
        for path in retained
        if SOURCE_FILE.search(path) and not TEST_FILE.search(path)
    ]
    present = [path for path in adjacent if (source / path).is_file()]
    return sorted(
        path for path in set(patched_tests) | set(present) if not path.startswith(UNSELECTED)
    )


def owning_package(source: Path, path: str) -> str:
    package = str(Path(path).parent)
    while package != "." and not (source / package / "package.json").is_file():
        package = str(Path(package).parent)
    if package == ".":
        raise SystemExit(f"no package owns the test file: {path}")
    return package


def run_package(source: Path, package: str, files: list[str]) -> dict[str, str]:
    """Run one package's selected files; return each file's outcome."""
    with tempfile.TemporaryDirectory(prefix="test-selection-") as scratch:
        report = Path(scratch) / "report.json"
        relative = [path[len(package) + 1 :] for path in files]
        completed = subprocess.run(
            [
                "npx", "--no-install", "vitest", "run",
                "--reporter=default", "--reporter=json", f"--outputFile.json={report}",
                *relative,
            ],
            cwd=source / package,
            check=False,
        )
        results: dict[str, str] = {}
        if report.is_file():
            for suite in json.loads(report.read_text(encoding="utf-8"))["testResults"]:
                name = Path(suite["name"]).resolve()
                results[str(name.relative_to(source.resolve()))] = suite["status"]
    outcomes = {}
    for path in files:
        status = results.get(path)
        if status is None:
            outcomes[path] = f"no result (vitest exited {completed.returncode})"
        elif status != "passed":
            outcomes[path] = status
        else:
            outcomes[path] = "passed"
    if completed.returncode != 0 and all(outcome == "passed" for outcome in outcomes.values()):
        outcomes[f"{package} (vitest)"] = f"exited {completed.returncode} with every file passed"
    return outcomes


def main(argv: list[str]) -> int:
    if len(argv) != 3:
        raise SystemExit(__doc__)
    artifacts, source = Path(argv[1]), Path(argv[2])
    selected = selection(artifacts, source)
    if not selected:
        raise SystemExit("the test selection is empty")
    by_package: dict[str, list[str]] = {}
    for path in selected:
        by_package.setdefault(owning_package(source, path), []).append(path)
    print(f"executing {len(selected)} test files in full, in {len(by_package)} packages:", flush=True)
    print("\n".join(selected), flush=True)
    outcomes: dict[str, str] = {}
    for package in sorted(by_package):
        outcomes.update(run_package(source, package, by_package[package]))
    failed = {path: outcome for path, outcome in outcomes.items() if outcome != "passed"}
    if failed:
        print(f"TEST_SELECTION_FAILED {len(failed)} of {len(selected)} selected files:", flush=True)
        for path, outcome in sorted(failed.items()):
            print(f"  {path}: {outcome}", flush=True)
        return 1
    print(f"TEST_SELECTION_OK files={len(selected)} packages={len(by_package)}", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
