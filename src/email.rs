use axum::{
    Extension, Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use chrono::{Duration, Utc};
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Tokio1Executor,
    message::{Mailbox, Message},
    transport::smtp::{
        authentication::Credentials,
        client::{Tls, TlsParameters},
    },
};
use rand::Rng;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::{
    AppState, InstalledState,
    quota,
    db::{self, DbKind, Pool},
};

const CODE_LEN: usize = 6;
const CODE_TTL_MINUTES: i64 = 10;
const RESEND_COOLDOWN_SECONDS: i64 = 60;
const MAX_ATTEMPTS: i64 = 5;

// ---------------------------------------------------------------------------
// validation + helpers
// ---------------------------------------------------------------------------

pub fn normalize_email(raw: &str) -> String {
    raw.trim().to_ascii_lowercase()
}

pub fn valid_email(email: &str) -> bool {
    // Minimal sanity check — good enough for a registration gate.
    // Length and exactly-one '@' with non-empty local/domain parts containing a dot.
    if email.is_empty() || email.len() > 254 {
        return false;
    }
    let mut parts = email.split('@');
    let local = parts.next().unwrap_or("");
    let domain = parts.next().unwrap_or("");
    if parts.next().is_some() {
        return false;
    }
    if local.is_empty() || domain.is_empty() || local.len() > 64 {
        return false;
    }
    if !domain.contains('.') {
        return false;
    }
    for ch in email.chars() {
        if ch.is_whitespace() {
            return false;
        }
    }
    true
}

fn gen_code() -> String {
    let mut rng = rand::rngs::OsRng;
    (0..CODE_LEN)
        .map(|_| char::from(b'0' + rng.gen_range(0..10)))
        .collect()
}

fn hash_code(code: &str) -> String {
    let mut h = Sha256::new();
    h.update(code.as_bytes());
    hex::encode(h.finalize())
}

// ---------------------------------------------------------------------------
// SMTP config + sender
// ---------------------------------------------------------------------------

struct SmtpConfig {
    host: String,
    port: u16,
    username: String,
    password: String,
    from_email: String,
    from_name: String,
    security: SmtpSecurity,
}

enum SmtpSecurity {
    None,
    StartTls,
    Tls,
}

impl SmtpSecurity {
    fn from_str(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" | "plain" => Self::None,
            "tls" | "ssl" | "smtps" => Self::Tls,
            _ => Self::StartTls,
        }
    }
}

async fn load_smtp(pool: &Pool, kind: DbKind) -> Result<SmtpConfig, String> {
    let host = quota::get_setting(pool, kind, "smtp_host")
        .await
        .unwrap_or_default();
    if host.trim().is_empty() {
        return Err("管理员尚未配置 SMTP 服务器".into());
    }
    let port = quota::get_setting_i64(pool, kind, "smtp_port", 587)
        .await
        .max(1) as u16;
    let username = quota::get_setting(pool, kind, "smtp_username")
        .await
        .unwrap_or_default();
    let password = quota::get_setting(pool, kind, "smtp_password")
        .await
        .unwrap_or_default();
    let from_email = quota::get_setting(pool, kind, "smtp_from_email")
        .await
        .unwrap_or_default();
    if from_email.trim().is_empty() {
        return Err("管理员尚未配置发件人邮箱".into());
    }
    let from_name = quota::get_setting(pool, kind, "smtp_from_name")
        .await
        .unwrap_or_else(|| "Yunova".into());
    let security_raw = quota::get_setting(pool, kind, "smtp_security")
        .await
        .unwrap_or_else(|| "starttls".into());
    Ok(SmtpConfig {
        host: host.trim().to_string(),
        port,
        username,
        password,
        from_email: from_email.trim().to_string(),
        from_name,
        security: SmtpSecurity::from_str(&security_raw),
    })
}

