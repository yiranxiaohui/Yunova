-- Stable per-machine identity for a device, so signing in can replace pairing.
--
-- Pairing existed only to name a machine the server had never seen. Once the
-- desktop client authenticates with the user's own account, the account
-- answers "who is this", and all that is left is "which of my computers" —
-- which is what this column answers.
--
-- The value is derived on the client from the machine (hostname, OS, user)
-- and is not a secret: it decides *which row to reuse*, never whether a
-- connection is allowed. Authorisation stays with `token_hash`, minted only
-- after a password login, so a guessed fingerprint grants nothing.
--
-- Nullable because existing rows were created by pairing and have no
-- fingerprint; they keep working and simply cannot be matched by machine.
-- Unique per user rather than globally: two users on one shared computer are
-- two separate devices.
ALTER TABLE agent_devices ADD COLUMN fingerprint TEXT;
CREATE UNIQUE INDEX idx_agent_devices_fingerprint
    ON agent_devices(user_id, fingerprint);
