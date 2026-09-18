-- Per-task approval policy for work-mode sessions. See the SQLite copy of this
-- migration for why the choice belongs on the session row and why it can only
-- tighten the execution target's own policy, never loosen it.
ALTER TABLE agent_sessions ADD COLUMN IF NOT EXISTS approval TEXT;
