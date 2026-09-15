//! 后端工蜂模块：WS 接入、在线注册表、REST、agent 循环。
use crate::channels;
use crate::{AppState, CurrentUser, InstalledState};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Extension, Json, Router};
use futures_util::stream::Stream;
use futures_util::{SinkExt, StreamExt};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, RwLock};

// --- WS 协议（与 worker/src/proto.rs 保持一致）---

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToServer {
    Hello { token: String, name: String },
    Heartbeat,
    ToolResult { call_id: String, ok: bool, output: String, #[serde(default)] truncated: bool },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToWorker {
    HelloOk { worker_id: i64 },
    Error { message: String },
    Exec { call_id: String, tool: String, args: serde_json::Value },
}

/// 一台在线工蜂的句柄：向其 WS 发送 Exec 的通道 + 等待结果的回执表。
pub struct WorkerHandle {
    pub user_id: i64,
    pub tx: mpsc::Sender<ToWorker>,
    /// call_id -> 结果回执 oneshot
    pub pending: Arc<RwLock<HashMap<String, oneshot::Sender<ToolOutcome>>>>,
}

#[derive(Clone, Debug)]
pub struct ToolOutcome {
    pub ok: bool,
    pub output: String,
}

/// 全局在线工蜂表：worker_id -> handle。
#[derive(Clone, Default)]
pub struct WorkerRegistry {
    inner: Arc<RwLock<HashMap<i64, Arc<WorkerHandle>>>>,
}

impl WorkerRegistry {
    pub fn new() -> Self { Self::default() }

    pub async fn insert(&self, worker_id: i64, handle: Arc<WorkerHandle>) {
        self.inner.write().await.insert(worker_id, handle);
    }
    pub async fn remove(&self, worker_id: i64) {
        self.inner.write().await.remove(&worker_id);
    }
    pub async fn get(&self, worker_id: i64) -> Option<Arc<WorkerHandle>> {
        self.inner.read().await.get(&worker_id).cloned()
    }
    pub async fn is_online(&self, worker_id: i64) -> bool {
        self.inner.read().await.contains_key(&worker_id)
    }
}

// ---------------------------------------------------------------------------
// REST 管理端点
// ---------------------------------------------------------------------------

fn gen_token() -> String {
    let mut b = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut b);
    hex::encode(b)
}

/// 生成配对码：建一条 worker 行，返回明文 token（仅此一次）。
async fn pair(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
) -> Response {
    let token = gen_token();
    let th = hash_token(&token);
    let res = sqlx::query(&crate::db::q(
        installed.kind,
        "INSERT INTO workers (user_id, name, token_hash) VALUES (?, ?, ?)",
    ))
    .bind(user.id)
    .bind("worker")
    .bind(&th)
    .execute(&installed.pool)
    .await;
    match res {
        Ok(_) => Json(json!({ "token": token })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("创建失败: {e}")).into_response(),
    }
}

async fn list(
    State(state): State<AppState>,
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
) -> Response {
    let rows: Vec<(i64, String, Option<String>)> = sqlx::query_as(&crate::db::q(
        installed.kind,
        "SELECT id, name, last_seen_at FROM workers WHERE user_id = ? ORDER BY id DESC",
    ))
    .bind(user.id)
    .fetch_all(&installed.pool)
    .await
    .unwrap_or_default();
    let mut out = Vec::new();
    for (id, name, last_seen) in rows {
        out.push(json!({
            "id": id,
            "name": name,
            "last_seen_at": last_seen,
            "online": state.workers.is_online(id).await,
        }));
    }
    Json(out).into_response()
}

#[derive(Deserialize)]
struct RenameReq {
    name: String,
}

async fn rename(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(req): Json<RenameReq>,
) -> Response {
    let _ = sqlx::query(&crate::db::q(
        installed.kind,
        "UPDATE workers SET name = ? WHERE id = ? AND user_id = ?",
    ))
    .bind(&req.name)
    .bind(id)
    .bind(user.id)
    .execute(&installed.pool)
    .await;
    Json(json!({"ok": true})).into_response()
}

async fn remove(
    State(state): State<AppState>,
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Response {
    let _ = sqlx::query(&crate::db::q(
        installed.kind,
        "DELETE FROM workers WHERE id = ? AND user_id = ?",
    ))
    .bind(id)
    .bind(user.id)
    .execute(&installed.pool)
    .await;
    state.workers.remove(id).await;
    Json(json!({"ok": true})).into_response()
}

/// 重命名工蜂会话（worker_sessions），带归属校验。
async fn rename_session(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(sid): Path<i64>,
    Json(req): Json<RenameReq>,
) -> Response {
    let res = sqlx::query(&crate::db::q(
        installed.kind,
        &format!(
            "UPDATE worker_sessions SET title = ?, updated_at = {} WHERE id = ? AND user_id = ?",
            crate::db::now_expr(installed.kind)
        ),
    ))
    .bind(&req.name)
    .bind(sid)
    .bind(user.id)
    .execute(&installed.pool)
    .await;
    match res {
        Ok(out) if out.rows_affected() > 0 => Json(json!({"ok": true})).into_response(),
        Ok(_) => (StatusCode::NOT_FOUND, "会话不存在").into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "重命名失败").into_response(),
    }
}

