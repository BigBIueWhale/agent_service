#!/usr/bin/env bash
set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/common.sh
source "${SCRIPT_DIR}/scripts/common.sh"
require_no_arguments "./status.sh" "$@"
check_host_tools_and_versions
check_pinned_inputs
require_clean_repository

SERVICE_NAME="$(lock_value '.service.container_name')"
SERVICE_IMAGE="$(lock_value '.service.image_tag')"
AGENT_IMAGE="$(lock_value '.agent.image_tag')"
AGENT_IMAGE_ID="$(lock_value '.agent.image_id')"
BACKEND_DIR="$(lock_value '.backend.project_dir')"
SERVED_MODEL="$(lock_value '.backend.served_model')"
MAX_MODEL_LEN="$(lock_value '.backend.max_model_len')"
KV_CACHE_DTYPE="$(lock_value '.backend.kv_cache_dtype')"
QWEN_VERSION="$(lock_value '.agent.qwen_code.version')"
QWEN_COMMIT="$(lock_value '.agent.qwen_code.commit')"
readonly SERVICE_NAME SERVICE_IMAGE AGENT_IMAGE AGENT_IMAGE_ID BACKEND_DIR
readonly SERVED_MODEL MAX_MODEL_LEN KV_CACHE_DTYPE QWEN_VERSION QWEN_COMMIT

require_equal "agent image ID" "$(image_id "${AGENT_IMAGE}")" "${AGENT_IMAGE_ID}"
require_agent_image_contract
"${BACKEND_DIR}/status.sh"
if ! container_exists "${SERVICE_NAME}"; then
  [[ -z "$(ss -H -ltn 'sport = :8090')" ]] || die "Service is absent but port 8090 is occupied"
  printf '\nAGENT SERVICE STOPPED — all checked pins are valid.\n'
  exit 0
fi
require_equal "service running state" \
  "$(docker inspect --format '{{.State.Running}}' "${SERVICE_NAME}")" true
require_equal "service image tag" \
  "$(docker inspect --format '{{.Config.Image}}' "${SERVICE_NAME}")" "${SERVICE_IMAGE}"
require_equal "service image ID" \
  "$(docker inspect --format '{{.Image}}' "${SERVICE_NAME}")" "$(image_id "${SERVICE_IMAGE}")"
require_equal "service network" \
  "$(docker inspect --format '{{.HostConfig.NetworkMode}}' "${SERVICE_NAME}")" host
require_equal "service published ports" \
  "$(docker inspect --format '{{json .HostConfig.PortBindings}}' "${SERVICE_NAME}")" '{}'
require_service_image_contract
require_loopback_listener 8090
require_loopback_listener 8000
require_equal "health response" \
  "$(curl --fail --silent --show-error --max-time 10 http://127.0.0.1:8090/healthz)" ok
printf '\nREADY — one mode only\n'
printf '  service: http://127.0.0.1:8090\n'
printf '  model:   %s\n' "${SERVED_MODEL}"
printf '  context: %s tokens\n' "${MAX_MODEL_LEN}"
printf '  KV:      %s\n' "${KV_CACHE_DTYPE}"
printf '  client:  Qwen Code %s (patched commit %s)\n' \
  "${QWEN_VERSION}" "${QWEN_COMMIT}"
