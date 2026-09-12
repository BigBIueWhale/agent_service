# syntax=docker/dockerfile:1.15@sha256:9857836c9ee4268391bb5b09f9f157f3c91bb15821bb77969642813b0d00518d

# Our images. Everything generic -- the pinned apt sets, the toolchains and the
# Node dependency bytes -- is in the two base images `docker/Dockerfile.base`
# builds, and this file reaches the network for nothing. The bases are pinned by
# image ID in `config/stack.lock.json` and `./build.sh` refuses any other, so a
# missing or drifted base is a named failure rather than a silent rebuild.
# Both are required of every build of this file, whichever target it asks for:
# a stage's base name is resolved when the file is parsed, not when the stage is
# reached, so a build that named only the base it needs would fail on the other.
# Neither has a default, because a default is how a build silently proceeds
# against an image nobody pinned.
ARG TOOLCHAIN_BASE_IMAGE
ARG RUNTIME_BASE_IMAGE
ARG RUST_IMAGE=rust@sha256:4c2fd73ef19c5ef9d54bee03b06b2839a392604fbfcd578ed948b71b37c1d7fb
ARG SOURCE_DATE_EPOCH=1786725153

# `COPY --from` resolves a stage name and does not expand build arguments, so
# the base is aliased here to be referable by name from stages that need a file
# out of it without being built on it.
FROM ${TOOLCHAIN_BASE_IMAGE} AS toolchain-base

FROM ${TOOLCHAIN_BASE_IMAGE} AS agent-os
# The identity this stack runs as. The base image carries pinned third-party
# dependencies and nothing of ours, so the account those dependencies will run
# under is established here instead -- the two were one `RUN` before, which is
# what made a generic layer impossible to separate from our own.
RUN test "$(getent passwd 1000)" = 'ubuntu:x:1000:1000:Ubuntu:/home/ubuntu:/bin/bash' \
 && test "$(getent group 1000)" = 'ubuntu:x:1000:' \
 && groupmod --new-name agent ubuntu \
 && usermod --login agent --comment Agent --home /home/agent --move-home --shell /bin/bash ubuntu \
 && test "$(getent passwd 1000)" = 'agent:x:1000:1000:Agent:/home/agent:/bin/bash' \
 && test "$(getent group 1000)" = 'agent:x:1000:' \
 && chage --lastday 0 agent

FROM agent-os AS qwen-build
ARG QWEN_SOURCE_ARCHIVE_SHA256
ARG QWEN_PATCH_SHA256
ARG QWEN_SOURCE_PATCH_MANIFEST_SHA256
ARG NPM_LOCK_PACKAGE_SET_SHA256
ARG NPM_LOCK_PACKAGE_SET_TEST_SHA256
ENV QWEN_SKIP_PREPARE=1
ENV SOURCE_DATE_EPOCH=1786725153
WORKDIR /src
COPY patches/qwen-code-0.21.12-agent-service.patch \
  /opt/agent-artifacts/patches/qwen-code-0.21.12-agent-service.patch
COPY patches/source_patch_v1 \
  /opt/agent-artifacts/patches/source_patch_v1
COPY protocol/stream-contract-v1.json /opt/agent-artifacts/protocol/stream-contract-v1.json
COPY protocol/test-vectors /opt/agent-artifacts/protocol/test-vectors
COPY scripts/generate-protocol-bindings.mjs \
  /opt/agent-artifacts/scripts/generate-protocol-bindings.mjs
COPY docker/scripts/npm_lock_package_set.py \
  /opt/agent-artifacts/scripts/npm_lock_package_set.py
COPY docker/tests/test_npm_lock_package_set.py \
  /opt/agent-artifacts/scripts/test_npm_lock_package_set.py
