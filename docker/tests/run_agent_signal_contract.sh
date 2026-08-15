#!/usr/bin/env bash
set -Eeuo pipefail

if [[ "$#" != 1 || ! -f "$1" ]]; then
  printf 'Usage: %s /absolute/run_agent.sh\n' "$0" >&2
  exit 2
fi
readonly WRAPPER="$1"

line_of() {
  local fixed="$1" line
  line="$(grep -nF -m1 -- "${fixed}" "${WRAPPER}" | cut -d: -f1)"
  [[ "${line}" =~ ^[1-9][0-9]*$ ]] || {
    printf 'missing wrapper contract line: %s\n' "${fixed}" >&2
    exit 1
  }
  printf '%s\n' "${line}"
}

TERM_TRAP_LINE="$(line_of "trap 'forward_termination 143' TERM")"
EVENTS_INIT_LINE="$(line_of ": >\"\${EVENTS_FILE}\"")"
STDERR_INIT_LINE="$(line_of ": >\"\${STDERR_FILE}\"")"
READY_LINE="$(line_of "printf '{\"model\":\"%s\",\"context_window\":262144,\"token_count\":%s}\\n'")"
readonly TERM_TRAP_LINE EVENTS_INIT_LINE STDERR_INIT_LINE READY_LINE

(( TERM_TRAP_LINE < READY_LINE )) || {
  printf 'TERM trap must be installed before readiness is published\n' >&2
  exit 1
}
(( EVENTS_INIT_LINE < READY_LINE && STDERR_INIT_LINE < READY_LINE )) || {
  printf 'required output sidecars must exist before readiness is published\n' >&2
  exit 1
}

line_of 'setsid node /opt/qwen-code/scripts/cli-entry.js' >/dev/null
line_of "kill -TERM -- \"-\${qwen_pgid}\"" >/dev/null
line_of "printf '%s\\n' \"\${exit_code}\" >\"\${EXIT_FILE}\"" >/dev/null
line_of "sync -f \"\${EXIT_FILE}\"" >/dev/null

bash -n "${WRAPPER}"
shellcheck -x "${WRAPPER}"
