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

test_network_none_metadata_is_exact() (
  docker() {
    case "$*" in
      "inspect --format {{.NetworkSettings.SandboxID}} contract-container")
        printf '%064d\n' 0
        ;;
      "inspect --format {{.NetworkSettings.SandboxKey}} contract-container")
        printf '%s\n' /var/run/docker/netns/000000000000
        ;;
      "network inspect --format {{.Id}} none")
        printf '%s\n' 1111111111111111111111111111111111111111111111111111111111111111
        ;;
      "inspect --format {{json .NetworkSettings.Networks}} contract-container")
        printf '%s\n' '{"none":{"IPAMConfig":null,"Links":null,"Aliases":null,"DriverOpts":null,"GwPriority":0,"NetworkID":"1111111111111111111111111111111111111111111111111111111111111111","EndpointID":"2222222222222222222222222222222222222222222222222222222222222222","Gateway":"","IPAddress":"","MacAddress":"","IPPrefixLen":0,"IPv6Gateway":"","GlobalIPv6Address":"","GlobalIPv6PrefixLen":0,"DNSNames":null}}'
        ;;
      "inspect --format {{json .NetworkSettings.Ports}} contract-container")
        printf '%s\n' '{}'
        ;;
      *)
        printf 'unexpected fake Docker invocation: %q' "$1" >&2
        printf ' %q' "${@:2}" >&2
        printf '\n' >&2
        return 97
        ;;
    esac
  }

  assert_network_none_docker_sandbox contract-container
)

test_network_none_metadata_rejects_an_address() (
  docker() {
    case "$*" in
      "inspect --format {{.NetworkSettings.SandboxID}} contract-container")
        printf '%064d\n' 0
        ;;
      "inspect --format {{.NetworkSettings.SandboxKey}} contract-container")
        printf '%s\n' /var/run/docker/netns/000000000000
        ;;
      "network inspect --format {{.Id}} none")
        printf '%s\n' 1111111111111111111111111111111111111111111111111111111111111111
        ;;
      "inspect --format {{json .NetworkSettings.Networks}} contract-container")
        printf '%s\n' '{"none":{"IPAMConfig":null,"Links":null,"Aliases":null,"DriverOpts":null,"GwPriority":0,"NetworkID":"1111111111111111111111111111111111111111111111111111111111111111","EndpointID":"2222222222222222222222222222222222222222222222222222222222222222","Gateway":"","IPAddress":"192.0.2.1","MacAddress":"","IPPrefixLen":0,"IPv6Gateway":"","GlobalIPv6Address":"","GlobalIPv6PrefixLen":0,"DNSNames":null}}'
        ;;
      *)
        printf 'unexpected fake Docker invocation: %q' "$1" >&2
        printf ' %q' "${@:2}" >&2
        printf '\n' >&2
        return 97
        ;;
    esac
  }

  assert_network_none_docker_sandbox contract-container
)

# The host check as every operator command runs it, with Docker's reported
# security options replaced by the given report. Nothing else about the host
# is faked.
host_isolation_check_on() (
  # Named apart from the check's own locals: bash scopes dynamically, so the
  # fake must not read a variable the check itself declares.
  local fake_security_options="$1"
  docker() {
    case "$*" in
      "version --format {{.Server.Version}}")
        printf '%s\n' 29.8.1
        ;;
      "info --format {{json .SecurityOptions}}")
        printf '%s\n' "${fake_security_options}"
        ;;
      *)
        printf 'unexpected fake Docker invocation: %q' "$1" >&2
        printf ' %q' "${@:2}" >&2
        printf '\n' >&2
        return 97
        ;;
    esac
  }
  check_host_tools_and_versions
)

test_readiness_replays_existing_event_and_stops_follower
test_network_none_metadata_is_exact

# Every case of the host-isolation rule's own corpus -- the file the vLLM
# backend repository carries byte-identically -- goes through the real host
# check: each accepted report passes silently, and each refused one exits with
# a refusal naming the failed property, what is required, what was reported,
# and a next action.
host_isolation_accepted=0
host_isolation_refused=0
while IFS=$'\x1f' read -r verdict report fragment; do
  if isolation_output="$(host_isolation_check_on "${report}" 2>&1)"; then
    isolation_status=0
  else
    isolation_status=$?
  fi
  case "${verdict}" in
    accept)
      [[ "${isolation_status}" == 0 && -z "${isolation_output}" ]] || {
        printf 'host check refused an accepted report %s (exit %s):\n%s\n' \
          "${report}" "${isolation_status}" "${isolation_output}" >&2
        exit 1
      }
      host_isolation_accepted=$((host_isolation_accepted + 1))
      ;;
    refuse)
      [[ "${isolation_status}" == 1 && \
         "${isolation_output}" == *'ERROR: This host does not provide the container isolation the deployment requires.'* && \
         "${isolation_output}" == *"Docker reports these security options: ${report:-<nothing>}"* && \
         "${isolation_output}" == *"${fragment}"* && \
         "${isolation_output}" == *'  Required: '* && \
         "${isolation_output}" == *'  Reported: '* && \
         "${isolation_output}" == *'  Next:     '* ]] || {
        printf 'host check did not refuse %s with "%s" (exit %s):\n%s\n' \
          "${report:-<nothing>}" "${fragment}" "${isolation_status}" "${isolation_output}" >&2
        exit 1
      }
      host_isolation_refused=$((host_isolation_refused + 1))
      ;;
    *)
      printf 'host-isolation case has no verdict: %q\n' "${verdict}" >&2
      exit 1
      ;;
  esac
done < <(host_isolation_cases)
((host_isolation_accepted > 0 && host_isolation_refused > 0)) || {
  printf 'the host-isolation corpus yielded %s accepted and %s refused cases\n' \
    "${host_isolation_accepted}" "${host_isolation_refused}" >&2
  exit 1
}

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

network_failure_output=''
if network_failure_output="$(test_network_none_metadata_rejects_an_address 2>&1)"; then
  printf 'addressed none-network metadata unexpectedly succeeded\n' >&2
  exit 1
fi
[[ "${network_failure_output}" == *'does not describe one exact addressless none-network endpoint'* ]] || {
  printf 'missing exact none-network metadata failure: %s\n' "${network_failure_output}" >&2
  exit 1
}

printf 'COMMON_CONTRACT_OK readiness=replayed-immediately follower=terminated failure=evidenced network-none=docker-and-kernel-proven host-isolation=%s-accepted-%s-refused\n' \
  "${host_isolation_accepted}" "${host_isolation_refused}"
