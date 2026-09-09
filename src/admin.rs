use axum::{
    Extension, Json, Router,
    extract::{Path, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, patch, post},
};
use serde::{Deserialize, Serialize};

use crate::{
    AppState, CurrentUser, InstalledState, auth,
    db::{self, DbKind, Pool},
};

#[derive(Serialize)]
pub struct AdminUser {
    pub id: i64,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub email: Option<String>,
    pub is_admin: bool,
    pub created_at: String,
    pub conversations: i64,
    pub messages: i64,
    pub skills: i64,
}

#[derive(Serialize)]
pub struct AdminStats {
    pub users: i64,
    pub admins: i64,
    pub conversations: i64,
    pub messages: i64,
    pub skills: i64,
    pub public_skills: i64,
    pub prompts: i64,
    pub public_prompts: i64,
    pub library_assets: i64,
    pub sessions: i64,
}

#[derive(Serialize)]
pub struct AdminInviteRow {
    pub inviter_id: i64,
    pub inviter_username: String,
    pub inviter_code: Option<String>,
    pub invitee_id: i64,
    pub invitee_username: String,
    pub invitee_created_at: String,
}

#[derive(Serialize)]
pub struct AdminSystemInfo {
    pub version: &'static str,
    pub db_kind: &'static str,
    pub data_dir: String,
    pub config_path: String,
    pub bind_addr: String,
    pub images_dir_bytes: u64,
    pub storage_backend: &'static str,
    pub storage_location: String,
}

#[derive(Serialize)]
pub struct AdminStorageSettings {
    pub backend: String,
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    pub prefix: String,
    pub path_style: bool,
    pub access_key_id_set: bool,
    pub access_key_id_hint: Option<String>,
    pub secret_access_key_set: bool,
    pub session_token_set: bool,
    pub active_backend: &'static str,
    pub active_location: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct UpdateStorageSettings {
    pub backend: String,
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    pub prefix: String,
    pub path_style: bool,
    /// Empty credential fields keep their existing persisted values.
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: String,
    pub clear_session_token: bool,
}

#[derive(Deserialize)]
pub struct UpdateUser {
    pub is_admin: Option<bool>,
    pub display_name: Option<String>,
    pub password: Option<String>,
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, msg.into()).into_response()
}

pub async fn is_admin(pool: &Pool, kind: DbKind, user_id: i64) -> bool {
    let sql = db::q(kind, "SELECT is_admin FROM users WHERE id = ?");
    let row: Option<(i64,)> = sqlx::query_as(&sql)
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    matches!(row, Some((v,)) if v != 0)
}

pub async fn require_admin(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    req: axum::extract::Request,
    next: Next,
) -> Response {
    if !is_admin(&installed.pool, installed.kind, user.id).await {
        return (StatusCode::FORBIDDEN, "admin only").into_response();
    }
    next.run(req).await
}

async fn scalar_i64(pool: &Pool, kind: DbKind, sql: &str) -> i64 {
    let q = db::q(kind, sql);
    let row: Option<(i64,)> = sqlx::query_as(&q).fetch_optional(pool).await.ok().flatten();
    row.map(|(v,)| v).unwrap_or(0)
}

async fn get_stats(Extension(installed): Extension<InstalledState>) -> Response {
    let true_lit = db::bool_true(installed.kind);
    let stats = AdminStats {
        users: scalar_i64(&installed.pool, installed.kind, "SELECT COUNT(*) FROM users").await,
        admins: scalar_i64(
            &installed.pool,
            installed.kind,
            &format!("SELECT COUNT(*) FROM users WHERE is_admin = {true_lit}"),
        )
        .await,
        conversations: scalar_i64(
            &installed.pool,
            installed.kind,
            "SELECT COUNT(*) FROM conversations",
        )
        .await,
        messages: scalar_i64(&installed.pool, installed.kind, "SELECT COUNT(*) FROM messages").await,
        skills: scalar_i64(&installed.pool, installed.kind, "SELECT COUNT(*) FROM skills").await,
        public_skills: scalar_i64(
            &installed.pool,
            installed.kind,
            &format!("SELECT COUNT(*) FROM skills WHERE is_public = {true_lit}"),
        )
        .await,
        prompts: scalar_i64(&installed.pool, installed.kind, "SELECT COUNT(*) FROM prompts").await,
        public_prompts: scalar_i64(
            &installed.pool,
            installed.kind,
            &format!("SELECT COUNT(*) FROM prompts WHERE is_public = {true_lit}"),
        )
        .await,
        library_assets: scalar_i64(
            &installed.pool,
            installed.kind,
            "SELECT COUNT(*) FROM media_library_assets",
        )
        .await,
        sessions: scalar_i64(
            &installed.pool,
            installed.kind,
            "SELECT COUNT(*) FROM sessions",
        )
        .await,
    };
    Json(stats).into_response()
}

