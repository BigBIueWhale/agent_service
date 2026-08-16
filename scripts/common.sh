#!/usr/bin/env bash
set -Eeuo pipefail

PROJECT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
readonly PROJECT_DIR
readonly STACK_LOCK="${PROJECT_DIR}/config/stack.lock.json"
readonly RELEASE_LOCK="${PROJECT_DIR}/config/release.lock.json"
readonly BUILD_INPUTS_MANIFEST="${PROJECT_DIR}/config/build-inputs.sha256"
readonly BROKER_POLICY="${PROJECT_DIR}/config/broker-policy-v1.json"

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

release_value() {
  local filter="$1"
  jq -er "${filter}" "${RELEASE_LOCK}" || die "Missing/invalid release lock field: ${filter}"
}

policy_value() {
  local filter="$1"
  jq -er "${filter}" "${BROKER_POLICY}" || die "Missing/invalid broker policy field: ${filter}"
}

validate_release_lock() {
  jq -e '
    type == "object" and
    (keys == ["build_inputs_manifest_sha256", "images", "implementation_commit", "profile", "schema_version", "stack_lock_sha256"]) and
    .schema_version == 1 and
    .profile == "qwen38-agent-service-v3" and
    (.implementation_commit | type == "string" and test("^[0-9a-f]{40}$")) and
    (.build_inputs_manifest_sha256 | type == "string" and test("^[0-9a-f]{64}$")) and
    (.stack_lock_sha256 | type == "string" and test("^[0-9a-f]{64}$")) and
    (.images | type == "object" and (keys == ["agent", "broker", "capture", "relay", "service"])) and
    ([.images[] | type == "string" and test("^sha256:[0-9a-f]{64}$")] | all)
  ' "${RELEASE_LOCK}" >/dev/null || die "Release lock violates its exact schema or one-mode identity contract"
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
  for tool in docker dockerd git jq sha256sum nvidia-smi nvidia-container-cli curl ss; do
    command -v "${tool}" >/dev/null 2>&1 || die "Required host diagnostic/control tool is missing: ${tool}"
  done
  jq -e . "${STACK_LOCK}" >/dev/null || die "Stack lock is not valid JSON"
  jq -e . "${RELEASE_LOCK}" >/dev/null || die "Release lock is not valid JSON"
  require_equal "Docker server version" \
    "$(docker version --format '{{.Server.Version}}')" \
    "$(lock_value '.host.docker_version')"
  require_equal "dockerd path" "$(command -v dockerd)" "$(lock_value '.host.dockerd_path')"
  require_equal "dockerd binary SHA256" \
    "$(sha256_file "$(lock_value '.host.dockerd_path')")" \
    "$(lock_value '.host.dockerd_sha256')"
  require_equal "Docker security options" \
    "$(docker info --format '{{json .SecurityOptions}}')" \
    "$(jq -c '.host.docker_security_options' "${STACK_LOCK}")"
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
  validate_release_lock
  require_equal "release profile" "$(release_value '.profile')" "$(lock_value '.profile')"
  require_equal "release stack-lock SHA256" \
    "$(sha256_file "${STACK_LOCK}")" "$(release_value '.stack_lock_sha256')"
  require_equal "build-input manifest SHA256" \
    "$(sha256_file "${BUILD_INPUTS_MANIFEST}")" \
    "$(release_value '.build_inputs_manifest_sha256')"
  (
    cd "${PROJECT_DIR}"
    sha256sum --check --strict config/build-inputs.sha256
  ) || die "Executable build-input manifest validation failed"
  require_equal "broker policy SHA256" \
    "$(sha256_file "${BROKER_POLICY}")" \
    "$(lock_value '.broker.policy_sha256')"
  require_equal "broker policy schema" "$(policy_value '.schema_version | tostring')" \
    "$(lock_value '.schema_version | tostring')"
  require_equal "broker policy identity" "$(policy_value '.policy_id')" \
    "$(lock_value '.broker.policy_id')"
  require_equal "broker policy profile" "$(policy_value '.profile')" "$(lock_value '.profile')"
  require_equal "broker policy Docker version" "$(policy_value '.docker_server_version')" \
    "$(lock_value '.host.docker_version')"
  require_equal "broker policy container name" "$(policy_value '.broker_container_name')" \
    "$(lock_value '.broker.container_name')"
  require_equal "broker policy image tag" "$(policy_value '.broker.image_tag')" \
    "$(lock_value '.broker.image_tag')"
  require_equal "broker policy memory" "$(policy_value '.broker.memory')" \
    "$(lock_value '.broker.memory')"
  require_equal "broker policy memory-swap" "$(policy_value '.broker.memory_swap')" \
    "$(lock_value '.broker.memory_swap')"
  require_equal "broker policy PID limit" "$(policy_value '.broker.pids_limit | tostring')" \
    "$(lock_value '.broker.pids_limit | tostring')"
  require_equal "broker policy UID" "$(policy_value '.broker.uid | tostring')" 1000
  require_equal "broker policy GID" "$(policy_value '.broker.gid | tostring')" \
    "$(lock_value '.host.docker_socket_gid | tostring')"
  require_equal "broker policy Docker socket" "$(policy_value '.broker.docker_socket')" \
    "$(lock_value '.host.docker_socket')"
  require_equal "broker policy socket path" "$(policy_value '.broker_socket_path')" \
    "$(lock_value '.broker.socket_path')"
  require_equal "broker policy runtime root" "$(policy_value '.runtime_root')" \
    "$(lock_value '.service.runtime_root')"
  require_equal "broker policy state directory" "$(policy_value '.state_dir')" \
    "$(lock_value '.service.state_dir')"
  require_equal "broker policy model socket directory" "$(policy_value '.model_socket_dir')" \
    "$(lock_value '.relay.model_socket_dir')"
  require_equal "broker policy service container" "$(policy_value '.service_container_name')" \
    "$(lock_value '.service.container_name')"
  require_equal "broker policy backend container" "$(policy_value '.backend_container_name')" \
    "$(lock_value '.backend.container_name')"
  require_equal "broker policy backend cache volume" "$(policy_value '.backend_cache_volume')" \
    "$(lock_value '.backend.cache_volume')"
  require_equal "broker policy backend cache mount" "$(policy_value '.backend_cache_mount')" \
    "$(lock_value '.backend.cache_mount')"
  require_equal "broker policy backend cache owner" "$(policy_value '.backend_cache_owner_mode')" \
    "$(lock_value '.backend.cache_owner_mode')"
  require_equal "broker policy model bridge" "$(policy_value '.model_bridge_container_name')" \
    "$(lock_value '.relay.model_bridge_container')"
  require_equal "broker policy model ingress" "$(policy_value '.model_ingress_container_name')" \
    "$(lock_value '.relay.model_ingress_container')"
  require_equal "broker policy agent image tag" "$(policy_value '.agent.image_tag')" \
    "$(lock_value '.agent.image_tag')"
  require_equal "broker policy agent image ID" "$(policy_value '.agent.image_id')" \
    "$(lock_value '.agent.image_id')"
  require_equal "broker policy agent memory" "$(policy_value '.agent.memory')" \
    "$(lock_value '.agent.memory')"
  require_equal "broker policy agent memory-swap" "$(policy_value '.agent.memory_swap')" \
    "$(lock_value '.agent.memory_swap')"
  require_equal "broker policy agent PID limit" "$(policy_value '.agent.pids_limit | tostring')" \
    "$(lock_value '.agent.pids_limit | tostring')"
  require_equal "broker policy agent /tmp" "$(policy_value '.agent.tmpfs_tmp')" \
    "$(lock_value '.agent.tmpfs_tmp')"
  require_equal "broker policy agent runtime" "$(policy_value '.agent.tmpfs_qwen_runtime')" \
    "$(lock_value '.agent.tmpfs_qwen_runtime')"
  require_equal "broker policy agent sandbox" "$(policy_value '.agent.sandbox')" \
    "$(lock_value '.agent.agent_exec_sandbox')"
  require_equal "broker policy agent ready event" "$(policy_value '.agent.ready_event_prefix')" \
    "AGENT_READY model=$(lock_value '.backend.served_model') context=$(lock_value '.backend.max_model_len | tostring') network=loopback-only token_count="
  require_equal "broker policy relay image tag" "$(policy_value '.relay.image_tag')" \
    "$(lock_value '.relay.image_tag')"
  require_equal "broker policy relay image ID" "$(policy_value '.relay.image_id')" \
    "$(lock_value '.relay.image_id')"
  require_equal "broker policy relay sandbox" "$(policy_value '.relay.sandbox')" \
    "$(lock_value '.relay.sandbox')"
  require_equal "broker policy relay memory" "$(policy_value '.relay.memory')" \
    "$(lock_value '.relay.memory')"
  require_equal "broker policy relay memory-swap" "$(policy_value '.relay.memory_swap')" \
    "$(lock_value '.relay.memory_swap')"
  require_equal "broker policy relay PID limit" "$(policy_value '.relay.pids_limit | tostring')" \
    "$(lock_value '.relay.pids_limit | tostring')"
  require_equal "broker policy relay role" "$(policy_value '.relay.role')" agent-model
  require_equal "broker policy capture image tag" "$(policy_value '.capture.image_tag')" \
    "$(lock_value '.capture.image_tag')"
  require_equal "broker policy capture image ID" "$(policy_value '.capture.image_id')" \
    "$(lock_value '.capture.image_id')"
  require_equal "broker policy capture implementation" "$(policy_value '.capture.capture_id')" \
    "$(lock_value '.capture.capture_id')"
  require_equal "broker policy capture memory" "$(policy_value '.capture.memory')" \
    "$(lock_value '.capture.memory')"
  require_equal "broker policy capture memory-swap" "$(policy_value '.capture.memory_swap')" \
    "$(lock_value '.capture.memory_swap')"
  require_equal "broker policy capture PID limit" \
    "$(policy_value '.capture.pids_limit | tostring')" \
    "$(lock_value '.capture.pids_limit | tostring')"
  require_equal "broker policy capture ready event" "$(policy_value '.capture.ready_event')" \
    "CAPTURE_READY capture=$(lock_value '.capture.capture_id') events=/streams/events.sock stderr=/streams/stderr.sock"
  require_equal "broker policy capture complete event" \
    "$(policy_value '.capture.complete_event_prefix')" \
    "CAPTURE_COMPLETE capture=$(lock_value '.capture.capture_id') events_bytes="
  require_equal "broker source SHA256" \
    "$(sha256_file "${PROJECT_DIR}/src/bin/docker_broker.rs")" \
    "$(lock_value '.broker.source_sha256')"
  require_equal "relay source SHA256" \
    "$(sha256_file "${PROJECT_DIR}/src/bin/fixed_relay.rs")" \
    "$(lock_value '.relay.source_sha256')"
  require_equal "session-capture source SHA256" \
    "$(sha256_file "${PROJECT_DIR}/src/bin/session_capture.rs")" \
    "$(lock_value '.capture.source_sha256')"
  require_equal "agent apt lock SHA256" \
    "$(sha256_file "${PROJECT_DIR}/config/agent-apt-packages.lock")" \
    "$(lock_value '.build.agent_apt_lock_sha256')"
  require_equal "JKS normalizer SHA256" \
    "$(sha256_file "${PROJECT_DIR}/docker/scripts/normalize_jks.py")" \
    "$(lock_value '.build.jks_normalizer_sha256')"
  require_equal "JKS normalizer test SHA256" \
    "$(sha256_file "${PROJECT_DIR}/docker/tests/test_normalize_jks.py")" \
    "$(lock_value '.build.jks_normalizer_test_sha256')"
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
  require_equal "Qwen system prompt SHA256" \
    "$(sha256_file "${PROJECT_DIR}/docker/config/system.md")" \
    "$(lock_value '.agent.system_prompt_sha256')"
  require_equal "Qwen deployment contract SHA256" \
    "$(sha256_file "${PROJECT_DIR}/docker/config/deployment-contract.md")" \
    "$(lock_value '.agent.deployment_contract_sha256')"
  require_equal "agent toolchain manifest SHA256" \
    "$(sha256_file "${PROJECT_DIR}/docker/config/toolchain-manifest.json")" \
    "$(lock_value '.agent.toolchain_manifest_sha256')"
  require_equal "agent toolchain verifier SHA256" \
    "$(sha256_file "${PROJECT_DIR}/docker/scripts/verify_toolchain.py")" \
    "$(lock_value '.build.toolchain_verifier_sha256')"
  require_equal "agent toolchain verifier test SHA256" \
    "$(sha256_file "${PROJECT_DIR}/docker/tests/test_verify_toolchain.py")" \
    "$(lock_value '.build.toolchain_verifier_test_sha256')"
  require_equal "agent runtime contract SHA256" \
    "$(sha256_file "${PROJECT_DIR}/config/agent-runtime-contract-v1.json")" \
    "$(lock_value '.agent.runtime_contract_sha256')"
  require_equal "agent runtime contract verifier SHA256" \
    "$(sha256_file "${PROJECT_DIR}/docker/scripts/verify_runtime_contract.py")" \
    "$(lock_value '.build.runtime_contract_verifier_sha256')"
  require_equal "agent runtime contract verifier test SHA256" \
    "$(sha256_file "${PROJECT_DIR}/docker/tests/test_verify_runtime_contract.py")" \
    "$(lock_value '.build.runtime_contract_verifier_test_sha256')"
  require_equal "agent wrapper contract test SHA256" \
    "$(sha256_file "${PROJECT_DIR}/docker/tests/run_agent_signal_contract.sh")" \
    "$(lock_value '.build.wrapper_contract_test_sha256')"
  require_equal "agent wrapper SHA256" \
    "$(sha256_file "${PROJECT_DIR}/docker/config/run_agent.sh")" \
    "$(lock_value '.agent.wrapper_sha256')"
  require_equal "agent_exec source SHA256" \
    "$(sha256_file "${PROJECT_DIR}/src/bin/agent_exec.rs")" \
    "$(lock_value '.agent.agent_exec_source_sha256')"
  bash -n "${PROJECT_DIR}/docker/config/run_agent.sh" || die "Agent wrapper shell syntax is invalid"
}

