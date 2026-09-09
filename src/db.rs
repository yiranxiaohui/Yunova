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
            ("epay_product_name".into(), "Yunova 积分充值".into()),
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
}
