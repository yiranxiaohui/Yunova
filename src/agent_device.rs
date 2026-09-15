//! Device connector: the user's own computer as an execution target.
//!
//! The desktop client dials in and holds a WebSocket open, rather than the
//! server connecting to it. A personal machine is usually behind NAT with no
//! reachable address, so an outbound connection is the only option that works
//! without port forwarding — the same reason the legacy worker did it this
//! way.
//!
//! What crosses the socket is the *same* pi RPC JSONL the cloud sandbox
//! speaks. The desktop client runs `pi --mode rpc` locally and pipes its stdio
//! through this relay, so [`DeviceTransport`] is only a pipe: every behaviour
//! above the transport — mirroring, fan-out, approvals, billing — is the code
//! already proven against the sandbox.
//!
//! Security posture differs sharply from the cloud target and the protocol
//! reflects it. A sandbox is disposable and isolated; a personal machine is
//! not. So the desktop client, not the server, owns the workspace allowlist
//! and the approval policy. The server never tells a device what to execute —
//! it forwards prompts and the local runtime decides.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{RwLock, mpsc};

use crate::agent_session::AgentTransport;
use crate::db::{self, DbKind, Pool};
use crate::{AppState, CurrentUser, InstalledState};

/// Namespaces a device pairing code so it cannot be confused with a gateway
/// agent token: the two authenticate different things and must never be
/// interchangeable.
const PAIR_PREFIX: &str = "ynd_";

const MAX_DEVICES_PER_USER: i64 = 20;

/// Frames from the desktop client.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FromDevice {
    /// First frame. Authenticates the socket and names the machine.
    Hello {
        token: String,
        name: String,
        #[serde(default)]
        platform: Option<String>,
    },
    /// Keeps the connection warm and refreshes `last_seen_at`.
    Heartbeat,
    /// One JSONL record from the local runtime's stdout, verbatim.
    Frame { session_id: i64, frame: Value },
    /// The local runtime for a session exited.
    RuntimeClosed {
        session_id: i64,
        #[serde(default)]
        reason: Option<String>,
    },
    /// A local runtime could not be started.
    RuntimeError { session_id: i64, message: String },
}

/// Frames to the desktop client.
#[derive(Debug, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToDevice {
    HelloOk { device_id: i64 },
    Error { message: String },
    /// Start a local runtime for this session.
    ///
    /// Carries the generated `models.json` so the device never holds an
    /// upstream provider key: it points at this server's gateway with a
    /// session-scoped token, exactly as the sandbox does.
    StartRuntime { session_id: i64, models_json: Value },
    /// One JSONL record for the local runtime's stdin, verbatim.
    Frame { session_id: i64, frame: Value },
    /// Stop the local runtime for this session.
    StopRuntime { session_id: i64 },
}

/// A connected desktop client.
pub struct DeviceHandle {
    pub user_id: i64,
    tx: mpsc::Sender<ToDevice>,
}

impl DeviceHandle {
    pub async fn send(&self, msg: ToDevice) -> Result<(), String> {
        self.tx
            .send(msg)
            .await
            .map_err(|_| "设备连接已断开".to_string())
    }
}

/// Online desktop clients, keyed by device id.
///
/// In-memory because it tracks open sockets. Durable device records live in
/// `agent_devices`, which is why `list` reports both the stored row and its
/// live connection state.
#[derive(Clone, Default)]
pub struct DeviceRegistry {
    inner: Arc<RwLock<HashMap<i64, Arc<DeviceHandle>>>>,
}

impl DeviceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn get(&self, device_id: i64) -> Option<Arc<DeviceHandle>> {
        self.inner.read().await.get(&device_id).cloned()
    }

    pub async fn is_online(&self, device_id: i64) -> bool {
        self.inner.read().await.contains_key(&device_id)
    }

    async fn insert(&self, device_id: i64, handle: Arc<DeviceHandle>) {
        self.inner.write().await.insert(device_id, handle);
    }

    async fn remove(&self, device_id: i64) {
        self.inner.write().await.remove(&device_id);
    }
}

