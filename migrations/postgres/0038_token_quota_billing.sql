-- Replace the per-call credit system with a token-metered quota balance.
--
-- Quota is the site's own unit: an integer shown directly in the UI with no
-- currency symbol. Model prices are entered against the provider's official
-- USD rate and stored as micro-USD (1 USD = 1_000_000), so the admin copies
-- published pricing verbatim instead of inventing per-model credit values.
--
-- Conversion of existing data keeps every account's purchasing power:
--   * 1 credit            -> 500 quota          (old recharge rate: 100 credits/CNY)
--   * 1 credit of price   -> 1000 micro-USD     (500 quota / 500000 quota-per-USD)

ALTER TABLE user_credits RENAME TO user_balances;
ALTER TABLE credit_ledger RENAME TO balance_ledger;

-- Token counts backing each chat charge; zero for per-call and non-usage rows.
ALTER TABLE balance_ledger ADD COLUMN input_tokens  BIGINT NOT NULL DEFAULT 0;
ALTER TABLE balance_ledger ADD COLUMN output_tokens BIGINT NOT NULL DEFAULT 0;
ALTER TABLE balance_ledger ADD COLUMN cached_tokens BIGINT NOT NULL DEFAULT 0;

UPDATE user_balances SET balance = balance * 500, lifetime_used = lifetime_used * 500;
UPDATE balance_ledger SET delta = delta * 500;

-- model_pricing: per-call credits become micro-USD prices, plus token rates.
ALTER TABLE model_pricing RENAME COLUMN cost_credits TO per_call_price;
ALTER TABLE model_pricing RENAME COLUMN base_credits TO base_price;
ALTER TABLE model_pricing RENAME COLUMN per_second   TO per_second_price;

-- micro-USD per 1M tokens, matching how providers publish their rates.
ALTER TABLE model_pricing ADD COLUMN input_price        BIGINT NOT NULL DEFAULT 0;
ALTER TABLE model_pricing ADD COLUMN output_price       BIGINT NOT NULL DEFAULT 0;
-- NULL means "cached input billed at the normal input rate".
ALTER TABLE model_pricing ADD COLUMN cached_input_price BIGINT NULL;

UPDATE model_pricing
   SET per_call_price   = per_call_price * 1000,
       base_price       = base_price * 1000,
       per_second_price = per_second_price * 1000;

ALTER TABLE video_jobs RENAME COLUMN cost_credits TO cost_quota;
UPDATE video_jobs SET cost_quota = cost_quota * 500;

ALTER TABLE payment_orders RENAME COLUMN credits TO quota;
UPDATE payment_orders SET quota = quota * 500;

-- Quota per USD of upstream spend, and a global markup applied to every model.
INSERT INTO app_settings (k, v) VALUES ('quota_per_usd', '500000');
INSERT INTO app_settings (k, v) VALUES ('price_multiplier_percent', '100');

UPDATE app_settings
   SET v = CAST(CAST(v AS BIGINT) * 500 AS TEXT)
 WHERE k IN ('signup_grant', 'invite_grant_inviter', 'invite_grant_invitee', 'epay_credits_per_yuan');
UPDATE app_settings SET k = 'epay_quota_per_yuan' WHERE k = 'epay_credits_per_yuan';
UPDATE app_settings SET v = 'Yunova 额度充值' WHERE k = 'epay_product_name' AND v = 'Yunova 积分充值';

-- Per-call chat/image costs are now per-model, not global.
DELETE FROM app_settings WHERE k IN ('cost_chat', 'cost_image');
