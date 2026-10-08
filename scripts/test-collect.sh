#!/usr/bin/env bash
# Proves, without Docker and without touching anything, the parts of the
# retention policy that can drift silently:
#
#   * scripts/collect.py recognises a release by the same strings
#     scripts/common.sh derives -- the archive name and the identity tags --
#     and nothing looser;
#   * each keep-or-collect rule decides the way the policy states;
#   * evidence is read from the service's store and the registered stores and
#     from nowhere else, registering a store is idempotent, a registered
#     store that is gone or unreadable refuses, naming its registration, and a
#     delete not told that every evidence store is registered removes nothing;
#   * the storage hook answers in exactly one line of valid hook JSON and exits
#     0 even when its check cannot run at all.
set -Eeuo pipefail
if (($# != 0)); then
  printf 'ERROR: no arguments are supported. Usage: ./scripts/test-collect.sh\n' >&2
  exit 2
fi
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/common.sh
source "${SCRIPT_DIR}/common.sh"

TEST_DIR="$(mktemp -d /tmp/qwen38-collect-test.XXXXXX)"
readonly TEST_DIR
trap 'chmod -R u+rwX -- "${TEST_DIR}" 2>/dev/null; rm -rf -- "${TEST_DIR}"' EXIT

probe="${TEST_DIR}/release.lock.json"
jq --arg commit "$(printf '1%.0s' {1..40})" --arg service "sha256:$(printf '2%.0s' {1..64})" \
  '.implementation_commit = $commit | .images.service = $service' "${RELEASE_LOCK}" >"${probe}"
tags=()
for component in agent relay capture broker service; do
  tags+=("$(release_identity_tag "${component}" "${probe}")")
done

PYTHONDONTWRITEBYTECODE=1 python3 - "${PROJECT_DIR}/scripts/collect.py" \
  "$(basename "$(service_archive_path "${probe}")")" "$(release_identity "${probe}")" "${tags[@]}" <<'PY'
import importlib.util
import sys

spec = importlib.util.spec_from_file_location("collect", sys.argv[1])
collect = importlib.util.module_from_spec(spec)
spec.loader.exec_module(collect)
archive_name, identity, tags = sys.argv[2], sys.argv[3], sys.argv[4:]


def fail(message):
    sys.exit(f"COLLECT CONTRACT FAILURE: {message}")


match = collect.SERVICE_ARCHIVE.match(archive_name)
if not match or f"{match.group(1)}-{match.group(2)}" != identity:
    fail(f"the archive name common.sh derives is not recognised as release {identity}: {archive_name}")
for tag in tags:
    if not collect.SERVICE_IDENTITY_TAG.match(tag.rpartition(":")[2]) or tag.rpartition(":")[2] != identity:
        fail(f"the identity tag common.sh derives is not recognised: {tag}")
for name in (archive_name.replace(".tar", ".tar.releasing"), "agent-service-images-" + identity[:12] + ".tar",
             archive_name.upper()):
    if collect.SERVICE_ARCHIVE.match(name):
        fail(f"a name no release derives was recognised as a release archive: {name}")
if not collect.LEGACY_SERVICE_ARCHIVE.match("agent-service-images.tar"):
    fail("the fixed name earlier releases bundled under is not recognised for hashing")
if not collect.BACKEND_ARCHIVE.match("qwen38-vllm-images-runtime-v26.tar") or \
        collect.BACKEND_ARCHIVE.match("qwen38-vllm-images-runtime-v26.tar.saving"):
    fail("the backend's per-version archive name is not recognised exactly")


def release(name, cut_at, image="sha256:" + "a" * 64):
    made = collect.Release("service", name, {"service": image}, {"service": "repo"}, "f" * 64, cut_at, "c" * 40)
    made.available_at = cut_at
    return made


def decide(image_id="sha256:" + "a" * 64, users=(), owners=(), kept=(), referenced=None,
           restorable=None, base_ids=(), first_pin=None, unresolved=(), tokens=()):
    evidence = {"tokens": set(tokens), "sources": {t: "evidence-file" for t in tokens},
                "unresolved": list(unresolved)}
    return collect.decide_image(image_id, {}, list(users), list(owners), set(kept), referenced or {},
                                restorable or {}, set(base_ids), first_pin or {}, evidence)[0]


old, anchor = release("old", 100), release("anchor", 200)
image = "sha256:" + "a" * 64
cases = [
    ("an image a container uses", decide(users=[{"name": "someone"}], owners=[old]), "KEEP"),
    ("a pinned base image", decide(base_ids=[image]), "KEEP"),
    ("an image of a kept release", decide(owners=[old, anchor], kept=[anchor]), "KEEP"),
    ("an image a kept archive restores", decide(owners=[old], restorable={image: "a.tar"}), "COLLECT"),
    ("a referenced release with no archive here", decide(owners=[old], referenced={old: "named"}), "KEEP"),
    ("an older release nothing references", decide(owners=[old]), "COLLECT"),
    ("an unreleased pin named by evidence",
     decide(first_pin={image: (100, "c" * 40)}, tokens=["a" * 64]), "KEEP"),
    ("an unreleased pin older than a record naming no release",
     decide(first_pin={image: (100, "c" * 40)}, unresolved=[(150, "record")]), "KEEP"),
    ("an unreleased pin nothing references", decide(first_pin={image: (100, "c" * 40)}), "COLLECT"),
    ("a labelled image no lock ever pinned", decide(), "COLLECT"),
]
for what, verdict, expected in cases:
    if verdict != expected:
        fail(f"{what} was decided {verdict}, not {expected}")

evidence = {"tokens": set(), "sources": {}, "unresolved": [(150, "record")]}
if collect.referencing(old, evidence) is None:
    fail("a release available before a record naming no release was not treated as referenced")
if collect.referencing(release("new", 300), evidence) is not None:
    fail("a release made after the newest record naming no release was treated as referenced")
evidence = {"tokens": {"1" * 40}, "sources": {"1" * 40: "evidence-file"}, "unresolved": []}
if collect.referencing(release("1" * 40 + "-" + "2" * 64, 300), evidence) is None:
    fail("a release whose implementation commit evidence names was not treated as referenced")

before = collect.before([anchor, old], anchor)
if before is not old or collect.before([anchor, old], old) is not None:
    fail("the release before another is not the next older one by cut time")
print(f"COLLECT_CONTRACT_OK names=agree-with-common.sh rules={len(cases)} references=bounded")
PY

# Evidence stores. A scratch tree holds the service's store with a record
# naming release X, a registered store whose pass names Y, an unregistered
# directory whose pass names Z, and an unreadable directory outside every
# store. Only X and Y may be referenced, and nothing outside the stores is read.
stores_root="${TEST_DIR}/stores"
commit_x="$(printf 'a%.0s' {1..40})" commit_y="$(printf 'b%.0s' {1..40})" commit_z="$(printf 'c%.0s' {1..40})"
session="s-$(printf 'd%.0s' {1..64})"
mkdir -p -- "${stores_root}/results/schema-7/${session}" \
  "${stores_root}/registered/full-suite-v1/release-provenance" \
  "${stores_root}/unregistered/full-suite-v1/release-provenance" \
  "${stores_root}/unreadable-outside"
printf '{"release":{"implementation_commit":"%s"}}\n' "${commit_x}" \
  >"${stores_root}/results/schema-7/${session}/accepted.json"
printf '{"implementation_commit":"%s"}\n' "${commit_y}" \
  >"${stores_root}/registered/full-suite-v1/release-provenance/release.json"
printf '{"implementation_commit":"%s"}\n' "${commit_z}" \
  >"${stores_root}/unregistered/full-suite-v1/release-provenance/release.json"
registry="${stores_root}/runtime/evidence-stores"
register_evidence_store "${stores_root}/registered" "${registry}" >/dev/null
register_evidence_store "${stores_root}/registered/" "${registry}" >/dev/null
registrations=("${registry}"/*)
[[ "${#registrations[@]}" == 1 && "$(cat -- "${registrations[0]}")" == "${stores_root}/registered" &&
  "$(basename -- "${registrations[0]}")" == "$(printf '%s' "${stores_root}/registered" | sha256sum | cut -d' ' -f1)" &&
  "$(stat -c '%a' "${registry}")" == 700 ]] || {
  printf 'COLLECT CONTRACT FAILURE: registering a store twice did not leave exactly its one registration\n' >&2
  exit 1
}
if (register_evidence_store "${stores_root}/absent" "${registry}") >/dev/null 2>&1; then
  printf 'COLLECT CONTRACT FAILURE: a directory that does not exist was registered\n' >&2
  exit 1
fi
chmod 000 -- "${stores_root}/unreadable-outside"
[[ ! -r "${stores_root}/unreadable-outside" ]] || {
  printf 'COLLECT CONTRACT FAILURE: this test needs a user that file modes bind; run it as the project user\n' >&2
  exit 1
}

PYTHONDONTWRITEBYTECODE=1 python3 - "${PROJECT_DIR}/scripts/collect.py" "${stores_root}" "${registry}" \
  "${registrations[0]}" "${commit_x}" "${commit_y}" "${commit_z}" <<'PY'
import importlib.util
import os
import shutil
import sys

spec = importlib.util.spec_from_file_location("collect", sys.argv[1])
collect = importlib.util.module_from_spec(spec)
spec.loader.exec_module(collect)
root, registry, registration, x, y, z = sys.argv[2:]
results, registered = os.path.join(root, "results"), os.path.join(root, "registered")
no_locks = {"lock_blobs": {}, "stack_blobs": {}}


def fail(message):
    sys.exit(f"COLLECT CONTRACT FAILURE: {message}")


def release(commit):
    made = collect.Release("service", commit + "-" + "e" * 64, {"service": "sha256:" + "e" * 64},
                           {"service": "repo"}, "f" * 64, 100, "c" * 40)
    made.available_at = 100
    return made


def refusal(what):
    try:
        collect.gather_evidence(collect.evidence_stores(results, registry), no_locks)
    except collect.Refusal as refused:
        return str(refused)
    fail(f"{what} did not refuse")


stores = collect.evidence_stores(results, registry)
if [(s["path"], s["registration"]) for s in stores] != [(results, None), (registered, registration)]:
    fail(f"the stores read are not the service's and the registered one: {stores}")
evidence = collect.gather_evidence(stores, no_locks)
for commit, expected, where in ((x, True, "the service's store"), (y, True, "a registered store"),
                                (z, False, "an unregistered directory")):
    if (collect.referencing(release(commit), evidence) is not None) != expected:
        fail(f"a release named only in {where} was {'not ' if expected else ''}treated as referenced")
never = collect.evidence_stores(os.path.join(root, "never-written"), os.path.join(root, "no-registry"))
if len(never) != 1 or never[0].get("absent") is not True:
    fail("a service store that does not exist was not reported absent")

inside = os.path.join(registered, "full-suite-v1")
os.chmod(inside, 0)
message = refusal("an unreadable directory inside a registered store")
os.chmod(inside, 0o755)
if registered not in message or registration not in message or inside not in message:
    fail(f"the refusal for an unreadable directory does not name the store, its registration and the directory: {message}")

shutil.move(registered, registered + ".moved")
message = refusal("a registered store that is gone")
shutil.move(registered + ".moved", registered)
if registered not in message or registration not in message or "is gone" not in message:
    fail(f"the refusal for a store that is gone does not name it and its registration: {message}")
# A delete removes nothing without the statement that every evidence store on
# the host is registered, and its refusal names the stores it read and the
# commands that register another and delete with the statement.
try:
    collect.require_all_evidence_registered(evidence, None)
except collect.Refusal as refused:
    message = str(refused)
    for needed in ("Nothing was removed", results, registered, "./scripts/register-evidence-store.sh",
                   "./collect.sh --delete --all-evidence-registered"):
        if needed not in message:
            fail(f"the refusal of an unstated delete does not say {needed!r}: {message}")
else:
    fail("a delete without the statement that every evidence store is registered was not refused")
collect.require_all_evidence_registered(evidence, collect.ALL_EVIDENCE_REGISTERED)
print("COLLECT_EVIDENCE_OK stores=declared+registered unregistered=unread unreadable-outside=untouched "
      "refusals=named delete=stated")
PY
chmod 755 -- "${stores_root}/unreadable-outside"

hook_output="$(env -u CLAUDE_PROJECT_DIR "${PROJECT_DIR}/scripts/collect-hook.sh" <<<'{"hook_event_name":"PostToolUse"}')" || {
  printf 'COLLECT CONTRACT FAILURE: the storage hook exited non-zero\n' >&2
  exit 1
}
[[ "$(wc -l <<<"${hook_output}")" == 1 ]] &&
  jq -e '.hookSpecificOutput.hookEventName == "PostToolUse"
         and (.systemMessage | startswith("Storage check could not run: "))
         and .systemMessage == .hookSpecificOutput.additionalContext' <<<"${hook_output}" >/dev/null || {
  printf 'COLLECT CONTRACT FAILURE: the storage hook did not say, in one line of hook JSON, that its check could not run: %s\n' \
    "${hook_output}" >&2
  exit 1
}
printf 'COLLECT_HOOK_OK exit=0 lines=1 json=valid silent-failure=impossible\n'
