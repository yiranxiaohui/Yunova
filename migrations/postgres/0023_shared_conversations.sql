-- Public-shared snapshots of conversations. See sqlite/0023 for the contract.
CREATE TABLE shared_conversations (
    token            TEXT PRIMARY KEY,
    user_id          BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    conversation_id  BIGINT NOT NULL,
    title            TEXT NOT NULL,
    creator_name     TEXT,
    snapshot_json    TEXT NOT NULL,
    view_count       BIGINT NOT NULL DEFAULT 0,
    created_at       TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS'),
    expires_at       TEXT
);
CREATE INDEX idx_shared_user_created
    ON shared_conversations(user_id, created_at DESC);
CREATE INDEX idx_shared_conv
    ON shared_conversations(conversation_id);
