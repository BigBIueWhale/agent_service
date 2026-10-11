#!/usr/bin/env bash
# Record the served tokenizer's count of every text the client's tests count.
#
# The tests count text with packages/core/src/test-utils/token-counter.ts,
# which answers from served-token-counts.json beside it and refuses a text it
# holds no count of: a test counter never estimates. The image's build runs its
# test selection against that record (docker/scripts/run_test_selection.py), so
# the record must hold every text that selection counts, on whatever day the
# build runs. This writes it from three runs of that same step, each run as the
# build runs it: in the pinned toolchain image, as root, in /src, with the
# environment the Dockerfile's qwen-build stage declares.
#
#   1. With recorder.ts in the counter's place and the served tokenizer
#      answering on a Unix socket (count_server.py, in the backend's base image,
#      on the tokenizer.json the backend's model manifest pins).
#   2. The same, on another day: another-day.cjs moves every clock the tests
#      read by ANOTHER_DAY_MS. A run whose counted texts differ from the first
#      is refused, naming the tests that counted them, because no record can
#      hold a text that names when, where or by whom it was counted.
#   3. With the counter back, against the record the first run wrote, on
#      another day: the image's own step, passed before the record is kept.
#
# Every run must end in the selection's one passing verdict.
#
# usage: scripts/served-token-counts/record.sh TREE
#   TREE  the reviewed qwen-code tree after the image's build steps: every file
#         the patch set leaves is the one it states, the record this writes
#         excepted, and it holds no .git, as the build's tree holds none
set -Eeuo pipefail
umask 022

HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd -- "${HERE}/../.." && pwd)"
readonly HERE PROJECT_DIR
readonly STACK_LOCK="${PROJECT_DIR}/config/stack.lock.json"
readonly DOCKERFILE="${PROJECT_DIR}/docker/Dockerfile"
readonly COUNTER_PATH="packages/core/src/test-utils/token-counter.ts"
readonly RECORD_PATH="packages/core/src/test-utils/served-token-counts.json"
# Another day: 397 days, 1 hour, 1 minute, 1 second and 1 millisecond later,
# so the second and third runs fall in another year and month, on another
# weekday, at another hour, minute, second and millisecond than the first.
readonly ANOTHER_DAY_MS=$(((397 * 24 * 3600 + 3600 + 60 + 1) * 1000 + 1))

die() {
  printf 'ERROR: %s\n' "$*" >&2
  exit 1
}

