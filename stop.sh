#!/usr/bin/env bash
set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/common.sh
source "${SCRIPT_DIR}/scripts/common.sh"
require_no_arguments "./stop.sh" "$@"
check_host_tools_and_versions

PROFILE="$(lock_value '.profile')"
SERVICE_NAME="$(lock_value '.service.container_name')"
SERVICE_IMAGE_ID="$(release_value '.images.service')"
BROKER_NAME="$(lock_value '.broker.container_name')"
BROKER_IMAGE_ID="$(lock_value '.broker.image_id')"
RELAY_IMAGE_ID="$(lock_value '.relay.image_id')"
SERVICE_BRIDGE_NAME="$(lock_value '.relay.service_bridge_container')"
SERVICE_INGRESS_NAME="$(lock_value '.relay.service_ingress_container')"
BACKEND_DIR="$(lock_value '.backend.project_dir')"
BROKER_SOCKET="$(lock_value '.broker.socket_path')"
SERVICE_SOCKET="$(lock_value '.relay.service_socket_dir')/relay.sock"
readonly PROFILE SERVICE_NAME SERVICE_IMAGE_ID BROKER_NAME BROKER_IMAGE_ID RELAY_IMAGE_ID
readonly SERVICE_BRIDGE_NAME SERVICE_INGRESS_NAME BACKEND_DIR BROKER_SOCKET SERVICE_SOCKET

# Resolve every name before changing anything. One collision aborts the whole
# teardown, so a partially removed project cannot be created by an ownership
# mistake.
component_container_exists "${SERVICE_INGRESS_NAME}" && \
  assert_owned_component "${SERVICE_INGRESS_NAME}" service-ingress "${RELAY_IMAGE_ID}"
component_container_exists "${SERVICE_BRIDGE_NAME}" && \
  assert_owned_component "${SERVICE_BRIDGE_NAME}" service-bridge "${RELAY_IMAGE_ID}"
component_container_exists "${SERVICE_NAME}" && \
  assert_owned_component "${SERVICE_NAME}" service "${SERVICE_IMAGE_ID}"
component_container_exists "${BROKER_NAME}" && \
  assert_owned_component "${BROKER_NAME}" docker-broker "${BROKER_IMAGE_ID}"
if [[ -e "${SERVICE_SOCKET}" ]]; then
  assert_socket_contract "${SERVICE_SOCKET}" 1000:1000:660
fi
if [[ -e "${BROKER_SOCKET}" ]]; then
  assert_socket_contract "${BROKER_SOCKET}" 1000:984:660
fi
if component_container_exists "${SERVICE_INGRESS_NAME}" && \
   [[ "$(docker inspect --format '{{.State.Running}}' "${SERVICE_INGRESS_NAME}")" == true ]]; then
  require_loopback_listener 8090
elif [[ -n "$(ss -H -ltn 'sport = :8090')" ]]; then
  die "TCP port 8090 is occupied without the exact running service ingress; no teardown was attempted."
fi
assert_backend_teardown_targets

