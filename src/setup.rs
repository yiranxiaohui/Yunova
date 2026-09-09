use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{AppState, InstalledState, auth, db};

// ---------------------------------------------------------------------------
// persisted config
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize, Deserialize)]
pub struct StoredConfig {
    pub database_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<crate::storage::StorageConfig>,
}

/// Reuse an existing configuration so an upgrade retains its database path.
pub fn config_path(data_dir: &Path, explicit: Option<&str>) -> std::path::PathBuf {
    if let Some(path) = explicit {
        return path.into();
    }
    let current = data_dir.join("yunova.toml");
    let legacy = data_dir.join("novachat.toml");
    if !current.exists() && legacy.exists() {
        legacy
    } else {
        current
    }
}

pub fn load_config(path: &Path) -> Result<StoredConfig, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    toml::from_str(&text).map_err(|e| e.to_string())
}

pub fn save_config(path: &Path, cfg: &StoredConfig) -> Result<(), String> {
    let body = toml::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("yunova.toml");
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp_path = path.with_file_name(format!(
        ".{file_name}.tmp-{}-{nonce}",
        std::process::id()
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let write_result = (|| -> Result<(), String> {
        let mut file = options.open(&temp_path).map_err(|e| e.to_string())?;
        file.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        std::fs::rename(&temp_path, path).map_err(|e| e.to_string())?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    write_result
}

// ---------------------------------------------------------------------------
// boot helpers
// ---------------------------------------------------------------------------

pub async fn boot_installed(url: &str) -> Result<InstalledState, String> {
    let kind = db::DbKind::from_url(url)
        .ok_or_else(|| "unsupported database url scheme".to_string())?;
    let pool = db::connect(url).await.map_err(|e| e.to_string())?;
    db::migrate(&pool, kind).await.map_err(|e| e.to_string())?;
    Ok(InstalledState { pool, kind })
}

// ---------------------------------------------------------------------------
// request / response shapes
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct StatusResponse {
    pub installed: bool,
    pub supported: Vec<&'static str>,
}

#[derive(Deserialize)]
pub struct ConnectionForm {
    pub kind: String,
    // for sqlite: relative file path (e.g. "yunova.db")
    // for mysql/postgres: host, port, user, password, database, with tls optional
    pub sqlite_path: Option<String>,

    pub host: Option<String>,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub password: Option<String>,
    pub database: Option<String>,
    pub tls: Option<bool>,
}

#[derive(Deserialize)]
pub struct InstallRequest {
    pub connection: ConnectionForm,
    pub admin_username: String,
    pub admin_password: String,
}

#[derive(Serialize)]
pub struct InstallResponse {
    pub ok: bool,
    pub admin_id: i64,
}

fn err(s: StatusCode, m: impl Into<String>) -> Response {
    (s, m.into()).into_response()
}

// ---------------------------------------------------------------------------
// url builder
// ---------------------------------------------------------------------------

fn build_url(form: &ConnectionForm, data_dir: &Path) -> Result<(db::DbKind, String), String> {
    match form.kind.as_str() {
        "sqlite" => {
            let raw = form
                .sqlite_path
                .as_deref()
                .unwrap_or("yunova.db")
                .trim();
            if raw.is_empty() {
                return Err("sqlite_path is empty".into());
            }
            if raw.contains("..") {
                return Err("invalid sqlite path".into());
            }
            let candidate = std::path::Path::new(raw);
            // absolute paths are used as-is; relative paths resolve under data_dir
            let full = if candidate.is_absolute() {
                candidate.to_path_buf()
            } else {
                data_dir.join(candidate)
            };
            if let Some(parent) = full.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("create sqlite dir: {e}"))?;
                }
            }
            let path_str = full
                .to_str()
                .ok_or_else(|| "sqlite path is not valid UTF-8".to_string())?
                .replace('\\', "/");
            Ok((db::DbKind::Sqlite, format!("sqlite://{path_str}")))
        }
        "mysql" | "postgres" => {
            let host = form.host.as_deref().unwrap_or("localhost");
            let default_port = if form.kind == "mysql" { 3306 } else { 5432 };
            let port = form.port.unwrap_or(default_port);
            let user = form.user.as_deref().unwrap_or("");
            let password = form.password.as_deref().unwrap_or("");
            let database = form.database.as_deref().unwrap_or("");
            if database.is_empty() {
                return Err("database name is required".into());
            }
            let scheme = if form.kind == "mysql" { "mysql" } else { "postgres" };
            let user_enc = percent_encode(user);
            let pass_enc = percent_encode(password);
            let auth = if user.is_empty() {
                String::new()
            } else if password.is_empty() {
                format!("{user_enc}@")
            } else {
                format!("{user_enc}:{pass_enc}@")
            };
            let mut query = String::new();
            if form.tls.unwrap_or(false) {
                if form.kind == "postgres" {
                    query.push_str("?sslmode=require");
                } else {
                    query.push_str("?ssl-mode=REQUIRED");
                }
            }
            let url = format!("{scheme}://{auth}{host}:{port}/{database}{query}");
            let kind = db::DbKind::from_url(&url).unwrap();
            Ok((kind, url))
        }
        other => Err(format!("unsupported db kind: {other}")),
    }
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// handlers
// ---------------------------------------------------------------------------

async fn status(State(state): State<AppState>) -> Json<StatusResponse> {
    Json(StatusResponse {
        installed: state.installed.read().await.is_some(),
        supported: vec!["sqlite", "mysql", "postgres"],
    })
}

async fn test_connection(
    State(state): State<AppState>,
    Json(form): Json<ConnectionForm>,
) -> Response {
    if state.installed.read().await.is_some() {
        return err(StatusCode::FORBIDDEN, "already installed");
    }
    if state.config_path.exists() {
        return err(StatusCode::FORBIDDEN, "already configured; setup is locked");
    }
    let (_kind, url) = match build_url(&form, &state.data_dir) {
        Ok(v) => v,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    match db::connect(&url).await {
        Ok(pool) => {
            let _ = pool.close().await;
            (StatusCode::OK, "ok").into_response()
        }
        Err(e) => err(StatusCode::BAD_GATEWAY, e.to_string()),
    }
}

async fn install(
    State(state): State<AppState>,
    Json(req): Json<InstallRequest>,
) -> Response {
    if state.installed.read().await.is_some() {
        return err(StatusCode::FORBIDDEN, "already installed");
    }
    // Defense in depth: once a config file exists on disk the app is already
    // configured — refuse setup even if the in-memory state is None (e.g. the
    // DB was briefly unreachable at boot). Otherwise the wizard could be
    // hijacked to repoint the app at an attacker-controlled database.
    if state.config_path.exists() {
        return err(StatusCode::FORBIDDEN, "already configured; setup is locked");
    }

    let username = req.admin_username.trim();
    if username.len() < 3 || username.len() > 32 {
        return err(StatusCode::BAD_REQUEST, "admin_username must be 3-32 chars");
    }
    if !username
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return err(
            StatusCode::BAD_REQUEST,
            "admin_username: letters, digits, _ or - only",
        );
    }
    if req.admin_password.len() < 6 {
        return err(
            StatusCode::BAD_REQUEST,
            "admin_password must be at least 6 characters",
        );
    }

    let (kind, url) = match build_url(&req.connection, &state.data_dir) {
        Ok(v) => v,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };

    let pool = match db::connect(&url).await {
        Ok(p) => p,
        Err(e) => return err(StatusCode::BAD_GATEWAY, format!("connect: {e}")),
    };

    if let Err(e) = db::migrate(&pool, kind).await {
        return err(StatusCode::BAD_GATEWAY, format!("migrate: {e}"));
    }

    // refuse if any user already exists (safety against re-running install against a used db)
    let count: (i64,) = match sqlx::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(&pool)
        .await
    {
        Ok(v) => v,
        Err(e) => return err(StatusCode::BAD_GATEWAY, e.to_string()),
    };
    if count.0 > 0 {
        return err(
            StatusCode::CONFLICT,
            "this database already has users; refusing to re-install",
        );
    }

    let phc = match auth::hash_password(&req.admin_password) {
        Ok(h) => h,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };

    let true_lit = db::bool_true(kind);
    let base_insert = db::q(
        kind,
        &format!(
            "INSERT INTO users (username, password_hash, is_admin) VALUES (?, ?, {true_lit})"
        ),
    );
    let admin_id = match kind {
        db::DbKind::Sqlite | db::DbKind::Postgres => {
            let row: Result<(i64,), _> =
                sqlx::query_as(&format!("{base_insert} RETURNING id"))
                    .bind(username)
                    .bind(&phc)
                    .fetch_one(&pool)
                    .await;
            match row {
                Ok((id,)) => id,
                Err(e) => return err(StatusCode::BAD_GATEWAY, e.to_string()),
            }
        }
        db::DbKind::Mysql => {
            let mut tx = match pool.begin().await {
                Ok(t) => t,
                Err(e) => return err(StatusCode::BAD_GATEWAY, e.to_string()),
            };
            if let Err(e) = sqlx::query(&base_insert)
                .bind(username)
                .bind(&phc)
                .execute(&mut *tx)
                .await
            {
                return err(StatusCode::BAD_GATEWAY, e.to_string());
            }
            let row: Result<(i64,), _> = sqlx::query_as("SELECT LAST_INSERT_ID()")
                .fetch_one(&mut *tx)
                .await;
            let id = match row {
                Ok((v,)) => v,
                Err(e) => return err(StatusCode::BAD_GATEWAY, e.to_string()),
            };
            if let Err(e) = tx.commit().await {
                return err(StatusCode::BAD_GATEWAY, e.to_string());
            }
            id
        }
    };

    if let Err(e) = save_config(
        &state.config_path,
        &StoredConfig {
            database_url: url.clone(),
            storage: None,
        },
    ) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, format!("write config: {e}"));
    }

    *state.installed.write().await = Some(InstalledState { pool, kind });
    Json(InstallResponse { ok: true, admin_id }).into_response()
}

