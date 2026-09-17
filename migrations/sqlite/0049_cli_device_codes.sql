-- Device-code login, so a CLI can sign in without ever seeing a password.
--
-- The CLI runs on a machine that may have no browser and must not be trusted
-- with account credentials: it is the same threat model as the desktop client
-- (the token lands on the user's own disk), except there is no window to read
-- a session cookie out of. So the CLI starts a code, the user approves it in a
-- browser they already trust, and only then does an agent token exist.
--
-- Two codes, deliberately asymmetric:
--
-- * `user_code` is short and typed by a human, so it is the thing shown on
--   screen. It is low-entropy by necessity, which is why approving it needs an
--   authenticated browser session and why it expires in minutes.
-- * `device_code` is high-entropy and polled by the CLI. Only its SHA-256 hash
--   is stored, because a leaked database must not let someone else collect a
--   token for a pending approval.
--
-- The minted token is stored hashed in `agent_tokens` as usual; `token_hash`
-- here only records *which* token an approval produced, so revoking the
-- approval later can find it. The plaintext is handed to the CLI exactly once,
-- on the poll that observes the approval, and `consumed_at` makes that once
-- literal: a second poll with the same device code gets nothing.
CREATE TABLE cli_device_codes (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    -- NULL until approved: the row exists before anyone has claimed it.
    user_id      INTEGER REFERENCES users(id) ON DELETE CASCADE,
    user_code    TEXT NOT NULL UNIQUE,
    device_hash  TEXT NOT NULL UNIQUE,
    -- What the user is approving, shown on the confirmation page so the
    -- decision is informed rather than a bare yes/no.
    client_name  TEXT NOT NULL,
    hostname     TEXT,
    platform     TEXT,
    created_at   TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at   TEXT NOT NULL,
    approved_at  TEXT,
    denied_at    TEXT,
    consumed_at  TEXT,
    -- Hash of the agent token this approval minted, so revoking is possible
    -- without storing the plaintext anywhere.
    token_hash   TEXT
);
CREATE INDEX idx_cli_device_codes_expiry ON cli_device_codes(expires_at);
