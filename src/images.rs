use std::time::Duration;

use axum::{
    Extension, Json, Router,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use rand::RngCore;
use serde::{Deserialize, Serialize};

use crate::{
    AppState, CurrentUser, channels, db, quota,
    net_guard,
    storage::{MediaKind, MediaStorage},
};

// ---------------------------------------------------------------------------
// upstream resolution (shared with sync + async paths)
// ---------------------------------------------------------------------------

fn trim_slash(s: &str) -> &str {
    s.trim_end_matches('/')
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ImageKind {
    Generate,
    Edit,
    Responses,
}

impl ImageKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Generate => "generate",
            Self::Edit => "edit",
            Self::Responses => "responses",
        }
    }
}

fn image_endpoint(host: &str, protocol: &str, model: &str, kind: ImageKind) -> String {
    let base = trim_slash(host);
    match (protocol, kind) {
        ("openai", ImageKind::Generate) => format!("{base}/v1/images/generations"),
        ("openai", ImageKind::Edit) => format!("{base}/v1/images/edits"),
        ("openai", ImageKind::Responses) => format!("{base}/v1/responses"),
        ("gemini", ImageKind::Generate) => format!("{base}/v1beta/models/{model}:predict"),
        ("gemini", ImageKind::Edit) => format!("{base}/v1beta/models/{model}:generateContent"),
        _ => format!("{base}/"),
    }
}

async fn resolve_image_upstream(
    state: &AppState,
    protocol: &str,
    kind: ImageKind,
    headers: &HeaderMap,
) -> Result<(String, String, bool, Option<String>, String), Response> {
    // BYOK when both upstream URL+key are supplied. No opt-out header needed.
    let hdr_url = headers.get("x-upstream-url").and_then(|v| v.to_str().ok());
    let hdr_key = headers.get("x-upstream-key").and_then(|v| v.to_str().ok());
    if let (Some(u), Some(k)) = (hdr_url, hdr_key) {
        if !u.is_empty() && !k.is_empty() {
            return Ok((u.to_string(), k.to_string(), false, None, String::new()));
        }
    }
    let installed = state.require_installed().await?;
    // Shared mode: model is client-chosen via X-Upstream-Model header.
    let client_model = headers
        .get("x-upstream-model")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or_default()
        .to_string();
    if client_model.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "请先在设置里选择模型（X-Upstream-Model 头缺失）",
        )
            .into_response());
    }
    // Pick highest-priority enabled channel that serves (model, "image").
    // Chain fallback is deferred to a follow-up task — image generation
    // already has retry/refund semantics that complicate mid-flight failover.
    let chain = channels::select_chain(&installed.pool, installed.kind, &client_model)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("channels query: {e}")).into_response())?;
    let choice = chain
        .into_iter()
        .find(|c| c.channel.protocol == protocol)
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                format!("模型 {client_model} 未绑定任何 {protocol} 图像渠道；请联系管理员在后台「渠道/模型定价」中添加"),
            )
                .into_response()
        })?;
    let upstream_model = if choice.upstream_model.is_empty() {
        client_model.clone()
    } else {
        choice.upstream_model.clone()
    };
    let url = image_endpoint(&choice.channel.base_url, protocol, &upstream_model, kind);
    Ok((url, choice.channel.api_key, true, Some(upstream_model), client_model))
}

fn override_json_model(body: &[u8], new_model: &str) -> Vec<u8> {
    let Ok(mut v) = serde_json::from_slice::<serde_json::Value>(body) else {
        return body.to_vec();
    };
    if let Some(map) = v.as_object_mut() {
        if map.contains_key("model") {
            map.insert("model".into(), serde_json::Value::String(new_model.into()));
        }
    }
    serde_json::to_vec(&v).unwrap_or_else(|_| body.to_vec())
}

async fn deduct_image_quota(
    state: &AppState,
    user_id: i64,
    protocol: &str,
    model: &str,
) -> Result<(), Response> {
    let installed = state.require_installed().await.map_err(|r| r)?;
    match channels::try_deduct_per_call(
        &installed.pool,
        installed.kind,
        user_id,
        model,
        "image",
        protocol,
        &format!("image_{protocol}"),
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(channels::DeductError::NotWhitelisted) => Err((
            StatusCode::FORBIDDEN,
            format!("模型 {model} 未启用：管理员尚未在「模型计费」中开放此图像模型"),
        )
            .into_response()),
        Err(channels::DeductError::Insufficient { balance, cost }) => Err((
            StatusCode::PAYMENT_REQUIRED,
            format!(
                "额度不足：当前剩余 {}，本次生图需要 {}；请在设置里填入自己的 API Key，或联系管理员充值",
                quota::format_quota(balance),
                quota::format_quota(cost)
            ),
        )
            .into_response()),
    }
}

async fn refund_image_quota(state: &AppState, user_id: i64, model: &str, reason: &str) {
    let Ok(installed) = state.require_installed().await else {
        return;
    };
    let cost = channels::per_call_quota(&installed.pool, installed.kind, model, "image")
        .await
        .unwrap_or(0);
    if cost > 0 {
        // Normalize the ledger reason to refund_image_<model>_<suffix>, mirroring
        // the chat-side convention. Callers historically pass "refund_<suffix>"
        // (e.g. refund_upstream_error); strip that prefix to avoid double "refund_".
        let suffix = reason.strip_prefix("refund_").unwrap_or(reason);
        let formatted = format!("refund_image_{model}_{suffix}");
        let _ = quota::grant(
            &installed.pool,
            installed.kind,
            user_id,
            cost,
            &formatted,
            &quota::LedgerMeta::refund_image(model),
        )
        .await;
    }
}

