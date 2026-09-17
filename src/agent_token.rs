//! Agent access tokens.
//!
//! An agent runtime that runs outside the browser — `pi` embedded in the
//! desktop client, or `pi` inside a cloud sandbox container — needs to reach
//! the platform chat gateway without a session cookie. Two rejected
//! alternatives explain the shape of this module:
//!
//! * Shipping an upstream channel key to the runtime would let it talk to the
//!   provider directly, bypassing the `model_pricing` whitelist, channel
//!   failover and token metering. The admin's key would also sit on a user's
//!   own machine.
//! * Reusing the `nc_session` cookie would tie a long-lived headless process
//!   to the browser session lifetime and to cookie semantics no CLI honors.
//!
//! So an agent token is a bearer credential scoped to one user, presented as
//! `Authorization: Bearer <token>` (or `x-api-key`, for Anthropic-shaped
//! clients). It resolves to the same [`CurrentUser`] the cookie path produces,
//! which means the existing authorize/meter/settle chain applies unchanged.
//!
//! Only the SHA-256 hash is stored, so a leaked database cannot be replayed
//! against the gateway, and the plaintext is shown exactly once at creation.
//!
//! Session-scoped tokens additionally carry an expiry. A runtime credential is
//! only meaningful while its session is live, and the device target hands the
//! plaintext to the user's own machine, so "valid until someone remembers to
//! revoke it" is the wrong default there. Tokens minted from the token manager
//! keep `expires_at = NULL`, because that list *is* their control surface.

