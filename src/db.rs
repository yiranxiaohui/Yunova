use std::str::FromStr;

use sqlx::any::{AnyConnectOptions, AnyPoolOptions};
use sqlx::{AnyPool, Executor};

pub type Pool = AnyPool;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DbKind {
    Sqlite,
    Mysql,
    Postgres,
}

impl DbKind {
    pub fn from_url(url: &str) -> Option<Self> {
        let u = url.trim().to_ascii_lowercase();
        if u.starts_with("sqlite:") {
            Some(Self::Sqlite)
        } else if u.starts_with("mysql:") || u.starts_with("mariadb:") {
            Some(Self::Mysql)
        } else if u.starts_with("postgres:") || u.starts_with("postgresql:") {
            Some(Self::Postgres)
        } else {
            None
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sqlite => "sqlite",
            Self::Mysql => "mysql",
            Self::Postgres => "postgres",
        }
    }
}

pub fn install_drivers() {
    sqlx::any::install_default_drivers();
}

pub async fn connect(url: &str) -> Result<Pool, sqlx::Error> {
    let normalized = normalize_url(url);
    let options = AnyConnectOptions::from_str(&normalized)?;
    AnyPoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await
}

fn normalize_url(url: &str) -> String {
    // SQLite: ensure create-if-missing via URL query (`mode=rwc`).
    if url.starts_with("sqlite:") && !url.contains("mode=") {
        return if url.contains('?') {
            format!("{url}&mode=rwc")
        } else {
            format!("{url}?mode=rwc")
        };
    }
    url.to_string()
}

// ---------------------------------------------------------------------------
// migrations
// ---------------------------------------------------------------------------

