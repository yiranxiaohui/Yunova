//! `yunova db-copy --from <url> --to <url>` — move an existing installation's
//! data to another backend.
//!
//! Switching from the single-file SQLite default to PostgreSQL is a normal
//! thing for a self-hosted install to outgrow, and the alternative is an
//! out-of-tree dump/convert script. Such a script has no way to stay correct:
//! it would need its own copy of the schema and would silently rot the next
//! time a migration adds a table. This lives in the binary that owns the
//! migrations, and derives everything else from the live catalog.
//!
//! What it does: run migrations on the target, copy every row of every table,
//! then fast-forward the target's `id` sequences. What it does not do: touch
//! the source, or rewrite the config file — the operator points
//! `YUNOVA_DATABASE_URL` (or `yunova.toml`) at the new database once the copy
//! has been verified.

use sqlx::{Column, Row, TypeInfo};

use crate::db::{self, DbKind, Pool};

/// Tables in foreign-key order: a table always follows everything it
/// references, so inserting in this order never trips a constraint.
///
/// `users` self-references through `invited_by`, which is why that column is
/// nulled during the first pass and filled in afterwards.
const TABLE_ORDER: &[&str] = &[
    "users",
    "user_settings",
    "user_balances",
    "balance_ledger",
    "app_settings",
    "sessions",
    "email_codes",
    "prompts",
    "skills",
    "conversations",
    "messages",
    "conversation_skills",
    "shared_conversations",
    "plaza_images",
    "plaza_image_likes",
    "plaza_image_comments",
    "payment_orders",
    "image_jobs",
    "studio_conversations",
    "studio_messages",
    "studio_jobs",
    "studio_generations",
    "upstream_channels",
    "model_pricing",
    "channel_models",
    "video_jobs",
    "workflows",
    "workflow_runs",
    "workflow_node_runs",
    "workflow_run_logs",
    "media_assets",
    "video_editor_projects",
    "video_editor_exports",
    "media_library_assets",
    "agent_devices",
    "agent_tokens",
    "agent_sessions",
    "agent_entries",
];

