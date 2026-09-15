//! HTTP surface for agent sessions.
//!
//! Deliberately split from the legacy `worker` module: that one couples the
//! thinking loop, the tool protocol and the transcript into one flow. Here the
//! server only relays — the runtime owns the loop — so these handlers are
//! identical for the cloud sandbox and for a user's own machine.
//!
//! The event stream is a *subscription*, not a response to a prompt. That is
//! what lets a phone, a browser and the desktop client watch the same session:
//! each opens its own stream, and prompts from any of them land in the same
//! place.

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use futures_util::stream::Stream;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::broadcast;

use crate::agent_rpc::{self, Inbound};
use crate::agent_session::{SessionEvent, Target};
use crate::db::{self, DbKind, Pool};
use crate::{AppState, CurrentUser, InstalledState};

// ---------------------------------------------------------------------------
// ownership checks
// ---------------------------------------------------------------------------

/// Resolve a session the caller owns.
///
/// Every handler goes through this. A session can drive shell commands on the
/// user's own machine, so "not yours" must be indistinguishable from "does not
/// exist" — otherwise the ids of other users' sessions become probeable.
async fn owned_session(
    pool: &Pool,
    kind: DbKind,
    user_id: i64,
    session_id: i64,
) -> Result<(String, Option<i64>), Response> {
    let row: Option<(i64, String, Option<i64>)> = sqlx::query_as(&db::q(
        kind,
        "SELECT user_id, target, device_id FROM agent_sessions WHERE id = ?",
    ))
    .bind(session_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    match row {
        Some((uid, target, device_id)) if uid == user_id => Ok((target, device_id)),
        _ => Err((StatusCode::NOT_FOUND, "会话不存在").into_response()),
    }
}

// ---------------------------------------------------------------------------
// session CRUD
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CreateSessionReq {
    /// "cloud" or "device".
    target: String,
    /// Required when `target` is "device".
    #[serde(default)]
    device_id: Option<i64>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    model: Option<String>,
}

async fn create_session(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Json(req): Json<CreateSessionReq>,
) -> Response {
    let Some(target) = Target::parse(&req.target) else {
        return (StatusCode::BAD_REQUEST, "target 必须是 cloud 或 device").into_response();
    };

    // A device session is meaningless without a machine to run on, and the
    // machine must belong to the caller: otherwise a session could be pointed
    // at someone else's computer.
    if target == Target::Device {
        let Some(device_id) = req.device_id else {
            return (StatusCode::BAD_REQUEST, "本地电脑任务需要指定设备").into_response();
        };
        let owns: Option<(i64,)> = sqlx::query_as(&db::q(
            installed.kind,
            "SELECT id FROM agent_devices WHERE id = ? AND user_id = ? AND revoked = 0",
        ))
        .bind(device_id)
        .bind(user.id)
        .fetch_optional(&installed.pool)
        .await
        .ok()
        .flatten();
        if owns.is_none() {
            return (StatusCode::NOT_FOUND, "设备不存在或无权访问").into_response();
        }
    }

    let title = req
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or("新任务")
        .chars()
        .take(80)
        .collect::<String>();

    let insert = db::q(
        installed.kind,
        "INSERT INTO agent_sessions (user_id, target, device_id, title, model) \
         VALUES (?, ?, ?, ?, ?)",
    );
    let id = match installed.kind {
        DbKind::Sqlite | DbKind::Postgres => {
            sqlx::query_as::<_, (i64,)>(&format!("{insert} RETURNING id"))
                .bind(user.id)
                .bind(target.as_str())
                .bind(req.device_id)
                .bind(&title)
                .bind(req.model.as_deref())
                .fetch_one(&installed.pool)
                .await
                .map(|r| r.0)
                .map_err(|e| e.to_string())
        }
        DbKind::Mysql => {
            async {
                let mut tx = installed.pool.begin().await.map_err(|e| e.to_string())?;
                sqlx::query(&insert)
                    .bind(user.id)
                    .bind(target.as_str())
                    .bind(req.device_id)
                    .bind(&title)
                    .bind(req.model.as_deref())
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| e.to_string())?;
                let (id,): (i64,) = sqlx::query_as("SELECT LAST_INSERT_ID()")
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(|e| e.to_string())?;
                tx.commit().await.map_err(|e| e.to_string())?;
                Ok(id)
            }
            .await
        }
    };

    match id {
        Ok(id) => Json(json!({ "id": id, "target": target.as_str(), "title": title })).into_response(),
        Err(e) => {
            eprintln!("[agent-api] create session failed: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "创建会话失败").into_response()
        }
    }
}

/// One row of the session list query.
type SessionRow = (
    i64,            // id
    String,         // target
    Option<i64>,    // device_id
    String,         // title
    Option<String>, // model
    String,         // status
    String,         // updated_at
);

async fn list_sessions(
    State(state): State<AppState>,
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
) -> Response {
    let rows: Vec<SessionRow> = sqlx::query_as(&db::q(
        installed.kind,
        "SELECT id, target, device_id, title, model, status, updated_at \
         FROM agent_sessions WHERE user_id = ? ORDER BY updated_at DESC",
    ))
    .bind(user.id)
    .fetch_all(&installed.pool)
    .await
    .unwrap_or_default();

    let mut out = Vec::with_capacity(rows.len());
    for (id, target, device_id, title, model, status, updated_at) in rows {
        out.push(json!({
            "id": id,
            "target": target,
            "device_id": device_id,
            "title": title,
            "model": model,
            "status": status,
            "updated_at": updated_at,
            // Whether a runtime is attached right now. Distinct from status:
            // an idle session can still hold a warm runtime.
            "live": state.agent_sessions.is_live(id).await,
        }));
    }
    Json(out).into_response()
}

/// Return the mirrored transcript.
///
/// `since` takes an entry id, matching the runtime's own cursor semantics, so
/// a reconnecting client asks for exactly what it is missing instead of
/// refetching the whole history.
#[derive(Deserialize)]
struct EntriesQuery {
    #[serde(default)]
    since: Option<String>,
}

async fn list_entries(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(sid): Path<i64>,
    axum::extract::Query(q): axum::extract::Query<EntriesQuery>,
) -> Response {
    if let Err(r) = owned_session(&installed.pool, installed.kind, user.id, sid).await {
        return r;
    }

    // Resolve the cursor to its row id, then select strictly after it. Using
    // the autoincrement id preserves append order, which is what the runtime's
    // `since` is defined against.
    let after: i64 = match q.since.as_deref() {
        Some(entry_id) => sqlx::query_scalar(&db::q(
            installed.kind,
            "SELECT id FROM agent_entries WHERE session_id = ? AND entry_id = ?",
        ))
        .bind(sid)
        .bind(entry_id)
        .fetch_optional(&installed.pool)
        .await
        .ok()
        .flatten()
        .unwrap_or(0),
        None => 0,
    };

    let rows: Vec<(String, Option<String>, String, String)> = sqlx::query_as(&db::q(
        installed.kind,
        "SELECT entry_id, parent_id, kind, payload FROM agent_entries \
         WHERE session_id = ? AND id > ? ORDER BY id",
    ))
    .bind(sid)
    .bind(after)
    .fetch_all(&installed.pool)
    .await
    .unwrap_or_default();

    let entries: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(entry_id, parent_id, kind, payload)| {
            // The payload is the runtime's own entry; hand it back verbatim so
            // the client renders the same shape pi produced.
            serde_json::from_str::<serde_json::Value>(&payload).unwrap_or_else(|_| {
                json!({ "id": entry_id, "parentId": parent_id, "type": kind })
            })
        })
        .collect();
    Json(json!({ "entries": entries })).into_response()
}

