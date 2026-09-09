mod admin;
mod auth;
mod channels;
mod conversations;
mod credits;
mod db;
mod email;
mod images;
mod invites;
mod net_guard;
mod payments;
mod profile;
mod prompts;
mod rate_limit;
mod runtime_env;
mod search;
mod settings;
mod setup;
mod sharing;
mod skills;
mod studio;
mod storage;
mod videos;
mod video_editor;
mod worker;
mod workflows;

use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode, Uri, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use chrono::Utc;
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(RustEmbed)]
#[folder = "web/dist/"]
struct Assets;

// ---------------------------------------------------------------------------
// state + config
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct AppState {
    pub installed: Arc<RwLock<Option<InstalledState>>>,
    pub http: reqwest::Client,
    /// Longer-timeout reqwest client for image generation upstreams.
    /// Image endpoints frequently take 1–5 minutes, and upstream relays add
    /// their own variability on top. Jobs run on a background tokio task so
    /// this doesn't tie up the user's HTTP connection.
    pub image_http: reqwest::Client,
    pub config_path: std::path::PathBuf,
    pub data_dir: std::path::PathBuf,
    /// Generated images/videos and avatars. The backend can be replaced at
    /// runtime by the admin storage settings API.
    pub storage: storage::MediaStorage,
    /// Serializes config-file updates from administrator settings pages.
    pub config_lock: Arc<tokio::sync::Mutex<()>>,
    /// Per-IP rate limiter for unauthenticated auth endpoints (login /
    /// register / send-code). See [`rate_limit`] for details.
    pub auth_limiter: std::sync::Arc<rate_limit::RateLimiter>,
    /// Per-IP limiter for anonymous BYOK proxy traffic. Authenticated users
    /// keep the existing unrestricted proxy behavior.
    pub guest_proxy_limiter: std::sync::Arc<rate_limit::RateLimiter>,
    /// Online worker registry: worker_id -> handle with WS send channel.
    pub workers: crate::worker::WorkerRegistry,
    /// Bounds CPU-heavy FFmpeg trim/merge work across all workflow runs.
    pub media_process_slots: std::sync::Arc<tokio::sync::Semaphore>,
    /// Human-in-the-loop approval map: call_id -> oneshot used by the agent
    /// loop to pause on shell/write_file until the user approves via REST.
    pub approvals: std::sync::Arc<tokio::sync::RwLock<std::collections::HashMap<String, tokio::sync::oneshot::Sender<bool>>>>,
}

#[derive(Clone)]
pub struct InstalledState {
    pub pool: db::Pool,
    pub kind: db::DbKind,
}

impl AppState {
    pub async fn require_installed(&self) -> Result<InstalledState, axum::response::Response> {
        use axum::http::StatusCode;
        use axum::response::IntoResponse;
        match self.installed.read().await.clone() {
            Some(s) => Ok(s),
            None => Err((StatusCode::SERVICE_UNAVAILABLE, "system not installed").into_response()),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CurrentUser {
    pub id: i64,
}

// ---------------------------------------------------------------------------
// auth endpoints
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct Credentials {
    username: String,
    password: String,
    #[serde(default)]
    invite_code: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    email_code: Option<String>,
}

#[derive(Serialize)]
pub struct UserDto {
    pub id: i64,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub is_admin: bool,
}

fn validate_credentials(c: &Credentials) -> Result<(), &'static str> {
    let u = c.username.trim();
    if u.len() < 3 || u.len() > 32 {
        return Err("username must be 3-32 characters");
    }
    if !u.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-') {
        return Err("username: letters, digits, _ or - only");
    }
    if c.password.len() < 6 || c.password.len() > 256 {
        return Err("password must be 6-256 characters");
    }
    Ok(())
}

/// Whether to set the `Secure` attribute on session cookies. Default ON —
/// the browser then only sends the cookie over HTTPS, blocking session-token
/// theft on plain-HTTP downgrades. Set `YUNOVA_INSECURE_COOKIE=1` only for
/// local dev when serving plain HTTP without a TLS-terminating proxy.
fn cookies_secure() -> bool {
    !matches!(
        crate::runtime_env::var("YUNOVA_INSECURE_COOKIE").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

fn session_cookie(token: String, max_age_days: i64) -> Cookie<'static> {
    let mut c = Cookie::new(auth::SESSION_COOKIE, token);
    c.set_http_only(true);
    c.set_secure(cookies_secure());
    c.set_same_site(SameSite::Lax);
    c.set_path("/");
    c.set_max_age(time::Duration::days(max_age_days));
    c
}

fn clear_cookie() -> Cookie<'static> {
    let mut c = Cookie::new(auth::SESSION_COOKIE, "");
    c.set_http_only(true);
    c.set_secure(cookies_secure());
    c.set_same_site(SameSite::Lax);
    c.set_path("/");
    c.set_max_age(time::Duration::ZERO);
    c
}

async fn register(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Json(creds): Json<Credentials>,
) -> Response {
    if !state.auth_limiter.allow(rate_limit::client_ip(&headers)).await {
        return (StatusCode::TOO_MANY_REQUESTS, "请求过于频繁，请稍后再试").into_response();
    }
    let installed = match state.require_installed().await {
        Ok(s) => s,
        Err(r) => return r,
    };
    let registration_enabled = credits::get_setting_bool(
        &installed.pool,
        installed.kind,
        "registration_enabled",
        true,
    )
    .await;
    if !registration_enabled {
        return (StatusCode::FORBIDDEN, "当前站点已关闭注册").into_response();
    }
    if let Err(msg) = validate_credentials(&creds) {
        return (StatusCode::BAD_REQUEST, msg).into_response();
    }

    // Email verification gate. When the admin has turned it on, registration
    // requires a previously-issued code that matches the supplied address.
    let require_email = credits::get_setting_bool(
        &installed.pool,
        installed.kind,
        "email_verification_required",
        false,
    )
    .await;
    let normalized_email: Option<String> = creds
        .email
        .as_deref()
        .map(email::normalize_email)
        .filter(|e| !e.is_empty());
    if require_email {
        let Some(e) = normalized_email.as_deref() else {
            return (StatusCode::BAD_REQUEST, "需要邮箱验证码").into_response();
        };
        if !email::valid_email(e) {
            return (StatusCode::BAD_REQUEST, "邮箱格式不正确").into_response();
        }
        let Some(code) = creds.email_code.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
            return (StatusCode::BAD_REQUEST, "需要邮箱验证码").into_response();
        };
        let ok = email::consume_code(&installed.pool, installed.kind, e, code, "register").await;
        if !ok {
            return (StatusCode::BAD_REQUEST, "验证码无效或已过期").into_response();
        }
    } else if let Some(e) = normalized_email.as_deref() {
        // Optional email: accept, but still validate format.
        if !email::valid_email(e) {
            return (StatusCode::BAD_REQUEST, "邮箱格式不正确").into_response();
        }
    }

