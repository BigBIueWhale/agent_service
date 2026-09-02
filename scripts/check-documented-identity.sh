#!/usr/bin/env bash
# Every identity the documentation states is one this repository derives.
#
# A hash, image ID, or commit in a document is provenance: a claim that some
# file, image, archive, or revision has exactly these bytes. The build-input
# manifest, the stack lock, the release lock, and the broker policy already fix
# every such value this repository owns, so a documented value matching none of
# them names an artifact nobody has. That is a documentation failure, not a
# reading error, and it is refused here rather than shipped.
#
# Identity this repository does not pin is named by its owning source instead
# of restated. A value with no local source has nothing to be checked against,
# and an unowned copy is the thing that drifts.
set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly PROJECT_DIR="$(cd -- "${SCRIPT_DIR}/.." && pwd)"

# Documents that state provenance, and the pinned sources that fix it.
readonly DOCUMENTS=(README.md patches/README.md)
readonly PINNED_SOURCES=(
  config/build-inputs.sha256
  config/stack.lock.json
  config/release.lock.json
  config/broker-policy-v1.json
  config/agent-runtime-contract-v1.json
)
# A 40-hex git commit or a 64-hex SHA-256, whole-token so the 64-hex form
# cannot also match as a truncated 40-hex one.
readonly IDENTITY='\b[0-9a-f]{40}\b|\b[0-9a-f]{64}\b'

for required in "${DOCUMENTS[@]}" "${PINNED_SOURCES[@]}"; do
  [[ -f "${PROJECT_DIR}/${required}" && ! -L "${PROJECT_DIR}/${required}" ]] || {
    printf 'ERROR: missing required input: %s\n' "${required}" >&2
    exit 1
  }
done

# The release lock records the build-input manifest, so it cannot be one of the
# inputs that manifest hashes; its own digest is taken here instead.
derived="$(
  {
    cut -d' ' -f1 "${PROJECT_DIR}/config/build-inputs.sha256"
    ( cd -- "${PROJECT_DIR}" && grep -ohE "${IDENTITY}" "${PINNED_SOURCES[@]}" )
    sha256sum "${PROJECT_DIR}/config/release.lock.json" | cut -d' ' -f1
  } | sort -u
)"

undocumented=0
for document in "${DOCUMENTS[@]}"; do
  stated="$(grep -ohE "${IDENTITY}" "${PROJECT_DIR}/${document}" | sort -u || true)"
  if [[ -z "${stated}" ]]; then
    printf 'ERROR: %s states no identity; the provenance extractor no longer matches it.\n' \
      "${document}" >&2
    exit 1
  fi
  underived="$(comm -23 <(printf '%s\n' "${stated}") <(printf '%s\n' "${derived}"))"
  if [[ -n "${underived}" ]]; then
    printf 'ERROR: %s states identity this repository does not derive:\n' "${document}" >&2
    while read -r value; do
      printf '  %s  (%s:%s)\n' "${value}" "${document}" \
        "$(grep -n -m1 -F "${value}" "${PROJECT_DIR}/${document}" | cut -d: -f1)" >&2
    done <<<"${underived}"
    undocumented=1
  fi
done
(( undocumented == 0 )) || exit 1

printf 'DOC_IDENTITY_CONTRACT_OK documents=%s derived=%s\n' \
  "${#DOCUMENTS[@]}" "$(printf '%s\n' "${derived}" | wc -l)"