require_clean_committed_repository() {
  local branch
  [[ -z "$(git -C "${PROJECT_DIR}" status --porcelain=v1 --untracked-files=all)" ]] || \
    die "Repository is dirty. Commit the exact intended build inputs before operating the stack."
  git -C "${PROJECT_DIR}" diff --quiet --exit-code || die "Tracked worktree differs from HEAD"
  git -C "${PROJECT_DIR}" diff --cached --quiet --exit-code || die "Index differs from HEAD"
  branch="$(git -C "${PROJECT_DIR}" symbolic-ref --quiet --short HEAD)" || \
    die "Repository is detached. The only supported release branch is master."
  require_equal "release branch" "${branch}" master
}

require_published_release() {
  local expected_remote actual_fetch_remote actual_push_remote head published
  require_clean_committed_repository
  expected_remote="https://github.com/BigBIueWhale/agent_service"
  actual_fetch_remote="$(git -C "${PROJECT_DIR}" remote get-url origin)" || \
    die "The exact origin fetch remote is unavailable."
  actual_push_remote="$(git -C "${PROJECT_DIR}" remote get-url --push origin)" || \
    die "The exact origin push remote is unavailable."
  require_equal "origin fetch remote" "${actual_fetch_remote}" "${expected_remote}"
  require_equal "origin push remote" "${actual_push_remote}" "${expected_remote}"
  head="$(git -C "${PROJECT_DIR}" rev-parse --verify HEAD)"
  published="$(git -C "${PROJECT_DIR}" ls-remote --exit-code origin refs/heads/master | awk 'NR == 1 {print $1}')" || \
    die "Could not query the exact GitHub master ref for the publication audit."
  [[ "${published}" =~ ^[0-9a-f]{40}$ ]] || \
    die "The queried GitHub master ref is not one exact commit: ${published:-<empty>}"
  require_equal "published GitHub master release" "${published}" "${head}"
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
  require_equal "agent image release identity" \
    "$(lock_value '.agent.image_id')" "$(release_value '.images.agent')"
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
  require_equal "agent image system prompt label" \
    "$(image_label "${image}" agent_service.system-prompt.sha256)" "$(lock_value '.agent.system_prompt_sha256')"
  require_equal "agent image deployment contract label" \
    "$(image_label "${image}" agent_service.deployment-contract.sha256)" "$(lock_value '.agent.deployment_contract_sha256')"
  require_equal "agent image toolchain manifest label" \
    "$(image_label "${image}" agent_service.toolchain-manifest.sha256)" "$(lock_value '.agent.toolchain_manifest_sha256')"
  require_equal "agent image toolchain verifier label" \
    "$(image_label "${image}" agent_service.toolchain-verifier.sha256)" "$(lock_value '.build.toolchain_verifier_sha256')"
  require_equal "agent image runtime contract label" \
    "$(image_label "${image}" agent_service.runtime-contract.sha256)" "$(lock_value '.agent.runtime_contract_sha256')"
  require_equal "agent image runtime contract verifier label" \
    "$(image_label "${image}" agent_service.runtime-contract-verifier.sha256)" "$(lock_value '.build.runtime_contract_verifier_sha256')"
  require_equal "agent image agent_exec source label" \
    "$(image_label "${image}" agent_service.agent-exec.source.sha256)" \
    "$(lock_value '.agent.agent_exec_source_sha256')"
  require_equal "agent image agent_exec sandbox label" \
    "$(image_label "${image}" agent_service.agent-exec.sandbox)" \
    "$(lock_value '.agent.agent_exec_sandbox')"
  require_equal "agent image wrapper label" \
    "$(image_label "${image}" agent_service.wrapper.sha256)" "$(lock_value '.agent.wrapper_sha256')"
}

