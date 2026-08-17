#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly SCRIPT_DIR
# shellcheck source=scripts/submission-common.sh
source "${SCRIPT_DIR}/scripts/submission-common.sh"

if [[ "$#" != 1 ]]; then
  printf 'Usage: ./resubmit.sh s-<64-lowercase-hex>\n' >&2
  exit 2
fi
readonly SESSION_ID="$1"
submission_require_handle "${SESSION_ID}" || exit 2

"${SCRIPT_DIR}/status.sh" >/dev/null
REQUEST_FILE="$(submission_validate_receipt "${SESSION_ID}")"
readonly REQUEST_FILE
printf 'Replaying the byte-exact private receipt for %s. This cannot create a second operation.\n' \
  "${SESSION_ID}" >&2
submission_post_receipt "${SESSION_ID}" "${REQUEST_FILE}"