/// Relays RPC frames to a runtime on the user's machine.
///
/// Deliberately thin. The frames are already-encoded JSONL lines, so this
/// wraps each one and lets the device write it to the local process's stdin
/// unchanged; framing stays identical to the subprocess and sandbox drivers.
pub struct DeviceTransport {
    device: Arc<DeviceHandle>,
    session_id: i64,
}

impl DeviceTransport {
    pub fn new(device: Arc<DeviceHandle>, session_id: i64) -> Self {
        Self { device, session_id }
    }
}

#[async_trait::async_trait]
impl AgentTransport for DeviceTransport {
    async fn send(&self, line: String) -> Result<(), String> {
        // The session layer hands us a framed JSONL line; parse it back so the
        // relay carries structured JSON rather than a string the device would
        // have to re-parse and could mangle.
        let frame: Value = serde_json::from_str(line.trim_end())
            .map_err(|e| format!("无法解析发往设备的指令: {e}"))?;
        self.device
            .send(ToDevice::Frame {
                session_id: self.session_id,
                frame,
            })
            .await
    }

    async fn shutdown(&self) {
        // Best-effort: the device may already be gone, which is not an error
        // worth surfacing during teardown.
        let _ = self
            .device
            .send(ToDevice::StopRuntime {
                session_id: self.session_id,
            })
            .await;
    }
}

// ---------------------------------------------------------------------------
// pairing and management
// ---------------------------------------------------------------------------

fn new_pair_code() -> String {
    format!("{PAIR_PREFIX}{}", crate::auth::generate_token())
}

#[derive(Deserialize)]
struct PairReq {
    #[serde(default)]
    name: Option<String>,
}

/// Issue a pairing code for a new machine.
///
/// Only the hash is stored, so a leaked database cannot be used to attach a
/// device. The plaintext is returned exactly once, for the user to paste into
/// the desktop client.
async fn pair(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Json(req): Json<PairReq>,
) -> Response {
    let live_sql = db::q(
        installed.kind,
        &format!(
            "SELECT COUNT(*) FROM agent_devices WHERE user_id = ? AND revoked = {}",
            match installed.kind {
                DbKind::Postgres => "FALSE",
                _ => "0",
            }
        ),
    );
    let live: i64 = sqlx::query_scalar(&live_sql)
        .bind(user.id)
        .fetch_one(&installed.pool)
        .await
        .unwrap_or(0);
    if live >= MAX_DEVICES_PER_USER {
        return (
            StatusCode::BAD_REQUEST,
            format!("最多只能绑定 {MAX_DEVICES_PER_USER} 台电脑，请先移除不用的"),
        )
            .into_response();
    }

    let name = req
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or("我的电脑")
        .chars()
        .take(60)
        .collect::<String>();

    let code = new_pair_code();
    let hash = crate::auth::token_hash(&code);
    let sql = db::q(
        installed.kind,
        "INSERT INTO agent_devices (user_id, kind, name, token_hash) VALUES (?, 'device', ?, ?)",
    );
    if let Err(e) = sqlx::query(&sql)
        .bind(user.id)
        .bind(&name)
        .bind(&hash)
        .execute(&installed.pool)
        .await
    {
        eprintln!("[agent-device] pairing insert failed: {e}");
        return (StatusCode::INTERNAL_SERVER_ERROR, "创建配对码失败").into_response();
    }

    Json(json!({ "code": code, "name": name })).into_response()
}

/// One row of the device list query.
type DeviceRow = (
    i64,            // id
    String,         // name
    Option<String>, // platform
    Option<String>, // last_seen_at
    i64,            // revoked
);

async fn list_devices(
    State(state): State<AppState>,
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
) -> Response {
    let revoked_col = db::bool_as_int(installed.kind, "revoked");
    let sql = db::q(
        installed.kind,
        &format!(
            "SELECT id, name, platform, last_seen_at, {revoked_col} \
             FROM agent_devices WHERE user_id = ? ORDER BY id DESC"
        ),
    );
    let rows: Vec<DeviceRow> = sqlx::query_as(&sql)
        .bind(user.id)
        .fetch_all(&installed.pool)
        .await
        .unwrap_or_default();

    let mut out = Vec::with_capacity(rows.len());
    for (id, name, platform, last_seen_at, revoked) in rows {
        out.push(json!({
            "id": id,
            "name": name,
            "platform": platform,
            "last_seen_at": last_seen_at,
            "revoked": revoked != 0,
            // Whether the desktop client is connected right now. A task
            // cannot start on an offline machine, so the picker needs this.
            "online": state.agent_devices.is_online(id).await,
        }));
    }
    Json(out).into_response()
}

