-- Sell upstream spend at parity: ¥1 of quota buys $1 of upstream cost.
--
-- The upstream this site resells from prices its own quota at 1 yuan per USD,
-- so a 1:1 rate is what actually reflects what an operator pays. Migration 40
-- could not know that: it mechanically folded the two obsolete point-scale
-- knobs into an exchange rate, and the old defaults (500000 points per USD,
-- 50000 points per yuan) produced 10 CNY per USD. Every model then billed ten
-- times its upstream cost.
--
-- Only the value migration 40 wrote by default is rewritten. A rate the
-- operator has since tuned by hand — or one derived from their own non-default
-- point settings — is a deliberate business decision and is left alone; the
-- rate stays editable in the admin console either way.

UPDATE app_settings
   SET v = '1000000'
 WHERE k = 'usd_to_cny_rate_micro'
   AND v = '10000000';