# The pinned upstream tarball comes from the base image, which fetched it once
# at an exact digest; its digest is proved again here, so this build trusts the
# base's bytes no further than it trusts anything else it is given.
RUN test "$(sha256sum /opt/upstream/qwen-code.tar.gz | cut -d' ' -f1)" = \
      "${QWEN_SOURCE_ARCHIVE_SHA256}" \
 && test "$(sha256sum /opt/agent-artifacts/patches/qwen-code-0.21.12-agent-service.patch | cut -d' ' -f1)" = \
      "${QWEN_PATCH_SHA256}" \
 && test "$(sha256sum /opt/agent-artifacts/patches/source_patch_v1/manifest.sha256 | cut -d' ' -f1)" = \
      "${QWEN_SOURCE_PATCH_MANIFEST_SHA256}" \
 && cd /opt/agent-artifacts \
 && sha256sum --check --strict patches/source_patch_v1/manifest.sha256 \
 && cd /src \
 && tar -xzf /opt/upstream/qwen-code.tar.gz --strip-components=1 -C /src \
 && cd /opt/agent-artifacts \
 && python3 -m unittest -v patches.source_patch_v1.test_framework \
 && python3 -m patches.source_patch_v1.apply_qwen_code_patchset \
      /src /opt/agent-artifacts \
 && cd /src \
 && test "$(sha256sum /opt/agent-artifacts/scripts/npm_lock_package_set.py | cut -d' ' -f1)" = \
      "${NPM_LOCK_PACKAGE_SET_SHA256}" \
 && test "$(sha256sum /opt/agent-artifacts/scripts/test_npm_lock_package_set.py | cut -d' ' -f1)" = \
      "${NPM_LOCK_PACKAGE_SET_TEST_SHA256}" \
 && python3 /opt/agent-artifacts/scripts/test_npm_lock_package_set.py \
 && if [ "$(python3 /opt/agent-artifacts/scripts/npm_lock_package_set.py \
      /src/package-lock.json)" != "$(cat /opt/locks/npm-package-set.sha256)" ]; then \
      echo 'The base image does not carry the packages this lock resolves.' >&2; \
      echo 'The patched lockfile asks for a tarball set the base image was not' >&2; \
      echo 'built with, so no offline install of it can be complete. Rebuild the' >&2; \
      echo 'base with ./scripts/build-base-images.sh and adopt its new ID.' >&2; \
      echo 'Nothing is fetched to make up the difference.' >&2; \
      exit 73; \
    fi \
 && npm ci --ignore-scripts --offline \
 && npx --no-install patch-package \
 && node /opt/agent-artifacts/scripts/generate-protocol-bindings.mjs \
      /src \
 && node /opt/agent-artifacts/scripts/generate-protocol-bindings.mjs \
      /src --check \
 && npm run build -- --cli-only \
 && npm run build --workspace=packages/webui \
 && grep -Fx "export const GIT_COMMIT_INFO = 'b965d5f8c24f';" \
      packages/cli/src/generated/git-commit.ts \
 && grep -Fx "export const CLI_VERSION = '0.21.12';" \
      packages/cli/src/generated/git-commit.ts \
 && cmp packages/cli/src/generated/git-commit.ts \
      packages/core/src/generated/git-commit.ts \
 && DEV=true node esbuild.config.js \
 && node scripts/copy_bundle_assets.js \
 && npx --no-install vitest run --config scripts/tests/vitest.config.ts \
      scripts/tests/package-assets.test.js \
      scripts/tests/stream-contract-assets.test.js \
 && PYTHONPATH=/opt/agent-artifacts python3 -c \
      'from patches.source_patch_v1.generated_qwen_code_stage import FINAL_FILES; print("\n".join(sorted(path for path, digest in FINAL_FILES.items() if digest is not None)))' \
      > /tmp/patched-files.txt \
 && grep -E '\.test\.tsx?$' /tmp/patched-files.txt > /tmp/patched-tests.txt \
 && grep -vE '\.test\.tsx?$' /tmp/patched-files.txt \
      | grep -E '\.tsx?$' \
      | sed -E 's/\.(tsx?)$/.test.\1/' > /tmp/adjacent-tests.txt \
 && while read -r f; do \
      test -f "/src/$f" \
        || { echo "patched test file is missing from the tree: $f"; exit 1; }; \
    done < /tmp/patched-tests.txt \
 && { while read -r f; do \
        test -f "/src/$f" && echo "$f"; \
      done < /tmp/adjacent-tests.txt || true; } > /tmp/adjacent-present.txt \
 && sort -u /tmp/patched-tests.txt /tmp/adjacent-present.txt \
      | grep -v '^integration-tests/' > /tmp/qwen-test-files.txt \
 && test -s /tmp/qwen-test-files.txt \
 && echo "executing $(wc -l < /tmp/qwen-test-files.txt) test files in full:" \
 && cat /tmp/qwen-test-files.txt \
 && cut -d/ -f1,2 /tmp/qwen-test-files.txt | sort -u > /tmp/qwen-test-packages.txt \
 && while read -r package; do \
      (cd "/src/$package" \
        && sed -n "s|^$package/||p" /tmp/qwen-test-files.txt \
          | xargs npx --no-install vitest run) || exit 1; \
    done < /tmp/qwen-test-packages.txt \
 && test "$(node dist/cli.js --version)" = '0.21.12' \
 && rm -rf node_modules/.vite /tmp/node-compile-cache

