#!/usr/bin/env bash
# Exercises the pin manipulation ./release.sh performs, against copies of the
# real configuration files. The release loop itself is not simulated here --
# it needs Docker and a real build -- but every byte-level edit it makes to a
# reviewed lock is proved here, including the refusals.
set -Eeuo pipefail
if (($# != 0)); then
  printf 'ERROR: no arguments are supported. Usage: ./scripts/test-release.sh\n' >&2
  exit 2
fi

# release.sh (through scripts/common.sh) owns SCRIPT_DIR and PROJECT_DIR and
# marks them readonly, so this harness must not define them first.
release_under_test="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)/release.sh"
readonly release_under_test

TEST_DIR="$(mktemp -d /tmp/qwen38-release-test.XXXXXX)"
readonly TEST_DIR
cleanup() { rm -rf -- "${TEST_DIR}"; }
trap cleanup EXIT

fail() {
  printf 'RELEASE CONTRACT FAILURE: %s\n' "$1" >&2
  exit 1
}

# shellcheck source=../release.sh
source "${release_under_test}"

# ---------------------------------------------------------------------------
# Every pin location the release loop would write must exist in the real files,
# and the five components' locations must be exactly the set that records an
# image ID. A component silently missing a location would leave a stale ID
# behind in a file the build does not check.
# ---------------------------------------------------------------------------
for component in agent relay capture broker service; do
  locations=0
  while IFS=$'\t' read -r file path; do
    [[ -n "${file}" ]] || continue
    [[ -f "${file}" ]] || fail "${component} names a pin file that does not exist: ${file}"
    jq -e "${path}" "${file}" >/dev/null ||
      fail "${component} names a pin path that does not resolve: ${path} in ${file}"
    value="$(jq -er "${path}" "${file}")"
    [[ "${value}" =~ ^sha256:[0-9a-f]{64}$ ]] ||
      fail "${component} pin ${path} is not an image ID: ${value}"
    locations=$((locations + 1))
  done < <(component_pin_locations "${component}")
  ((locations > 0)) || fail "${component} has no pin locations"
done

# The service image ID must be recorded in exactly one place, and that place
# must be excluded from the build-input manifest. This is the cascade's stop
# condition; if it ever stops being true the release loop cannot terminate.
service_locations="$(component_pin_locations service | wc -l)"
[[ "${service_locations}" == 1 ]] ||
  fail "the service image ID is recorded in ${service_locations} places; the release loop can only terminate if it is recorded outside the build inputs"
if "${PROJECT_DIR}/scripts/list-build-inputs.sh" | tr '\0' '\n' |
  grep -qx 'config/release.lock.json'; then
  fail 'config/release.lock.json is a build input; the release loop cannot terminate'
fi

# ---------------------------------------------------------------------------
# replace_exact_value rewrites one value and nothing else, and refuses when the
# current value is not unique -- a byte substitution that matched twice would
# silently corrupt an unrelated pin.
# ---------------------------------------------------------------------------
probe="${TEST_DIR}/probe.json"
printf '{\n  "a": "sha256:%s",\n  "b": "sha256:%s"\n}\n' "$(printf 'a%.0s' {1..64})" \
  "$(printf 'b%.0s' {1..64})" >"${probe}"
before_lines="$(wc -l <"${probe}")"
before_bytes="$(wc -c <"${probe}")"
new="sha256:$(printf 'c%.0s' {1..64})"
replace_exact_value "${probe}" '.a' "sha256:$(printf 'a%.0s' {1..64})" "${new}" >/dev/null
[[ "$(jq -er '.a' "${probe}")" == "${new}" ]] || fail 'replace_exact_value did not rewrite the target'
[[ "$(jq -er '.b' "${probe}")" == "sha256:$(printf 'b%.0s' {1..64})" ]] ||
  fail 'replace_exact_value disturbed an unrelated value'
# Same length substitution, so byte and line counts must both be untouched:
# the reviewed locks are read by humans and diffed, and a release must not
# reflow them.
[[ "$(wc -l <"${probe}")" == "${before_lines}" ]] ||
  fail 'replace_exact_value changed the file line count'
[[ "$(wc -c <"${probe}")" == "${before_bytes}" ]] ||
  fail 'replace_exact_value changed the file byte count for a same-length value'

# Same value twice: ambiguous, must refuse without writing.
dup="${TEST_DIR}/dup.json"
shared="sha256:$(printf 'd%.0s' {1..64})"
printf '{\n  "a": "%s",\n  "b": "%s"\n}\n' "${shared}" "${shared}" >"${dup}"
dup_before="$(sha256sum <"${dup}")"
# Refusal is fatal by design, so observe it from a subshell rather than letting
# it end this harness.
if (replace_exact_value "${dup}" '.a' "${shared}" "${new}") >/dev/null 2>&1; then
  fail 'replace_exact_value rewrote an ambiguous value instead of refusing'
fi
[[ "$(sha256sum <"${dup}")" == "${dup_before}" ]] ||
  fail 'a refused replace_exact_value still wrote to the file'

# ---------------------------------------------------------------------------
# The checked-in tree must already be self-consistent: every image ID the stack
# lock and broker policy record must equal the one the release lock records, and
# the recorded broker-policy hash must be the policy's real hash. The release
# loop assumes this holds before it starts.
# ---------------------------------------------------------------------------
for component in agent relay capture broker service; do
  expected="$(jq -er ".images.${component}" "${PROJECT_DIR}/config/release.lock.json")"
  while IFS=$'\t' read -r file path; do
    [[ -n "${file}" ]] || continue
    actual="$(jq -er "${path}" "${file}")"
    [[ "${actual}" == "${expected}" ]] ||
      fail "checked-in ${component} pin disagrees: ${path} in $(basename "${file}") is ${actual}, release lock says ${expected}"
  done < <(component_pin_locations "${component}")
done
recorded_policy="$(jq -er '.broker.policy_sha256' "${PROJECT_DIR}/config/stack.lock.json")"
computed_policy="$(sha256sum "${PROJECT_DIR}/config/broker-policy-v1.json" | cut -d' ' -f1)"
[[ "${recorded_policy}" == "${computed_policy}" ]] ||
  fail "the stack lock records broker policy ${recorded_policy} but the policy hashes to ${computed_policy}"

printf 'RELEASE_CONTRACT_OK pin-locations=proved unique-substitution=enforced ambiguity=refused termination=release-lock-excluded tree=self-consistent\n'