    let phc = match auth::hash_password(&creds.password) {
        Ok(h) => h,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    };
    let username = creds.username.trim().to_string();

    let base_insert = db::q(
        installed.kind,
        "INSERT INTO users (username, password_hash, email) VALUES (?, ?, ?)",
    );
    let user_id = match installed.kind {
        db::DbKind::Sqlite | db::DbKind::Postgres => {
            let row: Result<(i64,), _> = sqlx::query_as(&format!("{base_insert} RETURNING id"))
                .bind(&username)
                .bind(&phc)
                .bind(&normalized_email)
                .fetch_one(&installed.pool)
                .await;
            match row {
                Ok((id,)) => id,
                Err(sqlx::Error::Database(d)) if d.is_unique_violation() => {
                    let msg = if d.message().contains("email") {
                        "该邮箱已注册"
                    } else {
                        "username already taken"
                    };
                    return (StatusCode::CONFLICT, msg).into_response();
                }
                Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
            }
        }
        db::DbKind::Mysql => {
            let mut tx = match installed.pool.begin().await {
                Ok(t) => t,
                Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
            };
            if let Err(e) = sqlx::query(&base_insert)
                .bind(&username)
                .bind(&phc)
                .bind(&normalized_email)
                .execute(&mut *tx)
                .await
            {
                if let sqlx::Error::Database(d) = &e {
                    if d.is_unique_violation() {
                        let msg = if d.message().contains("email") {
                            "该邮箱已注册"
                        } else {
                            "username already taken"
                        };
                        return (StatusCode::CONFLICT, msg).into_response();
                    }
                }
                return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
            }
            let row: Result<(i64,), _> = sqlx::query_as("SELECT LAST_INSERT_ID()")
                .fetch_one(&mut *tx)
                .await;
            let id = match row {
                Ok((v,)) => v,
                Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
            };
            if let Err(e) = tx.commit().await {
                return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
            }
            id
        }
    };

    let is_admin = admin::is_admin(&installed.pool, installed.kind, user_id).await;
    let _ = credits::ensure_account(&installed.pool, installed.kind, user_id).await;
    let _ = invites::ensure_code(&installed.pool, installed.kind, user_id).await;

    if let Some(raw) = creds.invite_code.as_deref() {
        if let Some(inviter_id) =
            invites::resolve_inviter(&installed.pool, installed.kind, raw, user_id).await
        {
            let claimed =
                invites::set_invited_by(&installed.pool, installed.kind, user_id, inviter_id)
                    .await
                    .unwrap_or(false);
            let inviter_grant = credits::get_setting_i64(
                &installed.pool,
                installed.kind,
                "invite_grant_inviter",
                100,
            )
            .await;
            let invitee_grant = credits::get_setting_i64(
                &installed.pool,
                installed.kind,
                "invite_grant_invitee",
                100,
            )
            .await;
            if claimed && inviter_grant > 0 {
                let _ = credits::grant(
                    &installed.pool,
                    installed.kind,
                    inviter_id,
                    inviter_grant,
                    &format!("invite_reward_inviter:{username}"),
                    &credits::LedgerMeta::grant(),
                )
                .await;
            }
            if claimed && invitee_grant > 0 {
                let _ = credits::grant(
                    &installed.pool,
                    installed.kind,
                    user_id,
                    invitee_grant,
                    "invite_reward_invitee",
                    &credits::LedgerMeta::grant(),
                )
                .await;
            }
        }
    }