/// 删除工蜂会话及其消息，事务处理，带归属校验。
async fn remove_session(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(sid): Path<i64>,
) -> Response {
    let mut tx = match installed.pool.begin().await {
        Ok(tx) => tx,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "删除失败").into_response(),
    };
    // 先确认会话归属当前用户
    let owner: Option<(i64,)> = sqlx::query_as(&crate::db::q(
        installed.kind,
        "SELECT user_id FROM worker_sessions WHERE id = ?",
    ))
    .bind(sid)
    .fetch_optional(&mut *tx)
    .await
    .ok()
    .flatten();
    match owner {
        Some((uid,)) if uid == user.id => {}
        _ => return (StatusCode::NOT_FOUND, "会话不存在").into_response(),
    }
    if sqlx::query(&crate::db::q(
        installed.kind,
        "DELETE FROM worker_messages WHERE session_id = ?",
    ))
    .bind(sid)
    .execute(&mut *tx)
    .await
    .is_err()
    {
        return (StatusCode::INTERNAL_SERVER_ERROR, "删除失败").into_response();
    }
    if sqlx::query(&crate::db::q(
        installed.kind,
        "DELETE FROM worker_sessions WHERE id = ? AND user_id = ?",
    ))
    .bind(sid)
    .bind(user.id)
    .execute(&mut *tx)
    .await
    .is_err()
    {
        return (StatusCode::INTERNAL_SERVER_ERROR, "删除失败").into_response();
    }
    match tx.commit().await {
        Ok(_) => Json(json!({"ok": true})).into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "删除失败").into_response(),
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/worker/pair", post(pair))
        .route("/worker/list", get(list))
        .route("/worker/{id}", patch(rename).delete(remove))
        .route("/worker/sessions", post(create_session).get(list_sessions))
        .route(
            "/worker/sessions/{sid}",
            patch(rename_session).delete(remove_session),
        )
        .route("/worker/sessions/{sid}/messages", get(list_messages))
        .route("/worker/sessions/{sid}/message", post(session_message))
        .route("/worker/sessions/{sid}/approve", post(approve))
        .route("/worker/sessions/{sid}/compact", post(compact))
}

// ---------------------------------------------------------------------------
// Agent 循环 + 会话 SSE + 分级审批
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CreateSessionReq {
    worker_id: i64,
}

#[derive(Deserialize)]
struct MessageReq {
    worker_id: i64,
    model: String,
    text: String,
    #[serde(default)]
    auto_approve: bool,
}

#[derive(Deserialize)]
struct ApproveReq {
    call_id: String,
    decision: bool,
}

#[derive(Deserialize)]
struct CompactReq {
    model: String,
}

