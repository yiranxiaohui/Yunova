//! Multi-channel upstream + per-model pricing.
//!
//! Tables (see migrations/{sqlite,postgres}/0019_channels_pricing.sql):
//!   * `upstream_channels`  — admin-managed providers (protocol/base_url/api_key/enabled/priority)
//!   * `model_pricing`      — whitelist of callable models + their official-rate prices
//!   * `channel_models`     — which channels serve which model (optional upstream id alias)
//!
//! Prices are micro-USD (see [`crate::quota`]): chat models bill per token,
//! image models per call, and video models per second.
//!
//! See: docs/plans/2026-05-18-multi-channel-pricing.md

use axum::{
    Extension, Json, Router,
    extract::Path,
    http::{HeaderMap, StatusCode},
    middleware,
    response::{IntoResponse, Response},
    routing::{delete, get, patch},
};
use serde::{Deserialize, Serialize};

use crate::{
    AppState, InstalledState, admin,
    db::{self, DbKind, Pool},
};

// ---------------------------------------------------------------------------
// types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Channel {
    pub id: i64,
    pub name: String,
    pub protocol: String,   // 'openai' | 'claude' | 'gemini'
    pub base_url: String,
    pub api_key: String,
    pub enabled: bool,
    pub priority: i64,
}

/// A channel as the admin UI sees it: identical to [`Channel`] except the key
/// is a non-reversible hint instead of the secret.
///
/// A separate type rather than a skipped field, so adding a column to
/// `Channel` cannot silently start leaking it through this endpoint — the
/// mapping below has to be updated deliberately.
#[derive(Debug, Clone, Serialize)]
pub struct RedactedChannel {
    pub id: i64,
    pub name: String,
    pub protocol: String,
    pub base_url: String,
    /// Masked form, e.g. `sk-1…cdef`. Enough to tell two keys apart when
    /// checking which channel is misconfigured, useless if intercepted.
    pub api_key_hint: String,
    /// Whether a key is stored at all. The UI needs this to distinguish "not
    /// configured" from "configured but hidden".
    pub has_api_key: bool,
    pub enabled: bool,
    pub priority: i64,
}

impl From<&Channel> for RedactedChannel {
    fn from(c: &Channel) -> Self {
        Self {
            id: c.id,
            name: c.name.clone(),
            protocol: c.protocol.clone(),
            base_url: c.base_url.clone(),
            api_key_hint: mask_secret(&c.api_key),
            has_api_key: !c.api_key.trim().is_empty(),
            enabled: c.enabled,
            priority: c.priority,
        }
    }
}

/// Mask a secret to a recognisable but unusable hint.
///
/// Short values are masked entirely rather than partially: revealing 4 of 8
/// characters is a meaningful fraction of the search space, while revealing 4
/// of 48 is not.
fn mask_secret(secret: &str) -> String {
    let s = secret.trim();
    let chars: Vec<char> = s.chars().collect();
    if chars.is_empty() {
        return String::new();
    }
    if chars.len() <= 12 {
        return "…".repeat(3);
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}…{tail}")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SizeRule {
    pub size: String,
    /// Percent multiplier, 100 = 1.0x.
    pub multiplier: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPrice {
    pub id: i64,
    pub model: String,
    pub kind: String,
    pub display_name: Option<String>,
    pub enabled: bool,
    pub protocol: String,
    pub context_limit: Option<i64>,
    // chat billing: micro-USD per 1M tokens, mirroring published provider rates.
    // `cached_input_price` is None when the provider has no prompt-cache discount.
    pub input_price: i64,
    pub output_price: i64,
    pub cached_input_price: Option<i64>,
    /// image billing: micro-USD per generation call.
    pub per_call_price: i64,
    // video billing: cost = (base_price + per_second_price * seconds) * size multiplier
    pub base_price: i64,
    pub per_second_price: i64,
    pub allowed_seconds: Option<Vec<i64>>,
    pub size_rules: Option<Vec<SizeRule>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ChannelModel {
    pub channel_id: i64,
    pub model: String,
    pub upstream_id: Option<String>,
}

// ---------------------------------------------------------------------------
// channel queries
// ---------------------------------------------------------------------------

pub async fn list_channels(pool: &Pool, kind: DbKind) -> Result<Vec<Channel>, sqlx::Error> {
    // `enabled` is an integer column on both backends and decodes as i64;
    // it is converted to bool below.
    let sql = db::q(
        kind,
        "SELECT id, name, protocol, base_url, api_key, enabled, priority \
         FROM upstream_channels ORDER BY priority ASC, id ASC",
    );
    let rows: Vec<(i64, String, String, String, String, i64, i64)> =
        sqlx::query_as(&sql).fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|(id, name, protocol, base_url, api_key, enabled, priority)| Channel {
            id,
            name,
            protocol,
            base_url,
            api_key,
            enabled: enabled != 0,
            priority,
        })
        .collect())
}

/// All enabled channels that can serve `model`, sorted by priority ascending.
///
/// Routing first looks for explicit `channel_models` bindings for the model:
/// those act as routing restrictions and optional upstream aliases. When the
/// model has no binding (for example, a legacy or custom model), we fall back
/// to every enabled channel. `resolve_route` then narrows that set to the
/// channels whose protocol matches the wire protocol
/// the client declared (derived from `model_pricing.protocol` via the picker),
/// so auto-routing lands on the correct provider without manual binding.
#[allow(dead_code)] // consumed by Task 2.1 select_channel_for_model
pub async fn channels_for_model(
    pool: &Pool,
    kind: DbKind,
    model: &str,
) -> Result<Vec<(Channel, Option<String>)>, sqlx::Error> {
    let explicit_sql = db::q(
        kind,
        "SELECT c.id, c.name, c.protocol, c.base_url, c.api_key, \
             c.enabled, c.priority, cm.upstream_id \
             FROM upstream_channels c \
             INNER JOIN channel_models cm ON cm.channel_id = c.id \
             WHERE cm.model = ? AND c.enabled = 1 \
             ORDER BY c.priority ASC, c.id ASC",
    );
    let rows: Vec<(i64, String, String, String, String, i64, i64, Option<String>)> =
        sqlx::query_as(&explicit_sql)
            .bind(model)
            .fetch_all(pool)
            .await?;

    if !rows.is_empty() {
        return Ok(rows
            .into_iter()
            .map(|(id, name, protocol, base_url, api_key, enabled, priority, upstream_id)| {
                (
                    Channel {
                        id,
                        name,
                        protocol,
                        base_url,
                        api_key,
                        enabled: enabled != 0,
                        priority,
                    },
                    upstream_id,
                )
            })
            .collect());
    }

    // A binding remains a routing restriction even when every bound channel
    // is disabled. Do not silently fall back to an unrelated channel in that
    // case.
    if model_has_explicit_binding(pool, kind, model).await? {
        return Ok(Vec::new());
    }

    // No explicit binding → auto-route to all enabled channels.
    // upstream_id is None (send the model name as-is); resolve_route narrows
    // this set to the matching protocol afterwards.
    let fallback_sql = db::q(
        kind,
        "SELECT c.id, c.name, c.protocol, c.base_url, c.api_key, \
         c.enabled, c.priority \
         FROM upstream_channels c \
         WHERE c.enabled = 1 \
         ORDER BY c.priority ASC, c.id ASC",
    );
    let fb: Vec<(i64, String, String, String, String, i64, i64)> =
        sqlx::query_as(&fallback_sql).fetch_all(pool).await?;
    Ok(fb
        .into_iter()
        .map(|(id, name, protocol, base_url, api_key, enabled, priority)| {
            (
                Channel {
                    id,
                    name,
                    protocol,
                    base_url,
                    api_key,
                    enabled: enabled != 0,
                    priority,
                },
                None,
            )
        })
        .collect())
}

async fn model_has_explicit_binding(
    pool: &Pool,
    kind: DbKind,
    model: &str,
) -> Result<bool, sqlx::Error> {
    let sql = db::q(
        kind,
        "SELECT COUNT(*) FROM channel_models WHERE model = ?",
    );
    let (count,): (i64,) = sqlx::query_as(&sql)
        .bind(model)
        .fetch_one(pool)
        .await?;
    Ok(count > 0)
}

/// Resolved channel + the model id to send upstream (alias or original).
#[allow(dead_code)] // consumed by Task 2.2 chat/image callers
#[derive(Debug, Clone)]
pub struct ChannelChoice {
    pub channel: Channel,
    /// Upstream model identifier — channel_models.upstream_id if set, else `model`.
    pub upstream_model: String,
}

