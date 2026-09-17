//! CLI sign-in by device code.
//!
//! The desktop app can attach itself because it owns a window: it reads its
//! own webview's session cookie and proves the machine with that. A CLI has no
//! window, and the two obvious alternatives are both wrong:
//!
//! * Asking for the account password would put the credential that *is* the
//!   account into a terminal, a shell history and a user's own disk, on a
//!   machine the platform has no reason to trust.
//! * Asking the user to paste an agent token turns "log in" into a manual
//!   token-management chore, and users paste long-lived tokens into places
//!   they forget about.
//!
//! So the CLI starts a code and the *browser* approves it. The approval
//! happens in a session that is already authenticated, which is what makes the
//! short user code safe: knowing a code is not enough, someone must also be
//! signed in and click approve. What the CLI ends up holding is an ordinary
//! agent token — the same credential shape the sandbox and the desktop client
//! already use, so the gateway, the model whitelist and the metering chain are
//! unchanged.
//!
//! The asymmetry between the two codes is the whole design:
//!
//! * `user_code` is short because a human retypes it. It is therefore
//!   low-entropy, so it is useless without an authenticated approver and it
//!   expires in minutes.
//! * `device_code` is high-entropy because it is the bearer of the eventual
//!   token. Only its hash is stored, and the plaintext token is returned on
//!   exactly one poll — a second poll gets nothing, so a code captured from a
//!   log cannot be replayed into a working credential.