/// 传给 Claude 的 3 个工具定义。
fn tool_defs() -> serde_json::Value {
    json!([
        {
            "name": "shell",
            "description": "在远程服务器执行 shell 命令，返回 stdout/stderr 与退出码。",
            "input_schema": {"type":"object","properties":{
                "command":{"type":"string"},"cwd":{"type":"string"}
            },"required":["command"]}
        },
        {
            "name": "read_file",
            "description": "读取远程服务器上指定路径的文本文件内容。",
            "input_schema": {"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}
        },
        {
            "name": "write_file",
            "description": "写入/覆盖远程服务器上指定路径的文件。",
            "input_schema": {"type":"object","properties":{
                "path":{"type":"string"},"content":{"type":"string"}
            },"required":["path","content"]}
        }
    ])
}

/// 用解析出的渠道链发一次非流式 Claude Messages 请求，返回完整 JSON。
/// `with_tools`: 传 true 用于 agent 轮次；传 false 用于摘要等不应触发 tool_use 的调用。
async fn call_claude(
    state: &AppState,
    chain: &[channels::ChannelChoice],
    model: &str,
    messages: &serde_json::Value,
    with_tools: bool,
) -> Result<serde_json::Value, String> {
    let mut base_body = json!({
        "model": model,
        "max_tokens": 4096,
        "messages": messages,
    });
    if with_tools {
        base_body["tools"] = tool_defs();
    }
    for choice in chain {
        let upstream_model = if choice.upstream_model.is_empty() {
            model
        } else {
            choice.upstream_model.as_str()
        };
        let endpoint = format!("{}/v1/messages", choice.channel.base_url.trim_end_matches('/'));
        let client = match crate::net_guard::client_for_upstream(&state.http, &endpoint, true).await
        {
            Ok(c) => c,
            Err(_) => continue,
        };
        let mut sendbody = base_body.clone();
        sendbody["model"] = json!(upstream_model);
        let resp = client
            .post(&endpoint)
            .header("content-type", "application/json")
            .header("x-api-key", choice.channel.api_key.as_str())
            .header("anthropic-version", "2023-06-01")
            .json(&sendbody)
            .send()
            .await;
        let resp = match resp {
            Ok(r) => r,
            Err(_) => continue,
        };
        if !resp.status().is_success() {
            let _ = resp.bytes().await;
            continue;
        }
        match resp.json::<serde_json::Value>().await {
            Ok(v) => return Ok(v),
            Err(_) => continue,
        }
    }
    Err("所有 Claude 渠道均不可用".into())
}

/// 按 Claude 响应里的真实 token 用量结算工蜂调用的额度。
///
/// 工蜂走非流式接口，响应体里就带完整 `usage`，因此无需像聊天代理那样边转发
/// 边解析。上游没报 usage 时不计费，避免凭空估算 token。
async fn settle_worker_chat(
    pool: &crate::db::Pool,
    kind: crate::db::DbKind,
    user_id: i64,
    model: &str,
    resp: &serde_json::Value,
    reason: &str,
) {
    let Some(usage) = crate::usage::extract_usage(resp) else {
        return;
    };
    channels::settle_chat(pool, kind, user_id, model, "claude", usage, reason).await;
}

/// 落库一条 worker_message（无需返回 id）。
async fn insert_message(
    pool: &crate::db::Pool,
    kind: crate::db::DbKind,
    session_id: i64,
    role: &str,
    content: &str,
) {
    let _ = sqlx::query(&crate::db::q(
        kind,
        "INSERT INTO worker_messages (session_id, role, content) VALUES (?, ?, ?)",
    ))
    .bind(session_id)
    .bind(role)
    .bind(content)
    .execute(pool)
    .await;
}

/// 创建会话，返回新会话 id。
async fn create_session(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Json(req): Json<CreateSessionReq>,
) -> Response {
    // 校验工蜂归属当前用户
    let owns: Option<(i64,)> = sqlx::query_as(&crate::db::q(
        installed.kind,
        "SELECT id FROM workers WHERE id = ? AND user_id = ?",
    ))
    .bind(req.worker_id)
    .bind(user.id)
    .fetch_optional(&installed.pool)
    .await
    .ok()
    .flatten();
    if owns.is_none() {
        return (StatusCode::NOT_FOUND, "工蜂不存在或无权访问").into_response();
    }
    let base_insert = crate::db::q(
        installed.kind,
        "INSERT INTO worker_sessions (user_id, worker_id, title) VALUES (?, ?, ?)",
    );
    let id_res: Result<i64, String> = match installed.kind {
        crate::db::DbKind::Sqlite | crate::db::DbKind::Postgres => {
            sqlx::query_as::<_, (i64,)>(&format!("{base_insert} RETURNING id"))
                .bind(user.id)
                .bind(req.worker_id)
                .bind("新会话")
                .fetch_one(&installed.pool)
                .await
                .map(|r| r.0)
                .map_err(|e| e.to_string())
        }
        crate::db::DbKind::Mysql => {
            async {
                let mut tx = installed.pool.begin().await.map_err(|e| e.to_string())?;
                sqlx::query(&base_insert)
                    .bind(user.id)
                    .bind(req.worker_id)
                    .bind("新会话")
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
    match id_res {
        Ok(id) => Json(json!({ "id": id })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("创建失败: {e}")).into_response(),
    }
}

/// 列出当前用户的所有工蜂会话。
async fn list_sessions(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
) -> Response {
    let rows: Vec<(i64, i64, String, String)> = sqlx::query_as(&crate::db::q(
        installed.kind,
        "SELECT s.id, s.worker_id, s.title, s.updated_at \
         FROM worker_sessions s WHERE s.user_id = ? ORDER BY s.updated_at DESC",
    ))
    .bind(user.id)
    .fetch_all(&installed.pool)
    .await
    .unwrap_or_default();
    let out: Vec<_> = rows
        .into_iter()
        .map(|(id, worker_id, title, updated_at)| {
            json!({"id": id, "worker_id": worker_id, "title": title, "updated_at": updated_at})
        })
        .collect();
    Json(out).into_response()
}

/// 列出会话历史消息（校验会话归属）。
async fn list_messages(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(sid): Path<i64>,
) -> Response {
    let owner: Option<(i64,)> = sqlx::query_as(&crate::db::q(
        installed.kind,
        "SELECT user_id FROM worker_sessions WHERE id = ?",
    ))
    .bind(sid)
    .fetch_optional(&installed.pool)
    .await
    .ok()
    .flatten();
    match owner {
        Some((uid,)) if uid == user.id => {}
        _ => return (StatusCode::NOT_FOUND, "会话不存在").into_response(),
    }
    let rows: Vec<(i64, String, String, Option<String>)> = sqlx::query_as(&crate::db::q(
        installed.kind,
        "SELECT id, role, content, created_at FROM worker_messages WHERE session_id = ? ORDER BY id",
    ))
    .bind(sid)
    .fetch_all(&installed.pool)
    .await
    .unwrap_or_default();
    let out: Vec<_> = rows
        .into_iter()
        .map(|(id, role, content, created_at)| {
            json!({"id": id, "role": role, "content": content, "created_at": created_at})
        })
        .collect();
    Json(out).into_response()
}

/// 把历史 worker_messages 重建为 Anthropic messages 数组。
/// - role "user"   -> {"role":"user","content": <text>}
/// - role "assistant" -> {"role":"assistant","content": <反序列化的 content 数组>}
/// - role "tool"   -> {"role":"user","content":[{tool_result ...}]}
fn rebuild_messages(rows: &[(String, String)]) -> Vec<serde_json::Value> {
    let mut msgs: Vec<serde_json::Value> = Vec::new();
    for (role, content) in rows {
        match role.as_str() {
            "user" => msgs.push(json!({"role":"user","content": content})),
            "summary" => msgs.push(json!({
                "role": "user",
                "content": format!("[历史摘要]\n{content}")
            })),
            "assistant" => {
                // 存的是 content block 数组的序列化 JSON
                let blocks: serde_json::Value =
                    serde_json::from_str(content).unwrap_or_else(|_| json!(content));
                msgs.push(json!({"role":"assistant","content": blocks}));
            }
            "tool" => {
                // 存的是 {tool_use_id, ok, output}
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(content) {
                    let tool_use_id = v.get("tool_use_id").and_then(|x| x.as_str()).unwrap_or("");
                    let ok = v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false);
                    let output = v.get("output").and_then(|x| x.as_str()).unwrap_or("");
                    msgs.push(json!({"role":"user","content":[{
                        "type":"tool_result",
                        "tool_use_id": tool_use_id,
                        "content": output,
                        "is_error": !ok,
                    }]}));
                }
            }
            _ => {}
        }
    }
    msgs
}

/// 审批端点。
async fn approve(
    State(state): State<AppState>,
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(sid): Path<i64>,
    Json(req): Json<ApproveReq>,
) -> Response {
    // 校验会话归属当前用户
    let owner: Option<(i64,)> = sqlx::query_as(&crate::db::q(
        installed.kind,
        "SELECT user_id FROM worker_sessions WHERE id = ?",
    ))
    .bind(sid)
    .fetch_optional(&installed.pool)
    .await
    .ok()
    .flatten();
    match owner {
        Some((uid,)) if uid == user.id => {}
        _ => return (StatusCode::FORBIDDEN, "无权操作此会话").into_response(),
    }
    // 校验 call_id 属于该会话（key 形如 "{sid}-{n}"）
    if !req.call_id.starts_with(&format!("{sid}-")) {
        return (StatusCode::BAD_REQUEST, "无效的审批标识").into_response();
    }
    if let Some(slot) = state.approvals.write().await.remove(&req.call_id) {
        let _ = slot.send(req.decision);
    }
    Json(json!({"ok": true})).into_response()
}

/// 压缩历史端点：LLM 摘要旧历史，事务重排 worker_messages。
async fn compact(
    State(state): State<AppState>,
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(sid): Path<i64>,
    headers: HeaderMap,
    Json(req): Json<CompactReq>,
) -> Json<serde_json::Value> {
    macro_rules! fail {
        ($msg:expr) => {{
            return Json(json!({"ok": false, "message": $msg}));
        }};
    }

    let pool = installed.pool.clone();
    let kind = installed.kind;

    // 1. 校验会话归属
    let owner: Option<(i64,)> = sqlx::query_as(&crate::db::q(
        kind,
        "SELECT user_id FROM worker_sessions WHERE id = ?",
    ))
    .bind(sid)
    .fetch_optional(&pool)
    .await
    .ok()
    .flatten();
    match owner {
        Some((uid,)) if uid == user.id => {}
        _ => fail!("会话不存在或无权限"),
    }

    // 2. 解析渠道链
    let route = channels::resolve_route(&pool, kind, &headers, "chat", "claude", &req.model).await;
    let chain = match route {
        Ok(channels::Route::Channels { chain, .. }) => chain,
        Ok(channels::Route::Byok(_)) => fail!("请使用服务端渠道（暂不支持 BYOK）"),
        Err(_) => fail!("无可用 Claude 渠道"),
    };

    // 3. 读取全部消息（含 id）
    let rows: Vec<(i64, String, String)> = sqlx::query_as(&crate::db::q(
        kind,
        "SELECT id, role, content FROM worker_messages WHERE session_id = ? ORDER BY id",
    ))
    .bind(sid)
    .fetch_all(&pool)
    .await
    .unwrap_or_default();

    // 4. 切分 to_compact / keep，保留最近 2 个 user 轮
    let user_idxs: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, (_, role, _))| role == "user")
        .map(|(i, _)| i)
        .collect();
    if user_idxs.len() < 3 {
        fail!("历史太短，无需压缩");
    }
    let keep_start = user_idxs[user_idxs.len() - 2];
    let (to_compact, keep) = rows.split_at(keep_start);
    if to_compact.is_empty() {
        fail!("历史太短，无需压缩");
    }

    // 5. 构造摘要请求
    let compact_pairs: Vec<(String, String)> = to_compact
        .iter()
        .map(|(_, role, content)| (role.clone(), content.clone()))
        .collect();
    // 若待压缩段以 assistant 结尾，可能含未配对的 tool_use，会被 API 拒（400）；
    // 仅对发给 Claude 的摘要输入去掉末尾 assistant 轮，DB 重排不受影响。
    let mut llm_pairs = compact_pairs.clone();
    while llm_pairs
        .last()
        .map(|(r, _)| r == "assistant")
        .unwrap_or(false)
    {
        llm_pairs.pop();
    }
    if llm_pairs.is_empty() {
        fail!("历史太短，无需压缩");
    }
    let mut summary_msgs = rebuild_messages(&llm_pairs);
    summary_msgs.push(json!({
        "role": "user",
        "content": "请用中文简洁总结以上对话历史，保留关键事实、涉及的文件路径、命令及其结果、尚未完成的任务，供后续继续。只输出摘要正文，不要寒暄。"
    }));

    // 6. 额度校验（实际扣费在拿到 usage 之后）
    if channels::authorize_chat(&pool, kind, user.id, &req.model)
        .await
        .is_err()
    {
        fail!("额度不足或模型未启用");
    }

    // 7. 调 Claude（不带 tools）
    let resp = match call_claude(
        &state,
        &chain,
        &req.model,
        &serde_json::Value::Array(summary_msgs),
        false,
    )
    .await
    {
        Ok(v) => v,
        // No tokens were delivered, so there is nothing to charge.
        Err(e) => fail!(e),
    };

    // 压缩调用按上游实际返回的 token 用量结算。
    settle_worker_chat(&pool, kind, user.id, &req.model, &resp, "worker_compact").await;

    // 8. 提取摘要文本
    let summary_text = resp
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|arr| {
            arr.iter()
                .find(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                .and_then(|b| b.get("text").and_then(|t| t.as_str()))
        })
        .unwrap_or("")
        .to_string();
    if summary_text.is_empty() {
        // Already settled above: the upstream consumed and billed these tokens
        // even though the summary came back unusable.
        fail!("压缩失败：摘要为空");
    }

    // 9. 事务重排：删除全部消息，把摘要折叠进首个保留的 user 轮
    let mut tx = match pool.begin().await {
        Ok(t) => t,
        Err(_) => fail!("数据库事务开启失败"),
    };
    if sqlx::query(&crate::db::q(
        kind,
        "DELETE FROM worker_messages WHERE session_id = ?",
    ))
    .bind(sid)
    .execute(&mut *tx)
    .await
    .is_err()
    {
        let _ = tx.rollback().await;
        fail!("数据库删除失败");
    }
    let insert_sql = crate::db::q(
        kind,
        "INSERT INTO worker_messages (session_id, role, content) VALUES (?, ?, ?)",
    );
    for (i, (_, role, content)) in keep.iter().enumerate() {
        let (ins_role, ins_content) = if i == 0 {
            (
                "user".to_string(),
                format!("[历史摘要]\n{summary_text}\n\n[最近对话]\n{content}"),
            )
        } else {
            (role.clone(), content.clone())
        };
        if sqlx::query(&insert_sql)
            .bind(sid)
            .bind(&ins_role)
            .bind(&ins_content)
            .execute(&mut *tx)
            .await
            .is_err()
        {
            let _ = tx.rollback().await;
            fail!("写入失败");
        }
    }
    if tx.commit().await.is_err() {
        fail!("事务提交失败");
    }

    // 10. 估算压缩后 token
    let kept_chars: usize = keep.iter().map(|(_, _, c)| c.chars().count()).sum();
    // 粗估：字符数/4≈token（CJK 偏高，仅供 UI 参考）
    let after_estimate = ((summary_text.chars().count() + kept_chars) / 4) as i64;

    Json(json!({ "ok": true, "after_estimate": after_estimate, "summary": summary_text }))
}

/// agent 循环 SSE 端点。
async fn session_message(
    State(state): State<AppState>,
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(sid): Path<i64>,
    headers: HeaderMap,
    Json(req): Json<MessageReq>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let (tx, rx) = mpsc::channel::<Event>(64);

    // 把路由解析放到 spawn 之前，便于把 headers 留在外面。
    let route = channels::resolve_route(
        &installed.pool,
        installed.kind,
        &headers,
        "chat",
        "claude",
        &req.model,
    )
    .await;

    tokio::spawn(async move {
        macro_rules! emit {
            ($ev:expr, $data:expr) => {{
                let _ = tx
                    .send(Event::default().event($ev).data($data))
                    .await;
            }};
        }
        macro_rules! emit_err {
            ($msg:expr) => {{
                let payload = serde_json::to_string(&json!({"message": $msg}))
                    .unwrap_or_else(|_| "{\"message\":\"error\"}".to_string());
                let _ = tx.send(Event::default().event("error").data(payload)).await;
                return;
            }};
        }

        let pool = installed.pool.clone();
        let kind = installed.kind;

        // 1. 校验会话归属
        let owner: Option<(i64,)> = sqlx::query_as(&crate::db::q(
            kind,
            "SELECT user_id FROM worker_sessions WHERE id = ?",
        ))
        .bind(sid)
        .fetch_optional(&pool)
        .await
        .ok()
        .flatten();
        match owner {
            Some((uid,)) if uid == user.id => {}
            _ => emit_err!("会话不存在或无权限"),
        }

        // 2. 取在线工蜂句柄
        let handle = match state.workers.get(req.worker_id).await {
            Some(h) if h.user_id == user.id => h,
            _ => emit_err!("工蜂离线或无权限"),
        };

        // 3. 解析渠道链
        let chain = match route {
            Ok(channels::Route::Channels { chain, .. }) => chain,
            Ok(channels::Route::Byok(_)) => emit_err!("请使用服务端渠道（暂不支持 BYOK）"),
            Err(_) => emit_err!("无可用 Claude 渠道"),
        };

        // 4. 持久化用户消息
        insert_message(&pool, kind, sid, "user", &req.text).await;

        // 5. 重建历史 messages（含本轮用户消息——已落库，统一从库读）
        let hist: Vec<(String, String)> = sqlx::query_as(&crate::db::q(
            kind,
            "SELECT role, content FROM worker_messages WHERE session_id = ? ORDER BY id",
        ))
        .bind(sid)
        .fetch_all(&pool)
        .await
        .unwrap_or_default();
        let mut messages: Vec<serde_json::Value> = rebuild_messages(&hist);
        if messages.is_empty() {
            messages.push(json!({"role":"user","content": req.text}));
        }

        // 6. agent 循环
        let mut approve_counter: u64 = 0;
        let mut iterations: u32 = 0;
        loop {
            iterations += 1;
            if iterations > 25 {
                emit!(
                    "error",
                    serde_json::to_string(&json!({"message":"达到最大轮次上限（25），已停止"}))
                        .unwrap_or_else(|_| "{\"message\":\"error\"}".to_string())
                );
                return;
            }
            // a. 额度校验。每轮都查一次，这样余额耗尽时循环会停在下一轮，
            //    而不是把已经欠费的会话继续跑下去。
            if channels::authorize_chat(&pool, kind, user.id, &req.model)
                .await
                .is_err()
            {
                emit_err!("额度不足或模型未启用");
            }

            // b. 调 Claude
            let messages_val = serde_json::Value::Array(messages.clone());
            let resp = match call_claude(&state, &chain, &req.model, &messages_val, true).await {
                Ok(v) => v,
                // Nothing came back, so nothing is billed.
                Err(e) => emit_err!(e),
            };

            // 本轮按上游返回的 token 用量结算。
            settle_worker_chat(&pool, kind, user.id, &req.model, &resp, "worker_agent").await;

            // 发出本轮真实 token 用量(input+output ≈ 下一轮 context 起点)
            {
                let usage = resp.get("usage").cloned().unwrap_or_else(|| json!({}));
                let input_tokens = usage.get("input_tokens").and_then(|x| x.as_i64()).unwrap_or(0);
                let output_tokens = usage.get("output_tokens").and_then(|x| x.as_i64()).unwrap_or(0);
                emit!(
                    "usage",
                    serde_json::to_string(&json!({
                        "input_tokens": input_tokens,
                        "output_tokens": output_tokens,
                    }))
                    .unwrap_or_else(|_| "{}".to_string())
                );
            }

            // c. 解析 content / stop_reason
            let content = resp
                .get("content")
                .cloned()
                .unwrap_or_else(|| json!([]));
            let stop_reason = resp
                .get("stop_reason")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            // 追加 assistant 轮
            messages.push(json!({"role":"assistant","content": content.clone()}));
            // 落库 assistant content（block 数组的序列化 JSON）
            insert_message(
                &pool,
                kind,
                sid,
                "assistant",
                &serde_json::to_string(&content).unwrap_or_else(|_| "[]".into()),
            )
            .await;

            let blocks = content.as_array().cloned().unwrap_or_default();

            // d. 发出文本块（跳过空白块：工具回合常返回空 text，避免前端渲染占位符）
            for b in &blocks {
                if b.get("type").and_then(|t| t.as_str()) == Some("text") {
                    if let Some(txt) = b.get("text").and_then(|t| t.as_str()) {
                        if !txt.trim().is_empty() {
                            emit!("text", txt.to_string());
                        }
                    }
                }
            }

            // e. 非工具调用 -> 结束
            if stop_reason != "tool_use" {
                emit!("done", "{}".to_string());
                return;
            }

            // f. 逐个处理 tool_use
            let mut results: Vec<serde_json::Value> = Vec::new();
            for b in &blocks {
                if b.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
                    continue;
                }
                let tool_use_id = b.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
                let tool = b.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
                let input = b.get("input").cloned().unwrap_or_else(|| json!({}));

                emit!(
                    "tool_call",
                    serde_json::to_string(&json!({"tool": tool, "input": input}))
                        .unwrap_or_default()
                );

                let needs_approval =
                    (tool == "shell" || tool == "write_file") && !req.auto_approve;

                if needs_approval {
                    approve_counter += 1;
                    let approve_key = format!("{sid}-{approve_counter}");
                    let (atx, arx) = oneshot::channel::<bool>();
                    state
                        .approvals
                        .write()
                        .await
                        .insert(approve_key.clone(), atx);
                    emit!(
                        "approval_required",
                        serde_json::to_string(&json!({
                            "call_id": approve_key,
                            "tool": tool,
                            "input": input,
                        }))
                        .unwrap_or_default()
                    );
                    let decision = match tokio::time::timeout(
                        Duration::from_secs(300),
                        arx,
                    )
                    .await
                    {
                        Ok(Ok(d)) => d,
                        _ => {
                            state.approvals.write().await.remove(&approve_key);
                            false
                        }
                    };
                    if !decision {
                        // 清理（若审批端点未消费）
                        state.approvals.write().await.remove(&approve_key);
                        let out = "用户拒绝了此操作".to_string();
                        emit!(
                            "tool_result",
                            serde_json::to_string(&json!({"ok": false, "output": out}))
                                .unwrap_or_default()
                        );
                        insert_message(
                            &pool,
                            kind,
                            sid,
                            "tool",
                            &serde_json::to_string(&json!({
                                "tool_use_id": tool_use_id,
                                "ok": false,
                                "output": out,
                            }))
                            .unwrap_or_default(),
                        )
                        .await;
                        results.push(json!({
                            "type":"tool_result",
                            "tool_use_id": tool_use_id,
                            "content": out,
                            "is_error": true,
                        }));
                        continue;
                    }
                }

                // 派发给工蜂
                let call_id = format!("exec-{sid}-{tool_use_id}");
                let (rtx, rrx) = oneshot::channel::<ToolOutcome>();
                handle.pending.write().await.insert(call_id.clone(), rtx);
                let send_res = handle
                    .tx
                    .send(ToWorker::Exec {
                        call_id: call_id.clone(),
                        tool: tool.clone(),
                        args: input.clone(),
                    })
                    .await;
                let outcome = if send_res.is_err() {
                    handle.pending.write().await.remove(&call_id);
                    ToolOutcome {
                        ok: false,
                        output: "执行超时或工蜂断开".into(),
                    }
                } else {
                    match tokio::time::timeout(Duration::from_secs(120), rrx).await {
                        Ok(Ok(o)) => o,
                        _ => {
                            handle.pending.write().await.remove(&call_id);
                            ToolOutcome {
                                ok: false,
                                output: "执行超时或工蜂断开".into(),
                            }
                        }
                    }
                };

                emit!(
                    "tool_result",
                    serde_json::to_string(&json!({"ok": outcome.ok, "output": outcome.output}))
                        .unwrap_or_default()
                );
                insert_message(
                    &pool,
                    kind,
                    sid,
                    "tool",
                    &serde_json::to_string(&json!({
                        "tool_use_id": tool_use_id,
                        "ok": outcome.ok,
                        "output": outcome.output,
                    }))
                    .unwrap_or_default(),
                )
                .await;
                results.push(json!({
                    "type":"tool_result",
                    "tool_use_id": tool_use_id,
                    "content": outcome.output,
                    "is_error": !outcome.ok,
                }));
            }

            // g. 把工具结果作为 user 轮回灌，继续循环
            messages.push(json!({"role":"user","content": results}));
        }
    });

    let stream = tokio_stream::wrappers::ReceiverStream::new(rx).map(Ok::<_, Infallible>);
    Sse::new(stream)
}

