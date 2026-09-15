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
    id            BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,
    user_id       BIGINT NOT NULL,
    kind          VARCHAR(32) NOT NULL DEFAULT 'device',
    name          VARCHAR(160) NOT NULL,
    token_hash    VARCHAR(128) NOT NULL UNIQUE,
    platform      VARCHAR(64),
    created_at    DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    last_seen_at  DATETIME(3),
    revoked       TINYINT(1) NOT NULL DEFAULT 0,
    CONSTRAINT fk_agent_devices_user FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
    INDEX idx_agent_devices_user (user_id, id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE agent_sessions (
    id          BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,
    user_id     BIGINT NOT NULL,
    -- 'cloud' runs in a sandbox on this server; 'device' runs on the user's
    -- own machine through the desktop client.
    target      VARCHAR(32) NOT NULL,
    -- Set only for target='device'. ON DELETE SET NULL so removing a machine
    -- keeps its transcript readable instead of destroying the user's history.
    device_id   BIGINT,
    title       VARCHAR(200) NOT NULL DEFAULT '新任务',
    model       VARCHAR(160),
    -- idle | running | failed. Reset to idle on boot: a process that was
    -- streaming when the server stopped is gone, and a session stuck at
    -- 'running' could never be resumed.
    status      VARCHAR(32) NOT NULL DEFAULT 'idle',
    -- Last pi entry id mirrored into agent_entries. Doubles as the `since`
    -- cursor for incremental sync, so a reconnect never refetches or
    -- duplicates history.
    cursor      VARCHAR(64),
    created_at  DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    updated_at  DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    CONSTRAINT fk_agent_sessions_user FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
    CONSTRAINT fk_agent_sessions_device FOREIGN KEY (device_id) REFERENCES agent_devices(id) ON DELETE SET NULL,
    INDEX idx_agent_sessions_user_updated (user_id, updated_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Mirror of the runtime's own session tree. pi owns the authoritative JSONL on
-- the executing side; this copy is what every client reads, so a phone sees
-- the same transcript as the machine that ran the tools.
--
-- UNIQUE(session_id, entry_id) makes mirroring idempotent: replaying an
-- overlapping `get_entries` range after a reconnect cannot duplicate rows.
CREATE TABLE agent_entries (
    id          BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,
    session_id  BIGINT NOT NULL,
    entry_id    VARCHAR(64) NOT NULL,
    parent_id   VARCHAR(64),
    kind        VARCHAR(48) NOT NULL,
    payload     LONGTEXT NOT NULL,
    created_at  DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    CONSTRAINT fk_agent_entries_session FOREIGN KEY (session_id) REFERENCES agent_sessions(id) ON DELETE CASCADE,
    UNIQUE KEY uq_agent_entries (session_id, entry_id),
    INDEX idx_agent_entries_session (session_id, id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
