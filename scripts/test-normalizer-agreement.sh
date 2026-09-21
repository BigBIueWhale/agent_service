#!/usr/bin/env bash
# The normalizer agreement check.
#
# A byte bound stands in for tokens only in the form the served tokenizer
# reads: it normalizes to NFC before it splits anything, and NFC can make a
# text up to three times longer. Three normalizers measure that form in this
# deployment -- the Node the patched CLI runs on, the service's pinned
# unicode-normalization, and the suite materializer's Python unicodedata --
# and none of them is the tokenizer's own. This runs every Unicode scalar
# value, alone and in two composition probes, through all four, on the served
# tokenizer.json in the image the backend pins, and holds the three to it (see
# normalizer-agreement/tokenizer-check.py for what fails and why).
#
# It reads the served tokenizer from the backend checkout the stack lock names
# and proves it is the pinned file; Rust fetches the locked crates as the build
# does. Everything it writes is under one temporary directory it removes.
set -Eeuo pipefail
umask 077

PROJECT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
readonly PROJECT_DIR
readonly CHECK_DIR="${PROJECT_DIR}/scripts/normalizer-agreement"
readonly STACK_LOCK="${PROJECT_DIR}/config/stack.lock.json"

die() {
  printf 'ERROR: %s\n' "$*" >&2
  exit 1
}

for command in docker jq python3 sha256sum; do
  command -v "${command}" >/dev/null 2>&1 || die "required command is unavailable: ${command}"
done

lock() { jq -er "$1" "${STACK_LOCK}" || die "stack lock lacks $1"; }
BACKEND_DIR="$(lock '.backend.project_dir')"
MODEL_MANIFEST="${BACKEND_DIR}/manifests/$(lock '.backend.model_manifest')"
TOKENIZER="${BACKEND_DIR}/$(lock '.backend.model_directory')/tokenizer.json"
NODE_IMAGE="$(lock '.build.node_amd64_image')"
RUST_IMAGE="$(lock '.build.rust_amd64_image')"
readonly BACKEND_DIR MODEL_MANIFEST TOKENIZER NODE_IMAGE RUST_IMAGE

# The served tokenizer: the file the pinned model manifest names, and the image
# the backend builds its runtime from, which adds no Python package to it.
[[ "$(sha256sum -- "${MODEL_MANIFEST}" | cut -d' ' -f1)" == "$(lock '.backend.model_manifest_sha256')" ]] ||
  die "the backend model manifest is not the one the stack lock pins: ${MODEL_MANIFEST}"
[[ "$(sha256sum -- "${TOKENIZER}" | cut -d' ' -f1)" == "$(awk '$2 == "tokenizer.json" {print $1}' "${MODEL_MANIFEST}")" ]] ||
  die "the served tokenizer is not the file the model manifest pins: ${TOKENIZER}"
read -r TOKENIZER_IMAGE TOKENIZER_IMAGE_ID < <(
  # shellcheck source=/dev/null
  source "${BACKEND_DIR}/config/runtime-v1.sh" && printf '%s %s\n' "${BASE_IMAGE_TAG}" "${EXPECTED_BASE_IMAGE_ID}"
)
readonly TOKENIZER_IMAGE TOKENIZER_IMAGE_ID
[[ "$(docker image inspect --format '{{.Id}}' "${TOKENIZER_IMAGE}")" == "${TOKENIZER_IMAGE_ID}" ]] ||
  die "the backend base image ${TOKENIZER_IMAGE} is absent or not ${TOKENIZER_IMAGE_ID}"

WORK="$(mktemp -d /tmp/agent-service-normalizer-agreement.XXXXXX)"
readonly WORK
cleanup() { rm -rf -- "${WORK}"; }
trap cleanup EXIT
readonly USER_ARGS=(--user "$(id -u):$(id -g)")

# The materializer's normalizer is this host's Python, the one it asserts.
[[ "$(python3 -c 'import unicodedata; print(unicodedata.unidata_version)')" == 15.0.0 ]] ||
  die "this host's python3 is not the Unicode 15.0.0 the suite materializer requires"
python3 "${CHECK_DIR}/probes.py" "${WORK}"

docker run --rm --network none "${USER_ARGS[@]}" \
  -v "${WORK}:/work" -v "${CHECK_DIR}:/check:ro" \
  "${NODE_IMAGE}" node /check/node-nfc.mjs

mkdir -- "${WORK}/cargo-home" "${WORK}/target"
docker run --rm -i "${USER_ARGS[@]}" \
  -v "${PROJECT_DIR}:/src:ro" -w /src \
  -v "${WORK}/cargo-home:/cargo-home" -e CARGO_HOME=/cargo-home \
  -v "${WORK}/target:/target" -e CARGO_TARGET_DIR=/target \
  "${RUST_IMAGE}" cargo run --quiet --locked --example nfc_agreement \
  <"${WORK}/probes.json" >"${WORK}/rust.json"

docker run --rm --network none "${USER_ARGS[@]}" \
  -v "${WORK}:/work:ro" -v "${CHECK_DIR}:/check:ro" \
  -v "${TOKENIZER}:/tokenizer/tokenizer.json:ro" \
  -e PYTHONDONTWRITEBYTECODE=1 \
  --entrypoint python3 "${TOKENIZER_IMAGE}" /check/tokenizer-check.py
