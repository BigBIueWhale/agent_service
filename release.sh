#!/usr/bin/env bash
# The one way to cut a release.
#
# `./build.sh` deliberately only *asserts* that every pinned image ID is the ID
# the sources actually produce; it never advances a pin, because that assertion
# is what makes a checkout verifiable by anyone. Advancing the pins is this
# script, and it is a loop rather than a list of steps because the components
# form a chain: the agent image ID is compiled into the typed broker policy,
# that policy is compiled into the broker and the service, and the stack lock is
# compiled into the service. Moving one moves the next.
#
# The loop is: seal whatever changed, build, and if the build refuses because a
# component's real ID is not its pinned ID, adopt that ID and go round again.
# It terminates because the service image ID is recorded only in
# config/release.lock.json, which scripts/list-build-inputs.sh excludes from the
# build-input manifest -- so adopting it changes no build input and the next
# build has nothing left to disagree with.
#
# Nothing here is a judgement call at runtime. In particular the rule that a
# service repin must not advance implementation_commit is not special-cased: it
# falls out of "advance the release lock only when the build-input manifest
# actually changed", because release.lock.json is not a build input. Encoding it
# as a derived fact rather than a remembered exception is the point -- a baked
# SOURCE_COMMIT that no longer describes its own tree is the failure this
# prevents.
set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/common.sh
source "${SCRIPT_DIR}/scripts/common.sh"

readonly STACK_LOCK_PATH="${PROJECT_DIR}/config/stack.lock.json"
readonly RELEASE_LOCK_PATH="${PROJECT_DIR}/config/release.lock.json"
readonly BROKER_POLICY_PATH="${PROJECT_DIR}/config/broker-policy-v1.json"
readonly BUILD_INPUTS_PATH="${PROJECT_DIR}/config/build-inputs.sha256"
# One iteration is spent per component whose ID moves, plus one final build that
# must pass with nothing left to adopt. Five components plus that final pass is
# the exact upper bound; needing more means the chain is not converging and the
# script must say so instead of looping.
readonly MAX_ROUNDS=6

# Every component, in the order build.sh produces them, with every location that
# records its image ID. A component is repinned in all of its locations at once
# or not at all. Fields are: name, image-tag lock path, then the pin locations as
# file:jq-path pairs.
readonly COMPONENTS=(agent relay capture broker service)
component_tag_path() {
  case "$1" in
    agent) printf '.agent.image_tag' ;;
    relay) printf '.relay.image_tag' ;;
    capture) printf '.capture.image_tag' ;;
    broker) printf '.broker.image_tag' ;;
    service) printf '.service.image_tag' ;;
    *) die "unknown component: $1" ;;
  esac
}
# Locations are printed one per line as `<file>\t<jq path>`. The service image ID
# is recorded only in the release lock; that asymmetry is the cascade's stop
# condition, so it lives here as data rather than as a comment somewhere else.
component_pin_locations() {
  case "$1" in
    agent)
      printf '%s\t%s\n' "${STACK_LOCK_PATH}" '.agent.image_id'
      printf '%s\t%s\n' "${RELEASE_LOCK_PATH}" '.images.agent'
      printf '%s\t%s\n' "${BROKER_POLICY_PATH}" '.agent.image_id'
      ;;
    relay)
      printf '%s\t%s\n' "${STACK_LOCK_PATH}" '.relay.image_id'
      printf '%s\t%s\n' "${RELEASE_LOCK_PATH}" '.images.relay'
      printf '%s\t%s\n' "${BROKER_POLICY_PATH}" '.relay.image_id'
      ;;
    capture)
      printf '%s\t%s\n' "${STACK_LOCK_PATH}" '.capture.image_id'
      printf '%s\t%s\n' "${RELEASE_LOCK_PATH}" '.images.capture'
      printf '%s\t%s\n' "${BROKER_POLICY_PATH}" '.capture.image_id'
      ;;
    broker)
      printf '%s\t%s\n' "${STACK_LOCK_PATH}" '.broker.image_id'
      printf '%s\t%s\n' "${RELEASE_LOCK_PATH}" '.images.broker'
      ;;
    service)
      printf '%s\t%s\n' "${RELEASE_LOCK_PATH}" '.images.service'
      ;;
    *) die "unknown component: $1" ;;
  esac
}

json_value() {
  local file="$1" path="$2"
  jq -er "${path}" "${file}" ||
    die "missing/invalid pin ${path} in ${file}"
}