diagnostics=()
abort_preserving_dependencies() {
  local reason="$1"
  printf 'ERROR: teardown stopped before removing dependencies: %s\n' "${reason}" >&2
  if ((${#diagnostics[@]} != 0)); then
    printf '  - %s\n' "${diagnostics[@]}" >&2
  fi
  exit 1
}

stop_remove_component() {
  local name="$1" description="$2" exit_code running_state
  running_state="$(docker inspect --format '{{.State.Running}}' "${name}")" || {
    diagnostics+=("inspect ${description} running state")
    return 1
  }
  if [[ "${running_state}" == true ]]; then
    if ! docker stop --signal SIGTERM --timeout -1 "${name}" >/dev/null; then
      diagnostics+=("stop ${description}")
    fi
  fi
  running_state="$(docker inspect --format '{{.State.Running}}' "${name}")" || {
    diagnostics+=("inspect ${description} after stop")
    return 1
  }
  if [[ "${running_state}" != false ]]; then
    diagnostics+=("${description} did not reach stopped state")
    return 1
  fi
  exit_code="$(docker inspect --format '{{.State.ExitCode}}' "${name}" 2>/dev/null || printf unavailable)"
  if [[ "${exit_code}" != 0 ]]; then
    diagnostics+=("${description} exit code ${exit_code}")
    if ! docker logs --tail 300 "${name}" >&2; then
      diagnostics+=("read ${description} failure logs")
    fi
  fi
  if ! docker rm "${name}" >/dev/null; then
    diagnostics+=("remove stopped ${description}")
  fi
}

service_stopped=false
if component_container_exists "${SERVICE_NAME}"; then
  printf 'Stopping the service while its existing loopback path remains available; an active session is cancelled and terminal HTTP requests are drained without a deadline...\n'
  service_running="$(docker inspect --format '{{.State.Running}}' "${SERVICE_NAME}")" || {
    diagnostics+=("inspect service running state")
    abort_preserving_dependencies "the service state could not be determined"
  }
  if [[ "${service_running}" == true ]] && \
     ! docker stop --signal SIGTERM --timeout -1 "${SERVICE_NAME}" >/dev/null; then
    diagnostics+=("gracefully stop service")
  fi
  service_running="$(docker inspect --format '{{.State.Running}}' "${SERVICE_NAME}")" || {
    diagnostics+=("inspect service after stop")
    abort_preserving_dependencies "the service post-stop state could not be determined"
  }
  if [[ "${service_running}" != false ]]; then
    diagnostics+=("service did not reach stopped state")
    abort_preserving_dependencies "the live service still needs its relays, broker, and backend"
  fi
  service_stopped=true
  service_exit="$(docker inspect --format '{{.State.ExitCode}}' "${SERVICE_NAME}" 2>/dev/null || printf unavailable)"
  if [[ "${service_exit}" != 0 ]]; then
    diagnostics+=("service exit code ${service_exit}")
    docker logs --tail 300 "${SERVICE_NAME}" >&2 || diagnostics+=("read service failure logs")
  fi
fi

# A failed service shutdown must not strand a temporary agent topology by
# removing its sole typed Docker broker or model path. Prove there are no
# profile-owned session objects before dismantling any dependency. Unknown or
# partially torn-down owned objects are evidence and require the still-running
# broker/backend for explicit recovery.
if ! session_objects="$({
  docker ps -aq --no-trunc \
    --filter "label=agent_service.profile=${PROFILE}" \
    --filter label=agent_service.session
} 2>&1)"; then
  diagnostics+=("list exact profile-owned session containers: ${session_objects}")
  abort_preserving_dependencies "session-container absence could not be proved"
fi
if [[ -n "${session_objects}" ]]; then
  session_evidence="$(
    docker ps -a --no-trunc \
      --filter "label=agent_service.profile=${PROFILE}" \
      --filter label=agent_service.session \
      --format '{{.ID}} {{.Names}} {{.Image}} {{.Status}}' 2>&1 || true
  )"
  diagnostics+=("profile-owned session containers remain: ${session_evidence:-${session_objects}}")
  abort_preserving_dependencies "temporary session teardown is incomplete; recovery dependencies were preserved"
fi

if component_container_exists "${SERVICE_INGRESS_NAME}"; then
  stop_remove_component "${SERVICE_INGRESS_NAME}" "service ingress" || \
    abort_preserving_dependencies "the live service ingress still needs its bridge and socket"
fi
if component_container_exists "${SERVICE_BRIDGE_NAME}"; then
  stop_remove_component "${SERVICE_BRIDGE_NAME}" "service bridge" || \
    abort_preserving_dependencies "the live service bridge still owns the service network namespace"
fi

# Docker refuses to remove a network-namespace owner while the bridge still
# shares it.  Keep the stopped service as that owner until the bridge is absent.
if [[ "${service_stopped}" == true ]] && component_container_exists "${SERVICE_NAME}"; then
  docker rm "${SERVICE_NAME}" >/dev/null || diagnostics+=("remove stopped service")
fi

if component_container_exists "${BROKER_NAME}"; then
  stop_remove_component "${BROKER_NAME}" "Docker broker" || \
    abort_preserving_dependencies "the live Docker broker must retain its model/backend dependencies"
fi

if [[ -e "${SERVICE_SOCKET}" ]]; then
  remove_owned_socket "${SERVICE_SOCKET}" 1000:1000:660 || diagnostics+=("remove service socket")
fi
if [[ -e "${BROKER_SOCKET}" ]]; then
  remove_owned_socket "${BROKER_SOCKET}" 1000:984:660 || diagnostics+=("remove broker socket")
fi

"${BACKEND_DIR}/stop.sh" || diagnostics+=("stop pinned backend and model relays")

for name in "${SERVICE_NAME}" "${BROKER_NAME}" "${SERVICE_BRIDGE_NAME}" "${SERVICE_INGRESS_NAME}"; do
  component_container_exists "${name}" && diagnostics+=("container remains: ${name}")
done
[[ -z "$(ss -H -ltn 'sport = :8090')" ]] || diagnostics+=("TCP port 8090 remains occupied")

if ((${#diagnostics[@]} != 0)); then
  printf 'ERROR: teardown was incomplete:\n' >&2
  printf '  - %s\n' "${diagnostics[@]}" >&2
  exit 1
fi
printf 'STOPPED — service, broker, fixed relays, backend, and owned runtime sockets are absent. Persistent result bundles remain under .runtime/results.\n'
