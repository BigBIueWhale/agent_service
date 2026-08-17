#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd -- "${SCRIPT_DIR}/.." && pwd)"
readonly PROJECT_DIR

if (($# != 0)); then
  printf 'ERROR: no arguments are supported. Usage: ./scripts/list-build-inputs.sh\n' >&2
  exit 2
fi

paths_file="$(mktemp /tmp/qwen38-build-input-list.XXXXXX)"
case "${paths_file}" in
  /tmp/qwen38-build-input-list.*) ;;
  *) printf 'ERROR: unexpected build-input list scratch path: %s\n' "${paths_file}" >&2; exit 1 ;;
esac
cleanup() {
  rm -f -- "${paths_file}"
}
trap cleanup EXIT
if ! git -C "${PROJECT_DIR}" ls-files -z | LC_ALL=C sort -z >"${paths_file}"; then
  printf 'ERROR: unable to enumerate the complete tracked build-input path set\n' >&2
  exit 1
fi

# Emit the exact eligible tracked path set as a sorted NUL-delimited stream.
# Documentation and benchmark evidence are outside the Docker context; the
# generated manifest and release lock cannot include their own changing hashes.
while IFS= read -r -d '' path; do
  case "${path}" in
    README.md|.gitignore|docs/*|artifacts/*|config/build-inputs.sha256|config/release.lock.json)
      continue
      ;;
  esac
  case "${path}" in
    *$'\n'*|*$'\r'*|*\\*)
      printf 'ERROR: tracked build-input path is not representable in the canonical checksum manifest: %q\n' "${path}" >&2
      exit 1
      ;;
  esac
  [[ -f "${PROJECT_DIR}/${path}" && ! -L "${PROJECT_DIR}/${path}" ]] || {
    printf 'ERROR: tracked build/release input is not a regular non-symlink file: %s\n' "${path}" >&2
    exit 1
  }
  printf '%s\0' "${path}"
done <"${paths_file}"
