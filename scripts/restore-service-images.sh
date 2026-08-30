#!/usr/bin/env bash
# Restore the exact pinned agent_service component images from the offline
# archive the release bundled — the only cross-host transport for this stack,
# because the component builds are not bit-reproducible across hosts (layer
# timestamps are normalised, but toolchain byte differences still move the
# IDs). The archive carries the one set of bytes the release lock pins, so a
# second machine deploys the SAME images rather than a rebuild that merely
# claims the same sources.
set -Eeuo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/common.sh
source "${SCRIPT_DIR}/common.sh"

require_no_arguments "./scripts/restore-service-images.sh" "$@"
check_host_tools_and_versions
validate_release_lock

# Bytes fail closed before Docker sees anything: the pinned SHA256 is the
# archive's only trust anchor, so a stale bundle from an earlier release and
# a corrupt copy die identically on their hash — there is no name-derived
# second identity to trust or to drift.
verify_service_archive

printf 'Loading the exact pinned component images from the verified local archive...\n'
docker load --input "${SERVICE_ARCHIVE_PATH}"

# Loading proves nothing by itself; every image the archive claims to carry
# must now be present under its pinned tag with its pinned ID. Any mismatch
# is a wrong archive, and nothing is silently substituted.
declare -A expected=(
  [agent]="$(release_value '.images.agent')"
  [relay]="$(release_value '.images.relay')"
  [capture]="$(release_value '.images.capture')"
  [broker]="$(release_value '.images.broker')"
  [service]="$(release_value '.images.service')"
)
component_tag() {
  case "$1" in
    agent) lock_value '.agent.image_tag' ;;
    relay) lock_value '.relay.image_tag' ;;
    capture) lock_value '.capture.image_tag' ;;
    broker) lock_value '.broker.image_tag' ;;
    service) lock_value '.service.image_tag' ;;
    *) die "unknown component: $1" ;;
  esac
}
failures=()
for component in agent relay capture broker service; do
  tag="$(component_tag "${component}")"
  loaded="$(docker image inspect --format '{{.Id}}' "${tag}" 2>/dev/null || true)"
  if [[ "${loaded}" != "${expected[${component}]}" ]]; then
    failures+=("${component} (${tag}): expected ${expected[${component}]}; loaded ${loaded:-nothing}")
  fi
done
if ((${#failures[@]})); then
  die "Docker loaded image IDs that do not match the release lock." \
    "${failures[@]}"
fi

printf '\nRESTORED — exact pinned component images are available without a rebuild.\n'
for component in agent relay capture broker service; do
  printf '  %-8s %s\n' "${component}" "${expected[${component}]}"
done
