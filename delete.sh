#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

if [[ "$#" != 1 || ! "$1" =~ ^s-([0-9a-f]{64}|[0-9a-f]{32})$ ]]; then
  printf 'Usage: ./delete.sh s-<64-lowercase-hex>\n' >&2
  printf 'Historical committed 32-hex terminal session IDs remain deletable.\n' >&2
  exit 2
fi
readonly SESSION_ID="$1"
RESPONSE_FILE="$(mktemp /tmp/agent-service-delete.XXXXXX)"
readonly RESPONSE_FILE
trap 'rm -f -- "${RESPONSE_FILE}"' EXIT

set +e
HTTP_STATUS="$(curl --noproxy '*' --silent --show-error \
  --connect-timeout 5 --max-time 30 \
  --output "${RESPONSE_FILE}" --write-out '%{http_code}' \
  --request DELETE "http://127.0.0.1:8090/v1/agent/sessions/${SESSION_ID}")"
CURL_STATUS=$?
set -e

if ((CURL_STATUS != 0)); then
  printf 'ERROR: deletion transport failed for %s (curl=%s). Repeating the same DELETE is safe.\n' \
    "${SESSION_ID}" "${CURL_STATUS}" >&2
  exit 1
fi
case "${HTTP_STATUS}" in
  204)
    [[ ! -s "${RESPONSE_FILE}" ]] || {
      printf 'ERROR: successful DELETE returned an unexpected response body for %s.\n' "${SESSION_ID}" >&2
      sed -n '1,80p' "${RESPONSE_FILE}" >&2
      exit 1
    }
    printf 'Deleted terminal session %s and its exact retained resources.\n' "${SESSION_ID}"
    ;;
  404)
    if [[ -s "${RESPONSE_FILE}" ]]; then
      jq . "${RESPONSE_FILE}" >&2 || sed -n '1,80p' "${RESPONSE_FILE}" >&2
    fi
    printf 'Session %s is already absent. The requested deletion outcome is satisfied.\n' "${SESSION_ID}"
    ;;
  *)
    if [[ -s "${RESPONSE_FILE}" ]]; then
      jq . "${RESPONSE_FILE}" >&2 || sed -n '1,80p' "${RESPONSE_FILE}" >&2
    fi
    printf 'ERROR: deletion returned HTTP %s for %s. Cancel a running session first; if deletion is already in progress, repeat this same command later.\n' \
      "${HTTP_STATUS}" "${SESSION_ID}" >&2
    exit 1
    ;;
esac
