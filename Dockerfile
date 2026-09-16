# syntax=docker/dockerfile:1.7

# ---- Stage 1: build the web bundle with Bun ------------------------------
FROM oven/bun:1-debian AS webbuilder
WORKDIR /app/web
COPY web/package.json web/bun.lock ./
RUN bun install --frozen-lockfile
COPY web ./
RUN bun run build

# ---- Stage 2a: cargo-chef planner — derive a dep-only "recipe" ------------
FROM lukemathwalker/cargo-chef:latest-rust-1-bookworm AS chef
WORKDIR /app

FROM chef AS planner
COPY Cargo.toml Cargo.lock build.rs ./
COPY src ./src
COPY migrations ./migrations
# Workspace member `desktop` is not built into this image, but cargo refuses
# to load a workspace whose members are missing, so its manifest must be
# present.
COPY desktop ./desktop
RUN cargo chef prepare --recipe-path recipe.json

# ---- Stage 2b: cook deps (cached unless recipe.json changes) -------------
FROM chef AS rustbuilder
# Compile only the external crates. This layer is reused on every code-only
# change. It invalidates only when Cargo.toml / Cargo.lock actually change.
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# Now bring in the real sources and the prebuilt web assets and build the bin.
# We intentionally do NOT copy web/package.json into the rust build context so
# build.rs short-circuits and skips its embedded `bun run build` call.
COPY Cargo.toml Cargo.lock build.rs ./
COPY src ./src
COPY migrations ./migrations
# Workspace members must exist so cargo can load the workspace. We only build
# the `yunova` server binary here (`-p yunova`); `yunova-desktop` runs on the
# user's own machine and is distributed separately, not shipped in this image.
COPY desktop ./desktop
COPY --from=webbuilder /app/web/dist ./web/dist

RUN cargo build --release --locked -p yunova \
    && strip target/release/yunova

# ---- Stage 3: minimal runtime image --------------------------------------
FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates tini gosu ffmpeg \
    && rm -rf /var/lib/apt/lists/* \
    && ln -s /usr/sbin/gosu /usr/local/bin/su-exec

# Docker CLI for the cloud-computer sandbox driver.
#
# Work-mode sessions run in per-session containers that this server starts on
# the host's Docker daemon, and the driver shells out to `docker`. Without the
# binary every cloud session fails at start with ENOENT rather than with a
# usable message. Only the client is copied: the daemon stays on the host, and
# the socket is mounted by the deployment only when sandboxing is wanted.
COPY --from=docker:29-cli /usr/local/bin/docker /usr/local/bin/docker

RUN useradd --system --uid 10001 --home /data yunova \
    && mkdir -p /data \
    && chown -R yunova:yunova /data

COPY --from=rustbuilder /app/target/release/yunova /usr/local/bin/yunova
COPY docker/entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh

WORKDIR /data
VOLUME ["/data"]

EXPOSE 3000

ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
