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
#   - written to a unique temp on the same filesystem, verified with the exact
#     operations the consumer performs (gzip -t + tar -tzf) and required to
#     contain the env.sh the agent sources, BEFORE it is hashed or published;
#   - hashed from the verified bytes, fsync'd, atomically renamed, and only then
#     described by a manifest that is itself fsync'd;
#   - re-proved intact (gzip -t + manifest-sha match) before any skip, so a
#     corrupt or mismatched pair is treated as absent and re-warmed, never
#     grandfathered.
# There is exactly one accepted outcome: a fully-verified archive, or a loud
# failure that names the violated property. No size-only gate, no fall-through.

BENCH_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly BENCH_ROOT
readonly MAT_ROOT="${BENCH_ROOT}/full-suite-v1/materialization"
readonly DATASET_ROOT="${BENCH_ROOT}/evaluator-dataset"
readonly OUT_ROOT="${BENCH_ROOT}/full-suite-v1/task-env"
readonly PLAN_FILE="${BENCH_ROOT}/full-suite-v1/suite-plan.json"
readonly WARM_TIMEOUT_SEC=900
readonly MIN_ARCHIVE_BYTES=1024

for command in awk docker flock gzip jq mktemp sha256sum stat sync tar; do
  command -v "${command}" >/dev/null || { printf 'ERROR: required command is missing: %s\n' "${command}" >&2; exit 2; }
done

mkdir -p -- "${OUT_ROOT}"

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

# Fsync a file and its parent directory so an atomic rename is durable even
# across a crash, matching the materializer's discipline.
sync_path() {
  local path="$1"
  sync -f -- "${path}"
}

# A published pair is intact only when both files exist, the archive passes the
# consumer's own gzip+tar operations, and its bytes hash to exactly what the
# manifest committed to. Any other state -> not intact -> re-warm.
task_env_intact() {
  local out_dir="$1"
  local archive="${out_dir}/task-env.tar.gz"
  local manifest="${out_dir}/env-manifest.json"
  [[ -f "${archive}" && -f "${manifest}" ]] || return 1
  gzip -t -- "${archive}" >/dev/null 2>&1 || return 1
  tar -tzf "${archive}" >/dev/null 2>&1 || return 1
  local want got
  want="$(jq -er '.tar_sha256' "${manifest}" 2>/dev/null)" || return 1
  got="$(sha256sum -- "${archive}" | awk '{print $1}')"
  [[ "${want}" == "${got}" ]]
}