FROM ${RUST_IMAGE} AS agent-runtime-build
WORKDIR /build
ARG AGENT_EXEC_SOURCE_SHA256
COPY Cargo.toml Cargo.lock build.rs ./
COPY protocol protocol
COPY src/bin/agent_exec.rs src/bin/agent_exec.rs
COPY src/lib.rs src/error.rs src/result_parse.rs src/serde_contract.rs src/stream_contract.rs src/
COPY src/container_image.rs src/container_image.rs
COPY src/bin/event_certifier.rs src/bin/event_certifier.rs
COPY config/stack.lock.json config/agent-runtime-contract-v1.json config/
COPY docker/config/settings.json docker/config/settings.json
RUN test "$(rustc --version)" = 'rustc 1.95.0 (59807616e 2026-04-14)' \
 && test "$(sha256sum src/bin/agent_exec.rs | cut -d' ' -f1)" = "${AGENT_EXEC_SOURCE_SHA256}" \
 && cargo test --locked --package runtime_contract \
 && cargo test --locked --lib --bin agent_exec --bin event_certifier \
 && cargo build --locked --release --bin agent_exec --bin event_certifier

FROM agent-os AS agent
ARG AGENT_EXEC_SOURCE_SHA256
ARG AGENT_EXEC_SANDBOX
ARG SETTINGS_SHA256
ARG INSTRUCTIONS_SHA256
ARG SYSTEM_PROMPT_SHA256
ARG DEPLOYMENT_CONTRACT_SHA256
ARG TOOLCHAIN_MANIFEST_SHA256
ARG TOOLCHAIN_VERIFIER_SHA256
ARG TOOLCHAIN_VERIFIER_TEST_SHA256
ARG RUNTIME_CONTRACT_SHA256
ARG RUNTIME_CONTRACT_VERIFIER_SHA256
ARG RUNTIME_CONTRACT_VERIFIER_TEST_SHA256
ARG WRAPPER_CONTRACT_TEST_SHA256
ARG WRAPPER_SHA256
ARG QWEN_COMMIT
ARG QWEN_SOURCE_ARCHIVE_SHA256
ARG QWEN_PATCH_SHA256
ARG QWEN_SOURCE_PATCH_MANIFEST_SHA256
LABEL agent_service.profile="qwen38-agent-service-v3" \
      agent_service.qwen.version="0.21.12" \
      agent_service.qwen.commit="${QWEN_COMMIT}" \
      agent_service.qwen.archive.sha256="${QWEN_SOURCE_ARCHIVE_SHA256}" \
      agent_service.qwen.patch.sha256="${QWEN_PATCH_SHA256}" \
      agent_service.qwen.source-patch-manifest.sha256="${QWEN_SOURCE_PATCH_MANIFEST_SHA256}" \
      agent_service.settings.sha256="${SETTINGS_SHA256}" \
      agent_service.instructions.sha256="${INSTRUCTIONS_SHA256}" \
      agent_service.system-prompt.sha256="${SYSTEM_PROMPT_SHA256}" \
      agent_service.deployment-contract.sha256="${DEPLOYMENT_CONTRACT_SHA256}" \
      agent_service.toolchain-manifest.sha256="${TOOLCHAIN_MANIFEST_SHA256}" \
      agent_service.toolchain-verifier.sha256="${TOOLCHAIN_VERIFIER_SHA256}" \
      agent_service.runtime-contract.sha256="${RUNTIME_CONTRACT_SHA256}" \
      agent_service.runtime-contract-verifier.sha256="${RUNTIME_CONTRACT_VERIFIER_SHA256}" \
      agent_service.agent-exec.source.sha256="${AGENT_EXEC_SOURCE_SHA256}" \
      agent_service.agent-exec.sandbox="${AGENT_EXEC_SANDBOX}" \
      agent_service.wrapper.sha256="${WRAPPER_SHA256}"
