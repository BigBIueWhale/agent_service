#!/usr/bin/env bash
set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/common.sh
source "${SCRIPT_DIR}/scripts/common.sh"
require_no_arguments "./stop.sh" "$@"
check_host_tools_and_versions

SERVICE_NAME="$(lock_value '.service.container_name')"
BACKEND_NAME="$(lock_value '.backend.container_name')"
BACKEND_DIR="$(lock_value '.backend.project_dir')"
readonly SERVICE_NAME BACKEND_NAME BACKEND_DIR

if container_exists "${SERVICE_NAME}"; then
  printf 'Stopping the service; any active session is cancelled and teardown is awaited without a deadline...\n'
  docker stop --signal SIGTERM --timeout -1 "${SERVICE_NAME}" >/dev/null
  require_equal "service exit code" \
    "$(docker inspect --format '{{.State.ExitCode}}' "${SERVICE_NAME}")" 0
  docker rm "${SERVICE_NAME}" >/dev/null
fi
if container_exists "${BACKEND_NAME}"; then
  "${BACKEND_DIR}/stop.sh"
fi
[[ -z "$(ss -H -ltn 'sport = :8090')" ]] || die "Port 8090 remains occupied after teardown"
printf 'STOPPED — service and pinned backend containers are absent. Persistent result bundles remain under .runtime/results.\n'
