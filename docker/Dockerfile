# syntax=docker/dockerfile:1.15@sha256:9857836c9ee4268391bb5b09f9f157f3c91bb15821bb77969642813b0d00518d

ARG UBUNTU_IMAGE=ubuntu@sha256:019e8eb29a85e74d64925745884f2ec79aa27e3feab36353d24656f4d6b89467
ARG NODE_IMAGE=node@sha256:d649c27dae7ba0137b3cef5dd75baa422c08dc3d9e3fc0c23dfb172dc3cc6436
ARG RUST_IMAGE=rust@sha256:4c2fd73ef19c5ef9d54bee03b06b2839a392604fbfcd578ed948b71b37c1d7fb
ARG SOURCE_DATE_EPOCH=1786725153

FROM ${NODE_IMAGE} AS node-pin

FROM ${UBUNTU_IMAGE} AS agent-os
ARG DEBIAN_FRONTEND=noninteractive
ARG SOURCE_DATE_EPOCH
ARG UBUNTU_SNAPSHOT
ARG AGENT_APT_LOCK_SHA256
ARG JKS_NORMALIZER_SHA256
ARG JKS_NORMALIZER_TEST_SHA256
COPY config/agent-apt-packages.lock /opt/locks/agent-apt-packages.lock
COPY docker/scripts/normalize_jks.py /usr/local/libexec/agent-service/normalize_jks.py
COPY docker/tests/test_normalize_jks.py /usr/local/libexec/agent-service/test_normalize_jks.py
RUN test "$(sha256sum /opt/locks/agent-apt-packages.lock | cut -d' ' -f1)" = \
      "${AGENT_APT_LOCK_SHA256}" \
 && test "$(sha256sum /usr/local/libexec/agent-service/normalize_jks.py | cut -d' ' -f1)" = \
      "${JKS_NORMALIZER_SHA256}" \
 && test "$(sha256sum /usr/local/libexec/agent-service/test_normalize_jks.py | cut -d' ' -f1)" = \
      "${JKS_NORMALIZER_TEST_SHA256}" \
 && case "${UBUNTU_SNAPSHOT}" in \
      [0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]T[0-9][0-9][0-9][0-9][0-9][0-9]Z) ;; \
      *) printf 'invalid Ubuntu snapshot: %s\n' "${UBUNTU_SNAPSHOT}" >&2; exit 69 ;; \
    esac \
 && printf '%s\n' \
      'Types: deb' \
      "URIs: https://snapshot.ubuntu.com/ubuntu/${UBUNTU_SNAPSHOT}" \
      'Suites: noble noble-updates noble-security' \
      'Components: main universe restricted multiverse' \
      'Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg' \
      > /etc/apt/sources.list.d/ubuntu.sources \
 && apt-get -o Acquire::Check-Valid-Until=false -o Acquire::https::Verify-Peer=false update \
 && apt-get -o Acquire::https::Verify-Peer=false install -y --no-install-recommends \
      ca-certificates=20260601~24.04.1 \
 && apt-get -o Acquire::Check-Valid-Until=false update \
 && xargs -r -a /opt/locks/agent-apt-packages.lock \
      apt-get install -y --no-install-recommends \
 && while IFS= read -r specification; do \
      package="${specification%%=*}"; expected="${specification#*=}"; \
      actual="$(dpkg-query -W -f='${Version}' "${package}")"; \
      test "${actual}" = "${expected}" || { \
        printf 'package drift: %s expected=%s actual=%s\n' "${package}" "${expected}" "${actual}" >&2; \
        exit 70; \
      }; \
    done < /opt/locks/agent-apt-packages.lock \
 && test "$(git config --system --get filter.lfs.process)" = 'git-lfs filter-process' \
 && git config --system --remove-section filter.lfs \
 && git config --system filter.lfs.process 'git-lfs filter-process' \
 && git config --system filter.lfs.required true \
 && git config --system filter.lfs.clean 'git-lfs clean -- %f' \
 && git config --system filter.lfs.smudge 'git-lfs smudge -- %f' \
 && python3 /usr/local/libexec/agent-service/test_normalize_jks.py -v \
 && python3 /usr/local/libexec/agent-service/normalize_jks.py \
      /etc/ssl/certs/java/cacerts "${SOURCE_DATE_EPOCH}" changeit \
 && python3 /usr/local/libexec/agent-service/normalize_jks.py \
      /etc/ssl/certs/java/cacerts "${SOURCE_DATE_EPOCH}" changeit \
 && keytool -list -keystore /etc/ssl/certs/java/cacerts -storepass changeit >/dev/null \
 && find /usr/share/fonts /usr/local/share/fonts -xdev \
      -exec touch --no-dereference --date="@${SOURCE_DATE_EPOCH}" {} + \
 && fc-cache --force \
 && rm -rf /var/lib/apt/lists/* /var/log/apt /var/log/dpkg.log \
      /var/log/alternatives.log /var/cache/ldconfig/aux-cache
COPY --from=node-pin /usr/local/ /usr/local/
RUN test "$(node --version)" = 'v22.23.2' \
 && test "$(npm --version)" = '10.9.8' \
 && ln -s /usr/bin/fdfind /usr/local/bin/fd \
 && ln -s /usr/bin/batcat /usr/local/bin/bat \
 && test "$(getent passwd 1000)" = 'ubuntu:x:1000:1000:Ubuntu:/home/ubuntu:/bin/bash' \
 && test "$(getent group 1000)" = 'ubuntu:x:1000:' \
 && groupmod --new-name agent ubuntu \
 && usermod --login agent --comment Agent --home /home/agent --move-home --shell /bin/bash ubuntu \
 && test "$(getent passwd 1000)" = 'agent:x:1000:1000:Agent:/home/agent:/bin/bash' \
 && test "$(getent group 1000)" = 'agent:x:1000:' \
 && chage --lastday 0 agent \
 && rm -rf /tmp/node-compile-cache

FROM agent-os AS qwen-build
ARG QWEN_SOURCE_ARCHIVE
ARG QWEN_SOURCE_ARCHIVE_SHA256
ARG QWEN_PATCH_SHA256
ARG QWEN_SOURCE_PATCH_MANIFEST_SHA256
ENV QWEN_SKIP_PREPARE=1
ENV SOURCE_DATE_EPOCH=1786725153
WORKDIR /src
ADD --checksum=sha256:${QWEN_SOURCE_ARCHIVE_SHA256} \
  ${QWEN_SOURCE_ARCHIVE} /tmp/qwen-code.tar.gz
COPY patches/qwen-code-0.21.12-agent-service.patch \
  /opt/agent-artifacts/patches/qwen-code-0.21.12-agent-service.patch
COPY patches/source_patch_v1 \
  /opt/agent-artifacts/patches/source_patch_v1
RUN test "$(sha256sum /tmp/qwen-code.tar.gz | cut -d' ' -f1)" = \
      "${QWEN_SOURCE_ARCHIVE_SHA256}" \
 && test "$(sha256sum /opt/agent-artifacts/patches/qwen-code-0.21.12-agent-service.patch | cut -d' ' -f1)" = \
      "${QWEN_PATCH_SHA256}" \
 && test "$(sha256sum /opt/agent-artifacts/patches/source_patch_v1/manifest.sha256 | cut -d' ' -f1)" = \
      "${QWEN_SOURCE_PATCH_MANIFEST_SHA256}" \
 && cd /opt/agent-artifacts \
 && sha256sum --check --strict patches/source_patch_v1/manifest.sha256 \
 && cd /src \
 && tar -xzf /tmp/qwen-code.tar.gz --strip-components=1 -C /src \
 && cd /opt/agent-artifacts \
 && python3 -m unittest -v patches.source_patch_v1.test_framework \
 && python3 -m patches.source_patch_v1.apply_qwen_code_patchset \
      /src /opt/agent-artifacts \
 && cd /src \
 && npm ci --ignore-scripts \
 && npx --no-install patch-package \
 && npm run build -- --cli-only \
 && grep -Fx "export const GIT_COMMIT_INFO = 'b965d5f8c24f';" \
      packages/cli/src/generated/git-commit.ts \
 && grep -Fx "export const CLI_VERSION = '0.21.12';" \
      packages/cli/src/generated/git-commit.ts \
 && cmp packages/cli/src/generated/git-commit.ts \
      packages/core/src/generated/git-commit.ts \
 && DEV=true node esbuild.config.js \
 && node scripts/copy_bundle_assets.js \
 && npx --no-install vitest run \
      packages/core/src/core/geminiChat.test.ts \
      packages/core/src/core/baseLlmClient.test.ts \
      packages/core/src/core/coreToolScheduler.test.ts \
      packages/core/src/core/openaiContentGenerator/converter.test.ts \
      packages/core/src/core/openaiContentGenerator/pipeline.test.ts \
      packages/core/src/core/openaiContentGenerator/pipeline.tokenize.test.ts \
      packages/core/src/core/qwen38-deployment-prompt.test.ts \
      packages/core/src/core/tokenLimits.test.ts \
      packages/core/src/config/qwen38-agent-service-contract.test.ts \
      packages/core/src/agents/runtime/agent-context.test.ts \
      packages/core/src/permissions/permission-manager.test.ts \
      packages/core/src/services/chatCompressionService.test.ts \
      packages/core/src/subagents/builtin-agents.test.ts \
      packages/core/src/tools/agent/agent.test.ts \
      packages/core/src/tools/agent/qwen38-effect-journal.test.ts \
      packages/core/src/tools/agent/qwen38-subagent-scratch.test.ts \
      packages/core/src/utils/image-view.test.ts \
      packages/core/src/utils/qwen38-image-contract.test.ts \
      packages/core/src/utils/shellContextEnv.test.ts \
      packages/cli/src/config/config.test.ts \
      packages/cli/src/gemini.test.tsx \
      packages/cli/src/utils/nonInteractiveHelpers.test.ts \
 && test "$(node scripts/cli-entry.js --version)" = '0.21.12' \
 && rm -rf node_modules/.vite /tmp/node-compile-cache

FROM ${RUST_IMAGE} AS agent-runtime-build
ARG AGENT_EXEC_SOURCE_SHA256
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src/bin/agent_exec.rs src/bin/agent_exec.rs
RUN test "$(rustc --version)" = 'rustc 1.95.0 (59807616e 2026-04-14)' \
 && test "$(sha256sum src/bin/agent_exec.rs | cut -d' ' -f1)" = \
      "${AGENT_EXEC_SOURCE_SHA256}" \
 && cargo test --locked --bin agent_exec \
 && cargo build --locked --release --bin agent_exec

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
COPY --from=qwen-build /src/scripts/cli-entry.js /opt/qwen-code/scripts/cli-entry.js
COPY --from=qwen-build /src/LICENSE /opt/qwen-code/LICENSE
COPY --from=agent-runtime-build /build/target/release/agent_exec /opt/agent/agent_exec
RUN test "$(node /opt/qwen-code/scripts/cli-entry.js --version)" = '0.21.12' \
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
 && python3 /opt/agent/verify_runtime_contract.py \
      /opt/agent/runtime-contract.json \
      /opt/agent/settings.json /opt/agent/QWEN.md /opt/agent/system.md \
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
USER 1000:1000
WORKDIR /workspace
ENTRYPOINT ["/opt/agent/run_agent.sh"]

FROM ${RUST_IMAGE} AS service-build
ARG BROKER_POLICY_SHA256
ARG BROKER_SOURCE_SHA256
ARG CAPTURE_SOURCE_SHA256
ARG RELAY_SOURCE_SHA256
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY config/stack.lock.json config/stack.lock.json
COPY config/broker-policy-v1.json config/broker-policy-v1.json
COPY src src
RUN test "$(rustc --version)" = 'rustc 1.95.0 (59807616e 2026-04-14)' \
 && test "$(sha256sum config/broker-policy-v1.json | cut -d' ' -f1)" = "${BROKER_POLICY_SHA256}" \
 && test "$(sha256sum src/bin/docker_broker.rs | cut -d' ' -f1)" = "${BROKER_SOURCE_SHA256}" \
 && test "$(sha256sum src/bin/session_capture.rs | cut -d' ' -f1)" = "${CAPTURE_SOURCE_SHA256}" \
 && test "$(sha256sum src/bin/fixed_relay.rs | cut -d' ' -f1)" = "${RELAY_SOURCE_SHA256}" \
 && cargo test --locked \
 && cargo build --locked --release

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

FROM ${UBUNTU_IMAGE} AS service-os
ARG DEBIAN_FRONTEND=noninteractive
ARG UBUNTU_SNAPSHOT
ARG SERVICE_APT_LOCK_SHA256
COPY config/service-apt-packages.lock /opt/locks/service-apt-packages.lock
RUN test "$(sha256sum /opt/locks/service-apt-packages.lock | cut -d' ' -f1)" = \
      "${SERVICE_APT_LOCK_SHA256}" \
 && case "${UBUNTU_SNAPSHOT}" in \
      [0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]T[0-9][0-9][0-9][0-9][0-9][0-9]Z) ;; \
      *) printf 'invalid Ubuntu snapshot: %s\n' "${UBUNTU_SNAPSHOT}" >&2; exit 72 ;; \
    esac \
 && printf '%s\n' \
      'Types: deb' \
      "URIs: https://snapshot.ubuntu.com/ubuntu/${UBUNTU_SNAPSHOT}" \
      'Suites: noble noble-updates noble-security' \
      'Components: main universe restricted multiverse' \
      'Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg' \
      > /etc/apt/sources.list.d/ubuntu.sources \
 && apt-get -o Acquire::Check-Valid-Until=false -o Acquire::https::Verify-Peer=false update \
 && apt-get -o Acquire::https::Verify-Peer=false install -y --no-install-recommends \
      ca-certificates=20260601~24.04.1 \
 && apt-get -o Acquire::Check-Valid-Until=false update \
 && xargs -r -a /opt/locks/service-apt-packages.lock \
      apt-get install -y --no-install-recommends \
 && while IFS= read -r specification; do \
      package="${specification%%=*}"; expected="${specification#*=}"; \
      actual="$(dpkg-query -W -f='${Version}' "${package}")"; \
      test "${actual}" = "${expected}" || exit 71; \
    done < /opt/locks/service-apt-packages.lock \
 && rm -rf /var/lib/apt/lists/* /var/log/apt /var/log/dpkg.log \
      /var/cache/ldconfig/aux-cache \
 && test "$(getent passwd 1000)" = 'ubuntu:x:1000:1000:Ubuntu:/home/ubuntu:/bin/bash' \
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
