use axum::{
    Extension, Json, Router,
    extract::{Path, Query},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, patch},
};
use serde::{Deserialize, Serialize};

use crate::{AppState, CurrentUser, InstalledState, db::{self, DbKind, Pool}, prompts};

#[derive(Serialize, sqlx::FromRow)]
pub struct Conversation {
    pub id: i64,
    pub title: String,
    pub system_prompt: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct Message {
    pub id: i64,
    pub role: String,
    pub content: String,
    /// 推理模型的思考过程。只有 assistant 消息可能有值。
    pub reasoning: Option<String>,
    /// 思考耗时（毫秒），用于折叠标题里的「已思考 Ns」。
    pub reasoning_ms: Option<i64>,
    pub created_at: String,
}

#[derive(Deserialize)]
pub struct NewConversation {
    pub title: Option<String>,
    pub system_prompt: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateConversation {
    pub title: Option<String>,
    pub system_prompt: Option<String>,
}

#[derive(Deserialize)]
pub struct AppendMessages {
    pub messages: Vec<NewMessage>,
}

#[derive(Deserialize)]
pub struct TruncateQuery {
    pub from: i64,
}

#[derive(Deserialize)]
pub struct NewMessage {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub reasoning: Option<String>,
    #[serde(default)]
    pub reasoning_ms: Option<i64>,
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, msg.into()).into_response()
}

async fn ensure_owner(
    pool: &Pool,
    kind: DbKind,
    conv_id: i64,
    user_id: i64,
) -> Result<(), Response> {
    let sql = db::q(kind, "SELECT 1 FROM conversations WHERE id = ? AND user_id = ?");
    let row: Option<(i32,)> = sqlx::query_as(&sql)
        .bind(conv_id)
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    row.ok_or_else(|| err(StatusCode::NOT_FOUND, "conversation not found"))
        .map(|_| ())
}

fn sanitize_title(input: &str, fallback: &str) -> String {
    let t: String = input.chars().filter(|c| !c.is_control()).collect();
    let t = t.trim();
    if t.is_empty() {
        fallback.to_string()
    } else {
        let mut out: String = t.chars().take(80).collect();
        if out.chars().count() < t.chars().count() {
            out.push('…');
        }
        out
    }
}

async fn list_conversations(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
) -> Response {
    let sql = db::q(
        installed.kind,
        "SELECT id, title, system_prompt, created_at, updated_at FROM conversations
         WHERE user_id = ? ORDER BY updated_at DESC",
    );
    let rows: Result<Vec<Conversation>, _> = sqlx::query_as(&sql)
        .bind(user.id)
        .fetch_all(&installed.pool)
        .await;
    match rows {
        Ok(r) => Json(r).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn create_conversation(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<NewConversation>,
) -> Response {
    let title = sanitize_title(body.title.as_deref().unwrap_or(""), "新会话");
    let system_prompt = match body.system_prompt {
        Some(s) => s,
        None => prompts::default_system_prompt(&installed.pool, installed.kind, user.id)
            .await
            .unwrap_or_default(),
    };
    let base_insert = db::q(
        installed.kind,
        "INSERT INTO conversations (user_id, title, system_prompt) VALUES (?, ?, ?)",
    );

    let id = match installed.kind {
        DbKind::Sqlite | DbKind::Postgres => {
            let row: Result<(i64,), _> = sqlx::query_as(&format!("{base_insert} RETURNING id"))
                .bind(user.id)
                .bind(&title)
                .bind(&system_prompt)
                .fetch_one(&installed.pool)
                .await;
            match row {
                Ok((v,)) => v,
                Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
            }
        }
        DbKind::Mysql => {
            let mut tx = match installed.pool.begin().await {
                Ok(t) => t,
                Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
            };
            if let Err(e) = sqlx::query(&base_insert)
                .bind(user.id)
                .bind(&title)
                .bind(&system_prompt)
                .execute(&mut *tx)
                .await
            {
                return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
            }
            let row: Result<(i64,), _> = sqlx::query_as("SELECT LAST_INSERT_ID()")
                .fetch_one(&mut *tx)
                .await;
            let id = match row {
                Ok((v,)) => v,
                Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
            };
            if let Err(e) = tx.commit().await {
                return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
            }
            id
        }
    };

    let select_sql = db::q(
        installed.kind,
        "SELECT id, title, system_prompt, created_at, updated_at FROM conversations WHERE id = ?",
    );
    let row: Result<Conversation, _> = sqlx::query_as(&select_sql)
        .bind(id)
        .fetch_one(&installed.pool)
        .await;
    match row {
        Ok(c) => Json(c).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn update_conversation(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(body): Json<UpdateConversation>,
) -> Response {
    if let Err(r) = ensure_owner(&installed.pool, installed.kind, id, user.id).await {
        return r;
    }
    if body.title.is_none() && body.system_prompt.is_none() {
        return err(StatusCode::BAD_REQUEST, "nothing to update");
    }
    let title = body.title.as_deref().map(|t| sanitize_title(t, "新会话"));
    if let Some(sp) = body.system_prompt.as_ref() {
        if sp.len() > 32_000 {
            return err(StatusCode::BAD_REQUEST, "system_prompt too long");
        }
    }
    let now = db::now_expr(installed.kind);
    let sql = db::q(
        installed.kind,
        &format!(
            "UPDATE conversations
             SET title = COALESCE(?, title),
                 system_prompt = COALESCE(?, system_prompt),
                 updated_at = {now}
             WHERE id = ? AND user_id = ?"
        ),
    );
    let r = sqlx::query(&sql)
        .bind(title)
        .bind(body.system_prompt)
        .bind(id)
        .bind(user.id)
        .execute(&installed.pool)
        .await;
    match r {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn delete_conversation(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Response {
    let sql = db::q(
        installed.kind,
        "DELETE FROM conversations WHERE id = ? AND user_id = ?",
    );
    let r = sqlx::query(&sql)
        .bind(id)
        .bind(user.id)
        .execute(&installed.pool)
        .await;
    match r {
        Ok(out) if out.rows_affected() > 0 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => err(StatusCode::NOT_FOUND, "conversation not found"),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn delete_all_conversations(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
) -> Response {
    let sql = db::q(installed.kind, "DELETE FROM conversations WHERE user_id = ?");
    let r = sqlx::query(&sql)
        .bind(user.id)
        .execute(&installed.pool)
        .await;
    match r {
        Ok(out) => Json(serde_json::json!({ "deleted": out.rows_affected() })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn list_messages(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Response {
    if let Err(r) = ensure_owner(&installed.pool, installed.kind, id, user.id).await {
        return r;
    }
    let sql = db::q(
        installed.kind,
        "SELECT id, role, content, reasoning, reasoning_ms, created_at FROM messages
         WHERE conversation_id = ? ORDER BY id ASC",
    );
    let rows: Result<Vec<Message>, _> = sqlx::query_as(&sql)
        .bind(id)
        .fetch_all(&installed.pool)
        .await;
    match rows {
        Ok(r) => Json(r).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn append_messages(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(body): Json<AppendMessages>,
) -> Response {
    if let Err(r) = ensure_owner(&installed.pool, installed.kind, id, user.id).await {
        return r;
    }
    if body.messages.is_empty() {
        return err(StatusCode::BAD_REQUEST, "messages is empty");
    }
    for m in &body.messages {
        if !matches!(m.role.as_str(), "system" | "user" | "assistant") {
            return err(StatusCode::BAD_REQUEST, "invalid role");
        }
        if m.content.is_empty() {
            return err(StatusCode::BAD_REQUEST, "content is empty");
        }
        if m.reasoning.as_ref().is_some_and(|r| r.len() > 200_000) {
            return err(StatusCode::BAD_REQUEST, "reasoning too long");
        }
    }

    let mut tx = match installed.pool.begin().await {
        Ok(t) => t,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    let check_sql = db::q(
        installed.kind,
        "SELECT title, (SELECT COUNT(*) FROM messages WHERE conversation_id = ?) FROM conversations WHERE id = ?",
    );
    let existing: Option<(String, i64)> = sqlx::query_as(&check_sql)
        .bind(id)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .unwrap_or(None);

    let should_auto_title = matches!(&existing, Some((t, n)) if t == "新会话" && *n == 0);

    let insert_msg_sql = db::q(
        installed.kind,
        "INSERT INTO messages (conversation_id, role, content, reasoning, reasoning_ms)
         VALUES (?, ?, ?, ?, ?)",
    );
    for m in &body.messages {
        // 思考过程只对 assistant 有意义，其他角色一律落 NULL。
        let reasoning = if m.role == "assistant" {
            m.reasoning.as_deref().filter(|r| !r.is_empty())
        } else {
            None
        };
        let reasoning_ms = reasoning.and(m.reasoning_ms).filter(|v| *v >= 0);
        if let Err(e) = sqlx::query(&insert_msg_sql)
            .bind(id)
            .bind(&m.role)
            .bind(&m.content)
            .bind(reasoning)
            .bind(reasoning_ms)
            .execute(&mut *tx)
            .await
        {
            return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
    }

    if should_auto_title {
        if let Some(first_user) = body.messages.iter().find(|m| m.role == "user") {
            let t = sanitize_title(&first_user.content, "新会话");
            let sql =
                db::q(installed.kind, "UPDATE conversations SET title = ? WHERE id = ?");
            let _ = sqlx::query(&sql)
                .bind(t)
                .bind(id)
                .execute(&mut *tx)
                .await;
        }
    }

    let now = db::now_expr(installed.kind);
    let touch_sql = db::q(
        installed.kind,
        &format!("UPDATE conversations SET updated_at = {now} WHERE id = ?"),
    );
    let _ = sqlx::query(&touch_sql).bind(id).execute(&mut *tx).await;

    if let Err(e) = tx.commit().await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn truncate_messages(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Query(q): Query<TruncateQuery>,
) -> Response {
    if let Err(r) = ensure_owner(&installed.pool, installed.kind, id, user.id).await {
        return r;
    }
    if q.from <= 0 {
        return err(StatusCode::BAD_REQUEST, "from must be > 0");
    }
    let now = db::now_expr(installed.kind);
    let sql = db::q(
        installed.kind,
        "DELETE FROM messages WHERE conversation_id = ? AND id >= ?",
    );
    let r = sqlx::query(&sql)
        .bind(id)
        .bind(q.from)
        .execute(&installed.pool)
        .await;
    if let Err(e) = r {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    let touch = db::q(
        installed.kind,
        &format!("UPDATE conversations SET updated_at = {now} WHERE id = ?"),
    );
    let _ = sqlx::query(&touch)
        .bind(id)
        .execute(&installed.pool)
        .await;
    StatusCode::NO_CONTENT.into_response()
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/conversations",
            get(list_conversations)
                .post(create_conversation)
                .delete(delete_all_conversations),
        )
        .route(
            "/conversations/{id}",
            patch(update_conversation).delete(delete_conversation),
        )
        .route(
            "/conversations/{id}/messages",
            get(list_messages).post(append_messages).delete(truncate_messages),
        )
}
