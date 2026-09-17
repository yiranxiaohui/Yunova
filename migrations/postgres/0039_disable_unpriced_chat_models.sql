-- Disable chat models the quota migration could not price.
--
-- Under the old per-call credit scheme a chat model carried a single
-- "N credits per call" figure. Token billing needs separate input and output
-- rates, and that split cannot be derived from one number — migration 0038
-- therefore moved the old value into `per_call_price`, which the chat billing
-- path never reads. Left enabled, every such model would serve traffic for
-- free.
--
-- Disabling them makes the gap visible instead: an admin re-prices the model
-- (by hand or via the NewAPI import) and re-enables it. The original value
-- stays in `per_call_price` as a reference point for what the model used to
-- cost. Rows that already carry token rates are untouched, so re-running this
-- after a proper import cannot disable a correctly priced model.

UPDATE model_pricing
   SET enabled = 0
 WHERE kind = 'chat'
   AND input_price = 0
   AND output_price = 0;
