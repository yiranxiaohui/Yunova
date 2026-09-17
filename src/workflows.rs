//! Persistent node-canvas workflows for image generation, video generation,
//! trimming, and merging. The graph is stored as a versioned JSON snapshot on
//! every run so later edits never change an in-flight or historical run.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path as FsPath, PathBuf},
    time::Duration,
};

use axum::{
    Extension, Json, Router,
    extract::{Path, State},
    http::StatusCode,
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
    studio, videos,
};

const MAX_NODES: usize = 50;
const MAX_EDGES: usize = 100;
const MAX_MERGE_INPUTS: usize = 12;
const MAX_VIDEO_INPUT_BYTES: usize = 1024 * 1024 * 1024;
const MAX_GRAPH_BYTES: usize = 512 * 1024;
const MAX_NODE_DATA_BYTES: usize = 64 * 1024;
const DEFAULT_EDGE_PRIORITY: i64 = 100;
const MAX_EDGE_PRIORITY: i64 = 9999;

fn err(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({ "error": message.into() }))).into_response()
}

fn random_hex(n: usize) -> String {
    let mut bytes = vec![0_u8; n];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct WorkflowGraph {
    #[serde(default = "graph_version")]
    version: i64,
    nodes: Vec<WorkflowNode>,
    #[serde(default)]
    edges: Vec<WorkflowEdge>,
}

fn graph_version() -> i64 {
    1
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct WorkflowNode {
    id: String,
    #[serde(rename = "type")]
    node_type: String,
    x: f64,
    y: f64,
    #[serde(default)]
    data: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct WorkflowEdge {
    id: String,
    source: String,
    target: String,
    #[serde(default = "default_edge_priority")]
    priority: i64,
}

fn default_edge_priority() -> i64 {
    DEFAULT_EDGE_PRIORITY
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OutputKind {
    Image,
    Video,
}

fn output_kind(node_type: &str) -> Option<OutputKind> {
    match node_type {
        "image_generation" => Some(OutputKind::Image),
        "video_generation" | "video_trim" | "video_merge" => Some(OutputKind::Video),
        _ => None,
    }
}

fn validate_graph(graph: &WorkflowGraph, runnable: bool) -> Result<(), String> {
    if graph.version != 1 {
        return Err("不支持的工作流版本".into());
    }
    if graph.nodes.is_empty() {
        return Err("画布中至少需要一个节点".into());
    }
    if graph.nodes.len() > MAX_NODES || graph.edges.len() > MAX_EDGES {
        return Err(format!(
            "单个工作流最多 {MAX_NODES} 个节点、{MAX_EDGES} 条连线"
        ));
    }

    let mut node_index = HashMap::new();
    for (index, node) in graph.nodes.iter().enumerate() {
        if node.id.is_empty()
            || node.id.len() > 96
            || !node
                .id
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
        {
            return Err("节点 ID 格式无效".into());
        }
        if output_kind(&node.node_type).is_none() {
            return Err(format!("节点 {} 的类型不受支持", node.id));
        }
        if !node.x.is_finite() || !node.y.is_finite() {
            return Err(format!("节点 {} 的位置无效", node.id));
        }
        if serde_json::to_vec(&node.data)
            .map(|value| value.len() > MAX_NODE_DATA_BYTES)
            .unwrap_or(true)
        {
            return Err(format!("节点 {} 的配置过大", node.id));
        }
        if node_index.insert(node.id.as_str(), index).is_some() {
            return Err(format!("节点 ID 重复：{}", node.id));
        }
        if runnable {
            validate_node_config(node)?;
        }
    }

    let mut edge_ids = HashSet::new();
    let mut indegree = vec![0_usize; graph.nodes.len()];
    let mut outgoing: Vec<Vec<usize>> = vec![Vec::new(); graph.nodes.len()];
    let mut incoming: Vec<Vec<usize>> = vec![Vec::new(); graph.nodes.len()];
    for edge in &graph.edges {
        if edge.id.is_empty() || !edge_ids.insert(edge.id.as_str()) {
            return Err("连线 ID 为空或重复".into());
        }
        if !(0..=MAX_EDGE_PRIORITY).contains(&edge.priority) {
            return Err(format!(
                "连线 {} 的优先级需要在 0～{MAX_EDGE_PRIORITY} 之间",
                edge.id
            ));
        }
        let Some(&source) = node_index.get(edge.source.as_str()) else {
            return Err(format!("连线来源节点不存在：{}", edge.source));
        };
        let Some(&target) = node_index.get(edge.target.as_str()) else {
            return Err(format!("连线目标节点不存在：{}", edge.target));
        };
        if source == target {
            return Err("节点不能连接到自身".into());
        }
        indegree[target] += 1;
        outgoing[source].push(target);
        incoming[target].push(source);
    }

    let mut queue: VecDeque<usize> = indegree
        .iter()
        .enumerate()
        .filter_map(|(index, degree)| (*degree == 0).then_some(index))
        .collect();
    let mut visited = 0;
    while let Some(index) = queue.pop_front() {
        visited += 1;
        for &target in &outgoing[index] {
            indegree[target] -= 1;
            if indegree[target] == 0 {
                queue.push_back(target);
            }
        }
    }
    if visited != graph.nodes.len() {
        return Err("工作流中存在循环连线".into());
    }

    for (index, node) in graph.nodes.iter().enumerate() {
        let inputs = &incoming[index];
        match node.node_type.as_str() {
            "image_generation" => {
                if inputs.iter().any(|source| {
                    output_kind(&graph.nodes[*source].node_type) != Some(OutputKind::Image)
                }) {
                    return Err(format!("图片节点 {} 只能接收图片输入", node.id));
                }
            }
            "video_generation" => {
                if inputs.len() > 1
                    || inputs.iter().any(|source| {
                        output_kind(&graph.nodes[*source].node_type) != Some(OutputKind::Image)
                    })
                {
                    return Err(format!("视频生成节点 {} 最多接收一张图片", node.id));
                }
            }
            "video_trim" => {
                if inputs.len() > 1
                    || inputs.iter().any(|source| {
                        output_kind(&graph.nodes[*source].node_type) != Some(OutputKind::Video)
                    })
                    || (runnable && inputs.len() != 1)
                {
                    return Err(format!("视频裁剪节点 {} 必须接收一个视频", node.id));
                }
            }
            "video_merge" => {
                if inputs.len() > MAX_MERGE_INPUTS
                    || inputs.iter().any(|source| {
                        output_kind(&graph.nodes[*source].node_type) != Some(OutputKind::Video)
                    })
                    || (runnable && inputs.len() < 2)
                {
                    return Err(format!(
                        "视频合并节点 {} 需要 2～{MAX_MERGE_INPUTS} 个视频输入",
                        node.id
                    ));
                }
            }
            _ => unreachable!(),
        }
    }
    Ok(())
}

fn data_string(node: &WorkflowNode, key: &str) -> Option<String> {
    node.data
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn data_i64(node: &WorkflowNode, key: &str) -> Option<i64> {
    node.data.get(key).and_then(Value::as_i64)
}

fn data_f64(node: &WorkflowNode, key: &str) -> Option<f64> {
    node.data.get(key).and_then(Value::as_f64)
}

fn validate_node_config(node: &WorkflowNode) -> Result<(), String> {
    match node.node_type.as_str() {
        "image_generation" => {
            let model = data_string(node, "model");
            let prompt = data_string(node, "prompt");
            if model.is_none() || prompt.is_none() {
                return Err(format!("图片节点 {} 需要模型和提示词", node.id));
            }
            if model.is_some_and(|value| value.len() > 255)
                || prompt.is_some_and(|value| value.len() > 50_000)
            {
                return Err(format!("图片节点 {} 的参数过长", node.id));
            }
        }
        "video_generation" => {
            let model = data_string(node, "model");
            let prompt = data_string(node, "prompt");
            let size = data_string(node, "size");
            if model.is_none()
                || prompt.is_none()
                || size.is_none()
                || data_i64(node, "seconds").unwrap_or(0) <= 0
            {
                return Err(format!("视频节点 {} 的参数不完整", node.id));
            }
            if model.is_some_and(|value| value.len() > 255)
                || prompt.is_some_and(|value| value.len() > 50_000)
                || size.is_some_and(|value| value.len() > 64)
            {
                return Err(format!("视频节点 {} 的参数过长", node.id));
            }
        }
        "video_trim" => {
            let start = data_f64(node, "start").unwrap_or(-1.0);
            let end = data_f64(node, "end").unwrap_or(-1.0);
            if start < 0.0 || end <= start || end > 21_600.0 {
                return Err(format!("裁剪节点 {} 的起止时间无效", node.id));
            }
        }
        "video_merge" => {
            let width = data_i64(node, "width").unwrap_or(1280);
            let height = data_i64(node, "height").unwrap_or(720);
            if !(256..=3840).contains(&width)
                || !(256..=3840).contains(&height)
                || width % 2 != 0
                || height % 2 != 0
            {
                return Err(format!("合并节点 {} 的输出尺寸无效", node.id));
            }
        }
        _ => return Err(format!("节点类型不受支持：{}", node.node_type)),
    }
    Ok(())
}

#[derive(sqlx::FromRow)]
struct WorkflowRow {
    id: i64,
    name: String,
    graph_json: String,
    created_at: String,
    updated_at: String,
}

#[derive(Serialize)]
struct WorkflowView {
    id: i64,
    name: String,
    graph: Value,
    created_at: String,
    updated_at: String,
}

impl From<WorkflowRow> for WorkflowView {
    fn from(row: WorkflowRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            graph: serde_json::from_str(&row.graph_json).unwrap_or(Value::Null),
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

#[derive(Deserialize)]
struct SaveWorkflowReq {
    id: Option<i64>,
    name: String,
    graph: WorkflowGraph,
}

#[derive(Serialize)]
struct SaveWorkflowResp {
    id: i64,
}

async fn list_workflows(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
) -> Response {
    let sql = db::q(
        installed.kind,
        "SELECT id, name, graph_json, created_at, updated_at FROM workflows \
         WHERE user_id = ? ORDER BY updated_at DESC, id DESC",
    );
    match sqlx::query_as::<_, WorkflowRow>(&sql)
        .bind(user.id)
        .fetch_all(&installed.pool)
        .await
    {
        Ok(rows) => {
            Json(rows.into_iter().map(WorkflowView::from).collect::<Vec<_>>()).into_response()
        }
        Err(error) => err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

async fn save_workflow(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Json(request): Json<SaveWorkflowReq>,
) -> Response {
    let name = request.name.trim();
    if name.is_empty() || name.chars().count() > 100 || name.chars().any(char::is_control) {
        return err(StatusCode::BAD_REQUEST, "工作流名称需要 1～100 个字符");
    }
    if let Err(message) = validate_graph(&request.graph, false) {
        return err(StatusCode::BAD_REQUEST, message);
    }
    let graph_json = match serde_json::to_string(&request.graph) {
        Ok(value) => value,
        Err(error) => return err(StatusCode::BAD_REQUEST, error.to_string()),
    };
    if graph_json.len() > MAX_GRAPH_BYTES {
        return err(StatusCode::BAD_REQUEST, "工作流配置不能超过 512 KiB");
    }

    if let Some(id) = request.id {
        let now = db::now_expr(installed.kind);
        let sql = db::q(
            installed.kind,
            &format!(
                "UPDATE workflows SET name = ?, graph_json = ?, updated_at = {now} \
                 WHERE id = ? AND user_id = ?"
            ),
        );
        let changed = sqlx::query(&sql)
            .bind(name)
            .bind(&graph_json)
            .bind(id)
            .bind(user.id)
            .execute(&installed.pool)
            .await;
        return match changed {
            Ok(result) if result.rows_affected() > 0 => {
                Json(SaveWorkflowResp { id }).into_response()
            }
            Ok(_) => err(StatusCode::NOT_FOUND, "工作流不存在"),
            Err(error) => err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
        };
    }

    let sql = db::q(
        installed.kind,
        "INSERT INTO workflows (user_id, name, graph_json) VALUES (?, ?, ?) RETURNING id",
    );
    let id = sqlx::query_as::<_, (i64,)>(&sql)
        .bind(user.id)
        .bind(name)
        .bind(&graph_json)
        .fetch_one(&installed.pool)
        .await
        .map(|row| row.0);
    match id {
        Ok(id) => Json(SaveWorkflowResp { id }).into_response(),
        Err(error) => err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

async fn delete_workflow(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Response {
    let sql = db::q(
        installed.kind,
        "DELETE FROM workflows WHERE id = ? AND user_id = ?",
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

#[derive(Clone, sqlx::FromRow)]
struct RunRow {
    id: i64,
    token: String,
    workflow_id: Option<i64>,
    user_id: i64,
    name: String,
    graph_json: String,
    status: String,
    error: Option<String>,
    created_at: String,
    finished_at: Option<String>,
}

#[derive(Clone, sqlx::FromRow)]
struct NodeRunRow {
    id: i64,
    node_key: String,
    node_type: String,
    status: String,
    job_token: Option<String>,
    output_paths: Option<String>,
    error: Option<String>,
    started_at: Option<String>,
    finished_at: Option<String>,
}

#[derive(Clone, sqlx::FromRow)]
struct RunLogRow {
    id: i64,
    node_key: Option<String>,
    level: String,
    message: String,
    created_at: String,
}

#[derive(Serialize)]
struct NodeRunView {
    node_id: String,
    node_type: String,
    status: String,
    job_token: Option<String>,
    output_paths: Vec<String>,
    error: Option<String>,
    started_at: Option<String>,
    finished_at: Option<String>,
}

#[derive(Serialize)]
struct RunLogView {
    id: i64,
    node_id: Option<String>,
    level: String,
    message: String,
    created_at: String,
}

#[derive(Serialize)]
struct RunView {
    token: String,
    workflow_id: Option<i64>,
    name: String,
    graph: Value,
    status: String,
    error: Option<String>,
    created_at: String,
    finished_at: Option<String>,
    nodes: Vec<NodeRunView>,
    logs: Vec<RunLogView>,
}

fn paths_of(row: &NodeRunRow) -> Vec<String> {
    row.output_paths
        .as_deref()
        .and_then(|value| serde_json::from_str(value).ok())
        .unwrap_or_default()
}

fn input_edges<'a>(
    graph: &'a WorkflowGraph,
    target: &str,
    sort_by_priority: bool,
) -> Vec<&'a WorkflowEdge> {
    let mut edges: Vec<_> = graph
        .edges
        .iter()
        .filter(|edge| edge.target == target)
        .collect();
    if sort_by_priority {
        // `sort_by_key` is stable, so equal priorities retain connection order.
        edges.sort_by_key(|edge| edge.priority);
    }
    edges
}

async fn fetch_run(pool: &db::Pool, kind: db::DbKind, run_id: i64) -> Option<RunRow> {
    let sql = db::q(
        kind,
        "SELECT id, token, workflow_id, user_id, name, graph_json, status, error, \
         created_at, finished_at FROM workflow_runs WHERE id = ?",
    );
    sqlx::query_as(&sql)
        .bind(run_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
}

async fn fetch_run_by_token(pool: &db::Pool, kind: db::DbKind, token: &str) -> Option<RunRow> {
    let sql = db::q(
        kind,
        "SELECT id, token, workflow_id, user_id, name, graph_json, status, error, \
         created_at, finished_at FROM workflow_runs WHERE token = ?",
    );
    sqlx::query_as(&sql)
        .bind(token)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
}

async fn fetch_node_runs(pool: &db::Pool, kind: db::DbKind, run_id: i64) -> Vec<NodeRunRow> {
    let sql = db::q(
        kind,
        "SELECT id, node_key, node_type, status, job_token, output_paths, error, \
         started_at, finished_at FROM workflow_node_runs WHERE run_id = ? ORDER BY id",
    );
    sqlx::query_as(&sql)
        .bind(run_id)
        .fetch_all(pool)
        .await
        .unwrap_or_default()
}

async fn fetch_run_logs(pool: &db::Pool, kind: db::DbKind, run_id: i64) -> Vec<RunLogRow> {
    let sql = db::q(
        kind,
        "SELECT id, node_key, level, message, created_at FROM workflow_run_logs \
         WHERE run_id = ? ORDER BY id DESC LIMIT 500",
    );
    let mut rows: Vec<RunLogRow> = sqlx::query_as(&sql)
        .bind(run_id)
        .fetch_all(pool)
        .await
        .unwrap_or_default();
    rows.reverse();
    rows
}

async fn append_run_log(
    pool: &db::Pool,
    kind: db::DbKind,
    run_id: i64,
    node_key: Option<&str>,
    level: &str,
    message: &str,
) {
    let message: String = message.chars().take(2000).collect();
    let sql = db::q(
        kind,
        "INSERT INTO workflow_run_logs (run_id, node_key, level, message) VALUES (?, ?, ?, ?)",
    );
    let _ = sqlx::query(&sql)
        .bind(run_id)
        .bind(node_key)
        .bind(level)
        .bind(message)
        .execute(pool)
        .await;
}

async fn run_view(
    pool: &db::Pool,
    kind: db::DbKind,
    run: RunRow,
    include_logs: bool,
) -> RunView {
    let nodes = fetch_node_runs(pool, kind, run.id)
        .await
        .into_iter()
        .map(|row| {
            let output_paths = paths_of(&row);
            NodeRunView {
                node_id: row.node_key,
                node_type: row.node_type,
                status: row.status,
                job_token: row.job_token,
                output_paths,
                error: row.error,
                started_at: row.started_at,
                finished_at: row.finished_at,
            }
        })
        .collect();
    let logs = if include_logs {
        fetch_run_logs(pool, kind, run.id)
            .await
            .into_iter()
            .map(|row| RunLogView {
                id: row.id,
                node_id: row.node_key,
                level: row.level,
                message: row.message,
                created_at: row.created_at,
            })
            .collect()
    } else {
        Vec::new()
    };
    RunView {
        token: run.token,
        workflow_id: run.workflow_id,
        name: run.name,
        graph: serde_json::from_str(&run.graph_json).unwrap_or(Value::Null),
        status: run.status,
        error: run.error,
        created_at: run.created_at,
        finished_at: run.finished_at,
        nodes,
        logs,
    }
}

async fn get_run(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(token): Path<String>,
) -> Response {
    match fetch_run_by_token(&installed.pool, installed.kind, &token).await {
        Some(run) if run.user_id == user.id => {
            Json(run_view(&installed.pool, installed.kind, run, true).await).into_response()
        }
        _ => err(StatusCode::NOT_FOUND, "运行记录不存在"),
    }
}

async fn list_runs(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
) -> Response {
    let sql = db::q(
        installed.kind,
        "SELECT id, token, workflow_id, user_id, name, graph_json, status, error, \
         created_at, finished_at FROM workflow_runs WHERE user_id = ? \
         ORDER BY created_at DESC, id DESC LIMIT 30",
    );
    let rows: Vec<RunRow> = match sqlx::query_as(&sql)
        .bind(user.id)
        .fetch_all(&installed.pool)
        .await
    {
        Ok(rows) => rows,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    let mut views = Vec::with_capacity(rows.len());
    for row in rows {
        views.push(run_view(&installed.pool, installed.kind, row, false).await);
    }
    Json(views).into_response()
}

async fn insert_run(
    pool: &db::Pool,
    kind: db::DbKind,
    workflow_id: i64,
    user_id: i64,
    token: &str,
    name: &str,
    graph_json: &str,
) -> Result<i64, sqlx::Error> {
    let sql = db::q(
        kind,
        "INSERT INTO workflow_runs (token, workflow_id, user_id, name, graph_json, status) \
         VALUES (?, ?, ?, ?, ?, 'running') RETURNING id",
    );
    sqlx::query_as::<_, (i64,)>(&sql)
        .bind(token)
        .bind(workflow_id)
        .bind(user_id)
        .bind(name)
        .bind(graph_json)
        .fetch_one(pool)
        .await
        .map(|row| row.0)
}

#[derive(Serialize)]
struct CreateRunResp {
    token: String,
}

async fn create_run(
    State(state): State<AppState>,
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(workflow_id): Path<i64>,
) -> Response {
    let sql = db::q(
        installed.kind,
        "SELECT id, name, graph_json, created_at, updated_at FROM workflows \
         WHERE id = ? AND user_id = ?",
    );
    let workflow: WorkflowRow = match sqlx::query_as(&sql)
        .bind(workflow_id)
        .bind(user.id)
        .fetch_optional(&installed.pool)
        .await
    {
        Ok(Some(row)) => row,
        Ok(None) => return err(StatusCode::NOT_FOUND, "工作流不存在"),
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    let graph: WorkflowGraph = match serde_json::from_str(&workflow.graph_json) {
        Ok(graph) => graph,
        Err(error) => return err(StatusCode::BAD_REQUEST, error.to_string()),
    };
    if let Err(message) = validate_graph(&graph, true) {
        return err(StatusCode::BAD_REQUEST, message);
    }

    let token = random_hex(16);
    let run_id = match insert_run(
        &installed.pool,
        installed.kind,
        workflow.id,
        user.id,
        &token,
        &workflow.name,
        &workflow.graph_json,
    )
    .await
    {
        Ok(id) => id,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    let insert_node = db::q(
        installed.kind,
        "INSERT INTO workflow_node_runs (run_id, node_key, node_type, status) \
         VALUES (?, ?, ?, 'waiting')",
    );
    for node in &graph.nodes {
        if let Err(error) = sqlx::query(&insert_node)
            .bind(run_id)
            .bind(&node.id)
            .bind(&node.node_type)
            .execute(&installed.pool)
            .await
        {
            let cleanup = db::q(installed.kind, "DELETE FROM workflow_runs WHERE id = ?");
            let _ = sqlx::query(&cleanup)
                .bind(run_id)
                .execute(&installed.pool)
                .await;
            return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
        }
    }

    append_run_log(
        &installed.pool,
        installed.kind,
        run_id,
        None,
        "info",
        &format!("流水线已开始，共 {} 个节点", graph.nodes.len()),
    )
    .await;
    spawn_driver(state, run_id);
    (StatusCode::CREATED, Json(CreateRunResp { token })).into_response()
}

fn spawn_driver(state: AppState, run_id: i64) {
    tokio::spawn(async move {
        loop {
            let Some(installed) = state.installed.read().await.clone() else {
                return;
            };
            drive_run_once(&state, &installed, run_id).await;
            let active = fetch_run(&installed.pool, installed.kind, run_id)
                .await
                .is_some_and(|run| run.status == "running");
            if !active {
                return;
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    });
}

async fn set_node_failed(
    pool: &db::Pool,
    kind: db::DbKind,
    run_id: i64,
    node: &NodeRunRow,
    message: &str,
) {
    let now = db::now_expr(kind);
    let message: String = message.chars().take(1000).collect();
    let sql = db::q(
        kind,
        &format!(
            "UPDATE workflow_node_runs SET status = 'failed', error = ?, finished_at = {now} \
             WHERE id = ? AND status IN ('starting', 'running')"
        ),
    );
    let changed = sqlx::query(&sql)
        .bind(&message)
        .bind(node.id)
        .execute(pool)
        .await
        .map(|result| result.rows_affected())
        .unwrap_or(0);
    if changed > 0 {
        append_run_log(
            pool,
            kind,
            run_id,
            Some(&node.node_key),
            "error",
            &format!("执行失败：{message}"),
        )
        .await;
    }
}

async fn set_node_complete(
    pool: &db::Pool,
    kind: db::DbKind,
    run: &RunRow,
    node: &NodeRunRow,
    output_paths: &[String],
) {
    let now = db::now_expr(kind);
    let paths_json = serde_json::to_string(output_paths).unwrap_or_else(|_| "[]".into());
    let sql = db::q(
        kind,
        &format!(
            "UPDATE workflow_node_runs SET status = 'completed', output_paths = ?, error = NULL, \
             finished_at = {now} WHERE id = ? AND status IN ('starting', 'running')"
        ),
    );
    let changed = sqlx::query(&sql)
        .bind(&paths_json)
        .bind(node.id)
        .execute(pool)
        .await
        .map(|result| result.rows_affected())
        .unwrap_or(0);
    if changed == 0 {
        return;
    }
    append_run_log(
        pool,
        kind,
        run.id,
        Some(&node.node_key),
        "success",
        &format!("执行完成，生成 {} 个文件", output_paths.len()),
    )
    .await;
    let asset_kind = if node.node_type == "image_generation" {
        "image"
    } else {
        "video"
    };
    let insert = db::q(
        kind,
        "INSERT INTO media_assets \
         (user_id, workflow_run_id, workflow_node_run_id, kind, path, metadata_json) \
         VALUES (?, ?, ?, ?, ?, ?)",
    );
    for path in output_paths {
        let metadata = json!({ "node_id": node.node_key, "node_type": node.node_type }).to_string();
        let _ = sqlx::query(&insert)
            .bind(run.user_id)
            .bind(run.id)
            .bind(node.id)
            .bind(asset_kind)
            .bind(path)
            .bind(&metadata)
            .execute(pool)
            .await;
    }
}

async fn drive_run_once(state: &AppState, installed: &InstalledState, run_id: i64) {
    let Some(run) = fetch_run(&installed.pool, installed.kind, run_id).await else {
        return;
    };
    if run.status != "running" {
        return;
    }
    let Ok(graph) = serde_json::from_str::<WorkflowGraph>(&run.graph_json) else {
        finish_run(
            &installed.pool,
            installed.kind,
            run_id,
            "failed",
            Some("工作流快照损坏"),
        )
        .await;
        return;
    };

    // First synchronize asynchronous image/video child jobs.
    for node in fetch_node_runs(&installed.pool, installed.kind, run_id)
        .await
        .into_iter()
        .filter(|node| node.status == "running" && node.job_token.is_some())
    {
        let token = node.job_token.as_deref().unwrap_or_default();
        match node.node_type.as_str() {
            "image_generation" => match studio::workflow_generation_state(
                &installed.pool,
                installed.kind,
                run.user_id,
                token,
            )
            .await
            {
                Ok(status) if status.status == "done" => {
                    if let Some(path) = status.output_path {
                        set_node_complete(&installed.pool, installed.kind, &run, &node, &[path])
                            .await;
                    } else {
                        set_node_failed(
                            &installed.pool,
                            installed.kind,
                            run.id,
                            &node,
                            "图片任务没有输出",
                        )
                        .await;
                    }
                }
                Ok(status) if status.status == "failed" => {
                    set_node_failed(
                        &installed.pool,
                        installed.kind,
                        run.id,
                        &node,
                        status.error.as_deref().unwrap_or("图片生成失败"),
                    )
                    .await;
                }
                Err(message) => {
                    set_node_failed(&installed.pool, installed.kind, run.id, &node, &message).await
                }
                _ => {}
            },
            "video_generation" => {
                match videos::workflow_video_state(state, installed, run.user_id, token).await {
                    Ok(status) if status.status == "completed" => {
                        if let Some(path) = status.output_path {
                            set_node_complete(
                                &installed.pool,
                                installed.kind,
                                &run,
                                &node,
                                &[path],
                            )
                            .await;
                        } else {
                            set_node_failed(
                                &installed.pool,
                                installed.kind,
                                run.id,
                                &node,
                                "视频任务没有输出",
                            )
                            .await;
                        }
                    }
                    Ok(status) if status.status == "failed" => {
                        set_node_failed(
                            &installed.pool,
                            installed.kind,
                            run.id,
                            &node,
                            status.error.as_deref().unwrap_or("视频生成失败"),
                        )
                        .await;
                    }
                    Err(message) => {
                        set_node_failed(&installed.pool, installed.kind, run.id, &node, &message).await
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    let node_rows = fetch_node_runs(&installed.pool, installed.kind, run_id).await;
    let rows_by_key: HashMap<&str, &NodeRunRow> = node_rows
        .iter()
        .map(|node| (node.node_key.as_str(), node))
        .collect();
    let graph_nodes: HashMap<&str, &WorkflowNode> = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();

    for node_run in node_rows.iter().filter(|node| node.status == "waiting") {
        let predecessors: Vec<&NodeRunRow> = graph
            .edges
            .iter()
            .filter(|edge| edge.target == node_run.node_key)
            .filter_map(|edge| rows_by_key.get(edge.source.as_str()).copied())
            .collect();
        if predecessors.iter().any(|node| {
            node.status == "failed" || node.status == "blocked" || node.status == "cancelled"
        }) {
            let now = db::now_expr(installed.kind);
            let sql = db::q(
                installed.kind,
                &format!(
                    "UPDATE workflow_node_runs SET status = 'blocked', \
                     error = '上游节点失败', finished_at = {now} WHERE id = ? AND status = 'waiting'"
                ),
            );
            let changed = sqlx::query(&sql)
                .bind(node_run.id)
                .execute(&installed.pool)
                .await
                .map(|result| result.rows_affected())
                .unwrap_or(0);
            if changed > 0 {
                append_run_log(
                    &installed.pool,
                    installed.kind,
                    run.id,
                    Some(&node_run.node_key),
                    "warning",
                    "上游节点失败，本节点已跳过",
                )
                .await;
            }
            continue;
        }
        if !predecessors.iter().all(|node| node.status == "completed") {
            continue;
        }
        let now = db::now_expr(installed.kind);
        let claim = db::q(
            installed.kind,
            &format!(
                "UPDATE workflow_node_runs SET status = 'starting', started_at = {now}, error = NULL \
                 WHERE id = ? AND status = 'waiting'"
            ),
        );
        let claimed = sqlx::query(&claim)
            .bind(node_run.id)
            .execute(&installed.pool)
            .await
            .map(|result| result.rows_affected())
            .unwrap_or(0);
        if claimed == 0 {
            continue;
        }
        append_run_log(
            &installed.pool,
            installed.kind,
            run.id,
            Some(&node_run.node_key),
            "info",
            "开始执行",
        )
        .await;
        let Some(graph_node) = graph_nodes
            .get(node_run.node_key.as_str())
            .copied()
            .cloned()
        else {
            set_node_failed(
                &installed.pool,
                installed.kind,
                run.id,
                node_run,
                "节点定义不存在",
            )
            .await;
            continue;
        };
        let inputs: Vec<String> = input_edges(
            &graph,
            &node_run.node_key,
            graph_node.node_type == "video_merge",
        )
        .into_iter()
        .filter_map(|edge| rows_by_key.get(edge.source.as_str()).copied())
        .flat_map(paths_of)
        .collect();
        let state_clone = state.clone();
        let installed_clone = installed.clone();
        let run_clone = run.clone();
        let node_clone = node_run.clone();
        tokio::spawn(async move {
            if let Err(message) = execute_node(
                &state_clone,
                &installed_clone,
                &run_clone,
                &node_clone,
                &graph_node,
                inputs,
            )
            .await
            {
                set_node_failed(
                    &installed_clone.pool,
                    installed_clone.kind,
                    run_clone.id,
                    &node_clone,
                    &message,
                )
                .await;
            }
        });
    }

    let latest = fetch_node_runs(&installed.pool, installed.kind, run_id).await;
    if latest.iter().all(|node| node.status == "completed") {
        finish_run(&installed.pool, installed.kind, run_id, "completed", None).await;
    } else if latest.iter().all(|node| {
        matches!(
            node.status.as_str(),
            "completed" | "failed" | "blocked" | "cancelled"
        )
    }) {
        finish_run(
            &installed.pool,
            installed.kind,
            run_id,
            "failed",
            Some("一个或多个节点执行失败"),
        )
        .await;
    }
}

async fn finish_run(
    pool: &db::Pool,
    kind: db::DbKind,
    run_id: i64,
    status: &str,
    error: Option<&str>,
) {
    let now = db::now_expr(kind);
    let sql = db::q(
        kind,
        &format!(
            "UPDATE workflow_runs SET status = ?, error = ?, finished_at = {now} \
             WHERE id = ? AND status = 'running'"
        ),
    );
    let changed = sqlx::query(&sql)
        .bind(status)
        .bind(error)
        .bind(run_id)
        .execute(pool)
        .await
        .map(|result| result.rows_affected())
        .unwrap_or(0);
    if changed == 0 {
        return;
    }
    let (level, message) = if status == "completed" {
        ("success", "流水线运行完成".to_string())
    } else {
        (
            "error",
            error
                .map(|message| format!("流水线运行失败：{message}"))
                .unwrap_or_else(|| "流水线运行失败".to_string()),
        )
    };
    append_run_log(pool, kind, run_id, None, level, &message).await;
}

async fn mark_node_running(
    pool: &db::Pool,
    kind: db::DbKind,
    run_id: i64,
    node: &NodeRunRow,
    job_token: Option<&str>,
) -> Result<(), String> {
    let sql = db::q(
        kind,
        "UPDATE workflow_node_runs SET status = 'running', job_token = ? \
         WHERE id = ? AND status = 'starting'",
    );
    let changed = sqlx::query(&sql)
        .bind(job_token)
        .bind(node.id)
        .execute(pool)
        .await
        .map_err(|error| error.to_string())?
        .rows_affected();
    if changed == 0 {
        return Err("节点状态已变化".into());
    }
    let message = if job_token.is_some() {
        "生成任务已提交，等待处理结果"
    } else {
        "媒体处理已启动"
    };
    append_run_log(
        pool,
        kind,
        run_id,
        Some(&node.node_key),
        "info",
        message,
    )
    .await;
    Ok(())
}

async fn execute_node(
    state: &AppState,
    installed: &InstalledState,
    run: &RunRow,
    node_run: &NodeRunRow,
    node: &WorkflowNode,
    inputs: Vec<String>,
) -> Result<(), String> {
    match node.node_type.as_str() {
        "image_generation" => {
            let mut image_data_urls = Vec::new();
            for path in inputs {
                let name = media_name(&path, "/api/images/")?;
                let bytes = state
                    .storage
                    .get(MediaKind::Image, &name)
                    .await
                    .map_err(|error| format!("读取输入图片失败: {error}"))?;
                let mime = mime_guess::from_path(&name)
                    .first_or_octet_stream()
                    .essence_str()
                    .to_string();
                image_data_urls.push(format!("data:{mime};base64,{}", STANDARD.encode(bytes)));
            }
            let token = studio::start_workflow_generation(
                state.clone(),
                installed.clone(),
                run.user_id,
                studio::WorkflowImageRequest {
                    prompt: data_string(node, "prompt").ok_or("缺少图片提示词")?,
                    model: data_string(node, "model").ok_or("缺少图片模型")?,
                    size: data_string(node, "size").filter(|value| value != "auto"),
                    quality: data_string(node, "quality").filter(|value| value != "auto"),
                    style: data_string(node, "style"),
                    image_data_urls,
                },
            )
            .await?;
            mark_node_running(
                &installed.pool,
                installed.kind,
                run.id,
                node_run,
                Some(&token),
            )
            .await
        }
        "video_generation" => {
            let token = videos::start_workflow_video(
                state.clone(),
                installed.clone(),
                run.user_id,
                videos::WorkflowVideoRequest {
                    model: data_string(node, "model").ok_or("缺少视频模型")?,
                    prompt: data_string(node, "prompt").ok_or("缺少视频提示词")?,
                    seconds: data_i64(node, "seconds").ok_or("缺少视频时长")?,
                    size: data_string(node, "size").ok_or("缺少视频尺寸")?,
                    input_image_path: inputs.first().cloned(),
                },
            )
            .await?;
            mark_node_running(
                &installed.pool,
                installed.kind,
                run.id,
                node_run,
                Some(&token),
            )
            .await
        }
        "video_trim" => {
            mark_node_running(&installed.pool, installed.kind, run.id, node_run, None).await?;
            let _permit = state
                .media_process_slots
                .acquire()
                .await
                .map_err(|_| "媒体处理队列已关闭")?;
            let input = inputs.first().ok_or("裁剪节点没有视频输入")?;
            let output = trim_video(
                state,
                input,
                data_f64(node, "start").ok_or("缺少开始时间")?,
                data_f64(node, "end").ok_or("缺少结束时间")?,
            )
            .await?;
            set_node_complete(&installed.pool, installed.kind, run, node_run, &[output]).await;
            Ok(())
        }
        "video_merge" => {
            mark_node_running(&installed.pool, installed.kind, run.id, node_run, None).await?;
            let _permit = state
                .media_process_slots
                .acquire()
                .await
                .map_err(|_| "媒体处理队列已关闭")?;
            let output = merge_videos(
                state,
                &inputs,
                data_i64(node, "width").unwrap_or(1280),
                data_i64(node, "height").unwrap_or(720),
            )
            .await?;
            set_node_complete(&installed.pool, installed.kind, run, node_run, &[output]).await;
            Ok(())
        }
        _ => Err("不支持的节点类型".into()),
    }
}

fn media_name(path: &str, prefix: &str) -> Result<String, String> {
    let name = path
        .strip_prefix(prefix)
        .filter(|name| !name.is_empty() && !name.contains('/') && !name.contains(".."))
        .ok_or_else(|| "媒体路径无效".to_string())?;
    Ok(name.to_string())
}

fn workflow_temp_dir(state: &AppState) -> PathBuf {
    state
        .data_dir
        .join("workflow-tmp")
        .join(format!("job-{}", random_hex(12)))
}

async fn run_command(mut command: tokio::process::Command, label: &str) -> Result<(), String> {
    command.kill_on_drop(true);
    let timeout_seconds = crate::runtime_env::var("YUNOVA_MEDIA_TIMEOUT_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(7200)
        .clamp(60, 21_600);
    let output = tokio::time::timeout(Duration::from_secs(timeout_seconds), command.output())
        .await
        .map_err(|_| format!("{label} 超过 {timeout_seconds} 秒，已停止"))?
        .map_err(|error| format!("无法启动 {label}: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let detail: String = String::from_utf8_lossy(&output.stderr)
        .chars()
        .take(1000)
        .collect();
    Err(format!("{label} 失败: {detail}"))
}

fn ffmpeg_command() -> tokio::process::Command {
    tokio::process::Command::new(
        crate::runtime_env::var("YUNOVA_FFMPEG").unwrap_or_else(|_| "ffmpeg".into()),
    )
}

fn ffprobe_command() -> tokio::process::Command {
    tokio::process::Command::new(
        crate::runtime_env::var("YUNOVA_FFPROBE").unwrap_or_else(|_| "ffprobe".into()),
    )
}

async fn write_video_input(
    storage: &MediaStorage,
    path: &str,
    destination: &FsPath,
) -> Result<(), String> {
    let name = media_name(path, "/api/videos/")?;
    let size = storage
        .size(MediaKind::Video, &name)
        .await
        .map_err(|error| format!("读取输入视频信息失败: {error}"))?;
    if size > MAX_VIDEO_INPUT_BYTES as u64 {
        return Err("单个待处理视频不能超过 1 GiB".into());
    }
    let bytes = storage
        .get(MediaKind::Video, &name)
        .await
        .map_err(|error| format!("读取输入视频失败: {error}"))?;
    tokio::fs::write(destination, bytes)
        .await
        .map_err(|error| format!("写入临时视频失败: {error}"))
}

async fn persist_video_output(state: &AppState, source: &FsPath) -> Result<String, String> {
    let bytes = tokio::fs::read(source)
        .await
        .map_err(|error| format!("读取处理结果失败: {error}"))?;
    if bytes.is_empty() {
        return Err("媒体处理没有产生输出".into());
    }
    let name = format!("{}.mp4", random_hex(16));
    state
        .storage
        .put(MediaKind::Video, &name, bytes)
        .await
        .map_err(|error| format!("保存处理结果失败: {error}"))?;
    Ok(format!("/api/videos/{name}"))
}

async fn trim_video(
    state: &AppState,
    input_path: &str,
    start: f64,
    end: f64,
) -> Result<String, String> {
    let temp = workflow_temp_dir(state);
    tokio::fs::create_dir_all(&temp)
        .await
        .map_err(|error| format!("创建临时目录失败: {error}"))?;
    let input = temp.join("input.mp4");
    let output = temp.join("output.mp4");
    let result = async {
        write_video_input(&state.storage, input_path, &input).await?;
        let mut command = ffmpeg_command();
        command
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-y")
            .arg("-ss")
            .arg(format!("{start:.3}"))
            .arg("-i")
            .arg(&input)
            .arg("-t")
            .arg(format!("{:.3}", end - start))
            .arg("-map")
            .arg("0:v:0")
            .arg("-map")
            .arg("0:a?")
            .arg("-c:v")
            .arg("libx264")
            .arg("-preset")
            .arg("veryfast")
            .arg("-crf")
            .arg("20")
            .arg("-c:a")
            .arg("aac")
            .arg("-movflags")
            .arg("+faststart")
            .arg(&output);
        run_command(command, "FFmpeg 视频裁剪").await?;
        persist_video_output(state, &output).await
    }
    .await;
    let _ = tokio::fs::remove_dir_all(&temp).await;
    result
}

async fn video_has_audio(path: &FsPath) -> Result<bool, String> {
    let mut command = ffprobe_command();
    command
        .arg("-v")
        .arg("error")
        .arg("-select_streams")
        .arg("a:0")
        .arg("-show_entries")
        .arg("stream=index")
        .arg("-of")
        .arg("csv=p=0")
        .arg(path);
    command.kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(60), command.output())
        .await
        .map_err(|_| "FFprobe 读取视频超时")?
        .map_err(|error| format!("无法启动 FFprobe: {error}"))?;
    if !output.status.success() {
        return Err("FFprobe 无法读取视频".into());
    }
    Ok(!String::from_utf8_lossy(&output.stdout).trim().is_empty())
}

async fn normalize_video(
    input: &FsPath,
    output: &FsPath,
    width: i64,
    height: i64,
) -> Result<(), String> {
    let has_audio = video_has_audio(input).await?;
    let mut command = ffmpeg_command();
    command
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-y")
        .arg("-i")
        .arg(input);
    if !has_audio {
        command
            .arg("-f")
            .arg("lavfi")
            .arg("-i")
            .arg("anullsrc=channel_layout=stereo:sample_rate=48000");
    }
    let filter = format!(
        "scale={width}:{height}:force_original_aspect_ratio=decrease,pad={width}:{height}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps=30,format=yuv420p"
    );
    command
        .arg("-map")
        .arg("0:v:0")
        .arg("-map")
        .arg(if has_audio { "0:a:0" } else { "1:a:0" })
        .arg("-vf")
        .arg(filter);
    if has_audio {
        command.arg("-af").arg("apad");
    }
    command
        .arg("-c:v")
        .arg("libx264")
        .arg("-preset")
        .arg("veryfast")
        .arg("-crf")
        .arg("20")
        .arg("-c:a")
        .arg("aac")
        .arg("-ar")
        .arg("48000")
        .arg("-ac")
        .arg("2")
        .arg("-shortest")
        .arg("-movflags")
        .arg("+faststart")
        .arg(output);
    run_command(command, "FFmpeg 视频标准化").await
}

async fn merge_videos(
    state: &AppState,
    input_paths: &[String],
    width: i64,
    height: i64,
) -> Result<String, String> {
    if input_paths.len() < 2 {
        return Err("合并节点至少需要两个视频".into());
    }
    let temp = workflow_temp_dir(state);
    tokio::fs::create_dir_all(&temp)
        .await
        .map_err(|error| format!("创建临时目录失败: {error}"))?;
    let result = async {
        let mut normalized = Vec::new();
        for (index, path) in input_paths.iter().enumerate() {
            let input = temp.join(format!("input-{index}.mp4"));
            let output = temp.join(format!("normalized-{index}.mp4"));
            write_video_input(&state.storage, path, &input).await?;
            normalize_video(&input, &output, width, height).await?;
            normalized.push(output);
        }
        let concat_path = temp.join("concat.txt");
        let concat_body = normalized
            .iter()
            .enumerate()
            .map(|(index, _)| format!("file 'normalized-{index}.mp4'"))
            .collect::<Vec<_>>()
            .join("\n");
        tokio::fs::write(&concat_path, concat_body)
            .await
            .map_err(|error| format!("写入合并清单失败: {error}"))?;
        let output = temp.join("merged.mp4");
        let mut command = ffmpeg_command();
        command
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-y")
            .arg("-f")
            .arg("concat")
            .arg("-safe")
            .arg("0")
            .arg("-i")
            .arg(&concat_path)
            .arg("-c")
            .arg("copy")
            .arg("-movflags")
            .arg("+faststart")
            .arg(&output);
        run_command(command, "FFmpeg 视频合并").await?;
        persist_video_output(state, &output).await
    }
    .await;
    let _ = tokio::fs::remove_dir_all(&temp).await;
    result
}

async fn cancel_run(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(token): Path<String>,
) -> Response {
    let Some(run) = fetch_run_by_token(&installed.pool, installed.kind, &token).await else {
        return err(StatusCode::NOT_FOUND, "运行记录不存在");
    };
    if run.user_id != user.id {
        return err(StatusCode::NOT_FOUND, "运行记录不存在");
    }
    let now = db::now_expr(installed.kind);
    let run_sql = db::q(
        installed.kind,
        &format!(
            "UPDATE workflow_runs SET status = 'cancelled', error = '用户已停止后续节点', \
             finished_at = {now} WHERE id = ? AND status = 'running'"
        ),
    );
    let node_sql = db::q(
        installed.kind,
        &format!(
            "UPDATE workflow_node_runs SET status = 'cancelled', error = '用户已停止后续节点', \
             finished_at = {now} WHERE run_id = ? AND status IN ('waiting', 'starting')"
        ),
    );
    let changed = sqlx::query(&run_sql)
        .bind(run.id)
        .execute(&installed.pool)
        .await
        .map(|result| result.rows_affected())
        .unwrap_or(0);
    let _ = sqlx::query(&node_sql)
        .bind(run.id)
        .execute(&installed.pool)
        .await;
    if changed > 0 {
        append_run_log(
            &installed.pool,
            installed.kind,
            run.id,
            None,
            "warning",
            "用户已停止后续节点",
        )
        .await;
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn retry_node(
    State(state): State<AppState>,
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path((token, node_key)): Path<(String, String)>,
) -> Response {
    let Some(run) = fetch_run_by_token(&installed.pool, installed.kind, &token).await else {
        return err(StatusCode::NOT_FOUND, "运行记录不存在");
    };
    if run.user_id != user.id {
        return err(StatusCode::NOT_FOUND, "运行记录不存在");
    }
    let graph: WorkflowGraph = match serde_json::from_str(&run.graph_json) {
        Ok(graph) => graph,
        Err(error) => return err(StatusCode::BAD_REQUEST, error.to_string()),
    };
    if !graph.nodes.iter().any(|node| node.id == node_key) {
        return err(StatusCode::NOT_FOUND, "节点不存在");
    }
    let mut reset = HashSet::from([node_key.clone()]);
    loop {
        let before = reset.len();
        for edge in &graph.edges {
            if reset.contains(&edge.source) {
                reset.insert(edge.target.clone());
            }
        }
        if reset.len() == before {
            break;
        }
    }
    let reset_sql = db::q(
        installed.kind,
        "UPDATE workflow_node_runs SET status = 'waiting', job_token = NULL, output_paths = NULL, \
         error = NULL, started_at = NULL, finished_at = NULL WHERE run_id = ? AND node_key = ? \
         AND status IN ('failed', 'blocked', 'cancelled', 'completed')",
    );
    let mut reset_count = 0_u64;
    for key in &reset {
        reset_count += sqlx::query(&reset_sql)
            .bind(run.id)
            .bind(key)
            .execute(&installed.pool)
            .await
            .map(|result| result.rows_affected())
            .unwrap_or(0);
    }
    let run_sql = db::q(
        installed.kind,
        "UPDATE workflow_runs SET status = 'running', error = NULL, finished_at = NULL WHERE id = ?",
    );
    if let Err(error) = sqlx::query(&run_sql)
        .bind(run.id)
        .execute(&installed.pool)
        .await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    append_run_log(
        &installed.pool,
        installed.kind,
        run.id,
        Some(&node_key),
        "info",
        &format!("从此节点重新运行，共重置 {reset_count} 个节点"),
    )
    .await;
    spawn_driver(state, run.id);
    StatusCode::NO_CONTENT.into_response()
}

/// Recover persistent runs after a server restart. Generated video jobs can be
/// resumed; local FFmpeg operations are safe to rerun. A node interrupted while
/// creating a billable child job is failed for an explicit user retry.
pub async fn recover(pool: &db::Pool, kind: db::DbKind) {
    let recoverable_sql = db::q(
        kind,
        "SELECT run_id, node_key FROM workflow_node_runs WHERE status = 'running' \
         AND node_type IN ('video_trim', 'video_merge')",
    );
    let recoverable: Vec<(i64, String)> = sqlx::query_as(&recoverable_sql)
        .fetch_all(pool)
        .await
        .unwrap_or_default();
    let sql = db::q(
        kind,
        "UPDATE workflow_node_runs SET status = 'waiting', error = NULL, started_at = NULL \
         WHERE status = 'running' AND node_type IN ('video_trim', 'video_merge')",
    );
    let _ = sqlx::query(&sql).execute(pool).await;
    for (run_id, node_key) in recoverable {
        append_run_log(
            pool,
            kind,
            run_id,
            Some(&node_key),
            "warning",
            "服务器重启，媒体处理已重新排队",
        )
        .await;
    }

    let interrupted_sql = db::q(
        kind,
        "SELECT run_id, node_key FROM workflow_node_runs WHERE status = 'starting'",
    );
    let interrupted: Vec<(i64, String)> = sqlx::query_as(&interrupted_sql)
        .fetch_all(pool)
        .await
        .unwrap_or_default();
    let now = db::now_expr(kind);
    let sql = db::q(
        kind,
        &format!(
            "UPDATE workflow_node_runs SET status = 'failed', error = '服务器在创建任务时重启，请重试此节点', \
             finished_at = {now} WHERE status = 'starting'"
        ),
    );
    let _ = sqlx::query(&sql).execute(pool).await;
    for (run_id, node_key) in interrupted {
        append_run_log(
            pool,
            kind,
            run_id,
            Some(&node_key),
            "error",
            "服务器在创建任务时重启，请重试此节点",
        )
        .await;
    }
}

pub async fn sweep(state: &AppState, installed: &InstalledState) {
    let sql = db::q(
        installed.kind,
        "SELECT id FROM workflow_runs WHERE status = 'running' ORDER BY created_at LIMIT 20",
    );
    let rows: Vec<(i64,)> = sqlx::query_as(&sql)
        .fetch_all(&installed.pool)
        .await
        .unwrap_or_default();
    for (run_id,) in rows {
        drive_run_once(state, installed, run_id).await;
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/workflows", get(list_workflows).post(save_workflow))
        .route("/workflows/{id}", delete(delete_workflow))
        .route("/workflows/{id}/runs", post(create_run))
        .route("/workflow-runs", get(list_runs))
        .route("/workflow-runs/{token}", get(get_run))
        .route("/workflow-runs/{token}/cancel", post(cancel_run))
        .route(
            "/workflow-runs/{token}/nodes/{node_key}/retry",
            post(retry_node),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, node_type: &str) -> WorkflowNode {
        WorkflowNode {
            id: id.into(),
            node_type: node_type.into(),
            x: 0.0,
            y: 0.0,
            data: match node_type {
                "image_generation" => json!({ "model": "image-model", "prompt": "cat" }),
                "video_generation" => {
                    json!({ "model": "video-model", "prompt": "move", "seconds": 5, "size": "1280x720" })
                }
                "video_trim" => json!({ "start": 0.0, "end": 2.0 }),
                "video_merge" => json!({ "width": 1280, "height": 720 }),
                _ => json!({}),
            },
        }
    }

    fn edge(id: &str, source: &str, target: &str) -> WorkflowEdge {
        WorkflowEdge {
            id: id.into(),
            source: source.into(),
            target: target.into(),
            priority: DEFAULT_EDGE_PRIORITY,
        }
    }

    fn prioritized_edge(id: &str, source: &str, target: &str, priority: i64) -> WorkflowEdge {
        WorkflowEdge {
            priority,
            ..edge(id, source, target)
        }
    }

    #[test]
    fn validates_multi_video_merge_graph() {
        let graph = WorkflowGraph {
            version: 1,
            nodes: vec![
                node("image", "image_generation"),
                node("video-a", "video_generation"),
                node("video-b", "video_generation"),
                node("trim", "video_trim"),
                node("merge", "video_merge"),
            ],
            edges: vec![
                edge("e1", "image", "video-a"),
                edge("e2", "image", "video-b"),
                edge("e3", "video-a", "trim"),
                edge("e4", "trim", "merge"),
                edge("e5", "video-b", "merge"),
            ],
        };
        assert_eq!(validate_graph(&graph, true), Ok(()));
    }

    #[test]
    fn orders_merge_inputs_by_priority_then_connection_order() {
        let graph = WorkflowGraph {
            version: 1,
            nodes: vec![
                node("video-a", "video_generation"),
                node("video-b", "video_generation"),
                node("video-c", "video_generation"),
                node("merge", "video_merge"),
            ],
            edges: vec![
                prioritized_edge("e1", "video-a", "merge", 20),
                prioritized_edge("e2", "video-b", "merge", 10),
                prioritized_edge("e3", "video-c", "merge", 10),
            ],
        };

        let sources: Vec<_> = input_edges(&graph, "merge", true)
            .into_iter()
            .map(|edge| edge.source.as_str())
            .collect();
        assert_eq!(sources, vec!["video-b", "video-c", "video-a"]);
    }

    #[test]
    fn defaults_legacy_edge_priority_without_changing_order() {
        let edge: WorkflowEdge = serde_json::from_value(json!({
            "id": "legacy-edge",
            "source": "video-a",
            "target": "merge"
        }))
        .unwrap();
        assert_eq!(edge.priority, DEFAULT_EDGE_PRIORITY);
    }

    #[test]
    fn rejects_cycles_and_wrong_media_types() {
        let cycle = WorkflowGraph {
            version: 1,
            nodes: vec![node("a", "video_trim"), node("b", "video_trim")],
            edges: vec![edge("e1", "a", "b"), edge("e2", "b", "a")],
        };
        assert!(validate_graph(&cycle, false).unwrap_err().contains("循环"));

        let wrong_type = WorkflowGraph {
            version: 1,
            nodes: vec![
                node("video", "video_generation"),
                node("image", "image_generation"),
            ],
            edges: vec![edge("e1", "video", "image")],
        };
        assert!(
            validate_graph(&wrong_type, false)
                .unwrap_err()
                .contains("只能接收图片")
        );
    }

    #[test]
    fn allows_incomplete_drafts_but_not_incomplete_runs() {
        let graph = WorkflowGraph {
            version: 1,
            nodes: vec![node("trim", "video_trim"), node("merge", "video_merge")],
            edges: vec![],
        };
        assert_eq!(validate_graph(&graph, false), Ok(()));
        assert!(validate_graph(&graph, true).is_err());
    }

    #[test]
    fn validates_media_paths() {
        assert_eq!(
            media_name("/api/videos/a.mp4", "/api/videos/"),
            Ok("a.mp4".into())
        );
        assert!(media_name("/api/videos/../secret", "/api/videos/").is_err());
        assert!(media_name("https://example.com/a.mp4", "/api/videos/").is_err());
    }

    #[tokio::test]
    async fn sqlite_migration_creates_workflow_tables() {
        crate::db::install_drivers();
        let path = std::env::temp_dir().join(format!(
            "yunova-workflow-migration-{}-{}.db",
            std::process::id(),
            random_hex(6)
        ));
        let pool = crate::db::connect(&format!("sqlite:{}", path.display()))
            .await
            .unwrap();
        crate::db::migrate(&pool, crate::db::DbKind::Sqlite)
            .await
            .unwrap();
        let (migration,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM _migrations WHERE id = 34")
            .fetch_one(&pool)
            .await
            .unwrap();
        let (tables,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' \
             AND name IN ('workflows', 'workflow_runs', 'workflow_node_runs', 'media_assets', \
             'workflow_run_logs')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(migration, 1);
        assert_eq!(tables, 5);
        sqlx::query("INSERT INTO users (username, password_hash) VALUES ('log-test', 'test')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO workflow_runs (token, user_id, name, graph_json, status) \
             VALUES ('log-run', 1, 'test', '{}', 'running')",
        )
        .execute(&pool)
        .await
        .unwrap();
        append_run_log(
            &pool,
            crate::db::DbKind::Sqlite,
            1,
            Some("node-a"),
            "info",
            "开始执行",
        )
        .await;
        let logs = fetch_run_logs(&pool, crate::db::DbKind::Sqlite, 1).await;
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].node_key.as_deref(), Some("node-a"));
        assert_eq!(logs[0].message, "开始执行");
        pool.close().await;
        let _ = tokio::fs::remove_file(path).await;
    }
}