/// Priority-sorted list of channels that can serve a model.
///
/// Caller iterates the list; on 5xx / network error, falls back to the next.
/// Returns an empty vec when explicit bindings exist but none are enabled.
pub async fn select_chain(
    pool: &Pool,
    kind: DbKind,
    model: &str,
) -> Result<Vec<ChannelChoice>, sqlx::Error> {
    let rows = channels_for_model(pool, kind, model).await?;
    Ok(rows
        .into_iter()
        .map(|(channel, upstream_id)| ChannelChoice {
            upstream_model: upstream_id.unwrap_or_else(|| model.to_string()),
            channel,
        })
        .collect())
}

/// Resolve a model against the catalogs advertised by otherwise eligible
/// channels.
///
/// Explicit `channel_models` bindings still decide *which* channels are
/// eligible — `select_chain` applies them first — but they can't vouch for a
/// model the upstream no longer offers, so the catalog check runs either way.
/// That keeps this in step with `resolve_route` and the user-facing listings,
/// which apply the same rule.
pub async fn select_chain_by_advertised_model(
    http: &reqwest::Client,
    pool: &Pool,
    kind: DbKind,
    model: &str,
) -> Result<Vec<ChannelChoice>, sqlx::Error> {
    let candidates = select_chain(pool, kind, model).await?;
    if candidates.is_empty() {
        return Ok(candidates);
    }

    Ok(narrow_to_advertised(
        probe_chain_catalogs(http, candidates).await,
    ))
}

/// Attach each candidate channel's cached catalog to it.
async fn probe_chain_catalogs(
    http: &reqwest::Client,
    chain: Vec<ChannelChoice>,
) -> Vec<(ChannelChoice, Result<Vec<String>, String>)> {
    let probes = chain.into_iter().map(|choice| async move {
        let advertised = cached_channel_catalog(http, &choice.channel, false).await;
        (choice, advertised)
    });
    futures_util::future::join_all(probes).await
}

/// Narrow a chain to the channels that can still serve the model, so a request
/// is never sent to an upstream whose catalog no longer lists it. Priority
/// order is preserved.
///
/// Channels that advertise the model win. Only when none of them does do the
/// channels with an unreadable catalog stand in, so a provider that hides or
/// rate-limits its `/models` endpoint can't make a working model unusable. The
/// chain therefore empties only when every catalog was readable and none of
/// them advertised the model.
fn narrow_to_advertised(
    results: Vec<(ChannelChoice, Result<Vec<String>, String>)>,
) -> Vec<ChannelChoice> {
    let mut advertising: Vec<ChannelChoice> = Vec::new();
    let mut unknown: Vec<ChannelChoice> = Vec::new();
    for (choice, advertised) in results {
        match advertised {
            Ok(models) if models.iter().any(|m| m == &choice.upstream_model) => {
                advertising.push(choice);
            }
            Ok(_) => {}
            Err(_) => unknown.push(choice),
        }
    }
    if advertising.is_empty() { unknown } else { advertising }
}

/// Convenience: pick the top-priority channel that advertises `model`.
pub async fn select_one_by_advertised_model(
    http: &reqwest::Client,
    pool: &Pool,
    kind: DbKind,
    model: &str,
) -> Result<Option<ChannelChoice>, sqlx::Error> {
    Ok(select_chain_by_advertised_model(http, pool, kind, model)
        .await?
        .into_iter()
        .next())
}

/// Convenience: just the top-priority enabled channel, or None.
#[allow(dead_code)] // consumed by Task 2.2 single-shot callers (no fallback needed)
pub async fn select_one(
    pool: &Pool,
    kind: DbKind,
    model: &str,
) -> Result<Option<ChannelChoice>, sqlx::Error> {
    Ok(select_chain(pool, kind, model).await?.into_iter().next())
}

#[derive(Debug, Deserialize)]
pub struct ChannelInput {
    pub name: String,
    pub protocol: String,
    pub base_url: String,
    pub api_key: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_priority")]
    pub priority: i64,
}
fn default_true() -> bool { true }
fn default_priority() -> i64 { 100 }

pub async fn create_channel(
    pool: &Pool,
    kind: DbKind,
    input: &ChannelInput,
) -> Result<i64, sqlx::Error> {
    let sql = db::q(
        kind,
        "INSERT INTO upstream_channels (name, protocol, base_url, api_key, enabled, priority) \
         VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
    );
    // `enabled` is an integer column on both backends, so one binding works.
    let enabled_v: i64 = if input.enabled { 1 } else { 0 };
    let row: (i64,) = sqlx::query_as(&sql)
        .bind(&input.name)
        .bind(&input.protocol)
        .bind(&input.base_url)
        .bind(&input.api_key)
        .bind(enabled_v)
        .bind(input.priority)
        .fetch_one(pool)
        .await?;
    Ok(row.0)
}

#[derive(Debug, Deserialize, Default)]
pub struct ChannelPatch {
    pub name: Option<String>,
    pub protocol: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub enabled: Option<bool>,
    pub priority: Option<i64>,
}

pub async fn update_channel(
    pool: &Pool,
    kind: DbKind,
    id: i64,
    patch: &ChannelPatch,
) -> Result<(), sqlx::Error> {
    // Build SET clause dynamically.
    let mut sets: Vec<&str> = Vec::new();
    if patch.name.is_some()     { sets.push("name = ?"); }
    if patch.protocol.is_some() { sets.push("protocol = ?"); }
    if patch.base_url.is_some() { sets.push("base_url = ?"); }
    if patch.api_key.is_some()  { sets.push("api_key = ?"); }
    if patch.enabled.is_some()  { sets.push("enabled = ?"); }
    if patch.priority.is_some() { sets.push("priority = ?"); }
    if sets.is_empty() {
        return Ok(());
    }
    let now = db::now_expr(kind);
    let sql = db::q(
        kind,
        &format!(
            "UPDATE upstream_channels SET {}, updated_at = {now} WHERE id = ?",
            sets.join(", ")
        ),
    );
    let mut q = sqlx::query(&sql);
    if let Some(v) = &patch.name     { q = q.bind(v); }
    if let Some(v) = &patch.protocol { q = q.bind(v); }
    if let Some(v) = &patch.base_url { q = q.bind(v); }
    if let Some(v) = &patch.api_key  { q = q.bind(v); }
    if let Some(v) = patch.enabled   {
        q = if matches!(kind, DbKind::Postgres) { q.bind(v) } else { q.bind(if v { 1i64 } else { 0i64 }) };
    }
    if let Some(v) = patch.priority  { q = q.bind(v); }
    q.bind(id).execute(pool).await.map(|_| ())
}

pub async fn delete_channel(pool: &Pool, kind: DbKind, id: i64) -> Result<(), sqlx::Error> {
    let sql = db::q(kind, "DELETE FROM upstream_channels WHERE id = ?");
    sqlx::query(&sql).bind(id).execute(pool).await.map(|_| ())
}

// ---------------------------------------------------------------------------
// channel_models
// ---------------------------------------------------------------------------

pub async fn list_channel_models(
    pool: &Pool,
    kind: DbKind,
    channel_id: i64,
) -> Result<Vec<ChannelModel>, sqlx::Error> {
    let sql = db::q(
        kind,
        "SELECT channel_id, model, upstream_id FROM channel_models WHERE channel_id = ? ORDER BY model ASC",
    );
    sqlx::query_as::<_, ChannelModel>(&sql)
        .bind(channel_id)
        .fetch_all(pool)
        .await
}

#[derive(Debug, Deserialize)]
pub struct ChannelModelsInput {
    pub models: Vec<ChannelModelEntry>,
}
#[derive(Debug, Deserialize)]
pub struct ChannelModelEntry {
    pub model: String,
    #[serde(default)]
    pub upstream_id: Option<String>,
}

