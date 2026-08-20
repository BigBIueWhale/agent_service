#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

# Materialization-time warm-and-extract of per-task toolchains and dependency
# caches. The graders run each task's tests/test.sh with network and rely on the
# image's caches plus on-demand downloads; the agent runs network-none and must
# therefore receive the same toolchain and caches inside its workspace archive.
# This step runs test.sh once per task in a throwaway networked container
# (bounded), then harvests the toolchain/cache directories into task-env.tar.gz
# plus an env.sh the agent sources after extraction. Network is used only here,
# mirroring the grader's posture; agents still never get network.
#
# Correctness contract (every published task-env.tar.gz satisfies all of):
#   - built under a per-task exclusive lock, so two warmer instances can never
#     race two writers onto one file (the defect that truncated tarballs);
#   - the source container is paused before the harvest, so docker cp reads a
#     quiesced filesystem and no member can be captured half-written;
#   - written to a unique temp on the same filesystem, verified with the exact
#     operations the consumer performs (gzip -t + a SIGPIPE-safe tar -tzf
#     listing that must contain env.sh) BEFORE it is hashed or published;
#   - hashed from the verified bytes, fsync'd, atomically renamed, and only then
#     described by a manifest that is itself fsync'd;
#   - re-proved intact (gzip -t + env.sh + schema + manifest-sha) before any
#     skip, so a corrupt/mismatched/pre-schema pair is treated as absent and
#     re-warmed, never grandfathered.
# There is exactly one accepted outcome per task: a fully-verified archive, or a
# loud failure that names the violated property. Every command is fail-closed
# (never relying on set -e, which is void inside warm_one because the loop calls
# it as `warm_one ... || {...}`); a missing/empty plan and any signal are also
# handled, not silently discarded.

BENCH_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly BENCH_ROOT
readonly MAT_ROOT="${BENCH_ROOT}/full-suite-v1/materialization"
readonly DATASET_ROOT="${BENCH_ROOT}/evaluator-dataset"
readonly OUT_ROOT="${BENCH_ROOT}/full-suite-v1/task-env"
readonly PLAN_FILE="${BENCH_ROOT}/full-suite-v1/suite-plan.json"
readonly WARM_TIMEOUT_SEC=900
readonly MIN_ARCHIVE_BYTES=1024

for command in awk docker flock grep gzip jq mktemp sed sha256sum stat sync tar; do
  command -v "${command}" >/dev/null || { printf 'ERROR: required command is missing: %s\n' "${command}" >&2; exit 2; }
done

mkdir -p -- "${OUT_ROOT}" || { printf 'ERROR: cannot create task-env output root %s\n' "${OUT_ROOT}" >&2; exit 2; }

# Directories harvested when present. Covers the suite's ecosystems: Maven
# (+wrapped dists), Gradle, Go, JDKs, CPython prefixes, Node. env.sh maps them
# into the agent's environment.
readonly HARVEST_DIRS='
/root/.m2
/root/.gradle
/root/go
/root/.npm
/root/.cache
/usr/local/go
/usr/share/maven
/usr/bin/mvn
/opt/java
/usr/lib/jvm
/usr/local/bin
/usr/local/lib
/usr/local/include
'

# Signal-safe cleanup of the currently in-flight task's resources. warm_one sets
# these as it progresses and clears them on every normal return; this trap
# handles SIGINT/SIGTERM, which bypass the function's own cleanup, so an aborted
# run cannot strand a bridged container, a multi-GB stage, or a temp archive.
CURRENT_CONTAINER=""
CURRENT_STAGE=""
CURRENT_TMP=""
cleanup_current() {
  [[ -n "${CURRENT_CONTAINER}" ]] && docker rm -f "${CURRENT_CONTAINER}" >/dev/null 2>&1
  [[ -n "${CURRENT_STAGE}" ]] && rm -rf -- "${CURRENT_STAGE}" 2>/dev/null
  [[ -n "${CURRENT_TMP}" ]] && rm -f -- "${CURRENT_TMP}" 2>/dev/null
  return 0
}
trap 'cleanup_current; trap - INT; kill -INT $$' INT
trap 'cleanup_current; exit 143' TERM

sync_path() { sync -f -- "$1"; }

# Does the gzip'd tar list an env.sh at its root? SIGPIPE-safe: the listing is
# captured whole, so `grep -q` never closes the pipe under tar and trips
# pipefail (which spuriously failed valid multi-MB listings).
archive_has_env_sh() {
  local listing
  listing="$(tar -tzf "$1")" || return 1
  grep -qxE '[.]/env[.]sh|env[.]sh' <<<"${listing}"
}

