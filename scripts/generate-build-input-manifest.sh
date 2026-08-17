#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd -- "${SCRIPT_DIR}/.." && pwd)"
readonly PROJECT_DIR

if (($# != 0)); then
  printf 'ERROR: no arguments are supported. Usage: ./scripts/generate-build-input-manifest.sh\n' >&2
  exit 2
fi

output="${PROJECT_DIR}/config/build-inputs.sha256"
temporary="$(mktemp "${PROJECT_DIR}/config/.build-inputs.sha256.XXXXXX")"
paths_file="$(mktemp /tmp/qwen38-build-input-list.XXXXXX)"
case "${temporary}" in
  "${PROJECT_DIR}"/config/.build-inputs.sha256.*) ;;
  *) printf 'ERROR: unexpected temporary manifest path: %s\n' "${temporary}" >&2; exit 1 ;;
esac
case "${paths_file}" in
  /tmp/qwen38-build-input-list.*) ;;
  *) printf 'ERROR: unexpected build-input list scratch path: %s\n' "${paths_file}" >&2; exit 1 ;;
esac
cleanup() {
  rm -f -- "${temporary}"
  rm -f -- "${paths_file}"
}
trap cleanup EXIT

if ! "${SCRIPT_DIR}/list-build-inputs.sh" >"${paths_file}"; then
  printf 'ERROR: canonical build-input enumeration failed\n' >&2
  exit 1
fi

count=0
while IFS= read -r -d '' path; do
  (
    cd "${PROJECT_DIR}"
    sha256sum -- "${path}"
  ) >>"${temporary}"
  count=$((count + 1))
done <"${paths_file}"

((count >= 40)) || {
  printf 'ERROR: build-input allowlist unexpectedly contains only %s files\n' "${count}" >&2
  exit 1
}
chmod 0644 "${temporary}"
(
  cd "${PROJECT_DIR}"
  sha256sum --check --strict "${temporary}"
) >/dev/null
mv -- "${temporary}" "${output}"
rm -f -- "${paths_file}"
trap - EXIT
printf 'WROTE %s entries=%s sha256=%s\n' \
  "${output}" "${count}" "$(sha256sum -- "${output}" | awk '{print $1}')"