/// One value in transit. Mirrors the type set the `Any` driver supports, which
/// is also the set both schemas are declared in.
#[derive(Debug, Clone)]
enum Cell {
    /// A NULL, tagged with the target column's domain. `Any` turns a bound
    /// `None` into a typed parameter, and Postgres rejects a text NULL for a
    /// bigint column, so the tag has to come from the target schema rather
    /// than from the source value (whose type is simply "NULL").
    Null(NullAs),
    Int(i64),
    Real(f64),
    Text(String),
    Bool(bool),
    Blob(Vec<u8>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NullAs {
    Int,
    Text,
}

pub struct Options {
    pub from: String,
    pub to: String,
    /// Copy into a target that already holds rows. Off by default: the usual
    /// cause of a non-empty target is pointing `--to` at the database already
    /// in service.
    pub allow_nonempty: bool,
}

pub async fn run(opts: Options) -> Result<(), String> {
    let from_kind = DbKind::from_url(&opts.from)
        .ok_or_else(|| format!("unsupported --from url: {}", opts.from))?;
    let to_kind =
        DbKind::from_url(&opts.to).ok_or_else(|| format!("unsupported --to url: {}", opts.to))?;
    if opts.from.trim() == opts.to.trim() {
        return Err("--from and --to are the same database".into());
    }

    let src = db::connect(&opts.from)
        .await
        .map_err(|e| format!("connect --from: {e}"))?;
    let dst = db::connect(&opts.to)
        .await
        .map_err(|e| format!("connect --to: {e}"))?;

    // The source must already be migrated; copying from a half-migrated
    // database would silently drop whatever the pending migrations add.
    let src_version = max_migration(&src).await?;
    println!("source: {} (schema version {src_version})", from_kind.as_str());

    println!("target: {} — applying migrations", to_kind.as_str());
    db::migrate(&dst, to_kind)
        .await
        .map_err(|e| format!("migrate target: {e}"))?;

    let src_tables = table_names(&src, from_kind).await?;
    let dst_tables = table_names(&dst, to_kind).await?;

    // A table present in the source but unknown to TABLE_ORDER would be
    // skipped without this check, which is exactly the failure an external
    // script makes silently.
    let mut unlisted: Vec<&String> = src_tables
        .iter()
        .filter(|t| *t != "_migrations" && !TABLE_ORDER.contains(&t.as_str()))
        .collect();
    unlisted.sort();
    if !unlisted.is_empty() {
        return Err(format!(
            "source has tables this build does not know how to copy: {unlisted:?}. \
             Add them to TABLE_ORDER in src/db_copy.rs."
        ));
    }

    if !opts.allow_nonempty {
        for table in TABLE_ORDER {
            if !dst_tables.contains(&table.to_string()) {
                continue;
            }
            // app_settings is seeded by the migrations themselves, so it is
            // never empty and cannot signal "already in use".
            if *table == "app_settings" {
                continue;
            }
            if count_rows(&dst, table).await? > 0 {
                return Err(format!(
                    "target table `{table}` is not empty; refusing to copy. \
                     Use --allow-nonempty only if you are certain."
                ));
            }
        }
    }

    // Seeded rows would collide with the source's own copies of the same keys.
    if dst_tables.contains(&"app_settings".to_string())
        && src_tables.contains(&"app_settings".to_string())
    {
        sqlx::query("DELETE FROM app_settings")
            .execute(&dst)
            .await
            .map_err(|e| format!("clear seeded app_settings: {e}"))?;
    }

    let mut total = 0u64;
    for table in TABLE_ORDER {
        if !src_tables.contains(&table.to_string()) {
            continue;
        }
        if !dst_tables.contains(&table.to_string()) {
            return Err(format!("target is missing table `{table}`"));
        }
        let n = copy_table(&src, from_kind, &dst, to_kind, table).await?;
        total += n;
        if n > 0 {
            println!("  {table}: {n} rows");
        }
    }

    // Second pass for the self-reference nulled during the first.
    let invites = restore_invited_by(&src, from_kind, &dst, to_kind).await?;
    if invites > 0 {
        println!("  users.invited_by: {invites} rows linked");
    }

    if to_kind == DbKind::Postgres {
        // Ids were copied verbatim, so the sequences still sit at their
        // starting value and the next insert would collide with row 1.
        fix_sequences(&dst, &dst_tables).await?;
        println!("  id sequences fast-forwarded");
    }

    println!("copied {total} rows");
    println!(
        "Point YUNOVA_DATABASE_URL at the new database to use it; \
         the source was not modified."
    );
    src.close().await;
    dst.close().await;
    Ok(())
}

async fn max_migration(pool: &Pool) -> Result<i64, String> {
    let row: (Option<i64>,) = sqlx::query_as("SELECT MAX(id) FROM _migrations")
        .fetch_one(pool)
        .await
        .map_err(|e| format!("read source schema version: {e}"))?;
    row.0
        .ok_or_else(|| "source database has no applied migrations".to_string())
}

async fn count_rows(pool: &Pool, table: &str) -> Result<i64, String> {
    // `table` comes from TABLE_ORDER, never from user input.
    let row: (i64,) = sqlx::query_as(&format!("SELECT COUNT(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .map_err(|e| format!("count {table}: {e}"))?;
    Ok(row.0)
}

async fn table_names(pool: &Pool, kind: DbKind) -> Result<Vec<String>, String> {
    // `::text` on Postgres because information_schema hands back `name`,
    // which the Any driver does not decode.
    let sql = match kind {
        DbKind::Sqlite => {
            "SELECT name FROM sqlite_master \
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name"
        }
        DbKind::Postgres => {
            "SELECT table_name::text FROM information_schema.tables \
             WHERE table_schema = current_schema() AND table_type = 'BASE TABLE' \
             ORDER BY table_name"
        }
    };
    let rows: Vec<(String,)> = sqlx::query_as(sql)
        .fetch_all(pool)
        .await
        .map_err(|e| format!("list tables: {e}"))?;
    Ok(rows.into_iter().map(|(t,)| t).collect())
}

async fn column_names(pool: &Pool, kind: DbKind, table: &str) -> Result<Vec<String>, String> {
    Ok(columns_with_types(pool, kind, table)
        .await?
        .into_iter()
        .map(|(name, _)| name)
        .collect())
}

/// Column names in ordinal order, each with the domain to use for a NULL.
async fn columns_with_types(
    pool: &Pool,
    kind: DbKind,
    table: &str,
) -> Result<Vec<(String, NullAs)>, String> {
    let rows: Vec<(String, String)> = match kind {
        DbKind::Sqlite => {
            sqlx::query_as("SELECT name, type FROM pragma_table_info(?) ORDER BY cid")
                .bind(table)
                .fetch_all(pool)
                .await
        }
        DbKind::Postgres => sqlx::query_as(
            "SELECT column_name::text, data_type::text FROM information_schema.columns \
             WHERE table_schema = current_schema() AND table_name = $1 \
             ORDER BY ordinal_position",
        )
        .bind(table)
        .fetch_all(pool)
        .await,
    }
    .map_err(|e| format!("list columns of {table}: {e}"))?;
    Ok(rows
        .into_iter()
        .map(|(name, ty)| {
            let t = ty.to_ascii_uppercase();
            let null_as = if t.contains("INT") || t.contains("SERIAL") {
                NullAs::Int
            } else {
                NullAs::Text
            };
            (name, null_as)
        })
        .collect())
}

/// Copies one table in id order, in batches, so a large `messages` table does
/// not have to fit in memory all at once.
async fn copy_table(
    src: &Pool,
    src_kind: DbKind,
    dst: &Pool,
    dst_kind: DbKind,
    table: &str,
) -> Result<u64, String> {
    let src_cols = column_names(src, src_kind, table).await?;
    let dst_typed = columns_with_types(dst, dst_kind, table).await?;
    let dst_cols: Vec<String> = dst_typed.iter().map(|(n, _)| n.clone()).collect();

    // Copy the intersection: a column the target does not have cannot be
    // written, and one the source lacks keeps its default.
    let cols: Vec<String> = src_cols
        .iter()
        .filter(|c| dst_cols.contains(c))
        .cloned()
        .collect();
    if cols.is_empty() {
        return Err(format!("no common columns for {table}"));
    }
    for missing in dst_cols.iter().filter(|c| !src_cols.contains(c)) {
        println!("  note: {table}.{missing} not in source, using default");
    }
    // NULLs are tagged with the *target's* domain, since that is the schema
    // the parameter is type-checked against.
    let null_kinds: Vec<NullAs> = cols
        .iter()
        .map(|c| {
            dst_typed
                .iter()
                .find(|(name, _)| name == c)
                .map(|(_, k)| *k)
                .unwrap_or(NullAs::Text)
        })
        .collect();

    // `users.invited_by` points at another user that may not be inserted yet.
    let defer_invited_by = table == "users" && cols.iter().any(|c| c == "invited_by");

    let col_list = cols.join(", ");
    let placeholders = vec!["?"; cols.len()].join(", ");
    let insert = db::q(
        dst_kind,
        &format!("INSERT INTO {table} ({col_list}) VALUES ({placeholders})"),
    );

    const BATCH: i64 = 500;
    let ordered = src_cols.iter().any(|c| c == "id");
    let mut copied = 0u64;
    let mut offset = 0i64;
    loop {
        let select = if ordered {
            format!("SELECT {col_list} FROM {table} ORDER BY id LIMIT {BATCH} OFFSET {offset}")
        } else {
            format!("SELECT {col_list} FROM {table} LIMIT {BATCH} OFFSET {offset}")
        };
        let rows = sqlx::query(&select)
            .fetch_all(src)
            .await
            .map_err(|e| format!("read {table}: {e}"))?;
        if rows.is_empty() {
            break;
        }

        let mut tx = dst
            .begin()
            .await
            .map_err(|e| format!("begin on target: {e}"))?;
        for row in &rows {
            let mut q = sqlx::query(&insert);
            for (i, name) in cols.iter().enumerate() {
                let mut cell = read_cell(row, i, null_kinds[i])
                    .map_err(|e| format!("read {table}.{name}: {e}"))?;
                if defer_invited_by && name == "invited_by" {
                    cell = Cell::Null(NullAs::Int);
                }
                q = bind_cell(q, cell);
            }
            q.execute(&mut *tx)
                .await
                .map_err(|e| format!("insert into {table}: {e}"))?;
            copied += 1;
        }
        tx.commit()
            .await
            .map_err(|e| format!("commit {table}: {e}"))?;

        if (rows.len() as i64) < BATCH {
            break;
        }
        offset += BATCH;
    }
    Ok(copied)
}

/// Fills in the self-reference that the `users` pass deliberately left NULL.
async fn restore_invited_by(
    src: &Pool,
    src_kind: DbKind,
    dst: &Pool,
    dst_kind: DbKind,
) -> Result<u64, String> {
    let cols = column_names(src, src_kind, "users").await?;
    if !cols.iter().any(|c| c == "invited_by") {
        return Ok(0);
    }
    let pairs: Vec<(i64, i64)> =
        sqlx::query_as("SELECT id, invited_by FROM users WHERE invited_by IS NOT NULL")
            .fetch_all(src)
            .await
            .map_err(|e| format!("read users.invited_by: {e}"))?;
    if pairs.is_empty() {
        return Ok(0);
    }
    let sql = db::q(dst_kind, "UPDATE users SET invited_by = ? WHERE id = ?");
    let mut tx = dst
        .begin()
        .await
        .map_err(|e| format!("begin on target: {e}"))?;
    let mut n = 0u64;
    for (id, inviter) in pairs {
        sqlx::query(&sql)
            .bind(inviter)
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("link users.invited_by: {e}"))?;
        n += 1;
    }
    tx.commit()
        .await
        .map_err(|e| format!("commit users.invited_by: {e}"))?;
    Ok(n)
}

/// Moves every `BIGSERIAL` sequence past the highest copied id.
async fn fix_sequences(dst: &Pool, dst_tables: &[String]) -> Result<(), String> {
    for table in TABLE_ORDER {
        if !dst_tables.contains(&table.to_string()) {
            continue;
        }
        // Several tables are keyed by something else (`app_settings` by `k`,
        // the join tables by a composite key) and have no `id` at all, so the
        // column has to be checked before it can be referenced — a `WHERE`
        // clause would not help, since the SELECT list is still planned.
        let has_serial: (bool,) = sqlx::query_as(
            "SELECT pg_get_serial_sequence($1, 'id') IS NOT NULL \
             FROM information_schema.columns \
             WHERE table_schema = current_schema() \
               AND table_name = $1 AND column_name = 'id'",
        )
        .bind(table)
        .fetch_optional(dst)
        .await
        .map_err(|e| format!("inspect sequence for {table}: {e}"))?
        .unwrap_or((false,));
        if !has_serial.0 {
            continue;
        }
        // setval() on an empty table would push the sequence to 1 and then
        // hand out 2 for the first insert, so skip it when there are no rows.
        let sql = format!(
            "SELECT setval(\
                 pg_get_serial_sequence('{table}', 'id'), \
                 (SELECT MAX(id) FROM {table})\
             ) \
             WHERE EXISTS (SELECT 1 FROM {table})"
        );
        sqlx::query(&sql)
            .execute(dst)
            .await
            .map_err(|e| format!("reset sequence for {table}: {e}"))?;
    }
    Ok(())
}

fn read_cell(row: &sqlx::any::AnyRow, idx: usize, null_as: NullAs) -> Result<Cell, String> {
    // Decode against the column's reported type. SQLite reports "NULL" for a
    // NULL value regardless of the column's declared type, which is why the
    // target's domain is passed in for that case.
    let type_name = row
        .columns()
        .get(idx)
        .map(|c| c.type_info().name().to_string())
        .unwrap_or_default();
    let cell = match type_name.as_str() {
        "BIGINT" | "INTEGER" | "SMALLINT" => row
            .try_get::<Option<i64>, _>(idx)
            .map(|v| v.map_or(Cell::Null(null_as), Cell::Int)),
        "BOOLEAN" => row
            .try_get::<Option<bool>, _>(idx)
            .map(|v| v.map_or(Cell::Null(null_as), Cell::Bool)),
        "REAL" | "DOUBLE" => row
            .try_get::<Option<f64>, _>(idx)
            .map(|v| v.map_or(Cell::Null(null_as), Cell::Real)),
        "BLOB" => row
            .try_get::<Option<Vec<u8>>, _>(idx)
            .map(|v| v.map_or(Cell::Null(null_as), Cell::Blob)),
        _ => row
            .try_get::<Option<String>, _>(idx)
            .map(|v| v.map_or(Cell::Null(null_as), Cell::Text)),
    };
    cell.map_err(|e| e.to_string())
}

type AnyQuery<'q> = sqlx::query::Query<'q, sqlx::Any, sqlx::any::AnyArguments<'q>>;

fn bind_cell<'q>(q: AnyQuery<'q>, cell: Cell) -> AnyQuery<'q> {
    match cell {
        Cell::Null(NullAs::Int) => q.bind(Option::<i64>::None),
        Cell::Null(NullAs::Text) => q.bind(Option::<String>::None),
        Cell::Int(v) => q.bind(v),
        Cell::Real(v) => q.bind(v),
        Cell::Text(v) => q.bind(v),
        Cell::Bool(v) => q.bind(v),
        Cell::Blob(v) => q.bind(v),
    }
}
