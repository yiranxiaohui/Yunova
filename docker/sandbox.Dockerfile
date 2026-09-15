# Cloud sandbox image for Yunova agent tasks.
#
# Runs `pi --mode rpc` as an unprivileged user. The container is the security
# boundary for work-mode tasks: the agent inside has a shell and full write
# access to its own workspace, so it must not be able to reach the host, other
# users' data, or an upstream provider directly.
#
# Credentials are deliberately absent from this image. The runtime receives a
# generated models.json at start time pointing at Yunova's own gateway with a
# session-scoped token, so a compromised or prompt-injected agent can still
# only spend metered, whitelisted quota.
FROM node:24-bookworm-slim

# ripgrep and git back pi's own tooling; ffmpeg matches the server image so
# media tasks behave the same in a sandbox as they do on the host.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        bash ca-certificates curl git ripgrep ffmpeg python3 tini \
    && rm -rf /var/lib/apt/lists/*

# Pin the agent runtime so a sandbox cannot silently change behaviour when
# upstream publishes a new release.
ARG PI_VERSION=0.85.1
RUN npm install -g --ignore-scripts "@earendil-works/pi-coding-agent@${PI_VERSION}" \
    && npm cache clean --force

# Never run the agent as root. Matching the server image's uid keeps bind-mount
# ownership predictable.
RUN useradd --create-home --uid 10001 --shell /bin/bash agent \
    && mkdir -p /workspace /home/agent/.pi/agent \
    && chown -R agent:agent /workspace /home/agent

USER agent
WORKDIR /workspace

ENV PI_CODING_AGENT_DIR=/home/agent/.pi/agent \
    PI_OFFLINE=1 \
    PI_SKIP_VERSION_CHECK=1 \
    HOME=/home/agent

# tini reaps the processes the agent's shell tool spawns; without it a stopped
# container can leave zombies behind.
ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["pi", "--mode", "rpc", "--no-session"]
