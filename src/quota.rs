//! Site quota: balance, ledger, and the token-metered pricing engine.
//!
//! Quota is Yunova's own unit. It is an integer with no currency attached —
//! the UI prints the number directly. Model prices are configured against the
//! provider's *official USD rate* and stored as micro-USD (1 USD = 1_000_000),
//! so an admin transcribes published pricing instead of inventing per-model
//! credit values. Two global settings turn upstream cost into quota:
//!
//!   * `quota_per_usd`            — quota charged per 1 USD of upstream spend
//!   * `price_multiplier_percent` — markup applied to every model (100 = 1.0x)
//!
//! Chat is billed **after** the response, from the upstream's reported token
//! usage (see [`crate::usage`]). Image and video keep per-call / per-second
//! pricing because those APIs report no tokens.

use axum::{
    Extension, Json, Router,
    extract::{Path, Query},
    http::StatusCode,
    middleware,
    response::{IntoResponse, Response},
    routing::{get, patch},
};
use serde::{Deserialize, Serialize};

use crate::{
    AppState, CurrentUser, InstalledState, admin,
    db::{self, DbKind, Pool},
};

/// Micro-USD per whole USD. Prices are stored in this unit so that a rate like
/// $0.15 / 1M tokens is the exact integer 150_000 rather than a float.
pub const MICRO_USD: i64 = 1_000_000;

/// Divisor for per-token rates, which are configured per 1M tokens.
pub const TOKENS_PER_PRICE_UNIT: i64 = 1_000_000;

pub const DEFAULT_QUOTA_PER_USD: i64 = 500_000;
pub const DEFAULT_SIGNUP_GRANT: i64 = 100_000;
pub const DEFAULT_INVITE_GRANT: i64 = 50_000;

// ---------------------------------------------------------------------------
// app-wide settings (KV)
// ---------------------------------------------------------------------------

pub async fn get_setting(pool: &Pool, kind: DbKind, key: &str) -> Option<String> {
    let sql = db::q(kind, "SELECT v FROM app_settings WHERE k = ?");
    let row: Option<(String,)> = sqlx::query_as(&sql)
        .bind(key)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    row.map(|(v,)| v)
}

pub async fn get_setting_i64(pool: &Pool, kind: DbKind, key: &str, default: i64) -> i64 {
    get_setting(pool, kind, key)
        .await
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(default)
}

pub async fn get_setting_bool(pool: &Pool, kind: DbKind, key: &str, default: bool) -> bool {
    match get_setting(pool, kind, key).await.as_deref() {
        Some("true" | "1" | "yes") => true,
        Some("false" | "0" | "no") => false,
        _ => default,
    }
}

pub async fn set_setting(
    pool: &Pool,
    kind: DbKind,
    key: &str,
    val: &str,
) -> Result<(), sqlx::Error> {
    let now = db::now_expr(kind);
    let sql = match kind {
        DbKind::Sqlite => {
            format!(
                "INSERT INTO app_settings (k, v, updated_at) VALUES (?, ?, {now})
                 ON CONFLICT(k) DO UPDATE SET v = excluded.v, updated_at = {now}"
            )
        }
        DbKind::Postgres => format!(
            "INSERT INTO app_settings (k, v, updated_at) VALUES (?, ?, {now})
             ON CONFLICT (k) DO UPDATE SET v = EXCLUDED.v, updated_at = {now}"
        ),
        DbKind::Mysql => format!(
            "INSERT INTO app_settings (`k`, `v`, updated_at) VALUES (?, ?, {now})
             ON DUPLICATE KEY UPDATE `v` = VALUES(`v`), updated_at = {now}"
        ),
    };
    let sql = db::q(kind, &sql);
    sqlx::query(&sql)
        .bind(key)
        .bind(val)
        .execute(pool)
        .await
        .map(|_| ())
}

// ---------------------------------------------------------------------------
// pricing engine
// ---------------------------------------------------------------------------

/// The two global knobs that convert upstream micro-USD into site quota.
/// Read once per billing decision so an admin edit takes effect immediately
/// without restarting.
#[derive(Debug, Clone, Copy)]
pub struct QuotaRate {
    pub quota_per_usd: i64,
    pub multiplier_percent: i64,
}

impl QuotaRate {
    pub async fn load(pool: &Pool, kind: DbKind) -> Self {
        Self {
            quota_per_usd: get_setting_i64(pool, kind, "quota_per_usd", DEFAULT_QUOTA_PER_USD)
                .await
                .max(0),
            multiplier_percent: get_setting_i64(pool, kind, "price_multiplier_percent", 100)
                .await
                .max(0),
        }
    }

    /// Convert an upstream price in micro-USD to quota, applying the markup.
    /// Rounds up so that a non-zero cost never bills as free — otherwise a
    /// stream of tiny requests would be unbounded free usage.
    pub fn quota_for_micro_usd(&self, micro_usd: i64) -> i64 {
        if micro_usd <= 0 || self.quota_per_usd <= 0 || self.multiplier_percent <= 0 {
            return 0;
        }
        // (micro_usd / 1e6) * quota_per_usd * (multiplier / 100)
        let scaled =
            (micro_usd as i128) * (self.quota_per_usd as i128) * (self.multiplier_percent as i128);
        let divisor = (MICRO_USD as i128) * 100;
        let quota = (scaled + divisor - 1) / divisor; // ceil
        quota.clamp(0, i64::MAX as i128) as i64
    }
}