COPY --from=qwen-build /src/dist /opt/qwen-code/dist
COPY --from=qwen-build /src/node_modules /opt/qwen-code/node_modules
COPY --from=qwen-build /src/package.json /opt/qwen-code/package.json
COPY --from=qwen-build /src/LICENSE /opt/qwen-code/LICENSE
COPY --from=agent-runtime-build /build/target/release/agent_exec /opt/agent/agent_exec
COPY --from=agent-runtime-build /build/target/release/event_certifier /opt/agent/event_certifier
RUN test "$(node /opt/qwen-code/dist/cli.js --version)" = '0.21.12' \
 && test -n "${AGENT_EXEC_SOURCE_SHA256}" \
 && test "${AGENT_EXEC_SANDBOX}" = \
      'landlock-fs-v4-write-roots-v1+private-devpts-rw-v1+output-unmounted-v1' \
 && test -x /opt/agent/agent_exec \
 && rm -rf /tmp/node-compile-cache
COPY docker/config/settings.json /opt/agent/settings.json
COPY docker/config/QWEN.md /opt/agent/QWEN.md
COPY docker/config/system.md /opt/agent/system.md
COPY docker/config/deployment-contract.md /opt/agent/deployment-contract.md
COPY config/agent-runtime-contract-v1.json /opt/agent/runtime-contract.json
COPY docker/scripts/verify_runtime_contract.py /opt/agent/verify_runtime_contract.py
COPY src/bin/agent_exec.rs /usr/share/agent-service/agent_exec.rs
COPY docker/config/toolchain-manifest.json /opt/agent/toolchain-manifest.json
COPY docker/scripts/verify_toolchain.py /opt/agent/verify_toolchain.py
COPY docker/config/run_agent.sh /opt/agent/run_agent.sh
COPY docker/tests/run_agent_signal_contract.sh /tmp/run_agent_signal_contract.sh
COPY config/agent-runtime-contract-v1.json /tmp/runtime-contract-tests/config/agent-runtime-contract-v1.json
COPY docker/config/settings.json /tmp/runtime-contract-tests/docker/config/settings.json
COPY docker/config/QWEN.md /tmp/runtime-contract-tests/docker/config/QWEN.md
COPY docker/config/system.md /tmp/runtime-contract-tests/docker/config/system.md
COPY docker/config/deployment-contract.md /tmp/runtime-contract-tests/docker/config/deployment-contract.md
COPY docker/config/toolchain-manifest.json /tmp/runtime-contract-tests/docker/config/toolchain-manifest.json
COPY docker/config/run_agent.sh /tmp/runtime-contract-tests/docker/config/run_agent.sh
COPY src/bin/agent_exec.rs /tmp/runtime-contract-tests/src/bin/agent_exec.rs
COPY docker/scripts/verify_runtime_contract.py /tmp/runtime-contract-tests/docker/scripts/verify_runtime_contract.py
COPY docker/tests/test_verify_runtime_contract.py /tmp/runtime-contract-tests/docker/tests/test_verify_runtime_contract.py
COPY docker/scripts/check_headless_cli.py /tmp/runtime-contract-tests/docker/scripts/check_headless_cli.py
COPY docker/tests/test_headless_cli_smoke.py /tmp/runtime-contract-tests/docker/tests/test_headless_cli_smoke.py
COPY docker/scripts/verify_toolchain.py /tmp/toolchain-tests/docker/scripts/verify_toolchain.py
COPY docker/tests/test_verify_toolchain.py /tmp/toolchain-tests/docker/tests/test_verify_toolchain.py
COPY LICENSE /usr/share/doc/agent_service/UNLICENSE
COPY THIRD_PARTY_NOTICES.md /usr/share/doc/agent_service/THIRD_PARTY_NOTICES.md
COPY LICENSES/Apache-2.0.txt /usr/share/doc/agent_service/Apache-2.0.txt
RUN test "$(sha256sum /opt/agent/settings.json | cut -d' ' -f1)" = "${SETTINGS_SHA256}" \
 && test "$(sha256sum /opt/agent/QWEN.md | cut -d' ' -f1)" = "${INSTRUCTIONS_SHA256}" \
 && test "$(sha256sum /opt/agent/system.md | cut -d' ' -f1)" = "${SYSTEM_PROMPT_SHA256}" \
 && test "$(sha256sum /opt/agent/deployment-contract.md | cut -d' ' -f1)" = "${DEPLOYMENT_CONTRACT_SHA256}" \
 && test "$(sha256sum /opt/agent/runtime-contract.json | cut -d' ' -f1)" = "${RUNTIME_CONTRACT_SHA256}" \
 && test "$(sha256sum /opt/agent/verify_runtime_contract.py | cut -d' ' -f1)" = "${RUNTIME_CONTRACT_VERIFIER_SHA256}" \
 && test "$(sha256sum /tmp/runtime-contract-tests/docker/tests/test_verify_runtime_contract.py | cut -d' ' -f1)" = "${RUNTIME_CONTRACT_VERIFIER_TEST_SHA256}" \
 && test "$(sha256sum /opt/agent/toolchain-manifest.json | cut -d' ' -f1)" = "${TOOLCHAIN_MANIFEST_SHA256}" \
 && test "$(sha256sum /opt/agent/verify_toolchain.py | cut -d' ' -f1)" = "${TOOLCHAIN_VERIFIER_SHA256}" \
 && test "$(sha256sum /tmp/toolchain-tests/docker/tests/test_verify_toolchain.py | cut -d' ' -f1)" = "${TOOLCHAIN_VERIFIER_TEST_SHA256}" \
 && test "$(sha256sum /opt/agent/run_agent.sh | cut -d' ' -f1)" = "${WRAPPER_SHA256}" \
 && test "$(sha256sum /tmp/run_agent_signal_contract.sh | cut -d' ' -f1)" = "${WRAPPER_CONTRACT_TEST_SHA256}" \
 && test -s /opt/qwen-code/LICENSE \
 && test -s /usr/share/doc/agent_service/UNLICENSE \
 && test -s /usr/share/doc/agent_service/THIRD_PARTY_NOTICES.md \
 && test -s /usr/share/doc/agent_service/Apache-2.0.txt \
 && bash /tmp/run_agent_signal_contract.sh /opt/agent/run_agent.sh \
 && python3 /tmp/runtime-contract-tests/docker/tests/test_verify_runtime_contract.py -v \
 && python3 /tmp/runtime-contract-tests/docker/tests/test_headless_cli_smoke.py -v \
 && python3 /opt/agent/verify_runtime_contract.py \
      /opt/agent/runtime-contract.json \
      /opt/agent/settings.json \
      /opt/agent/QWEN.md /opt/agent/system.md \
      /opt/agent/deployment-contract.md /opt/agent/toolchain-manifest.json \
      /opt/agent/run_agent.sh /usr/share/agent-service/agent_exec.rs \
 && python3 /tmp/toolchain-tests/docker/tests/test_verify_toolchain.py -v \
 && python3 /opt/agent/verify_toolchain.py \
      /opt/agent/toolchain-manifest.json /opt/locks/agent-apt-packages.lock \
 && rm -f /tmp/run_agent_signal_contract.sh \
 && rm -rf /tmp/runtime-contract-tests \
 && rm -rf /tmp/toolchain-tests \
 && chmod 0555 /opt/agent/run_agent.sh \
      /opt/agent/agent_exec \
      /opt/agent/verify_runtime_contract.py \
      /opt/agent/verify_toolchain.py \
 && chmod 0444 /opt/qwen-code/LICENSE \
      /usr/share/agent-service/agent_exec.rs \
      /usr/share/doc/agent_service/UNLICENSE \
      /usr/share/doc/agent_service/THIRD_PARTY_NOTICES.md \
      /usr/share/doc/agent_service/Apache-2.0.txt \
 && chmod -R a-w /opt/agent