async fn list_users(Extension(installed): Extension<InstalledState>) -> Response {
    let admin_col = db::bool_as_int(installed.kind, "u.is_admin");
    let sql = db::q(
        installed.kind,
        &format!(
            "SELECT u.id, u.username, u.display_name, u.avatar_url, u.email, {admin_col}, u.created_at,
                    (SELECT COUNT(*) FROM conversations c WHERE c.user_id = u.id) AS convs,
                    (SELECT COUNT(*) FROM messages m
                        JOIN conversations c ON c.id = m.conversation_id
                        WHERE c.user_id = u.id) AS msgs,
                    (SELECT COUNT(*) FROM skills s WHERE s.user_id = u.id) AS skl
             FROM users u
             ORDER BY u.id ASC"
        ),
    );
    let rows: Result<
        Vec<(
            i64,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            i64,
            String,
            i64,
            i64,
            i64,
        )>,
        _,
    > = sqlx::query_as(&sql).fetch_all(&installed.pool).await;
    match rows {
        Ok(rs) => {
            let out: Vec<AdminUser> = rs
                .into_iter()
                .map(
                    |(
                        id,
                        username,
                        display_name,
                        avatar_url,
                        email,
                        is_admin,
                        created_at,
                        c,
                        m,
                        s,
                    )| {
                        AdminUser {
                            id,
                            username,
                            display_name,
                            avatar_url,
                            email,
                            is_admin: is_admin != 0,
                            created_at,
                            conversations: c,
                            messages: m,
                            skills: s,
                        }
                    },
                )
                .collect();
            Json(out).into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn count_admins(pool: &Pool, kind: DbKind) -> i64 {
    let true_lit = db::bool_true(kind);
    scalar_i64(
        pool,
        kind,
        &format!("SELECT COUNT(*) FROM users WHERE is_admin = {true_lit}"),
    )
    .await
}

async fn update_user(
    Extension(installed): Extension<InstalledState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(body): Json<UpdateUser>,
) -> Response {
    if body.is_admin.is_none() && body.display_name.is_none() && body.password.is_none() {
        return err(StatusCode::BAD_REQUEST, "nothing to update");
    }

    if let Some(false) = body.is_admin {
        if id == current.id {
            return err(StatusCode::BAD_REQUEST, "cannot demote yourself");
        }
        if count_admins(&installed.pool, installed.kind).await <= 1 {
            return err(StatusCode::BAD_REQUEST, "cannot demote the last admin");
        }
    }

    if let Some(ref pw) = body.password {
        if pw.len() < 6 || pw.len() > 256 {
            return err(StatusCode::BAD_REQUEST, "password must be 6-256 characters");
        }
    }

    if let Some(flag) = body.is_admin {
        let sql = db::q(installed.kind, "UPDATE users SET is_admin = ? WHERE id = ?");
        if let Err(e) = sqlx::query(&sql)
            .bind(flag)
            .bind(id)
            .execute(&installed.pool)
            .await
        {
            return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
    }

    if let Some(name) = body.display_name {
        let trimmed: String = name.chars().filter(|c| !c.is_control()).collect();
        let trimmed = trimmed.trim();
        let value = if trimmed.is_empty() { None } else { Some(trimmed.to_string()) };
        let sql = db::q(
            installed.kind,
            "UPDATE users SET display_name = ? WHERE id = ?",
        );
        if let Err(e) = sqlx::query(&sql)
            .bind(value)
            .bind(id)
            .execute(&installed.pool)
            .await
        {
            return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
    }

    if let Some(pw) = body.password {
        let hash = match auth::hash_password(&pw) {
            Ok(h) => h,
            Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
        };
        let sql = db::q(
            installed.kind,
            "UPDATE users SET password_hash = ? WHERE id = ?",
        );
        if let Err(e) = sqlx::query(&sql)
            .bind(&hash)
            .bind(id)
            .execute(&installed.pool)
            .await
        {
            return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
        let del = db::q(installed.kind, "DELETE FROM sessions WHERE user_id = ?");
        let _ = sqlx::query(&del)
            .bind(id)
            .execute(&installed.pool)
            .await;
    }

    StatusCode::NO_CONTENT.into_response()
}

async fn delete_user(
    Extension(installed): Extension<InstalledState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Response {
    if id == current.id {
        return err(StatusCode::BAD_REQUEST, "cannot delete yourself");
    }
    let admin_sql = db::q(installed.kind, "SELECT is_admin FROM users WHERE id = ?");
    let target_admin: Option<(i64,)> = sqlx::query_as(&admin_sql)
        .bind(id)
        .fetch_optional(&installed.pool)
        .await
        .ok()
        .flatten();
    let Some((flag,)) = target_admin else {
        return err(StatusCode::NOT_FOUND, "user not found");
    };
    if flag != 0 && count_admins(&installed.pool, installed.kind).await <= 1 {
        return err(StatusCode::BAD_REQUEST, "cannot delete the last admin");
    }

    let sql = db::q(installed.kind, "DELETE FROM users WHERE id = ?");
    match sqlx::query(&sql)
        .bind(id)
        .execute(&installed.pool)
        .await
    {
        Ok(r) if r.rows_affected() > 0 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => err(StatusCode::NOT_FOUND, "user not found"),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn list_invites(Extension(installed): Extension<InstalledState>) -> Response {
    let sql = db::q(
        installed.kind,
        "SELECT inviter.id, inviter.username, inviter.invite_code,
                invitee.id, invitee.username, invitee.created_at
         FROM users invitee
         JOIN users inviter ON inviter.id = invitee.invited_by
         ORDER BY invitee.created_at DESC, invitee.id DESC
         LIMIT 500",
    );
    let rows: Result<Vec<(i64, String, Option<String>, i64, String, String)>, _> =
        sqlx::query_as(&sql).fetch_all(&installed.pool).await;
    match rows {
        Ok(rs) => {
            let out: Vec<AdminInviteRow> = rs
                .into_iter()
                .map(
                    |(inviter_id, inviter_username, inviter_code, invitee_id, invitee_username, invitee_created_at)| {
                        AdminInviteRow {
                            inviter_id,
                            inviter_username,
                            inviter_code,
                            invitee_id,
                            invitee_username,
                            invitee_created_at,
                        }
                    },
                )
                .collect();
            Json(out).into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn get_system_info(
    State(state): State<AppState>,
    Extension(installed): Extension<InstalledState>,
) -> Response {
    let images_dir = state.data_dir.join("images");
    let images_dir_bytes = dir_size_bytes(&images_dir).await;

    let info = AdminSystemInfo {
        version: env!("CARGO_PKG_VERSION"),
        db_kind: installed.kind.as_str(),
        data_dir: state.data_dir.display().to_string(),
        config_path: state.config_path.display().to_string(),
        bind_addr: crate::runtime_env::var("YUNOVA_BIND").unwrap_or_else(|_| "127.0.0.1:3000".into()),
        images_dir_bytes,
        storage_backend: state.storage.backend_name(),
        storage_location: state.storage.location(),
    };
    Json(info).into_response()
}

fn trimmed(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn credential_hint(value: Option<&str>) -> Option<String> {
    value.map(|value| {
        let suffix: String = value
            .chars()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        format!("••••{suffix}")
    })
}

fn storage_settings(
    state: &AppState,
    config: &crate::storage::StorageConfig,
) -> AdminStorageSettings {
    let active_backend = state.storage.backend_name();
    AdminStorageSettings {
        backend: trimmed(&config.backend).unwrap_or_else(|| active_backend.to_string()),
        endpoint: config.endpoint.clone().unwrap_or_default(),
        region: config.region.clone().unwrap_or_else(|| "us-east-1".into()),
        bucket: config.bucket.clone().unwrap_or_default(),
        prefix: config.prefix.clone().unwrap_or_else(|| {
            if active_backend == "s3" {
                crate::storage::DEFAULT_S3_PREFIX.into()
            } else {
                "yunova".into()
            }
        }),
        path_style: config.path_style.unwrap_or(true),
        access_key_id_set: config
            .access_key_id
            .as_deref()
            .is_some_and(|value| !value.is_empty()),
        access_key_id_hint: credential_hint(config.access_key_id.as_deref()),
        secret_access_key_set: config
            .secret_access_key
            .as_deref()
            .is_some_and(|value| !value.is_empty()),
        session_token_set: config
            .session_token
            .as_deref()
            .is_some_and(|value| !value.is_empty()),
        active_backend,
        active_location: state.storage.location(),
    }
}

fn read_storage_config(state: &AppState) -> Result<crate::storage::StorageConfig, String> {
    if !state.config_path.exists() {
        return Ok(Default::default());
    }
    crate::setup::load_config(&state.config_path)
        .map(|config| config.storage.unwrap_or_default())
        .map_err(|error| format!("read config: {error}"))
}

fn read_config_for_update(state: &AppState) -> Result<crate::setup::StoredConfig, String> {
    if state.config_path.exists() {
        return crate::setup::load_config(&state.config_path)
            .map_err(|error| format!("read config: {error}"));
    }

    let database_url = crate::runtime_env::var("YUNOVA_DATABASE_URL")
        .ok()
        .or_else(|| std::env::var("DATABASE_URL").ok())
        .ok_or_else(|| "database configuration is missing".to_string())?;
    Ok(crate::setup::StoredConfig {
        database_url,
        storage: None,
    })
}

fn validate_setting_length(name: &str, value: &str, max: usize) -> Result<(), String> {
    if value.len() > max {
        Err(format!("{name} is too long"))
    } else {
        Ok(())
    }
}

fn merge_storage_settings(
    request: &UpdateStorageSettings,
    existing: Option<crate::storage::StorageConfig>,
) -> Result<crate::storage::StorageConfig, String> {
    for (name, value, max) in [
        ("endpoint", request.endpoint.as_str(), 2048),
        ("region", request.region.as_str(), 128),
        ("bucket", request.bucket.as_str(), 255),
        ("prefix", request.prefix.as_str(), 1024),
        ("access key id", request.access_key_id.as_str(), 1024),
        ("secret access key", request.secret_access_key.as_str(), 4096),
        ("session token", request.session_token.as_str(), 16 * 1024),
    ] {
        validate_setting_length(name, value, max)?;
    }

    let backend = request.backend.trim().to_ascii_lowercase();
    if backend != "local" && backend != "s3" {
        return Err("storage backend must be local or s3".into());
    }

    let mut config = existing.unwrap_or_default();
    config.backend = backend;
    config.endpoint = trimmed(&request.endpoint);
    config.region = trimmed(&request.region);
    config.bucket = trimmed(&request.bucket);
    config.prefix = trimmed(&request.prefix);
    config.path_style = Some(request.path_style);
    if let Some(value) = trimmed(&request.access_key_id) {
        config.access_key_id = Some(value);
    }
    if let Some(value) = trimmed(&request.secret_access_key) {
        config.secret_access_key = Some(value);
    }
    if request.clear_session_token {
        config.session_token = None;
    } else if let Some(value) = trimmed(&request.session_token) {
        config.session_token = Some(value);
    }
    Ok(config)
}

async fn get_storage_settings(State(state): State<AppState>) -> Response {
    match read_storage_config(&state) {
        Ok(config) => Json(storage_settings(&state, &config)).into_response(),
        Err(error) => err(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

async fn test_storage_settings(
    State(state): State<AppState>,
    Json(request): Json<UpdateStorageSettings>,
) -> Response {
    let existing = match read_storage_config(&state) {
        Ok(config) => Some(config),
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error),
    };
    let config = match merge_storage_settings(&request, existing) {
        Ok(config) => config,
        Err(error) => return err(StatusCode::BAD_REQUEST, error),
    };
    let candidate = match crate::storage::MediaStorage::from_stored_config(
        state.data_dir.clone(),
        Some(&config),
    ) {
        Ok(storage) => storage,
        Err(error) => return err(StatusCode::BAD_REQUEST, error),
    };
    match candidate.test_connection().await {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(error) => err(StatusCode::BAD_GATEWAY, error.to_string()),
    }
}

async fn update_storage_settings(
    State(state): State<AppState>,
    Json(request): Json<UpdateStorageSettings>,
) -> Response {
    let _guard = state.config_lock.lock().await;
    let mut stored = match read_config_for_update(&state) {
        Ok(config) => config,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error),
    };
    let config = match merge_storage_settings(&request, stored.storage.clone()) {
        Ok(config) => config,
        Err(error) => return err(StatusCode::BAD_REQUEST, error),
    };
    let candidate = match crate::storage::MediaStorage::from_stored_config(
        state.data_dir.clone(),
        Some(&config),
    ) {
        Ok(storage) => storage,
        Err(error) => return err(StatusCode::BAD_REQUEST, error),
    };
    if let Err(error) = candidate.test_connection().await {
        return err(StatusCode::BAD_GATEWAY, error.to_string());
    }

    stored.storage = Some(config.clone());
    if let Err(error) = crate::setup::save_config(&state.config_path, &stored) {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("write config: {error}"),
        );
    }
    state.storage.replace_with(&candidate);
    Json(storage_settings(&state, &config)).into_response()
}

async fn dir_size_bytes(path: &std::path::Path) -> u64 {
    let mut total: u64 = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(p) = stack.pop() {
        let Ok(mut rd) = tokio::fs::read_dir(&p).await else {
            continue;
        };
        while let Ok(Some(entry)) = rd.next_entry().await {
            let Ok(meta) = entry.metadata().await else {
                continue;
            };
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                total = total.saturating_add(meta.len());
            }
        }
    }
    total
}

async fn prune_sessions(Extension(installed): Extension<InstalledState>) -> Response {
    let sql = db::q(
        installed.kind,
        "DELETE FROM sessions WHERE expires_at < ?",
    );
    let now = chrono::Utc::now().to_rfc3339();
    match sqlx::query(&sql)
        .bind(&now)
        .execute(&installed.pool)
        .await
    {
        Ok(r) => Json(serde_json::json!({ "removed": r.rows_affected() })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/admin/stats", get(get_stats))
        .route("/admin/system", get(get_system_info))
        .route(
            "/admin/storage",
            get(get_storage_settings).put(update_storage_settings),
        )
        .route("/admin/storage/test", post(test_storage_settings))
        .route("/admin/users", get(list_users))
        .route("/admin/users/{id}", patch(update_user).delete(delete_user))
        .route("/admin/invites", get(list_invites))
        .route("/admin/sessions/prune", post(prune_sessions))
        .route_layer(middleware::from_fn(require_admin))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_update_keeps_masked_credentials_when_inputs_are_blank() {
        let existing = crate::storage::StorageConfig {
            backend: "s3".into(),
            access_key_id: Some("existing-key".into()),
            secret_access_key: Some("existing-secret".into()),
            session_token: Some("existing-token".into()),
            ..Default::default()
        };
        let request = UpdateStorageSettings {
            backend: "s3".into(),
            endpoint: " https://objects.example.com ".into(),
            region: " auto ".into(),
            bucket: " media ".into(),
            prefix: " yunova ".into(),
            path_style: true,
            ..Default::default()
        };

        let merged = merge_storage_settings(&request, Some(existing)).unwrap();
        assert_eq!(merged.endpoint.as_deref(), Some("https://objects.example.com"));
        assert_eq!(merged.region.as_deref(), Some("auto"));
        assert_eq!(merged.bucket.as_deref(), Some("media"));
        assert_eq!(merged.prefix.as_deref(), Some("yunova"));
        assert_eq!(merged.access_key_id.as_deref(), Some("existing-key"));
        assert_eq!(merged.secret_access_key.as_deref(), Some("existing-secret"));
        assert_eq!(merged.session_token.as_deref(), Some("existing-token"));
        assert_eq!(merged.path_style, Some(true));
    }

    #[test]
    fn storage_update_can_replace_credentials_and_clear_session_token() {
        let existing = crate::storage::StorageConfig {
            session_token: Some("old-token".into()),
            ..Default::default()
        };
        let request = UpdateStorageSettings {
            backend: "s3".into(),
            access_key_id: "new-key".into(),
            secret_access_key: "new-secret".into(),
            session_token: "ignored-token".into(),
            clear_session_token: true,
            ..Default::default()
        };

        let merged = merge_storage_settings(&request, Some(existing)).unwrap();
        assert_eq!(merged.access_key_id.as_deref(), Some("new-key"));
        assert_eq!(merged.secret_access_key.as_deref(), Some("new-secret"));
        assert!(merged.session_token.is_none());
    }

    #[test]
    fn storage_update_rejects_unknown_backend() {
        let request = UpdateStorageSettings {
            backend: "ftp".into(),
            ..Default::default()
        };
        assert!(merge_storage_settings(&request, None).is_err());
    }
}
