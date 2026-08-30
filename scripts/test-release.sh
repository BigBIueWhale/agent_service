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

# The archive extends the same stop condition: it contains the service image,
# so its bytes exist only after the loop converges, and its name and SHA are
# recorded only in the release lock. If the tar or its pins ever became build
# inputs, pinning the archive would move the inputs, rebake the service
# image's SOURCE_COMMIT, and change the bundle — a chase with no fixed point.
if "${PROJECT_DIR}/scripts/list-build-inputs.sh" | tr '\0' '\n' |
  grep -q '^artifacts/'; then
  fail 'artifacts/ entries are build inputs; bundling the archive would unsettle the release loop'
fi
archive_pin_mentions="$(grep -rl 'agent-service-images-' "${PROJECT_DIR}/config" 2>/dev/null | sort || true)"
[[ "${archive_pin_mentions}" == "${PROJECT_DIR}/config/release.lock.json" ]] ||
  fail "archive pins leaked outside the release lock: ${archive_pin_mentions}"

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

# ---------------------------------------------------------------------------
# The release lock's exact schema now includes the archive identity, and every
# malformed shape is refused: a lock without the archive keys, a name that
# does not follow the release naming, and a hash that is not a SHA256. The
# real checked-in lock must pass the same gate.
# ---------------------------------------------------------------------------
validate_release_lock >/dev/null || fail 'the checked-in release lock violates its own schema'
lock_copy="${TEST_DIR}/release.lock.json"
jq 'del(.archive)' "${PROJECT_DIR}/config/release.lock.json" >"${lock_copy}"
if (validate_release_lock "${lock_copy}") >/dev/null 2>&1; then
  fail 'a release lock without the archive identity was accepted'
fi
jq '.archive.name = "agent-service-images-latest.tar"'   "${PROJECT_DIR}/config/release.lock.json" >"${lock_copy}"
if (validate_release_lock "${lock_copy}") >/dev/null 2>&1; then
  fail 'a release lock with a non-release-derived archive name was accepted'
fi
jq '.archive.sha256 = "not-a-hash"'   "${PROJECT_DIR}/config/release.lock.json" >"${lock_copy}"
if (validate_release_lock "${lock_copy}") >/dev/null 2>&1; then
  fail 'a release lock with a malformed archive hash was accepted'
fi

# The recorded name must derive from the recorded implementation commit;
# anything else is a stale bundle wearing this release's pins.
recorded_archive_name="$(jq -er '.archive.name' "${PROJECT_DIR}/config/release.lock.json")"
derived_archive_name="$(service_archive_name_for_commit   "$(jq -er '.implementation_commit' "${PROJECT_DIR}/config/release.lock.json")")"
[[ "${recorded_archive_name}" == "${derived_archive_name}" ]] ||
  fail "the recorded archive name ${recorded_archive_name} does not derive from the implementation commit (${derived_archive_name})"

# ---------------------------------------------------------------------------
# verify_service_archive_contents proves a bundle carries exactly the pinned
# images, from the tar's own OCI index (whose per-image manifest digest IS
# the Docker image ID under the containerd store). Proved against synthetic
# tars: agreement passes; a drifted ID, a missing component, a stowaway
# image, and a tar without the one supported layout are each refused.
# ---------------------------------------------------------------------------
make_probe_archive() {
  local out="$1" index="$2" member="${3:-index.json}"
  python3 - "${out}" "${index}" "${member}" <<'PY'
import io, sys, tarfile
out, index, member = sys.argv[1], sys.argv[2], sys.argv[3]
data = index.encode()
with tarfile.open(out, "w") as tar:
    info = tarfile.TarInfo(member)
    info.size = len(data)
    tar.addfile(info, io.BytesIO(data))
PY
}
agreeing_index="$(jq -c '
  {manifests: [
    {digest: .images.agent,
     annotations: {"io.containerd.image.name": ("docker.io/library/" + $stack_docs[0].agent.image_tag)}},
    {digest: .images.relay,
     annotations: {"io.containerd.image.name": ("docker.io/library/" + $stack_docs[0].relay.image_tag)}},
    {digest: .images.capture,
     annotations: {"io.containerd.image.name": ("docker.io/library/" + $stack_docs[0].capture.image_tag)}},
    {digest: .images.broker,
     annotations: {"io.containerd.image.name": ("docker.io/library/" + $stack_docs[0].broker.image_tag)}},
    {digest: .images.service,
     annotations: {"io.containerd.image.name": ("docker.io/library/" + $stack_docs[0].service.image_tag)}}
  ]}' --slurpfile stack_docs "${PROJECT_DIR}/config/stack.lock.json" \
  "${PROJECT_DIR}/config/release.lock.json")"
probe_tar="${TEST_DIR}/probe-archive.tar"
make_probe_archive "${probe_tar}" "${agreeing_index}"
verify_service_archive_contents "${probe_tar}" >/dev/null ||
  fail 'an archive agreeing with every pin was refused'
make_probe_archive "${probe_tar}" \
  "$(jq -c '.manifests[0].digest = ("sha256:" + ("e" * 64))' <<<"${agreeing_index}")"
if (verify_service_archive_contents "${probe_tar}") >/dev/null 2>&1; then
  fail 'an archive with a drifted image ID was accepted'
fi
make_probe_archive "${probe_tar}" "$(jq -c '.manifests |= .[1:]' <<<"${agreeing_index}")"
if (verify_service_archive_contents "${probe_tar}") >/dev/null 2>&1; then
  fail 'an archive missing a pinned component was accepted'
fi
make_probe_archive "${probe_tar}" \
  "$(jq -c '.manifests += [{digest: ("sha256:" + ("f" * 64)), annotations: {"io.containerd.image.name": "docker.io/library/stowaway:latest"}}]' <<<"${agreeing_index}")"
if (verify_service_archive_contents "${probe_tar}") >/dev/null 2>&1; then
  fail 'an archive carrying an unpinned stowaway image was accepted'
fi
make_probe_archive "${probe_tar}" "${agreeing_index}" "manifest.json"
if (verify_service_archive_contents "${probe_tar}") >/dev/null 2>&1; then
  fail 'a tar without the one supported OCI layout was accepted'
fi

# ---------------------------------------------------------------------------
# When the pinned archive is present (it is gitignored, so a fresh clone
# legitimately lacks it until ./release.sh or a restore produces it), its
# bytes and contents must agree with the pins; verify_service_archive must
# also refuse a corrupted copy.
# ---------------------------------------------------------------------------
pinned_archive="${PROJECT_DIR}/artifacts/${recorded_archive_name}"
if [[ -f "${pinned_archive}" ]]; then
  verify_service_archive >/dev/null || fail 'the checked-in archive pins refuse the archive on disk'
  verify_service_archive_contents "${pinned_archive}" >/dev/null ||
    fail 'the archive on disk does not carry the pinned images'
  printf 'archive-on-disk=verified\n'
else
  printf 'archive-on-disk=absent (gitignored; produced by ./release.sh, transported by restore)\n'
fi

printf 'RELEASE_ARCHIVE_OK schema=exact name=commit-derived contents=proved termination=artifacts-excluded\n'