ENV LANG=C.UTF-8 LC_ALL=C.UTF-8
COPY --chown=1000:1000 docker/scripts/check_headless_cli.py /tmp/check_headless_cli.py
RUN install -d -o 1000 -g 1000 -m 0700 /workspace
USER 1000:1000
RUN PYTHONPATH=/opt/agent python3 /tmp/check_headless_cli.py \
      /opt/qwen-code/dist/cli.js /opt/agent/settings.json \
      /usr/share/agent-service/agent_exec.rs /opt/agent/event_certifier \
 && rm /tmp/check_headless_cli.py
WORKDIR /workspace
ENTRYPOINT ["/opt/agent/run_agent.sh"]

FROM ${RUST_IMAGE} AS service-build
ARG BROKER_POLICY_SHA256
ARG BROKER_SOURCE_SHA256
ARG CAPTURE_SOURCE_SHA256
ARG RELAY_SOURCE_SHA256
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY .dockerignore ./
COPY build.rs ./
COPY protocol protocol
COPY docker/Dockerfile docker/Dockerfile
COPY scripts/generate-protocol-bindings.mjs scripts/generate-protocol-bindings.mjs
COPY config/stack.lock.json config/stack.lock.json
COPY config/agent-runtime-contract-v1.json config/agent-runtime-contract-v1.json
COPY docker/config/settings.json docker/config/settings.json
COPY config/broker-policy-v1.json config/broker-policy-v1.json
COPY src src
COPY docker/tests docker/tests
RUN test "$(rustc --version)" = 'rustc 1.95.0 (59807616e 2026-04-14)' \
 && test "$(sha256sum config/broker-policy-v1.json | cut -d' ' -f1)" = "${BROKER_POLICY_SHA256}" \
 && test "$(sha256sum src/bin/docker_broker.rs | cut -d' ' -f1)" = "${BROKER_SOURCE_SHA256}" \
 && test "$(sha256sum src/bin/session_capture.rs | cut -d' ' -f1)" = "${CAPTURE_SOURCE_SHA256}" \
 && test "$(sha256sum src/bin/fixed_relay.rs | cut -d' ' -f1)" = "${RELAY_SOURCE_SHA256}" \
 && cargo test --locked --workspace \
 && cargo build --locked --release