/// Replace the model set for `channel_id` atomically.
pub async fn set_channel_models(
    pool: &Pool,
    kind: DbKind,
    channel_id: i64,
    entries: &[ChannelModelEntry],
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    let del = db::q(kind, "DELETE FROM channel_models WHERE channel_id = ?");
    sqlx::query(&del).bind(channel_id).execute(&mut *tx).await?;
    let ins = db::q(
        kind,
        "INSERT INTO channel_models (channel_id, model, upstream_id) VALUES (?, ?, ?)",
    );
    for e in entries {
        sqlx::query(&ins)
            .bind(channel_id)
            .bind(&e.model)
            .bind(e.upstream_id.as_deref())
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await
}

// ---------------------------------------------------------------------------
// model_pricing
// ---------------------------------------------------------------------------

/// One `model_pricing` row. sqlx decodes tuples up to 16 elements, which the
/// 15 selected columns fit exactly.
type PriceRow = (
    i64, String, String, Option<String>, i64, String, Option<i64>,
    i64, i64, Option<i64>, i64, i64, i64, Option<String>, Option<String>,
);

fn parse_price_row(
    (id, model, kind_, display_name, enabled, protocol, context_limit,
     input_price, output_price, cached_input_price, per_call_price,
     base_price, per_second_price, allowed_seconds, size_rules): PriceRow,
) -> ModelPrice {
    ModelPrice {
        id,
        model,
        kind: kind_,
        display_name,
        enabled: enabled != 0,
        protocol,
        context_limit,
        input_price,
        output_price,
        // 0 means the admin left the field blank; treat it as "no cache
        // discount" so cached tokens fall back to the input rate instead of
        // becoming free.
        cached_input_price: cached_input_price.filter(|v| *v > 0),
        per_call_price,
        base_price,
        per_second_price,
        allowed_seconds: allowed_seconds.map(|s| serde_json::from_str(&s).unwrap_or_default()),
        size_rules: size_rules.map(|s| serde_json::from_str(&s).unwrap_or_default()),
    }
}

const PRICE_COLS: &str = "id, model, kind, display_name, enabled, protocol, context_limit, \
     input_price, output_price, cached_input_price, per_call_price, \
     base_price, per_second_price, allowed_seconds, size_rules";

fn price_select(kind: DbKind, tail: &str) -> String {
    db::q(kind, &format!("SELECT {PRICE_COLS} FROM model_pricing {tail}"))
}

pub async fn list_pricing(pool: &Pool, kind: DbKind) -> Result<Vec<ModelPrice>, sqlx::Error> {
    let sql = price_select(kind, "ORDER BY kind, model");
    let rows: Vec<PriceRow> = sqlx::query_as(&sql).fetch_all(pool).await?;
    Ok(rows.into_iter().map(parse_price_row).collect())
}

pub async fn get_price(
    pool: &Pool,
    kind: DbKind,
    model: &str,
) -> Result<Option<ModelPrice>, sqlx::Error> {
    let sql = price_select(kind, "WHERE model = ?");
    let row: Option<PriceRow> = sqlx::query_as(&sql).bind(model).fetch_optional(pool).await?;
    Ok(row.map(parse_price_row))
}

#[derive(Debug, Deserialize)]
pub struct PricingInput {
    pub model: String,
    pub kind: String,
    /// When present, replace this model's channel bindings with these IDs.
    /// Omitted by lightweight edits (for example enable/disable) to preserve bindings.
    #[serde(default)]
    pub channel_ids: Option<Vec<i64>>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_protocol")]
    pub protocol: String,
    #[serde(default)]
    pub context_limit: Option<i64>,
    // chat billing: micro-USD per 1M tokens (the provider's published rate).
    #[serde(default)]
    pub input_price: i64,
    #[serde(default)]
    pub output_price: i64,
    #[serde(default)]
    pub cached_input_price: Option<i64>,
    /// image billing: micro-USD per call.
    #[serde(default)]
    pub per_call_price: i64,
    // video billing fields; ignored for chat/image
    #[serde(default)]
    pub base_price: i64,
    #[serde(default)]
    pub per_second_price: i64,
    #[serde(default)]
    pub allowed_seconds: Option<Vec<i64>>,
    #[serde(default)]
    pub size_rules: Option<Vec<SizeRule>>,
}
fn default_protocol() -> String { "openai".to_string() }

/// Video rows must carry a complete, well-formed rule set; chat/image rows
/// must not carry one (their video columns are stored as NULL).
pub fn validate_pricing_input(input: &PricingInput) -> Result<(), String> {
    if input.model.trim().is_empty() {
        return Err("model 不能为空".into());
    }
    for (label, value) in [
        ("输入价格", input.input_price),
        ("输出价格", input.output_price),
        ("每次调用价格", input.per_call_price),
    ] {
        if value < 0 {
            return Err(format!("{label}必须 >= 0"));
        }
    }
    if input.cached_input_price.is_some_and(|v| v < 0) {
        return Err("缓存输入价格必须 >= 0".into());
    }
    if input.kind != "video" {
        return Ok(());
    }
    if input.base_price < 0 {
        return Err("基础价格必须 >= 0".into());
    }
    if input.per_second_price < 0 {
        return Err("每秒价格必须 >= 0".into());
    }
    let seconds = input.allowed_seconds.as_deref().unwrap_or(&[]);
    if seconds.is_empty() {
        return Err("allowed_seconds 不能为空".into());
    }
    if seconds.iter().any(|s| *s <= 0) {
        return Err("allowed_seconds 中的时长必须大于 0".into());
    }
    let rules = input.size_rules.as_deref().unwrap_or(&[]);
    if rules.is_empty() {
        return Err("size_rules 不能为空".into());
    }
    for r in rules {
        let ok = r
            .size
            .split_once('x')
            .is_some_and(|(w, h)| w.parse::<u32>().is_ok() && h.parse::<u32>().is_ok());
        if !ok {
            return Err(format!("size 格式不合法: {}", r.size));
        }
        if r.multiplier <= 0 {
            return Err(format!("size {} 的 multiplier 必须大于 0", r.size));
        }
    }
    Ok(())
}

pub async fn upsert_price(
    pool: &Pool,
    kind: DbKind,
    input: &PricingInput,
) -> Result<(), sqlx::Error> {
    const COLS: &str = "model, kind, display_name, enabled, protocol, context_limit, \
         input_price, output_price, cached_input_price, per_call_price, \
         base_price, per_second_price, allowed_seconds, size_rules";
    const PLACEHOLDERS: &str = "?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?";
    /// Every column except the natural key `model`, which identifies the row.
    const UPDATED: &[&str] = &[
        "kind", "display_name", "enabled", "protocol", "context_limit",
        "input_price", "output_price", "cached_input_price", "per_call_price",
        "base_price", "per_second_price", "allowed_seconds", "size_rules",
    ];

    let now = db::now_expr(kind);
    // `excluded` is the incoming row on both backends, so one statement serves
    // both (identifiers are case-insensitive, and SQLite accepts the space
    // before the conflict target).
    let assignments = UPDATED
        .iter()
        .map(|c| format!("{c} = excluded.{c}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = db::q(
        kind,
        &format!(
            "INSERT INTO model_pricing ({COLS}, updated_at) VALUES ({PLACEHOLDERS}, {now}) \
             ON CONFLICT (model) DO UPDATE SET {assignments}, updated_at = {now}"
        ),
    );
    let enabled_v: i64 = if input.enabled { 1 } else { 0 };
    let ctx: Option<i64> = input.context_limit.filter(|n| *n > 0);
    let is_video = input.kind == "video";
    let is_chat = input.kind == "chat";
    let is_image = input.kind == "image";
    let json_or_none = |v: Option<&Vec<SizeRule>>| -> Option<String> {
        v.map(|v| serde_json::to_string(v).unwrap_or_else(|_| "[]".into()))
    };
    let allowed_seconds: Option<String> = if is_video {
        input
            .allowed_seconds
            .as_ref()
            .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "[]".into()))
    } else {
        None
    };
    let size_rules: Option<String> = if is_video {
        json_or_none(input.size_rules.as_ref())
    } else {
        None
    };

    let q = sqlx::query(&sql)
        .bind(&input.model)
        .bind(&input.kind)
        .bind(input.display_name.as_deref())
        .bind(enabled_v);
    // Zero out the columns that do not apply to this kind so a model converted
    // from, say, video to chat cannot keep charging its old per-second rate.
    let q = q
        .bind(&input.protocol)
        .bind(ctx)
        .bind(if is_chat { input.input_price } else { 0 })
        .bind(if is_chat { input.output_price } else { 0 })
        .bind(if is_chat { input.cached_input_price.filter(|v| *v > 0) } else { None })
        .bind(if is_image { input.per_call_price } else { 0 })
        .bind(if is_video { input.base_price } else { 0 })
        .bind(if is_video { input.per_second_price } else { 0 })
        .bind(allowed_seconds)
        .bind(size_rules);

    let mut tx = pool.begin().await?;
    q.execute(&mut *tx).await?;
    if let Some(channel_ids) = &input.channel_ids {
        let del = db::q(kind, "DELETE FROM channel_models WHERE model = ?");
        sqlx::query(&del).bind(&input.model).execute(&mut *tx).await?;
        let ins = db::q(
            kind,
            "INSERT INTO channel_models (channel_id, model, upstream_id) VALUES (?, ?, NULL)",
        );
        let unique_ids: std::collections::BTreeSet<i64> = channel_ids.iter().copied().collect();
        for channel_id in unique_ids {
            sqlx::query(&ins)
                .bind(channel_id)
                .bind(&input.model)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await
}

async fn channel_ids_match_protocol(
    pool: &Pool,
    kind: DbKind,
    channel_ids: &[i64],
    protocol: &str,
) -> Result<bool, sqlx::Error> {
    let sql = db::q(
        kind,
        "SELECT COUNT(*) FROM upstream_channels WHERE id = ? AND protocol = ?",
    );
    for channel_id in channel_ids.iter().copied().collect::<std::collections::BTreeSet<_>>() {
        let (count,): (i64,) = sqlx::query_as(&sql)
            .bind(channel_id)
            .bind(protocol)
            .fetch_one(pool)
            .await?;
        if count == 0 {
            return Ok(false);
        }
    }
    Ok(true)
}

pub async fn delete_price(pool: &Pool, kind: DbKind, model: &str) -> Result<(), sqlx::Error> {
    let models = [model.to_string()];
    delete_prices(pool, kind, &models).await.map(|_| ())
}

/// Delete several pricing rules in one transaction, dropping their channel
/// bindings too: a `channel_models` row whose model no longer exists in
/// `model_pricing` is dead weight that would silently restrict routing if the
/// same model id were created again later.
pub async fn delete_prices(
    pool: &Pool,
    kind: DbKind,
    models: &[String],
) -> Result<u64, sqlx::Error> {
    if models.is_empty() {
        return Ok(0);
    }
    let del_bindings = db::q(kind, "DELETE FROM channel_models WHERE model = ?");
    let del_price = db::q(kind, "DELETE FROM model_pricing WHERE model = ?");
    let mut tx = pool.begin().await?;
    let mut deleted = 0u64;
    for model in models {
        sqlx::query(&del_bindings)
            .bind(model)
            .execute(&mut *tx)
            .await?;
        deleted += sqlx::query(&del_price)
            .bind(model)
            .execute(&mut *tx)
            .await?
            .rows_affected();
    }
    tx.commit().await?;
    Ok(deleted)
}

/// Convenience: top-priority enabled channel matching a protocol,
/// regardless of any specific model binding. Used by GET /v1/models to
/// surface an upstream catalog when in shared mode.
#[allow(dead_code)] // consumed by main.rs proxy_get_forward
pub async fn any_enabled_channel(
    pool: &Pool,
    kind: DbKind,
    protocol: &str,
) -> Result<Option<Channel>, sqlx::Error> {
    let sql = db::q(
        kind,
        "SELECT id, name, protocol, base_url, api_key, enabled, priority \
         FROM upstream_channels \
         WHERE protocol = ? AND enabled = 1 \
         ORDER BY priority ASC, id ASC LIMIT 1",
    );
    let row: Option<(i64, String, String, String, String, i64, i64)> =
        sqlx::query_as(&sql)
            .bind(protocol)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(id, name, protocol, base_url, api_key, enabled, priority)| Channel {
        id,
        name,
        protocol,
        base_url,
        api_key,
        enabled: enabled != 0,
        priority,
    }))
}

