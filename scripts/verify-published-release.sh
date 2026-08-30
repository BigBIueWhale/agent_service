#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/common.sh
source "${SCRIPT_DIR}/common.sh"

require_no_arguments "./scripts/verify-published-release.sh" "$@"
check_pinned_inputs
require_release_commit
require_published_release
# The offline archive is the release's only cross-host transport and its
# disaster-recovery path; a published release whose archive is absent,
# corrupt, or carrying different images than the pins is not verified. (The
# archive is gitignored, so this runs where the release was cut or restored —
# the same boundary the backend's archive discipline has.)
verify_service_archive
verify_service_archive_contents "${SERVICE_ARCHIVE_PATH}"

printf 'PUBLISHED RELEASE VERIFIED\n'
printf 'remote https://github.com/BigBIueWhale/agent_service\n'
printf 'branch master\n'
printf 'commit %s\n' "$(git -C "${PROJECT_DIR}" rev-parse --verify HEAD)"