// ---------------------------------------------------------------------------
// response decoding → on-disk images
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone)]
pub struct GeneratedImages {
    pub images: Vec<GeneratedImage>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct GeneratedImage {
    pub path: String,
    pub revised_prompt: Option<String>,
}

fn err(s: StatusCode, m: impl Into<String>) -> Response {
    (s, m.into()).into_response()
}

fn random_hex(n: usize) -> String {
    let mut bytes = vec![0u8; n];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Fatal upstream / decoding error. Carries a user-facing message.
#[derive(Debug)]
struct JobError(String);

impl std::fmt::Display for JobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

async fn decode_openai_body(
    state: &AppState,
    raw: &[u8],
) -> Result<GeneratedImages, JobError> {
    let parsed: serde_json::Value =
        serde_json::from_slice(raw).map_err(|e| JobError(format!("parse: {e}")))?;

    let items = parsed
        .get("data")
        .and_then(|d| d.as_array())
        .cloned()
        .unwrap_or_default();

    let mut out = Vec::new();
    for item in items {
        let revised = item
            .get("revised_prompt")
            .and_then(|v| v.as_str())
            .map(String::from);

        let bytes: Vec<u8> = if let Some(b64) = item.get("b64_json").and_then(|v| v.as_str()) {
            STANDARD
                .decode(b64)
                .map_err(|e| JobError(format!("b64 decode: {e}")))?
        } else if let Some(remote) = item.get("url").and_then(|v| v.as_str()) {
            let (_parsed, host, addrs) = net_guard::validate_upstream_url(remote)
                .await
                .map_err(|r| JobError(format!("validate remote url: {}", response_to_text(r))))?;
            let client = net_guard::guarded_client_with_timeout(
                &host,
                &addrs,
                Duration::from_secs(120),
            )
            .map_err(|r| JobError(format!("client: {}", response_to_text(r))))?;
            let resp = client
                .get(remote)
                .send()
                .await
                .map_err(|e| JobError(format!("fetch image: {e}")))?;
            resp.bytes()
                .await
                .map_err(|e| JobError(format!("fetch image bytes: {e}")))?
                .to_vec()
        } else {
            continue;
        };

        let name = format!("{}.png", random_hex(16));
        state.storage.put(MediaKind::Image, &name, bytes)
            .await
            .map_err(|e| JobError(format!("write: {e}")))?;
        out.push(GeneratedImage {
            path: format!("/api/images/{name}"),
            revised_prompt: revised,
        });
    }

    if out.is_empty() {
        return Err(JobError("upstream returned no images".into()));
    }
    Ok(GeneratedImages { images: out })
}

async fn decode_gemini_body(
    state: &AppState,
    raw: &[u8],
) -> Result<GeneratedImages, JobError> {
    let parsed: serde_json::Value =
        serde_json::from_slice(raw).map_err(|e| JobError(format!("parse: {e}")))?;

    let mut b64_items: Vec<(String, Option<String>)> = Vec::new();
    if let Some(preds) = parsed.get("predictions").and_then(|d| d.as_array()) {
        for p in preds {
            let data = p
                .get("bytesBase64Encoded")
                .and_then(|v| v.as_str())
                .or_else(|| p.pointer("/image/imageBytes").and_then(|v| v.as_str()));
            if let Some(d) = data {
                let mime = p.get("mimeType").and_then(|v| v.as_str()).map(String::from);
                b64_items.push((d.to_string(), mime));
            }
        }
    } else if let Some(cands) = parsed.get("candidates").and_then(|d| d.as_array()) {
        for c in cands {
            let Some(parts) = c.pointer("/content/parts").and_then(|v| v.as_array()) else {
                continue;
            };
            for part in parts {
                let Some(inline) = part.get("inlineData").or_else(|| part.get("inline_data"))
                else {
                    continue;
                };
                let Some(data) = inline.get("data").and_then(|v| v.as_str()) else {
                    continue;
                };
                let mime = inline
                    .get("mimeType")
                    .or_else(|| inline.get("mime_type"))
                    .and_then(|v| v.as_str())
                    .map(String::from);
                b64_items.push((data.to_string(), mime));
            }
        }
    }

    let mut out = Vec::new();
    for (b64, mime) in b64_items {
        let bytes = STANDARD
            .decode(&b64)
            .map_err(|e| JobError(format!("b64 decode: {e}")))?;
        let ext = match mime.as_deref() {
            Some("image/jpeg") => "jpg",
            Some("image/webp") => "webp",
            _ => "png",
        };
        let name = format!("{}.{ext}", random_hex(16));
        state.storage.put(MediaKind::Image, &name, bytes)
            .await
            .map_err(|e| JobError(format!("write: {e}")))?;
        out.push(GeneratedImage {
            path: format!("/api/images/{name}"),
            revised_prompt: None,
        });
    }

    if out.is_empty() {
        return Err(JobError("upstream returned no images".into()));
    }
    Ok(GeneratedImages { images: out })
}

fn response_to_text(_r: Response) -> String {
    "upstream validation failed".into()
}

// ---------------------------------------------------------------------------
// core upstream calls (plain async fns; usable from sync handler or task)
// ---------------------------------------------------------------------------

struct OpenAiJsonReq {
    url: String,
    key: String,
    used_shared: bool,
    body: Vec<u8>,
}

async fn call_openai_json(
    state: &AppState,
    req: OpenAiJsonReq,
) -> Result<GeneratedImages, JobError> {
    let client = net_guard::client_for_upstream_with_timeout(
        &state.image_http,
        &req.url,
        req.used_shared,
        Duration::from_secs(600),
    )
    .await
    .map_err(|_| JobError("failed to build HTTP client".into()))?;

    let resp = client
        .post(&req.url)
        .bearer_auth(&req.key)
        .header(header::CONTENT_TYPE, "application/json")
        .body(req.body)
        .send()
        .await
        .map_err(|e| { eprintln!("[image] upstream connect failed: {e}"); JobError("上游服务暂时不可用".into()) })?;

    let status = resp.status();
    let raw = resp.bytes().await.unwrap_or_default();
    if !status.is_success() {
        // Don't surface the upstream body — shared-channel calls use the
        // admin's API key and providers often echo headers in error bodies.
        return Err(JobError(format!("上游服务返回 {status}")));
    }
    decode_openai_body(state, &raw).await
}

struct OpenAiMultipartReq {
    url: String,
    key: String,
    used_shared: bool,
    content_type: String,
    body: Vec<u8>,
}

async fn call_openai_multipart(
    state: &AppState,
    req: OpenAiMultipartReq,
) -> Result<GeneratedImages, JobError> {
    let client = net_guard::client_for_upstream_with_timeout(
        &state.image_http,
        &req.url,
        req.used_shared,
        Duration::from_secs(600),
    )
    .await
    .map_err(|_| JobError("failed to build HTTP client".into()))?;

    let resp = client
        .post(&req.url)
        .bearer_auth(&req.key)
        .header(header::CONTENT_TYPE, req.content_type)
        .body(req.body)
        .send()
        .await
        .map_err(|e| { eprintln!("[image] upstream connect failed: {e}"); JobError("上游服务暂时不可用".into()) })?;

    let status = resp.status();
    let raw = resp.bytes().await.unwrap_or_default();
    if !status.is_success() {
        // Don't surface the upstream body — shared-channel calls use the
        // admin's API key and providers often echo headers in error bodies.
        return Err(JobError(format!("上游服务返回 {status}")));
    }
    decode_openai_body(state, &raw).await
}

struct GeminiJsonReq {
    url: String,
    key: String,
    used_shared: bool,
    body: Vec<u8>,
}

async fn call_gemini_json(
    state: &AppState,
    req: GeminiJsonReq,
) -> Result<GeneratedImages, JobError> {
    let client = net_guard::client_for_upstream_with_timeout(
        &state.image_http,
        &req.url,
        req.used_shared,
        Duration::from_secs(600),
    )
    .await
    .map_err(|_| JobError("failed to build HTTP client".into()))?;

    let resp = client
        .post(&req.url)
        .header("x-goog-api-key", req.key.as_str())
        .header(header::CONTENT_TYPE, "application/json")
        .body(req.body)
        .send()
        .await
        .map_err(|e| { eprintln!("[image] upstream connect failed: {e}"); JobError("上游服务暂时不可用".into()) })?;

    let status = resp.status();
    let raw = resp.bytes().await.unwrap_or_default();
    if !status.is_success() {
        // Don't surface the upstream body — shared-channel calls use the
        // admin's API key and providers often echo headers in error bodies.
        return Err(JobError(format!("上游服务返回 {status}")));
    }
    decode_gemini_body(state, &raw).await
}

// ---------------------------------------------------------------------------
// legacy sync proxy endpoints (kept for backwards compatibility)
// ---------------------------------------------------------------------------

async fn proxy_openai_images(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let (url, key, used_shared, shared_model, _client_model) =
        match resolve_image_upstream(&state, "openai", ImageKind::Generate, &headers).await {
            Ok(v) => v,
            Err(r) => return r,
        };
    let body = match (used_shared, shared_model.as_deref()) {
        (true, Some(m)) if !m.is_empty() => override_json_model(&body, m),
        _ => body.to_vec(),
    };
    if used_shared {
        if let Err(r) = deduct_image_quota(&state, user.id, "openai", &_client_model).await {
            return r;
        }
    }
    match call_openai_json(&state, OpenAiJsonReq { url, key, used_shared, body }).await {
        Ok(r) => Json(r).into_response(),
        Err(e) => {
            if used_shared {
                refund_image_quota(&state, user.id, &_client_model, "refund_upstream_error").await;
            }
            err(StatusCode::BAD_GATEWAY, e.0)
        }
    }
}

async fn proxy_openai_images_edits(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let (url, key, used_shared, _shared_model, _client_model) =
        match resolve_image_upstream(&state, "openai", ImageKind::Edit, &headers).await {
            Ok(v) => v,
            Err(r) => return r,
        };
    let content_type = match headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
    {
        Some(v) if v.starts_with("multipart/form-data") => v.to_string(),
        _ => return err(StatusCode::BAD_REQUEST, "expected multipart/form-data body"),
    };
    if used_shared {
        if let Err(r) = deduct_image_quota(&state, user.id, "openai", &_client_model).await {
            return r;
        }
    }
    match call_openai_multipart(
        &state,
        OpenAiMultipartReq {
            url,
            key,
            used_shared,
            content_type,
            body: body.to_vec(),
        },
    )
    .await
    {
        Ok(r) => Json(r).into_response(),
        Err(e) => {
            if used_shared {
                refund_image_quota(&state, user.id, &_client_model, "refund_upstream_error").await;
            }
            err(StatusCode::BAD_GATEWAY, e.0)
        }
    }
}

async fn proxy_gemini_images(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let kind = headers
        .get("x-upstream-url")
        .and_then(|v| v.to_str().ok())
        .filter(|u| u.contains(":generateContent"))
        .map(|_| ImageKind::Edit)
        .unwrap_or(ImageKind::Generate);
    let (url, key, used_shared, _shared_model, _client_model) =
        match resolve_image_upstream(&state, "gemini", kind, &headers).await {
            Ok(v) => v,
            Err(r) => return r,
        };
    if used_shared {
        if let Err(r) = deduct_image_quota(&state, user.id, "gemini", &_client_model).await {
            return r;
        }
    }
    match call_gemini_json(&state, GeminiJsonReq { url, key, used_shared, body: body.to_vec() }).await {
        Ok(r) => Json(r).into_response(),
        Err(e) => {
            if used_shared {
                refund_image_quota(&state, user.id, &_client_model, "refund_upstream_error").await;
            }
            err(StatusCode::BAD_GATEWAY, e.0)
        }
    }
}

// ---------------------------------------------------------------------------
// async job endpoints
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct JobCreated {
    token: String,
}

#[derive(Serialize)]
struct JobStatus {
    token: String,
    status: String,
    protocol: String,
    kind: String,
    error: Option<String>,
    images: Option<Vec<GeneratedImage>>,
    created_at: String,
    finished_at: Option<String>,
}

fn new_token() -> String {
    random_hex(16)
}

async fn insert_job(
    state: &AppState,
    user_id: i64,
    protocol: &str,
    kind: ImageKind,
    used_shared: bool,
) -> Result<String, Response> {
    let installed = state.require_installed().await?;
    let token = new_token();
    let sql = db::q(
        installed.kind,
        "INSERT INTO image_jobs (token, user_id, protocol, kind, used_shared, status)
         VALUES (?, ?, ?, ?, ?, 'pending')",
    );
    sqlx::query(&sql)
        .bind(&token)
        .bind(user_id)
        .bind(protocol)
        .bind(kind.as_str())
        .bind(if used_shared { 1_i64 } else { 0 })
        .execute(&installed.pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("db: {e}")))?;
    Ok(token)
}

async fn mark_running(state: &AppState, token: &str) {
    let Ok(installed) = state.require_installed().await else {
        return;
    };
    let now = db::now_expr(installed.kind);
    let sql = db::q(
        installed.kind,
        &format!(
            "UPDATE image_jobs SET status = 'running', started_at = {now} WHERE token = ?"
        ),
    );
    let _ = sqlx::query(&sql).bind(token).execute(&installed.pool).await;
}

async fn mark_done(state: &AppState, token: &str, result: &GeneratedImages) {
    let Ok(installed) = state.require_installed().await else {
        return;
    };
    let now = db::now_expr(installed.kind);
    let json = serde_json::to_string(result).unwrap_or_default();
    let sql = db::q(
        installed.kind,
        &format!(
            "UPDATE image_jobs SET status = 'done', result_json = ?, finished_at = {now}
             WHERE token = ?"
        ),
    );
    let _ = sqlx::query(&sql)
        .bind(&json)
        .bind(token)
        .execute(&installed.pool)
        .await;
}

async fn mark_failed(state: &AppState, token: &str, message: &str) {
    let Ok(installed) = state.require_installed().await else {
        return;
    };
    let now = db::now_expr(installed.kind);
    let trimmed: String = message.chars().take(2000).collect();
    let sql = db::q(
        installed.kind,
        &format!(
            "UPDATE image_jobs SET status = 'failed', error = ?, finished_at = {now}
             WHERE token = ?"
        ),
    );
    let _ = sqlx::query(&sql)
        .bind(&trimmed)
        .bind(token)
        .execute(&installed.pool)
        .await;
}

async fn start_openai_generate_job(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let (url, key, used_shared, shared_model, _client_model) =
        match resolve_image_upstream(&state, "openai", ImageKind::Generate, &headers).await {
            Ok(v) => v,
            Err(r) => return r,
        };
    let body = match (used_shared, shared_model.as_deref()) {
        (true, Some(m)) if !m.is_empty() => override_json_model(&body, m),
        _ => body.to_vec(),
    };
    if used_shared {
        if let Err(r) = deduct_image_quota(&state, user.id, "openai", &_client_model).await {
            return r;
        }
    }
    let token = match insert_job(&state, user.id, "openai", ImageKind::Generate, used_shared).await {
        Ok(t) => t,
        Err(r) => {
            if used_shared {
                refund_image_quota(&state, user.id, &_client_model, "refund_job_create_error").await;
            }
            return r;
        }
    };

    let state_c = state.clone();
    let token_c = token.clone();
    tokio::spawn(async move {
        mark_running(&state_c, &token_c).await;
        let res = call_openai_json(
            &state_c,
            OpenAiJsonReq { url, key, used_shared, body },
        )
        .await;
        match res {
            Ok(r) => mark_done(&state_c, &token_c, &r).await,
            Err(e) => {
                if used_shared {
                    refund_image_quota(&state_c, user.id, &_client_model, "refund_upstream_error").await;
                }
                mark_failed(&state_c, &token_c, &e.0).await;
            }
        }
    });

    Json(JobCreated { token }).into_response()
}

async fn start_openai_edit_job(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let (url, key, used_shared, _shared_model, _client_model) =
        match resolve_image_upstream(&state, "openai", ImageKind::Edit, &headers).await {
            Ok(v) => v,
            Err(r) => return r,
        };
    let content_type = match headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
    {
        Some(v) if v.starts_with("multipart/form-data") => v.to_string(),
        _ => return err(StatusCode::BAD_REQUEST, "expected multipart/form-data body"),
    };
    if used_shared {
        if let Err(r) = deduct_image_quota(&state, user.id, "openai", &_client_model).await {
            return r;
        }
    }
    let token = match insert_job(&state, user.id, "openai", ImageKind::Edit, used_shared).await {
        Ok(t) => t,
        Err(r) => {
            if used_shared {
                refund_image_quota(&state, user.id, &_client_model, "refund_job_create_error").await;
            }
            return r;
        }
    };

    let state_c = state.clone();
    let token_c = token.clone();
    let body_vec = body.to_vec();
    tokio::spawn(async move {
        mark_running(&state_c, &token_c).await;
        let res = call_openai_multipart(
            &state_c,
            OpenAiMultipartReq {
                url,
                key,
                used_shared,
                content_type,
                body: body_vec,
            },
        )
        .await;
        match res {
            Ok(r) => mark_done(&state_c, &token_c, &r).await,
            Err(e) => {
                if used_shared {
                    refund_image_quota(&state_c, user.id, &_client_model, "refund_upstream_error").await;
                }
                mark_failed(&state_c, &token_c, &e.0).await;
            }
        }
    });

    Json(JobCreated { token }).into_response()
}

async fn start_gemini_job(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let kind = headers
        .get("x-upstream-url")
        .and_then(|v| v.to_str().ok())
        .filter(|u| u.contains(":generateContent"))
        .map(|_| ImageKind::Edit)
        .unwrap_or(ImageKind::Generate);
    let (url, key, used_shared, _shared_model, _client_model) =
        match resolve_image_upstream(&state, "gemini", kind, &headers).await {
            Ok(v) => v,
            Err(r) => return r,
        };
    if used_shared {
        if let Err(r) = deduct_image_quota(&state, user.id, "gemini", &_client_model).await {
            return r;
        }
    }
    let token = match insert_job(&state, user.id, "gemini", kind, used_shared).await {
        Ok(t) => t,
        Err(r) => {
            if used_shared {
                refund_image_quota(&state, user.id, &_client_model, "refund_job_create_error").await;
            }
            return r;
        }
    };

    let state_c = state.clone();
    let token_c = token.clone();
    let body_vec = body.to_vec();
    tokio::spawn(async move {
        mark_running(&state_c, &token_c).await;
        let res = call_gemini_json(
            &state_c,
            GeminiJsonReq { url, key, used_shared, body: body_vec },
        )
        .await;
        match res {
            Ok(r) => mark_done(&state_c, &token_c, &r).await,
            Err(e) => {
                if used_shared {
                    refund_image_quota(&state_c, user.id, &_client_model, "refund_upstream_error").await;
                }
                mark_failed(&state_c, &token_c, &e.0).await;
            }
        }
    });

    Json(JobCreated { token }).into_response()
}

async fn get_job(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(token): Path<String>,
) -> Response {
    let Ok(installed) = state.require_installed().await else {
        return err(StatusCode::SERVICE_UNAVAILABLE, "not installed");
    };
    let sql = db::q(
        installed.kind,
        "SELECT token, protocol, kind, status, result_json, error, created_at, finished_at
         FROM image_jobs WHERE token = ? AND user_id = ?",
    );
    let row: Option<(
        String,
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        String,
        Option<String>,
    )> = sqlx::query_as(&sql)
        .bind(&token)
        .bind(user.id)
        .fetch_optional(&installed.pool)
        .await
        .ok()
        .flatten();
    let Some((token, protocol, kind, status, result_json, error, created_at, finished_at)) = row
    else {
        return err(StatusCode::NOT_FOUND, "job not found");
    };
    let images = result_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<GeneratedImages>(s).ok())
        .map(|r| r.images);
    Json(JobStatus {
        token,
        status,
        protocol,
        kind,
        error,
        images,
        created_at,
        finished_at,
    })
    .into_response()
}