require_capture_image_contract() {
  local image
  image="$(lock_value '.capture.image_tag')"
  require_equal "session-capture image ID" \
    "$(image_id "${image}")" "$(lock_value '.capture.image_id')"
  require_equal "session-capture release identity" \
    "$(lock_value '.capture.image_id')" "$(release_value '.images.capture')"
  require_equal "session-capture profile label" \
    "$(image_label "${image}" agent_service.profile)" "$(lock_value '.profile')"
  require_equal "session-capture component label" \
    "$(image_label "${image}" agent_service.component)" session-capture
  require_equal "session-capture source label" \
    "$(image_label "${image}" agent_service.capture.source.sha256)" \
    "$(lock_value '.capture.source_sha256')"
  require_equal "session-capture implementation label" \
    "$(image_label "${image}" agent_service.capture.id)" \
    "$(lock_value '.capture.capture_id')"
}

require_relay_image_contract() {
  local image
  image="$(lock_value '.relay.image_tag')"
  require_equal "fixed-relay image ID" "$(image_id "${image}")" "$(lock_value '.relay.image_id')"
  require_equal "fixed-relay release identity" \
    "$(lock_value '.relay.image_id')" "$(release_value '.images.relay')"
  require_equal "fixed-relay profile label" \
    "$(image_label "${image}" agent_service.profile)" "$(lock_value '.profile')"
  require_equal "fixed-relay component label" \
    "$(image_label "${image}" agent_service.component)" fixed-relay
  require_equal "fixed-relay source label" \
    "$(image_label "${image}" agent_service.relay.source.sha256)" \
    "$(lock_value '.relay.source_sha256')"
  require_equal "fixed-relay kernel sandbox label" \
    "$(image_label "${image}" agent_service.relay.sandbox)" \
    "$(lock_value '.relay.sandbox')"
}

