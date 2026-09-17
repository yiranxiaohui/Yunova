CREATE TABLE plaza_images (
    id             BIGSERIAL PRIMARY KEY,
    user_id        BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    filename       TEXT NOT NULL,
    prompt         TEXT NOT NULL,
    revised_prompt TEXT,
    clone_count    BIGINT NOT NULL DEFAULT 0,
    created_at     TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS')
);
CREATE INDEX idx_plaza_images_user ON plaza_images(user_id, created_at DESC);
CREATE INDEX idx_plaza_images_hot  ON plaza_images(clone_count DESC, created_at DESC);
CREATE UNIQUE INDEX idx_plaza_images_filename ON plaza_images(filename);
