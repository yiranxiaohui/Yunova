# Yunova Agent Instructions

The repository is `git@github.com:yiranxiaohui/Yunova.git`, the local primary
checkout is `/home/orca/projects/Yunova`, and task worktrees belong under
`/home/orca/worktrees/Yunova`. New builds use `yunova` and
`ghcr.io/yiranxiaohui/yunova` (mirror: `docker.yunnet.top/github/yiranxiaohui/yunova`).

Production was migrated on 2026-09-15 to a dedicated LXC and now runs on
`root@10.1.51.1` from `/opt/yunova/docker-compose.yml`, service/container
`yunova`, host port `4300`, with persistent state in the bind mount
`/opt/yunova/data`. The former host `114.66.55.93` no longer runs it, and
`/opt/NovaChat` there was removed. Inspect these live targets before deploying,
and preserve existing database paths and mounts when updating the image.

Two deployment details are easy to break. The Compose file pins
`extra_hosts: hz.yunnet.top:10.1.10.1`, because the LAN resolver hands back a
fake-ip for the S3 endpoint and TLS then fails; keep that mapping. The image is
selected by `YUNOVA_IMAGE` in `/opt/yunova/.env`, so update that entry rather
than editing the Compose file. The public entry point `https://chat.yunnet.top`
still resolves to `114.66.55.93`, whose OpenResty proxies to `10.1.51.1:4300`;
the user manages that reverse proxy himself.

## Release and deployment

When the user asks to publish a release, remember to complete the full flow:

1. Default to incrementing only the patch version: `vX.Y.Z` → `vX.Y.(Z+1)`.
2. Publish the tag and GitHub Release, then wait for the container release
   workflow to succeed.
3. Deploy directly from the current trusted host with SSH to
   `root@10.1.51.1`; do not delegate production deployment to GitHub Actions.
4. Back up the current deployment's Compose file and SQLite database into
   `/opt/yunova/backups`, update `YUNOVA_IMAGE` to the release tag, and
   recreate only the application service with
   `docker compose up -d --no-deps yunova`.
5. Verify the container is healthy with zero restarts, HTTP returns 200,
   migrations and database integrity pass, and recent logs have no critical
   errors before reporting completion.

Keep this as an agent-operated process. Do not add release or deployment scripts
unless the user explicitly asks for automation code.
