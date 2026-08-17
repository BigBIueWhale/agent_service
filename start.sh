#!/usr/bin/env bash
set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/common.sh
source "${SCRIPT_DIR}/scripts/common.sh"
require_no_arguments "./start.sh" "$@"
check_host_tools_and_versions
check_pinned_inputs
require_release_commit
require_clean_committed_repository

PROFILE="$(lock_value '.profile')"
SERVICE_NAME="$(lock_value '.service.container_name')"
SERVICE_IMAGE_ID="$(release_value '.images.service')"
BROKER_NAME="$(lock_value '.broker.container_name')"
BROKER_IMAGE_ID="$(lock_value '.broker.image_id')"
RELAY_IMAGE_ID="$(lock_value '.relay.image_id')"
CAPTURE_IMAGE_ID="$(lock_value '.capture.image_id')"
SERVICE_BRIDGE_NAME="$(lock_value '.relay.service_bridge_container')"
SERVICE_INGRESS_NAME="$(lock_value '.relay.service_ingress_container')"
BACKEND_NAME="$(lock_value '.backend.container_name')"
BACKEND_DIR="$(lock_value '.backend.project_dir')"
RUNTIME_ROOT="$(lock_value '.service.runtime_root')"
STATE_DIR="$(lock_value '.service.state_dir')"
RESULTS_DIR="$(lock_value '.service.results_dir')"
CONTROL_DIR="$(dirname -- "$(lock_value '.broker.socket_path')")"
BROKER_SOCKET="$(lock_value '.broker.socket_path')"
MODEL_SOCKET_DIR="$(lock_value '.relay.model_socket_dir')"
SERVICE_SOCKET_DIR="$(lock_value '.relay.service_socket_dir')"
SERVICE_SOCKET="${SERVICE_SOCKET_DIR}/relay.sock"
DOCKER_SOCKET="$(lock_value '.host.docker_socket')"
readonly PROFILE SERVICE_NAME SERVICE_IMAGE_ID BROKER_NAME BROKER_IMAGE_ID RELAY_IMAGE_ID CAPTURE_IMAGE_ID
readonly SERVICE_BRIDGE_NAME SERVICE_INGRESS_NAME BACKEND_NAME BACKEND_DIR RUNTIME_ROOT
readonly STATE_DIR RESULTS_DIR CONTROL_DIR BROKER_SOCKET MODEL_SOCKET_DIR SERVICE_SOCKET_DIR
readonly SERVICE_SOCKET DOCKER_SOCKET

require_equal "service image release/tag identity" \
  "$(image_id "$(lock_value '.service.image_tag')")" "${SERVICE_IMAGE_ID}"
require_equal "broker image release/stack identity" \
  "${BROKER_IMAGE_ID}" "$(release_value '.images.broker')"
require_equal "relay image release/stack identity" \
  "${RELAY_IMAGE_ID}" "$(release_value '.images.relay')"
require_equal "session-capture image release/stack identity" \
  "${CAPTURE_IMAGE_ID}" "$(release_value '.images.capture')"
require_equal "agent image ID" \
  "$(image_id "$(lock_value '.agent.image_tag')")" "$(lock_value '.agent.image_id')"
require_agent_image_contract
require_relay_image_contract
require_capture_image_contract
require_broker_image_contract
require_service_image_contract

for name in "${SERVICE_NAME}" "${BROKER_NAME}" "${SERVICE_BRIDGE_NAME}" "${SERVICE_INGRESS_NAME}"; do
  if component_container_exists "${name}"; then
    die "Stack component ${name} already exists. Run ./status.sh for evidence or ./stop.sh for ownership-checked teardown; start never replaces it."
  fi
done
for socket in "${BROKER_SOCKET}" "${SERVICE_SOCKET}"; do
  [[ ! -e "${socket}" ]] || die "Refusing to replace pre-existing runtime socket/path: ${socket}" "Run ./stop.sh for ownership-checked teardown."
