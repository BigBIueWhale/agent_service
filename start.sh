#!/usr/bin/env bash
set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/common.sh
source "${SCRIPT_DIR}/scripts/common.sh"
require_no_arguments "./start.sh" "$@"
check_host_tools_and_versions
check_pinned_inputs
require_clean_repository

SERVICE_NAME="$(lock_value '.service.container_name')"
SERVICE_IMAGE="$(lock_value '.service.image_tag')"
AGENT_IMAGE="$(lock_value '.agent.image_tag')"
BACKEND_NAME="$(lock_value '.backend.container_name')"
BACKEND_DIR="$(lock_value '.backend.project_dir')"
PROFILE="$(lock_value '.profile')"
DOCKER_SOCKET_GID="$(lock_value '.host.docker_socket_gid')"
DOCKER_SOCKET="$(lock_value '.host.docker_socket')"
HOST_INPUT_ROOT="$(lock_value '.service.host_input_root')"
readonly SERVICE_NAME SERVICE_IMAGE AGENT_IMAGE BACKEND_NAME BACKEND_DIR PROFILE
readonly DOCKER_SOCKET_GID DOCKER_SOCKET HOST_INPUT_ROOT

[[ -n "$(image_id "${SERVICE_IMAGE}")" ]] || die "Service image is missing. Run ./build.sh"
require_equal "agent image ID" "$(image_id "${AGENT_IMAGE}")" "$(lock_value '.agent.image_id')"
require_agent_image_contract
require_service_image_contract

if container_exists "${SERVICE_NAME}"; then
  die "Service container ${SERVICE_NAME} already exists. Run ./status.sh or ./stop.sh; no implicit replacement is performed."
fi

if ! container_exists "${BACKEND_NAME}"; then
  printf 'Starting the sole pinned loopback vLLM backend...\n'
  "${BACKEND_DIR}/start.sh"
else
  "${BACKEND_DIR}/status.sh"
fi
require_loopback_listener 8000

install -d -m 0700 "${PROJECT_DIR}/.runtime/state" "${PROJECT_DIR}/.runtime/results"
printf 'Starting Docker-only agent service on 127.0.0.1:8090...\n'
docker run -d \
  --name "${SERVICE_NAME}" \
  --label "agent_service.profile=${PROFILE}" \
  --network host \
  --user 1000:1000 \
  --group-add "${DOCKER_SOCKET_GID}" \
  --read-only \
  --tmpfs /tmp:rw,nosuid,nodev,noexec,size=256m,mode=1777 \
  --cap-drop ALL \
  --security-opt no-new-privileges:true \
  --memory 2g \
  --memory-swap 2g \
  --pids-limit 512 \
  --mount "type=bind,src=${DOCKER_SOCKET},dst=/var/run/docker.sock" \
  --mount "type=bind,src=${HOST_INPUT_ROOT},dst=${HOST_INPUT_ROOT},readonly" \
  --mount "type=bind,src=${PROJECT_DIR}/.runtime,dst=${PROJECT_DIR}/.runtime" \
  "${SERVICE_IMAGE}" >/dev/null

cleanup_failed_service() {
  if ! container_exists "${SERVICE_NAME}"; then
    return
  fi
  local observed_profile
  observed_profile="$(docker inspect --format '{{index .Config.Labels "agent_service.profile"}}' "${SERVICE_NAME}")" || {
    printf 'Refusing failed-start cleanup because container ownership could not be inspected: %s\n' "${SERVICE_NAME}" >&2
    return 1
  }
  if [[ "${observed_profile}" != "${PROFILE}" ]]; then
    printf 'Refusing failed-start cleanup of unowned container %s (profile=%s)\n' \
      "${SERVICE_NAME}" "${observed_profile}" >&2
    return 1
  fi
  docker container rm --force "${SERVICE_NAME}" >/dev/null
}

wait_for_listen_event() {
  local grep_status
  # docker logs --follow blocks in the daemon and wakes on the exact Rust
  # readiness event or container exit. grep closes the stream on the first
  # match; inspect PIPESTATUS for grep itself so Docker's expected SIGPIPE is
  # not mistaken for a failed match.
  set +o pipefail
  timeout --foreground 120s docker logs --follow --since 0s "${SERVICE_NAME}" 2>&1 |
    grep --fixed-strings --max-count=1 'listening (loopback only)' >/dev/null
  grep_status="${PIPESTATUS[1]}"
  set -o pipefail
  return "${grep_status}"
}

if ! wait_for_listen_event; then
  if ! docker logs --tail 300 "${SERVICE_NAME}" >&2; then
    printf 'Additionally failed to read service logs.\n' >&2
  fi
  cleanup_failed_service || true
  die "Service did not emit its exact loopback-listener readiness event within 120 seconds"
fi

if ! curl --fail --silent --show-error --max-time 10 \
  http://127.0.0.1:8090/healthz >/dev/null; then
  docker logs --tail 300 "${SERVICE_NAME}" >&2 || true
  cleanup_failed_service || true
  die "Service emitted readiness but its single health check failed"
fi
if ! "${PROJECT_DIR}/status.sh"; then
  cleanup_failed_service || true
  die "Service emitted readiness but failed the complete live contract"
fi