use axum::{
    Extension, Json, Router,
    extract::Path,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use serde::Deserialize;
use serde_json::json;

use crate::{
    AppState, CurrentUser, InstalledState,
    auth,
    db::{self, DbKind, Pool},
};

/// Distinguishes agent tokens from other secrets in logs and support tickets,
/// and lets the gateway reject a session cookie value pasted as a bearer token.
const TOKEN_PREFIX: &str = "yna_";

/// Characters of the plaintext kept in `prefix` for display. Long enough to
/// tell two tokens apart, short enough to be useless on its own.
const DISPLAY_PREFIX_LEN: usize = TOKEN_PREFIX.len() + 8;

const MAX_TOKENS_PER_USER: i64 = 20;

/// Lifetime of a token minted for one agent session.
///
/// Deliberately generous relative to the sandbox lifetime ceiling: this is a
/// backstop for the paths that cannot revoke deterministically (a device that
/// never reports its runtime closing, a server that dies mid-session), not the
/// primary control. Normal teardown revokes immediately.
pub const SESSION_TOKEN_TTL_HOURS: i64 = 12;

fn new_token() -> String {
    format!("{TOKEN_PREFIX}{}", auth::generate_token())
}

/// Extract an agent token from the request headers.
///
/// `Authorization: Bearer` covers OpenAI-shaped clients; `x-api-key` covers
/// Anthropic-shaped ones. Both are checked because pi picks the header from
/// the provider's `api` type, not from our gateway.
pub fn token_from_headers(headers: &HeaderMap) -> Option<String> {
    let bearer = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")))
        .map(str::trim);
    let api_key = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim);
    // Also accept Gemini-shaped clients, which send the key in its own header.
    let goog = headers
        .get("x-goog-api-key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim);

    [bearer, api_key, goog]
        .into_iter()
        .flatten()
        .find(|t| t.starts_with(TOKEN_PREFIX))
        .map(|t| t.to_string())
}

/// Resolve an agent token to its owning user, refusing revoked or expired
/// tokens.
///
/// `last_used_at` is advanced on every successful lookup so a user can tell
/// which tokens are live before revoking one. The update is best-effort: a
/// failed bookkeeping write must not fail an otherwise valid chat request.
pub async fn user_for_token(pool: &Pool, kind: DbKind, token: &str) -> Option<i64> {
    if !token.starts_with(TOKEN_PREFIX) {
        return None;
    }
    let hash = auth::token_hash(token);
    let sql = db::q(
        kind,
        "SELECT id, user_id, revoked, expires_at FROM agent_tokens WHERE token_hash = ?",
    );
    let row: Option<(i64, i64, i64, Option<String>)> = sqlx::query_as(&sql)
        .bind(&hash)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    let (id, user_id, revoked, expires_at) = row?;
    if revoked != 0 {
        return None;
    }
    // A NULL expiry means the token is managed by hand in the token list. An
    // unparseable one is treated as expired: a credential whose lifetime we
    // cannot establish must not keep spending quota.
    if let Some(raw) = expires_at.as_deref()
        && is_expired(raw, chrono::Utc::now())
    {
        return None;
    }

    let touch = db::q(kind, "UPDATE agent_tokens SET last_used_at = ? WHERE id = ?");
    let _ = sqlx::query(&touch)
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(id)
        .execute(pool)
        .await;

    Some(user_id)
}

/// Whether an RFC3339 expiry has passed. An unparseable value counts as
/// expired so a corrupt row fails closed rather than granting forever.
fn is_expired(raw: &str, now: chrono::DateTime<chrono::Utc>) -> bool {
    match chrono::DateTime::parse_from_rfc3339(raw) {
        Ok(at) => at.with_timezone(&chrono::Utc) <= now,
        Err(_) => true,
    }
}

// ---------------------------------------------------------------------------
// management endpoints
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CreateReq {
    #[serde(default)]
    name: Option<String>,
}

/// One row of the token list query.
type TokenRow = (
    i64,            // id
    String,         // name
    String,         // prefix
    String,         // created_at
    Option<String>, // last_used_at
    i64,            // revoked
    Option<String>, // expires_at
);

async fn list_tokens(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
) -> Response {
    let sql = db::q(
        installed.kind,
        "SELECT id, name, prefix, created_at, last_used_at, revoked, expires_at \
             FROM agent_tokens WHERE user_id = ? ORDER BY id DESC",
    );
    let rows: Vec<TokenRow> = sqlx::query_as(&sql)
        .bind(user.id)
        .fetch_all(&installed.pool)
        .await
        .unwrap_or_default();
    let now = chrono::Utc::now();
    let out: Vec<_> = rows
        .into_iter()
        .map(
            |(id, name, prefix, created_at, last_used_at, revoked, expires_at)| {
                json!({
                    "id": id,
                    "name": name,
                    "prefix": prefix,
                    "created_at": created_at,
                    "last_used_at": last_used_at,
                    "revoked": revoked != 0,
                    "expires_at": expires_at,
                    // Precomputed so the list does not have to reimplement the
                    // fail-closed parse rule the gateway applies.
                    "expired": expires_at.as_deref().is_some_and(|raw| is_expired(raw, now)),
                })
            },
        )
        .collect();
    Json(out).into_response()
}

/// Mint a token for `user_id` and persist only its hash.
///
/// Shared by the management endpoint and by the session launcher, which needs
/// to provision a gateway credential for a runtime it is about to start.
/// Returns the plaintext, which is the only time it exists outside the caller.
///
/// `ttl_hours` bounds how long the credential stays usable if it is never
/// revoked explicitly. Pass `None` only for tokens the user manages by hand.
pub async fn mint(
    pool: &Pool,
    kind: DbKind,
    user_id: i64,
    name: &str,
    ttl_hours: Option<i64>,
) -> Result<String, String> {
    let token = new_token();
    let hash = auth::token_hash(&token);
    let prefix: String = token.chars().take(DISPLAY_PREFIX_LEN).collect();
    let expires_at = ttl_hours
        .map(|h| (chrono::Utc::now() + chrono::Duration::hours(h)).to_rfc3339());
    let sql = db::q(
        kind,
        "INSERT INTO agent_tokens (user_id, name, token_hash, prefix, expires_at) \
         VALUES (?, ?, ?, ?, ?)",
    );
    sqlx::query(&sql)
        .bind(user_id)
        .bind(name)
        .bind(&hash)
        .bind(&prefix)
        .bind(expires_at.as_deref())
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;
    Ok(token)
}

/// Revoke a token by its plaintext.
///
/// Used to retire a session-scoped credential when its runtime exits, so a
/// token that leaked from a stopped sandbox is already dead.
pub async fn revoke_by_plaintext(pool: &Pool, kind: DbKind, token: &str) {
    let sql = db::q(
        kind,
        &format!(
            "UPDATE agent_tokens SET revoked = {} WHERE token_hash = ?",
            "1"
        ),
    );
    let _ = sqlx::query(&sql)
        .bind(auth::token_hash(token))
        .execute(pool)
        .await;
}

async fn create_token(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Json(req): Json<CreateReq>,
) -> Response {
    let name = req
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or("Agent")
        .chars()
        .take(60)
        .collect::<String>();

    // Cap live tokens per account. Without a bound, a compromised session
    // could mint credentials faster than the user notices them in the list.
    let live_sql = db::q(
        installed.kind,
        &format!(
            "SELECT COUNT(*) FROM agent_tokens WHERE user_id = ? AND revoked = {}",
            "0"
        ),
    );
    let live: i64 = sqlx::query_scalar(&live_sql)
        .bind(user.id)
        .fetch_one(&installed.pool)
        .await
        .unwrap_or(0);
    if live >= MAX_TOKENS_PER_USER {
        return (
            StatusCode::BAD_REQUEST,
            format!("最多只能同时保留 {MAX_TOKENS_PER_USER} 个 Agent 令牌，请先撤销不用的"),
        )
            .into_response();
    }

    // A token created here is managed by hand in the token list, so it gets no
    // expiry: that list and its revoke button are the intended control. Session
    // credentials are minted by the launcher with a TTL instead.
    let token = match mint(&installed.pool, installed.kind, user.id, &name, None).await {
        Ok(t) => t,
        Err(e) => {
            eprintln!("[agent-token] create endpoint insert failed: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "创建令牌失败").into_response();
        }
    };
    let prefix: String = token.chars().take(DISPLAY_PREFIX_LEN).collect();

    // The only time the plaintext is ever returned.
    Json(json!({ "token": token, "name": name, "prefix": prefix })).into_response()
}

/// Revoke rather than delete, so a token that shows up in upstream logs later
/// can still be traced to the credential it came from.
async fn revoke_token(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Response {
    let sql = db::q(
        installed.kind,
        &format!(
            "UPDATE agent_tokens SET revoked = {} WHERE id = ? AND user_id = ?",
            "1"
        ),
    );
    match sqlx::query(&sql)
        .bind(id)
        .bind(user.id)
        .execute(&installed.pool)
        .await
    {
        Ok(r) if r.rows_affected() == 0 => (StatusCode::NOT_FOUND, "令牌不存在").into_response(),
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            eprintln!("[agent-token] revoke failed: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "撤销令牌失败").into_response()
        }
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/agent/tokens", get(list_tokens).post(create_token))
        .route("/agent/tokens/{id}", delete(revoke_token))
        .route("/agent/runtime-config", post(runtime_config))
}

// ---------------------------------------------------------------------------
// runtime config
// ---------------------------------------------------------------------------

/// Build the `models.json` an agent runtime needs to talk to this gateway.
///
/// The runtime must never learn an upstream provider key, so the generated
/// config points at our own `/api/proxy/*` endpoints and carries the caller's
/// agent token instead. Model lists come from `model_pricing`, which means a
/// runtime can only ever see models the admin whitelisted and priced.
async fn runtime_config(
    Extension(installed): Extension<InstalledState>,
    Extension(_user): Extension<CurrentUser>,
    Json(req): Json<RuntimeConfigReq>,
) -> Response {
    let prices = match crate::channels::list_pricing(&installed.pool, installed.kind).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[agent-token] pricing lookup failed: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "读取模型定价失败").into_response();
        }
    };
    let base = req.base_url.trim_end_matches('/').to_string();
    Json(runtime_models_json(&prices, &base, &req.token)).into_response()
}

#[derive(Deserialize)]
struct RuntimeConfigReq {
    /// Public origin the runtime should call, e.g. `https://yunnet.top`.
    base_url: String,
    /// The agent token the runtime will present. Passed in rather than minted
    /// here so the caller decides whether to reuse an existing credential.
    token: String,
}

/// Provider key in the generated `models.json` for a pricing protocol.
///
/// The runtime only ever sees Yunova's own gateway providers, so this mapping
/// is also the whitelist of protocols work mode can run on: a model priced
/// under any other protocol has no provider to be selected from, which is why
/// callers treat `None` as "not available to an agent" rather than guessing.
pub fn runtime_provider_for(protocol: &str) -> Option<&'static str> {
    RUNTIME_PROVIDERS
        .iter()
        .find(|(p, ..)| *p == protocol)
        .map(|(_, provider, ..)| *provider)
}

