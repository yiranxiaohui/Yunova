-- Per-task working directory for device sessions. See the SQLite copy of this
-- migration for why the directory belongs on the session row and why the
-- stored value is a record of the user's choice rather than an instruction the
-- client obeys blindly.
ALTER TABLE agent_sessions ADD COLUMN IF NOT EXISTS workspace TEXT;
