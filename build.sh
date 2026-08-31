#!/usr/bin/env bash
set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/common.sh
source "${SCRIPT_DIR}/scripts/common.sh"
require_no_arguments "./build.sh" "$@"
check_host_tools_and_versions
check_pinned_inputs
require_release_commit
require_clean_committed_repository
"${PROJECT_DIR}/scripts/test-common.sh"
"${PROJECT_DIR}/scripts/test-submission.sh"
"${PROJECT_DIR}/scripts/test-release.sh"

AGENT_IMAGE="$(lock_value '.agent.image_tag')"
RELAY_IMAGE="$(lock_value '.relay.image_tag')"
CAPTURE_IMAGE="$(lock_value '.capture.image_tag')"
BROKER_IMAGE="$(lock_value '.broker.image_tag')"
SERVICE_IMAGE="$(lock_value '.service.image_tag')"
SOURCE_COMMIT="$(release_value '.implementation_commit')"
BUILD_INPUTS_MANIFEST_SHA256="$(release_value '.build_inputs_manifest_sha256')"
RELAY_SOURCE_SHA256="$(lock_value '.relay.source_sha256')"
RELAY_SANDBOX="$(lock_value '.relay.sandbox')"
CAPTURE_SOURCE_SHA256="$(lock_value '.capture.source_sha256')"
CAPTURE_ID="$(lock_value '.capture.capture_id')"
BROKER_POLICY_SHA256="$(lock_value '.broker.policy_sha256')"
BROKER_SOURCE_SHA256="$(lock_value '.broker.source_sha256')"
SETTINGS_SHA256="$(lock_value '.agent.settings_sha256')"
INSTRUCTIONS_SHA256="$(lock_value '.agent.instructions_sha256')"
SYSTEM_PROMPT_SHA256="$(lock_value '.agent.system_prompt_sha256')"
DEPLOYMENT_CONTRACT_SHA256="$(lock_value '.agent.deployment_contract_sha256')"
TOOLCHAIN_MANIFEST_SHA256="$(lock_value '.agent.toolchain_manifest_sha256')"
TOOLCHAIN_VERIFIER_SHA256="$(lock_value '.build.toolchain_verifier_sha256')"
TOOLCHAIN_VERIFIER_TEST_SHA256="$(lock_value '.build.toolchain_verifier_test_sha256')"
RUNTIME_CONTRACT_SHA256="$(lock_value '.agent.runtime_contract_sha256')"
RUNTIME_CONTRACT_VERIFIER_SHA256="$(lock_value '.build.runtime_contract_verifier_sha256')"
RUNTIME_CONTRACT_VERIFIER_TEST_SHA256="$(lock_value '.build.runtime_contract_verifier_test_sha256')"
WRAPPER_CONTRACT_TEST_SHA256="$(lock_value '.build.wrapper_contract_test_sha256')"
WRAPPER_SHA256="$(lock_value '.agent.wrapper_sha256')"
AGENT_EXEC_SOURCE_SHA256="$(lock_value '.agent.agent_exec_source_sha256')"
AGENT_EXEC_SANDBOX="$(lock_value '.agent.agent_exec_sandbox')"
STACK_LOCK_SHA256="$(sha256_file "${STACK_LOCK}")"
CARGO_LOCK_SHA256="$(sha256_file "${PROJECT_DIR}/Cargo.lock")"
UBUNTU_IMAGE="$(lock_value '.build.ubuntu_amd64_image')"
NODE_IMAGE="$(lock_value '.build.node_amd64_image')"
RUST_IMAGE="$(lock_value '.build.rust_amd64_image')"
UBUNTU_SNAPSHOT="$(lock_value '.build.ubuntu_snapshot')"
AGENT_APT_LOCK_SHA256="$(lock_value '.build.agent_apt_lock_sha256')"
JKS_NORMALIZER_SHA256="$(lock_value '.build.jks_normalizer_sha256')"
JKS_NORMALIZER_TEST_SHA256="$(lock_value '.build.jks_normalizer_test_sha256')"
SERVICE_APT_LOCK_SHA256="$(lock_value '.build.service_apt_lock_sha256')"
QWEN_COMMIT="$(lock_value '.agent.qwen_code.commit')"
QWEN_SOURCE_ARCHIVE="$(lock_value '.agent.qwen_code.source_archive')"
QWEN_SOURCE_ARCHIVE_SHA256="$(lock_value '.agent.qwen_code.source_archive_sha256')"
QWEN_PATCH_SHA256="$(lock_value '.agent.qwen_code.patch_sha256')"
QWEN_SOURCE_PATCH_MANIFEST_SHA256="$(lock_value '.agent.qwen_code.source_patch_manifest_sha256')"
DOCKER_CLI_ARCHIVE="$(lock_value '.build.docker_cli_archive')"
DOCKER_CLI_ARCHIVE_SHA256="$(lock_value '.build.docker_cli_archive_sha256')"
GO_ARCHIVE="$(lock_value '.build.go_archive')"
GO_ARCHIVE_SHA256="$(lock_value '.build.go_archive_sha256')"
readonly AGENT_IMAGE RELAY_IMAGE CAPTURE_IMAGE BROKER_IMAGE SERVICE_IMAGE SOURCE_COMMIT
readonly BUILD_INPUTS_MANIFEST_SHA256 RELAY_SOURCE_SHA256 RELAY_SANDBOX CAPTURE_SOURCE_SHA256 CAPTURE_ID
readonly BROKER_POLICY_SHA256 BROKER_SOURCE_SHA256
readonly SETTINGS_SHA256 INSTRUCTIONS_SHA256
readonly SYSTEM_PROMPT_SHA256 DEPLOYMENT_CONTRACT_SHA256 TOOLCHAIN_MANIFEST_SHA256
readonly TOOLCHAIN_VERIFIER_SHA256 TOOLCHAIN_VERIFIER_TEST_SHA256
readonly RUNTIME_CONTRACT_SHA256 RUNTIME_CONTRACT_VERIFIER_SHA256
readonly RUNTIME_CONTRACT_VERIFIER_TEST_SHA256 WRAPPER_CONTRACT_TEST_SHA256
readonly WRAPPER_SHA256 AGENT_EXEC_SOURCE_SHA256 AGENT_EXEC_SANDBOX
readonly STACK_LOCK_SHA256 CARGO_LOCK_SHA256 UBUNTU_IMAGE NODE_IMAGE
readonly RUST_IMAGE UBUNTU_SNAPSHOT AGENT_APT_LOCK_SHA256 SERVICE_APT_LOCK_SHA256
readonly JKS_NORMALIZER_SHA256 JKS_NORMALIZER_TEST_SHA256
readonly QWEN_COMMIT QWEN_SOURCE_ARCHIVE QWEN_SOURCE_ARCHIVE_SHA256 QWEN_PATCH_SHA256
readonly QWEN_SOURCE_PATCH_MANIFEST_SHA256
readonly DOCKER_CLI_ARCHIVE DOCKER_CLI_ARCHIVE_SHA256
readonly GO_ARCHIVE GO_ARCHIVE_SHA256
SOURCE_DATE_EPOCH="$(lock_value '.build.source_date_epoch')"
readonly SOURCE_DATE_EPOCH

