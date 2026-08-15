#!/usr/bin/env bash
set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
if [[ "$#" != 2 ]]; then
  printf 'Usage: ./run.sh /absolute/folder /absolute/task-prompt.txt\n' >&2
  exit 2
fi
readonly FOLDER="$1"
readonly PROMPT_FILE="$2"
readonly API=http://127.0.0.1:8090
[[ "${FOLDER}" == /* ]] || {
  printf 'ERROR: folder must be an absolute path: %s\n' "${FOLDER}" >&2
  exit 2
}
[[ "${PROMPT_FILE}" == /* && -f "${PROMPT_FILE}" && ! -L "${PROMPT_FILE}" && -r "${PROMPT_FILE}" ]] || {
  printf 'ERROR: prompt must be an absolute, readable, regular non-symlink file: %s\n' "${PROMPT_FILE}" >&2
  exit 2
}
PROMPT_BYTES="$(wc -c <"${PROMPT_FILE}")"
readonly PROMPT_BYTES
(( PROMPT_BYTES > 0 && PROMPT_BYTES <= 1048576 )) || {
  printf 'ERROR: prompt file is %s bytes; required range is 1..1048576 bytes.\n' "${PROMPT_BYTES}" >&2
  exit 2
}
"${SCRIPT_DIR}/status.sh" >/dev/null
REQUEST_FILE="$(mktemp)"
readonly REQUEST_FILE
trap 'rm -f -- "${REQUEST_FILE}"' EXIT
jq -n --arg folder "${FOLDER}" --rawfile prompt "${PROMPT_FILE}" \
  '{folder:$folder,prompt:$prompt}' >"${REQUEST_FILE}"
CREATED="$(curl --fail-with-body --silent --show-error \
  --header 'content-type: application/json' --data-binary "@${REQUEST_FILE}" \
  "${API}/v1/agent/sessions")"
readonly CREATED
SESSION_ID="$(jq -er '.session_id' <<<"${CREATED}")"
readonly SESSION_ID
printf 'Session %s is ready and running. Waiting without polling...\n' "${SESSION_ID}" >&2
FINISHED="$(curl --fail-with-body --silent --show-error \
  "${API}/v1/agent/sessions/${SESSION_ID}/wait")"
readonly FINISHED
jq -r '.response' <<<"${FINISHED}"
jq -e '.status == "completed" and .is_process_error == false' <<<"${FINISHED}" >/dev/null || {
  jq . <<<"${FINISHED}" >&2
  exit 1
}