// ---------------------------------------------------------------------------
// Responses API (multi-turn image generation for OpenAI-compatible upstreams)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ResponsesHistoryTurn {
    role: String,
    #[serde(default)]
    text: Option<String>,
    /// Image paths like "/api/images/xxx.png" to re-submit as input_image.
    #[serde(default)]
    images: Vec<String>,
}

#[derive(Deserialize)]
struct ResponsesJobReq {
    history: Vec<ResponsesHistoryTurn>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    quality: Option<String>,
}

/// Load a previously-stored image, return as data: URL.
async fn stored_image_to_data_url(storage: &MediaStorage, web_path: &str) -> Option<String> {
    // Only /api/images/xxx.png paths are valid — we store them ourselves.
    let name = web_path.rsplit('/').next()?;
    if name.is_empty() || name.contains("..") || name.contains('\\') {
        return None;
    }
    let bytes = storage.get(MediaKind::Image, name).await.ok()?;
    let mime = match std::path::Path::new(name).extension().and_then(|e| e.to_str()) {
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        _ => "image/png",
    };
    Some(format!("data:{mime};base64,{}", STANDARD.encode(&bytes)))
}

async fn build_responses_input(
    storage: &MediaStorage,
    history: &[ResponsesHistoryTurn],
) -> Vec<serde_json::Value> {
    use serde_json::json;
    let mut input = Vec::new();
    for t in history {
        let mut parts: Vec<serde_json::Value> = Vec::new();
        if let Some(text) = t.text.as_deref() {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                let ty = if t.role == "assistant" { "output_text" } else { "input_text" };
                parts.push(json!({ "type": ty, "text": trimmed }));
            }
        }
        for p in &t.images {
            let durl = if p.starts_with("data:") {
                // Inline attachment — pass through as-is.
                Some(p.clone())
            } else {
                stored_image_to_data_url(storage, p).await
            };
            if let Some(d) = durl {
                parts.push(json!({
                    "type": "input_image",
                    "image_url": d,
                }));
            }
        }
        if parts.is_empty() {
            continue;
        }
        input.push(json!({
            "role": if t.role == "assistant" { "assistant" } else { "user" },
            "content": parts,
        }));
    }
    input
}