# Replace one exact JSON string value in place, by byte substitution rather than
# a jq rewrite, so that formatting, key order, and every unrelated byte of these
# reviewed configuration files are preserved exactly.
replace_exact_value() {
  local file="$1" path="$2" old="$3" new="$4"
  # Checked explicitly rather than left to `set -e`: when this function is
  # called from a condition context `set -e` is suspended for its whole body,
  # and a failed substitution would otherwise fall through to the verification
  # below and be reported as a value mismatch instead of as what it is.
  if ! python3 - "${file}" "${path}" "${old}" "${new}" <<'PY'
import json, sys
file, path, old, new = sys.argv[1:5]
with open(file, encoding="utf-8") as handle:
    raw = handle.read()
occurrences = raw.count(old)
if occurrences != 1:
    sys.exit(
        f"refusing to repin {path} in {file}: the current value appears "
        f"{occurrences} times, so a byte substitution is not unambiguous"
    )
with open(file, "w", encoding="utf-8") as handle:
    handle.write(raw.replace(old, new))
PY
  then
    die "refusing to repin ${path} in ${file}: the substitution was not unambiguous"
  fi
  require_equal "repinned ${path} in $(basename "${file}")" \
    "$(json_value "${file}" "${path}")" "${new}"
}

# The typed broker policy is itself hashed into the stack lock, so any repin
# that touched it must carry that hash forward in the same step.
resync_broker_policy_hash() {
  local recorded computed
  recorded="$(json_value "${STACK_LOCK_PATH}" '.broker.policy_sha256')"
  computed="$(sha256_file "${BROKER_POLICY_PATH}")"
  if [[ "${recorded}" != "${computed}" ]]; then
    replace_exact_value "${STACK_LOCK_PATH}" '.broker.policy_sha256' \
      "${recorded}" "${computed}"
    printf '  broker policy hash %s -> %s\n' "${recorded:0:12}" "${computed:0:12}"
  fi
}

# Adopt the ID a component's sources actually produce, everywhere it is recorded.
repin_component() {
  local component="$1" observed="$2" file path current
  printf 'Adopting the %s image ID the build produced: %s\n' "${component}" "${observed}"
  while IFS=$'\t' read -r file path; do
    [[ -n "${file}" ]] || continue
    current="$(json_value "${file}" "${path}")"
    if [[ "${current}" == "${observed}" ]]; then
      continue
    fi
    replace_exact_value "${file}" "${path}" "${current}" "${observed}"
    printf '  %s %s -> %s\n' "$(basename "${file}")" "${path}" "${observed:0:19}"
  done < <(component_pin_locations "${component}")
  resync_broker_policy_hash
}

# Find the single component whose real image disagrees with its pins. build.sh
# stops at the first disagreement, so at most one can be outstanding; a
# component whose image is not present locally has simply not been built yet.
drifted_component() {
  local component tag observed pinned found=""
  for component in "${COMPONENTS[@]}"; do
    tag="$(lock_value "$(component_tag_path "${component}")")"
    observed="$(docker image inspect "${tag}" --format '{{.Id}}' 2>/dev/null || true)"
    [[ -n "${observed}" ]] || continue
    pinned="$(json_value "${RELEASE_LOCK_PATH}" ".images.${component}")"
    if [[ "${observed}" != "${pinned}" ]]; then
      [[ -z "${found}" ]] ||
        die "more than one component disagrees with its pins (${found} and ${component}); refusing to guess which release this is"
      found="${component}"
      printf '%s\t%s\n' "${component}" "${observed}"
    fi
  done
  [[ -n "${found}" ]]
}

# Bring the release lock into agreement with the tree, and commit whatever
# changed. The release lock names an implementation commit only when the
# build-input manifest actually moved, which is what keeps the service image's
# baked SOURCE_COMMIT describing its own tree.
seal() {
  local subject="$1" manifest_before manifest_after commit stack_sha
  manifest_before="$(sha256_file "${BUILD_INPUTS_PATH}")"
  "${PROJECT_DIR}/scripts/generate-build-input-manifest.sh" >/dev/null
  manifest_after="$(sha256_file "${BUILD_INPUTS_PATH}")"

  if [[ "${manifest_after}" != "${manifest_before}" ]]; then
    # Build inputs moved, so this is a new implementation of the stack. Commit
    # them first: the release lock has to name a commit that already contains
    # them, and build.sh requires that commit to be an ancestor of HEAD.
    if ! git -C "${PROJECT_DIR}" diff --quiet ||
      ! git -C "${PROJECT_DIR}" diff --cached --quiet; then
      git -C "${PROJECT_DIR}" add -A
      git -C "${PROJECT_DIR}" commit -q -m "${subject}" \
        -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
      printf '  inputs commit: %s\n' "$(git -C "${PROJECT_DIR}" log --oneline -1)"
    fi
    commit="$(git -C "${PROJECT_DIR}" rev-parse HEAD)"
    stack_sha="$(sha256_file "${STACK_LOCK_PATH}")"
    replace_exact_value "${RELEASE_LOCK_PATH}" '.implementation_commit' \
      "$(json_value "${RELEASE_LOCK_PATH}" '.implementation_commit')" "${commit}"
    replace_exact_value "${RELEASE_LOCK_PATH}" '.build_inputs_manifest_sha256' \
      "$(json_value "${RELEASE_LOCK_PATH}" '.build_inputs_manifest_sha256')" \
      "${manifest_after}"
    replace_exact_value "${RELEASE_LOCK_PATH}" '.stack_lock_sha256' \
      "$(json_value "${RELEASE_LOCK_PATH}" '.stack_lock_sha256')" "${stack_sha}"
    printf '  release lock -> commit %s manifest %s\n' \
      "${commit:0:12}" "${manifest_after:0:12}"
  fi

  if ! git -C "${PROJECT_DIR}" diff --quiet ||
    ! git -C "${PROJECT_DIR}" diff --cached --quiet; then
    git -C "${PROJECT_DIR}" add -A
    git -C "${PROJECT_DIR}" commit -q -m "${subject}" \
      -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
    printf '  commit: %s\n' "$(git -C "${PROJECT_DIR}" log --oneline -1)"
  fi
}