/// Token counts reported by an upstream response. `cached_input` is the subset
/// of `input` that hit the provider's prompt cache; it is billed at the model's
/// `cached_input_price` when configured and excluded from the normal input
/// tokens so it is never charged twice.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input: i64,
    pub output: i64,
    pub cached_input: i64,
}

impl TokenUsage {
    pub fn is_empty(&self) -> bool {
        self.input <= 0 && self.output <= 0
    }

    /// Input tokens billed at the full rate (total input minus the cached part).
    pub fn billable_input(&self) -> i64 {
        (self.input - self.cached_input).max(0)
    }
}

/// Quota cost of one chat completion given the model's rates.
/// `input_price` / `output_price` / `cached_input_price` are micro-USD per 1M
/// tokens; a NULL cached price bills cached tokens at the normal input rate.
pub fn chat_quota_cost(
    rate: QuotaRate,
    input_price: i64,
    output_price: i64,
    cached_input_price: Option<i64>,
    usage: TokenUsage,
) -> i64 {
    let cached_rate = cached_input_price.unwrap_or(input_price).max(0);
    let micro = (usage.billable_input() as i128) * (input_price.max(0) as i128)
        + (usage.output.max(0) as i128) * (output_price.max(0) as i128)
        + (usage.cached_input.max(0) as i128) * (cached_rate as i128);
    // Rates are per 1M tokens; divide before converting to quota, rounding up
    // so sub-unit usage still costs something.
    let per_unit = TOKENS_PER_PRICE_UNIT as i128;
    let micro_usd = ((micro + per_unit - 1) / per_unit).clamp(0, i64::MAX as i128) as i64;
    rate.quota_for_micro_usd(micro_usd)
}

// ---------------------------------------------------------------------------
// balance + deduction
// ---------------------------------------------------------------------------

/// Structured metadata written alongside every ledger row so the cost-stats
/// dashboard can `GROUP BY model` / protocol / kind without parsing the
/// free-form `reason` string. Rows written before these columns existed carry
/// NULL — those buckets become "unknown" in the dashboard.
pub struct LedgerMeta<'a> {
    /// One of "chat", "image", "video", "grant", "recharge", "adjust".
    pub kind: &'a str,
    /// "openai" / "claude" / "gemini" when applicable.
    pub protocol: Option<&'a str>,
    pub model: Option<&'a str>,
    /// Token counts behind a chat charge; zero elsewhere.
    pub usage: TokenUsage,
}

impl<'a> LedgerMeta<'a> {
    /// Chat charge carrying the token counts it was computed from.
    pub fn chat_usage(protocol: &'a str, model: &'a str, usage: TokenUsage) -> Self {
        Self { kind: "chat", protocol: Some(protocol), model: Some(model), usage }
    }
    pub fn image(protocol: &'a str, model: &'a str) -> Self {
        Self { kind: "image", protocol: Some(protocol), model: Some(model), usage: TokenUsage::default() }
    }
    pub fn refund_image(model: &'a str) -> Self {
        Self { kind: "image", protocol: None, model: Some(model), usage: TokenUsage::default() }
    }
    pub fn video(model: &'a str) -> Self {
        Self { kind: "video", protocol: Some("openai"), model: Some(model), usage: TokenUsage::default() }
    }
    /// Refund of a video deduction — kept under "video" kind so net-spend
    /// math (sum deltas where kind='video') stays correct.
    pub fn refund_video(model: &'a str) -> Self {
        Self { kind: "video", protocol: None, model: Some(model), usage: TokenUsage::default() }
    }
    pub fn grant() -> Self {
        Self { kind: "grant", protocol: None, model: None, usage: TokenUsage::default() }
    }
    pub fn recharge() -> Self {
        Self { kind: "recharge", protocol: None, model: None, usage: TokenUsage::default() }
    }
    pub fn adjust() -> Self {
        Self { kind: "adjust", protocol: None, model: None, usage: TokenUsage::default() }
    }
}

const LEDGER_INSERT_SQL: &str =
    "INSERT INTO balance_ledger (user_id, delta, reason, kind, protocol, model,
                                 input_tokens, output_tokens, cached_tokens)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)";

async fn insert_ledger(
    pool: &Pool,
    kind: DbKind,
    user_id: i64,
    delta: i64,
    reason: &str,
    meta: &LedgerMeta<'_>,
) {
    let sql = db::q(kind, LEDGER_INSERT_SQL);
    let _ = sqlx::query(&sql)
        .bind(user_id)
        .bind(delta)
        .bind(reason)
        .bind(meta.kind)
        .bind(meta.protocol)
        .bind(meta.model)
        .bind(meta.usage.input)
        .bind(meta.usage.output)
        .bind(meta.usage.cached_input)
        .execute(pool)
        .await;
}

