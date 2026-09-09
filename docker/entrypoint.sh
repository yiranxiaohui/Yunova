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
    exec /usr/bin/tini -- /usr/local/bin/su-exec yunova:yunova /usr/local/bin/yunova "$@"
fi

exec /usr/bin/tini -- /usr/local/bin/yunova "$@"
