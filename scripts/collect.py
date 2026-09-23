#!/usr/bin/env python3
"""The retention policy behind ./collect.sh, evaluated on the host it runs on.

What can be regenerated decides what may go:

* An image can be restored from an archive that carries it.
* An archive can be rebuilt from its commit on the host that built it: every
  release proves its build reproduces there, and images reproduce per host,
  never across hosts.
* So the images of the deployed release and of the release before it stay, and
  so do their archives. The archive of every release that evidence references
  stays as well. Every older release keeps its archive and drops its images.

Both repositories' releases are governed here, because everything that decides
their fate lives in this one: the stack lock pins the backend image each
service release runs on, ./start.sh deploys both, and every session record,
judgement and benchmark pass is written by this repository.

A release is a lock state that pinned an archive: the commit at which an
archive hash first appears in config/release.lock.json (service) or
config/runtime-v1.sh (backend) fixes the images that archive carries, because
both producers bundle only images proved equal to the pins they adopt. An
object is ours only when a lock in either history pins it, its name is one a
lock derives, or it carries this project's own label; anything else is never
touched and never listed. What is deployed is read from the containers, not
from a file, and every state the evaluation cannot explain is a refusal naming
what was expected, what was found and what to do next.

Evidence is found rather than assumed to live where it should: the home
directory is walked (one filesystem, `.git` skipped) for the files the evidence
producers write -- session records (`s-<64 hex>/accepted.json`, `finished.json`,
`judgement.md`) and benchmark pass provenance (`release-provenance/release.json`,
per-run `result.json`, `pair-summary*.json`, `benchmark-lock.json`, status
files). Every full commit, image or archive hash such a file names references
that release, and a lock hash stands for everything that lock pinned. A session
record that names no release (schema 2) still ran on a release that existed
when it was accepted, so every release available before the newest such record
counts as referenced: an over-approximation that can only keep more, and the
report names the record that forces it.

`report` prints every one of our objects with its size, its release identity
and the rule that decides it, then one summary line. `delete` evaluates the
same way, then removes exactly the objects marked COLLECT, re-proving each one
immediately before removing it, and stops at the first removal that is refused.
"""

import hashlib
import http.client
import json
import os
import re
import shutil
import socket
import stat
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

HEX40 = re.compile(r"^[0-9a-f]{40}$")
HEX64 = re.compile(r"^[0-9a-f]{64}$")
IMAGE_ID = re.compile(r"^sha256:[0-9a-f]{64}$")
TOKEN40 = re.compile(r"(?<![0-9a-f])[0-9a-f]{40}(?![0-9a-f])")
TOKEN64 = re.compile(r"(?<![0-9a-f])[0-9a-f]{64}(?![0-9a-f])")
SESSION_DIR = re.compile(r"^s-[0-9a-f]{64}$")
SERVICE_ARCHIVE = re.compile(r"^agent-service-images-([0-9a-f]{40})-([0-9a-f]{64})\.tar$")
# The names earlier releases bundled under before the name carried the release:
# one fixed name, and briefly the first twelve hex of the commit. The code that
# derived them is gone, so such a file is identified only by its hash.
LEGACY_SERVICE_ARCHIVE = re.compile(r"^agent-service-images(-[0-9a-f]{12})?\.tar$")
BACKEND_ARCHIVE = re.compile(r"^qwen38-vllm-images-(runtime-v[0-9]+)\.tar$")
SERVICE_IDENTITY_TAG = re.compile(r"^[0-9a-f]{40}-[0-9a-f]{64}$")
BACKEND_IDENTITY_TAG = re.compile(r"^runtime-v[0-9]+-([0-9a-f]{64})$")
NULL_IMAGE = "sha256:" + "0" * 64
COMPONENTS = ("agent", "relay", "capture", "broker", "service")
BACKEND_FIELDS = ("IMAGE_TAG", "EXPECTED_IMAGE_ID", "IMAGE_ARCHIVE_NAME",
                  "IMAGE_ARCHIVE_SHA256", "EXPECTED_BASE_IMAGE_ID")
BACKEND_PROFILE_FIELDS = ("PROFILE_VERSION", "IMAGE_PROFILE_VERSION")
RECORD_FILES = {"accepted.json", "finished.json", "judgement.md"}
PASS_FILES = {"release.json", "result.json", "benchmark-lock.json",
              "production-status.txt", "status-latest.txt"}
PASS_PREFIX = "pair-summary"


class Refusal(Exception):
    pass


def refuse(*lines):
    raise Refusal("\n".join(lines))


def run(argv, *, stdin=None, what):
    result = subprocess.run(argv, input=stdin, capture_output=True, check=False)
    if result.returncode != 0:
        refuse(f"{what} failed (exit {result.returncode}): {' '.join(argv)}",
               result.stderr.decode(errors="replace").strip())
    return result.stdout


def gb(size):
    return f"{size / 1e9:.2f} GB"


def short(image_id):
    return image_id.removeprefix("sha256:")[:12]


def utc(seconds):
    return time.strftime("%Y-%m-%dT%H:%MZ", time.gmtime(seconds))


# ---------------------------------------------------------------------------
# Releases, from the lock histories
# ---------------------------------------------------------------------------

