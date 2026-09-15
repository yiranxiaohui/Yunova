use std::time::Duration;

use axum::{
    Extension, Json, Router,
    body::to_bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    AppState, CurrentUser, InstalledState, channels, db, quota,
    net_guard,
    storage::{MediaKind, MediaStorage, StorageError},
};

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn err(s: StatusCode, m: impl Into<String>) -> Response {
    (s, m.into()).into_response()
}

fn random_hex(n: usize) -> String {
    let mut bytes = vec![0u8; n];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn read_header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// Parse a data: URL into raw bytes + best-guess extension.
fn parse_data_url(s: &str) -> Option<(Vec<u8>, &'static str, &'static str)> {
    let rest = s.strip_prefix("data:")?;
    let (meta, b64) = rest.split_once(";base64,")?;
    let (ext, mime) = if meta.contains("jpeg") || meta.contains("jpg") {
        ("jpg", "image/jpeg")
    } else if meta.contains("webp") {
        ("webp", "image/webp")
    } else {
        ("png", "image/png")
    };
    let bytes = STANDARD.decode(b64.trim()).ok()?;
    Some((bytes, ext, mime))
}

async fn write_image(
    storage: &MediaStorage,
    bytes: &[u8],
    ext: &str,
) -> Result<String, StorageError> {
    let name = format!("{}.{}", random_hex(16), ext);
    storage.put(MediaKind::Image, &name, bytes.to_vec()).await?;
    Ok(format!("/api/images/{name}"))
}

// ---------------------------------------------------------------------------
// upstream resolution
// ---------------------------------------------------------------------------

async fn resolve_openai_image_upstream(
    state: &AppState,
    headers: &HeaderMap,
    edit: bool,
    model: &str,
) -> Result<(String, String, bool, Option<String>), Response> {
    // BYOK when both upstream URL+key are supplied. No opt-out header needed.
    let hdr_url = read_header(headers, "x-upstream-url");
    let hdr_key = read_header(headers, "x-upstream-key");
    if let (Some(u), Some(k)) = (hdr_url, hdr_key) {
        if !u.is_empty() && !k.is_empty() {
            let base = u.trim_end_matches('/');
            let path = if edit { "/v1/images/edits" } else { "/v1/images/generations" };
            return Ok((format!("{base}{path}"), k.to_string(), false, None));
        }
    }
    let installed = state.require_installed().await?;
    // Pick highest-priority enabled OpenAI image channel whose catalog still
    // lists `model`, matching what the model picker offers.
    let choice = channels::select_chain_by_advertised_model(
        &state.http,
        &installed.pool,
        installed.kind,
        model,
    )
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("channels query: {e}")))?
    .into_iter()
    .find(|c| c.channel.protocol == "openai")
    .ok_or_else(|| {
        err(
            StatusCode::BAD_REQUEST,
            format!("模型 {model} 当前没有可用的 OpenAI 图像渠道；请联系管理员在后台「渠道/模型定价」中检查"),
        )
    })?;
    let base = choice.channel.base_url.trim_end_matches('/');
    let path = if edit { "/v1/images/edits" } else { "/v1/images/generations" };
    let upstream_model = if choice.upstream_model.is_empty() {
        None
    } else {
        Some(choice.upstream_model.clone())
    };
    Ok((format!("{base}{path}"), choice.channel.api_key, true, upstream_model))
}

// ---------------------------------------------------------------------------
// generation row
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone)]
struct Generation {
    id: i64,
    token: String,
    status: String,
    error: Option<String>,
    prompt: String,
    revised_prompt: Option<String>,
    model: Option<String>,
    size: Option<String>,
    quality: Option<String>,
    style: Option<String>,
    n: i64,
    image_path: Option<String>,
    source_path: Option<String>,
    /// All attached base images (data URLs were persisted to disk). The first
    /// element mirrors `source_path` for backward compatibility.
    source_paths: Vec<String>,
    used_shared: bool,
    created_at: String,
    finished_at: Option<String>,
    negative_prompt: Option<String>,
    seed: Option<i64>,
    background: Option<String>,
}

