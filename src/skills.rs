use axum::{
    Extension, Json, Router,
    extract::{Path, Query},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post},
};
use serde::{Deserialize, Serialize};

use crate::{
    AppState, CurrentUser, InstalledState,
    db::{self, DbKind, Pool},
};

pub const MAX_SKILLS_PER_USER: i64 = 200;
pub const MAX_PUBLIC_SKILLS_PER_USER: i64 = 50;
pub const MAX_SKILLS_PER_CONVERSATION: i64 = 10;
pub const MAX_INSTRUCTIONS_LEN: usize = 32_000;
pub const MAX_DESCRIPTION_LEN: usize = 500;
pub const MAX_NAME_CHARS: usize = 80;

#[derive(Serialize, sqlx::FromRow)]
pub struct Skill {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub instructions: String,
    #[serde(serialize_with = "ser_bool_int")]
    pub is_public: i64,
    pub created_at: String,
    pub updated_at: String,
}

fn ser_bool_int<S: serde::Serializer>(v: &i64, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_bool(*v != 0)
}

#[derive(Serialize, sqlx::FromRow)]
pub struct PublicSkill {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub instructions: String,
    pub author_username: String,
    pub created_at: String,
    pub clone_count: i64,
}

#[derive(Deserialize)]
pub struct NewSkill {
    pub name: String,
    pub description: Option<String>,
    pub instructions: String,
}

#[derive(Deserialize)]
pub struct UpdateSkill {
    pub name: Option<String>,
    pub description: Option<String>,
    pub instructions: Option<String>,
    pub is_public: Option<bool>,
}

#[derive(Deserialize)]
pub struct AttachSkill {
    pub skill_id: i64,
}

#[derive(Deserialize)]
pub struct PublicQuery {
    pub search: Option<String>,
    pub page: Option<i64>,
    pub sort: Option<String>,
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
    if trimmed.chars().count() > MAX_NAME_CHARS {
        return Err(err(StatusCode::BAD_REQUEST, "name too long"));
    }
    Ok(trimmed.to_string())
}

fn validate_description(desc: &str) -> Result<String, Response> {
    let trimmed: String = desc.chars().filter(|c| !c.is_control() || *c == '\n').collect();
    let trimmed = trimmed.trim();
    if trimmed.len() > MAX_DESCRIPTION_LEN {
        return Err(err(StatusCode::BAD_REQUEST, "description too long"));
    }
    Ok(trimmed.to_string())
}

fn validate_instructions(c: &str) -> Result<(), Response> {
    if c.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "instructions is empty"));
    }
    if c.len() > MAX_INSTRUCTIONS_LEN {
        return Err(err(StatusCode::BAD_REQUEST, "instructions too long"));
    }
    Ok(())
}

async fn skill_belongs(
    pool: &Pool,
    kind: DbKind,
    id: i64,
    user_id: i64,
) -> Result<(), Response> {
    let sql = db::q(kind, "SELECT 1 FROM skills WHERE id = ? AND user_id = ?");
    let row: Option<(i32,)> = sqlx::query_as(&sql)
        .bind(id)
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    row.ok_or_else(|| err(StatusCode::NOT_FOUND, "skill not found"))
        .map(|_| ())
}

async fn conversation_belongs(
    pool: &Pool,
    kind: DbKind,
    conv_id: i64,
    user_id: i64,
) -> Result<(), Response> {
    let sql = db::q(
        kind,
        "SELECT 1 FROM conversations WHERE id = ? AND user_id = ?",
    );
    let row: Option<(i32,)> = sqlx::query_as(&sql)
        .bind(conv_id)
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    row.ok_or_else(|| err(StatusCode::NOT_FOUND, "conversation not found"))
        .map(|_| ())
}