/// Ensures a user_balances row exists. Returns the user's current balance.
pub async fn ensure_account(pool: &Pool, kind: DbKind, user_id: i64) -> Result<i64, sqlx::Error> {
    let sql = db::q(kind, "SELECT balance FROM user_balances WHERE user_id = ?");
    let existing: Option<(i64,)> = sqlx::query_as(&sql)
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    if let Some((b,)) = existing {
        return Ok(b);
    }

    let initial = get_setting_i64(pool, kind, "signup_grant", DEFAULT_SIGNUP_GRANT)
        .await
        .max(0);
    let ins = db::q(kind, "INSERT INTO user_balances (user_id, balance) VALUES (?, ?)");
    // Best-effort; race is harmless (UNIQUE on user_id).
    let _ = sqlx::query(&ins)
        .bind(user_id)
        .bind(initial)
        .execute(pool)
        .await;

    if initial > 0 {
        insert_ledger(pool, kind, user_id, initial, "signup_grant", &LedgerMeta::grant()).await;
    }

    let sql2 = db::q(kind, "SELECT balance FROM user_balances WHERE user_id = ?");
    let (b,): (i64,) = sqlx::query_as(&sql2)
        .bind(user_id)
        .fetch_one(pool)
        .await?;
    Ok(b)
}

pub async fn get_balance(pool: &Pool, kind: DbKind, user_id: i64) -> i64 {
    ensure_account(pool, kind, user_id).await.unwrap_or(0)
}

/// Gate a request that will be billed *after* the fact (chat). Token counts
/// are unknown up front, so the only sane admission test is "the account is
/// not already in the red". Settlement may overshoot into a small negative
/// balance; the next request is then refused until the user tops up.
pub async fn has_spendable_balance(pool: &Pool, kind: DbKind, user_id: i64) -> Result<i64, i64> {
    let balance = get_balance(pool, kind, user_id).await;
    if balance > 0 { Ok(balance) } else { Err(balance) }
}