// ---------------------------------------------------------------------------
// admin routes
// ---------------------------------------------------------------------------

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

async fn admin_list_channels(Extension(s): Extension<InstalledState>) -> Response {
    match list_channels(&s.pool, s.kind).await {
        // Redacted: the list is a management view, and nothing in the UI needs
        // the secret back. Returning it meant one compromised admin session,
        // or one logged response, disclosed every upstream key at once — keys
        // this deployment cannot rotate on its own. Editing still works
        // because `admin_patch_channel` treats an omitted key as "unchanged".
        Ok(v) => Json(v.iter().map(RedactedChannel::from).collect::<Vec<_>>()).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn admin_create_channel(
    Extension(s): Extension<InstalledState>,
    Json(input): Json<ChannelInput>,
) -> Response {
    if let Err(e) = validate_protocol(&input.protocol) {
        return err(StatusCode::BAD_REQUEST, e);
    }
    match create_channel(&s.pool, s.kind, &input).await {
        Ok(id) => Json(serde_json::json!({ "id": id })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn admin_patch_channel(
    Extension(s): Extension<InstalledState>,
    Path(id): Path<i64>,
    Json(patch): Json<ChannelPatch>,
) -> Response {
    if let Some(p) = &patch.protocol {
        if let Err(e) = validate_protocol(p) {
            return err(StatusCode::BAD_REQUEST, e);
        }
    }
    match update_channel(&s.pool, s.kind, id, &patch).await {
        // The edit may point the channel at a different upstream or key, so its
        // cached catalog no longer describes it.
        Ok(_) => {
            invalidate_catalog(id);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn admin_delete_channel(
    Extension(s): Extension<InstalledState>,
    Path(id): Path<i64>,
) -> Response {
    match delete_channel(&s.pool, s.kind, id).await {
        Ok(_) => {
            invalidate_catalog(id);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn admin_get_channel_models(
    Extension(s): Extension<InstalledState>,
    Path(id): Path<i64>,
) -> Response {
    match list_channel_models(&s.pool, s.kind, id).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn admin_put_channel_models(
    Extension(s): Extension<InstalledState>,
    Path(id): Path<i64>,
    Json(input): Json<ChannelModelsInput>,
) -> Response {
    match set_channel_models(&s.pool, s.kind, id, &input.models).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// Aggregate the models advertised by every enabled channel. Functional type
/// is deliberately absent: the admin assigns chat/image/video when saving the
/// model pricing rule.
#[derive(Serialize)]
struct AllChannelModel {
    model: String,
    channels: Vec<AllChannelModelChannel>,
}

#[derive(Serialize)]
struct AllChannelModelChannel {
    id: i64,
    name: String,
    protocol: String,
}

// ---------------------------------------------------------------------------
// upstream availability
// ---------------------------------------------------------------------------
//
// A priced model is only usable when some enabled channel actually serves it.
// Listing a model whose upstream disappeared just produces a 404/400 from the
// provider, so the catalogs advertised by the channels decide what the user
// sees: a model without any upstream is reported as unavailable and is left
// out of every user-facing listing.
//
// Catalogs are cached per channel because the pricing list and the model
// pickers ask for them on every page load, and a cold probe costs one HTTP
// round trip per channel.

/// How long a successful catalog probe is reused.
const CATALOG_TTL: std::time::Duration = std::time::Duration::from_secs(300);
/// Failed probes are retried sooner so a recovered upstream comes back fast.
const CATALOG_ERROR_TTL: std::time::Duration = std::time::Duration::from_secs(60);

struct CachedCatalog {
    /// Connection fingerprint; an edited channel invalidates its own entry.
    signature: u64,
    fetched_at: std::time::Instant,
    models: Result<Vec<String>, String>,
}

fn catalog_cache() -> &'static std::sync::Mutex<std::collections::HashMap<i64, CachedCatalog>> {
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<i64, CachedCatalog>>,
    > = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Hash of everything that decides what a channel advertises. The API key is
/// hashed rather than stored so the cache never holds a second copy of it.
fn channel_signature(ch: &Channel) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    ch.protocol.hash(&mut h);
    ch.base_url.hash(&mut h);
    ch.api_key.hash(&mut h);
    h.finish()
}

/// Drop a channel's cached catalog after an edit or removal.
fn invalidate_catalog(channel_id: i64) {
    if let Ok(mut cache) = catalog_cache().lock() {
        cache.remove(&channel_id);
    }
}

/// Probe a channel's catalog, reusing a recent result when possible.
async fn cached_channel_catalog(
    http: &reqwest::Client,
    ch: &Channel,
    refresh: bool,
) -> Result<Vec<String>, String> {
    let signature = channel_signature(ch);
    if !refresh {
        if let Ok(cache) = catalog_cache().lock() {
            if let Some(hit) = cache.get(&ch.id) {
                let ttl = if hit.models.is_ok() {
                    CATALOG_TTL
                } else {
                    CATALOG_ERROR_TTL
                };
                if hit.signature == signature && hit.fetched_at.elapsed() < ttl {
                    return hit.models.clone();
                }
            }
        }
    }
    let models = probe_channel_models(http, ch).await;
    if let Ok(mut cache) = catalog_cache().lock() {
        cache.insert(
            ch.id,
            CachedCatalog {
                signature,
                fetched_at: std::time::Instant::now(),
                models: models.clone(),
            },
        );
    }
    models
}

/// One enabled channel plus the catalog it advertises.
pub struct ChannelCatalog {
    pub channel: Channel,
    /// `Err` means the catalog is unknown (timeout, 4xx, unparsable body).
    pub models: Result<Vec<String>, String>,
}

/// Which channels serve a model, and under which upstream alias.
type Bindings = std::collections::HashMap<String, Vec<(i64, Option<String>)>>;

async fn load_bindings(pool: &Pool, kind: DbKind) -> Result<Bindings, sqlx::Error> {
    let sql = db::q(kind, "SELECT model, channel_id, upstream_id FROM channel_models");
    let rows: Vec<(String, i64, Option<String>)> = sqlx::query_as(&sql).fetch_all(pool).await?;
    let mut out: Bindings = std::collections::HashMap::new();
    for (model, channel_id, upstream_id) in rows {
        out.entry(model).or_default().push((channel_id, upstream_id));
    }
    Ok(out)
}

/// Snapshot used to answer "does this model still have an upstream?" for a
/// whole list of models without re-probing per model.
pub struct AvailabilityIndex {
    catalogs: Vec<ChannelCatalog>,
    bindings: Bindings,
    /// Channels whose catalog could not be read, for admin-facing display.
    pub errors: Vec<(String, String)>,
}

impl AvailabilityIndex {
    /// Build an index from the enabled channels, probing their catalogs
    /// concurrently. `refresh` bypasses the cache (admin "refresh" action).
    pub async fn load(
        http: &reqwest::Client,
        pool: &Pool,
        kind: DbKind,
        refresh: bool,
    ) -> Result<Self, sqlx::Error> {
        let channels: Vec<Channel> = list_channels(pool, kind)
            .await?
            .into_iter()
            .filter(|c| c.enabled)
            .collect();
        let probes = channels.into_iter().map(|channel| async move {
            let models = cached_channel_catalog(http, &channel, refresh).await;
            ChannelCatalog { channel, models }
        });
        let catalogs: Vec<ChannelCatalog> = futures_util::future::join_all(probes).await;
        let errors = catalogs
            .iter()
            .filter_map(|c| {
                c.models
                    .as_ref()
                    .err()
                    .map(|e| (c.channel.name.clone(), e.clone()))
            })
            .collect();
        let bindings = load_bindings(pool, kind).await?;
        Ok(Self {
            catalogs,
            bindings,
            errors,
        })
    }

    /// Channels of `protocol` that can currently serve `model`.
    pub fn serving_channels(&self, model: &str, protocol: &str) -> Vec<&Channel> {
        serving_channels(
            model,
            protocol,
            self.bindings.get(model).map(|v| v.as_slice()),
            &self.catalogs,
        )
    }

    /// True when at least one enabled channel serves the model. A channel whose
    /// catalog could not be read counts as "might serve it": a provider that
    /// hides or rate-limits `/models` must not disable a working model.
    pub fn is_available(&self, model: &str, protocol: &str) -> bool {
        !self.serving_channels(model, protocol).is_empty()
    }
}

/// Pure core of the availability rule, kept separate so it can be unit tested.
///
/// `bindings` is `Some` only when the model has explicit `channel_models` rows;
/// those rows restrict routing, so an unbound channel is never a candidate even
/// if it advertises the model.
fn serving_channels<'a>(
    model: &str,
    protocol: &str,
    bindings: Option<&[(i64, Option<String>)]>,
    catalogs: &'a [ChannelCatalog],
) -> Vec<&'a Channel> {
    let mut out: Vec<&Channel> = Vec::new();
    for catalog in catalogs {
        if !catalog.channel.enabled || catalog.channel.protocol != protocol {
            continue;
        }
        let upstream_model = match bindings {
            Some(bound) => match bound.iter().find(|(id, _)| *id == catalog.channel.id) {
                Some((_, alias)) => alias.clone().unwrap_or_else(|| model.to_string()),
                None => continue,
            },
            None => model.to_string(),
        };
        let serves = match &catalog.models {
            Ok(list) => list.iter().any(|m| m == &upstream_model),
            // Unknown catalog: keep the channel as a candidate (fail open).
            Err(_) => true,
        };
        if serves {
            out.push(&catalog.channel);
        }
    }
    out
}

/// Probe a single channel's upstream `/models` endpoint and return the list of
/// model IDs it advertises. Returns Err(string) for caller-facing display.
async fn probe_channel_models(
    http: &reqwest::Client,
    ch: &Channel,
) -> Result<Vec<String>, String> {
    let base = ch.base_url.trim_end_matches('/');
    let (url, protocol) = match ch.protocol.as_str() {
        "openai" => (format!("{base}/v1/models"), "openai"),
        "claude" => (format!("{base}/v1/models"), "claude"),
        "gemini" => (format!("{base}/v1beta/models?pageSize=200"), "gemini"),
        other => return Err(format!("unknown protocol {other}")),
    };
    let mut req = http
        .get(&url)
        .header(axum::http::header::ACCEPT, "application/json")
        .timeout(std::time::Duration::from_secs(15));
    req = match protocol {
        "openai" => req.bearer_auth(&ch.api_key),
        "claude" => req
            .header("x-api-key", ch.api_key.as_str())
            .header("anthropic-version", "2023-06-01"),
        "gemini" => req.header("x-goog-api-key", ch.api_key.as_str()),
        _ => req,
    };
    let resp = req.send().await.map_err(|e| format!("connect: {e}"))?;
    let status = resp.status();
    let bytes = resp.bytes().await.map_err(|e| format!("read: {e}"))?;
    if !status.is_success() {
        let body = String::from_utf8_lossy(&bytes);
        let snippet: String = body.chars().take(120).collect();
        return Err(format!("upstream {status}: {snippet}"));
    }
    let v: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|e| format!("parse: {e}"))?;
    // OpenAI: {"data":[{"id":"gpt-4o", ...}, ...]}
    // Claude: {"data":[{"id":"claude-3-5-sonnet-...", ...}, ...]}
    // Gemini: {"models":[{"name":"models/gemini-1.5-pro", ...}, ...]}
    let mut out: Vec<String> = Vec::new();
    if protocol == "gemini" {
        if let Some(arr) = v.get("models").and_then(|m| m.as_array()) {
            for item in arr {
                if let Some(name) = item.get("name").and_then(|n| n.as_str()) {
                    // strip "models/" prefix if present
                    let id = name.strip_prefix("models/").unwrap_or(name).to_string();
                    out.push(id);
                }
            }
        }
    } else if let Some(arr) = v.get("data").and_then(|d| d.as_array()) {
        for item in arr {
            if let Some(id) = item.get("id").and_then(|s| s.as_str()) {
                out.push(id.to_string());
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_admin_channel_list_never_carries_the_secret() {
        // This endpoint is the one place an upstream key used to leave the
        // server. Serialize the real payload and assert the secret is absent
        // from the bytes, so a future field addition cannot reintroduce it.
        let mut c = channel(1, "openai");
        c.api_key = "sk-proj-supersecretvalue-0123456789".to_string();
        let body = serde_json::to_string(&RedactedChannel::from(&c)).unwrap();

        assert!(
            !body.contains("sk-proj-supersecretvalue-0123456789"),
            "the plaintext key must not be serialized: {body}"
        );
        assert!(!body.contains("\"api_key\""), "no api_key field at all: {body}");
        assert!(body.contains("sk-p…6789"), "a usable hint is still shown: {body}");
        assert!(body.contains("\"has_api_key\":true"));
    }

    #[test]
    fn a_short_secret_is_masked_completely() {
        // Revealing 8 of 10 characters would be worse than revealing nothing.
        assert_eq!(mask_secret("sk-123"), "………");
        assert_eq!(mask_secret("123456789012"), "………");
        assert_eq!(mask_secret("1234567890123"), "1234…0123");
    }

    #[test]
    fn an_unset_secret_is_reported_as_unconfigured() {
        // The UI has to tell "hidden" apart from "never set", or a broken
        // channel looks identical to a working one. An empty hint is correct
        // here: there is no secret to hint at.
        let mut c = channel(1, "openai");
        c.api_key = "   ".to_string();
        let r = RedactedChannel::from(&c);
        assert!(!r.has_api_key);
        assert_eq!(r.api_key_hint, "");
    }

    fn channel(id: i64, protocol: &str) -> Channel {
        Channel {
            id,
            name: format!("channel-{id}"),
            protocol: protocol.to_string(),
            base_url: format!("https://channel-{id}.example"),
            api_key: "test-key".to_string(),
            enabled: true,
            priority: id,
        }
    }

    fn choice(id: i64, model: &str) -> ChannelChoice {
        ChannelChoice {
            channel: channel(id, "openai"),
            upstream_model: model.to_string(),
        }
    }

    fn catalog(id: i64, protocol: &str, models: Result<&[&str], &str>) -> ChannelCatalog {
        ChannelCatalog {
            channel: channel(id, protocol),
            models: models
                .map(|list| list.iter().map(|m| m.to_string()).collect())
                .map_err(|e| e.to_string()),
        }
    }

    #[test]
    fn advertised_model_filter_skips_higher_priority_channel_without_model() {
        let selected = narrow_to_advertised(vec![
            (choice(1, "video-b"), Ok(vec!["video-a".to_string()])),
            (choice(2, "video-b"), Ok(vec!["video-b".to_string()])),
        ]);

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].channel.id, 2);
    }

    #[test]
    fn advertised_model_filter_prefers_channels_that_list_the_model() {
        let selected = narrow_to_advertised(vec![
            (choice(1, "video-b"), Err("timeout".to_string())),
            (choice(2, "video-b"), Ok(vec!["video-b".to_string()])),
        ]);

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].channel.id, 2);
    }

    /// A hidden or rate-limited `/models` endpoint must not take a working
    /// model offline, so an unreadable catalog still routes.
    #[test]
    fn advertised_model_filter_falls_back_to_unreadable_catalogs() {
        let selected = narrow_to_advertised(vec![
            (choice(1, "video-b"), Err("timeout".to_string())),
            (choice(2, "video-b"), Ok(vec!["video-a".to_string()])),
        ]);

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].channel.id, 1);
    }

    #[test]
    fn model_without_any_advertising_channel_has_no_upstream() {
        let catalogs = vec![catalog(1, "openai", Ok(&["gpt-5"]))];

        assert!(serving_channels("gpt-5", "openai", None, &catalogs).len() == 1);
        assert!(serving_channels("gpt-4o", "openai", None, &catalogs).is_empty());
    }

    /// The channel's protocol must match the one the model declares, otherwise
    /// a same-named model on a different protocol would look available.
    #[test]
    fn availability_requires_a_matching_protocol() {
        let catalogs = vec![catalog(1, "openai", Ok(&["claude-sonnet-4"]))];

        assert!(serving_channels("claude-sonnet-4", "claude", None, &catalogs).is_empty());
        assert_eq!(
            serving_channels("claude-sonnet-4", "openai", None, &catalogs).len(),
            1
        );
    }

    /// Explicit bindings restrict routing, so an unbound channel can't make a
    /// model available even when it advertises it.
    #[test]
    fn availability_honours_explicit_bindings_and_aliases() {
        let catalogs = vec![
            catalog(1, "openai", Ok(&["gpt-5"])),
            catalog(2, "openai", Ok(&["vendor-gpt-5"])),
        ];

        let bound_to_alias = [(2i64, Some("vendor-gpt-5".to_string()))];
        let serving = serving_channels("gpt-5", "openai", Some(&bound_to_alias), &catalogs);
        assert_eq!(serving.len(), 1);
        assert_eq!(serving[0].id, 2);

        let bound_without_alias = [(2i64, None)];
        assert!(
            serving_channels("gpt-5", "openai", Some(&bound_without_alias), &catalogs).is_empty()
        );
    }

    /// Deleting a priced model must also drop its `channel_models` rows:
    /// a leftover binding restricts routing for any future model with the
    /// same id, which looks like an upstream outage that nothing explains.
    #[tokio::test]
    async fn bulk_delete_removes_the_rules_and_their_channel_bindings() {
        use sqlx::any::AnyPoolOptions;
        crate::db::install_drivers();
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::migrate(&pool, DbKind::Sqlite).await.unwrap();
        sqlx::query("DELETE FROM model_pricing").execute(&pool).await.unwrap();

        sqlx::query(
            "INSERT INTO upstream_channels (name, protocol, base_url, api_key) \
             VALUES ('c1', 'openai', 'https://c1.example', 'k')",
        )
        .execute(&pool)
        .await
        .unwrap();
        for model in ["dead-a", "dead-b", "alive"] {
            let input = PricingInput {
                model: model.into(),
                kind: "chat".into(),
                channel_ids: Some(vec![1]),
                display_name: None,
                enabled: true,
                protocol: "openai".into(),
                context_limit: None,
                input_price: 0,
                output_price: 0,
                cached_input_price: None,
                per_call_price: 0,
                base_price: 0,
                per_second_price: 0,
                allowed_seconds: None,
                size_rules: None,
            };
            upsert_price(&pool, DbKind::Sqlite, &input).await.unwrap();
        }

        let deleted = delete_prices(
            &pool,
            DbKind::Sqlite,
            &["dead-a".to_string(), "dead-b".to_string(), "never-existed".to_string()],
        )
        .await
        .unwrap();

        assert_eq!(deleted, 2, "only the two existing rows count as deleted");
        let remaining: Vec<String> = list_pricing(&pool, DbKind::Sqlite)
            .await
            .unwrap()
            .into_iter()
            .map(|p| p.model)
            .collect();
        assert_eq!(remaining, vec!["alive".to_string()]);
        let (bindings,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM channel_models WHERE model LIKE 'dead-%'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(bindings, 0, "bindings of a deleted model must go too");
    }

    /// Nothing to delete must stay a no-op rather than an empty `IN ()`.
    #[tokio::test]
    async fn bulk_delete_of_an_empty_list_touches_nothing() {
        use sqlx::any::AnyPoolOptions;
        crate::db::install_drivers();
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::migrate(&pool, DbKind::Sqlite).await.unwrap();
        let before = list_pricing(&pool, DbKind::Sqlite).await.unwrap().len();

        assert_eq!(delete_prices(&pool, DbKind::Sqlite, &[]).await.unwrap(), 0);
        assert_eq!(list_pricing(&pool, DbKind::Sqlite).await.unwrap().len(), before);
    }

    #[test]
    fn unreadable_catalog_keeps_the_model_available() {
        let catalogs = vec![catalog(1, "openai", Err("upstream 403"))];

        assert_eq!(
            serving_channels("gpt-5", "openai", None, &catalogs).len(),
            1
        );
    }
}

async fn admin_list_all_channel_models(
    axum::extract::State(state): axum::extract::State<AppState>,
    Extension(s): Extension<InstalledState>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Response {
    // `?refresh=1` re-probes every upstream; the default reuses the cached
    // catalogs so opening the pricing dialog doesn't hit every provider.
    let refresh = matches!(q.get("refresh").map(String::as_str), Some("1" | "true"));
    let index = match AvailabilityIndex::load(&state.http, &s.pool, s.kind, refresh).await {
        Ok(v) => v,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    let mut map: std::collections::BTreeMap<String, Vec<AllChannelModelChannel>> =
        std::collections::BTreeMap::new();
    for catalog in &index.catalogs {
        let Ok(models) = &catalog.models else { continue };
        for model in models {
            map.entry(model.clone())
                .or_default()
                .push(AllChannelModelChannel {
                    id: catalog.channel.id,
                    name: catalog.channel.name.clone(),
                    protocol: catalog.channel.protocol.clone(),
                });
        }
    }
    let errors: Vec<serde_json::Value> = index
        .errors
        .iter()
        .map(|(channel, error)| serde_json::json!({ "channel": channel, "error": error }))
        .collect();
    let models: Vec<AllChannelModel> = map
        .into_iter()
        .map(|(model, channels)| AllChannelModel { model, channels })
        .collect();
    Json(serde_json::json!({
        "models": models,
        "errors": errors,
    }))
    .into_response()
}

/// A priced model plus whether an upstream still serves it. The admin list
/// shows unavailable models as disabled, because calling one only produces an
/// upstream error.
#[derive(Serialize)]
struct AdminModelPrice {
    #[serde(flatten)]
    price: ModelPrice,
    /// False when no enabled channel of the model's protocol serves it.
    upstream_available: bool,
    /// Names of the channels that currently serve it, for the admin UI.
    upstream_channels: Vec<String>,
}

async fn admin_list_pricing(
    axum::extract::State(state): axum::extract::State<AppState>,
    Extension(s): Extension<InstalledState>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Response {
    let refresh = matches!(q.get("refresh").map(String::as_str), Some("1" | "true"));
    let pricing = match list_pricing(&s.pool, s.kind).await {
        Ok(v) => v,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    // A probe failure must not break the pricing table: fall back to reporting
    // every model as available so the admin can still edit prices.
    let index = AvailabilityIndex::load(&state.http, &s.pool, s.kind, refresh)
        .await
        .ok();
    let errors: Vec<serde_json::Value> = index
        .as_ref()
        .map(|i| {
            i.errors
                .iter()
                .map(|(channel, error)| serde_json::json!({ "channel": channel, "error": error }))
                .collect()
        })
        .unwrap_or_default();
    let models: Vec<AdminModelPrice> = pricing
        .into_iter()
        .map(|price| {
            let serving = index
                .as_ref()
                .map(|i| i.serving_channels(&price.model, &price.protocol))
                .unwrap_or_default();
            AdminModelPrice {
                upstream_available: index.is_none() || !serving.is_empty(),
                upstream_channels: serving.into_iter().map(|c| c.name.clone()).collect(),
                price,
            }
        })
        .collect();
    Json(serde_json::json!({ "models": models, "errors": errors })).into_response()
}

async fn admin_upsert_pricing(
    Extension(s): Extension<InstalledState>,
    Json(input): Json<PricingInput>,
) -> Response {
    if let Err(e) = validate_protocol_kind(&input.protocol, &input.kind) {
        return err(StatusCode::BAD_REQUEST, e);
    }
    if let Err(e) = validate_pricing_input(&input) {
        return err(StatusCode::BAD_REQUEST, e);
    }
    if let Some(channel_ids) = &input.channel_ids {
        match channel_ids_match_protocol(&s.pool, s.kind, channel_ids, &input.protocol).await {
            Ok(true) => {}
            Ok(false) => {
                return err(StatusCode::BAD_REQUEST, "所选渠道不存在或协议与模型不一致");
            }
            Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        }
    }
    match upsert_price(&s.pool, s.kind, &input).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// Bulk-delete every priced model that no enabled channel serves any more.
///
/// The same rule the list endpoint uses decides what "no upstream" means, so
/// the admin deletes exactly the rows shown as 无上游. A channel whose catalog
/// could not be read keeps its models alive (fail open), and a probe failure
/// that prevents building the index aborts the whole operation rather than
/// deleting a working whitelist.
async fn admin_prune_unavailable_pricing(
    axum::extract::State(state): axum::extract::State<AppState>,
    Extension(s): Extension<InstalledState>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Response {
    let refresh = matches!(q.get("refresh").map(String::as_str), Some("1" | "true"));
    let pricing = match list_pricing(&s.pool, s.kind).await {
        Ok(v) => v,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let index = match AvailabilityIndex::load(&state.http, &s.pool, s.kind, refresh).await {
        Ok(v) => v,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let models: Vec<String> = pricing
        .into_iter()
        .filter(|p| !index.is_available(&p.model, &p.protocol))
        .map(|p| p.model)
        .collect();
    let deleted = match delete_prices(&s.pool, s.kind, &models).await {
        Ok(n) => n,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let errors: Vec<serde_json::Value> = index
        .errors
        .iter()
        .map(|(channel, error)| serde_json::json!({ "channel": channel, "error": error }))
        .collect();
    Json(serde_json::json!({
        "deleted": deleted,
        "models": models,
        "errors": errors,
    }))
    .into_response()
}

async fn admin_delete_pricing(
    Extension(s): Extension<InstalledState>,
    Path(model): Path<String>,
) -> Response {
    match delete_price(&s.pool, s.kind, &model).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// Protocol validation for callers outside this module (pricing import).
pub fn validate_protocol_public(protocol: &str) -> Result<(), String> {
    validate_protocol(protocol)
}

fn validate_protocol(protocol: &str) -> Result<(), String> {
    if !matches!(protocol, "openai" | "claude" | "gemini") {
        return Err("protocol must be openai/claude/gemini".into());
    }
    Ok(())
}

fn validate_protocol_kind(protocol: &str, kind: &str) -> Result<(), String> {
    validate_protocol(protocol)?;
    if !matches!(kind, "chat" | "image" | "video") {
        return Err("kind must be chat/image/video".into());
    }
    if kind == "video" && protocol != "openai" {
        return Err("视频渠道仅支持 openai 协议".into());
    }
    Ok(())
}

pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route("/admin/channels", get(admin_list_channels).post(admin_create_channel))
        .route("/admin/channels/{id}", patch(admin_patch_channel).delete(admin_delete_channel))
        .route(
            "/admin/channels/{id}/models",
            get(admin_get_channel_models).put(admin_put_channel_models),
        )
        .route("/admin/channels/all-models", get(admin_list_all_channel_models))
        .route("/admin/pricing", get(admin_list_pricing).post(admin_upsert_pricing))
        .route(
            "/admin/pricing/sync-newapi",
            axum::routing::post(crate::newapi_sync::admin_sync_pricing),
        )
        .route(
            "/admin/pricing/prune-unavailable",
            axum::routing::post(admin_prune_unavailable_pricing),
        )
        .route("/admin/pricing/{model}", delete(admin_delete_pricing))
        .route_layer(middleware::from_fn(admin::require_admin))
}

// ---------------------------------------------------------------------------
// user-facing routes (no admin gate; auth via parent router)
// ---------------------------------------------------------------------------

/// Public model listing for the "use platform credits" mode in settings.
/// Returns every model in `model_pricing` (enabled=1) that has at least one
/// enabled `upstream_channels` row bound via `channel_models`. Each entry
/// carries the protocol of its top-priority channel so the client can group
/// chat models by OpenAI / Claude / Gemini in the picker.
#[derive(Serialize)]
struct PlatformModel {
    model: String,
    display_name: Option<String>,
    kind: String, // "chat" | "image" | "video"
    protocol: String, // top-priority channel's protocol
    context_limit: Option<i64>,
    /// Provider key this model would carry inside an agent runtime's generated
    /// `models.json`, or `None` when work mode cannot run it. Reported rather
    /// than derived client-side so the picker and the runtime can never
    /// disagree about what is selectable.
    agent_provider: Option<String>,
    // Rates already converted to site quota so the picker can show what a
    // model costs without knowing about USD or the markup. All in
    // micro-quota (1 quota = 1 CNY = 1e6 micro-quota).
    input_micro_quota_per_1m: i64,
    output_micro_quota_per_1m: i64,
    cached_input_micro_quota_per_1m: Option<i64>,
    per_call_micro_quota: i64,
}

async fn user_list_platform_models(
    axum::extract::State(state): axum::extract::State<AppState>,
    Extension(s): Extension<InstalledState>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Response {
    let flavor_filter = q.get("flavor").cloned();
    let pricing = match list_pricing(&s.pool, s.kind).await {
        Ok(v) => v,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    // Availability decides what the picker may offer. A probe failure leaves
    // `None`, which keeps every model listed rather than emptying the picker.
    let index = AvailabilityIndex::load(&state.http, &s.pool, s.kind, false)
        .await
        .ok();
    let rate = crate::quota::QuotaRate::load(&s.pool, s.kind).await;
    let mut out: Vec<PlatformModel> = Vec::new();
    for p in pricing {
        if !p.enabled { continue; }
        // video has its own listing (/videos/models) with full billing rules.
        if p.kind == "video" { continue; }
        if let Some(ref f) = flavor_filter {
            if &p.kind != f { continue; }
        }
        // Auto-routing: a priced model declares its protocol in model_pricing.
        // Offer it only while an enabled channel of that protocol still serves
        // it, so the picker never lists a model the client can't route.
        if let Some(index) = &index {
            if !index.is_available(&p.model, &p.protocol) {
                continue;
            }
        }
        out.push(PlatformModel {
            model: p.model.clone(),
            display_name: p.display_name.clone(),
            kind: p.kind.clone(),
            protocol: p.protocol.clone(),
            context_limit: p.context_limit,
            agent_provider: if p.kind == "chat" {
                crate::agent_token::runtime_provider_for(&p.protocol)
                    .map(str::to_string)
            } else {
                None
            },
            input_micro_quota_per_1m: rate.micro_quota_for_micro_usd(p.input_price),
            output_micro_quota_per_1m: rate.micro_quota_for_micro_usd(p.output_price),
            cached_input_micro_quota_per_1m: p
                .cached_input_price
                .map(|v| rate.micro_quota_for_micro_usd(v)),
            per_call_micro_quota: rate.micro_quota_for_micro_usd(p.per_call_price),
        });
    }
    Json(out).into_response()
}

pub fn user_routes() -> Router<AppState> {
    Router::new().route("/channels/models", get(user_list_platform_models))
}

// ---------------------------------------------------------------------------
// routing — call-site helpers (Task 2.2)
// ---------------------------------------------------------------------------
//
// Two-tier resolution:
//   1. BYOK — client supplies X-Upstream-Url/Key headers and didn't set
//      X-Use-Shared=1: no credits deducted, no routing.
//   2. Channels — admin-configured upstream_channels matching the model,
//      sorted by priority ASC. Caller iterates the chain on transient errors.

fn header_str<'a>(h: &'a HeaderMap, name: &str) -> Option<&'a str> {
    h.get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}


/// BYOK route: a single concrete upstream (base_url + key) provided by the
/// client. No credits, no fallback.
#[derive(Debug, Clone)]
pub struct ByokRoute {
    pub base_url: String,
    pub api_key: String,
}

/// Resolved route for a request.
#[derive(Debug)]
pub enum Route {
    /// Client supplied X-Upstream-Url/Key and didn't ask for shared.
    Byok(ByokRoute),
    /// Server-side channels matching the model and request protocol. Empty Vec means
    /// "no upstream available" — caller should return 400.
    Channels {
        model: String,
        chain: Vec<ChannelChoice>,
    },
}

/// Extract `model` from a chat request body / header.
/// OpenAI + Claude: JSON body `{"model": "..."}`. Gemini: URL path, but the
/// client sets `X-Upstream-Model` header for shared mode — we accept either.
pub fn extract_chat_model(body: &[u8], headers: &HeaderMap) -> Option<String> {
    if let Some(m) = header_str(headers, "x-upstream-model") {
        return Some(m.to_string());
    }
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    v.get("model")
        .and_then(|m| m.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

/// Round-robin cursor keyed by `(protocol, model, priority-tier)`. Used to
/// rotate the starting channel among equal-priority channels so load spreads
/// evenly across them request-to-request. In-memory only (resets on restart) —
/// exact fairness across a process restart isn't required.
fn rr_next(key: &str) -> u64 {
    static COUNTERS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, u64>>> =
        std::sync::OnceLock::new();
    let m = COUNTERS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
    let c = g.entry(key.to_string()).or_insert(0);
    let v = *c;
    *c = c.wrapping_add(1);
    v
}

/// Reorder a priority-sorted chain so that, within each priority tier of 2+
/// channels, the starting channel rotates round-robin. Tiers stay in ascending
/// priority order, so failover still walks lower-priority tiers afterwards.
fn balance_chain(chain: Vec<ChannelChoice>, key_prefix: &str) -> Vec<ChannelChoice> {
    let mut out: Vec<ChannelChoice> = Vec::with_capacity(chain.len());
    let mut i = 0;
    while i < chain.len() {
        let prio = chain[i].channel.priority;
        let mut j = i;
        while j < chain.len() && chain[j].channel.priority == prio {
            j += 1;
        }
        let group = &chain[i..j];
        if group.len() > 1 {
            let start = (rr_next(&format!("{key_prefix}:{prio}")) as usize) % group.len();
            for k in 0..group.len() {
                out.push(group[(start + k) % group.len()].clone());
            }
        } else {
            out.push(group[0].clone());
        }
        i = j;
    }
    out
}

/// Resolve a request to either a BYOK route or a channel chain.
///
/// `flavor` is "chat" or "image". `protocol` is the wire protocol the client is
/// speaking ("openai"/"claude"/"gemini"); the channel chain is filtered to
/// channels of that exact protocol so a request is never sent to a channel that
/// speaks a different protocol. `model` is the user-requested model name.
/// For BYOK requests `model` may be empty — it's only used to look up channels.
///
/// The chain is finally narrowed to the channels whose (cached) catalog still
/// advertises the model, so a model whose upstream disappeared fails with a
/// clear "no upstream" message instead of an opaque provider error. This is the
/// same rule the user-facing model listings apply, so anything offered in the
/// picker is routable.
pub async fn resolve_route(
    http: &reqwest::Client,
    pool: &Pool,
    kind: DbKind,
    headers: &HeaderMap,
    flavor: &str,
    protocol: &str,
    model: &str,
) -> Result<Route, Response> {
    // BYOK only when both URL and key are present. `X-Use-Shared` header is
    // no longer consulted — admin-configured channels are the default path.
    let hdr_url = header_str(headers, "x-upstream-url");
    let hdr_key = header_str(headers, "x-upstream-key");
    if let (Some(u), Some(k)) = (hdr_url, hdr_key) {
        if !u.is_empty() && !k.is_empty() {
            return Ok(Route::Byok(ByokRoute {
                base_url: u.to_string(),
                api_key: k.to_string(),
            }));
        }
    }
    if model.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "请求体缺少 model 字段（或 X-Upstream-Model 头）",
        )
            .into_response());
    }
    let chain = select_chain(pool, kind, model).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("channel lookup failed: {e}"),
        )
            .into_response()
    })?;
    let chain: Vec<ChannelChoice> = chain
        .into_iter()
        .filter(|c| c.channel.protocol == protocol)
        .collect();
    if chain.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("模型 {model} 未配置任何可用的 {protocol} 渠道"),
        )
            .into_response());
    }
    let chain = narrow_to_advertised(probe_chain_catalogs(http, chain).await);
    if chain.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("模型 {model} 当前没有上游渠道提供，已暂停使用；请联系管理员"),
        )
            .into_response());
    }
    let chain = balance_chain(chain, &format!("{flavor}:{protocol}:{model}"));
    Ok(Route::Channels {
        model: model.to_string(),
        chain,
    })
}

// ---------------------------------------------------------------------------
// pricing-aware quota operations
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum DeductError {
    /// Model missing from `model_pricing` or disabled there.
    NotWhitelisted,
    /// Priced fine, but the account cannot cover it.
    Insufficient { balance: i64, cost: i64 },
}

/// Resolve `model` in `model_pricing`, requiring it to be enabled and to match
/// `flavor` ("chat" / "image" / "video"). This is the whitelist gate every
/// platform-billed request passes through.
pub async fn enabled_price(
    pool: &Pool,
    kind: DbKind,
    model: &str,
    flavor: &str,
) -> Option<ModelPrice> {
    let price = get_price(pool, kind, model).await.ok().flatten()?;
    if !price.enabled || price.kind != flavor {
        return None;
    }
    Some(price)
}

/// Admission check for token-metered chat: confirm the model is open to
/// platform billing, then confirm the account is not already overdrawn. The
/// actual charge lands in [`settle_chat`] once the upstream reports usage.
pub async fn authorize_chat(
    pool: &Pool,
    kind: DbKind,
    user_id: i64,
    model: &str,
) -> Result<ModelPrice, DeductError> {
    let price = enabled_price(pool, kind, model, "chat")
        .await
        .ok_or(DeductError::NotWhitelisted)?;
    // A free model stays usable even at a zero balance.
    if price.input_price <= 0 && price.output_price <= 0 {
        return Ok(price);
    }
    match crate::quota::has_spendable_balance(pool, kind, user_id).await {
        Ok(_) => Ok(price),
        Err(balance) => Err(DeductError::Insufficient { balance, cost: 0 }),
    }
}

/// Charge a completed chat call from the token counts the upstream reported.
///
/// Prices are re-read here rather than captured at admission time so that the
/// charge reflects the configuration in force when the call finished. Returns
/// the quota charged (0 when the upstream reported no usage, which means the
/// call produced nothing billable).
pub async fn settle_chat(
    pool: &Pool,
    kind: DbKind,
    user_id: i64,
    model: &str,
    protocol: &str,
    usage: crate::quota::TokenUsage,
    reason: &str,
) -> i64 {
    if usage.is_empty() {
        return 0;
    }
    let Some(price) = enabled_price(pool, kind, model, "chat").await else {
        return 0;
    };
    let rate = crate::quota::QuotaRate::load(pool, kind).await;
    let cost = crate::quota::chat_quota_cost(
        rate,
        price.input_price,
        price.output_price,
        price.cached_input_price,
        usage,
    );
    let meta = crate::quota::LedgerMeta::chat_usage(protocol, model, usage);
    crate::quota::settle(pool, kind, user_id, cost, reason, &meta).await;
    cost
}

/// Per-call deduction for image generation, which reports no tokens.
///
/// Returns `(new_balance, cost_deducted)` on success — callers MUST use the
/// returned `cost` for any refund rather than re-reading the price, which could
/// change between deduction and refund.
pub async fn try_deduct_per_call(
    pool: &Pool,
    kind: DbKind,
    user_id: i64,
    model: &str,
    flavor: &str,
    protocol: &str,
    reason: &str,
) -> Result<(i64, i64), DeductError> {
    let price = enabled_price(pool, kind, model, flavor)
        .await
        .ok_or(DeductError::NotWhitelisted)?;
    let rate = crate::quota::QuotaRate::load(pool, kind).await;
    let cost = rate.micro_quota_for_micro_usd(price.per_call_price.max(0));
    let meta = crate::quota::LedgerMeta::image(protocol, model);
    match crate::quota::try_deduct(pool, kind, user_id, cost, reason, &meta).await {
        Ok(bal) => Ok((bal, cost)),
        Err(bal) => Err(DeductError::Insufficient { balance: bal, cost }),
    }
}

/// Micro-quota cost of one per-call generation without deducting — used by
/// refund paths that already deducted via [`try_deduct_per_call`].
pub async fn per_call_quota(
    pool: &Pool,
    kind: DbKind,
    model: &str,
    flavor: &str,
) -> Option<i64> {
    let price = enabled_price(pool, kind, model, flavor).await?;
    let rate = crate::quota::QuotaRate::load(pool, kind).await;
    Some(rate.micro_quota_for_micro_usd(price.per_call_price.max(0)))
}
