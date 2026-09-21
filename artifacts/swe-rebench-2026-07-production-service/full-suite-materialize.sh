#!/usr/bin/env bash
# Materializes the SWE-rebench 2026-07 full suite that full-suite-run.sh runs and
# warm-task-env.sh prepares, from the benchmark's own two locks.
#
# A materialization is identified by the benchmark, never by a service release:
# the dataset pinned in full-suite-dataset.lock.json and, for each task, the
# environment image pinned in full-suite-images.lock.json. That image is both the
# tree the agent's workspace is copied from and the evaluator that grades the
# result. It cannot be rebuilt into the same image, so a host that lacks it loads
# its pinned archive, and nothing here builds one.
#
# Every task, in task-id order, has its dataset package and declarations
# verified, its pinned image ensured, and the image's working tree copied out
# through a sandboxed extractor and proved equal to the image in content, entry
# type, mode, link target and Git status. The outcome is tasks/<task_id>/. A task
# directory that already exists is proved again and must derive a byte-identical
# manifest.
#
# plan.json then lists the tasks whose inputs fit the service's input limits --
# the prompt measured as full-suite-run.sh submits it, the committed preamble
# followed by the task statement, in NFC as the service measures it -- binds
# each task to its manifest by SHA-256, and records the lock hashes, the
# preamble's hash and size, this script's Git blob and the limits it was
# derived against.
set -Eeuo pipefail
shopt -s inherit_errexit
umask 077

