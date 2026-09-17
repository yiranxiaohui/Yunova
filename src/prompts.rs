use axum::{
    Extension, Json, Router,
    extract::{Path, Query},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, patch, post},
};
use serde::{Deserialize, Serialize};

use crate::{
    AppState, CurrentUser, InstalledState,
    db::{self, DbKind, Pool},
};

pub const MAX_PROMPTS_PER_USER: i64 = 500;
pub const MAX_PUBLIC_PER_USER: i64 = 50;

#[derive(Serialize, sqlx::FromRow)]
pub struct Prompt {
    pub id: i64,
    pub name: String,
    pub content: String,
    #[serde(serialize_with = "ser_bool_int")]
    pub is_public: i64,
    pub created_at: String,
    pub updated_at: String,
}

fn ser_bool_int<S: serde::Serializer>(v: &i64, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_bool(*v != 0)
}

#[derive(Serialize, sqlx::FromRow)]
pub struct PublicPrompt {
    pub id: i64,
    pub name: String,
    pub content: String,
    pub author_username: String,
    pub created_at: String,
    pub clone_count: i64,
}

#[derive(Deserialize)]
pub struct NewPrompt {
    pub name: String,
    pub content: String,
}

#[derive(Deserialize)]
pub struct UpdatePrompt {
    pub name: Option<String>,
    pub content: Option<String>,
    pub is_public: Option<bool>,
}

#[derive(Serialize)]
pub struct Prefs {
    pub default_prompt_id: Option<i64>,
}

#[derive(Deserialize)]
pub struct UpdatePrefs {
    pub default_prompt_id: Option<i64>,
}

#[derive(Deserialize)]
pub struct PublicQuery {
    pub search: Option<String>,
    pub page: Option<i64>,
    pub sort: Option<String>,
}

#[derive(Deserialize)]
pub struct ImportBundle {
    #[allow(dead_code)]
    pub version: Option<u32>,
    pub prompts: Vec<ImportItem>,
}

#[derive(Deserialize, Serialize)]
pub struct ImportItem {
    pub name: String,
    pub content: String,
}

#[derive(Serialize)]
pub struct ImportResult {
    pub imported: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, msg.into()).into_response()
}

fn validate_name(name: &str) -> Result<String, Response> {
    let trimmed: String = name.chars().filter(|c| !c.is_control()).collect();
    let trimmed = trimmed.trim();
    if trimmed.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "name is empty"));
    }
    if trimmed.chars().count() > 80 {
        return Err(err(StatusCode::BAD_REQUEST, "name too long"));
    }
    Ok(trimmed.to_string())
}

fn validate_content(content: &str) -> Result<(), Response> {
    if content.len() > 32_000 {
        return Err(err(StatusCode::BAD_REQUEST, "content too long"));
    }
    Ok(())
}

async fn prompt_belongs(
    pool: &Pool,
    kind: DbKind,
    id: i64,
    user_id: i64,
) -> Result<(), Response> {
    let sql = db::q(kind, "SELECT 1 FROM prompts WHERE id = ? AND user_id = ?");
    let row: Option<(i32,)> = sqlx::query_as(&sql)
        .bind(id)
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    row.ok_or_else(|| err(StatusCode::NOT_FOUND, "prompt not found"))
        .map(|_| ())
}

