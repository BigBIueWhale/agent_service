#!/usr/bin/env bash
set -Eeuo pipefail

readonly PROMPT_FILE=/run/agent/prompt.txt
readonly START_GATE_FILE=/run/agent/start-gate.lock
readonly STREAMS_DIR=/streams
readonly AGENT_EXEC=/opt/agent/agent_exec
readonly AGENT_EXEC_SANDBOX=landlock-fs-v4-write-roots-v1+output-unmounted-v1
readonly SETTINGS_SOURCE=/opt/agent/settings.json
readonly INSTRUCTIONS_SOURCE=/opt/agent/QWEN.md
readonly SYSTEM_PROMPT_SOURCE=/opt/agent/system.md
readonly DEPLOYMENT_CONTRACT_SOURCE=/opt/agent/deployment-contract.md
readonly RUNTIME_CONTRACT_SOURCE=/opt/agent/runtime-contract.json
readonly RUNTIME_CONTRACT_VERIFIER_SOURCE=/opt/agent/verify_runtime_contract.py
readonly AGENT_EXEC_SOURCE=/usr/share/agent-service/agent_exec.rs
readonly TOOLCHAIN_MANIFEST_SOURCE=/opt/agent/toolchain-manifest.json
readonly TOOLCHAIN_VERIFIER_SOURCE=/opt/agent/verify_toolchain.py
readonly AGENT_APT_LOCK_SOURCE=/opt/locks/agent-apt-packages.lock
readonly QWEN_HOME=/opt/agent
readonly QWEN_RUNTIME_DIR=/qwen-runtime
readonly MODEL_ID=qwen3.8-27b-nvfp4-k8v4
readonly MODEL_BASE=http://127.0.0.1:18000
readonly EXPECTED_INTERFACE=lo
readonly EXPECTED_IPV4_ADDRESS=127.0.0.1/8

qwen_pid=""
qwen_pgid=""
qwen_status=255
termination_exit=0
termination_forward_failed=0

fatal() {
  local code="$1"
  shift
  printf 'FATAL[%s]: %s\n' "${code}" "$*" >&2
  if [[ -n "${qwen_pgid}" ]] && kill -0 -- "-${qwen_pgid}" 2>/dev/null; then
    kill -TERM -- "-${qwen_pgid}" 2>/dev/null || true
  fi
  exit "${code}"
}

umask 077
export QWEN_HOME QWEN_RUNTIME_DIR
export QWEN38_AGENT_SERVICE_LOCKED=1
export QWEN_SYSTEM_MD="${SYSTEM_PROMPT_SOURCE}"
export QWEN_DEPLOYMENT_CONTRACT_MD="${DEPLOYMENT_CONTRACT_SOURCE}"
export QWEN38_LOCAL_API_KEY=local-loopback-only
export NO_COLOR=1
export QWEN_TELEMETRY_ENABLED=false
export XDG_CACHE_HOME=/qwen-runtime/cache
export NPM_CONFIG_CACHE=/qwen-runtime/npm
export PIP_CACHE_DIR=/qwen-runtime/pip
export CARGO_HOME=/qwen-runtime/cargo
export GOPATH=/qwen-runtime/go
[[ "$(id -u)" == 1000 && "$(id -g)" == 1000 ]] || \
  fatal 90 "agent must run as uid:gid 1000:1000"
[[ -f "${PROMPT_FILE}" && ! -L "${PROMPT_FILE}" ]] || \
  fatal 91 "${PROMPT_FILE} must be a regular, non-symlink file"
[[ -f "${START_GATE_FILE}" && ! -L "${START_GATE_FILE}" && \
   "$(stat -c '%u:%g:%a' "${START_GATE_FILE}")" == 1000:1000:600 ]] || \
  fatal 108 "${START_GATE_FILE} must be a regular uid:gid 1000:1000 mode-0600 gate"
[[ -r "${SETTINGS_SOURCE}" && -r "${INSTRUCTIONS_SOURCE}" && \
   -r "${SYSTEM_PROMPT_SOURCE}" && -r "${DEPLOYMENT_CONTRACT_SOURCE}" && \
   -r "${RUNTIME_CONTRACT_SOURCE}" && -x "${RUNTIME_CONTRACT_VERIFIER_SOURCE}" && \
   -r "${AGENT_EXEC_SOURCE}" && \
   -r "${TOOLCHAIN_MANIFEST_SOURCE}" && -x "${TOOLCHAIN_VERIFIER_SOURCE}" && \
   -r "${AGENT_APT_LOCK_SOURCE}" ]] || \
  fatal 92 "pinned Qwen configuration is missing from /opt/agent"