// ---------------------------------------------------------------------------
// routes
// ---------------------------------------------------------------------------

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/setup/status", get(status))
        .route("/setup/test", post(test_connection))
        .route("/setup/install", post(install))
}

#[cfg(test)]
mod tests {
    use std::{path::Path, time::{SystemTime, UNIX_EPOCH}};

    use super::{StoredConfig, config_path, load_config, save_config};

    #[test]
    fn config_discovery_preserves_existing_database_and_honors_overrides() {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("yunova-config-discovery-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let current = dir.join("yunova.toml");
        let legacy = dir.join("novachat.toml");
        assert_eq!(config_path(&dir, None), current);
        let original = StoredConfig {
            database_url: "sqlite://data/novachat.db".into(),
            storage: None,
        };
        save_config(&legacy, &original).unwrap();
        assert_eq!(config_path(&dir, None), legacy);
        assert_eq!(load_config(&config_path(&dir, None)).unwrap().database_url, original.database_url);
        save_config(&current, &original).unwrap();
        assert_eq!(config_path(&dir, None), current);
        assert_eq!(config_path(&dir, Some("custom.toml")), Path::new("custom.toml"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn legacy_config_without_storage_still_loads() {
        let config: StoredConfig =
            toml::from_str("database_url = 'sqlite://data/test.db'").unwrap();
        assert!(config.storage.is_none());
    }

    #[test]
    fn s3_storage_config_loads_from_toml() {
        let config: StoredConfig = toml::from_str(
            "database_url = 'sqlite://data/test.db'\n\
             [storage]\n\
             backend = 's3'\n\
             bucket = 'media'\n\
             access_key_id = 'key'\n\
             secret_access_key = 'secret'\n",
        )
        .unwrap();
        let storage = config.storage.unwrap();
        assert_eq!(storage.backend, "s3");
        assert_eq!(storage.bucket.as_deref(), Some("media"));
    }

    #[test]
    fn saved_config_round_trips_and_uses_private_permissions() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "yunova-config-test-{}-{nonce}",
            std::process::id()
        ));
        let path = dir.join("yunova.toml");
        let config = StoredConfig {
            database_url: "sqlite:///data/yunova.db".into(),
            storage: Some(crate::storage::StorageConfig {
                backend: "s3".into(),
                secret_access_key: Some("secret".into()),
                ..Default::default()
            }),
        };

        save_config(&path, &config).unwrap();
        let loaded = load_config(&path).unwrap();
        assert_eq!(loaded.database_url, config.database_url);
        assert_eq!(
            loaded
                .storage
                .and_then(|storage| storage.secret_access_key)
                .as_deref(),
            Some("secret")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }

        let _ = std::fs::remove_dir_all(dir);
    }
}
