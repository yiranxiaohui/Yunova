-- Agent access tokens: bearer credentials that let an agent runtime outside
-- the browser (pi on a desktop client, a cloud sandbox container) reach the
-- platform chat gateway. Session cookies can't be used there, and handing out
-- the admin's upstream channel keys would bypass the whitelist and metering.
--
-- Only the SHA-256 hash of the token is stored. `prefix` keeps the leading
-- characters so the UI can still identify a token it can no longer display.
CREATE TABLE agent_tokens (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    token_hash   TEXT NOT NULL UNIQUE,
    prefix       TEXT NOT NULL,
    created_at   TEXT NOT NULL DEFAULT (datetime('now')),
    last_used_at TEXT,
    revoked      INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_agent_tokens_user_created
    ON agent_tokens(user_id, created_at DESC);
