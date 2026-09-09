-- Update only the former defaults, preserving administrator customization.
UPDATE app_settings SET `v` = 'Yunova' WHERE `k` = 'smtp_from_name' AND BINARY `v` = 'NovaChat';
UPDATE app_settings SET `v` = 'Yunova 积分充值' WHERE `k` = 'epay_product_name' AND BINARY `v` = 'NovaChat 积分充值';
