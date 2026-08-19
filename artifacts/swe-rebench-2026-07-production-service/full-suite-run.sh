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

(($# == 0)) || die 'no arguments are supported. Usage: ./full-suite-run.sh'
for command in awk cmp cp curl date docker find git grep jq mkdir mv rm sha256sum sort stat sync tar timeout tr unzip wc xargs zip; do
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
# but the pre-guards backend). v6 runs all 41 pairs fresh under the final
# v14 release whose backend carries the turboquant fail-closed guards.
readonly PASS_ROOT="${BENCH_ROOT}/full-suite-v6"
readonly RUNS_ROOT="${PASS_ROOT}/runs"
readonly PROVENANCE_ROOT="${PASS_ROOT}/release-provenance"
readonly PREAMBLE_FILE="${BENCH_ROOT}/prompt-preamble.md"
readonly TASK_ENV_ROOT="${BENCH_ROOT}/full-suite-v1/task-env"
readonly DATASET_ROOT="${BENCH_ROOT}/evaluator-dataset"
readonly PLAN_FILE="${SUITE_ROOT}/suite-plan.json"
readonly AGENT_TIMEOUT_SEC=3000
readonly TEARDOWN_TIMEOUT_SEC=900
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

[[ -f "${PLAN_FILE}" ]] || die "suite plan is missing: ${PLAN_FILE}"
jq -e '.eligible_count == 41 and (.runs | length) == 41' "${PLAN_FILE}" >/dev/null ||
  die 'suite plan does not carry the exact expected 41 eligible runs'

# ---------------------------------------------------------------------------
# Release provenance: recorded once when this pass starts; every later
# invocation must observe the identical release, or the pass refuses to mix
# two service versions into one suite.
# ---------------------------------------------------------------------------
mkdir -p -- "${PASS_ROOT}" "${RUNS_ROOT}"
chmod 0700 -- "${PASS_ROOT}" "${RUNS_ROOT}"

RELEASE_LOCK_SHA="$(sha256sum -- "${SERVICE_ROOT}/config/release.lock.json" | awk '{print $1}')"
STACK_LOCK_SHA="$(sha256sum -- "${SERVICE_ROOT}/config/stack.lock.json" | awk '{print $1}')"
IMPLEMENTATION_COMMIT="$(jq -er '.implementation_commit' "${SERVICE_ROOT}/config/release.lock.json")"
SERVICE_IMAGE_ID="$(jq -er '.images.service' "${SERVICE_ROOT}/config/release.lock.json")"
AGENT_SANDBOX="$(jq -er '.agent.agent_exec_sandbox' "${SERVICE_ROOT}/config/stack.lock.json")"
readonly RELEASE_LOCK_SHA STACK_LOCK_SHA IMPLEMENTATION_COMMIT SERVICE_IMAGE_ID AGENT_SANDBOX
[[ -z "$(git -C "${SERVICE_ROOT}" status --porcelain=v1 --untracked-files=all)" ]] ||
  die 'agent_service worktree must be clean before a suite pass'

if [[ -e "${PROVENANCE_ROOT}/release.json" ]]; then
  jq -e --arg release "${RELEASE_LOCK_SHA}" --arg stack "${STACK_LOCK_SHA}" \
    --arg commit "${IMPLEMENTATION_COMMIT}" \
    '.release_lock_sha256 == $release and .stack_lock_sha256 == $stack and .implementation_commit == $commit' \
    "${PROVENANCE_ROOT}/release.json" >/dev/null ||
    die "this pass was started on a different release; refusing to mix releases in ${PASS_ROOT}"
else
  mkdir -p -- "${PROVENANCE_ROOT}"
  chmod 0700 -- "${PROVENANCE_ROOT}"
  jq -n --arg release "${RELEASE_LOCK_SHA}" --arg stack "${STACK_LOCK_SHA}" \
    --arg commit "${IMPLEMENTATION_COMMIT}" --arg service_image "${SERVICE_IMAGE_ID}" \
    '{schema_version:1,release_lock_sha256:$release,stack_lock_sha256:$stack,
      implementation_commit:$commit,service_image:$service_image}' \
    >"${PROVENANCE_ROOT}/release.json"
  sync -f -- "${PROVENANCE_ROOT}/release.json"
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
curl --fail-with-body --silent --show-error "${API}/v1/agent/sessions" |
  jq -e '.sessions | map(select(.status == "running")) | length == 0' >/dev/null ||
  die 'production service already has a running session'

# ---------------------------------------------------------------------------
# Per-task machinery
# ---------------------------------------------------------------------------

verify_task_source() {
  local task_id="$1" task_dir="$2" marker="$3"
  [[ ! -f "${marker}" ]] || { printf 'source already verified for %s\n' "${task_id}" >&2; return 0; }
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
  printf 'verified\n' >"${marker}"
  sync -f -- "${marker}"
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

poll_until_terminal() {
  local session_id="$1" terminal_file="$2" timed_out_var="$3"
  local started deadline now body status
  started="$(date +%s)"
  deadline=$((started + AGENT_TIMEOUT_SEC))
  local cancelled=false
  while :; do
    body="$(mktemp /tmp/qwen38-suite-poll.XXXXXX)"
    local http
    http="$(curl --noproxy '*' --silent --show-error --connect-timeout 5 --max-time 30 \
      --output "${body}" --write-out '%{http_code}' \
      "${API}/v1/agent/sessions/${session_id}")" ||
      { rm -f -- "${body}"; die "session poll transport failed for ${session_id}"; }
    [[ "${http}" == 200 ]] || { rm -f -- "${body}"; die "session poll returned HTTP ${http} for ${session_id}"; }
    status="$(jq -er '.status' "${body}")"
    if [[ "${status}" != running ]]; then
      mv -- "${body}" "${terminal_file}"
      printf -v "${timed_out_var}" '%s' "${cancelled}"
      return 0
    fi
    rm -f -- "${body}"
    now="$(date +%s)"
    if [[ "${cancelled}" == false && "${now}" -ge "${deadline}" ]]; then
      printf 'Agent deadline reached for %s; requesting durable cancellation.\n' "${session_id}" >&2
      curl --noproxy '*' --silent --show-error --connect-timeout 5 --max-time 30 \
        --request POST --output /dev/null --write-out '' \
        "${API}/v1/agent/sessions/${session_id}/cancel" ||
        die "cancellation transport failed for ${session_id}"
      cancelled=true
      deadline=$((now + TEARDOWN_TIMEOUT_SEC))
    elif [[ "${cancelled}" == true && "${now}" -ge "${deadline}" ]]; then
      die "session ${session_id} did not reach a terminal state within ${TEARDOWN_TIMEOUT_SEC}s of durable cancellation"
    fi
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
  mkdir -p -- "${run_dir}"
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
    { rm -rf -- "${composed_ws}"; die "workspace hardlink copy failed for ${task_id}"; }
  cp -- "${env_dir}/task-env.tar.gz" "${composed_ws}/.task-env.tar.gz"
  request_file="$(submission_create_receipt "${session_id}" "${composed_ws}" "${composed_prompt}" "${policy}")" ||
    { rm -rf -- "${composed_ws}"; die "receipt construction failed for ${task_id} ${label}"; }
  rm -rf -- "${composed_ws}"
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
  local terminal="${run_dir}/terminal.json" agent_timed_out=false
  poll_until_terminal "${session_id}" "${terminal}" agent_timed_out
  jq -e --arg id "${session_id}" --argjson policy "${policy}" \
    '.session_id == $id and (.status == "completed" or .status == "cancelled") and
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
  if docker ps --all --format '{{.Names}}' | grep -qF -- "${session_id}"; then
    die "session containers survived teardown for ${session_id}"
  fi
  if [[ "${agent_timed_out}" == false ]]; then
    jq -e '.status == "completed"' "${terminal}" >/dev/null || die 'non-timeout session did not complete'
  fi

  # --- bundle over the connection -----------------------------------------
  "${SERVICE_ROOT}/bundle.sh" "${session_id}" "${run_dir}/production-bundle.tar.zst" >&2
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
  mkdir -- "${run_dir}/patch"
  docker run --rm --network none --security-opt no-new-privileges \
    --cpus "${TASK_CPUS}" --memory 2048m --memory-swap 2048m --pids-limit 512 \
    --env TASK_BASE_COMMIT="${task_base_commit}" \
    --env TASK_WORKING_DIR="${task_working_dir}" \
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
      tar -C /candidate --exclude=./.git --exclude=.git --exclude=./.task-env.tar.gz --exclude=.task-env.tar.gz -cf - . |
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

  if [[ "${agent_timed_out}" == true ]]; then
    classification=agent_timeout
  elif jq -e '.is_process_error == true or .agent_exit_code != 0 or .container_exit_code != 0' "${terminal}" >/dev/null; then
    classification=production_agent_process_failure
  elif [[ "${reward}" == 1 ]]; then
    classification=resolved
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
    --argjson agent_timed_out "${agent_timed_out}" \
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
      production:{created:$created[0],terminal:$terminal[0],session_id:$session_id,agent_timed_out:$agent_timed_out},
      evidence:{bundle_sha256:$bundle_sha256,candidate_patch_sha256:$patch_sha256,candidate_patch_bytes:$patch_bytes},
      verifier:{exit_code:$grader_exit_code,reward:$reward,report:$verifier[0]},
      outcome:{classification:$classification,resolved:($reward == 1)}
    }' >"${run_dir}/result.json.partial"
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
  verify_task_source "${task_id}" "${task_dir}" "${target}/source-verified.txt"
  verify_dataset_inputs "${task_id}" "${task_dir}"
  ensure_environment_image "${task_id}" "${task_dir}"

  local order ordinal label policy
  order="$(jq -cer '.policy_order' "${task_dir}/manifest.json")"
  local index=0
  for policy in $(jq -er '.[]' <<<"${order}"); do
    index=$((index + 1))
    ordinal="$(printf '%02d' "${index}")"
    if [[ "${policy}" == false ]]; then label=unpreserved; else label=preserved; fi
    run_variant "${task_id}" "${task_dir}" "${ordinal}" "${policy}" "${label}"
  done

  jq -n \
    --slurpfile first "${target}/01-"*/result.json \
    --slurpfile second "${target}/02-"*/result.json \
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
  sync -f -- "${target}/pair-summary.json.partial"
  mv -- "${target}/pair-summary.json.partial" "${target}/pair-summary.json"
  sync -f -- "${target}"
  printf 'Task %s pair complete.\n' "${task_id}" >&2
}

# ---------------------------------------------------------------------------
# The pass: plan order, resumable, one task pair at a time.
# ---------------------------------------------------------------------------
completed=0
while read -r task_id; do
  run_task "${task_id}"
  completed=$((completed + 1))
  printf '=== Suite progress: %s of 41 task pairs have accepted summaries ===\n' \
    "$(find "${RUNS_ROOT}" -mindepth 2 -maxdepth 2 -name pair-summary.json | wc -l)" >&2
done < <(jq -er '.runs[].task_id' "${PLAN_FILE}")

printf 'SUITE_PASS_COMPLETE tasks=%s runs_root=%s\n' "${completed}" "${RUNS_ROOT}"
