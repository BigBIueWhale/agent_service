#!/usr/bin/env bash
set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/common.sh
source "${SCRIPT_DIR}/scripts/common.sh"
report_status_audit_failure() {
  local status="$1" source="$2" line="$3" command="$4"
  printf 'ERROR: unhandled status audit failure (exit %d) at %s:%s while running: %s\n' \
    "${status}" "${source}" "${line}" "${command}" >&2
}
trap 'report_status_audit_failure "$?" "${BASH_SOURCE[0]}" "${LINENO}" "${BASH_COMMAND}"' ERR
require_no_arguments "./status.sh" "$@"
check_host_tools_and_versions
check_pinned_inputs
require_release_commit
require_clean_committed_repository

SERVICE_NAME="$(lock_value '.service.container_name')"
SERVICE_IMAGE_ID="$(release_value '.images.service')"
BROKER_NAME="$(lock_value '.broker.container_name')"
BROKER_IMAGE_ID="$(lock_value '.broker.image_id')"
RELAY_IMAGE_ID="$(lock_value '.relay.image_id')"
CAPTURE_IMAGE_ID="$(lock_value '.capture.image_id')"
SERVICE_BRIDGE_NAME="$(lock_value '.relay.service_bridge_container')"
SERVICE_INGRESS_NAME="$(lock_value '.relay.service_ingress_container')"
BACKEND_DIR="$(lock_value '.backend.project_dir')"
RUNTIME_ROOT="$(lock_value '.service.runtime_root')"
STATE_DIR="$(lock_value '.service.state_dir')"
RESULTS_DIR="$(lock_value '.service.results_dir')"
CONTROL_DIR="$(dirname -- "$(lock_value '.broker.socket_path')")"
BROKER_SOCKET="$(lock_value '.broker.socket_path')"
MODEL_SOCKET_DIR="$(lock_value '.relay.model_socket_dir')"
SERVICE_SOCKET_DIR="$(lock_value '.relay.service_socket_dir')"
SERVICE_SOCKET="${SERVICE_SOCKET_DIR}/relay.sock"
readonly SERVICE_NAME SERVICE_IMAGE_ID BROKER_NAME BROKER_IMAGE_ID RELAY_IMAGE_ID CAPTURE_IMAGE_ID
readonly SERVICE_BRIDGE_NAME SERVICE_INGRESS_NAME BACKEND_DIR RUNTIME_ROOT STATE_DIR RESULTS_DIR
readonly CONTROL_DIR BROKER_SOCKET MODEL_SOCKET_DIR SERVICE_SOCKET_DIR SERVICE_SOCKET

require_equal "agent image ID" \
  "$(image_id "$(lock_value '.agent.image_tag')")" "$(lock_value '.agent.image_id')"
require_equal "session-capture image release/stack identity" \
  "${CAPTURE_IMAGE_ID}" "$(release_value '.images.capture')"
require_agent_image_contract
require_relay_image_contract
require_capture_image_contract
require_broker_image_contract
require_service_image_contract
"${BACKEND_DIR}/status.sh"