done
[[ -z "$(ss -H -ltn 'sport = :8090')" ]] || \
  die "TCP port 8090 is already occupied; the unknown listener was not modified." \
    "$(ss -H -ltnp 'sport = :8090')"

if [[ -e "${RUNTIME_ROOT}" ]]; then
  [[ -d "${RUNTIME_ROOT}" && ! -L "${RUNTIME_ROOT}" ]] || \
    die "Runtime root is not a real directory: ${RUNTIME_ROOT}"
  require_equal "runtime root owner" "$(stat -c '%u:%g' "${RUNTIME_ROOT}")" 1000:1000
else
  install -d -m 0700 "${RUNTIME_ROOT}"
fi
assert_runtime_directory "${RUNTIME_ROOT}" 700
for runtime_directory in "${STATE_DIR}" "${RESULTS_DIR}" "${CONTROL_DIR}" "${SERVICE_SOCKET_DIR}"; do
  if [[ -e "${runtime_directory}" ]]; then
    assert_runtime_directory "${runtime_directory}" 700
  else
    install -d -m 0700 "${runtime_directory}"
    assert_runtime_directory "${runtime_directory}" 700
  fi
done

backend_started=false
broker_created=false
service_created=false
service_bridge_created=false
service_ingress_created=false
cleanup_required=true
cleanup_failed_start() {
  local status="$?"
  local cleanup_failures=0 session_objects session_evidence
  local preserve_recovery_dependencies=false
  trap - EXIT
  if [[ "${cleanup_required}" == true ]]; then
    printf 'Startup failed; cleaning only exact components created by this attempt.\n' >&2
    if [[ "${service_ingress_created}" == true ]] && \
       ! remove_owned_component_if_exact "${SERVICE_INGRESS_NAME}" service-ingress "${RELAY_IMAGE_ID}"; then
      printf 'CLEANUP FAILURE: service ingress was not removed.\n' >&2
      cleanup_failures=$((cleanup_failures + 1))
    fi
    if [[ "${service_bridge_created}" == true ]] && \
       ! remove_owned_component_if_exact "${SERVICE_BRIDGE_NAME}" service-bridge "${RELAY_IMAGE_ID}"; then
      printf 'CLEANUP FAILURE: service bridge was not removed.\n' >&2
      cleanup_failures=$((cleanup_failures + 1))
    fi
    if [[ "${service_created}" == true ]] && \
       ! remove_owned_component_if_exact "${SERVICE_NAME}" service "${SERVICE_IMAGE_ID}"; then
      printf 'CLEANUP FAILURE: service container was not removed.\n' >&2
      cleanup_failures=$((cleanup_failures + 1))
    fi
    if ! session_objects="$({
      docker ps -aq --no-trunc \
        --filter "label=agent_service.profile=${PROFILE}" \
        --filter label=agent_service.session
    } 2>&1)"; then
      printf 'CLEANUP FAILURE: exact-owned session-container absence could not be proved: %s\n' \
        "${session_objects}" >&2
      cleanup_failures=$((cleanup_failures + 1))
      preserve_recovery_dependencies=true
    elif [[ -n "${session_objects}" ]]; then
      session_evidence="$(
        docker ps -a --no-trunc \
          --filter "label=agent_service.profile=${PROFILE}" \
          --filter label=agent_service.session \
          --format '{{.ID}} {{.Names}} {{.Image}} {{.Status}}' 2>&1 || true
      )"
      printf 'CLEANUP FAILURE: temporary session containers remain; preserving broker/backend recovery dependencies: %s\n' \
        "${session_evidence:-${session_objects}}" >&2
      cleanup_failures=$((cleanup_failures + 1))
      preserve_recovery_dependencies=true
    fi
    if [[ "${broker_created}" == true && "${preserve_recovery_dependencies}" == false ]] && \
       ! remove_owned_component_if_exact "${BROKER_NAME}" docker-broker "${BROKER_IMAGE_ID}"; then
      printf 'CLEANUP FAILURE: Docker broker was not removed.\n' >&2
      cleanup_failures=$((cleanup_failures + 1))
    fi
    if [[ -e "${SERVICE_SOCKET}" ]]; then
      if [[ -S "${SERVICE_SOCKET}" && "$(stat -c '%u:%g:%a' "${SERVICE_SOCKET}")" == 1000:1000:660 ]]; then
        if ! rm -- "${SERVICE_SOCKET}"; then
          printf 'CLEANUP FAILURE: service socket was not removed.\n' >&2
          cleanup_failures=$((cleanup_failures + 1))
        fi
      else
        printf 'CLEANUP FAILURE: unrecognized service socket/path was preserved: %s\n' "${SERVICE_SOCKET}" >&2
        cleanup_failures=$((cleanup_failures + 1))
      fi
    fi
    if [[ -e "${BROKER_SOCKET}" && "${preserve_recovery_dependencies}" == false ]]; then
      if [[ -S "${BROKER_SOCKET}" && "$(stat -c '%u:%g:%a' "${BROKER_SOCKET}")" == 1000:984:660 ]]; then
        if ! rm -- "${BROKER_SOCKET}"; then
          printf 'CLEANUP FAILURE: broker socket was not removed.\n' >&2
          cleanup_failures=$((cleanup_failures + 1))
        fi
      else
        printf 'CLEANUP FAILURE: unrecognized broker socket/path was preserved: %s\n' "${BROKER_SOCKET}" >&2
        cleanup_failures=$((cleanup_failures + 1))
      fi
    fi
    if [[ "${backend_started}" == true && "${preserve_recovery_dependencies}" == false ]] && \
       ! "${BACKEND_DIR}/stop.sh"; then
      printf 'CLEANUP FAILURE: the backend started by this attempt was not completely stopped.\n' >&2
      cleanup_failures=$((cleanup_failures + 1))
    fi
    printf 'Failed-start cleanup completed with %d diagnostic failure(s).\n' "${cleanup_failures}" >&2
  fi
  exit "${status}"
}
trap cleanup_failed_start EXIT