class Release:
    def __init__(self, kind, name, images, repositories, archive, cut_at, cut_commit):
        self.kind = kind                  # "service" or "backend"
        self.name = name                  # the identity its archive name and tags carry
        self.images = images              # component -> image ID
        self.repositories = repositories  # component -> repository of its tags
        self.archives = [archive]         # every archive hash pinned for it
        self.cut_at = cut_at
        self.cut_commit = cut_commit      # the commit whose lock pinned the archive
        self.backend_image = None         # service: the backend image it pins
        self.available_at = None          # when a lock first pinned its defining image
        self.archive_name = None          # backend: the per-version archive name

    def short(self):
        if self.kind == "service":
            return f"service {self.name[:12]}-{self.name[41:53]}"
        version, _, image = self.name.rpartition("-")
        return f"backend {version}-{image[:12]}"


def git_history(repo, path):
    """(commit, commit time, bytes) for every commit that recorded path."""
    fields = run(["git", "-C", str(repo), "log", "--reverse", "--format=%H %ct", "--", path],
                 what=f"reading the history of {path} in {repo}").decode().split()
    commits = list(zip(fields[0::2], (int(t) for t in fields[1::2])))
    if not commits:
        refuse(f"{repo} records no history for {path}.",
               "Next: run this from a complete clone of the repository.")
    requests = "".join(f"{commit}:{path}\n" for commit, _ in commits).encode()
    raw = run(["git", "-C", str(repo), "cat-file", "--batch"], stdin=requests,
              what=f"reading every recorded version of {path}")
    history, offset = [], 0
    for commit, when in commits:
        end = raw.index(b"\n", offset)
        header = raw[offset:end].decode().split()
        if len(header) != 3 or header[1] != "blob":
            offset = end + 1          # the commit deleted the path
            continue
        size = int(header[2])
        history.append((commit, when, raw[end + 1:end + 1 + size]))
        offset = end + 1 + size + 1
    return history


def committed(repo, path):
    """The working file, which must be exactly what HEAD records."""
    working = Path(repo, path)
    if working.is_symlink() or not working.is_file():
        refuse(f"Expected a regular file at {working}; found "
               f"{'a symlink' if working.is_symlink() else 'none'}.",
               f"Next: git -C {repo} checkout -- {path}")
    head = run(["git", "-C", str(repo), "show", f"HEAD:{path}"], what=f"reading HEAD:{path}")
    if working.read_bytes() != head:
        refuse(f"{working} differs from what HEAD records: a release is in progress, or the "
               "lock was edited by hand. Retention is evaluated against committed locks only.",
               "Next: let ./release.sh finish, or commit or discard the edit.")
    return head


def parse_json(blob, where):
    try:
        return json.loads(blob)
    except ValueError as error:
        refuse(f"{where} does not parse as JSON: {error}",
               "Next: repair it in git; nothing was evaluated.")


def backend_values(blob, where):
    text, values = blob.decode(), {}
    for name in BACKEND_FIELDS + BACKEND_PROFILE_FIELDS:
        found = re.findall(rf'^readonly {name}="([^"]*)"$', text, re.MULTILINE)
        if len(found) > 1 or (not found and name in BACKEND_FIELDS):
            refuse(f"{where} declares {name} {len(found)} times; exactly once is required.",
                   "Next: repair it in git; nothing was evaluated.")
        values[name] = found[0] if found else None
    return values


def repository_of(tag, where):
    repository, _, version = tag.rpartition(":")
    if not repository or not version or "@" in tag:
        refuse(f"{where} names {tag!r}, which is not a repository:tag image tag.")
    return repository