require_broker_image_contract() {
  local image
  image="$(lock_value '.broker.image_tag')"
  require_equal "broker image ID" "$(image_id "${image}")" "$(lock_value '.broker.image_id')"
  require_equal "broker release identity" \
    "$(lock_value '.broker.image_id')" "$(release_value '.images.broker')"
  require_equal "broker profile label" \
    "$(image_label "${image}" agent_service.profile)" "$(lock_value '.profile')"
  require_equal "broker component label" \
    "$(image_label "${image}" agent_service.component)" docker-broker
  require_equal "broker policy label" \
    "$(image_label "${image}" agent_service.broker.policy.sha256)" \
    "$(lock_value '.broker.policy_sha256')"
  require_equal "broker source label" \
    "$(image_label "${image}" agent_service.broker.source.sha256)" \
    "$(lock_value '.broker.source_sha256')"
}

require_service_image_contract() {
  local image
  image="$(lock_value '.service.image_tag')"
  require_equal "service image ID" "$(image_id "${image}")" "$(release_value '.images.service')"
  require_equal "service image profile label" \
    "$(image_label "${image}" agent_service.profile)" "$(lock_value '.profile')"
  require_equal "service source label" \
    "$(image_label "${image}" agent_service.source.commit)" \
    "$(release_value '.implementation_commit')"
  require_equal "service build-input manifest label" \
    "$(image_label "${image}" agent_service.build-inputs.sha256)" \
    "$(release_value '.build_inputs_manifest_sha256')"
  require_equal "service stack-lock label" \
    "$(image_label "${image}" agent_service.stack-lock.sha256)" "$(sha256_file "${STACK_LOCK}")"
  require_equal "service Cargo-lock label" \
    "$(image_label "${image}" agent_service.cargo-lock.sha256)" "$(sha256_file "${PROJECT_DIR}/Cargo.lock")"
}