// ---------------------------------------------------------------------------
// live event subscription
// ---------------------------------------------------------------------------

/// Subscribe to a session's live event stream.
///
/// Every client opens its own subscription, which is what makes a session
/// visible from the browser, the desktop app and a phone at the same time. A
/// client that falls too far behind receives a `resync` hint instead of
/// silently missing frames: the authoritative history is in the mirror, so it
/// can recover with `GET /entries?since=…`.
async fn session_events(
    State(state): State<AppState>,
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(sid): Path<i64>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, Response> {
    owned_session(&installed.pool, installed.kind, user.id, sid).await?;

    let Some(live) = state.agent_sessions.get(sid).await else {
        return Err((StatusCode::CONFLICT, "会话当前没有运行中的运行时").into_response());
    };

    let mut rx = live.subscribe();
    let pending = live.pending_approvals().await;

    let stream = async_stream::stream! {
        // Replay outstanding approvals first. A client that connects while the
        // runtime is already blocked would otherwise see no reason for the
        // stall and no way to unblock it.
        for req in pending {
            let data = serde_json::to_string(&req).unwrap_or_else(|_| "{}".into());
            yield Ok(Event::default().event("extension_ui_request").data(data));
        }

        loop {
            match rx.recv().await {
                Ok(SessionEvent::Frame(v)) => {
                    let name = v
                        .get("type")
                        .and_then(|t| t.as_str())
                        .unwrap_or("event")
                        .to_string();
                    let data = serde_json::to_string(&v).unwrap_or_else(|_| "{}".into());
                    yield Ok(Event::default().event(name).data(data));
                }
                Ok(SessionEvent::ApprovalResolved { request_id }) => {
                    let data = serde_json::to_string(&json!({ "id": request_id }))
                        .unwrap_or_else(|_| "{}".into());
                    yield Ok(Event::default().event("approval_resolved").data(data));
                }
                Ok(SessionEvent::Settled) => {
                    yield Ok(Event::default().event("settled").data("{}"));
                }
                Ok(SessionEvent::Closed { reason }) => {
                    let data = serde_json::to_string(&json!({ "reason": reason }))
                        .unwrap_or_else(|_| "{}".into());
                    yield Ok(Event::default().event("closed").data(data));
                    break;
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    // Frames were dropped for this subscriber only. Tell it to
                    // reconcile from the mirror rather than render a gap.
                    let data = serde_json::to_string(&json!({ "dropped": n }))
                        .unwrap_or_else(|_| "{}".into());
                    yield Ok(Event::default().event("resync").data(data));
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

// ---------------------------------------------------------------------------
// commands
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct PromptReq {
    message: String,
    /// "steer" or "followUp". Required when the agent is already streaming;
    /// pi rejects an unqualified prompt in that state.
    #[serde(default)]
    streaming_behavior: Option<String>,
}

async fn prompt(
    State(state): State<AppState>,
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(sid): Path<i64>,
    Json(req): Json<PromptReq>,
) -> Response {
    if let Err(r) = owned_session(&installed.pool, installed.kind, user.id, sid).await {
        return r;
    }
    if req.message.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "消息不能为空").into_response();
    }
    let Some(live) = state.agent_sessions.get(sid).await else {
        return (StatusCode::CONFLICT, "会话当前没有运行中的运行时").into_response();
    };

    let behavior = req.streaming_behavior.as_deref().filter(|b| {
        // Guard the protocol: pi accepts only these two, and a typo would
        // otherwise surface as an opaque rejection.
        matches!(*b, "steer" | "followUp")
    });

    let cmd = agent_rpc::cmd_prompt("", &req.message, behavior);
    match live.request(cmd).await {
        Ok(Inbound::Response { success: true, .. }) => {
            set_status(&installed.pool, installed.kind, sid, "running").await;
            Json(json!({ "ok": true })).into_response()
        }
        Ok(Inbound::Response { error, .. }) => (
            StatusCode::BAD_REQUEST,
            error.unwrap_or_else(|| "运行时拒绝了该消息".into()),
        )
            .into_response(),
        Ok(_) => (StatusCode::BAD_GATEWAY, "运行时返回了意外响应").into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, e).into_response(),
    }
}

async fn abort(
    State(state): State<AppState>,
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(sid): Path<i64>,
) -> Response {
    if let Err(r) = owned_session(&installed.pool, installed.kind, user.id, sid).await {
        return r;
    }
    let Some(live) = state.agent_sessions.get(sid).await else {
        return (StatusCode::CONFLICT, "会话当前没有运行中的运行时").into_response();
    };
    match live.request(agent_rpc::cmd_abort("")).await {
        Ok(_) => {
            set_status(&installed.pool, installed.kind, sid, "idle").await;
            Json(json!({ "ok": true })).into_response()
        }
        Err(e) => (StatusCode::BAD_GATEWAY, e).into_response(),
    }
}

#[derive(Deserialize)]
struct ApproveReq {
    /// The `extension_ui_request` id being answered.
    request_id: String,
    /// For `confirm` dialogs.
    #[serde(default)]
    confirmed: Option<bool>,
    /// For `select`/`input`/`editor` dialogs.
    #[serde(default)]
    value: Option<String>,
    /// Dismiss the dialog.
    #[serde(default)]
    cancelled: bool,
}

/// Answer a blocking dialog from any client.
///
/// This is the cross-device approval path: the request is broadcast to every
/// subscriber, and whichever device answers first wins. `resolve_approval`
/// enforces once-only semantics so a race between two devices cannot send two
/// answers for one request.
async fn approve(
    State(state): State<AppState>,
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(sid): Path<i64>,
    Json(req): Json<ApproveReq>,
) -> Response {
    if let Err(r) = owned_session(&installed.pool, installed.kind, user.id, sid).await {
        return r;
    }
    let Some(live) = state.agent_sessions.get(sid).await else {
        return (StatusCode::CONFLICT, "会话当前没有运行中的运行时").into_response();
    };

    let answer = if req.cancelled {
        agent_rpc::extension_ui_cancel(&req.request_id)
    } else if let Some(c) = req.confirmed {
        agent_rpc::extension_ui_confirm(&req.request_id, c)
    } else if let Some(v) = req.value.as_deref() {
        agent_rpc::extension_ui_value(&req.request_id, v)
    } else {
        return (StatusCode::BAD_REQUEST, "需要提供 confirmed、value 或 cancelled").into_response();
    };

    match live.resolve_approval(&req.request_id, answer).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => (StatusCode::CONFLICT, e).into_response(),
    }
}

async fn set_status(pool: &Pool, kind: DbKind, session_id: i64, status: &str) {
    let _ = sqlx::query(&db::q(
        kind,
        "UPDATE agent_sessions SET status = ?, updated_at = ? WHERE id = ?",
    ))
    .bind(status)
    .bind(chrono::Utc::now().to_rfc3339())
    .bind(session_id)
    .execute(pool)
    .await;
}

// ---------------------------------------------------------------------------
// runtime lifecycle
// ---------------------------------------------------------------------------

/// Port the gateway listens on, as seen from inside a sandbox.
///
/// A container reaches the host through its `host-gateway` alias, so only the
/// port is needed; the hostname is supplied by the sandbox driver.
fn gateway_port() -> u16 {
    crate::runtime_env::var("YUNOVA_BIND")
        .ok()
        .and_then(|bind| bind.rsplit(':').next().and_then(|p| p.parse().ok()))
        .unwrap_or(3000)
}

/// Base URL a sandboxed runtime should call.
///
/// Distinct from [`gateway_base_url`]: a container cannot reach a
/// loopback-bound server through `127.0.0.1`, so the generated config must
/// name the gateway alias the sandbox driver adds on its internal network.
fn sandbox_gateway_url() -> String {
    crate::runtime_env::var("YUNOVA_SANDBOX_GATEWAY_URL")
        .unwrap_or_else(|_| format!("http://yunova-gateway:{}", gateway_port()))
}

/// Public origin the runtime should call back on.
///
/// The runtime reaches the gateway over HTTP like any other client, so it
/// needs an absolute URL. Defaults to loopback, which is correct for a
/// same-host runtime; a containerised runtime needs the address that resolves
/// from inside its network.
fn gateway_base_url() -> String {
    crate::runtime_env::var("YUNOVA_AGENT_GATEWAY_URL").unwrap_or_else(|_| {
        let bind = crate::runtime_env::var("YUNOVA_BIND")
            .unwrap_or_else(|_| "127.0.0.1:3000".to_string());
        format!("http://{bind}")
    })
}

/// Whether cloud sessions run in a container.
///
/// On by default: a work-mode task has a shell, and running that directly on
/// the host would put every user's agent in the same filesystem and network
/// namespace as the server. The escape hatch exists for development hosts
/// without a usable container runtime.
fn sandbox_enabled() -> bool {
    !matches!(
        crate::runtime_env::var("YUNOVA_SANDBOX")
            .unwrap_or_default()
            .as_str(),
        "0" | "off" | "false"
    )
}

/// Directory holding each session's private pi config.
fn session_runtime_root(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join("agent-runtimes")
}

/// Start a runtime for a session and attach it.
///
/// Only the cloud target is startable here: a device session's runtime lives
/// on the user's own machine and is attached when the desktop client dials in,
/// which is why this returns a clear error rather than silently running the
/// user's "local" task on the server.
async fn start_session(
    State(state): State<AppState>,
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(sid): Path<i64>,
) -> Response {
    let (target, _device_id) =
        match owned_session(&installed.pool, installed.kind, user.id, sid).await {
            Ok(v) => v,
            Err(r) => return r,
        };
    if Target::parse(&target) != Some(Target::Cloud) {
        return (
            StatusCode::BAD_REQUEST,
            "本地电脑任务由桌面客户端接入，不在服务器上启动",
        )
            .into_response();
    }

    // Reuse a warm runtime instead of starting a second one for the same
    // session, which would fork the transcript.
    if state.agent_sessions.is_live(sid).await {
        return Json(json!({ "ok": true, "reused": true })).into_response();
    }

    // Build the runtime's model config from the priced whitelist. This is the
    // security boundary: the runtime receives a gateway URL plus a
    // session-scoped token, never an upstream provider key.
    let prices = match crate::channels::list_pricing(&installed.pool, installed.kind).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[agent-api] pricing lookup failed: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "读取模型定价失败").into_response();
        }
    };
    let token = match crate::agent_token::mint(
        &installed.pool,
        installed.kind,
        user.id,
        &format!("session-{sid}"),
    )
    .await
    {
        Ok(t) => t,
        Err(e) => {
            eprintln!("[agent-api] minting a session token failed: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "创建运行时凭据失败").into_response();
        }
    };
    // A sandboxed runtime cannot reach a loopback-bound gateway, so the URL
    // baked into its config differs by target.
    let sandboxed = sandbox_enabled();
    let base_url = if sandboxed {
        sandbox_gateway_url()
    } else {
        gateway_base_url()
    };
    let models = crate::agent_token::runtime_models_json(&prices, &base_url, &token);
    if models["providers"]
        .as_object()
        .map(|m| m.is_empty())
        .unwrap_or(true)
    {
        crate::agent_token::revoke_by_plaintext(&installed.pool, installed.kind, &token).await;
        return (
            StatusCode::BAD_REQUEST,
            "管理员尚未开放任何对话模型，无法启动 Agent",
        )
            .into_response();
    }

    let root = session_runtime_root(&state.data_dir).join(format!("s{sid}"));
    let agent_dir = root.join("agent");
    let cwd = root.join("workspace");
    if let Err(e) = tokio::fs::create_dir_all(&cwd).await {
        crate::agent_token::revoke_by_plaintext(&installed.pool, installed.kind, &token).await;
        eprintln!("[agent-api] creating the workspace failed: {e}");
        return (StatusCode::INTERNAL_SERVER_ERROR, "创建工作目录失败").into_response();
    }
    if let Err(e) = crate::agent_driver::write_agent_dir(&agent_dir, &models).await {
        crate::agent_token::revoke_by_plaintext(&installed.pool, installed.kind, &token).await;
        return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
    }

    let spawned = if sandboxed {
        let limits = crate::agent_sandbox::SandboxLimits::from_env();
        crate::agent_sandbox::spawn(sid, &agent_dir, gateway_port(), &limits).await
    } else {
        let program =
            crate::runtime_env::var("YUNOVA_PI_BIN").unwrap_or_else(|_| "pi".to_string());
        crate::agent_driver::spawn(crate::agent_driver::SpawnConfig {
            program,
            cwd,
            agent_dir,
            name: Some(format!("yunova-{sid}")),
        })
        .await
    };
    let (transport, frames) = match spawned {
        Ok(v) => v,
        Err(e) => {
            crate::agent_token::revoke_by_plaintext(&installed.pool, installed.kind, &token).await;
            return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
        }
    };

    let live = state
        .agent_sessions
        .attach(
            installed.pool.clone(),
            installed.kind,
            sid,
            user.id,
            Target::Cloud,
            transport,
            frames,
        )
        .await;

    // Bound the sandbox's wall-clock lifetime. A session that is never stopped
    // would otherwise hold its container, and its quota-spending credential,
    // indefinitely. Retire both when the ceiling is reached.
    if sandboxed {
        let limits = crate::agent_sandbox::SandboxLimits::from_env();
        let registry = state.agent_sessions.clone();
        let pool = installed.pool.clone();
        let kind = installed.kind;
        let token = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(limits.max_lifetime_secs)).await;
            // Only act if this exact session is still the live one; a restart
            // in between would have replaced it.
            if registry
                .get(live.session_id)
                .await
                .is_some_and(|current| Arc::ptr_eq(&current, &live))
            {
                eprintln!(
                    "[agent-sandbox] session {} hit its lifetime ceiling; stopping",
                    live.session_id
                );
                registry.remove(live.session_id).await;
                live.shutdown().await;
                crate::agent_sandbox::remove_container(live.session_id).await;
            }
            crate::agent_token::revoke_by_plaintext(&pool, kind, &token).await;
        });
    }

    Json(json!({ "ok": true, "reused": false, "sandboxed": sandboxed })).into_response()
}

