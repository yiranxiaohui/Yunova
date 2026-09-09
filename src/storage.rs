use std::{
    fmt,
    io::SeekFrom,
    ops::Range,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::{Duration, SystemTime},
};

use bytes::Bytes;
use futures_util::{StreamExt, stream::BoxStream};
use reqwest::{StatusCode, header};
use rusty_s3::{Bucket, Credentials, S3Action, UrlStyle};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

const SIGNED_URL_TTL: Duration = Duration::from_secs(15 * 60);

// Existing objects use this namespace when no explicit prefix was configured.
// Renaming it would make those objects inaccessible. New UI configurations
// explicitly choose the Yunova prefix instead.
pub const DEFAULT_S3_PREFIX: &str = "novachat";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct StorageConfig {
    #[serde(default)]
    pub backend: String,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub bucket: Option<String>,
    #[serde(default)]
    pub access_key_id: Option<String>,
    #[serde(default)]
    pub secret_access_key: Option<String>,
    #[serde(default)]
    pub session_token: Option<String>,
    #[serde(default)]
    pub prefix: Option<String>,
    #[serde(default)]
    pub path_style: Option<bool>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaKind {
    Image,
    Video,
    Audio,
    Avatar,
}

impl MediaKind {
    fn directory(self) -> &'static str {
        match self {
            Self::Image => "images",
            Self::Video => "videos",
            Self::Audio => "audio",
            Self::Avatar => "avatars",
        }
    }
}

#[derive(Debug)]
pub enum StorageError {
    NotFound,
    Backend(String),
}

impl StorageError {
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound)
    }
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("media object not found"),
            Self::Backend(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for StorageError {}

/// A media object whose bytes can be forwarded without buffering the whole
/// object in Yunova first. The stream owns its file or upstream response, so
/// it remains valid after the storage backend is reconfigured.
pub struct MediaStream {
    content_length: Option<u64>,
    stream: BoxStream<'static, Result<Bytes, StorageError>>,
}

impl MediaStream {
    pub fn content_length(&self) -> Option<u64> {
        self.content_length
    }

    pub fn into_stream(self) -> BoxStream<'static, Result<Bytes, StorageError>> {
        self.stream
    }
}

#[derive(Clone)]
pub struct MediaStorage {
    data_dir: PathBuf,
    backend: Arc<RwLock<Backend>>,
}

#[derive(Clone)]
enum Backend {
    Local,
    S3(Arc<S3Backend>),
}

struct S3Backend {
    bucket: Bucket,
    credentials: Credentials,
    prefix: String,
    client: reqwest::Client,
    location: String,
}

#[derive(Debug, Eq, PartialEq)]
enum ResolvedStorage {
    Local,
    S3(ResolvedS3),
}

#[derive(Debug, Eq, PartialEq)]
struct ResolvedS3 {
    endpoint: String,
    region: String,
    bucket: String,
    access_key_id: String,
    secret_access_key: String,
    session_token: Option<String>,
    prefix: String,
    path_style: bool,
}

impl MediaStorage {
    pub fn from_config(data_dir: PathBuf, config: Option<&StorageConfig>) -> Result<Self, String> {
        let resolved = resolve_storage(config, |name| std::env::var(name).ok())?;
        Self::from_resolved(data_dir, resolved)
    }

    /// Build a backend from the persisted configuration only. This is used by
    /// the admin settings API so a saved web setting is applied immediately
    /// and cannot be unexpectedly replaced by a legacy environment variable.
    pub fn from_stored_config(
        data_dir: PathBuf,
        config: Option<&StorageConfig>,
    ) -> Result<Self, String> {
        let resolved = resolve_storage(config, |_| None)?;
        Self::from_resolved(data_dir, resolved)
    }

    fn from_resolved(data_dir: PathBuf, resolved: ResolvedStorage) -> Result<Self, String> {
        let backend = match resolved {
            ResolvedStorage::Local => Backend::Local,
            ResolvedStorage::S3(config) => {
                let endpoint: reqwest::Url = config
                    .endpoint
                    .parse()
                    .map_err(|e| format!("invalid S3 endpoint: {e}"))?;
                let style = if config.path_style {
                    UrlStyle::Path
                } else {
                    UrlStyle::VirtualHost
                };
                let bucket = Bucket::new(
                    endpoint,
                    style,
                    config.bucket.clone(),
                    config.region.clone(),
                )
                .map_err(|e| format!("invalid S3 bucket configuration: {e:?}"))?;
                let credentials = match config.session_token {
                    Some(token) => Credentials::new_with_token(
                        config.access_key_id,
                        config.secret_access_key,
                        token,
                    ),
                    None => Credentials::new(config.access_key_id, config.secret_access_key),
                };
                let client = reqwest::Client::builder()
                    .timeout(Duration::from_secs(600))
                    .build()
                    .map_err(|e| format!("create S3 HTTP client: {e}"))?;
                let location = if config.prefix.is_empty() {
                    format!("s3://{}", config.bucket)
                } else {
                    format!("s3://{}/{}", config.bucket, config.prefix)
                };
                Backend::S3(Arc::new(S3Backend {
                    bucket,
                    credentials,
                    prefix: config.prefix,
                    client,
                    location,
                }))
            }
        };

        Ok(Self {
            data_dir,
            backend: Arc::new(RwLock::new(backend)),
        })
    }

