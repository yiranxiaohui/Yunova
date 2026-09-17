use std::str::FromStr;

use sqlx::any::{AnyConnectOptions, AnyPoolOptions};
use sqlx::{AnyPool, Executor};

pub type Pool = AnyPool;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DbKind {
    Sqlite,
    Postgres,
}

impl DbKind {
    pub fn from_url(url: &str) -> Option<Self> {
        let u = url.trim().to_ascii_lowercase();
        if u.starts_with("sqlite:") {
            Some(Self::Sqlite)
        } else if u.starts_with("postgres:") || u.starts_with("postgresql:") {
            Some(Self::Postgres)
        } else {
            None
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sqlite => "sqlite",
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
    // 26–28 created the legacy worker tables; the feature is gone, so fresh
    // installs skip them and migration 44 drops them where they exist.
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
    (43, include_str!("../migrations/sqlite/0043_agent_sessions.sql")),
    (44, include_str!("../migrations/sqlite/0044_drop_workers.sql")),
    (45, include_str!("../migrations/sqlite/0045_agent_token_expiry.sql")),
    (46, include_str!("../migrations/sqlite/0046_device_fingerprint.sql")),
    (47, include_str!("../migrations/sqlite/0047_usd_parity_rate.sql")),
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
    (43, include_str!("../migrations/postgres/0043_agent_sessions.sql")),
    (44, include_str!("../migrations/postgres/0044_drop_workers.sql")),
    (45, include_str!("../migrations/postgres/0045_agent_token_expiry.sql")),
    (46, include_str!("../migrations/postgres/0046_device_fingerprint.sql")),
    (47, include_str!("../migrations/postgres/0047_usd_parity_rate.sql")),
    // Converts a database created before the types were corrected to what
    // `sqlx::Any` can decode. No-op on a fresh install.
    (48, include_str!("../migrations/postgres/0048_any_compatible_domain.sql")),
];

fn migrations_for(kind: DbKind) -> &'static [(i32, &'static str)] {
    match kind {
        DbKind::Sqlite => SQLITE_MIGRATIONS,
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
        DbKind::Postgres => {
            // TEXT rather than `timestamptz`, for the reason documented at the
            // top of migrations/postgres/0001_init.sql.
            "CREATE TABLE IF NOT EXISTS _migrations (
                id INT PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS')
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

/// Splits a SQL blob into individual statements separated by `;` outside
/// strings, quoted identifiers, line comments and dollar-quoted blocks.
///
/// Dollar quoting matters because migration 48 is a `DO $tag$ ... $tag$` block
/// whose body is full of `;`. Splitting on those would hand Postgres a
/// truncated `DO` statement.
fn split_sql(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut chars = src.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;
    // `Some(tag)` while inside `$tag$ ... $tag$`; the tag may be empty (`$$`).
    let mut dollar_tag: Option<String> = None;
    while let Some(c) = chars.next() {
        // Inside a dollar-quoted body nothing is special except the matching
        // closing tag, so look for that before the normal rules apply.
        if let Some(tag) = dollar_tag.clone() {
            buf.push(c);
            if c == '$' {
                // `c` is already consumed, so match only `tag$`.
                let rest: String = tag.chars().chain(std::iter::once('$')).collect();
                let mut probe = chars.clone();
                let matched: String = probe.by_ref().take(rest.chars().count()).collect();
                if matched == rest {
                    buf.push_str(&matched);
                    chars = probe;
                    dollar_tag = None;
                }
            }
            continue;
        }
        match c {
            '$' if !in_single && !in_double => {
                // A dollar quote opens with `$tag$`, where tag is empty or an
                // identifier. Anything else (e.g. a `$1` placeholder) is data.
                let mut probe = chars.clone();
                let mut tag = String::new();
                let mut opened = false;
                loop {
                    match probe.next() {
                        Some('$') => {
                            opened = true;
                            break;
                        }
                        Some(ch) if ch == '_' || ch.is_alphanumeric() => tag.push(ch),
                        _ => break,
                    }
                }
                buf.push(c);
                if opened {
                    buf.push_str(&tag);
                    buf.push('$');
                    chars = probe;
                    dollar_tag = Some(tag);
                }
            }
            '-' if !in_single && !in_double && chars.peek() == Some(&'-') => {
                // line comment
                for cc in chars.by_ref() {
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
///
/// Both backends store timestamps as `YYYY-MM-DD HH:MM:SS` in UTC, so these
/// two expressions produce byte-identical values.
pub fn now_expr(kind: DbKind) -> &'static str {
    match kind {
        DbKind::Sqlite => "datetime('now')",
        DbKind::Postgres => "to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS')",
    }
}

// Note on aggregates: Postgres widens `SUM()` over a bigint to `numeric`,
// which the `Any` driver cannot decode, so every aggregate in this codebase
// is written as `CAST(COALESCE(SUM(x), 0) AS BIGINT)`. The cast is a no-op on
// SQLite, which already returns an integer.

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

/// Case-insensitive equality fragment for usernames: `LOWER(col) = LOWER(?)`
/// on Postgres, direct `col = ?` on SQLite (handled by COLLATE NOCASE on the
/// column).
pub fn ci_eq(kind: DbKind, col: &str) -> String {
    match kind {
        DbKind::Sqlite => format!("{col} = ?"),
        DbKind::Postgres => format!("LOWER({col}) = LOWER(?)"),
    }
}

/// Returns a SQL expression that truncates a stored timestamp to a day string
/// like `2026-05-23`. Both backends keep timestamps as `YYYY-MM-DD HH:MM:SS`
/// text, so slicing off the date is the same operation on each.
pub fn day_bucket(col: &str) -> String {
    format!("substr({col}, 1, 10)")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `Any` pool decodes only a fixed set of SQL types, so a migration
    /// that declares a native timestamp or boolean makes its whole table
    /// unreadable at runtime rather than failing at migrate time. Guarding the
    /// migration text is the cheap way to keep that from coming back: the
    /// failure it prevents shows up only against a real Postgres server.
    #[test]
    fn postgres_migrations_avoid_types_the_any_driver_cannot_decode() {
        for (id, body) in POSTGRES_MIGRATIONS {
            // Migration 48 names both types deliberately, to convert them.
            if *id == 48 {
                continue;
            }
            // Comments discuss these types on purpose; check only real SQL.
            let sql: String = body
                .lines()
                .map(|line| line.split("--").next().unwrap_or(""))
                .collect::<Vec<_>>()
                .join("\n")
                .to_uppercase();
            for banned in ["TIMESTAMPTZ", "TIMESTAMP", "BOOLEAN", "NUMERIC", "DECIMAL"] {
                assert!(
                    !sql.contains(banned),
                    "migration {id} declares {banned}, which sqlx::Any cannot decode; \
                     use TEXT for timestamps and INT for booleans"
                );
            }
        }
    }

    /// Migration 48 is a single `DO $$ ... $$` block whose body is full of
    /// semicolons. Splitting on those would send Postgres a truncated
    /// statement, so the splitter has to treat a dollar-quoted body as opaque.
    #[test]
    fn sql_splitter_keeps_dollar_quoted_blocks_whole() {
        let src = "SELECT 1;\n\
                   DO $mig$ BEGIN EXECUTE 'a; b'; EXECUTE 'c'; END $mig$;\n\
                   SELECT 2;";
        let parts: Vec<String> = split_sql(src)
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(parts.len(), 3, "got {parts:?}");
        assert_eq!(parts[0], "SELECT 1");
        assert!(parts[1].starts_with("DO $mig$"));
        assert!(parts[1].ends_with("$mig$"), "block was cut: {}", parts[1]);
        assert!(parts[1].contains("EXECUTE 'c'"));
        assert_eq!(parts[2], "SELECT 2");

        // `$1` placeholders are not dollar quotes and must not swallow the rest.
        let parts = split_sql("UPDATE t SET a = $1 WHERE b = $2; SELECT 3;");
        let parts: Vec<String> = parts
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(parts.len(), 2, "got {parts:?}");
    }

    /// Both backends store timestamps as the same UTC text, which is what lets
    /// the day bucket be a plain substring and comparisons be lexicographic.
    /// A drift in either expression would silently break usage stats.
    #[tokio::test]
    async fn sqlite_now_expr_matches_the_stored_timestamp_format() {
        install_drivers();
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        migrate(&pool, DbKind::Sqlite).await.unwrap();

        let now = now_expr(DbKind::Sqlite);
        let (stamp,): (String,) = sqlx::query_as(&format!("SELECT {now}"))
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stamp.len(), 19, "expected YYYY-MM-DD HH:MM:SS, got {stamp}");
        assert_eq!(&stamp[4..5], "-");
        assert_eq!(&stamp[10..11], " ");
        assert_eq!(day_bucket("x"), "substr(x, 1, 10)");

        // The column default must agree with now_expr, or rows written by an
        // INSERT and by an UPDATE would sort differently.
        pool.execute("INSERT INTO users (id, username, password_hash) VALUES (1, 'u', 'x')")
            .await
            .unwrap();
        let (created,): (String,) = sqlx::query_as("SELECT created_at FROM users WHERE id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(created.len(), 19, "column default drifted: {created}");

        pool.close().await;
    }

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
        // 500000 points/USD ÷ 50000 points/yuan = 10 CNY per USD. (Migration
        // 47 does not re-run here, so this is migration 40's output verbatim.)
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

    /// Migration 40 derived a 10 CNY-per-USD rate from the obsolete default
    /// point scale, which billed every model ten times its upstream cost.
    /// Migration 47 corrects that to parity, because the gateways this site
    /// resells from price their own quota at 1 yuan per USD of list price.
    #[tokio::test]
    async fn usd_parity_migration_fixes_the_tenfold_default_rate() {
        install_drivers();
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        migrate(&pool, DbKind::Sqlite).await.unwrap();

        // A fresh install must bill at parity out of the box.
        let (rate,): (String,) =
            sqlx::query_as("SELECT v FROM app_settings WHERE k = 'usd_to_cny_rate_micro'")
                .fetch_one(&pool).await.unwrap();
        assert_eq!(rate, "1000000", "¥1 of quota buys $1 of upstream spend");

        // An operator who tuned the rate by hand keeps it: the correction only
        // replaces the value migration 40 wrote by default.
        pool.execute("DELETE FROM _migrations WHERE id = 47").await.unwrap();
        pool.execute("UPDATE app_settings SET v = '7200000' WHERE k = 'usd_to_cny_rate_micro'")
            .await.unwrap();
        migrate(&pool, DbKind::Sqlite).await.unwrap();
        let (kept,): (String,) =
            sqlx::query_as("SELECT v FROM app_settings WHERE k = 'usd_to_cny_rate_micro'")
                .fetch_one(&pool).await.unwrap();
        assert_eq!(kept, "7200000", "a deliberate rate is a business decision");

        // The 10x default, wherever it survived, is the one value rewritten.
        pool.execute("DELETE FROM _migrations WHERE id = 47").await.unwrap();
        pool.execute("UPDATE app_settings SET v = '10000000' WHERE k = 'usd_to_cny_rate_micro'")
            .await.unwrap();
        migrate(&pool, DbKind::Sqlite).await.unwrap();
        let (fixed,): (String,) =
            sqlx::query_as("SELECT v FROM app_settings WHERE k = 'usd_to_cny_rate_micro'")
                .fetch_one(&pool).await.unwrap();
        assert_eq!(fixed, "1000000");

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

    /// Migration 46 replaces pairing with sign-in, so a machine now identifies
    /// itself by fingerprint. Devices created by the old pairing flow have
    /// none, and several of them must still coexist: a unique index that
    /// treated NULL as a value would break every account with two paired
    /// machines the moment it ran.
    #[tokio::test]
    async fn device_fingerprint_migration_keeps_paired_devices_and_scopes_uniqueness() {
        install_drivers();
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        migrate(&pool, DbKind::Sqlite).await.unwrap();

        // Rewind to the pre-fingerprint schema and insert two legacy paired
        // devices, as an account that paired a laptop and a desktop would have.
        pool.execute("DELETE FROM _migrations WHERE id = 46").await.unwrap();
        pool.execute("DROP INDEX idx_agent_devices_fingerprint").await.unwrap();
        pool.execute("ALTER TABLE agent_devices DROP COLUMN fingerprint").await.unwrap();
        pool.execute("INSERT INTO users (id, username, password_hash) VALUES (1, 'u1', 'x')")
            .await
            .unwrap();
        pool.execute("INSERT INTO users (id, username, password_hash) VALUES (2, 'u2', 'x')")
            .await
            .unwrap();
        pool.execute(
            "INSERT INTO agent_devices (user_id, name, token_hash) VALUES (1, 'laptop', 'h1')",
        )
        .await
        .unwrap();
        pool.execute(
            "INSERT INTO agent_devices (user_id, name, token_hash) VALUES (1, 'desktop', 'h2')",
        )
        .await
        .unwrap();

        migrate(&pool, DbKind::Sqlite).await.unwrap();

        // Both paired devices survive, with no fingerprint to match on.
        let rows: Vec<(String, Option<String>)> =
            sqlx::query_as("SELECT name, fingerprint FROM agent_devices ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            rows,
            vec![
                ("laptop".to_string(), None),
                ("desktop".to_string(), None)
            ],
            "pre-46 paired devices must keep working"
        );

        // One machine per user: a second sign-in from the same computer has to
        // collide so the server rebinds instead of duplicating the row.
        sqlx::query(
            "INSERT INTO agent_devices (user_id, name, token_hash, fingerprint) \
             VALUES (1, 'signed-in', 'h3', 'fp-a')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let dup = sqlx::query(
            "INSERT INTO agent_devices (user_id, name, token_hash, fingerprint) \
             VALUES (1, 'signed-in-again', 'h4', 'fp-a')",
        )
        .execute(&pool)
        .await;
        assert!(dup.is_err(), "one fingerprint per user must not duplicate");

        // Two users on one shared computer are two devices, not one.
        sqlx::query(
            "INSERT INTO agent_devices (user_id, name, token_hash, fingerprint) \
             VALUES (2, 'shared-pc', 'h5', 'fp-a')",
        )
        .execute(&pool)
        .await
        .expect("the same machine under another account is a separate device");

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

    /// Migration 43 adds the target-agnostic session model. Two cascade
    /// choices carry product meaning and must not drift:
    /// removing a machine keeps its transcript readable, while removing the
    /// account removes everything.
    #[tokio::test]
    async fn agent_session_migration_keeps_transcripts_when_a_device_is_removed() {
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
        pool.execute(
            "INSERT INTO agent_devices (id, user_id, name, token_hash) \
             VALUES (1, 1, 'laptop', 'h1')",
        )
        .await
        .unwrap();
        pool.execute(
            "INSERT INTO agent_sessions (id, user_id, target, device_id, title) \
             VALUES (1, 1, 'device', 1, 't')",
        )
        .await
        .unwrap();
        pool.execute(
            "INSERT INTO agent_entries (session_id, entry_id, kind, payload) \
             VALUES (1, 'a1', 'message', '{}')",
        )
        .await
        .unwrap();

        // A session starts idle and without a cursor: nothing is mirrored yet.
        let (status, cursor): (String, Option<String>) =
            sqlx::query_as("SELECT status, cursor FROM agent_sessions WHERE id = 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "idle");
        assert_eq!(cursor, None);

        // Mirroring is idempotent: replaying an overlapping range after a
        // reconnect must not duplicate an entry.
        let dup = sqlx::query(
            "INSERT INTO agent_entries (session_id, entry_id, kind, payload) \
             VALUES (1, 'a1', 'message', '{}')",
        )
        .execute(&pool)
        .await;
        assert!(dup.is_err(), "(session_id, entry_id) must be unique");

        // Removing the machine must not destroy the user's history.
        pool.execute("DELETE FROM agent_devices WHERE id = 1").await.unwrap();
        let (device_id,): (Option<i64>,) =
            sqlx::query_as("SELECT device_id FROM agent_sessions WHERE id = 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(device_id, None, "the session survives with no machine bound");
        let (entries,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM agent_entries WHERE session_id = 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(entries, 1, "the transcript must remain readable");

        // Removing the account removes its sessions and their entries.
        pool.execute("DELETE FROM users WHERE id = 1").await.unwrap();
        let (sessions,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM agent_sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
        let (entries,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM agent_entries")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!((sessions, entries), (0, 0));

        pool.close().await;
    }
}
