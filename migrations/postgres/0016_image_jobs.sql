CREATE TABLE image_jobs (
    id           BIGSERIAL PRIMARY KEY,
    token        TEXT NOT NULL UNIQUE,
    user_id      BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    protocol     TEXT NOT NULL,
    kind         TEXT NOT NULL,
    used_shared  INT NOT NULL DEFAULT 0,
    status       TEXT NOT NULL DEFAULT 'pending',
    result_json  TEXT,
    error        TEXT,
    created_at   TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS'),
    started_at   TEXT,
    finished_at  TEXT
);
CREATE INDEX idx_image_jobs_user ON image_jobs(user_id, created_at DESC);
CREATE INDEX idx_image_jobs_status ON image_jobs(status, created_at DESC);