    pub fn backend_name(&self) -> &'static str {
        match self.current_backend() {
            Backend::Local => "local",
            Backend::S3(_) => "s3",
        }
    }

    pub fn location(&self) -> String {
        match self.current_backend() {
            Backend::Local => self.data_dir.display().to_string(),
            Backend::S3(s3) => s3.location.clone(),
        }
    }

    /// Atomically replace the active backend. In-flight operations keep their
    /// cloned backend while subsequent operations use the new configuration.
    pub fn replace_with(&self, replacement: &Self) {
        let backend = replacement.current_backend();
        *self.backend.write().unwrap_or_else(|lock| lock.into_inner()) = backend;
    }

    /// Verify that the configured bucket accepts the operations Yunova
    /// needs. The probe writes and deletes one empty, uniquely named object.
    pub async fn test_connection(&self) -> Result<(), StorageError> {
        let Backend::S3(s3) = self.current_backend() else {
            return Ok(());
        };
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let name = format!(".yunova-storage-test-{}-{nonce}", std::process::id());
        let key = if s3.prefix.is_empty() {
            name
        } else {
            format!("{}/{name}", s3.prefix)
        };

        let put_url = s3
            .bucket
            .put_object(Some(&s3.credentials), &key)
            .sign(SIGNED_URL_TTL);
        let put_response = s3
            .client
            .put(put_url)
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header(header::CONTENT_LENGTH, 0)
            .body(Vec::new())
            .send()
            .await
            .map_err(|error| request_error("connection test put", &key, error))?;
        if !put_response.status().is_success() {
            return Err(response_error(
                "connection test put",
                &key,
                put_response.status(),
            ));
        }

        let delete_url = s3
            .bucket
            .delete_object(Some(&s3.credentials), &key)
            .sign(SIGNED_URL_TTL);
        let delete_response = s3
            .client
            .delete(delete_url)
            .send()
            .await
            .map_err(|error| request_error("connection test delete", &key, error))?;
        if delete_response.status().is_success()
            || delete_response.status() == StatusCode::NOT_FOUND
        {
            Ok(())
        } else {
            Err(response_error(
                "connection test delete",
                &key,
                delete_response.status(),
            ))
        }
    }

    fn current_backend(&self) -> Backend {
        self.backend
            .read()
            .unwrap_or_else(|lock| lock.into_inner())
            .clone()
    }

    pub async fn put(
        &self,
        kind: MediaKind,
        name: &str,
        bytes: Vec<u8>,
    ) -> Result<(), StorageError> {
        validate_name(name)?;
        match self.current_backend() {
            Backend::Local => local_put(&self.data_dir, kind, name, &bytes).await,
            Backend::S3(s3) => {
                let key = s3.key(kind, name);
                let url = s3
                    .bucket
                    .put_object(Some(&s3.credentials), &key)
                    .sign(SIGNED_URL_TTL);
                let response = s3
                    .client
                    .put(url)
                    .header(header::CONTENT_TYPE, content_type(kind, name))
                    .body(bytes)
                    .send()
                    .await
                    .map_err(|e| request_error("put", &key, e))?;
                if response.status().is_success() {
                    Ok(())
                } else {
                    Err(response_error("put", &key, response.status()))
                }
            }
        }
    }

    pub async fn get(&self, kind: MediaKind, name: &str) -> Result<Vec<u8>, StorageError> {
        validate_name(name)?;
        match self.current_backend() {
            Backend::Local => local_get(&self.data_dir, kind, name).await,
            Backend::S3(s3) => {
                let key = s3.key(kind, name);
                let result = async {
                    let url = s3
                        .bucket
                        .get_object(Some(&s3.credentials), &key)
                        .sign(SIGNED_URL_TTL);
                    let response = s3
                        .client
                        .get(url)
                        .send()
                        .await
                        .map_err(|e| request_error("get", &key, e))?;
                    if response.status().is_success() {
                        response
                            .bytes()
                            .await
                            .map(|bytes| bytes.to_vec())
                            .map_err(|e| request_error("read", &key, e))
                    } else {
                        Err(response_error("get", &key, response.status()))
                    }
                }
                .await;
                self.with_local_fallback(kind, name, result, |data_dir, kind, name| async move {
                    local_get(&data_dir, kind, &name).await
                })
                .await
            }
        }
    }

    /// Open an object as a byte stream. Unlike [`Self::get`], the S3 response
    /// body is not collected before this method returns, allowing HTTP callers
    /// to send response headers as soon as S3 accepts the request.
    pub async fn open_stream(
        &self,
        kind: MediaKind,
        name: &str,
    ) -> Result<MediaStream, StorageError> {
        validate_name(name)?;
        match self.current_backend() {
            Backend::Local => local_open_stream(&self.data_dir, kind, name, None).await,
            Backend::S3(s3) => {
                let key = s3.key(kind, name);
                let result = s3_open_stream(&s3, &key, None).await;
                self.with_local_fallback(kind, name, result, |data_dir, kind, name| async move {
                    local_open_stream(&data_dir, kind, &name, None).await
                })
                .await
            }
        }
    }

    /// Open a single byte range as a stream. Conformant S3 backends are
    /// forwarded directly; a backend that ignores Range is buffered only as a
    /// compatibility fallback so the returned bytes remain correct.
    pub async fn open_range_stream(
        &self,
        kind: MediaKind,
        name: &str,
        range: Range<u64>,
    ) -> Result<MediaStream, StorageError> {
        validate_name(name)?;
        if range.start >= range.end {
            return Err(StorageError::Backend("invalid media range".into()));
        }
        match self.current_backend() {
            Backend::Local => local_open_stream(&self.data_dir, kind, name, Some(range)).await,
            Backend::S3(s3) => {
                let key = s3.key(kind, name);
                let result = s3_open_stream(&s3, &key, Some(range.clone())).await;
                self.with_local_fallback(kind, name, result, move |data_dir, kind, name| {
                    let range = range.clone();
                    async move { local_open_stream(&data_dir, kind, &name, Some(range)).await }
                })
                .await
            }
        }
    }

    pub async fn size(&self, kind: MediaKind, name: &str) -> Result<u64, StorageError> {
        validate_name(name)?;
        match self.current_backend() {
            Backend::Local => local_size(&self.data_dir, kind, name).await,
            Backend::S3(s3) => {
                let key = s3.key(kind, name);
                let result = async {
                    let url = s3
                        .bucket
                        .head_object(Some(&s3.credentials), &key)
                        .sign(SIGNED_URL_TTL);
                    let response = s3
                        .client
                        .head(url)
                        .send()
                        .await
                        .map_err(|e| request_error("head", &key, e))?;
                    if response.status().is_success() {
                        response
                            .headers()
                            .get(header::CONTENT_LENGTH)
                            .and_then(|value| value.to_str().ok())
                            .and_then(|value| value.parse::<u64>().ok())
                            .ok_or_else(|| {
                                StorageError::Backend(format!(
                                    "S3 head {key}: missing Content-Length"
                                ))
                            })
                    } else {
                        Err(response_error("head", &key, response.status()))
                    }
                }
                .await;
                self.with_local_fallback(kind, name, result, |data_dir, kind, name| async move {
                    local_size(&data_dir, kind, &name).await
                })
                .await
            }
        }
    }

    pub async fn get_range(
        &self,
        kind: MediaKind,
        name: &str,
        range: Range<u64>,
    ) -> Result<Vec<u8>, StorageError> {
        validate_name(name)?;
        if range.start >= range.end {
            return Ok(Vec::new());
        }
        match self.current_backend() {
            Backend::Local => local_get_range(&self.data_dir, kind, name, range).await,
            Backend::S3(s3) => {
                let key = s3.key(kind, name);
                let result = async {
                    let url = s3
                        .bucket
                        .get_object(Some(&s3.credentials), &key)
                        .sign(SIGNED_URL_TTL);
                    let response = s3
                        .client
                        .get(url)
                        .header(
                            header::RANGE,
                            format!("bytes={}-{}", range.start, range.end - 1),
                        )
                        .send()
                        .await
                        .map_err(|e| request_error("range get", &key, e))?;
                    if response.status().is_success() {
                        let status = response.status();
                        let bytes = response
                            .bytes()
                            .await
                            .map_err(|e| request_error("range read", &key, e))?
                            .to_vec();
                        if status == StatusCode::OK && bytes.len() as u64 >= range.end {
                            Ok(bytes[range.start as usize..range.end as usize].to_vec())
                        } else {
                            Ok(bytes)
                        }
                    } else {
                        Err(response_error("range get", &key, response.status()))
                    }
                }
                .await;
                self.with_local_fallback(kind, name, result, move |data_dir, kind, name| {
                    let range = range.clone();
                    async move { local_get_range(&data_dir, kind, &name, range).await }
                })
                .await
            }
        }
    }

    pub async fn delete(&self, kind: MediaKind, name: &str) -> Result<(), StorageError> {
        validate_name(name)?;
        match self.current_backend() {
            Backend::Local => local_delete(&self.data_dir, kind, name).await,
            Backend::S3(s3) => {
                let key = s3.key(kind, name);
                let result = async {
                    let url = s3
                        .bucket
                        .delete_object(Some(&s3.credentials), &key)
                        .sign(SIGNED_URL_TTL);
                    let response = s3
                        .client
                        .delete(url)
                        .send()
                        .await
                        .map_err(|e| request_error("delete", &key, e))?;
                    if response.status().is_success() || response.status() == StatusCode::NOT_FOUND
                    {
                        Ok(())
                    } else {
                        Err(response_error("delete", &key, response.status()))
                    }
                }
                .await;

                // Remove a legacy local copy too. Its failure must not turn a
                // successful S3 deletion into an error.
                let _ = local_delete(&self.data_dir, kind, name).await;
                result
            }
        }
    }

    async fn with_local_fallback<T, F, Fut>(
        &self,
        kind: MediaKind,
        name: &str,
        primary: Result<T, StorageError>,
        fallback: F,
    ) -> Result<T, StorageError>
    where
        F: FnOnce(PathBuf, MediaKind, String) -> Fut,
        Fut: std::future::Future<Output = Result<T, StorageError>>,
    {
        match primary {
            Ok(value) => Ok(value),
            Err(primary_error) => {
                match fallback(self.data_dir.clone(), kind, name.to_string()).await {
                    Ok(value) => Ok(value),
                    Err(StorageError::NotFound) => Err(primary_error),
                    Err(local_error) => Err(local_error),
                }
            }
        }
    }
}

