#!/usr/bin/env bash
# Report the images and archives this host holds for both repositories, and
# the retention rule that keeps or collects each one; with --delete, remove
# exactly the ones the report marks COLLECT.
#
# The policy and its derivation are in scripts/collect.py and the README's
# "Storage and retention" section. There is one evaluation: the report is what
# --delete acts on, re-proved object by object immediately before each removal.
# Anything the evaluation cannot explain -- no Docker, a lock that does not
# parse, a deployed release it cannot verify, an archive whose bytes are not
# the ones pinned for its name -- stops it with the reason and a next step,
# and nothing is removed.
set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/common.sh
source "${SCRIPT_DIR}/scripts/common.sh"

case "$#:${1:-}" in
  0:) mode=report ;;
  1:--delete) mode=delete ;;
  *)
    printf 'Usage: ./collect.sh [--delete]\n' >&2
    exit 2
    ;;
esac
for tool in docker git python3; do
  command -v "${tool}" >/dev/null 2>&1 || die "Required host tool is missing: ${tool}"
done
exec python3 "${PROJECT_DIR}/scripts/collect.py" "${PROJECT_DIR}" "${mode}"