def service_catalog(project):
    stacks, stack_blobs, base_ids = [], {}, set()
    for commit, when, blob in git_history(project, "config/stack.lock.json"):
        stack = parse_json(blob, f"config/stack.lock.json at {commit[:12]}")
        stacks.append((commit, stack))
        stack_blobs[hashlib.sha256(blob).hexdigest()] = stack
        for base in ("toolchain", "runtime"):
            image = (((stack.get("build") or {}).get("base") or {}).get(base) or {}).get("image_id")
            if isinstance(image, str) and IMAGE_ID.match(image):
                base_ids.add(image)
    order = {commit: index for index, commit in enumerate(
        run(["git", "-C", str(project), "rev-list", "--reverse", "HEAD"],
            what="ordering the history").decode().split())}

    releases, by_name, first_pin, lock_blobs, profiles, seen = [], {}, {}, {}, set(), set()
    for commit, when, blob in git_history(project, "config/release.lock.json"):
        lock = parse_json(blob, f"config/release.lock.json at {commit[:12]}")
        lock_blobs[hashlib.sha256(blob).hexdigest()] = lock
        profiles.add(lock.get("profile"))
        images = lock.get("images") or {}
        for component in COMPONENTS:
            image = images.get(component)
            if isinstance(image, str) and IMAGE_ID.match(image) and image != NULL_IMAGE:
                first_pin.setdefault(image, (when, commit))
        archive = lock.get("archive")
        sha = archive.get("sha256") if isinstance(archive, dict) else None
        if not isinstance(sha, str) or sha in seen:
            continue   # no archive yet, or an archive pin carried forward from its release
        seen.add(sha)
        implementation = lock.get("implementation_commit")
        if not (HEX64.match(sha) and isinstance(implementation, str) and HEX40.match(implementation)
                and all(isinstance(images.get(c), str) and IMAGE_ID.match(images[c]) for c in COMPONENTS)):
            refuse(f"The release lock at {commit[:12]} pins archive {sha[:12]} without a complete "
                   "release identity.", "Next: repair the lock history; nothing was evaluated.")
        cut_with = [s for c, s in stacks if order[c] <= order[commit]]
        if not cut_with:
            refuse(f"No stack lock is recorded at or before {commit[:12]}, where an archive was pinned.")
        stack = cut_with[-1]
        repositories = {c: repository_of((stack.get(c) or {}).get("image_tag", ""),
                                         f"the stack lock at {commit[:12]}") for c in COMPONENTS}
        pinned = {c: images[c] for c in COMPONENTS}
        name = f"{implementation}-{images['service'].removeprefix('sha256:')}"
        if name in by_name:
            if by_name[name].images != pinned:
                refuse(f"Two releases claim the identity {name}: {by_name[name].cut_commit[:12]} "
                       f"pins {by_name[name].images} and {commit[:12]} pins {pinned}.",
                       "An identity names exactly one set of images.",
                       "Next: find how the lock came to record both; nothing was evaluated.")
            by_name[name].archives.append(sha)   # the same release, bundled again
            continue
        release = Release("service", name, pinned, repositories, sha, when, commit)
        backend = (stack.get("backend") or {}).get("image_id")
        release.backend_image = backend if isinstance(backend, str) and IMAGE_ID.match(backend) else None
        by_name[name] = release
        releases.append(release)
    for release in releases:
        release.available_at = first_pin[release.images["service"]][0]
    return {"releases": releases, "by_name": by_name, "first_pin": first_pin,
            "profiles": {p for p in profiles if isinstance(p, str)}, "base_ids": base_ids,
            "lock_blobs": lock_blobs, "stack_blobs": stack_blobs}


def backend_catalog(backend_dir):
    releases, by_name, first_pin, profiles, base_ids, seen = [], {}, {}, set(), set(), set()
    for commit, when, blob in git_history(backend_dir, "config/runtime-v1.sh"):
        values = backend_values(blob, f"config/runtime-v1.sh at {commit[:12]}")
        image, sha = values["EXPECTED_IMAGE_ID"], values["IMAGE_ARCHIVE_SHA256"]
        profiles |= {values[f] for f in BACKEND_PROFILE_FIELDS if values[f]}
        if not (IMAGE_ID.match(image) and HEX64.match(sha) and IMAGE_ID.match(values["EXPECTED_BASE_IMAGE_ID"])):
            refuse(f"config/runtime-v1.sh at {commit[:12]} pins a malformed image or archive hash.",
                   "Next: repair the lock history; nothing was evaluated.")
        base_ids.add(values["EXPECTED_BASE_IMAGE_ID"])
        first_pin.setdefault(image, (when, commit))
        if sha in seen:
            continue
        seen.add(sha)
        version = BACKEND_ARCHIVE.match(values["IMAGE_ARCHIVE_NAME"])
        if not version:
            refuse(f"config/runtime-v1.sh at {commit[:12]} names the archive "
                   f"{values['IMAGE_ARCHIVE_NAME']}, which is not a per-version archive name.")
        name = f"{version.group(1)}-{image.removeprefix('sha256:')}"
        if name in by_name:
            by_name[name].archives.append(sha)   # the same image, saved again
            continue
        release = Release("backend", name, {"runtime": image},
                          {"runtime": repository_of(values["IMAGE_TAG"], f"config/runtime-v1.sh at {commit[:12]}")},
                          sha, when, commit)
        release.archive_name = values["IMAGE_ARCHIVE_NAME"]
        by_name[name] = release
        releases.append(release)
    for release in releases:
        release.available_at = first_pin[release.images["runtime"]][0]
    return {"releases": releases, "by_name": by_name, "first_pin": first_pin,
            "profiles": profiles, "base_ids": base_ids}


def before(releases, release):
    ordered = sorted(releases, key=lambda r: r.cut_at)
    index = ordered.index(release)
    return ordered[index - 1] if index else None


# ---------------------------------------------------------------------------
# Docker, read through the daemon's own API
# ---------------------------------------------------------------------------

class UnixHTTPConnection(http.client.HTTPConnection):
    def __init__(self, path):
        super().__init__("localhost", timeout=300)
        self.socket_path = path

    def connect(self):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(300)
        self.sock.connect(self.socket_path)


