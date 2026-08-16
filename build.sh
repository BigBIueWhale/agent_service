#!/usr/bin/env bash
set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/common.sh
source "${SCRIPT_DIR}/scripts/common.sh"
require_no_arguments "./build.sh" "$@"
check_host_tools_and_versions
check_pinned_inputs
require_clean_repository

AGENT_IMAGE="$(lock_value '.agent.image_tag')"
SERVICE_IMAGE="$(lock_value '.service.image_tag')"
SOURCE_COMMIT="$(git -C "${PROJECT_DIR}" rev-parse HEAD)"
SETTINGS_SHA256="$(lock_value '.agent.settings_sha256')"
INSTRUCTIONS_SHA256="$(lock_value '.agent.instructions_sha256')"
WRAPPER_SHA256="$(lock_value '.agent.wrapper_sha256')"
STACK_LOCK_SHA256="$(sha256_file "${STACK_LOCK}")"
CARGO_LOCK_SHA256="$(sha256_file "${PROJECT_DIR}/Cargo.lock")"
UBUNTU_IMAGE="$(lock_value '.build.ubuntu_amd64_image')"
NODE_IMAGE="$(lock_value '.build.node_amd64_image')"
RUST_IMAGE="$(lock_value '.build.rust_amd64_image')"
UBUNTU_SNAPSHOT="$(lock_value '.build.ubuntu_snapshot')"
AGENT_APT_LOCK_SHA256="$(lock_value '.build.agent_apt_lock_sha256')"
SERVICE_APT_LOCK_SHA256="$(lock_value '.build.service_apt_lock_sha256')"
QWEN_COMMIT="$(lock_value '.agent.qwen_code.commit')"
QWEN_SOURCE_ARCHIVE="$(lock_value '.agent.qwen_code.source_archive')"
QWEN_SOURCE_ARCHIVE_SHA256="$(lock_value '.agent.qwen_code.source_archive_sha256')"
QWEN_PATCH_SHA256="$(lock_value '.agent.qwen_code.patch_sha256')"
QWEN_SOURCE_PATCH_MANIFEST_SHA256="$(lock_value '.agent.qwen_code.source_patch_manifest_sha256')"
DOCKER_CLI_ARCHIVE="$(lock_value '.build.docker_cli_archive')"
DOCKER_CLI_ARCHIVE_SHA256="$(lock_value '.build.docker_cli_archive_sha256')"
readonly AGENT_IMAGE SERVICE_IMAGE SOURCE_COMMIT SETTINGS_SHA256 INSTRUCTIONS_SHA256
readonly WRAPPER_SHA256 STACK_LOCK_SHA256 CARGO_LOCK_SHA256 UBUNTU_IMAGE NODE_IMAGE
readonly RUST_IMAGE UBUNTU_SNAPSHOT AGENT_APT_LOCK_SHA256 SERVICE_APT_LOCK_SHA256
readonly QWEN_COMMIT QWEN_SOURCE_ARCHIVE QWEN_SOURCE_ARCHIVE_SHA256 QWEN_PATCH_SHA256
readonly QWEN_SOURCE_PATCH_MANIFEST_SHA256
readonly DOCKER_CLI_ARCHIVE DOCKER_CLI_ARCHIVE_SHA256
readonly SOURCE_DATE_EPOCH=1786725153

printf 'Building the one pinned agent image (clean Qwen source + reviewed patch + contract tests)...\n'
docker buildx build \
  --builder default \
  --platform linux/amd64 \
  --provenance=false \
  --load \
  --target agent \
  --tag "${AGENT_IMAGE}" \
  --build-arg "SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH}" \
  --build-arg "UBUNTU_IMAGE=${UBUNTU_IMAGE}" \
  --build-arg "NODE_IMAGE=${NODE_IMAGE}" \
  --build-arg "UBUNTU_SNAPSHOT=${UBUNTU_SNAPSHOT}" \
  --build-arg "AGENT_APT_LOCK_SHA256=${AGENT_APT_LOCK_SHA256}" \
  --build-arg "QWEN_COMMIT=${QWEN_COMMIT}" \
  --build-arg "QWEN_SOURCE_ARCHIVE=${QWEN_SOURCE_ARCHIVE}" \
  --build-arg "QWEN_SOURCE_ARCHIVE_SHA256=${QWEN_SOURCE_ARCHIVE_SHA256}" \
  --build-arg "QWEN_PATCH_SHA256=${QWEN_PATCH_SHA256}" \
  --build-arg "QWEN_SOURCE_PATCH_MANIFEST_SHA256=${QWEN_SOURCE_PATCH_MANIFEST_SHA256}" \
  --build-arg "SETTINGS_SHA256=${SETTINGS_SHA256}" \
  --build-arg "INSTRUCTIONS_SHA256=${INSTRUCTIONS_SHA256}" \
  --build-arg "WRAPPER_SHA256=${WRAPPER_SHA256}" \
  --file "${PROJECT_DIR}/docker/Dockerfile" \
  "${PROJECT_DIR}"
require_equal "agent image ID" "$(image_id "${AGENT_IMAGE}")" "$(lock_value '.agent.image_id')"
require_agent_image_contract

printf 'Building the pinned Docker-only service image from committed source %s...\n' "${SOURCE_COMMIT}"
docker buildx build \
  --builder default \
  --platform linux/amd64 \
  --provenance=false \
  --load \
  --target service \
  --tag "${SERVICE_IMAGE}" \
  --build-arg "SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH}" \
  --build-arg "UBUNTU_IMAGE=${UBUNTU_IMAGE}" \
  --build-arg "RUST_IMAGE=${RUST_IMAGE}" \
  --build-arg "UBUNTU_SNAPSHOT=${UBUNTU_SNAPSHOT}" \
  --build-arg "SERVICE_APT_LOCK_SHA256=${SERVICE_APT_LOCK_SHA256}" \
  --build-arg "DOCKER_CLI_ARCHIVE=${DOCKER_CLI_ARCHIVE}" \
  --build-arg "DOCKER_CLI_ARCHIVE_SHA256=${DOCKER_CLI_ARCHIVE_SHA256}" \
  --build-arg "SOURCE_COMMIT=${SOURCE_COMMIT}" \
  --build-arg "STACK_LOCK_SHA256=${STACK_LOCK_SHA256}" \
  --build-arg "CARGO_LOCK_SHA256=${CARGO_LOCK_SHA256}" \
  --file "${PROJECT_DIR}/docker/Dockerfile" \
  "${PROJECT_DIR}"

require_service_image_contract
printf 'Build complete. Agent=%s Service=%s\n' "$(image_id "${AGENT_IMAGE}")" "$(image_id "${SERVICE_IMAGE}")"
