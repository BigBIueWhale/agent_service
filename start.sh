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

for _ in $(seq 1 120); do
  if curl --fail --silent --show-error --max-time 2 http://127.0.0.1:8090/healthz >/dev/null 2>&1; then
    "${PROJECT_DIR}/status.sh"
    exit 0
  fi
  if ! service_running="$(docker inspect --format '{{.State.Running}}' "${SERVICE_NAME}")"; then
    die "Could not inspect service container while awaiting readiness"
  fi
  if [[ "${service_running}" != true ]]; then
    if ! docker logs --tail 300 "${SERVICE_NAME}" >&2; then
      printf 'Additionally failed to read service logs.\n' >&2
    fi
    die "Service container exited before health readiness"
  fi
  sleep 1
done
if ! docker logs --tail 300 "${SERVICE_NAME}" >&2; then
  printf 'Additionally failed to read service logs.\n' >&2
fi
die "Service did not become healthy within 120 seconds"
