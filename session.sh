#!/usr/bin/env bash
set -Eeuo pipefail

if [[ "$#" != 1 || ! "$1" =~ ^s-([0-9a-f]{64}|[0-9a-f]{32})$ ]]; then
  printf 'Usage: ./session.sh s-<64-lowercase-hex>\n' >&2
  printf 'Historical committed 32-hex session IDs remain readable.\n' >&2
  exit 2
fi
readonly SESSION_ID="$1"
RESPONSE_FILE="$(mktemp /tmp/agent-service-session.XXXXXX)"
readonly RESPONSE_FILE
trap 'rm -f -- "${RESPONSE_FILE}"' EXIT

set +e
HTTP_STATUS="$(curl --noproxy '*' --silent --show-error \
  --connect-timeout 5 --max-time 30 \
  --output "${RESPONSE_FILE}" --write-out '%{http_code}' \
  "http://127.0.0.1:8090/v1/agent/sessions/${SESSION_ID}")"
CURL_STATUS=$?
set -e
if [[ -s "${RESPONSE_FILE}" ]]; then
  jq . "${RESPONSE_FILE}" || sed -n '1,80p' "${RESPONSE_FILE}"
fi
if (( CURL_STATUS != 0 )); then
  printf 'ERROR: state read failed for %s (curl=%s). The operation is unaffected; retry this GET later.\n' "${SESSION_ID}" "${CURL_STATUS}" >&2
  exit 1
fi
if [[ "${HTTP_STATUS}" != 200 ]]; then
  printf 'ERROR: state read returned HTTP %s for %s.\n' "${HTTP_STATUS}" "${SESSION_ID}" >&2
  exit 1
fi