if (( $# != 0 )); then
  printf 'ERROR: full-suite materialization accepts no arguments.\n' >&2
  exit 2
fi

die() {
  printf 'ERROR: %s\n' "$*" >&2
  exit 1
}

require_equal() {
  local label="$1" expected="$2" actual="$3"
  [[ "${actual}" == "${expected}" ]] ||
    die "${label} mismatch: expected ${expected}, got ${actual}"
}

sha256_of() {
  local digest
  digest="$(sha256sum -- "$1")" || die "cannot hash $1"
  printf '%s' "${digest%% *}"
}

require_sha256() {
  local path="$1" expected="$2"
  [[ -f "${path}" && ! -L "${path}" ]] || die "required regular file is absent or a symlink: ${path}"
  require_equal "SHA-256 of ${path}" "${expected}" "$(sha256_of "${path}")"
}

ACTIVE_CONTAINER=
SCRATCH=
cleanup() {
  local rc=$?
  set +e
  if [[ -n "${ACTIVE_CONTAINER}" ]]; then
    docker rm -f "${ACTIVE_CONTAINER}" >/dev/null 2>&1
  fi
  if [[ -n "${SCRATCH}" ]]; then
    rm -rf -- "${SCRATCH}"
  fi
  exit "${rc}"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

for command in awk chmod cmp comm cut date dirname docker find flock git grep id jq \
  mkdir mktemp mv python3 readlink realpath rm sha256sum sort stat sync wc xargs; do
  command -v "${command}" >/dev/null 2>&1 ||
    die "required host command is unavailable: ${command}"
done

# A prompt's byte limit stands in for its tokens, and bytes bound tokens only in
# NFC, the form the served tokenizer counts; the service therefore refuses a
# prompt not in NFC and measures one that is. This measures the same way, with
# Python's unicodedata, which agrees with the service's unicode-normalization
# 0.1.22 only at the Unicode version both carry.
readonly PROMPT_UNICODE_VERSION=15.0.0
require_equal "Unicode version of python3's unicodedata, which measures prompts as the service does" \
  "${PROMPT_UNICODE_VERSION}" "$(python3 -c 'import unicodedata; print(unicodedata.unidata_version)')"

MATERIALIZER="$(readlink -e -- "${BASH_SOURCE[0]}")"
BENCH_ROOT="$(dirname -- "${MATERIALIZER}")"
SERVICE_ROOT="$(git -C "${BENCH_ROOT}" rev-parse --show-toplevel)"
MATERIALIZER_RELATIVE="$(realpath --relative-to="${SERVICE_ROOT}" -- "${MATERIALIZER}")"
BENCH_RELATIVE="$(dirname -- "${MATERIALIZER_RELATIVE}")"
HOST_UID="$(id -u)"
HOST_GID="$(id -g)"
readonly MATERIALIZER BENCH_ROOT SERVICE_ROOT MATERIALIZER_RELATIVE BENCH_RELATIVE HOST_UID HOST_GID
readonly DATASET_ROOT="${BENCH_ROOT}/evaluator-dataset"
readonly DATASET_LOCK="${BENCH_ROOT}/full-suite-dataset.lock.json"
readonly IMAGES_LOCK="${BENCH_ROOT}/full-suite-images.lock.json"
readonly STACK_LOCK="${SERVICE_ROOT}/config/stack.lock.json"
readonly PREAMBLE="${BENCH_ROOT}/prompt-preamble.md"
readonly MATERIALIZATION_ROOT="${BENCH_ROOT}/full-suite-materialization"
readonly TASKS_ROOT="${MATERIALIZATION_ROOT}/tasks"
readonly PLAN="${MATERIALIZATION_ROOT}/plan.json"
readonly CONTAINER_PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
readonly TASK_DIRECTORY_ENTRIES=$'d source\nf initial-git-status.z\nf manifest.json\nf source-modes.z\nf source-regular.sha256z'
# Every task's environment/Dockerfile adds exactly these two layers to its base.
readonly UV_VERSION=0.7.13
readonly UV_INSTALL_LINE="RUN curl -LsSf https://astral.sh/uv/${UV_VERSION}/install.sh | env UV_INSTALL_DIR=/usr/local/bin sh"
readonly LOGS_LINE='RUN mkdir -p /logs'

# The plan names this script by Git blob and the locks and the prompt preamble by
# SHA-256, and the service input limits come from the stack lock, so all five
# must be committed bytes.
for input in "${MATERIALIZER_RELATIVE}" "${BENCH_RELATIVE}/full-suite-dataset.lock.json" \
  "${BENCH_RELATIVE}/full-suite-images.lock.json" "${BENCH_RELATIVE}/prompt-preamble.md" \
  config/stack.lock.json; do
  git -C "${SERVICE_ROOT}" ls-files --error-unmatch -- "${input}" >/dev/null 2>&1 ||
    die "materialization input is not tracked in Git: ${input}"
  cmp -s -- "${SERVICE_ROOT}/${input}" <(git -C "${SERVICE_ROOT}" show "HEAD:${input}") ||
    die "materialization input differs from HEAD: ${input}"
done
MATERIALIZER_GIT_BLOB="$(git -C "${SERVICE_ROOT}" rev-parse "HEAD:${MATERIALIZER_RELATIVE}")"
DATASET_LOCK_SHA256="$(sha256_of "${DATASET_LOCK}")"
IMAGES_LOCK_SHA256="$(sha256_of "${IMAGES_LOCK}")"
PREAMBLE_SHA256="$(sha256_of "${PREAMBLE}")"
PREAMBLE_BYTES="$(stat -c '%s' -- "${PREAMBLE}")"
readonly MATERIALIZER_GIT_BLOB DATASET_LOCK_SHA256 IMAGES_LOCK_SHA256 PREAMBLE_SHA256 PREAMBLE_BYTES

jq -e '
  def simple_name: type == "string" and test("^[A-Za-z0-9][A-Za-z0-9._-]*$");
  def sha256_hex: type == "string" and test("^[0-9a-f]{64}$");
  .schema_version == 1 and
  (.dataset.name | type == "string" and
    test("^[A-Za-z0-9][A-Za-z0-9._-]*/[A-Za-z0-9][A-Za-z0-9._-]*$")) and
  (.dataset.tasks | type == "array" and length > 0) and
  .dataset.task_count == (.dataset.tasks | length) and
  all(.dataset.tasks[]; (.task_id | simple_name) and (.content_hash | sha256_hex)) and
  ([.dataset.tasks[].task_id] | . == unique) and
  (.dataset.files | type == "array") and
  all(.dataset.files[]; (.path | simple_name) and (.content_hash | sha256_hex) and
    (.size_bytes | type == "number" and . >= 0 and . == floor)) and
  ([.dataset.files[].path] | . == unique)' "${DATASET_LOCK}" >/dev/null ||
  die 'full-suite-dataset.lock.json is malformed'

jq -e --arg dataset_lock_sha256 "${DATASET_LOCK_SHA256}" --slurpfile dataset "${DATASET_LOCK}" '
  def sha256_hex: type == "string" and test("^[0-9a-f]{64}$");
  keys == ["dataset_lock_sha256", "environments", "integrity_contract", "schema_version",
    "source_extractor"] and
  .schema_version == 1 and
  .dataset_lock_sha256 == $dataset_lock_sha256 and
  (.integrity_contract | type == "string" and length > 0) and
  (.source_extractor | keys == ["image"]) and
  (.source_extractor.image | type == "string" and
    test("^[a-z0-9]+([._-][a-z0-9]+)*(/[a-z0-9]+([._-][a-z0-9]+)*)*@sha256:[0-9a-f]{64}$")) and
  [.environments[].task_id] == [$dataset[0].dataset.tasks[].task_id] and
  all(.environments[];
    keys == ["archive", "image_id", "image_tag", "task_id"] and
    (.image_tag | type == "string" and
      test("^[a-z0-9]+([._-][a-z0-9]+)*:[A-Za-z0-9_][A-Za-z0-9_.-]{0,127}$")) and
    (.image_tag | sub("^[^:]*:"; "")) == (.task_id | ascii_downcase) and
    (.image_id | type == "string" and test("^sha256:[0-9a-f]{64}$")) and
    (.archive | keys == ["bytes", "path", "sha256"]) and
    (.archive.path | type == "string" and
      test("^[A-Za-z0-9_][A-Za-z0-9_.-]*(/[A-Za-z0-9_][A-Za-z0-9_.-]*)*$")) and
    (.archive.sha256 | sha256_hex) and
    (.archive.bytes | type == "number" and . > 0 and . == floor)) and
  ([.environments[].image_tag] | length == (unique | length)) and
  ([.environments[].image_id] | length == (unique | length))' "${IMAGES_LOCK}" >/dev/null ||
  die 'full-suite-images.lock.json is malformed or does not pin the locked dataset'

SERVICE_LIMITS="$(jq -ce '
  .limits | {max_prompt_bytes, max_staged_bytes, max_staged_files, max_staged_entries} |
  if all(.[]; type == "number" and . > 0 and . == floor) then . else error("not positive integers") end' \
  "${STACK_LOCK}")" ||
  die 'config/stack.lock.json does not carry positive integer service input limits'
DATASET_NAME="$(jq -er '.dataset.name' "${DATASET_LOCK}")"
SOURCE_EXTRACTOR_IMAGE="$(jq -er '.source_extractor.image' "${IMAGES_LOCK}")"
TASK_COUNT="$(jq -er '.dataset.tasks | length' "${DATASET_LOCK}")"
mapfile -t TASK_IDS < <(jq -r '.dataset.tasks[].task_id' "${DATASET_LOCK}")
require_equal 'locked task count' "${TASK_COUNT}" "${#TASK_IDS[@]}"
readonly SERVICE_LIMITS DATASET_NAME SOURCE_EXTRACTOR_IMAGE TASK_COUNT TASK_IDS

# Harbor 0.21.0's Packager.compute_content_hash, which the dataset lock's hashes
# come from: SHA-256 over `relative_path NUL file_sha256 LF` for task.toml,
# instruction.md, README.md and every file under environment/, tests/, solution/
# and steps/, sorted by path. Harbor's default-ignored paths must be absent,
# because ignore rules are not evaluated here.
task_content_hash() {
  local task_root="$1" path transient
  [[ ! -e "${task_root}/.gitignore" ]] ||
    die "task-level .gitignore would need Harbor's ignore evaluation: ${task_root}"
  transient="$(find "${task_root}" \( -path '*/__pycache__/*' -o -name '*.pyc' -o \
    -name '.DS_Store' -o -name '*.swp' -o -name '*.swo' -o -name '*~' \) -print -quit)"
  [[ -z "${transient}" ]] || die "task holds a path Harbor ignores by default: ${transient}"
  {
    for path in task.toml instruction.md README.md; do
      [[ ! -f "${task_root}/${path}" ]] || printf '%s\0' "${task_root}/${path}"
    done
    for path in environment tests solution steps; do
      [[ ! -d "${task_root}/${path}" ]] || find "${task_root}/${path}" -type f -print0
    done
  } | LC_ALL=C sort -z |
    while IFS= read -r -d '' path; do
      printf '%s\0%s\n' "${path#"${task_root}"/}" "$(sha256_of "${path}")"
    done | sha256sum | cut -d' ' -f1
}

exact_line_count() {
  awk -v expected="$2" '$0 == expected {count++} END {print count + 0}' "$1"
}

verify_task_package() {
  local task_id="$1"
  local root="${DATASET_ROOT}/${task_id}" path network image_name
  require_equal "top-level layout of task ${task_id}" \
    $'d environment\nd solution\nd tests\nf instruction.md\nf task.toml' \
    "$(find "${root}" -mindepth 1 -maxdepth 1 -printf '%y %f\n' | LC_ALL=C sort)"
  for path in environment/Dockerfile tests/config.json tests/test.sh tests/swan_log_parsers.py; do
    [[ -f "${root}/${path}" ]] || die "task input is absent: ${task_id}/${path}"
  done
  require_equal "Harbor content hash of task ${task_id}" \
    "$(jq -er --arg task "${task_id}" '.dataset.tasks[] | select(.task_id == $task) | .content_hash' \
      "${DATASET_LOCK}")" \
    "$(task_content_hash "${root}")"
  jq -e --arg task "${task_id}" \
    '.instance_id == $task and (.language | type == "string" and length > 0) and
     (.base_commit | type == "string" and test("^[0-9a-f]{40}$")) and
     (.image_name | type == "string")' \
    "${root}/tests/config.json" >/dev/null || die "tests/config.json does not describe task ${task_id}"

  # Every task must declare the suite's one set of conditions. The driver grades
  # each under a 3000 s verifier timeout, one CPU and 4096 MiB with network, and
  # the agent timeout and storage declarations are uniform as well. Harbor 0.21.0
  # reads the legacy `allow_internet = true` as public network.
  require_equal "task name declarations in ${task_id}" 1 \
    "$(exact_line_count "${root}/task.toml" "name = \"${DATASET_NAME%%/*}/${task_id}\"")"
  require_equal "timeout declarations in ${task_id}" 2 \
    "$(exact_line_count "${root}/task.toml" 'timeout_sec = 3000.0')"
  require_equal "CPU declarations in ${task_id}" 1 \
    "$(exact_line_count "${root}/task.toml" 'cpus = 1')"
  require_equal "memory declarations in ${task_id}" 1 \
    "$(exact_line_count "${root}/task.toml" 'memory_mb = 4096')"
  require_equal "storage declarations in ${task_id}" 1 \
    "$(exact_line_count "${root}/task.toml" 'storage_mb = 10240')"
  network="$(awk '/^(network_mode|allow_internet) = /' "${root}/task.toml")"
  [[ "${network}" == 'network_mode = "public"' || "${network}" == 'allow_internet = true' ]] ||
    die "task ${task_id} does not declare exactly one public network setting: ${network}"

  # The environment image is this recipe built; verify_environment_recipe checks
  # what its two RUN lines add.
  image_name="$(jq -er '.image_name' "${root}/tests/config.json")"
  require_equal "environment/Dockerfile of task ${task_id}" \
    "FROM ${image_name}"$'\n'"${UV_INSTALL_LINE}"$'\n'"${LOGS_LINE}" \
    "$(grep -v '^$' "${root}/environment/Dockerfile")"
}

# A host that does not hold the pinned image loads it from its pinned archive;
# either way the tag must name exactly the pinned image.
ensure_environment_image() {
  local task_id="$1"
  local entry image_tag image_id archive_path archive_sha256 archive_bytes archive observed
  entry="$(jq -er --arg task "${task_id}" '.environments[] | select(.task_id == $task) |
    [.image_tag, .image_id, .archive.path, .archive.sha256, .archive.bytes] | @tsv' "${IMAGES_LOCK}")"
  IFS=$'\t' read -r image_tag image_id archive_path archive_sha256 archive_bytes <<<"${entry}"
  if ! observed="$(docker image inspect --format '{{.Id}}' "${image_tag}" 2>/dev/null)"; then
    archive="${BENCH_ROOT}/${archive_path}"
    printf 'Loading the pinned environment archive for %s.\n' "${task_id}" >&2
    [[ -f "${archive}" && ! -L "${archive}" ]] ||
      die "environment image ${image_tag} is absent and so is its pinned archive: ${archive}"
    require_equal "size of ${archive}" "${archive_bytes}" "$(stat -c '%s' -- "${archive}")"
    require_sha256 "${archive}" "${archive_sha256}"
    docker load --input "${archive}" >/dev/null
    observed="$(docker image inspect --format '{{.Id}}' "${image_tag}")"
  fi
  require_equal "image named by ${image_tag}" "${image_id}" "${observed}"
}

# What each environment/Dockerfile adds to its base, and grading relies on: uv
# at the pinned version, and /logs.
verify_environment_recipe() {
  local env_id="$1"
  docker run --rm --network none --cap-drop ALL --security-opt no-new-privileges \
    --read-only --memory 1g --pids-limit 128 \
    --tmpfs /tmp:rw,nosuid,nodev,noexec,size=16m,mode=1777 \
    --env EXPECTED_UV_VERSION="uv ${UV_VERSION}" --env HOME=/tmp/no-home \
    --env LC_ALL=C --env PATH="${CONTAINER_PATH}" \
    --user 0:0 --entrypoint bash "${env_id}" -Eeuo pipefail -c \
    'test "$(uv --version)" = "$EXPECTED_UV_VERSION"
     test -d /logs' ||
    die "environment image ${env_id} lacks what its environment/Dockerfile adds"
}

# The image's own tools describe its working tree, as root in a sandbox with no
# network, no capabilities and a read-only root filesystem.
image_regular_manifest() {
  local env_id="$1"
  docker run --rm --network none --cap-drop ALL --security-opt no-new-privileges \
    --read-only --memory 1g --pids-limit 128 \
    --tmpfs /tmp:rw,nosuid,nodev,noexec,size=256m,mode=1777 \
    --env LC_ALL=C --env PATH="${CONTAINER_PATH}" \
    --user 0:0 --entrypoint bash "${env_id}" -Eeuo pipefail -c \
    'find . -type f -print0 | sort -z | xargs -0 -r sha256sum --zero --'
}

image_mode_manifest() {
  local env_id="$1"
  docker run --rm --network none --cap-drop ALL --security-opt no-new-privileges \
    --read-only --memory 1g --pids-limit 128 \
    --tmpfs /tmp:rw,nosuid,nodev,noexec,size=256m,mode=1777 \
    --env LC_ALL=C --env PATH="${CONTAINER_PATH}" \
    --user 0:0 --entrypoint bash "${env_id}" -Eeuo pipefail -c \
    'find . -mindepth 1 -printf "%y %m %P -> %l\0" | sort -z'
}

image_git_status() {
  local env_id="$1" base_commit="$2"
  docker run --rm --network none --cap-drop ALL --security-opt no-new-privileges \
    --read-only --memory 1g --pids-limit 128 \
    --tmpfs /tmp:rw,nosuid,nodev,noexec,size=16m,mode=1777 \
    --env EXPECTED_BASE_COMMIT="${base_commit}" \
    --env GIT_CONFIG_NOSYSTEM=1 --env GIT_OPTIONAL_LOCKS=0 --env HOME=/tmp/no-home \
    --env LC_ALL=C --env PATH="${CONTAINER_PATH}" \
    --user 0:0 --entrypoint bash "${env_id}" -Eeuo pipefail -c \
    'git_bin="$(command -v git)"
     git() { "$git_bin" -c core.fsmonitor=false -c core.hooksPath=/dev/null "$@"; }
     test -d .git
     test ! -L .git
     test "$PWD" = "$(pwd -P)"
     test "$PWD" = "$(git rev-parse --show-toplevel)"
     test "$(git rev-parse HEAD)" = "$EXPECTED_BASE_COMMIT"
     git status --porcelain=v1 -z --untracked-files=all --ignore-submodules=none'
}

write_image_evidence() {
  local env_id="$1" base_commit="$2" directory="$3"
  image_regular_manifest "${env_id}" >"${directory}/source-regular.sha256z"
  image_mode_manifest "${env_id}" >"${directory}/source-modes.z"
  image_git_status "${env_id}" "${base_commit}" >"${directory}/initial-git-status.z"
}

# The host's tools describe the copy, printing the same records as the image's.
host_regular_manifest() {
  local root="$1"
  (cd -- "${root}" && find . -type f -print0 | LC_ALL=C sort -z | xargs -0 -r sha256sum --zero --)
}

host_mode_manifest() {
  local root="$1"
  (cd -- "${root}" && find . -mindepth 1 -printf '%y %m %P -> %l\0' | LC_ALL=C sort -z)
}

# The image's own git reads the copy, mounted read-only, as the invoking user.
copy_git_status() {
  local env_id="$1" base_commit="$2" source_root="$3"
  docker run --rm --network none --cap-drop ALL --security-opt no-new-privileges \
    --read-only --memory 1g --pids-limit 128 \
    --tmpfs /tmp:rw,nosuid,nodev,noexec,size=16m,mode=1777 \
    --env EXPECTED_BASE_COMMIT="${base_commit}" \
    --env GIT_CONFIG_NOSYSTEM=1 --env GIT_OPTIONAL_LOCKS=0 --env HOME=/tmp/no-home \
    --env LC_ALL=C --env PATH="${CONTAINER_PATH}" \
    --mount "type=bind,src=${source_root},dst=/source,readonly" \
    --workdir /source --user "${HOST_UID}:${HOST_GID}" --entrypoint bash "${env_id}" -Eeuo pipefail -c \
    'git_bin="$(command -v git)"
     git() { "$git_bin" -c core.fsmonitor=false -c core.hooksPath=/dev/null -c safe.directory=/source "$@"; }
     test -d .git
     test ! -L .git
     test "$(git rev-parse HEAD)" = "$EXPECTED_BASE_COMMIT"
     git status --porcelain=v1 -z --untracked-files=all --ignore-submodules=none'
}

ensure_source_extractor() {
  docker image inspect "${SOURCE_EXTRACTOR_IMAGE}" >/dev/null 2>&1 ||
    docker pull "${SOURCE_EXTRACTOR_IMAGE}" >/dev/null
}

# The daemon streams the image's working tree as a tar archive, and GNU tar in the
# pinned extractor restores it as the invoking user: without chown, with each
# entry's permission bits, and with directory permissions applied last so trees
# holding read-only directories restore completely. The extractor can write only
# the destination, and what it writes is proved against the image before anything
# records it.
extract_working_tree() {
  local task_id="$1" env_id="$2" working_dir="$3" destination="$4" container
  container="qwen38-swe-materialize-$(printf '%s' "${task_id}" | sha256sum | cut -c1-20)"
  [[ -z "$(docker ps --all --quiet --filter "name=^/${container}$")" ]] ||
    die "materialization container name is already in use: ${container}"
  ensure_source_extractor
  ACTIVE_CONTAINER="${container}"
  docker create --name "${container}" --network none --entrypoint true "${env_id}" >/dev/null
  docker cp "${container}:${working_dir}/." - |
    docker run --rm --interactive --network none --cap-drop ALL --security-opt no-new-privileges \
      --read-only --memory 1g --pids-limit 128 --user "${HOST_UID}:${HOST_GID}" \
      --mount "type=bind,src=${destination},dst=/output" \
      --entrypoint tar "${SOURCE_EXTRACTOR_IMAGE}" \
      --extract --file=- --directory=/output --no-same-owner --same-permissions \
      --delay-directory-restore
  docker rm "${container}" >/dev/null
  ACTIVE_CONTAINER=
}

# Proves the copied working tree in a task directory against the evidence files
# beside it, which already hold its environment image's own description, and
# prints the task's manifest. environment.archive_path is relative to the
# directory holding this script.
derive_task_manifest() {
  local task_id="$1" task_dir="$2" env_id="$3" base_commit="$4" working_dir="$5"
  local source_root="${task_dir}/source" task_root="${DATASET_ROOT}/${task_id}"
  local not_owned environment task_content_hash language
  local instruction_sha256 instruction_bytes test_sh_sha256 test_config_sha256 test_parser_sha256
  local regular_manifest_sha256 mode_manifest_sha256 initial_git_status_sha256
  local directory_count regular_file_count regular_file_bytes symlink_count special_file_count
  [[ -d "${source_root}/.git" && ! -L "${source_root}/.git" ]] ||
    die "copied working tree of ${task_id} has no real .git directory"
  host_regular_manifest "${source_root}" >"${SCRATCH}/copy-regular.sha256z"
  cmp -s -- "${task_dir}/source-regular.sha256z" "${SCRATCH}/copy-regular.sha256z" ||
    die "regular files copied for ${task_id} differ from its environment image"
  host_mode_manifest "${source_root}" >"${SCRATCH}/copy-modes.z"
  cmp -s -- "${task_dir}/source-modes.z" "${SCRATCH}/copy-modes.z" ||
    die "entry types, modes or link targets copied for ${task_id} differ from its environment image"
  copy_git_status "${env_id}" "${base_commit}" "${source_root}" >"${SCRATCH}/copy-git-status.z"
  cmp -s -- "${task_dir}/initial-git-status.z" "${SCRATCH}/copy-git-status.z" ||
    die "Git status of the tree copied for ${task_id} differs from its environment image"
  not_owned="$(find "${source_root}" \! -user "${HOST_UID}" -print -quit)"
  [[ -z "${not_owned}" ]] ||
    die "tree copied for ${task_id} holds an entry not owned by uid ${HOST_UID}: ${not_owned}"

  environment="$(jq -ec --arg task "${task_id}" --arg working_dir "${working_dir}" \
    '.environments[] | select(.task_id == $task) |
     {image_tag, image_id, working_dir: $working_dir, archive_path: .archive.path,
      archive_sha256: .archive.sha256, archive_bytes: .archive.bytes}' "${IMAGES_LOCK}")"
  task_content_hash="$(jq -er --arg task "${task_id}" \
    '.dataset.tasks[] | select(.task_id == $task) | .content_hash' "${DATASET_LOCK}")"
  language="$(jq -er '.language' "${task_root}/tests/config.json")"
  instruction_sha256="$(sha256_of "${task_root}/instruction.md")"
  instruction_bytes="$(stat -c '%s' -- "${task_root}/instruction.md")"
  test_sh_sha256="$(sha256_of "${task_root}/tests/test.sh")"
  test_config_sha256="$(sha256_of "${task_root}/tests/config.json")"
  test_parser_sha256="$(sha256_of "${task_root}/tests/swan_log_parsers.py")"
  regular_manifest_sha256="$(sha256_of "${task_dir}/source-regular.sha256z")"
  mode_manifest_sha256="$(sha256_of "${task_dir}/source-modes.z")"
  initial_git_status_sha256="$(sha256_of "${task_dir}/initial-git-status.z")"
  directory_count="$(find "${source_root}" -mindepth 1 -type d -printf '.' | wc -c)"
  regular_file_count="$(find "${source_root}" -type f -printf '.' | wc -c)"
  regular_file_bytes="$(find "${source_root}" -type f -printf '%s\n' | jq -s 'add // 0')"
  symlink_count="$(find "${source_root}" -type l -printf '.' | wc -c)"
  special_file_count="$(find "${source_root}" \! -type d \! -type f \! -type l -printf '.' | wc -c)"

  jq -n \
    --arg task_id "${task_id}" \
    --arg task_content_hash "${task_content_hash}" \
    --arg language "${language}" \
    --arg instruction_sha256 "${instruction_sha256}" \
    --argjson instruction_bytes "${instruction_bytes}" \
    --arg test_sh_sha256 "${test_sh_sha256}" \
    --arg test_config_sha256 "${test_config_sha256}" \
    --arg test_parser_sha256 "${test_parser_sha256}" \
    --argjson environment "${environment}" \
    --arg base_commit "${base_commit}" \
    --arg regular_manifest_sha256 "${regular_manifest_sha256}" \
    --arg mode_manifest_sha256 "${mode_manifest_sha256}" \
    --arg initial_git_status_sha256 "${initial_git_status_sha256}" \
    --argjson directory_count "${directory_count}" \
    --argjson regular_file_count "${regular_file_count}" \
    --argjson regular_file_bytes "${regular_file_bytes}" \
    --argjson symlink_count "${symlink_count}" \
    --argjson special_file_count "${special_file_count}" \
    '{schema_version: 2,
      task_id: $task_id,
      task_content_hash: $task_content_hash,
      language: $language,
      inputs: {
        instruction_sha256: $instruction_sha256,
        instruction_bytes: $instruction_bytes,
        test_sh_sha256: $test_sh_sha256,
        test_config_sha256: $test_config_sha256,
        test_parser_sha256: $test_parser_sha256},
      environment: $environment,
      source: {
        base_commit: $base_commit,
        regular_manifest_sha256: $regular_manifest_sha256,
        mode_manifest_sha256: $mode_manifest_sha256,
        initial_git_status_sha256: $initial_git_status_sha256,
        directory_count: $directory_count,
        regular_file_count: $regular_file_count,
        regular_file_bytes: $regular_file_bytes,
        symlink_count: $symlink_count,
        special_file_count: $special_file_count}}'
}

materialize_task() {
  local task_id="$1"
  local task_dir="${TASKS_ROOT}/${task_id}" partial="${TASKS_ROOT}/${task_id}.partial"
  local env_id base_commit working_dir evidence
  ensure_environment_image "${task_id}"
  env_id="$(jq -er --arg task "${task_id}" '.environments[] | select(.task_id == $task) | .image_id' \
    "${IMAGES_LOCK}")"
  base_commit="$(jq -er '.base_commit' "${DATASET_ROOT}/${task_id}/tests/config.json")"
  working_dir="$(docker image inspect --format '{{.Config.WorkingDir}}' "${env_id}")"
  [[ "${working_dir}" == /* && "${working_dir}" != / && "${working_dir}" != *$'\n'* ]] ||
    die "environment image of ${task_id} has no usable working directory: ${working_dir}"
  verify_environment_recipe "${env_id}"

  if [[ -e "${task_dir}" ]]; then
    [[ -d "${task_dir}" && ! -L "${task_dir}" ]] || die "task entry is not a real directory: ${task_dir}"
    require_equal "entries of ${task_dir}" "${TASK_DIRECTORY_ENTRIES}" \
      "$(find "${task_dir}" -mindepth 1 -maxdepth 1 -printf '%y %f\n' | LC_ALL=C sort)"
    write_image_evidence "${env_id}" "${base_commit}" "${SCRATCH}"
    for evidence in source-regular.sha256z source-modes.z initial-git-status.z; do
      cmp -s -- "${task_dir}/${evidence}" "${SCRATCH}/${evidence}" ||
        die "${evidence} of ${task_id} no longer describes its environment image"
    done
    derive_task_manifest "${task_id}" "${task_dir}" "${env_id}" "${base_commit}" "${working_dir}" \
      >"${SCRATCH}/manifest.json"
    cmp -s -- "${task_dir}/manifest.json" "${SCRATCH}/manifest.json" ||
      die "manifest of ${task_id} differs from the one derived now; move ${task_dir} aside to materialize the task again"
    printf 'VERIFIED task=%s image=%s\n' "${task_id}" "${env_id}"
    return
  fi

  mkdir -- "${partial}" "${partial}/source"
  write_image_evidence "${env_id}" "${base_commit}" "${partial}"
  extract_working_tree "${task_id}" "${env_id}" "${working_dir}" "${partial}/source"
  chmod 0700 -- "${partial}/source"
  derive_task_manifest "${task_id}" "${partial}" "${env_id}" "${base_commit}" "${working_dir}" \
    >"${partial}/manifest.json.partial"
  mv -- "${partial}/manifest.json.partial" "${partial}/manifest.json"
  sync -f -- "${partial}"
  mv -- "${partial}" "${task_dir}"
  sync -f -- "${TASKS_ROOT}"
  printf 'MATERIALIZED task=%s image=%s\n' "${task_id}" "${env_id}"
}

[[ -d "${DATASET_ROOT}" && ! -L "${DATASET_ROOT}" ]] ||
  die "dataset root is absent or a symlink: ${DATASET_ROOT}"
dataset_irregular="$(find "${DATASET_ROOT}" \! -type d \! -type f -print -quit)"
[[ -z "${dataset_irregular}" ]] ||
  die "dataset holds a symbolic link or special file: ${dataset_irregular}"
require_equal 'dataset task directories' "$(printf '%s\n' "${TASK_IDS[@]}")" \
  "$(find "${DATASET_ROOT}" -mindepth 1 -maxdepth 1 -type d -printf '%f\n' | LC_ALL=C sort)"
require_equal 'dataset top-level files' "$(jq -r '.dataset.files[].path' "${DATASET_LOCK}")" \
  "$(find "${DATASET_ROOT}" -mindepth 1 -maxdepth 1 -type f -printf '%f\n' | LC_ALL=C sort)"
while IFS=$'\t' read -r dataset_file content_hash size_bytes; do
  require_sha256 "${DATASET_ROOT}/${dataset_file}" "${content_hash}"
  require_equal "size of dataset file ${dataset_file}" "${size_bytes}" \
    "$(stat -c '%s' -- "${DATASET_ROOT}/${dataset_file}")"
done < <(jq -r '.dataset.files[] | [.path, .content_hash, .size_bytes] | @tsv' "${DATASET_LOCK}")
for task_id in "${TASK_IDS[@]}"; do
  verify_task_package "${task_id}"
done

mkdir -p -- "${TASKS_ROOT}"
exec 9>"${MATERIALIZATION_ROOT}/materialize.lock"
flock -n 9 || die "another materializer holds ${MATERIALIZATION_ROOT}/materialize.lock"
SCRATCH="$(mktemp -d "${MATERIALIZATION_ROOT}/.scratch.XXXXXXXX")"
readonly SCRATCH

strays="$(find "${TASKS_ROOT}" -mindepth 1 -maxdepth 1 -printf '%f\n' | LC_ALL=C sort |
  LC_ALL=C comm -23 - <(printf '%s\n' "${TASK_IDS[@]}"))"
[[ -z "${strays}" ]] ||
  die "tasks/ holds entries that are not materialized tasks; an interrupted run leaves <task_id>.partial, which must be inspected and removed: ${strays}"

for task_id in "${TASK_IDS[@]}"; do
  materialize_task "${task_id}"
done

# A task is excluded when its inputs already exceed a service input limit. The
# prompt is measured as full-suite-run.sh submits it: the committed preamble
# followed by the task statement, byte for byte, and as the service measures
# it: a prompt not in NFC is refused there, and one in NFC is its own normal
# form, so its size is the sum of the two files. Both facts are measured on
# the composed text, because NFC of a concatenation is not the concatenation of
# NFCs. The plan records the preamble it measured, and the driver refuses a plan
# whose preamble is not the one it submits. Staging accepts directories, regular
# files and symbolic links, and counts every entry below the workspace root,
# every regular file, and regular-file bytes. The driver adds the task
# environment to the workspace, and checks the composed workspace against the
# staging limits again before it submits.
: >"${SCRATCH}/plan-rows.jsonl"
for task_id in "${TASK_IDS[@]}"; do
  manifest="${TASKS_ROOT}/${task_id}/manifest.json"
  manifest_sha256="$(sha256_of "${manifest}")"
  prompt="$(python3 -c '
import json, sys, unicodedata
composed = open(sys.argv[1], "rb").read() + open(sys.argv[2], "rb").read()
text = composed.decode("utf-8")
print(json.dumps({"nfc": unicodedata.is_normalized("NFC", text),
                  "bytes": len(unicodedata.normalize("NFC", text).encode("utf-8"))}))
' "${PREAMBLE}" "${DATASET_ROOT}/${task_id}/instruction.md")" ||
    die "the composed prompt of ${task_id} is not UTF-8 text"
  jq -c --arg manifest_sha256 "${manifest_sha256}" --argjson prompt "${prompt}" \
    '{manifest_sha256: $manifest_sha256, manifest: ., prompt: $prompt}' \
    "${manifest}" >>"${SCRATCH}/plan-rows.jsonl"
done
# A prompt already in NFC measures what its files hold, which ties the
# measurement to the statement the manifest recorded.
jq -se --argjson preamble_bytes "${PREAMBLE_BYTES}" \
  'all(.[]; (.prompt.nfc | not) or .prompt.bytes == $preamble_bytes + .manifest.inputs.instruction_bytes)' \
  "${SCRATCH}/plan-rows.jsonl" >/dev/null ||
  die 'an NFC prompt measured other than its preamble and statement bytes'
jq -s \
  --arg dataset_lock_sha256 "${DATASET_LOCK_SHA256}" \
  --arg images_lock_sha256 "${IMAGES_LOCK_SHA256}" \
  --arg materializer_git_blob "${MATERIALIZER_GIT_BLOB}" \
  --arg preamble_sha256 "${PREAMBLE_SHA256}" \
  --argjson preamble_bytes "${PREAMBLE_BYTES}" \
  --argjson service_limits "${SERVICE_LIMITS}" \
  --arg prompt_unicode_version "${PROMPT_UNICODE_VERSION}" \
  'def prompt_bytes: .prompt.bytes;
   def exclusion_reason:
     .manifest as $manifest |
     if .prompt.nfc | not then
       "prompt_is_not_nfc"
     elif prompt_bytes > $service_limits.max_prompt_bytes then
       "prompt_bytes_exceed_max_prompt_bytes"
     elif $manifest.source.special_file_count > 0 then
       "source_holds_special_files"
     elif $manifest.source.regular_file_count > $service_limits.max_staged_files then
       "source_regular_files_exceed_max_staged_files"
     elif $manifest.source.regular_file_bytes > $service_limits.max_staged_bytes then
       "source_regular_file_bytes_exceed_max_staged_bytes"
     elif $manifest.source.directory_count + $manifest.source.regular_file_count
       + $manifest.source.symlink_count > $service_limits.max_staged_entries then
       "source_entries_exceed_max_staged_entries"
     else null end;
   map(. + {reason: exclusion_reason, prompt_bytes: prompt_bytes}) |
   {schema_version: 5,
    dataset_lock_sha256: $dataset_lock_sha256,
    images_lock_sha256: $images_lock_sha256,
    materializer_git_blob: $materializer_git_blob,
    prompt_preamble: {sha256: $preamble_sha256, bytes: $preamble_bytes},
    prompt_measure: {form: "NFC", unicode_version: $prompt_unicode_version},
    service_limits: $service_limits,
    tasks: [.[] | select(.reason == null) |
      {task_id: .manifest.task_id, language: .manifest.language, manifest_sha256}],
    excluded: [.[] | select(.reason != null) |
      {task_id: .manifest.task_id, reason, manifest_sha256} +
      if .reason == "prompt_bytes_exceed_max_prompt_bytes" or .reason == "prompt_is_not_nfc"
      then {prompt_bytes} else {} end]}' \
  "${SCRATCH}/plan-rows.jsonl" >"${PLAN}.partial"
jq -e --argjson task_count "${TASK_COUNT}" '(.tasks | length) + (.excluded | length) == $task_count' \
  "${PLAN}.partial" >/dev/null || die 'derived plan does not account for every task exactly once'

# A pass records the SHA-256 of the plan it ran, so a replaced plan is kept beside
# its successor.
if [[ -e "${PLAN}" ]] && cmp -s -- "${PLAN}" "${PLAN}.partial"; then
  rm -- "${PLAN}.partial"
else
  sync -f -- "${PLAN}.partial"
  if [[ -e "${PLAN}" ]]; then
    superseded="${PLAN}.superseded-$(date -u +%Y%m%dT%H%M%SZ)"
    [[ ! -e "${superseded}" ]] || die "plan archive already exists: ${superseded}"
    mv -- "${PLAN}" "${superseded}"
    printf 'PLAN_SUPERSEDED archived=%s\n' "${superseded}"
  fi
  mv -- "${PLAN}.partial" "${PLAN}"
  sync -f -- "${MATERIALIZATION_ROOT}"
fi

jq -r '.prompt_preamble.bytes as $preamble | .service_limits.max_prompt_bytes as $limit |
  .excluded[] | "EXCLUDED \(.task_id): \(.reason)" +
    if .reason == "prompt_bytes_exceed_max_prompt_bytes" then
      "; the submitted prompt is \(.prompt_bytes) bytes, a \($preamble)-byte preamble and a " +
      "\(.prompt_bytes - $preamble)-byte task statement, past max_prompt_bytes \($limit)"
    elif .reason == "prompt_is_not_nfc" then
      "; the submitted prompt, the preamble followed by the task statement, is not in NFC, " +
      "which the service refuses; in NFC it is \(.prompt_bytes) bytes"
    else "" end' "${PLAN}"
printf 'FULL_SUITE_MATERIALIZATION_COMPLETE tasks=%s eligible=%s excluded=%s plan=%s\n' \
  "${TASK_COUNT}" "$(jq -r '.tasks | length' "${PLAN}")" "$(jq -r '.excluded | length' "${PLAN}")" \
  "${PLAN}"
