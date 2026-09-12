#!/usr/bin/env bash
# One script fetches. Nothing else does.
#
# The generic layers of this stack -- pinned apt packages, the toolchains and
# the Node dependency bytes -- live in two base images built here. `./build.sh`
# builds our own images from them and reaches the network for nothing, which is
# the point: rebuilding our logic, which happens constantly, no longer
# re-downloads several hundred packages from a third party's mirror to arrive
# at bytes we already had.
#
# Run this only when a pin in `config/stack.lock.json` moves. It is the rare
# operation; `./build.sh` is the common one. Both images are then pinned by ID
# and travel to another machine inside the release archive, never by a rebuild,
# because images do not reproduce across hosts.
set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/common.sh
source "${SCRIPT_DIR}/common.sh"
require_no_arguments "./scripts/build-base-images.sh" "$@"
check_host_tools_and_versions

TOOLCHAIN_IMAGE="$(lock_value '.build.base.toolchain.image_tag')"
RUNTIME_IMAGE="$(lock_value '.build.base.runtime.image_tag')"
UBUNTU_IMAGE="$(lock_value '.build.ubuntu_amd64_image')"
NODE_IMAGE="$(lock_value '.build.node_amd64_image')"
UBUNTU_SNAPSHOT="$(lock_value '.build.ubuntu_snapshot')"
SOURCE_DATE_EPOCH="$(lock_value '.build.source_date_epoch')"
AGENT_APT_LOCK_SHA256="$(lock_value '.build.agent_apt_lock_sha256')"
SERVICE_APT_LOCK_SHA256="$(lock_value '.build.service_apt_lock_sha256')"
JKS_NORMALIZER_SHA256="$(lock_value '.build.jks_normalizer_sha256')"
JKS_NORMALIZER_TEST_SHA256="$(lock_value '.build.jks_normalizer_test_sha256')"
NPM_LOCK_PACKAGE_SET_SHA256="$(lock_value '.build.npm_lock_package_set_sha256')"
GO_ARCHIVE="$(lock_value '.build.go_archive')"
GO_ARCHIVE_SHA256="$(lock_value '.build.go_archive_sha256')"
QWEN_SOURCE_ARCHIVE="$(lock_value '.agent.qwen_code.source_archive')"
QWEN_SOURCE_ARCHIVE_SHA256="$(lock_value '.agent.qwen_code.source_archive_sha256')"
readonly TOOLCHAIN_IMAGE RUNTIME_IMAGE UBUNTU_IMAGE NODE_IMAGE UBUNTU_SNAPSHOT
readonly SOURCE_DATE_EPOCH AGENT_APT_LOCK_SHA256 SERVICE_APT_LOCK_SHA256
readonly JKS_NORMALIZER_SHA256 JKS_NORMALIZER_TEST_SHA256
readonly NPM_LOCK_PACKAGE_SET_SHA256 GO_ARCHIVE GO_ARCHIVE_SHA256
readonly QWEN_SOURCE_ARCHIVE QWEN_SOURCE_ARCHIVE_SHA256

BASE_EXPORT_DIR="$(mktemp -d /tmp/qwen38-base-image-build.XXXXXX)"
case "${BASE_EXPORT_DIR}" in
  /tmp/qwen38-base-image-build.*) ;;
  *) die "Unexpected temporary base-export directory: ${BASE_EXPORT_DIR}" ;;
esac
readonly BASE_EXPORT_DIR
cleanup_base_export() {
  rm -rf -- "${BASE_EXPORT_DIR}"
}
trap cleanup_base_export EXIT

# The normalizer and the lock reducer are ours and run inside these builds, so
# they are proved here exactly as the build proves every other input it trusts.
require_equal "JKS normalizer SHA256" \
  "$(sha256_file "${PROJECT_DIR}/docker/scripts/normalize_jks.py")" \
  "${JKS_NORMALIZER_SHA256}"
require_equal "JKS normalizer test SHA256" \
  "$(sha256_file "${PROJECT_DIR}/docker/tests/test_normalize_jks.py")" \
  "${JKS_NORMALIZER_TEST_SHA256}"
require_equal "npm lock reducer SHA256" \
  "$(sha256_file "${PROJECT_DIR}/docker/scripts/npm_lock_package_set.py")" \
  "${NPM_LOCK_PACKAGE_SET_SHA256}"
python3 "${PROJECT_DIR}/docker/tests/test_npm_lock_package_set.py" >/dev/null

build_base() {
  local target="$1" tag="$2" archive="${BASE_EXPORT_DIR}/$1.tar"
  printf 'Building the generic %s base image...\n' "${target}"
  docker buildx build \
    --builder default \
    --platform linux/amd64 \
    --provenance=false \
    --pull=false \
    --no-cache \
    --target "${target}" \
    --output "type=docker,dest=${archive},name=${tag},rewrite-timestamp=true" \
    --build-arg "SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH}" \
    --build-arg "UBUNTU_IMAGE=${UBUNTU_IMAGE}" \
    --build-arg "NODE_IMAGE=${NODE_IMAGE}" \
    --build-arg "UBUNTU_SNAPSHOT=${UBUNTU_SNAPSHOT}" \
    --build-arg "AGENT_APT_LOCK_SHA256=${AGENT_APT_LOCK_SHA256}" \
    --build-arg "SERVICE_APT_LOCK_SHA256=${SERVICE_APT_LOCK_SHA256}" \
    --build-arg "JKS_NORMALIZER_SHA256=${JKS_NORMALIZER_SHA256}" \
    --build-arg "JKS_NORMALIZER_TEST_SHA256=${JKS_NORMALIZER_TEST_SHA256}" \
    --build-arg "NPM_LOCK_PACKAGE_SET_SHA256=${NPM_LOCK_PACKAGE_SET_SHA256}" \
    --build-arg "GO_ARCHIVE=${GO_ARCHIVE}" \
    --build-arg "GO_ARCHIVE_SHA256=${GO_ARCHIVE_SHA256}" \
    --build-arg "QWEN_SOURCE_ARCHIVE=${QWEN_SOURCE_ARCHIVE}" \
    --build-arg "QWEN_SOURCE_ARCHIVE_SHA256=${QWEN_SOURCE_ARCHIVE_SHA256}" \
    --file "${PROJECT_DIR}/docker/Dockerfile.base" \
    "${PROJECT_DIR}"
  docker load --input "${archive}"
  rm -f -- "${archive}"
}

build_base noble-toolchain "${TOOLCHAIN_IMAGE}"
build_base noble-runtime "${RUNTIME_IMAGE}"

# Report what was produced. Verification belongs where the artifact is
# consumed, not where it is made: `./build.sh` requires these exact IDs and
# refuses any other, so a rebuild that lands on different bytes is an adoption
# the operator makes deliberately in `config/stack.lock.json`, never a
# difference a build absorbs.
printf '\nBASE_IMAGES_BUILT\n'
printf '  %s\n    %s\n' "${TOOLCHAIN_IMAGE}" "$(image_id "${TOOLCHAIN_IMAGE}")"
printf '  %s\n    %s\n' "${RUNTIME_IMAGE}" "$(image_id "${RUNTIME_IMAGE}")"
printf '\nPin these under .build.base in config/stack.lock.json. ./build.sh\n'
printf 'refuses to build against any base but the pinned one.\n'
