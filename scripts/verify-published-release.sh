#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/common.sh
source "${SCRIPT_DIR}/common.sh"

require_no_arguments "./scripts/verify-published-release.sh" "$@"
check_pinned_inputs
require_release_commit
require_published_release

printf 'PUBLISHED RELEASE VERIFIED\n'
printf 'remote https://github.com/BigBIueWhale/agent_service\n'
printf 'branch master\n'
printf 'commit %s\n' "$(git -C "${PROJECT_DIR}" rev-parse --verify HEAD)"
