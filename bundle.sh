#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

# Retrieve one terminal session's result bundle over the connection and prove
# it against both the transport's declared commitment and the session
# resource before publishing it at the requested path. Nothing is read from
# the service's private filesystem.

if [[ "$#" != 2 || ! "$1" =~ ^s-([0-9a-f]{64}|[0-9a-f]{32})$ || "$2" != /* ]]; then
  printf 'Usage: ./bundle.sh s-<64-lowercase-hex> /absolute/output/bundle.tar.zst\n' >&2
  printf 'Historical committed 32-hex session IDs remain readable.\n' >&2
  exit 2
fi
readonly SESSION_ID="$1"
readonly OUTPUT_PATH="$2"

[[ ! -e "${OUTPUT_PATH}" ]] || {
  printf 'ERROR: refusing to overwrite existing output path %s.\n' "${OUTPUT_PATH}" >&2
  exit 1
}

SCRATCH_DIR="$(mktemp -d /tmp/agent-service-bundle.XXXXXX)"
readonly SCRATCH_DIR
trap 'rm -rf -- "${SCRATCH_DIR}"' EXIT
readonly BODY_FILE="${SCRATCH_DIR}/session.json"
readonly HEADER_FILE="${SCRATCH_DIR}/bundle-headers.txt"
readonly BUNDLE_FILE="${SCRATCH_DIR}/bundle.tar.zst"

set +e
HTTP_STATUS="$(curl --noproxy '*' --silent --show-error \
  --connect-timeout 5 --max-time 30 \
  --output "${BODY_FILE}" --write-out '%{http_code}' \
  "http://127.0.0.1:8090/v1/agent/sessions/${SESSION_ID}")"
CURL_STATUS=$?
set -e
if (( CURL_STATUS != 0 )) || [[ "${HTTP_STATUS}" != 200 ]]; then
  if [[ -s "${BODY_FILE}" ]]; then jq . "${BODY_FILE}" >&2 || sed -n '1,40p' "${BODY_FILE}" >&2; fi
  printf 'ERROR: session read returned curl=%s HTTP %s for %s; the bundle state is unknown, retry later.\n' \
    "${CURL_STATUS}" "${HTTP_STATUS}" "${SESSION_ID}" >&2
  exit 1
fi
RESOURCE_STATUS="$(jq -r '.status' "${BODY_FILE}")"
RESOURCE_SHA256="$(jq -r '.bundle_sha256' "${BODY_FILE}")"
RESOURCE_BYTES="$(jq -r '.bundle_compressed_bytes' "${BODY_FILE}")"
readonly RESOURCE_STATUS RESOURCE_SHA256 RESOURCE_BYTES
if [[ "${RESOURCE_STATUS}" == running ]]; then
  printf 'ERROR: session %s is still running; a bundle exists only for terminal sessions.\n' "${SESSION_ID}" >&2
  exit 1
fi
if [[ ! "${RESOURCE_SHA256}" =~ ^[0-9a-f]{64}$ ]]; then
  printf 'ERROR: terminal session %s accepted no result bundle; its terminal record is the only artifact.\n' "${SESSION_ID}" >&2
  exit 1
fi

set +e
HTTP_STATUS="$(curl --noproxy '*' --silent --show-error \
  --connect-timeout 5 --max-time 900 \
  --output "${BUNDLE_FILE}" --dump-header "${HEADER_FILE}" \
  --write-out '%{http_code}' \
  "http://127.0.0.1:8090/v1/agent/sessions/${SESSION_ID}/bundle")"
CURL_STATUS=$?
set -e
if (( CURL_STATUS != 0 )) || [[ "${HTTP_STATUS}" != 200 ]]; then
  printf 'ERROR: bundle download returned curl=%s HTTP %s for %s; nothing was published, repeating the same download is safe.\n' \
    "${CURL_STATUS}" "${HTTP_STATUS}" "${SESSION_ID}" >&2
  exit 1
fi

HEADER_SHA256="$(tr -d '\r' <"${HEADER_FILE}" | awk 'tolower($1)=="x-bundle-sha256:"{print $2}' | tail -1)"
OBSERVED_BYTES="$(stat -c '%s' -- "${BUNDLE_FILE}")"
OBSERVED_SHA256="$(sha256sum -- "${BUNDLE_FILE}" | awk '{print $1}')"
readonly HEADER_SHA256 OBSERVED_BYTES OBSERVED_SHA256
if [[ "${OBSERVED_SHA256}" != "${RESOURCE_SHA256}" || "${OBSERVED_SHA256}" != "${HEADER_SHA256}" \
      || "${OBSERVED_BYTES}" != "${RESOURCE_BYTES}" ]]; then
  printf 'ERROR: downloaded bundle for %s does not match its commitments (observed %s bytes sha %s; resource %s bytes sha %s; header sha %s). Nothing was published; repeating the same download is safe.\n' \
    "${SESSION_ID}" "${OBSERVED_BYTES}" "${OBSERVED_SHA256}" "${RESOURCE_BYTES}" "${RESOURCE_SHA256}" "${HEADER_SHA256}" >&2
  exit 1
fi

mv --no-clobber -- "${BUNDLE_FILE}" "${OUTPUT_PATH}" || {
  printf 'ERROR: cannot publish verified bundle at %s without overwriting.\n' "${OUTPUT_PATH}" >&2
  exit 1
}
sync -f -- "${OUTPUT_PATH}"
printf 'Published verified bundle for %s at %s (%s bytes, sha256 %s).\n' \
  "${SESSION_ID}" "${OUTPUT_PATH}" "${OBSERVED_BYTES}" "${OBSERVED_SHA256}"
