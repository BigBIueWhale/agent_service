# syntax=docker/dockerfile:1.15@sha256:9857836c9ee4268391bb5b09f9f157f3c91bb15821bb77969642813b0d00518d

ARG UBUNTU_IMAGE=ubuntu@sha256:019e8eb29a85e74d64925745884f2ec79aa27e3feab36353d24656f4d6b89467
ARG NODE_IMAGE=node@sha256:d649c27dae7ba0137b3cef5dd75baa422c08dc3d9e3fc0c23dfb172dc3cc6436
ARG RUST_IMAGE=rust@sha256:4c2fd73ef19c5ef9d54bee03b06b2839a392604fbfcd578ed948b71b37c1d7fb
ARG SOURCE_DATE_EPOCH=1786725153

FROM ${NODE_IMAGE} AS node-pin

FROM ${UBUNTU_IMAGE} AS agent-os
ARG DEBIAN_FRONTEND=noninteractive
ARG UBUNTU_SNAPSHOT
ARG AGENT_APT_LOCK_SHA256
COPY config/agent-apt-packages.lock /opt/locks/agent-apt-packages.lock
RUN test "$(sha256sum /opt/locks/agent-apt-packages.lock | cut -d' ' -f1)" = \
      "${AGENT_APT_LOCK_SHA256}" \
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
 && rm -rf /var/lib/apt/lists/* /var/log/apt /var/log/dpkg.log
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
 && chage --lastday 0 agent

FROM agent-os AS qwen-build
ARG QWEN_SOURCE_ARCHIVE
ARG QWEN_SOURCE_ARCHIVE_SHA256
ARG QWEN_PATCH_SHA256
ENV QWEN_SKIP_PREPARE=1
ENV SOURCE_DATE_EPOCH=1786725153
WORKDIR /src
ADD --checksum=sha256:${QWEN_SOURCE_ARCHIVE_SHA256} \
  ${QWEN_SOURCE_ARCHIVE} /tmp/qwen-code.tar.gz
COPY patches/qwen-code-0.21.12-agent-service.patch /tmp/agent-service.patch
RUN test "$(sha256sum /tmp/qwen-code.tar.gz | cut -d' ' -f1)" = \
      "${QWEN_SOURCE_ARCHIVE_SHA256}" \
 && test "$(sha256sum /tmp/agent-service.patch | cut -d' ' -f1)" = \
      "${QWEN_PATCH_SHA256}" \
 && tar -xzf /tmp/qwen-code.tar.gz --strip-components=1 -C /src \
 && git apply --check /tmp/agent-service.patch \
 && git apply /tmp/agent-service.patch \
 && npm ci --ignore-scripts \
 && npx --no-install patch-package \
 && npm run build -- --cli-only \
 && node -e "const fs=require('node:fs'); for (const p of ['packages/cli/src/generated/git-commit.ts','packages/core/src/generated/git-commit.ts']) { let s=fs.readFileSync(p,'utf8'); s=s.replace(\"export const GIT_COMMIT_INFO = 'N/A';\",\"export const GIT_COMMIT_INFO = 'b965d5f8c24f';\"); fs.writeFileSync(p,s); }" \
 && DEV=true node esbuild.config.js \
 && node scripts/copy_bundle_assets.js \
 && npx --no-install vitest run \
      packages/core/src/core/geminiChat.test.ts \
      packages/core/src/core/openaiContentGenerator/converter.test.ts \
      packages/core/src/core/openaiContentGenerator/pipeline.tokenize.test.ts \
      packages/core/src/core/tokenLimits.test.ts \
      packages/core/src/config/qwen38-agent-service-contract.test.ts \
      packages/core/src/permissions/permission-manager.test.ts \
      packages/core/src/tools/agent/agent.test.ts \
      packages/core/src/utils/image-view.test.ts \
      packages/core/src/utils/qwen38-image-contract.test.ts \
      packages/cli/src/config/config.test.ts \
      packages/cli/src/gemini.test.tsx \
      packages/cli/src/utils/nonInteractiveHelpers.test.ts \
 && test "$(node scripts/cli-entry.js --version)" = '0.21.12'

FROM agent-os AS agent
ARG SETTINGS_SHA256
ARG INSTRUCTIONS_SHA256
ARG WRAPPER_SHA256
ARG QWEN_COMMIT
ARG QWEN_SOURCE_ARCHIVE_SHA256
ARG QWEN_PATCH_SHA256
LABEL agent_service.profile="qwen38-agent-service-v2" \
      agent_service.qwen.version="0.21.12" \
      agent_service.qwen.commit="${QWEN_COMMIT}" \
      agent_service.qwen.archive.sha256="${QWEN_SOURCE_ARCHIVE_SHA256}" \
      agent_service.qwen.patch.sha256="${QWEN_PATCH_SHA256}" \
      agent_service.settings.sha256="${SETTINGS_SHA256}" \
      agent_service.instructions.sha256="${INSTRUCTIONS_SHA256}" \
      agent_service.wrapper.sha256="${WRAPPER_SHA256}"
COPY --from=qwen-build /src/dist /opt/qwen-code/dist
COPY --from=qwen-build /src/node_modules /opt/qwen-code/node_modules
COPY --from=qwen-build /src/package.json /opt/qwen-code/package.json
COPY --from=qwen-build /src/scripts/cli-entry.js /opt/qwen-code/scripts/cli-entry.js
RUN test "$(node /opt/qwen-code/scripts/cli-entry.js --version)" = '0.21.12'
COPY docker/config/settings.json /opt/agent/settings.json
COPY docker/config/QWEN.md /opt/agent/QWEN.md
COPY docker/config/run_agent.sh /opt/agent/run_agent.sh
COPY docker/tests/run_agent_signal_contract.sh /tmp/run_agent_signal_contract.sh
RUN test "$(sha256sum /opt/agent/settings.json | cut -d' ' -f1)" = "${SETTINGS_SHA256}" \
 && test "$(sha256sum /opt/agent/QWEN.md | cut -d' ' -f1)" = "${INSTRUCTIONS_SHA256}" \
 && test "$(sha256sum /opt/agent/run_agent.sh | cut -d' ' -f1)" = "${WRAPPER_SHA256}" \
 && bash /tmp/run_agent_signal_contract.sh /opt/agent/run_agent.sh \
 && rm -f /tmp/run_agent_signal_contract.sh \
 && chmod 0555 /opt/agent/run_agent.sh \
 && chmod -R a-w /opt/agent
ENV LANG=C.UTF-8 LC_ALL=C.UTF-8
USER 1000:1000
WORKDIR /workspace
ENTRYPOINT ["/opt/agent/run_agent.sh"]

FROM ${RUST_IMAGE} AS service-build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY config/stack.lock.json config/stack.lock.json
COPY src src
RUN test "$(rustc --version)" = 'rustc 1.95.0 (59807616e 2026-04-14)' \
 && cargo test --locked \
 && cargo build --locked --release

FROM ${UBUNTU_IMAGE} AS service-os
ARG DEBIAN_FRONTEND=noninteractive
ARG UBUNTU_SNAPSHOT
ARG SERVICE_APT_LOCK_SHA256
ARG DOCKER_CLI_ARCHIVE
ARG DOCKER_CLI_ARCHIVE_SHA256
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
ADD --checksum=sha256:${DOCKER_CLI_ARCHIVE_SHA256} \
  ${DOCKER_CLI_ARCHIVE} /tmp/docker.tgz
RUN test "$(sha256sum /tmp/docker.tgz | cut -d' ' -f1)" = \
      "${DOCKER_CLI_ARCHIVE_SHA256}" \
 && tar -xzf /tmp/docker.tgz -C /tmp docker/docker \
 && install -o root -g root -m 0555 /tmp/docker/docker /usr/local/bin/docker \
 && test "$(docker --version)" = 'Docker version 29.7.2, build a7dcaa6' \
 && rm -rf /tmp/docker /tmp/docker.tgz

FROM service-os AS service
ARG SOURCE_COMMIT
ARG STACK_LOCK_SHA256
ARG CARGO_LOCK_SHA256
LABEL agent_service.profile="qwen38-agent-service-v2" \
      agent_service.source.commit="${SOURCE_COMMIT}" \
      agent_service.stack-lock.sha256="${STACK_LOCK_SHA256}" \
      agent_service.cargo-lock.sha256="${CARGO_LOCK_SHA256}"
COPY --from=service-build /build/target/release/agent_service /usr/local/bin/agent_service
RUN test -n "${SOURCE_COMMIT}" \
 && test "${#SOURCE_COMMIT}" = 40 \
 && test "${#STACK_LOCK_SHA256}" = 64 \
 && test "${#CARGO_LOCK_SHA256}" = 64 \
 && chmod 0555 /usr/local/bin/agent_service
ENV LANG=C.UTF-8 LC_ALL=C.UTF-8
USER 1000:1000
WORKDIR /home/user/Desktop/agent_service
ENTRYPOINT ["/usr/local/bin/agent_service"]