/// `(pricing protocol, generated provider key, pi api, gateway path)`.
const RUNTIME_PROVIDERS: [(&str, &str, &str, &str); 2] = [
    (
        "openai",
        "yunova-openai",
        "openai-responses",
        "/api/proxy/openai",
    ),
    (
        "claude",
        "yunova-claude",
        "anthropic-messages",
        "/api/proxy/claude",
    ),
];

/// Pure builder for the runtime `models.json`, kept separate from the handler
/// so it is directly testable.
pub fn runtime_models_json(
    prices: &[crate::channels::ModelPrice],
    base_url: &str,
    token: &str,
) -> serde_json::Value {
    let mut providers = serde_json::Map::new();

    for (protocol, provider, api, path) in RUNTIME_PROVIDERS {
        let models: Vec<serde_json::Value> = prices
            .iter()
            .filter(|p| p.enabled && p.kind == "chat" && p.protocol == protocol)
            .map(|p| {
                json!({
                    "id": p.model,
                    "name": p.display_name.clone().unwrap_or_else(|| p.model.clone()),
                    "reasoning": true,
                    "input": ["text", "image"],
                    "contextWindow": p.context_limit.unwrap_or(200_000),
                })
            })
            .collect();
        if models.is_empty() {
            continue;
        }
        providers.insert(
            provider.to_string(),
            json!({
                "name": format!("Yunova ({protocol})"),
                // The runtime's SDK appends the vendor's canonical path to
                // this base (`/responses` for OpenAI, `/v1/messages` for
                // Anthropic). `build_router` exposes those aliases, so the
                // request lands back in `proxy_forward` with billing intact.
                "baseUrl": format!("{base_url}{path}"),
                "api": api,
                "apiKey": token,
                "models": models,
            }),
        );
    }

    json!({ "providers": providers })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn price(model: &str, protocol: &str, kind: &str, enabled: bool) -> crate::channels::ModelPrice {
        crate::channels::ModelPrice {
            id: 1,
            model: model.to_string(),
            kind: kind.to_string(),
            display_name: None,
            enabled,
            protocol: protocol.to_string(),
            context_limit: None,
            input_price: 0,
            output_price: 0,
            cached_input_price: None,
            per_call_price: 0,
            base_price: 0,
            per_second_price: 0,
            allowed_seconds: None,
            size_rules: None,
        }
    }

    #[tokio::test]
    async fn an_expired_session_token_stops_authenticating() {
        // End-to-end over a real pool, because the expiry check spans the
        // migration, the INSERT and the lookup; a unit test of `is_expired`
        // alone would not catch a column that was never written.
        use crate::db::{DbKind, install_drivers};
        use sqlx::any::AnyPoolOptions;

        install_drivers();
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::migrate(&pool, DbKind::Sqlite).await.unwrap();
        sqlx::query("INSERT INTO users (id, username, password_hash) VALUES (1, 'u1', 'x')")
            .execute(&pool)
            .await
            .unwrap();

        let live = mint(&pool, DbKind::Sqlite, 1, "session-1", Some(12))
            .await
            .unwrap();
        assert_eq!(
            user_for_token(&pool, DbKind::Sqlite, &live).await,
            Some(1),
            "a freshly minted session token must work"
        );

        // Backdate it rather than sleeping.
        let past = (chrono::Utc::now() - chrono::Duration::minutes(1)).to_rfc3339();
        sqlx::query("UPDATE agent_tokens SET expires_at = ? WHERE token_hash = ?")
            .bind(&past)
            .bind(auth::token_hash(&live))
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            user_for_token(&pool, DbKind::Sqlite, &live).await,
            None,
            "an expired token must stop spending quota even if never revoked"
        );

        // A hand-managed token has no expiry and keeps working.
        let manual = mint(&pool, DbKind::Sqlite, 1, "laptop", None).await.unwrap();
        assert_eq!(user_for_token(&pool, DbKind::Sqlite, &manual).await, Some(1));

        pool.close().await;
    }

    #[tokio::test]
    async fn revoking_by_plaintext_kills_the_credential() {
        use crate::db::{DbKind, install_drivers};
        use sqlx::any::AnyPoolOptions;

        install_drivers();
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::migrate(&pool, DbKind::Sqlite).await.unwrap();
        sqlx::query("INSERT INTO users (id, username, password_hash) VALUES (1, 'u1', 'x')")
            .execute(&pool)
            .await
            .unwrap();

        let token = mint(&pool, DbKind::Sqlite, 1, "session-1", Some(12))
            .await
            .unwrap();
        assert_eq!(user_for_token(&pool, DbKind::Sqlite, &token).await, Some(1));

        revoke_by_plaintext(&pool, DbKind::Sqlite, &token).await;
        assert_eq!(user_for_token(&pool, DbKind::Sqlite, &token).await, None);
        // Teardown paths may both fire; the second must be harmless.
        revoke_by_plaintext(&pool, DbKind::Sqlite, &token).await;
        assert_eq!(user_for_token(&pool, DbKind::Sqlite, &token).await, None);

        pool.close().await;
    }

    #[test]
    fn bearer_header_is_recognized() {
        let mut h = HeaderMap::new();
        h.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer yna_abc"),
        );
        assert_eq!(token_from_headers(&h).as_deref(), Some("yna_abc"));
    }

    #[test]
    fn an_expiry_in_the_past_retires_the_token() {
        let now = chrono::Utc::now();
        let past = (now - chrono::Duration::seconds(1)).to_rfc3339();
        let future = (now + chrono::Duration::hours(1)).to_rfc3339();
        assert!(is_expired(&past, now));
        assert!(!is_expired(&future, now));
    }

    #[test]
    fn an_expiry_exactly_at_the_deadline_is_already_over() {
        // Boundary is inclusive: a credential is not usable during the instant
        // it expires.
        let now = chrono::Utc::now();
        assert!(is_expired(&now.to_rfc3339(), now));
    }

    #[test]
    fn an_unparseable_expiry_fails_closed() {
        // A corrupt row must not grant an unlimited credential. Failing open
        // here would turn a storage bug into a quota-spending one.
        let now = chrono::Utc::now();
        for raw in ["", "never", "2026-13-45", "1789"] {
            assert!(is_expired(raw, now), "{raw:?} must count as expired");
        }
    }

    #[test]
    fn anthropic_and_gemini_headers_are_recognized() {
        let mut h = HeaderMap::new();
        h.insert("x-api-key", HeaderValue::from_static("yna_xyz"));
        assert_eq!(token_from_headers(&h).as_deref(), Some("yna_xyz"));

        let mut h = HeaderMap::new();
        h.insert("x-goog-api-key", HeaderValue::from_static("yna_g"));
        assert_eq!(token_from_headers(&h).as_deref(), Some("yna_g"));
    }

    #[test]
    fn foreign_credentials_are_ignored() {
        // A user's own upstream key must not be mistaken for an agent token:
        // BYOK requests are meant to bypass platform billing entirely.
        let mut h = HeaderMap::new();
        h.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer sk-ant-123"),
        );
        assert_eq!(token_from_headers(&h), None);
    }

    #[test]
    fn minted_tokens_carry_the_namespace_prefix() {
        let t = new_token();
        assert!(t.starts_with(TOKEN_PREFIX));
        assert!(t.len() > DISPLAY_PREFIX_LEN);
    }

    #[test]
    fn runtime_config_exposes_only_enabled_chat_models() {
        let prices = vec![
            price("gpt-5", "openai", "chat", true),
            price("claude-opus-4-5", "claude", "chat", true),
            price("gpt-5-disabled", "openai", "chat", false),
            price("sora", "openai", "video", true),
        ];
        let cfg = runtime_models_json(&prices, "https://yunnet.top", "yna_t");
        let providers = cfg["providers"].as_object().unwrap();

        let openai = providers["yunova-openai"]["models"].as_array().unwrap();
        assert_eq!(openai.len(), 1, "disabled and non-chat models must be hidden");
        assert_eq!(openai[0]["id"], "gpt-5");

        let claude = providers["yunova-claude"]["models"].as_array().unwrap();
        assert_eq!(claude[0]["id"], "claude-opus-4-5");
    }

    #[test]
    fn runtime_config_points_at_the_gateway_not_the_upstream() {
        let prices = vec![price("gpt-5", "openai", "chat", true)];
        let cfg = runtime_models_json(&prices, "https://yunnet.top/", "yna_t");
        let p = &cfg["providers"]["yunova-openai"];
        // Trailing slash trimmed by the handler, so assert the un-trimmed form
        // the builder receives stays well-formed.
        assert_eq!(p["baseUrl"], "https://yunnet.top//api/proxy/openai");
        assert_eq!(p["api"], "openai-responses");
        assert_eq!(p["apiKey"], "yna_t");
    }

    #[test]
    fn runtime_config_omits_providers_without_models() {
        let prices = vec![price("gpt-5", "openai", "chat", true)];
        let cfg = runtime_models_json(&prices, "https://yunnet.top", "yna_t");
        let providers = cfg["providers"].as_object().unwrap();
        assert!(providers.contains_key("yunova-openai"));
        assert!(
            !providers.contains_key("yunova-claude"),
            "an empty provider would surface as a broken entry in pi's model list"
        );
    }

    #[test]
    fn provider_keys_match_the_generated_config() {
        // The picker sends `provider` straight to pi's `set_model`, so a
        // mismatch between this mapping and the generated config would make
        // every switch fail with "model not found".
        let prices = vec![
            price("gpt-5", "openai", "chat", true),
            price("claude-opus-4-5", "claude", "chat", true),
        ];
        let cfg = runtime_models_json(&prices, "https://yunnet.top", "yna_t");
        for protocol in ["openai", "claude"] {
            let key = runtime_provider_for(protocol).expect("a chat protocol must map");
            assert!(cfg["providers"].get(key).is_some(), "{key} must exist");
        }
        // Gemini is priced and callable in chat mode but has no agent
        // provider, so work mode must not offer it.
        assert_eq!(runtime_provider_for("gemini"), None);
    }
}
