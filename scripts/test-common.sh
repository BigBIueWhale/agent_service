#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/common.sh
source "${SCRIPT_DIR}/common.sh"

require_no_arguments "./scripts/test-common.sh" "$@"

test_readiness_replays_existing_event_and_stops_follower() (
  docker() {
    if [[ "$1" == logs && "$2" == --follow ]]; then
      printf '%s\n' unrelated 'READY exact-event'
      exec sleep 30
    fi
    if [[ "$1" == inspect && "$2" == --format ]]; then
      printf '%s\n' true
      return 0
    fi
    printf 'unexpected fake Docker invocation: %q' "$1" >&2
    printf ' %q' "${@:2}" >&2
    printf '\n' >&2
    return 97
  }

  local started elapsed
  started="${SECONDS}"
  wait_for_container_event contract-container 'READY exact-event' 5
  elapsed=$((SECONDS - started))
  ((elapsed <= 1)) || {
    printf 'readiness replay did not return immediately: elapsed=%ss\n' "${elapsed}" >&2
    return 1
  }
)

test_readiness_reports_stream_failure() (
  docker() {
    if [[ "$1" == logs && "$2" == --follow ]]; then
      printf '%s\n' 'not the exact event'
      return 7
    fi
    if [[ "$1" == inspect && "$2" == --format ]]; then
      printf '%s\n' 'Container readiness failure state: synthetic'
      return 0
    fi
    printf 'unexpected fake Docker invocation: %q' "$1" >&2
    printf ' %q' "${@:2}" >&2
    printf '\n' >&2
    return 97
  }

  wait_for_container_event contract-container 'READY exact-event' 5
)

test_readiness_replays_existing_event_and_stops_follower

failure_output=''
if failure_output="$(test_readiness_reports_stream_failure 2>&1)"; then
  printf 'missing readiness event unexpectedly succeeded\n' >&2
  exit 1
fi
[[ "${failure_output}" == *'Container readiness failure state: synthetic'* ]] || {
  printf 'missing container-state evidence in readiness failure: %s\n' "${failure_output}" >&2
  exit 1
}
[[ "${failure_output}" == *'Read status=1; log-follower status=7.'* ]] || {
  printf 'missing exact stream statuses in readiness failure: %s\n' "${failure_output}" >&2
  exit 1
}

printf 'COMMON_CONTRACT_OK readiness=replayed-immediately follower=terminated failure=evidenced\n'