BUILD_EXPORT_DIR="$(mktemp -d /tmp/qwen38-agent-service-build.XXXXXX)"
case "${BUILD_EXPORT_DIR}" in
  /tmp/qwen38-agent-service-build.*) ;;
  *) die "Unexpected temporary build-export directory: ${BUILD_EXPORT_DIR}" ;;
esac
readonly BUILD_EXPORT_DIR
cleanup_build_export() {
  rm -rf -- "${BUILD_EXPORT_DIR}"
}
trap cleanup_build_export EXIT
AGENT_ARCHIVE="${BUILD_EXPORT_DIR}/agent.tar"
RELAY_ARCHIVE="${BUILD_EXPORT_DIR}/relay.tar"
CAPTURE_ARCHIVE="${BUILD_EXPORT_DIR}/capture.tar"
BROKER_ARCHIVE="${BUILD_EXPORT_DIR}/broker.tar"
SERVICE_ARCHIVE="${BUILD_EXPORT_DIR}/service.tar"
readonly AGENT_ARCHIVE RELAY_ARCHIVE CAPTURE_ARCHIVE BROKER_ARCHIVE SERVICE_ARCHIVE

printf 'Building the one pinned agent image (clean Qwen source + reviewed patch + contract tests)...\n'
docker buildx build \
  --builder default \
  --platform linux/amd64 \
  --provenance=false \
  --pull=false \
  --no-cache \
  --target agent \
  --output "type=docker,dest=${AGENT_ARCHIVE},name=${AGENT_IMAGE},rewrite-timestamp=true" \
  --build-arg "SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH}" \
  --build-arg "UBUNTU_IMAGE=${UBUNTU_IMAGE}" \
  --build-arg "NODE_IMAGE=${NODE_IMAGE}" \
  --build-arg "RUST_IMAGE=${RUST_IMAGE}" \
  --build-arg "UBUNTU_SNAPSHOT=${UBUNTU_SNAPSHOT}" \
  --build-arg "AGENT_APT_LOCK_SHA256=${AGENT_APT_LOCK_SHA256}" \
  --build-arg "JKS_NORMALIZER_SHA256=${JKS_NORMALIZER_SHA256}" \
  --build-arg "JKS_NORMALIZER_TEST_SHA256=${JKS_NORMALIZER_TEST_SHA256}" \
  --build-arg "QWEN_COMMIT=${QWEN_COMMIT}" \
  --build-arg "QWEN_SOURCE_ARCHIVE=${QWEN_SOURCE_ARCHIVE}" \
  --build-arg "QWEN_SOURCE_ARCHIVE_SHA256=${QWEN_SOURCE_ARCHIVE_SHA256}" \
  --build-arg "QWEN_PATCH_SHA256=${QWEN_PATCH_SHA256}" \
  --build-arg "QWEN_SOURCE_PATCH_MANIFEST_SHA256=${QWEN_SOURCE_PATCH_MANIFEST_SHA256}" \
  --build-arg "GO_ARCHIVE=${GO_ARCHIVE}" \
  --build-arg "GO_ARCHIVE_SHA256=${GO_ARCHIVE_SHA256}" \
  --build-arg "SETTINGS_SHA256=${SETTINGS_SHA256}" \
  --build-arg "INSTRUCTIONS_SHA256=${INSTRUCTIONS_SHA256}" \
  --build-arg "SYSTEM_PROMPT_SHA256=${SYSTEM_PROMPT_SHA256}" \
  --build-arg "DEPLOYMENT_CONTRACT_SHA256=${DEPLOYMENT_CONTRACT_SHA256}" \
  --build-arg "TOOLCHAIN_MANIFEST_SHA256=${TOOLCHAIN_MANIFEST_SHA256}" \
  --build-arg "TOOLCHAIN_VERIFIER_SHA256=${TOOLCHAIN_VERIFIER_SHA256}" \
  --build-arg "TOOLCHAIN_VERIFIER_TEST_SHA256=${TOOLCHAIN_VERIFIER_TEST_SHA256}" \
  --build-arg "RUNTIME_CONTRACT_SHA256=${RUNTIME_CONTRACT_SHA256}" \
  --build-arg "RUNTIME_CONTRACT_VERIFIER_SHA256=${RUNTIME_CONTRACT_VERIFIER_SHA256}" \
  --build-arg "RUNTIME_CONTRACT_VERIFIER_TEST_SHA256=${RUNTIME_CONTRACT_VERIFIER_TEST_SHA256}" \
  --build-arg "WRAPPER_CONTRACT_TEST_SHA256=${WRAPPER_CONTRACT_TEST_SHA256}" \
  --build-arg "WRAPPER_SHA256=${WRAPPER_SHA256}" \
  --build-arg "AGENT_EXEC_SOURCE_SHA256=${AGENT_EXEC_SOURCE_SHA256}" \
  --build-arg "AGENT_EXEC_SANDBOX=${AGENT_EXEC_SANDBOX}" \
  --file "${PROJECT_DIR}/docker/Dockerfile" \
  "${PROJECT_DIR}"