FROM service-build AS composition-gate-build
# The gate exporter is a Node script and this is a Rust stage, so it borrows the
# one pinned Node the toolchain base already carries rather than pulling another.
COPY --from=toolchain-base /usr/local/bin/node /node
RUN cargo test --locked --release --no-run --message-format=json \
      --bin agent_service --bin docker_broker --bin session_capture > /gate-compiler.jsonl \
 && /node /build/docker/tests/export_composition_artifacts.mjs

FROM scratch AS composition-gate-artifacts
COPY --from=composition-gate-build /gate-artifacts/ /

FROM scratch AS relay
ARG RELAY_SOURCE_SHA256
ARG RELAY_SANDBOX
LABEL agent_service.profile="qwen38-agent-service-v3" \
      agent_service.component="fixed-relay" \
      agent_service.relay.roles="model-bridge,model-ingress,service-bridge,service-ingress,agent-model" \
      agent_service.relay.source.sha256="${RELAY_SOURCE_SHA256}" \
      agent_service.relay.sandbox="${RELAY_SANDBOX}"
COPY --from=service-build /build/target/release/fixed_relay /fixed_relay
COPY --from=service-build /lib/x86_64-linux-gnu/libc.so.6 /lib/x86_64-linux-gnu/libc.so.6
COPY --from=service-build /lib/x86_64-linux-gnu/libm.so.6 /lib/x86_64-linux-gnu/libm.so.6
COPY --from=service-build /lib/x86_64-linux-gnu/libgcc_s.so.1 /lib/x86_64-linux-gnu/libgcc_s.so.1
COPY --from=service-build /lib/x86_64-linux-gnu/ld-linux-x86-64.so.2 /lib64/ld-linux-x86-64.so.2
USER 1000:1000
ENTRYPOINT ["/fixed_relay"]