    match auth::create_session(&installed.pool, installed.kind, user_id).await {
        Ok((token, _)) => {
            let jar = jar.add(session_cookie(token, auth::SESSION_TTL_DAYS));
            (
                jar,
                Json(UserDto {
                    id: user_id,
                    username,
                    display_name: None,
                    avatar_url: None,
                    is_admin,
                }),
            )
                .into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn login(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Json(creds): Json<Credentials>,
) -> Response {
    if !state.auth_limiter.allow(rate_limit::client_ip(&headers)).await {
        return (StatusCode::TOO_MANY_REQUESTS, "请求过于频繁，请稍后再试").into_response();
    }
    let installed = match state.require_installed().await {
        Ok(s) => s,
        Err(r) => return r,
    };
    let username = creds.username.trim();
    let admin_col = db::bool_as_int(installed.kind, "is_admin");
    let sel = db::q(
        installed.kind,
        &format!(
            "SELECT id, username, password_hash, display_name, avatar_url, {admin_col}
             FROM users WHERE {}",
            db::ci_eq(installed.kind, "username")
        ),
    );
    let row: Option<(i64, String, String, Option<String>, Option<String>, i64)> =
        sqlx::query_as(&sel)
            .bind(username)
            .fetch_optional(&installed.pool)
            .await
            .unwrap_or(None);

    let Some((id, username, phc, display_name, avatar_url, is_admin)) = row else {
        return (StatusCode::UNAUTHORIZED, "invalid credentials").into_response();
    };
    if !auth::verify_password(&creds.password, &phc) {
        return (StatusCode::UNAUTHORIZED, "invalid credentials").into_response();
    }

    match auth::create_session(&installed.pool, installed.kind, id).await {
        Ok((token, _)) => {
            let jar = jar.add(session_cookie(token, auth::SESSION_TTL_DAYS));
            (
                jar,
                Json(UserDto {
                    id,
                    username,
                    display_name,
                    avatar_url,
                    is_admin: is_admin != 0,
                }),
            )
                .into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Serialize)]
struct AuthConfig {
    email_verification_required: bool,
}

async fn auth_config(State(state): State<AppState>) -> Response {
    let installed = match state.require_installed().await {
        Ok(s) => s,
        Err(_) => {
            return Json(AuthConfig {
                email_verification_required: false,
            })
            .into_response();
        }
    };
    let email_verification_required = credits::get_setting_bool(
        &installed.pool,
        installed.kind,
        "email_verification_required",
        false,
    )
    .await;
    Json(AuthConfig {
        email_verification_required,
    })
    .into_response()
}

async fn logout(State(state): State<AppState>, jar: CookieJar) -> Response {
    if let Ok(installed) = state.require_installed().await {
        if let Some(c) = jar.get(auth::SESSION_COOKIE) {
            let _ = auth::delete_session(&installed.pool, installed.kind, c.value()).await;
        }
    }
    let jar = jar.add(clear_cookie());
    (jar, StatusCode::NO_CONTENT).into_response()
}

async fn me(State(state): State<AppState>, jar: CookieJar) -> Response {
    let installed = match state.require_installed().await {
        Ok(s) => s,
        Err(r) => return r,
    };
    let Some(c) = jar.get(auth::SESSION_COOKIE) else {
        return (StatusCode::UNAUTHORIZED, "not logged in").into_response();
    };
    let Some((id, _)) =
        auth::user_for_token(&installed.pool, installed.kind, c.value()).await
    else {
        return (StatusCode::UNAUTHORIZED, "session expired").into_response();
    };
    let admin_col = db::bool_as_int(installed.kind, "is_admin");
    let sql = db::q(
        installed.kind,
        &format!(
            "SELECT id, username, display_name, avatar_url, {admin_col}
             FROM users WHERE id = ?"
        ),
    );
    let row: Option<(i64, String, Option<String>, Option<String>, i64)> = sqlx::query_as(&sql)
        .bind(id)
        .fetch_optional(&installed.pool)
        .await
        .unwrap_or(None);
    match row {
        Some((id, username, display_name, avatar_url, is_admin)) => Json(UserDto {
            id,
            username,
            display_name,
            avatar_url,
            is_admin: is_admin != 0,
        })
        .into_response(),
        None => (StatusCode::UNAUTHORIZED, "user not found").into_response(),
    }
}

// ---------------------------------------------------------------------------
// auth middleware (for protected routes)
// ---------------------------------------------------------------------------

async fn require_auth(
    State(state): State<AppState>,
    jar: CookieJar,
    mut req: axum::extract::Request,
    next: Next,
) -> Response {
    let installed = match state.require_installed().await {
        Ok(s) => s,
        Err(r) => return r,
    };
    let Some(c) = jar.get(auth::SESSION_COOKIE) else {
        return (StatusCode::UNAUTHORIZED, "login required").into_response();
    };
    let Some((id, _)) = auth::user_for_token(&installed.pool, installed.kind, c.value()).await
    else {
        return (StatusCode::UNAUTHORIZED, "session expired").into_response();
    };
    req.extensions_mut().insert(CurrentUser { id });
    req.extensions_mut().insert(installed);
    next.run(req).await
}

/// Load an authenticated user when a valid session cookie is present, while
/// still allowing anonymous requests through. Used only by BYOK proxy routes:
/// anonymous callers may forward their own upstream credentials, but shared
/// platform channels remain account-only.
async fn optional_auth(
    State(state): State<AppState>,
    jar: CookieJar,
    mut req: axum::extract::Request,
    next: Next,
) -> Response {
    let installed = match state.require_installed().await {
        Ok(s) => s,
        Err(r) => return r,
    };
    let mut authenticated = false;
    if let Some(c) = jar.get(auth::SESSION_COOKIE)
        && let Some((id, _)) =
            auth::user_for_token(&installed.pool, installed.kind, c.value()).await
    {
        req.extensions_mut().insert(CurrentUser { id });
        authenticated = true;
    }
    if !authenticated
        && !state
            .guest_proxy_limiter
            .allow(rate_limit::client_ip(req.headers()))
            .await
    {
        return (StatusCode::TOO_MANY_REQUESTS, "请求过于频繁，请稍后再试").into_response();
    }
    req.extensions_mut().insert(installed);
    next.run(req).await
}

// ---------------------------------------------------------------------------
// chat proxy (optional, for upstreams without CORS)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub enum Protocol {
    OpenAi,
    Claude,
    Gemini,
}

impl Protocol {
    fn name(self) -> &'static str {
        match self {
            Protocol::OpenAi => "openai",
            Protocol::Claude => "claude",
            Protocol::Gemini => "gemini",
        }
    }
}

fn trim_slash(s: &str) -> &str {
    s.trim_end_matches('/')
}

pub fn chat_endpoint(host: &str, protocol: Protocol, model: &str) -> String {
    let base = trim_slash(host);
    match protocol {
        Protocol::OpenAi => format!("{base}/v1/responses"),
        Protocol::Claude => format!("{base}/v1/messages"),
        Protocol::Gemini => {
            // `alt=sse` is required for an SSE stream — without it Gemini
            // returns a chunked JSON array the SSE client can't parse.
            format!("{base}/v1beta/models/{model}:streamGenerateContent?alt=sse")
        }
    }
}

pub fn models_endpoint(host: &str, protocol: Protocol) -> String {
    let base = trim_slash(host);
    match protocol {
        Protocol::OpenAi | Protocol::Claude => format!("{base}/v1/models"),
        Protocol::Gemini => format!("{base}/v1beta/models?pageSize=200"),
    }
}

/// Rewrite the `model` field of a JSON body, returning the new bytes.
/// OpenAI and Claude chat bodies have `{"model": "...", ...}`; Gemini does
/// not (model lives in URL). Image generations (`/v1/images/generations`)
/// also have `{"model": "..."}`. Image edits are multipart — don't rewrite.

pub fn header_str(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn has_byok_headers(headers: &HeaderMap) -> bool {
    header_str(headers, "x-upstream-url").is_some()
        && header_str(headers, "x-upstream-key").is_some()
}

#[cfg(test)]
mod proxy_access_tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn anonymous_byok_requires_both_non_empty_headers() {
        let mut headers = HeaderMap::new();
        assert!(!has_byok_headers(&headers));

        headers.insert(
            "x-upstream-url",
            HeaderValue::from_static("https://api.example.com/v1/responses"),
        );
        assert!(!has_byok_headers(&headers));

        headers.insert("x-upstream-key", HeaderValue::from_static("secret"));
        assert!(has_byok_headers(&headers));

        headers.insert("x-upstream-key", HeaderValue::from_static("   "));
        assert!(!has_byok_headers(&headers));
    }
}

fn override_json_model(body: &[u8], new_model: &str) -> axum::body::Bytes {
    let Ok(mut v) = serde_json::from_slice::<serde_json::Value>(body) else {
        return axum::body::Bytes::copy_from_slice(body);
    };
    if let Some(map) = v.as_object_mut() {
        if map.contains_key("model") {
            map.insert("model".into(), serde_json::Value::String(new_model.into()));
        }
    }
    match serde_json::to_vec(&v) {
        Ok(b) => axum::body::Bytes::from(b),
        Err(_) => axum::body::Bytes::copy_from_slice(body),
    }
}

async fn proxy_forward(
    state: &AppState,
    user: Option<CurrentUser>,
    protocol: Protocol,
    headers: &HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let installed = match state.require_installed().await {
        Ok(s) => s,
        Err(r) => return r,
    };

    // Anonymous access is deliberately limited to BYOK. Without both client
    // headers the resolver would select a paid platform channel.
    if user.is_none() && !has_byok_headers(headers) {
        return (StatusCode::UNAUTHORIZED, "云端模型需要登录").into_response();
    }

    // Extract the requested model (used for channel lookup + body rewrite).
    let req_model = channels::extract_chat_model(&body, headers).unwrap_or_default();

    // Resolve route: BYOK (client headers) or server channel chain.
    let route = match channels::resolve_route(
        &installed.pool,
        installed.kind,
        headers,
        "chat",
        protocol.name(),
        &req_model,
    )
    .await
    {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    match route {
        channels::Route::Byok(byok) => {
            // BYOK: single shot, no credits, no fallback.
            let client = match net_guard::client_for_upstream(&state.http, &byok.base_url, false).await {
                Ok(c) => c,
                Err(r) => return r,
            };
            send_chat_once(client, &byok.base_url, &byok.api_key, protocol, &body, headers).await
        }
        channels::Route::Channels { chain, model } => {
            let Some(user) = user else {
                return (StatusCode::UNAUTHORIZED, "云端模型需要登录").into_response();
            };
            // Deduct based on per-model pricing (whitelist gate).
            // Refund on hard failure.
            let cost = match channels::try_deduct_for_model(
                &installed.pool,
                installed.kind,
                user.id,
                &model,
                "chat",
                protocol.name(),
                &format!("chat_{}", protocol.name()),
            )
            .await
            {
                // Use the amount actually deducted for any later refund — never
                // re-read the price (an admin price change mid-request would
                // otherwise refund a different amount than was charged).
                Ok((_new_bal, deducted)) => deducted,
                Err(channels::DeductError::NotWhitelisted) => {
                    return (
                        StatusCode::FORBIDDEN,
                        format!("模型 {model} 未启用：管理员尚未在「模型计费」中开放此模型，或请在设置里使用自己的 API Key"),
                    )
                        .into_response();
                }
                Err(channels::DeductError::Insufficient { balance, cost }) => {
                    return (
                        StatusCode::PAYMENT_REQUIRED,
                        format!("积分不足：当前 {balance}，本次请求需要 {cost}；请在设置里填入自己的 API Key，或联系管理员充值"),
                    )
                        .into_response();
                }
            };


            // Iterate chain. Fall back on:
            //   * connect error
            //   * non-success status BEFORE we start streaming (pre-stream 5xx/429)
            // First success is streamed back. After streaming starts we can't
            // recover, so any mid-stream error surfaces to the client as-is.
            let mut last_err: Option<(StatusCode, String)> = None;
            for choice in chain {
                let endpoint = chat_endpoint(&choice.channel.base_url, protocol, &choice.upstream_model);
                let client = match net_guard::client_for_upstream(&state.http, &endpoint, true).await {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                // Rewrite model in body for protocols that embed it.
                let attempt_body = match protocol {
                    Protocol::OpenAi | Protocol::Claude if !choice.upstream_model.is_empty() => {
                        override_json_model(&body, &choice.upstream_model)
                    }
                    _ => axum::body::Bytes::copy_from_slice(&body),
                };

                let mut req = client
                    .post(&endpoint)
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::ACCEPT, "text/event-stream")
                    .body(attempt_body);
                req = match protocol {
                    Protocol::OpenAi => req.bearer_auth(&choice.channel.api_key),
                    Protocol::Claude => req
                        .header("x-api-key", choice.channel.api_key.as_str())
                        .header("anthropic-version", "2023-06-01"),
                    Protocol::Gemini => req.header("x-goog-api-key", choice.channel.api_key.as_str()),
                };

                let resp = match req.send().await {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("[chat] channel {} connect failed: {e} — trying next", choice.channel.name);
                        // Don't surface reqwest's error to the client — it can
                        // include the admin-configured upstream URL.
                        last_err = Some((StatusCode::BAD_GATEWAY, "上游服务暂时不可用".to_string()));
                        continue;
                    }
                };
                let status = resp.status();
                if !status.is_success() {
                    // Drain and discard the upstream body — for shared channels
                    // it's generated against the admin's API key and many
                    // providers echo request headers (including Authorization)
                    // back in error responses. Logging or forwarding it would
                    // leak that key.
                    let _ = resp.bytes().await;
                    eprintln!("[chat] channel {} status {status} — trying next", choice.channel.name);
                    last_err = Some((StatusCode::BAD_GATEWAY, format!("上游服务返回 {status}")));
                    continue;
                }

                // Wrap the upstream stream so that if the channel returned 200
                // but then died before sending any bytes, the deducted cost is
                // refunded — otherwise the user pays full price for nothing.
                let guarded = GuardedStream {
                    inner: Box::pin(resp.bytes_stream()),
                    guard: StreamRefundGuard {
                        pool: installed.pool.clone(),
                        kind: installed.kind,
                        user_id: user.id,
                        cost,
                        protocol: protocol.name().to_string(),
                        model: model.to_string(),
                        streamed: false,
                    },
                };
                return Response::builder()
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .header(header::CACHE_CONTROL, "no-cache")
                    .header("x-accel-buffering", "no")
                    .body(Body::from_stream(guarded))
                    .unwrap();
            }

            // All channels failed — refund.
            if cost > 0 {
                let _ = credits::grant(
                    &installed.pool,
                    installed.kind,
                    user.id,
                    cost,
                    &format!("refund_chat_{model}_all_failed"),
                    &credits::LedgerMeta::refund_chat(protocol.name(), &model),
                )
                .await;
            }
            let (status, msg) = last_err
                .unwrap_or((StatusCode::BAD_GATEWAY, format!("模型 {model} 所有渠道均不可用")));
            (status, msg).into_response()
        }
    }
}

/// Refunds the chat cost on drop if the upstream stream never produced bytes —
/// covers a channel that returned HTTP 200 then died before sending anything.
struct StreamRefundGuard {
    pool: db::Pool,
    kind: db::DbKind,
    user_id: i64,
    cost: i64,
    protocol: String,
    model: String,
    streamed: bool,
}

impl Drop for StreamRefundGuard {
    fn drop(&mut self) {
        if self.streamed || self.cost <= 0 {
            return;
        }
        let pool = self.pool.clone();
        let kind = self.kind;
        let user_id = self.user_id;
        let cost = self.cost;
        let protocol = self.protocol.clone();
        let model = self.model.clone();
        let reason = format!("refund_chat_{}_stream_empty", model);
        tokio::spawn(async move {
            let _ = credits::grant(
                &pool,
                kind,
                user_id,
                cost,
                &reason,
                &credits::LedgerMeta::refund_chat(&protocol, &model),
            )
            .await;
        });
    }
}

/// Byte-stream wrapper that marks its guard as soon as one non-empty chunk
/// passes through; the guard refunds on drop if nothing ever did.
struct GuardedStream {
    inner: std::pin::Pin<
        Box<dyn futures_util::stream::Stream<Item = reqwest::Result<axum::body::Bytes>> + Send>,
    >,
    guard: StreamRefundGuard,
}

impl futures_util::stream::Stream for GuardedStream {
    type Item = reqwest::Result<axum::body::Bytes>;
    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let polled = this.inner.as_mut().poll_next(cx);
        if let std::task::Poll::Ready(Some(Ok(chunk))) = &polled {
            if !chunk.is_empty() {
                this.guard.streamed = true;
            }
        }
        polled
    }
}

/// Send one chat request (BYOK path — no fallback, no credits).
async fn send_chat_once(
    client: reqwest::Client,
    base_url: &str,
    api_key: &str,
    protocol: Protocol,
    body: &axum::body::Bytes,
    _headers: &HeaderMap,
) -> Response {
    // BYOK clients supply the full URL already (it points at /v1/chat/...).
    let url = base_url.to_string();
    let mut req = client
        .post(&url)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "text/event-stream")
        .body(body.clone());
    req = match protocol {
        Protocol::OpenAi => req.bearer_auth(api_key),
        Protocol::Claude => req
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01"),
        Protocol::Gemini => req.header("x-goog-api-key", api_key),
    };
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[chat byok] upstream connect failed: {e}");
            return (StatusCode::BAD_GATEWAY, "上游服务暂时不可用".to_string())
                .into_response();
        }
    };
    if !resp.status().is_success() {
        let status = resp.status();
        // Don't forward the upstream body — even on BYOK, providers can echo
        // the caller's own Authorization header back into error responses.
        let _ = resp.bytes().await;
        return (StatusCode::BAD_GATEWAY, format!("上游服务返回 {status}"))
            .into_response();
    }
    let stream = resp.bytes_stream();
    Response::builder()
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header("x-accel-buffering", "no")
        .body(Body::from_stream(stream))
        .unwrap()
}