/// WS 接入端点 —— 公开（鉴权靠配对 token），单独挂载，不经 require_auth。
pub fn public_routes() -> Router<AppState> {
    Router::new().route("/worker/connect", get(ws_connect))
}

fn hash_token(token: &str) -> String {
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    hex::encode(h.finalize())
}

async fn ws_connect(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| handle_socket(state, socket))
}

async fn handle_socket(state: AppState, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();

    // 第一帧必须是 hello
    let first = match stream.next().await {
        Some(Ok(Message::Text(t))) => t.to_string(),
        _ => return,
    };
    let hello: ToServer = match serde_json::from_str(&first) {
        Ok(m) => m,
        Err(_) => return,
    };
    let (token, name) = match hello {
        ToServer::Hello { token, name } => (token, name),
        _ => return,
    };

    let installed = {
        let g = state.installed.read().await;
        match g.clone() {
            Some(i) => i,
            None => return,
        }
    };
    let th = hash_token(&token);
    let row: Option<(i64, i64)> = sqlx::query_as(&crate::db::q(
        installed.kind,
        "SELECT id, user_id FROM workers WHERE token_hash = ?",
    ))
    .bind(&th)
    .fetch_optional(&installed.pool)
    .await
    .ok()
    .flatten();
    let (worker_id, user_id) = match row {
        Some(v) => v,
        None => {
            let err =
                serde_json::to_string(&ToWorker::Error { message: "配对码无效".into() }).unwrap();
            let _ = sink.send(Message::Text(err.into())).await;
            return;
        }
    };

    // 更新名字 + last_seen
    let _ = sqlx::query(&crate::db::q(
        installed.kind,
        &format!(
            "UPDATE workers SET name = ?, last_seen_at = {} WHERE id = ?",
            crate::db::now_expr(installed.kind)
        ),
    ))
    .bind(&name)
    .bind(worker_id)
    .execute(&installed.pool)
    .await;

    // 注册
    let (tx, mut rx) = mpsc::channel::<ToWorker>(64);
    let pending: Arc<RwLock<HashMap<String, oneshot::Sender<ToolOutcome>>>> =
        Arc::new(RwLock::new(HashMap::new()));
    let handle = Arc::new(WorkerHandle {
        user_id,
        tx: tx.clone(),
        pending: pending.clone(),
    });
    state.workers.insert(worker_id, handle).await;

    // 确认
    let ok = serde_json::to_string(&ToWorker::HelloOk { worker_id }).unwrap();
    if sink.send(Message::Text(ok.into())).await.is_err() {
        state.workers.remove(worker_id).await;
        return;
    }

    // 出站任务：把 Exec 推给工蜂
    let mut send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let txt = serde_json::to_string(&msg).unwrap();
            if sink.send(Message::Text(txt.into())).await.is_err() {
                break;
            }
        }
    });

    // 入站循环：心跳 / tool_result
    let pool = installed.pool.clone();
    let kind = installed.kind;
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(frame)) = stream.next().await {
            match frame {
                Message::Text(t) => {
                    if let Ok(msg) = serde_json::from_str::<ToServer>(&t) {
                        match msg {
                            ToServer::Heartbeat => {
                                let _ = sqlx::query(&crate::db::q(
                                    kind,
                                    &format!(
                                        "UPDATE workers SET last_seen_at = {} WHERE id = ?",
                                        crate::db::now_expr(kind)
                                    ),
                                ))
                                .bind(worker_id)
                                .execute(&pool)
                                .await;
                            }
                            ToServer::ToolResult { call_id, ok, output, .. } => {
                                if let Some(slot) = pending.write().await.remove(&call_id) {
                                    let _ = slot.send(ToolOutcome { ok, output });
                                }
                            }
                            ToServer::Hello { .. } => {}
                        }
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    });

    // 任一任务结束即下线
    tokio::select! {
        _ = &mut send_task => {}
        _ = &mut recv_task => {}
    }
    send_task.abort();
    recv_task.abort();
    state.workers.remove(worker_id).await;
}