async fn count_user_skills(pool: &Pool, kind: DbKind, user_id: i64) -> Result<i64, Response> {
    let sql = db::q(kind, "SELECT COUNT(*) FROM skills WHERE user_id = ?");
    let (n,): (i64,) = sqlx::query_as(&sql)
        .bind(user_id)
        .fetch_one(pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(n)
}

async fn count_user_public_skills(
    pool: &Pool,
    kind: DbKind,
    user_id: i64,
) -> Result<i64, Response> {
    let sql = db::q(
        kind,
        "SELECT COUNT(*) FROM skills WHERE user_id = ? AND is_public = 1",
    );
    let (n,): (i64,) = sqlx::query_as(&sql)
        .bind(user_id)
        .fetch_one(pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(n)
}

async fn count_conv_skills(pool: &Pool, kind: DbKind, conv_id: i64) -> Result<i64, Response> {
    let sql = db::q(
        kind,
        "SELECT COUNT(*) FROM conversation_skills WHERE conversation_id = ?",
    );
    let (n,): (i64,) = sqlx::query_as(&sql)
        .bind(conv_id)
        .fetch_one(pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(n)
}

async fn list_skills(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
) -> Response {
    let sql = db::q(
        installed.kind,
        "SELECT id, name, description, instructions, is_public, created_at, updated_at
             FROM skills WHERE user_id = ? ORDER BY updated_at DESC",
    );
    let rows: Result<Vec<Skill>, _> = sqlx::query_as(&sql)
        .bind(user.id)
        .fetch_all(&installed.pool)
        .await;
    match rows {
        Ok(r) => Json(r).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn insert_skill(
    pool: &Pool,
    kind: DbKind,
    user_id: i64,
    name: &str,
    description: &str,
    instructions: &str,
) -> Result<Skill, Response> {
    let n = count_user_skills(pool, kind, user_id).await?;
    if n >= MAX_SKILLS_PER_USER {
        return Err(err(
            StatusCode::BAD_REQUEST,
            format!("maximum {MAX_SKILLS_PER_USER} skills reached"),
        ));
    }

    let base_insert = db::q(
        kind,
        "INSERT INTO skills (user_id, name, description, instructions) VALUES (?, ?, ?, ?)",
    );
    let row: (i64,) = sqlx::query_as(&format!("{base_insert} RETURNING id"))
        .bind(user_id)
        .bind(name)
        .bind(description)
        .bind(instructions)
        .fetch_one(pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let id = row.0;

    let sel = db::q(
        kind,
        "SELECT id, name, description, instructions, is_public, created_at, updated_at
         FROM skills WHERE id = ?",
    );
    sqlx::query_as::<_, Skill>(&sel)
        .bind(id)
        .fetch_one(pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn create_skill(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<NewSkill>,
) -> Response {
    let name = match validate_name(&body.name) {
        Ok(n) => n,
        Err(r) => return r,
    };
    let description = match validate_description(body.description.as_deref().unwrap_or("")) {
        Ok(d) => d,
        Err(r) => return r,
    };
    if let Err(r) = validate_instructions(&body.instructions) {
        return r;
    }

    match insert_skill(
        &installed.pool,
        installed.kind,
        user.id,
        &name,
        &description,
        &body.instructions,
    )
    .await
    {
        Ok(s) => Json(s).into_response(),
        Err(r) => r,
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
        Some("new") => "s.created_at DESC, s.id DESC",
        _ => "s.clone_count DESC, s.created_at DESC, s.id DESC",
    };

    let rows: Result<Vec<PublicSkill>, _> = if let Some(s) = search {
        let pattern = format!("%{s}%");
        let sql = db::q(
            installed.kind,
            &format!(
                "SELECT s.id, s.name, s.description, s.instructions,
                        u.username AS author_username, s.created_at, s.clone_count
                 FROM skills s JOIN users u ON u.id = s.user_id
                 WHERE s.is_public = 1
                   AND (s.name LIKE ? OR s.description LIKE ? OR s.instructions LIKE ?)
                 ORDER BY {order_by}
                 LIMIT ? OFFSET ?"
            ),
        );
        sqlx::query_as(&sql)
            .bind(&pattern)
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
                "SELECT s.id, s.name, s.description, s.instructions,
                        u.username AS author_username, s.created_at, s.clone_count
                 FROM skills s JOIN users u ON u.id = s.user_id
                 WHERE s.is_public = 1
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
        "SELECT name, description, instructions, user_id
             FROM skills WHERE id = ? AND is_public = 1",
    );
    let row: Option<(String, String, String, i64)> = match sqlx::query_as(&sel)
        .bind(id)
        .fetch_optional(&installed.pool)
        .await
    {
        Ok(r) => r,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let Some((name, description, instructions, author_id)) = row else {
        return err(StatusCode::NOT_FOUND, "public skill not found");
    };
    let inserted = match insert_skill(
        &installed.pool,
        installed.kind,
        user.id,
        &name,
        &description,
        &instructions,
    )
    .await
    {
        Ok(s) => s,
        Err(r) => return r,
    };
    if author_id != user.id {
        let bump = db::q(
            installed.kind,
            "UPDATE skills SET clone_count = clone_count + 1 WHERE id = ?",
        );
        let _ = sqlx::query(&bump).bind(id).execute(&installed.pool).await;
    }
    Json(inserted).into_response()
}

async fn update_skill(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(body): Json<UpdateSkill>,
) -> Response {
    if let Err(r) = skill_belongs(&installed.pool, installed.kind, id, user.id).await {
        return r;
    }

    let new_name = match body.name {
        Some(n) => match validate_name(&n) {
            Ok(v) => Some(v),
            Err(r) => return r,
        },
        None => None,
    };
    let new_desc = match body.description {
        Some(d) => match validate_description(&d) {
            Ok(v) => Some(v),
            Err(r) => return r,
        },
        None => None,
    };
    if let Some(ref c) = body.instructions {
        if let Err(r) = validate_instructions(c) {
            return r;
        }
    }

    if let Some(true) = body.is_public {
        let sel = db::q(
            installed.kind,
            "SELECT is_public FROM skills WHERE id = ? AND user_id = ?",
        );
        let current_is_public: Option<(i64,)> = sqlx::query_as(&sel)
            .bind(id)
            .bind(user.id)
            .fetch_optional(&installed.pool)
            .await
            .unwrap_or(None);
        let already = matches!(current_is_public, Some((v,)) if v != 0);
        if !already {
            let count = match count_user_public_skills(&installed.pool, installed.kind, user.id)
                .await
            {
                Ok(n) => n,
                Err(r) => return r,
            };
            if count >= MAX_PUBLIC_SKILLS_PER_USER {
                return err(
                    StatusCode::BAD_REQUEST,
                    format!("maximum {MAX_PUBLIC_SKILLS_PER_USER} public skills reached"),
                );
            }
        }
    }

    let now = db::now_expr(installed.kind);
    let sql = db::q(
        installed.kind,
        &format!(
            "UPDATE skills
             SET name = COALESCE(?, name),
                 description = COALESCE(?, description),
                 instructions = COALESCE(?, instructions),
                 is_public = COALESCE(?, is_public),
                 updated_at = {now}
             WHERE id = ? AND user_id = ?"
        ),
    );
    let r = sqlx::query(&sql)
        .bind(new_name)
        .bind(new_desc)
        .bind(body.instructions)
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

async fn delete_skill(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Response {
    let sql = db::q(
        installed.kind,
        "DELETE FROM skills WHERE id = ? AND user_id = ?",
    );
    let r = sqlx::query(&sql)
        .bind(id)
        .bind(user.id)
        .execute(&installed.pool)
        .await;
    match r {
        Ok(out) if out.rows_affected() > 0 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => err(StatusCode::NOT_FOUND, "skill not found"),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn list_conversation_skills(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(conv_id): Path<i64>,
) -> Response {
    if let Err(r) = conversation_belongs(&installed.pool, installed.kind, conv_id, user.id).await {
        return r;
    }
    let sql = db::q(
        installed.kind,
        "SELECT s.id, s.name, s.description, s.instructions, s.is_public, s.created_at, s.updated_at
             FROM conversation_skills cs
             JOIN skills s ON s.id = cs.skill_id
             WHERE cs.conversation_id = ? AND s.user_id = ?
             ORDER BY cs.created_at ASC, s.id ASC",
    );
    let rows: Result<Vec<Skill>, _> = sqlx::query_as(&sql)
        .bind(conv_id)
        .bind(user.id)
        .fetch_all(&installed.pool)
        .await;
    match rows {
        Ok(r) => Json(r).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn attach_skill(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(conv_id): Path<i64>,
    Json(body): Json<AttachSkill>,
) -> Response {
    if let Err(r) = conversation_belongs(&installed.pool, installed.kind, conv_id, user.id).await {
        return r;
    }
    if let Err(r) = skill_belongs(&installed.pool, installed.kind, body.skill_id, user.id).await {
        return r;
    }

    let count = match count_conv_skills(&installed.pool, installed.kind, conv_id).await {
        Ok(n) => n,
        Err(r) => return r,
    };

    // check if already attached (so duplicate attach is idempotent, doesn't count toward cap)
    let exists_sql = db::q(
        installed.kind,
        "SELECT 1 FROM conversation_skills WHERE conversation_id = ? AND skill_id = ?",
    );
    let exists: Option<(i32,)> = sqlx::query_as(&exists_sql)
        .bind(conv_id)
        .bind(body.skill_id)
        .fetch_optional(&installed.pool)
        .await
        .unwrap_or(None);
    if exists.is_some() {
        return StatusCode::NO_CONTENT.into_response();
    }

    if count >= MAX_SKILLS_PER_CONVERSATION {
        return err(
            StatusCode::BAD_REQUEST,
            format!("maximum {MAX_SKILLS_PER_CONVERSATION} skills per conversation"),
        );
    }

    let ins = db::q(
        installed.kind,
        "INSERT INTO conversation_skills (conversation_id, skill_id) VALUES (?, ?)",
    );
    let r = sqlx::query(&ins)
        .bind(conv_id)
        .bind(body.skill_id)
        .execute(&installed.pool)
        .await;
    match r {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn detach_skill(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path((conv_id, skill_id)): Path<(i64, i64)>,
) -> Response {
    if let Err(r) = conversation_belongs(&installed.pool, installed.kind, conv_id, user.id).await {
        return r;
    }
    let sql = db::q(
        installed.kind,
        "DELETE FROM conversation_skills WHERE conversation_id = ? AND skill_id = ?",
    );
    let r = sqlx::query(&sql)
        .bind(conv_id)
        .bind(skill_id)
        .execute(&installed.pool)
        .await;
    match r {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/skills", get(list_skills).post(create_skill))
        .route("/skills/public", get(list_public))
        .route("/skills/{id}/clone", post(clone_public))
        .route("/skills/{id}", patch(update_skill).delete(delete_skill))
        .route(
            "/conversations/{id}/skills",
            get(list_conversation_skills).post(attach_skill),
        )
        .route(
            "/conversations/{id}/skills/{skill_id}",
            delete(detach_skill),
        )
}
