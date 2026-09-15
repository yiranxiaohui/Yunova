-- Persist assistant reasoning so it survives a page refresh.
ALTER TABLE messages ADD COLUMN reasoning MEDIUMTEXT NULL;
ALTER TABLE messages ADD COLUMN reasoning_ms BIGINT NULL;
