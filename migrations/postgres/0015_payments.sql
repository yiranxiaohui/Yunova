CREATE TABLE payment_orders (
    id             BIGSERIAL PRIMARY KEY,
    user_id        BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    out_trade_no   TEXT NOT NULL UNIQUE,
    provider       TEXT NOT NULL DEFAULT 'epay',
    payway         TEXT NOT NULL,
    amount_cents   BIGINT NOT NULL,
    credits        BIGINT NOT NULL,
    status         TEXT NOT NULL DEFAULT 'pending',
    trade_no       TEXT,
    client_ip      TEXT,
    created_at     TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS'),
    paid_at        TEXT,
    updated_at     TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS')
);
CREATE INDEX idx_payment_orders_user ON payment_orders(user_id, created_at DESC);
CREATE INDEX idx_payment_orders_status ON payment_orders(status, created_at DESC);

INSERT INTO app_settings (k, v) VALUES ('epay_enabled', 'false');
INSERT INTO app_settings (k, v) VALUES ('epay_api_url', '');
INSERT INTO app_settings (k, v) VALUES ('epay_pid', '');
INSERT INTO app_settings (k, v) VALUES ('epay_key', '');
INSERT INTO app_settings (k, v) VALUES ('epay_sign_type', 'MD5');
INSERT INTO app_settings (k, v) VALUES ('epay_credits_per_yuan', '100');
INSERT INTO app_settings (k, v) VALUES ('epay_product_name', 'NovaChat 积分充值');
INSERT INTO app_settings (k, v) VALUES ('epay_min_yuan', '1');
INSERT INTO app_settings (k, v) VALUES ('epay_max_yuan', '5000');
INSERT INTO app_settings (k, v) VALUES ('epay_return_url', '');
INSERT INTO app_settings (k, v) VALUES ('epay_notify_url', '');