impl S3Backend {
    fn key(&self, kind: MediaKind, name: &str) -> String {
        if self.prefix.is_empty() {
            format!("{}/{name}", kind.directory())
        } else {
            format!("{}/{}/{name}", self.prefix, kind.directory())
        }
    }
}

async fn s3_open_stream(
    s3: &S3Backend,
    key: &str,
    range: Option<Range<u64>>,
) -> Result<MediaStream, StorageError> {
    let url = s3
        .bucket
        .get_object(Some(&s3.credentials), key)
        .sign(SIGNED_URL_TTL);
    let mut request = s3.client.get(url);
    if let Some(range) = &range {
        request = request.header(
            header::RANGE,
            format!("bytes={}-{}", range.start, range.end - 1),
        );
    }
    let response = request
        .send()
        .await
        .map_err(|error| request_error("stream", key, error))?;
    let status = response.status();
    if !status.is_success() {
        return Err(response_error("stream", key, status));
    }

    if let Some(range) = range {
        if status != StatusCode::PARTIAL_CONTENT {
            // A few S3-compatible services ignore Range and return 200. Keep
            // the old compatibility behavior without penalizing conformant
            // backends with whole-object buffering.
            let bytes = response
                .bytes()
                .await
                .map_err(|error| request_error("range read", key, error))?;
            if range.end > bytes.len() as u64 {
                return Err(StorageError::Backend(
                    "S3 range response is shorter than requested".into(),
                ));
            }
            let bytes = bytes.slice(range.start as usize..range.end as usize);
            return Ok(buffered_stream(bytes));
        }
        let content_length = Some(range.end - range.start);
        return Ok(response_stream(response, key, content_length));
    }

    let content_length = response.content_length();
    Ok(response_stream(response, key, content_length))
}