# The invariant the whole release turns on, asserted against the repository
# rather than trusted: the commit the release lock names must already contain
# the tree whose build-input manifest it records, and it must be an ancestor of
# HEAD. A release that advanced implementation_commit after the service image
# was built would bake a SOURCE_COMMIT describing a different tree, and the
# image would silently misreport what produced it.
require_release_lock_describes_its_own_tree() {
  local commit manifest_recorded manifest_at_commit stack_recorded stack_at_commit
  commit="$(json_value "${RELEASE_LOCK_PATH}" '.implementation_commit')"
  git -C "${PROJECT_DIR}" merge-base --is-ancestor "${commit}" HEAD ||
    die "release lock names ${commit}, which is not an ancestor of HEAD"
  manifest_recorded="$(json_value "${RELEASE_LOCK_PATH}" '.build_inputs_manifest_sha256')"
  manifest_at_commit="$(git -C "${PROJECT_DIR}" show "${commit}:config/build-inputs.sha256" |
    sha256sum | cut -d' ' -f1)" ||
    die "commit ${commit} does not contain a build-input manifest"
  require_equal "build-input manifest recorded by the release lock vs the one in ${commit:0:12}" \
    "${manifest_recorded}" "${manifest_at_commit}"
  stack_recorded="$(json_value "${RELEASE_LOCK_PATH}" '.stack_lock_sha256')"
  stack_at_commit="$(git -C "${PROJECT_DIR}" show "${commit}:config/stack.lock.json" |
    sha256sum | cut -d' ' -f1)" ||
    die "commit ${commit} does not contain a stack lock"
  require_equal "stack lock recorded by the release lock vs the one in ${commit:0:12}" \
    "${stack_recorded}" "${stack_at_commit}"
}

main() {
  local round drift drifted_name drifted_id component_name
  check_host_tools_and_versions
  # Repinning while the stack is up would leave the running containers
  # describing a release the lock no longer names, and ./stop.sh would then
  # refuse to tear them down. Refuse before touching anything rather than
  # creating that state.
  for component_name in "$(lock_value '.service.container_name')" \
    "$(lock_value '.broker.container_name')" \
    "$(lock_value '.relay.service_bridge_container')" \
    "$(lock_value '.relay.service_ingress_container')"; do
    if component_container_exists "${component_name}"; then
      die "the stack is up (${component_name} exists); run ./stop.sh first, because repinning a running component's image ID would leave ./stop.sh unable to tear it down"
    fi
  done
  require_clean_committed_repository

  printf 'Releasing: build, adopt whatever moved, repeat until the build agrees.\n'
  for ((round = 1; round <= MAX_ROUNDS; round++)); do
    printf '\n== round %d/%d ==\n' "${round}" "${MAX_ROUNDS}"
    seal "Advance the pinned stack"

    if "${PROJECT_DIR}/build.sh"; then
      seal "Advance the pinned stack"
      check_pinned_inputs
      require_release_lock_describes_its_own_tree
      require_clean_committed_repository
      printf '\nRELEASED — every component image is the image its committed sources produce.\n'
      for component_name in "${COMPONENTS[@]}"; do
        printf '  %-8s %s\n' "${component_name}" \
          "$(json_value "${RELEASE_LOCK_PATH}" ".images.${component_name}")"
      done
      printf '  commit   %s\n' "$(json_value "${RELEASE_LOCK_PATH}" '.implementation_commit')"
      printf 'Deploy it with ./start.sh; verify a checkout of this commit with ./build.sh.\n'
      return 0
    fi

    drift="$(drifted_component)" ||
      die "the build failed for a reason other than an image-ID disagreement; its output above is the evidence"
    IFS=$'\t' read -r drifted_name drifted_id <<<"${drift}"
    repin_component "${drifted_name}" "${drifted_id}"
    seal "Pin the rebuilt ${drifted_name} image"
  done

  die "the pinned image IDs did not converge in ${MAX_ROUNDS} rounds; the chain is not settling and this needs a human"
}

# Sourced by scripts/test-release.sh to exercise the pin manipulation against
# copies; executed directly it cuts the release.
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  require_no_arguments "./release.sh" "$@"
  main
fi