static SQLITE_MIGRATIONS: &[(i32, &str)] = &[
    (1, include_str!("../migrations/sqlite/0001_init.sql")),
    (2, include_str!("../migrations/sqlite/0002_user_settings.sql")),
    (3, include_str!("../migrations/sqlite/0003_image_settings.sql")),
    (4, include_str!("../migrations/sqlite/0004_image_protocol.sql")),
    (5, include_str!("../migrations/sqlite/0005_prompt_clone_count.sql")),
    (6, include_str!("../migrations/sqlite/0006_skills.sql")),
    (7, include_str!("../migrations/sqlite/0007_skills_public.sql")),
    (8, include_str!("../migrations/sqlite/0008_user_profile.sql")),
    (9, include_str!("../migrations/sqlite/0009_plaza_images.sql")),
    (10, include_str!("../migrations/sqlite/0010_admin.sql")),
    (11, include_str!("../migrations/sqlite/0011_credits.sql")),
    (12, include_str!("../migrations/sqlite/0012_plaza_social.sql")),
    (13, include_str!("../migrations/sqlite/0013_invites.sql")),
    (14, include_str!("../migrations/sqlite/0014_email.sql")),
    (15, include_str!("../migrations/sqlite/0015_payments.sql")),
    (16, include_str!("../migrations/sqlite/0016_image_jobs.sql")),
    (17, include_str!("../migrations/sqlite/0017_image_studio.sql")),
    (18, include_str!("../migrations/sqlite/0018_studio_generations.sql")),
    (19, include_str!("../migrations/sqlite/0019_channels_pricing.sql")),
    (20, include_str!("../migrations/sqlite/0020_payment_trade_no_unique.sql")),
    (21, include_str!("../migrations/sqlite/0021_studio_extra_params.sql")),
    (22, include_str!("../migrations/sqlite/0022_credit_ledger_meta.sql")),
    (23, include_str!("../migrations/sqlite/0023_shared_conversations.sql")),
    (24, include_str!("../migrations/sqlite/0024_model_pricing_protocol.sql")),
    (25, include_str!("../migrations/sqlite/0025_studio_source_paths.sql")),
    (26, include_str!("../migrations/sqlite/0026_workers.sql")),
    (27, include_str!("../migrations/sqlite/0027_worker_sessions.sql")),
    (28, include_str!("../migrations/sqlite/0028_worker_messages.sql")),
    (29, include_str!("../migrations/sqlite/0029_model_pricing_context.sql")),
    (30, include_str!("../migrations/sqlite/0030_video_generation.sql")),
    (31, include_str!("../migrations/sqlite/0031_unify_video_pricing.sql")),
    (32, include_str!("../migrations/sqlite/0032_drop_channel_kind.sql")),
    (33, include_str!("../migrations/sqlite/0033_workflow_canvas.sql")),
    (34, include_str!("../migrations/sqlite/0034_workflow_run_logs.sql")),
    (35, include_str!("../migrations/sqlite/0035_video_editor.sql")),
    (36, include_str!("../migrations/sqlite/0036_unify_media_library.sql")),
    (37, include_str!("../migrations/sqlite/0037_yunova_brand.sql")),
    (38, include_str!("../migrations/sqlite/0038_token_quota_billing.sql")),
    (39, include_str!("../migrations/sqlite/0039_disable_unpriced_chat_models.sql")),
    (40, include_str!("../migrations/sqlite/0040_quota_in_cny.sql")),
    (41, include_str!("../migrations/sqlite/0041_message_reasoning.sql")),
    (42, include_str!("../migrations/sqlite/0042_agent_tokens.sql")),
];
static MYSQL_MIGRATIONS: &[(i32, &str)] = &[
    (1, include_str!("../migrations/mysql/0001_init.sql")),
    (2, include_str!("../migrations/mysql/0002_user_settings.sql")),
    (3, include_str!("../migrations/mysql/0003_image_settings.sql")),
    (4, include_str!("../migrations/mysql/0004_image_protocol.sql")),
    (5, include_str!("../migrations/mysql/0005_prompt_clone_count.sql")),
    (6, include_str!("../migrations/mysql/0006_skills.sql")),
    (7, include_str!("../migrations/mysql/0007_skills_public.sql")),
    (8, include_str!("../migrations/mysql/0008_user_profile.sql")),
    (9, include_str!("../migrations/mysql/0009_plaza_images.sql")),
    (10, include_str!("../migrations/mysql/0010_admin.sql")),
    (11, include_str!("../migrations/mysql/0011_credits.sql")),
    (12, include_str!("../migrations/mysql/0012_plaza_social.sql")),
    (13, include_str!("../migrations/mysql/0013_invites.sql")),
    (14, include_str!("../migrations/mysql/0014_email.sql")),
    (15, include_str!("../migrations/mysql/0015_payments.sql")),
    (16, include_str!("../migrations/mysql/0016_image_jobs.sql")),
    (17, include_str!("../migrations/mysql/0017_image_studio.sql")),
    (18, include_str!("../migrations/mysql/0018_studio_generations.sql")),
    (19, include_str!("../migrations/mysql/0019_channels_pricing.sql")),
    (20, include_str!("../migrations/mysql/0020_payment_trade_no_unique.sql")),
    (21, include_str!("../migrations/mysql/0021_studio_extra_params.sql")),
    (22, include_str!("../migrations/mysql/0022_credit_ledger_meta.sql")),
    (23, include_str!("../migrations/mysql/0023_shared_conversations.sql")),
    (24, include_str!("../migrations/mysql/0024_model_pricing_protocol.sql")),
    (25, include_str!("../migrations/mysql/0025_studio_source_paths.sql")),
    (26, include_str!("../migrations/mysql/0026_workers.sql")),
    (27, include_str!("../migrations/mysql/0027_worker_sessions.sql")),
    (28, include_str!("../migrations/mysql/0028_worker_messages.sql")),
    (29, include_str!("../migrations/mysql/0029_model_pricing_context.sql")),
    (30, include_str!("../migrations/mysql/0030_video_generation.sql")),
    (31, include_str!("../migrations/mysql/0031_unify_video_pricing.sql")),
    (32, include_str!("../migrations/mysql/0032_drop_channel_kind.sql")),
    (33, include_str!("../migrations/mysql/0033_workflow_canvas.sql")),
    (34, include_str!("../migrations/mysql/0034_workflow_run_logs.sql")),
    (35, include_str!("../migrations/mysql/0035_video_editor.sql")),
    (36, include_str!("../migrations/mysql/0036_unify_media_library.sql")),
    (37, include_str!("../migrations/mysql/0037_yunova_brand.sql")),
    (38, include_str!("../migrations/mysql/0038_token_quota_billing.sql")),
    (39, include_str!("../migrations/mysql/0039_disable_unpriced_chat_models.sql")),
    (40, include_str!("../migrations/mysql/0040_quota_in_cny.sql")),
    (41, include_str!("../migrations/mysql/0041_message_reasoning.sql")),
    (42, include_str!("../migrations/mysql/0042_agent_tokens.sql")),
];
static POSTGRES_MIGRATIONS: &[(i32, &str)] = &[
    (1, include_str!("../migrations/postgres/0001_init.sql")),
    (2, include_str!("../migrations/postgres/0002_user_settings.sql")),
    (3, include_str!("../migrations/postgres/0003_image_settings.sql")),
    (4, include_str!("../migrations/postgres/0004_image_protocol.sql")),
    (5, include_str!("../migrations/postgres/0005_prompt_clone_count.sql")),
    (6, include_str!("../migrations/postgres/0006_skills.sql")),
    (7, include_str!("../migrations/postgres/0007_skills_public.sql")),
    (8, include_str!("../migrations/postgres/0008_user_profile.sql")),
    (9, include_str!("../migrations/postgres/0009_plaza_images.sql")),
    (10, include_str!("../migrations/postgres/0010_admin.sql")),
    (11, include_str!("../migrations/postgres/0011_credits.sql")),
    (12, include_str!("../migrations/postgres/0012_plaza_social.sql")),
    (13, include_str!("../migrations/postgres/0013_invites.sql")),
    (14, include_str!("../migrations/postgres/0014_email.sql")),
    (15, include_str!("../migrations/postgres/0015_payments.sql")),
    (16, include_str!("../migrations/postgres/0016_image_jobs.sql")),
    (17, include_str!("../migrations/postgres/0017_image_studio.sql")),
    (18, include_str!("../migrations/postgres/0018_studio_generations.sql")),
    (19, include_str!("../migrations/postgres/0019_channels_pricing.sql")),
    (20, include_str!("../migrations/postgres/0020_payment_trade_no_unique.sql")),
    (21, include_str!("../migrations/postgres/0021_studio_extra_params.sql")),
    (22, include_str!("../migrations/postgres/0022_credit_ledger_meta.sql")),
    (23, include_str!("../migrations/postgres/0023_shared_conversations.sql")),
    (24, include_str!("../migrations/postgres/0024_model_pricing_protocol.sql")),
    (25, include_str!("../migrations/postgres/0025_studio_source_paths.sql")),
    (26, include_str!("../migrations/postgres/0026_workers.sql")),
    (27, include_str!("../migrations/postgres/0027_worker_sessions.sql")),
    (28, include_str!("../migrations/postgres/0028_worker_messages.sql")),
    (29, include_str!("../migrations/postgres/0029_model_pricing_context.sql")),
    (30, include_str!("../migrations/postgres/0030_video_generation.sql")),
    (31, include_str!("../migrations/postgres/0031_unify_video_pricing.sql")),
    (32, include_str!("../migrations/postgres/0032_drop_channel_kind.sql")),
    (33, include_str!("../migrations/postgres/0033_workflow_canvas.sql")),
    (34, include_str!("../migrations/postgres/0034_workflow_run_logs.sql")),
    (35, include_str!("../migrations/postgres/0035_video_editor.sql")),
    (36, include_str!("../migrations/postgres/0036_unify_media_library.sql")),
    (37, include_str!("../migrations/postgres/0037_yunova_brand.sql")),
    (38, include_str!("../migrations/postgres/0038_token_quota_billing.sql")),
    (39, include_str!("../migrations/postgres/0039_disable_unpriced_chat_models.sql")),
    (40, include_str!("../migrations/postgres/0040_quota_in_cny.sql")),
    (41, include_str!("../migrations/postgres/0041_message_reasoning.sql")),
    (42, include_str!("../migrations/postgres/0042_agent_tokens.sql")),
];