// Named struct (rather than a tuple) — sqlx's tuple `FromRow` impls only go
// up to ~16 elements, and we have 20 columns now.
#[derive(sqlx::FromRow)]
struct GenRow {
    id: i64,
    token: String,
    status: String,
    error: Option<String>,
    prompt: String,
    revised_prompt: Option<String>,
    model: Option<String>,
    size: Option<String>,
    quality: Option<String>,
    style: Option<String>,
    n: i64,
    image_path: Option<String>,
    source_path: Option<String>,
    source_paths: Option<String>,
    used_shared: i64,
    created_at: String,
    finished_at: Option<String>,
    negative_prompt: Option<String>,
    seed: Option<i64>,
    background: Option<String>,
}

const GEN_COLS: &str =
    "id, token, status, error, prompt, revised_prompt, model, size, quality, style,
     n, image_path, source_path, source_paths, used_shared, created_at, finished_at,
     negative_prompt, seed, background";

fn gen_from_row(r: GenRow) -> Generation {
    Generation {
        id: r.id,
        token: r.token,
        status: r.status,
        error: r.error,
        prompt: r.prompt,
        revised_prompt: r.revised_prompt,
        model: r.model,
        size: r.size,
        quality: r.quality,
        style: r.style,
        n: r.n,
        image_path: r.image_path,
        source_paths: {
            // Prefer the JSON array column; fall back to the single legacy
            // source_path for rows written before multi-base support.
            let parsed = r
                .source_paths
                .as_deref()
                .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
                .unwrap_or_default();
            if parsed.is_empty() {
                r.source_path.clone().into_iter().collect()
            } else {
                parsed
            }
        },
        source_path: r.source_path,
        used_shared: r.used_shared != 0,
        created_at: r.created_at,
        finished_at: r.finished_at,
        negative_prompt: r.negative_prompt,
        seed: r.seed,
        background: r.background,
    }
}

// ---------------------------------------------------------------------------
// params + submit
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct GenerateReq {
    prompt: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    quality: Option<String>,
    #[serde(default)]
    style: Option<String>,
    /// Optional attached source image (data: URL) — switches to /v1/images/edits.
    /// Kept for backward compatibility; `image_data_urls` is preferred.
    #[serde(default)]
    image_data_url: Option<String>,
    /// Multiple attached source images (data: URLs). All are forwarded to
    /// /v1/images/edits as `image[]` parts. Takes precedence over the single
    /// `image_data_url` field when non-empty.
    #[serde(default)]
    image_data_urls: Option<Vec<String>>,
    /// Number of images to ask the upstream for (1-10). Storage currently
    /// keeps only the first image; the DB still records the requested `n`.
    #[serde(default)]
    n: Option<i32>,
    /// Negative prompt — forwarded to providers that accept it (Gemini Imagen,
    /// SD-compatible relays). OpenAI ignores it; harmless on those upstreams.
    #[serde(default)]
    negative_prompt: Option<String>,
    /// Random seed for reproducibility — forwarded to providers that accept it.
    #[serde(default)]
    seed: Option<i64>,
    /// gpt-image-1 specific: `transparent` / `opaque` / `auto`. Set
    /// `transparent` to get PNGs with alpha channel.
    #[serde(default)]
    background: Option<String>,
}

#[derive(Serialize)]
struct GenerateCreated {
    token: String,
}

/// Platform-backed image request used by the persistent workflow executor.
/// Workflow definitions intentionally never persist BYOK credentials.
pub(crate) struct WorkflowImageRequest {
    pub prompt: String,
    pub model: String,
    pub size: Option<String>,
    pub quality: Option<String>,
    pub style: Option<String>,
    pub image_data_urls: Vec<String>,
}

pub(crate) struct WorkflowImageState {
    pub status: String,
    pub output_path: Option<String>,
    pub error: Option<String>,
}