fn response_stream(
    response: reqwest::Response,
    key: &str,
    content_length: Option<u64>,
) -> MediaStream {
    let key = key.to_string();
    let stream = response
        .bytes_stream()
        .map(move |chunk| chunk.map_err(|error| request_error("stream read", &key, error)))
        .boxed();
    MediaStream {
        content_length,
        stream,
    }
}

fn buffered_stream(bytes: Bytes) -> MediaStream {
    let content_length = Some(bytes.len() as u64);
    MediaStream {
        content_length,
        stream: futures_util::stream::once(async move { Ok(bytes) }).boxed(),
    }
}

fn request_error(operation: &str, key: &str, error: reqwest::Error) -> StorageError {
    StorageError::Backend(format!("S3 {operation} {key}: {}", error.without_url()))
}

fn response_error(operation: &str, key: &str, status: StatusCode) -> StorageError {
    if status == StatusCode::NOT_FOUND {
        return StorageError::NotFound;
    }
    StorageError::Backend(format!("S3 {operation} {key}: HTTP {status}"))
}

fn validate_name(name: &str) -> Result<(), StorageError> {
    if name.is_empty() || name.contains("..") || name.contains('/') || name.contains('\\') {
        return Err(StorageError::Backend("invalid media object name".into()));
    }
    Ok(())
}

fn local_path(data_dir: &Path, kind: MediaKind, name: &str) -> PathBuf {
    data_dir.join(kind.directory()).join(name)
}

async fn local_put(
    data_dir: &Path,
    kind: MediaKind,
    name: &str,
    bytes: &[u8],
) -> Result<(), StorageError> {
    let path = local_path(data_dir, kind, name);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(map_io_error)?;
    }
    tokio::fs::write(path, bytes).await.map_err(map_io_error)
}

async fn local_get(data_dir: &Path, kind: MediaKind, name: &str) -> Result<Vec<u8>, StorageError> {
    tokio::fs::read(local_path(data_dir, kind, name))
        .await
        .map_err(map_io_error)
}