docker load --input "${AGENT_ARCHIVE}"
rm -f -- "${AGENT_ARCHIVE}"
require_equal "agent image ID" "$(image_id "${AGENT_IMAGE}")" "$(lock_value '.agent.image_id')"
require_agent_image_contract

printf 'Building the minimal fixed-purpose relay image...\n'
docker buildx build \
  --builder default \
  --platform linux/amd64 \
  --provenance=false \
  --pull=false \
  --no-cache \
  --target relay \
  --output "type=docker,dest=${RELAY_ARCHIVE},name=${RELAY_IMAGE},rewrite-timestamp=true" \
  --build-arg "SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH}" \
  --build-arg "RUST_IMAGE=${RUST_IMAGE}" \
  --build-arg "RELAY_SOURCE_SHA256=${RELAY_SOURCE_SHA256}" \
  --build-arg "RELAY_SANDBOX=${RELAY_SANDBOX}" \
  --build-arg "CAPTURE_SOURCE_SHA256=${CAPTURE_SOURCE_SHA256}" \
  --build-arg "BROKER_POLICY_SHA256=${BROKER_POLICY_SHA256}" \
  --build-arg "BROKER_SOURCE_SHA256=${BROKER_SOURCE_SHA256}" \
  --file "${PROJECT_DIR}/docker/Dockerfile" \
  "${PROJECT_DIR}"