(($# == 1)) || die "usage: scripts/served-token-counts/record.sh TREE"
TREE="$(cd -- "$1" && pwd)"
readonly TREE
readonly COUNTER="${TREE}/${COUNTER_PATH}"
readonly RECORD="${TREE}/${RECORD_PATH}"
[[ ! -e "${TREE}/.git" ]] ||
  die "${TREE} holds .git, and the tree the image's build runs its tests in holds none, so a test that asks whether its tree is a repository would count another text here; keep the tree's history outside it (git --git-dir=...)"
[[ -d "${TREE}/node_modules" && -f "${TREE}/dist/record-verifier.js" ]] ||
  die "${TREE} has not been through the image's build steps (docker/Dockerfile's qwen-build stage, up to the test selection): it lacks node_modules or dist/record-verifier.js"

# The tree is the one the patch set states: the selection run_test_selection.py
# derives is that patch set's, so a record made over another tree is a record
# of another selection.
PYTHONDONTWRITEBYTECODE=1 python3 - "${PROJECT_DIR}" "${TREE}" "${RECORD_PATH}" <<'PY' ||
import hashlib
import sys
from pathlib import Path

project, tree, record = sys.argv[1], Path(sys.argv[2]), sys.argv[3]
sys.path.insert(0, project)
from patches.source_patch_v1.generated_qwen_code_stage import FINAL_FILES

differ = []
for path, digest in sorted(FINAL_FILES.items()):
    if path == record:
        continue
    target = tree / path
    if digest is None:
        if target.exists():
            differ.append(f"{path} (the patch set removes it)")
    elif not target.is_file() or hashlib.sha256(target.read_bytes()).hexdigest() != digest:
        differ.append(path)
if differ:
    print(
        f"{tree} is not the tree the patch set states: {len(differ)} files differ, first "
        + ", ".join(differ[:5])
        + ". Compile the stage from this tree (patches/source_patch_v1/compile_review_diff.py),"
        " or record over the tree it states.",
        file=sys.stderr,
    )
    sys.exit(1)
PY
  die "the tree is not the one the patch set states; see above"

lock() { jq -er "$1" "${STACK_LOCK}" || die "stack lock lacks $1"; }
BACKEND_DIR="$(lock '.backend.project_dir')"
MODEL_MANIFEST="${BACKEND_DIR}/manifests/$(lock '.backend.model_manifest')"
TOKENIZER="${BACKEND_DIR}/$(lock '.backend.model_directory')/tokenizer.json"
TOOLCHAIN_IMAGE="$(lock '.build.base.toolchain.image_tag')"
TOOLCHAIN_IMAGE_ID="$(lock '.build.base.toolchain.image_id')"
readonly BACKEND_DIR MODEL_MANIFEST TOKENIZER TOOLCHAIN_IMAGE TOOLCHAIN_IMAGE_ID
[[ "$(docker image inspect --format '{{.Id}}' "${TOOLCHAIN_IMAGE}")" == "${TOOLCHAIN_IMAGE_ID}" ]] ||
  die "the toolchain image ${TOOLCHAIN_IMAGE} is absent or not ${TOOLCHAIN_IMAGE_ID}"

# The served tokenizer: the file the pinned model manifest names.
[[ "$(sha256sum -- "${MODEL_MANIFEST}" | cut -d' ' -f1)" == "$(lock '.backend.model_manifest_sha256')" ]] ||
  die "the backend model manifest is not the one the stack lock pins: ${MODEL_MANIFEST}"
TOKENIZER_SHA256="$(sha256sum -- "${TOKENIZER}" | cut -d' ' -f1)"
readonly TOKENIZER_SHA256
[[ "${TOKENIZER_SHA256}" == "$(awk '$2 == "tokenizer.json" {print $1}' "${MODEL_MANIFEST}")" ]] ||
  die "the served tokenizer is not the file the model manifest pins: ${TOKENIZER}"
read -r BASE_IMAGE BASE_IMAGE_ID < <(
  # shellcheck source=/dev/null
  source "${BACKEND_DIR}/config/runtime-v1.sh" && printf '%s %s\n' "${BASE_IMAGE_TAG}" "${EXPECTED_BASE_IMAGE_ID}"
)
readonly BASE_IMAGE BASE_IMAGE_ID
[[ "$(docker image inspect --format '{{.Id}}' "${BASE_IMAGE}")" == "${BASE_IMAGE_ID}" ]] ||
  die "the backend base image ${BASE_IMAGE} is absent or not ${BASE_IMAGE_ID}"

# The build stage's environment, read from the stage: it runs as root, in /src,
# with the variables its ENV lines set and the toolchain image's own.
STAGE="$(awk '/^FROM / { inside = ($0 ~ / AS qwen-build$/) } inside' "${DOCKERFILE}")"
readonly STAGE
[[ -n "${STAGE}" ]] || die "${DOCKERFILE} has no qwen-build stage"
if grep -q '^USER ' <<<"${STAGE}"; then
  die "the qwen-build stage names a USER, and this records as root, as a stage without one runs"
fi
[[ "$(grep '^WORKDIR ' <<<"${STAGE}")" == 'WORKDIR /src' ]] ||
  die "the qwen-build stage's working directory is not /src alone, where this mounts the tree"
STAGE_ENV=()
while read -r directive assignment rest; do
  [[ -z "${rest}" && "${assignment}" =~ ^[A-Z_][A-Z0-9_]*=[A-Za-z0-9_./:-]*$ ]] ||
    die "the qwen-build stage sets its environment in a form this does not read: ${directive} ${assignment} ${rest}"
  STAGE_ENV+=(-e "${assignment}")
done < <(grep '^ENV ' <<<"${STAGE}")
readonly STAGE_ENV

WORK="$(mktemp -d /tmp/agent-service-token-counts.XXXXXX)"
readonly WORK
readonly SERVER="agent-service-token-counter-$$"
mkdir -- "${WORK}/sock" "${WORK}/rec"
chmod 0777 "${WORK}/sock" "${WORK}/rec"
cp -- "${COUNTER}" "${WORK}/token-counter.ts"
cp -- "${RECORD}" "${WORK}/served-token-counts.json"
cleanup() {
  cp -- "${WORK}/token-counter.ts" "${COUNTER}"
  docker stop "${SERVER}" >/dev/null 2>&1 || :
  rm -rf -- "${WORK}"
}
trap cleanup EXIT

docker run --detach --rm --name "${SERVER}" --network none --read-only \
  --user "$(id -u):$(id -g)" \
  -v "${TOKENIZER}:/tokenizer.json:ro" -v "${HERE}/count_server.py:/count_server.py:ro" \
  -v "${WORK}/sock:/sock" \
  --entrypoint python3 "${BASE_IMAGE}" /count_server.py /tokenizer.json /sock/count.sock >/dev/null
for _ in $(seq 600); do
  [[ -S "${WORK}/sock/count.sock" ]] && break
  sleep 0.1
done
[[ -S "${WORK}/sock/count.sock" ]] || die "the served tokenizer did not start answering"

# One run of the image's test selection step, as the build runs it, its output
# in WORK/NAME.log; extra docker arguments set the day. What the run writes into
# the tree is handed to the caller, whose tree it is.
selection_run() {
  local name="$1"
  shift
  docker run --rm --network none --user 0:0 -w /src "${STAGE_ENV[@]}" "$@" \
    -e OWNER="$(id -u):$(id -g)" \
    -v "${TREE}:/src" -v "${PROJECT_DIR}:/opt/agent-artifacts:ro" \
    -v "${WORK}/sock:/sock" -v "${WORK}/rec:/rec" \
    --entrypoint bash "${TOOLCHAIN_IMAGE}" -c '
      touch /tmp/started
      python3 /opt/agent-artifacts/docker/scripts/run_test_selection.py /opt/agent-artifacts /src
      verdict=$?
      find /src -newer /tmp/started -user 0 -exec chown -h "${OWNER}" {} +
      exit "${verdict}"' >"${WORK}/${name}.log" 2>&1
}
failed_run() {
  die "the $1 run of the image's test selection did not pass$2; the end of its output:" \
    "$(tail -40 "${WORK}/$1.log")"
}
readonly ANOTHER_DAY=(
  -e "ANOTHER_DAY_MS=${ANOTHER_DAY_MS}" -e NODE_OPTIONS=--require=/another-day.cjs
  -v "${HERE}/another-day.cjs:/another-day.cjs:ro"
)

cp -- "${HERE}/recorder.ts" "${COUNTER}"
: >"${WORK}/rec/counts.jsonl"
selection_run first || failed_run first ''
mv -- "${WORK}/rec/counts.jsonl" "${WORK}/first.jsonl"
: >"${WORK}/rec/counts.jsonl"
selection_run another-day "${ANOTHER_DAY[@]}" || failed_run another-day ''
mv -- "${WORK}/rec/counts.jsonl" "${WORK}/another-day.jsonl"
cp -- "${WORK}/token-counter.ts" "${COUNTER}"

PYTHONDONTWRITEBYTECODE=1 python3 - "${WORK}/first.jsonl" "${WORK}/another-day.jsonl" <<'PY' ||
import json
import sys

def counted(path):
    texts = {}
    for line in open(path, encoding="utf-8"):
        row = json.loads(line)
        texts.setdefault(row["sha"], row)
    return texts

first, another = counted(sys.argv[1]), counted(sys.argv[2])
differ = [("first day", first[sha]) for sha in sorted(first.keys() - another.keys())] + [
    ("another day", another[sha]) for sha in sorted(another.keys() - first.keys())
]
if differ:
    print(
        f"the tests counted {len(differ)} texts on one day that they did not on the other, so no"
        " record can hold them: a counted text names something that differs between runs -- the"
        " day, the hour, a temporary path, the host. Count a text that stays the same; these were"
        " counted, each by the first test that counted it in its process:",
        file=sys.stderr,
    )
    for day, row in differ:
        where = row.get("file", "?").removeprefix("/src/")
        test = row.get("test", "(outside any test)")
        print(f"  {day}: {row['bytes']} bytes, SHA-256 {row['sha']}, {where} > {test}", file=sys.stderr)
    sys.exit(1)
PY
  die "the counted texts depend on the day or the run; see above"

python3 "${HERE}/build_fixture.py" "${WORK}/first.jsonl" "${TOKENIZER_SHA256}" "${RECORD}"
if ! selection_run against-the-record "${ANOTHER_DAY[@]}"; then
  cp -- "${WORK}/served-token-counts.json" "${RECORD}"
  failed_run against-the-record ' with the counter answering from the record just written, so the record is left as it was'
fi
