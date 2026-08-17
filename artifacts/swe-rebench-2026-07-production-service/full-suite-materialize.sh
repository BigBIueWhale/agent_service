#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

if (( $# != 0 )); then
  printf 'ERROR: full-suite materialization accepts no arguments.\n' >&2
  exit 2
fi

readonly SERVICE_ROOT=/home/user/Desktop/agent_service
readonly BENCH_ROOT="${SERVICE_ROOT}/artifacts/swe-rebench-2026-07-production-service"
readonly MATERIALIZER_RELATIVE=artifacts/swe-rebench-2026-07-production-service/full-suite-materialize.sh
readonly DATASET_LOCK_RELATIVE=artifacts/swe-rebench-2026-07-production-service/full-suite-dataset.lock.json
readonly MATERIALIZER_PATH="${SERVICE_ROOT}/${MATERIALIZER_RELATIVE}"
readonly DATASET_ROOT="${BENCH_ROOT}/evaluator-dataset"
readonly DATASET_LOCK="${SERVICE_ROOT}/${DATASET_LOCK_RELATIVE}"
readonly SUITE_ROOT="${BENCH_ROOT}/full-suite-v1"
readonly MATERIALIZATION_ROOT="${SUITE_ROOT}/materialization"
readonly DATASET_NAME=ibragim-badertdinov/swe-rebench-07-2026@2026-07
readonly DATASET_CONTENT_DIGEST=sha256:e2e357045bf03e4900d2506c36562f6eaff7acd37f63780600967ea3aecdcd79
readonly HARBOR_VERSION=0.21.0
readonly HARBOR_COMMIT=64afbbcb62165950301e1a6407c729aa26d844ff
readonly DATASET_LOCK_SHA256=f994d9f7638f5b9f9ef29ca7a1385b25e61824641c05ef5b87a09231e52be1b2
readonly SERVICE_RELEASE_COMMIT=7a329f61665a7126e3f8cd9a4e3b7a6b66a639bc
readonly SERVICE_IMPLEMENTATION_COMMIT=bc67dae720894cbbcd62122a2a9ff6b56b042168
readonly SERVICE_RELEASE_LOCK_SHA256=a43ffd0738749771fda13ce4d4b491e58356e2f0be430880334747ac5761f5d4
readonly STACK_LOCK_SHA256=de1307bd8598cd928191b1a0947c086fcb9af2cc91c17c4488f70d06ca528de3
readonly SOURCE_EXTRACTOR_IMAGE_ID=sha256:1dc84a6f4e03b62a9540794a353c0b1e175a07e6afbcfed6441fe5f2d0f7d1ec
readonly SOURCE_EXTRACTOR_TAR_VERSION='tar (GNU tar) 1.35'
readonly EXPECTED_TASKS=111
readonly MAX_STAGED_FILES=200000
readonly MAX_STAGED_BYTES=4294967296
readonly MAX_PROMPT_BYTES=1048576
readonly ENV_REPOSITORY=qwen38-swerebench-full-v1
readonly UV_INSTALL='RUN curl -LsSf https://astral.sh/uv/0.7.13/install.sh | env UV_INSTALL_DIR=/usr/local/bin sh'
readonly LOGS_INSTALL='RUN mkdir -p /logs'

ACTIVE_CONTAINER=

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
  [[ -f "${path}" && ! -L "${path}" ]] ||
    die "required regular file is absent or a symlink: ${path}"
  actual="$(sha256sum -- "${path}" | awk '{print $1}')"
  require_equal "SHA-256 for ${path}" "${expected}" "${actual}"
}

cleanup() {
  local rc=$?
  set +e
  if [[ -n "${ACTIVE_CONTAINER}" ]] &&
    docker container inspect "${ACTIVE_CONTAINER}" >/dev/null 2>&1; then
    docker rm -f "${ACTIVE_CONTAINER}" >/dev/null 2>&1
  fi
  exit "${rc}"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

for command in awk bash chmod cmp cp curl cut date docker find flock git grep id jq \
  mkdir mktemp mv readlink rm sed sha256sum sort stat sync tr wc xargs; do
  command -v "${command}" >/dev/null 2>&1 ||
    die "required host command is unavailable: ${command}"
done

[[ "$(id -u)" == 1000 && "$(id -g)" == 1000 ]] ||
  die 'materialization must run as pinned host uid:gid 1000:1000'
[[ "$(readlink -f -- "${SERVICE_ROOT}")" == "${SERVICE_ROOT}" ]] ||
  die 'service root canonical path drift'
[[ ! -L "${MATERIALIZER_PATH}" && "$(readlink -f -- "${BASH_SOURCE[0]}")" == "${MATERIALIZER_PATH}" ]] ||
  die 'materializer must be invoked from its exact non-symlink project path'
[[ -d "${BENCH_ROOT}" && ! -L "${BENCH_ROOT}" ]] ||
  die 'benchmark root is absent or a symlink'
[[ "$(stat -c '%u:%g:%a' -- "${BENCH_ROOT}")" == 1000:1000:700 ]] ||
  die 'benchmark root must be owned by 1000:1000 with mode 0700'
[[ -d "${DATASET_ROOT}" && ! -L "${DATASET_ROOT}" ]] ||
  die 'dataset root is absent or a symlink'
[[ -z "$(git -C "${SERVICE_ROOT}" status --porcelain=v1 --untracked-files=all)" ]] ||
  die 'agent_service tracked worktree must be clean before materialization'
require_equal 'agent_service branch' master "$(git -C "${SERVICE_ROOT}" branch --show-current)"
for committed_input in "${MATERIALIZER_RELATIVE}" "${DATASET_LOCK_RELATIVE}"; do
  git -C "${SERVICE_ROOT}" ls-files --error-unmatch -- "${committed_input}" >/dev/null 2>&1 ||
    die "benchmark input is not tracked in Git: ${committed_input}"
  cmp -- "${SERVICE_ROOT}/${committed_input}" \
    <(git -C "${SERVICE_ROOT}" show "HEAD:${committed_input}") ||
    die "benchmark input differs from committed HEAD: ${committed_input}"
done
git -C "${SERVICE_ROOT}" merge-base --is-ancestor "${SERVICE_RELEASE_COMMIT}" HEAD ||
  die 'benchmark tooling commit does not descend from the accepted production release'
require_sha256 "${SERVICE_ROOT}/config/release.lock.json" "${SERVICE_RELEASE_LOCK_SHA256}"
require_sha256 "${SERVICE_ROOT}/config/stack.lock.json" "${STACK_LOCK_SHA256}"
require_equal 'source-extractor image ID in stack lock' "${SOURCE_EXTRACTOR_IMAGE_ID}" \
  "$(jq -er '.agent.image_id' "${SERVICE_ROOT}/config/stack.lock.json")"
require_equal 'source-extractor image architecture' amd64 \
  "$(docker image inspect --format '{{.Architecture}}' "${SOURCE_EXTRACTOR_IMAGE_ID}")"
require_equal 'source-extractor image OS' linux \
  "$(docker image inspect --format '{{.Os}}' "${SOURCE_EXTRACTOR_IMAGE_ID}")"
require_equal 'source-extractor GNU tar version' "${SOURCE_EXTRACTOR_TAR_VERSION}" \
  "$(docker run --rm --network none --cap-drop ALL --security-opt no-new-privileges \
    --read-only --user 1000:1000 --entrypoint tar "${SOURCE_EXTRACTOR_IMAGE_ID}" \
    --version | sed -n '1p')"
jq -e \
  --arg implementation "${SERVICE_IMPLEMENTATION_COMMIT}" \
  --arg stack "${STACK_LOCK_SHA256}" \
  '.implementation_commit == $implementation and .stack_lock_sha256 == $stack' \
  "${SERVICE_ROOT}/config/release.lock.json" >/dev/null ||
  die 'accepted production release-lock semantics drifted'
require_sha256 "${DATASET_LOCK}" "${DATASET_LOCK_SHA256}"
jq -e \
  --arg dataset "${DATASET_NAME%@*}" \
  --arg digest "${DATASET_CONTENT_DIGEST}" \
  --arg harbor_version "${HARBOR_VERSION}" \
  --arg harbor_commit "${HARBOR_COMMIT}" \
  --argjson tasks "${EXPECTED_TASKS}" \
  '.schema_version == 1 and .dataset.name == $dataset and
   .dataset.selected_reference == $digest and .dataset.task_count == $tasks and
   (.dataset.tasks | length) == $tasks and
   ([.dataset.tasks[].task_id] | unique | length) == $tasks and
   ([.dataset.tasks[].task_id] == ([.dataset.tasks[].task_id] | sort)) and
   ([.dataset.tasks[].content_hash] | all(test("^[0-9a-f]{64}$"))) and
   (.dataset.files == [{path:"README.md",
     content_hash:"2cbef204aa1f09c36c62b94b9b72c6abfee38017a534575564f9ceff7ce21cca",
     size_bytes:1756,
     storage_path:"packages/ibragim-badertdinov/swe-rebench-07-2026/2cbef204aa1f09c36c62b94b9b72c6abfee38017a534575564f9ceff7ce21cca/README.md"}]) and
   .harbor.version == $harbor_version and .harbor.commit == $harbor_commit' \
  "${DATASET_LOCK}" >/dev/null || die 'pinned full-suite dataset lock is invalid'

TASK_COUNT="$(find "${DATASET_ROOT}" -mindepth 1 -maxdepth 1 -type d -printf '%f\n' | wc -l)"
readonly TASK_COUNT
require_equal 'dataset task count' "${EXPECTED_TASKS}" "${TASK_COUNT}"
[[ -z "$(find "${DATASET_ROOT}" -type l -print -quit)" ]] ||
  die 'dataset package itself contains a symbolic link'
[[ -z "$(find "${DATASET_ROOT}" \! -type d \! -type f -print -quit)" ]] ||
  die 'dataset package contains a special file'
require_equal 'dataset-level file set' README.md \
  "$(find "${DATASET_ROOT}" -mindepth 1 -maxdepth 1 -type f -printf '%f\n' | LC_ALL=C sort)"
cmp \
  <(find "${DATASET_ROOT}" -mindepth 1 -maxdepth 1 -type d -printf '%f\n' | LC_ALL=C sort) \
  <(jq -r '.dataset.tasks[].task_id' "${DATASET_LOCK}" | LC_ALL=C sort) ||
  die 'dataset task-directory set differs from the pinned registry digest'
require_sha256 "${DATASET_ROOT}/README.md" \
  "$(jq -er '.dataset.files[] | select(.path == "README.md") | .content_hash' "${DATASET_LOCK}")"
require_equal 'dataset README bytes' \
  "$(jq -er '.dataset.files[] | select(.path == "README.md") | .size_bytes' "${DATASET_LOCK}")" \
  "$(stat -c '%s' -- "${DATASET_ROOT}/README.md")"

compute_harbor_task_content_hash() {
  local task_root="$1" path relative digest
  [[ ! -e "${task_root}/.gitignore" ]] ||
    die "unexpected task-level .gitignore requires pinned PathSpec evaluation: ${task_root}"
  [[ -z "$(find "${task_root}" \
    \( -path '*/__pycache__/*' -o -name '*.pyc' -o -name '.DS_Store' -o \
       -name '*.swp' -o -name '*.swo' -o -name '*~' \) -print -quit)" ]] ||
    die "task contains a Harbor-default-ignored transient path: ${task_root}"
  {
    for path in task.toml instruction.md README.md; do
      [[ ! -f "${task_root}/${path}" ]] || printf '%s\0' "${task_root}/${path}"
    done
    for path in environment tests solution steps; do
      [[ ! -d "${task_root}/${path}" ]] || find "${task_root}/${path}" -type f -print0
    done
  } | LC_ALL=C sort -z |
    while IFS= read -r -d '' path; do
      relative="${path#"${task_root}"/}"
      digest="$(sha256sum -- "${path}" | awk '{print $1}')"
      printf '%s\0%s\n' "${relative}" "${digest}"
    done | sha256sum | awk '{print $1}'
}

verified_task_hashes=0
while IFS=$'\t' read -r task_id expected_content_hash; do
  task_root="${DATASET_ROOT}/${task_id}"
  require_equal "task package top-level shape for ${task_id}" \
    $'d environment\nd solution\nd tests\nf instruction.md\nf task.toml' \
    "$(find "${task_root}" -mindepth 1 -maxdepth 1 -printf '%y %f\n' | LC_ALL=C sort)"
  require_equal "Harbor content hash for ${task_id}" "${expected_content_hash}" \
    "$(compute_harbor_task_content_hash "${task_root}")"
  verified_task_hashes=$((verified_task_hashes + 1))
done < <(jq -r '.dataset.tasks[] | [.task_id,.content_hash] | @tsv' "${DATASET_LOCK}")
require_equal 'verified Harbor task content-hash count' "${EXPECTED_TASKS}" "${verified_task_hashes}"

mkdir -p -- "${SUITE_ROOT}" "${MATERIALIZATION_ROOT}"
chmod 0700 -- "${SUITE_ROOT}" "${MATERIALIZATION_ROOT}"
exec 9>"${SUITE_ROOT}/materialize.lock"
flock -n 9 || die 'another full-suite materializer already holds the suite lock'

write_regular_manifest() {
  local root="$1" output="$2"
  (
    cd "${root}"
    find . -type f -print0 | LC_ALL=C sort -z |
      xargs -0 -r sha256sum --zero --
  ) >"${output}"
}

write_mode_manifest() {
  local root="$1" output="$2"
  (
    cd "${root}"
    find . -mindepth 1 -printf '%y %m %P -> %l\0' | LC_ALL=C sort -z
  ) >"${output}"
}

write_symlink_manifest() {
  local root="$1" output="$2"
  (
    cd "${root}"
    find . -type l -printf '%P -> %l\0' | LC_ALL=C sort -z
  ) >"${output}"
}

write_image_regular_manifest() {
  local env_id="$1" output="$2"
  docker run --rm --network none --cap-drop ALL --security-opt no-new-privileges \
    --read-only --memory 1g --pids-limit 128 --env LC_ALL=C \
    --tmpfs /tmp:rw,nosuid,nodev,noexec,size=256m,mode=1777 \
    --env PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    --user 0:0 --entrypoint bash "${env_id}" -Eeuo pipefail -c \
    'find . -type f -print0 | sort -z | xargs -0 -r sha256sum --zero --' >"${output}"
}

write_image_mode_manifest() {
  local env_id="$1" output="$2"
  docker run --rm --network none --cap-drop ALL --security-opt no-new-privileges \
    --read-only --memory 1g --pids-limit 128 --env LC_ALL=C \
    --tmpfs /tmp:rw,nosuid,nodev,noexec,size=256m,mode=1777 \
    --env PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    --user 0:0 --entrypoint bash "${env_id}" -Eeuo pipefail -c \
    'find . -mindepth 1 -printf "%y %m %P -> %l\0" | sort -z' >"${output}"
}

write_image_symlink_manifest() {
  local env_id="$1" output="$2"
  docker run --rm --network none --cap-drop ALL --security-opt no-new-privileges \
    --read-only --memory 1g --pids-limit 128 --env LC_ALL=C \
    --tmpfs /tmp:rw,nosuid,nodev,noexec,size=256m,mode=1777 \
    --env PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    --user 0:0 --entrypoint bash "${env_id}" -Eeuo pipefail -c \
    'find . -type l -printf "%P -> %l\0" | sort -z' >"${output}"
}

write_environment_git_status() {
  local env_id="$1" expected_base_commit="$2" output="$3"
  docker run --rm --network none --cap-drop ALL \
    --security-opt no-new-privileges --read-only --memory 1g --pids-limit 128 \
    --tmpfs /tmp:rw,nosuid,nodev,noexec,size=16m,mode=1777 \
    --env EXPECTED_BASE_COMMIT="${expected_base_commit}" \
    --env GIT_CONFIG_NOSYSTEM=1 --env GIT_OPTIONAL_LOCKS=0 --env HOME=/tmp/no-home \
    --env LC_ALL=C \
    --env PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    --user 0:0 --entrypoint bash "${env_id}" -Eeuo pipefail -c \
    'test -d .git && test ! -L .git;
     git_bin="$(command -v git)"; test -n "$git_bin"; test -x "$git_bin";
     test "$PWD" = "$(pwd -P)";
     test "$PWD" = "$("$git_bin" -c core.fsmonitor=false -c core.hooksPath=/dev/null rev-parse --show-toplevel)";
     test "$("$git_bin" -c core.fsmonitor=false -c core.hooksPath=/dev/null rev-parse HEAD)" = "$EXPECTED_BASE_COMMIT";
     "$git_bin" -c core.fsmonitor=false -c core.hooksPath=/dev/null status \
       --porcelain=v1 -z --untracked-files=all --ignore-submodules=none' >"${output}"
}

write_copied_git_status() {
  local env_id="$1" expected_base_commit="$2" source_root="$3" output="$4"
  docker run --rm --network none --cap-drop ALL \
    --security-opt no-new-privileges --read-only --memory 1g --pids-limit 128 \
    --tmpfs /tmp:rw,nosuid,nodev,noexec,size=16m,mode=1777 \
    --env EXPECTED_BASE_COMMIT="${expected_base_commit}" \
    --env GIT_CONFIG_NOSYSTEM=1 --env GIT_OPTIONAL_LOCKS=0 --env HOME=/tmp/no-home \
    --env LC_ALL=C \
    --env PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    --mount "type=bind,src=${source_root},dst=/source,readonly" \
    --workdir /source --user 1000:1000 --entrypoint bash "${env_id}" -Eeuo pipefail -c \
    'test -d .git && test ! -L .git;
     git_bin="$(command -v git)"; test -n "$git_bin"; test -x "$git_bin";
     test "$("$git_bin" -c core.fsmonitor=false -c core.hooksPath=/dev/null -c safe.directory=/source rev-parse HEAD)" = "$EXPECTED_BASE_COMMIT";
     "$git_bin" -c core.fsmonitor=false -c core.hooksPath=/dev/null -c safe.directory=/source status \
       --porcelain=v1 -z --untracked-files=all --ignore-submodules=none' >"${output}"
}

verify_source_manifests() {
  local task_dir="$1"
  local source_root="${task_dir}/source" scratch env_id
  env_id="$(jq -er '.environment.image_id' "${task_dir}/manifest.json")"
  scratch="$(mktemp -d "${SUITE_ROOT}/.verify-source.XXXXXXXX")"
  write_regular_manifest "${source_root}" "${scratch}/regular.sha256z"
  write_mode_manifest "${source_root}" "${scratch}/modes.z"
  write_symlink_manifest "${source_root}" "${scratch}/symlinks.z"
  write_image_regular_manifest "${env_id}" "${scratch}/image-regular.sha256z"
  write_image_mode_manifest "${env_id}" "${scratch}/image-modes.z"
  write_image_symlink_manifest "${env_id}" "${scratch}/image-symlinks.z"
  cmp -- "${task_dir}/source-regular.sha256z" "${scratch}/regular.sha256z" ||
    die "source regular-file content drift: ${source_root}"
  cmp -- "${task_dir}/source-modes.z" "${scratch}/modes.z" ||
    die "source type/mode/target drift: ${source_root}"
  cmp -- "${task_dir}/source-symlinks.z" "${scratch}/symlinks.z" ||
    die "source symlink drift: ${source_root}"
  cmp -- "${task_dir}/source-regular.sha256z" "${scratch}/image-regular.sha256z" ||
    die "materialized regular files differ from the exact environment image: ${source_root}"
  cmp -- "${task_dir}/source-modes.z" "${scratch}/image-modes.z" ||
    die "materialized types/modes/targets differ from the exact environment image: ${source_root}"
  cmp -- "${task_dir}/source-symlinks.z" "${scratch}/image-symlinks.z" ||
    die "materialized symlinks differ from the exact environment image: ${source_root}"
  rm -rf -- "${scratch}"
}

verify_materialized_repository() {
  local task_dir="$1" env_id base_commit status_path scratch environment_status copied_status
  local status_sha status_bytes status_clean manifest_has_status_fields migration_partial
  env_id="$(jq -er '.environment.image_id' "${task_dir}/manifest.json")"
  base_commit="$(jq -er '.source.base_commit' "${task_dir}/manifest.json")"
  status_path="${task_dir}/initial-git-status.z"
  scratch="$(mktemp -d "${SUITE_ROOT}/.verify-git-status.XXXXXXXX")"
  environment_status="${scratch}/environment.z"
  copied_status="${scratch}/copied.z"
  write_environment_git_status "${env_id}" "${base_commit}" "${environment_status}"
  write_copied_git_status "${env_id}" "${base_commit}" "${task_dir}/source" "${copied_status}"
  cmp -- "${environment_status}" "${copied_status}" ||
    die "copied repository Git status differs from the exact environment image: $(basename "${task_dir}")"

  status_sha="$(sha256sum -- "${environment_status}" | awk '{print $1}')"
  status_bytes="$(stat -c '%s' -- "${environment_status}")"
  status_clean=false
  (( status_bytes != 0 )) || status_clean=true
  manifest_has_status_fields="$(jq -r \
    'if (.source.initial_git_status_sha256 | type) == "string" and
        (.source.initial_git_status_bytes | type) == "number" and
        (.source.initial_worktree_clean | type) == "boolean"
     then "complete"
     elif .source.initial_git_status_sha256 == null and
          .source.initial_git_status_bytes == null and
          .source.initial_worktree_clean == null
     then "absent"
     else "partial"
     end' "${task_dir}/manifest.json")"
  [[ "${manifest_has_status_fields}" != partial ]] ||
    die "initial Git-status fields are only partially present: $(basename "${task_dir}")"

  if [[ -e "${status_path}" ]]; then
    [[ -f "${status_path}" && ! -L "${status_path}" ]] ||
      die "initial Git-status evidence is not a regular file: ${status_path}"
    [[ ! -e "${status_path}.partial" ]] ||
      die "initial Git-status evidence has an unexpected partial sibling: ${status_path}.partial"
    cmp -- "${status_path}" "${environment_status}" ||
      die "initial Git-status evidence differs from the exact environment image: $(basename "${task_dir}")"
  else
    if [[ -e "${status_path}.partial" ]]; then
      [[ -f "${status_path}.partial" && ! -L "${status_path}.partial" ]] ||
        die "partial initial Git-status evidence is not a regular file: ${status_path}.partial"
      cmp -- "${status_path}.partial" "${environment_status}" ||
        die "partial initial Git-status evidence differs from the exact environment image: $(basename "${task_dir}")"
    else
      cp -- "${environment_status}" "${status_path}.partial"
    fi
    sync -f "${status_path}.partial"
    mv -- "${status_path}.partial" "${status_path}"
    sync -f "${task_dir}"
  fi

  if [[ "${manifest_has_status_fields}" == absent ]]; then
    migration_partial="${task_dir}/manifest.json.git-status-migration.partial"
    jq --arg status_sha "${status_sha}" --argjson status_bytes "${status_bytes}" \
      --argjson status_clean "${status_clean}" \
      '.source.initial_git_status_sha256 = $status_sha |
       .source.initial_git_status_bytes = $status_bytes |
       .source.initial_worktree_clean = $status_clean' \
      "${task_dir}/manifest.json" >"${scratch}/migrated-manifest.json"
    if [[ -e "${migration_partial}" ]]; then
      [[ -f "${migration_partial}" && ! -L "${migration_partial}" ]] ||
        die "partial Git-status manifest migration is not a regular file: ${migration_partial}"
      cmp -- "${migration_partial}" "${scratch}/migrated-manifest.json" ||
        die "partial Git-status manifest migration differs from the exact regenerated form: $(basename "${task_dir}")"
    else
      cp -- "${scratch}/migrated-manifest.json" "${migration_partial}"
    fi
    sync -f "${migration_partial}"
    mv -- "${migration_partial}" "${task_dir}/manifest.json"
    sync -f "${task_dir}"
  else
    [[ ! -e "${task_dir}/manifest.json.git-status-migration.partial" ]] ||
      die "completed Git-status manifest has an unexpected partial migration sibling: $(basename "${task_dir}")"
  fi

  jq -e --arg status_sha "${status_sha}" --argjson status_bytes "${status_bytes}" \
    --argjson status_clean "${status_clean}" \
    '.source.initial_git_status_sha256 == $status_sha and
     .source.initial_git_status_bytes == $status_bytes and
     .source.initial_worktree_clean == $status_clean' \
    "${task_dir}/manifest.json" >/dev/null ||
    die "initial Git-status manifest evidence mismatch: $(basename "${task_dir}")"
  require_sha256 "${status_path}" "${status_sha}"
  rm -rf -- "${scratch}"
}

restore_environment_image_if_needed() {
  local task_dir="$1" env_id env_tag archive
  env_id="$(jq -er '.environment.image_id' "${task_dir}/manifest.json")"
  env_tag="$(jq -er '.environment.image_tag' "${task_dir}/manifest.json")"
  archive="${task_dir}/environment-image.tar"
  if docker image inspect "${env_tag}" >/dev/null 2>&1; then
    require_equal "existing environment tag for $(basename "${task_dir}")" "${env_id}" \
      "$(docker image inspect --format '{{.Id}}' "${env_tag}")"
  elif ! docker image inspect "${env_id}" >/dev/null 2>&1; then
    docker load --input "${archive}" >"${task_dir}/restore.log.partial"
    sync -f "${task_dir}/restore.log.partial"
    mv -- "${task_dir}/restore.log.partial" "${task_dir}/restore.log"
    sync -f "${task_dir}"
  else
    docker load --input "${archive}" >"${task_dir}/restore.log.partial"
    sync -f "${task_dir}/restore.log.partial"
    mv -- "${task_dir}/restore.log.partial" "${task_dir}/restore.log"
    sync -f "${task_dir}"
  fi
  require_equal "restored environment image for $(basename "${task_dir}")" "${env_id}" \
    "$(docker image inspect --format '{{.Id}}' "${env_tag}")"
}

validate_completed_task() {
  local task_id="$1"
  local task_dir="${MATERIALIZATION_ROOT}/${task_id}"
  local manifest="${task_dir}/manifest.json" archive_sha archive_bytes registry_content_hash evidence
  local materialization_method
  [[ -d "${task_dir}" && ! -L "${task_dir}" ]] ||
    die "completed task directory is absent or a symlink: ${task_dir}"
  [[ -f "${manifest}" && ! -L "${manifest}" ]] ||
    die "completed task manifest is absent or a symlink: ${manifest}"
  for evidence in pull.log build.log load.log environment-probe.txt \
    materialize-container-id.txt materialize-container-removed-id.txt \
    source-regular.sha256z source-modes.z source-symlinks.z; do
    [[ -f "${task_dir}/${evidence}" && ! -L "${task_dir}/${evidence}" ]] ||
      die "completed task evidence is absent or a symlink: ${task_dir}/${evidence}"
  done
  [[ -z "$(find "${task_dir}" -mindepth 1 -maxdepth 1 \
    \! -type d \! -type f -print -quit)" ]] ||
    die "completed task top level contains a symlink or special file: ${task_id}"
  registry_content_hash="$(jq -er --arg task "${task_id}" \
    '.dataset.tasks[] | select(.task_id == $task) | .content_hash' "${DATASET_LOCK}")"
  jq -e --arg task "${task_id}" \
    --arg dataset_digest "${DATASET_CONTENT_DIGEST}" \
    --arg dataset_lock_sha "${DATASET_LOCK_SHA256}" \
    --arg registry_content_hash "${registry_content_hash}" \
    --arg release_commit "${SERVICE_RELEASE_COMMIT}" \
    --arg release_lock_sha "${SERVICE_RELEASE_LOCK_SHA256}" \
    --arg stack_lock_sha "${STACK_LOCK_SHA256}" \
    '.schema_version == 1 and .task_id == $task and
     .dataset.content_digest == $dataset_digest and
     .dataset.lock_sha256 == $dataset_lock_sha and
     .inputs.harbor_content_hash == $registry_content_hash and
     .production_release.release_commit == $release_commit and
     .production_release.release_lock_sha256 == $release_lock_sha and
     .production_release.stack_lock_sha256 == $stack_lock_sha and
     (.classification == "eligible" or .classification == "production-input-contract-exclusion") and
     (.policy_order == [false,true] or .policy_order == [true,false])' \
    "${manifest}" >/dev/null || die "completed task manifest semantic mismatch: ${task_id}"
  materialization_method="$(jq -r '.source.materialization_method // "legacy Docker direct destination copy"' \
    "${manifest}")"
  if [[ "${materialization_method}" == \
    'Docker archive stream plus pinned non-root GNU tar delayed-directory restoration' ]]; then
    require_equal "source extractor image for ${task_id}" "${SOURCE_EXTRACTOR_IMAGE_ID}" \
      "$(jq -er '.source.extractor_image_id' "${manifest}")"
    require_equal "source extractor tar version for ${task_id}" "${SOURCE_EXTRACTOR_TAR_VERSION}" \
      "$(jq -er '.source.extractor_tar_version' "${manifest}")"
    for evidence in source-archive.log source-extract.log; do
      [[ -f "${task_dir}/${evidence}" && ! -L "${task_dir}/${evidence}" ]] ||
        die "completed streamed-copy evidence is absent or a symlink: ${task_dir}/${evidence}"
    done
  elif [[ "${materialization_method}" != 'legacy Docker direct destination copy' ]]; then
    die "unrecognized source materialization method for ${task_id}: ${materialization_method}"
  fi
  archive_sha="$(jq -er '.environment.archive_sha256' "${manifest}")"
  archive_bytes="$(jq -er '.environment.archive_bytes' "${manifest}")"
  require_sha256 "${task_dir}/environment-image.tar" "${archive_sha}"
  require_equal "environment archive bytes for ${task_id}" "${archive_bytes}" \
    "$(stat -c '%s' -- "${task_dir}/environment-image.tar")"
  require_equal "environment archive ownership/mode for ${task_id}" 1000:1000:600 \
    "$(stat -c '%u:%g:%a' -- "${task_dir}/environment-image.tar")"
  require_sha256 "${task_dir}/source-regular.sha256z" \
    "$(jq -er '.source.regular_manifest_sha256' "${manifest}")"
  require_sha256 "${task_dir}/source-modes.z" \
    "$(jq -er '.source.mode_manifest_sha256' "${manifest}")"
  require_sha256 "${task_dir}/source-symlinks.z" \
    "$(jq -er '.source.symlink_manifest_sha256' "${manifest}")"
  restore_environment_image_if_needed "${task_dir}"
  [[ -d "${task_dir}/source/.git" && ! -L "${task_dir}/source/.git" ]] ||
    die "materialized source has no real .git directory: ${task_id}"
  verify_materialized_repository "${task_dir}"
  [[ -f "${task_dir}/initial-git-status.z" && ! -L "${task_dir}/initial-git-status.z" ]] ||
    die "completed task Git-status evidence is absent or a symlink: ${task_id}"
  verify_source_manifests "${task_dir}"
}

readonly DATASET_REGULAR_MANIFEST="${SUITE_ROOT}/dataset-regular.sha256z"
readonly DATASET_MODE_MANIFEST="${SUITE_ROOT}/dataset-modes.z"
if [[ ! -e "${DATASET_REGULAR_MANIFEST}" && ! -e "${DATASET_MODE_MANIFEST}" ]]; then
  write_regular_manifest "${DATASET_ROOT}" "${DATASET_REGULAR_MANIFEST}.partial"
  write_mode_manifest "${DATASET_ROOT}" "${DATASET_MODE_MANIFEST}.partial"
  sync -f "${DATASET_REGULAR_MANIFEST}.partial"
  sync -f "${DATASET_MODE_MANIFEST}.partial"
  mv -- "${DATASET_REGULAR_MANIFEST}.partial" "${DATASET_REGULAR_MANIFEST}"
  mv -- "${DATASET_MODE_MANIFEST}.partial" "${DATASET_MODE_MANIFEST}"
  sync -f "${SUITE_ROOT}"
else
  [[ -f "${DATASET_REGULAR_MANIFEST}" && -f "${DATASET_MODE_MANIFEST}" ]] ||
    die 'dataset manifest pair is incomplete'
  DATASET_VERIFY_DIR="$(mktemp -d "${SUITE_ROOT}/.verify-dataset.XXXXXXXX")"
  readonly DATASET_VERIFY_DIR
  write_regular_manifest "${DATASET_ROOT}" "${DATASET_VERIFY_DIR}/regular.sha256z"
  write_mode_manifest "${DATASET_ROOT}" "${DATASET_VERIFY_DIR}/modes.z"
  cmp -- "${DATASET_REGULAR_MANIFEST}" "${DATASET_VERIFY_DIR}/regular.sha256z" ||
    die 'dataset regular-file content drift'
  cmp -- "${DATASET_MODE_MANIFEST}" "${DATASET_VERIFY_DIR}/modes.z" ||
    die 'dataset type/mode drift'
  rm -rf -- "${DATASET_VERIFY_DIR}"
fi

materialize_task() {
  local task_id="$1" task_index="$2"
  local task_root="${DATASET_ROOT}/${task_id}"
  local final_dir="${MATERIALIZATION_ROOT}/${task_id}"
  local partial_dir="${MATERIALIZATION_ROOT}/${task_id}.partial"
  local dockerfile="${task_root}/environment/Dockerfile"
  local instruction="${task_root}/instruction.md"
  local task_toml="${task_root}/task.toml"
  local test_config="${task_root}/tests/config.json"
  local test_sh="${task_root}/tests/test.sh"
  local test_parser="${task_root}/tests/swan_log_parsers.py"
  local source_root="${partial_dir}/source"
  local base_ref base_repository base_id base_digest env_tag env_id workdir config_user base_commit
  local language log_parser instruction_bytes file_count source_bytes symlink_count special_count
  local classification=eligible exclusion_reason='' policy_first=false policy_second=true
  local archive_sha archive_bytes regular_sha modes_sha symlinks_sha container_name source_archive
  local initial_git_status_sha initial_git_status_bytes initial_worktree_clean copied_git_status
  local registry_content_hash network_policy_source
  local -a docker_lines

  if [[ -e "${final_dir}" ]]; then
    [[ ! -e "${partial_dir}" ]] || die "both final and partial task states exist: ${task_id}"
    validate_completed_task "${task_id}"
    printf 'MATERIALIZATION_REUSED task=%s index=%s classification=%s\n' \
      "${task_id}" "${task_index}" "$(jq -er '.classification' "${final_dir}/manifest.json")"
    return
  fi
  [[ ! -e "${partial_dir}" ]] ||
    die "partial task state requires explicit investigation before resume: ${partial_dir}"
  mkdir -- "${partial_dir}"
  chmod 0700 -- "${partial_dir}"

  for path in "${dockerfile}" "${instruction}" "${task_toml}" "${test_config}" \
    "${test_sh}" "${test_parser}"; do
    [[ -f "${path}" && ! -L "${path}" ]] || die "task input is absent or a symlink: ${path}"
  done
  jq -e --arg task "${task_id}" '.instance_id == $task and (.language | type == "string") and
    (.install_config.log_parser | type == "string") and (.base_commit | test("^[0-9a-f]{40}$"))' \
    "${test_config}" >/dev/null || die "test config identity is malformed: ${task_id}"
  registry_content_hash="$(jq -er --arg task "${task_id}" \
    '.dataset.tasks[] | select(.task_id == $task) | .content_hash' "${DATASET_LOCK}")"
  require_equal "task name declaration count for ${task_id}" 1 \
    "$(awk -v expected="name = \"ibragim-badertdinov/${task_id}\"" \
      '$0 == expected {count++} END {print count + 0}' "${task_toml}")"
  require_equal "agent/verifier timeout declarations for ${task_id}" 2 \
    "$(awk '$0 == "timeout_sec = 3000.0" {count++} END {print count + 0}' "${task_toml}")"
  require_equal "CPU declaration count for ${task_id}" 1 \
    "$(awk '$0 == "cpus = 1" {count++} END {print count + 0}' "${task_toml}")"
  require_equal "memory declaration count for ${task_id}" 1 \
    "$(awk '$0 == "memory_mb = 4096" {count++} END {print count + 0}' "${task_toml}")"
  require_equal "storage declaration count for ${task_id}" 1 \
    "$(awk '$0 == "storage_mb = 10240" {count++} END {print count + 0}' "${task_toml}")"
  if [[ "${task_id}" == apache__dubbo-go-3357 ]]; then
    require_equal 'apache__dubbo-go-3357 explicit network_mode declaration count' 0 \
      "$(awk '$0 == "network_mode = \"public\"" {count++} END {print count + 0}' "${task_toml}")"
    require_equal 'apache__dubbo-go-3357 legacy allow_internet declaration count' 1 \
      "$(awk '$0 == "allow_internet = true" {count++} END {print count + 0}' "${task_toml}")"
    network_policy_source='Harbor v0.21.0 legacy allow_internet=true migration to public'
  else
    require_equal "public network declaration count for ${task_id}" 1 \
      "$(awk '$0 == "network_mode = \"public\"" {count++} END {print count + 0}' "${task_toml}")"
    require_equal "legacy allow_internet declaration count for ${task_id}" 0 \
      "$(awk '$0 == "allow_internet = true" {count++} END {print count + 0}' "${task_toml}")"
    network_policy_source='explicit task.toml environment.network_mode=public'
  fi
  mapfile -t docker_lines < <(grep -v '^$' "${dockerfile}")
  require_equal "Dockerfile nonblank line count for ${task_id}" 3 "${#docker_lines[@]}"
  [[ "${docker_lines[0]}" == FROM\ docker.io/swerebenchv2/*:v0.1.0 ]] ||
    die "unexpected task base image reference: ${docker_lines[0]}"
  require_equal "uv installer line for ${task_id}" "${UV_INSTALL}" "${docker_lines[1]}"
  require_equal "logs directory line for ${task_id}" "${LOGS_INSTALL}" "${docker_lines[2]}"
  base_ref="${docker_lines[0]#FROM }"
  require_equal "test-config image for ${task_id}" "${base_ref}" \
    "$(jq -er '.image_name' "${test_config}")"
  language="$(jq -er '.language' "${test_config}")"
  log_parser="$(jq -er '.install_config.log_parser' "${test_config}")"
  base_commit="$(jq -er '.base_commit' "${test_config}")"
  instruction_bytes="$(stat -c '%s' -- "${instruction}")"

  printf 'Pulling immutable candidate base for %s (%s).\n' "${task_id}" "${base_ref}" >&2
  docker pull --platform linux/amd64 "${base_ref}" >"${partial_dir}/pull.log" 2>&1
  base_id="$(docker image inspect --format '{{.Id}}' "${base_ref}")"
  base_repository="${base_ref#docker.io/}"
  base_repository="${base_repository%:v0.1.0}"
  base_digest="$(docker image inspect "${base_ref}" | jq -er --arg repository "${base_repository}" \
    '.[0].RepoDigests | map(select(startswith($repository + "@sha256:"))) |
     if length == 1 then .[0] else error("expected exactly one matching repository digest") end')"
  [[ "${base_id}" == sha256:* && "${base_digest}" == *@sha256:* ]] ||
    die "pulled base lacks content identities: ${task_id}"
  require_equal "base architecture for ${task_id}" amd64 \
    "$(docker image inspect --format '{{.Architecture}}' "${base_ref}")"
  require_equal "base OS for ${task_id}" linux \
    "$(docker image inspect --format '{{.Os}}' "${base_ref}")"

  env_tag="${ENV_REPOSITORY}:$(printf '%s' "${task_id}" | tr '[:upper:]' '[:lower:]')"
  [[ "${#env_tag}" -le 180 ]] || die "derived environment tag is unexpectedly long: ${env_tag}"
  if docker image inspect "${env_tag}" >/dev/null 2>&1; then
    die "unowned environment tag already exists without a completed manifest: ${env_tag}"
  fi
  printf 'Building and archiving exact evaluator environment for %s.\n' "${task_id}" >&2
  BUILDKIT_PROGRESS=plain docker buildx build --builder default --platform linux/amd64 \
    --pull=false --no-cache --network=default --provenance=false \
    --output "type=docker,name=${env_tag},dest=${partial_dir}/environment-image.tar.partial" \
    "${task_root}/environment" >"${partial_dir}/build.log" 2>&1
  grep -Fq 'no checksums to verify' "${partial_dir}/build.log" ||
    die "uv installer checksum observation is absent: ${task_id}"
  grep -Fq "${base_ref}@${base_digest#*@}" "${partial_dir}/build.log" ||
    die "build log does not prove the pulled base digest: ${task_id}"
  chmod 0600 -- "${partial_dir}/environment-image.tar.partial"
  sync -f "${partial_dir}/environment-image.tar.partial"
  mv -- "${partial_dir}/environment-image.tar.partial" "${partial_dir}/environment-image.tar"
  docker load --input "${partial_dir}/environment-image.tar" >"${partial_dir}/load.log"
  env_id="$(docker image inspect --format '{{.Id}}' "${env_tag}")"
  require_equal "environment architecture for ${task_id}" amd64 \
    "$(docker image inspect --format '{{.Architecture}}' "${env_id}")"
  require_equal "environment OS for ${task_id}" linux \
    "$(docker image inspect --format '{{.Os}}' "${env_id}")"
  workdir="$(docker image inspect --format '{{.Config.WorkingDir}}' "${env_id}")"
  config_user="$(docker image inspect --format '{{.Config.User}}' "${env_id}")"
  [[ "${workdir}" == /* && "${workdir}" != / && "${workdir}" != *$'\n'* ]] ||
    die "unsafe or empty environment working directory for ${task_id}: ${workdir}"

  docker run --rm --network none --cap-drop ALL --security-opt no-new-privileges \
    --user 0:0 --env GIT_CONFIG_NOSYSTEM=1 --env GIT_OPTIONAL_LOCKS=0 --env HOME=/tmp/no-home \
    --entrypoint bash "${env_id}" -Eeuo pipefail -c \
    'test "$PWD" = "$(git -c core.fsmonitor=false -c core.hooksPath=/dev/null rev-parse --show-toplevel)";
     test "$PWD" = "$(pwd -P)";
     test "$(uv --version)" = "uv 0.7.13";
     test -d /logs;
     git -c core.fsmonitor=false -c core.hooksPath=/dev/null rev-parse HEAD' >"${partial_dir}/environment-probe.txt"
  require_equal "environment repository HEAD for ${task_id}" "${base_commit}" \
    "$(tr -d '[:space:]' <"${partial_dir}/environment-probe.txt")"
  write_environment_git_status "${env_id}" "${base_commit}" \
    "${partial_dir}/initial-git-status.z"

  mkdir -- "${source_root}"
  container_name="qwen38-swe-materialize-$(printf '%s' "${task_id}" | sha256sum | cut -c1-20)"
  [[ -z "$(docker ps -a --filter "name=^/${container_name}$" --format '{{.ID}}')" ]] ||
    die "materialization container name collision: ${container_name}"
  ACTIVE_CONTAINER="${container_name}"
  docker create --name "${container_name}" --network none --entrypoint true "${env_id}" \
    >"${partial_dir}/materialize-container-id.txt"
  source_archive="${partial_dir}/source.tar.partial"
  docker cp "${container_name}:${workdir}/." - >"${source_archive}" \
    2>"${partial_dir}/source-archive.log"
  chmod 0600 -- "${source_archive}"
  sync -f "${source_archive}"
  docker rm "${container_name}" >"${partial_dir}/materialize-container-removed-id.txt"
  ACTIVE_CONTAINER=
  docker run --rm --network none --cap-drop ALL --security-opt no-new-privileges \
    --read-only --memory 1g --pids-limit 128 --user 1000:1000 \
    --mount "type=bind,src=${source_archive},dst=/input/source.tar,readonly" \
    --mount "type=bind,src=${source_root},dst=/output" \
    --entrypoint tar "${SOURCE_EXTRACTOR_IMAGE_ID}" \
    --extract --file=/input/source.tar --directory=/output --no-same-owner \
    --same-permissions --delay-directory-restore >"${partial_dir}/source-extract.log" 2>&1
  rm -- "${source_archive}"
  sync -f "${partial_dir}"
  chmod 0700 -- "${source_root}"
  [[ -d "${source_root}/.git" && ! -L "${source_root}/.git" ]] ||
    die "materialized source lacks a real .git directory: ${task_id}"
  [[ -z "$(find "${source_root}" \! -user 1000 -print -quit)" ]] ||
    die "materialized source contains a file not owned by uid 1000: ${task_id}"
  copied_git_status="${partial_dir}/copied-git-status.verify"
  write_copied_git_status "${env_id}" "${base_commit}" "${source_root}" \
    "${copied_git_status}"
  cmp -- "${partial_dir}/initial-git-status.z" "${copied_git_status}" ||
    die "copied repository Git status differs from the exact environment image: ${task_id}"
  rm -- "${copied_git_status}"
  initial_git_status_sha="$(sha256sum -- "${partial_dir}/initial-git-status.z" | awk '{print $1}')"
  initial_git_status_bytes="$(stat -c '%s' -- "${partial_dir}/initial-git-status.z")"
  initial_worktree_clean=false
  (( initial_git_status_bytes != 0 )) || initial_worktree_clean=true

  file_count="$(find "${source_root}" -type f -printf '.\n' | wc -l)"
  source_bytes="$(find "${source_root}" -type f -printf '%s\n' | awk '{sum += $1} END {print sum + 0}')"
  symlink_count="$(find "${source_root}" -type l -printf '.\n' | wc -l)"
  special_count="$(find "${source_root}" \! -type d \! -type f \! -type l -printf '.\n' | wc -l)"
  write_regular_manifest "${source_root}" "${partial_dir}/source-regular.sha256z"
  write_mode_manifest "${source_root}" "${partial_dir}/source-modes.z"
  write_symlink_manifest "${source_root}" "${partial_dir}/source-symlinks.z"
  write_image_regular_manifest "${env_id}" "${partial_dir}/source-image-regular.verify"
  write_image_mode_manifest "${env_id}" "${partial_dir}/source-image-modes.verify"
  write_image_symlink_manifest "${env_id}" "${partial_dir}/source-image-symlinks.verify"
  cmp -- "${partial_dir}/source-regular.sha256z" \
    "${partial_dir}/source-image-regular.verify" ||
    die "materialized regular files differ from the exact environment image: ${task_id}"
  cmp -- "${partial_dir}/source-modes.z" "${partial_dir}/source-image-modes.verify" ||
    die "materialized types/modes/targets differ from the exact environment image: ${task_id}"
  cmp -- "${partial_dir}/source-symlinks.z" \
    "${partial_dir}/source-image-symlinks.verify" ||
    die "materialized symlinks differ from the exact environment image: ${task_id}"
  rm -- "${partial_dir}/source-image-regular.verify" \
    "${partial_dir}/source-image-modes.verify" \
    "${partial_dir}/source-image-symlinks.verify"
  regular_sha="$(sha256sum -- "${partial_dir}/source-regular.sha256z" | awk '{print $1}')"
  modes_sha="$(sha256sum -- "${partial_dir}/source-modes.z" | awk '{print $1}')"
  symlinks_sha="$(sha256sum -- "${partial_dir}/source-symlinks.z" | awk '{print $1}')"

  if (( instruction_bytes > MAX_PROMPT_BYTES )); then
    classification=production-input-contract-exclusion
    exclusion_reason=prompt_bytes_exceed_service_limit
  elif (( symlink_count > 0 )); then
    classification=production-input-contract-exclusion
    exclusion_reason=source_contains_symbolic_links
  elif (( special_count > 0 )); then
    classification=production-input-contract-exclusion
    exclusion_reason=source_contains_special_files
  elif (( file_count > MAX_STAGED_FILES )); then
    classification=production-input-contract-exclusion
    exclusion_reason=source_file_count_exceeds_service_limit
  elif (( source_bytes > MAX_STAGED_BYTES )); then
    classification=production-input-contract-exclusion
    exclusion_reason=source_bytes_exceed_service_limit
  fi
  if (( task_index % 2 == 1 )); then
    policy_first=true
    policy_second=false
  fi

  archive_sha="$(sha256sum -- "${partial_dir}/environment-image.tar" | awk '{print $1}')"
  archive_bytes="$(stat -c '%s' -- "${partial_dir}/environment-image.tar")"
  require_equal "environment archive ownership/mode for ${task_id}" 1000:1000:600 \
    "$(stat -c '%u:%g:%a' -- "${partial_dir}/environment-image.tar")"

  # shellcheck disable=SC2016 # The dollar-prefixed names in the filter are jq variables.
  jq -n \
    --arg dataset_name "${DATASET_NAME}" \
    --arg dataset_digest "${DATASET_CONTENT_DIGEST}" \
    --arg dataset_lock_sha "${DATASET_LOCK_SHA256}" \
    --arg harbor_version "${HARBOR_VERSION}" \
    --arg harbor_commit "${HARBOR_COMMIT}" \
    --arg release_commit "${SERVICE_RELEASE_COMMIT}" \
    --arg implementation_commit "${SERVICE_IMPLEMENTATION_COMMIT}" \
    --arg release_lock_sha "${SERVICE_RELEASE_LOCK_SHA256}" \
    --arg stack_lock_sha "${STACK_LOCK_SHA256}" \
    --arg task_id "${task_id}" \
    --argjson task_index "${task_index}" \
    --arg language "${language}" \
    --arg log_parser "${log_parser}" \
    --arg classification "${classification}" \
    --arg exclusion_reason "${exclusion_reason}" \
    --arg registry_content_hash "${registry_content_hash}" \
    --arg network_policy_source "${network_policy_source}" \
    --arg base_ref "${base_ref}" \
    --arg base_id "${base_id}" \
    --arg base_digest "${base_digest}" \
    --arg env_tag "${env_tag}" \
    --arg env_id "${env_id}" \
    --arg workdir "${workdir}" \
    --arg config_user "${config_user}" \
    --arg archive_sha "${archive_sha}" \
    --argjson archive_bytes "${archive_bytes}" \
    --arg base_commit "${base_commit}" \
    --arg regular_sha "${regular_sha}" \
    --arg modes_sha "${modes_sha}" \
    --arg symlinks_sha "${symlinks_sha}" \
    --arg source_extractor_image "${SOURCE_EXTRACTOR_IMAGE_ID}" \
    --arg source_extractor_tar_version "${SOURCE_EXTRACTOR_TAR_VERSION}" \
    --arg initial_git_status_sha "${initial_git_status_sha}" \
    --argjson initial_git_status_bytes "${initial_git_status_bytes}" \
    --argjson initial_worktree_clean "${initial_worktree_clean}" \
    --argjson instruction_bytes "${instruction_bytes}" \
    --argjson file_count "${file_count}" \
    --argjson source_bytes "${source_bytes}" \
    --argjson symlink_count "${symlink_count}" \
    --argjson special_count "${special_count}" \
    --arg instruction_sha "$(sha256sum -- "${instruction}" | awk '{print $1}')" \
    --arg task_toml_sha "$(sha256sum -- "${task_toml}" | awk '{print $1}')" \
    --arg dockerfile_sha "$(sha256sum -- "${dockerfile}" | awk '{print $1}')" \
    --arg test_config_sha "$(sha256sum -- "${test_config}" | awk '{print $1}')" \
    --arg test_sh_sha "$(sha256sum -- "${test_sh}" | awk '{print $1}')" \
    --arg test_parser_sha "$(sha256sum -- "${test_parser}" | awk '{print $1}')" \
    --argjson policy_first "${policy_first}" \
    --argjson policy_second "${policy_second}" \
    '{
      schema_version:1,
      dataset:{name:$dataset_name,content_digest:$dataset_digest,lock_sha256:$dataset_lock_sha},
      harbor:{version:$harbor_version,commit:$harbor_commit},
      production_release:{release_commit:$release_commit,
        implementation_commit:$implementation_commit,
        release_lock_sha256:$release_lock_sha,stack_lock_sha256:$stack_lock_sha},
      task_id:$task_id,
      task_index:$task_index,
      language:$language,
      log_parser:$log_parser,
      classification:$classification,
      exclusion_reason:(if $exclusion_reason == "" then null else $exclusion_reason end),
      policy_order:[$policy_first,$policy_second],
      inputs:{
        harbor_content_hash:$registry_content_hash,
        instruction_sha256:$instruction_sha,instruction_bytes:$instruction_bytes,
        task_toml_sha256:$task_toml_sha,
        environment_dockerfile_sha256:$dockerfile_sha,
        test_config_sha256:$test_config_sha,
        test_sh_sha256:$test_sh_sha,
        test_parser_sha256:$test_parser_sha
      },
      environment:{
        base_ref:$base_ref,base_image_id:$base_id,base_repo_digest:$base_digest,
        image_tag:$env_tag,image_id:$env_id,working_dir:$workdir,
        configured_user:$config_user,archive_path:"environment-image.tar",
        archive_sha256:$archive_sha,archive_bytes:$archive_bytes,
        build_network:"default (required only for the dataset Dockerfile uv installer)",
        runtime_network_mode:"public",
        runtime_network_policy_source:$network_policy_source,
        installer_observation:"uv 0.7.13 installer reported: no checksums to verify",
        rerun_authority:"preserved environment-image.tar, not another mutable network build"
      },
      source:{
        relative_path:"source",base_commit:$base_commit,
        regular_file_count:$file_count,regular_file_bytes:$source_bytes,
        symlink_count:$symlink_count,special_file_count:$special_count,
        regular_manifest_sha256:$regular_sha,
        mode_manifest_sha256:$modes_sha,
        symlink_manifest_sha256:$symlinks_sha,
        materialization_method:"Docker archive stream plus pinned non-root GNU tar delayed-directory restoration",
        extractor_image_id:$source_extractor_image,
        extractor_tar_version:$source_extractor_tar_version,
        initial_git_status_sha256:$initial_git_status_sha,
        initial_git_status_bytes:$initial_git_status_bytes,
        initial_worktree_clean:$initial_worktree_clean
      }
    }' >"${partial_dir}/manifest.json.partial"
  jq -e --arg task "${task_id}" --arg classification "${classification}" \
    '.schema_version == 1 and .task_id == $task and .classification == $classification and
     (.policy_order == [false,true] or .policy_order == [true,false])' \
    "${partial_dir}/manifest.json.partial" >/dev/null ||
    die "generated task manifest failed its semantic assertion: ${task_id}"
  sync -f "${partial_dir}/manifest.json.partial"
  mv -- "${partial_dir}/manifest.json.partial" "${partial_dir}/manifest.json"
  sync -f "${partial_dir}"
  mv -- "${partial_dir}" "${final_dir}"
  sync -f "${MATERIALIZATION_ROOT}"
  validate_completed_task "${task_id}"
  printf 'MATERIALIZED task=%s index=%s classification=%s language=%s image=%s\n' \
    "${task_id}" "${task_index}" "${classification}" "${language}" "${env_id}"
}

task_index=0
while IFS= read -r -d '' task_id; do
  materialize_task "${task_id}" "${task_index}"
  task_index=$((task_index + 1))
done < <(find "${DATASET_ROOT}" -mindepth 1 -maxdepth 1 -type d -printf '%f\0' | LC_ALL=C sort -z)
require_equal 'materialized task iteration count' "${EXPECTED_TASKS}" "${task_index}"

readonly SUMMARY_PATH="${SUITE_ROOT}/materialization-summary.json"
readonly PLAN_PATH="${SUITE_ROOT}/suite-plan.json"
readonly SUMMARY_PARTIAL="${SUMMARY_PATH}.partial"
readonly PLAN_PARTIAL="${PLAN_PATH}.partial"
# shellcheck disable=SC2016 # The dollar-prefixed names in the filter are jq variables.
find "${MATERIALIZATION_ROOT}" -mindepth 2 -maxdepth 2 -name manifest.json -type f -print0 |
  LC_ALL=C sort -z | xargs -0 jq -s \
  --arg dataset_name "${DATASET_NAME}" \
  --arg dataset_digest "${DATASET_CONTENT_DIGEST}" \
  --arg dataset_lock_sha256 "${DATASET_LOCK_SHA256}" \
  --arg dataset_regular_manifest_sha256 "$(sha256sum -- "${DATASET_REGULAR_MANIFEST}" | awk '{print $1}')" \
  --arg dataset_mode_manifest_sha256 "$(sha256sum -- "${DATASET_MODE_MANIFEST}" | awk '{print $1}')" \
  --arg release_commit "${SERVICE_RELEASE_COMMIT}" \
  --arg implementation_commit "${SERVICE_IMPLEMENTATION_COMMIT}" \
  --arg release_lock_sha256 "${SERVICE_RELEASE_LOCK_SHA256}" \
  --arg stack_lock_sha256 "${STACK_LOCK_SHA256}" \
  '{
    schema_version:1,
    dataset:{name:$dataset_name,content_digest:$dataset_digest,
      lock_sha256:$dataset_lock_sha256,
      regular_manifest_sha256:$dataset_regular_manifest_sha256,
      mode_manifest_sha256:$dataset_mode_manifest_sha256},
    production_release:{release_commit:$release_commit,
      implementation_commit:$implementation_commit,
      release_lock_sha256:$release_lock_sha256,
      stack_lock_sha256:$stack_lock_sha256},
    task_count:length,
    eligible_count:(map(select(.classification == "eligible"))|length),
    exclusion_count:(map(select(.classification == "production-input-contract-exclusion"))|length),
    languages:(group_by(.language)|map({language:.[0].language,count:length})),
    tasks:sort_by(.task_id)
  }' >"${SUMMARY_PARTIAL}"
jq -e --argjson expected "${EXPECTED_TASKS}" \
  '.schema_version == 1 and .task_count == $expected and
   (.eligible_count + .exclusion_count == .task_count)' \
  "${SUMMARY_PARTIAL}" >/dev/null || die 'materialization summary failed its semantic assertion'
jq '{
  schema_version:1,
  methodology:"one paired full-suite pass through production agent_service; policy order alternates by sorted dataset index",
  eligible_count:.eligible_count,
  excluded:[.tasks[]|select(.classification != "eligible")|{task_id,reason:.exclusion_reason}],
  runs:[.tasks[]|select(.classification == "eligible")|{task_id,task_index,language,policy_order}]
}' "${SUMMARY_PARTIAL}" >"${PLAN_PARTIAL}"
sync -f "${SUMMARY_PARTIAL}"
sync -f "${PLAN_PARTIAL}"
if [[ -e "${SUMMARY_PATH}" || -e "${PLAN_PATH}" ]]; then
  [[ -f "${SUMMARY_PATH}" && -f "${PLAN_PATH}" ]] || die 'existing suite summary/plan pair is incomplete'
  cmp -- "${SUMMARY_PATH}" "${SUMMARY_PARTIAL}" || die 'existing materialization summary differs from regenerated summary'
  cmp -- "${PLAN_PATH}" "${PLAN_PARTIAL}" || die 'existing suite plan differs from regenerated plan'
  rm -f -- "${SUMMARY_PARTIAL}" "${PLAN_PARTIAL}"
else
  mv -- "${SUMMARY_PARTIAL}" "${SUMMARY_PATH}"
  mv -- "${PLAN_PARTIAL}" "${PLAN_PATH}"
  sync -f "${SUITE_ROOT}"
fi

printf 'FULL_SUITE_MATERIALIZATION_COMPLETE tasks=%s eligible=%s excluded=%s plan=%s\n' \
  "${EXPECTED_TASKS}" "$(jq -er '.eligible_count' "${SUMMARY_PATH}")" \
  "$(jq -er '.exclusion_count' "${SUMMARY_PATH}")" "${PLAN_PATH}"
