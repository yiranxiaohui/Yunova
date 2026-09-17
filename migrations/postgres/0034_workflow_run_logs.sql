CREATE TABLE workflow_run_logs (
    id          BIGSERIAL PRIMARY KEY,
    run_id      BIGINT NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
    node_key    TEXT,
    level       TEXT NOT NULL DEFAULT 'info',
    message     TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS')
);
CREATE INDEX idx_workflow_run_logs_run ON workflow_run_logs(run_id, id);