/// Extract image bytes (base64) from a Responses API output item.
fn extract_responses_image_b64(item: &serde_json::Value) -> Option<String> {
    // gpt-image-1 style: top-level "result"
    if let Some(s) = item.get("result").and_then(|v| v.as_str()) {
        return Some(s.to_string());
    }
    // Alternative shapes seen in community relays.
    if let Some(s) = item.pointer("/image/b64_json").and_then(|v| v.as_str()) {
        return Some(s.to_string());
    }
    if let Some(s) = item.pointer("/output/b64_json").and_then(|v| v.as_str()) {
        return Some(s.to_string());
    }
    None
}

fn extract_responses_text(item: &serde_json::Value) -> Option<String> {
    // message.content[] entries with text
    let content = item.get("content")?.as_array()?;
    let mut out = String::new();
    for part in content {
        if let Some(t) = part.get("text").and_then(|v| v.as_str()) {
            out.push_str(t);
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

async fn start_openai_responses_job(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    headers: HeaderMap,
    Json(body): Json<ResponsesJobReq>,
) -> Response {
    use serde_json::json;

    if body.history.is_empty() {
        return err(StatusCode::BAD_REQUEST, "history 不能为空");
    }

    let (url, key, used_shared, shared_model, _client_model) =
        match resolve_image_upstream(
        &state,
        "openai",
        ImageKind::Responses,
        &headers,
    )
    .await
    {
        Ok(v) => v,
        Err(r) => return r,
    };

    let model = body
        .model
        .clone()
        .filter(|s| !s.is_empty())
        .or(shared_model)
        .unwrap_or_else(|| "gpt-image-1".into());

    if used_shared {
        if let Err(r) = deduct_image_quota(&state, user.id, "openai", &_client_model).await {
            return r;
        }
    }

    let token = match insert_job(&state, user.id, "openai-responses", ImageKind::Responses, used_shared).await {
        Ok(t) => t,
        Err(r) => {
            if used_shared {
                refund_image_quota(&state, user.id, &_client_model, "refund_job_create_error").await;
            }
            return r;
        }
    };

    let state_c = state.clone();
    let token_c = token.clone();
    let size = body.size.clone();
    let quality = body.quality.clone();
    let history = body.history;

    tokio::spawn(async move {
        mark_running(&state_c, &token_c).await;

        let input = build_responses_input(&state_c.storage, &history).await;
        if input.is_empty() {
            if used_shared {
                refund_image_quota(&state_c, user.id, &_client_model, "refund_upstream_error").await;
            }
            mark_failed(&state_c, &token_c, "history 展开后为空").await;
            return;
        }

        // OpenAI's built-in `image_generation` hosted tool. Size/quality are
        // accepted on gpt-image-1 per OpenAI docs; relay behavior varies.
        let mut tool = serde_json::Map::new();
        tool.insert("type".into(), json!("image_generation"));
        if let Some(s) = size.as_deref() {
            if !s.is_empty() && s != "auto" {
                tool.insert("size".into(), json!(s));
            }
        }
        if let Some(q) = quality.as_deref() {
            if !q.is_empty() && q != "auto" {
                tool.insert("quality".into(), json!(q));
            }
        }

        let payload = json!({
            "model": model,
            "input": input,
            "tools": [ tool ],
        });

        let client = match net_guard::client_for_upstream_with_timeout(
            &state_c.image_http,
            &url,
            used_shared,
            Duration::from_secs(600),
        )
        .await
        {
            Ok(c) => c,
            Err(_) => {
                if used_shared {
                    refund_image_quota(&state_c, user.id, &_client_model, "refund_upstream_error").await;
                }
                mark_failed(&state_c, &token_c, "HTTP client 构建失败").await;
                return;
            }
        };

        let resp = match client
            .post(&url)
            .bearer_auth(&key)
            .header(header::CONTENT_TYPE, "application/json")
            .json(&payload)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                if used_shared {
                    refund_image_quota(&state_c, user.id, &_client_model, "refund_upstream_error").await;
                }
                mark_failed(&state_c, &token_c, &format!("upstream: {e}")).await;
                return;
            }
        };

        let status = resp.status();
        let raw = resp.bytes().await.unwrap_or_default();
        if !status.is_success() {
            if used_shared {
                refund_image_quota(&state_c, user.id, &_client_model, "refund_upstream_error").await;
            }
            mark_failed(
                &state_c,
                &token_c,
                &format!("上游服务返回 {status}"),
            )
            .await;
            return;
        }

        let parsed: serde_json::Value = match serde_json::from_slice(&raw) {
            Ok(v) => v,
            Err(e) => {
                if used_shared {
                    refund_image_quota(&state_c, user.id, &_client_model, "refund_upstream_error").await;
                }
                mark_failed(&state_c, &token_c, &format!("parse: {e}")).await;
                return;
            }
        };

        // Walk output[] — collect images + assistant text.
        let mut out_images: Vec<GeneratedImage> = Vec::new();
        let mut text_out = String::new();

        if let Some(items) = parsed.get("output").and_then(|v| v.as_array()) {
            for it in items {
                let ty = it.get("type").and_then(|v| v.as_str()).unwrap_or_default();
                match ty {
                    "image_generation_call" => {
                        let Some(b64) = extract_responses_image_b64(it) else {
                            continue;
                        };
                        let bytes = match STANDARD.decode(&b64) {
                            Ok(b) => b,
                            Err(_) => continue,
                        };
                        let name = format!("{}.png", random_hex(16));
                        if state_c.storage.put(MediaKind::Image, &name, bytes).await.is_err() {
                            continue;
                        }
                        out_images.push(GeneratedImage {
                            path: format!("/api/images/{name}"),
                            revised_prompt: it
                                .get("revised_prompt")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                        });
                    }
                    "message" => {
                        if let Some(t) = extract_responses_text(it) {
                            if !text_out.is_empty() {
                                text_out.push('\n');
                            }
                            text_out.push_str(&t);
                        }
                    }
                    _ => {}
                }
            }
        }
        // Fallback: some relays return `output_text` at top level.
        if text_out.is_empty() {
            if let Some(t) = parsed.get("output_text").and_then(|v| v.as_str()) {
                text_out.push_str(t);
            }
        }

        if out_images.is_empty() && text_out.is_empty() {
            if used_shared {
                refund_image_quota(&state_c, user.id, &_client_model, "refund_upstream_error").await;
            }
            mark_failed(&state_c, &token_c, "上游未返回图片或文本").await;
            return;
        }

        // Prepend any assistant text to the first image's revised_prompt so the
        // UI can surface it as a caption (frontend renders revised_prompt).
        if !text_out.is_empty() && !out_images.is_empty() {
            let prior = out_images[0].revised_prompt.clone().unwrap_or_default();
            out_images[0].revised_prompt = Some(if prior.is_empty() {
                text_out.clone()
            } else {
                format!("{prior}\n{text_out}")
            });
        }

        // If only text came back (no image), surface a synthetic entry so the
        // caller sees something to display.
        if out_images.is_empty() {
            out_images.push(GeneratedImage {
                path: String::new(),
                revised_prompt: Some(text_out),
            });
        }

        mark_done(&state_c, &token_c, &GeneratedImages { images: out_images }).await;
    });

    Json(JobCreated { token }).into_response()
}

/// On startup, mark any still-running jobs as failed. Invoked from main
/// after the database is ready.
pub async fn cleanup_stale_jobs(pool: &db::Pool, kind: db::DbKind) {
    let now = db::now_expr(kind);
    let sql = db::q(
        kind,
        &format!(
            "UPDATE image_jobs SET status = 'failed', error = 'server restarted', finished_at = {now}
             WHERE status IN ('pending', 'running')"
        ),
    );
    let _ = sqlx::query(&sql).execute(pool).await;
}

// ---------------------------------------------------------------------------
// persist a base64 image from the main chat stream (hosted image_generation
// tool returns the bytes inline — the browser uploads them here so we can
// reference the image by /api/images/<name> just like every other stored
// image in the app).
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SaveImageReq {
    b64: String,
    #[serde(default)]
    mime: Option<String>,
}

#[derive(Serialize)]
struct SaveImageResp {
    path: String,
}

async fn save_b64_image(
    State(state): State<AppState>,
    Extension(_user): Extension<CurrentUser>,
    Json(req): Json<SaveImageReq>,
) -> Response {
    let bytes = match STANDARD.decode(req.b64.as_bytes()) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("b64 decode: {e}")),
    };
    let ext = match req.mime.as_deref() {
        Some("image/jpeg") => "jpg",
        Some("image/webp") => "webp",
        Some("image/gif") => "gif",
        _ => "png",
    };
    let name = format!("{}.{ext}", random_hex(16));
    if let Err(e) = state.storage.put(MediaKind::Image, &name, bytes).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, format!("write: {e}"));
    }
    Json(SaveImageResp {
        path: format!("/api/images/{name}"),
    })
    .into_response()
}

