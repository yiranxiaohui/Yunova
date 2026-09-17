use axum::{
    Extension, Json, Router,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use rand::RngCore;
use serde::Deserialize;

use crate::{AppState, CurrentUser, InstalledState, UserDto, auth, db, storage::MediaKind};

pub const MAX_DISPLAY_NAME: usize = 64;
pub const MAX_AVATAR_BYTES: usize = 2 * 1024 * 1024;

#[derive(Deserialize)]
pub struct UpdateProfile {
    pub display_name: Option<String>,
}

#[derive(Deserialize)]
pub struct ChangePassword {
    pub current_password: String,
    pub new_password: String,
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, msg.into()).into_response()
}

fn normalize_display_name(v: &str) -> Result<Option<String>, Response> {
    let cleaned: String = v.chars().filter(|c| !c.is_control()).collect();
    let trimmed = cleaned.trim();
    if trimmed.chars().count() > MAX_DISPLAY_NAME {
        return Err(err(StatusCode::BAD_REQUEST, "display_name too long"));
    }
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed.to_string()))
    }
}

async fn load_user_dto(installed: &InstalledState, user_id: i64) -> Result<UserDto, Response> {
    let sql = db::q(
        installed.kind,
        "SELECT id, username, display_name, avatar_url, is_admin
             FROM users WHERE id = ?",
    );
    let row: Option<(i64, String, Option<String>, Option<String>, i64)> = sqlx::query_as(&sql)
        .bind(user_id)
        .fetch_optional(&installed.pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    match row {
        Some((id, username, display_name, avatar_url, is_admin)) => Ok(UserDto {
            id,
            username,
            display_name,
            avatar_url,
            is_admin: is_admin != 0,
        }),
        None => Err(err(StatusCode::NOT_FOUND, "user not found")),
    }
}

async fn get_profile(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
) -> Response {
    match load_user_dto(&installed, user.id).await {
        Ok(dto) => Json(dto).into_response(),
        Err(r) => r,
    }
}

async fn update_profile(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<UpdateProfile>,
) -> Response {
    if let Some(raw) = body.display_name {
        let value = match normalize_display_name(&raw) {
            Ok(v) => v,
            Err(r) => return r,
        };
        let sql = db::q(
            installed.kind,
            "UPDATE users SET display_name = ? WHERE id = ?",
        );
        if let Err(e) = sqlx::query(&sql)
            .bind(value)
            .bind(user.id)
            .execute(&installed.pool)
            .await
        {
            return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
    }

    match load_user_dto(&installed, user.id).await {
        Ok(dto) => Json(dto).into_response(),
        Err(r) => r,
    }
}

async fn change_password(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<ChangePassword>,
) -> Response {
    if body.new_password.len() < 6 || body.new_password.len() > 256 {
        return err(
            StatusCode::BAD_REQUEST,
            "new password must be 6-256 characters",
        );
    }

    let sel = db::q(
        installed.kind,
        "SELECT password_hash FROM users WHERE id = ?",
    );
    let row: Option<(String,)> = sqlx::query_as(&sel)
        .bind(user.id)
        .fetch_optional(&installed.pool)
        .await
        .unwrap_or(None);
    let Some((phc,)) = row else {
        return err(StatusCode::UNAUTHORIZED, "user not found");
    };
    if !auth::verify_password(&body.current_password, &phc) {
        return err(StatusCode::UNAUTHORIZED, "current password incorrect");
    }
    let new_hash = match auth::hash_password(&body.new_password) {
        Ok(h) => h,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let sql = db::q(
        installed.kind,
        "UPDATE users SET password_hash = ? WHERE id = ?",
    );
    match sqlx::query(&sql)
        .bind(&new_hash)
        .bind(user.id)
        .execute(&installed.pool)
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

fn random_hex(n: usize) -> String {
    let mut bytes = vec![0u8; n];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn ext_for_image_mime(mime: &str) -> Option<&'static str> {
    match mime.split(';').next().map(|s| s.trim().to_ascii_lowercase()) {
        Some(m) => match m.as_str() {
            "image/png" => Some("png"),
            "image/jpeg" | "image/jpg" => Some("jpg"),
            "image/webp" => Some("webp"),
            "image/gif" => Some("gif"),
            _ => None,
        },
        None => None,
    }
}

async fn upload_avatar(
    State(state): State<AppState>,
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let mime = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let Some(ext) = ext_for_image_mime(mime) else {
        return err(
            StatusCode::BAD_REQUEST,
            "unsupported image type (use png, jpeg, webp, or gif)",
        );
    };
    if body.is_empty() {
        return err(StatusCode::BAD_REQUEST, "empty upload");
    }
    if body.len() > MAX_AVATAR_BYTES {
        return err(StatusCode::PAYLOAD_TOO_LARGE, "avatar exceeds 2MB limit");
    }

    let filename = format!("{}.{ext}", random_hex(16));
    if let Err(e) = state.storage.put(MediaKind::Avatar, &filename, body.to_vec()).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, format!("write: {e}"));
    }

    let prev_sql = db::q(installed.kind, "SELECT avatar_url FROM users WHERE id = ?");
    let prev: Option<(Option<String>,)> = sqlx::query_as(&prev_sql)
        .bind(user.id)
        .fetch_optional(&installed.pool)
        .await
        .unwrap_or(None);

    let new_url = format!("/api/avatars/{filename}");
    let upd = db::q(installed.kind, "UPDATE users SET avatar_url = ? WHERE id = ?");
    if let Err(e) = sqlx::query(&upd)
        .bind(&new_url)
        .bind(user.id)
        .execute(&installed.pool)
        .await
    {
        let _ = state.storage.delete(MediaKind::Avatar, &filename).await;
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }

    if let Some((Some(prev_url),)) = prev {
        if let Some(name) = prev_url.strip_prefix("/api/avatars/") {
            if !name.contains('/') && !name.contains('\\') && !name.contains("..") {
                let _ = state.storage.delete(MediaKind::Avatar, name).await;
            }
        }
    }

    match load_user_dto(&installed, user.id).await {
        Ok(dto) => Json(dto).into_response(),
        Err(r) => r,
    }
}

async fn serve_avatar(State(state): State<AppState>, Path(name): Path<String>) -> Response {
    if name.contains("..") || name.contains('/') || name.contains('\\') {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let bytes = match state.storage.get(MediaKind::Avatar, &name).await {
        Ok(b) => b,
        Err(e) if e.is_not_found() => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            eprintln!("[storage] serve avatar {name}: {e}");
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
        .route("/profile", get(get_profile).put(update_profile))
        .route("/profile/password", post(change_password))
        .route("/profile/avatar", post(upload_avatar))
        .route("/avatars/{name}", get(serve_avatar))
}
