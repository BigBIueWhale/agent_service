#!/usr/bin/env bash
# The backend's refusals of a turn, held to the backend that serves them.
#
# The client redraws a turn the backend refused for what the model wrote, and
# tells such a refusal from every other error by the type the backend gives it:
# SERVED_TURN_REFUSALS, in the reviewed client patch's
# packages/core/src/core/generation-refusal.ts, declares one row per type. The
# backend pins the types it serves as SERVED_TOOL_CALL_REFUSAL_TYPES in its
# config/runtime-v1.sh, which its ./scripts/build-vllm.sh derives from the
# reviewed vLLM tree and refuses when they differ. This holds the table to the
# pin, so a type added, renamed or removed on one side alone stops a build on
# one side or the other: a type the backend serves that the table lacks would
# end a run the model could have corrected, as the backend's own failure, and a
# row the backend never serves is a refusal nothing sends.
#
# It reads the backend lock as the backend checkout the stack lock names
# records it at HEAD, and only when that lock pins the backend image this
# client pins: the backend serves only the image its lock pins, and only when
# that image was built from the commit's own inputs (its serve-check), so the
# pinned types are the ones any backend this client runs against serves. The
# table is read by the native certifier's own reader
# (protocol/engine/served_turn_refusals.rs), compiled from
# protocol/engine/examples/served_turn_refusals.rs in the pinned Rust image
# with no network, so it is read one way. Everything it writes is under one
# temporary directory it removes.
set -Eeuo pipefail
umask 077

PROJECT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
readonly PROJECT_DIR
readonly STACK_LOCK="${PROJECT_DIR}/config/stack.lock.json"
readonly CLIENT_PATCH="${PROJECT_DIR}/patches/qwen-code-0.21.12-agent-service.patch"
readonly ENGINE_DIR="${PROJECT_DIR}/protocol/engine"

die() {
  printf 'ERROR: %s\n' "$*" >&2
  exit 1
}

(($# == 0)) || die "no arguments are supported. Usage: ./scripts/check-served-refusals.sh"
for command in docker git jq; do
  command -v "${command}" >/dev/null 2>&1 || die "required command is unavailable: ${command}"
done

lock() { jq -er "$1" "${STACK_LOCK}" || die "stack lock lacks $1"; }
BACKEND_DIR="$(lock '.backend.project_dir')"
BACKEND_IMAGE_ID="$(lock '.backend.image_id')"
RUST_IMAGE="$(lock '.build.rust_amd64_image')"
readonly BACKEND_DIR BACKEND_IMAGE_ID RUST_IMAGE
readonly BACKEND_LOCK="config/runtime-v1.sh"

# The backend lock as its HEAD records it: the working file must be that, so
# what is read is a committed pin and not an edit in progress.
git -C "${BACKEND_DIR}" diff --quiet HEAD -- "${BACKEND_LOCK}" ||
  die "${BACKEND_DIR}/${BACKEND_LOCK} differs from what the backend's HEAD records." \
    "Next: commit or discard the edit there, and run this again."
backend_lock="$(git -C "${BACKEND_DIR}" show "HEAD:${BACKEND_LOCK}")" ||
  die "the backend checkout ${BACKEND_DIR} records no ${BACKEND_LOCK} at HEAD"
readonly backend_lock

# The one value the backend lock declares for name, which it must declare once.
backend_value() {
  local name="$1" lines
  lines="$(grep -E "^readonly ${name}=\"[^\"]*\"$" <<<"${backend_lock}" || :)"
  [[ -n "${lines}" && "${lines}" != *$'\n'* ]] ||
    die "${BACKEND_DIR}/${BACKEND_LOCK} must declare ${name} once, as readonly ${name}=\"...\""
  lines="${lines#*=\"}"
  printf '%s' "${lines%\"}"
}

backend_image_id="$(backend_value EXPECTED_IMAGE_ID)"
[[ "${backend_image_id}" == "${BACKEND_IMAGE_ID}" ]] ||
  die "the backend checkout ${BACKEND_DIR} pins image ${backend_image_id}, not the" \
    "image ${BACKEND_IMAGE_ID} this client pins, so the refusals its lock pins are" \
    "not the ones the backend this client runs against serves." \
    "Next: check out the backend release this client pins, or pin the one checked out."
served="$(backend_value SERVED_TOOL_CALL_REFUSAL_TYPES)"
readonly served

WORK="$(mktemp -d /tmp/agent-service-served-refusals.XXXXXX)"
readonly WORK
cleanup() { rm -rf -- "${WORK}"; }
trap cleanup EXIT

declared="$(
  docker run --rm --network none --read-only --user "$(id -u):$(id -g)" \
    --tmpfs /tmp:rw,nosuid,nodev,exec,size=256m \
    -v "${ENGINE_DIR}/served_turn_refusals.rs:/engine/served_turn_refusals.rs:ro" \
    -v "${ENGINE_DIR}/examples/served_turn_refusals.rs:/engine/examples/served_turn_refusals.rs:ro" \
    -v "${CLIENT_PATCH}:/patch:ro" \
    --entrypoint sh "${RUST_IMAGE}" -c '
      rustc --edition 2021 --crate-name served_turn_refusals -o /tmp/reader \
        /engine/examples/served_turn_refusals.rs && exec /tmp/reader /patch'
)" || die "the client's SERVED_TURN_REFUSALS could not be read from ${CLIENT_PATCH}; the reader's message is above."
readonly declared

[[ "${declared}" == "${served}" ]] ||
  die "the backend serves the tool-call refusal types [${served}] (SERVED_TOOL_CALL_REFUSAL_TYPES" \
    "in ${BACKEND_DIR}/${BACKEND_LOCK}), but the client's SERVED_TURN_REFUSALS declares" \
    "[${declared}]. A type the backend serves and the table lacks ends a run as the" \
    "backend's failure, and a row the backend never serves is a refusal nothing sends." \
    "Next: declare one row for each type the backend serves, and no other, in" \
    "packages/core/src/core/generation-refusal.ts."
printf 'SERVED_TURN_REFUSALS_OK types=%s\n' "${declared// /,}"