async fn local_open_stream(
    data_dir: &Path,
    kind: MediaKind,
    name: &str,
    range: Option<Range<u64>>,
) -> Result<MediaStream, StorageError> {
    let mut file = tokio::fs::File::open(local_path(data_dir, kind, name))
        .await
        .map_err(map_io_error)?;
    let total = file.metadata().await.map_err(map_io_error)?.len();

    let (content_length, stream) = if let Some(range) = range {
        if range.start >= range.end || range.end > total {
            return Err(StorageError::Backend(
                "media range exceeds object size".into(),
            ));
        }
        file.seek(SeekFrom::Start(range.start))
            .await
            .map_err(map_io_error)?;
        let length = range.end - range.start;
        let stream = ReaderStream::new(file.take(length))
            .map(|chunk| chunk.map_err(map_io_error))
            .boxed();
        (Some(length), stream)
    } else {
        let stream = ReaderStream::new(file)
            .map(|chunk| chunk.map_err(map_io_error))
            .boxed();
        (Some(total), stream)
    };

    Ok(MediaStream {
        content_length,
        stream,
    })
}

async fn local_size(data_dir: &Path, kind: MediaKind, name: &str) -> Result<u64, StorageError> {
    tokio::fs::metadata(local_path(data_dir, kind, name))
        .await
        .map(|metadata| metadata.len())
        .map_err(map_io_error)
}

async fn local_get_range(
    data_dir: &Path,
    kind: MediaKind,
    name: &str,
    range: Range<u64>,
) -> Result<Vec<u8>, StorageError> {
    let bytes = local_get(data_dir, kind, name).await?;
    if range.end > bytes.len() as u64 {
        return Err(StorageError::Backend(
            "media range exceeds object size".into(),
        ));
    }
    Ok(bytes[range.start as usize..range.end as usize].to_vec())
}

async fn local_delete(data_dir: &Path, kind: MediaKind, name: &str) -> Result<(), StorageError> {
    match tokio::fs::remove_file(local_path(data_dir, kind, name)).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(map_io_error(error)),
    }
}

fn map_io_error(error: std::io::Error) -> StorageError {
    if error.kind() == std::io::ErrorKind::NotFound {
        StorageError::NotFound
    } else {
        StorageError::Backend(error.to_string())
    }
}

fn content_type(kind: MediaKind, name: &str) -> String {
    match kind {
        MediaKind::Video | MediaKind::Audio => mime_guess::from_path(name)
            .first_or_octet_stream()
            .to_string(),
        MediaKind::Image | MediaKind::Avatar => mime_guess::from_path(name)
            .first_or_octet_stream()
            .essence_str()
            .to_string(),
    }
}

fn nonempty(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn config_value(value: &Option<String>) -> Option<String> {
    nonempty(value.clone())
}

fn env_value<F>(getenv: &mut F, names: &[&str]) -> Option<String>
where
    F: FnMut(&str) -> Option<String>,
{
    names.iter().find_map(|name| {
        nonempty(crate::runtime_env::value_with(name, &mut *getenv))
    })
}

fn parse_bool(name: &str, value: &str) -> Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(format!("{name} must be true or false")),
    }
}

fn normalize_prefix(prefix: String) -> Result<String, String> {
    let mut parts = Vec::new();
    for part in prefix.split('/').filter(|part| !part.is_empty()) {
        if part == "." || part == ".." || part.contains('\\') {
            return Err("S3 prefix contains an invalid path segment".into());
        }
        parts.push(part);
    }
    Ok(parts.join("/"))
}

