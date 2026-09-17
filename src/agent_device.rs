//! Device connector: the user's own computer as an execution target.
//!
//! The desktop client dials in and holds a WebSocket open, rather than the
//! server connecting to it. A personal machine is usually behind NAT with no
//! reachable address, so an outbound connection is the only option that works
//! without port forwarding.
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
//!
//! A machine attaches by signing in with the user's own account, and the
//! server binds it on the spot. Pairing codes are gone: they only existed to
//! name a machine the server had never seen, and the account already answers
//! "whose computer is this". What the client stores afterwards is a
//! device-scoped token, not the password, so a stolen config file attaches one
//! machine rather than taking over the account.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::{Extension, Json, Router};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{RwLock, mpsc};

use crate::agent_session::AgentTransport;
use crate::db::{self, DbKind, Pool};
use crate::{AppState, CurrentUser, InstalledState};

/// Namespaces a device token so it cannot be confused with a gateway agent
/// token: the two authenticate different things and must never be
/// interchangeable.
const DEVICE_PREFIX: &str = "ynd_";

const MAX_DEVICES_PER_USER: i64 = 20;

/// Frames from the desktop client.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FromDevice {
    /// First frame when the client already holds a device token.
    Hello {
        token: String,
        name: String,
        #[serde(default)]
        platform: Option<String>,
    },
    /// First frame when the client has no token yet: the user's account
    /// credentials, which bind this machine and mint one.
    ///
    /// Carried on this socket rather than a separate HTTP endpoint so there is
    /// exactly one place a device authenticates, and so the client needs no
    /// second protocol stack just to sign in.
    Login {
        username: String,
        password: String,
        name: String,
        #[serde(default)]
        platform: Option<String>,
        /// Stable machine identifier. Decides *which* of the user's computers
        /// this is, so re-running the client rebinds instead of registering a
        /// duplicate; it never decides whether the connection is allowed.
        #[serde(default)]
        fingerprint: Option<String>,
    },
    /// First frame from the desktop app when the site window it embeds is
    /// already signed in: the browser session token stands in for the
    /// password.
    ///
    /// This is what makes the app attach by itself. The alternative — asking
    /// for the account password a second time, in a second window, after the
    /// user already signed in to the page — is why a freshly installed client
    /// used to sit there while the web UI reported the machine as offline.
    /// The session token is strictly weaker than the password: it is already
    /// in that webview, it expires, and revoking the session revokes this
    /// path with it.
    Attach {
        session: String,
        name: String,
        #[serde(default)]
        platform: Option<String>,
        #[serde(default)]
        fingerprint: Option<String>,
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
    HelloOk {
        device_id: i64,
        /// Present only right after a password login: the device token the
        /// client should store so it never has to hold the password. Rotated
        /// on every login, so whatever was on that disk before stops working.
        #[serde(skip_serializing_if = "Option::is_none")]
        token: Option<String>,
        /// Which account this machine is bound to. Sent with a freshly minted
        /// token because the client may not know it: attaching through the
        /// site's session never asks for a username, and a panel that then
        /// says "已绑定" with no name reads as a half-finished bind.
        #[serde(skip_serializing_if = "Option::is_none")]
        username: Option<String>,
    },
    Error {
        message: String,
    },
    /// Start a local runtime for this session.
    ///
    /// Carries the generated `models.json` so the device never holds an
    /// upstream provider key: it points at this server's gateway with a
    /// session-scoped token, exactly as the sandbox does.
    StartRuntime {
        session_id: i64,
        models_json: Value,
    },
    /// One JSONL record for the local runtime's stdin, verbatim.
    Frame {
        session_id: i64,
        frame: Value,
    },
    /// Stop the local runtime for this session.
    StopRuntime {
        session_id: i64,
    },
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
// sign-in binding and management
// ---------------------------------------------------------------------------

fn new_device_token() -> String {
    format!("{DEVICE_PREFIX}{}", crate::auth::generate_token())
}

/// Trim a user-supplied machine name to something a list can render.
fn clean_name(raw: Option<&str>) -> String {
    raw.map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or("我的电脑")
        .chars()
        .take(60)
        .collect()
}

/// A fingerprint identifies *which* of the user's machines this is, so it is
/// only ever used to pick a row to reuse. Length-capped because it arrives
/// from the client and is stored.
fn clean_fingerprint(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|f| !f.is_empty())
        .map(|f| f.chars().take(128).collect())
}

