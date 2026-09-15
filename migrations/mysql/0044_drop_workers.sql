-- Remove the legacy worker (工蜂) feature. Agent tasks (`agent_*` tables) now
-- own remote execution, so these tables have no reader left. Fresh installs
-- never create them — migrations 26–28 were deleted along with the feature —
-- hence `IF EXISTS`.
DROP TABLE IF EXISTS worker_messages;
DROP TABLE IF EXISTS worker_sessions;
DROP TABLE IF EXISTS workers;
