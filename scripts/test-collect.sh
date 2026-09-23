#!/usr/bin/env bash
# Proves, without Docker and without touching anything, the parts of the
# retention policy that can drift silently:
#
#   * scripts/collect.py recognises a release by the same strings
#     scripts/common.sh derives -- the archive name and the identity tags --
#     and nothing looser;
#   * each keep-or-collect rule decides the way the policy states;
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
trap 'rm -rf -- "${TEST_DIR}"' EXIT

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