async fn live_device_count(installed: &InstalledState, user_id: i64) -> i64 {
    let sql = db::q(
        installed.kind,
        &format!(
            "SELECT COUNT(*) FROM agent_devices WHERE user_id = ? AND revoked = {}",
            match installed.kind {
                DbKind::Postgres => "FALSE",
                _ => "0",
            }
        ),
    );
    sqlx::query_scalar(&sql)
        .bind(user_id)
        .fetch_one(&installed.pool)
        .await
        .unwrap_or(0)
}

/// Attach this machine to `user_id`, returning its id and a fresh token.
///
/// Re-running the client on a machine it already bound must not pile up
/// duplicate rows, so a fingerprint match rebinds the existing device. The
/// token is rotated on every bind: the client is about to store the new one,
/// and a login is exactly the moment where invalidating whatever was on that
/// disk before is the safe choice.
async fn bind_device(
    installed: &InstalledState,
    user_id: i64,
    name: &str,
    platform: Option<&str>,
    fingerprint: Option<&str>,
) -> Result<(i64, String, bool), String> {
    let token = new_device_token();
    let hash = crate::auth::token_hash(&token);

    let existing: Option<i64> = match fingerprint {
        Some(fp) => {
            let sql = db::q(
                installed.kind,
                "SELECT id FROM agent_devices WHERE user_id = ? AND fingerprint = ?",
            );
            sqlx::query_scalar(&sql)
                .bind(user_id)
                .bind(fp)
                .fetch_optional(&installed.pool)
                .await
                .ok()
                .flatten()
        }
        None => None,
    };

    if let Some(id) = existing {
        // Re-attaching a known machine also un-revokes it: the user just
        // proved account ownership from that computer, which is a stronger
        // signal than the earlier "remove" click.
        let sql = db::q(
            installed.kind,
            &format!(
                "UPDATE agent_devices SET token_hash = ?, name = ?, platform = ?, \
                 revoked = {}, last_seen_at = {} WHERE id = ? AND user_id = ?",
                match installed.kind {
                    DbKind::Postgres => "FALSE",
                    _ => "0",
                },
                db::now_expr(installed.kind)
            ),
        );
        sqlx::query(&sql)
            .bind(&hash)
            .bind(name)
            .bind(platform)
            .bind(id)
            .bind(user_id)
            .execute(&installed.pool)
            .await
            .map_err(|e| {
                eprintln!("[agent-device] rebind failed: {e}");
                "绑定设备失败".to_string()
            })?;
        return Ok((id, token, false));
    }

    // Only a *new* machine consumes quota; rebinding one the user already has
    // must keep working even at the cap.
    if live_device_count(installed, user_id).await >= MAX_DEVICES_PER_USER {
        return Err(format!(
            "最多只能绑定 {MAX_DEVICES_PER_USER} 台电脑，请先在网页移除不用的"
        ));
    }

    let sql = db::q(
        installed.kind,
        &format!(
            "INSERT INTO agent_devices (user_id, kind, name, token_hash, platform, fingerprint, last_seen_at) \
             VALUES (?, 'device', ?, ?, ?, ?, {})",
            db::now_expr(installed.kind)
        ),
    );
    sqlx::query(&sql)
        .bind(user_id)
        .bind(name)
        .bind(&hash)
        .bind(platform)
        .bind(fingerprint)
        .execute(&installed.pool)
        .await
        .map_err(|e| {
            eprintln!("[agent-device] bind insert failed: {e}");
            "绑定设备失败".to_string()
        })?;

    let id_sql = db::q(
        installed.kind,
        "SELECT id FROM agent_devices WHERE token_hash = ?",
    );
    let id: i64 = sqlx::query_scalar(&id_sql)
        .bind(&hash)
        .fetch_one(&installed.pool)
        .await
        .map_err(|e| {
            eprintln!("[agent-device] bind lookup failed: {e}");
            "绑定设备失败".to_string()
        })?;
    Ok((id, token, true))
}

