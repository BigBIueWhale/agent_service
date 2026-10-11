#!/usr/bin/env bash
# Record the served tokenizer's count of every text the client's tests count.
#
# The tests count text with packages/core/src/test-utils/token-counter.ts,
# which answers from served-token-counts.json beside it and refuses a text it
# holds no count of: a test counter never estimates. This writes that record.
# It puts recorder.ts in the counter's place, runs the named test files twice
# in the pinned toolchain image with the served tokenizer answering on a Unix
# socket (count_server.py, in the backend's base image, on the tokenizer.json
# the backend's model manifest pins), refuses a run whose counted texts differ
# between the two, puts the counter back, and writes the record from what was
# counted. Every text a test counts must therefore be the same on every run.
#
# usage: scripts/served-token-counts/record.sh TREE PACKAGE/TEST_FILE...
#   TREE       the reviewed qwen-code tree, applied and installed
#   TEST_FILE  a test file, as PACKAGE/PATH, e.g. core/src/tools/read-file.test.ts
set -Eeuo pipefail
umask 022

HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd -- "${HERE}/../.." && pwd)"
readonly HERE PROJECT_DIR
readonly STACK_LOCK="${PROJECT_DIR}/config/stack.lock.json"

die() {
  printf 'ERROR: %s\n' "$*" >&2
  exit 1
}

(($# >= 2)) || die "usage: scripts/served-token-counts/record.sh TREE PACKAGE/TEST_FILE..."
TREE="$(cd -- "$1" && pwd)"
shift
readonly TREE
readonly COUNTER="${TREE}/packages/core/src/test-utils/token-counter.ts"
readonly RECORD="${TREE}/packages/core/src/test-utils/served-token-counts.json"
[[ -f "${COUNTER}" && -d "${TREE}/node_modules" ]] ||
  die "${TREE} is not an installed qwen-code tree: it lacks ${COUNTER} or node_modules"

lock() { jq -er "$1" "${STACK_LOCK}" || die "stack lock lacks $1"; }
BACKEND_DIR="$(lock '.backend.project_dir')"
MODEL_MANIFEST="${BACKEND_DIR}/manifests/$(lock '.backend.model_manifest')"
TOKENIZER="${BACKEND_DIR}/$(lock '.backend.model_directory')/tokenizer.json"
TOOLCHAIN_IMAGE="$(lock '.build.base.toolchain.image_tag')"
readonly BACKEND_DIR MODEL_MANIFEST TOKENIZER TOOLCHAIN_IMAGE

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

WORK="$(mktemp -d /tmp/agent-service-token-counts.XXXXXX)"
readonly WORK
readonly SERVER="agent-service-token-counter-$$"
mkdir -- "${WORK}/sock" "${WORK}/rec"
chmod 0777 "${WORK}/sock" "${WORK}/rec"
cp -- "${COUNTER}" "${WORK}/token-counter.ts"
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

# The files by package, each package's run in one Vitest process.
declare -A files_of=()
for file in "$@"; do
  [[ "${file}" == */* ]] || die "${file} names no package: write it as PACKAGE/PATH"
  files_of["${file%%/*}"]+=" ${file#*/}"
done
cp -- "${HERE}/recorder.ts" "${COUNTER}"
for run in 1 2; do
  : >"${WORK}/rec/counts.jsonl"
  for package in "${!files_of[@]}"; do
    # shellcheck disable=SC2086
    docker run --rm --network none --user "$(id -u):$(id -g)" -e HOME=/tmp \
      -e SOURCE_DATE_EPOCH="$(lock '.build.source_date_epoch')" \
      -v "${TREE}:/src" -v "${WORK}/sock:/sock" -v "${WORK}/rec:/rec" \
      -w "/src/packages/${package}" --entrypoint npx "${TOOLCHAIN_IMAGE}" \
      --no-install vitest run --coverage.enabled=false ${files_of[${package}]} \
      >"${WORK}/vitest-${run}-${package}.log" 2>&1 ||
      die "${package}'s tests failed with the served tokenizer counting; the end of their output:" \
        "$(tail -40 "${WORK}/vitest-${run}-${package}.log")"
  done
  cp -- "${WORK}/rec/counts.jsonl" "${WORK}/run-${run}.jsonl"
done
cmp -s <(jq -r .sha "${WORK}/run-1.jsonl" | sort -u) <(jq -r .sha "${WORK}/run-2.jsonl" | sort -u) ||
  die "the tests counted texts on one run that they did not on the other, so no record can hold them: a counted text names something that differs between runs, such as a mkdtemp path"
cp -- "${WORK}/token-counter.ts" "${COUNTER}"
python3 "${HERE}/build_fixture.py" "${WORK}/run-1.jsonl" "${TOKENIZER_SHA256}" "${RECORD}"
