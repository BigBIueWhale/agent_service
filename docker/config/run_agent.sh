#!/usr/bin/env bash
set -Eeuo pipefail

readonly PROMPT_FILE=/run/agent/prompt.txt
readonly EVENTS_FILE=/output/events.jsonl
readonly STDERR_FILE=/output/qwen.stderr
readonly EXIT_FILE=/output/qwen-exit-code
readonly READY_FILE=/output/ready.json
readonly SETTINGS_SOURCE=/opt/agent/settings.json
readonly INSTRUCTIONS_SOURCE=/opt/agent/QWEN.md
readonly QWEN_HOME=/qwen-home
readonly QWEN_RUNTIME_DIR=/qwen-runtime
readonly MODEL_ID=qwen3.8-27b-nvfp4-k8v4
readonly MODEL_BASE=http://127.0.0.1:18000
readonly STRICT_TOOLS=agent,edit,glob,grep_search,list_directory,notebook_edit,read_file,run_shell_command,todo_write,write_file

fatal() {
  local code="$1"
  shift
  printf 'FATAL[%s]: %s\n' "${code}" "$*" >&2
  printf '%s\n' "${code}" >"${EXIT_FILE}"
  exit "${code}"
}

umask 077
export QWEN_HOME QWEN_RUNTIME_DIR
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
[[ -r "${SETTINGS_SOURCE}" && -r "${INSTRUCTIONS_SOURCE}" ]] || \
  fatal 92 "pinned Qwen configuration is missing from /opt/agent"
[[ -d /workspace && -d /artifacts && -d /output ]] || \
  fatal 93 "required workspace, artifacts, or output mount is missing"

mkdir -p "${QWEN_HOME}" /qwen-runtime
cp --no-preserve=mode,ownership "${SETTINGS_SOURCE}" "${QWEN_HOME}/settings.json"
cp --no-preserve=mode,ownership "${INSTRUCTIONS_SOURCE}" "${QWEN_HOME}/QWEN.md"
chmod 0600 "${QWEN_HOME}/settings.json" "${QWEN_HOME}/QWEN.md"

route_report="$(ip -4 route show)"
[[ -z "${route_report}" ]] || fatal 94 "network-none invariant failed; IPv4 route table is not empty: ${route_report}"

models_tmp="$(mktemp /tmp/agent-models.XXXXXXXX.json)"
tokenize_tmp="$(mktemp /tmp/agent-tokenize.XXXXXXXX.json)"
events_fifo_dir="$(mktemp -d /tmp/agent-events.XXXXXXXX)"
events_fifo="${events_fifo_dir}/stream"
mkfifo -m 0600 "${events_fifo}"
cleanup() {
  # ShellCheck does not follow EXIT-trap reachability into this handler.
  # shellcheck disable=SC2317
  rm -f -- "${models_tmp}" "${tokenize_tmp}" "${events_fifo}"
  # shellcheck disable=SC2317
  rmdir -- "${events_fifo_dir}"
}
trap cleanup EXIT

qwen_pid=""
qwen_pgid=""
qwen_status=255
tee_status=255
termination_exit=0
termination_forward_failed=0
termination_record_failed=0
forward_termination() {
  local exit_code="$1"
  termination_exit="${exit_code}"
  if ! printf '%s\n' "${exit_code}" >"${EXIT_FILE}" || \
     ! sync -f "${EXIT_FILE}"; then
    termination_record_failed=1
    printf 'AGENT_ERROR code=102 message=failed to durably record requested termination\n' >&2
  fi
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

curl --fail --silent --show-error \
  --retry 30 --retry-all-errors --retry-connrefused --retry-delay 1 \
  --connect-timeout 1 --max-time 35 \
  "${MODEL_BASE}/v1/models" >"${models_tmp}" || \
  fatal 95 "the sole loopback model endpoint never became reachable"

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

: >"${EVENTS_FILE}"
: >"${STDERR_FILE}"
printf '{"model":"%s","context_window":262144,"token_count":%s}\n' \
  "${MODEL_ID}" "$(jq -r '.count' "${tokenize_tmp}")" >"${READY_FILE}"
printf 'AGENT_READY model=%s context=262144 network=loopback-only\n' "${MODEL_ID}"

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
tee "${EVENTS_FILE}" <"${events_fifo}" &
tee_pid="$!"
setsid node /opt/qwen-code/scripts/cli-entry.js \
  --input-format=text \
  --approval-mode=yolo \
  --output-format=stream-json \
  --strict-tools="${STRICT_TOOLS}" \
  --foreground-agents-only \
  --max-subagent-depth=1 \
  --max-session-turns=-1 \
  --max-tool-calls=-1 \
  --no-chat-recording \
  <"${PROMPT_FILE}" \
  >"${events_fifo}" \
  2>>"${STDERR_FILE}" &
qwen_pid="$!"
qwen_pgid="${qwen_pid}"
if (( termination_exit != 0 )); then
  forward_termination "${termination_exit}"
fi
wait_for_child "${qwen_pid}" qwen_status
wait_for_child "${tee_pid}" tee_status
set -e

(( termination_record_failed == 0 )) || fatal 102 "failed to durably record a requested termination"
(( termination_forward_failed == 0 )) || fatal 101 "failed to forward a requested termination signal"
[[ "${tee_status}" == 0 ]] || fatal 99 "event capture failed with tee exit ${tee_status}"
if (( termination_exit != 0 && qwen_status == 0 )); then
  qwen_status="${termination_exit}"
fi
printf '%s\n' "${qwen_status}" >"${EXIT_FILE}"
sync -f "${EVENTS_FILE}" "${STDERR_FILE}" "${EXIT_FILE}" || \
  fatal 100 "failed to flush agent output files"
exit "${qwen_status}"
