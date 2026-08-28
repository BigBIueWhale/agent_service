#!/usr/bin/env bash
# One resumable paired pass of every eligible SWE-rebench task through the
# production agent service on the wire-transport release. For each task the
# exact materialized workspace is frozen into a hash-committed zip receipt,
# submitted once per history policy in the task's planned order, waited on by
# polling the connection-independent session resource, retrieved back over
# the connection with its bundle commitment, and graded by the preserved
# immutable per-task evaluator image. Infrastructure failures fail closed and
# are never recorded as model scores; a completed variant is never rerun and
# never overwritten.
set -Eeuo pipefail
umask 077

BENCH_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
SERVICE_ROOT="$(cd -- "${BENCH_ROOT}/../.." && pwd)"
readonly BENCH_ROOT SERVICE_ROOT

die() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }
require_equal() { [[ "$2" == "$3" ]] || die "$1 mismatch: expected ${3@Q}, observed ${2@Q}"; }

# Remove a composed workspace that may contain read-only cache directories.
# Go's module cache marks its directories mode 0555, so a plain `rm -rf` cannot
# unlink their contents and fails mid-pass. Only directories are made writable:
# the files inside are hardlinks (cp -al) into the shared materialized
# source/task-env, so chmod'ing a file would mutate that shared inode, whereas
# cp -al always creates fresh directories unique to this workspace. Cleanup is
# best-effort -- a leftover temp workspace is a bounded /tmp leak, never a
# reason to abort the pass.
discard_workspace() {
  local ws="$1"
  [[ -n "${ws}" && -d "${ws}" ]] || return 0
  find "${ws}" -type d ! -perm -u+w -exec chmod u+w {} + 2>/dev/null || true
  rm -rf -- "${ws}" 2>/dev/null || true
}