pub(crate) async fn start_workflow_generation(
    state: AppState,
    installed: InstalledState,
    user_id: i64,
    request: WorkflowImageRequest,
) -> Result<String, String> {
    let response = submit_generate(
        State(state),
        Extension(installed),
        Extension(CurrentUser { id: user_id }),
        HeaderMap::new(),
        Json(GenerateReq {
            prompt: request.prompt,
            model: Some(request.model),
            size: request.size,
            quality: request.quality,
            style: request.style,
            image_data_url: None,
            image_data_urls: (!request.image_data_urls.is_empty())
                .then_some(request.image_data_urls),
            n: Some(1),
            negative_prompt: None,
            seed: None,
            background: None,
        }),
    )
    .await;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .map_err(|e| format!("读取图片任务响应失败: {e}"))?;
    if !status.is_success() {
        return Err(String::from_utf8_lossy(&bytes).to_string());
    }
    serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|value| value.get("token").and_then(Value::as_str).map(str::to_string))
        .ok_or_else(|| "图片任务响应缺少 token".to_string())
}

pub(crate) async fn workflow_generation_state(
    pool: &db::Pool,
    kind: db::DbKind,
    user_id: i64,
    token: &str,
) -> Result<WorkflowImageState, String> {
    let sql = db::q(
        kind,
        "SELECT status, image_path, error FROM studio_generations WHERE token = ? AND user_id = ?",
    );
    let row: Option<(String, Option<String>, Option<String>)> = sqlx::query_as(&sql)
        .bind(token)
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())?;
    row.map(|(status, output_path, error)| WorkflowImageState {
        status,
        output_path,
        error,
    })
    .ok_or_else(|| "图片任务不存在".to_string())
}

