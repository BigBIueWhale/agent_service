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
case "${temporary}" in
  "${PROJECT_DIR}"/config/.build-inputs.sha256.*) ;;
  *) printf 'ERROR: unexpected temporary manifest path: %s\n' "${temporary}" >&2; exit 1 ;;
esac
cleanup() {
  rm -f -- "${temporary}"
}
trap cleanup EXIT

count=0
while IFS= read -r -d '' path; do
  case "${path}" in
    README.md|.gitignore|config/build-inputs.sha256|config/release.lock.json)
      continue
      ;;
  esac
  [[ -f "${PROJECT_DIR}/${path}" && ! -L "${PROJECT_DIR}/${path}" ]] || {
    printf 'ERROR: tracked build/release input is not a regular non-symlink file: %s\n' "${path}" >&2
    exit 1
  }
  (
    cd "${PROJECT_DIR}"
    sha256sum -- "${path}"
  ) >>"${temporary}"
  count=$((count + 1))
done < <(git -C "${PROJECT_DIR}" ls-files -z | LC_ALL=C sort -z)

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
trap - EXIT
printf 'WROTE %s entries=%s sha256=%s\n' \
  "${output}" "${count}" "$(sha256sum -- "${output}" | awk '{print $1}')"