def docker_state():
    endpoint = run(["docker", "context", "inspect", "--format", "{{.Endpoints.docker.Host}}"],
                   what="resolving the Docker endpoint").decode().strip()
    if not endpoint.startswith("unix://"):
        refuse(f"The Docker endpoint is {endpoint!r}; only the local daemon's unix socket is supported.",
               "Next: run this on the host whose daemon holds the images.")
    path = endpoint.removeprefix("unix://")
    try:
        connection = UnixHTTPConnection(path)
        connection.request("GET", "/system/df?type=image&type=container")
        response = connection.getresponse()
        body = response.read()
    except OSError as error:
        refuse(f"The Docker socket {path} does not answer: {error}",
               "Next: start the Docker daemon, or run this where it is reachable.")
    if response.status != 200:
        refuse(f"The Docker API answered HTTP {response.status}: {body[:300]!r}")
    usage = json.loads(body)
    images = {}
    for entry in usage.get("Images") or []:
        shared = entry.get("SharedSize")
        if not isinstance(shared, int) or shared < 0:
            refuse(f"Docker reported no shared size for {entry['Id']}, so its unique size is unknown.")
        images[entry["Id"]] = {"tags": sorted(t for t in entry.get("RepoTags") or [] if t != "<none>:<none>"),
                               "labels": entry.get("Labels") or {},
                               "size": entry["Size"], "unique": entry["Size"] - shared}
    users = {}
    for entry in usage.get("Containers") or []:
        users.setdefault(entry["ImageID"], []).append(
            {"name": entry["Names"][0].lstrip("/"), "labels": entry.get("Labels") or {}})
    return images, users


def container(users, name):
    found = [(image, entry) for image, entries in users.items() for entry in entries if entry["name"] == name]
    if len(found) > 1:
        refuse(f"Docker reports {len(found)} containers named {name}.")
    return found[0] if found else (None, None)


# ---------------------------------------------------------------------------
# What is deployed, read from the containers
# ---------------------------------------------------------------------------

def deployed_releases(stack, users, service, backend):
    names = {"service": stack["service"]["container_name"], "broker": stack["broker"]["container_name"],
             "bridge": stack["relay"]["service_bridge_container"],
             "ingress": stack["relay"]["service_ingress_container"]}
    present = {role: container(users, name) for role, name in names.items()}
    deployed_service = None
    if any(image for image, _ in present.values()):
        missing = [names[role] for role, (image, _) in present.items() if image is None]
        if missing:
            refuse("The service stack is partially present; missing " + ", ".join(missing) + ".",
                   "Next: ./stop.sh for an ownership-checked teardown, or ./start.sh.")
        for role, (_, entry) in present.items():
            if entry["labels"].get("agent_service.profile") not in service["profiles"]:
                refuse(f"Container {names[role]} has a stack name but not this project's label; it is "
                       "not ours, so what is deployed cannot be verified.",
                       "Next: find out whose container it is.")
        image = present["service"][0]
        matches = [r for r in service["releases"] if r.images["service"] == image]
        if len(matches) != 1:
            refuse(f"The running service image {short(image)} matches {len(matches)} releases; "
                   "the deployed release cannot be verified.",
                   "Next: deploy a released lock: ./stop.sh, then ./start.sh.")
        deployed_service = matches[0]
        for role, component in (("broker", "broker"), ("bridge", "relay"), ("ingress", "relay")):
            if present[role][0] != deployed_service.images[component]:
                refuse(f"Container {names[role]} runs {short(present[role][0])}, but the deployed "
                       f"{deployed_service.short()} pins {short(deployed_service.images[component])}.",
                       "The deployed release cannot be verified.",
                       "Next: ./stop.sh, then ./start.sh the release you mean to run.")
    image, entry = container(users, stack["backend"]["container_name"])
    deployed_backend = None
    if image:
        if entry["labels"].get("qwen38.project") != stack["backend"]["project_label"]:
            refuse(f"Container {stack['backend']['container_name']} is not labelled "
                   f"qwen38.project={stack['backend']['project_label']}; it is not ours.",
                   "Next: find out whose container it is.")
        matches = [r for r in backend["releases"] if r.images["runtime"] == image]
        if not matches:
            refuse(f"The running backend image {short(image)} matches no backend release; "
                   "the deployed release cannot be verified.",
                   "Next: deploy a released backend: ./stop.sh, then ./start.sh.")
        deployed_backend = max(matches, key=lambda r: r.cut_at)
        if deployed_service and deployed_service.backend_image != image:
            refuse(f"The deployed {deployed_service.short()} pins backend "
                   f"{short(deployed_service.backend_image or 'none')}, but {short(image)} is running.",
                   "Next: ./stop.sh, then ./start.sh one released stack.")
    return deployed_service, deployed_backend


# ---------------------------------------------------------------------------
# Evidence
# ---------------------------------------------------------------------------

def evidence_files(root, extra_root):
    device = os.lstat(root).st_dev
    found, crossed, pending, walked = [], [], [str(root)], 0
    extra = os.path.realpath(extra_root)
    if os.path.isdir(extra) and not (extra + os.sep).startswith(os.path.realpath(root) + os.sep):
        pending.append(extra)   # the results root lives elsewhere; it is searched too
    while pending:
        directory = pending.pop()
        walked += 1
        try:
            entries = list(os.scandir(directory))
        except OSError as error:
            refuse(f"The evidence search cannot read {directory}: {error.strerror}.",
                   "Evidence could be there, so nothing can be proved unreferenced.",
                   "Next: make it readable to this user.")
        for entry in entries:
            if entry.is_symlink():
                continue
            if entry.is_dir(follow_symlinks=False):
                if entry.name == ".git":
                    continue
                if entry.stat(follow_symlinks=False).st_dev != device:
                    crossed.append(entry.path)
                else:
                    pending.append(entry.path)
                continue
            if not entry.is_file(follow_symlinks=False):
                continue
            parent = os.path.basename(directory)
            if entry.name in RECORD_FILES and (SESSION_DIR.match(parent) or parent == "service-record"):
                found.append(entry.path)
            elif entry.name == "release.json" and parent == "release-provenance":
                found.append(entry.path)
            elif entry.name in PASS_FILES - {"release.json"} or entry.name.startswith(PASS_PREFIX):
                found.append(entry.path)
    return found, crossed, walked


