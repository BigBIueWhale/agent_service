#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly SCRIPT_DIR
# shellcheck source=scripts/submission-common.sh
source "${SCRIPT_DIR}/scripts/submission-common.sh"

if [[ "$#" != 2 ]]; then
  printf 'Usage: ./run.sh /absolute/canonical/folder /absolute/task-prompt.txt\n' >&2
  exit 2
fi
readonly FOLDER="$1"
readonly PROMPT_FILE="$2"

[[ "${FOLDER}" == /* && -d "${FOLDER}" && ! -L "${FOLDER}" ]] || {
  printf 'ERROR: folder must be an existing absolute ordinary directory, not a symlink: %s\n' "${FOLDER}" >&2
  exit 2
}
readonly CANONICAL_FOLDER="$(realpath -e -- "${FOLDER}")"
[[ "${CANONICAL_FOLDER}" == "${FOLDER}" ]] || {
  printf 'ERROR: folder is not in its one canonical absolute spelling. Supplied: %s Canonical: %s\n' "${FOLDER}" "${CANONICAL_FOLDER}" >&2
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

HANDLE_HEX="$(LC_ALL=C od -An -N32 -tx1 /dev/urandom | tr -d ' \n')"
readonly HANDLE_HEX
[[ "${HANDLE_HEX}" =~ ^[0-9a-f]{64}$ ]] || {
  printf 'ERROR: the operating-system CSPRNG did not produce exactly 32 bytes as 64 lowercase hexadecimal characters.\n' >&2
  exit 1
}
readonly SESSION_ID="s-${HANDLE_HEX}"
REQUEST_FILE="$(submission_create_receipt "${SESSION_ID}" "${FOLDER}" "${PROMPT_FILE}")"
readonly REQUEST_FILE

printf 'Session handle: %s\n' "${SESSION_ID}"
printf 'The byte-exact request is durably retained until acceptance is proved. After any interruption, use: ./resubmit.sh %s\n' \
  "${SESSION_ID}" >&2
submission_post_receipt "${SESSION_ID}" "${REQUEST_FILE}"