for sealed_prompt in "${SYSTEM_PROMPT_SOURCE}" "${DEPLOYMENT_CONTRACT_SOURCE}" \
  "${RUNTIME_CONTRACT_SOURCE}" "${RUNTIME_CONTRACT_VERIFIER_SOURCE}" \
  "${TOOLCHAIN_MANIFEST_SOURCE}" "${TOOLCHAIN_VERIFIER_SOURCE}"; do
  [[ -f "${sealed_prompt}" && ! -L "${sealed_prompt}" ]] || \
    fatal 104 "${sealed_prompt} must be a regular, non-symlink file"
done
[[ -d /workspace && -d /artifacts && -d "${STREAMS_DIR}" ]] || \
  fatal 93 "required workspace, artifacts, or read-only stream mount is missing"
[[ ! -e /output ]] || \
  fatal 110 "/output must be absent from the Qwen container; only the capture component owns it"
[[ -x "${AGENT_EXEC}" ]] || \
  fatal 111 "the pinned agent_exec sandbox launcher is absent"

mkdir -p /qwen-runtime
mkdir --mode=0700 /qwen-runtime/effects
mkdir --mode=0700 /tmp/qwen-subagents
[[ "$(stat -c '%a' /qwen-runtime/effects)" == 700 && \
   "$(stat -c '%a' /tmp/qwen-subagents)" == 700 ]] || \
  fatal 105 "effect-journal and subagent-scratch roots must have mode 0700"
[[ "${SETTINGS_SOURCE}" == "${QWEN_HOME}/settings.json" && \
   "${INSTRUCTIONS_SOURCE}" == "${QWEN_HOME}/QWEN.md" && \
   "${QWEN_SYSTEM_MD}" == "${QWEN_HOME}/system.md" && \
   "${QWEN_DEPLOYMENT_CONTRACT_MD}" == "${QWEN_HOME}/deployment-contract.md" ]] || \
  fatal 103 "QWEN_HOME must be the immutable /opt/agent configuration directory"

python3 "${RUNTIME_CONTRACT_VERIFIER_SOURCE}" \
  "${RUNTIME_CONTRACT_SOURCE}" \
  "${SETTINGS_SOURCE}" \
  "${INSTRUCTIONS_SOURCE}" \
  "${SYSTEM_PROMPT_SOURCE}" \
  "${DEPLOYMENT_CONTRACT_SOURCE}" \
  "${TOOLCHAIN_MANIFEST_SOURCE}" \
  /opt/agent/run_agent.sh "${AGENT_EXEC_SOURCE}" || \
  fatal 107 "canonical runtime contract validation failed"

python3 "${TOOLCHAIN_VERIFIER_SOURCE}" \
  "${TOOLCHAIN_MANIFEST_SOURCE}" "${AGENT_APT_LOCK_SOURCE}" || \
  fatal 106 "immutable offline toolchain contract validation failed"

interface_report="$(
  find /sys/class/net -mindepth 1 -maxdepth 1 -printf '%f\n' | LC_ALL=C sort
)"
[[ "${interface_report}" == "${EXPECTED_INTERFACE}" ]] || \
  fatal 94 "network-none invariant failed; expected only interface ${EXPECTED_INTERFACE}, found: ${interface_report:-none}"

ipv4_address_report="$(
  ip -o -4 address show | awk '{print $2 " " $4}' | LC_ALL=C sort
)"
[[ "${ipv4_address_report}" == "${EXPECTED_INTERFACE} ${EXPECTED_IPV4_ADDRESS}" ]] || \
  fatal 94 "network-none invariant failed; unexpected IPv4 addresses: ${ipv4_address_report:-none}"

ipv6_address_report="$(ip -o -6 address show)"
[[ -z "${ipv6_address_report}" ]] || \
  fatal 94 "network-none invariant failed; IPv6 addresses are present: ${ipv6_address_report}"

ipv4_route_report="$(ip -4 route show)"
[[ -z "${ipv4_route_report}" ]] || \
  fatal 94 "network-none invariant failed; IPv4 route table is not empty: ${ipv4_route_report}"

ipv6_route_report="$(ip -6 route show)"
[[ -z "${ipv6_route_report}" ]] || \
  fatal 94 "network-none invariant failed; IPv6 route table is not empty: ${ipv6_route_report}"

models_tmp="$(mktemp /tmp/agent-models.XXXXXXXX.json)"
tokenize_tmp="$(mktemp /tmp/agent-tokenize.XXXXXXXX.json)"
launcher_control_dir="$(mktemp -d /tmp/agent-launcher.XXXXXXXX)"
attestation_fifo="${launcher_control_dir}/attestation"
release_fifo="${launcher_control_dir}/release"
mkfifo -m 0600 "${attestation_fifo}" "${release_fifo}"
cleanup() {
  # ShellCheck does not follow EXIT-trap reachability into this handler.
  # shellcheck disable=SC2317
  rm -f -- "${models_tmp}" "${tokenize_tmp}" \
    "${attestation_fifo}" "${release_fifo}"
  # shellcheck disable=SC2317
  rmdir -- "${launcher_control_dir}" 2>/dev/null || true
}
trap cleanup EXIT

