ALTER TABLE users ADD COLUMN is_admin INT NOT NULL DEFAULT 0;
UPDATE users SET is_admin = 1 WHERE id = (SELECT MIN(id) FROM users);
CREATE INDEX idx_users_is_admin ON users(is_admin) WHERE is_admin = 1;