require_release_commit() {
  local commit
  commit="$(release_value '.implementation_commit')"
  [[ "${#commit}" == 40 && "${commit}" =~ ^[0-9a-f]{40}$ ]] || \
    die "Release implementation commit is not a lowercase forty-hex Git object: ${commit}"
  git -C "${PROJECT_DIR}" cat-file -e "${commit}^{commit}" 2>/dev/null || \
    die "Release implementation commit is absent from this repository: ${commit}"
  git -C "${PROJECT_DIR}" merge-base --is-ancestor "${commit}" HEAD || \
    die "Release implementation commit is not an ancestor of the checked-out master history: ${commit}"
}

require_loopback_listener() {
  local port="$1" output
  output="$(ss -H -ltn "sport = :${port}")"
  [[ "$(wc -l <<<"${output}")" == 1 && "${output}" == *"127.0.0.1:${port}"* ]] || \
    die "Expected exactly one 127.0.0.1:${port} listener; observed: ${output:-<none>}"
}

assert_runtime_directory() {
  local path="$1" mode="$2" observed
  [[ -d "${path}" && ! -L "${path}" ]] || \
    die "Required runtime path is not a real directory: ${path}"
  observed="$(stat -c '%u:%g:%a' "${path}")"
  require_equal "runtime directory ${path}" "${observed}" "1000:1000:${mode}"
}

assert_socket_contract() {
  local path="$1" uid_gid_mode="$2"
  [[ -S "${path}" && ! -L "${path}" ]] || die "Required Unix socket is absent or not a real socket: ${path}"
  require_equal "Unix socket ${path}" "$(stat -c '%u:%g:%a' "${path}")" "${uid_gid_mode}"
}

component_container_exists() {
  container_exists "$1"
}