if ! component_container_exists "${SERVICE_NAME}"; then
  leftovers=()
  component_container_exists "${BROKER_NAME}" && leftovers+=("container ${BROKER_NAME}")
  component_container_exists "${SERVICE_BRIDGE_NAME}" && leftovers+=("container ${SERVICE_BRIDGE_NAME}")
  component_container_exists "${SERVICE_INGRESS_NAME}" && leftovers+=("container ${SERVICE_INGRESS_NAME}")
  [[ -e "${BROKER_SOCKET}" ]] && leftovers+=("path ${BROKER_SOCKET}")
  [[ -e "${SERVICE_SOCKET}" ]] && leftovers+=("path ${SERVICE_SOCKET}")
  [[ -n "$(ss -H -ltn 'sport = :8090')" ]] && leftovers+=("TCP listener 8090")
  if ((${#leftovers[@]} != 0)); then
    printf 'ERROR: service container is absent but partial stack state remains:\n' >&2
    printf '  - %s\n' "${leftovers[@]}" >&2
    printf 'Run ./stop.sh for exact ownership-checked teardown.\n' >&2
    exit 1
  fi
  printf '\nAGENT SERVICE STOPPED — release pins, images, and backend contract are valid; no service/broker/ingress state remains.\n'
  exit 0
fi

for name in "${BROKER_NAME}" "${SERVICE_BRIDGE_NAME}" "${SERVICE_INGRESS_NAME}"; do
  component_container_exists "${name}" || die "Required stack component is absent while the service exists: ${name}"
done

assert_runtime_directory "${RUNTIME_ROOT}" 700
assert_runtime_directory "${STATE_DIR}" 700
assert_runtime_directory "${RESULTS_DIR}" 700
assert_runtime_directory "${CONTROL_DIR}" 700
assert_runtime_directory "${MODEL_SOCKET_DIR}" 700
assert_runtime_directory "${SERVICE_SOCKET_DIR}" 700
assert_socket_contract "${BROKER_SOCKET}" 1000:984:660
assert_socket_contract "${MODEL_SOCKET_DIR}/relay.sock" 1000:1000:660
assert_socket_contract "${SERVICE_SOCKET}" 1000:1000:660

assert_hardened_component_base "${SERVICE_NAME}" service "${SERVICE_IMAGE_ID}" \
  "$(lock_value '.service.user')" none \
  "$(lock_value '.service.memory')" "$(lock_value '.service.memory_swap')" \
  "$(lock_value '.service.pids_limit')"
require_equal "service entrypoint" \
  "$(docker inspect --format '{{json .Config.Entrypoint}}' "${SERVICE_NAME}")" \
  '["/usr/local/bin/agent_service"]'
require_equal "service command" \
  "$(docker inspect --format '{{json .Config.Cmd}}' "${SERVICE_NAME}")" null
require_equal "service exact readiness count" \
  "$(docker logs "${SERVICE_NAME}" 2>&1 | grep --fixed-strings --line-regexp --count \
      "SERVICE_READY profile=$(lock_value '.profile') listen=127.0.0.1:8090 network=none" || true)" 1
require_equal "service /tmp contract" \
  "$(docker inspect --format '{{index .HostConfig.Tmpfs "/tmp"}}' "${SERVICE_NAME}")" \
  "$(lock_value '.service.tmpfs_tmp')"
require_equal "service mount count" \
  "$(docker inspect --format '{{len .Mounts}}' "${SERVICE_NAME}")" 5

mount_contract() {
  local name="$1" destination="$2"
  docker inspect --format \
    "{{range .Mounts}}{{if eq .Destination \"${destination}\"}}{{.Source}}|{{.RW}}|{{.Type}}|{{.Propagation}}{{end}}{{end}}" \
    "${name}"
}
require_equal "service input-root mount" \
  "$(mount_contract "${SERVICE_NAME}" "$(lock_value '.service.host_input_root')")" \
  "$(lock_value '.service.host_input_root')|false|bind|rprivate"
require_equal "service state mount" "$(mount_contract "${SERVICE_NAME}" "${STATE_DIR}")" \
  "${STATE_DIR}|true|bind|rprivate"
require_equal "service results mount" "$(mount_contract "${SERVICE_NAME}" "${RESULTS_DIR}")" \
  "${RESULTS_DIR}|true|bind|rprivate"
require_equal "service broker-control mount" "$(mount_contract "${SERVICE_NAME}" "${CONTROL_DIR}")" \
  "${CONTROL_DIR}|false|bind|rprivate"
require_equal "service model-socket mount" "$(mount_contract "${SERVICE_NAME}" "${MODEL_SOCKET_DIR}")" \
  "${MODEL_SOCKET_DIR}|false|bind|rprivate"
[[ -z "$(docker exec "${SERVICE_NAME}" ip -4 route show)" ]] || \
  die "Service has an unexpected IPv4 route despite network=none"
[[ -z "$(docker exec "${SERVICE_NAME}" ip -6 route show)" ]] || \
  die "Service has an unexpected IPv6 route despite network=none"
assert_network_none_proc "${SERVICE_NAME}"

assert_hardened_component_base "${BROKER_NAME}" docker-broker "${BROKER_IMAGE_ID}" \
  1000:984 none "$(lock_value '.broker.memory')" "$(lock_value '.broker.memory_swap')" \
  "$(lock_value '.broker.pids_limit')"
require_equal "broker entrypoint" \
  "$(docker inspect --format '{{json .Config.Entrypoint}}' "${BROKER_NAME}")" '["/docker_broker"]'
require_equal "broker command" "$(docker inspect --format '{{json .Config.Cmd}}' "${BROKER_NAME}")" null
require_equal "broker mount count" "$(docker inspect --format '{{len .Mounts}}' "${BROKER_NAME}")" 3
require_equal "broker Docker-socket mount" \
  "$(mount_contract "${BROKER_NAME}" "$(lock_value '.host.docker_socket')")" \
  "$(lock_value '.host.docker_socket')|false|bind|rprivate"
require_equal "broker control mount" "$(mount_contract "${BROKER_NAME}" "${CONTROL_DIR}")" \
  "${CONTROL_DIR}|true|bind|rprivate"
require_equal "broker state mount" "$(mount_contract "${BROKER_NAME}" "${STATE_DIR}")" \
  "${STATE_DIR}|false|bind|rprivate"
assert_network_none_proc "${BROKER_NAME}"
service_id="$(docker inspect --format '{{.Id}}' "${SERVICE_NAME}")"
assert_hardened_component_base "${SERVICE_BRIDGE_NAME}" service-bridge "${RELAY_IMAGE_ID}" \
  1000:1000 "container:${service_id}" \
  "$(lock_value '.relay.memory')" "$(lock_value '.relay.memory_swap')" \
  "$(lock_value '.relay.pids_limit')"
assert_hardened_component_base "${SERVICE_INGRESS_NAME}" service-ingress "${RELAY_IMAGE_ID}" \
  1000:1000 host "$(lock_value '.relay.memory')" "$(lock_value '.relay.memory_swap')" \
  "$(lock_value '.relay.pids_limit')"
for record in \
  "${SERVICE_BRIDGE_NAME}|service-bridge|true|unix:/sock/relay.sock|tcp:127.0.0.1:8090" \
  "${SERVICE_INGRESS_NAME}|service-ingress|false|tcp:127.0.0.1:8090|unix:/sock/relay.sock"; do
  IFS='|' read -r name role writable listen target <<<"${record}"
  require_equal "${name} entrypoint" \
    "$(docker inspect --format '{{json .Config.Entrypoint}}' "${name}")" '["/fixed_relay"]'
  require_equal "${name} command" \
    "$(docker inspect --format '{{json .Config.Cmd}}' "${name}")" "[\"${role}\"]"
  require_equal "${name} mount count" "$(docker inspect --format '{{len .Mounts}}' "${name}")" 1
  require_equal "${name} fixed socket mount" "$(mount_contract "${name}" /sock)" \
    "${SERVICE_SOCKET_DIR}|${writable}|bind|rprivate"
  assert_relay_kernel_sandbox "${name}" \
    "RELAY_READY role=${role} sandbox=$(lock_value '.relay.sandbox') listen=${listen} target=${target}"
done

require_loopback_listener 8000
require_loopback_listener 8090
require_equal "service health response" \
  "$(curl --fail --silent --show-error --max-time 10 http://127.0.0.1:8090/healthz)" ok
require_equal "model health response" \
  "$(curl --fail --silent --show-error --max-time 10 http://127.0.0.1:8000/health)" ''

printf '\nREADY — one defensively validated mode only\n'
printf '  service: http://127.0.0.1:8090 (fixed ingress -> Unix socket -> network-none service)\n'
printf '  model:   http://127.0.0.1:8000 (%s)\n' "$(lock_value '.backend.served_model')"
printf '  context: %s total tokens; KV: %s; vision: %s full-quality PNGs\n' \
  "$(lock_value '.backend.max_model_len')" "$(lock_value '.backend.kv_cache_dtype')" \
  "$(lock_value '.backend.vision.max_images')"
printf '  client:  Qwen Code %s, xhigh thinking, preserved historical thinking off\n' \
  "$(lock_value '.agent.qwen_code.version')"
