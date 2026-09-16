#!/bin/sh
set -e

# Apply container defaults after checking legacy overrides, so baked-in new
# environment names do not shadow an existing deployment's configuration.
: "${YUNOVA_BIND:=${NOVACHAT_BIND:-0.0.0.0:3000}}"
: "${YUNOVA_DATA_DIR:=${NOVACHAT_DATA_DIR:-/data}}"
export YUNOVA_BIND YUNOVA_DATA_DIR

# When /data is a bind mount, its owner is inherited from the host and the
# baked-in chown from the Dockerfile is lost. Fix it up on startup, then drop
# privileges to the yunova user.
if [ "$(id -u)" = "0" ]; then
    chown -R yunova:yunova /data || true

    # Cloud-computer sandboxes are started through a mounted Docker socket,
    # whose group id belongs to the host and so cannot be baked into the
    # image. Grant it by group rather than by chowning the socket: the socket
    # is shared with the host's own daemon clients, and changing its ownership
    # would affect them too. The server still runs unprivileged; a deployment
    # that does not mount the socket keeps exactly the permissions it had.
    if [ -S /var/run/docker.sock ]; then
        sock_gid="$(stat -c %g /var/run/docker.sock 2>/dev/null || echo)"
        if [ -n "$sock_gid" ] && [ "$sock_gid" != "0" ]; then
            sock_group="$(getent group "$sock_gid" | cut -d: -f1)"
            if [ -z "$sock_group" ]; then
                sock_group=dockerhost
                groupadd -g "$sock_gid" "$sock_group" || true
            fi
            usermod -aG "$sock_group" yunova || true
        fi
    fi

    # Drop to the user without naming a group. Spelling the group out
    # (`yunova:yunova`) makes gosu set exactly that one gid and discard the
    # supplementary groups, which would silently undo the socket grant above.
    # Omitting it keeps the same primary gid from /etc/passwd and the
    # supplementary groups with it.
    exec /usr/bin/tini -- /usr/local/bin/su-exec yunova /usr/local/bin/yunova "$@"
fi

exec /usr/bin/tini -- /usr/local/bin/yunova "$@"