async fn proxy_openai(
    State(state): State<AppState>,
    user: Option<axum::Extension<CurrentUser>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    proxy_forward(
        &state,
        user.map(|axum::Extension(user)| user),
        Protocol::OpenAi,
        &headers,
        body,
    )
    .await
}

async fn proxy_claude(
    State(state): State<AppState>,
    user: Option<axum::Extension<CurrentUser>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    proxy_forward(
        &state,
        user.map(|axum::Extension(user)| user),
        Protocol::Claude,
        &headers,
        body,
    )
    .await
}

async fn proxy_gemini(
    State(state): State<AppState>,
    user: Option<axum::Extension<CurrentUser>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    proxy_forward(
        &state,
        user.map(|axum::Extension(user)| user),
        Protocol::Gemini,
        &headers,
        body,
    )
    .await
}

async fn proxy_get_forward(
    state: &AppState,
    protocol: Protocol,
    headers: &HeaderMap,
    allow_shared: bool,
) -> Response {
    let installed = match state.require_installed().await {
        Ok(s) => s,
        Err(r) => return r,
    };

    // Client-supplied URL wins (it already points at /v1/models etc.).
    // Otherwise — when the client sent empty X-Upstream-Url/Key headers —
    // fall back to any admin-configured channel for this protocol.
    let hdr_url = headers.get("x-upstream-url").and_then(|v| v.to_str().ok());
    let hdr_key = headers.get("x-upstream-key").and_then(|v| v.to_str().ok());

    let use_client_headers =
        matches!((hdr_url, hdr_key), (Some(u), Some(k)) if !u.is_empty() && !k.is_empty());

    if !use_client_headers && !allow_shared {
        return (StatusCode::UNAUTHORIZED, "云端模型需要登录").into_response();
    }

    let (url, key, used_shared) = if use_client_headers {
        (
            hdr_url.unwrap().to_string(),
            hdr_key.unwrap().to_string(),
            false,
        )
    } else {
        match channels::any_enabled_channel(
            &installed.pool,
            installed.kind,
            protocol.name(),
        )
        .await
        .ok()
        .flatten()
        {
            Some(ch) => (models_endpoint(&ch.base_url, protocol), ch.api_key, true),
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    "未配置任何启用的上游渠道",
                )
                    .into_response();
            }
        }
    };

    let client = match net_guard::client_for_upstream(&state.http, &url, used_shared).await {
        Ok(c) => c,
        Err(r) => return r,
    };

    let mut req = client
        .get(&url)
        .header(header::ACCEPT, "application/json");
    req = match protocol {
        Protocol::OpenAi => req.bearer_auth(&key),
        Protocol::Claude => req
            .header("x-api-key", key.as_str())
            .header("anthropic-version", "2023-06-01"),
        Protocol::Gemini => req.header("x-goog-api-key", key.as_str()),
    };

    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[models] upstream connect failed: {e}");
            return (StatusCode::BAD_GATEWAY, "上游服务暂时不可用".to_string())
                .into_response();
        }
    };

    let status = resp.status();
    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json")
        .to_string();
    let bytes = resp.bytes().await.unwrap_or_default();

    if !status.is_success() {
        // Don't forward the upstream body — for shared channels it was made
        // with the admin's API key and providers often echo headers back.
        return (StatusCode::BAD_GATEWAY, format!("上游服务返回 {status}"))
            .into_response();
    }

    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(bytes))
        .unwrap()
}