docker load --input "${RELAY_ARCHIVE}"
rm -f -- "${RELAY_ARCHIVE}"
require_relay_image_contract

printf 'Building the minimal trusted session-capture image...\n'
docker buildx build \
  --builder default \
  --platform linux/amd64 \
  --provenance=false \
  --pull=false \
  --no-cache \
  --target session-capture \
  --output "type=docker,dest=${CAPTURE_ARCHIVE},name=${CAPTURE_IMAGE},rewrite-timestamp=true" \
  --build-arg "SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH}" \
  --build-arg "RUST_IMAGE=${RUST_IMAGE}" \
  --build-arg "RELAY_SOURCE_SHA256=${RELAY_SOURCE_SHA256}" \
  --build-arg "CAPTURE_SOURCE_SHA256=${CAPTURE_SOURCE_SHA256}" \
  --build-arg "BROKER_POLICY_SHA256=${BROKER_POLICY_SHA256}" \
  --build-arg "BROKER_SOURCE_SHA256=${BROKER_SOURCE_SHA256}" \
  --build-arg "CAPTURE_ID=${CAPTURE_ID}" \
  --file "${PROJECT_DIR}/docker/Dockerfile" \
  "${PROJECT_DIR}"
docker load --input "${CAPTURE_ARCHIVE}"
rm -f -- "${CAPTURE_ARCHIVE}"
require_capture_image_contract