/// Verify account credentials, returning the user id.
///
/// Deliberately returns an `Option`: the caller must not be able to tell
/// "unknown user" from "wrong password" apart, or this becomes an account
/// enumeration oracle.
async fn verify_login(
    installed: &InstalledState,
    username: &str,
    password: &str,
) -> Option<i64> {
    let sel = db::q(
        installed.kind,
        &format!(
            "SELECT id, password_hash FROM users WHERE {}",
            db::ci_eq(installed.kind, "username")
        ),
    );
    let row: Option<(i64, String)> = sqlx::query_as(&sel)
        .bind(username.trim())
        .fetch_optional(&installed.pool)
        .await
        .ok()
        .flatten();
    let (id, phc) = row?;
    crate::auth::verify_password(password, &phc).then_some(id)
}

#[derive(Deserialize)]
struct RenameReq {
    name: String,
}

/// Rename a device from the web UI.
///
/// The name now arrives from the machine itself, so the only way to correct a
/// hostname like `DESKTOP-4F2K1A` is here.
async fn rename_device(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(req): Json<RenameReq>,
) -> Response {
    let name = clean_name(Some(&req.name));
    let sql = db::q(
        installed.kind,
        "UPDATE agent_devices SET name = ? WHERE id = ? AND user_id = ?",
    );
    match sqlx::query(&sql)
        .bind(&name)
        .bind(id)
        .bind(user.id)
        .execute(&installed.pool)
        .await
    {
        Ok(r) if r.rows_affected() == 0 => (StatusCode::NOT_FOUND, "设备不存在").into_response(),
        Ok(_) => Json(json!({ "id": id, "name": name })).into_response(),
        Err(e) => {
            eprintln!("[agent-device] rename failed: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "重命名失败").into_response()
        }
    }
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
        .route("/agent/devices/{id}", delete(revoke_device).patch(rename_device))
}

pub fn public_routes() -> Router<AppState> {
    // Sign-in happens on the socket itself (see `FromDevice::Login`), so a
    // device needs exactly one endpoint and one protocol.
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
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    // The peer address is captured before the upgrade because a password may
    // cross this socket: the login path shares the browser's rate limiter, and
    // after the upgrade there are no request headers left to read it from.
    let ip = crate::rate_limit::client_ip(&headers);
    ws.on_upgrade(move |socket| handle_socket(state, socket, q.token, ip))
}

/// Resolve a device token to its device, refusing revoked ones.
async fn device_for_token(pool: &Pool, kind: DbKind, token: &str) -> Option<(i64, i64)> {
    if !token.starts_with(DEVICE_PREFIX) {
        return None;
    }
    let hash = crate::auth::token_hash(token);
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

/// The outcome of a device handshake.
struct Attached {
    device_id: i64,
    user_id: i64,
    /// Set when the handshake minted one, so the client can store a token and
    /// stop holding whatever credential it arrived with.
    issued_token: Option<String>,
    /// The account name, reported alongside a freshly minted token because a
    /// client that attached through the site's session never typed one.
    username: Option<String>,
}

/// Authenticate the first frame, binding the machine when it is a login.
///
/// Both paths end in the same place — a device row this socket may act as —
/// so everything after the handshake is identical regardless of how the client
/// arrived.
async fn attach(
    state: &AppState,
    installed: &InstalledState,
    hello: FromDevice,
    query_token: Option<String>,
    ip: std::net::IpAddr,
) -> Result<(Attached, String, Option<String>), String> {
    match hello {
        FromDevice::Hello {
            token,
            name,
            platform,
        } => {
            // A token may also arrive as a query parameter, which some client
            // HTTP stacks handle more easily than custom headers on an
            // upgrade request.
            let token = if token.is_empty() {
                query_token.unwrap_or_default()
            } else {
                token
            };
            let (device_id, user_id) =
                device_for_token(&installed.pool, installed.kind, &token)
                    .await
                    .ok_or("设备凭证已失效，请在客户端重新登录")?;
            Ok((
                Attached {
                    device_id,
                    user_id,
                    issued_token: None,
                    username: None,
                },
                name,
                platform,
            ))
        }
        FromDevice::Login {
            username,
            password,
            name,
            platform,
            fingerprint,
        } => {
            // Same limiter as the browser's login: this is the same credential
            // surface, and an unthrottled second door would undo the first
            // one's protection.
            if !state.auth_limiter.allow(ip).await {
                return Err("登录过于频繁，请稍后再试".into());
            }
            let user_id = verify_login(installed, &username, &password)
                .await
                // One message for "no such user" and "wrong password", so the
                // socket cannot be used to enumerate accounts.
                .ok_or("用户名或密码错误")?;
            let clean = clean_name(Some(&name));
            let (device_id, token, created) = bind_device(
                installed,
                user_id,
                &clean,
                platform.as_deref(),
                clean_fingerprint(fingerprint.as_deref()).as_deref(),
            )
            .await?;
            eprintln!(
                "[agent-device] {} device {device_id} for user {user_id} via sign-in",
                if created { "bound new" } else { "rebound" }
            );
            Ok((
                Attached {
                    device_id,
                    user_id,
                    issued_token: Some(token),
                    username: Some(username.trim().to_string()),
                },
                clean,
                platform,
            ))
        }
        FromDevice::Attach {
            session,
            name,
            platform,
            fingerprint,
        } => {
            // Rate limited like the password door: this is still a path from
            // an arbitrary socket to a bound device, so guessing at session
            // tokens must cost the same as guessing at passwords.
            if !state.auth_limiter.allow(ip).await {
                return Err("请求过于频繁，请稍后再试".into());
            }
            let (user_id, username) =
                crate::auth::user_for_token(&installed.pool, installed.kind, session.trim())
                    .await
                    // Expired or unknown: the client falls back to asking the
                    // user to sign in, so this must read as "sign in again"
                    // rather than as a broken install.
                    .ok_or("登录状态已失效，请在窗口重新登录")?;
            let clean = clean_name(Some(&name));
            let (device_id, token, created) = bind_device(
                installed,
                user_id,
                &clean,
                platform.as_deref(),
                clean_fingerprint(fingerprint.as_deref()).as_deref(),
            )
            .await?;
            eprintln!(
                "[agent-device] {} device {device_id} for user {user_id} via web session",
                if created { "bound new" } else { "rebound" }
            );
            Ok((
                Attached {
                    device_id,
                    user_id,
                    issued_token: Some(token),
                    username: Some(username),
                },
                clean,
                platform,
            ))
        }
        _ => Err("首帧必须是 hello、attach 或 login".into()),
    }
}

async fn handle_socket(
    state: AppState,
    socket: WebSocket,
    query_token: Option<String>,
    ip: std::net::IpAddr,
) {
    let (mut sink, mut stream) = socket.split();

    let installed = match state.installed.read().await.clone() {
        Some(i) => i,
        None => return,
    };

    // The first frame must authenticate: either with a stored device token or
    // with the user's account, which binds this machine on the spot.
    let first = match stream.next().await {
        Some(Ok(Message::Text(t))) => t.to_string(),
        _ => return,
    };
    let Ok(hello) = serde_json::from_str::<FromDevice>(&first) else {
        return;
    };

    let (attached, name, platform) =
        match attach(&state, &installed, hello, query_token, ip).await {
            Ok(v) => v,
            Err(message) => {
                let err =
                    serde_json::to_string(&ToDevice::Error { message }).unwrap_or_default();
                let _ = sink.send(Message::Text(err.into())).await;
                return;
            }
        };
    let Attached {
        device_id,
        user_id,
        issued_token,
        username,
    } = attached;

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

    let ok = serde_json::to_string(&ToDevice::HelloOk {
        device_id,
        token: issued_token,
        username,
    })
    .unwrap_or_default();
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
            FromDevice::Hello { .. } | FromDevice::Attach { .. } | FromDevice::Login { .. } => {
                // Already handled during the handshake. Re-authenticating on
                // a live socket is ignored rather than honoured: rebinding
                // mid-stream would move sessions under the running runtimes.
            }
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

    /// An installed instance backed by an in-memory database.
    async fn installed() -> InstalledState {
        crate::db::install_drivers();
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::migrate(&pool, DbKind::Sqlite).await.unwrap();
        InstalledState {
            pool,
            kind: DbKind::Sqlite,
        }
    }

    async fn user_with_session(installed: &InstalledState) -> (i64, String) {
        let hash = crate::auth::hash_password("pw").unwrap();
        sqlx::query(&db::q(
            installed.kind,
            "INSERT INTO users (username, password_hash, is_admin) VALUES (?, ?, 0)",
        ))
        .bind("orca")
        .bind(&hash)
        .execute(&installed.pool)
        .await
        .unwrap();
        let user_id: i64 = sqlx::query_scalar(&db::q(
            installed.kind,
            "SELECT id FROM users WHERE username = ?",
        ))
        .bind("orca")
        .fetch_one(&installed.pool)
        .await
        .unwrap();
        let (token, _) = crate::auth::create_session(&installed.pool, installed.kind, user_id)
            .await
            .unwrap();
        (user_id, token)
    }

    #[tokio::test]
    async fn a_web_session_identifies_the_account_a_machine_binds_to() {
        // The desktop app attaches with the session its own window already
        // holds, so this is the credential path that replaces asking for the
        // password a second time.
        let installed = installed().await;
        let (user_id, session) = user_with_session(&installed).await;

        assert_eq!(
            crate::auth::user_for_token(&installed.pool, installed.kind, &session)
                .await
                .map(|(id, _)| id),
            Some(user_id)
        );
        // An unknown session must resolve to nobody, or the socket would be a
        // way to attach to an account without any credential at all.
        assert!(
            crate::auth::user_for_token(&installed.pool, installed.kind, "not-a-session")
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn attaching_on_every_launch_rebinds_one_machine() {
        // The app now attaches by itself each time it starts, so a device row
        // per launch would fill the user's list and burn the 20-device cap
        // within a week.
        let installed = installed().await;
        let (user_id, _) = user_with_session(&installed).await;

        let (first, first_token, created) =
            bind_device(&installed, user_id, "laptop", Some("linux"), Some("fp"))
                .await
                .unwrap();
        assert!(created);
        let (again, second_token, created_again) =
            bind_device(&installed, user_id, "laptop", Some("linux"), Some("fp"))
                .await
                .unwrap();
        assert_eq!(first, again, "the same machine must reuse its device row");
        assert!(!created_again);
        assert_eq!(live_device_count(&installed, user_id).await, 1);
        // The token rotates, so whatever was on that disk before stops
        // working even though the row is reused.
        assert_ne!(first_token, second_token);
        assert!(
            device_for_token(&installed.pool, installed.kind, &first_token)
                .await
                .is_none()
        );
        assert_eq!(
            device_for_token(&installed.pool, installed.kind, &second_token).await,
            Some((again, user_id))
        );
    }

    #[test]
    fn device_tokens_are_namespaced_away_from_gateway_tokens() {
        // A device token and an agent gateway token authenticate different
        // things; making them interchangeable would let one stand in for the
        // other.
        let token = new_device_token();
        assert!(token.starts_with(DEVICE_PREFIX));
        assert!(!token.starts_with("yna_"));
        assert_ne!(new_device_token(), new_device_token());
    }

    #[test]
    fn a_machine_name_always_renders_as_something() {
        // The name now comes from the machine, so blank and oversized values
        // both have to be survivable rather than reaching the device list.
        assert_eq!(clean_name(None), "我的电脑");
        assert_eq!(clean_name(Some("   ")), "我的电脑");
        assert_eq!(clean_name(Some("  laptop ")), "laptop");
        assert_eq!(clean_name(Some(&"x".repeat(200))).chars().count(), 60);
        // Multi-byte names must be truncated by character, not by byte, or the
        // stored value would be invalid UTF-8 at the boundary.
        assert_eq!(clean_name(Some(&"电".repeat(80))).chars().count(), 60);
    }

    #[test]
    fn a_fingerprint_is_optional_and_bounded() {
        // Absent means "cannot be matched by machine", which is the old
        // paired-device behaviour and must stay representable.
        assert_eq!(clean_fingerprint(None), None);
        assert_eq!(clean_fingerprint(Some("  ")), None);
        assert_eq!(clean_fingerprint(Some(" abc ")).as_deref(), Some("abc"));
        assert_eq!(
            clean_fingerprint(Some(&"f".repeat(400)))
                .map(|f| f.chars().count()),
            Some(128)
        );
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

        // The desktop app's automatic path: the window is already signed in,
        // so it attaches with that session instead of asking again.
        let attach: FromDevice = serde_json::from_str(
            r#"{"type":"attach","session":"deadbeef","name":"pc","fingerprint":"fp"}"#,
        )
        .unwrap();
        match attach {
            FromDevice::Attach {
                session,
                name,
                platform,
                fingerprint,
            } => {
                assert_eq!(session, "deadbeef");
                assert_eq!(name, "pc");
                assert!(platform.is_none());
                assert_eq!(fingerprint.as_deref(), Some("fp"));
            }
            other => panic!("expected attach, got {other:?}"),
        }

        // The sign-in frame is the replacement for pairing: an account plus
        // the machine it is being bound to.
        let login: FromDevice = serde_json::from_str(
            r#"{"type":"login","username":"orca","password":"s","name":"pc","fingerprint":"fp"}"#,
        )
        .unwrap();
        match login {
            FromDevice::Login {
                username,
                password,
                name,
                platform,
                fingerprint,
            } => {
                assert_eq!(username, "orca");
                assert_eq!(password, "s");
                assert_eq!(name, "pc");
                assert!(platform.is_none());
                assert_eq!(fingerprint.as_deref(), Some("fp"));
            }
            other => panic!("expected login, got {other:?}"),
        }
    }

    #[test]
    fn a_token_is_only_announced_when_one_was_just_minted() {
        // A reconnect must not restate the credential the client already has:
        // the client would rewrite its config file on every reconnect, and the
        // token would appear in logs that only needed a device id.
        let plain = serde_json::to_string(&ToDevice::HelloOk {
            device_id: 3,
            token: None,
            username: None,
        })
        .unwrap();
        let v: Value = serde_json::from_str(&plain).unwrap();
        assert_eq!(v["device_id"], 3);
        assert!(v.get("token").is_none());

        let fresh = serde_json::to_string(&ToDevice::HelloOk {
            device_id: 3,
            token: Some("ynd_new".into()),
            username: Some("orca".into()),
        })
        .unwrap();
        let v: Value = serde_json::from_str(&fresh).unwrap();
        assert_eq!(v["token"], "ynd_new");
        // The client may have attached with a session and never seen a
        // username, so the bind has to report which account it landed on.
        assert_eq!(v["username"], "orca");
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