async fn proxy_openai_models(
    State(state): State<AppState>,
    user: Option<axum::Extension<CurrentUser>>,
    headers: HeaderMap,
) -> Response {
    proxy_get_forward(&state, Protocol::OpenAi, &headers, user.is_some()).await
}

async fn proxy_claude_models(
    State(state): State<AppState>,
    user: Option<axum::Extension<CurrentUser>>,
    headers: HeaderMap,
) -> Response {
    proxy_get_forward(&state, Protocol::Claude, &headers, user.is_some()).await
}

async fn proxy_gemini_models(
    State(state): State<AppState>,
    user: Option<axum::Extension<CurrentUser>>,
    headers: HeaderMap,
) -> Response {
    proxy_get_forward(&state, Protocol::Gemini, &headers, user.is_some()).await
}

// ---------------------------------------------------------------------------
// static + health
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct Health {
    status: &'static str,
    version: &'static str,
    time: String,
}

async fn health() -> Json<Health> {
    Json(Health {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
        time: Utc::now().to_rfc3339(),
    })
}

async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    if let Some(res) = serve(path) {
        return res;
    }
    if let Some(res) = serve("index.html") {
        return res;
    }
    (StatusCode::NOT_FOUND, "404 Not Found").into_response()
}

fn serve(path: &str) -> Option<Response> {
    let file = Assets::get(path)?;
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    Some(
        Response::builder()
            .header(header::CONTENT_TYPE, mime.as_ref())
            .body(Body::from(file.data.into_owned()))
            .unwrap(),
    )
}

