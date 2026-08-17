#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

if (( $# != 0 )); then
  printf 'ERROR: this pilot has one locked behavior and accepts no arguments.\n' >&2
  exit 2
fi

readonly SERVICE_ROOT=/home/user/Desktop/agent_service
readonly BENCH_ROOT="${SERVICE_ROOT}/artifacts/swe-rebench-2026-07-production-service"
readonly DATASET_ROOT="${BENCH_ROOT}/evaluator-dataset"
readonly TASK_ID=Gentleman-Programming__gentle-ai-595
readonly TASK_ROOT="${DATASET_ROOT}/${TASK_ID}"
readonly SOURCE_ROOT="${BENCH_ROOT}/agent-inputs/${TASK_ID}"
readonly RUNS_ROOT="${BENCH_ROOT}/runs/${TASK_ID}"
readonly RELEASE_PROVENANCE_ROOT="${BENCH_ROOT}/provenance/release-7a329f6"
readonly API=http://127.0.0.1:8090

readonly DATASET_NAME=ibragim-badertdinov/swe-rebench-07-2026@2026-07
readonly DATASET_OCI_DIGEST=sha256:e2e357045bf03e4900d2506c36562f6eaff7acd37f63780600967ea3aecdcd79
readonly HARBOR_VERSION=0.21.0
readonly HARBOR_COMMIT=64afbbcb62165950301e1a6407c729aa26d844ff
readonly SERVICE_RELEASE_COMMIT=7a329f61665a7126e3f8cd9a4e3b7a6b66a639bc
readonly SERVICE_IMPLEMENTATION_COMMIT=bc67dae720894cbbcd62122a2a9ff6b56b042168
readonly SERVICE_RELEASE_LOCK_SHA256=a43ffd0738749771fda13ce4d4b491e58356e2f0be430880334747ac5761f5d4
readonly STACK_LOCK_SHA256=de1307bd8598cd928191b1a0947c086fcb9af2cc91c17c4488f70d06ca528de3
readonly VLLM_IMAGE=sha256:587e8710c6630edd249f19b46837c12ebe5b5dcdc98486e215ac48a66644dc7f
readonly AGENT_IMAGE=sha256:1dc84a6f4e03b62a9540794a353c0b1e175a07e6afbcfed6441fe5f2d0f7d1ec
readonly BROKER_IMAGE=sha256:f9d3b77ed2e10d69648c2e443fa5e49ff06fca7eedf6fc580f9d8762d9bfb054
readonly SERVICE_IMAGE=sha256:8f8d4b2e68bf47c9d92c6c5c0f77fdbf60d0056ef32155a34ecc96357dfd41f4

readonly TASK_BASE_IMAGE=sha256:4714a9461b2e40cfb122afa32c2c7ad6b154f59e0f9239ae1610018e05fa2029
readonly TASK_ENV_IMAGE=sha256:8abec1b1eb1f496fe1762f74f7b0cc535ac287683b8b81316c1e35ef34c819c2
readonly TASK_BASE_COMMIT=36051a1b41d879b1bf76f4aa9aa984d74e54c26d
readonly TASK_WORKDIR=/gentle-ai
readonly TASK_ENV_ARCHIVE="${BENCH_ROOT}/environment-images/Gentleman-Programming__gentle-ai-595-sha256-8abec1b1.tar"
readonly TASK_ENV_ARCHIVE_SHA256=2dd8689b6568f5b8742a41dbf050f8dd2aeb48ff40b42c6624c98e985a85e069
readonly TASK_ENV_ARCHIVE_BYTES=445108736
readonly AGENT_TIMEOUT_SEC=3000
readonly VERIFIER_TIMEOUT_SEC=3000
readonly TASK_CPUS=1
readonly TASK_MEMORY_MB=4096

readonly INSTRUCTION_SHA256=ed30f7bcf909ca5ba322c67ebe1d3268eeff84d95aa398a1999729d51b0aa07e
readonly TASK_TOML_SHA256=f43c9704c1b8f61622482b208b1760b396904cf3a19805ac8310e751878c31fc
readonly ENV_DOCKERFILE_SHA256=4d043435cced3194f43eb194934e80cae11c159535e4b1eedfa363c6b2844334
readonly TEST_SH_SHA256=b476376ea874e772e03db5905681b23bd125242b5695395ba30e95494a92de65
readonly TEST_CONFIG_SHA256=b57b74ca95373f3c8b41e6e1df5b48ac632107923b30ea96600d5f6a19e2c732
readonly TEST_PARSER_SHA256=3bd13777a0e303178e76fc3a135e7ccd303b6496e59a973c4503fefd6a0dd114

ACTIVE_SESSION_ID=
ACTIVE_RUN_DIR=
ACTIVE_GRADER=

die() {
  printf 'ERROR: %s\n' "$*" >&2
  exit 1
}

require_equal() {
  local label="$1" expected="$2" actual="$3"
  [[ "${actual}" == "${expected}" ]] ||
    die "${label} mismatch: expected ${expected}, got ${actual}"
}

require_sha256() {
  local path="$1" expected="$2" actual
  [[ -f "${path}" && ! -L "${path}" ]] || die "required regular file is absent or a symlink: ${path}"
  actual="$(sha256sum -- "${path}" | awk '{print $1}')"
  require_equal "SHA-256 for ${path}" "${expected}" "${actual}"
}

cleanup() {
  local rc=$?
  set +e
  if [[ -n "${ACTIVE_GRADER}" ]] && docker container inspect "${ACTIVE_GRADER}" >/dev/null 2>&1; then
    if [[ -n "${ACTIVE_RUN_DIR}" ]]; then
      docker container inspect "${ACTIVE_GRADER}" >"${ACTIVE_RUN_DIR}/grader-interrupted-inspect.json" 2>/dev/null
    fi
    docker stop --time 30 "${ACTIVE_GRADER}" >/dev/null 2>&1
    docker rm "${ACTIVE_GRADER}" >/dev/null 2>&1
  fi
  if [[ -n "${ACTIVE_SESSION_ID}" ]]; then
    if [[ -n "${ACTIVE_RUN_DIR}" ]]; then
      curl --silent --show-error --request POST \
        "${API}/v1/agent/sessions/${ACTIVE_SESSION_ID}/cancel" \
        >"${ACTIVE_RUN_DIR}/interrupted-cancel.json" 2>"${ACTIVE_RUN_DIR}/interrupted-cancel.stderr"
    else
      curl --silent --show-error --request POST \
        "${API}/v1/agent/sessions/${ACTIVE_SESSION_ID}/cancel" >/dev/null 2>&1
    fi
  fi
  exit "${rc}"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

for command in awk chmod cmp cp curl date docker find git grep id jq mkdir mv readlink rm sha256sum stat sync tar timeout tr wc xargs; do
  command -v "${command}" >/dev/null 2>&1 || die "required host command is unavailable: ${command}"
done

[[ "$(id -u)" == 1000 && "$(id -g)" == 1000 ]] ||
  die 'the benchmark must run as the pinned host uid:gid 1000:1000'
[[ "$(readlink -f -- "${SERVICE_ROOT}")" == "${SERVICE_ROOT}" ]] || die 'service root canonical path drift'
[[ -d "${BENCH_ROOT}" && ! -L "${BENCH_ROOT}" ]] || die 'benchmark evidence root is absent or a symlink'
[[ "$(stat -c '%u:%g:%a' -- "${BENCH_ROOT}")" == 1000:1000:700 ]] ||
  die 'benchmark evidence root must be owned by 1000:1000 with mode 0700'

require_equal 'agent_service release commit' "${SERVICE_RELEASE_COMMIT}" "$(git -C "${SERVICE_ROOT}" rev-parse HEAD)"
[[ -z "$(git -C "${SERVICE_ROOT}" status --porcelain=v1 --untracked-files=all)" ]] ||
  die 'agent_service tracked worktree is not clean'
require_sha256 "${SERVICE_ROOT}/config/release.lock.json" "${SERVICE_RELEASE_LOCK_SHA256}"
require_sha256 "${SERVICE_ROOT}/config/stack.lock.json" "${STACK_LOCK_SHA256}"
jq -e \
  --arg implementation "${SERVICE_IMPLEMENTATION_COMMIT}" \
  --arg agent "${AGENT_IMAGE}" \
  --arg broker "${BROKER_IMAGE}" \
  --arg service "${SERVICE_IMAGE}" \
  '.implementation_commit == $implementation and .images.agent == $agent and
   .images.broker == $broker and .images.service == $service' \
  "${SERVICE_ROOT}/config/release.lock.json" >/dev/null || die 'release lock semantic identity mismatch'
jq -e --arg vllm "${VLLM_IMAGE}" --arg base "${TASK_BASE_IMAGE}" \
  '.backend.image_id == $vllm and $base != ""' \
  "${SERVICE_ROOT}/config/stack.lock.json" >/dev/null || die 'stack lock backend image mismatch'

require_sha256 "${TASK_ROOT}/instruction.md" "${INSTRUCTION_SHA256}"
require_sha256 "${TASK_ROOT}/task.toml" "${TASK_TOML_SHA256}"
require_sha256 "${TASK_ROOT}/environment/Dockerfile" "${ENV_DOCKERFILE_SHA256}"
require_sha256 "${TASK_ROOT}/tests/test.sh" "${TEST_SH_SHA256}"
require_sha256 "${TASK_ROOT}/tests/config.json" "${TEST_CONFIG_SHA256}"
require_sha256 "${TASK_ROOT}/tests/swan_log_parsers.py" "${TEST_PARSER_SHA256}"

[[ -d "${SOURCE_ROOT}/.git" && ! -L "${SOURCE_ROOT}/.git" ]] || die 'clean agent input has no real .git directory'
require_equal 'clean agent input HEAD' "${TASK_BASE_COMMIT}" "$(git -C "${SOURCE_ROOT}" rev-parse HEAD)"
[[ -z "$(git -C "${SOURCE_ROOT}" status --porcelain=v1 --untracked-files=all)" ]] || die 'clean agent input is dirty'
[[ -z "$(find "${SOURCE_ROOT}" -type l -print -quit)" ]] || die 'clean agent input contains a symlink and is inadmissible'
[[ -z "$(find "${SOURCE_ROOT}" \! -type d \! -type f -print -quit)" ]] || die 'clean agent input contains a special file'

require_equal 'realized task environment image' "${TASK_ENV_IMAGE}" \
  "$(docker image inspect --format '{{.Id}}' "${TASK_ENV_IMAGE}")"
require_equal 'task environment working directory' "${TASK_WORKDIR}" \
  "$(docker image inspect --format '{{.Config.WorkingDir}}' "${TASK_ENV_IMAGE}")"
require_sha256 "${TASK_ENV_ARCHIVE}" "${TASK_ENV_ARCHIVE_SHA256}"
require_equal 'task environment archive bytes' "${TASK_ENV_ARCHIVE_BYTES}" \
  "$(stat -c '%s' -- "${TASK_ENV_ARCHIVE}")"
require_equal 'task environment archive ownership/mode' 1000:1000:600 \
  "$(stat -c '%u:%g:%a' -- "${TASK_ENV_ARCHIVE}")"
docker run --rm --network none --cap-drop ALL --security-opt no-new-privileges \
  --env TASK_BASE_COMMIT="${TASK_BASE_COMMIT}" \
  --entrypoint bash "${TASK_ENV_IMAGE}" -Eeuo pipefail -c \
  'test "$(git -C /gentle-ai rev-parse HEAD)" = "$TASK_BASE_COMMIT";
   test -z "$(git -C /gentle-ai status --porcelain=v1 --untracked-files=all)";
   test "$(uv --version)" = "uv 0.7.13";
   test -d /logs'

readonly EXPECTED_FALLOW_SYMLINKS='crates/cli/schema.json -> ../../schema.json
crates/cli/templates/ci/gitlab-ci.yml -> ../../../../ci/gitlab-ci.yml
crates/cli/templates/ci/scripts/comment.sh -> ../../../../../ci/scripts/comment.sh
crates/cli/templates/ci/scripts/review.sh -> ../../../../../ci/scripts/review.sh'
readonly EXPECTED_ANY_LLM_SYMLINKS='.venv/bin/python -> /usr/local/bin/python3.13
.venv/bin/python3 -> python
.venv/bin/python3.13 -> python
.venv/lib64 -> lib
CLAUDE.md -> AGENTS.md'
readonly FALLOW_ROOT="${BENCH_ROOT}/agent-inputs/fallow-rs__fallow-824"
readonly ANY_LLM_ROOT="${BENCH_ROOT}/agent-inputs/mozilla-ai__any-llm-1121"
require_equal 'fallow-rs__fallow-824 exclusion symlinks' "${EXPECTED_FALLOW_SYMLINKS}" \
  "$(find "${FALLOW_ROOT}" -type l -printf '%P -> %l\n' | LC_ALL=C sort)"
require_equal 'mozilla-ai__any-llm-1121 exclusion symlinks' "${EXPECTED_ANY_LLM_SYMLINKS}" \
  "$(find "${ANY_LLM_ROOT}" -type l -printf '%P -> %l\n' | LC_ALL=C sort)"

[[ ! -e "${RELEASE_PROVENANCE_ROOT}" ]] ||
  die "corrected-release provenance already exists: ${RELEASE_PROVENANCE_ROOT}"
mkdir -p -- "${RELEASE_PROVENANCE_ROOT}" "${RUNS_ROOT}"
chmod 0700 -- "${BENCH_ROOT}/provenance" "${RELEASE_PROVENANCE_ROOT}" \
  "${BENCH_ROOT}/runs" "${RUNS_ROOT}"
[[ "$(stat -c '%u:%g:%a' -- "${RUNS_ROOT}")" == 1000:1000:700 ]] || die 'run root ownership/mode mismatch'

readonly LOCK_PATH="${RELEASE_PROVENANCE_ROOT}/benchmark-lock.json"
readonly LOCK_PARTIAL="${LOCK_PATH}.partial"
[[ ! -e "${LOCK_PATH}" && ! -e "${LOCK_PARTIAL}" ]] || die "benchmark lock already exists: ${LOCK_PATH}"
jq -n \
  --arg dataset_name "${DATASET_NAME}" \
  --arg dataset_digest "${DATASET_OCI_DIGEST}" \
  --arg harbor_version "${HARBOR_VERSION}" \
  --arg harbor_commit "${HARBOR_COMMIT}" \
  --arg task_id "${TASK_ID}" \
  --arg task_base_image "${TASK_BASE_IMAGE}" \
  --arg task_environment_image "${TASK_ENV_IMAGE}" \
  --arg task_environment_archive_sha256 "${TASK_ENV_ARCHIVE_SHA256}" \
  --argjson task_environment_archive_bytes "${TASK_ENV_ARCHIVE_BYTES}" \
  --arg task_base_commit "${TASK_BASE_COMMIT}" \
  --arg service_release_commit "${SERVICE_RELEASE_COMMIT}" \
  --arg service_implementation_commit "${SERVICE_IMPLEMENTATION_COMMIT}" \
  --arg service_release_lock_sha256 "${SERVICE_RELEASE_LOCK_SHA256}" \
  --arg stack_lock_sha256 "${STACK_LOCK_SHA256}" \
  --arg vllm_image "${VLLM_IMAGE}" \
  --arg agent_image "${AGENT_IMAGE}" \
  --arg broker_image "${BROKER_IMAGE}" \
  --arg service_image "${SERVICE_IMAGE}" \
  --arg instruction_sha256 "${INSTRUCTION_SHA256}" \
  --arg task_toml_sha256 "${TASK_TOML_SHA256}" \
  --arg environment_dockerfile_sha256 "${ENV_DOCKERFILE_SHA256}" \
  --arg test_sh_sha256 "${TEST_SH_SHA256}" \
  --arg test_config_sha256 "${TEST_CONFIG_SHA256}" \
  --arg test_parser_sha256 "${TEST_PARSER_SHA256}" \
  --argjson agent_timeout_sec "${AGENT_TIMEOUT_SEC}" \
  --argjson verifier_timeout_sec "${VERIFIER_TIMEOUT_SEC}" \
  --argjson cpus "${TASK_CPUS}" \
  --argjson memory_mb "${TASK_MEMORY_MB}" \
  '{
    schema_version: 1,
    methodology: "production-agent-service-post-session-swe-rebench-evaluator-v1",
    production_boundary: "All model and agent execution uses POST /v1/agent/sessions on the accepted production agent_service. Harbor supplies only pinned task/evaluator inputs; it is not a model adapter.",
    dataset: {name:$dataset_name, oci_digest:$dataset_digest},
    harbor: {version:$harbor_version, commit:$harbor_commit},
    task: {
      id:$task_id, base_image:$task_base_image,
      realized_environment_image:$task_environment_image,
      environment_materialization:{
        source:"exact dataset environment/Dockerfile",
        network_fetch:"https://astral.sh/uv/0.7.13/install.sh",
        installer_observation:"uv installer reported: no checksums to verify",
        rerun_authority:"the preserved realized Docker image archive, not a repeat network fetch",
        archive_sha256:$task_environment_archive_sha256,
        archive_bytes:$task_environment_archive_bytes
      },
      base_commit:$task_base_commit,
      instruction_sha256:$instruction_sha256,
      task_toml_sha256:$task_toml_sha256,
      environment_dockerfile_sha256:$environment_dockerfile_sha256,
      test_sh_sha256:$test_sh_sha256,
      test_config_sha256:$test_config_sha256,
      test_parser_sha256:$test_parser_sha256,
      agent_timeout_sec:$agent_timeout_sec,
      verifier_timeout_sec:$verifier_timeout_sec,
      cpus:$cpus,
      memory_mb:$memory_mb,
      network_mode:"public"
    },
    production_release: {
      release_commit:$service_release_commit,
      implementation_commit:$service_implementation_commit,
      release_lock_sha256:$service_release_lock_sha256,
      stack_lock_sha256:$stack_lock_sha256,
      images:{vllm:$vllm_image,agent:$agent_image,broker:$broker_image,service:$service_image}
    },
    comparison: {
      order:[false,true],
      sole_changed_request_field:"preserve_thinking",
      invariant_settings:"same production service, model, Qwen Code, xhigh reasoning, sampling, tools, source, instruction, evaluator, and time limits"
    }
  }' >"${LOCK_PARTIAL}"
jq -e '.schema_version == 1 and .comparison.order == [false,true]' "${LOCK_PARTIAL}" >/dev/null ||
  die 'generated benchmark lock failed its schema assertion'
sync -f "${LOCK_PARTIAL}"
mv -- "${LOCK_PARTIAL}" "${LOCK_PATH}"
sync -f "${RELEASE_PROVENANCE_ROOT}"

readonly EXCLUSIONS_PATH="${RELEASE_PROVENANCE_ROOT}/preflight-exclusions.json"
readonly EXCLUSIONS_PARTIAL="${EXCLUSIONS_PATH}.partial"
[[ ! -e "${EXCLUSIONS_PATH}" && ! -e "${EXCLUSIONS_PARTIAL}" ]] || die 'preflight exclusion record already exists'
jq -n \
  --arg fallow_symlinks "${EXPECTED_FALLOW_SYMLINKS}" \
  --arg any_llm_symlinks "${EXPECTED_ANY_LLM_SYMLINKS}" \
  '{
    schema_version:1,
    classification:"production-input-contract-exclusion",
    explanation:"The production service rejects every source symlink. These tasks were not flattened, rewritten, or submitted; they are infrastructure/input-contract exclusions, not model failures.",
    exclusions:[
      {task_id:"fallow-rs__fallow-824", image:"sha256:ee909afe9e08877cfd1cd09335a6b298ac6a7ce8f635e54dd2ee7ef3dc11c94a", symlinks:($fallow_symlinks|split("\n"))},
      {task_id:"mozilla-ai__any-llm-1121", image:"sha256:1e1d1966686c60085052be84f27b6b2b9414b0f076eb172b1c07a6a5e13b0ee4", symlinks:($any_llm_symlinks|split("\n"))}
    ]
  }' >"${EXCLUSIONS_PARTIAL}"
sync -f "${EXCLUSIONS_PARTIAL}"
mv -- "${EXCLUSIONS_PARTIAL}" "${EXCLUSIONS_PATH}"
sync -f "${RELEASE_PROVENANCE_ROOT}"

readonly PREFLIGHT_STATUS="${RELEASE_PROVENANCE_ROOT}/production-status.txt"
[[ ! -e "${PREFLIGHT_STATUS}" ]] || die 'production preflight status record already exists'
(cd "${SERVICE_ROOT}" && ./status.sh) >"${PREFLIGHT_STATUS}" 2>&1
grep -Fqx 'READY — one defensively validated mode only' "${PREFLIGHT_STATUS}" ||
  die 'production status did not reach its exact ready terminal'
sync -f "${PREFLIGHT_STATUS}"

readonly SESSION_LIST="${RELEASE_PROVENANCE_ROOT}/sessions-before.json"
[[ ! -e "${SESSION_LIST}" ]] || die 'preflight session list already exists'
curl --fail-with-body --silent --show-error "${API}/v1/agent/sessions" >"${SESSION_LIST}"
jq -e '.sessions | map(select(.status == "running")) | length == 0' "${SESSION_LIST}" >/dev/null ||
  die 'production service already has a running session'

readonly INPUT_MANIFEST="${RELEASE_PROVENANCE_ROOT}/${TASK_ID}-tracked-files.sha256"
[[ ! -e "${INPUT_MANIFEST}" ]] || die 'input manifest already exists'
(
  cd "${SOURCE_ROOT}"
  git ls-files -z | LC_ALL=C sort -z | xargs -0 sha256sum --
) >"${INPUT_MANIFEST}"
[[ "$(wc -l <"${INPUT_MANIFEST}")" == "$(git -C "${SOURCE_ROOT}" ls-files | wc -l)" ]] ||
  die 'tracked input manifest omitted a file'
sync -f "${INPUT_MANIFEST}"

verify_original_input() {
  require_equal 'agent input HEAD after session' "${TASK_BASE_COMMIT}" "$(git -C "${SOURCE_ROOT}" rev-parse HEAD)"
  [[ -z "$(git -C "${SOURCE_ROOT}" status --porcelain=v1 --untracked-files=all)" ]] ||
    die 'production service mutated the original agent input'
  local current_manifest
  current_manifest="$(mktemp)"
  (
    cd "${SOURCE_ROOT}"
    git ls-files -z | LC_ALL=C sort -z | xargs -0 sha256sum --
  ) >"${current_manifest}"
  cmp -- "${INPUT_MANIFEST}" "${current_manifest}" || die 'original agent input content changed'
  rm -f -- "${current_manifest}"
}

run_policy() {
  local ordinal="$1" policy="$2" label="$3"
  local run_dir="${RUNS_ROOT}/${ordinal}-${label}"
  local request created http_code session_id wait_rc terminal bundle_path expected_bundle
  local bundle_sha patch_sha patch_bytes grader_name grader_id grader_rc reward classification
  local agent_timed_out=false

  [[ "${policy}" == false || "${policy}" == true ]] || die "invalid internal policy: ${policy}"
  [[ ! -e "${run_dir}" ]] || die "run directory already exists: ${run_dir}"
  mkdir -- "${run_dir}"
  chmod 0700 -- "${run_dir}"
  ACTIVE_RUN_DIR="${run_dir}"

  request="${run_dir}/request.json"
  jq -n --arg folder "${SOURCE_ROOT}" --rawfile prompt "${TASK_ROOT}/instruction.md" \
    --argjson preserve_thinking "${policy}" \
    '{folder:$folder,prompt:$prompt,preserve_thinking:$preserve_thinking}' >"${request}"
  jq -e --arg folder "${SOURCE_ROOT}" --argjson policy "${policy}" \
    '.folder == $folder and .preserve_thinking == $policy and (.prompt|length) > 0' \
    "${request}" >/dev/null || die 'request construction failed'

  created="${run_dir}/created.json"
  set +e
  http_code="$(curl --silent --show-error --output "${created}" --write-out '%{http_code}' \
    --header 'content-type: application/json' --data-binary "@${request}" \
    "${API}/v1/agent/sessions")"
  local create_rc=$?
  set -e
  [[ "${create_rc}" == 0 ]] || die "production session creation transport failed with rc=${create_rc}"
  require_equal 'production session creation HTTP status' 201 "${http_code}"
  jq -e --argjson policy "${policy}" \
    '.status == "running" and .model == "qwen3.8-27b-nvfp4-k8v4" and
     .context_window == 262144 and .preserve_thinking == $policy' \
    "${created}" >/dev/null || die 'production session readiness body violated the locked contract'
  session_id="$(jq -er '.session_id | select(test("^s-[0-9a-f]{32}$"))' "${created}")"
  ACTIVE_SESSION_ID="${session_id}"
  printf '%s\n' "${session_id}" >"${run_dir}/session-id.txt"
  printf 'Production session %s is running with preserve_thinking=%s; waiting by the production /wait notification.\n' \
    "${session_id}" "${policy}" >&2

  terminal="${run_dir}/terminal.json"
  set +e
  timeout --signal=TERM --kill-after=10s "${AGENT_TIMEOUT_SEC}s" \
    curl --fail-with-body --silent --show-error \
      "${API}/v1/agent/sessions/${session_id}/wait" >"${terminal}.wait" 2>"${run_dir}/wait.stderr"
  wait_rc=$?
  set -e
  if [[ "${wait_rc}" == 124 || "${wait_rc}" == 137 ]]; then
    agent_timed_out=true
    curl --fail-with-body --silent --show-error --request POST \
      "${API}/v1/agent/sessions/${session_id}/cancel" >"${terminal}.cancel"
    cp -- "${terminal}.cancel" "${terminal}"
  elif [[ "${wait_rc}" == 0 ]]; then
    cp -- "${terminal}.wait" "${terminal}"
  else
    die "production /wait transport failed with rc=${wait_rc}"
  fi
  ACTIVE_SESSION_ID=

  jq -e --arg id "${session_id}" --argjson policy "${policy}" \
    '.session_id == $id and (.status == "completed" or .status == "cancelled") and
     .model == "qwen3.8-27b-nvfp4-k8v4" and .context_window == 262144 and
     .preserve_thinking == $policy and .finished_at_unix > 0 and
     .bundle_archive_path != "" and .bundle_compressed_bytes > 0 and
     .bundle_file_count > 0 and .raw_session_tree_retained == false and
     .teardown_diagnostics == []' \
    "${terminal}" >/dev/null || die 'production terminal body or required bundle contract failed'
  if [[ "${agent_timed_out}" == false ]]; then
    jq -e '.status == "completed"' "${terminal}" >/dev/null || die 'non-timeout session did not complete'
  fi

  expected_bundle="${SERVICE_ROOT}/.runtime/results/${session_id}/bundle.tar.zst"
  bundle_path="$(jq -er '.bundle_archive_path' "${terminal}")"
  require_equal 'production bundle path' "${expected_bundle}" "${bundle_path}"
  [[ -f "${bundle_path}" && ! -L "${bundle_path}" ]] || die 'production bundle is absent or a symlink'
  require_equal 'production bundle byte count' "$(jq -er '.bundle_compressed_bytes' "${terminal}")" \
    "$(stat -c '%s' -- "${bundle_path}")"
  cp --reflink=auto --preserve=mode,timestamps -- "${bundle_path}" "${run_dir}/production-bundle.tar.zst"
  bundle_sha="$(sha256sum -- "${run_dir}/production-bundle.tar.zst" | awk '{print $1}')"
  printf '%s  %s\n' "${bundle_sha}" production-bundle.tar.zst >"${run_dir}/production-bundle.sha256"

  mkdir -- "${run_dir}/bundle"
  tar --zstd --extract --no-same-owner --file "${run_dir}/production-bundle.tar.zst" \
    --directory "${run_dir}/bundle"
  [[ -d "${run_dir}/bundle/staged" && ! -L "${run_dir}/bundle/staged" ]] || die 'bundle lacks a real staged workspace'
  [[ -z "$(find "${run_dir}/bundle" -type l -print -quit)" ]] || die 'accepted production bundle contains a symlink'
  cmp -- "${TASK_ROOT}/instruction.md" "${run_dir}/bundle/control/prompt.txt" || die 'bundle prompt differs from task instruction'
  jq -e --argjson policy "${policy}" '.preserve_thinking == $policy and (keys == ["preserve_thinking"])' \
    "${run_dir}/bundle/control/history-policy.json" >/dev/null || die 'bundle history policy mismatch'
  jq -e --argjson policy "${policy}" \
    '.model == "qwen3.8-27b-nvfp4-k8v4" and .context_window == 262144 and
     .preserve_thinking == $policy and .sandbox == "landlock-fs-v4-write-roots-v1+private-devpts-rw-v1+output-unmounted-v1"' \
    "${run_dir}/bundle/output/ready.json" >/dev/null || die 'bundle readiness record mismatch'
  verify_original_input

  mkdir -- "${run_dir}/patch"
  docker run --rm --network none --security-opt no-new-privileges \
    --cpus "${TASK_CPUS}" --memory 2048m --memory-swap 2048m --pids-limit 512 \
    --env TASK_BASE_COMMIT="${TASK_BASE_COMMIT}" \
    --mount "type=bind,src=${run_dir}/bundle/staged,dst=/candidate,readonly" \
    --mount "type=bind,src=${run_dir}/patch,dst=/out" \
    --entrypoint bash "${TASK_ENV_IMAGE}" -Eeuo pipefail -c '
      test -z "$(find /candidate -type l -print -quit)"
      cd /gentle-ai
      test "$(git rev-parse HEAD)" = "$TASK_BASE_COMMIT"
      test -z "$(git status --porcelain=v1 --untracked-files=all)"
      git clean -ffdqx
      find . -mindepth 1 -maxdepth 1 ! -name .git -exec rm -rf -- {} +
      tar -C /candidate --exclude=./.git --exclude=.git -cf - . |
        tar -C /gentle-ai --no-same-owner --no-overwrite-dir -xf -
      git add --all -- .
      git diff --cached --check
      git diff --cached --binary --full-index "$TASK_BASE_COMMIT" -- > /out/candidate.patch
      git diff --cached --stat -- > /out/candidate.stat
      git diff --cached --name-status -- > /out/candidate.name-status
      chmod 0644 /out/candidate.patch /out/candidate.stat /out/candidate.name-status
    '
  [[ -f "${run_dir}/patch/candidate.patch" && ! -L "${run_dir}/patch/candidate.patch" ]] || die 'trusted patch construction failed'
  patch_sha="$(sha256sum -- "${run_dir}/patch/candidate.patch" | awk '{print $1}')"
  patch_bytes="$(stat -c '%s' -- "${run_dir}/patch/candidate.patch")"

  mkdir -- "${run_dir}/grader-logs" "${run_dir}/grader-logs/verifier"
  grader_name="qwen38-swerebench-${ordinal}-${session_id}"
  [[ -z "$(docker ps -a --filter "name=^/${grader_name}$" --format '{{.ID}}')" ]] || die 'exact grader container name already exists'
  grader_id="$(docker create \
    --name "${grader_name}" \
    --label qwen38.benchmark.role=post-session-verifier \
    --label "qwen38.benchmark.session=${session_id}" \
    --network bridge \
    --cpus "${TASK_CPUS}" --memory "${TASK_MEMORY_MB}m" --memory-swap "${TASK_MEMORY_MB}m" --pids-limit 4096 \
    --security-opt no-new-privileges \
    --env TASK_BASE_COMMIT="${TASK_BASE_COMMIT}" \
    --mount "type=bind,src=${run_dir}/patch/candidate.patch,dst=/candidate.patch,readonly" \
    --mount "type=bind,src=${TASK_ROOT}/tests,dst=/benchmark-tests,readonly" \
    --mount "type=bind,src=${run_dir}/grader-logs,dst=/logs" \
    --entrypoint bash "${TASK_ENV_IMAGE}" -Eeuo pipefail -c '
      cd /gentle-ai
      test "$(git rev-parse HEAD)" = "$TASK_BASE_COMMIT"
      test -z "$(git status --porcelain=v1 --untracked-files=all)"
      git reset --hard "$TASK_BASE_COMMIT"
      git clean -ffdqx
      if test -s /candidate.patch; then
        git apply --binary --index --check /candidate.patch
        git apply --binary --index /candidate.patch
      fi
      git diff --cached --check
      rm -rf /tests
      cp -a /benchmark-tests /tests
      exec bash /tests/test.sh
    ')"
  require_equal 'grader create ID' "${grader_id}" "$(docker inspect --format '{{.Id}}' "${grader_name}")"
  ACTIVE_GRADER="${grader_name}"
  docker inspect "${grader_name}" >"${run_dir}/grader-created-inspect.json"
  jq -e --arg image "${TASK_ENV_IMAGE}" --arg session "${session_id}" \
    'length == 1 and .[0].Image == $image and .[0].HostConfig.NetworkMode == "bridge" and
     .[0].HostConfig.NanoCpus == 1000000000 and .[0].HostConfig.Memory == 4294967296 and
     .[0].HostConfig.MemorySwap == 4294967296 and .[0].HostConfig.PidsLimit == 4096 and
     .[0].HostConfig.PortBindings == {} and
     .[0].Config.Labels["qwen38.benchmark.session"] == $session and
     ([.[0].Mounts[].Destination] | sort) == ["/benchmark-tests","/candidate.patch","/logs"] and
     ([.[0].Mounts[] | select(.Destination == "/benchmark-tests" or .Destination == "/candidate.patch") | .RW] | all(. == false)) and
     ([.[0].Mounts[] | select(.Destination == "/logs") | .RW] == [true])' \
    "${run_dir}/grader-created-inspect.json" >/dev/null || die 'grader container contract mismatch before start'

  printf 'Grading production session %s in the immutable SWE-rebench environment.\n' "${session_id}" >&2
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
    "${run_dir}/grader-finished-inspect.json" >/dev/null || die 'grader did not terminate cleanly or was OOM-killed'
  require_equal 'grader attach exit and container exit' "${grader_rc}" \
    "$(jq -er '.[0].State.ExitCode' "${run_dir}/grader-finished-inspect.json")"
  docker rm "${grader_name}" >"${run_dir}/grader-removed-id.txt"
  ACTIVE_GRADER=

  if [[ "${grader_rc}" == 124 || "${grader_rc}" == 137 ]]; then
    die 'official verifier timed out; this is not a model score'
  fi
  [[ -f "${run_dir}/grader-logs/verifier/reward.txt" && ! -L "${run_dir}/grader-logs/verifier/reward.txt" ]] ||
    die 'official verifier did not write reward.txt'
  [[ -f "${run_dir}/grader-logs/verifier/report.json" && ! -L "${run_dir}/grader-logs/verifier/report.json" ]] ||
    die 'official verifier did not write report.json'
  reward="$(tr -d '[:space:]' <"${run_dir}/grader-logs/verifier/reward.txt")"
  [[ "${reward}" == 0 || "${reward}" == 1 ]] || die "invalid verifier reward: ${reward}"
  jq -e --argjson resolved "${reward}" '.resolved == ($resolved == 1)' \
    "${run_dir}/grader-logs/verifier/report.json" >/dev/null || die 'verifier reward/report mismatch'
  if [[ "${reward}" == 1 ]]; then
    require_equal 'resolved verifier exit' 0 "${grader_rc}"
  else
    require_equal 'unresolved verifier exit' 1 "${grader_rc}"
  fi

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
    --slurpfile lock "${LOCK_PATH}" \
    --slurpfile created "${created}" \
    --slurpfile terminal "${terminal}" \
    --slurpfile verifier "${run_dir}/grader-logs/verifier/report.json" \
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
    '{
      schema_version:1,
      benchmark_lock:$lock[0],
      run_order:($ordinal|tonumber),
      sole_variant:{preserve_thinking:$preserve_thinking},
      production:{created:$created[0],terminal:$terminal[0],session_id:$session_id,agent_timed_out:$agent_timed_out},
      evidence:{bundle_sha256:$bundle_sha256,candidate_patch_sha256:$patch_sha256,candidate_patch_bytes:$patch_bytes},
      verifier:{exit_code:$grader_exit_code,reward:$reward,report:$verifier[0]},
      outcome:{classification:$classification,resolved:($reward == 1)}
    }' >"${run_dir}/result.json.partial"
  jq -e --argjson policy "${policy}" --arg classification "${classification}" \
    '.sole_variant.preserve_thinking == $policy and .outcome.classification == $classification' \
    "${run_dir}/result.json.partial" >/dev/null || die 'generated result failed its schema assertion'
  sync -f "${run_dir}/result.json.partial"
  mv -- "${run_dir}/result.json.partial" "${run_dir}/result.json"
  sync -f "${run_dir}"
  verify_original_input
  ACTIVE_RUN_DIR=

  printf 'Run %s complete: preserve_thinking=%s reward=%s classification=%s turns=%s duration_ms=%s\n' \
    "${ordinal}" "${policy}" "${reward}" "${classification}" \
    "$(jq -er '.num_turns' "${terminal}")" "$(jq -er '.duration_wall_ms' "${terminal}")" >&2
}