/// Stop a session's runtime without deleting its transcript.
async fn stop_session(
    State(state): State<AppState>,
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(sid): Path<i64>,
) -> Response {
    if let Err(r) = owned_session(&installed.pool, installed.kind, user.id, sid).await {
        return r;
    }
    match state.agent_sessions.remove(sid).await {
        Some(live) => {
            live.shutdown().await;
            // Also clear the container. `--rm` handles the normal exit, but a
            // runtime that ignored stdin EOF would otherwise leak a container
            // and hold its name against the next start.
            crate::agent_sandbox::remove_container(sid).await;
            set_status(&installed.pool, installed.kind, sid, "idle").await;
            Json(json!({ "ok": true })).into_response()
        }
        None => {
            // Nothing live in this process, but a container can still survive
            // a crash; removing it keeps stop idempotent and self-healing.
            crate::agent_sandbox::remove_container(sid).await;
            Json(json!({ "ok": true, "already_stopped": true })).into_response()
        }
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/agent/sessions", post(create_session).get(list_sessions))
        .route("/agent/sessions/{sid}/entries", get(list_entries))
        .route("/agent/sessions/{sid}/events", get(session_events))
        .route("/agent/sessions/{sid}/start", post(start_session))
        .route("/agent/sessions/{sid}/stop", post(stop_session))
        .route("/agent/sessions/{sid}/prompt", post(prompt))
        .route("/agent/sessions/{sid}/abort", post(abort))
        .route("/agent/sessions/{sid}/approve", post(approve))
}
