#!/usr/bin/env bash
# The storage hook: run by Claude Code after every successful Bash tool call
# (the PostToolUse event, matcher "Bash"), from the Claude project settings of
# the checkout the session works in.
#
# ITS CONTRACT IS DELIBERATELY NOT THE ONE ./collect.sh HAS.
#   ./collect.sh refuses loudly and exits non-zero on anything it cannot
#   explain, because it decides what to delete. This hook never blocks and
#   always exits 0, because a storage reminder must never stop a session. It
#   also never hides: when its own check cannot run, it says so in one line
#   with the reason. Silence means exactly one thing -- the check ran and
#   nothing is collectable. It never removes anything; it reports, and a person
#   or Claude runs ./collect.sh. Do not "fix" either one to match the other: a
#   hook that failed like collect.sh would stop sessions over a reminder, and a
#   collect.sh that swallowed errors like this hook would collect on a guess.
#   That is why there is no `set -e` here and the only exit is the last line.
#
# WHEN IT EVALUATES. A release ends by pinning its archive in
# config/release.lock.json, and a backend release by pinning its image and
# archive in the backend's config/runtime-v1.sh; a deploy replaces the sockets
# the running stack serves on (the model socket and the service socket the
# stack lock names). Those pins and the two sockets' inode and change time are
# the fingerprint. Every invocation compares it with the one this hook last
# evaluated, which costs one jq, one stat and one small file read -- no Docker.
# Only a changed fingerprint runs ./collect.sh's dry run, and its verdict is
# reported once. While a release is in flight the lock pins no archive; that is
# not a moment to evaluate, so the hook waits for the release to finish.

set -o pipefail

PROJECT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
# Measured dry runs take 13-21 seconds; four minutes is far above that, and
# below the 300-second timeout the settings give this hook, so a slow run is
# reported here rather than cut off silently by Claude Code.
readonly EVALUATION_SECONDS=240

input="$(cat)"
event="PostToolUse"
if [[ "${input}" =~ \"hook_event_name\"[[:space:]]*:[[:space:]]*\"([A-Za-z]+)\" ]]; then
  event="${BASH_REMATCH[1]}"
fi

json_string() {
  local text="$1"
  text="${text//\\/\\\\}"
  text="${text//\"/\\\"}"
  text="${text//$'\n'/ }"
  printf '"%s"' "${text}"
}

say() {
  local line="$1"
  printf '{"systemMessage":%s,"hookSpecificOutput":{"hookEventName":%s,"additionalContext":%s}}\n' \
    "$(json_string "${line}")" "$(json_string "${event}")" "$(json_string "${line}")"
}

evaluate() {
  local state_dir state lock stack backend_dir paths release backend_pins fingerprint
  local recorded output status last first message
  if [[ -z "${CLAUDE_PROJECT_DIR:-}" ]]; then
    say "Storage check could not run: CLAUDE_PROJECT_DIR is not set, so there is nowhere to keep its state."
    return
  fi
  state_dir="${CLAUDE_PROJECT_DIR}/.claude"
  state="${state_dir}/collect-hook.state"
  lock="${PROJECT_DIR}/config/release.lock.json"
  stack="${PROJECT_DIR}/config/stack.lock.json"
  if ! command -v jq >/dev/null 2>&1; then
    say "Storage check could not run: jq is not installed."
    return
  fi
  if ! paths="$(jq -er '.backend.project_dir, (.relay.model_socket_dir + "/relay.sock"),
                        (.relay.service_socket_dir + "/relay.sock")' "${stack}" 2>&1)"; then
    say "Storage check could not run: ${stack} did not yield its backend and socket paths (${paths//$'\n'/ })."
    return
  fi
  backend_dir="$(sed -n 1p <<<"${paths}")"
  if ! release="$(jq -er '.archive | if type == "object" then .sha256 else "in-flight" end' "${lock}" 2>&1)"; then
    say "Storage check could not run: ${lock} did not yield its archive pin (${release//$'\n'/ })."
    return
  fi
  if [[ "${release}" == "in-flight" ]]; then
    return   # a release is between its first seal and its bundle
  fi
  if ! backend_pins="$(grep -E '^readonly (EXPECTED_IMAGE_ID|IMAGE_ARCHIVE_SHA256)=' \
                         "${backend_dir}/config/runtime-v1.sh" 2>&1)"; then
    say "Storage check could not run: ${backend_dir}/config/runtime-v1.sh did not yield its image and archive pins."
    return
  fi
  fingerprint="${release} ${backend_pins//$'\n'/ } $(sed -n '2,3p' <<<"${paths}" | while IFS= read -r socket; do
    stat -c '%i:%Z' -- "${socket}" 2>/dev/null || printf 'absent'
    printf ' '
  done)"

  if ! mkdir -p -- "${state_dir}" 2>/dev/null; then
    say "Storage check could not run: cannot create ${state_dir}."
    return
  fi
  exec 9>>"${state}.lock" || { say "Storage check could not run: cannot open ${state}.lock."; return; }
  if ! flock -n 9; then
    return   # another invocation is evaluating this same change and will report it
  fi
  recorded=""
  [[ -f "${state}" ]] && recorded="$(<"${state}")"
  if [[ "${recorded}" == "${fingerprint}" ]]; then
    return
  fi

  output="$(timeout "${EVALUATION_SECONDS}" "${PROJECT_DIR}/collect.sh" 2>&1)"
  status=$?
  message=""
  if ((status == 124)); then
    message="Storage check could not run: ./collect.sh did not finish within ${EVALUATION_SECONDS} seconds."
  elif ((status != 0)); then
    first="$(grep -m1 '^ERROR: ' <<<"${output}")"
    [[ -n "${first}" ]] || first="$(grep -v '^[[:space:]]*$' <<<"${output}" | tail -n 1)"
    first="${first#ERROR: }"
    message="Storage check could not run (./collect.sh exited ${status}): ${first:-no output}"
  else
    last="$(tail -n 1 <<<"${output}")"
    case "${last}" in
      "collectable: nothing") ;;
      collectable:*) message="Storage: ${last#collectable: } are collectable on this host; ${PROJECT_DIR}/collect.sh lists them." ;;
      *) message="Storage check could not run: ./collect.sh ended without its summary line." ;;
    esac
  fi
  # The fingerprint is recorded whatever the verdict, so each change is
  # reported once; a state that cannot be recorded is said, since the check
  # will then run again after every command.
  if ! printf '%s\n' "${fingerprint}" >"${state}" 2>/dev/null; then
    message="${message:+${message} }Storage check state could not be recorded in ${state}, so it runs after every command."
  fi
  [[ -z "${message}" ]] || say "${message}"
}

evaluate
exit 0