forward_termination() {
  local exit_code="$1"
  termination_exit="${exit_code}"
  if [[ -n "${qwen_pgid}" ]] && kill -0 -- "-${qwen_pgid}" 2>/dev/null; then
    if ! kill -TERM -- "-${qwen_pgid}" 2>/dev/null && \
       kill -0 -- "-${qwen_pgid}" 2>/dev/null; then
      termination_forward_failed=1
      printf 'AGENT_ERROR code=101 message=failed to forward termination to Qwen process group %s\n' \
        "${qwen_pgid}" >&2
    fi
  fi
}
trap 'forward_termination 143' TERM
trap 'forward_termination 130' INT

# The service locks this file before Docker creates the agent. The typed broker
# releases us only after the agent-namespace relay has emitted its exact bound
# listener event. This is an event-backed gate, not a retry loop or a fallback.
flock --exclusive "${START_GATE_FILE}" true || \
  fatal 109 "the broker-verified model-relay start gate failed"

curl --fail --silent --show-error \
  --connect-timeout 2 --max-time 10 \
  "${MODEL_BASE}/v1/models" >"${models_tmp}" || \
  fatal 95 "the broker-ready sole loopback model endpoint failed its one preflight request"

jq -e --arg model "${MODEL_ID}" \
  '.data | length == 1 and .[0].id == $model and .[0].max_model_len == 262144' \
  "${models_tmp}" >/dev/null || \
  fatal 96 "model identity or context length does not match the locked contract: $(tr -d '\n' <"${models_tmp}")"

curl --fail --silent --show-error \
  --connect-timeout 2 --max-time 30 \
  -H 'content-type: application/json' \
  --data '{"model":"qwen3.8-27b-nvfp4-k8v4","prompt":"agent-service-tokenizer-preflight"}' \
  "${MODEL_BASE}/tokenize" >"${tokenize_tmp}" || \
  fatal 97 "vLLM real-tokenizer preflight failed"
jq -e '.count > 0 and .max_model_len == 262144 and (.tokens | type == "array")' \
  "${tokenize_tmp}" >/dev/null || \
  fatal 98 "vLLM tokenizer response violates the locked contract: $(tr -d '\n' <"${tokenize_tmp}")"

wait_for_child() {
  local child_pid="$1" result_name="$2" child_status
  while true; do
    set +e
    wait "${child_pid}"
    child_status="$?"
    set -e
    if ! kill -0 "${child_pid}" 2>/dev/null; then
      printf -v "${result_name}" '%s' "${child_status}"
      return 0
    fi
  done
}

set +e
exec {attestation_fd}<>"${attestation_fifo}"
exec {release_fd}<>"${release_fifo}"
setsid "${AGENT_EXEC}" \
  3>&"${attestation_fd}" \
  4<&"${release_fd}" \
  <"${PROMPT_FILE}" \
  &
qwen_pid="$!"
qwen_pgid="${qwen_pid}"
if (( termination_exit != 0 )); then
  forward_termination "${termination_exit}"
fi

attestation=""
if ! IFS= read -r -t 15 -u "${attestation_fd}" attestation; then
  set -e
  fatal 112 "agent_exec did not prove its sandbox within the exact 15-second setup deadline"
fi
expected_attestation="AGENT_EXEC_READY sandbox=${AGENT_EXEC_SANDBOX}"
if [[ "${attestation}" != "${expected_attestation}" ]]; then
  set -e
  fatal 113 "agent_exec attestation drift: expected ${expected_attestation}, observed ${attestation:-<empty>}"
fi

token_count="$(jq -r '.count' "${tokenize_tmp}")"
printf 'AGENT_READY model=%s context=262144 network=loopback-only token_count=%s sandbox=%s\n' \
  "${MODEL_ID}" "${token_count}" "${AGENT_EXEC_SANDBOX}"
if ! printf 'EXEC\n' >&"${release_fd}"; then
  set -e
  fatal 114 "failed to release the attested agent_exec process"
fi
exec {attestation_fd}>&-
exec {release_fd}>&-
rm -f -- "${attestation_fifo}" "${release_fifo}"
rmdir -- "${launcher_control_dir}"

wait_for_child "${qwen_pid}" qwen_status
set -e

(( termination_forward_failed == 0 )) || fatal 101 "failed to forward a requested termination signal"
if (( termination_exit != 0 && qwen_status == 0 )); then
  qwen_status="${termination_exit}"
fi
exit "${qwen_status}"