assert_backend_teardown_targets() {
  local backend bridge ingress relay_image model_socket
  local expected_profile expected_image observed_project observed_profile observed_image configured_image
  backend="$(lock_value '.backend.container_name')"
  bridge="$(lock_value '.relay.model_bridge_container')"
  ingress="$(lock_value '.relay.model_ingress_container')"
  relay_image="$(lock_value '.relay.image_id')"
  model_socket="$(lock_value '.relay.model_socket_dir')/relay.sock"
  expected_profile="$(lock_value '.backend.profile_label')"
  expected_image="$(lock_value '.backend.image_id')"

  if component_container_exists "${backend}"; then
    observed_project="$(docker inspect --format '{{index .Config.Labels "qwen38.project"}}' "${backend}")"
    observed_profile="$(docker inspect --format '{{index .Config.Labels "qwen38.runtime.profile"}}' "${backend}")"
    observed_image="$(docker inspect --format '{{.Image}}' "${backend}")"
    configured_image="$(docker inspect --format '{{.Config.Image}}' "${backend}")"
    [[ "${observed_project}" == "$(lock_value '.backend.project_label')" && \
       "${observed_profile}" == "${expected_profile}" && \
       "${observed_image}" == "${expected_image}" && \
       "${configured_image}" == "${expected_image}" ]] || \
      die "Refusing teardown because the backend name is not the exact locked deployment: ${backend}" \
        "Observed project/profile/image/configured-image: ${observed_project:-missing}/${observed_profile:-missing}/${observed_image:-missing}/${configured_image:-missing}"
  fi
  component_container_exists "${bridge}" && \
    assert_owned_component "${bridge}" model-bridge "${relay_image}"
  component_container_exists "${ingress}" && \
    assert_owned_component "${ingress}" model-ingress "${relay_image}"
  if [[ -e "${model_socket}" ]]; then
    assert_socket_contract "${model_socket}" 1000:1000:660
  fi
  if component_container_exists "${ingress}" && \
     [[ "$(docker inspect --format '{{.State.Running}}' "${ingress}")" == true ]]; then
    require_loopback_listener 8000
  elif [[ -n "$(ss -H -ltn 'sport = :8000')" ]]; then
    die "TCP port 8000 is occupied without the exact running model ingress; no teardown was attempted."
  fi
}

assert_owned_component() {
  local name="$1" component="$2" expected_image="$3"
  local profile observed_component image configured_image
  profile="$(docker inspect --format '{{index .Config.Labels "agent_service.profile"}}' "${name}")" || \
    die "Cannot inspect project ownership for container ${name}"
  observed_component="$(docker inspect --format '{{index .Config.Labels "agent_service.component"}}' "${name}")" || \
    die "Cannot inspect component ownership for container ${name}"
  image="$(docker inspect --format '{{.Image}}' "${name}")" || \
    die "Cannot inspect image ownership for container ${name}"
  configured_image="$(docker inspect --format '{{.Config.Image}}' "${name}")" || \
    die "Cannot inspect configured-image ownership for container ${name}"
  [[ "${profile}" == "$(lock_value '.profile')" && \
     "${observed_component}" == "${component}" && \
     "${image}" == "${expected_image}" && \
     "${configured_image}" == "${expected_image}" ]] || \
    die "Refusing to modify unrecognized container ${name}." \
      "Expected profile/component/image/configured-image: $(lock_value '.profile')/${component}/${expected_image}/${expected_image}" \
      "Observed profile/component/image/configured-image: ${profile:-missing}/${observed_component:-missing}/${image:-missing}/${configured_image:-missing}"
}

remove_owned_component_if_exact() {
  local name="$1" component="$2" expected_image="$3"
  component_container_exists "${name}" || return 0
  local profile observed_component image configured_image running
  profile="$(docker inspect --format '{{index .Config.Labels "agent_service.profile"}}' "${name}" 2>/dev/null)" || return 1
  observed_component="$(docker inspect --format '{{index .Config.Labels "agent_service.component"}}' "${name}" 2>/dev/null)" || return 1
  image="$(docker inspect --format '{{.Image}}' "${name}" 2>/dev/null)" || return 1
  configured_image="$(docker inspect --format '{{.Config.Image}}' "${name}" 2>/dev/null)" || return 1
  if [[ "${profile}" != "$(lock_value '.profile')" || \
        "${observed_component}" != "${component}" || \
        "${image}" != "${expected_image}" || \
        "${configured_image}" != "${expected_image}" ]]; then
    printf 'REFUSED cleanup of unrecognized container %s: observed profile/component/image/configured-image=%s/%s/%s/%s\n' \
      "${name}" "${profile:-missing}" "${observed_component:-missing}" "${image:-missing}" "${configured_image:-missing}" >&2
    return 1
  fi
  running="$(docker inspect --format '{{.State.Running}}' "${name}")" || return 1
  if [[ "${running}" == true ]]; then
    docker stop --timeout -1 "${name}" >/dev/null || return 1
  fi
  if [[ "$(docker inspect --format '{{.State.Running}}' "${name}")" != false ]]; then
    printf 'Failed-start component did not reach stopped state: %s\n' "${name}" >&2
    return 1
  fi
  docker rm "${name}" >/dev/null
}

