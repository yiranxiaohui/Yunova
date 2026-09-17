-- Timestamps are TEXT and booleans are INT, not `timestamptz` / `boolean`.
--
-- The pool is `sqlx::Any`, whose type table covers only bool, the integer
-- widths, float/double, text and blob. A `timestamptz` column is therefore
-- undecodable: the driver rejects the *value*, so a single such column makes
-- every row of its table unreadable no matter how the query is written. The
-- same applies to `numeric`, which is why sums are cast to BIGINT in code.
--
-- Storing `YYYY-MM-DD HH24:MI:SS` in UTC matches SQLite's `datetime('now')`
-- byte for byte, so lexicographic comparison, `substr`-based day bucketing
-- and the Rust-side parsing are identical on both backends. Booleans as 0/1
-- match SQLite's INTEGER for the same reason.
--
-- Migration 48 converts databases that were created with the original
-- (unusable) native types.

CREATE TABLE users (
    id                BIGSERIAL PRIMARY KEY,
    username          TEXT NOT NULL,
    password_hash     TEXT NOT NULL,
    default_prompt_id BIGINT,
    created_at        TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS')
);
CREATE UNIQUE INDEX users_username_lower ON users (LOWER(username));

CREATE TABLE sessions (
    token_hash TEXT PRIMARY KEY,
    user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS'),
    expires_at TEXT NOT NULL
);
CREATE INDEX idx_sessions_user ON sessions(user_id);
CREATE INDEX idx_sessions_expires ON sessions(expires_at);

CREATE TABLE conversations (
    id            BIGSERIAL PRIMARY KEY,
    user_id       BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    title         TEXT NOT NULL DEFAULT '新会话',
    system_prompt TEXT NOT NULL DEFAULT '',
    created_at    TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS'),
    updated_at    TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS')
);
CREATE INDEX idx_conversations_user_updated
    ON conversations(user_id, updated_at DESC);

CREATE TABLE messages (
    id              BIGSERIAL PRIMARY KEY,
    conversation_id BIGINT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    role            TEXT NOT NULL CHECK (role IN ('system','user','assistant')),
    content         TEXT NOT NULL,
    created_at      TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS')
);
CREATE INDEX idx_messages_conversation ON messages(conversation_id, id);

CREATE TABLE prompts (
    id         BIGSERIAL PRIMARY KEY,
    user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    content    TEXT NOT NULL,
    is_public  INT NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS'),
    updated_at TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS')
);
CREATE INDEX idx_prompts_user ON prompts(user_id, updated_at DESC);
CREATE INDEX idx_prompts_public
    ON prompts(is_public, created_at DESC)
    WHERE is_public = 1;
