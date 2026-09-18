-- Per-task reasoning level for work-mode sessions. See the SQLite copy of this
-- migration for why the level belongs on the session row and why NULL keeps
-- meaning "the runtime's own default".
ALTER TABLE agent_sessions ADD COLUMN IF NOT EXISTS thinking_level TEXT;
