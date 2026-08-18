#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

# Materialization-time warm-and-extract of per-task toolchains and
# dependency caches. The graders run each task's tests/test.sh with
# network and rely on the image's caches plus on-demand downloads; the
# agent runs network-none and therefore must receive the same toolchain
# and caches inside its workspace archive. This step runs test.sh once
# per task in a throwaway networked container (bounded), then harvests
# the toolchain/cache directories into task-env.tar.gz plus an env.sh
# the agent can source after extraction. Network is used only here, at
# materialization time, mirroring the grader's own posture; agents
# still never get network.

BENCH_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly BENCH_ROOT
readonly MAT_ROOT="${BENCH_ROOT}/full-suite-v1/materialization"
readonly DATASET_ROOT="${BENCH_ROOT}/evaluator-dataset"
readonly OUT_ROOT="${BENCH_ROOT}/full-suite-v1/task-env"
readonly PLAN_FILE="${BENCH_ROOT}/full-suite-v1/suite-plan.json"
readonly WARM_TIMEOUT_SEC=900

mkdir -p -- "${OUT_ROOT}"

# Directories harvested when present. Covers the ecosystems in the
# suite: Maven (+wrapped dists), Gradle, Go, JDKs, CPython prefixes,
# Node. env.sh below maps them into the agent's environment.
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

warm_one() {
  local task_id="$1"
  local out_dir="${OUT_ROOT}/${task_id}"
  if [[ -f "${out_dir}/task-env.tar.gz" && -f "${out_dir}/env-manifest.json" ]]; then
    printf '%s: already warmed; skipping.\n' "${task_id}" >&2
    return 0
  fi
  local manifest="${MAT_ROOT}/${task_id}/manifest.json"
  [[ -f "${manifest}" ]] || { printf 'ERROR %s: no manifest\n' "${task_id}" >&2; return 1; }
  local image working_dir
  image="$(jq -er '.environment.image_tag' "${manifest}")"
  working_dir="$(jq -er '.environment.working_dir' "${manifest}")"
  docker image inspect "${image}" >/dev/null 2>&1 || {
    docker load --input "${MAT_ROOT}/${task_id}/environment-image.tar" >/dev/null
  }
  local name="qwen38-taskenv-$(date +%s)-$$"
  mkdir -p -- "${out_dir}"
  rm -f -- "${out_dir}/task-env.tar.gz.partial"

  # Phase 1: warm caches by running the canonical grader script, bounded.
  # A timeout or test failure is acceptable: dependency resolution runs
  # first, and a partial cache is still a strictly better agent
  # environment than none. The container keeps running afterward so the
  # harvest happens in the same filesystem.
  set +e
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
      # Undo whatever test.sh did to the worktree; caches live outside it.
      git reset --hard >/dev/null 2>&1
      git clean -ffdqx >/dev/null 2>&1
      sleep 100000
    ' >/dev/null
  local rc=$?
  set -e
  (( rc == 0 )) || { docker rm -f "${name}" >/dev/null 2>&1; printf 'ERROR %s: warm container failed to start\n' "${task_id}" >&2; return 1; }

  # Wait for the warm phase (test.sh under timeout) to finish.
  local waited=0
  until docker exec "${name}" sh -c 'grep -q WARM_RC /warm.log 2>/dev/null'; do
    sleep 10; waited=$((waited + 10))
    if (( waited > WARM_TIMEOUT_SEC + 300 )); then
      printf 'WARN %s: warm phase overran; harvesting current state\n' "${task_id}" >&2
      break
    fi
  done
  docker exec "${name}" sh -c 'tail -3 /warm.log' > "${out_dir}/warm-tail.txt" 2>/dev/null || true

  # Phase 2: write the relocation-aware environment script the agent
  # sources after extracting the tarball to /tmp/task-env, then harvest.
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
  # Relocated CPython prefix: PYTHONHOME scopes this shell to the task
  # interpreter and its site-packages.
  export PYTHONHOME="$TASK_ENV/usr/local"
  export LD_LIBRARY_PATH="$TASK_ENV/usr/local/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
fi
[ -d "$TASK_ENV/usr/local/lib/node_modules" ] && \
  export NODE_PATH="$TASK_ENV/usr/local/lib/node_modules"
[ -d "$TASK_ENV/usr/local/bin" ] && PATH="$TASK_ENV/usr/local/bin:$PATH"
export PATH
ENVEOF'

  # Harvest with docker cp and a host-side tar: several task images ship
  # no tar/gzip at all, so nothing may be assumed about in-container
  # tooling beyond sh. env.sh rides at tar root.
  local dirs
  dirs="$(docker exec "${name}" sh -c '
    printf "env.sh\n" > /harvest.list
    for d in '"$(printf '%s ' ${HARVEST_DIRS})"'; do
      [ -e "$d" ] && printf "%s\n" "${d#/}" >> /harvest.list
    done
    cat /harvest.list')"
  [[ -n "${dirs}" ]] || { docker rm -f "${name}" >/dev/null; printf 'ERROR %s: nothing to harvest (container dead?)\n' "${task_id}" >&2; return 1; }
  local stage rel
  stage="$(mktemp -d /tmp/qwen38-taskenv-stage.XXXXXX)"
  local harvest_ok=true
  while IFS= read -r rel; do
    [[ -n "${rel}" ]] || continue
    mkdir -p -- "${stage}/$(dirname "${rel}")"
    docker cp "${name}:/${rel}" "${stage}/$(dirname "${rel}")/" >/dev/null 2>&1 ||
      { printf 'WARN %s: docker cp failed for /%s\n' "${task_id}" "${rel}" >&2; harvest_ok=false; }
  done <<<"${dirs}"
  docker rm -f "${name}" >/dev/null
  [[ "${harvest_ok}" == true ]] ||
    { rm -rf -- "${stage}"; printf 'ERROR %s: harvest copy incomplete\n' "${task_id}" >&2; return 1; }
  tar -czf "${out_dir}/task-env.tar.gz.partial" -C "${stage}" .
  rm -rf -- "${stage}"

  local bytes sha
  bytes="$(stat -c '%s' "${out_dir}/task-env.tar.gz.partial")"
  (( bytes > 1024 )) || { printf 'ERROR %s: harvest produced %s bytes\n' "${task_id}" "${bytes}" >&2; return 1; }
  mv -- "${out_dir}/task-env.tar.gz.partial" "${out_dir}/task-env.tar.gz"
  sha="$(sha256sum "${out_dir}/task-env.tar.gz" | awk '{print $1}')"

  jq -n --arg task "${task_id}" --arg image "${image}" \
    --arg sha256 "${sha}" --argjson bytes "${bytes}" \
    --arg dirs "$(echo ${dirs} | tr '\n' ' ')" \
    '{schema_version:1, task_id:$task, image:$image,
      tar_sha256:$sha256, tar_bytes:$bytes, harvested:$dirs,
      warm_timeout_sec:'"${WARM_TIMEOUT_SEC}"'}' \
    > "${out_dir}/env-manifest.json"
  printf '%s: warmed and harvested (%s bytes).\n' "${task_id}" "${bytes}" >&2
}

failures=0
while read -r task_id; do
  warm_one "${task_id}" || { failures=$((failures + 1)); printf 'TASK_ENV_FAILED %s\n' "${task_id}" >&2; }
done < <(jq -er '.runs[].task_id' "${PLAN_FILE}")
printf 'TASK_ENV_PASS_COMPLETE failures=%s\n' "${failures}"
(( failures == 0 ))