// ---------------------------------------------------------------------------
// router
// ---------------------------------------------------------------------------

fn build_router(state: AppState) -> Router {
    let protected = Router::new()
        .merge(conversations::routes())
        .merge(prompts::routes())
        .merge(skills::routes())
        .merge(images::routes())
        .merge(settings::routes())
        .merge(profile::routes())
        .merge(admin::routes())
        .merge(credits::user_routes())
        .merge(credits::admin_routes())
        .merge(channels::admin_routes())
        .merge(channels::user_routes())
        .merge(payments::user_routes())
        .merge(payments::admin_routes())
        .merge(studio::routes())
        .merge(videos::routes())
        .merge(video_editor::routes())
        .merge(workflows::routes())
        .merge(invites::routes())
        .merge(search::routes())
        .merge(sharing::user_routes())
        .merge(worker::routes())
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth));

    let proxy = Router::new()
        .route("/proxy/openai", post(proxy_openai))
        .route("/proxy/claude", post(proxy_claude))
        .route("/proxy/gemini", post(proxy_gemini))
        .route("/proxy/openai/models", get(proxy_openai_models))
        .route("/proxy/claude/models", get(proxy_claude_models))
        .route("/proxy/gemini/models", get(proxy_gemini_models))
        .route_layer(middleware::from_fn_with_state(state.clone(), optional_auth));

    let public = Router::new()
        .route("/health", get(health))
        .route("/auth/config", get(auth_config))
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/auth/me", get(me))
        .merge(email::public_routes())
        .merge(payments::public_routes())
        .merge(setup::routes())
        .merge(images::public_routes())
        .merge(sharing::public_routes())
        .merge(worker::public_routes())
        .merge(videos::public_routes())
        .merge(video_editor::public_routes());

    Router::new()
        .nest("/api", public.merge(proxy).merge(protected))
        .layer(DefaultBodyLimit::max(1024 * 1024 * 1024))
        .with_state(state)
        .fallback(static_handler)
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    db::install_drivers();

    let data_dir = std::path::PathBuf::from(
        crate::runtime_env::var("YUNOVA_DATA_DIR").unwrap_or_else(|_| "data".into()),
    );
    if let Err(e) = std::fs::create_dir_all(&data_dir) {
        eprintln!("WARNING: failed to create data dir {}: {e}", data_dir.display());
    }
    let config_path = setup::config_path(
        &data_dir,
        runtime_env::var("YUNOVA_CONFIG").ok().as_deref(),
    );

    // priority: env DATABASE_URL -> config file -> install wizard
    let env_url = crate::runtime_env::var("YUNOVA_DATABASE_URL")
        .ok()
        .or_else(|| std::env::var("DATABASE_URL").ok());
    let stored_config = setup::load_config(&config_path).ok();
    let media_storage = storage::MediaStorage::from_config(
        data_dir.clone(),
        stored_config.as_ref().and_then(|config| config.storage.as_ref()),
    )
    .unwrap_or_else(|e| {
        eprintln!("FATAL: invalid media storage configuration ({e})");
        std::process::exit(1);
    });

    let installed = Arc::new(RwLock::new(None));
    let state = AppState {
        installed: installed.clone(),
        http: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(600))
            .build()
            .expect("reqwest client"),
        image_http: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(600))
            .build()
            .expect("image reqwest client"),
        config_path: config_path.clone(),
        data_dir: data_dir.clone(),
        storage: media_storage,
        config_lock: Default::default(),
        auth_limiter: {
            // 20 attempts per 5 minutes per IP, shared across login / register /
            // send-code. Plenty of headroom for legitimate users; brute force
            // (e.g. attempts/sec) gets stopped fast.
            let lim = Arc::new(rate_limit::RateLimiter::new(
                20,
                std::time::Duration::from_secs(300),
            ));
            let pruner = lim.clone();
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    pruner.prune().await;
                }
            });
            lim
        },
        guest_proxy_limiter: {
            let lim = Arc::new(rate_limit::RateLimiter::new(
                60,
                std::time::Duration::from_secs(60),
            ));
            let pruner = lim.clone();
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    pruner.prune().await;
                }
            });
            lim
        },
        workers: crate::worker::WorkerRegistry::new(),
        media_process_slots: std::sync::Arc::new(tokio::sync::Semaphore::new(
            crate::runtime_env::var("YUNOVA_MEDIA_CONCURRENCY")
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(2)
                .clamp(1, 8),
        )),
        approvals: Default::default(),
    };

    let effective_url = match env_url {
        Some(u) => {
            // env wins and also persists to config for cross-restart consistency
            let _ = setup::save_config(
                &config_path,
                &setup::StoredConfig {
                    database_url: u.clone(),
                    storage: stored_config
                        .as_ref()
                        .and_then(|config| config.storage.clone()),
                },
            );
            Some(u)
        }
        None => stored_config.map(|config| config.database_url),
    };

    if let Some(url) = effective_url.as_deref() {
        match setup::boot_installed(url).await {
            Ok(s) => {
                images::cleanup_stale_jobs(&s.pool, s.kind).await;
                studio::cleanup_stale_jobs(&s.pool, s.kind).await;
                workflows::recover(&s.pool, s.kind).await;
                video_editor::recover(&s.pool, s.kind).await;
                *state.installed.write().await = Some(s.clone());
                println!("  database: {} ({})", s.kind.as_str(), url);
            }
            Err(e) => {
                // A database is already configured. Refuse to start rather than
                // fall back to the setup wizard, which could be hijacked to
                // repoint the app at an attacker-controlled database.
                eprintln!("FATAL: configured database is unreachable ({e}); refusing to start");
                std::process::exit(1);
            }
        }
    }

    // Video-generation sweeper: fixes hung polling locks, times out stale
    // jobs (>2h) with a refund, and advances orphaned jobs whose owner closed
    // the page before polling completed. Cheap no-op when there's no video
    // activity. Runs every 60s, separate from the rate-limiter pruner above
    // because `state.installed` isn't populated yet at that point.
    let sweeper_state = state.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            let installed = sweeper_state.installed.read().await.clone();
            if let Some(s) = installed {
                videos::sweep(&sweeper_state.http, &s.pool, s.kind, &sweeper_state.storage).await;
            }
        }
    });

    // Workflow runs normally have a short-lived per-run driver. This sweeper
    // resumes them after restarts and covers any driver interrupted by a panic.
    let workflow_state = state.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(15)).await;
            let installed = workflow_state.installed.read().await.clone();
            if let Some(installed) = installed {
                workflows::sweep(&workflow_state, &installed).await;
            }
        }
    });

    let addr = crate::runtime_env::var("YUNOVA_BIND").unwrap_or_else(|_| "127.0.0.1:3000".into());
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    println!("Yunova listening on http://{addr}");
    println!(
        "  media storage: {} ({})",
        state.storage.backend_name(),
        state.storage.location()
    );
    if state.installed.read().await.is_none() {
        println!("  (not yet installed — open http://{addr}/setup to configure)");
    }
    axum::serve(listener, build_router(state))
        .await
        .expect("server error");
}