// ---------------------------------------------------------------------------
// persist a base64 document (PDF / text / code) uploaded as a chat attachment.
// Stored under data_dir/files and referenced by /api/files/<name>. The chat
// layer fetches the bytes back and sends them as a native document block for
// the active protocol (OpenAI input_file / Claude document / Gemini inline).
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SaveFileReq {
    b64: String,
    #[serde(default)]
    mime: Option<String>,
    #[serde(default)]
    filename: Option<String>,
}

#[derive(Serialize)]
struct SaveFileResp {
    path: String,
}

fn ext_for_file(filename: Option<&str>, mime: Option<&str>) -> String {
    // Prefer the uploaded filename's extension (sanitized), else map from mime.
    if let Some(name) = filename {
        if let Some(dot) = name.rfind('.') {
            let raw = &name[dot + 1..];
            let clean: String = raw
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .take(8)
                .collect::<String>()
                .to_ascii_lowercase();
            if !clean.is_empty() {
                return clean;
            }
        }
    }
    match mime {
        Some("application/pdf") => "pdf",
        Some("text/markdown") => "md",
        Some("text/csv") => "csv",
        Some("application/json") => "json",
        Some(m) if m.starts_with("text/") => "txt",
        _ => "bin",
    }
    .to_string()
}

async fn save_b64_file(
    State(state): State<AppState>,
    Extension(_user): Extension<CurrentUser>,
    Json(req): Json<SaveFileReq>,
) -> Response {
    let bytes = match STANDARD.decode(req.b64.as_bytes()) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("b64 decode: {e}")),
    };
    let ext = ext_for_file(req.filename.as_deref(), req.mime.as_deref());
    let files_dir = state.data_dir.join("files");
    if let Err(e) = tokio::fs::create_dir_all(&files_dir).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, format!("mkdir: {e}"));
    }
    let name = format!("{}.{ext}", random_hex(16));
    let path = files_dir.join(&name);
    if let Err(e) = tokio::fs::write(&path, &bytes).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, format!("write: {e}"));
    }
    Json(SaveFileResp {
        path: format!("/api/files/{name}"),
    })
    .into_response()
}

