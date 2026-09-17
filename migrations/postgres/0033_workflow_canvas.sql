CREATE TABLE workflows (
    id          BIGSERIAL PRIMARY KEY,
    user_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    graph_json  TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS'),
    updated_at  TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS')
);
CREATE INDEX idx_workflows_user_updated ON workflows(user_id, updated_at DESC);

CREATE TABLE workflow_runs (
    id           BIGSERIAL PRIMARY KEY,
    token        TEXT NOT NULL UNIQUE,
    workflow_id  BIGINT REFERENCES workflows(id) ON DELETE SET NULL,
    user_id      BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    graph_json   TEXT NOT NULL,
    status       TEXT NOT NULL DEFAULT 'running',
    error        TEXT,
    created_at   TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS'),
    finished_at  TEXT
);
CREATE INDEX idx_workflow_runs_user_created ON workflow_runs(user_id, created_at DESC);
CREATE INDEX idx_workflow_runs_status ON workflow_runs(status, created_at DESC);

CREATE TABLE workflow_node_runs (
    id            BIGSERIAL PRIMARY KEY,
    run_id        BIGINT NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
    node_key      TEXT NOT NULL,
    node_type     TEXT NOT NULL,
    status        TEXT NOT NULL DEFAULT 'waiting',
    job_token     TEXT,
    output_paths  TEXT,
    error         TEXT,
    created_at    TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS'),
    started_at    TEXT,
    finished_at   TEXT,
    UNIQUE(run_id, node_key)
);
CREATE INDEX idx_workflow_node_runs_run ON workflow_node_runs(run_id, id);
CREATE INDEX idx_workflow_node_runs_status ON workflow_node_runs(status, created_at);

CREATE TABLE media_assets (
    id                   BIGSERIAL PRIMARY KEY,
    user_id              BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    workflow_run_id      BIGINT REFERENCES workflow_runs(id) ON DELETE SET NULL,
    workflow_node_run_id BIGINT REFERENCES workflow_node_runs(id) ON DELETE SET NULL,
    kind                 TEXT NOT NULL,
    path                 TEXT NOT NULL,
    metadata_json        TEXT,
    created_at           TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS')
);
CREATE INDEX idx_media_assets_user_created ON media_assets(user_id, created_at DESC);
CREATE INDEX idx_media_assets_node ON media_assets(workflow_node_run_id);