printf 'Building the no-network typed Docker broker image...\n'
docker buildx build \
  --builder default \
  --platform linux/amd64 \
  --provenance=false \
  --pull=false \
  --no-cache \
  --target broker \
  --output "type=docker,dest=${BROKER_ARCHIVE},name=${BROKER_IMAGE},rewrite-timestamp=true" \
  --build-arg "SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH}" \
  --build-arg "UBUNTU_IMAGE=${UBUNTU_IMAGE}" \
  --build-arg "RUST_IMAGE=${RUST_IMAGE}" \
  --build-arg "UBUNTU_SNAPSHOT=${UBUNTU_SNAPSHOT}" \
  --build-arg "SERVICE_APT_LOCK_SHA256=${SERVICE_APT_LOCK_SHA256}" \
  --build-arg "DOCKER_CLI_ARCHIVE=${DOCKER_CLI_ARCHIVE}" \
  --build-arg "DOCKER_CLI_ARCHIVE_SHA256=${DOCKER_CLI_ARCHIVE_SHA256}" \
  --build-arg "BROKER_POLICY_SHA256=${BROKER_POLICY_SHA256}" \
  --build-arg "BROKER_SOURCE_SHA256=${BROKER_SOURCE_SHA256}" \
  --build-arg "RELAY_SOURCE_SHA256=${RELAY_SOURCE_SHA256}" \
  --build-arg "CAPTURE_SOURCE_SHA256=${CAPTURE_SOURCE_SHA256}" \
  --file "${PROJECT_DIR}/docker/Dockerfile" \
  "${PROJECT_DIR}"
docker load --input "${BROKER_ARCHIVE}"
rm -f -- "${BROKER_ARCHIVE}"
require_broker_image_contract

printf 'Building the pinned Docker-only service image from committed source %s...\n' "${SOURCE_COMMIT}"
docker buildx build \
  --builder default \
  --platform linux/amd64 \
  --provenance=false \
  --pull=false \
  --no-cache \
  --target service \
  --output "type=docker,dest=${SERVICE_ARCHIVE},name=${SERVICE_IMAGE},rewrite-timestamp=true" \
  --build-arg "SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH}" \
  --build-arg "UBUNTU_IMAGE=${UBUNTU_IMAGE}" \
  --build-arg "RUST_IMAGE=${RUST_IMAGE}" \
  --build-arg "UBUNTU_SNAPSHOT=${UBUNTU_SNAPSHOT}" \
  --build-arg "SERVICE_APT_LOCK_SHA256=${SERVICE_APT_LOCK_SHA256}" \
  --build-arg "DOCKER_CLI_ARCHIVE=${DOCKER_CLI_ARCHIVE}" \
  --build-arg "DOCKER_CLI_ARCHIVE_SHA256=${DOCKER_CLI_ARCHIVE_SHA256}" \
  --build-arg "SOURCE_COMMIT=${SOURCE_COMMIT}" \
  --build-arg "BUILD_INPUTS_MANIFEST_SHA256=${BUILD_INPUTS_MANIFEST_SHA256}" \
  --build-arg "STACK_LOCK_SHA256=${STACK_LOCK_SHA256}" \
  --build-arg "CARGO_LOCK_SHA256=${CARGO_LOCK_SHA256}" \
  --build-arg "RELAY_SOURCE_SHA256=${RELAY_SOURCE_SHA256}" \
  --build-arg "CAPTURE_SOURCE_SHA256=${CAPTURE_SOURCE_SHA256}" \
  --build-arg "BROKER_POLICY_SHA256=${BROKER_POLICY_SHA256}" \
  --build-arg "BROKER_SOURCE_SHA256=${BROKER_SOURCE_SHA256}" \
  --file "${PROJECT_DIR}/docker/Dockerfile" \
  "${PROJECT_DIR}"
docker load --input "${SERVICE_ARCHIVE}"
rm -f -- "${SERVICE_ARCHIVE}"

require_service_image_contract
printf 'Build complete. Agent=%s Relay=%s Capture=%s Broker=%s Service=%s\n' \
  "$(image_id "${AGENT_IMAGE}")" "$(image_id "${RELAY_IMAGE}")" \
  "$(image_id "${CAPTURE_IMAGE}")" \
  "$(image_id "${BROKER_IMAGE}")" "$(image_id "${SERVICE_IMAGE}")"