async fn insert_pending(
    pool: &db::Pool,
    kind: db::DbKind,
    user_id: i64,
    token: &str,
    prompt: &str,
    model: Option<&str>,
    size: Option<&str>,
    quality: Option<&str>,
    style: Option<&str>,
    n: i64,
    negative_prompt: Option<&str>,
    seed: Option<i64>,
    background: Option<&str>,
    source_path: Option<&str>,
    source_paths: Option<&str>,
    used_shared: bool,
) -> Result<i64, sqlx::Error> {
    let ins = db::q(
        kind,
        "INSERT INTO studio_generations
           (token, user_id, status, prompt, model, size, quality, style,
            n, negative_prompt, seed, background, source_path, source_paths, used_shared)
         VALUES (?, ?, 'pending', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    );
    match kind {
        db::DbKind::Sqlite | db::DbKind::Postgres => {
            let row: (i64,) = sqlx::query_as(&format!("{ins} RETURNING id"))
                .bind(token)
                .bind(user_id)
                .bind(prompt)
                .bind(model)
                .bind(size)
                .bind(quality)
                .bind(style)
                .bind(n)
                .bind(negative_prompt)
                .bind(seed)
                .bind(background)
                .bind(source_path)
                .bind(source_paths)
                .bind(if used_shared { 1_i64 } else { 0 })
                .fetch_one(pool)
                .await?;
            Ok(row.0)
        }
        db::DbKind::Mysql => {
            let mut tx = pool.begin().await?;
            sqlx::query(&ins)
                .bind(token)
                .bind(user_id)
                .bind(prompt)
                .bind(model)
                .bind(size)
                .bind(quality)
                .bind(style)
                .bind(n)
                .bind(negative_prompt)
                .bind(seed)
                .bind(background)
                .bind(source_path)
                .bind(source_paths)
                .bind(if used_shared { 1_i64 } else { 0 })
                .execute(&mut *tx)
                .await?;
            let (id,): (i64,) =
                sqlx::query_as("SELECT LAST_INSERT_ID()").fetch_one(&mut *tx).await?;
            tx.commit().await?;
            Ok(id)
        }
    }
}

async fn finalize_done(
    pool: &db::Pool,
    kind: db::DbKind,
    token: &str,
    image_path: &str,
    revised_prompt: Option<&str>,
) {
    let now = db::now_expr(kind);
    let sql = db::q(
        kind,
        &format!(
            "UPDATE studio_generations
             SET status = 'done', image_path = ?, revised_prompt = ?, finished_at = {now}
             WHERE token = ?"
        ),
    );
    let _ = sqlx::query(&sql)
        .bind(image_path)
        .bind(revised_prompt)
        .bind(token)
        .execute(pool)
        .await;
}

async fn finalize_failed(pool: &db::Pool, kind: db::DbKind, token: &str, error: &str) {
    let now = db::now_expr(kind);
    let trimmed: String = error.chars().take(2000).collect();
    let sql = db::q(
        kind,
        &format!(
            "UPDATE studio_generations
             SET status = 'failed', error = ?, finished_at = {now}
             WHERE token = ?"
        ),
    );
    let _ = sqlx::query(&sql)
        .bind(&trimmed)
        .bind(token)
        .execute(pool)
        .await;
}

async fn submit_generate(
    State(state): State<AppState>,
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    headers: HeaderMap,
    Json(body): Json<GenerateReq>,
) -> Response {
    let prompt = body.prompt.trim().to_string();
    if prompt.is_empty() {
        return err(StatusCode::BAD_REQUEST, "prompt 不能为空");
    }

    // Persist attached images as sources (so they can be re-used in history
    // view). Prefer the multi-image field; fall back to the legacy single one.
    let input_urls: Vec<String> = match body.image_data_urls.as_ref() {
        Some(v) if !v.is_empty() => v.clone(),
        _ => body.image_data_url.clone().into_iter().collect(),
    };
    let mut source_paths: Vec<String> = Vec::new();
    let mut source_bytes: Vec<(Vec<u8>, &'static str)> = Vec::new();
    for durl in &input_urls {
        match parse_data_url(durl) {
            Some((bytes, ext, mime)) => {
                match write_image(&state.storage, &bytes, ext).await {
                    Ok(p) => source_paths.push(p),
                    Err(e) => {
                        return err(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            format!("save source: {e}"),
                        );
                    }
                }
                source_bytes.push((bytes, mime));
            }
            None => return err(StatusCode::BAD_REQUEST, "附加图片格式不支持"),
        }
    }
    let source_path_first = source_paths.first().cloned();
    let source_paths_json = if source_paths.is_empty() {
        None
    } else {
        serde_json::to_string(&source_paths).ok()
    };

    let edit = !source_bytes.is_empty();
    // Compute model first (used by both channel selection and request body).
    let model = body
        .model
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "gpt-image-1".into());

    let (url, key, used_shared, admin_model) =
        match resolve_openai_image_upstream(&state, &headers, edit, &model).await {
            Ok(v) => v,
            Err(r) => return r,
        };

    // admin_model now carries the upstream alias from the chosen channel.
    // When non-empty, override the client-facing model name in the request body.
    let upstream_model = admin_model.clone().unwrap_or_else(|| model.clone());
    let _ = admin_model;

    // Credits.
    if used_shared {
        match channels::try_deduct_per_call(
            &installed.pool,
            installed.kind,
            user.id,
            &model,
            "image",
            "openai",
            "studio_generate",
        )
        .await
        {
            Ok(_) => {}
            Err(channels::DeductError::NotWhitelisted) => {
                return err(
                    StatusCode::FORBIDDEN,
                    format!("模型 {model} 未启用：管理员尚未在「模型计费」中开放此图像模型"),
                );
            }
            Err(channels::DeductError::Insufficient { balance, cost }) => {
                return err(
                    StatusCode::PAYMENT_REQUIRED,
                    format!(
                        "额度不足：当前剩余 {}，本次需要 {}",
                        quota::format_quota(balance),
                        quota::format_quota(cost)
                    ),
                );
            }
        }
    }

    let token = random_hex(16);
    let n_req: i64 = body.n.map(|v| (v as i64).clamp(1, 10)).unwrap_or(1);
    if let Err(e) = insert_pending(
        &installed.pool,
        installed.kind,
        user.id,
        &token,
        &prompt,
        Some(&model),
        body.size.as_deref(),
        body.quality.as_deref(),
        body.style.as_deref(),
        n_req,
        body.negative_prompt.as_deref(),
        body.seed,
        body.background.as_deref(),
        source_path_first.as_deref(),
        source_paths_json.as_deref(),
        used_shared,
    )
    .await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }

    // Spawn background task.
    let state_c = state.clone();
    let token_c = token.clone();
    let prompt_c = prompt.clone();
    let model_c = upstream_model.clone();
    let size_c = body.size.clone();
    let quality_c = body.quality.clone();
    let style_c = body.style.clone();
    let n_req_c = n_req;
    let neg_c = body.negative_prompt.clone();
    let seed_c = body.seed;
    let bg_c = body.background.clone();
    let source_bytes_c = source_bytes;
    tokio::spawn(async move {
        let Ok(installed) = state_c.require_installed().await else { return };
        let pool = installed.pool.clone();
        let kind = installed.kind;

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
                finalize_failed(&pool, kind, &token_c, "HTTP client 构建失败").await;
                if used_shared {
                    let cost = channels::per_call_quota(&pool, kind, &model_c, "image").await.unwrap_or(0);
                    if cost > 0 {
                        let _ = quota::grant(
                            &pool,
                            kind,
                            user.id,
                            cost,
                            "refund_studio_error",
                            &quota::LedgerMeta::refund_image(&model_c),
                        )
                        .await;
                    }
                }
                return;
            }
        };

        // Retry transient upstream errors (connection errors, 429, 5xx) up to
        // MAX_ATTEMPTS times with exponential backoff. 4xx other than 429 are
        // returned as terminal immediately — they're usually prompt violations
        // or config problems where retrying just wastes time.
        const MAX_ATTEMPTS: u32 = 3;
        let mut attempt: u32 = 0;
        let mut last_transient: Option<String> = None;
        let (status, raw) = loop {
            attempt += 1;
            let resp_result = if !source_bytes_c.is_empty() {
                // /v1/images/edits → multipart form. Bytes are cloned per
                // attempt so retries can rebuild the form. Multiple base images
                // are sent as `image[]` (gpt-image-1); a single one keeps the
                // `image` field for dall-e-2 compatibility.
                let mut form = reqwest::multipart::Form::new()
                    .text("model", model_c.clone())
                    .text("prompt", prompt_c.clone())
                    .text("n", n_req_c.to_string());
                let field = if source_bytes_c.len() > 1 { "image[]" } else { "image" };
                let mut part_err: Option<String> = None;
                for (idx, (bytes, mime)) in source_bytes_c.iter().enumerate() {
                    match reqwest::multipart::Part::bytes(bytes.clone())
                        .file_name(format!("source{idx}.{}", mime_to_ext(mime)))
                        .mime_str(mime)
                    {
                        Ok(p) => form = form.part(field, p),
                        Err(e) => {
                            part_err = Some(format!("multipart: {e}"));
                            break;
                        }
                    }
                }
                if let Some(msg) = part_err {
                    finalize_failed(&pool, kind, &token_c, &msg).await;
                    if used_shared {
                        let cost = channels::per_call_quota(&pool, kind, &model_c, "image").await.unwrap_or(0);
                        if cost > 0 {
                            let _ = quota::grant(
                                &pool,
                                kind,
                                user.id,
                                cost,
                                "refund_studio_error",
                                &quota::LedgerMeta::refund_image(&model_c),
                            )
                            .await;
                        }
                    }
                    return;
                }
                if let Some(s) = size_c.as_deref() {
                    if !s.is_empty() && s != "auto" {
                        form = form.text("size", s.to_string());
                    }
                }
                if let Some(q) = quality_c.as_deref() {
                    if !q.is_empty() {
                        form = form.text("quality", q.to_string());
                    }
                }
                if let Some(neg) = neg_c.as_deref() {
                    if !neg.is_empty() {
                        form = form.text("negative_prompt", neg.to_string());
                    }
                }
                if let Some(seed) = seed_c {
                    form = form.text("seed", seed.to_string());
                }
                if let Some(bg) = bg_c.as_deref() {
                    if !bg.is_empty() {
                        form = form.text("background", bg.to_string());
                    }
                }
                client.post(&url).bearer_auth(&key).multipart(form).send().await
            } else {
                // /v1/images/generations → JSON
                let mut body = json!({
                    "model": model_c,
                    "prompt": prompt_c,
                    "n": n_req_c,
                });
                if let Some(s) = size_c.as_deref() {
                    if !s.is_empty() {
                        body["size"] = json!(s);
                    }
                }
                if let Some(q) = quality_c.as_deref() {
                    if !q.is_empty() {
                        body["quality"] = json!(q);
                    }
                }
                if let Some(st) = style_c.as_deref() {
                    if !st.is_empty() {
                        body["style"] = json!(st);
                    }
                }
                if let Some(neg) = neg_c.as_deref() {
                    if !neg.is_empty() {
                        body["negative_prompt"] = json!(neg);
                    }
                }
                if let Some(seed) = seed_c {
                    body["seed"] = json!(seed);
                }
                if let Some(bg) = bg_c.as_deref() {
                    if !bg.is_empty() {
                        body["background"] = json!(bg);
                    }
                }
                // gpt-image-1 returns b64 by default; dall-e/classic models need
                // response_format explicitly. Always request b64 for consistent parsing.
                body["response_format"] = json!("b64_json");
                client
                    .post(&url)
                    .bearer_auth(&key)
                    .header(header::CONTENT_TYPE, "application/json")
                    .json(&body)
                    .send()
                    .await
            };

            match resp_result {
                Ok(r) => {
                    let status = r.status();
                    let code = status.as_u16();
                    let transient = code == 429 || code >= 500;
                    let raw = r.bytes().await.unwrap_or_default();
                    if status.is_success() || !transient || attempt >= MAX_ATTEMPTS {
                        break (status, raw);
                    }
                    last_transient = Some(format!(
                        "upstream {status}: {}",
                        String::from_utf8_lossy(&raw)
                    ));
                }
                Err(e) => {
                    if attempt >= MAX_ATTEMPTS {
                        let msg = match last_transient {
                            Some(prev) => format!(
                                "upstream: {e}（已重试 {} 次，上次：{}）",
                                attempt - 1,
                                prev
                            ),
                            None => format!("upstream: {e}（已重试 {} 次）", attempt - 1),
                        };
                        finalize_failed(&pool, kind, &token_c, &msg).await;
                        if used_shared {
                            let cost = channels::per_call_quota(&pool, kind, &model_c, "image").await.unwrap_or(0);
                    if cost > 0 {
                        let _ = quota::grant(
                            &pool,
                            kind,
                            user.id,
                            cost,
                            "refund_studio_error",
                            &quota::LedgerMeta::refund_image(&model_c),
                        )
                        .await;
                    }
                        }
                        return;
                    }
                    last_transient = Some(format!("upstream: {e}"));
                }
            }

            // Backoff: 0.8s, 2.4s (multiplied by 3^n).
            let delay_ms = 800u64 * 3u64.pow(attempt - 1);
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        };

        if !status.is_success() {
            let msg = if attempt > 1 {
                format!(
                    "upstream {status}（已重试 {} 次）: {}",
                    attempt - 1,
                    String::from_utf8_lossy(&raw)
                )
            } else {
                format!("upstream {status}: {}", String::from_utf8_lossy(&raw))
            };
            finalize_failed(&pool, kind, &token_c, &msg).await;
            if used_shared {
                let cost = channels::per_call_quota(&pool, kind, &model_c, "image").await.unwrap_or(0);
                    if cost > 0 {
                        let _ = quota::grant(
                            &pool,
                            kind,
                            user.id,
                            cost,
                            "refund_studio_error",
                            &quota::LedgerMeta::refund_image(&model_c),
                        )
                        .await;
                    }
            }
            return;
        }

        // Parse { data: [{ b64_json, revised_prompt? }] } — take first item.
        let parsed: Value = match serde_json::from_slice(&raw) {
            Ok(v) => v,
            Err(e) => {
                finalize_failed(&pool, kind, &token_c, &format!("parse: {e}")).await;
                return;
            }
        };
        let first = parsed
            .get("data")
            .and_then(|d| d.as_array())
            .and_then(|a| a.first());
        let Some(item) = first else {
            finalize_failed(&pool, kind, &token_c, "上游未返回图像数据").await;
            if used_shared {
                let cost = channels::per_call_quota(&pool, kind, &model_c, "image").await.unwrap_or(0);
                    if cost > 0 {
                        let _ = quota::grant(
                            &pool,
                            kind,
                            user.id,
                            cost,
                            "refund_studio_error",
                            &quota::LedgerMeta::refund_image(&model_c),
                        )
                        .await;
                    }
            }
            return;
        };
        let revised = item
            .get("revised_prompt")
            .and_then(|v| v.as_str())
            .map(String::from);
        let bytes = if let Some(b64) = item.get("b64_json").and_then(|v| v.as_str()) {
            match STANDARD.decode(b64) {
                Ok(b) => b,
                Err(e) => {
                    finalize_failed(&pool, kind, &token_c, &format!("b64 decode: {e}")).await;
                    return;
                }
            }
        } else if let Some(remote) = item.get("url").and_then(|v| v.as_str()) {
            let (_parsed, host, addrs) = match net_guard::validate_upstream_url(remote).await {
                Ok(v) => v,
                Err(_) => {
                    finalize_failed(&pool, kind, &token_c, "remote image URL 校验失败").await;
                    return;
                }
            };
            let gc = match net_guard::guarded_client_with_timeout(
                &host,
                &addrs,
                Duration::from_secs(120),
            ) {
                Ok(c) => c,
                Err(_) => {
                    finalize_failed(&pool, kind, &token_c, "remote client 构建失败").await;
                    return;
                }
            };
            match gc.get(remote).send().await {
                Ok(r) => r.bytes().await.unwrap_or_default().to_vec(),
                Err(e) => {
                    finalize_failed(&pool, kind, &token_c, &format!("fetch image: {e}")).await;
                    return;
                }
            }
        } else {
            finalize_failed(&pool, kind, &token_c, "上游返回缺少 b64_json/url").await;
            return;
        };

        let stored = match write_image(&state_c.storage, &bytes, "png").await {
            Ok(p) => p,
            Err(e) => {
                finalize_failed(&pool, kind, &token_c, &format!("write: {e}")).await;
                return;
            }
        };

        finalize_done(&pool, kind, &token_c, &stored, revised.as_deref()).await;
    });

    Json(GenerateCreated { token }).into_response()
}