fn migrations_for(kind: DbKind) -> &'static [(i32, &'static str)] {
    match kind {
        DbKind::Sqlite => SQLITE_MIGRATIONS,
        DbKind::Mysql => MYSQL_MIGRATIONS,
        DbKind::Postgres => POSTGRES_MIGRATIONS,
    }
}

pub async fn migrate(pool: &Pool, kind: DbKind) -> Result<(), sqlx::Error> {
    let create_table = match kind {
        DbKind::Sqlite => {
            "CREATE TABLE IF NOT EXISTS _migrations (
                id INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            )"
        }
        DbKind::Mysql => {
            "CREATE TABLE IF NOT EXISTS _migrations (
                id INT NOT NULL PRIMARY KEY,
                applied_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3)
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4"
        }
        DbKind::Postgres => {
            "CREATE TABLE IF NOT EXISTS _migrations (
                id INT PRIMARY KEY,
                applied_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )"
        }
    };
    pool.execute(create_table).await?;

    let rows: Vec<(i32,)> = sqlx::query_as("SELECT id FROM _migrations")
        .fetch_all(pool)
        .await?;
    let applied: std::collections::HashSet<i32> = rows.into_iter().map(|(x,)| x).collect();

    for (id, body) in migrations_for(kind) {
        if applied.contains(id) {
            continue;
        }
        for stmt in split_sql(body) {
            let s = stmt.trim();
            if s.is_empty() {
                continue;
            }
            pool.execute(s).await?;
        }
        let record = q(kind, "INSERT INTO _migrations (id) VALUES (?)");
        sqlx::query(&record)
            .bind(id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// Splits a SQL blob into individual statements separated by `;` outside strings/comments.
/// Good enough for the migration files we ship (no dollar-quoting, no stored procs).
fn split_sql(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut chars = src.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;
    while let Some(c) = chars.next() {
        match c {
            '-' if !in_single && !in_double && chars.peek() == Some(&'-') => {
                // line comment
                while let Some(cc) = chars.next() {
                    if cc == '\n' {
                        buf.push('\n');
                        break;
                    }
                }
            }
            '\'' if !in_double => {
                // SQL '' escape: two single quotes inside a string is a literal
                // quote, not the end of the string.
                if in_single && chars.peek() == Some(&'\'') {
                    buf.push(c);
                    buf.push(chars.next().unwrap());
                } else {
                    in_single = !in_single;
                    buf.push(c);
                }
            }
            '"' if !in_single => {
                // Mirror "" escape for quoted identifiers (Postgres).
                if in_double && chars.peek() == Some(&'"') {
                    buf.push(c);
                    buf.push(chars.next().unwrap());
                } else {
                    in_double = !in_double;
                    buf.push(c);
                }
            }
            ';' if !in_single && !in_double => {
                out.push(std::mem::take(&mut buf));
            }
            _ => buf.push(c),
        }
    }
    if !buf.trim().is_empty() {
        out.push(buf);
    }
    out
}

// ---------------------------------------------------------------------------
// dialect-aware SQL helpers
// ---------------------------------------------------------------------------

/// `now()`-style default timestamp expression, for use in UPDATE statements.
pub fn now_expr(kind: DbKind) -> &'static str {
    match kind {
        DbKind::Sqlite => "datetime('now')",
        DbKind::Mysql => "CURRENT_TIMESTAMP(3)",
        DbKind::Postgres => "NOW()",
    }
}

/// Returns a fragment that appends "RETURNING id" on Postgres / SQLite
/// (SQLite 3.35+).  MySQL doesn't support it, so callers fall back to
/// `LAST_INSERT_ID()` via `last_insert_id()`.
pub fn returning_id(kind: DbKind) -> &'static str {
    match kind {
        DbKind::Postgres | DbKind::Sqlite => " RETURNING id",
        DbKind::Mysql => "",
    }
}

