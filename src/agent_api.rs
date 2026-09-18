//! HTTP surface for agent sessions.
//!
//! The server only relays here — the runtime owns the thinking loop and the
//! tool protocol — so these handlers are identical for the cloud sandbox and
//! for a user's own machine.
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
use axum::routing::{delete, get, post};
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
) -> Result<(String, Option<i64>, Option<String>), Response> {
    let row: Option<(i64, String, Option<i64>, Option<String>)> = sqlx::query_as(&db::q(
        kind,
        "SELECT user_id, target, device_id, workspace FROM agent_sessions WHERE id = ?",
    ))
    .bind(session_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    match row {
        Some((uid, target, device_id, workspace)) if uid == user_id => {
            Ok((target, device_id, workspace))
        }
        _ => Err((StatusCode::NOT_FOUND, "会话不存在").into_response()),
    }
}

// ---------------------------------------------------------------------------
// model selection
// ---------------------------------------------------------------------------

/// Resolve a model name to the provider a runtime would address it by.
///
/// Goes through the priced whitelist rather than trusting the client: the
/// stored model is replayed into `set_model` on every start, so an unchecked
/// value would let a session pin a model the admin has since disabled or never
/// priced. Returns the provider key used in the generated `models.json`.
async fn resolve_agent_model(pool: &Pool, kind: DbKind, model: &str) -> Result<String, Response> {
    let Some(price) = crate::channels::enabled_price(pool, kind, model, "chat").await else {
        return Err((StatusCode::BAD_REQUEST, "该模型未开放或不可用于对话").into_response());
    };
    crate::agent_token::runtime_provider_for(&price.protocol)
        .map(str::to_string)
        .ok_or_else(|| {
            // Gemini is callable in chat mode but has no agent provider, so a
            // clear message beats an opaque "model not found" from pi.
            (StatusCode::BAD_REQUEST, "该模型所属协议暂不支持工作模式").into_response()
        })
}

/// The model a session is pinned to, if any.
async fn session_model(pool: &Pool, kind: DbKind, session_id: i64) -> Option<String> {
    sqlx::query_scalar::<_, Option<String>>(&db::q(
        kind,
        "SELECT model FROM agent_sessions WHERE id = ?",
    ))
    .bind(session_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .flatten()
    .filter(|m| !m.trim().is_empty())
}

/// The reasoning level a session is pinned to, if any.
///
/// Validated on the way out as well as on the way in: the column is plain text
/// and survives a downgrade, so a level written by a newer build must not be
/// replayed into a runtime that would reject it and leave the session running
/// at a level nothing on screen claims.
async fn session_thinking_level(pool: &Pool, kind: DbKind, session_id: i64) -> Option<String> {
    sqlx::query_scalar::<_, Option<String>>(&db::q(
        kind,
        "SELECT thinking_level FROM agent_sessions WHERE id = ?",
    ))
    .bind(session_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .flatten()
    .map(|l| l.trim().to_string())
    .filter(|l| agent_rpc::is_thinking_level(l))
}

/// Ask a live runtime to switch models.
///
/// Used both when a runtime starts and when the user switches mid-session, so
/// the two paths fail identically instead of one of them silently leaving the
/// session on a different model than the UI shows.
async fn apply_model(
    live: &Arc<crate::agent_session::LiveSession>,
    provider: &str,
    model: &str,
) -> Result<(), String> {
    match live
        .request(agent_rpc::cmd_set_model("", provider, model))
        .await?
    {
        Inbound::Response { success: true, .. } => Ok(()),
        Inbound::Response { error, .. } => {
            Err(error.unwrap_or_else(|| "运行时拒绝了该模型".into()))
        }
        _ => Err("运行时返回了意外响应".into()),
    }
}

/// Ask a live runtime to change how hard it reasons.
///
/// Shared by start-time replay and the mid-session switch for the same reason
/// `apply_model` is: the two paths must fail identically, or one of them would
/// leave the runtime on a level the UI does not show.
async fn apply_thinking_level(
    live: &Arc<crate::agent_session::LiveSession>,
    level: &str,
) -> Result<(), String> {
    match live
        .request(agent_rpc::cmd_set_thinking_level("", level))
        .await?
    {
        Inbound::Response { success: true, .. } => Ok(()),
        Inbound::Response { error, .. } => {
            Err(error.unwrap_or_else(|| "运行时拒绝了该推理级别".into()))
        }
        _ => Err("运行时返回了意外响应".into()),
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
    /// How hard the agent should reason, from pi's ladder
    /// (`off`…`max`). Omitted means the runtime's own default.
    #[serde(default)]
    thinking_level: Option<String>,
    /// Directory on the device this task should run in.
    ///
    /// Only meaningful for `target = "device"`, and only ever a record of what
    /// the user picked: the machine re-checks it against the directories it
    /// was told to allow before starting anything there. Omitted means "the
    /// client's default workspace".
    #[serde(default)]
    workspace: Option<String>,
}

async fn create_session(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Json(req): Json<CreateSessionReq>,
) -> Response {
    let Some(target) = Target::parse(&req.target) else {
        return (StatusCode::BAD_REQUEST, "target 必须是 cloud 或 device").into_response();
    };

    // Validate the model up front rather than at start: a task created with an
    // unusable model would otherwise look fine until its first prompt.
    let model = req
        .model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty());
    if let Some(model) = model
        && let Err(r) = resolve_agent_model(&installed.pool, installed.kind, model).await
    {
        return r;
    }

    // Refused rather than silently dropped: the level is replayed on every
    // later start, so a value pi would reject has to be caught while the user
    // is still looking at the control that produced it.
    let thinking_level = req
        .thinking_level
        .as_deref()
        .map(str::trim)
        .filter(|l| !l.is_empty());
    if let Some(level) = thinking_level
        && !agent_rpc::is_thinking_level(level)
    {
        return (StatusCode::BAD_REQUEST, "推理级别无效").into_response();
    }

    // A device session is meaningless without a machine to run on, and the
    // machine must belong to the caller: otherwise a session could be pointed
    // at someone else's computer.
    let mut workspace = None;
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

        // Length-capped but otherwise untouched: the server has no view of
        // that filesystem, so any validation it invented would either reject
        // legitimate paths or give false assurance. The device is the only
        // party that can tell whether this directory is one the user allowed,
        // and it checks before it starts.
        workspace = req
            .workspace
            .as_deref()
            .map(str::trim)
            .filter(|w| !w.is_empty())
            .map(|w| w.chars().take(1024).collect::<String>());
    } else if req.workspace.is_some() {
        // A cloud task runs in a disposable sandbox with a fixed working
        // directory, so accepting one here would store a preference nothing
        // can honour.
        return (StatusCode::BAD_REQUEST, "云电脑任务不支持指定工作目录").into_response();
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
        "INSERT INTO agent_sessions (user_id, target, device_id, title, model, workspace, \
         thinking_level) VALUES (?, ?, ?, ?, ?, ?, ?)",
    );
    // Both backends support RETURNING, so the id comes back from the insert.
    let id = sqlx::query_as::<_, (i64,)>(&format!("{insert} RETURNING id"))
        .bind(user.id)
        .bind(target.as_str())
        .bind(req.device_id)
        .bind(&title)
        .bind(model)
        .bind(workspace.as_deref())
        .bind(thinking_level)
        .fetch_one(&installed.pool)
        .await
        .map(|r| r.0)
        .map_err(|e| e.to_string());

    match id {
        Ok(id) => Json(json!({
            "id": id,
            "target": target.as_str(),
            "title": title,
            "model": model,
            "workspace": workspace,
            "thinking_level": thinking_level,
        }))
        .into_response(),
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
    Option<String>, // workspace
    Option<String>, // thinking_level
);

async fn list_sessions(
    State(state): State<AppState>,
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
) -> Response {
    let rows: Vec<SessionRow> = sqlx::query_as(&db::q(
        installed.kind,
        "SELECT id, target, device_id, title, model, status, updated_at, workspace, \
         thinking_level FROM agent_sessions WHERE user_id = ? ORDER BY updated_at DESC",
    ))
    .bind(user.id)
    .fetch_all(&installed.pool)
    .await
    .unwrap_or_default();

    let mut out = Vec::with_capacity(rows.len());
    for (id, target, device_id, title, model, status, updated_at, workspace, thinking_level) in rows
    {
        out.push(json!({
            "id": id,
            "target": target,
            "device_id": device_id,
            "title": title,
            "model": model,
            "status": status,
            "updated_at": updated_at,
            // Where the tools ran. Part of the transcript's meaning, not just
            // a launch parameter: a task reopened later has to be able to say
            // which project it was working on.
            "workspace": workspace,
            // How hard this task reasons. Null means the runtime's default,
            // which is what every task did before this could be chosen.
            "thinking_level": thinking_level,
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
struct SetModelReq {
    model: String,
}

/// Pin a session to a model, applying it immediately when a runtime is live.
///
/// The choice is stored regardless, because a session outlives its runtime:
/// the next start replays it, so a task resumed tomorrow runs on the model the
/// user picked rather than on whatever pi would default to. Persisting only
/// after the runtime accepts keeps the two in step — a rejected model must not
/// become the value replayed on every later start.
async fn set_model(
    State(state): State<AppState>,
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(sid): Path<i64>,
    Json(req): Json<SetModelReq>,
) -> Response {
    if let Err(r) = owned_session(&installed.pool, installed.kind, user.id, sid).await {
        return r;
    }
    let model = req.model.trim();
    if model.is_empty() {
        return (StatusCode::BAD_REQUEST, "模型不能为空").into_response();
    }
    let provider = match resolve_agent_model(&installed.pool, installed.kind, model).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    if let Some(live) = state.agent_sessions.get(sid).await
        && let Err(e) = apply_model(&live, &provider, model).await
    {
        return (StatusCode::BAD_GATEWAY, e).into_response();
    }

    let sql = db::q(
        installed.kind,
        "UPDATE agent_sessions SET model = ?, updated_at = ? WHERE id = ?",
    );
    if let Err(e) = sqlx::query(&sql)
        .bind(model)
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(sid)
        .execute(&installed.pool)
        .await
    {
        eprintln!("[agent-api] persisting the session model failed: {e}");
        return (StatusCode::INTERNAL_SERVER_ERROR, "保存模型选择失败").into_response();
    }

    Json(json!({ "ok": true, "model": model, "provider": provider })).into_response()
}

#[derive(Deserialize)]
struct SetThinkingReq {
    level: String,
}

/// Pin a session's reasoning level, applying it immediately when a runtime is
/// live.
///
/// Stored for the same reason the model is: a session outlives its runtime, so
/// a task resumed tomorrow has to reason as hard as the user asked rather than
/// falling back to pi's default. Persisting only after the runtime accepts
/// keeps the two in step — a rejected level must not become the value replayed
/// on every later start.
async fn set_thinking_level(
    State(state): State<AppState>,
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(sid): Path<i64>,
    Json(req): Json<SetThinkingReq>,
) -> Response {
    if let Err(r) = owned_session(&installed.pool, installed.kind, user.id, sid).await {
        return r;
    }
    let level = req.level.trim();
    if !agent_rpc::is_thinking_level(level) {
        return (StatusCode::BAD_REQUEST, "推理级别无效").into_response();
    }

    if let Some(live) = state.agent_sessions.get(sid).await
        && let Err(e) = apply_thinking_level(&live, level).await
    {
        return (StatusCode::BAD_GATEWAY, e).into_response();
    }

    let sql = db::q(
        installed.kind,
        "UPDATE agent_sessions SET thinking_level = ?, updated_at = ? WHERE id = ?",
    );
    if let Err(e) = sqlx::query(&sql)
        .bind(level)
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(sid)
        .execute(&installed.pool)
        .await
    {
        eprintln!("[agent-api] persisting the session thinking level failed: {e}");
        return (StatusCode::INTERNAL_SERVER_ERROR, "保存推理级别失败").into_response();
    }

    Json(json!({ "ok": true, "thinking_level": level })).into_response()
}

/// Which reasoning levels the session's current model actually supports.
///
/// Asked of the live runtime rather than derived from the model name: the
/// ladder is model-dependent (`xhigh`/`max` exist only on some), so a picker
/// built from guesses would offer levels that silently collapse to another
/// one. Without a runtime there is nothing authoritative to report, which the
/// client renders as the full ladder.
async fn available_thinking_levels(
    State(state): State<AppState>,
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(sid): Path<i64>,
) -> Response {
    if let Err(r) = owned_session(&installed.pool, installed.kind, user.id, sid).await {
        return r;
    }
    let current = session_thinking_level(&installed.pool, installed.kind, sid).await;
    let Some(live) = state.agent_sessions.get(sid).await else {
        return Json(json!({ "levels": [], "thinking_level": current })).into_response();
    };
    let levels = match live
        .request(agent_rpc::cmd_get_available_thinking_levels(""))
        .await
    {
        Ok(Inbound::Response {
            success: true,
            data: Some(data),
            ..
        }) => agent_rpc::parse_thinking_levels(&data),
        // Non-fatal on purpose: a runtime that cannot answer should degrade to
        // "unknown", not break the composer the control lives in.
        _ => Vec::new(),
    };
    Json(json!({ "levels": levels, "thinking_level": current })).into_response()
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

/// Base URL a runtime on the user's own machine should call.
///
/// A personal machine is on a different network, so neither loopback nor the
/// sandbox's container alias resolves there — and neither does the wildcard
/// address the server binds. Nothing here can be derived from the server's own
/// listener, so this refuses instead of falling back: a runtime handed
/// `http://0.0.0.0:3000` starts happily and then fails every model call with an
/// opaque "Connection error.", which is far harder to diagnose than a session
/// that declines to start.
fn device_gateway_url() -> Result<String, String> {
    let url = crate::runtime_env::var("YUNOVA_DEVICE_GATEWAY_URL")
        .or_else(|_| crate::runtime_env::var("YUNOVA_PUBLIC_URL"))
        // An explicitly set agent gateway is a deliberate absolute URL, unlike
        // the bind-derived default `gateway_base_url` invents.
        .or_else(|_| crate::runtime_env::var("YUNOVA_AGENT_GATEWAY_URL"))
        .ok()
        .map(|u| u.trim().trim_end_matches('/').to_string())
        .filter(|u| !u.is_empty())
        .ok_or_else(|| {
            "服务器未配置 YUNOVA_DEVICE_GATEWAY_URL：本机电脑任务需要一个你的电脑能访问到的地址"
                .to_string()
        })?;
    if !reachable_from_another_host(&url) {
        return Err(format!(
            "YUNOVA_DEVICE_GATEWAY_URL（{url}）只在服务器本机有效，请改为你的电脑能访问到的地址"
        ));
    }
    Ok(url)
}

/// Host part of a base URL, without scheme, port, path or IPv6 brackets.
fn url_host(url: &str) -> String {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = after_scheme.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);

    // A bracketed IPv6 literal keeps its colons; anything else loses a
    // trailing numeric port.
    let host = if let Some(end) = authority.find(']') {
        &authority[..=end]
    } else {
        match authority.rsplit_once(':') {
            Some((h, port)) if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => h,
            _ => authority,
        }
    };
    host.trim_matches(['[', ']']).to_ascii_lowercase()
}

/// Whether a base URL could plausibly be dialled from a different machine.
///
/// Only the hosts that are *definitely* wrong are rejected: a wildcard bind
/// resolves nowhere, and loopback resolves to whoever dials it. Everything else
/// is the operator's call — a LAN address is perfectly valid for a desktop
/// client on the same network.
fn reachable_from_another_host(url: &str) -> bool {
    let host = url_host(url);
    !host.is_empty()
        && !host.starts_with("127.")
        && !matches!(host.as_str(), "0.0.0.0" | "localhost" | "::" | "::1" | "0")
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
    let (target, session_device_id, session_workspace) =
        match owned_session(&installed.pool, installed.kind, user.id, sid).await {
            Ok(v) => v,
            Err(r) => return r,
        };
    let Some(target) = Target::parse(&target) else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "会话目标无法识别").into_response();
    };

    // Reuse a warm runtime instead of starting a second one for the same
    // session, which would fork the transcript.
    if state.agent_sessions.is_live(sid).await {
        return Json(json!({ "ok": true, "reused": true })).into_response();
    }

    // A session's pinned model, resolved before anything is started: the
    // runtime is told which model to use, so a value that is no longer priced
    // must fail here rather than after a container is already running.
    let pinned = match session_model(&installed.pool, installed.kind, sid).await {
        Some(model) => {
            match resolve_agent_model(&installed.pool, installed.kind, &model).await {
                Ok(provider) => Some((provider, model)),
                // Do not fail the start. The admin may have retired the model
                // long after the task was created, and refusing to open an
                // existing task is worse than running it on pi's default.
                Err(_) => {
                    eprintln!(
                        "[agent-api] session {sid} pinned an unavailable model; using the default"
                    );
                    None
                }
            }
        }
        None => None,
    };

    // Same treatment as the model: read before anything is spawned, and
    // dropped rather than fatal if it is no longer a level this build knows.
    let pinned_level = session_thinking_level(&installed.pool, installed.kind, sid).await;
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
        Some(crate::agent_token::SESSION_TOKEN_TTL_HOURS),
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
    // The gateway URL baked into the runtime's config depends on where that
    // runtime lives: a container reaches us by its network alias, a personal
    // machine over the public origin, a host process over loopback.
    let sandboxed = target == Target::Cloud && sandbox_enabled();
    let base_url = match target {
        // A device is on someone's home network, so it needs the address the
        // browser uses, not a loopback or container-internal one.
        Target::Device => match device_gateway_url() {
            Ok(url) => url,
            Err(e) => {
                crate::agent_token::revoke_by_plaintext(&installed.pool, installed.kind, &token)
                    .await;
                eprintln!("[agent-api] session {sid} has no usable device gateway URL: {e}");
                return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
            }
        },
        Target::Cloud if sandboxed => sandbox_gateway_url(),
        Target::Cloud => gateway_base_url(),
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

    // --- device target: the runtime lives on the user's own machine ---
    //
    // Nothing is spawned here. The desktop client owns its workspace and
    // approval policy, so the server only asks it to start a runtime and then
    // relays frames; it never tells the machine what to execute.
    if target == Target::Device {
        let Some(device_id) = session_device_id else {
            crate::agent_token::revoke_by_plaintext(&installed.pool, installed.kind, &token).await;
            return (StatusCode::BAD_REQUEST, "该任务没有绑定设备").into_response();
        };
        let Some(device) = state.agent_devices.get(device_id).await else {
            crate::agent_token::revoke_by_plaintext(&installed.pool, installed.kind, &token).await;
            return (
                StatusCode::CONFLICT,
                "该电脑当前不在线，请先打开桌面客户端",
            )
                .into_response();
        };
        // Defence in depth: the session's owner was already checked, but a
        // device must never be driven on behalf of another account.
        if device.user_id != user.id {
            crate::agent_token::revoke_by_plaintext(&installed.pool, installed.kind, &token).await;
            return (StatusCode::NOT_FOUND, "设备不存在或无权访问").into_response();
        }

        // Route the device's frames for this session back into the pump before
        // asking it to start, so nothing the runtime emits can be lost.
        let (frame_tx, frame_rx) = tokio::sync::mpsc::channel::<serde_json::Value>(256);
        if !crate::agent_device::register_session_frames(&state, device_id, sid, frame_tx).await {
            crate::agent_token::revoke_by_plaintext(&installed.pool, installed.kind, &token).await;
            return (StatusCode::CONFLICT, "设备连接已断开").into_response();
        }

        if let Err(e) = device
            .send(crate::agent_device::ToDevice::StartRuntime {
                session_id: sid,
                models_json: models,
                // The stored choice, forwarded verbatim. The client decides
                // whether it is still inside a directory the user allows and
                // refuses the start if not, so this is a request rather than
                // an order — the machine at risk keeps the last word.
                workspace: session_workspace,
            })
            .await
        {
            crate::agent_token::revoke_by_plaintext(&installed.pool, installed.kind, &token).await;
            return (StatusCode::BAD_GATEWAY, e).into_response();
        }

        let transport = std::sync::Arc::new(crate::agent_device::DeviceTransport::new(
            device, sid,
        ));
        let live = state
            .agent_sessions
            .attach(
                installed.pool.clone(),
                installed.kind,
                sid,
                user.id,
                Target::Device,
                transport,
                frame_rx,
                // The device target is the reason this binding exists: the
                // plaintext was just handed to a machine the user controls, so
                // it must die with the session rather than outlive it.
                Some(token),
            )
            .await;
        if let Some((provider, model)) = &pinned
            && let Err(e) = apply_model(&live, provider, model).await
        {
            // Non-fatal: the runtime is up and usable on its default model, and
            // tearing the session down over a model switch would lose it.
            eprintln!("[agent-api] session {sid} could not select {model}: {e}");
        }
        // After the model, never before: the ladder is model-dependent, so a
        // level applied first could be clamped against the wrong model.
        if let Some(level) = &pinned_level
            && let Err(e) = apply_thinking_level(&live, level).await
        {
            eprintln!("[agent-api] session {sid} could not set thinking level {level}: {e}");
        }
        return Json(json!({ "ok": true, "reused": false, "sandboxed": false })).into_response();
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
            Some(token.clone()),
        )
        .await;

    if let Some((provider, model)) = &pinned
        && let Err(e) = apply_model(&live, provider, model).await
    {
        eprintln!("[agent-api] session {sid} could not select {model}: {e}");
    }
    if let Some(level) = &pinned_level
        && let Err(e) = apply_thinking_level(&live, level).await
    {
        eprintln!("[agent-api] session {sid} could not set thinking level {level}: {e}");
    }

    // Bound the sandbox's wall-clock lifetime. A session that is never stopped
    // would otherwise hold its container, and its quota-spending credential,
    // indefinitely. Retire both when the ceiling is reached.
    if sandboxed {
        let limits = crate::agent_sandbox::SandboxLimits::from_env();
        let registry = state.agent_sessions.clone();
        let pool = installed.pool.clone();
        let kind = installed.kind;
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
            live.revoke_token(&pool, kind).await;
        });
    }

    Json(json!({ "ok": true, "reused": false, "sandboxed": sandboxed })).into_response()
}

/// Release everything a session's runtime holds, leaving its rows alone.
///
/// Shared by stop and delete so the two cannot drift: whichever way a session
/// is retired, the credential is revoked, the container is gone and the device
/// no longer has a route for its frames.
///
/// Returns whether a live runtime was actually detached here.
async fn teardown_runtime(
    state: &AppState,
    installed: &InstalledState,
    device_id: Option<i64>,
    sid: i64,
) -> bool {
    let was_live = match state.agent_sessions.remove(sid).await {
        Some(live) => {
            live.shutdown().await;
            // Retire the credential here too. The frame pump revokes on its
            // own exit, but `remove` already detached the session, so that
            // path may never run for a runtime that ignores stdin EOF — and
            // "stopped" must mean the token is dead, not dead within an hour.
            live.revoke_token(&installed.pool, installed.kind).await;
            true
        }
        None => false,
    };
    // Clear the container either way. `--rm` handles the normal exit, but a
    // runtime that ignored stdin EOF — or one that outlived a crash of this
    // process — would otherwise leak a container and hold its name against the
    // next start, so doing it unconditionally keeps teardown self-healing.
    crate::agent_sandbox::remove_container(sid).await;
    // Drop the device's route for this session. One socket multiplexes every
    // session on that machine, so a stale entry would keep relaying frames
    // into a pump nobody reads.
    if let Some(device_id) = device_id {
        crate::agent_device::unregister_session_frames(state, device_id, sid).await;
    }
    was_live
}

/// Stop a session's runtime without deleting its transcript.
async fn stop_session(
    State(state): State<AppState>,
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(sid): Path<i64>,
) -> Response {
    let device_id = match owned_session(&installed.pool, installed.kind, user.id, sid).await {
        Ok((_, device_id, _)) => device_id,
        Err(r) => return r,
    };
    if teardown_runtime(&state, &installed, device_id, sid).await {
        set_status(&installed.pool, installed.kind, sid, "idle").await;
        Json(json!({ "ok": true })).into_response()
    } else {
        // Nothing was live in this process, but teardown still ran, so a
        // container that survived a crash is gone: stop stays idempotent.
        Json(json!({ "ok": true, "already_stopped": true })).into_response()
    }
}

/// Delete a session: its runtime, its transcript and its workspace.
///
/// Stopping is not enough for a user who wants the task gone — the title stays
/// in the sidebar and the transcript stays readable — so this is what the
/// list's "删除" needs. The runtime is torn down first: dropping the rows while
/// a container still mirrors into them would leave a half-written transcript
/// pointing at a session that no longer exists.
async fn delete_session(
    State(state): State<AppState>,
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(sid): Path<i64>,
) -> Response {
    let device_id = match owned_session(&installed.pool, installed.kind, user.id, sid).await {
        Ok((_, device_id, _)) => device_id,
        Err(r) => return r,
    };

    teardown_runtime(&state, &installed, device_id, sid).await;

    match delete_session_rows(&installed.pool, installed.kind, user.id, sid).await {
        Ok(true) => {}
        Ok(false) => return (StatusCode::NOT_FOUND, "会话不存在").into_response(),
        Err(e) => {
            eprintln!("[agent-api] deleting session {sid} failed: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "删除任务失败").into_response();
        }
    }

    // The session's private pi config and workspace go too: it holds whatever
    // the agent wrote plus a generated `models.json` carrying a (now revoked)
    // gateway credential, so leaving it behind would accumulate unreachable
    // files for every deleted task.
    let root = session_runtime_root(&state.data_dir).join(format!("s{sid}"));
    if let Err(e) = tokio::fs::remove_dir_all(&root).await
        && e.kind() != std::io::ErrorKind::NotFound
    {
        // Not fatal: the session is already gone from the user's view, and
        // failing here would invite a retry that can no longer find it.
        eprintln!(
            "[agent-api] removing the workspace of session {sid} failed: {e} ({})",
            root.display()
        );
    }

    StatusCode::NO_CONTENT.into_response()
}

/// Drop a session and its mirrored transcript.
///
/// Returns whether a row was actually removed, so a repeated delete answers
/// 404 instead of reporting success for a session that is already gone.
async fn delete_session_rows(
    pool: &Pool,
    kind: DbKind,
    user_id: i64,
    sid: i64,
) -> Result<bool, String> {
    // Entries are removed explicitly rather than left to the FK cascade:
    // Postgres declares ON DELETE CASCADE, but SQLite only honours it with
    // `PRAGMA foreign_keys` on, which the pool does not set. Without this, a
    // SQLite deployment would keep every mirrored entry of every deleted task.
    //
    // Scoped through the session's owner so this cannot reach another
    // account's transcript even if called with an id that is not the caller's.
    sqlx::query(&db::q(
        kind,
        "DELETE FROM agent_entries WHERE session_id IN \
         (SELECT id FROM agent_sessions WHERE id = ? AND user_id = ?)",
    ))
    .bind(sid)
    .bind(user_id)
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    let deleted = sqlx::query(&db::q(
        kind,
        "DELETE FROM agent_sessions WHERE id = ? AND user_id = ?",
    ))
    .bind(sid)
    .bind(user_id)
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;
    Ok(deleted.rows_affected() > 0)
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/agent/sessions", post(create_session).get(list_sessions))
        .route("/agent/sessions/{sid}", delete(delete_session))
        .route("/agent/sessions/{sid}/entries", get(list_entries))
        .route("/agent/sessions/{sid}/events", get(session_events))
        .route("/agent/sessions/{sid}/start", post(start_session))
        .route("/agent/sessions/{sid}/stop", post(stop_session))
        .route("/agent/sessions/{sid}/prompt", post(prompt))
        .route("/agent/sessions/{sid}/model", post(set_model))
        .route(
            "/agent/sessions/{sid}/thinking",
            post(set_thinking_level).get(available_thinking_levels),
        )
        .route("/agent/sessions/{sid}/abort", post(abort))
        .route("/agent/sessions/{sid}/approve", post(approve))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::install_drivers;
    use sqlx::any::AnyPoolOptions;

    async fn pool_with(models: &[(&str, &str, bool)]) -> Pool {
        install_drivers();
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::migrate(&pool, DbKind::Sqlite).await.unwrap();
        for (model, protocol, enabled) in models {
            crate::channels::upsert_price(
                &pool,
                DbKind::Sqlite,
                &crate::channels::PricingInput {
                    model: (*model).into(),
                    kind: "chat".into(),
                    channel_ids: None,
                    display_name: None,
                    enabled: *enabled,
                    protocol: (*protocol).into(),
                    context_limit: None,
                    input_price: 0,
                    output_price: 0,
                    cached_input_price: None,
                    per_call_price: 0,
                    base_price: 0,
                    per_second_price: 0,
                    allowed_seconds: None,
                    size_rules: None,
                },
            )
            .await
            .unwrap();
        }
        pool
    }

    /// The stored model is replayed into `set_model` on every start, so an
    /// unchecked value would let a session keep spending on a model the admin
    /// has since retired — the whitelist has to be enforced here, not in the
    /// picker that happens to populate it.
    #[tokio::test]
    async fn only_enabled_chat_models_on_an_agent_protocol_can_be_pinned() {
        let pool = pool_with(&[
            ("gpt-5", "openai", true),
            ("claude-opus-4-5", "claude", true),
            ("gpt-5-retired", "openai", false),
            ("gemini-3-pro", "gemini", true),
        ])
        .await;

        assert_eq!(
            resolve_agent_model(&pool, DbKind::Sqlite, "gpt-5")
                .await
                .ok(),
            Some("yunova-openai".to_string())
        );
        assert_eq!(
            resolve_agent_model(&pool, DbKind::Sqlite, "claude-opus-4-5")
                .await
                .ok(),
            Some("yunova-claude".to_string())
        );
        assert!(
            resolve_agent_model(&pool, DbKind::Sqlite, "gpt-5-retired")
                .await
                .is_err(),
            "a disabled model must not be selectable"
        );
        assert!(
            resolve_agent_model(&pool, DbKind::Sqlite, "unknown-model")
                .await
                .is_err(),
            "an unpriced model must not be selectable"
        );
        assert!(
            resolve_agent_model(&pool, DbKind::Sqlite, "gemini-3-pro")
                .await
                .is_err(),
            "a protocol with no agent provider must not be selectable"
        );

        pool.close().await;
    }

    /// A session with no stored model runs on pi's default, which is what every
    /// task did before the picker existed; a blank string must not be mistaken
    /// for a pinned choice and replayed as one.
    #[tokio::test]
    async fn a_session_without_a_pinned_model_reports_none() {
        let pool = pool_with(&[("gpt-5", "openai", true)]).await;
        sqlx::query("INSERT INTO users (id, username, password_hash) VALUES (1, 'u1', 'x')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO agent_sessions (id, user_id, target, title, model) \
             VALUES (1, 1, 'cloud', 't', NULL), (2, 1, 'cloud', 't', '  '), \
                    (3, 1, 'cloud', 't', 'gpt-5')",
        )
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(session_model(&pool, DbKind::Sqlite, 1).await, None);
        assert_eq!(session_model(&pool, DbKind::Sqlite, 2).await, None);
        assert_eq!(
            session_model(&pool, DbKind::Sqlite, 3).await,
            Some("gpt-5".to_string())
        );

        pool.close().await;
    }

    /// The column is plain text and survives a downgrade, so a level written
    /// by a newer build must not be replayed into a runtime that would reject
    /// it — the session would then run at a level nothing on screen claims.
    #[tokio::test]
    async fn only_a_level_this_build_understands_is_replayed() {
        let pool = pool_with(&[]).await;
        sqlx::query("INSERT INTO users (id, username, password_hash) VALUES (1, 'u1', 'x')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO agent_sessions (id, user_id, target, title, thinking_level) \
             VALUES (1, 1, 'cloud', 't', NULL), (2, 1, 'cloud', 't', '  '), \
                    (3, 1, 'cloud', 't', 'high'), (4, 1, 'cloud', 't', 'ludicrous')",
        )
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(session_thinking_level(&pool, DbKind::Sqlite, 1).await, None);
        assert_eq!(session_thinking_level(&pool, DbKind::Sqlite, 2).await, None);
        assert_eq!(
            session_thinking_level(&pool, DbKind::Sqlite, 3).await,
            Some("high".to_string())
        );
        assert_eq!(
            session_thinking_level(&pool, DbKind::Sqlite, 4).await,
            None,
            "a level this build cannot send must not be replayed"
        );

        pool.close().await;
    }

    /// The regression this guards: production ran with no device gateway
    /// configured, so the URL fell back to the server's own bind address and
    /// every desktop task was handed `http://0.0.0.0:3000`. The runtime
    /// started, failed each model call with "Connection error.", and work mode
    /// looked like it was simply ignoring the user.
    #[test]
    fn an_address_only_valid_on_the_server_is_not_a_device_gateway() {
        for bad in [
            "http://0.0.0.0:3000",
            "http://127.0.0.1:3000",
            "http://127.0.0.53",
            "http://localhost:3000",
            "http://[::1]:3000",
            "http://[::]:3000",
        ] {
            assert!(
                !reachable_from_another_host(bad),
                "{bad} cannot be dialled from the user's own machine"
            );
        }
    }

    /// A LAN address is the operator's call: a desktop client on the same
    /// network reaches it, and refusing it would break self-hosted setups.
    #[test]
    fn a_routable_address_is_accepted_whatever_its_shape() {
        for good in [
            "https://chat.yunnet.top",
            "http://10.1.51.1:4300",
            "https://example.com:8443/base",
            "http://[2001:db8::1]:3000",
        ] {
            assert!(reachable_from_another_host(good), "{good} is routable");
        }
    }

    /// The host has to survive ports, paths and IPv6 brackets, because a
    /// mis-parsed host would either reject a valid public URL or let the
    /// broken loopback one through.
    #[test]
    fn the_host_is_extracted_without_scheme_port_or_path() {
        assert_eq!(url_host("https://chat.yunnet.top/api"), "chat.yunnet.top");
        assert_eq!(url_host("http://10.1.51.1:4300"), "10.1.51.1");
        assert_eq!(url_host("http://[2001:db8::1]:3000"), "2001:db8::1");
        assert_eq!(url_host("HTTP://Example.COM"), "example.com");
        assert_eq!(url_host("0.0.0.0:3000"), "0.0.0.0");
    }

    /// Deleting a task must take its mirrored transcript with it. The schema's
    /// cascade is not enough on SQLite, where the pool leaves
    /// `PRAGMA foreign_keys` off, so without an explicit delete every removed
    /// task would leave its whole history behind.
    #[tokio::test]
    async fn deleting_a_session_takes_its_entries_with_it() {
        let pool = pool_with(&[]).await;
        sqlx::query(
            "INSERT INTO users (id, username, password_hash) \
             VALUES (1, 'u1', 'x'), (2, 'u2', 'x')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO agent_sessions (id, user_id, target, title) \
             VALUES (1, 1, 'cloud', 'mine'), (2, 2, 'cloud', 'theirs')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO agent_entries (session_id, entry_id, kind, payload) \
             VALUES (1, 'e1', 'message', '{}'), (1, 'e2', 'message', '{}'), \
                    (2, 'e1', 'message', '{}')",
        )
        .execute(&pool)
        .await
        .unwrap();

        assert!(
            delete_session_rows(&pool, DbKind::Sqlite, 1, 1)
                .await
                .unwrap()
        );

        let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
        let entries: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_entries")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(sessions, 1, "only the deleted session's row is gone");
        assert_eq!(entries, 1, "its entries go, the other session's stay");

        // A repeated delete has nothing left to remove, which is how the
        // handler knows to answer 404 rather than report success twice.
        assert!(
            !delete_session_rows(&pool, DbKind::Sqlite, 1, 1)
                .await
                .unwrap()
        );

        pool.close().await;
    }

    /// "Not yours" must behave exactly like "does not exist": a session can
    /// drive shell commands on someone's own machine, so a delete that reached
    /// across accounts would be the worst possible place to trust an id.
    #[tokio::test]
    async fn a_session_belonging_to_someone_else_cannot_be_deleted() {
        let pool = pool_with(&[]).await;
        sqlx::query(
            "INSERT INTO users (id, username, password_hash) \
             VALUES (1, 'u1', 'x'), (2, 'u2', 'x')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO agent_sessions (id, user_id, target, title) \
             VALUES (2, 2, 'cloud', 'theirs')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO agent_entries (session_id, entry_id, kind, payload) \
             VALUES (2, 'e1', 'message', '{}')",
        )
        .execute(&pool)
        .await
        .unwrap();

        assert!(
            !delete_session_rows(&pool, DbKind::Sqlite, 1, 2)
                .await
                .unwrap(),
            "user 1 must not delete user 2's session"
        );
        let entries: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_entries")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(entries, 1, "and must not touch its transcript either");

        pool.close().await;
    }

    /// The task's directory has to survive into `start`, because that is where
    /// it is forwarded to the machine. Storing it and then reading the session
    /// back without it would silently run every task in the client's default
    /// directory while the UI claimed otherwise.
    #[tokio::test]
    async fn a_device_session_remembers_the_directory_it_was_created_for() {
        let pool = pool_with(&[]).await;
        sqlx::query(
            "INSERT INTO users (id, username, password_hash) VALUES (1, 'u1', 'x'), (2, 'u2', 'x')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO agent_devices (id, user_id, name, token_hash) \
             VALUES (7, 1, 'laptop', 'h')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO agent_sessions (id, user_id, target, device_id, title, workspace) \
             VALUES (1, 1, 'device', 7, 't', '/home/u/code/app'), \
                    (2, 1, 'device', 7, 't', NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let (target, device_id, workspace) =
            owned_session(&pool, DbKind::Sqlite, 1, 1).await.unwrap();
        assert_eq!(target, "device");
        assert_eq!(device_id, Some(7));
        assert_eq!(workspace.as_deref(), Some("/home/u/code/app"));

        // A task that named nothing must stay `None` rather than becoming an
        // empty string: the client reads absent as "use your default", and an
        // empty path would be a request it has to refuse.
        let (_, _, none) = owned_session(&pool, DbKind::Sqlite, 1, 2).await.unwrap();
        assert_eq!(none, None);

        // And the directory is only readable by the session's owner, for the
        // same reason the rest of the row is: it names a path on someone's
        // personal machine.
        assert!(
            owned_session(&pool, DbKind::Sqlite, 2, 1).await.is_err(),
            "another account must not learn a path on this user's computer"
        );

        pool.close().await;
    }
}