def gather_evidence(home, results_dir, service):
    files, crossed, walked = evidence_files(home, results_dir)
    tokens, sources = set(), {}
    passes = sorted(os.path.dirname(os.path.dirname(f)) for f in files
                    if f.endswith(os.sep + os.path.join("release-provenance", "release.json")))
    records, copies, unresolved = 0, 0, []
    for path in files:
        with open(path, "rb") as handle:
            text = handle.read().decode(errors="replace")
        for token in TOKEN40.findall(text) + TOKEN64.findall(text):
            tokens.add(token)
            sources.setdefault(token, path)
        if os.path.basename(path) != "accepted.json":
            continue
        record = parse_json(text.encode(), path)
        session = SESSION_DIR.match(os.path.basename(os.path.dirname(path))) is not None
        records, copies = records + session, copies + (not session)
        if "release" in record:
            continue
        if not session and any(path.startswith(p + os.sep) for p in passes):
            continue   # a copy a benchmark pass kept; its pass recorded the release
        accepted = record.get("accepted_at_unix")
        if not isinstance(accepted, int):
            refuse(f"{path} names neither its release nor when it was accepted, so nothing bounds "
                   "which release it ran on.",
                   "Next: record the release it ran under beside it; nothing was evaluated.")
        unresolved.append((accepted, os.path.dirname(path)))
    # A lock hash a record names stands for every value that lock pinned.
    for sha in [t for t in tokens if len(t) == 64]:
        for table in (service["lock_blobs"], service["stack_blobs"]):
            if sha in table:
                text = json.dumps(table[sha])
                for token in TOKEN40.findall(text) + TOKEN64.findall(text):
                    tokens.add(token)
                    sources.setdefault(token, sources[sha])
    return {"tokens": tokens, "sources": sources, "files": len(files), "walked": walked,
            "crossed": crossed, "records": records, "copies": copies, "passes": len(passes),
            "unresolved": sorted(unresolved)}


def referencing(release, evidence):
    """Why evidence references this release, or None."""
    names = list(release.archives) + [image.removeprefix("sha256:") for image in
                                      ([release.images["service"]] if release.kind == "service"
                                       else [release.images["runtime"]])]
    if release.kind == "service":
        names.insert(0, release.name[:40])
    for token in names:
        if token in evidence["tokens"]:
            return f"named by {evidence['sources'][token]}"
    if evidence["unresolved"] and release.available_at < evidence["unresolved"][-1][0]:
        when, record = evidence["unresolved"][-1]
        return f"available before {utc(when)}, when {record} was accepted without naming its release"
    return None


# ---------------------------------------------------------------------------
# Archives
# ---------------------------------------------------------------------------

