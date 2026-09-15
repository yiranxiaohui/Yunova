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

/// Resolve an agent token to its owning user, refusing revoked tokens.
///
/// `last_used_at` is advanced on every successful lookup so a user can tell
/// which tokens are live before revoking one. The update is best-effort: a
/// failed bookkeeping write must not fail an otherwise valid chat request.
pub async fn user_for_token(pool: &Pool, kind: DbKind, token: &str) -> Option<i64> {
    if !token.starts_with(TOKEN_PREFIX) {
        return None;
    }
    let hash = auth::token_hash(token);
    let revoked_col = db::bool_as_int(kind, "revoked");
    let sql = db::q(
        kind,
        &format!("SELECT id, user_id, {revoked_col} FROM agent_tokens WHERE token_hash = ?"),
    );
    let row: Option<(i64, i64, i64)> = sqlx::query_as(&sql)
        .bind(&hash)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    let (id, user_id, revoked) = row?;
    if revoked != 0 {
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

// ---------------------------------------------------------------------------
// management endpoints
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CreateReq {
    #[serde(default)]
    name: Option<String>,
}

async fn list_tokens(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
) -> Response {
    let revoked_col = db::bool_as_int(installed.kind, "revoked");
    let sql = db::q(
        installed.kind,
        &format!(
            "SELECT id, name, prefix, created_at, last_used_at, {revoked_col} \
             FROM agent_tokens WHERE user_id = ? ORDER BY id DESC"
        ),
    );
    let rows: Vec<(i64, String, String, String, Option<String>, i64)> = sqlx::query_as(&sql)
        .bind(user.id)
        .fetch_all(&installed.pool)
        .await
        .unwrap_or_default();
    let out: Vec<_> = rows
        .into_iter()
        .map(|(id, name, prefix, created_at, last_used_at, revoked)| {
            json!({
                "id": id,
                "name": name,
                "prefix": prefix,
                "created_at": created_at,
                "last_used_at": last_used_at,
                "revoked": revoked != 0,
            })
        })
        .collect();
    Json(out).into_response()
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
            match installed.kind {
                DbKind::Postgres => "FALSE",
                _ => "0",
            }
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

    let token = new_token();
    let hash = auth::token_hash(&token);
    let prefix: String = token.chars().take(DISPLAY_PREFIX_LEN).collect();
    let sql = db::q(
        installed.kind,
        "INSERT INTO agent_tokens (user_id, name, token_hash, prefix) VALUES (?, ?, ?, ?)",
    );
    if let Err(e) = sqlx::query(&sql)
        .bind(user.id)
        .bind(&name)
        .bind(&hash)
        .bind(&prefix)
        .execute(&installed.pool)
        .await
    {
        eprintln!("[agent-token] insert failed: {e}");
        return (StatusCode::INTERNAL_SERVER_ERROR, "创建令牌失败").into_response();
    }

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
            match installed.kind {
                DbKind::Postgres => "TRUE",
                _ => "1",
            }
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

/// Pure builder for the runtime `models.json`, kept separate from the handler
/// so it is directly testable.
pub fn runtime_models_json(
    prices: &[crate::channels::ModelPrice],
    base_url: &str,
    token: &str,
) -> serde_json::Value {
    let mut providers = serde_json::Map::new();

    for (protocol, api, path) in [
        ("openai", "openai-responses", "/api/proxy/openai"),
        ("claude", "anthropic-messages", "/api/proxy/claude"),
    ] {
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
            format!("yunova-{protocol}"),
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
}