/// Atomically deduct `cost` quota if the balance is sufficient.
/// Returns `Ok(new_balance)` on success, `Err(current_balance)` on insufficient funds.
pub async fn try_deduct(
    pool: &Pool,
    kind: DbKind,
    user_id: i64,
    cost: i64,
    reason: &str,
    meta: &LedgerMeta<'_>,
) -> Result<i64, i64> {
    if cost <= 0 {
        let b = ensure_account(pool, kind, user_id).await.unwrap_or(0);
        if cost == 0 {
            // Write a zero-delta ledger row so a free call is still recorded —
            // keeps the audit trail complete and the ledger reconcilable
            // against the balance.
            insert_ledger(pool, kind, user_id, 0, reason, meta).await;
        }
        return Ok(b);
    }
    if ensure_account(pool, kind, user_id).await.is_err() {
        return Err(0);
    }

    let now = db::now_expr(kind);
    let sql = db::q(
        kind,
        &format!(
            "UPDATE user_balances
             SET balance = balance - ?, lifetime_used = lifetime_used + ?, updated_at = {now}
             WHERE user_id = ? AND balance >= ?"
        ),
    );
    let affected = sqlx::query(&sql)
        .bind(cost)
        .bind(cost)
        .bind(user_id)
        .bind(cost)
        .execute(pool)
        .await
        .map(|r| r.rows_affected())
        .unwrap_or(0);

    if affected == 0 {
        let bal_sql = db::q(kind, "SELECT balance FROM user_balances WHERE user_id = ?");
        let b: Option<(i64,)> = sqlx::query_as(&bal_sql)
            .bind(user_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
        return Err(b.map(|(v,)| v).unwrap_or(0));
    }

    insert_ledger(pool, kind, user_id, -cost, reason, meta).await;

    let bal_sql = db::q(kind, "SELECT balance FROM user_balances WHERE user_id = ?");
    let (b,): (i64,) = sqlx::query_as(&bal_sql)
        .bind(user_id)
        .fetch_one(pool)
        .await
        .unwrap_or((0,));
    Ok(b)
}

/// Settle an already-delivered response. Unlike [`try_deduct`] this never
/// fails on insufficient funds: the tokens were consumed upstream and must be
/// charged, so the balance is allowed to go negative by at most one request.
pub async fn settle(
    pool: &Pool,
    kind: DbKind,
    user_id: i64,
    cost: i64,
    reason: &str,
    meta: &LedgerMeta<'_>,
) -> i64 {
    if ensure_account(pool, kind, user_id).await.is_err() {
        return 0;
    }
    if cost <= 0 {
        insert_ledger(pool, kind, user_id, 0, reason, meta).await;
        return get_balance(pool, kind, user_id).await;
    }

    let now = db::now_expr(kind);
    let sql = db::q(
        kind,
        &format!(
            "UPDATE user_balances
             SET balance = balance - ?, lifetime_used = lifetime_used + ?, updated_at = {now}
             WHERE user_id = ?"
        ),
    );
    if sqlx::query(&sql)
        .bind(cost)
        .bind(cost)
        .bind(user_id)
        .execute(pool)
        .await
        .is_err()
    {
        return get_balance(pool, kind, user_id).await;
    }

    insert_ledger(pool, kind, user_id, -cost, reason, meta).await;
    get_balance(pool, kind, user_id).await
}

pub async fn grant(
    pool: &Pool,
    kind: DbKind,
    user_id: i64,
    delta: i64,
    reason: &str,
    meta: &LedgerMeta<'_>,
) -> Result<i64, sqlx::Error> {
    ensure_account(pool, kind, user_id).await?;
    let now = db::now_expr(kind);
    let sql = db::q(
        kind,
        &format!(
            "UPDATE user_balances
             SET balance = balance + ?, updated_at = {now}
             WHERE user_id = ?"
        ),
    );
    sqlx::query(&sql)
        .bind(delta)
        .bind(user_id)
        .execute(pool)
        .await?;
    insert_ledger(pool, kind, user_id, delta, reason, meta).await;
    let bal_sql = db::q(kind, "SELECT balance FROM user_balances WHERE user_id = ?");
    let (b,): (i64,) = sqlx::query_as(&bal_sql)
        .bind(user_id)
        .fetch_one(pool)
        .await?;
    Ok(b)
}

/// Overwrite the balance with an exact value (admin action).
pub async fn set_balance(
    pool: &Pool,
    kind: DbKind,
    user_id: i64,
    new_balance: i64,
    reason: &str,
    meta: &LedgerMeta<'_>,
) -> Result<i64, sqlx::Error> {
    let before = ensure_account(pool, kind, user_id).await?;
    let delta = new_balance - before;
    let now = db::now_expr(kind);
    let sql = db::q(
        kind,
        &format!(
            "UPDATE user_balances
             SET balance = ?, updated_at = {now}
             WHERE user_id = ?"
        ),
    );
    sqlx::query(&sql)
        .bind(new_balance)
        .bind(user_id)
        .execute(pool)
        .await?;
    if delta != 0 {
        insert_ledger(pool, kind, user_id, delta, reason, meta).await;
    }
    Ok(new_balance)
}

// ---------------------------------------------------------------------------
// user-facing endpoints
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct BalanceMe {
    balance: i64,
    lifetime_used: i64,
    quota_per_usd: i64,
    price_multiplier_percent: i64,
}

async fn get_my_balance(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
) -> Response {
    let _ = ensure_account(&installed.pool, installed.kind, user.id).await;
    let sql = db::q(
        installed.kind,
        "SELECT balance, lifetime_used FROM user_balances WHERE user_id = ?",
    );
    let row: (i64, i64) = sqlx::query_as(&sql)
        .bind(user.id)
        .fetch_one(&installed.pool)
        .await
        .unwrap_or((0, 0));
    let rate = QuotaRate::load(&installed.pool, installed.kind).await;
    Json(BalanceMe {
        balance: row.0,
        lifetime_used: row.1,
        quota_per_usd: rate.quota_per_usd,
        price_multiplier_percent: rate.multiplier_percent,
    })
    .into_response()
}

#[derive(Serialize)]
struct LedgerEntry {
    id: i64,
    delta: i64,
    reason: String,
    created_at: String,
    kind: Option<String>,
    model: Option<String>,
    input_tokens: i64,
    output_tokens: i64,
    cached_tokens: i64,
}

#[derive(Deserialize)]
struct LedgerQuery {
    page: Option<i64>,
}

type LedgerRow = (i64, i64, String, String, Option<String>, Option<String>, i64, i64, i64);

async fn get_my_ledger(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Query(q): Query<LedgerQuery>,
) -> Response {
    let page = q.page.unwrap_or(1).max(1);
    let per_page: i64 = 30;
    let offset = (page - 1) * per_page;
    let sql = db::q(
        installed.kind,
        "SELECT id, delta, reason, created_at, kind, model,
                input_tokens, output_tokens, cached_tokens
         FROM balance_ledger
         WHERE user_id = ?
         ORDER BY created_at DESC, id DESC
         LIMIT ? OFFSET ?",
    );
    let rows: Result<Vec<LedgerRow>, _> = sqlx::query_as(&sql)
        .bind(user.id)
        .bind(per_page)
        .bind(offset)
        .fetch_all(&installed.pool)
        .await;
    match rows {
        Ok(rs) => {
            let out: Vec<LedgerEntry> = rs
                .into_iter()
                .map(|(id, delta, reason, created_at, kind, model, inp, outp, cached)| LedgerEntry {
                    id,
                    delta,
                    reason,
                    created_at,
                    kind,
                    model,
                    input_tokens: inp,
                    output_tokens: outp,
                    cached_tokens: cached,
                })
                .collect();
            Json(out).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ---------------------------------------------------------------------------
// stats dashboard — shared between user (`/quota/stats`) and admin
// (`/admin/quota/stats`)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct StatsQuery {
    /// One of "7d" (default), "30d", "90d", "all".
    period: Option<String>,
}

#[derive(Serialize)]
struct DailyPoint {
    date: String,
    spent: i64,
    refunded: i64,
}

#[derive(Serialize)]
struct ModelBucket {
    model: String,
    kind: String,
    protocol: Option<String>,
    count: i64,
    spent: i64,
    refunded: i64,
    input_tokens: i64,
    output_tokens: i64,
}

#[derive(Serialize)]
struct KindBucket {
    kind: String,
    total: i64,
}

#[derive(Serialize)]
struct StatsResponse {
    period: String,
    start: String,
    spent: i64,
    refunded: i64,
    net_spent: i64,
    granted: i64,
    recharged: i64,
    input_tokens: i64,
    output_tokens: i64,
    daily: Vec<DailyPoint>,
    by_model: Vec<ModelBucket>,
    by_kind: Vec<KindBucket>,
}

#[derive(Serialize)]
struct TopUserRow {
    user_id: i64,
    username: String,
    spent: i64,
    refunded: i64,
}

#[derive(Serialize)]
struct AdminStatsResponse {
    #[serde(flatten)]
    base: StatsResponse,
    top_users: Vec<TopUserRow>,
}

/// Returns `(cutoff_iso, normalized_period_label)`. Cutoff is a UTC timestamp
/// in `YYYY-MM-DD HH:MM:SS` form, which sorts correctly against all three
/// dialects' stored datetime strings. "all" maps to a 100-year window.
fn period_window(period: Option<&str>) -> (String, String) {
    let raw = period.unwrap_or("7d");
    let (days, label): (i64, &str) = match raw {
        "30d" => (30, "30d"),
        "90d" => (90, "90d"),
        "all" => (36500, "all"),
        _ => (7, "7d"),
    };
    let cutoff = chrono::Utc::now() - chrono::Duration::days(days);
    let iso = cutoff.format("%Y-%m-%d %H:%M:%S").to_string();
    (iso, label.to_string())
}

/// Usage kinds that count as "spend" in the dashboard. Grants, recharges and
/// admin adjustments are reported separately.
const SPEND_KINDS: &str = "('chat', 'image', 'video')";

/// Fills in the same StatsResponse used by both /quota/stats and the admin
/// equivalent — caller supplies the `user_id = ?` filter (or empty for global)
/// and the binding tuple.
async fn build_stats(
    pool: &Pool,
    kind: DbKind,
    user_filter_sql: &str,
    user_id_bind: Option<i64>,
    start: &str,
) -> Result<StatsResponse, sqlx::Error> {
    // Totals — one row. CASE clauses partition the ledger into "spent" /
    // "refunded" / "granted" / "recharged" buckets so each shows up
    // independently in the summary cards, plus overall token counters.
    let totals_sql = db::q(
        kind,
        &format!(
            "SELECT
               COALESCE(SUM(CASE WHEN delta < 0 AND kind IN {SPEND_KINDS} THEN -delta ELSE 0 END), 0),
               COALESCE(SUM(CASE WHEN delta > 0 AND kind IN {SPEND_KINDS} THEN delta ELSE 0 END), 0),
               COALESCE(SUM(CASE WHEN delta > 0 AND kind = 'grant' THEN delta ELSE 0 END), 0),
               COALESCE(SUM(CASE WHEN delta > 0 AND kind = 'recharge' THEN delta ELSE 0 END), 0),
               COALESCE(SUM(input_tokens), 0),
               COALESCE(SUM(output_tokens), 0)
             FROM balance_ledger
             WHERE created_at >= ?{user_filter_sql}"
        ),
    );
    let mut q1 = sqlx::query_as::<_, (i64, i64, i64, i64, i64, i64)>(&totals_sql).bind(start);
    if let Some(uid) = user_id_bind {
        q1 = q1.bind(uid);
    }
    let (spent, refunded, granted, recharged, input_tokens, output_tokens) =
        q1.fetch_one(pool).await?;

    // Daily breakdown. day_bucket truncates the timestamp to YYYY-MM-DD per
    // dialect; aliasing it as `day` lets GROUP/ORDER use the same name.
    let day = db::day_bucket(kind, "created_at");
    let daily_sql = db::q(
        kind,
        &format!(
            "SELECT {day} AS day,
                    COALESCE(SUM(CASE WHEN delta < 0 AND kind IN {SPEND_KINDS} THEN -delta ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN delta > 0 AND kind IN {SPEND_KINDS} THEN delta ELSE 0 END), 0)
             FROM balance_ledger
             WHERE created_at >= ?{user_filter_sql}
             GROUP BY day
             ORDER BY day ASC"
        ),
    );
    let mut q2 = sqlx::query_as::<_, (String, i64, i64)>(&daily_sql).bind(start);
    if let Some(uid) = user_id_bind {
        q2 = q2.bind(uid);
    }
    let daily: Vec<DailyPoint> = q2
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|(date, spent, refunded)| DailyPoint { date, spent, refunded })
        .collect();

    // By model — top 20 by spend. Order by column position (works across
    // dialects without aliasing).
    let model_sql = db::q(
        kind,
        &format!(
            "SELECT model, kind, protocol,
                    COUNT(CASE WHEN delta < 0 THEN 1 END),
                    COALESCE(SUM(CASE WHEN delta < 0 THEN -delta ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN delta > 0 THEN delta ELSE 0 END), 0),
                    COALESCE(SUM(input_tokens), 0),
                    COALESCE(SUM(output_tokens), 0)
             FROM balance_ledger
             WHERE created_at >= ? AND model IS NOT NULL AND kind IN {SPEND_KINDS}{user_filter_sql}
             GROUP BY model, kind, protocol
             ORDER BY 5 DESC
             LIMIT 20"
        ),
    );
    let mut q3 =
        sqlx::query_as::<_, (String, String, Option<String>, i64, i64, i64, i64, i64)>(&model_sql)
            .bind(start);
    if let Some(uid) = user_id_bind {
        q3 = q3.bind(uid);
    }
    let by_model: Vec<ModelBucket> = q3
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|(model, kind, protocol, count, spent, refunded, inp, outp)| ModelBucket {
            model,
            kind,
            protocol,
            count,
            spent,
            refunded,
            input_tokens: inp,
            output_tokens: outp,
        })
        .collect();

    // By kind — total signed delta per kind (NULL kinds skipped).
    let kind_sql = db::q(
        kind,
        &format!(
            "SELECT kind, COALESCE(SUM(delta), 0)
             FROM balance_ledger
             WHERE created_at >= ? AND kind IS NOT NULL{user_filter_sql}
             GROUP BY kind"
        ),
    );
    let mut q4 = sqlx::query_as::<_, (String, i64)>(&kind_sql).bind(start);
    if let Some(uid) = user_id_bind {
        q4 = q4.bind(uid);
    }
    let by_kind: Vec<KindBucket> = q4
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|(kind, total)| KindBucket { kind, total })
        .collect();

    Ok(StatsResponse {
        period: String::new(), // filled by caller
        start: start.to_string(),
        spent,
        refunded,
        net_spent: spent - refunded,
        granted,
        recharged,
        input_tokens,
        output_tokens,
        daily,
        by_model,
        by_kind,
    })
}