/// Content-Type for a stored attachment. PDFs and known text/code extensions
/// are pinned explicitly because `mime_guess` mislabels several code suffixes
/// (e.g. `.ts` → video/mp2t), which would otherwise make the chat layer treat
/// a source file as binary and drop it.
fn file_content_type(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "json" => "application/json",
        "csv" => "text/csv; charset=utf-8",
        "md" | "markdown" | "txt" | "log" | "text" | "xml" | "yaml" | "yml" | "html"
        | "htm" | "css" | "ts" | "tsx" | "js" | "jsx" | "py" | "rs" | "go" | "java"
        | "c" | "h" | "cpp" | "hpp" | "cc" | "sh" | "rb" | "php" | "sql" | "toml"
        | "ini" | "conf" | "env" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

async fn serve_file(State(state): State<AppState>, Path(name): Path<String>) -> Response {
    if name.contains("..") || name.contains('/') || name.contains('\\') {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let path = state.data_dir.join("files").join(&name);
    let bytes = match tokio::fs::read(&path).await {
        Ok(b) => b,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    Response::builder()
        .header(header::CONTENT_TYPE, file_content_type(&name))
        .header(header::CACHE_CONTROL, "private, max-age=86400, immutable")
        .body(Body::from(bytes))
        .unwrap()
}

// ---------------------------------------------------------------------------
// serve stored image
// ---------------------------------------------------------------------------

async fn serve_image(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Response {
    if name.contains("..") || name.contains('/') || name.contains('\\') {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let bytes = match state.storage.get(MediaKind::Image, &name).await {
        Ok(b) => b,
        Err(e) if e.is_not_found() => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            eprintln!("[storage] serve image {name}: {e}");
            return StatusCode::BAD_GATEWAY.into_response();
        }
    };
    let mime = mime_guess::from_path(&name).first_or_octet_stream();
    Response::builder()
        .header(header::CONTENT_TYPE, mime.as_ref())
        .header(header::CACHE_CONTROL, "private, max-age=86400, immutable")
        .body(Body::from(bytes))
        .unwrap()
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/proxy/openai/images", post(proxy_openai_images))
        .route("/proxy/openai/images/edits", post(proxy_openai_images_edits))
        .route("/proxy/gemini/images", post(proxy_gemini_images))
        .route("/images/jobs/openai", post(start_openai_generate_job))
        .route("/images/jobs/openai/edits", post(start_openai_edit_job))
        .route("/images/jobs/openai/responses", post(start_openai_responses_job))
        .route("/images/jobs/gemini", post(start_gemini_job))
        .route("/images/save", post(save_b64_image))
        .route("/files/save", post(save_b64_file))
        .route("/images/jobs/{token}", get(get_job))
}

/// `/api/images/{name}` is split out into a public route — generated images
/// are referenced by markdown in shared snapshots, and the random 16-byte hex
/// filename is the only handle. Auth never gated per-user image access (any
/// logged-in user could fetch any image by URL), so exposing it publicly does
/// not relax an existing isolation guarantee.
pub fn public_routes() -> Router<AppState> {
    Router::new()
        .route("/images/{name}", get(serve_image))
        .route("/files/{name}", get(serve_file))
}