/// Revoke a device, disconnecting it immediately.
///
/// Revoking has to drop the live socket too: leaving it attached would let a
/// machine the user just removed keep executing work.
async fn revoke_device(
    State(state): State<AppState>,
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Response {
    let sql = db::q(
        installed.kind,
        &format!(
            "UPDATE agent_devices SET revoked = {} WHERE id = ? AND user_id = ?",
            match installed.kind {
                DbKind::Postgres => "TRUE",
                _ => "1",
            }
        ),
    );
    match sqlx::query(&sql)
        .bind(id)
        .bind(user.id)
        .execute(&installed.pool)
        .await
    {
        Ok(r) if r.rows_affected() == 0 => {
            return (StatusCode::NOT_FOUND, "设备不存在").into_response();
        }
        Err(e) => {
            eprintln!("[agent-device] revoke failed: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "移除设备失败").into_response();
        }
        Ok(_) => {}
    }

    if let Some(handle) = state.agent_devices.get(id).await {
        let _ = handle
            .send(ToDevice::Error {
                message: "该设备已被移除".into(),
            })
            .await;
        state.agent_devices.remove(id).await;
    }
    StatusCode::NO_CONTENT.into_response()
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/agent/devices", get(list_devices))
        .route("/agent/devices/pair", post(pair))
        .route("/agent/devices/{id}", delete(revoke_device))
}

pub fn public_routes() -> Router<AppState> {
    Router::new().route("/agent/devices/connect", get(ws_connect))
}

// ---------------------------------------------------------------------------
// websocket relay
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ConnectQuery {
    #[serde(default)]
    token: Option<String>,
}

async fn ws_connect(
    State(state): State<AppState>,
    Query(q): Query<ConnectQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    ws.on_upgrade(move |socket| handle_socket(state, socket, q.token))
}

/// Resolve a pairing code to its device, refusing revoked ones.
async fn device_for_code(pool: &Pool, kind: DbKind, code: &str) -> Option<(i64, i64)> {
    if !code.starts_with(PAIR_PREFIX) {
        return None;
    }
    let hash = crate::auth::token_hash(code);
    let revoked_col = db::bool_as_int(kind, "revoked");
    let sql = db::q(
        kind,
        &format!(
            "SELECT id, user_id, {revoked_col} FROM agent_devices WHERE token_hash = ?"
        ),
    );
    let row: Option<(i64, i64, i64)> = sqlx::query_as(&sql)
        .bind(&hash)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    let (id, user_id, revoked) = row?;
    if revoked != 0 {
        return None;
    }
    Some((id, user_id))
}

async fn handle_socket(state: AppState, socket: WebSocket, query_token: Option<String>) {
    let (mut sink, mut stream) = socket.split();

    let installed = match state.installed.read().await.clone() {
        Some(i) => i,
        None => return,
    };

    // The first frame must authenticate. A token may also arrive as a query
    // parameter, which some client HTTP stacks handle more easily than custom
    // headers on an upgrade request.
    let first = match stream.next().await {
        Some(Ok(Message::Text(t))) => t.to_string(),
        _ => return,
    };
    let Ok(hello) = serde_json::from_str::<FromDevice>(&first) else {
        return;
    };
    let (token, name, platform) = match hello {
        FromDevice::Hello {
            token,
            name,
            platform,
        } => (
            if token.is_empty() {
                query_token.unwrap_or_default()
            } else {
                token
            },
            name,
            platform,
        ),
        _ => return,
    };

    let Some((device_id, user_id)) =
        device_for_code(&installed.pool, installed.kind, &token).await
    else {
        let err = serde_json::to_string(&ToDevice::Error {
            message: "配对码无效或已被移除".into(),
        })
        .unwrap_or_default();
        let _ = sink.send(Message::Text(err.into())).await;
        return;
    };

    // Record what connected, so the user can tell their machines apart.
    let _ = sqlx::query(&db::q(
        installed.kind,
        &format!(
            "UPDATE agent_devices SET name = ?, platform = ?, last_seen_at = {} WHERE id = ?",
            db::now_expr(installed.kind)
        ),
    ))
    .bind(&name)
    .bind(platform.as_deref())
    .bind(device_id)
    .execute(&installed.pool)
    .await;

    let (tx, mut rx) = mpsc::channel::<ToDevice>(128);
    let handle = Arc::new(DeviceHandle {
        user_id,
        tx: tx.clone(),
    });
    state.agent_devices.insert(device_id, handle.clone()).await;

    let ok = serde_json::to_string(&ToDevice::HelloOk { device_id }).unwrap_or_default();
    if sink.send(Message::Text(ok.into())).await.is_err() {
        state.agent_devices.remove(device_id).await;
        return;
    }
    eprintln!("[agent-device] device {device_id} ({name}) connected");

    // Writer: server -> device.
    let writer = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let Ok(text) = serde_json::to_string(&msg) else {
                continue;
            };
            if sink.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    // Reader: device -> server. Frames are routed into the live session that
    // owns them, which is what makes a device-hosted runtime indistinguishable
    // from a sandboxed one to everything above the transport.
    let frame_senders: Arc<RwLock<HashMap<i64, mpsc::Sender<Value>>>> =
        Arc::new(RwLock::new(HashMap::new()));
    state
        .agent_device_frames
        .write()
        .await
        .insert(device_id, frame_senders.clone());

    while let Some(Ok(msg)) = stream.next().await {
        let Message::Text(text) = msg else {
            continue;
        };
        let Ok(parsed) = serde_json::from_str::<FromDevice>(&text) else {
            continue;
        };
        match parsed {
            FromDevice::Heartbeat => {
                let _ = sqlx::query(&db::q(
                    installed.kind,
                    &format!(
                        "UPDATE agent_devices SET last_seen_at = {} WHERE id = ?",
                        db::now_expr(installed.kind)
                    ),
                ))
                .bind(device_id)
                .execute(&installed.pool)
                .await;
            }
            FromDevice::Frame { session_id, frame } => {
                // Only forward into a session this device is actually bound
                // to; otherwise a compromised device could inject frames into
                // another session, including another user's.
                let sender = frame_senders.read().await.get(&session_id).cloned();
                if let Some(sender) = sender {
                    let _ = sender.send(frame).await;
                }
            }
            FromDevice::RuntimeClosed { session_id, reason } => {
                // Dropping the frame channel ends the session pump, which
                // performs the same teardown as a sandbox exiting.
                if let Some(reason) = reason {
                    eprintln!(
                        "[agent-device] device {device_id} session {session_id} runtime closed: {reason}"
                    );
                }
                frame_senders.write().await.remove(&session_id);
            }
            FromDevice::RuntimeError {
                session_id,
                message,
            } => {
                eprintln!("[agent-device] device {device_id} session {session_id}: {message}");
                frame_senders.write().await.remove(&session_id);
            }
            FromDevice::Hello { .. } => { /* already handled */ }
        }
    }

    // Disconnected. Drop every session's pump so the sessions do not sit
    // "live" against a machine that is gone.
    state.agent_device_frames.write().await.remove(&device_id);
    frame_senders.write().await.clear();
    state.agent_devices.remove(device_id).await;
    writer.abort();
    eprintln!("[agent-device] device {device_id} disconnected");
}

/// Register a session's frame channel with its device.
///
/// Returns false when the device is not connected, which the caller reports
/// rather than silently starting a session that can never receive anything.
pub async fn register_session_frames(
    state: &AppState,
    device_id: i64,
    session_id: i64,
    sender: mpsc::Sender<Value>,
) -> bool {
    let map = state.agent_device_frames.read().await.get(&device_id).cloned();
    match map {
        Some(map) => {
            map.write().await.insert(session_id, sender);
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_codes_are_namespaced_away_from_gateway_tokens() {
        // A device pairing code and an agent gateway token authenticate
        // different things; making them interchangeable would let one stand in
        // for the other.
        let code = new_pair_code();
        assert!(code.starts_with(PAIR_PREFIX));
        assert!(!code.starts_with("yna_"));
        assert_ne!(new_pair_code(), new_pair_code());
    }

    #[test]
    fn device_frames_deserialize_from_the_client_wire_format() {
        let hello: FromDevice = serde_json::from_str(
            r#"{"type":"hello","token":"ynd_x","name":"laptop","platform":"linux"}"#,
        )
        .unwrap();
        match hello {
            FromDevice::Hello {
                token,
                name,
                platform,
            } => {
                assert_eq!(token, "ynd_x");
                assert_eq!(name, "laptop");
                assert_eq!(platform.as_deref(), Some("linux"));
            }
            other => panic!("expected hello, got {other:?}"),
        }

        let frame: FromDevice =
            serde_json::from_str(r#"{"type":"frame","session_id":7,"frame":{"type":"response"}}"#)
                .unwrap();
        match frame {
            FromDevice::Frame { session_id, frame } => {
                assert_eq!(session_id, 7);
                assert_eq!(frame["type"], "response");
            }
            other => panic!("expected frame, got {other:?}"),
        }

        // `platform` is optional so an older client still connects.
        let minimal: FromDevice =
            serde_json::from_str(r#"{"type":"hello","token":"ynd_x","name":"pc"}"#).unwrap();
        assert!(matches!(minimal, FromDevice::Hello { .. }));
    }

    #[test]
    fn server_frames_serialize_with_the_credential_the_device_needs() {
        let msg = ToDevice::StartRuntime {
            session_id: 3,
            models_json: json!({"providers":{"yunova-claude":{"apiKey":"yna_t"}}}),
        };
        let v: Value = serde_json::from_str(&serde_json::to_string(&msg).unwrap()).unwrap();
        assert_eq!(v["type"], "start_runtime");
        assert_eq!(v["session_id"], 3);
        // The device must receive a gateway-scoped credential, never an
        // upstream provider key.
        assert_eq!(v["models_json"]["providers"]["yunova-claude"]["apiKey"], "yna_t");
    }

    #[tokio::test]
    async fn the_transport_relays_a_framed_line_as_structured_json() {
        let (tx, mut rx) = mpsc::channel::<ToDevice>(4);
        let device = Arc::new(DeviceHandle { user_id: 1, tx });
        let transport = DeviceTransport::new(device, 9);

        // The session layer hands over an encoded JSONL line, newline included.
        transport
            .send("{\"id\":\"s9-1\",\"type\":\"prompt\",\"message\":\"hi\"}\n".to_string())
            .await
            .unwrap();

        match rx.recv().await.unwrap() {
            ToDevice::Frame { session_id, frame } => {
                assert_eq!(session_id, 9);
                // Re-parsed rather than forwarded as a string, so the device
                // writes back exactly what the runtime expects.
                assert_eq!(frame["type"], "prompt");
                assert_eq!(frame["message"], "hi");
            }
            other => panic!("expected a frame, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn shutdown_asks_the_device_to_stop_that_session_only() {
        let (tx, mut rx) = mpsc::channel::<ToDevice>(4);
        let device = Arc::new(DeviceHandle { user_id: 1, tx });
        DeviceTransport::new(device, 42).shutdown().await;
        match rx.recv().await.unwrap() {
            ToDevice::StopRuntime { session_id } => assert_eq!(session_id, 42),
            other => panic!("expected stop_runtime, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_disconnected_device_fails_sends_instead_of_hanging() {
        let (tx, rx) = mpsc::channel::<ToDevice>(1);
        drop(rx);
        let device = Arc::new(DeviceHandle { user_id: 1, tx });
        let err = DeviceTransport::new(device, 1)
            .send("{\"type\":\"abort\"}\n".to_string())
            .await
            .expect_err("a dead socket must surface as an error");
        assert!(err.contains("断开"));
    }

    #[tokio::test]
    async fn the_registry_tracks_liveness_per_device() {
        let reg = DeviceRegistry::new();
        let (tx, _rx) = mpsc::channel::<ToDevice>(1);
        assert!(!reg.is_online(5).await);
        reg.insert(5, Arc::new(DeviceHandle { user_id: 1, tx })).await;
        assert!(reg.is_online(5).await);
        assert!(reg.get(5).await.is_some());
        reg.remove(5).await;
        assert!(!reg.is_online(5).await, "a disconnected device must not look available");
    }
}
