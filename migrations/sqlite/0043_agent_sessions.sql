-- Agent sessions: the target-agnostic session model shared by the cloud
-- sandbox and a user's own machine.
--
-- Both execution targets run `pi --mode rpc` and speak the same JSONL
-- protocol, so a target differs only in *transport* (a local subprocess or a
-- relayed device socket). Keeping one session/entry model means the web, the
-- desktop client and the phone all read the same history regardless of where
-- the tools actually ran.

-- A registered execution endpoint owned by a user. `kind='cloud'` is a sandbox
-- this server allocates; `kind='device'` is the user's own machine running the
-- desktop client, which dials in and holds the socket open (NAT-friendly, the
-- same reason the legacy worker did it this way).
CREATE TABLE agent_devices (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id       INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind          TEXT NOT NULL DEFAULT 'device',
    name          TEXT NOT NULL,
    token_hash    TEXT NOT NULL UNIQUE,
    platform      TEXT,
    created_at    TEXT NOT NULL DEFAULT (datetime('now')),
    last_seen_at  TEXT,
    revoked       INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_agent_devices_user ON agent_devices(user_id, id DESC);

CREATE TABLE agent_sessions (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- 'cloud' runs in a sandbox on this server; 'device' runs on the user's
    -- own machine through the desktop client.
    target      TEXT NOT NULL,
    -- Set only for target='device'. ON DELETE SET NULL so removing a machine
    -- keeps its transcript readable instead of destroying the user's history.
    device_id   INTEGER REFERENCES agent_devices(id) ON DELETE SET NULL,
    title       TEXT NOT NULL DEFAULT '新任务',
    model       TEXT,
    -- idle | running | failed. Reset to idle on boot: a process that was
    -- streaming when the server stopped is gone, and a session stuck at
    -- 'running' could never be resumed.
    status      TEXT NOT NULL DEFAULT 'idle',
    -- Last pi entry id mirrored into agent_entries. Doubles as the `since`
    -- cursor for incremental sync, so a reconnect never refetches or
    -- duplicates history.
    cursor      TEXT,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX idx_agent_sessions_user_updated
    ON agent_sessions(user_id, updated_at DESC);

-- Mirror of the runtime's own session tree. pi owns the authoritative JSONL on
-- the executing side; this copy is what every client reads, so a phone sees
-- the same transcript as the machine that ran the tools.
--
-- UNIQUE(session_id, entry_id) makes mirroring idempotent: replaying an
-- overlapping `get_entries` range after a reconnect cannot duplicate rows.
CREATE TABLE agent_entries (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id  INTEGER NOT NULL REFERENCES agent_sessions(id) ON DELETE CASCADE,
    entry_id    TEXT NOT NULL,
    parent_id   TEXT,
    kind        TEXT NOT NULL,
    payload     TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (session_id, entry_id)
);
CREATE INDEX idx_agent_entries_session ON agent_entries(session_id, id);