warm_one() {
  local task_id="$1"
  local out_dir="${OUT_ROOT}/${task_id}"
  mkdir -p -- "${out_dir}"

  # Per-task exclusive lock. Two warmer instances must never harvest or publish
  # the same task concurrently; the loser blocks, then observes the verified
  # output and skips. flock releases on fd close / process exit, so a crashed
  # holder cannot deadlock the task.
  local lockfd
  exec {lockfd}>"${out_dir}/.warm.lock"
  flock "${lockfd}"

  if task_env_intact "${out_dir}"; then
    printf '%s: already warmed and verified; skipping.\n' "${task_id}" >&2
    exec {lockfd}>&-
    return 0
  fi
  # A present-but-not-intact pair is corrupt/partial: remove it loudly and
  # rebuild, so no unverified bytes survive.
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
  local stage="" tmp=""
  # Cleanup on every exit path: kill the warm container and drop scratch. The
  # published archive (if any) is deliberately preserved.
  cleanup_warm() {
    docker rm -f "${name}" >/dev/null 2>&1 || true
    [[ -n "${stage}" ]] && rm -rf -- "${stage}"
    [[ -n "${tmp}" ]] && rm -f -- "${tmp}"
    exec {lockfd}>&-
  }

  # Phase 1: warm caches by running the canonical grader script, bounded. A
  # test failure or timeout is expected -- dependency resolution runs first and
  # populates the caches we harvest. The container then idles so the harvest
  # reads the same filesystem.
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
      echo "WARM_RC=$?" >> /warm.log
      git reset --hard >/dev/null 2>&1
      git clean -ffdqx >/dev/null 2>&1
      sleep 100000
    ' >/dev/null \
    || { printf 'ERROR %s: warm container failed to start\n' "${task_id}" >&2; cleanup_warm; return 1; }

  # Wait for the bounded warm phase to finish (WARM_RC is written when test.sh
  # returns or is killed by its own timeout). An overrun beyond that budget is a
  # hard failure, not a silently-harvested indeterminate state.
  local waited=0
  until docker exec "${name}" sh -c 'grep -q WARM_RC /warm.log 2>/dev/null'; do
    sleep 10; waited=$((waited + 10))
    if (( waited > WARM_TIMEOUT_SEC + 300 )); then
      printf 'ERROR %s: warm phase did not complete within %ss; refusing an indeterminate harvest\n' "${task_id}" "$((WARM_TIMEOUT_SEC + 300))" >&2
      cleanup_warm; return 1
    fi
  done
  local warm_rc
  warm_rc="$(docker exec "${name}" sh -c 'sed -n "s/^WARM_RC=//p" /warm.log | tail -1')" \
    || { printf 'ERROR %s: could not read WARM_RC\n' "${task_id}" >&2; cleanup_warm; return 1; }
  [[ "${warm_rc}" =~ ^[0-9]+$ ]] || { printf 'ERROR %s: WARM_RC is not numeric: %q\n' "${task_id}" "${warm_rc}" >&2; cleanup_warm; return 1; }
  docker exec "${name}" sh -c 'tail -3 /warm.log' > "${out_dir}/warm-tail.txt" 2>/dev/null || true

  # Phase 2: write the relocation-aware environment script the agent sources
  # after extracting the tarball to /tmp/task-env, then harvest.
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

  # Harvest with docker cp and a host-side tar: several task images ship no
  # tar/gzip at all, so nothing is assumed about in-container tooling beyond sh.
  # env.sh rides at tar root. Any docker cp failure is fatal (fail-closed).
  local dirs
  dirs="$(docker exec "${name}" sh -c '
    printf "env.sh\n" > /harvest.list
    for d in '"$(printf '%s ' ${HARVEST_DIRS})"'; do
      [ -e "$d" ] && printf "%s\n" "${d#/}" >> /harvest.list
    done
    cat /harvest.list')" \
    || { printf 'ERROR %s: could not enumerate harvest targets\n' "${task_id}" >&2; cleanup_warm; return 1; }
  [[ -n "${dirs}" ]] || { printf 'ERROR %s: nothing to harvest (container dead?)\n' "${task_id}" >&2; cleanup_warm; return 1; }

  stage="$(mktemp -d /tmp/qwen38-taskenv-stage.XXXXXX)" \
    || { printf 'ERROR %s: could not create staging dir\n' "${task_id}" >&2; cleanup_warm; return 1; }
  local rel
  while IFS= read -r rel; do
    [[ -n "${rel}" ]] || continue
    mkdir -p -- "${stage}/$(dirname "${rel}")" \
      || { printf 'ERROR %s: could not create stage subdir for /%s\n' "${task_id}" "${rel}" >&2; cleanup_warm; return 1; }
    docker cp "${name}:/${rel}" "${stage}/$(dirname "${rel}")/" >/dev/null 2>&1 \
      || { printf 'ERROR %s: docker cp failed for /%s\n' "${task_id}" "${rel}" >&2; cleanup_warm; return 1; }
  done <<<"${dirs}"
  [[ -f "${stage}/env.sh" ]] || { printf 'ERROR %s: harvested stage is missing env.sh\n' "${task_id}" >&2; cleanup_warm; return 1; }
  docker rm -f "${name}" >/dev/null 2>&1 || true

  # Build the archive into a unique temp on the target filesystem, then verify
  # the exact bytes that will ship using the consumer's own operations.
  tmp="$(mktemp "${out_dir}/.task-env.XXXXXX.tar.gz")" \
    || { printf 'ERROR %s: could not create temp archive\n' "${task_id}" >&2; cleanup_warm; return 1; }
  tar -czf "${tmp}" -C "${stage}" . \
    || { printf 'ERROR %s: tar -czf failed\n' "${task_id}" >&2; cleanup_warm; return 1; }
  rm -rf -- "${stage}"; stage=""

  gzip -t -- "${tmp}" \
    || { printf 'ERROR %s: archive failed gzip integrity (gzip -t); refusing to publish\n' "${task_id}" >&2; cleanup_warm; return 1; }
  tar -tzf "${tmp}" >/dev/null \
    || { printf 'ERROR %s: archive failed tar listing (tar -tzf); refusing to publish\n' "${task_id}" >&2; cleanup_warm; return 1; }
  tar -tzf "${tmp}" | grep -qxE '[.]/env[.]sh|env[.]sh' \
    || { printf 'ERROR %s: archive does not contain env.sh at its root; refusing to publish\n' "${task_id}" >&2; cleanup_warm; return 1; }

  local bytes
  bytes="$(stat -c '%s' -- "${tmp}")" || { printf 'ERROR %s: could not stat temp archive\n' "${task_id}" >&2; cleanup_warm; return 1; }
  (( bytes > MIN_ARCHIVE_BYTES )) \
    || { printf 'ERROR %s: archive is only %s bytes (<= %s); refusing to publish\n' "${task_id}" "${bytes}" "${MIN_ARCHIVE_BYTES}" >&2; cleanup_warm; return 1; }

  local sha
  sha="$(sha256sum -- "${tmp}" | awk '{print $1}')" || { printf 'ERROR %s: could not hash temp archive\n' "${task_id}" >&2; cleanup_warm; return 1; }

  # Durable atomic publish: fsync the verified bytes, rename into place, fsync
  # the directory, then write and fsync the manifest (which hashes exactly the
  # bytes we just verified).
  sync_path "${tmp}"
  mv -- "${tmp}" "${out_dir}/task-env.tar.gz" \
    || { printf 'ERROR %s: could not publish archive\n' "${task_id}" >&2; cleanup_warm; return 1; }
  tmp=""
  sync_path "${out_dir}"

  local manifest_out="${out_dir}/env-manifest.json"
  local manifest_tmp
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
  sync_path "${manifest_tmp}"
  mv -- "${manifest_tmp}" "${manifest_out}" \
    || { rm -f -- "${manifest_tmp}"; printf 'ERROR %s: could not publish manifest\n' "${task_id}" >&2; cleanup_warm; return 1; }
  sync_path "${out_dir}"

  # Final proof: the published pair must pass the exact intact check a consumer
  # (and the skip-guard) will apply.
  task_env_intact "${out_dir}" \
    || { printf 'ERROR %s: published pair failed post-publish intact re-proof\n' "${task_id}" >&2; cleanup_warm; return 1; }

  cleanup_warm
  printf '%s: warmed, verified, and published (%s bytes, warm_test_rc=%s).\n' "${task_id}" "${bytes}" "${warm_rc}" >&2
}

failures=0
while read -r task_id; do
  warm_one "${task_id}" || { failures=$((failures + 1)); printf 'TASK_ENV_FAILED %s\n' "${task_id}" >&2; }
done < <(jq -er '.runs[].task_id' "${PLAN_FILE}")
printf 'TASK_ENV_PASS_COMPLETE failures=%s\n' "${failures}"
(( failures == 0 ))
