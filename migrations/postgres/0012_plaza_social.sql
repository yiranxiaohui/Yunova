ALTER TABLE plaza_images ADD COLUMN like_count    BIGINT NOT NULL DEFAULT 0;
ALTER TABLE plaza_images ADD COLUMN comment_count BIGINT NOT NULL DEFAULT 0;

CREATE TABLE plaza_image_likes (
    image_id   BIGINT NOT NULL REFERENCES plaza_images(id) ON DELETE CASCADE,
    user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS'),
    PRIMARY KEY (image_id, user_id)
);
CREATE INDEX idx_plaza_image_likes_user ON plaza_image_likes(user_id, created_at DESC);

CREATE TABLE plaza_image_comments (
    id         BIGSERIAL PRIMARY KEY,
    image_id   BIGINT NOT NULL REFERENCES plaza_images(id) ON DELETE CASCADE,
    user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    content    TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS')
);
CREATE INDEX idx_plaza_image_comments_image ON plaza_image_comments(image_id, created_at DESC);
