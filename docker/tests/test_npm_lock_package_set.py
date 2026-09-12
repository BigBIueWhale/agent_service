#!/usr/bin/env python3
# SPDX-License-Identifier: Unlicense

from __future__ import annotations

from pathlib import Path
import sys
import unittest


SCRIPT_DIR = Path(__file__).resolve().parent
TOOL_DIR = SCRIPT_DIR.parent / "scripts"
if not TOOL_DIR.is_dir():
    TOOL_DIR = SCRIPT_DIR
sys.path.insert(0, str(TOOL_DIR))

from npm_lock_package_set import digest, package_set  # noqa: E402


def lock(packages: dict[str, object]) -> dict[str, object]:
    return {"lockfileVersion": 3, "packages": packages}


RESOLVED = {
    "": {"name": "root", "version": "1.0.0"},
    "node_modules/left": {
        "version": "1.2.3",
        "resolved": "https://registry.example/left/-/left-1.2.3.tgz",
        "integrity": "sha512-left",
    },
    "node_modules/right": {
        "version": "4.5.6",
        "resolved": "https://registry.example/right/-/right-4.5.6.tgz",
        "integrity": "sha512-right",
    },
    "packages/workspace": {"link": True, "resolved": "packages/workspace"},
}


class PackageSetTests(unittest.TestCase):
    def test_counts_only_fetchable_entries(self) -> None:
        # The root project and workspace links are produced by the install, not
        # downloaded for it, so a cache does not have to hold them.
        self.assertEqual(
            package_set(lock(RESOLVED)),
            [
                "node_modules/left\t1.2.3\tsha512-left",
                "node_modules/right\t4.5.6\tsha512-right",
            ],
        )

    def test_is_insensitive_to_everything_that_is_not_the_tarball_set(self) -> None:
        # Exactly the shape of our patch's edit to the upstream lock: dev
        # classification, dependency edges and optionalDependencies move, while
        # the set of tarballs an install fetches does not.
        reclassified = {
            "": {"name": "root", "version": "1.0.0", "dependencies": {"left": "1.2.3"}},
            "node_modules/left": {
                "version": "1.2.3",
                "resolved": "https://registry.example/left/-/left-1.2.3.tgz",
                "integrity": "sha512-left",
                "optionalDependencies": {"right": "4.5.6"},
            },
            "node_modules/right": {
                "version": "4.5.6",
                "resolved": "https://registry.example/right/-/right-4.5.6.tgz",
                "integrity": "sha512-right",
            },
            "packages/workspace": {"link": True, "resolved": "packages/workspace"},
        }
        reclassified["node_modules/left"].pop("dev", None)
        self.assertEqual(digest(lock(RESOLVED)), digest(lock(reclassified)))

    def test_an_added_dependency_changes_the_digest(self) -> None:
        # The case the refusal exists for: a future patch that adds a package
        # the cache does not hold must not be discoverable only as a miss.
        extended = dict(RESOLVED)
        extended["node_modules/added"] = {
            "version": "0.1.0",
            "resolved": "https://registry.example/added/-/added-0.1.0.tgz",
            "integrity": "sha512-added",
        }
        self.assertNotEqual(digest(lock(RESOLVED)), digest(lock(extended)))

    def test_a_moved_version_changes_the_digest(self) -> None:
        moved = {key: dict(value) for key, value in RESOLVED.items()}  # type: ignore[arg-type]
        moved["node_modules/left"]["version"] = "1.2.4"
        self.assertNotEqual(digest(lock(RESOLVED)), digest(lock(moved)))

    def test_a_republished_tarball_changes_the_digest(self) -> None:
        # Same name and version, different bytes. The cache would satisfy the
        # name and serve the wrong content, so integrity is part of the set.
        republished = {key: dict(value) for key, value in RESOLVED.items()}  # type: ignore[arg-type]
        republished["node_modules/left"]["integrity"] = "sha512-tampered"
        self.assertNotEqual(digest(lock(RESOLVED)), digest(lock(republished)))

    def test_refuses_a_lockfile_that_resolves_nothing(self) -> None:
        with self.assertRaises(SystemExit):
            package_set(lock({"": {"name": "root", "version": "1.0.0"}}))

    def test_refuses_a_resolved_entry_missing_its_identity(self) -> None:
        for missing in ("version", "integrity"):
            broken = {key: dict(value) for key, value in RESOLVED.items()}  # type: ignore[arg-type]
            broken["node_modules/left"].pop(missing)
            with self.assertRaises(SystemExit):
                package_set(lock(broken))

    def test_refuses_a_lockfile_that_is_not_an_object(self) -> None:
        for value in ([], "lock", None):
            with self.assertRaises(SystemExit):
                package_set(value)


if __name__ == "__main__":
    unittest.main(verbosity=2)
