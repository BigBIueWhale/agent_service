#!/usr/bin/env bash
# Register a directory this host keeps release evidence in -- session records
# copied out of the service's store, their judgements, benchmark passes -- so
# that ./collect.sh reads it and keeps every release it names. A writer of
# evidence registers its store when it writes there; a person keeping evidence
# by hand runs this once for the directory they keep it in. Registering a store
# again changes nothing. A store stops counting when its registration file,
# which ./collect.sh names in its report, is removed.
set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/common.sh
source "${SCRIPT_DIR}/common.sh"

if (($# != 1)); then
  printf 'Usage: ./scripts/register-evidence-store.sh <directory>\n' >&2
  exit 2
fi
command -v jq >/dev/null 2>&1 || die "Required host tool is missing: jq"
register_evidence_store "$1"