fn resolve_storage<F>(
    config: Option<&StorageConfig>,
    mut getenv: F,
) -> Result<ResolvedStorage, String>
where
    F: FnMut(&str) -> Option<String>,
{
    let config = config.cloned().unwrap_or_default();
    // Persisted settings win so values saved from the admin page remain
    // authoritative after a restart. Environment variables are retained as a
    // backwards-compatible fallback for deployments without [storage].
    let backend = nonempty(Some(config.backend.clone()))
        .or_else(|| env_value(&mut getenv, &["YUNOVA_STORAGE_BACKEND"]))
        .unwrap_or_else(|| "local".into())
        .to_ascii_lowercase();
    if backend == "local" {
        return Ok(ResolvedStorage::Local);
    }
    if backend != "s3" {
        return Err("storage backend must be local or s3".into());
    }

    let region = config_value(&config.region)
        .or_else(|| env_value(&mut getenv, &["YUNOVA_S3_REGION"]))
        .or_else(|| env_value(&mut getenv, &["AWS_REGION", "AWS_DEFAULT_REGION"]))
        .unwrap_or_else(|| "us-east-1".into());
    let custom_endpoint = config_value(&config.endpoint)
        .or_else(|| env_value(&mut getenv, &["YUNOVA_S3_ENDPOINT"]))
        .or_else(|| env_value(&mut getenv, &["AWS_ENDPOINT_URL_S3"]));
    let endpoint = custom_endpoint
        .clone()
        .unwrap_or_else(|| format!("https://s3.{region}.amazonaws.com"));
    let bucket = config_value(&config.bucket)
        .or_else(|| env_value(&mut getenv, &["YUNOVA_S3_BUCKET"]))
        .ok_or_else(|| "S3 bucket is required".to_string())?;
    if bucket.contains('/') || bucket.contains('\\') {
        return Err("S3 bucket must not contain slashes".into());
    }
    let access_key_id = config_value(&config.access_key_id)
        .or_else(|| env_value(&mut getenv, &["YUNOVA_S3_ACCESS_KEY_ID"]))
        .or_else(|| env_value(&mut getenv, &["AWS_ACCESS_KEY_ID"]))
        .ok_or_else(|| "S3 access key id is required".to_string())?;
    let secret_access_key = config_value(&config.secret_access_key)
        .or_else(|| env_value(&mut getenv, &["YUNOVA_S3_SECRET_ACCESS_KEY"]))
        .or_else(|| env_value(&mut getenv, &["AWS_SECRET_ACCESS_KEY"]))
        .ok_or_else(|| "S3 secret access key is required".to_string())?;
    let session_token = config_value(&config.session_token)
        .or_else(|| env_value(&mut getenv, &["YUNOVA_S3_SESSION_TOKEN"]))
        .or_else(|| env_value(&mut getenv, &["AWS_SESSION_TOKEN"]));
    let prefix = config_value(&config.prefix)
        .or_else(|| env_value(&mut getenv, &["YUNOVA_S3_PREFIX"]))
        .unwrap_or_else(|| DEFAULT_S3_PREFIX.into());
    let prefix = normalize_prefix(prefix)?;
    let path_style = match config.path_style {
        Some(value) => value,
        None => match env_value(&mut getenv, &["YUNOVA_S3_PATH_STYLE"]) {
            Some(value) => parse_bool("YUNOVA_S3_PATH_STYLE", &value)?,
            None => custom_endpoint.is_some(),
        },
    };

    Ok(ResolvedStorage::S3(ResolvedS3 {
        endpoint,
        region,
        bucket,
        access_key_id,
        secret_access_key,
        session_token,
        prefix,
        path_style,
    }))
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        convert::Infallible,
        time::{Duration, SystemTime},
    };

    use axum::{
        Router,
        body::{Body, to_bytes},
        extract::State,
        http::{Method, Request, Response, StatusCode, header},
    };
    use tokio::sync::RwLock;

    use super::*;

    type Objects = Arc<RwLock<HashMap<String, Vec<u8>>>>;

    fn test_data_dir(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "yunova-storage-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    async fn mock_s3(State(objects): State<Objects>, request: Request<Body>) -> Response<Body> {
        let method = request.method().clone();
        let key = request
            .uri()
            .path()
            .strip_prefix("/media/")
            .unwrap_or_default()
            .to_string();
        let requested_range = request
            .headers()
            .get(header::RANGE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let content_length = request
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);

        match method {
            Method::PUT => {
                if key.contains(".yunova-storage-test-")
                    && content_length.as_deref() != Some("0")
                {
                    return Response::builder()
                        .status(StatusCode::BAD_REQUEST)
                        .body(Body::empty())
                        .unwrap();
                }
                let bytes = to_bytes(request.into_body(), usize::MAX)
                    .await
                    .unwrap()
                    .to_vec();
                objects.write().await.insert(key, bytes);
                Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::empty())
                    .unwrap()
            }
            Method::GET => {
                let Some(bytes) = objects.read().await.get(&key).cloned() else {
                    return Response::builder()
                        .status(StatusCode::NOT_FOUND)
                        .body(Body::empty())
                        .unwrap();
                };
                if let Some(range) = requested_range.and_then(|value| {
                    let value = value.strip_prefix("bytes=")?;
                    let (start, end) = value.split_once('-')?;
                    Some((start.parse::<usize>().ok()?, end.parse::<usize>().ok()?))
                }) {
                    let bytes = bytes[range.0..=range.1].to_vec();
                    return Response::builder()
                        .status(StatusCode::PARTIAL_CONTENT)
                        .header(header::CONTENT_LENGTH, bytes.len())
                        .body(Body::from(bytes))
                        .unwrap();
                }
                if key.ends_with("slow.mp4") {
                    let content_length = bytes.len();
                    let first = Bytes::copy_from_slice(&bytes[..1]);
                    let rest = Bytes::copy_from_slice(&bytes[1..]);
                    let chunks =
                        futures_util::stream::once(async move { Ok::<_, Infallible>(first) })
                            .chain(futures_util::stream::once(async move {
                                tokio::time::sleep(Duration::from_secs(1)).await;
                                Ok::<_, Infallible>(rest)
                            }));
                    return Response::builder()
                        .status(StatusCode::OK)
                        .header(header::CONTENT_LENGTH, content_length)
                        .body(Body::from_stream(chunks))
                        .unwrap();
                }
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_LENGTH, bytes.len())
                    .body(Body::from(bytes))
                    .unwrap()
            }
            Method::HEAD => {
                let Some(size) = objects.read().await.get(&key).map(Vec::len) else {
                    return Response::builder()
                        .status(StatusCode::NOT_FOUND)
                        .body(Body::empty())
                        .unwrap();
                };
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_LENGTH, size)
                    .body(Body::empty())
                    .unwrap()
            }
            Method::DELETE => {
                objects.write().await.remove(&key);
                Response::builder()
                    .status(StatusCode::NO_CONTENT)
                    .body(Body::empty())
                    .unwrap()
            }
            _ => Response::builder()
                .status(StatusCode::METHOD_NOT_ALLOWED)
                .body(Body::empty())
                .unwrap(),
        }
    }

    #[test]
    fn defaults_to_local_storage() {
        assert_eq!(
            resolve_storage(None, |_| None).unwrap(),
            ResolvedStorage::Local
        );
    }

    #[test]
    fn renamed_s3_environment_preserves_objects_and_prefers_new_settings() {
        let mut environment = HashMap::from([
            ("NOVACHAT_STORAGE_BACKEND", "s3"),
            ("NOVACHAT_S3_BUCKET", "existing-media"),
            ("NOVACHAT_S3_ACCESS_KEY_ID", "test-key"),
            ("NOVACHAT_S3_SECRET_ACCESS_KEY", "test-secret"),
        ]);
        let ResolvedStorage::S3(legacy) = resolve_storage(None, |name| {
            environment.get(name).map(ToString::to_string)
        }).unwrap() else { panic!("expected legacy S3 configuration") };
        assert_eq!(legacy.bucket, "existing-media");
        assert_eq!(legacy.prefix, "novachat");

        environment.insert("YUNOVA_S3_BUCKET", "new-media");
        environment.insert("YUNOVA_S3_PREFIX", "yunova");
        let ResolvedStorage::S3(current) = resolve_storage(None, |name| {
            environment.get(name).map(ToString::to_string)
        }).unwrap() else { panic!("expected renamed S3 configuration") };
        assert_eq!(current.bucket, "new-media");
        assert_eq!(current.prefix, "yunova");
    }

    #[test]
    fn resolves_s3_config_and_normalizes_prefix() {
        let config = StorageConfig {
            backend: "s3".into(),
            endpoint: Some("https://objects.example.com".into()),
            region: Some("auto".into()),
            bucket: Some("media".into()),
            access_key_id: Some("key".into()),
            secret_access_key: Some("secret".into()),
            prefix: Some("/tenant//yunova/".into()),
            ..Default::default()
        };

        let resolved = resolve_storage(Some(&config), |_| None).unwrap();
        let ResolvedStorage::S3(resolved) = resolved else {
            panic!("expected S3 storage");
        };
        assert_eq!(resolved.prefix, "tenant/yunova");
        assert!(resolved.path_style);
    }

    #[test]
    fn persisted_storage_values_win_over_legacy_environment_values() {
        let config = StorageConfig {
            backend: "s3".into(),
            endpoint: Some("https://saved.example.com".into()),
            region: Some("saved-region".into()),
            bucket: Some("saved-bucket".into()),
            access_key_id: Some("saved-key".into()),
            secret_access_key: Some("saved-secret".into()),
            prefix: Some("saved-prefix".into()),
            path_style: Some(false),
            ..Default::default()
        };
        let environment = HashMap::from([
            ("YUNOVA_STORAGE_BACKEND", "local"),
            ("YUNOVA_S3_ENDPOINT", "https://env.example.com"),
            ("YUNOVA_S3_REGION", "env-region"),
            ("YUNOVA_S3_BUCKET", "env-bucket"),
            ("YUNOVA_S3_ACCESS_KEY_ID", "env-key"),
            ("YUNOVA_S3_SECRET_ACCESS_KEY", "env-secret"),
            ("YUNOVA_S3_PREFIX", "env-prefix"),
            ("YUNOVA_S3_PATH_STYLE", "true"),
        ]);

        let resolved = resolve_storage(Some(&config), |name| {
            environment.get(name).map(ToString::to_string)
        })
        .unwrap();
        let ResolvedStorage::S3(resolved) = resolved else {
            panic!("expected persisted S3 backend");
        };
        assert_eq!(resolved.endpoint, "https://saved.example.com");
        assert_eq!(resolved.region, "saved-region");
        assert_eq!(resolved.bucket, "saved-bucket");
        assert_eq!(resolved.access_key_id, "saved-key");
        assert_eq!(resolved.secret_access_key, "saved-secret");
        assert_eq!(resolved.prefix, "saved-prefix");
        assert!(!resolved.path_style);
    }

    #[tokio::test]
    async fn local_storage_round_trip_range_and_delete() {
        let data_dir = test_data_dir("local");
        let storage =
            MediaStorage::from_resolved(data_dir.clone(), ResolvedStorage::Local).unwrap();
        storage
            .put(MediaKind::Video, "sample.mp4", b"0123456789".to_vec())
            .await
            .unwrap();
        assert_eq!(
            storage.size(MediaKind::Video, "sample.mp4").await.unwrap(),
            10
        );
        let media = storage
            .open_stream(MediaKind::Video, "sample.mp4")
            .await
            .unwrap();
        assert_eq!(media.content_length(), Some(10));
        assert_eq!(
            to_bytes(Body::from_stream(media.into_stream()), usize::MAX)
                .await
                .unwrap(),
            b"0123456789".as_slice()
        );
        let media = storage
            .open_range_stream(MediaKind::Video, "sample.mp4", 2..6)
            .await
            .unwrap();
        assert_eq!(media.content_length(), Some(4));
        assert_eq!(
            to_bytes(Body::from_stream(media.into_stream()), usize::MAX)
                .await
                .unwrap(),
            b"2345".as_slice()
        );
        assert_eq!(
            storage
                .get_range(MediaKind::Video, "sample.mp4", 2..6)
                .await
                .unwrap(),
            b"2345"
        );
        storage
            .delete(MediaKind::Video, "sample.mp4")
            .await
            .unwrap();
        assert!(
            storage
                .get(MediaKind::Video, "sample.mp4")
                .await
                .unwrap_err()
                .is_not_found()
        );
        let _ = tokio::fs::remove_dir_all(data_dir).await;
    }

    #[tokio::test]
    async fn s3_storage_round_trip_range_and_delete() {
        let objects: Objects = Default::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(
            axum::serve(
                listener,
                Router::new().fallback(mock_s3).with_state(objects.clone()),
            )
            .into_future(),
        );
        let config = StorageConfig {
            backend: "s3".into(),
            endpoint: Some(format!("http://{address}")),
            region: Some("test".into()),
            bucket: Some("media".into()),
            access_key_id: Some("key".into()),
            secret_access_key: Some("secret".into()),
            prefix: Some("yunova".into()),
            path_style: Some(true),
            ..Default::default()
        };
        let resolved = resolve_storage(Some(&config), |_| None).unwrap();
        let data_dir = test_data_dir("s3");
        let storage = MediaStorage::from_resolved(data_dir.clone(), resolved).unwrap();

        storage.test_connection().await.unwrap();
        assert!(objects.read().await.is_empty());

        local_put(&data_dir, MediaKind::Image, "legacy.png", b"legacy")
            .await
            .unwrap();
        assert_eq!(
            storage.get(MediaKind::Image, "legacy.png").await.unwrap(),
            b"legacy"
        );

        storage
            .put(MediaKind::Video, "sample.mp4", b"0123456789".to_vec())
            .await
            .unwrap();
        assert_eq!(
            objects
                .read()
                .await
                .get("yunova/videos/sample.mp4")
                .unwrap(),
            b"0123456789"
        );
        assert_eq!(
            storage.size(MediaKind::Video, "sample.mp4").await.unwrap(),
            10
        );
        let media = storage
            .open_stream(MediaKind::Video, "sample.mp4")
            .await
            .unwrap();
        assert_eq!(media.content_length(), Some(10));
        assert_eq!(
            to_bytes(Body::from_stream(media.into_stream()), usize::MAX)
                .await
                .unwrap(),
            b"0123456789".as_slice()
        );
        let media = storage
            .open_range_stream(MediaKind::Video, "sample.mp4", 3..7)
            .await
            .unwrap();
        assert_eq!(media.content_length(), Some(4));
        assert_eq!(
            to_bytes(Body::from_stream(media.into_stream()), usize::MAX)
                .await
                .unwrap(),
            b"3456".as_slice()
        );
        assert_eq!(
            storage
                .get_range(MediaKind::Video, "sample.mp4", 3..7)
                .await
                .unwrap(),
            b"3456"
        );
        assert_eq!(
            storage.get(MediaKind::Video, "sample.mp4").await.unwrap(),
            b"0123456789"
        );

        storage
            .put(MediaKind::Video, "slow.mp4", b"streamed".to_vec())
            .await
            .unwrap();
        let media = tokio::time::timeout(
            Duration::from_millis(500),
            storage.open_stream(MediaKind::Video, "slow.mp4"),
        )
        .await
        .expect("opening an S3 stream must not wait for the complete body")
        .unwrap();
        assert_eq!(
            to_bytes(Body::from_stream(media.into_stream()), usize::MAX)
                .await
                .unwrap(),
            b"streamed".as_slice()
        );
        storage.delete(MediaKind::Video, "slow.mp4").await.unwrap();
        storage
            .delete(MediaKind::Video, "sample.mp4")
            .await
            .unwrap();
        assert!(objects.read().await.is_empty());

        let local =
            MediaStorage::from_resolved(data_dir.clone(), ResolvedStorage::Local).unwrap();
        storage.replace_with(&local);
        assert_eq!(storage.backend_name(), "local");
        storage
            .put(MediaKind::Image, "after-switch.png", b"local".to_vec())
            .await
            .unwrap();
        assert_eq!(
            local_get(&data_dir, MediaKind::Image, "after-switch.png")
                .await
                .unwrap(),
            b"local"
        );

        server.abort();
        let _ = tokio::fs::remove_dir_all(data_dir).await;
    }
}