if ! component_container_exists "${BACKEND_NAME}"; then
  printf 'Starting the sole pinned network-none vLLM backend and fixed model ingress...\n'
  "${BACKEND_DIR}/start.sh"
  backend_started=true
else
  "${BACKEND_DIR}/status.sh"
fi
require_loopback_listener 8000
assert_runtime_directory "${MODEL_SOCKET_DIR}" 700
assert_socket_contract "${MODEL_SOCKET_DIR}/relay.sock" 1000:1000:660

printf 'Starting the no-network typed Docker session broker...\n'
docker run --detach \
  --name "${BROKER_NAME}" \
  --label "agent_service.profile=${PROFILE}" \
  --label agent_service.component=docker-broker \
  --network none \
  --restart no \
  --user 1000:984 \
  --read-only \
  --cap-drop ALL \
  --security-opt no-new-privileges:true \
  --memory "$(lock_value '.broker.memory')" \
  --memory-swap "$(lock_value '.broker.memory_swap')" \
  --pids-limit "$(lock_value '.broker.pids_limit')" \
  --mount "type=bind,src=${DOCKER_SOCKET},dst=${DOCKER_SOCKET},readonly" \
  --mount "type=bind,src=${CONTROL_DIR},dst=${CONTROL_DIR}" \
  --mount "type=bind,src=${STATE_DIR},dst=${STATE_DIR},readonly" \
  "${BROKER_IMAGE_ID}" >/dev/null
broker_created=true
wait_for_container_event "${BROKER_NAME}" \
  "BROKER_READY policy=$(lock_value '.broker.policy_id') socket=${BROKER_SOCKET}" 30
assert_socket_contract "${BROKER_SOCKET}" 1000:984:660

