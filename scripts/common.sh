#!/usr/bin/env bash
set -Eeuo pipefail

PROJECT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
readonly PROJECT_DIR
readonly STACK_LOCK="${PROJECT_DIR}/config/stack.lock.json"

die() {
  printf 'ERROR: %s\n' "$*" >&2
  exit 1
}

require_no_arguments() {
  local usage="$1"
  shift
  [[ "$#" == 0 ]] || die "No arguments are supported. Usage: ${usage}"
}

lock_value() {
  local filter="$1"
  jq -er "${filter}" "${STACK_LOCK}" || die "Missing/invalid stack lock field: ${filter}"
}

sha256_file() {
  sha256sum -- "$1" | awk '{print $1}'
}

require_equal() {
  local role="$1" actual="$2" expected="$3"
  [[ "${actual}" == "${expected}" ]] || \
    die "${role} drift: expected '${expected}', observed '${actual}'"
}

check_host_tools_and_versions() {
  local tool
  for tool in docker git jq sha256sum nvidia-smi nvidia-container-cli curl ss; do
    command -v "${tool}" >/dev/null 2>&1 || die "Required host diagnostic/control tool is missing: ${tool}"
  done
  jq -e . "${STACK_LOCK}" >/dev/null || die "Stack lock is not valid JSON"
  require_equal "Docker server version" \
    "$(docker version --format '{{.Server.Version}}')" \
    "$(lock_value '.host.docker_version')"
  require_equal "Docker Buildx version" \
    "$(docker buildx version | awk '{print $2}')" \
    "$(lock_value '.host.docker_buildx_version')"
  require_equal "BuildKit version" \
    "$(docker buildx inspect --bootstrap | awk -F': *' '/BuildKit version:/ {print $2; exit}')" \
    "$(lock_value '.host.buildkit_version')"
  require_equal "Git version" "$(git --version | awk '{print $3}')" "$(lock_value '.host.git_version')"
  require_equal "jq version" "$(jq --version)" "$(lock_value '.host.jq_version')"
  require_equal "coreutils version" \
    "$(sha256sum --version | awk 'NR==1 {print $NF}')" \
    "$(lock_value '.host.coreutils_version')"
  require_equal "NVIDIA container CLI version" \
    "$(nvidia-container-cli --version 2>&1 | awk -F': ' '/cli-version:/ {print $2; exit}')" \
    "$(lock_value '.host.nvidia_container_cli_version')"

  local gpu_line gpu_name gpu_memory gpu_driver
  gpu_line="$(nvidia-smi --query-gpu=name,memory.total,driver_version --format=csv,noheader,nounits)"
  [[ "$(wc -l <<<"${gpu_line}")" == 1 ]] || die "Exactly one GPU is required; observed: ${gpu_line}"
  IFS=',' read -r gpu_name gpu_memory gpu_driver <<<"${gpu_line}"
  gpu_name="${gpu_name#"${gpu_name%%[![:space:]]*}"}"
  gpu_memory="${gpu_memory#"${gpu_memory%%[![:space:]]*}"}"
  gpu_driver="${gpu_driver#"${gpu_driver%%[![:space:]]*}"}"
  require_equal "GPU name" "${gpu_name}" "$(lock_value '.host.gpu_name')"
  require_equal "GPU memory MiB" "${gpu_memory}" "$(lock_value '.host.gpu_memory_mib | tostring')"
  require_equal "NVIDIA driver" "${gpu_driver}" "$(lock_value '.host.driver_version')"
}

check_pinned_inputs() {
  require_equal "agent apt lock SHA256" \
    "$(sha256_file "${PROJECT_DIR}/config/agent-apt-packages.lock")" \
    "$(lock_value '.build.agent_apt_lock_sha256')"
  require_equal "service apt lock SHA256" \
    "$(sha256_file "${PROJECT_DIR}/config/service-apt-packages.lock")" \
    "$(lock_value '.build.service_apt_lock_sha256')"
  require_equal "Qwen patch SHA256" \
    "$(sha256_file "${PROJECT_DIR}/patches/qwen-code-0.21.12-agent-service.patch")" \
    "$(lock_value '.agent.qwen_code.patch_sha256')"
  require_equal "Qwen source patch manifest SHA256" \
    "$(sha256_file "${PROJECT_DIR}/patches/source_patch_v1/manifest.sha256")" \
    "$(lock_value '.agent.qwen_code.source_patch_manifest_sha256')"
  (
    cd "${PROJECT_DIR}"
    sha256sum --check --strict patches/source_patch_v1/manifest.sha256
  ) || die "Qwen source transformer manifest validation failed"
  require_equal "Qwen settings SHA256" \
    "$(sha256_file "${PROJECT_DIR}/docker/config/settings.json")" \
    "$(lock_value '.agent.settings_sha256')"
  require_equal "Qwen instructions SHA256" \
    "$(sha256_file "${PROJECT_DIR}/docker/config/QWEN.md")" \
    "$(lock_value '.agent.instructions_sha256')"
  require_equal "agent wrapper SHA256" \
    "$(sha256_file "${PROJECT_DIR}/docker/config/run_agent.sh")" \
    "$(lock_value '.agent.wrapper_sha256')"
  bash -n "${PROJECT_DIR}/docker/config/run_agent.sh" || die "Agent wrapper shell syntax is invalid"
}