run_policy 03 false unpreserved
run_policy 04 true preserved

readonly SUMMARY_PATH="${RUNS_ROOT}/pair-summary-release-7a329f6.json"
[[ ! -e "${SUMMARY_PATH}" ]] || die 'pair summary already exists'
jq -n \
  --slurpfile unpreserved "${RUNS_ROOT}/03-unpreserved/result.json" \
  --slurpfile preserved "${RUNS_ROOT}/04-preserved/result.json" \
  '{
    schema_version:1,
    task_id:"Gentleman-Programming__gentle-ai-595",
    methodology:"both variants ran through the accepted production agent_service; SWE-rebench evaluated only captured post-session workspaces",
    runs:[$unpreserved[0],$preserved[0]],
    comparison:{
      resolved:{unpreserved:$unpreserved[0].outcome.resolved,preserved:$preserved[0].outcome.resolved},
      turns:{unpreserved:$unpreserved[0].production.terminal.num_turns,preserved:$preserved[0].production.terminal.num_turns},
      duration_wall_ms:{unpreserved:$unpreserved[0].production.terminal.duration_wall_ms,preserved:$preserved[0].production.terminal.duration_wall_ms}
    }
  }' >"${SUMMARY_PATH}.partial"
sync -f "${SUMMARY_PATH}.partial"
mv -- "${SUMMARY_PATH}.partial" "${SUMMARY_PATH}"
sync -f "${RUNS_ROOT}"

printf 'PRODUCTION_SERVICE_SWE_REBENCH_PAIR_COMPLETE %s\n' "${SUMMARY_PATH}"