wait_for_container_event() {
  local name="$1" event="$2" seconds="$3"
  local deadline log_fd log_pid line remaining read_status=0 follower_status=0
  local found=false running

  # Every caller has just created a new, uniquely named container. Read its
  # complete log so a fast readiness event emitted before attachment cannot be
  # lost. A Bash timed read is the event-driven deadline: there is no polling.
  # Closing the process-substitution FD and terminating only the exact Docker
  # log follower makes the successful path return immediately even when the
  # ready container produces no subsequent output.
  deadline=$((SECONDS + seconds))
  exec {log_fd}< <(docker logs --follow "${name}" 2>&1)
  log_pid="$!"
  while ((SECONDS < deadline)); do
    remaining=$((deadline - SECONDS))
    if IFS= read -r -t "${remaining}" line <&"${log_fd}"; then
      printf '%s\n' "${line}" >&2
      if [[ "${line}" == "${event}" ]]; then
        found=true
        break
      fi
    else
      read_status="$?"
      break
    fi
  done

  exec {log_fd}<&-
  if kill -0 "${log_pid}" 2>/dev/null; then
    kill "${log_pid}" 2>/dev/null || follower_status="$?"
  fi
  wait "${log_pid}" 2>/dev/null || follower_status="$?"

  if [[ "${found}" == true ]]; then
    running="$(docker inspect --format '{{.State.Running}}' "${name}")" || \
      die "Container ${name} emitted readiness but its running state could not be inspected"
    require_equal "container ${name} post-readiness running state" "${running}" true
    return 0
  fi

  if ! docker inspect --format \
    'Container readiness failure state: name={{.Name}} status={{.State.Status}} running={{.State.Running}} exit={{.State.ExitCode}} oom={{.State.OOMKilled}} error={{json .State.Error}}' \
    "${name}" >&2; then
    printf 'Could not inspect failure state for container %s.\n' "${name}" >&2
  fi
  die "Container ${name} did not emit its exact readiness event within ${seconds}s: ${event}" \
    "Read status=${read_status}; log-follower status=${follower_status}."
}

assert_relay_kernel_sandbox() {
  local name="$1" event="$2" pid status_file
  pid="$(docker inspect --format '{{.State.Pid}}' "${name}")"
  [[ "${pid}" =~ ^[1-9][0-9]*$ ]] || die "Relay ${name} has an invalid host PID: ${pid}"
  status_file="/proc/${pid}/status"
  [[ -r "${status_file}" ]] || die "Cannot inspect kernel sandbox state for relay ${name}: ${status_file}"
  require_equal "${name} kernel no_new_privs" \
    "$(awk '$1 == "NoNewPrivs:" {print $2}' "${status_file}")" 1
  require_equal "${name} kernel seccomp mode" \
    "$(awk '$1 == "Seccomp:" {print $2}' "${status_file}")" 2
  # One filter is Docker's pinned builtin profile; the relay stacks its
  # socket-domain and no-new-bind filters on top of it.
  require_equal "${name} stacked seccomp filter count" \
    "$(awk '$1 == "Seccomp_filters:" {print $2}' "${status_file}")" 3
  require_equal "${name} exact sandbox readiness count" \
    "$(docker logs "${name}" 2>&1 | grep --fixed-strings --line-regexp --count "${event}" || true)" 1
}

remove_owned_socket() {
  local path="$1" uid_gid_mode="$2"
  [[ -e "${path}" ]] || return 0
  assert_socket_contract "${path}" "${uid_gid_mode}"
  rm -- "${path}"
}

memory_bytes() {
  local value="$1"
  numfmt --from=iec "${value^^}" || die "Invalid locked memory value: ${value}"
}