FROM scratch AS session-capture
ARG CAPTURE_SOURCE_SHA256
ARG CAPTURE_ID
LABEL agent_service.profile="qwen38-agent-service-v3" \
      agent_service.component="session-capture" \
      agent_service.capture.source.sha256="${CAPTURE_SOURCE_SHA256}" \
      agent_service.capture.id="${CAPTURE_ID}"
COPY --from=service-build /build/target/release/session_capture /session_capture
COPY --from=service-build /lib/x86_64-linux-gnu/libc.so.6 /lib/x86_64-linux-gnu/libc.so.6
COPY --from=service-build /lib/x86_64-linux-gnu/libm.so.6 /lib/x86_64-linux-gnu/libm.so.6
COPY --from=service-build /lib/x86_64-linux-gnu/libgcc_s.so.1 /lib/x86_64-linux-gnu/libgcc_s.so.1
COPY --from=service-build /lib/x86_64-linux-gnu/ld-linux-x86-64.so.2 /lib64/ld-linux-x86-64.so.2
USER 1000:1000
ENTRYPOINT ["/session_capture"]

FROM ${RUNTIME_BASE_IMAGE} AS service-os
# As in `agent-os`: the base is generic, so our account -- here with no login
# shell and a membership in the Docker group the broker needs -- is ours to set
# up on top of it.
RUN test "$(getent passwd 1000)" = 'ubuntu:x:1000:1000:Ubuntu:/home/ubuntu:/bin/bash' \
 && test "$(getent group 1000)" = 'ubuntu:x:1000:' \
 && groupmod --new-name agent ubuntu \
 && usermod --login agent --comment Agent --home /home/agent --move-home --shell /usr/sbin/nologin ubuntu \
 && test "$(getent passwd 1000)" = 'agent:x:1000:1000:Agent:/home/agent:/usr/sbin/nologin' \
 && test "$(getent group 1000)" = 'agent:x:1000:' \
 && ! getent group 984 \
 && groupadd --gid 984 docker \
 && usermod --append --groups 984 agent \
 && chage --lastday 0 agent

FROM service-os AS broker-os
ARG DOCKER_CLI_ARCHIVE
ARG DOCKER_CLI_ARCHIVE_SHA256
ADD --checksum=sha256:${DOCKER_CLI_ARCHIVE_SHA256} \
  ${DOCKER_CLI_ARCHIVE} /tmp/docker.tgz