use axum::{
    Extension, Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::json;

use crate::{
    AppState, CurrentUser, InstalledState, agent_token, auth,
    db::{self, DbKind, Pool},
    rate_limit,
};

/// How long an unapproved code stays usable.
///
/// Short on purpose: the user code is only a handful of characters, and its
/// safety rests on the window in which guessing is even possible being small.
/// Ten minutes is long enough to walk to another device and sign in.
const CODE_TTL_MINUTES: i64 = 10;

/// How long the CLI should wait between polls, in seconds. Advertised to the
/// client rather than enforced, because the rate limiter is the actual
/// control; this just keeps a well-behaved client from hammering.
const POLL_INTERVAL_SECONDS: i64 = 3;

/// Lifetime of the token a successful approval mints.
///
/// Unlike a session credential this is not tied to anything that ends, so it
/// gets a long but finite life: a CLI the user forgets about should eventually
/// stop spending quota, and `/logout` or the token list can end it sooner.
const CLI_TOKEN_TTL_HOURS: i64 = 24 * 30;

/// Characters used in the user-visible code.
///
/// No `0/O`, `1/I/L`, `5/S`, `8/B`: the code is read off one screen and typed
/// into another, and a character set that invites transcription errors turns a
/// login into three failed attempts.
const CODE_ALPHABET: &[u8] = b"ACDEFGHJKMNPQRTUVWXY234679";

/// Length of each half of the user code, rendered as `XXXX-XXXX`.
const CODE_HALF: usize = 4;

fn new_user_code() -> String {
    use rand::Rng;
    let mut rng = rand::rngs::OsRng;
    let pick = |rng: &mut rand::rngs::OsRng| {
        let idx = rng.gen_range(0..CODE_ALPHABET.len());
        CODE_ALPHABET[idx] as char
    };
    let left: String = (0..CODE_HALF).map(|_| pick(&mut rng)).collect();
    let right: String = (0..CODE_HALF).map(|_| pick(&mut rng)).collect();
    format!("{left}-{right}")
}

/// Normalise what a user typed into what is stored.
///
/// People paste the code with the dash, without it, in lower case, or with a
/// stray space. All of those are the same code, and rejecting them would be a
/// puzzle rather than a security boundary — the entropy is in the characters,
/// not in the punctuation.
pub fn normalize_user_code(raw: &str) -> String {
    let compact: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if compact.len() == CODE_HALF * 2 {
        format!("{}-{}", &compact[..CODE_HALF], &compact[CODE_HALF..])
    } else {
        compact
    }
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Whether an RFC3339 instant has passed. Unparseable counts as expired, so a
/// corrupt row fails closed instead of granting forever.
fn is_past(raw: &str, now: chrono::DateTime<chrono::Utc>) -> bool {
    match chrono::DateTime::parse_from_rfc3339(raw) {
        Ok(at) => at.with_timezone(&chrono::Utc) <= now,
        Err(_) => true,
    }
}

/// Trim and bound a client-supplied display string.
///
/// These are shown on the approval page, which is where a user decides whether
/// to hand over a credential, so they are length-capped and stripped of
/// control characters: a "client name" containing newlines could otherwise
/// push the real prompt off screen.
fn clean_label(raw: Option<&str>, fallback: &str, max: usize) -> String {
    let cleaned: String = raw
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control())
        .take(max)
        .collect();
    let cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() {
        fallback.to_string()
    } else {
        cleaned
    }
}

// ---------------------------------------------------------------------------
// start
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct StartReq {
    /// What is asking, e.g. `pi`. Shown on the approval page.
    #[serde(default)]
    client_name: Option<String>,
    #[serde(default)]
    hostname: Option<String>,
    #[serde(default)]
    platform: Option<String>,
}

/// Begin a login. Public: the caller has no credential yet, that is the point.
///
/// Rate-limited on the shared auth limiter because this endpoint creates rows
/// and user codes; without a bound, an anonymous caller could fill the code
/// space and make legitimate codes collide.
async fn start(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<StartReq>,
) -> Response {
    let installed = match state.require_installed().await {
        Ok(s) => s,
        Err(r) => return r,
    };
    if !state
        .auth_limiter
        .allow(rate_limit::client_ip(&headers))
        .await
    {
        return (StatusCode::TOO_MANY_REQUESTS, "请求过于频繁，请稍后再试").into_response();
    }

    // Opportunistic cleanup: expired rows have no use and their user codes
    // should return to the pool. Doing it here rather than in a sweeper keeps
    // the table self-maintaining without another background task.
    prune_expired(&installed.pool, installed.kind).await;

    let device_code = auth::generate_token();
    let device_hash = auth::token_hash(&device_code);
    let expires_at =
        (chrono::Utc::now() + chrono::Duration::minutes(CODE_TTL_MINUTES)).to_rfc3339();

    let client_name = clean_label(req.client_name.as_deref(), "命令行工具", 60);
    let hostname = clean_label(req.hostname.as_deref(), "未知设备", 60);
    let platform = clean_label(req.platform.as_deref(), "unknown", 32);

    // Retry on collision rather than trusting one draw: the user code is short
    // by design, so duplicates are rare but not impossible, and a UNIQUE
    // violation surfacing as "登录失败" would be a confusing dead end.
    let mut last_err = String::new();
    for _ in 0..8 {
        let user_code = new_user_code();
        let sql = db::q(
            installed.kind,
            "INSERT INTO cli_device_codes \
             (user_code, device_hash, client_name, hostname, platform, expires_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
        );
        match sqlx::query(&sql)
            .bind(&user_code)
            .bind(&device_hash)
            .bind(&client_name)
            .bind(&hostname)
            .bind(&platform)
            .bind(&expires_at)
            .execute(&installed.pool)
            .await
        {
            Ok(_) => {
                return Json(json!({
                    "device_code": device_code,
                    "user_code": user_code,
                    "verification_uri": "/cli/login",
                    "expires_in": CODE_TTL_MINUTES * 60,
                    "interval": POLL_INTERVAL_SECONDS,
                }))
                .into_response();
            }
            Err(e) => last_err = e.to_string(),
        }
    }
    eprintln!("[cli-auth] could not allocate a user code: {last_err}");
    (StatusCode::INTERNAL_SERVER_ERROR, "无法创建登录码").into_response()
}

// ---------------------------------------------------------------------------
// poll
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct PollReq {
    device_code: String,
}

/// One row of the code lookup.
type CodeRow = (
    i64,            // id
    Option<i64>,    // user_id
    String,         // expires_at
    Option<String>, // approved_at
    Option<String>, // denied_at
    Option<String>, // consumed_at
);

/// Report whether the code has been approved, and hand over the token once.
///
/// The status vocabulary matches the OAuth device flow because CLI authors
/// already know it: `authorization_pending` means keep polling, anything else
/// is terminal.
async fn poll(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<PollReq>,
) -> Response {
    let installed = match state.require_installed().await {
        Ok(s) => s,
        Err(r) => return r,
    };
    if !state
        .auth_limiter
        .allow(rate_limit::client_ip(&headers))
        .await
    {
        return (StatusCode::TOO_MANY_REQUESTS, "请求过于频繁，请稍后再试").into_response();
    }

    let hash = auth::token_hash(req.device_code.trim());
    let sql = db::q(
        installed.kind,
        "SELECT id, user_id, expires_at, approved_at, denied_at, consumed_at \
         FROM cli_device_codes WHERE device_hash = ?",
    );
    let row: Option<CodeRow> = sqlx::query_as(&sql)
        .bind(&hash)
        .fetch_optional(&installed.pool)
        .await
        .ok()
        .flatten();

    let Some((id, user_id, expires_at, approved_at, denied_at, consumed_at)) = row else {
        return Json(json!({ "status": "expired_token" })).into_response();
    };
    if denied_at.is_some() {
        return Json(json!({ "status": "access_denied" })).into_response();
    }
    // Checked before approval so an approval that arrived after the deadline
    // cannot be collected late.
    if is_past(&expires_at, chrono::Utc::now()) {
        return Json(json!({ "status": "expired_token" })).into_response();
    }
    if consumed_at.is_some() {
        // The token was already handed out. Saying so plainly is better than
        // "pending", which would leave a second CLI polling forever.
        return Json(json!({ "status": "expired_token" })).into_response();
    }
    let (Some(user_id), Some(_)) = (user_id, approved_at.as_ref()) else {
        return Json(json!({
            "status": "authorization_pending",
            "interval": POLL_INTERVAL_SECONDS,
        }))
        .into_response();
    };

    // Approved and not yet collected: mint now, on the poll, rather than at
    // approval time. The plaintext then exists only in this response, so it is
    // never stored waiting to be picked up.
    let token = match agent_token::mint(
        &installed.pool,
        installed.kind,
        user_id,
        "CLI 登录",
        Some(CLI_TOKEN_TTL_HOURS),
    )
    .await
    {
        Ok(t) => t,
        Err(e) => {
            eprintln!("[cli-auth] minting the CLI token failed: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "签发令牌失败").into_response();
        }
    };

    // Mark consumed only for a row that is still unconsumed, so two polls
    // racing cannot both mint. The loser sees zero rows affected and must not
    // return its token.
    let claim = db::q(
        installed.kind,
        "UPDATE cli_device_codes SET consumed_at = ?, token_hash = ? \
         WHERE id = ? AND consumed_at IS NULL",
    );
    let claimed = sqlx::query(&claim)
        .bind(now_rfc3339())
        .bind(auth::token_hash(&token))
        .bind(id)
        .execute(&installed.pool)
        .await
        .map(|r| r.rows_affected())
        .unwrap_or(0);
    if claimed == 0 {
        // Another poll won. Revoke what this one minted rather than leaving an
        // orphan credential nobody will ever see in a list.
        agent_token::revoke_by_plaintext(&installed.pool, installed.kind, &token).await;
        return Json(json!({ "status": "expired_token" })).into_response();
    }

    let username: Option<String> = {
        let sql = db::q(installed.kind, "SELECT username FROM users WHERE id = ?");
        sqlx::query_scalar(&sql)
            .bind(user_id)
            .fetch_optional(&installed.pool)
            .await
            .ok()
            .flatten()
    };

    Json(json!({
        "status": "ok",
        "token": token,
        "username": username,
        "expires_in": CLI_TOKEN_TTL_HOURS * 3600,
    }))
    .into_response()
}

// ---------------------------------------------------------------------------
// approve / deny (browser, authenticated)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CodeLookupReq {
    user_code: String,
}

/// One row of the approval-page lookup.
///
/// Named rather than inlined because the tuple is what `sqlx` hands back and
/// an anonymous six-field tuple at the call site says nothing about which
/// column is which.
type DescribeRow = (
    String,         // client_name
    Option<String>, // hostname
    Option<String>, // platform
    String,         // expires_at
    Option<String>, // approved_at
    Option<String>, // denied_at
);

/// What the approval page shows before the user decides.
///
/// Authenticated: only a signed-in user may even learn whether a code exists,
/// which is what keeps the short code from being enumerable by an anonymous
/// caller.
async fn describe(
    Extension(installed): Extension<InstalledState>,
    Extension(_user): Extension<CurrentUser>,
    Json(req): Json<CodeLookupReq>,
) -> Response {
    let code = normalize_user_code(&req.user_code);
    let sql = db::q(
        installed.kind,
        "SELECT client_name, hostname, platform, expires_at, approved_at, denied_at \
         FROM cli_device_codes WHERE user_code = ?",
    );
    let row: Option<DescribeRow> = sqlx::query_as(&sql)
        .bind(&code)
        .fetch_optional(&installed.pool)
        .await
        .ok()
        .flatten();

    let Some((client_name, hostname, platform, expires_at, approved_at, denied_at)) = row else {
        return (StatusCode::NOT_FOUND, "登录码不存在或已过期").into_response();
    };
    if is_past(&expires_at, chrono::Utc::now()) {
        return (StatusCode::GONE, "登录码已过期，请在命令行重新发起登录").into_response();
    }
    Json(json!({
        "client_name": client_name,
        "hostname": hostname,
        "platform": platform,
        "expires_at": expires_at,
        "approved": approved_at.is_some(),
        "denied": denied_at.is_some(),
    }))
    .into_response()
}

/// Approve a pending code, binding it to the signed-in account.
///
/// The `user_id IS NULL` guard makes approval idempotent in the direction that
/// matters: a second click by the same person is harmless, but a code already
/// claimed by one account can never be re-pointed at another.
async fn approve(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Json(req): Json<CodeLookupReq>,
) -> Response {
    let code = normalize_user_code(&req.user_code);
    let now = now_rfc3339();
    let sql = db::q(
        installed.kind,
        "UPDATE cli_device_codes SET user_id = ?, approved_at = ? \
         WHERE user_code = ? AND user_id IS NULL AND denied_at IS NULL AND expires_at > ?",
    );
    let affected = sqlx::query(&sql)
        .bind(user.id)
        .bind(&now)
        .bind(&code)
        .bind(&now)
        .execute(&installed.pool)
        .await
        .map(|r| r.rows_affected())
        .unwrap_or(0);

    if affected == 0 {
        // Distinguish "already mine" from "not available": re-approving one's
        // own code must not read as a failure.
        let check = db::q(
            installed.kind,
            "SELECT user_id FROM cli_device_codes WHERE user_code = ?",
        );
        let owner: Option<Option<i64>> = sqlx::query_scalar(&check)
            .bind(&code)
            .fetch_optional(&installed.pool)
            .await
            .ok()
            .flatten();
        if let Some(Some(owner)) = owner
            && owner == user.id
        {
            return StatusCode::NO_CONTENT.into_response();
        }
        return (
            StatusCode::BAD_REQUEST,
            "登录码无效、已过期或已被处理，请在命令行重新发起登录",
        )
            .into_response();
    }
    StatusCode::NO_CONTENT.into_response()
}

/// Refuse a code.
///
/// Worth having as its own action: a user who sees a code they did not start
/// needs a way to end it that is not "wait ten minutes and hope".
async fn deny(
    Extension(installed): Extension<InstalledState>,
    Extension(_user): Extension<CurrentUser>,
    Json(req): Json<CodeLookupReq>,
) -> Response {
    let code = normalize_user_code(&req.user_code);
    let sql = db::q(
        installed.kind,
        "UPDATE cli_device_codes SET denied_at = ? WHERE user_code = ? AND approved_at IS NULL",
    );
    let _ = sqlx::query(&sql)
        .bind(now_rfc3339())
        .bind(&code)
        .execute(&installed.pool)
        .await;
    StatusCode::NO_CONTENT.into_response()
}

/// Drop rows whose deadline has passed.
///
/// Consumed rows go too: once the token exists it lives in `agent_tokens`,
/// which is the list the user actually manages, so keeping the code row adds
/// nothing but a table that grows forever.
async fn prune_expired(pool: &Pool, kind: DbKind) {
    let sql = db::q(kind, "DELETE FROM cli_device_codes WHERE expires_at < ?");
    let _ = sqlx::query(&sql).bind(now_rfc3339()).execute(pool).await;
}

// ---------------------------------------------------------------------------
// models
// ---------------------------------------------------------------------------

/// The runtime config a CLI needs, addressed to the origin it called.
///
/// Distinct from `POST /api/agent/runtime-config`, which takes a token in the
/// body because the caller is minting config for *another* process. Here the
/// caller is the runtime, it authenticates with its own token, and it should
/// never have to repeat that token back to us to be told what it may use.
async fn models(
    State(state): State<AppState>,
    Extension(installed): Extension<InstalledState>,
    Extension(_user): Extension<CurrentUser>,
    headers: HeaderMap,
) -> Response {
    let prices = match crate::channels::list_pricing(&installed.pool, installed.kind).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[cli-auth] pricing lookup failed: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "读取模型定价失败").into_response();
        }
    };
    let _ = &state;

    // The gateway base has to be the address the CLI actually reached, not a
    // configured guess: a self-hosted instance behind a different hostname
    // would otherwise hand out a baseUrl that resolves nowhere.
    let base = origin_from_headers(&headers);
    let models: Vec<serde_json::Value> = prices
        .iter()
        .filter(|p| p.enabled && p.kind == "chat")
        .filter(|p| agent_token::runtime_provider_for(&p.protocol).is_some())
        .map(|p| {
            json!({
                "id": p.model,
                "name": p.display_name.clone().unwrap_or_else(|| p.model.clone()),
                "protocol": p.protocol,
                "contextWindow": p.context_limit.unwrap_or(200_000),
            })
        })
        .collect();

    Json(json!({ "base_url": base, "models": models })).into_response()
}