# A published pair is intact only when both files exist, the archive passes the
# consumer's own gzip+tar operations, lists env.sh at its root, carries the
# current manifest schema, and its bytes hash to exactly what the manifest
# committed to. Any other state -> not intact -> re-warm.
task_env_intact() {
  local out_dir="$1"
  local archive="${out_dir}/task-env.tar.gz"
  local manifest="${out_dir}/env-manifest.json"
  [[ -f "${archive}" && -f "${manifest}" ]] || return 1
  gzip -t -- "${archive}" >/dev/null 2>&1 || return 1
  archive_has_env_sh "${archive}" || return 1
  jq -e '.schema_version == 2' "${manifest}" >/dev/null 2>&1 || return 1
  local want got
  want="$(jq -er '.tar_sha256' "${manifest}" 2>/dev/null)" || return 1
  got="$(sha256sum -- "${archive}" | awk '{print $1}')"
  [[ "${want}" == "${got}" ]]
}

warm_one() {
  local task_id="$1"
  local out_dir="${OUT_ROOT}/${task_id}"
  mkdir -p -- "${out_dir}" || { printf 'ERROR %s: cannot create output dir %s\n' "${task_id}" "${out_dir}" >&2; return 1; }

  # Per-task exclusive lock (guarded: a failed lock must not fall through to an
  # unlocked publish). Releases on crash via fd close; the loser blocks, then
  # observes the verified output and skips.
  local lockfd
  exec {lockfd}>"${out_dir}/.warm.lock" || { printf 'ERROR %s: cannot open per-task lock file\n' "${task_id}" >&2; return 1; }
  flock "${lockfd}" || { printf 'ERROR %s: cannot acquire per-task warm lock on %s\n' "${task_id}" "${out_dir}/.warm.lock" >&2; exec {lockfd}>&-; return 1; }

  if task_env_intact "${out_dir}"; then
    printf '%s: already warmed and verified; skipping.\n' "${task_id}" >&2
    exec {lockfd}>&-; return 0
  fi
  if [[ -e "${out_dir}/task-env.tar.gz" || -e "${out_dir}/env-manifest.json" ]]; then
    printf '%s: existing task-env failed integrity re-proof; discarding and re-warming.\n' "${task_id}" >&2
    rm -f -- "${out_dir}/task-env.tar.gz" "${out_dir}/env-manifest.json"
  fi

  local manifest="${MAT_ROOT}/${task_id}/manifest.json"
  [[ -f "${manifest}" ]] || { printf 'ERROR %s: materialization manifest is missing at %s\n' "${task_id}" "${manifest}" >&2; exec {lockfd}>&-; return 1; }
  local image working_dir
  image="$(jq -er '.environment.image_tag' "${manifest}")" || { printf 'ERROR %s: manifest lacks .environment.image_tag\n' "${task_id}" >&2; exec {lockfd}>&-; return 1; }
  working_dir="$(jq -er '.environment.working_dir' "${manifest}")" || { printf 'ERROR %s: manifest lacks .environment.working_dir\n' "${task_id}" >&2; exec {lockfd}>&-; return 1; }
  if ! docker image inspect "${image}" >/dev/null 2>&1; then
    docker load --input "${MAT_ROOT}/${task_id}/environment-image.tar" >/dev/null \
      || { printf 'ERROR %s: could not load environment image %s\n' "${task_id}" "${image}" >&2; exec {lockfd}>&-; return 1; }
  fi

  local name="qwen38-taskenv-${task_id//[^A-Za-z0-9_.-]/_}-$$"
  # Cleanup on every non-crash return path (the trap covers signals). The
  # published archive/manifest are deliberately preserved.
  cleanup_warm() {
    [[ -n "${CURRENT_CONTAINER}" ]] && docker rm -f "${CURRENT_CONTAINER}" >/dev/null 2>&1
    [[ -n "${CURRENT_STAGE}" ]] && rm -rf -- "${CURRENT_STAGE}"
    [[ -n "${CURRENT_TMP}" ]] && rm -f -- "${CURRENT_TMP}"
    CURRENT_CONTAINER=""; CURRENT_STAGE=""; CURRENT_TMP=""
    exec {lockfd}>&-
    return 0
  }

  # Phase 1: warm caches by running the canonical grader script, bounded. A test
  # failure or timeout is expected -- dependency resolution runs first and
  # populates the caches we harvest. WARM_RC is written only after test.sh AND
  # the worktree reset finish, so /warm.rc is a true 'quiesced' signal; the
  # container then idles so the harvest reads that same filesystem.
  docker run --detach --name "${name}" \
    --network bridge \
    --cpus 4 --memory 16384m --pids-limit 4096 \
    --security-opt no-new-privileges \
    --env TASK_WORKING_DIR="${working_dir}" \
    --mount "type=bind,src=${DATASET_ROOT}/${task_id}/tests,dst=/benchmark-tests,readonly" \
    --entrypoint sh "${image}" -c '
      rm -rf /tests && cp -a /benchmark-tests /tests
      cd "$TASK_WORKING_DIR" || exit 9
      timeout '"${WARM_TIMEOUT_SEC}"' bash /tests/test.sh > /warm.log 2>&1
      rc=$?
      git reset --hard >/dev/null 2>&1
      git clean -ffdqx >/dev/null 2>&1
      echo "WARM_RC=$rc" > /warm.rc
      sleep 100000
    ' >/dev/null \
    || { printf 'ERROR %s: warm container failed to start\n' "${task_id}" >&2; cleanup_warm; return 1; }
  CURRENT_CONTAINER="${name}"

  # Wait for the bounded warm+quiesce phase (a dedicated /warm.rc marker, not an
  # unanchored grep of test output). An overrun beyond the budget is a hard
  # failure, not a silently-harvested indeterminate state.
  local waited=0
  until docker exec "${name}" sh -c 'test -s /warm.rc' 2>/dev/null; do
    sleep 10; waited=$((waited + 10))
    if (( waited > WARM_TIMEOUT_SEC + 300 )); then
      printf 'ERROR %s: warm phase did not complete within %ss; refusing an indeterminate harvest\n' "${task_id}" "$((WARM_TIMEOUT_SEC + 300))" >&2
      cleanup_warm; return 1
    fi
  done
  local warm_rc
  warm_rc="$(docker exec "${name}" sh -c 'sed -n "s/^WARM_RC=//p" /warm.rc | tail -1')" \
    || { printf 'ERROR %s: could not read WARM_RC\n' "${task_id}" >&2; cleanup_warm; return 1; }
  [[ "${warm_rc}" =~ ^[0-9]+$ ]] || { printf 'ERROR %s: WARM_RC is not numeric: %q\n' "${task_id}" "${warm_rc}" >&2; cleanup_warm; return 1; }
  docker exec "${name}" sh -c 'tail -3 /warm.log' > "${out_dir}/warm-tail.txt" 2>/dev/null || true

  # Phase 2: write the relocation-aware environment script the agent sources
  # after extracting the tarball to /tmp/task-env.
  docker exec "${name}" sh -c 'cat > /env.sh <<"ENVEOF"
# Task toolchain environment. Source after: tar -xzf .task-env.tar.gz -C /tmp/task-env
TASK_ENV="${TASK_ENV:-/tmp/task-env}"
if [ -d "$TASK_ENV/opt/java/openjdk" ]; then
  export JAVA_HOME="$TASK_ENV/opt/java/openjdk"
elif [ -d "$TASK_ENV/usr/lib/jvm" ]; then
  for j in "$TASK_ENV"/usr/lib/jvm/*/bin/java; do
    [ -x "$j" ] && export JAVA_HOME="${j%/bin/java}" && break
  done
fi
[ -n "$JAVA_HOME" ] && PATH="$JAVA_HOME/bin:$PATH"
if [ -d "$TASK_ENV/usr/local/go" ]; then
  export GOROOT="$TASK_ENV/usr/local/go"
  PATH="$GOROOT/bin:$PATH"
fi
if [ -d "$TASK_ENV/root/go" ]; then
  export GOPATH="$TASK_ENV/root/go"
  export GOMODCACHE="$TASK_ENV/root/go/pkg/mod"
fi
if [ -d "$TASK_ENV/usr/share/maven/bin" ]; then
  export M2_HOME="$TASK_ENV/usr/share/maven"
  PATH="$M2_HOME/bin:$PATH"
fi
[ -d "$TASK_ENV/root/.m2/repository" ] && \
  export MAVEN_OPTS="${MAVEN_OPTS:-} -Dmaven.repo.local=$TASK_ENV/root/.m2/repository"
[ -d "$TASK_ENV/root/.gradle" ] && export GRADLE_USER_HOME="$TASK_ENV/root/.gradle"
if [ -x "$TASK_ENV/usr/local/bin/python3" ]; then
  export PYTHONHOME="$TASK_ENV/usr/local"
  export LD_LIBRARY_PATH="$TASK_ENV/usr/local/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
fi
[ -d "$TASK_ENV/usr/local/lib/node_modules" ] && \
  export NODE_PATH="$TASK_ENV/usr/local/lib/node_modules"
[ -d "$TASK_ENV/usr/local/bin" ] && PATH="$TASK_ENV/usr/local/bin:$PATH"
export PATH
ENVEOF' \
    || { printf 'ERROR %s: could not write env.sh into the warm container\n' "${task_id}" >&2; cleanup_warm; return 1; }

  # Enumerate harvest targets. env.sh rides at tar root.
  local dirs
  dirs="$(docker exec "${name}" sh -c '
    printf "env.sh\n" > /harvest.list
    for d in '"$(printf '%s ' ${HARVEST_DIRS})"'; do
      [ -e "$d" ] && printf "%s\n" "${d#/}" >> /harvest.list
    done
    cat /harvest.list')" \
    || { printf 'ERROR %s: could not enumerate harvest targets\n' "${task_id}" >&2; cleanup_warm; return 1; }
  [[ -n "${dirs}" ]] || { printf 'ERROR %s: nothing to harvest (container dead?)\n' "${task_id}" >&2; cleanup_warm; return 1; }

  # Quiesce the container before the copy: test daemons (gradle/maven) can keep
  # writing the very caches we harvest, and docker cp reads a live filesystem;
  # a paused container yields a coherent, non-changing tree.
  docker pause "${name}" >/dev/null 2>&1 \
    || { printf 'ERROR %s: could not pause the warm container before harvest\n' "${task_id}" >&2; cleanup_warm; return 1; }

  # Harvest with docker cp and a host-side tar: several task images ship no
  # tar/gzip at all, so nothing is assumed about in-container tooling beyond sh.
  # Any docker cp failure is fatal (fail-closed).
  CURRENT_STAGE="$(mktemp -d /tmp/qwen38-taskenv-stage.XXXXXX)" \
    || { printf 'ERROR %s: could not create staging dir\n' "${task_id}" >&2; cleanup_warm; return 1; }
  local stage="${CURRENT_STAGE}" rel
  while IFS= read -r rel; do
    [[ -n "${rel}" ]] || continue
    mkdir -p -- "${stage}/$(dirname "${rel}")" \
      || { printf 'ERROR %s: could not create stage subdir for /%s\n' "${task_id}" "${rel}" >&2; cleanup_warm; return 1; }
    docker cp "${name}:/${rel}" "${stage}/$(dirname "${rel}")/" >/dev/null 2>&1 \
      || { printf 'ERROR %s: docker cp failed for /%s\n' "${task_id}" "${rel}" >&2; cleanup_warm; return 1; }
  done <<<"${dirs}"
  [[ -f "${stage}/env.sh" ]] || { printf 'ERROR %s: harvested stage is missing env.sh\n' "${task_id}" >&2; cleanup_warm; return 1; }
  docker rm -f "${name}" >/dev/null 2>&1 || true
  CURRENT_CONTAINER=""

  # Build the archive into a unique temp on the target filesystem, then verify
  # the exact bytes that will ship using the consumer's own operations.
  CURRENT_TMP="$(mktemp "${out_dir}/.task-env.XXXXXX.tar.gz")" \
    || { printf 'ERROR %s: could not create temp archive\n' "${task_id}" >&2; cleanup_warm; return 1; }
  local tmp="${CURRENT_TMP}"
  tar -czf "${tmp}" -C "${stage}" . \
    || { printf 'ERROR %s: tar -czf failed\n' "${task_id}" >&2; cleanup_warm; return 1; }
  rm -rf -- "${stage}"; CURRENT_STAGE=""

  gzip -t -- "${tmp}" \
    || { printf 'ERROR %s: archive failed gzip integrity (gzip -t); refusing to publish\n' "${task_id}" >&2; cleanup_warm; return 1; }
  archive_has_env_sh "${tmp}" \
    || { printf 'ERROR %s: archive is not a listable tar containing env.sh at its root; refusing to publish\n' "${task_id}" >&2; cleanup_warm; return 1; }

  local bytes
  bytes="$(stat -c '%s' -- "${tmp}")" || { printf 'ERROR %s: could not stat temp archive\n' "${task_id}" >&2; cleanup_warm; return 1; }
  (( bytes > MIN_ARCHIVE_BYTES )) \
    || { printf 'ERROR %s: archive is only %s bytes (<= %s); refusing to publish\n' "${task_id}" "${bytes}" "${MIN_ARCHIVE_BYTES}" >&2; cleanup_warm; return 1; }

  local sha
  sha="$(sha256sum -- "${tmp}" | awk '{print $1}')" || { printf 'ERROR %s: could not hash temp archive\n' "${task_id}" >&2; cleanup_warm; return 1; }

  # Durable atomic publish: fsync the verified bytes, rename, fsync the dir, then
  # write+fsync the manifest (hashing exactly the bytes we verified). Every sync
  # is guarded so a writeback error cannot be silently swallowed.
  sync_path "${tmp}" || { printf 'ERROR %s: could not fsync archive before publish\n' "${task_id}" >&2; cleanup_warm; return 1; }
  mv -- "${tmp}" "${out_dir}/task-env.tar.gz" \
    || { printf 'ERROR %s: could not publish archive\n' "${task_id}" >&2; cleanup_warm; return 1; }
  CURRENT_TMP=""
  sync_path "${out_dir}" || { printf 'ERROR %s: could not fsync directory after publish\n' "${task_id}" >&2; cleanup_warm; return 1; }

  local manifest_out="${out_dir}/env-manifest.json" manifest_tmp
  manifest_tmp="$(mktemp "${out_dir}/.env-manifest.XXXXXX.json")" \
    || { printf 'ERROR %s: could not create temp manifest\n' "${task_id}" >&2; cleanup_warm; return 1; }
  jq -n --arg task "${task_id}" --arg image "${image}" \
    --arg sha256 "${sha}" --argjson bytes "${bytes}" \
    --argjson warm_rc "${warm_rc}" \
    --arg dirs "$(printf '%s ' ${dirs})" \
    '{schema_version:2, task_id:$task, image:$image,
      tar_sha256:$sha256, tar_bytes:$bytes, harvested:($dirs|gsub("\\s+$";"")),
      warm_test_rc:$warm_rc, warm_timeout_sec:'"${WARM_TIMEOUT_SEC}"'}' \
    > "${manifest_tmp}" \
    || { rm -f -- "${manifest_tmp}"; printf 'ERROR %s: could not write manifest\n' "${task_id}" >&2; cleanup_warm; return 1; }
  sync_path "${manifest_tmp}" || { rm -f -- "${manifest_tmp}"; printf 'ERROR %s: could not fsync manifest\n' "${task_id}" >&2; cleanup_warm; return 1; }
  mv -- "${manifest_tmp}" "${manifest_out}" \
    || { rm -f -- "${manifest_tmp}"; printf 'ERROR %s: could not publish manifest\n' "${task_id}" >&2; cleanup_warm; return 1; }
  sync_path "${out_dir}" || { printf 'ERROR %s: could not fsync directory after manifest\n' "${task_id}" >&2; cleanup_warm; return 1; }

  # Final proof: the published pair must pass the exact intact check a consumer
  # (and the skip-guard) will apply.
  task_env_intact "${out_dir}" \
    || { printf 'ERROR %s: published pair failed post-publish intact re-proof\n' "${task_id}" >&2; cleanup_warm; return 1; }

  cleanup_warm
  printf '%s: warmed, verified, and published (%s bytes, warm_test_rc=%s).\n' "${task_id}" "${bytes}" "${warm_rc}" >&2
}

# Read the plan fail-closed BEFORE the loop: a process substitution's exit
# status is discarded, so a missing/malformed/empty plan would otherwise report
# a clean success having warmed nothing.
plan_tasks="$(jq -er '.runs[].task_id' "${PLAN_FILE}")" \
  || { printf 'ERROR: could not read task ids from %s (missing, malformed, or zero runs)\n' "${PLAN_FILE}" >&2; exit 2; }
[[ -n "${plan_tasks}" ]] || { printf 'ERROR: suite plan %s contains zero runs\n' "${PLAN_FILE}" >&2; exit 2; }

failures=0
while read -r task_id; do
  warm_one "${task_id}" || { failures=$((failures + 1)); printf 'TASK_ENV_FAILED %s\n' "${task_id}" >&2; }
done <<<"${plan_tasks}"
printf 'TASK_ENV_PASS_COMPLETE failures=%s\n' "${failures}"
(( failures == 0 ))
