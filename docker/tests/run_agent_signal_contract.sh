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
READY_LINE="$(line_of "printf 'AGENT_READY model=%s context=262144 network=loopback-only token_count=%s preserve_thinking=%s sandbox=%s\\n'")"
HISTORY_POLICY_LINE="$(line_of 'history_policy_payload="$(<"${HISTORY_POLICY_FILE}")"')"
# These are deliberately literal source landmarks; expansion would make the
# contract test search for this test process's environment instead.
# shellcheck disable=SC2016
TOOLCHAIN_VERIFY_LINE="$(line_of 'python3 "${TOOLCHAIN_VERIFIER_SOURCE}"')"
# shellcheck disable=SC2016
RUNTIME_CONTRACT_VERIFY_LINE="$(line_of 'python3 "${RUNTIME_CONTRACT_VERIFIER_SOURCE}"')"
EFFECT_ROOT_LINE="$(line_of 'mkdir --mode=0700 /qwen-runtime/effects')"
SCRATCH_ROOT_LINE="$(line_of 'mkdir --mode=0700 /tmp/qwen-subagents')"
# shellcheck disable=SC2016
START_GATE_LINE="$(line_of 'flock --exclusive "${START_GATE_FILE}" true')"
DEVPTS_LINE="$(line_of 'devpts rw,nosuid,noexec,relatime,gid=5,mode=620,ptmxmode=666')"
# shellcheck disable=SC2016
AGENT_EXEC_LINE="$(line_of 'setsid "${AGENT_EXEC}"')"
# shellcheck disable=SC2016
ATTEST_LINE="$(line_of 'expected_attestation="AGENT_EXEC_READY sandbox=${AGENT_EXEC_SANDBOX}"')"
RELEASE_LINE="$(line_of "if ! printf 'EXEC\\n'")"
readonly TERM_TRAP_LINE READY_LINE AGENT_EXEC_LINE ATTEST_LINE RELEASE_LINE
readonly HISTORY_POLICY_LINE
readonly TOOLCHAIN_VERIFY_LINE EFFECT_ROOT_LINE SCRATCH_ROOT_LINE
readonly RUNTIME_CONTRACT_VERIFY_LINE
readonly START_GATE_LINE
readonly DEVPTS_LINE

(( TERM_TRAP_LINE < READY_LINE )) || {
  printf 'TERM trap must be installed before readiness is published\n' >&2
  exit 1
}
(( TOOLCHAIN_VERIFY_LINE < READY_LINE )) || {
  printf 'toolchain contract must be validated before readiness is published\n' >&2
  exit 1
}
(( RUNTIME_CONTRACT_VERIFY_LINE < TOOLCHAIN_VERIFY_LINE )) || {
  printf 'canonical runtime contract must precede toolchain validation and readiness\n' >&2
  exit 1
}
(( HISTORY_POLICY_LINE < RUNTIME_CONTRACT_VERIFY_LINE )) || {
  printf 'the canonical history policy must select an immutable Qwen home before contract validation\n' >&2
  exit 1
}
(( EFFECT_ROOT_LINE < READY_LINE && SCRATCH_ROOT_LINE < READY_LINE )) || {
  printf 'effect-journal and subagent-scratch roots must exist before readiness\n' >&2
  exit 1
}
(( TERM_TRAP_LINE < START_GATE_LINE && START_GATE_LINE < READY_LINE )) || {
  printf 'the termination handler must cover the start gate and the gate must precede readiness\n' >&2
  exit 1
}
(( DEVPTS_LINE < AGENT_EXEC_LINE )) || {
  printf 'the exact isolated devpts contract must be proven before agent_exec starts\n' >&2
  exit 1
}
(( AGENT_EXEC_LINE < ATTEST_LINE && ATTEST_LINE < READY_LINE && READY_LINE < RELEASE_LINE )) || {
  printf 'Qwen must be sandbox-attested before readiness and released only afterward\n' >&2
  exit 1
}
if grep -Eq -- '--retry|retry-connrefused|retry-all-errors' "${WRAPPER}"; then
  printf 'model readiness must use the broker gate and one preflight, not curl retries\n' >&2
  exit 1
fi

line_of '[[ ! -e /output ]]' >/dev/null
line_of 'readonly AGENT_EXEC=/opt/agent/agent_exec' >/dev/null
# This deliberately searches for a literal source landmark.
# shellcheck disable=SC2016
line_of '$(readlink /dev/ptmx)' >/dev/null
line_of "'character special file:5:2:666:0:0'" >/dev/null
line_of "kill -TERM -- \"-\${qwen_pgid}\"" >/dev/null
if grep -Eq -- '/output/(events|qwen|ready|response)|setsid node|tee ' "${WRAPPER}"; then
  printf 'Qwen wrapper must not hold the service output mount or capture files\n' >&2
  exit 1
fi

bash -n "${WRAPPER}"
shellcheck -x "${WRAPPER}"