/// Reconstruct the origin the client used from the proxy headers.
///
/// Falls back to a relative base rather than inventing a hostname: a wrong
/// absolute URL is worse than none, because the CLI would cache it.
fn origin_from_headers(headers: &HeaderMap) -> Option<String> {
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get(axum::http::header::HOST))
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|h| !h.is_empty())?;
    // Behind TLS termination the scheme is only knowable from the header the
    // proxy sets; default to https because that is what a public deployment
    // is, and a downgraded guess would send a token over plaintext.
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(
            if host.starts_with("localhost") || host.starts_with("127.0.0.1") {
                "http"
            } else {
                "https"
            },
        );
    Some(format!("{scheme}://{host}"))
}

// ---------------------------------------------------------------------------
// routes
// ---------------------------------------------------------------------------

/// Endpoints a CLI reaches before it has any credential.
pub fn public_routes() -> Router<AppState> {
    Router::new()
        .route("/cli/auth/start", post(start))
        .route("/cli/auth/poll", post(poll))
}

/// Endpoints that require a signed-in browser: approving a code is the one
/// action a CLI must never be able to perform for itself.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/cli/auth/code", post(describe))
        .route("/cli/auth/approve", post(approve))
        .route("/cli/auth/deny", post(deny))
}

/// Endpoints a signed-in browser *or* a token-bearing CLI may call.
///
/// Separated because the CLI is exactly the caller here and it has no cookie
/// jar: leaving `models` on the cookie-only router made `/login` succeed and
/// then report no models at all, which is the failure this split exists to
/// prevent.
pub fn agent_routes() -> Router<AppState> {
    Router::new().route("/cli/models", get(models))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_user_code_avoids_characters_people_confuse() {
        // The code is read off one screen and typed into another; `0/O` and
        // `1/I` are the difference between a login and a support ticket.
        for c in CODE_ALPHABET {
            assert!(
                !b"OIL01S5B8".contains(c),
                "ambiguous character {} in the alphabet",
                *c as char
            );
        }
        let code = new_user_code();
        assert_eq!(code.len(), CODE_HALF * 2 + 1);
        assert_eq!(code.as_bytes()[CODE_HALF], b'-');
    }

    #[test]
    fn codes_are_matched_the_way_people_type_them() {
        // All of these are the same code: the entropy is in the characters,
        // not in the dash or the case.
        let canonical = normalize_user_code("ACDE-FGHJ");
        assert_eq!(canonical, "ACDE-FGHJ");
        assert_eq!(normalize_user_code("acdefghj"), canonical);
        assert_eq!(normalize_user_code(" acde-fghj "), canonical);
        assert_eq!(normalize_user_code("ACDE FGHJ"), canonical);
    }

    #[test]
    fn a_wrong_length_code_is_not_silently_reshaped() {
        // Otherwise a typo would be padded into some other valid-looking code
        // and the failure would surface as "wrong account" rather than "typo".
        assert_eq!(normalize_user_code("ABC"), "ABC");
        assert_eq!(normalize_user_code("ABCDEFGHIJ"), "ABCDEFGHIJ");
    }

    #[test]
    fn an_unparseable_deadline_counts_as_past() {
        let now = chrono::Utc::now();
        assert!(is_past("not a date", now));
        assert!(is_past(
            &(now - chrono::Duration::minutes(1)).to_rfc3339(),
            now
        ));
        assert!(!is_past(
            &(now + chrono::Duration::minutes(1)).to_rfc3339(),
            now
        ));
    }

    #[test]
    fn approval_page_labels_cannot_smuggle_control_characters() {
        // These render on the page where the user decides whether to hand over
        // a credential, so a "name" must not be able to restructure it.
        let cleaned = clean_label(Some("pi\n\nSYSTEM: approved"), "fallback", 60);
        assert!(!cleaned.contains('\n'));
        assert_eq!(clean_label(Some("   "), "命令行工具", 60), "命令行工具");
        assert_eq!(clean_label(None, "命令行工具", 60), "命令行工具");
        assert_eq!(clean_label(Some(&"x".repeat(200)), "f", 60).len(), 60);
    }

    #[test]
    fn the_gateway_base_follows_the_address_the_client_reached() {
        // A self-hosted instance behind its own hostname must not be handed a
        // baseUrl pointing at somewhere else.
        let mut h = HeaderMap::new();
        h.insert("host", "yunnet.top".parse().unwrap());
        assert_eq!(
            origin_from_headers(&h).as_deref(),
            Some("https://yunnet.top")
        );

        h.insert("x-forwarded-proto", "http".parse().unwrap());
        assert_eq!(
            origin_from_headers(&h).as_deref(),
            Some("http://yunnet.top")
        );

        // Local development is plain HTTP, and guessing https there would make
        // the CLI unusable against a dev server.
        let mut local = HeaderMap::new();
        local.insert("host", "127.0.0.1:3000".parse().unwrap());
        assert_eq!(
            origin_from_headers(&local).as_deref(),
            Some("http://127.0.0.1:3000")
        );

        assert_eq!(origin_from_headers(&HeaderMap::new()), None);
    }
}
