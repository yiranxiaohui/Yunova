ALTER TABLE users ADD COLUMN email TEXT;
CREATE UNIQUE INDEX idx_users_email ON users(email) WHERE email IS NOT NULL;

CREATE TABLE email_codes (
    id         BIGSERIAL PRIMARY KEY,
    email      TEXT NOT NULL,
    code_hash  TEXT NOT NULL,
    purpose    TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    used       INT NOT NULL DEFAULT 0,
    attempts   INT NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS')
);
CREATE INDEX idx_email_codes_email_purpose ON email_codes(email, purpose, created_at DESC);

INSERT INTO app_settings (k, v) VALUES ('email_verification_required', 'false');
INSERT INTO app_settings (k, v) VALUES ('smtp_host', '');
INSERT INTO app_settings (k, v) VALUES ('smtp_port', '587');
INSERT INTO app_settings (k, v) VALUES ('smtp_username', '');
INSERT INTO app_settings (k, v) VALUES ('smtp_password', '');
INSERT INTO app_settings (k, v) VALUES ('smtp_from_email', '');
INSERT INTO app_settings (k, v) VALUES ('smtp_from_name', 'NovaChat');
INSERT INTO app_settings (k, v) VALUES ('smtp_security', 'starttls');