RUN test "$(sha256sum /tmp/docker.tgz | cut -d' ' -f1)" = \
      "${DOCKER_CLI_ARCHIVE_SHA256}" \
 && tar -xzf /tmp/docker.tgz -C /tmp docker/docker \
 && install -o root -g root -m 0555 /tmp/docker/docker /usr/local/bin/docker \
 && test "$(docker --version)" = 'Docker version 29.7.2, build a7dcaa6' \
 && rm -rf /tmp/docker /tmp/docker.tgz

FROM scratch AS broker
ARG BROKER_POLICY_SHA256
ARG BROKER_SOURCE_SHA256
LABEL agent_service.profile="qwen38-agent-service-v3" \
      agent_service.component="docker-broker" \
      agent_service.broker.policy.sha256="${BROKER_POLICY_SHA256}" \
      agent_service.broker.source.sha256="${BROKER_SOURCE_SHA256}"
COPY --from=service-build /build/target/release/docker_broker /docker_broker
COPY --from=broker-os /usr/local/bin/docker /docker
COPY --from=service-build /lib/x86_64-linux-gnu/libc.so.6 /lib/x86_64-linux-gnu/libc.so.6
COPY --from=service-build /lib/x86_64-linux-gnu/libm.so.6 /lib/x86_64-linux-gnu/libm.so.6
COPY --from=service-build /lib/x86_64-linux-gnu/libgcc_s.so.1 /lib/x86_64-linux-gnu/libgcc_s.so.1
COPY --from=service-build /lib/x86_64-linux-gnu/ld-linux-x86-64.so.2 /lib64/ld-linux-x86-64.so.2
ENV DOCKER_HOST=unix:///var/run/docker.sock HOME=/nonexistent LANG=C.UTF-8 LC_ALL=C.UTF-8
USER 1000:984
ENTRYPOINT ["/docker_broker"]

FROM service-os AS service
ARG SOURCE_COMMIT
ARG BUILD_INPUTS_MANIFEST_SHA256
ARG STACK_LOCK_SHA256
ARG CARGO_LOCK_SHA256
LABEL agent_service.profile="qwen38-agent-service-v3" \
      agent_service.source.commit="${SOURCE_COMMIT}" \
      agent_service.build-inputs.sha256="${BUILD_INPUTS_MANIFEST_SHA256}" \
      agent_service.stack-lock.sha256="${STACK_LOCK_SHA256}" \
      agent_service.cargo-lock.sha256="${CARGO_LOCK_SHA256}"
COPY --from=service-build /build/target/release/agent_service /usr/local/bin/agent_service
COPY LICENSE /usr/share/doc/agent_service/UNLICENSE
COPY THIRD_PARTY_NOTICES.md /usr/share/doc/agent_service/THIRD_PARTY_NOTICES.md
COPY LICENSES/Apache-2.0.txt /usr/share/doc/agent_service/Apache-2.0.txt
RUN test -n "${SOURCE_COMMIT}" \
 && test "${#SOURCE_COMMIT}" = 40 \
 && test "${#BUILD_INPUTS_MANIFEST_SHA256}" = 64 \
 && test "${#STACK_LOCK_SHA256}" = 64 \
 && test "${#CARGO_LOCK_SHA256}" = 64 \
 && test -s /usr/share/doc/agent_service/UNLICENSE \
 && test -s /usr/share/doc/agent_service/THIRD_PARTY_NOTICES.md \
 && test -s /usr/share/doc/agent_service/Apache-2.0.txt \
 && chmod 0555 /usr/local/bin/agent_service \
 && chmod 0444 /usr/share/doc/agent_service/UNLICENSE \
      /usr/share/doc/agent_service/THIRD_PARTY_NOTICES.md \
      /usr/share/doc/agent_service/Apache-2.0.txt
ENV LANG=C.UTF-8 LC_ALL=C.UTF-8
USER 1000:1000
WORKDIR /home/user/Desktop/agent_service
ENTRYPOINT ["/usr/local/bin/agent_service"]
