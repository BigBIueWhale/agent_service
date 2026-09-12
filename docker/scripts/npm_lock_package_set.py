#!/usr/bin/env python3
"""Digest the set of tarballs an npm lockfile requires.

The generic base image holds the Node dependency bytes resolved from the
unmodified upstream lock. Our patch rewrites that lockfile, so the two files
never match byte for byte -- but what the cache has to satisfy is not the file,
it is the set of tarballs an install would fetch. This reduces a lockfile to
exactly that set and digests it, so a build can prove the cache covers the tree
it is about to install rather than discovering a miss partway through.

Only entries carrying `resolved` are counted: those are the ones npm fetches.
The root project and workspace links resolve to paths inside the tree and are
produced by the install, not downloaded for it.
"""

from __future__ import annotations

import hashlib
import json
import sys


def package_set(lock: object) -> list[str]:
    if not isinstance(lock, dict):
        raise SystemExit("lockfile is not an object")
    packages = lock.get("packages")
    if not isinstance(packages, dict):
        raise SystemExit("lockfile has no packages object")
    entries: list[str] = []
    for path, entry in packages.items():
        if not isinstance(entry, dict):
            raise SystemExit(f"lockfile entry is not an object: {path}")
        if entry.get("link") is True:
            continue
        resolved = entry.get("resolved")
        if not isinstance(resolved, str) or not resolved:
            continue
        version = entry.get("version")
        if not isinstance(version, str) or not version:
            raise SystemExit(f"resolved lockfile entry has no version: {path}")
        integrity = entry.get("integrity")
        if not isinstance(integrity, str) or not integrity:
            raise SystemExit(f"resolved lockfile entry has no integrity: {path}")
        entries.append(f"{path}\t{version}\t{integrity}")
    if not entries:
        raise SystemExit("lockfile resolves no packages")
    return sorted(entries)


def digest(lock: object) -> str:
    body = "\n".join(package_set(lock)) + "\n"
    return hashlib.sha256(body.encode("utf-8")).hexdigest()


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("usage: npm_lock_package_set.py PACKAGE_LOCK_JSON")
    with open(sys.argv[1], encoding="utf-8") as handle:
        lock = json.load(handle)
    print(digest(lock))


if __name__ == "__main__":
    main()
