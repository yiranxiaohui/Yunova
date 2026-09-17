use axum::{
    Extension, Json, Router,
    extract::Query,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};

use crate::{AppState, CurrentUser, InstalledState, db};

#[derive(Deserialize)]
pub struct SearchParams {
    pub q: String,
}

#[derive(Serialize)]
pub struct SearchHit {
    pub conversation_id: i64,
    pub conversation_title: String,
    pub message_id: Option<i64>,
    pub role: Option<String>,
    pub snippet: String,
    pub created_at: String,
    pub kind: &'static str,
}

const MAX_TITLE_HITS: i64 = 20;
const MAX_CONTENT_HITS: i64 = 50;
const MIN_QUERY_CHARS: usize = 2;
const MAX_QUERY_CHARS: usize = 200;
const SNIPPET_BEFORE: usize = 40;
const SNIPPET_AFTER: usize = 120;

/// Escape LIKE wildcards (`%` `_`) and the escape sentinel itself (`#`) so
/// user input is matched literally. Pair with `LIKE ? ESCAPE '#'` in SQL.
/// Sentinel `#` is chosen instead of `\` to avoid backslash-escape quirks.
fn escape_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for c in s.chars() {
        if matches!(c, '#' | '%' | '_') {
            out.push('#');
        }
        out.push(c);
    }
    out
}

/// Build a short snippet around the first case-insensitive occurrence of
/// `needle` in `content`, with leading/trailing ellipsis when truncated.
/// Newlines/tabs collapse to single spaces for one-line display.
fn build_snippet(content: &str, needle: &str) -> String {
    let lower_content = content.to_lowercase();
    let lower_needle = needle.to_lowercase();
    let total_chars = content.chars().count();

    let start_char = match lower_content.find(&lower_needle) {
        Some(byte_idx) => content[..byte_idx].chars().count().saturating_sub(SNIPPET_BEFORE),
        None => 0,
    };
    let take_chars = SNIPPET_BEFORE + needle.chars().count() + SNIPPET_AFTER;
    let end_char = (start_char + take_chars).min(total_chars);

    let raw: String = content
        .chars()
        .skip(start_char)
        .take(end_char - start_char)
        .map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c })
        .collect();

    let mut snippet = String::with_capacity(raw.len() + 6);
    if start_char > 0 {
        snippet.push('…');
    }
    snippet.push_str(&raw);
    if end_char < total_chars {
        snippet.push('…');
    }
    snippet
}

async fn search_conversations(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Query(params): Query<SearchParams>,
) -> Response {
    let q = params.q.trim();
    let q_len = q.chars().count();
    if q_len < MIN_QUERY_CHARS {
        return (StatusCode::BAD_REQUEST, "搜索关键词至少 2 个字符").into_response();
    }
    if q_len > MAX_QUERY_CHARS {
        return (StatusCode::BAD_REQUEST, "搜索关键词过长").into_response();
    }

    let needle = q.to_string();
    let like_pat = format!("%{}%", escape_like(&needle.to_lowercase()));

    let title_sql = db::q(
        installed.kind,
        "SELECT id, title, updated_at FROM conversations
         WHERE user_id = ? AND LOWER(title) LIKE ? ESCAPE '#'
         ORDER BY updated_at DESC LIMIT ?",
    );
    let title_rows: Vec<(i64, String, String)> = match sqlx::query_as(&title_sql)
        .bind(user.id)
        .bind(&like_pat)
        .bind(MAX_TITLE_HITS)
        .fetch_all(&installed.pool)
        .await
    {
        Ok(r) => r,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let content_sql = db::q(
        installed.kind,
        "SELECT m.id, m.role, m.content, m.created_at, c.id, c.title
         FROM messages m
         JOIN conversations c ON c.id = m.conversation_id
         WHERE c.user_id = ? AND LOWER(m.content) LIKE ? ESCAPE '#'
         ORDER BY m.created_at DESC LIMIT ?",
    );
    let content_rows: Vec<(i64, String, String, String, i64, String)> =
        match sqlx::query_as(&content_sql)
            .bind(user.id)
            .bind(&like_pat)
            .bind(MAX_CONTENT_HITS)
            .fetch_all(&installed.pool)
            .await
        {
            Ok(r) => r,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };

    let mut hits: Vec<SearchHit> = Vec::with_capacity(title_rows.len() + content_rows.len());
    for (cid, title, updated_at) in title_rows {
        hits.push(SearchHit {
            conversation_id: cid,
            conversation_title: title.clone(),
            message_id: None,
            role: None,
            snippet: title,
            created_at: updated_at,
            kind: "title",
        });
    }
    for (mid, role, content, created_at, cid, c_title) in content_rows {
        let snippet = build_snippet(&content, &needle);
        hits.push(SearchHit {
            conversation_id: cid,
            conversation_title: c_title,
            message_id: Some(mid),
            role: Some(role),
            snippet,
            created_at,
            kind: "content",
        });
    }
    Json(hits).into_response()
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/search/conversations", get(search_conversations))
}
