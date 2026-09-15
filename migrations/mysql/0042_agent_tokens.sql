-- Agent access tokens: bearer credentials that let an agent runtime outside
-- the browser (pi on a desktop client, a cloud sandbox container) reach the
-- platform chat gateway. Session cookies can't be used there, and handing out
-- the admin's upstream channel keys would bypass the whitelist and metering.
--
-- Only the SHA-256 hash of the token is stored. `prefix` keeps the leading
-- characters so the UI can still identify a token it can no longer display.
CREATE TABLE agent_tokens (
    id           BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,
    user_id      BIGINT NOT NULL,
    name         VARCHAR(160) NOT NULL,
    token_hash   VARCHAR(128) NOT NULL UNIQUE,
    prefix       VARCHAR(32) NOT NULL,
    created_at   DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    last_used_at DATETIME(3),
    revoked      TINYINT(1) NOT NULL DEFAULT 0,
    CONSTRAINT fk_agent_tokens_user FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
    INDEX idx_agent_tokens_user_created (user_id, created_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
