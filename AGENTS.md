# Yunova Agent Instructions

The repository is `git@github.com:yiranxiaohui/Yunova.git`, the local primary
checkout is `/home/orca/projects/Yunova`, and task worktrees belong under
`/home/orca/worktrees/Yunova`. New builds use `yunova` and
`ghcr.io/yiranxiaohui/yunova` (mirror: `docker.yunnet.top/github/yiranxiaohui/yunova`).

The source rename does not relocate the existing production deployment:
on `root@114.66.55.93` it still uses `/opt/NovaChat/docker-compose.yml`,
service/container `novachat`, and host port `4300`. Inspect these live targets
before deploying. Preserve existing database paths and mounts when switching
to a Yunova image; do not assume `/opt/Yunova` already exists.

## Release and deployment

When the user asks to publish a release, remember to complete the full flow:

1. Default to incrementing only the patch version: `vX.Y.Z` → `vX.Y.(Z+1)`.
2. Publish the tag and GitHub Release, then wait for the container release
   workflow to succeed.
3. Deploy directly from the current trusted host with SSH to
   `root@114.66.55.93`; do not delegate production deployment to GitHub Actions.
4. Back up the current deployment's Compose file and SQLite database, update the
   application image to the release tag, and recreate only its application service.
5. Verify the container is healthy with zero restarts, HTTP returns 200,
   migrations and database integrity pass, and recent logs have no critical
   errors before reporting completion.

Keep this as an agent-operated process. Do not add release or deployment scripts
unless the user explicitly asks for automation code.
