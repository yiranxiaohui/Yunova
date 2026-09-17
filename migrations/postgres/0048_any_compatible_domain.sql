-- Convert native `timestamptz` / `boolean` columns to the text/integer domain
-- the `sqlx::Any` pool can actually decode.
--
-- Migrations 1–47 originally declared `TIMESTAMPTZ` and `BOOLEAN`. Neither is
-- in the `Any` driver's type table, and the driver rejects the *value*, not
-- the query — so one such column made every row of its table unreadable, and
-- this backend could be installed but never used. Those files now declare
-- TEXT and INT, which is what a fresh install gets.
--
-- This migration is for a database created before that fix. It is driven off
-- `information_schema` rather than a fixed column list, so it converts exactly
-- what is actually native in this database and is a no-op on a fresh install
-- (where nothing matches) as well as on a second run.
--
-- The text format and the UTC normalization are chosen to match SQLite's
-- `datetime('now')` output byte for byte, so existing timestamps stay
-- comparable with rows written after the switch.

-- Partial indexes whose predicate is a boolean test (`WHERE is_public = true`,
-- `WHERE is_admin`) have to go first: `ALTER COLUMN ... TYPE INT` rewrites
-- them, and Postgres re-checks the predicate against the new type, failing
-- with `operator does not exist: integer = boolean`. They are recreated
-- against the integer domain at the end.
DROP INDEX IF EXISTS idx_users_is_admin;
DROP INDEX IF EXISTS idx_prompts_public;
DROP INDEX IF EXISTS idx_prompts_public_clones;
DROP INDEX IF EXISTS idx_skills_public_clones;

DO $migrate_any_domain$
DECLARE
    col RECORD;
BEGIN
    FOR col IN
        SELECT c.table_name AS tbl, c.column_name AS col, c.column_default AS def
        FROM information_schema.columns c
        JOIN information_schema.tables t
          ON t.table_schema = c.table_schema
         AND t.table_name = c.table_name
        WHERE c.table_schema = current_schema()
          AND t.table_type = 'BASE TABLE'
          AND c.data_type = 'timestamp with time zone'
    LOOP
        -- The default has to go first: it is a timestamptz expression and
        -- would not survive the type change.
        EXECUTE format('ALTER TABLE %I ALTER COLUMN %I DROP DEFAULT', col.tbl, col.col);
        EXECUTE format(
            'ALTER TABLE %I ALTER COLUMN %I TYPE TEXT '
            'USING to_char(%I AT TIME ZONE ''UTC'', ''YYYY-MM-DD HH24:MI:SS'')',
            col.tbl, col.col, col.col);
        -- Only columns that had a default get one back; a nullable
        -- `finished_at` must stay defaultless.
        IF col.def IS NOT NULL THEN
            EXECUTE format(
                'ALTER TABLE %I ALTER COLUMN %I SET DEFAULT '
                'to_char(now() AT TIME ZONE ''UTC'', ''YYYY-MM-DD HH24:MI:SS'')',
                col.tbl, col.col);
        END IF;
    END LOOP;

    FOR col IN
        SELECT c.table_name AS tbl, c.column_name AS col, c.column_default AS def
        FROM information_schema.columns c
        JOIN information_schema.tables t
          ON t.table_schema = c.table_schema
         AND t.table_name = c.table_name
        WHERE c.table_schema = current_schema()
          AND t.table_type = 'BASE TABLE'
          AND c.data_type = 'boolean'
    LOOP
        EXECUTE format('ALTER TABLE %I ALTER COLUMN %I DROP DEFAULT', col.tbl, col.col);
        -- NULL must stay NULL: a nullable flag like `image_use_proxy` treats
        -- NULL as "unset", which the app reads as its own default. Folding it
        -- into 0 would silently turn that setting off. `CASE WHEN col` alone
        -- sends NULL down the ELSE branch, hence the explicit guard.
        EXECUTE format(
            'ALTER TABLE %I ALTER COLUMN %I TYPE INT '
            'USING (CASE WHEN %I IS NULL THEN NULL WHEN %I THEN 1 ELSE 0 END)',
            col.tbl, col.col, col.col, col.col);
        -- `true`/`false` defaults become 1/0; anything else is left off
        -- rather than guessed at.
        IF col.def LIKE 'true%' THEN
            EXECUTE format('ALTER TABLE %I ALTER COLUMN %I SET DEFAULT 1', col.tbl, col.col);
        ELSIF col.def LIKE 'false%' THEN
            EXECUTE format('ALTER TABLE %I ALTER COLUMN %I SET DEFAULT 0', col.tbl, col.col);
        END IF;
    END LOOP;
END
$migrate_any_domain$;

-- Recreated now that the columns are integers.
CREATE INDEX idx_users_is_admin ON users(is_admin) WHERE is_admin = 1;

CREATE INDEX idx_prompts_public
    ON prompts(is_public, created_at DESC)
    WHERE is_public = 1;

CREATE INDEX idx_prompts_public_clones
    ON prompts(is_public, clone_count DESC, created_at DESC)
    WHERE is_public = 1;

CREATE INDEX idx_skills_public_clones
    ON skills(is_public, clone_count DESC, created_at DESC)
    WHERE is_public = 1;