async fn get_my_stats(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Query(q): Query<StatsQuery>,
) -> Response {
    let (start, period) = period_window(q.period.as_deref());
    match build_stats(
        &installed.pool,
        installed.kind,
        " AND user_id = ?",
        Some(user.id),
        &start,
    )
    .await
    {
        Ok(mut s) => {
            s.period = period;
            Json(s).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn admin_get_stats(
    Extension(installed): Extension<InstalledState>,
    Query(q): Query<StatsQuery>,
) -> Response {
    let (start, period) = period_window(q.period.as_deref());
    let base = match build_stats(&installed.pool, installed.kind, "", None, &start).await {
        Ok(mut s) => {
            s.period = period;
            s
        }
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    // Top users by net spend within the period.
    let top_sql = db::q(
        installed.kind,
        &format!(
            "SELECT l.user_id, u.username,
                    COALESCE(SUM(CASE WHEN l.delta < 0 AND l.kind IN {SPEND_KINDS} THEN -l.delta ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN l.delta > 0 AND l.kind IN {SPEND_KINDS} THEN l.delta ELSE 0 END), 0)
             FROM balance_ledger l
             JOIN users u ON u.id = l.user_id
             WHERE l.created_at >= ?
             GROUP BY l.user_id, u.username
             ORDER BY 3 DESC
             LIMIT 20"
        ),
    );
    let top_rows: Vec<(i64, String, i64, i64)> = sqlx::query_as(&top_sql)
        .bind(&start)
        .fetch_all(&installed.pool)
        .await
        .unwrap_or_default();
    let top_users: Vec<TopUserRow> = top_rows
        .into_iter()
        .filter(|(_, _, s, _)| *s > 0)
        .map(|(user_id, username, spent, refunded)| TopUserRow {
            user_id,
            username,
            spent,
            refunded,
        })
        .collect();

    Json(AdminStatsResponse { base, top_users }).into_response()
}

// ---------------------------------------------------------------------------
// admin endpoints
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct AdminSettingsView {
    registration_enabled: bool,
    signup_grant: i64,
    quota_per_usd: i64,
    price_multiplier_percent: i64,
    invite_grant_inviter: i64,
    invite_grant_invitee: i64,
    // email verification + SMTP
    email_verification_required: bool,
    smtp_host: String,
    smtp_port: i64,
    smtp_username: String,
    smtp_from_email: String,
    smtp_from_name: String,
    smtp_security: String,
    smtp_password_set: bool,
}

async fn admin_get_settings(Extension(installed): Extension<InstalledState>) -> Response {
    let pool = &installed.pool;
    let kind = installed.kind;

    async fn s(pool: &Pool, kind: DbKind, key: &str) -> String {
        get_setting(pool, kind, key).await.unwrap_or_default()
    }
    async fn has(pool: &Pool, kind: DbKind, key: &str) -> bool {
        get_setting(pool, kind, key)
            .await
            .map(|v| !v.is_empty())
            .unwrap_or(false)
    }

    let view = AdminSettingsView {
        registration_enabled: get_setting_bool(pool, kind, "registration_enabled", true).await,
        signup_grant: get_setting_i64(pool, kind, "signup_grant", DEFAULT_SIGNUP_GRANT).await,
        quota_per_usd: get_setting_i64(pool, kind, "quota_per_usd", DEFAULT_QUOTA_PER_USD).await,
        price_multiplier_percent: get_setting_i64(pool, kind, "price_multiplier_percent", 100).await,
        invite_grant_inviter: get_setting_i64(pool, kind, "invite_grant_inviter", DEFAULT_INVITE_GRANT).await,
        invite_grant_invitee: get_setting_i64(pool, kind, "invite_grant_invitee", DEFAULT_INVITE_GRANT).await,
        email_verification_required: get_setting_bool(pool, kind, "email_verification_required", false).await,
        smtp_host: s(pool, kind, "smtp_host").await,
        smtp_port: get_setting_i64(pool, kind, "smtp_port", 587).await,
        smtp_username: s(pool, kind, "smtp_username").await,
        smtp_from_email: s(pool, kind, "smtp_from_email").await,
        smtp_from_name: s(pool, kind, "smtp_from_name").await,
        smtp_security: {
            let raw = s(pool, kind, "smtp_security").await;
            if raw.is_empty() { "starttls".into() } else { raw }
        },
        smtp_password_set: has(pool, kind, "smtp_password").await,
    };
    Json(view).into_response()
}

#[derive(Deserialize)]
struct AdminSettingsUpdate {
    registration_enabled: Option<bool>,
    signup_grant: Option<i64>,
    quota_per_usd: Option<i64>,
    price_multiplier_percent: Option<i64>,
    invite_grant_inviter: Option<i64>,
    invite_grant_invitee: Option<i64>,
    // email verification + SMTP
    email_verification_required: Option<bool>,
    smtp_host: Option<String>,
    smtp_port: Option<i64>,
    smtp_username: Option<String>,
    smtp_password: Option<String>,
    smtp_from_email: Option<String>,
    smtp_from_name: Option<String>,
    smtp_security: Option<String>,
}

async fn admin_patch_settings(
    Extension(installed): Extension<InstalledState>,
    Json(body): Json<AdminSettingsUpdate>,
) -> Response {
    let pool = &installed.pool;
    let kind = installed.kind;

    async fn maybe_set(
        pool: &Pool,
        kind: DbKind,
        key: &str,
        val: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        if let Some(v) = val {
            set_setting(pool, kind, key, v).await?;
        }
        Ok(())
    }

    let ops: Vec<(&str, Option<String>)> = vec![
        ("registration_enabled", body.registration_enabled.map(|b| b.to_string())),
        ("signup_grant", body.signup_grant.map(|v| v.max(0).to_string())),
        // 0 would make every model free; clamp to at least 1 quota per USD.
        ("quota_per_usd", body.quota_per_usd.map(|v| v.max(1).to_string())),
        ("price_multiplier_percent", body.price_multiplier_percent.map(|v| v.max(0).to_string())),
        ("invite_grant_inviter", body.invite_grant_inviter.map(|v| v.max(0).to_string())),
        ("invite_grant_invitee", body.invite_grant_invitee.map(|v| v.max(0).to_string())),
        ("email_verification_required", body.email_verification_required.map(|b| b.to_string())),
        ("smtp_host", body.smtp_host.map(|s| s.trim().to_string())),
        ("smtp_port", body.smtp_port.map(|v| v.clamp(1, 65535).to_string())),
        ("smtp_username", body.smtp_username.map(|s| s.trim().to_string())),
        ("smtp_password", body.smtp_password),
        ("smtp_from_email", body.smtp_from_email.map(|s| s.trim().to_string())),
        ("smtp_from_name", body.smtp_from_name.map(|s| s.trim().to_string())),
        ("smtp_security", body.smtp_security.map(|s| s.trim().to_lowercase())),
    ];
    for (k, v) in ops {
        if let Err(e) = maybe_set(pool, kind, k, v.as_deref()).await {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    }
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Serialize)]
struct AdminUserBalance {
    user_id: i64,
    username: String,
    balance: i64,
    lifetime_used: i64,
}

async fn admin_list_user_balances(Extension(installed): Extension<InstalledState>) -> Response {
    let sql = db::q(
        installed.kind,
        "SELECT u.id, u.username,
                COALESCE(c.balance, 0) AS balance,
                COALESCE(c.lifetime_used, 0) AS lifetime_used
         FROM users u
         LEFT JOIN user_balances c ON c.user_id = u.id
         ORDER BY u.id ASC",
    );
    let rows: Result<Vec<(i64, String, i64, i64)>, _> =
        sqlx::query_as(&sql).fetch_all(&installed.pool).await;
    match rows {
        Ok(rs) => {
            let out: Vec<AdminUserBalance> = rs
                .into_iter()
                .map(|(user_id, username, balance, lifetime_used)| AdminUserBalance {
                    user_id,
                    username,
                    balance,
                    lifetime_used,
                })
                .collect();
            Json(out).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
struct AdjustBalance {
    // either set an absolute balance OR add a delta
    balance: Option<i64>,
    delta: Option<i64>,
    reason: Option<String>,
}

async fn admin_adjust_balance(
    Extension(installed): Extension<InstalledState>,
    Path(user_id): Path<i64>,
    Json(body): Json<AdjustBalance>,
) -> Response {
    let reason = body.reason.unwrap_or_else(|| "admin_adjust".into());
    let meta = LedgerMeta::adjust();
    if let Some(b) = body.balance {
        if b < 0 {
            return (StatusCode::BAD_REQUEST, "balance must be >= 0").into_response();
        }
        match set_balance(&installed.pool, installed.kind, user_id, b, &reason, &meta).await {
            Ok(new) => return Json(serde_json::json!({ "balance": new })).into_response(),
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
    if let Some(d) = body.delta {
        match grant(&installed.pool, installed.kind, user_id, d, &reason, &meta).await {
            Ok(new) => return Json(serde_json::json!({ "balance": new })).into_response(),
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
    (StatusCode::BAD_REQUEST, "provide balance or delta").into_response()
}

// ---------------------------------------------------------------------------
// routes
// ---------------------------------------------------------------------------

pub fn user_routes() -> Router<AppState> {
    Router::new()
        .route("/quota/me", get(get_my_balance))
        .route("/quota/ledger", get(get_my_ledger))
        .route("/quota/stats", get(get_my_stats))
}

pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route("/admin/app-settings", get(admin_get_settings).patch(admin_patch_settings))
        .route("/admin/quota", get(admin_list_user_balances))
        .route("/admin/quota/stats", get(admin_get_stats))
        .route("/admin/quota/{id}", patch(admin_adjust_balance).post(admin_adjust_balance))
        .route(
            "/admin/email/test",
            axum::routing::post(crate::email::admin_send_test),
        )
        .route_layer(middleware::from_fn(admin::require_admin))
}

#[cfg(test)]
mod pricing_tests {
    use super::*;

    const RATE: QuotaRate = QuotaRate { quota_per_usd: 500_000, multiplier_percent: 100 };

    #[test]
    fn official_usd_rates_convert_to_whole_quota() {
        // GPT-5-class pricing: $1.25 in / $10 out per 1M tokens.
        // 1M input tokens = $1.25 -> 625_000 quota at 500k quota/USD.
        assert_eq!(
            chat_quota_cost(
                RATE,
                1_250_000,
                10_000_000,
                None,
                TokenUsage { input: 1_000_000, output: 0, cached_input: 0 },
            ),
            625_000
        );
        // 1M output tokens = $10 -> 5_000_000 quota.
        assert_eq!(
            chat_quota_cost(
                RATE,
                1_250_000,
                10_000_000,
                None,
                TokenUsage { input: 0, output: 1_000_000, cached_input: 0 },
            ),
            5_000_000
        );
    }

    #[test]
    fn cached_tokens_use_the_cached_rate_and_are_not_double_charged() {
        // 1M input of which 900k cached, cached rate is 1/10th of input.
        let usage = TokenUsage { input: 1_000_000, output: 0, cached_input: 900_000 };
        let cost = chat_quota_cost(RATE, 1_250_000, 10_000_000, Some(125_000), usage);
        // 100k * 1.25 + 900k * 0.125 per 1M = $0.125 + $0.1125 = $0.2375
        assert_eq!(cost, (0.2375_f64 * 500_000.0).round() as i64);
    }

    #[test]
    fn a_null_cached_rate_bills_cached_tokens_as_normal_input() {
        let usage = TokenUsage { input: 1_000_000, output: 0, cached_input: 400_000 };
        let with_null = chat_quota_cost(RATE, 1_250_000, 0, None, usage);
        let no_cache = chat_quota_cost(
            RATE,
            1_250_000,
            0,
            None,
            TokenUsage { input: 1_000_000, output: 0, cached_input: 0 },
        );
        assert_eq!(with_null, no_cache);
    }

    #[test]
    fn the_global_multiplier_marks_every_model_up() {
        let doubled = QuotaRate { quota_per_usd: 500_000, multiplier_percent: 200 };
        let usage = TokenUsage { input: 1_000_000, output: 0, cached_input: 0 };
        assert_eq!(
            chat_quota_cost(doubled, 1_250_000, 0, None, usage),
            2 * chat_quota_cost(RATE, 1_250_000, 0, None, usage)
        );
    }

    #[test]
    fn tiny_usage_still_costs_at_least_one_quota() {
        // A single token against a cheap model rounds up rather than to zero,
        // so high-frequency小额 calls can't be free.
        let usage = TokenUsage { input: 1, output: 0, cached_input: 0 };
        assert_eq!(chat_quota_cost(RATE, 150_000, 600_000, None, usage), 1);
    }

    #[test]
    fn a_free_model_costs_nothing() {
        let usage = TokenUsage { input: 5_000, output: 2_000, cached_input: 0 };
        assert_eq!(chat_quota_cost(RATE, 0, 0, None, usage), 0);
    }

    #[test]
    fn empty_usage_is_detected_so_failed_calls_are_not_billed() {
        assert!(TokenUsage::default().is_empty());
        assert!(TokenUsage { input: 0, output: 0, cached_input: 7 }.is_empty());
        assert!(!TokenUsage { input: 1, output: 0, cached_input: 0 }.is_empty());
    }
}