assert_hardened_component_base() {
  local name="$1" component="$2" image="$3" user="$4" network="$5"
  local memory="$6" memory_swap="$7" pids="$8"
  require_equal "${name} running state" "$(docker inspect --format '{{.State.Running}}' "${name}")" true
  require_equal "${name} image ID" "$(docker inspect --format '{{.Image}}' "${name}")" "${image}"
  require_equal "${name} configured immutable image ID" \
    "$(docker inspect --format '{{.Config.Image}}' "${name}")" "${image}"
  require_equal "${name} user" "$(docker inspect --format '{{.Config.User}}' "${name}")" "${user}"
  require_equal "${name} network" \
    "$(docker inspect --format '{{.HostConfig.NetworkMode}}' "${name}")" "${network}"
  require_equal "${name} read-only root" \
    "$(docker inspect --format '{{.HostConfig.ReadonlyRootfs}}' "${name}")" true
  require_equal "${name} privileged flag" \
    "$(docker inspect --format '{{.HostConfig.Privileged}}' "${name}")" false
  require_equal "${name} restart policy" \
    "$(docker inspect --format '{{.HostConfig.RestartPolicy.Name}}' "${name}")" no
  require_equal "${name} memory" \
    "$(docker inspect --format '{{.HostConfig.Memory}}' "${name}")" "$(memory_bytes "${memory}")"
  require_equal "${name} memory+swap" \
    "$(docker inspect --format '{{.HostConfig.MemorySwap}}' "${name}")" "$(memory_bytes "${memory_swap}")"
  require_equal "${name} PID limit" \
    "$(docker inspect --format '{{.HostConfig.PidsLimit}}' "${name}")" "${pids}"
  require_equal "${name} capability drop" \
    "$(docker inspect --format '{{json .HostConfig.CapDrop}}' "${name}")" '["ALL"]'
  require_equal "${name} capability additions" \
    "$(docker inspect --format '{{json .HostConfig.CapAdd}}' "${name}")" null
  require_equal "${name} no-new-privileges" \
    "$(docker inspect --format '{{json .HostConfig.SecurityOpt}}' "${name}")" '["no-new-privileges:true"]'
  require_equal "${name} device list" \
    "$(docker inspect --format '{{json .HostConfig.Devices}}' "${name}")" '[]'
  require_equal "${name} device requests" \
    "$(docker inspect --format '{{json .HostConfig.DeviceRequests}}' "${name}")" null
  require_equal "${name} PID namespace" \
    "$(docker inspect --format '{{.HostConfig.PidMode}}' "${name}")" ''
  require_equal "${name} IPC namespace" \
    "$(docker inspect --format '{{.HostConfig.IpcMode}}' "${name}")" private
  require_equal "${name} UTS namespace" \
    "$(docker inspect --format '{{.HostConfig.UTSMode}}' "${name}")" ''
  require_equal "${name} published-port bindings" \
    "$(docker inspect --format '{{json .HostConfig.PortBindings}}' "${name}")" '{}'
  [[ -z "$(docker port "${name}")" ]] || die "${name} has forbidden Docker-published ports"
  require_equal "${name} profile label" \
    "$(docker inspect --format '{{index .Config.Labels "agent_service.profile"}}' "${name}")" \
    "$(lock_value '.profile')"
  require_equal "${name} component label" \
    "$(docker inspect --format '{{index .Config.Labels "agent_service.component"}}' "${name}")" \
    "${component}"
  require_equal "${name} AppArmor profile" \
    "$(docker inspect --format '{{.AppArmorProfile}}' "${name}")" \
    "$(lock_value '.host.container_apparmor_profile')"
}

assert_network_none_proc() {
  local name="$1" pid route_file ipv6_route_file dev_file interfaces namespace host_namespace
  pid="$(docker inspect --format '{{.State.Pid}}' "${name}")"
  [[ "${pid}" =~ ^[1-9][0-9]*$ ]] || die "${name} has an invalid host PID: ${pid}"
  namespace="$(readlink "/proc/${pid}/ns/net")"
  host_namespace="$(readlink /proc/self/ns/net)"
  [[ -n "${namespace}" && "${namespace}" != "${host_namespace}" ]] || \
    die "${name} does not have a distinct network namespace: component=${namespace:-unreadable} host=${host_namespace:-unreadable}"
  route_file="/proc/${pid}/net/route"
  ipv6_route_file="/proc/${pid}/net/ipv6_route"
  dev_file="/proc/${pid}/net/dev"
  [[ -r "${route_file}" && "$(wc -l < "${route_file}")" == 1 ]] || \
    die "${name} network-none namespace has an unexpected IPv4 route table"
  if [[ -e "${ipv6_route_file}" && ( ! -r "${ipv6_route_file}" || -s "${ipv6_route_file}" ) ]]; then
    die "${name} network-none namespace has an unexpected IPv6 route table"
  fi
  [[ -r "${dev_file}" ]] || die "Cannot inspect ${name} network interfaces: ${dev_file}"
  interfaces="$(awk -F: 'NR > 2 {gsub(/[[:space:]]/, "", $1); if ($1 != "") print $1}' "${dev_file}")"
  require_equal "${name} network-none interfaces" "${interfaces}" lo
}