printf 'Starting the complex agent service with network=none and no Docker socket...\n'
docker run --detach \
  --name "${SERVICE_NAME}" \
  --label "agent_service.profile=${PROFILE}" \
  --label agent_service.component=service \
  --network none \
  --restart no \
  --user "$(lock_value '.service.user')" \
  --read-only \
  --tmpfs "/tmp:$(lock_value '.service.tmpfs_tmp')" \
  --cap-drop ALL \
  --security-opt no-new-privileges:true \
  --memory "$(lock_value '.service.memory')" \
  --memory-swap "$(lock_value '.service.memory_swap')" \
  --pids-limit "$(lock_value '.service.pids_limit')" \
  --mount "type=bind,src=${STATE_DIR},dst=${STATE_DIR}" \
  --mount "type=bind,src=${RESULTS_DIR},dst=${RESULTS_DIR}" \
  --mount "type=bind,src=${CONTROL_DIR},dst=${CONTROL_DIR},readonly" \
  --mount "type=bind,src=${MODEL_SOCKET_DIR},dst=${MODEL_SOCKET_DIR},readonly" \
  "${SERVICE_IMAGE_ID}" >/dev/null
service_created=true
wait_for_container_event "${SERVICE_NAME}" \
  "SERVICE_READY profile=${PROFILE} listen=127.0.0.1:8090 network=none" 120

service_id="$(docker inspect --format '{{.Id}}' "${SERVICE_NAME}")"
printf 'Starting the service-namespace bridge that owns the central service socket...\n'
docker run --detach \
  --name "${SERVICE_BRIDGE_NAME}" \
  --label "agent_service.profile=${PROFILE}" \
  --label agent_service.component=service-bridge \
  --network "container:${service_id}" \
  --restart no \
  --user 1000:1000 \
  --read-only \
  --cap-drop ALL \
  --security-opt no-new-privileges:true \
  --memory "$(lock_value '.relay.memory')" \
  --memory-swap "$(lock_value '.relay.memory_swap')" \
  --pids-limit "$(lock_value '.relay.pids_limit')" \
  --mount "type=bind,src=${SERVICE_SOCKET_DIR},dst=/sock" \
  "${RELAY_IMAGE_ID}" service-bridge >/dev/null
service_bridge_created=true
wait_for_container_event "${SERVICE_BRIDGE_NAME}" \
  "RELAY_READY role=service-bridge sandbox=$(lock_value '.relay.sandbox') listen=unix:/sock/relay.sock target=tcp:127.0.0.1:8090" 30
assert_socket_contract "${SERVICE_SOCKET}" 1000:1000:660

printf 'Starting the sole fixed host-loopback ingress for 127.0.0.1:8090...\n'
docker run --detach \
  --name "${SERVICE_INGRESS_NAME}" \
  --label "agent_service.profile=${PROFILE}" \
  --label agent_service.component=service-ingress \
  --network host \
  --restart no \
  --user 1000:1000 \
  --read-only \
  --cap-drop ALL \
  --security-opt no-new-privileges:true \
  --memory "$(lock_value '.relay.memory')" \
  --memory-swap "$(lock_value '.relay.memory_swap')" \
  --pids-limit "$(lock_value '.relay.pids_limit')" \
  --mount "type=bind,src=${SERVICE_SOCKET_DIR},dst=/sock,readonly" \
  "${RELAY_IMAGE_ID}" service-ingress >/dev/null
service_ingress_created=true
wait_for_container_event "${SERVICE_INGRESS_NAME}" \
  "RELAY_READY role=service-ingress sandbox=$(lock_value '.relay.sandbox') listen=tcp:127.0.0.1:8090 target=unix:/sock/relay.sock" 30
require_loopback_listener 8090
require_equal "service health response" \
  "$(curl --fail --silent --show-error --max-time 10 http://127.0.0.1:8090/healthz)" ok

"${PROJECT_DIR}/status.sh"
cleanup_required=false
trap - EXIT
