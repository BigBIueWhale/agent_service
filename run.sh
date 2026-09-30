#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly SCRIPT_DIR
# shellcheck source=scripts/submission-common.sh
source "${SCRIPT_DIR}/scripts/submission-common.sh"

usage() {
  printf 'Usage: ./run.sh /absolute/canonical/folder /absolute/task-prompt.txt --deliverables=JSON_ARRAY [--max-session-turns=N]\n' >&2
  printf '  --deliverables       required: a JSON array of the files the run must leave in /artifacts, as paths relative to it, e.g. %s; %s declares none.\n' \
    "'[\"report.md\"]'" "'[]'" >&2
  printf '  --max-session-turns  turn budget for this session and every subagent it starts, 1..%s; omitted selects the locked default of %s.\n' \
    "${SUBMISSION_MAX_SESSION_TURNS_CEILING}" "${SUBMISSION_DEFAULT_MAX_SESSION_TURNS}" >&2
  exit 2
}

# The creation-body fields are named rather than positional: each option's one
# accepted spelling is the field's own name, so a submission reads as what the
# service receives. Their values are not judged here beyond their JSON type --
# the submission library reads the turn ceiling from the stack lock, and the
# service alone applies the deliverables path rule, before it spools a single
# archive byte -- so this client cannot refuse a request the service admits or
# offer one it refuses. `--deliverables` has no default: `[]` is written out.
MAX_SESSION_TURNS=""
DELIVERABLES=""
DELIVERABLES_GIVEN=false
POSITIONAL=()
while (( $# > 0 )); do
  case "$1" in
    --max-session-turns=*) MAX_SESSION_TURNS="${1#*=}" ;;
    --deliverables=*)
      if [[ "${DELIVERABLES_GIVEN}" == true ]]; then
        printf 'ERROR: --deliverables was given twice; declare every file in one JSON array.\n' >&2
        usage
      fi
      DELIVERABLES="${1#*=}"
      DELIVERABLES_GIVEN=true
      ;;
    --*)
      printf 'ERROR: unknown option: %s\n' "$1" >&2
      usage
      ;;
    *) POSITIONAL+=("$1") ;;
  esac
  shift
done
readonly MAX_SESSION_TURNS DELIVERABLES DELIVERABLES_GIVEN
(( ${#POSITIONAL[@]} == 2 )) || usage
[[ "${DELIVERABLES_GIVEN}" == true ]] || {
  printf 'ERROR: --deliverables is required; pass --deliverables='"'"'[]'"'"' when the run is to leave no file in /artifacts.\n' >&2
  usage
}
readonly FOLDER="${POSITIONAL[0]}"
readonly PROMPT_FILE="${POSITIONAL[1]}"

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
(( PROMPT_BYTES > 0 && PROMPT_BYTES <= SUBMISSION_MAX_PROMPT_BYTES )) || {
  printf 'ERROR: prompt file is %s bytes; required range is 1..%s bytes.\n' "${PROMPT_BYTES}" "${SUBMISSION_MAX_PROMPT_BYTES}" >&2
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
REQUEST_FILE="$(submission_create_receipt "${SESSION_ID}" "${FOLDER}" "${PROMPT_FILE}" \
  "${DELIVERABLES}" "${MAX_SESSION_TURNS}")"
readonly REQUEST_FILE

printf 'Session handle: %s\n' "${SESSION_ID}"
printf 'The byte-exact request is durably retained until acceptance is proved. After any interruption, use: ./resubmit.sh %s\n' \
  "${SESSION_ID}" >&2
submission_post_receipt "${SESSION_ID}" "${REQUEST_FILE}"