/// Rewrites `?` placeholders to `$1, $2, …` for Postgres.
pub fn q(kind: DbKind, sql: &str) -> String {
    match kind {
        DbKind::Postgres => {
            // `?` → `$N` rewrite, skipping placeholders inside SQL string
            // literals and quoted identifiers. Handles '' / "" escapes
            // properly so a literal quote inside a string/identifier doesn't
            // flip the quote-state and silently mis-rewrite later `?`s.
            let mut out = String::with_capacity(sql.len() + 8);
            let mut i = 0usize;
            let mut in_single = false; // inside '...' string literal
            let mut in_double = false; // inside "..." quoted identifier
            let mut chars = sql.chars().peekable();
            while let Some(c) = chars.next() {
                match c {
                    '\'' if !in_double => {
                        if in_single && chars.peek() == Some(&'\'') {
                            out.push(c);
                            out.push(chars.next().unwrap());
                            continue;
                        }
                        in_single = !in_single;
                    }
                    '"' if !in_single => {
                        if in_double && chars.peek() == Some(&'"') {
                            out.push(c);
                            out.push(chars.next().unwrap());
                            continue;
                        }
                        in_double = !in_double;
                    }
                    '?' if !in_single && !in_double => {
                        i += 1;
                        out.push('$');
                        out.push_str(&i.to_string());
                        continue;
                    }
                    _ => {}
                }
                out.push(c);
            }
            out
        }
        _ => sql.to_string(),
    }
}

/// Case-insensitive equality fragment for usernames: `LOWER(col) = LOWER(?)` on PG/MySQL,
/// direct `col = ?` on SQLite (handled by COLLATE NOCASE on the column).
pub fn ci_eq(kind: DbKind, col: &str) -> String {
    match kind {
        DbKind::Sqlite => format!("{col} = ?"),
        DbKind::Mysql | DbKind::Postgres => format!("LOWER({col}) = LOWER(?)"),
    }
}

/// Select a nullable bool-valued column as an optional integer across backends.
pub fn opt_bool_as_int(kind: DbKind, col: &str) -> String {
    match kind {
        DbKind::Postgres => format!(
            "CASE WHEN {col} IS NULL THEN NULL WHEN {col} THEN 1 ELSE 0 END AS {}",
            col_alias(col)
        ),
        _ => col.to_string(),
    }
}

