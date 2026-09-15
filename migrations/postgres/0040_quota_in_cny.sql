-- Denominate quota in CNY: 1 quota is 1 yuan.
--
-- The previous scheme priced quota at `quota_per_usd` points per USD (default
-- 500000) and recharged at `epay_quota_per_yuan` points per yuan (50000), so a
-- point was 1/50000 yuan — a scale users had to mentally convert. Quota now
-- reads as money instead.
--
-- A single chat call can cost a fraction of a fen, so quota is stored in
-- micro-quota (1 quota = 1_000_000) rather than as whole yuan; an integer
-- balance in yuan would round every small charge to nothing.
--
-- Conversion preserves value exactly: the old recharge rate made 50000 points
-- one yuan, and one yuan is 1_000_000 micro-quota, so one old point becomes 20
-- micro-quota.

UPDATE user_balances
   SET balance       = balance * 20,
       lifetime_used = lifetime_used * 20;

UPDATE balance_ledger SET delta = delta * 20;

UPDATE payment_orders SET quota = quota * 20;

UPDATE video_jobs SET cost_quota = cost_quota * 20;

UPDATE app_settings
   SET v = CAST(CAST(v AS BIGINT) * 20 AS TEXT)
 WHERE k IN ('signup_grant', 'invite_grant_inviter', 'invite_grant_invitee');

-- Model prices stay in micro-USD; only the USD→quota conversion changes.
-- The old pair (points-per-USD / points-per-yuan) collapses into one exchange
-- rate, stored as micro-CNY per USD so a rate like 7.2 stays exact.
INSERT INTO app_settings (k, v) VALUES ('usd_to_cny_rate_micro', '10000000');

UPDATE app_settings
   SET v = CAST(
             CAST((SELECT v FROM app_settings WHERE k = 'quota_per_usd') AS BIGINT)
             * 1000000
             / CAST((SELECT v FROM app_settings WHERE k = 'epay_quota_per_yuan') AS BIGINT)
           AS TEXT)
 WHERE k = 'usd_to_cny_rate_micro'
   AND EXISTS (SELECT 1 FROM app_settings WHERE k = 'quota_per_usd')
   AND EXISTS (
         SELECT 1 FROM app_settings
          WHERE k = 'epay_quota_per_yuan'
            AND CAST(v AS BIGINT) > 0
       );

-- Recharge is now the identity 1 yuan = 1 quota, so the rate is obsolete.
DELETE FROM app_settings WHERE k IN ('quota_per_usd', 'epay_quota_per_yuan');