if (($# != 2)); then
  die 'usage: ./full-suite-run.sh <shard_index> <shard_count>. This instance runs exactly the tasks whose 0-based plan position mod <shard_count> equals <shard_index>; the machines run disjoint shards with no shared runs tree and no cross-instance coordination. Use "0 1" for the whole suite on one machine.'
fi
# No leading zeros: bash arithmetic reads a zero-padded value as octal while jq
# --argjson reads it as decimal, so "010" would run one shard while the plan
# denominator and completion line describe another -- silently breaking the
# partition. Accept only 0 or an unpadded positive integer.
[[ "$1" =~ ^(0|[1-9][0-9]*)$ && "$2" =~ ^[1-9][0-9]*$ ]] ||
  die "shard index must be 0 or a positive integer with no leading zero, and count must be a positive integer with no leading zero; got ${1@Q} ${2@Q}"
readonly SHARD_INDEX="$1" SHARD_COUNT="$2"
(( SHARD_INDEX < SHARD_COUNT )) ||
  die "shard index ${SHARD_INDEX} must be strictly less than shard count ${SHARD_COUNT}"
for command in awk cmp cp curl date docker du find flock git grep gzip jq mkdir mktemp mv od rm sha256sum sleep sort stat sync tar timeout tr unzip wc xargs zip zstd; do
  command -v "${command}" >/dev/null || die "required command is missing: ${command}"
done

readonly API='http://127.0.0.1:8090'
readonly SUITE_ROOT="${BENCH_ROOT}/full-suite-v1"
# Pass v5 is the corrected-conditions pass: every variant's prompt is the
# committed harness preamble (time budget, grading mechanics, environment)
# plus the untouched task statement, and every workspace carries the task's
# warmed toolchain and dependency caches (.task-env.tar.gz, produced by
# warm-task-env.sh) so the agent can actually build and run tests offline.
# Earlier passes are retained as historical evidence: v3 (misleading
# read_file pages tool), v4 (pages fix alone), v5 (corrected conditions
# but the pre-guards backend), v6 (41 pairs under the v14-guarded backend).
# v7 is the first pass under the 200 GiB re-release, run with corrected task
# conditions. Eligibility was re-derived from verified evidence (symbolic-link
# sources are stageable, and the 200 GiB staging cap admits the two largest
# repositories), so the plan covers all 111 paired tasks. An initial v7 attempt
# was discarded rather than reported: forensics on its timed-out sessions showed
# every Rust task shipped an empty task environment (no cargo registry, no
# toolchain -- those agents could never build), the preamble did not say which
# tools exist or that extracting inside the workspace pollutes the graded patch,
# and three sessions did exactly that. All three are fixed here and in the
# warmer, so this pass measures the model rather than the harness.
readonly PASS_ROOT="${BENCH_ROOT}/full-suite-v9"
readonly RUNS_ROOT="${PASS_ROOT}/runs"
readonly PROVENANCE_ROOT="${PASS_ROOT}/release-provenance"
readonly PREAMBLE_FILE="${BENCH_ROOT}/prompt-preamble.md"
readonly TASK_ENV_ROOT="${BENCH_ROOT}/full-suite-v1/task-env"
readonly DATASET_ROOT="${BENCH_ROOT}/evaluator-dataset"
readonly PLAN_FILE="${SUITE_ROOT}/suite-plan.json"
# The agent's working budget is its turn count, enforced by the agent itself from
# the locked limits.max_session_turns; this harness imposes no wall-clock bound on
# a session. VERIFIER_TIMEOUT_SEC below bounds the deterministic evaluator
# container, which is ordinary offline test execution and not agent work.
readonly VERIFIER_TIMEOUT_SEC=3000
readonly TASK_CPUS=1
readonly TASK_MEMORY_MB=4096
readonly POLL_INTERVAL_SEC=20

# The submission machinery expects SCRIPT_DIR at the service root so receipts
# live in the service's own private runtime tree.
SCRIPT_DIR="${SERVICE_ROOT}"
readonly SCRIPT_DIR
# shellcheck source=../../scripts/submission-common.sh
source "${SERVICE_ROOT}/scripts/submission-common.sh"

# Staging caps the server enforces beyond the client transport cap. The
# graceful-exclusion guard in run_task needs them too; read fail-closed from the
# same lock the service validates against its compiled LimitsLock.
MAX_STAGED_BYTES="$(jq -er '.limits.max_staged_bytes | numbers' "${SERVICE_ROOT}/config/stack.lock.json")" ||
  die 'stack lock .limits.max_staged_bytes is absent or not a number'
MAX_STAGED_FILES="$(jq -er '.limits.max_staged_files | numbers' "${SERVICE_ROOT}/config/stack.lock.json")" ||
  die 'stack lock .limits.max_staged_files is absent or not a number'
MAX_STAGED_ENTRIES="$(jq -er '.limits.max_staged_entries | numbers' "${SERVICE_ROOT}/config/stack.lock.json")" ||
  die 'stack lock .limits.max_staged_entries is absent or not a number'
readonly MAX_STAGED_BYTES MAX_STAGED_FILES MAX_STAGED_ENTRIES

[[ -f "${PLAN_FILE}" ]] || die "suite plan is missing: ${PLAN_FILE}"
jq -e '(.runs | length) > 0 and .eligible_count == (.runs | length)
       and all(.runs[]; (.task_id | type == "string") and (.task_id | length > 0))' "${PLAN_FILE}" >/dev/null ||
  die 'suite plan is inconsistent: eligible_count must equal the run count, at least one run is required, and every run must carry a non-empty string task_id'
# Number of tasks this shard owns (0-based plan position mod count == index).
SHARD_TASK_COUNT="$(jq -er --argjson i "${SHARD_INDEX}" --argjson n "${SHARD_COUNT}" \
  '[.runs | keys[] | select(. % $n == $i)] | length' "${PLAN_FILE}")"
readonly SHARD_TASK_COUNT
(( SHARD_TASK_COUNT > 0 )) ||
  printf 'NOTE: shard %s/%s owns no tasks (shard count exceeds the plan length); this pass completes with zero task pairs.\n' \
    "${SHARD_INDEX}" "${SHARD_COUNT}" >&2

# ---------------------------------------------------------------------------
# Release provenance: recorded once when this pass starts; every later
# invocation must observe the identical release, or the pass refuses to mix
# two service versions into one suite.
# ---------------------------------------------------------------------------
mkdir -p -- "${PASS_ROOT}" "${RUNS_ROOT}"
chmod 0700 -- "${PASS_ROOT}" "${RUNS_ROOT}"

# One instance per shard per machine. Two instances of the same shard started
# near-simultaneously would both clear the "already has a running session" gate
# during a long grading window and interleave writes into one run_dir. A
# per-shard advisory lock, held open for the whole pass, makes same-shard
# concurrency impossible; disjoint shards take disjoint locks and run freely.
exec 9>"${PASS_ROOT}/.shard-${SHARD_INDEX}-of-${SHARD_COUNT}.lock"
flock -n 9 ||
  die "another instance of shard ${SHARD_INDEX}/${SHARD_COUNT} is already running against ${PASS_ROOT}"

RELEASE_LOCK_SHA="$(sha256sum -- "${SERVICE_ROOT}/config/release.lock.json" | awk '{print $1}')"
STACK_LOCK_SHA="$(sha256sum -- "${SERVICE_ROOT}/config/stack.lock.json" | awk '{print $1}')"
# The plan is untracked/unpinned by git; pin its exact bytes into this pass's
# provenance so a resumed or sibling-machine invocation cannot silently run a
# different task sequence (which would drop or duplicate tasks across the suite).
PLAN_SHA="$(sha256sum -- "${PLAN_FILE}" | awk '{print $1}')"
IMPLEMENTATION_COMMIT="$(jq -er '.implementation_commit' "${SERVICE_ROOT}/config/release.lock.json")"
SERVICE_IMAGE_ID="$(jq -er '.images.service' "${SERVICE_ROOT}/config/release.lock.json")"
AGENT_SANDBOX="$(jq -er '.agent.agent_exec_sandbox' "${SERVICE_ROOT}/config/stack.lock.json")"
readonly RELEASE_LOCK_SHA STACK_LOCK_SHA PLAN_SHA IMPLEMENTATION_COMMIT SERVICE_IMAGE_ID AGENT_SANDBOX
[[ -z "$(git -C "${SERVICE_ROOT}" status --porcelain=v1 --untracked-files=all)" ]] ||
  die 'agent_service worktree must be clean before a suite pass'

if [[ -e "${PROVENANCE_ROOT}/release.json" ]]; then
  jq -e --arg release "${RELEASE_LOCK_SHA}" --arg stack "${STACK_LOCK_SHA}" \
    --arg commit "${IMPLEMENTATION_COMMIT}" --arg plan "${PLAN_SHA}" \
    '.release_lock_sha256 == $release and .stack_lock_sha256 == $stack
     and .implementation_commit == $commit and .plan_sha256 == $plan' \
    "${PROVENANCE_ROOT}/release.json" >/dev/null ||
    die "this pass was started on a different release or suite plan; refusing to mix them in ${PASS_ROOT}"
else
  mkdir -p -- "${PROVENANCE_ROOT}"
  chmod 0700 -- "${PROVENANCE_ROOT}"
  # Atomic publish: a crash mid-write must never leave a corrupt release.json
  # that wedges every future invocation at the check above.
  jq -n --arg release "${RELEASE_LOCK_SHA}" --arg stack "${STACK_LOCK_SHA}" \
    --arg commit "${IMPLEMENTATION_COMMIT}" --arg service_image "${SERVICE_IMAGE_ID}" \
    --arg plan "${PLAN_SHA}" \
    '{schema_version:2,release_lock_sha256:$release,stack_lock_sha256:$stack,
      implementation_commit:$commit,service_image:$service_image,plan_sha256:$plan}' \
    >"${PROVENANCE_ROOT}/release.json.partial"
  sync -f -- "${PROVENANCE_ROOT}/release.json.partial"
  mv -- "${PROVENANCE_ROOT}/release.json.partial" "${PROVENANCE_ROOT}/release.json"
  sync -f -- "${PROVENANCE_ROOT}"
fi

printf 'Validating the live production release before touching any task...\n' >&2
STATUS_SNAPSHOT="$(mktemp "${PROVENANCE_ROOT}/status.XXXXXX")"
(cd "${SERVICE_ROOT}" && ./status.sh) >"${STATUS_SNAPSHOT}" 2>&1 ||
  { tail -5 "${STATUS_SNAPSHOT}" >&2; die 'production status validation failed'; }
grep -Fqx 'READY — one defensively validated mode only' "${STATUS_SNAPSHOT}" ||
  die 'production status did not reach its exact ready terminal'
mv -- "${STATUS_SNAPSHOT}" "${PROVENANCE_ROOT}/status-latest.txt"
require_equal 'live service image' \
  "$(docker inspect --format '{{.Image}}' "$(jq -er '.service.container_name' "${SERVICE_ROOT}/config/stack.lock.json")")" \
  "${SERVICE_IMAGE_ID}"
curl --noproxy '*' --fail-with-body --silent --show-error "${API}/v1/agent/sessions" |
  jq -e '.sessions | map(select(.status == "running")) | length == 0' >/dev/null ||
  die 'production service already has a running session'

# ---------------------------------------------------------------------------
# Per-task machinery
# ---------------------------------------------------------------------------

verify_task_source() {
  local task_id="$1" task_dir="$2" marker="$3"
  # Test the marker's content, not just its existence: a crash between creating
  # the file and writing "verified" would otherwise let an empty marker skip
  # verification forever. A missing or torn marker re-verifies from scratch.
  if [[ -f "${marker}" && "$(cat -- "${marker}" 2>/dev/null)" == verified ]]; then
    printf 'source already verified for %s\n' "${task_id}" >&2
    return 0
  fi
  printf 'Verifying materialized source integrity for %s...\n' "${task_id}" >&2
  local scratch
  scratch="$(mktemp -d /tmp/qwen38-suite-verify.XXXXXX)"
  (
    cd "${task_dir}/source"
    find . -type f -print0 | LC_ALL=C sort -z | xargs -0 -r sha256sum --zero --
  ) >"${scratch}/regular.sha256z"
  cmp -- "${task_dir}/source-regular.sha256z" "${scratch}/regular.sha256z" ||
    { rm -rf -- "${scratch}"; die "materialized source content drifted for ${task_id}"; }
  (
    cd "${task_dir}/source"
    find . -mindepth 1 -printf '%y %m %P -> %l\0' | LC_ALL=C sort -z
  ) >"${scratch}/modes.z"
  cmp -- "${task_dir}/source-modes.z" "${scratch}/modes.z" ||
    { rm -rf -- "${scratch}"; die "materialized source modes/layout drifted for ${task_id}"; }
  rm -rf -- "${scratch}"
  require_equal "recorded regular manifest for ${task_id}" \
    "$(sha256sum -- "${task_dir}/source-regular.sha256z" | awk '{print $1}')" \
    "$(jq -er '.source.regular_manifest_sha256' "${task_dir}/manifest.json")"
  require_equal "recorded mode manifest for ${task_id}" \
    "$(sha256sum -- "${task_dir}/source-modes.z" | awk '{print $1}')" \
    "$(jq -er '.source.mode_manifest_sha256' "${task_dir}/manifest.json")"
  require_equal "task base commit for ${task_id}" \
    "$(git -C "${task_dir}/source" rev-parse HEAD)" \
    "$(jq -er '.source.base_commit' "${task_dir}/manifest.json")"
  printf 'verified\n' >"${marker}.partial"
  sync -f -- "${marker}.partial"
  mv -- "${marker}.partial" "${marker}"
  sync -f -- "${marker%/*}"
}

ensure_environment_image() {
  local task_id="$1" task_dir="$2"
  local image_tag image_id observed
  image_tag="$(jq -er '.environment.image_tag' "${task_dir}/manifest.json")"
  image_id="$(jq -er '.environment.image_id' "${task_dir}/manifest.json")"
  observed="$(docker image inspect --format '{{.Id}}' "${image_tag}" 2>/dev/null || true)"
  if [[ "${observed}" != "${image_id}" ]]; then
    printf 'Loading preserved evaluator image for %s...\n' "${task_id}" >&2
    require_equal "evaluator archive for ${task_id}" \
      "$(sha256sum -- "${task_dir}/environment-image.tar" | awk '{print $1}')" \
      "$(jq -er '.environment.archive_sha256' "${task_dir}/manifest.json")"
    docker load --input "${task_dir}/environment-image.tar" >/dev/null
    observed="$(docker image inspect --format '{{.Id}}' "${image_tag}")"
  fi
  require_equal "evaluator image for ${task_id}" "${observed}" "${image_id}"
}

verify_dataset_inputs() {
  local task_id="$1" task_dir="$2"
  local dataset_dir="${DATASET_ROOT}/${task_id}"
  [[ -d "${dataset_dir}/tests" && -f "${dataset_dir}/instruction.md" ]] ||
    die "harbor dataset entry is incomplete for ${task_id}"
  require_equal "instruction for ${task_id}" \
    "$(sha256sum -- "${dataset_dir}/instruction.md" | awk '{print $1}')" \
    "$(jq -er '.inputs.instruction_sha256' "${task_dir}/manifest.json")"
  require_equal "test.sh for ${task_id}" \
    "$(sha256sum -- "${dataset_dir}/tests/test.sh" | awk '{print $1}')" \
    "$(jq -er '.inputs.test_sh_sha256' "${task_dir}/manifest.json")"
  require_equal "test config for ${task_id}" \
    "$(sha256sum -- "${dataset_dir}/tests/config.json" | awk '{print $1}')" \
    "$(jq -er '.inputs.test_config_sha256' "${task_dir}/manifest.json")"
  require_equal "test parser for ${task_id}" \
    "$(sha256sum -- "${dataset_dir}/tests/swan_log_parsers.py" | awk '{print $1}')" \
    "$(jq -er '.inputs.test_parser_sha256' "${task_dir}/manifest.json")"
}

# Wait for the session to reach its own terminal state. There is deliberately no
# deadline here: a session is bounded by its turn budget (limits.max_session_turns
# in the stack lock, enforced by the agent itself), which is a hardware-independent
# measure of agent progress, whereas a wall-clock cap would score the same
# trajectory differently depending on how fast the backend happens to generate.
poll_until_terminal() {
  local session_id="$1" terminal_file="$2"
  local body status
  while :; do
    body="$(mktemp /tmp/qwen38-suite-poll.XXXXXX)"
    # The GET is idempotent; a transient transport error or non-200 must not
    # kill a pass that may have run for hours. Retry with backoff; die only when
    # the connection-independent resource is persistently unreachable.
    local http poll_attempt=0
    while :; do
      http="$(curl --noproxy '*' --silent --show-error --connect-timeout 5 --max-time 30 \
        --output "${body}" --write-out '%{http_code}' \
        "${API}/v1/agent/sessions/${session_id}")" && [[ "${http}" == 200 ]] && break
      poll_attempt=$((poll_attempt + 1))
      if (( poll_attempt >= 5 )); then
        rm -f -- "${body}"
        die "session poll for ${session_id} failed after 5 attempts (last: HTTP ${http:-transport-error})"
      fi
      printf 'WARN: session poll for %s attempt %s failed (HTTP %s); retrying.\n' "${session_id}" "${poll_attempt}" "${http:-transport-error}" >&2
      sleep $((poll_attempt * 3))
      : > "${body}"
    done
    status="$(jq -er '.status' "${body}")"
    if [[ "${status}" != running ]]; then
      mv -- "${body}" "${terminal_file}"
      return 0
    fi
    rm -f -- "${body}"
    sleep "${POLL_INTERVAL_SEC}"
  done
}

run_variant() {
  local task_id="$1" task_dir="$2" ordinal="$3" policy="$4" label="$5"
  local run_dir="${RUNS_ROOT}/${task_id}/${ordinal}-${label}"
  local dataset_dir="${DATASET_ROOT}/${task_id}"
  if [[ -f "${run_dir}/result.json" ]]; then
    printf 'Variant %s of %s already has an accepted result; skipping.\n' "${label}" "${task_id}" >&2
    return 0
  fi
  [[ ! -e "${run_dir}" ]] ||
    die "partial variant evidence already exists at ${run_dir}; archive it explicitly before rerunning"
  # Non-recursive mkdir is the atomic mutual exclusion: the parent (the task's
  # runs dir) already exists from run_task, and a plain mkdir fails closed if the
  # run_dir appeared between the check above and here.
  mkdir -- "${run_dir}"
  chmod 0700 -- "${run_dir}"

  local task_env_image task_base_commit task_working_dir
  task_env_image="$(jq -er '.environment.image_tag' "${task_dir}/manifest.json")"
  task_base_commit="$(jq -er '.source.base_commit' "${task_dir}/manifest.json")"
  task_working_dir="$(jq -er '.environment.working_dir' "${task_dir}/manifest.json")"
  [[ "${task_working_dir}" == /* ]] || die "evaluator working dir is not absolute for ${task_id}"

  # --- submission over the connection -------------------------------------
  local handle_hex session_id request_file created
  handle_hex="$(LC_ALL=C od -An -N32 -tx1 /dev/urandom | tr -d ' \n')"
  [[ "${handle_hex}" =~ ^[0-9a-f]{64}$ ]] || die 'CSPRNG did not produce 32 bytes'
  session_id="s-${handle_hex}"
  printf '%s\n' "${session_id}" >"${run_dir}/session-id.txt"
  # Corrected-conditions submission: the prompt is the committed harness
  # preamble plus the untouched task statement, and the workspace is the
  # verified materialized source plus the task's warmed toolchain tarball
  # (hardlink copy of the source keeps this cheap; the composed tree is
  # only needed while the receipt zip is built).
  local env_dir="${TASK_ENV_ROOT}/${task_id}"
  [[ -f "${env_dir}/task-env.tar.gz" && -f "${env_dir}/env-manifest.json" ]] ||
    die "task environment is not warmed for ${task_id}; run ./warm-task-env.sh first"
  # Prove the archive is a valid, extractable tarball -- the exact property the
  # agent relies on (it runs 'tar -xzf .task-env.tar.gz'). The sha-vs-manifest
  # check below only proves the bytes equal what the manifest committed to,
  # which is NOT integrity: a manifest can faithfully hash a truncated archive
  # (this is how 11 corrupt task-envs shipped). This gate rejects that class.
  gzip -t -- "${env_dir}/task-env.tar.gz" 2>/dev/null ||
    die "task-env archive for ${task_id} fails gzip integrity (gzip -t); re-warm it -- shipping a corrupt cache wastes the agent's entire budget on recovery"
  tar -tzf "${env_dir}/task-env.tar.gz" >/dev/null 2>&1 ||
    die "task-env archive for ${task_id} is not a listable tar (tar -tzf); re-warm it"
  require_equal "task-env tarball hash for ${task_id}" \
    "$(sha256sum -- "${env_dir}/task-env.tar.gz" | awk '{print $1}')" \
    "$(jq -er '.tar_sha256' "${env_dir}/env-manifest.json")"
  cp -- "${env_dir}/env-manifest.json" "${run_dir}/task-env-manifest.json"

  local composed_prompt="${run_dir}/prompt.txt" composed_ws
  cat -- "${PREAMBLE_FILE}" "${dataset_dir}/instruction.md" >"${composed_prompt}"
  local prompt_sha
  prompt_sha="$(sha256sum -- "${composed_prompt}" | awk '{print $1}')"

  composed_ws="$(mktemp -d /tmp/qwen38-suite-ws.XXXXXX)"
  cp -al -- "${task_dir}/source/." "${composed_ws}/" ||
    { discard_workspace "${composed_ws}"; die "workspace hardlink copy failed for ${task_id}"; }
  cp -- "${env_dir}/task-env.tar.gz" "${composed_ws}/.task-env.tar.gz"
  request_file="$(submission_create_receipt "${session_id}" "${composed_ws}" "${composed_prompt}" "${policy}")" ||
    { discard_workspace "${composed_ws}"; die "receipt construction failed for ${task_id} ${label}"; }
  discard_workspace "${composed_ws}"
  created="${run_dir}/created.json"
  submission_post_receipt "${session_id}" "${request_file}" >"${created}" ||
    die "submission failed for ${task_id} ${label}"
  jq -e --arg id "${session_id}" --argjson policy "${policy}" \
    '.session_id == $id and .status == "running" and .model == "qwen3.8-27b-nvfp4-k8v4" and
     .context_window == 262144 and .preserve_thinking == $policy' \
    "${created}" >/dev/null || die 'session creation body violated the locked contract'
  printf 'Production session %s is running (%s, preserve_thinking=%s); polling the connection-independent resource.\n' \
    "${session_id}" "${task_id}" "${policy}" >&2

  # --- terminal ------------------------------------------------------------
  local terminal="${run_dir}/terminal.json"
  poll_until_terminal "${session_id}" "${terminal}"
  jq -e --arg id "${session_id}" --argjson policy "${policy}" \
    '.session_id == $id and .status == "completed" and
     .model == "qwen3.8-27b-nvfp4-k8v4" and .context_window == 262144 and
     .preserve_thinking == $policy and .finished_at_unix > 0 and
     (.bundle_sha256 | test("^[0-9a-f]{64}$")) and .bundle_compressed_bytes > 0 and
     .bundle_file_count > 0 and .raw_session_tree_retained == false' \
    "${terminal}" >/dev/null || die 'production terminal body or required bundle contract failed'
  # Recorded agent failure (is_process_error / non-empty teardown_diagnostics,
  # e.g. Qwen's loop-detection halt) is classifiable evidence for
  # production_agent_process_failure, never a reason to kill the pass.
  # Infrastructure teardown truth is observed directly instead: no container
  # owned by this session may survive its terminal record.
  # Capture the listing, then grep the string. `docker ps | grep -q` lets grep
  # close the pipe on first match; under pipefail the resulting SIGPIPE (141) on
  # docker would make this leak check silently pass despite a surviving container.
  local live_container_names
  live_container_names="$(docker ps --all --format '{{.Names}}')"
  if grep -qF -- "${session_id}" <<<"${live_container_names}"; then
    die "session containers survived teardown for ${session_id}"
  fi

  # --- bundle over the connection -----------------------------------------
  # The bundle is the longest single transfer (bundle.sh uses --max-time 900);
  # one transient reset after hours of work must not kill the pass. bundle.sh is
  # idempotent -- it verifies the sha against the resource and its header and
  # publishes with `mv --no-clobber`, so a failed attempt publishes nothing and
  # repeating the identical download is safe.
  local bundle_attempt=0
  until "${SERVICE_ROOT}/bundle.sh" "${session_id}" "${run_dir}/production-bundle.tar.zst" >&2; do
    bundle_attempt=$((bundle_attempt + 1))
    (( bundle_attempt < 5 )) ||
      die "bundle download for ${session_id} failed after 5 attempts"
    printf 'WARN: bundle download for %s attempt %s failed; retrying.\n' "${session_id}" "${bundle_attempt}" >&2
    sleep $((bundle_attempt * 3))
  done
  local bundle_sha
  bundle_sha="$(sha256sum -- "${run_dir}/production-bundle.tar.zst" | awk '{print $1}')"
  require_equal 'downloaded bundle hash' "${bundle_sha}" "$(jq -er '.bundle_sha256' "${terminal}")"
  printf '%s  production-bundle.tar.zst\n' "${bundle_sha}" >"${run_dir}/production-bundle.sha256"

  mkdir -- "${run_dir}/bundle"
  tar --zstd --extract --no-same-owner --file "${run_dir}/production-bundle.tar.zst" \
    --directory "${run_dir}/bundle"
  [[ -d "${run_dir}/bundle/staged" && ! -L "${run_dir}/bundle/staged" ]] || die 'bundle lacks a real staged workspace'
  cmp -- "${composed_prompt}" "${run_dir}/bundle/control/prompt.txt" ||
    die 'bundle prompt differs from the composed preamble+instruction prompt'
  jq -e --argjson policy "${policy}" '.preserve_thinking == $policy and (keys == ["preserve_thinking"])' \
    "${run_dir}/bundle/control/history-policy.json" >/dev/null || die 'bundle history policy mismatch'
  jq -e --argjson policy "${policy}" --arg sandbox "${AGENT_SANDBOX}" \
    '.model == "qwen3.8-27b-nvfp4-k8v4" and .context_window == 262144 and
     .preserve_thinking == $policy and .sandbox == $sandbox' \
    "${run_dir}/bundle/output/ready.json" >/dev/null || die 'bundle readiness record mismatch'

  # --- trusted candidate patch --------------------------------------------
  # Defensive de-pollution. The prompt tells the agent to extract
  # .task-env.tar.gz under /tmp, but an agent can still unpack it inside the
  # workspace -- including at the workspace root, by running tar without -C.
  # Those files are the environment this harness shipped, not the agent's work,
  # and must never enter the candidate patch: earlier runs carried ~22,800 extra
  # files and up to 300 MB of environment noise in patches whose real change was
  # a handful of source files. Exclude only trees proven to be an extraction of
  # THIS task's environment, identified by an env.sh byte-identical to the one
  # inside its archive; nothing is excluded on the basis of its name alone.
  local staged_root="${run_dir}/bundle/staged"
  local env_sh_sha task_env_excludes="" candidate_env rel member
  env_sh_sha="$(tar -xzOf "${env_dir}/task-env.tar.gz" --occurrence=1 ./env.sh | sha256sum | awk '{print $1}')" ||
    die "cannot read env.sh from the task-env archive for ${task_id}"
  while IFS= read -r -d '' candidate_env; do
    [[ "$(sha256sum -- "${candidate_env}" | awk '{print $1}')" == "${env_sh_sha}" ]] || continue
    rel="${candidate_env#"${staged_root}/"}"
    rel="${rel%/env.sh}"
    if [[ "${rel}" == env.sh ]]; then
      # Extracted directly onto the workspace root: exclude exactly the
      # archive's own top-level members, and nothing else.
      while IFS= read -r member; do
        [[ -n "${member}" ]] || continue
        task_env_excludes+="./${member}"$'\n'
      done < <(tar -tzf "${env_dir}/task-env.tar.gz" | sed -e 's|^\./||' -e 's|/.*$||' | LC_ALL=C sort -u)
    else
      task_env_excludes+="./${rel}"$'\n'
    fi
  done < <(find "${staged_root}" -type f -name env.sh -print0)
  [[ -z "${task_env_excludes}" ]] ||
    printf 'NOTE: %s %s extracted the task environment inside the workspace; excluding it from the candidate patch:\n%s' \
      "${task_id}" "${label}" "${task_env_excludes}" >&2

  mkdir -- "${run_dir}/patch"
  docker run --rm --network none --security-opt no-new-privileges \
    --cpus "${TASK_CPUS}" --memory 2048m --memory-swap 2048m --pids-limit 512 \
    --env TASK_BASE_COMMIT="${task_base_commit}" \
    --env TASK_WORKING_DIR="${task_working_dir}" \
    --env TASK_ENV_EXCLUDES="${task_env_excludes}" \
    --mount "type=bind,src=${run_dir}/bundle/staged,dst=/candidate,readonly" \
    --mount "type=bind,src=${run_dir}/patch,dst=/out" \
    --entrypoint bash "${task_env_image}" -Eeuo pipefail -c '
      cd "$TASK_WORKING_DIR"
      test "$(git rev-parse HEAD)" = "$TASK_BASE_COMMIT"
      # Some dataset images deliberately ship a dirty baked worktree (for
      # example offline-build pom fixes). That baseline is recorded as
      # evidence and cross-checked against the materializer'"'"'s own
      # recording below; the grader reconstructs base + candidate.patch, so
      # the baked fixes flow through the patch and grading stays exact.
      git status --porcelain=v1 --untracked-files=all > /out/baseline.status
      git clean -ffdqx
      find . -mindepth 1 -maxdepth 1 ! -name .git -exec rm -rf -- {} +
      excludes=(--exclude=./.git --exclude=.git --exclude=./.task-env.tar.gz --exclude=.task-env.tar.gz)
      while IFS= read -r excluded_path; do
        [ -n "$excluded_path" ] || continue
        excludes+=("--exclude=$excluded_path")
      done <<< "$TASK_ENV_EXCLUDES"
      tar -C /candidate "${excludes[@]}" -cf - . |
        tar -C "$TASK_WORKING_DIR" --no-same-owner --no-overwrite-dir -xf -
      git add --all -- .
      git diff --cached --binary --full-index "$TASK_BASE_COMMIT" -- > /out/candidate.patch
      git diff --cached --stat -- > /out/candidate.stat
      git diff --cached --name-status -- > /out/candidate.name-status
      chmod 0644 /out/baseline.status /out/candidate.patch /out/candidate.stat /out/candidate.name-status
    ' || die "candidate patch construction failed for ${task_id} ${label}"
  [[ -f "${run_dir}/patch/candidate.patch" && ! -L "${run_dir}/patch/candidate.patch" ]] ||
    die 'trusted patch construction failed'
  # The observed baked baseline must byte-match what materialization
  # recorded from this exact image; anything else is working-tree drift.
  tr '\0' '\n' <"${task_dir}/initial-git-status.z" | cmp -s - "${run_dir}/patch/baseline.status" ||
    die "environment image working-tree drift for ${task_id}: observed baseline differs from materialized initial-git-status"
  local patch_sha patch_bytes
  patch_sha="$(sha256sum -- "${run_dir}/patch/candidate.patch" | awk '{print $1}')"
  patch_bytes="$(stat -c '%s' -- "${run_dir}/patch/candidate.patch")"

  # --- immutable evaluator -------------------------------------------------
  mkdir -- "${run_dir}/grader-logs" "${run_dir}/grader-logs/verifier"
  local grader_name grader_rc reward classification
  grader_name="qwen38-swerebench-${session_id:2:24}"
  [[ -z "$(docker ps -a --filter "name=^/${grader_name}$" --format '{{.ID}}')" ]] ||
    die 'exact grader container name already exists'
  docker create \
    --name "${grader_name}" \
    --label qwen38.benchmark.role=post-session-verifier \
    --label "qwen38.benchmark.session=${session_id}" \
    --label "qwen38.benchmark.task=${task_id}" \
    --network bridge \
    --cpus "${TASK_CPUS}" --memory "${TASK_MEMORY_MB}m" --memory-swap "${TASK_MEMORY_MB}m" --pids-limit 4096 \
    --security-opt no-new-privileges \
    --env TASK_BASE_COMMIT="${task_base_commit}" \
    --env TASK_WORKING_DIR="${task_working_dir}" \
    --mount "type=bind,src=${run_dir}/patch/candidate.patch,dst=/candidate.patch,readonly" \
    --mount "type=bind,src=${dataset_dir}/tests,dst=/benchmark-tests,readonly" \
    --mount "type=bind,src=${run_dir}/grader-logs,dst=/logs" \
    --entrypoint bash "${task_env_image}" -Eeuo pipefail -c '
      cd "$TASK_WORKING_DIR"
      test "$(git rev-parse HEAD)" = "$TASK_BASE_COMMIT"
      # A baked-dirty worktree is legitimate dataset state; the reset and
      # clean below reconstruct pristine base, and the candidate patch
      # carries every intended deviation (baked fixes plus agent work).
      git reset --hard "$TASK_BASE_COMMIT"
      git clean -ffdqx
      if test -s /candidate.patch; then
        # Worktree-only apply, never --index: the dataset'"'"'s test.sh guards
        # expect candidate-created files to be untracked (it removes a
        # colliding test path only when untracked before applying its own
        # test patch). Rewards read worktree bytes, so grading is identical.
        git apply --binary --check /candidate.patch
        git apply --binary /candidate.patch
      fi
      rm -rf /tests
      cp -a /benchmark-tests /tests
      exec bash /tests/test.sh
    ' >/dev/null || die "grader container creation failed for ${task_id} ${label}"
  printf 'Grading %s session %s in the preserved evaluator image.\n' "${task_id}" "${session_id}" >&2
  set +e
  timeout --signal=TERM --kill-after=10s "${VERIFIER_TIMEOUT_SEC}s" \
    docker start --attach "${grader_name}" >"${run_dir}/grader.log" 2>&1
  grader_rc=$?
  set -e
  if [[ "${grader_rc}" == 124 || "${grader_rc}" == 137 ]]; then
    docker stop --time 30 "${grader_name}" >/dev/null
  fi
  docker inspect "${grader_name}" >"${run_dir}/grader-finished-inspect.json"
  jq -e 'length == 1 and .[0].State.Running == false and .[0].State.OOMKilled == false' \
    "${run_dir}/grader-finished-inspect.json" >/dev/null ||
    die 'grader did not terminate cleanly or was OOM-killed'
  docker rm "${grader_name}" >/dev/null
  if [[ "${grader_rc}" == 124 || "${grader_rc}" == 137 ]]; then
    die 'official verifier timed out; this is not a model score'
  fi
  [[ -f "${run_dir}/grader-logs/verifier/reward.txt" && -f "${run_dir}/grader-logs/verifier/report.json" ]] ||
    die 'official verifier did not write its reward and report'
  reward="$(tr -d '[:space:]' <"${run_dir}/grader-logs/verifier/reward.txt")"
  [[ "${reward}" == 0 || "${reward}" == 1 ]] || die "invalid verifier reward: ${reward}"
  jq -e --argjson resolved "${reward}" '.resolved == ($resolved == 1)' \
    "${run_dir}/grader-logs/verifier/report.json" >/dev/null || die 'verifier reward/report mismatch'

  # Exit 53 is qwen-code reporting that the session reached its locked turn
  # budget. That is an ordinary terminal outcome, graded on the work actually
  # done, never an infrastructure failure -- so it is not a process error, and a
  # budget-exhausted session whose patch resolves the task is recorded resolved.
  local turn_budget_exhausted=false
  jq -e '.agent_exit_code == 53' "${terminal}" >/dev/null && turn_budget_exhausted=true
  if jq -e '.is_process_error == true
            or (.agent_exit_code != 0 and .agent_exit_code != 53)
            or (.container_exit_code != 0 and .container_exit_code != 53)' "${terminal}" >/dev/null; then
    classification=production_agent_process_failure
  elif [[ "${reward}" == 1 ]]; then
    classification=resolved
  elif [[ "${turn_budget_exhausted}" == true ]]; then
    classification=turn_budget_exhausted
  else
    classification=unresolved
  fi

  jq -n \
    --slurpfile created "${created}" \
    --slurpfile terminal "${terminal}" \
    --slurpfile verifier "${run_dir}/grader-logs/verifier/report.json" \
    --slurpfile release "${PROVENANCE_ROOT}/release.json" \
    --arg task_id "${task_id}" \
    --arg ordinal "${ordinal}" \
    --argjson preserve_thinking "${policy}" \
    --arg session_id "${session_id}" \
    --arg bundle_sha256 "${bundle_sha}" \
    --arg patch_sha256 "${patch_sha}" \
    --argjson patch_bytes "${patch_bytes}" \
    --argjson grader_exit_code "${grader_rc}" \
    --argjson reward "${reward}" \
    --arg classification "${classification}" \
    --argjson turn_budget_exhausted "${turn_budget_exhausted}" \
    --arg prompt_sha256 "${prompt_sha}" \
    --slurpfile task_env "${run_dir}/task-env-manifest.json" \
    '{
      schema_version:3,
      task_id:$task_id,
      release:$release[0],
      prompt_sha256:$prompt_sha256,
      task_env:$task_env[0],
      run_order:($ordinal|tonumber),
      variant:{preserve_thinking:$preserve_thinking},
      production:{created:$created[0],terminal:$terminal[0],session_id:$session_id,turn_budget_exhausted:$turn_budget_exhausted},
      evidence:{bundle_sha256:$bundle_sha256,candidate_patch_sha256:$patch_sha256,candidate_patch_bytes:$patch_bytes},
      verifier:{exit_code:$grader_exit_code,reward:$reward,report:$verifier[0]},
      outcome:{classification:$classification,resolved:($reward == 1)}
    }' >"${run_dir}/result.json.partial"
  [[ -s "${run_dir}/result.json.partial" ]] ||
    die "result summary for ${task_id} ${label} serialized to an empty file"
  sync -f -- "${run_dir}/result.json.partial"
  mv -- "${run_dir}/result.json.partial" "${run_dir}/result.json"
  sync -f -- "${run_dir}"
  printf 'Variant complete: %s %s reward=%s classification=%s turns=%s input_tokens_visible_in_bundle\n' \
    "${task_id}" "${label}" "${reward}" "${classification}" "$(jq -er '.num_turns' "${terminal}")" >&2
}

run_task() {
  local task_id="$1"
  local task_dir="${SUITE_ROOT}/materialization/${task_id}"
  local target="${RUNS_ROOT}/${task_id}"
  [[ -d "${task_dir}" ]] || die "materialized task is missing: ${task_dir}"
  if [[ -f "${target}/pair-summary.json" ]]; then
    printf 'Task %s already has an accepted pair summary; skipping.\n' "${task_id}" >&2
    return 0
  fi
  mkdir -p -- "${target}"
  chmod 0700 -- "${target}"

  # Fail-closed staging/transport guard. The composed workspace the service
  # stages is the materialized source tree plus exactly one extra file,
  # .task-env.tar.gz, so it must satisfy every service cap -- not only the
  # archive-byte cap. The binding byte cap is max_staged_bytes (smaller than the
  # archive cap), and the service also caps staged file and entry counts. du -sb
  # reports directory-apparent size, an upper bound on the regular-file bytes the
  # service counts, so any task that passes here provably fits; one that fails is
  # recorded once as an infrastructure exclusion instead of aborting the whole
  # pass when the service later rejects the receipt.
  local env_file="${TASK_ENV_ROOT}/${task_id}/task-env.tar.gz"
  [[ -f "${env_file}" ]] ||
    die "task environment is not warmed for ${task_id}; run ./warm-task-env.sh first"
  local src_bytes env_bytes staged_bytes src_files src_entries staged_files staged_entries
  local exclusion_reason=""
  src_bytes="$(du -sb -- "${task_dir}/source" | awk '{print $1}')"
  env_bytes="$(stat -c '%s' -- "${env_file}")"
  src_files="$(find "${task_dir}/source" -type f -printf '.' | wc -c)"
  src_entries="$(find "${task_dir}/source" -mindepth 1 -printf '.' | wc -c)"
  staged_bytes=$((src_bytes + env_bytes))
  staged_files=$((src_files + 1))
  staged_entries=$((src_entries + 1))
  if (( staged_bytes > MAX_STAGED_BYTES )); then
    exclusion_reason="staged bytes ~${staged_bytes} exceed max_staged_bytes ${MAX_STAGED_BYTES}"
  elif (( staged_bytes > SUBMISSION_MAX_ARCHIVE_BYTES )); then
    exclusion_reason="estimated archive bytes ~${staged_bytes} exceed max_archive_bytes ${SUBMISSION_MAX_ARCHIVE_BYTES}"
  elif (( staged_files > MAX_STAGED_FILES )); then
    exclusion_reason="staged files ${staged_files} exceed max_staged_files ${MAX_STAGED_FILES}"
  elif (( staged_entries > MAX_STAGED_ENTRIES )); then
    exclusion_reason="staged entries ${staged_entries} exceed max_staged_entries ${MAX_STAGED_ENTRIES}"
  fi
  if [[ -n "${exclusion_reason}" ]]; then
    printf 'EXCLUDING %s: %s; recording an infrastructure exclusion.\n' "${task_id}" "${exclusion_reason}" >&2
    jq -n --arg task "${task_id}" --arg reason "${exclusion_reason}" \
      --argjson staged_bytes "${staged_bytes}" --argjson staged_files "${staged_files}" \
      --argjson staged_entries "${staged_entries}" \
      --argjson max_staged_bytes "${MAX_STAGED_BYTES}" --argjson max_archive_bytes "${SUBMISSION_MAX_ARCHIVE_BYTES}" \
      --argjson max_staged_files "${MAX_STAGED_FILES}" --argjson max_staged_entries "${MAX_STAGED_ENTRIES}" \
      '{schema_version:2, task_id:$task, excluded:true,
        exclusion:{reason:"composed_workspace_exceeds_service_staging_cap", detail:$reason,
                   estimated:{staged_bytes:$staged_bytes, staged_files:$staged_files, staged_entries:$staged_entries},
                   caps:{max_staged_bytes:$max_staged_bytes, max_archive_bytes:$max_archive_bytes,
                         max_staged_files:$max_staged_files, max_staged_entries:$max_staged_entries}},
        runs:[], paired:null}' >"${target}/pair-summary.json.partial"
    [[ -s "${target}/pair-summary.json.partial" ]] ||
      die "exclusion summary for ${task_id} serialized to an empty file"
    sync -f -- "${target}/pair-summary.json.partial"
    mv -- "${target}/pair-summary.json.partial" "${target}/pair-summary.json"
    sync -f -- "${target}"
    return 0
  fi

  verify_task_source "${task_id}" "${task_dir}" "${target}/source-verified.txt"
  verify_dataset_inputs "${task_id}" "${task_dir}"
  ensure_environment_image "${task_id}" "${task_dir}"

  local order ordinal label policy
  order="$(jq -cer '.policy_order' "${task_dir}/manifest.json")"
  # The pair aggregation below pivots on exactly one false and one true variant.
  # A policy_order that is not a permutation of [false, true] would make the
  # paired select()s empty, and jq -n would then write a zero-byte pair-summary
  # (proven: `jq -n '{a:(empty)}'` emits nothing and exits 0), marking the task
  # complete forever with no result. Reject anything else fail-closed.
  jq -e 'type == "array" and length == 2 and sort == [false, true]' <<<"${order}" >/dev/null ||
    die "policy_order for ${task_id} is not a permutation of [false, true]: ${order}"
  local index=0 first_result="" second_result=""
  for policy in $(jq -r '.[]' <<<"${order}"); do
    index=$((index + 1))
    ordinal="$(printf '%02d' "${index}")"
    if [[ "${policy}" == false ]]; then label=unpreserved; else label=preserved; fi
    run_variant "${task_id}" "${task_dir}" "${ordinal}" "${policy}" "${label}"
    # Aggregate from the exact variant directory this loop produced, never a
    # glob that could also match an archived sibling (e.g. 01-*.archived).
    if (( index == 1 )); then first_result="${target}/${ordinal}-${label}/result.json"
    else second_result="${target}/${ordinal}-${label}/result.json"; fi
  done
  [[ -f "${first_result}" && -f "${second_result}" ]] ||
    die "pair aggregation for ${task_id} is missing an exact variant result.json"

  jq -n \
    --slurpfile first "${first_result}" \
    --slurpfile second "${second_result}" \
    --arg task_id "${task_id}" \
    '{
      schema_version:2,
      task_id:$task_id,
      runs:[$first[0],$second[0]],
      paired:{
        resolved:{
          unpreserved:([$first[0],$second[0]][] | select(.variant.preserve_thinking == false) | .outcome.resolved),
          preserved:([$first[0],$second[0]][] | select(.variant.preserve_thinking == true) | .outcome.resolved)
        },
        turns:{
          unpreserved:([$first[0],$second[0]][] | select(.variant.preserve_thinking == false) | .production.terminal.num_turns),
          preserved:([$first[0],$second[0]][] | select(.variant.preserve_thinking == true) | .production.terminal.num_turns)
        }
      }
    }' >"${target}/pair-summary.json.partial"
  [[ -s "${target}/pair-summary.json.partial" ]] ||
    die "pair summary for ${task_id} serialized to an empty file"
  sync -f -- "${target}/pair-summary.json.partial"
  mv -- "${target}/pair-summary.json.partial" "${target}/pair-summary.json"
  sync -f -- "${target}"
  printf 'Task %s pair complete.\n' "${task_id}" >&2
}

# ---------------------------------------------------------------------------
# The pass: plan order, resumable, one task pair at a time.
# ---------------------------------------------------------------------------
completed=0
plan_index=0
while read -r task_id; do
  if (( plan_index % SHARD_COUNT == SHARD_INDEX )); then
    run_task "${task_id}"
    completed=$((completed + 1))
    printf '=== Suite progress (shard %s/%s): %s of %s shard task pairs done ===\n' \
      "${SHARD_INDEX}" "${SHARD_COUNT}" "${completed}" "${SHARD_TASK_COUNT}" >&2
  fi
  plan_index=$((plan_index + 1))
done < <(jq -er '.runs[].task_id' "${PLAN_FILE}")

# A failure inside the process substitution above (a jq/IO error mid-stream)
# cannot be seen by set -e, so the loop could consume only a prefix and still
# reach here. Prove conservation both ways before declaring the pass complete:
# every plan row was streamed, and every task this shard owns was processed.
PLAN_ROW_COUNT="$(jq -er '.runs | length' "${PLAN_FILE}")"
readonly PLAN_ROW_COUNT
(( plan_index == PLAN_ROW_COUNT )) ||
  die "the plan stream yielded ${plan_index} of ${PLAN_ROW_COUNT} rows; refusing to report a truncated pass as complete"
(( completed == SHARD_TASK_COUNT )) ||
  die "shard ${SHARD_INDEX}/${SHARD_COUNT} processed ${completed} of ${SHARD_TASK_COUNT} owned task pairs; the plan stream ended early"

printf 'SUITE_PASS_COMPLETE shard=%s/%s tasks=%s runs_root=%s\n' "${SHARD_INDEX}" "${SHARD_COUNT}" "${completed}" "${RUNS_ROOT}"