fn mime_to_ext(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        _ => "png",
    }
}

// ---------------------------------------------------------------------------
// polling + history + delete
// ---------------------------------------------------------------------------

async fn get_generation(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(token): Path<String>,
) -> Response {
    let sql = db::q(
        installed.kind,
        &format!(
            "SELECT {GEN_COLS} FROM studio_generations WHERE token = ? AND user_id = ?"
        ),
    );
    let row: Option<GenRow> = sqlx::query_as(&sql)
        .bind(&token)
        .bind(user.id)
        .fetch_optional(&installed.pool)
        .await
        .ok()
        .flatten();
    match row {
        Some(r) => Json(gen_from_row(r)).into_response(),
        None => err(StatusCode::NOT_FOUND, "任务不存在"),
    }
}

#[derive(Deserialize)]
struct ListQuery {
    page: Option<i64>,
}

async fn list_generations(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Query(q): Query<ListQuery>,
) -> Response {
    let page = q.page.unwrap_or(1).max(1);
    let per_page: i64 = 30;
    let offset = (page - 1) * per_page;
    let sql = db::q(
        installed.kind,
        &format!(
            "SELECT {GEN_COLS} FROM studio_generations
             WHERE user_id = ?
             ORDER BY created_at DESC, id DESC
             LIMIT ? OFFSET ?"
        ),
    );
    let rows: Result<Vec<GenRow>, _> = sqlx::query_as(&sql)
        .bind(user.id)
        .bind(per_page)
        .bind(offset)
        .fetch_all(&installed.pool)
        .await;
    match rows {
        Ok(r) => Json(r.into_iter().map(gen_from_row).collect::<Vec<_>>()).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn delete_generation(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Response {
    let sql = db::q(
        installed.kind,
        "DELETE FROM studio_generations WHERE id = ? AND user_id = ?",
    );
    let _ = sqlx::query(&sql)
        .bind(id)
        .bind(user.id)
        .execute(&installed.pool)
        .await;
    StatusCode::NO_CONTENT.into_response()
}

/// On startup: anything left pending is stale.
pub async fn cleanup_stale_jobs(pool: &db::Pool, kind: db::DbKind) {
    let now = db::now_expr(kind);
    let sql = db::q(
        kind,
        &format!(
            "UPDATE studio_generations SET status = 'failed', error = 'server restarted',
                    finished_at = {now}
             WHERE status IN ('pending', 'running')"
        ),
    );
    let _ = sqlx::query(&sql).execute(pool).await;
}

// ---------------------------------------------------------------------------
// routes
// ---------------------------------------------------------------------------

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/studio/generate", post(submit_generate))
        .route("/studio/generations", get(list_generations))
        .route("/studio/generations/{id}", axum::routing::delete(delete_generation))
        .route("/studio/jobs/{token}", get(get_generation))
        .route("/studio/models", get(list_models))
}

// ---------------------------------------------------------------------------
// models: resolve the same upstream (image flavor for shared) and GET /v1/models
// ---------------------------------------------------------------------------

async fn list_models(
    State(state): State<AppState>,
    Extension(_installed): Extension<InstalledState>,
    Extension(_user): Extension<CurrentUser>,
    headers: HeaderMap,
) -> Response {
    let (base_url, key, used_shared) = {
        let hdr_url = read_header(&headers, "x-upstream-url");
        let hdr_key = read_header(&headers, "x-upstream-key");
        match (hdr_url, hdr_key) {
            (Some(u), Some(k)) if !u.is_empty() && !k.is_empty() => {
                (u.trim_end_matches('/').to_string(), k.to_string(), false)
            }
            _ => {
                // Fall back to any admin-configured OpenAI image channel.
                let installed = match state.require_installed().await {
                    Ok(s) => s,
                    Err(r) => return r,
                };
                match channels::any_enabled_channel(
                    &installed.pool,
                    installed.kind,
                    "openai",
                )
                .await
                .ok()
                .flatten()
                {
                    Some(ch) => (
                        ch.base_url.trim_end_matches('/').to_string(),
                        ch.api_key,
                        true,
                    ),
                    None => {
                        return err(
                            StatusCode::SERVICE_UNAVAILABLE,
                            "未配置任何启用的 OpenAI 图像渠道",
                        );
                    }
                }
            }
        }
    };

    let url = format!("{base_url}/v1/models");
    let client = match net_guard::client_for_upstream(&state.http, &url, used_shared).await {
        Ok(c) => c,
        Err(r) => return r,
    };

    let resp = match client
        .get(&url)
        .header(header::ACCEPT, "application/json")
        .bearer_auth(&key)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return err(StatusCode::BAD_GATEWAY, format!("upstream: {e}")),
    };
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return err(StatusCode::BAD_GATEWAY, format!("upstream {status}: {body}"));
    }
    let parsed: Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => return err(StatusCode::BAD_GATEWAY, format!("parse: {e}")),
    };

    let mut models: Vec<String> = parsed
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("id").and_then(|x| x.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();
    models.sort();
    models.dedup();

    Json(json!({ "models": models })).into_response()
}