def sha256_file(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        while chunk := handle.read(1 << 22):
            digest.update(chunk)
    return digest.hexdigest()


def tar_files(directory):
    found = []
    if not directory.is_dir():
        return found
    for path in sorted(directory.iterdir()):
        if not path.name.endswith(".tar"):
            continue
        info = path.lstat()
        if stat.S_ISLNK(info.st_mode):
            refuse(f"{path} is a symlink to {os.readlink(path)}; an archive must be a regular file.",
                   "Next: replace it with the file itself, or with a hard link to it.")
        if not stat.S_ISREG(info.st_mode):
            refuse(f"{path} is not a regular file.")
        found.append((path, info))
    return found


def identify_archives(project, backend_dir, service, backend):
    candidates, untouched = [], []
    for path, info in tar_files(project / "artifacts"):
        derived = SERVICE_ARCHIVE.match(path.name)
        if derived:
            release = service["by_name"].get(f"{derived.group(1)}-{derived.group(2)}")
            if release is None:
                refuse(f"{path} is named for a release no lock in this checkout records.",
                       "Next: bring the checkout up to the commit that released it.")
            candidates.append((path, info, [release]))
        elif LEGACY_SERVICE_ARCHIVE.match(path.name):
            candidates.append((path, info, service["releases"]))
        else:
            untouched.append((path, info, "no release derives this name"))
    for path, info in tar_files(backend_dir / "artifacts"):
        owners = [r for r in backend["releases"] if r.archive_name == path.name]
        if owners:
            candidates.append((path, info, owners))
        else:
            untouched.append((path, info, "no backend release pinned this name"))
    with ThreadPoolExecutor(max_workers=4) as pool:
        digests = list(pool.map(lambda c: sha256_file(c[0]), candidates))
    archives = []
    for (path, info, owners), digest in zip(candidates, digests):
        release = next((r for r in owners if digest in r.archives), None)
        if release is not None:
            archives.append({"path": path, "info": info, "release": release})
        elif LEGACY_SERVICE_ARCHIVE.match(path.name):
            untouched.append((path, info, f"its SHA-256 {digest[:12]} matches no archive pin"))
        else:
            refuse(f"{path} carries a release's name, but its SHA-256 {digest} matches no archive "
                   "pin recorded for that name: the bytes are not the bundle the lock names.",
                   "Next: re-bundle that release on the host that built it (./release.sh, or "
                   "./scripts/save-images.sh in the backend), or remove the file by hand once you "
                   "know what it is.")
    return archives, untouched


# ---------------------------------------------------------------------------
# The decisions
# ---------------------------------------------------------------------------

def regenerate(release):
    return f"check out {release.cut_commit[:12]} and rebuild it on the host that built it"


def decide_archives(archives, kept, referenced):
    decisions, restorable = [], {}
    by_inode = {}
    for archive in archives:
        by_inode.setdefault((archive["info"].st_dev, archive["info"].st_ino), []).append(archive)
    for group in by_inode.values():
        release = group[0]["release"]
        derived = [a for a in group if SERVICE_ARCHIVE.match(a["path"].name) or BACKEND_ARCHIVE.match(a["path"].name)]
        primary = (derived or group)[0]
        info = primary["info"]
        if release in kept:
            verdict, rule = "KEEP", "the archive of a kept release"
            for image in release.images.values():
                restorable.setdefault(image, primary["path"].name)
        elif release in referenced:
            verdict, rule = "KEEP", f"evidence references its release: {referenced[release]}"
            for image in release.images.values():
                restorable.setdefault(image, primary["path"].name)
        else:
            verdict, rule = "COLLECT", f"no evidence references its release; {regenerate(release)}"
        frees = info.st_size if verdict == "COLLECT" and info.st_nlink == len(group) else 0
        if verdict == "COLLECT" and info.st_nlink > len(group):
            rule += f"; another name outside {primary['path'].parent} keeps the bytes, so this frees nothing"
        decisions.append({"kind": "archive", "verdict": verdict, "rule": rule, "path": primary["path"],
                          "info": info, "size": info.st_size, "frees": frees, "identity": release.short()})
        for other in group:
            if other is primary:
                continue
            decisions.append({"kind": "archive", "verdict": "COLLECT", "path": other["path"],
                              "info": other["info"], "size": info.st_size, "frees": 0,
                              "identity": release.short(),
                              "rule": f"a second name for {primary['path'].name}, which is decided on "
                                      "its own; removing this name frees nothing"})
    return decisions, restorable


def decide_image(image_id, image, users, owners, kept, referenced, restorable, base_ids,
                 first_pin, evidence):
    if users:
        return "KEEP", "used by container " + ", ".join(sorted(u["name"] for u in users))
    if image_id in base_ids:
        return "KEEP", "a pinned base image: a build input, not a release"
    if any(r in kept for r in owners):
        return "KEEP", "an image of a kept release"
    if image_id in restorable:
        return "COLLECT", f"restorable from the kept, verified archive {restorable[image_id]}"
    for release in owners:
        if release in referenced:
            return "KEEP", (f"evidence references its release ({referenced[release]}) and no kept "
                            "archive on this host carries it")
    if owners:
        return "COLLECT", f"an older release no evidence references; {regenerate(max(owners, key=lambda r: r.cut_at))}"
    if image_id in first_pin:
        when, commit = first_pin[image_id]
        token = image_id.removeprefix("sha256:")
        if token in evidence["tokens"]:
            return "KEEP", f"evidence names it: {evidence['sources'][token]}"
        if evidence["unresolved"] and when < evidence["unresolved"][-1][0]:
            return "KEEP", (f"pinned before {evidence['unresolved'][-1][1]} was accepted without naming "
                            "its release, and no archive carries it")
        return "COLLECT", (f"pinned only by the unreleased lock state at {commit[:12]} and named by no "
                           "evidence: an intermediate build; rebuild that commit to regenerate it")
    return "COLLECT", "carries this project's label but no lock ever pinned it: a build never adopted"


def check_identity_tags(image_id, tags, service, backend):
    """Every identity tag must name the image its release pins.

    Only releases -- lock states that pinned an archive -- carry identity tags.
    Earlier lock histories hold intermediate states that share a release's
    commit and service image but not its other images, so an identity is
    unambiguous only among releases, which is also the only place ./release.sh
    and the restore script ever apply one.
    """
    for tag in tags:
        repository, _, value = tag.rpartition(":")
        if SERVICE_IDENTITY_TAG.match(value):
            release = service["by_name"].get(value)
            if release is None:
                refuse(f"Image {short(image_id)} carries the identity tag {tag}, which names no release "
                       "in this checkout's history.",
                       "Next: bring the checkout up to the commit that released it.")
            component = next((c for c, r in release.repositories.items() if r == repository), None)
            if component is None or release.images[component] != image_id:
                refuse(f"Image {short(image_id)} carries the identity tag {tag}, but {release.short()} pins "
                       + (f"{short(release.images[component])} as its {component} image. " if component
                          else f"no image under {repository}. ")
                       + "An identity tag must name the image its release pins.",
                       "Next: find how the tag moved; nothing was evaluated.")
        elif (match := BACKEND_IDENTITY_TAG.match(value)) and repository in \
                {r.repositories["runtime"] for r in backend["releases"]}:
            if value not in backend["by_name"]:
                refuse(f"Image {short(image_id)} carries the identity tag {tag}, which names no backend "
                       "release in this checkout's history.",
                       "Next: bring the backend checkout up to the commit that released it.")
            if "sha256:" + match.group(1) != image_id:
                refuse(f"Image {short(image_id)} carries the identity tag {tag}, which names image "
                       f"{match.group(1)[:12]}. An identity tag must name the image its release pins.",
                       "Next: find how the tag moved; nothing was evaluated.")


def evaluate(project):
    stack = parse_json(committed(project, "config/stack.lock.json"), "config/stack.lock.json")
    lock = parse_json(committed(project, "config/release.lock.json"), "config/release.lock.json")
    if not isinstance(lock.get("archive"), dict):
        refuse("The checked-out release lock pins no archive: a release is in progress, or it "
               "stopped before bundling. Retention is evaluated only between releases.",
               "Next: let ./release.sh finish.")
    backend_dir = Path(stack["backend"]["project_dir"])
    results_dir = Path(stack["service"]["results_dir"])
    backend_lock = backend_values(committed(backend_dir, "config/runtime-v1.sh"),
                                  f"{backend_dir}/config/runtime-v1.sh")
    service, backend = service_catalog(project), backend_catalog(backend_dir)

    head = service["by_name"].get(f"{lock['implementation_commit']}-{lock['images']['service'].removeprefix('sha256:')}")
    if head is None or lock["archive"]["sha256"] not in head.archives:
        refuse("The checked-out release lock is not a release its own history records.")
    backend_head = next((r for r in backend["releases"]
                         if backend_lock["IMAGE_ARCHIVE_SHA256"] in r.archives), None)
    if backend_head is None or backend_head.images["runtime"] != backend_lock["EXPECTED_IMAGE_ID"]:
        refuse(f"{backend_dir}/config/runtime-v1.sh pins image {short(backend_lock['EXPECTED_IMAGE_ID'])} "
               f"with archive {backend_lock['IMAGE_ARCHIVE_SHA256'][:12]}, a pair no backend release records.",
               "Next: finish adopting the backend release, image and archive together.")

    images, users = docker_state()
    deployed_service, deployed_backend = deployed_releases(stack, users, service, backend)

    # Kept whole: what runs, what this checkout would run, and the release
    # before each; a kept service release keeps the backend it pins.
    kept = set()
    for anchor in filter(None, (deployed_service, head)):
        kept |= {anchor, before(service["releases"], anchor)}
    backend_anchors = {r for r in (deployed_backend, backend_head) if r}
    for release in [r for r in kept if r]:
        backend_anchors |= {b for b in backend["releases"] if b.images["runtime"] == release.backend_image}
    for anchor in backend_anchors:
        kept |= {anchor, before(backend["releases"], anchor)}
    kept.discard(None)

    evidence = gather_evidence(Path.home(), results_dir, service)
    referenced = {}
    for release in service["releases"] + backend["releases"]:
        why = referencing(release, evidence)
        if why:
            referenced[release] = why
    for release in [r for r in referenced if r.kind == "service"]:
        for pinned in backend["releases"]:
            if pinned.images["runtime"] == release.backend_image:
                referenced.setdefault(pinned, f"the backend of {release.short()}, {referenced[release]}")

    archives, untouched = identify_archives(project, backend_dir, service, backend)
    decisions, restorable = decide_archives(archives, kept, referenced)

    owners_of = {}
    for release in service["releases"] + backend["releases"]:
        for image in release.images.values():
            owners_of.setdefault(image, []).append(release)
    first_pin = {**backend["first_pin"], **service["first_pin"]}
    base_ids = service["base_ids"] | backend["base_ids"]
    for image_id, image in sorted(images.items()):
        labels = image["labels"]
        if not (labels.get("agent_service.profile") in service["profiles"]
                or labels.get("qwen38.runtime.profile") in backend["profiles"]
                or image_id in first_pin or image_id in base_ids):
            continue
        check_identity_tags(image_id, image["tags"], service, backend)
        owners = sorted(owners_of.get(image_id, []), key=lambda r: r.cut_at)
        verdict, rule = decide_image(image_id, image, users.get(image_id, []), owners, kept, referenced,
                                     restorable, base_ids, first_pin, evidence)
        if owners:
            identity = owners[-1].short() + (f" and {len(owners) - 1} earlier" if len(owners) > 1 else "")
        elif image_id in base_ids:
            identity = "base image"
        elif image_id in first_pin:
            identity = f"pinned at {first_pin[image_id][1][:12]}, never released"
        else:
            identity = "never pinned"
        decisions.append({"kind": "image", "verdict": verdict, "rule": rule, "id": image_id,
                          "tags": image["tags"], "size": image["size"],
                          "frees": image["unique"] if verdict == "COLLECT" else 0, "identity": identity})
    return {"decisions": decisions, "untouched": untouched, "evidence": evidence, "kept": kept,
            "deployed": (deployed_service, deployed_backend), "head": (head, backend_head),
            "filesystems": {project, backend_dir}}


# ---------------------------------------------------------------------------
# Output and removal
# ---------------------------------------------------------------------------

def report(result):
    evidence = result["evidence"]
    deployed_service, deployed_backend = result["deployed"]
    head, backend_head = result["head"]
    print("Retention on this host (nothing is removed without --delete)")
    print(f"  deployed:    {deployed_service.short() if deployed_service else 'no service stack is running'}; "
          f"{deployed_backend.short() if deployed_backend else 'no backend is running'}")
    print(f"  checked out: {head.short()}; {backend_head.short()}")
    print("  kept whole:  " + ", ".join(r.short() for r in sorted(result["kept"], key=lambda r: (r.kind, r.cut_at))))
    print(f"  evidence:    {evidence['walked']} directories under {Path.home()} walked (one filesystem, "
          f".git skipped): {evidence['records']} session records, {evidence['copies']} record copies, "
          f"{evidence['passes']} benchmark passes, {evidence['files']} evidence files read")
    if evidence["unresolved"]:
        when, record = evidence["unresolved"][-1]
        print(f"               {len(evidence['unresolved'])} session records name no release; the newest, "
              f"{record}, was accepted {utc(when)}, so every release available before then counts as referenced")
    for path in evidence["crossed"]:
        print(f"               not searched, another filesystem: {path}")
    for path, info, why in result["untouched"]:
        print(f"  untouched:   {path} ({gb(info.st_size)}): {why}")
    print()
    order = {"COLLECT": 0, "KEEP": 1}
    for d in sorted(result["decisions"], key=lambda d: (order[d["verdict"]], d["kind"], d["identity"])):
        name = str(d["path"]) if d["kind"] == "archive" else \
            f"{short(d['id'])} {' '.join(d['tags']) if d['tags'] else '<untagged>'}"
        print(f"{d['verdict']:<8} {d['kind']:<7} {name}")
        print(f"         {gb(d['size'])}, frees {gb(d['frees'])} | {d['identity']} | {d['rule']}")
    print()
    print(summary(result))


def summary(result):
    collect = [d for d in result["decisions"] if d["verdict"] == "COLLECT"]
    if not collect:
        return "collectable: nothing"
    images = [d for d in collect if d["kind"] == "image"]
    archives = [d for d in collect if d["kind"] == "archive"]
    # A unique size is Docker's own count of the bytes only that image holds;
    # a layer shared only among collected images is counted in none of them,
    # so removing them all frees at least the sum.
    return (f"collectable: {plural(len(images), 'image')} (at least {gb(sum(d['frees'] for d in images))}), "
            f"{plural(len(archives), 'archive')} ({gb(sum(d['frees'] for d in archives))})")


def plural(count, noun):
    return f"{count} {noun}{'' if count == 1 else 's'}"


def delete(result):
    devices = {}
    for path in sorted(result["filesystems"]):
        devices.setdefault(os.stat(path).st_dev, path)
    free_before = {path: shutil.disk_usage(path).free for path in devices.values()}
    removed = 0
    for decision in [d for d in result["decisions"] if d["verdict"] == "COLLECT"]:
        if decision["kind"] == "archive":
            path, then = decision["path"], decision["info"]
            now = path.lstat()
            if (now.st_dev, now.st_ino, now.st_size, now.st_mtime_ns, stat.S_ISREG(now.st_mode)) != \
                    (then.st_dev, then.st_ino, then.st_size, then.st_mtime_ns, True):
                refuse(f"{path} changed after it was evaluated; {removed} objects were removed before it.",
                       "Next: run ./collect.sh again.")
            path.unlink()
            print(f"REMOVED archive {path}")
        else:
            image_id = decision["id"]
            inspected = json.loads(run(["docker", "image", "inspect", image_id],
                                       what=f"re-reading image {short(image_id)}"))[0]
            tags = sorted(t for t in inspected.get("RepoTags") or [] if t != "<none>:<none>")
            in_use = run(["docker", "ps", "--all", "--quiet", "--no-trunc", "--filter", f"ancestor={image_id}"],
                         what=f"listing containers of {short(image_id)}").decode().split()
            if tags != decision["tags"] or in_use:
                refuse(f"Image {short(image_id)} changed after it was evaluated (tags or containers); "
                       f"{removed} objects were removed before it.", "Next: run ./collect.sh again.")
            for tag in tags or [image_id]:
                run(["docker", "image", "rm", tag], what=f"removing {tag}")
            gone = subprocess.run(["docker", "image", "inspect", image_id], capture_output=True).returncode != 0
            if not gone:
                refuse(f"Image {short(image_id)} survived the removal of every name it had.")
            print(f"REMOVED image {short(image_id)} {' '.join(tags) or '<untagged>'}")
        removed += 1
    freed = {path: shutil.disk_usage(path).free - free for path, free in free_before.items()}
    print(f"removed: {removed} objects; free space change: " +
          ", ".join(f"{path} {gb(delta)}" for path, delta in sorted(freed.items())))


def main():
    if len(sys.argv) != 3 or sys.argv[2] not in ("report", "delete"):
        print("usage: collect.py <agent_service checkout> report|delete", file=sys.stderr)
        return 2
    try:
        result = evaluate(Path(sys.argv[1]).resolve())
        report(result)
        if sys.argv[2] == "delete":
            print()
            delete(result)
    except Refusal as refusal:
        print(f"ERROR: {refusal}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
