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
# Workspace members `worker` and `desktop` are not built into this image, but
# cargo refuses to load a workspace whose members are missing, so their
# manifests must be present.
COPY worker ./worker
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
# the `yunova` server binary here (`-p yunova`); `yunova-worker` and
# `yunova-desktop` run on the user's own machines and are distributed
# separately (see CI release job, worker/README.md), not shipped in this image.
COPY worker ./worker
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
