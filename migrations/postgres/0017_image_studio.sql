CREATE TABLE studio_conversations (
    id          BIGSERIAL PRIMARY KEY,
    user_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    title       TEXT NOT NULL DEFAULT '新建工作台',
    model       TEXT,
    created_at  TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS'),
    updated_at  TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS')
);
CREATE INDEX idx_studio_conv_user ON studio_conversations(user_id, updated_at DESC);

CREATE TABLE studio_messages (
    id              BIGSERIAL PRIMARY KEY,
    conversation_id BIGINT NOT NULL REFERENCES studio_conversations(id) ON DELETE CASCADE,
    role            TEXT NOT NULL,
    text            TEXT,
    images_json     TEXT,
    created_at      TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS')
);
CREATE INDEX idx_studio_msg_conv ON studio_messages(conversation_id, id);

CREATE TABLE studio_jobs (
    id              BIGSERIAL PRIMARY KEY,
    token           TEXT NOT NULL UNIQUE,
    user_id         BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    conversation_id BIGINT NOT NULL REFERENCES studio_conversations(id) ON DELETE CASCADE,
    status          TEXT NOT NULL DEFAULT 'pending',
    error           TEXT,
    created_at      TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS'),
    finished_at     TEXT
);
CREATE INDEX idx_studio_jobs_user ON studio_jobs(user_id, created_at DESC);
