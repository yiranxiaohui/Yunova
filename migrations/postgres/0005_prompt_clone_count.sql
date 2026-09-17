ALTER TABLE prompts ADD COLUMN clone_count BIGINT NOT NULL DEFAULT 0;
CREATE INDEX idx_prompts_public_clones
    ON prompts(is_public, clone_count DESC, created_at DESC)
    WHERE is_public = 1;