async fn send_mail(cfg: &SmtpConfig, to: &str, subject: &str, body: String) -> Result<(), String> {
    let from_mailbox: Mailbox = format!("{} <{}>", cfg.from_name, cfg.from_email)
        .parse()
        .map_err(|e: lettre::address::AddressError| format!("发件人地址无效: {e}"))?;
    let to_mailbox: Mailbox = to
        .parse()
        .map_err(|e: lettre::address::AddressError| format!("收件人地址无效: {e}"))?;
    let message = Message::builder()
        .from(from_mailbox)
        .to(to_mailbox)
        .subject(subject)
        .header(lettre::message::header::ContentType::TEXT_PLAIN)
        .body(body)
        .map_err(|e| format!("构建邮件失败: {e}"))?;

    let tls_params = TlsParameters::new(cfg.host.clone())
        .map_err(|e| format!("TLS 配置失败: {e}"))?;
    let builder = match cfg.security {
        SmtpSecurity::Tls => {
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&cfg.host)
                .tls(Tls::Wrapper(tls_params))
        }
        SmtpSecurity::StartTls => {
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&cfg.host)
                .tls(Tls::Required(tls_params))
        }
        SmtpSecurity::None => {
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&cfg.host).tls(Tls::None)
        }
    };
    let builder = builder.port(cfg.port);
    let builder = if !cfg.username.is_empty() || !cfg.password.is_empty() {
        builder.credentials(Credentials::new(cfg.username.clone(), cfg.password.clone()))
    } else {
        builder
    };
    let mailer = builder.build();
    mailer
        .send(message)
        .await
        .map_err(|e| format!("发送邮件失败: {e}"))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// code storage + verification
// ---------------------------------------------------------------------------

async fn recent_code_seconds(
    pool: &Pool,
    kind: DbKind,
    email: &str,
    purpose: &str,
) -> Option<i64> {
    let sql = db::q(
        kind,
        "SELECT created_at FROM email_codes
         WHERE email = ? AND purpose = ?
         ORDER BY created_at DESC LIMIT 1",
    );
    let row: Option<(String,)> = sqlx::query_as(&sql)
        .bind(email)
        .bind(purpose)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    let (created,) = row?;
    let parsed = chrono::DateTime::parse_from_rfc3339(&created)
        .ok()
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(&created, "%Y-%m-%d %H:%M:%S%.f")
                .ok()
                .map(|n| n.and_utc().into())
        })
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(&created, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|n| n.and_utc().into())
        })?;
    let dt = parsed.with_timezone(&Utc);
    Some((Utc::now() - dt).num_seconds())
}

async fn store_code(
    pool: &Pool,
    kind: DbKind,
    email: &str,
    code: &str,
    purpose: &str,
) -> Result<(), sqlx::Error> {
    let expires = (Utc::now() + Duration::minutes(CODE_TTL_MINUTES)).to_rfc3339();
    let sql = db::q(
        kind,
        "INSERT INTO email_codes (email, code_hash, purpose, expires_at) VALUES (?, ?, ?, ?)",
    );
    sqlx::query(&sql)
        .bind(email)
        .bind(hash_code(code))
        .bind(purpose)
        .bind(&expires)
        .execute(pool)
        .await?;
    Ok(())
}

/// Checks the code and marks it used on success. Returns true iff the code
/// matches an unused, non-expired row for this email+purpose.
pub async fn consume_code(
    pool: &Pool,
    kind: DbKind,
    email: &str,
    code: &str,
    purpose: &str,
) -> bool {
    let normalized_email = normalize_email(email);
    let code_trim = code.trim();
    if code_trim.is_empty() {
        return false;
    }
    let hash = hash_code(code_trim);
    // Find the most recent unused, non-expired row for this email+purpose.
    let now = Utc::now().to_rfc3339();
    let sql = db::q(
        kind,
        "SELECT id, code_hash, attempts FROM email_codes
         WHERE email = ? AND purpose = ? AND used = 0 AND expires_at > ?
         ORDER BY created_at DESC LIMIT 1",
    );
    let row: Option<(i64, String, i64)> = sqlx::query_as(&sql)
        .bind(&normalized_email)
        .bind(purpose)
        .bind(&now)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    let Some((id, stored_hash, attempts)) = row else {
        return false;
    };
    if attempts >= MAX_ATTEMPTS {
        return false;
    }
    if stored_hash != hash {
        let upd = db::q(
            kind,
            "UPDATE email_codes SET attempts = attempts + 1 WHERE id = ?",
        );
        let _ = sqlx::query(&upd).bind(id).execute(pool).await;
        return false;
    }
    let upd = db::q(kind, "UPDATE email_codes SET used = 1 WHERE id = ?");
    sqlx::query(&upd)
        .bind(id)
        .execute(pool)
        .await
        .map(|_| true)
        .unwrap_or(false)
}

