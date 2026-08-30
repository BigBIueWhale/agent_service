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
# so its bytes exist only after the loop converges, and its SHA is recorded
# only in the release lock. If the tar or its pin ever became build inputs,
# pinning the archive would move the inputs, rebake the service image's
# SOURCE_COMMIT, and change the bundle — a chase with no fixed point.
if "${PROJECT_DIR}/scripts/list-build-inputs.sh" | tr '\0' '\n' |
  grep -q '^artifacts/'; then
  fail 'artifacts/ entries are build inputs; bundling the archive would unsettle the release loop'
fi
archive_pin="$(jq -er '.archive.sha256' "${PROJECT_DIR}/config/release.lock.json")"
archive_pin_mentions="$(grep -rl "${archive_pin}" "${PROJECT_DIR}/config" 2>/dev/null | sort || true)"
[[ "${archive_pin_mentions}" == "${PROJECT_DIR}/config/release.lock.json" ]] ||
  fail "the archive pin leaked outside the release lock: ${archive_pin_mentions}"

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
# The release lock's exact schema includes the archive identity, and every
# malformed shape is refused: a lock without the archive pin, one that
# records an archive name (the name is a constant, not release state — a
# recorded name is exactly what a derivation gate grows back from), and a
# hash that is not a SHA256. The real checked-in lock must pass the same
# gate.
# ---------------------------------------------------------------------------
validate_release_lock >/dev/null || fail 'the checked-in release lock violates its own schema'
lock_copy="${TEST_DIR}/release.lock.json"
jq 'del(.archive)' "${PROJECT_DIR}/config/release.lock.json" >"${lock_copy}"
if (validate_release_lock "${lock_copy}") >/dev/null 2>&1; then
  fail 'a release lock without the archive identity was accepted'
fi
jq '.archive.name = "agent-service-images.tar"' \
  "${PROJECT_DIR}/config/release.lock.json" >"${lock_copy}"
if (validate_release_lock "${lock_copy}") >/dev/null 2>&1; then
  fail 'a release lock recording an archive name was accepted'
fi
jq '.archive.sha256 = "not-a-hash"'   "${PROJECT_DIR}/config/release.lock.json" >"${lock_copy}"
if (validate_release_lock "${lock_copy}") >/dev/null 2>&1; then
  fail 'a release lock with a malformed archive hash was accepted'
fi

# ---------------------------------------------------------------------------
# A release that changes any build input advances implementation_commit at
# its FIRST seal — before the loop's next build, and rounds before the
# converged images can be bundled and the archive pin adopted. Every build
# inside the loop therefore runs against a lock whose archive pin still
# names the PREVIOUS release's bundle, and every gate the build applies to
# the lock must accept exactly that shape. A gate that tied the archive pin
# to implementation_commit shipped once: it failed inside the loop, was not
# an image-ID disagreement, so adoption could not repair it, and the
# bundling step that would have satisfied it sat unreachable behind the
# failing build — no input-changing release could be cut. Exercised on a
# copy with the release's own edit primitive: first the seal-step commit
# advance with the archive pin left behind (mid-loop shape), then the
# bundle-step hash adoption (converged shape); the lock gate must accept
# both.
# ---------------------------------------------------------------------------
advancing_lock="${TEST_DIR}/advancing.lock.json"
cp -- "${PROJECT_DIR}/config/release.lock.json" "${advancing_lock}"
sealed_commit="$(jq -er '.implementation_commit' "${advancing_lock}")"
advanced_commit="0000000000000000000000000000000000000000"
[[ "${advanced_commit}" != "${sealed_commit}" ]] ||
  fail 'the advanced-commit probe collided with the recorded implementation commit'
replace_exact_value "${advancing_lock}" '.implementation_commit' \
  "${sealed_commit}" "${advanced_commit}" >/dev/null
validate_release_lock "${advancing_lock}" >/dev/null ||
  fail 'a mid-release lock (implementation_commit advanced, archive pin not yet re-adopted) was refused; no input-changing release could be cut'
replace_exact_value "${advancing_lock}" '.archive.sha256' \
  "$(jq -er '.archive.sha256' "${advancing_lock}")" \
  "$(printf 'a%.0s' {1..64})" >/dev/null
validate_release_lock "${advancing_lock}" >/dev/null ||
  fail 'adopting a freshly bundled archive hash was refused by the lock gate'

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
# Deliberately absent: any assertion about the tar under artifacts/. This
# harness is a build input and runs inside every build of the release loop,
# where the tar on disk is legitimately the PREVIOUS release's bundle (the
# new one is produced only after the loop converges, and mid-loop the lock's
# image pins have already moved past it). An on-disk check here is an
# assertion about a post-convergence artifact from inside the loop — the
# wedge class the advancing-release probe above proves refused. The tar is
# verified where it is produced (./release.sh, before and after the pin is
# adopted) and everywhere it is consumed (scripts/restore-service-images.sh,
# scripts/verify-published-release.sh) — every path that touches its bytes,
# none of which run inside the build.
# ---------------------------------------------------------------------------

printf 'RELEASE_ARCHIVE_OK schema=exact identity=sha256-only contents=proved advancing-release=accepted termination=artifacts-excluded\n'