async fn count_user_prompts(pool: &Pool, kind: DbKind, user_id: i64) -> Result<i64, Response> {
    let sql = db::q(kind, "SELECT COUNT(*) FROM prompts WHERE user_id = ?");
    let (n,): (i64,) = sqlx::query_as(&sql)
        .bind(user_id)
        .fetch_one(pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(n)
}

async fn count_user_public(pool: &Pool, kind: DbKind, user_id: i64) -> Result<i64, Response> {
    let sql = db::q(
        kind,
        "SELECT COUNT(*) FROM prompts WHERE user_id = ? AND is_public = 1",
    );
    let (n,): (i64,) = sqlx::query_as(&sql)
        .bind(user_id)
        .fetch_one(pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(n)
}

async fn insert_prompt(
    pool: &Pool,
    kind: DbKind,
    user_id: i64,
    name: &str,
    content: &str,
) -> Result<Prompt, Response> {
    let n = count_user_prompts(pool, kind, user_id).await?;
    if n >= MAX_PROMPTS_PER_USER {
        return Err(err(
            StatusCode::BAD_REQUEST,
            format!("maximum {MAX_PROMPTS_PER_USER} prompts reached"),
        ));
    }

    let base_insert = db::q(
        kind,
        "INSERT INTO prompts (user_id, name, content) VALUES (?, ?, ?)",
    );
    let row: (i64,) = sqlx::query_as(&format!("{base_insert} RETURNING id"))
        .bind(user_id)
        .bind(name)
        .bind(content)
        .fetch_one(pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let id = row.0;

    let sel_sql = db::q(
        kind,
        "SELECT id, name, content, is_public, created_at, updated_at FROM prompts WHERE id = ?",
    );
    let row: Prompt = sqlx::query_as(&sel_sql)
        .bind(id)
        .fetch_one(pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(row)
}

async fn list_prompts(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
) -> Response {
    let sql = db::q(
        installed.kind,
        "SELECT id, name, content, is_public, created_at, updated_at FROM prompts
             WHERE user_id = ? ORDER BY updated_at DESC",
    );
    let rows: Result<Vec<Prompt>, _> = sqlx::query_as(&sql)
        .bind(user.id)
        .fetch_all(&installed.pool)
        .await;
    match rows {
        Ok(r) => Json(r).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn create_prompt(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<NewPrompt>,
) -> Response {
    let name = match validate_name(&body.name) {
        Ok(n) => n,
        Err(r) => return r,
    };
    if let Err(r) = validate_content(&body.content) {
        return r;
    }
    match insert_prompt(&installed.pool, installed.kind, user.id, &name, &body.content).await {
        Ok(p) => Json(p).into_response(),
        Err(r) => r,
    }
}

async fn update_prompt(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(body): Json<UpdatePrompt>,
) -> Response {
    if let Err(r) = prompt_belongs(&installed.pool, installed.kind, id, user.id).await {
        return r;
    }

    let new_name = match body.name {
        Some(n) => match validate_name(&n) {
            Ok(v) => Some(v),
            Err(r) => return r,
        },
        None => None,
    };
    if let Some(ref c) = body.content {
        if let Err(r) = validate_content(c) {
            return r;
        }
    }

    if let Some(true) = body.is_public {
        let sel = db::q(
            installed.kind,
            "SELECT is_public FROM prompts WHERE id = ? AND user_id = ?",
        );
        let current_is_public: Option<(i64,)> = sqlx::query_as(&sel)
            .bind(id)
            .bind(user.id)
            .fetch_optional(&installed.pool)
            .await
            .unwrap_or(None);
        let already = matches!(current_is_public, Some((v,)) if v != 0);
        if !already {
            let count = match count_user_public(&installed.pool, installed.kind, user.id).await {
                Ok(n) => n,
                Err(r) => return r,
            };
            if count >= MAX_PUBLIC_PER_USER {
                return err(
                    StatusCode::BAD_REQUEST,
                    format!("maximum {MAX_PUBLIC_PER_USER} public prompts reached"),
                );
            }
        }
    }

    let now = db::now_expr(installed.kind);
    let update_sql = db::q(
        installed.kind,
        &format!(
            "UPDATE prompts
             SET name = COALESCE(?, name),
                 content = COALESCE(?, content),
                 is_public = COALESCE(?, is_public),
                 updated_at = {now}
             WHERE id = ? AND user_id = ?"
        ),
    );
    let r = sqlx::query(&update_sql)
        .bind(new_name)
        .bind(body.content)
        .bind(body.is_public)
        .bind(id)
        .bind(user.id)
        .execute(&installed.pool)
        .await;
    match r {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn delete_prompt(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Response {
    let sql = db::q(
        installed.kind,
        "DELETE FROM prompts WHERE id = ? AND user_id = ?",
    );
    let r = sqlx::query(&sql)
        .bind(id)
        .bind(user.id)
        .execute(&installed.pool)
        .await;
    match r {
        Ok(out) if out.rows_affected() > 0 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => err(StatusCode::NOT_FOUND, "prompt not found"),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn get_prefs(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
) -> Response {
    let sql = db::q(
        installed.kind,
        "SELECT default_prompt_id FROM users WHERE id = ?",
    );
    let row: Result<(Option<i64>,), _> = sqlx::query_as(&sql)
        .bind(user.id)
        .fetch_one(&installed.pool)
        .await;
    match row {
        Ok((id,)) => Json(Prefs { default_prompt_id: id }).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn update_prefs(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<UpdatePrefs>,
) -> Response {
    if let Some(pid) = body.default_prompt_id {
        if let Err(r) = prompt_belongs(&installed.pool, installed.kind, pid, user.id).await {
            return r;
        }
    }
    let sql = db::q(
        installed.kind,
        "UPDATE users SET default_prompt_id = ? WHERE id = ?",
    );
    let r = sqlx::query(&sql)
        .bind(body.default_prompt_id)
        .bind(user.id)
        .execute(&installed.pool)
        .await;
    match r {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn list_public(
    Extension(installed): Extension<InstalledState>,
    Extension(_user): Extension<CurrentUser>,
    Query(q): Query<PublicQuery>,
) -> Response {
    let page = q.page.unwrap_or(1).max(1);
    let per_page: i64 = 20;
    let offset = (page - 1) * per_page;
    let search = q
        .search
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    let order_by = match q.sort.as_deref() {
        Some("new") => "p.created_at DESC, p.id DESC",
        _ => "p.clone_count DESC, p.created_at DESC, p.id DESC",
    };

    let rows: Result<Vec<PublicPrompt>, _> = if let Some(s) = search {
        let pattern = format!("%{s}%");
        let sql = db::q(
            installed.kind,
            &format!(
                "SELECT p.id, p.name, p.content, u.username AS author_username, p.created_at, p.clone_count
                 FROM prompts p JOIN users u ON u.id = p.user_id
                 WHERE p.is_public = 1 AND (p.name LIKE ? OR p.content LIKE ?)
                 ORDER BY {order_by}
                 LIMIT ? OFFSET ?"
            ),
        );
        sqlx::query_as(&sql)
            .bind(&pattern)
            .bind(&pattern)
            .bind(per_page)
            .bind(offset)
            .fetch_all(&installed.pool)
            .await
    } else {
        let sql = db::q(
            installed.kind,
            &format!(
                "SELECT p.id, p.name, p.content, u.username AS author_username, p.created_at, p.clone_count
                 FROM prompts p JOIN users u ON u.id = p.user_id
                 WHERE p.is_public = 1
                 ORDER BY {order_by}
                 LIMIT ? OFFSET ?"
            ),
        );
        sqlx::query_as(&sql)
            .bind(per_page)
            .bind(offset)
            .fetch_all(&installed.pool)
            .await
    };
    match rows {
        Ok(r) => Json(r).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn clone_public(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Response {
    let sel = db::q(
        installed.kind,
        "SELECT name, content, user_id FROM prompts WHERE id = ? AND is_public = 1",
    );
    let row: Option<(String, String, i64)> = match sqlx::query_as(&sel)
        .bind(id)
        .fetch_optional(&installed.pool)
        .await
    {
        Ok(r) => r,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let Some((name, content, author_id)) = row else {
        return err(StatusCode::NOT_FOUND, "public prompt not found");
    };
    let inserted =
        match insert_prompt(&installed.pool, installed.kind, user.id, &name, &content).await {
            Ok(p) => p,
            Err(r) => return r,
        };
    if author_id != user.id {
        let bump = db::q(
            installed.kind,
            "UPDATE prompts SET clone_count = clone_count + 1 WHERE id = ?",
        );
        let _ = sqlx::query(&bump).bind(id).execute(&installed.pool).await;
    }
    Json(inserted).into_response()
}

async fn export_prompts(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
) -> Response {
    let sql = db::q(
        installed.kind,
        "SELECT name, content FROM prompts WHERE user_id = ? ORDER BY id ASC",
    );
    let rows: Result<Vec<(String, String)>, _> = sqlx::query_as(&sql)
        .bind(user.id)
        .fetch_all(&installed.pool)
        .await;
    let rows = match rows {
        Ok(r) => r,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let items: Vec<ImportItem> = rows
        .into_iter()
        .map(|(name, content)| ImportItem { name, content })
        .collect();
    let bundle = serde_json::json!({
        "version": 1,
        "prompts": items,
    });
    let body = serde_json::to_vec_pretty(&bundle).unwrap_or_default();
    let date = chrono::Utc::now().format("%Y%m%d").to_string();
    Response::builder()
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .header(
            header::CONTENT_DISPOSITION,
            format!(r#"attachment; filename="prompts-{date}.json""#),
        )
        .body(axum::body::Body::from(body))
        .unwrap()
}

async fn import_prompts(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Json(bundle): Json<ImportBundle>,
) -> Response {
    if bundle.prompts.is_empty() {
        return err(StatusCode::BAD_REQUEST, "prompts is empty");
    }
    if bundle.prompts.len() > 1000 {
        return err(StatusCode::BAD_REQUEST, "too many prompts in one request");
    }

    let mut result = ImportResult {
        imported: 0,
        skipped: 0,
        errors: Vec::new(),
    };

    for (i, item) in bundle.prompts.into_iter().enumerate() {
        let name = match validate_name(&item.name) {
            Ok(n) => n,
            Err(_) => {
                result.skipped += 1;
                result.errors.push(format!("#{i}: invalid name"));
                continue;
            }
        };
        if validate_content(&item.content).is_err() {
            result.skipped += 1;
            result.errors.push(format!("#{i}: content too long"));
            continue;
        }
        match insert_prompt(&installed.pool, installed.kind, user.id, &name, &item.content).await {
            Ok(_) => result.imported += 1,
            Err(_) => {
                result.skipped += 1;
                result
                    .errors
                    .push(format!("#{i}: quota reached or db error"));
                break;
            }
        }
    }

    Json(result).into_response()
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/prompts", get(list_prompts).post(create_prompt))
        .route("/prompts/public", get(list_public))
        .route("/prompts/export", get(export_prompts))
        .route("/prompts/import", post(import_prompts))
        .route("/prompts/{id}/clone", post(clone_public))
        .route(
            "/prompts/{id}",
            patch(update_prompt).delete(delete_prompt),
        )
        .route("/prefs", get(get_prefs).patch(update_prefs))
}

pub async fn default_system_prompt(
    pool: &Pool,
    kind: DbKind,
    user_id: i64,
) -> Option<String> {
    let sql = db::q(
        kind,
        "SELECT p.content FROM users u
         JOIN prompts p ON p.id = u.default_prompt_id AND p.user_id = u.id
         WHERE u.id = ?",
    );
    let row: Option<(String,)> = sqlx::query_as(&sql)
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    row.map(|(c,)| c)
}
