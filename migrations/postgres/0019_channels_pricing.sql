-- Multi-channel + per-model pricing
-- See: docs/plans/2026-05-18-multi-channel-pricing.md

CREATE TABLE upstream_channels (
    id          BIGSERIAL    PRIMARY KEY,
    name        TEXT         NOT NULL UNIQUE,
    protocol    TEXT         NOT NULL,                       -- 'openai' | 'claude' | 'gemini'
    kind        TEXT         NOT NULL,                       -- 'chat'   | 'image'
    base_url    TEXT         NOT NULL,
    api_key     TEXT         NOT NULL,
    enabled     INT      NOT NULL DEFAULT 1,
    priority    INTEGER      NOT NULL DEFAULT 100,
    created_at  TEXT  NOT NULL DEFAULT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS'),
    updated_at  TEXT  NOT NULL DEFAULT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS')
);
CREATE INDEX idx_upstream_channels_enabled ON upstream_channels(enabled, priority);

CREATE TABLE model_pricing (
    id           BIGSERIAL    PRIMARY KEY,
    model        TEXT         NOT NULL UNIQUE,
    kind         TEXT         NOT NULL,
    cost_credits BIGINT       NOT NULL,
    display_name TEXT,
    enabled      INT      NOT NULL DEFAULT 1,
    created_at   TEXT  NOT NULL DEFAULT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS'),
    updated_at   TEXT  NOT NULL DEFAULT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS')
);
CREATE INDEX idx_model_pricing_enabled ON model_pricing(enabled, kind);

CREATE TABLE channel_models (
    channel_id  BIGINT NOT NULL REFERENCES upstream_channels(id) ON DELETE CASCADE,
    model       TEXT   NOT NULL,
    upstream_id TEXT,
    PRIMARY KEY (channel_id, model)
);
CREATE INDEX idx_channel_models_model ON channel_models(model);