require_clean_repository() {
  [[ -z "$(git -C "${PROJECT_DIR}" status --porcelain=v1)" ]] || \
    die "Repository is dirty. Commit the exact intended build inputs before operating the stack."
  git -C "${PROJECT_DIR}" diff --quiet --exit-code || die "Tracked worktree differs from HEAD"
  git -C "${PROJECT_DIR}" diff --cached --quiet --exit-code || die "Index differs from HEAD"
}

container_exists() {
  docker container inspect "$1" >/dev/null 2>&1
}

image_id() {
  docker image inspect --format '{{.Id}}' "$1" 2>/dev/null || true
}

image_label() {
  local image="$1" label="$2"
  docker image inspect --format "{{index .Config.Labels \"${label}\"}}" "${image}" || \
    die "Could not inspect label ${label} on image ${image}"
}

require_agent_image_contract() {
  local image
  image="$(lock_value '.agent.image_tag')"
  require_equal "agent image profile label" \
    "$(image_label "${image}" agent_service.profile)" "$(lock_value '.profile')"
  require_equal "agent image Qwen version label" \
    "$(image_label "${image}" agent_service.qwen.version)" "$(lock_value '.agent.qwen_code.version')"
  require_equal "agent image Qwen commit label" \
    "$(image_label "${image}" agent_service.qwen.commit)" "$(lock_value '.agent.qwen_code.commit')"
  require_equal "agent image Qwen archive label" \
    "$(image_label "${image}" agent_service.qwen.archive.sha256)" "$(lock_value '.agent.qwen_code.source_archive_sha256')"
  require_equal "agent image Qwen patch label" \
    "$(image_label "${image}" agent_service.qwen.patch.sha256)" "$(lock_value '.agent.qwen_code.patch_sha256')"
  require_equal "agent image Qwen source patch manifest label" \
    "$(image_label "${image}" agent_service.qwen.source-patch-manifest.sha256)" \
    "$(lock_value '.agent.qwen_code.source_patch_manifest_sha256')"
  require_equal "agent image settings label" \
    "$(image_label "${image}" agent_service.settings.sha256)" "$(lock_value '.agent.settings_sha256')"
  require_equal "agent image instructions label" \
    "$(image_label "${image}" agent_service.instructions.sha256)" "$(lock_value '.agent.instructions_sha256')"
  require_equal "agent image wrapper label" \
    "$(image_label "${image}" agent_service.wrapper.sha256)" "$(lock_value '.agent.wrapper_sha256')"
}

require_service_image_contract() {
  local image
  image="$(lock_value '.service.image_tag')"
  require_equal "service image profile label" \
    "$(image_label "${image}" agent_service.profile)" "$(lock_value '.profile')"
  require_equal "service source label" \
    "$(image_label "${image}" agent_service.source.commit)" "$(git -C "${PROJECT_DIR}" rev-parse HEAD)"
  require_equal "service stack-lock label" \
    "$(image_label "${image}" agent_service.stack-lock.sha256)" "$(sha256_file "${STACK_LOCK}")"
  require_equal "service Cargo-lock label" \
    "$(image_label "${image}" agent_service.cargo-lock.sha256)" "$(sha256_file "${PROJECT_DIR}/Cargo.lock")"
}

require_loopback_listener() {
  local port="$1" output
  output="$(ss -H -ltn "sport = :${port}")"
  [[ "$(wc -l <<<"${output}")" == 1 && "${output}" == *"127.0.0.1:${port}"* ]] || \
    die "Expected exactly one 127.0.0.1:${port} listener; observed: ${output:-<none>}"
}