/// Select a bool-valued column as an integer so Rust `i64` decoding works across backends.
/// SQLite/MySQL already store it as INTEGER/TINYINT; Postgres needs a cast from BOOLEAN.
pub fn bool_as_int(kind: DbKind, col: &str) -> String {
    match kind {
        DbKind::Postgres => format!("CASE WHEN {col} THEN 1 ELSE 0 END AS {}", col_alias(col)),
        _ => col.to_string(),
    }
}

fn col_alias(col: &str) -> String {
    // given "p.is_public" return "is_public"
    col.rsplit('.').next().unwrap_or(col).to_string()
}

/// Literal "true" value for the given dialect, usable inside WHERE clauses against a bool column.
pub fn bool_true(kind: DbKind) -> &'static str {
    match kind {
        DbKind::Postgres => "TRUE",
        _ => "1",
    }
}

/// Returns a SQL expression that truncates a timestamp column to a day string
/// like `2026-05-23`. Suitable for use in SELECT / GROUP BY across dialects.
pub fn day_bucket(kind: DbKind, col: &str) -> String {
    match kind {
        DbKind::Sqlite => format!("substr({col}, 1, 10)"),
        DbKind::Mysql => format!("DATE_FORMAT({col}, '%Y-%m-%d')"),
        DbKind::Postgres => format!("to_char({col}, 'YYYY-MM-DD')"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Quota is denominated in CNY, so migration 40 must rescale balances from
    /// the old point scale (50000 points per yuan) into micro-quota
    /// (1_000_000 per yuan) without changing what anyone can buy.
    #[tokio::test]
    async fn cny_migration_preserves_value_and_derives_the_exchange_rate() {
        install_drivers();
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        migrate(&pool, DbKind::Sqlite).await.unwrap();

        // Rewind migration 40 and restore the point-scale settings it removes.
        pool.execute("DELETE FROM _migrations WHERE id = 40").await.unwrap();
        pool.execute("DELETE FROM app_settings WHERE k = 'usd_to_cny_rate_micro'").await.unwrap();
        pool.execute("INSERT INTO app_settings (k, v) VALUES ('quota_per_usd', '500000')").await.unwrap();
        pool.execute("INSERT INTO app_settings (k, v) VALUES ('epay_quota_per_yuan', '50000')").await.unwrap();
        pool.execute("UPDATE app_settings SET v = '100000' WHERE k = 'signup_grant'").await.unwrap();
        pool.execute("INSERT INTO users (id, username, password_hash) VALUES (1, 'u1', 'x')").await.unwrap();
        // 100000 points at 50000 points/yuan = 2 yuan of purchasing power.
        pool.execute("INSERT INTO user_balances (user_id, balance, lifetime_used) VALUES (1, 100000, 50000)").await.unwrap();
        pool.execute("INSERT INTO balance_ledger (user_id, delta, reason) VALUES (1, -25000, 'chat_openai')").await.unwrap();

        migrate(&pool, DbKind::Sqlite).await.unwrap();

        // 2 yuan must still be 2 yuan, now expressed in micro-quota.
        let (balance, lifetime): (i64, i64) =
            sqlx::query_as("SELECT balance, lifetime_used FROM user_balances WHERE user_id = 1")
                .fetch_one(&pool).await.unwrap();
        assert_eq!(balance, 2_000_000, "100000 points = 2 yuan = 2e6 micro-quota");
        assert_eq!(lifetime, 1_000_000);

        // A 25000-point charge was half a yuan and must stay half a yuan.
        let (delta,): (i64,) =
            sqlx::query_as("SELECT delta FROM balance_ledger WHERE user_id = 1")
                .fetch_one(&pool).await.unwrap();
        assert_eq!(delta, -500_000);

        // The two old knobs collapse into one exchange rate:
        // 500000 points/USD ÷ 50000 points/yuan = 10 CNY per USD.
        let (rate,): (String,) =
            sqlx::query_as("SELECT v FROM app_settings WHERE k = 'usd_to_cny_rate_micro'")
                .fetch_one(&pool).await.unwrap();
        assert_eq!(rate, "10000000", "derived from the operator's own old settings");

        // Grants are amounts of money too.
        let (grant,): (String,) =
            sqlx::query_as("SELECT v FROM app_settings WHERE k = 'signup_grant'")
                .fetch_one(&pool).await.unwrap();
        assert_eq!(grant, "2000000", "100000 points = 2 yuan");

        // The obsolete point-scale settings are gone.
        let (leftover,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM app_settings WHERE k IN ('quota_per_usd', 'epay_quota_per_yuan')",
        )
        .fetch_one(&pool).await.unwrap();
        assert_eq!(leftover, 0);

        pool.close().await;
    }

    /// A chat model priced only under the old per-call scheme cannot be
    /// converted to token rates — the single credit figure carries no
    /// input/output split. Such a row must end up disabled rather than
    /// enabled at zero, which would serve the model for free.
    #[tokio::test]
    async fn quota_migration_disables_chat_models_it_cannot_price() {
        install_drivers();
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        migrate(&pool, DbKind::Sqlite).await.unwrap();

        // Rewind past the quota migrations and rebuild a pre-0038 pricing row,
        // mirroring what a real upgrade from the credit era looks like.
        pool.execute("DELETE FROM _migrations WHERE id IN (38, 39)").await.unwrap();
        pool.execute("ALTER TABLE user_balances RENAME TO user_credits").await.unwrap();
        pool.execute("ALTER TABLE balance_ledger RENAME TO credit_ledger").await.unwrap();
        for col in ["input_tokens", "output_tokens", "cached_tokens"] {
            pool.execute(format!("ALTER TABLE credit_ledger DROP COLUMN {col}").as_str()).await.unwrap();
        }
        for col in ["input_price", "output_price", "cached_input_price"] {
            pool.execute(format!("ALTER TABLE model_pricing DROP COLUMN {col}").as_str()).await.unwrap();
        }
        pool.execute("ALTER TABLE model_pricing RENAME COLUMN per_call_price TO cost_credits").await.unwrap();
        pool.execute("ALTER TABLE model_pricing RENAME COLUMN base_price TO base_credits").await.unwrap();
        pool.execute("ALTER TABLE model_pricing RENAME COLUMN per_second_price TO per_second").await.unwrap();
        pool.execute("ALTER TABLE video_jobs RENAME COLUMN cost_quota TO cost_credits").await.unwrap();
        pool.execute("ALTER TABLE payment_orders RENAME COLUMN quota TO credits").await.unwrap();
        pool.execute("DELETE FROM app_settings WHERE k IN ('quota_per_usd', 'price_multiplier_percent')").await.unwrap();
        pool.execute("UPDATE app_settings SET k = 'epay_credits_per_yuan' WHERE k = 'epay_quota_per_yuan'").await.unwrap();
        // An enabled per-call chat model, plus an image model that converts fine.
        pool.execute("INSERT INTO model_pricing (model, kind, cost_credits, enabled, protocol, base_credits, per_second) \
                      VALUES ('legacy-chat', 'chat', 3, 1, 'openai', 0, 0)").await.unwrap();
        pool.execute("INSERT INTO model_pricing (model, kind, cost_credits, enabled, protocol, base_credits, per_second) \
                      VALUES ('legacy-image', 'image', 5, 1, 'openai', 0, 0)").await.unwrap();

        migrate(&pool, DbKind::Sqlite).await.unwrap();

        // The chat row is parked, not silently free.
        let (enabled, per_call, input, output): (i64, i64, i64, i64) = sqlx::query_as(
            "SELECT enabled, per_call_price, input_price, output_price \
             FROM model_pricing WHERE model = 'legacy-chat'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(enabled, 0, "an unpriceable chat model must be disabled");
        assert_eq!((input, output), (0, 0));
        // The old figure is retained so an admin can see what it used to cost.
        assert_eq!(per_call, 3_000);

        // Image billing is per call, so that row converts cleanly and stays on.
        let (img_enabled, img_price): (i64, i64) = sqlx::query_as(
            "SELECT enabled, per_call_price FROM model_pricing WHERE model = 'legacy-image'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(img_enabled, 1, "image pricing converts cleanly and stays enabled");
        assert_eq!(img_price, 5_000);

        // Re-running must not disable a model that now has real token rates.
        pool.execute(
            "UPDATE model_pricing SET input_price = 2000000, output_price = 10000000, enabled = 1 \
             WHERE model = 'legacy-chat'",
        )
        .await
        .unwrap();
        pool.execute("DELETE FROM _migrations WHERE id = 39").await.unwrap();
        migrate(&pool, DbKind::Sqlite).await.unwrap();
        let (still_on,): (i64,) =
            sqlx::query_as("SELECT enabled FROM model_pricing WHERE model = 'legacy-chat'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(still_on, 1, "a properly priced model must stay enabled");

        pool.close().await;
    }

    /// Migration 38 must carry every account's purchasing power across the
    /// credit→quota switch: balances scale by 500 and per-call prices become
    /// the micro-USD that buys the same amount at 500_000 quota per USD.
    #[tokio::test]
    async fn quota_migration_preserves_purchasing_power() {
        install_drivers();
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        migrate(&pool, DbKind::Sqlite).await.unwrap();

        // Rewind to the pre-quota schema and re-create the old-shaped rows.
        pool.execute("DELETE FROM _migrations WHERE id = 38").await.unwrap();
        pool.execute("ALTER TABLE user_balances RENAME TO user_credits").await.unwrap();
        pool.execute("ALTER TABLE balance_ledger RENAME TO credit_ledger").await.unwrap();
        for col in ["input_tokens", "output_tokens", "cached_tokens"] {
            pool.execute(format!("ALTER TABLE credit_ledger DROP COLUMN {col}").as_str()).await.unwrap();
        }
        for col in ["input_price", "output_price", "cached_input_price"] {
            pool.execute(format!("ALTER TABLE model_pricing DROP COLUMN {col}").as_str()).await.unwrap();
        }
        pool.execute("ALTER TABLE model_pricing RENAME COLUMN per_call_price TO cost_credits").await.unwrap();
        pool.execute("ALTER TABLE model_pricing RENAME COLUMN base_price TO base_credits").await.unwrap();
        pool.execute("ALTER TABLE model_pricing RENAME COLUMN per_second_price TO per_second").await.unwrap();
        pool.execute("ALTER TABLE video_jobs RENAME COLUMN cost_quota TO cost_credits").await.unwrap();
        pool.execute("ALTER TABLE payment_orders RENAME COLUMN quota TO credits").await.unwrap();
        pool.execute("DELETE FROM app_settings WHERE k IN ('quota_per_usd', 'price_multiplier_percent')").await.unwrap();
        pool.execute("UPDATE app_settings SET k = 'epay_credits_per_yuan' WHERE k = 'epay_quota_per_yuan'").await.unwrap();
        pool.execute("INSERT INTO users (id, username, password_hash) VALUES (1, 'u1', 'x')").await.unwrap();
        pool.execute("INSERT INTO user_credits (user_id, balance, lifetime_used) VALUES (1, 200, 40)").await.unwrap();
        pool.execute("INSERT INTO credit_ledger (user_id, delta, reason) VALUES (1, -5, 'chat_openai')").await.unwrap();
        pool.execute("INSERT INTO model_pricing (model, kind, cost_credits, enabled, protocol, base_credits, per_second) \
                      VALUES ('gpt-x', 'image', 5, 1, 'openai', 0, 0)").await.unwrap();

        migrate(&pool, DbKind::Sqlite).await.unwrap();

        // 200 credits bought 2 CNY of usage; it must still buy 100_000 quota.
        let (balance, lifetime): (i64, i64) =
            sqlx::query_as("SELECT balance, lifetime_used FROM user_balances WHERE user_id = 1")
                .fetch_one(&pool).await.unwrap();
        assert_eq!((balance, lifetime), (100_000, 20_000));

        let (delta,): (i64,) =
            sqlx::query_as("SELECT delta FROM balance_ledger WHERE user_id = 1")
                .fetch_one(&pool).await.unwrap();
        assert_eq!(delta, -2_500);

        // A 5-credit image call cost 1/40 CNY; at 500k quota/USD that is
        // 5_000 micro-USD, which still charges exactly 2_500 quota.
        let (per_call,): (i64,) =
            sqlx::query_as("SELECT per_call_price FROM model_pricing WHERE model = 'gpt-x'")
                .fetch_one(&pool).await.unwrap();
        assert_eq!(per_call, 5_000);

        let (rate,): (String,) =
            sqlx::query_as("SELECT v FROM app_settings WHERE k = 'quota_per_usd'")
                .fetch_one(&pool).await.unwrap();
        let quota_per_usd: i64 = rate.parse().unwrap();
        assert_eq!(per_call * quota_per_usd / 1_000_000, 2_500);

        // The obsolete global per-call settings are gone.
        let (leftover,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM app_settings WHERE k IN ('cost_chat', 'cost_image')",
        )
        .fetch_one(&pool).await.unwrap();
        assert_eq!(leftover, 0);

        pool.close().await;
    }

    #[tokio::test]
    async fn brand_migration_updates_defaults_and_preserves_custom_settings() {
        install_drivers();
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        migrate(&pool, DbKind::Sqlite).await.unwrap();
        let read = "SELECT k, v FROM app_settings WHERE k IN ('smtp_from_name', 'epay_product_name') ORDER BY k";
        let defaults: Vec<(String, String)> = sqlx::query_as(read).fetch_all(&pool).await.unwrap();
        assert_eq!(defaults, vec![
            ("epay_product_name".into(), "Yunova 额度充值".into()),
            ("smtp_from_name".into(), "Yunova".into()),
        ]);

        // Simulate upgrading a database with customized names at version 36.
        pool.execute("DELETE FROM _migrations WHERE id = 37").await.unwrap();
        pool.execute("UPDATE app_settings SET v = 'My workspace' WHERE k = 'smtp_from_name'").await.unwrap();
        pool.execute("UPDATE app_settings SET v = 'Custom credits' WHERE k = 'epay_product_name'").await.unwrap();
        migrate(&pool, DbKind::Sqlite).await.unwrap();
        migrate(&pool, DbKind::Sqlite).await.unwrap();
        let customized: Vec<(String, String)> = sqlx::query_as(read).fetch_all(&pool).await.unwrap();
        assert_eq!(customized, vec![
            ("epay_product_name".into(), "Custom credits".into()),
            ("smtp_from_name".into(), "My workspace".into()),
        ]);
        pool.close().await;
    }

    /// Migration 41 adds the reasoning columns so a refresh can replay the
    /// thinking block. Existing rows must survive with NULL reasoning.
    #[tokio::test]
    async fn reasoning_migration_adds_nullable_columns_and_keeps_old_messages() {
        install_drivers();
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        migrate(&pool, DbKind::Sqlite).await.unwrap();

        // Rewind to the pre-reasoning schema and insert a legacy message.
        pool.execute("DELETE FROM _migrations WHERE id = 41").await.unwrap();
        pool.execute("ALTER TABLE messages DROP COLUMN reasoning").await.unwrap();
        pool.execute("ALTER TABLE messages DROP COLUMN reasoning_ms").await.unwrap();
        pool.execute("INSERT INTO users (id, username, password_hash) VALUES (1, 'u1', 'x')").await.unwrap();
        pool.execute("INSERT INTO conversations (id, user_id, title) VALUES (1, 1, 'c')").await.unwrap();
        pool.execute("INSERT INTO messages (conversation_id, role, content) VALUES (1, 'assistant', 'old')").await.unwrap();

        migrate(&pool, DbKind::Sqlite).await.unwrap();

        let (content, reasoning, ms): (String, Option<String>, Option<i64>) = sqlx::query_as(
            "SELECT content, reasoning, reasoning_ms FROM messages WHERE conversation_id = 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(content, "old");
        assert_eq!(reasoning, None, "pre-41 messages have no stored thinking");
        assert_eq!(ms, None);

        // New messages round-trip the thinking block and its duration.
        pool.execute(
            "INSERT INTO messages (conversation_id, role, content, reasoning, reasoning_ms) \
             VALUES (1, 'assistant', 'hi', 'let me think', 1200)",
        )
        .await
        .unwrap();
        let (reasoning, ms): (String, i64) = sqlx::query_as(
            "SELECT reasoning, reasoning_ms FROM messages WHERE content = 'hi'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!((reasoning.as_str(), ms), ("let me think", 1200));

        pool.close().await;
    }

    /// Agent tokens are bearer credentials usable from outside the browser, so
    /// migration 42 must create them hash-only and tie them to the account:
    /// deleting a user has to take their agent credentials with it, otherwise
    /// a closed account could still spend quota through the chat gateway.
    #[tokio::test]
    async fn agent_token_migration_stores_hashes_and_cascades_on_user_delete() {
        install_drivers();
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        migrate(&pool, DbKind::Sqlite).await.unwrap();
        pool.execute("PRAGMA foreign_keys = ON").await.unwrap();

        pool.execute("INSERT INTO users (id, username, password_hash) VALUES (1, 'u1', 'x')")
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO agent_tokens (user_id, name, token_hash, prefix) VALUES (?, ?, ?, ?)",
        )
        .bind(1_i64)
        .bind("laptop")
        .bind("deadbeef")
        .bind("yna_dead")
        .execute(&pool)
        .await
        .unwrap();

        // New tokens are live by default; nothing stores the plaintext.
        let (revoked,): (i64,) =
            sqlx::query_as("SELECT revoked FROM agent_tokens WHERE token_hash = 'deadbeef'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(revoked, 0);

        // The same hash must not be insertable twice: a duplicate would make
        // one plaintext resolve to two accounts.
        let dup = sqlx::query(
            "INSERT INTO agent_tokens (user_id, name, token_hash, prefix) VALUES (?, ?, ?, ?)",
        )
        .bind(1_i64)
        .bind("other")
        .bind("deadbeef")
        .bind("yna_dead")
        .execute(&pool)
        .await;
        assert!(dup.is_err(), "token_hash must be unique");

        pool.execute("DELETE FROM users WHERE id = 1").await.unwrap();
        let (left,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM agent_tokens")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(left, 0, "a deleted account must not keep usable credentials");

        pool.close().await;
    }
}