async fn email_already_registered(pool: &Pool, kind: DbKind, email: &str) -> bool {
    let sql = db::q(kind, "SELECT 1 FROM users WHERE email = ? LIMIT 1");
    sqlx::query_as::<_, (i64,)>(&sql)
        .bind(email)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .is_some()
}

// ---------------------------------------------------------------------------
// HTTP endpoints (public)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SendCodeReq {
    email: String,
    #[serde(default)]
    purpose: Option<String>,
}

async fn send_code(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<SendCodeReq>,
) -> Response {
    if !state.auth_limiter.allow(crate::rate_limit::client_ip(&headers)).await {
        return (StatusCode::TOO_MANY_REQUESTS, "请求过于频繁，请稍后再试").into_response();
    }
    let installed = match state.require_installed().await {
        Ok(s) => s,
        Err(r) => return r,
    };

    let email = normalize_email(&req.email);
    if !valid_email(&email) {
        return (StatusCode::BAD_REQUEST, "邮箱格式不正确").into_response();
    }
    let purpose = req.purpose.as_deref().unwrap_or("register");
    if purpose != "register" {
        return (StatusCode::BAD_REQUEST, "未知用途").into_response();
    }

    // Only allow sending a registration code when the feature is actually on —
    // otherwise we'd pointlessly spam. This lets the frontend gate the button
    // on /auth/config instead of silently hitting this.
    let required = quota::get_setting_bool(
        &installed.pool,
        installed.kind,
        "email_verification_required",
        false,
    )
    .await;
    if !required {
        return (StatusCode::BAD_REQUEST, "当前未启用邮箱验证").into_response();
    }

    // Don't let an already-registered address trigger a code (would leak which
    // addresses are used, but acceptable tradeoff: avoids harassment emails).
    if email_already_registered(&installed.pool, installed.kind, &email).await {
        return (StatusCode::CONFLICT, "该邮箱已注册").into_response();
    }

    if let Some(secs) =
        recent_code_seconds(&installed.pool, installed.kind, &email, purpose).await
    {
        if secs < RESEND_COOLDOWN_SECONDS {
            let wait = RESEND_COOLDOWN_SECONDS - secs;
            return (
                StatusCode::TOO_MANY_REQUESTS,
                format!("请 {wait} 秒后再试"),
            )
                .into_response();
        }
    }

    let smtp = match load_smtp(&installed.pool, installed.kind).await {
        Ok(c) => c,
        Err(e) => return (StatusCode::SERVICE_UNAVAILABLE, e).into_response(),
    };

    let code = gen_code();
    if let Err(e) = store_code(&installed.pool, installed.kind, &email, &code, purpose).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }

    let subject = "Yunova 注册验证码";
    let body = format!(
        "您好，\n\n您的 Yunova 注册验证码是：{code}\n\n该验证码 {CODE_TTL_MINUTES} 分钟内有效。如果不是您本人操作，请忽略此邮件。\n"
    );
    if let Err(e) = send_mail(&smtp, &email, subject, body).await {
        return (StatusCode::BAD_GATEWAY, e).into_response();
    }

    StatusCode::NO_CONTENT.into_response()
}

// ---------------------------------------------------------------------------
// admin: test SMTP by sending to a given address
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct TestMailReq {
    pub email: String,
}

pub async fn admin_send_test(
    Extension(installed): Extension<InstalledState>,
    Json(req): Json<TestMailReq>,
) -> Response {
    let email = normalize_email(&req.email);
    if !valid_email(&email) {
        return (StatusCode::BAD_REQUEST, "邮箱格式不正确").into_response();
    }
    let smtp = match load_smtp(&installed.pool, installed.kind).await {
        Ok(c) => c,
        Err(e) => return (StatusCode::SERVICE_UNAVAILABLE, e).into_response(),
    };
    let subject = "Yunova SMTP 测试邮件";
    let body = "这是一封 Yunova 测试邮件，用于验证 SMTP 配置是否正常。\n".to_string();
    match send_mail(&smtp, &email, subject, body).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, e).into_response(),
    }
}

// ---------------------------------------------------------------------------
// routes
// ---------------------------------------------------------------------------

pub fn public_routes() -> Router<AppState> {
    Router::new().route("/auth/email/send-code", post(send_code))
}
