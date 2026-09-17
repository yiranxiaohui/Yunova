//! Persistent, multi-track browser video editing projects and server-side
//! FFmpeg rendering. Editor snapshots deliberately keep media references as
//! Yunova storage paths so generated, workflow, uploaded, and public assets
//! can all be cut without copying large files between features.

use std::{
    collections::HashSet,
    path::{Path as FsPath, PathBuf},
    time::Duration,
};

use axum::{
    Extension, Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    AppState, CurrentUser, InstalledState, db,
    storage::{MediaKind, MediaStorage},
};

const MAX_SNAPSHOT_BYTES: usize = 2 * 1024 * 1024;
const MAX_UPLOAD_BYTES: usize = 512 * 1024 * 1024;
const MAX_TRACKS: usize = 24;
const MAX_CLIPS: usize = 300;
const MAX_PROJECT_SECONDS: f64 = 2.0 * 60.0 * 60.0;
const MAX_TEXT_CHARS: usize = 2_000;

fn err(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({ "error": message.into() }))).into_response()
}

fn random_hex(n: usize) -> String {
    let mut bytes = vec![0_u8; n];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn valid_name(name: &str, label: &str, max: usize) -> Result<String, String> {
    let value = name.trim();
    if value.is_empty() || value.chars().count() > max || value.chars().any(char::is_control) {
        return Err(format!("{label}需要 1～{max} 个字符"));
    }
    Ok(value.to_string())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct EditorSnapshot {
    #[serde(default = "snapshot_version")]
    version: i64,
    #[serde(default = "default_width")]
    width: i64,
    #[serde(default = "default_height")]
    height: i64,
    #[serde(default = "default_fps")]
    fps: f64,
    #[serde(default = "default_background")]
    background: String,
    #[serde(default)]
    tracks: Vec<EditorTrack>,
}

fn snapshot_version() -> i64 {
    1
}
fn default_width() -> i64 {
    1920
}
fn default_height() -> i64 {
    1080
}
fn default_fps() -> f64 {
    30.0
}
fn default_background() -> String {
    "#000000".into()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct EditorTrack {
    id: String,
    #[serde(default)]
    name: String,
    kind: String,
    #[serde(default)]
    hidden: bool,
    #[serde(default)]
    muted: bool,
    #[serde(default)]
    locked: bool,
    #[serde(default)]
    clips: Vec<EditorClip>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct EditorClip {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    asset_path: Option<String>,
    #[serde(default)]
    asset_kind: Option<String>,
    start: f64,
    duration: f64,
    #[serde(default)]
    source_in: f64,
    #[serde(default = "default_speed")]
    speed: f64,
    #[serde(default = "default_one")]
    opacity: f64,
    #[serde(default = "default_one")]
    volume: f64,
    #[serde(default)]
    fade_in: f64,
    #[serde(default)]
    fade_out: f64,
    #[serde(default)]
    transform: ClipTransform,
    #[serde(default)]
    text: Option<String>,
    #[serde(default = "default_font_size")]
    font_size: f64,
    #[serde(default = "default_text_color")]
    color: String,
}

fn default_speed() -> f64 {
    1.0
}
fn default_one() -> f64 {
    1.0
}
fn default_font_size() -> f64 {
    64.0
}
fn default_text_color() -> String {
    "#ffffff".into()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ClipTransform {
    #[serde(default)]
    x: f64,
    #[serde(default)]
    y: f64,
    #[serde(default = "default_one")]
    scale: f64,
    #[serde(default)]
    rotation: f64,
}

impl Default for ClipTransform {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            scale: 1.0,
            rotation: 0.0,
        }
    }
}

fn finite_in(value: f64, range: std::ops::RangeInclusive<f64>) -> bool {
    value.is_finite() && range.contains(&value)
}

fn safe_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
}

fn color_value(value: &str, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() == 7
        && trimmed.starts_with('#')
        && trimmed[1..].chars().all(|ch| ch.is_ascii_hexdigit())
    {
        trimmed.to_ascii_lowercase()
    } else {
        fallback.into()
    }
}

fn validate_snapshot(snapshot: &EditorSnapshot, require_media: bool) -> Result<(), String> {
    if snapshot.version != 1 {
        return Err("不支持的剪辑项目版本".into());
    }
    if !(320..=7680).contains(&snapshot.width)
        || !(240..=4320).contains(&snapshot.height)
        || snapshot.width % 2 != 0
        || snapshot.height % 2 != 0
    {
        return Err("项目分辨率无效，宽高需要是偶数且不超过 8K".into());
    }
    if !finite_in(snapshot.fps, 1.0..=120.0) {
        return Err("项目帧率需要在 1～120 FPS 之间".into());
    }
    if snapshot.tracks.len() > MAX_TRACKS {
        return Err(format!("单个项目最多 {MAX_TRACKS} 条轨道"));
    }

    let mut ids = HashSet::new();
    let mut clip_count = 0_usize;
    for track in &snapshot.tracks {
        if !safe_id(&track.id) || !ids.insert(track.id.as_str()) {
            return Err("轨道 ID 为空、重复或格式无效".into());
        }
        if !matches!(track.kind.as_str(), "video" | "audio" | "text") {
            return Err(format!("轨道 {} 的类型无效", track.id));
        }
        for clip in &track.clips {
            clip_count += 1;
            if clip_count > MAX_CLIPS {
                return Err(format!("单个项目最多 {MAX_CLIPS} 个片段"));
            }
            if !safe_id(&clip.id) || !ids.insert(clip.id.as_str()) {
                return Err("片段 ID 为空、重复或格式无效".into());
            }
            if !finite_in(clip.start, 0.0..=MAX_PROJECT_SECONDS)
                || !finite_in(clip.duration, 0.01..=MAX_PROJECT_SECONDS)
                || clip.start + clip.duration > MAX_PROJECT_SECONDS
                || !finite_in(clip.source_in, 0.0..=MAX_PROJECT_SECONDS)
                || !finite_in(clip.speed, 0.25..=4.0)
                || !finite_in(clip.opacity, 0.0..=1.0)
                || !finite_in(clip.volume, 0.0..=4.0)
                || !finite_in(clip.fade_in, 0.0..=clip.duration)
                || !finite_in(clip.fade_out, 0.0..=clip.duration)
                || !finite_in(clip.transform.scale, 0.05..=4.0)
                || !finite_in(clip.transform.x, -16_000.0..=16_000.0)
                || !finite_in(clip.transform.y, -16_000.0..=16_000.0)
                || !finite_in(clip.transform.rotation, -3600.0..=3600.0)
            {
                return Err(format!("片段 {} 的时间或效果参数无效", clip.id));
            }
            if track.kind == "text" {
                let text = clip.text.as_deref().unwrap_or_default();
                if text.trim().is_empty() || text.chars().count() > MAX_TEXT_CHARS {
                    return Err(format!("字幕片段 {} 的文本无效", clip.id));
                }
                if !finite_in(clip.font_size, 8.0..=400.0) {
                    return Err(format!("字幕片段 {} 的字号无效", clip.id));
                }
            } else if require_media || clip.asset_path.is_some() {
                let path = clip
                    .asset_path
                    .as_deref()
                    .ok_or_else(|| format!("片段 {} 缺少素材", clip.id))?;
                let (media_kind, _) =
                    stored_media(path).map_err(|_| format!("片段 {} 的素材路径无效", clip.id))?;
                if (track.kind == "video" && media_kind == MediaKind::Audio)
                    || (track.kind == "audio" && media_kind == MediaKind::Image)
                {
                    return Err(format!("片段 {} 的素材类型与轨道不匹配", clip.id));
                }
            }
        }
    }
    Ok(())
}

fn project_duration(snapshot: &EditorSnapshot) -> f64 {
    snapshot
        .tracks
        .iter()
        .flat_map(|track| track.clips.iter())
        .map(|clip| clip.start + clip.duration)
        .fold(0.0_f64, f64::max)
}

#[derive(sqlx::FromRow)]
struct ProjectRow {
    id: i64,
    name: String,
    timeline_json: String,
    created_at: String,
    updated_at: String,
}

#[derive(Serialize)]
struct ProjectView {
    id: i64,
    name: String,
    timeline: Value,
    created_at: String,
    updated_at: String,
}

impl From<ProjectRow> for ProjectView {
    fn from(row: ProjectRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            timeline: serde_json::from_str(&row.timeline_json).unwrap_or(Value::Null),
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

#[derive(Deserialize)]
struct SaveProjectReq {
    id: Option<i64>,
    name: String,
    timeline: EditorSnapshot,
}

async fn list_projects(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
) -> Response {
    let sql = db::q(
        installed.kind,
        "SELECT id, name, timeline_json, created_at, updated_at FROM video_editor_projects \
         WHERE user_id = ? ORDER BY updated_at DESC, id DESC LIMIT 100",
    );
    match sqlx::query_as::<_, ProjectRow>(&sql)
        .bind(user.id)
        .fetch_all(&installed.pool)
        .await
    {
        Ok(rows) => {
            Json(rows.into_iter().map(ProjectView::from).collect::<Vec<_>>()).into_response()
        }
        Err(error) => err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

async fn get_project(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Response {
    let sql = db::q(
        installed.kind,
        "SELECT id, name, timeline_json, created_at, updated_at FROM video_editor_projects \
         WHERE id = ? AND user_id = ?",
    );
    match sqlx::query_as::<_, ProjectRow>(&sql)
        .bind(id)
        .bind(user.id)
        .fetch_optional(&installed.pool)
        .await
    {
        Ok(Some(row)) => Json(ProjectView::from(row)).into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, "剪辑项目不存在"),
        Err(error) => err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

async fn save_project(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Json(request): Json<SaveProjectReq>,
) -> Response {
    let name = match valid_name(&request.name, "项目名称", 100) {
        Ok(value) => value,
        Err(message) => return err(StatusCode::BAD_REQUEST, message),
    };
    if let Err(message) = validate_snapshot(&request.timeline, false) {
        return err(StatusCode::BAD_REQUEST, message);
    }
    let timeline_json = match serde_json::to_string(&request.timeline) {
        Ok(value) if value.len() <= MAX_SNAPSHOT_BYTES => value,
        Ok(_) => return err(StatusCode::BAD_REQUEST, "项目数据不能超过 2 MiB"),
        Err(error) => return err(StatusCode::BAD_REQUEST, error.to_string()),
    };

    if let Some(id) = request.id {
        let now = db::now_expr(installed.kind);
        let sql = db::q(
            installed.kind,
            &format!(
                "UPDATE video_editor_projects SET name = ?, timeline_json = ?, updated_at = {now} \
                 WHERE id = ? AND user_id = ?"
            ),
        );
        return match sqlx::query(&sql)
            .bind(&name)
            .bind(&timeline_json)
            .bind(id)
            .bind(user.id)
            .execute(&installed.pool)
            .await
        {
            Ok(result) if result.rows_affected() > 0 => Json(json!({ "id": id })).into_response(),
            Ok(_) => err(StatusCode::NOT_FOUND, "剪辑项目不存在"),
            Err(error) => err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
        };
    }

    let sql = db::q(
        installed.kind,
        "INSERT INTO video_editor_projects (user_id, name, timeline_json) \
         VALUES (?, ?, ?) RETURNING id",
    );
    let inserted = sqlx::query_as::<_, (i64,)>(&sql)
        .bind(user.id)
        .bind(&name)
        .bind(&timeline_json)
        .fetch_one(&installed.pool)
        .await
        .map(|row| row.0);
    match inserted {
        Ok(id) => Json(json!({ "id": id })).into_response(),
        Err(error) => err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

async fn delete_project(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Response {
    let sql = db::q(
        installed.kind,
        "DELETE FROM video_editor_projects WHERE id = ? AND user_id = ?",
    );
    match sqlx::query(&sql)
        .bind(id)
        .bind(user.id)
        .execute(&installed.pool)
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Personal and public material library
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
struct AssetView {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    library_id: Option<i64>,
    title: String,
    kind: String,
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    thumbnail_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    width: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    height: Option<i64>,
    source: String,
    is_public: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    author: Option<String>,
    created_at: String,
}

#[derive(sqlx::FromRow)]
struct LibraryAssetRow {
    id: i64,
    title: String,
    kind: String,
    path: String,
    metadata_json: Option<String>,
    source: String,
    is_public: i64,
    created_at: String,
    author: Option<String>,
}

fn meta_number(meta: &Value, key: &str) -> Option<f64> {
    meta.get(key).and_then(Value::as_f64)
}

fn library_view(row: LibraryAssetRow) -> AssetView {
    let meta: Value = row
        .metadata_json
        .as_deref()
        .and_then(|value| serde_json::from_str(value).ok())
        .unwrap_or(Value::Null);
    AssetView {
        id: format!("library:{}", row.id),
        library_id: Some(row.id),
        title: row.title,
        kind: row.kind,
        path: row.path,
        thumbnail_path: meta
            .get("thumbnail_path")
            .and_then(Value::as_str)
            .map(str::to_string),
        duration: meta_number(&meta, "duration"),
        width: meta_number(&meta, "width").map(|value| value as i64),
        height: meta_number(&meta, "height").map(|value| value as i64),
        source: row.source,
        is_public: row.is_public != 0,
        author: row.author,
        created_at: row.created_at,
    }
}

#[derive(Deserialize)]
struct AssetQuery {
    #[serde(default)]
    scope: Option<String>,
}

async fn list_assets(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Query(query): Query<AssetQuery>,
) -> Response {
    let public = query.scope.as_deref() == Some("public");
    let mut assets = Vec::new();
    let mut seen = HashSet::new();

    let library_sql = if public {
        db::q(
            installed.kind,
            "SELECT a.id, a.title, a.kind, a.path, a.metadata_json, a.source, \
                 a.is_public, a.created_at, u.username AS author \
                 FROM media_library_assets a JOIN users u ON u.id = a.user_id \
                 WHERE a.is_public = 1 \
                 ORDER BY a.created_at DESC, a.id DESC LIMIT 120",
        )
    } else {
        db::q(
            installed.kind,
            "SELECT a.id, a.title, a.kind, a.path, a.metadata_json, a.source, \
                 a.is_public, a.created_at, u.username AS author \
                 FROM media_library_assets a JOIN users u ON u.id = a.user_id \
                 WHERE a.user_id = ? ORDER BY a.created_at DESC, a.id DESC LIMIT 120",
        )
    };
    let library_rows = if public {
        sqlx::query_as::<_, LibraryAssetRow>(&library_sql)
            .fetch_all(&installed.pool)
            .await
    } else {
        sqlx::query_as::<_, LibraryAssetRow>(&library_sql)
            .bind(user.id)
            .fetch_all(&installed.pool)
            .await
    };
    let rows = match library_rows {
        Ok(rows) => rows,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    for row in rows {
        if !seen.insert(row.path.clone()) {
            continue;
        }
        let mut view = library_view(row);
        if !public {
            view.author = None;
        }
        assets.push(view);
    }

    if public {
        return Json(assets).into_response();
    }

    #[derive(sqlx::FromRow)]
    struct VideoAssetRow {
        id: i64,
        prompt: String,
        seconds: i64,
        size: String,
        video_path: String,
        input_image_path: Option<String>,
        created_at: String,
    }
    let video_sql = db::q(
        installed.kind,
        "SELECT id, prompt, seconds, size, video_path, input_image_path, created_at \
         FROM video_jobs WHERE user_id = ? AND status = 'completed' AND video_path IS NOT NULL \
         ORDER BY created_at DESC, id DESC LIMIT 100",
    );
    if let Ok(rows) = sqlx::query_as::<_, VideoAssetRow>(&video_sql)
        .bind(user.id)
        .fetch_all(&installed.pool)
        .await
    {
        for row in rows {
            if seen.insert(row.video_path.clone()) {
                let (width, height) = parse_size(&row.size);
                assets.push(AssetView {
                    id: format!("video-job:{}", row.id),
                    library_id: None,
                    title: row.prompt.chars().take(48).collect(),
                    kind: "video".into(),
                    path: row.video_path,
                    thumbnail_path: row.input_image_path,
                    duration: Some(row.seconds as f64),
                    width,
                    height,
                    source: "generated".into(),
                    is_public: false,
                    author: None,
                    created_at: row.created_at,
                });
            }
        }
    }

    #[derive(sqlx::FromRow)]
    struct ImageAssetRow {
        id: i64,
        prompt: String,
        image_path: String,
        created_at: String,
    }
    let image_sql = db::q(
        installed.kind,
        "SELECT id, prompt, image_path, created_at FROM studio_generations \
         WHERE user_id = ? AND status = 'completed' AND image_path IS NOT NULL \
         ORDER BY created_at DESC, id DESC LIMIT 100",
    );
    if let Ok(rows) = sqlx::query_as::<_, ImageAssetRow>(&image_sql)
        .bind(user.id)
        .fetch_all(&installed.pool)
        .await
    {
        for row in rows {
            if seen.insert(row.image_path.clone()) {
                assets.push(AssetView {
                    id: format!("studio:{}", row.id),
                    library_id: None,
                    title: row.prompt.chars().take(48).collect(),
                    kind: "image".into(),
                    path: row.image_path.clone(),
                    thumbnail_path: Some(row.image_path),
                    duration: None,
                    width: None,
                    height: None,
                    source: "generated".into(),
                    is_public: false,
                    author: None,
                    created_at: row.created_at,
                });
            }
        }
    }

    #[derive(sqlx::FromRow)]
    struct WorkflowAssetRow {
        id: i64,
        kind: String,
        path: String,
        metadata_json: Option<String>,
        created_at: String,
    }
    let workflow_sql = db::q(
        installed.kind,
        "SELECT id, kind, path, metadata_json, created_at FROM media_assets \
         WHERE user_id = ? ORDER BY created_at DESC, id DESC LIMIT 100",
    );
    if let Ok(rows) = sqlx::query_as::<_, WorkflowAssetRow>(&workflow_sql)
        .bind(user.id)
        .fetch_all(&installed.pool)
        .await
    {
        for row in rows {
            if !seen.insert(row.path.clone()) {
                continue;
            }
            let meta: Value = row
                .metadata_json
                .as_deref()
                .and_then(|value| serde_json::from_str(value).ok())
                .unwrap_or(Value::Null);
            assets.push(AssetView {
                id: format!("workflow:{}", row.id),
                library_id: None,
                title: meta
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("流水线产物")
                    .to_string(),
                kind: row.kind,
                path: row.path,
                thumbnail_path: meta
                    .get("thumbnail_path")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                duration: meta_number(&meta, "duration"),
                width: meta_number(&meta, "width").map(|value| value as i64),
                height: meta_number(&meta, "height").map(|value| value as i64),
                source: "workflow".into(),
                is_public: false,
                author: None,
                created_at: row.created_at,
            });
        }
    }
    Json(assets).into_response()
}

fn parse_size(value: &str) -> (Option<i64>, Option<i64>) {
    value
        .split_once('x')
        .or_else(|| value.split_once('×'))
        .and_then(|(width, height)| Some((width.parse().ok()?, height.parse().ok()?)))
        .map(|(width, height)| (Some(width), Some(height)))
        .unwrap_or((None, None))
}

#[derive(Deserialize)]
struct UploadAssetReq {
    filename: String,
    mime: String,
    b64: String,
    #[serde(default)]
    duration: Option<f64>,
    #[serde(default)]
    width: Option<i64>,
    #[serde(default)]
    height: Option<i64>,
}

fn upload_kind_and_ext(mime: &str, filename: &str) -> Option<(&'static str, MediaKind, String)> {
    let lower = filename.to_ascii_lowercase();
    let ext = lower
        .rsplit_once('.')
        .map(|(_, ext)| ext)
        .filter(|ext| ext.chars().all(|ch| ch.is_ascii_alphanumeric()))
        .unwrap_or("");
    if mime.starts_with("image/") {
        let ext = match ext {
            "jpg" | "jpeg" | "png" | "webp" | "gif" => ext,
            _ => "png",
        };
        Some(("image", MediaKind::Image, ext.to_string()))
    } else if mime.starts_with("video/") {
        let ext = match ext {
            "mp4" | "webm" | "mov" | "m4v" | "mkv" => ext,
            _ => "mp4",
        };
        Some(("video", MediaKind::Video, ext.to_string()))
    } else if mime.starts_with("audio/") {
        let ext = match ext {
            "mp3" | "wav" | "m4a" | "aac" | "ogg" | "flac" => ext,
            _ => "mp3",
        };
        Some(("audio", MediaKind::Audio, ext.to_string()))
    } else {
        None
    }
}

async fn insert_library_asset(
    installed: &InstalledState,
    user_id: i64,
    title: &str,
    kind: &str,
    path: &str,
    metadata_json: &str,
    source: &str,
) -> Result<i64, sqlx::Error> {
    let sql = db::q(
        installed.kind,
        "INSERT INTO media_library_assets \
         (user_id, title, kind, path, metadata_json, source) \
         VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
    );
    sqlx::query_as::<_, (i64,)>(&sql)
        .bind(user_id)
        .bind(title)
        .bind(kind)
        .bind(path)
        .bind(metadata_json)
        .bind(source)
        .fetch_one(&installed.pool)
        .await
        .map(|row| row.0)
}

async fn upload_asset(
    State(state): State<AppState>,
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Json(request): Json<UploadAssetReq>,
) -> Response {
    let title = match valid_name(&request.filename, "素材名称", 200) {
        Ok(value) => value,
        Err(message) => return err(StatusCode::BAD_REQUEST, message),
    };
    let Some((kind, storage_kind, ext)) = upload_kind_and_ext(&request.mime, &request.filename)
    else {
        return err(StatusCode::BAD_REQUEST, "仅支持图片、视频和音频素材");
    };
    let bytes = match STANDARD.decode(request.b64.trim().as_bytes()) {
        Ok(bytes) if !bytes.is_empty() && bytes.len() <= MAX_UPLOAD_BYTES => bytes,
        Ok(_) => return err(StatusCode::BAD_REQUEST, "素材需要在 1 字节～512 MiB 之间"),
        Err(error) => return err(StatusCode::BAD_REQUEST, format!("素材编码无效: {error}")),
    };
    let filename = format!("{}.{}", random_hex(16), ext);
    if let Err(error) = state.storage.put(storage_kind, &filename, bytes).await {
        return err(StatusCode::BAD_GATEWAY, format!("保存素材失败: {error}"));
    }
    let path = match storage_kind {
        MediaKind::Image => format!("/api/images/{filename}"),
        MediaKind::Video => format!("/api/videos/{filename}"),
        MediaKind::Audio => format!("/api/editor-media/audio/{filename}"),
        MediaKind::Avatar => unreachable!(),
    };
    let metadata = json!({
        "mime": request.mime,
        "duration": request.duration,
        "width": request.width,
        "height": request.height,
    })
    .to_string();
    match insert_library_asset(
        &installed, user.id, &title, kind, &path, &metadata, "upload",
    )
    .await
    {
        Ok(id) => Json(json!({ "id": id, "path": path })).into_response(),
        Err(error) => {
            let _ = state.storage.delete(storage_kind, &filename).await;
            err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

#[derive(Deserialize)]
struct ImportAssetReq {
    title: String,
    kind: String,
    path: String,
    #[serde(default)]
    duration: Option<f64>,
    #[serde(default)]
    width: Option<i64>,
    #[serde(default)]
    height: Option<i64>,
    #[serde(default)]
    thumbnail_path: Option<String>,
}

async fn import_asset(
    State(state): State<AppState>,
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Json(request): Json<ImportAssetReq>,
) -> Response {
    let title = match valid_name(&request.title, "素材名称", 200) {
        Ok(value) => value,
        Err(message) => return err(StatusCode::BAD_REQUEST, message),
    };
    if request.kind != "image" && request.kind != "video" && request.kind != "audio" {
        return err(StatusCode::BAD_REQUEST, "素材类型无效");
    }
    let (storage_kind, name) = match stored_media(&request.path) {
        Ok(value) => value,
        Err(message) => return err(StatusCode::BAD_REQUEST, message),
    };
    match state.storage.size(storage_kind, &name).await {
        Ok(_) => {}
        Err(error) if error.is_not_found() => return err(StatusCode::NOT_FOUND, "素材文件不存在"),
        Err(error) => return err(StatusCode::BAD_GATEWAY, error.to_string()),
    }
    let existing_sql = db::q(
        installed.kind,
        "SELECT id FROM media_library_assets WHERE user_id = ? AND path = ? LIMIT 1",
    );
    if let Ok(Some((id,))) = sqlx::query_as::<_, (i64,)>(&existing_sql)
        .bind(user.id)
        .bind(&request.path)
        .fetch_optional(&installed.pool)
        .await
    {
        return Json(json!({ "id": id })).into_response();
    }
    let metadata = json!({
        "duration": request.duration,
        "width": request.width,
        "height": request.height,
        "thumbnail_path": request.thumbnail_path,
    })
    .to_string();
    match insert_library_asset(
        &installed,
        user.id,
        &title,
        &request.kind,
        &request.path,
        &metadata,
        "imported",
    )
    .await
    {
        Ok(id) => Json(json!({ "id": id })).into_response(),
        Err(error) => err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

#[derive(Deserialize)]
struct VisibilityReq {
    is_public: bool,
}

async fn set_asset_visibility(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(request): Json<VisibilityReq>,
) -> Response {
    let now = db::now_expr(installed.kind);
    let sql = db::q(
        installed.kind,
        &format!(
            "UPDATE media_library_assets SET is_public = ?, updated_at = {now} \
             WHERE id = ? AND user_id = ?"
        ),
    );
    match sqlx::query(&sql)
        .bind(request.is_public)
        .bind(id)
        .bind(user.id)
        .execute(&installed.pool)
        .await
    {
        Ok(result) if result.rows_affected() > 0 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => err(StatusCode::NOT_FOUND, "素材不存在"),
        Err(error) => err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

async fn delete_asset(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Response {
    // Only remove the library record. The object may still be referenced by
    // saved projects or exports, so destructive media cleanup is intentionally
    // left to a future reference-counted garbage collector.
    let sql = db::q(
        installed.kind,
        "DELETE FROM media_library_assets WHERE id = ? AND user_id = ?",
    );
    match sqlx::query(&sql)
        .bind(id)
        .bind(user.id)
        .execute(&installed.pool)
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

// ---------------------------------------------------------------------------
// FFmpeg export jobs
// ---------------------------------------------------------------------------

#[derive(Clone, sqlx::FromRow)]
struct ExportRow {
    token: String,
    project_id: Option<i64>,
    status: String,
    progress: i64,
    video_path: Option<String>,
    error: Option<String>,
    created_at: String,
    started_at: Option<String>,
    finished_at: Option<String>,
}

#[derive(Serialize)]
struct ExportView {
    token: String,
    project_id: Option<i64>,
    status: String,
    progress: i64,
    video_path: Option<String>,
    error: Option<String>,
    created_at: String,
    started_at: Option<String>,
    finished_at: Option<String>,
}

impl From<ExportRow> for ExportView {
    fn from(row: ExportRow) -> Self {
        Self {
            token: row.token,
            project_id: row.project_id,
            status: row.status,
            progress: row.progress,
            video_path: row.video_path,
            error: row.error,
            created_at: row.created_at,
            started_at: row.started_at,
            finished_at: row.finished_at,
        }
    }
}

async fn create_export(
    State(state): State<AppState>,
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(project_id): Path<i64>,
) -> Response {
    let sql = db::q(
        installed.kind,
        "SELECT timeline_json FROM video_editor_projects WHERE id = ? AND user_id = ?",
    );
    let snapshot_json: String = match sqlx::query_as::<_, (String,)>(&sql)
        .bind(project_id)
        .bind(user.id)
        .fetch_optional(&installed.pool)
        .await
    {
        Ok(Some((value,))) => value,
        Ok(None) => return err(StatusCode::NOT_FOUND, "剪辑项目不存在，请先保存"),
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    let snapshot: EditorSnapshot = match serde_json::from_str(&snapshot_json) {
        Ok(value) => value,
        Err(error) => return err(StatusCode::BAD_REQUEST, format!("项目数据无效: {error}")),
    };
    if let Err(message) = validate_snapshot(&snapshot, true) {
        return err(StatusCode::BAD_REQUEST, message);
    }
    if project_duration(&snapshot) < 0.05 {
        return err(StatusCode::BAD_REQUEST, "时间线为空，无法导出");
    }
    let token = random_hex(16);
    let insert = db::q(
        installed.kind,
        "INSERT INTO video_editor_exports \
         (token, project_id, user_id, snapshot_json, status) VALUES (?, ?, ?, ?, 'pending')",
    );
    if let Err(error) = sqlx::query(&insert)
        .bind(&token)
        .bind(project_id)
        .bind(user.id)
        .bind(&snapshot_json)
        .execute(&installed.pool)
        .await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }

    let state_clone = state.clone();
    let installed_clone = installed.clone();
    let token_clone = token.clone();
    tokio::spawn(async move {
        run_export_job(state_clone, installed_clone, token_clone, snapshot).await;
    });
    Json(json!({ "token": token })).into_response()
}

async fn get_export(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(token): Path<String>,
) -> Response {
    let sql = db::q(
        installed.kind,
        "SELECT token, project_id, status, progress, video_path, error, created_at, \
         started_at, finished_at FROM video_editor_exports WHERE token = ? AND user_id = ?",
    );
    match sqlx::query_as::<_, ExportRow>(&sql)
        .bind(&token)
        .bind(user.id)
        .fetch_optional(&installed.pool)
        .await
    {
        Ok(Some(row)) => Json(ExportView::from(row)).into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, "导出任务不存在"),
        Err(error) => err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

async fn list_exports(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
) -> Response {
    let sql = db::q(
        installed.kind,
        "SELECT token, project_id, status, progress, video_path, error, created_at, \
         started_at, finished_at FROM video_editor_exports WHERE user_id = ? \
         ORDER BY created_at DESC LIMIT 30",
    );
    match sqlx::query_as::<_, ExportRow>(&sql)
        .bind(user.id)
        .fetch_all(&installed.pool)
        .await
    {
        Ok(rows) => {
            Json(rows.into_iter().map(ExportView::from).collect::<Vec<_>>()).into_response()
        }
        Err(error) => err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

fn export_video_name(path: &str) -> Option<&str> {
    let name = path.strip_prefix("/api/videos/")?;
    (!name.is_empty() && !name.contains('/')).then_some(name)
}

async fn delete_export(
    State(state): State<AppState>,
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(token): Path<String>,
) -> Response {
    let select = db::q(
        installed.kind,
        "SELECT status, video_path FROM video_editor_exports WHERE token = ? AND user_id = ?",
    );
    let export = match sqlx::query_as::<_, (String, Option<String>)>(&select)
        .bind(&token)
        .bind(user.id)
        .fetch_optional(&installed.pool)
        .await
    {
        Ok(Some(row)) => row,
        Ok(None) => return err(StatusCode::NOT_FOUND, "导出记录不存在"),
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };

    if export.0 == "pending" || export.0 == "running" {
        return err(StatusCode::CONFLICT, "导出任务进行中，暂不能删除");
    }

    if let Some(name) = export.1.as_deref().and_then(export_video_name)
        && let Err(error) = state.storage.delete(MediaKind::Video, name).await
    {
        return err(
            StatusCode::BAD_GATEWAY,
            format!("删除导出视频失败: {error}"),
        );
    }

    let delete = db::q(
        installed.kind,
        "DELETE FROM video_editor_exports WHERE token = ? AND user_id = ?",
    );
    match sqlx::query(&delete)
        .bind(&token)
        .bind(user.id)
        .execute(&installed.pool)
        .await
    {
        Ok(result) if result.rows_affected() > 0 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => err(StatusCode::NOT_FOUND, "导出记录不存在"),
        Err(error) => err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

async fn update_export(
    installed: &InstalledState,
    token: &str,
    status: &str,
    progress: i64,
    path: Option<&str>,
    error: Option<&str>,
    finished: bool,
) {
    let now = db::now_expr(installed.kind);
    let sql = if finished {
        format!(
            "UPDATE video_editor_exports SET status = ?, progress = ?, video_path = ?, error = ?, \
             finished_at = {now} WHERE token = ?"
        )
    } else {
        format!(
            "UPDATE video_editor_exports SET status = ?, progress = ?, video_path = ?, error = ?, \
             started_at = COALESCE(started_at, {now}) WHERE token = ?"
        )
    };
    let sql = db::q(installed.kind, &sql);
    let _ = sqlx::query(&sql)
        .bind(status)
        .bind(progress)
        .bind(path)
        .bind(error)
        .bind(token)
        .execute(&installed.pool)
        .await;
}

async fn run_export_job(
    state: AppState,
    installed: InstalledState,
    token: String,
    snapshot: EditorSnapshot,
) {
    let permit = match state.media_process_slots.acquire().await {
        Ok(permit) => permit,
        Err(_) => {
            update_export(
                &installed,
                &token,
                "failed",
                0,
                None,
                Some("媒体处理队列已关闭"),
                true,
            )
            .await;
            return;
        }
    };
    // Keep the persisted status as `pending` while this export is waiting for
    // a media-processing slot. The client can then distinguish queued work
    // from the export that FFmpeg is actively rendering.
    update_export(&installed, &token, "running", 5, None, None, false).await;
    let result = render_snapshot(&state, &installed, &token, &snapshot).await;
    drop(permit);
    match result {
        Ok(path) => {
            update_export(
                &installed,
                &token,
                "completed",
                100,
                Some(&path),
                None,
                true,
            )
            .await;
        }
        Err(message) => {
            update_export(&installed, &token, "failed", 0, None, Some(&message), true).await;
        }
    }
}

#[derive(Clone)]
struct RenderInput {
    clip: EditorClip,
    track_kind: String,
    track_muted: bool,
    input_index: usize,
    has_audio: bool,
}

fn stored_media(path: &str) -> Result<(MediaKind, String), String> {
    let prefixes = [
        ("/api/images/", MediaKind::Image),
        ("/api/videos/", MediaKind::Video),
        ("/api/editor-media/audio/", MediaKind::Audio),
    ];
    for (prefix, kind) in prefixes {
        if let Some(name) = path.strip_prefix(prefix)
            && !name.is_empty()
            && name.len() <= 200
            && !name.contains('/')
            && !name.contains('\\')
            && !name.contains("..")
        {
            return Ok((kind, name.to_string()));
        }
    }
    Err("素材路径无效".into())
}

fn editor_temp_dir(state: &AppState, token: &str) -> PathBuf {
    state
        .data_dir
        .join("editor-tmp")
        .join(format!("export-{token}"))
}

async fn write_media_input(
    storage: &MediaStorage,
    path: &str,
    destination: &FsPath,
) -> Result<MediaKind, String> {
    let (kind, name) = stored_media(path)?;
    let size = storage
        .size(kind, &name)
        .await
        .map_err(|error| format!("读取素材信息失败: {error}"))?;
    if size > MAX_UPLOAD_BYTES as u64 {
        return Err("单个导出素材不能超过 512 MiB".into());
    }
    let bytes = storage
        .get(kind, &name)
        .await
        .map_err(|error| format!("读取素材失败: {error}"))?;
    tokio::fs::write(destination, bytes)
        .await
        .map_err(|error| format!("写入临时素材失败: {error}"))?;
    Ok(kind)
}

fn ffmpeg_program() -> String {
    crate::runtime_env::var("YUNOVA_FFMPEG").unwrap_or_else(|_| "ffmpeg".into())
}

fn ffprobe_program() -> String {
    crate::runtime_env::var("YUNOVA_FFPROBE").unwrap_or_else(|_| "ffprobe".into())
}

async fn has_audio_stream(path: &FsPath) -> bool {
    let output = tokio::process::Command::new(ffprobe_program())
        .arg("-v")
        .arg("error")
        .arg("-select_streams")
        .arg("a:0")
        .arg("-show_entries")
        .arg("stream=index")
        .arg("-of")
        .arg("csv=p=0")
        .arg(path)
        .output()
        .await;
    output
        .ok()
        .filter(|result| result.status.success())
        .is_some_and(|result| !result.stdout.is_empty())
}

fn escape_filter_text(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace(':', "\\:")
        .replace('\'', "\\'")
        .replace(',', "\\,")
        .replace(';', "\\;")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('%', "\\%")
        .replace('\n', "\\n")
}

fn atempo_chain(speed: f64) -> String {
    if speed < 0.5 {
        format!("atempo=0.5,atempo={:.6}", speed / 0.5)
    } else if speed > 2.0 {
        format!("atempo=2.0,atempo={:.6}", speed / 2.0)
    } else {
        format!("atempo={speed:.6}")
    }
}

fn visual_filter(input: &RenderInput, snapshot: &EditorSnapshot, index: usize) -> String {
    let clip = &input.clip;
    let source_duration = clip.duration * clip.speed;
    let scaled_width = (snapshot.width as f64 * clip.transform.scale)
        .round()
        .clamp(2.0, 16_384.0) as i64;
    let scaled_height = (snapshot.height as f64 * clip.transform.scale)
        .round()
        .clamp(2.0, 16_384.0) as i64;
    let mut effects = format!(
        "[{}:v]trim=start={:.6}:duration={:.6},setpts=(PTS-STARTPTS)/{:.6},\
         scale={}:{}:force_original_aspect_ratio=decrease,format=rgba",
        input.input_index, clip.source_in, source_duration, clip.speed, scaled_width, scaled_height
    );
    if clip.transform.rotation.abs() > 0.001 {
        effects.push_str(&format!(
            ",rotate={:.6}*PI/180:ow=rotw(iw):oh=roth(ih):c=none",
            clip.transform.rotation
        ));
    }
    effects.push_str(&format!(",colorchannelmixer=aa={:.6}", clip.opacity));
    if clip.fade_in > 0.001 {
        effects.push_str(&format!(",fade=t=in:st=0:d={:.6}:alpha=1", clip.fade_in));
    }
    if clip.fade_out > 0.001 {
        effects.push_str(&format!(
            ",fade=t=out:st={:.6}:d={:.6}:alpha=1",
            (clip.duration - clip.fade_out).max(0.0),
            clip.fade_out
        ));
    }
    effects.push_str(&format!(",setpts=PTS+{:.6}/TB[v{index}]", clip.start));
    effects
}

fn audio_filter(input: &RenderInput, index: usize) -> String {
    let clip = &input.clip;
    let source_duration = clip.duration * clip.speed;
    let mut effects = format!(
        "[{}:a]atrim=start={:.6}:duration={:.6},asetpts=PTS-STARTPTS,{},volume={:.6}",
        input.input_index,
        clip.source_in,
        source_duration,
        atempo_chain(clip.speed),
        clip.volume
    );
    if clip.fade_in > 0.001 {
        effects.push_str(&format!(",afade=t=in:st=0:d={:.6}", clip.fade_in));
    }
    if clip.fade_out > 0.001 {
        effects.push_str(&format!(
            ",afade=t=out:st={:.6}:d={:.6}",
            (clip.duration - clip.fade_out).max(0.0),
            clip.fade_out
        ));
    }
    effects.push_str(&format!(
        ",adelay={}:all=1[a{index}]",
        (clip.start * 1000.0).round() as i64
    ));
    effects
}

async fn render_snapshot(
    state: &AppState,
    installed: &InstalledState,
    token: &str,
    snapshot: &EditorSnapshot,
) -> Result<String, String> {
    let temp = editor_temp_dir(state, token);
    tokio::fs::create_dir_all(&temp)
        .await
        .map_err(|error| format!("创建导出目录失败: {error}"))?;
    let output_path = temp.join("output.mp4");
    let result = async {
        let mut command = tokio::process::Command::new(ffmpeg_program());
        command
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-y");

        let mut inputs = Vec::new();
        let mut input_index = 0_usize;
        for track in &snapshot.tracks {
            if track.hidden && track.kind != "audio" {
                continue;
            }
            for clip in &track.clips {
                if track.kind == "text" {
                    continue;
                }
                let path = clip.asset_path.as_deref().ok_or("片段缺少素材路径")?;
                let (_, original_name) = stored_media(path)?;
                let extension = original_name
                    .rsplit_once('.')
                    .map(|(_, value)| value)
                    .unwrap_or("bin");
                let local = temp.join(format!("input-{input_index}.{extension}"));
                let kind = write_media_input(&state.storage, path, &local).await?;
                if kind == MediaKind::Image {
                    command.arg("-loop").arg("1");
                    command
                        .arg("-framerate")
                        .arg(format!("{:.3}", snapshot.fps));
                }
                command.arg("-i").arg(&local);
                let has_audio = matches!(kind, MediaKind::Audio)
                    || (matches!(kind, MediaKind::Video) && has_audio_stream(&local).await);
                inputs.push(RenderInput {
                    clip: clip.clone(),
                    track_kind: track.kind.clone(),
                    track_muted: track.muted,
                    input_index,
                    has_audio,
                });
                input_index += 1;
            }
        }
        update_export(installed, token, "running", 25, None, None, false).await;

        let duration = project_duration(snapshot);
        let background = color_value(&snapshot.background, "#000000");
        let mut filters = vec![format!(
            "color=c={}:s={}x{}:r={:.6}:d={:.6},format=rgba[base0]",
            background, snapshot.width, snapshot.height, snapshot.fps, duration
        )];

        // Track zero is displayed at the top of the editor. Render lower
        // tracks first so higher tracks are overlaid last.
        let mut visual: Vec<&RenderInput> = inputs
            .iter()
            .filter(|input| input.track_kind == "video")
            .collect();
        visual.reverse();
        let mut base_index = 0_usize;
        for (index, input) in visual.iter().enumerate() {
            filters.push(visual_filter(input, snapshot, index));
            let clip = &input.clip;
            filters.push(format!(
                "[base{base_index}][v{index}]overlay=x=(W-w)/2+{:.3}:y=(H-h)/2+{:.3}:\
                 eof_action=pass:enable='between(t,{:.6},{:.6})'[base{}]",
                clip.transform.x,
                clip.transform.y,
                clip.start,
                clip.start + clip.duration,
                base_index + 1
            ));
            base_index += 1;
        }

        for track in snapshot.tracks.iter().rev() {
            if track.hidden || track.kind != "text" {
                continue;
            }
            for clip in &track.clips {
                let text = escape_filter_text(clip.text.as_deref().unwrap_or_default());
                let color = color_value(&clip.color, "#ffffff");
                filters.push(format!(
                    "[base{base_index}]drawtext=text='{}':fontsize={:.3}:fontcolor={}:\
                     x=(w-text_w)/2+{:.3}:y=(h-text_h)/2+{:.3}:\
                     enable='between(t,{:.6},{:.6})'[base{}]",
                    text,
                    clip.font_size,
                    color,
                    clip.transform.x,
                    clip.transform.y,
                    clip.start,
                    clip.start + clip.duration,
                    base_index + 1
                ));
                base_index += 1;
            }
        }
        filters.push(format!("[base{base_index}]format=yuv420p[vout]"));

        let audible: Vec<&RenderInput> = inputs
            .iter()
            .filter(|input| {
                input.has_audio
                    && !input.track_muted
                    && input.clip.volume > 0.0
                    && matches!(input.track_kind.as_str(), "video" | "audio")
            })
            .collect();
        if audible.is_empty() {
            filters.push(format!(
                "anullsrc=r=48000:cl=stereo,atrim=duration={duration:.6}[aout]"
            ));
        } else {
            for (index, input) in audible.iter().enumerate() {
                filters.push(audio_filter(input, index));
            }
            let refs = (0..audible.len())
                .map(|index| format!("[a{index}]"))
                .collect::<String>();
            filters.push(format!(
                "{refs}amix=inputs={}:normalize=0:dropout_transition=0,\
                 apad,atrim=duration={duration:.6}[aout]",
                audible.len()
            ));
        }

        command
            .arg("-filter_complex")
            .arg(filters.join(";"))
            .arg("-map")
            .arg("[vout]")
            .arg("-map")
            .arg("[aout]")
            .arg("-t")
            .arg(format!("{duration:.6}"))
            .arg("-r")
            .arg(format!("{:.3}", snapshot.fps))
            .arg("-c:v")
            .arg("libx264")
            .arg("-preset")
            .arg("medium")
            .arg("-crf")
            .arg("20")
            .arg("-c:a")
            .arg("aac")
            .arg("-b:a")
            .arg("192k")
            .arg("-movflags")
            .arg("+faststart")
            .arg(&output_path)
            .kill_on_drop(true);

        update_export(installed, token, "running", 40, None, None, false).await;
        let timeout_seconds = crate::runtime_env::var("YUNOVA_MEDIA_TIMEOUT_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(7200)
            .clamp(60, 21_600);
        let output = tokio::time::timeout(Duration::from_secs(timeout_seconds), command.output())
            .await
            .map_err(|_| format!("视频导出超过 {timeout_seconds} 秒，已停止"))?
            .map_err(|error| format!("无法启动 FFmpeg: {error}"))?;
        if !output.status.success() {
            let detail: String = String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(2_000)
                .collect();
            return Err(format!("FFmpeg 导出失败: {detail}"));
        }
        update_export(installed, token, "running", 90, None, None, false).await;
        let bytes = tokio::fs::read(&output_path)
            .await
            .map_err(|error| format!("读取导出结果失败: {error}"))?;
        if bytes.is_empty() {
            return Err("导出没有生成视频".into());
        }
        let name = format!("{}.mp4", random_hex(16));
        state
            .storage
            .put(MediaKind::Video, &name, bytes)
            .await
            .map_err(|error| format!("保存导出结果失败: {error}"))?;
        Ok(format!("/api/videos/{name}"))
    }
    .await;
    let _ = tokio::fs::remove_dir_all(&temp).await;
    result
}

// ---------------------------------------------------------------------------
// Audio serving (same opaque-path convention as generated image/video files)
// ---------------------------------------------------------------------------

fn parse_range(value: &str, total: u64) -> Option<(u64, u64)> {
    if total == 0 || value.contains(',') {
        return None;
    }
    let (start, end) = value.split_once('-')?;
    if start.is_empty() {
        let length = end.parse::<u64>().ok()?.min(total);
        return (length > 0).then_some((total - length, total - 1));
    }
    let start = start.parse::<u64>().ok()?;
    if start >= total {
        return None;
    }
    let end = if end.is_empty() {
        total - 1
    } else {
        end.parse::<u64>().ok()?.min(total - 1)
    };
    (start <= end).then_some((start, end))
}

async fn serve_audio(
    State(state): State<AppState>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    if name.contains("..") || name.contains('/') || name.contains('\\') {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let mime = mime_guess::from_path(&name).first_or_octet_stream();
    if let Some(range) = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("bytes="))
    {
        let total = match state.storage.size(MediaKind::Audio, &name).await {
            Ok(value) => value,
            Err(error) if error.is_not_found() => return StatusCode::NOT_FOUND.into_response(),
            Err(_) => return StatusCode::BAD_GATEWAY.into_response(),
        };
        let Some((start, end)) = parse_range(range, total) else {
            return Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::CONTENT_RANGE, format!("bytes */{total}"))
                .body(Body::empty())
                .unwrap();
        };
        return match state
            .storage
            .get_range(MediaKind::Audio, &name, start..end + 1)
            .await
        {
            Ok(bytes) => Response::builder()
                .status(StatusCode::PARTIAL_CONTENT)
                .header(header::CONTENT_TYPE, mime.as_ref())
                .header(header::ACCEPT_RANGES, "bytes")
                .header(
                    header::CONTENT_RANGE,
                    format!("bytes {start}-{end}/{total}"),
                )
                .body(Body::from(bytes))
                .unwrap(),
            Err(error) if error.is_not_found() => StatusCode::NOT_FOUND.into_response(),
            Err(_) => StatusCode::BAD_GATEWAY.into_response(),
        };
    }
    match state.storage.get(MediaKind::Audio, &name).await {
        Ok(bytes) => Response::builder()
            .header(header::CONTENT_TYPE, mime.as_ref())
            .header(header::ACCEPT_RANGES, "bytes")
            .body(Body::from(bytes))
            .unwrap(),
        Err(error) if error.is_not_found() => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::BAD_GATEWAY.into_response(),
    }
}

pub fn public_routes() -> Router<AppState> {
    Router::new().route("/editor-media/audio/{name}", get(serve_audio))
}

/// FFmpeg processes cannot survive a Yunova restart. Keep persisted export
/// history truthful instead of leaving interrupted jobs permanently pending.
pub async fn recover(pool: &db::Pool, kind: db::DbKind) {
    let now = db::now_expr(kind);
    let sql = format!(
        "UPDATE video_editor_exports SET status = 'failed', progress = 0, \
         error = '服务器在导出期间重启，请重新导出', finished_at = {now} \
         WHERE status IN ('pending', 'running')"
    );
    let _ = sqlx::query(&sql).execute(pool).await;
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/video-editor/projects",
            get(list_projects).post(save_project),
        )
        .route(
            "/video-editor/projects/{id}",
            get(get_project).delete(delete_project),
        )
        .route("/video-editor/projects/{id}/exports", post(create_export))
        .route("/video-editor/exports", get(list_exports))
        .route(
            "/video-editor/exports/{token}",
            get(get_export).delete(delete_export),
        )
        .route("/video-editor/assets", get(list_assets))
        .route("/video-editor/assets/upload", post(upload_asset))
        .route("/video-editor/assets/import", post(import_asset))
        .route(
            "/video-editor/assets/{id}/visibility",
            post(set_asset_visibility),
        )
        .route("/video-editor/assets/{id}", delete(delete_asset))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> EditorSnapshot {
        EditorSnapshot {
            version: 1,
            width: 1920,
            height: 1080,
            fps: 30.0,
            background: "#000000".into(),
            tracks: vec![EditorTrack {
                id: "video_1".into(),
                name: "V1".into(),
                kind: "video".into(),
                hidden: false,
                muted: false,
                locked: false,
                clips: vec![EditorClip {
                    id: "clip_1".into(),
                    name: "shot".into(),
                    asset_path: Some("/api/videos/abc.mp4".into()),
                    asset_kind: Some("video".into()),
                    start: 1.0,
                    duration: 3.0,
                    source_in: 0.5,
                    speed: 1.0,
                    opacity: 1.0,
                    volume: 1.0,
                    fade_in: 0.0,
                    fade_out: 0.0,
                    transform: ClipTransform::default(),
                    text: None,
                    font_size: 64.0,
                    color: "#ffffff".into(),
                }],
            }],
        }
    }

    #[test]
    fn validates_a_renderable_snapshot() {
        assert!(validate_snapshot(&snapshot(), true).is_ok());
    }

    #[test]
    fn rejects_unsafe_media_paths_and_overlong_projects() {
        let mut value = snapshot();
        value.tracks[0].clips[0].asset_path = Some("/api/videos/../secret.mp4".into());
        assert!(validate_snapshot(&value, true).is_err());
        value.tracks[0].clips[0].asset_path = Some("/api/videos/safe.mp4".into());
        value.tracks[0].clips[0].start = MAX_PROJECT_SECONDS;
        assert!(validate_snapshot(&value, true).is_err());
    }

    #[test]
    fn parses_audio_byte_ranges() {
        assert_eq!(parse_range("0-99", 1000), Some((0, 99)));
        assert_eq!(parse_range("900-", 1000), Some((900, 999)));
        assert_eq!(parse_range("-100", 1000), Some((900, 999)));
        assert_eq!(parse_range("1000-", 1000), None);
    }

    #[test]
    fn splits_extents_and_builds_tempo_chains() {
        assert_eq!(parse_size("1920x1080"), (Some(1920), Some(1080)));
        assert!(atempo_chain(0.25).contains("atempo=0.5"));
        assert!(atempo_chain(4.0).contains("atempo=2.0"));
    }

    #[test]
    fn extracts_only_direct_export_video_names() {
        assert_eq!(
            export_video_name("/api/videos/export-123.mp4"),
            Some("export-123.mp4")
        );
        assert_eq!(export_video_name("/api/images/export-123.mp4"), None);
        assert_eq!(export_video_name("/api/videos/nested/export.mp4"), None);
        assert_eq!(export_video_name("/api/videos/"), None);
    }
}
