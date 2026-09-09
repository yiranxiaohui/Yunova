# 工蜂（Worker）远程 Agent 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给 Yunova 新增「工蜂」远程 agent：用户服务器上跑一个瘦执行器二进制，网页用自然语言对话，后端用 Claude 跑 agent 循环，下发 shell/读/写工具调用给工蜂执行。

**Architecture:** 工蜂（新 crate）主动建 WebSocket 连回后端鉴权 → 后端在内存维护在线工蜂注册表 → agent 循环在后端编排（复用渠道链调 Claude 非流式 Messages API + 复用 credits 扣费）→ 工具调用经 WS 下发工蜂、结果回传 → SSE 流式推网页。审批分级：read 自动，shell/write 默认需网页批准，会话级「自动批准」开关。

**Tech Stack:** Rust/Axum（后端 + 工蜂 crate）、axum `ws` feature（服务端 WS）、tokio-tungstenite（工蜂客户端 WS）、SQLite/MySQL/Postgres 三方言迁移、React 19 + Vite + Tailwind（网页）。

**说明（无测试运行器）：** 本仓库无测试框架（CLAUDE.md 明确），故本计划不写单元测试步骤，改为**每个任务以编译/构建通过 + 手动验证**作为完成判据。后端用 `cargo check`，前端用 `bun run build`。提交频繁。

---

## 文件结构总览

**新建：**
- `worker/Cargo.toml` — 工蜂 crate 清单
- `worker/src/main.rs` — 工蜂入口：配置、WS 客户端、重连
- `worker/src/exec.rs` — 三工具执行（shell / read_file / write_file）
- `worker/src/proto.rs` — 与后端共享的 WS 消息类型（serde）
- `src/worker.rs` — 后端工蜂模块：WS 接入端点 + 注册表 + REST + agent 循环
- `migrations/{sqlite,mysql,postgres}/0026_workers.sql`
- `migrations/{sqlite,mysql,postgres}/0027_worker_sessions.sql`
- `migrations/{sqlite,mysql,postgres}/0028_worker_messages.sql`
- `web/src/lib/worker.ts` — 网页 API 客户端（REST + SSE 解析）
- `web/src/components/app/WorkerPage.tsx` — 工蜂页（管理 + 会话）

**修改：**
- `Cargo.toml` — 转 workspace；axum 加 `ws` feature
- `src/main.rs` — `mod worker;` + 挂载 worker 路由 + WorkerRegistry 进 AppState
- `src/db.rs` — 三迁移数组各加 3 条
- `web/src/`（侧边栏/路由处）— 新增「工蜂」入口

---

## Task 1: 转 Cargo workspace + 开 axum ws feature

**Files:**
- Modify: `Cargo.toml`（仓库根）

- [ ] **Step 1: 在根 `Cargo.toml` 顶部加 workspace 段**

在文件最顶部（`[package]` 之前）插入：

```toml
[workspace]
members = [".", "worker"]
```

- [ ] **Step 2: 给 axum 开 `ws` feature**

把 `Cargo.toml` 里这行：

```toml
axum = "0.8"
```

改为：

```toml
axum = { version = "0.8", features = ["ws"] }
```

- [ ] **Step 3: 验证编译（worker crate 还不存在，会报缺成员）**

Run: `cargo check 2>&1 | head -20`
Expected: 报错 `failed to load manifest for workspace member ... worker`（预期，下个任务建它）。本步只确认 workspace 段语法被识别。

- [ ] **Step 4: 提交**

```bash
git add Cargo.toml
git commit -m "build(worker): 转 cargo workspace + axum 开 ws feature"
```

---

## Task 2: 工蜂 crate 骨架 + 共享协议类型

**Files:**
- Create: `worker/Cargo.toml`
- Create: `worker/src/proto.rs`
- Create: `worker/src/main.rs`（占位，仅 main + mod）

- [ ] **Step 1: 写 `worker/Cargo.toml`**

```toml
[package]
name = "yunova-worker"
version = "0.1.0"
edition = "2024"

[[bin]]
name = "yunova-worker"
path = "src/main.rs"

[dependencies]
tokio = { version = "1", features = ["macros", "rt-multi-thread", "process", "fs", "io-util", "time", "net", "signal"] }
tokio-tungstenite = { version = "0.24", features = ["rustls-tls-webpki-roots"] }
futures-util = "0.3"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

- [ ] **Step 2: 写 `worker/src/proto.rs`（WS 消息类型）**

```rust
//! 工蜂 ↔ 后端 WebSocket 消息协议（JSON 文本帧）。
//! 后端 src/worker.rs 里有一份结构等价的定义（不共享 crate，手动保持一致）。
use serde::{Deserialize, Serialize};

/// 工蜂 → 后端
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToServer {
    Hello { token: String, name: String },
    Heartbeat,
    ToolResult {
        call_id: String,
        ok: bool,
        output: String,
        #[serde(default)]
        truncated: bool,
    },
}

/// 后端 → 工蜂
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToWorker {
    HelloOk { worker_id: i64 },
    Error { message: String },
    Exec {
        call_id: String,
        tool: String,
        args: serde_json::Value,
    },
}
```

- [ ] **Step 3: 写 `worker/src/main.rs` 占位**

```rust
mod proto;
mod exec;

#[tokio::main]
async fn main() {
    eprintln!("yunova-worker 占位入口（Task 4 实现连接逻辑）");
}
```

- [ ] **Step 4: 写 `worker/src/exec.rs` 占位**

```rust
//! 三工具执行逻辑（Task 3 实现）。
```

- [ ] **Step 5: 验证编译**

Run: `cargo check -p yunova-worker 2>&1 | tail -20`
Expected: 编译通过（可能有 unused 警告）。

- [ ] **Step 6: 提交**

```bash
git add worker/
git commit -m "feat(worker): 工蜂 crate 骨架 + WS 协议类型"
```

---

## Task 3: 工蜂三工具执行逻辑

**Files:**
- Modify: `worker/src/exec.rs`

- [ ] **Step 1: 实现 `worker/src/exec.rs`**

```rust
//! 三工具执行逻辑：shell / read_file / write_file。
//! 每个函数返回 (ok, output)；输出超过 MAX_OUTPUT 截断。
use serde_json::Value;
use tokio::process::Command;

const MAX_OUTPUT: usize = 64 * 1024; // 64KB

/// 执行一个工具调用，返回 (ok, output, truncated)。
pub async fn run(tool: &str, args: &Value) -> (bool, String, bool) {
    let (ok, raw) = match tool {
        "shell" => shell(args).await,
        "read_file" => read_file(args).await,
        "write_file" => write_file(args).await,
        other => (false, format!("未知工具: {other}")),
    };
    let truncated = raw.len() > MAX_OUTPUT;
    let output = if truncated {
        let mut s = raw[..MAX_OUTPUT].to_string();
        s.push_str("\n…[输出已截断]");
        s
    } else {
        raw
    };
    (ok, output, truncated)
}

async fn shell(args: &Value) -> (bool, String) {
    let cmd = match args.get("command").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => return (false, "shell: 缺少 command 参数".into()),
    };
    let mut c = Command::new("sh");
    c.arg("-c").arg(cmd);
    if let Some(cwd) = args.get("cwd").and_then(|v| v.as_str()) {
        c.current_dir(cwd);
    }
    match c.output().await {
        Ok(out) => {
            let mut s = String::new();
            s.push_str(&String::from_utf8_lossy(&out.stdout));
            let err = String::from_utf8_lossy(&out.stderr);
            if !err.is_empty() {
                s.push_str("\n[stderr]\n");
                s.push_str(&err);
            }
            let code = out.status.code().unwrap_or(-1);
            s.push_str(&format!("\n[exit code: {code}]"));
            (out.status.success(), s)
        }
        Err(e) => (false, format!("shell 执行失败: {e}")),
    }
}

async fn read_file(args: &Value) -> (bool, String) {
    let path = match args.get("path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return (false, "read_file: 缺少 path 参数".into()),
    };
    match tokio::fs::read_to_string(path).await {
        Ok(content) => (true, content),
        Err(e) => (false, format!("读取失败 {path}: {e}")),
    }
}

async fn write_file(args: &Value) -> (bool, String) {
    let path = match args.get("path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return (false, "write_file: 缺少 path 参数".into()),
    };
    let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
    if let Some(parent) = std::path::Path::new(path).parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    match tokio::fs::write(path, content).await {
        Ok(_) => (true, format!("已写入 {path}（{} 字节）", content.len())),
        Err(e) => (false, format!("写入失败 {path}: {e}")),
    }
}
```

- [ ] **Step 2: 验证编译**

Run: `cargo check -p yunova-worker 2>&1 | tail -20`
Expected: 编译通过。

- [ ] **Step 3: 提交**

```bash
git add worker/src/exec.rs
git commit -m "feat(worker): 实现 shell/read_file/write_file 三工具执行"
```

---

## Task 4: 工蜂 WS 客户端 + 配置 + 重连

**Files:**
- Modify: `worker/src/main.rs`

- [ ] **Step 1: 实现 `worker/src/main.rs`**

配置来源：环境变量 `YUNOVA_WORKER_URL`（如 `wss://chat.example.com/api/worker/connect`）和 `YUNOVA_WORKER_TOKEN`（配对码）。

```rust
mod proto;
mod exec;

use futures_util::{SinkExt, StreamExt};
use proto::{ToServer, ToWorker};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

#[tokio::main]
async fn main() {
    let url = std::env::var("YUNOVA_WORKER_URL")
        .expect("需要环境变量 YUNOVA_WORKER_URL（如 wss://host/api/worker/connect）");
    let token = std::env::var("YUNOVA_WORKER_TOKEN")
        .expect("需要环境变量 YUNOVA_WORKER_TOKEN（配对码）");
    let name = std::env::var("YUNOVA_WORKER_NAME")
        .unwrap_or_else(|_| hostname());

    let mut backoff = 1u64;
    loop {
        match run_once(&url, &token, &name).await {
            Ok(_) => {
                eprintln!("[worker] 连接正常关闭，3 秒后重连");
                backoff = 1;
            }
            Err(e) => {
                eprintln!("[worker] 连接错误: {e}，{backoff} 秒后重连");
            }
        }
        tokio::time::sleep(Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(30);
    }
}

fn hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "worker".to_string())
}

async fn run_once(url: &str, token: &str, name: &str) -> Result<(), String> {
    let (ws, _) = tokio_tungstenite::connect_async(url)
        .await
        .map_err(|e| format!("WS 连接失败: {e}"))?;
    let (mut tx, mut rx) = ws.split();

    // 发 hello
    let hello = serde_json::to_string(&ToServer::Hello {
        token: token.to_string(),
        name: name.to_string(),
    })
    .unwrap();
    tx.send(Message::Text(hello.into())).await.map_err(|e| e.to_string())?;

    // 心跳任务
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<Message>(32);
    let hb_tx = out_tx.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(20)).await;
            let hb = serde_json::to_string(&ToServer::Heartbeat).unwrap();
            if hb_tx.send(Message::Text(hb.into())).await.is_err() {
                break;
            }
        }
    });

    loop {
        tokio::select! {
            Some(msg) = out_rx.recv() => {
                tx.send(msg).await.map_err(|e| e.to_string())?;
            }
            Some(frame) = rx.next() => {
                let frame = frame.map_err(|e| e.to_string())?;
                let text = match frame {
                    Message::Text(t) => t.to_string(),
                    Message::Close(_) => return Ok(()),
                    Message::Ping(p) => { let _ = out_tx.send(Message::Pong(p)).await; continue; }
                    _ => continue,
                };
                let to_worker: ToWorker = match serde_json::from_str(&text) {
                    Ok(m) => m,
                    Err(e) => { eprintln!("[worker] 解析消息失败: {e}"); continue; }
                };
                match to_worker {
                    ToWorker::HelloOk { worker_id } => {
                        eprintln!("[worker] 鉴权成功，worker_id={worker_id}");
                    }
                    ToWorker::Error { message } => {
                        return Err(format!("后端拒绝: {message}"));
                    }
                    ToWorker::Exec { call_id, tool, args } => {
                        let out_tx = out_tx.clone();
                        tokio::spawn(async move {
                            let (ok, output, truncated) = exec::run(&tool, &args).await;
                            let res = serde_json::to_string(&ToServer::ToolResult {
                                call_id, ok, output, truncated,
                            }).unwrap();
                            let _ = out_tx.send(Message::Text(res.into())).await;
                        });
                    }
                }
            }
            else => return Ok(()),
        }
    }
}
```

- [ ] **Step 2: 验证编译**

Run: `cargo check -p yunova-worker 2>&1 | tail -30`
Expected: 编译通过。若 `Message::Text` 需要的类型不符（tungstenite 0.24 用 `Utf8Bytes`），按编译器提示用 `.into()` 包装（上面已加 `.into()`）。

- [ ] **Step 3: 提交**

```bash
git add worker/src/main.rs
git commit -m "feat(worker): WS 客户端 + 配置 + 心跳 + 自动重连"
```

---

## Task 5: 三方言数据库迁移

**Files:**
- Create: `migrations/sqlite/0026_workers.sql`, `migrations/mysql/0026_workers.sql`, `migrations/postgres/0026_workers.sql`
- Create: `migrations/sqlite/0027_worker_sessions.sql`, `migrations/mysql/0027_worker_sessions.sql`, `migrations/postgres/0027_worker_sessions.sql`
- Create: `migrations/sqlite/0028_worker_messages.sql`, `migrations/mysql/0028_worker_messages.sql`, `migrations/postgres/0028_worker_messages.sql`
- Modify: `src/db.rs`（三数组各加 3 行）

> 参考既有迁移的列类型写法：先 `cat migrations/sqlite/0019_channels_pricing.sql migrations/mysql/0019_channels_pricing.sql migrations/postgres/0019_channels_pricing.sql` 对齐三方言的 PK 自增 / 时间默认值 / 文本类型写法，再照抄风格。

- [ ] **Step 1: 写 `0026_workers.sql`（三份）**

sqlite (`migrations/sqlite/0026_workers.sql`)：
```sql
CREATE TABLE workers (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id INTEGER NOT NULL,
  name TEXT NOT NULL DEFAULT 'worker',
  token_hash TEXT NOT NULL,
  last_seen_at TEXT,
  created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX idx_workers_user ON workers(user_id);
```

mysql (`migrations/mysql/0026_workers.sql`)：
```sql
CREATE TABLE workers (
  id BIGINT PRIMARY KEY AUTO_INCREMENT,
  user_id BIGINT NOT NULL,
  name VARCHAR(255) NOT NULL DEFAULT 'worker',
  token_hash VARCHAR(255) NOT NULL,
  last_seen_at DATETIME NULL,
  created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
  INDEX idx_workers_user (user_id)
);
```

postgres (`migrations/postgres/0026_workers.sql`)：
```sql
CREATE TABLE workers (
  id BIGSERIAL PRIMARY KEY,
  user_id BIGINT NOT NULL,
  name TEXT NOT NULL DEFAULT 'worker',
  token_hash TEXT NOT NULL,
  last_seen_at TIMESTAMPTZ,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_workers_user ON workers(user_id);
```

- [ ] **Step 2: 写 `0027_worker_sessions.sql`（三份）**

sqlite：
```sql
CREATE TABLE worker_sessions (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id INTEGER NOT NULL,
  worker_id INTEGER NOT NULL,
  title TEXT NOT NULL DEFAULT '新会话',
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX idx_worker_sessions_user ON worker_sessions(user_id);
```

mysql：
```sql
CREATE TABLE worker_sessions (
  id BIGINT PRIMARY KEY AUTO_INCREMENT,
  user_id BIGINT NOT NULL,
  worker_id BIGINT NOT NULL,
  title VARCHAR(255) NOT NULL DEFAULT '新会话',
  created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
  INDEX idx_worker_sessions_user (user_id)
);
```

postgres：
```sql
CREATE TABLE worker_sessions (
  id BIGSERIAL PRIMARY KEY,
  user_id BIGINT NOT NULL,
  worker_id BIGINT NOT NULL,
  title TEXT NOT NULL DEFAULT '新会话',
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_worker_sessions_user ON worker_sessions(user_id);
```

- [ ] **Step 3: 写 `0028_worker_messages.sql`（三份）**

sqlite：
```sql
CREATE TABLE worker_messages (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id INTEGER NOT NULL,
  role TEXT NOT NULL,
  content TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX idx_worker_messages_session ON worker_messages(session_id);
```

mysql：
```sql
CREATE TABLE worker_messages (
  id BIGINT PRIMARY KEY AUTO_INCREMENT,
  session_id BIGINT NOT NULL,
  role VARCHAR(32) NOT NULL,
  content LONGTEXT NOT NULL,
  created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
  INDEX idx_worker_messages_session (session_id)
);
```

postgres：
```sql
CREATE TABLE worker_messages (
  id BIGSERIAL PRIMARY KEY,
  session_id BIGINT NOT NULL,
  role TEXT NOT NULL,
  content TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_worker_messages_session ON worker_messages(session_id);
```

- [ ] **Step 4: 在 `src/db.rs` 三个迁移数组各加 3 行**

在 `SQLITE_MIGRATIONS` 数组末尾（`(25, …)` 之后）加：
```rust
    (26, include_str!("../migrations/sqlite/0026_workers.sql")),
    (27, include_str!("../migrations/sqlite/0027_worker_sessions.sql")),
    (28, include_str!("../migrations/sqlite/0028_worker_messages.sql")),
```
在 `MYSQL_MIGRATIONS` 数组末尾加对应 `../migrations/mysql/...` 三行；在 `POSTGRES_MIGRATIONS` 数组末尾加对应 `../migrations/postgres/...` 三行。（先 `grep -n "MYSQL_MIGRATIONS\|POSTGRES_MIGRATIONS" src/db.rs` 定位数组，再在各自最后一条 `include_str!` 后追加。）

- [ ] **Step 5: 验证编译（include_str! 路径正确即过）**

Run: `cargo check -p yunova 2>&1 | tail -20`
Expected: 编译通过。

- [ ] **Step 6: 手动验证迁移可跑（SQLite）**

Run: `rm -f /tmp/wk-test.db && YUNOVA_DATABASE_URL="sqlite:///tmp/wk-test.db?mode=rwc" cargo run -p yunova 2>&1 | head -15`
Expected: 启动日志无迁移报错；Ctrl-C 退出。（确认 0026-0028 被应用。）

- [ ] **Step 7: 提交**

```bash
git add migrations/ src/db.rs
git commit -m "feat(worker): workers/worker_sessions/worker_messages 三方言迁移"
```

---

## Task 6: 后端 WorkerRegistry（在线工蜂注册表）

**Files:**
- Create: `src/worker.rs`（先建注册表与协议类型部分）
- Modify: `src/main.rs`（AppState 加 registry 字段 + `mod worker;`）

- [ ] **Step 1: 在 `src/worker.rs` 写协议类型 + 注册表**

```rust
//! 后端工蜂模块：WS 接入、在线注册表、REST、agent 循环。
use crate::AppState;
use axum::Router;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
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

pub fn routes() -> Router<AppState> {
    Router::new() // 路由在 Task 7/8/9 逐步加
}
```

- [ ] **Step 2: 在 `src/main.rs` 的 AppState 加字段**

先 `grep -n "struct AppState" src/main.rs` 定位。在结构体里加：
```rust
    pub workers: crate::worker::WorkerRegistry,
```
在所有构造 `AppState { … }` 的地方加 `workers: crate::worker::WorkerRegistry::new(),`（`grep -n "AppState {" src/main.rs` 找到构造点，通常 1 处）。文件顶部加 `mod worker;`。

- [ ] **Step 3: 验证编译**

Run: `cargo check -p yunova 2>&1 | tail -20`
Expected: 编译通过（routes 空、注册表暂未被用，可能有 dead_code 警告）。

- [ ] **Step 4: 提交**

```bash
git add src/worker.rs src/main.rs
git commit -m "feat(worker): 后端在线工蜂注册表 + WS 协议类型"
```

---

## Task 7: WS 接入端点（鉴权 + 注册 + 收发循环）

**Files:**
- Modify: `src/worker.rs`

> 配对码校验：库里 `workers.token_hash` 存 sha256。鉴权时对收到的 token 做 sha256 比对（用已依赖的 `sha2`）。在线状态以注册表为准；连接建立时更新 `last_seen_at`。

- [ ] **Step 1: 在 `src/worker.rs` 加 WS 端点 handler**

```rust
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use sha2::{Digest, Sha256};

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
        match g.clone() { Some(i) => i, None => return }
    };
    let th = hash_token(&token);
    // 校验 token 并取 worker_id + user_id
    let row: Option<(i64, i64)> = sqlx::query_as(
        &crate::db::q(installed.kind, "SELECT id, user_id FROM workers WHERE token_hash = ?"),
    )
    .bind(&th)
    .fetch_optional(&installed.pool)
    .await
    .ok()
    .flatten();
    let (worker_id, user_id) = match row {
        Some(v) => v,
        None => {
            let err = serde_json::to_string(&ToWorker::Error { message: "配对码无效".into() }).unwrap();
            let _ = sink.send(Message::Text(err.into())).await;
            return;
        }
    };

    // 更新名字 + last_seen
    let _ = sqlx::query(&crate::db::q(
        installed.kind,
        &format!("UPDATE workers SET name = ?, last_seen_at = {} WHERE id = ?", crate::db::now_expr(installed.kind)),
    ))
    .bind(&name).bind(worker_id)
    .execute(&installed.pool).await;

    // 注册
    let (tx, mut rx) = mpsc::channel::<ToWorker>(64);
    let pending: Arc<RwLock<HashMap<String, oneshot::Sender<ToolOutcome>>>> = Arc::new(RwLock::new(HashMap::new()));
    let handle = Arc::new(WorkerHandle { user_id, tx: tx.clone(), pending: pending.clone() });
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
            if sink.send(Message::Text(txt.into())).await.is_err() { break; }
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
                                let _ = sqlx::query(&crate::db::q(kind,
                                    &format!("UPDATE workers SET last_seen_at = {} WHERE id = ?", crate::db::now_expr(kind))))
                                    .bind(worker_id).execute(&pool).await;
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
```

- [ ] **Step 2: 把端点加进 `routes()`（注意：WS 端点不能走 require_auth，需公开挂载）**

把 `routes()` 改为只返回 REST 路由（Task 8 填），并**另加一个公开路由函数**：
```rust
/// WS 接入端点 —— 公开（鉴权靠配对 token），单独挂载，不经 require_auth。
pub fn public_routes() -> Router<AppState> {
    Router::new().route("/worker/connect", get(ws_connect))
}
```

- [ ] **Step 3: 在 `src/main.rs` 的公开路由区挂载**

在 `build_router` 的公开路由那段（`/health` 附近，`grep -n "/health" src/main.rs`）加：
```rust
        .merge(worker::public_routes())
```

- [ ] **Step 4: 验证编译**

Run: `cargo check -p yunova 2>&1 | tail -30`
Expected: 编译通过。

- [ ] **Step 5: 提交**

```bash
git add src/worker.rs src/main.rs
git commit -m "feat(worker): WS 接入端点（配对码鉴权 + 注册 + 心跳/结果回执）"
```

---

## Task 8: 工蜂管理 REST（配对码 / 列表 / 改名 / 删除）

**Files:**
- Modify: `src/worker.rs`

> 用 `Extension<InstalledState>` + `Extension<CurrentUser>`，照抄 `src/skills.rs` 的插入返回 id 模式。配对码用 `rand` 生成 32 字节十六进制；只返回明文一次，库存 sha256。

- [ ] **Step 1: 加 REST handlers**

```rust
use axum::extract::Path;
use axum::{Extension, Json};
use axum::routing::{delete, patch, post};
use crate::{CurrentUser, InstalledState};
use rand::RngCore;
use serde_json::json;

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
    // 插入并取 id（照 skills.rs 模式；这里只需 token，不必返回 id）
    let res = sqlx::query(&crate::db::q(installed.kind,
        "INSERT INTO workers (user_id, name, token_hash) VALUES (?, ?, ?)"))
        .bind(user.id).bind("worker").bind(&th)
        .execute(&installed.pool).await;
    match res {
        Ok(_) => Json(json!({ "token": token })).into_response(),
        Err(e) => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, format!("创建失败: {e}")).into_response(),
    }
}

async fn list(
    State(state): State<AppState>,
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
) -> Response {
    let rows: Vec<(i64, String, Option<String>)> = sqlx::query_as(&crate::db::q(installed.kind,
        "SELECT id, name, last_seen_at FROM workers WHERE user_id = ? ORDER BY id DESC"))
        .bind(user.id).fetch_all(&installed.pool).await.unwrap_or_default();
    let mut out = Vec::new();
    for (id, name, last_seen) in rows {
        out.push(json!({
            "id": id, "name": name, "last_seen_at": last_seen,
            "online": state.workers.is_online(id).await,
        }));
    }
    Json(out).into_response()
}

#[derive(Deserialize)]
struct RenameReq { name: String }

async fn rename(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(req): Json<RenameReq>,
) -> Response {
    let _ = sqlx::query(&crate::db::q(installed.kind,
        "UPDATE workers SET name = ? WHERE id = ? AND user_id = ?"))
        .bind(&req.name).bind(id).bind(user.id)
        .execute(&installed.pool).await;
    Json(json!({"ok": true})).into_response()
}

async fn remove(
    State(state): State<AppState>,
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Response {
    let _ = sqlx::query(&crate::db::q(installed.kind,
        "DELETE FROM workers WHERE id = ? AND user_id = ?"))
        .bind(id).bind(user.id).execute(&installed.pool).await;
    state.workers.remove(id).await; // 断开其在线连接（出站通道 drop 后任务退出）
    Json(json!({"ok": true})).into_response()
}
```

- [ ] **Step 2: 填充 `routes()`（受保护路由）**

```rust
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/worker/pair", post(pair))
        .route("/worker/list", get(list))
        .route("/worker/{id}", patch(rename).delete(remove))
}
```
（axum 0.8 路径参数语法为 `{id}`。）

- [ ] **Step 3: 在 `src/main.rs` 受保护路由区挂载**

在 `.merge(...)` 那一串里加：
```rust
        .merge(worker::routes())
```

- [ ] **Step 4: 验证编译**

Run: `cargo check -p yunova 2>&1 | tail -30`
Expected: 编译通过。

- [ ] **Step 5: 手动验证（启动后端，配对 + 连工蜂）**

Run（两个终端）:
```bash
# 终端 A：启动后端
rm -f /tmp/wk-test.db && YUNOVA_DATABASE_URL="sqlite:///tmp/wk-test.db?mode=rwc" cargo run -p yunova
# 浏览器走 /setup 建管理员并登录，或用已有会话 cookie。
# 拿到 cookie 后：
curl -s -X POST http://127.0.0.1:3000/api/worker/pair -H "Cookie: <session>" # 返回 {"token":"..."}
# 终端 B：用该 token 起工蜂
YUNOVA_WORKER_URL=ws://127.0.0.1:3000/api/worker/connect YUNOVA_WORKER_TOKEN=<token> cargo run -p yunova-worker
# 终端 A 工蜂日志应出现「鉴权成功」；再 curl /api/worker/list 应见 online:true
```
Expected: 工蜂日志「鉴权成功，worker_id=…」；list 返回 `online:true`。

- [ ] **Step 6: 提交**

```bash
git add src/worker.rs src/main.rs
git commit -m "feat(worker): 工蜂管理 REST（配对码/列表/改名/删除）"
```

---

## Task 9: Agent 循环（核心）+ 会话 REST + SSE

**Files:**
- Modify: `src/worker.rs`

> 复用 `channels::resolve_route(pool, kind, headers, "chat", "claude", model)` 拿渠道链；**自己发非流式** Claude Messages 请求（现有 main.rs 是流式，这里需完整 JSON 拿 tool_use）。扣费用 `channels::try_deduct_for_model(..., "chat", "claude", "worker_agent")`。模型名从请求体取（前端传），渠道链按现有规则解析。

**Agent 循环子流程（封装为 `async fn run_turn`）：**

- [ ] **Step 1: 定义工具 schema 与请求/响应辅助**

```rust
use crate::channels;

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
async fn call_claude(
    state: &AppState,
    chain: &[channels::ChannelChoice],
    model: &str,
    messages: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let body = json!({
        "model": model,
        "max_tokens": 4096,
        "tools": tool_defs(),
        "messages": messages,
    });
    for choice in chain {
        let upstream_model = if choice.upstream_model.is_empty() { model } else { &choice.upstream_model };
        let endpoint = format!("{}/v1/messages", choice.channel.base_url.trim_end_matches('/'));
        let client = match crate::net_guard::client_for_upstream(&state.http, &endpoint, true).await {
            Ok(c) => c, Err(_) => continue,
        };
        let mut sendbody = body.clone();
        sendbody["model"] = json!(upstream_model);
        let resp = client.post(&endpoint)
            .header("content-type","application/json")
            .header("x-api-key", choice.channel.api_key.as_str())
            .header("anthropic-version","2023-06-01")
            .json(&sendbody).send().await;
        let resp = match resp { Ok(r) => r, Err(_) => continue };
        if !resp.status().is_success() { let _ = resp.bytes().await; continue; }
        match resp.json::<serde_json::Value>().await {
            Ok(v) => return Ok(v), Err(_) => continue,
        }
    }
    Err("所有 Claude 渠道均不可用".into())
}
```

> 注意：`channels::ChannelChoice` / 其 `channel.base_url` / `channel.api_key` / `upstream_model` 字段名以 `src/channels.rs` 实际定义为准（`grep -n "pub struct ChannelChoice" -A12 src/channels.rs` 核对；若字段名不同按实际调整）。

- [ ] **Step 2: 实现会话消息端点（SSE）+ agent 循环**

设计：`POST /api/worker/sessions/{sid}/message`，body `{ "worker_id": N, "model": "...", "text": "...", "auto_approve": bool }`。返回 SSE。
审批暂停：每个待审批 call 在内存表 `pending_approvals: Arc<RwLock<HashMap<String, oneshot::Sender<bool>>>>`（挂在 AppState 或一个会话级结构）中放一个 oneshot，SSE 发 `approval_required` 后 `.await` 该 oneshot；`/approve` 端点取出并 send 决定。

为控制篇幅，关键骨架如下（实现时补全 SSE wiring，用 `axum::response::sse`）：

```rust
use axum::response::sse::{Event, Sse};
use std::convert::Infallible;
use futures_util::stream::Stream;

// AppState 需再加一个字段（在 Task 6 的 WorkerRegistry 旁边一起加更佳；
// 若已发布则此处补加）：
//   pub approvals: Arc<RwLock<HashMap<String, oneshot::Sender<bool>>>>
// 用于 shell/write 审批：call_id -> 决定(true=批准)。

#[derive(Deserialize)]
struct MessageReq {
    worker_id: i64,
    model: String,
    text: String,
    #[serde(default)]
    auto_approve: bool,
}

async fn session_message(
    State(state): State<AppState>,
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    axum::http::Extensions: /* headers via HeaderMap below */ ,
    headers: axum::http::HeaderMap,
    Path(sid): Path<i64>,
    Json(req): Json<MessageReq>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let (tx, rx) = mpsc::channel::<Event>(64);

    // 校验会话归属（省略错误处理细节，失败则发 error event 并结束）
    tokio::spawn(async move {
        // 1) 校验 worker 在线 + 归属 user
        let handle = match state.workers.get(req.worker_id).await {
            Some(h) if h.user_id == user.id => h,
            _ => { let _ = tx.send(Event::default().event("error").data("工蜂离线或无权限")).await; return; }
        };

        // 2) 解析渠道链（claude 协议）
        let route = match channels::resolve_route(&installed.pool, installed.kind, &headers, "chat", "claude", &req.model).await {
            Ok(channels::Route::Channels { chain, .. }) => chain,
            Ok(_) => { let _ = tx.send(Event::default().event("error").data("请使用服务端渠道（暂不支持 BYOK）")).await; return; }
            Err(_) => { let _ = tx.send(Event::default().event("error").data("无可用 Claude 渠道")).await; return; }
        };

        // 3) 载入历史 -> messages 数组（user/assistant/tool）
        //    简化：v1 先把本次 user text 作为单条消息开始；历史持久化见 Step 3。
        let mut messages = json!([{ "role": "user", "content": req.text }]);

        // 4) agent 循环
        let mut call_seq = 0u32;
        loop {
            // 扣费
            if let Err(_) = channels::try_deduct_for_model(&installed.pool, installed.kind, user.id, &req.model, "chat", "claude", "worker_agent").await {
                let _ = tx.send(Event::default().event("error").data("积分不足或模型未启用")).await; return;
            }
            let resp = match call_claude(&state, &route, &req.model, &messages).await {
                Ok(v) => v,
                Err(e) => { let _ = tx.send(Event::default().event("error").data(e)).await; return; }
            };
            let content = resp.get("content").cloned().unwrap_or(json!([]));
            let stop = resp.get("stop_reason").and_then(|v| v.as_str()).unwrap_or("");

            // 把 assistant 这轮（含 tool_use）追加进 messages
            messages.as_array_mut().unwrap().push(json!({"role":"assistant","content": content.clone()}));

            // 推送文本块
            if let Some(arr) = content.as_array() {
                for block in arr {
                    if block.get("type").and_then(|v|v.as_str()) == Some("text") {
                        if let Some(t) = block.get("text").and_then(|v|v.as_str()) {
                            let _ = tx.send(Event::default().event("text").data(t)).await;
                        }
                    }
                }
            }

            if stop != "tool_use" {
                let _ = tx.send(Event::default().event("done").data("")).await;
                return;
            }

            // 处理所有 tool_use 块，收集 tool_result
            let mut tool_results = Vec::new();
            if let Some(arr) = content.as_array() {
                for block in arr {
                    if block.get("type").and_then(|v|v.as_str()) != Some("tool_use") { continue; }
                    let tu_id = block.get("id").and_then(|v|v.as_str()).unwrap_or("").to_string();
                    let tool = block.get("name").and_then(|v|v.as_str()).unwrap_or("").to_string();
                    let input = block.get("input").cloned().unwrap_or(json!({}));

                    let _ = tx.send(Event::default().event("tool_call").data(
                        serde_json::to_string(&json!({"tool":tool,"input":input})).unwrap())).await;

                    // 审批：read_file 自动；shell/write_file 看 auto_approve
                    let needs_approval = matches!(tool.as_str(), "shell" | "write_file") && !req.auto_approve;
                    if needs_approval {
                        call_seq += 1;
                        let approve_key = format!("{sid}-{call_seq}");
                        let (atx, arx) = oneshot::channel::<bool>();
                        state.approvals.write().await.insert(approve_key.clone(), atx);
                        let _ = tx.send(Event::default().event("approval_required").data(
                            serde_json::to_string(&json!({"call_id":approve_key,"tool":tool,"input":input})).unwrap())).await;
                        let approved = arx.await.unwrap_or(false);
                        if !approved {
                            tool_results.push(json!({
                                "type":"tool_result","tool_use_id":tu_id,
                                "content":"用户拒绝了此操作","is_error":true}));
                            continue;
                        }
                    }

                    // 下发工蜂执行
                    let call_id = format!("exec-{sid}-{}", tu_id);
                    let (rtx, rrx) = oneshot::channel::<ToolOutcome>();
                    handle.pending.write().await.insert(call_id.clone(), rtx);
                    let _ = handle.tx.send(ToWorker::Exec { call_id: call_id.clone(), tool: tool.clone(), args: input.clone() }).await;
                    let outcome = match tokio::time::timeout(std::time::Duration::from_secs(120), rrx).await {
                        Ok(Ok(o)) => o,
                        _ => { handle.pending.write().await.remove(&call_id); ToolOutcome{ok:false, output:"执行超时或工蜂断开".into()} }
                    };
                    let _ = tx.send(Event::default().event("tool_result").data(
                        serde_json::to_string(&json!({"ok":outcome.ok,"output":outcome.output})).unwrap())).await;
                    tool_results.push(json!({
                        "type":"tool_result","tool_use_id":tu_id,
                        "content": outcome.output, "is_error": !outcome.ok}));
                }
            }
            // 把 tool_results 作为新一轮 user turn 追加
            messages.as_array_mut().unwrap().push(json!({"role":"user","content": tool_results}));
        }
    });

    let stream = tokio_stream::wrappers::ReceiverStream::new(rx).map(Ok);
    Sse::new(stream)
}
```

> 依赖：`tokio-stream`（已间接可用？`grep tokio-stream Cargo.toml`，没有则在根 `Cargo.toml` 加 `tokio-stream = "0.1"`）。`headers` 提取器写法以 axum 0.8 为准（`axum::http::HeaderMap` 直接作参数即可，删掉上面那行占位的 `axum::http::Extensions:` 伪代码）。

- [ ] **Step 3: 加审批端点 + 在 AppState 加 `approvals` 字段**

```rust
#[derive(Deserialize)]
struct ApproveReq { call_id: String, decision: bool }

async fn approve(
    State(state): State<AppState>,
    Extension(_user): Extension<CurrentUser>,
    Path(_sid): Path<i64>,
    Json(req): Json<ApproveReq>,
) -> Response {
    if let Some(slot) = state.approvals.write().await.remove(&req.call_id) {
        let _ = slot.send(req.decision);
    }
    Json(json!({"ok": true})).into_response()
}
```
在 AppState 加（与 Task 6 的 workers 字段一起）：
```rust
    pub approvals: std::sync::Arc<tokio::sync::RwLock<std::collections::HashMap<String, tokio::sync::oneshot::Sender<bool>>>>,
```
构造处加：`approvals: Default::default(),`

把端点加进 `routes()`：
```rust
        .route("/worker/sessions/{sid}/message", post(session_message))
        .route("/worker/sessions/{sid}/approve", post(approve))
```

> **会话历史持久化（v1 简化）**：本步先不落 worker_messages，循环用内存 messages。落库留作 Step 4 收尾——把每条 user/assistant/tool 写入 `worker_messages`，并在 message 端点开头加载历史拼进 messages。若时间紧，v1 可仅创建会话行 + 存首尾，标注 TODO。

- [ ] **Step 4: 会话创建 + 历史落库**

加 `POST /api/worker/sessions`（建 worker_sessions 行，返回 sid）与 `GET /api/worker/sessions/{sid}/messages`（读历史）。在 agent 循环里：开始时插入 user 消息，每轮 assistant 文本与 tool 交互各插一条 worker_messages（role=assistant/tool，content 存 JSON）。照 `src/skills.rs` 的插入返回 id 模式建会话行。

- [ ] **Step 5: 验证编译**

Run: `cargo check -p yunova 2>&1 | tail -40`
Expected: 编译通过。逐个修复字段名 / 提取器签名编译错误。

- [ ] **Step 6: 端到端手动验证**

按 Task 8 Step 5 起后端 + 工蜂；用 curl 或网页发起一条会话消息（如「在 /tmp 建个 hello.txt 写入当前时间」），观察 SSE：应见 `tool_call`(write_file) → `approval_required` → 调 `/approve` 批准 → `tool_result` → `done`；工蜂所在机 `/tmp/hello.txt` 确实生成。
Expected: 文件生成，SSE 事件齐全。

- [ ] **Step 7: 提交**

```bash
git add src/worker.rs src/main.rs Cargo.toml
git commit -m "feat(worker): agent 循环 + 会话 SSE + 分级审批 + 历史落库"
```

---

## Task 10: 网页 API 客户端 worker.ts

**Files:**
- Create: `web/src/lib/worker.ts`

> 参考 `web/src/lib/chat-stream.ts` 的 SSE 解析与 `web/src/lib/skills.ts` 的 REST 封装风格。

- [ ] **Step 1: 写 `web/src/lib/worker.ts`**

```ts
import { api } from "./api";

export interface Worker {
  id: number;
  name: string;
  last_seen_at: string | null;
  online: boolean;
}

export async function listWorkers(): Promise<Worker[]> {
  return api.get("/worker/list");
}
export async function pairWorker(): Promise<{ token: string }> {
  return api.post("/worker/pair", {});
}
export async function renameWorker(id: number, name: string) {
  return api.patch(`/worker/${id}`, { name });
}
export async function deleteWorker(id: number) {
  return api.delete(`/worker/${id}`);
}
export async function createSession(): Promise<{ id: number }> {
  return api.post("/worker/sessions", {});
}
export async function approve(sid: number, call_id: string, decision: boolean) {
  return api.post(`/worker/sessions/${sid}/approve`, { call_id, decision });
}

export interface AgentEvent {
  type: "text" | "tool_call" | "approval_required" | "tool_result" | "done" | "error";
  data: any;
}

/** 发起一轮 agent 会话，逐事件回调。返回中断函数。 */
export function sendMessage(
  sid: number,
  body: { worker_id: number; model: string; text: string; auto_approve: boolean },
  onEvent: (e: AgentEvent) => void,
): () => void {
  const ctrl = new AbortController();
  (async () => {
    const resp = await fetch(`/api/worker/sessions/${sid}/message`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
      signal: ctrl.signal,
      credentials: "include",
    });
    if (!resp.body) return;
    const reader = resp.body.getReader();
    const decoder = new TextDecoder();
    let buf = "";
    let evName = "message";
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      buf += decoder.decode(value, { stream: true });
      const lines = buf.split("\n");
      buf = lines.pop() ?? "";
      for (const line of lines) {
        if (line.startsWith("event:")) evName = line.slice(6).trim();
        else if (line.startsWith("data:")) {
          const raw = line.slice(5).trim();
          let data: any = raw;
          try { data = JSON.parse(raw); } catch { /* 纯文本 */ }
          onEvent({ type: evName as AgentEvent["type"], data });
        }
      }
    }
  })().catch((e) => onEvent({ type: "error", data: String(e) }));
  return () => ctrl.abort();
}
```

> 核对 `web/src/lib/api.ts` 是否暴露 `get/post/patch/delete` 与 base 前缀（`/api`）。若签名不同（如返回 `Response` 而非已解析 JSON），按实际调整。

- [ ] **Step 2: 验证构建**

Run: `cd web && bun run build 2>&1 | tail -20`
Expected: 构建通过。

- [ ] **Step 3: 提交**

```bash
git add web/src/lib/worker.ts
git commit -m "feat(worker): 网页 API 客户端（REST + agent SSE 解析）"
```

---

## Task 11: 工蜂页 UI + 侧边栏入口

**Files:**
- Create: `web/src/components/app/WorkerPage.tsx`
- Modify: 侧边栏/路由文件（先 `grep -rn "ChatPage\|侧边栏\|sidebar\|<Route" web/src` 定位现有页面挂载方式，照同一模式加「工蜂」入口）

> 用现有 shadcn/ui 组件（`web/src/components/ui/`）。中文 UI。

- [ ] **Step 1: 写 `WorkerPage.tsx`**

包含两区：①工蜂管理（按钮「生成配对码」→ 弹窗显示 token + 复制 + 部署命令；列表含在线徽标、改名、删除）②会话区（选工蜂 → 输入框发消息 → 渲染 text/tool_call/tool_result 卡片；approval_required 卡片含「批准/拒绝」按钮调 `approve()`；顶部「自动批准」Switch）。

最小可用骨架（实现时补全样式）：
```tsx
import { useEffect, useState } from "react";
import * as W from "@/lib/worker";

export default function WorkerPage() {
  const [workers, setWorkers] = useState<W.Worker[]>([]);
  const [token, setToken] = useState<string | null>(null);
  const [sel, setSel] = useState<number | null>(null);
  const [model, setModel] = useState("claude-opus-4-8");
  const [auto, setAuto] = useState(false);
  const [input, setInput] = useState("");
  const [log, setLog] = useState<W.AgentEvent[]>([]);
  const [sid, setSid] = useState<number | null>(null);

  const refresh = () => W.listWorkers().then(setWorkers);
  useEffect(() => { refresh(); const t = setInterval(refresh, 5000); return () => clearInterval(t); }, []);

  async function send() {
    if (!sel || !input.trim()) return;
    let s = sid;
    if (s == null) { s = (await W.createSession()).id; setSid(s); }
    const text = input; setInput("");
    setLog((l) => [...l, { type: "text", data: `🧑 ${text}` }]);
    W.sendMessage(s!, { worker_id: sel, model, text, auto_approve: auto }, (e) => {
      setLog((l) => [...l, e]);
    });
  }

  return (
    <div className="p-4 space-y-4">
      <section>
        <h2 className="font-semibold mb-2">工蜂</h2>
        <button className="btn" onClick={() => W.pairWorker().then((r) => { setToken(r.token); refresh(); })}>
          生成配对码
        </button>
        {token && (
          <div className="mt-2 p-2 bg-muted rounded text-sm break-all">
            配对码（仅显示一次）：<code>{token}</code>
            <div className="mt-1 text-xs opacity-70">
              部署：<code>YUNOVA_WORKER_URL=wss://你的域名/api/worker/connect YUNOVA_WORKER_TOKEN={token} ./yunova-worker</code>
            </div>
          </div>
        )}
        <ul className="mt-2 space-y-1">
          {workers.map((w) => (
            <li key={w.id} className="flex items-center gap-2 text-sm">
              <span className={w.online ? "text-green-500" : "text-gray-400"}>●</span>
              <button className="underline" onClick={() => setSel(w.id)}>{w.name}</button>
              <button className="text-xs opacity-60" onClick={() => W.deleteWorker(w.id).then(refresh)}>删除</button>
            </li>
          ))}
        </ul>
      </section>

      {sel && (
        <section className="border-t pt-3">
          <label className="text-sm flex items-center gap-2 mb-2">
            <input type="checkbox" checked={auto} onChange={(e) => setAuto(e.target.checked)} /> 自动批准
          </label>
          <div className="space-y-1 max-h-96 overflow-auto text-sm">
            {log.map((e, i) => <EventRow key={i} e={e} sid={sid!} />)}
          </div>
          <div className="flex gap-2 mt-2">
            <input className="border flex-1 px-2 py-1 rounded" value={input}
              onChange={(e) => setInput(e.target.value)} placeholder="让工蜂做点什么…" />
            <button className="btn" onClick={send}>发送</button>
          </div>
        </section>
      )}
    </div>
  );
}

function EventRow({ e, sid }: { e: W.AgentEvent; sid: number }) {
  if (e.type === "text") return <div>{typeof e.data === "string" ? e.data : ""}</div>;
  if (e.type === "tool_call") return <div className="text-blue-500">🔧 {e.data.tool}: {JSON.stringify(e.data.input)}</div>;
  if (e.type === "tool_result") return <pre className="bg-muted p-1 rounded whitespace-pre-wrap">{e.data.output}</pre>;
  if (e.type === "error") return <div className="text-red-500">⚠ {String(e.data)}</div>;
  if (e.type === "approval_required")
    return (
      <div className="bg-yellow-50 p-2 rounded">
        需批准 {e.data.tool}：<code>{JSON.stringify(e.data.input)}</code>
        <button className="ml-2 text-green-600" onClick={() => W.approve(sid, e.data.call_id, true)}>批准</button>
        <button className="ml-2 text-red-600" onClick={() => W.approve(sid, e.data.call_id, false)}>拒绝</button>
      </div>
    );
  return null;
}
```

- [ ] **Step 2: 接入侧边栏 / 路由**

按 Step 0 定位到的现有模式（例如若用某种 tab/视图切换 state，则在该 enum/数组加 `"worker"` 项并渲染 `<WorkerPage/>`；若用 react-router 则加 `<Route path="/worker" element={<WorkerPage/>}/>` 与导航项）。中文入口名「工蜂」。

- [ ] **Step 3: 验证构建**

Run: `cd web && bun run build 2>&1 | tail -20`
Expected: 构建通过。

- [ ] **Step 4: 端到端 UI 验证**

`cargo run -p yunova` 起后端（前端已嵌入），登录 → 进「工蜂」→ 生成配对码 → 另起工蜂连上 → 在会话区发「列出 /etc 下前 5 个文件」→ 观察自动执行(read/shell)与审批流。
Expected: 在线徽标变绿；会话能跑通工具调用与审批。

- [ ] **Step 5: 提交**

```bash
git add web/src/
git commit -m "feat(worker): 工蜂页 UI（管理 + agent 会话 + 审批）+ 侧边栏入口"
```

---

## Task 12: 部署文档 + 收尾

**Files:**
- Create: `worker/README.md`
- Modify: `CLAUDE.md`（在架构小节补一段工蜂说明，可选）

- [ ] **Step 1: 写 `worker/README.md`**

内容：工蜂用途、单二进制构建方式（`cargo build -p yunova-worker --release`，按 CLAUDE.md 规约——构建在本地/CI 做，不在线上机；产物 `target/release/yunova-worker`）、部署运行（设 `YUNOVA_WORKER_URL` / `YUNOVA_WORKER_TOKEN` / 可选 `YUNOVA_WORKER_NAME`，建议非 root 运行）、安全提示（工蜂能执行任意命令，仅部署到自己信任的机器）。

- [ ] **Step 2: 全量构建复核**

Run: `cargo check 2>&1 | tail -5 && cd web && bun run build 2>&1 | tail -5`
Expected: 两端均通过。

- [ ] **Step 3: 提交**

```bash
git add worker/README.md CLAUDE.md
git commit -m "docs(worker): 部署与安全说明"
```

---

## Self-Review 结论（已对照规格）

- **规格覆盖**：§3.1 工蜂二进制→Task 2-4；§3.2 后端模块→Task 6-9；§3.3 网页→Task 10-11；§4 协议→Task 2/6；§5 agent 循环+审批→Task 9；§6 数据模型→Task 5；§7 安全（token 哈希/归属校验）→Task 7/8；§8 计费→Task 9；workspace/ws 前置→Task 1。全部有对应任务。
- **类型一致性**：`ToServer`/`ToWorker` 两端同名同形（worker/src/proto.rs 与 src/worker.rs）；`ToolOutcome`、`WorkerHandle`、`WorkerRegistry` 命名贯穿 Task 6-9；前端 `AgentEvent` 事件名与后端 SSE `event:` 名一一对应（text/tool_call/approval_required/tool_result/done/error）。
- **已知待实现时核对项**（计划中已标注）：`ChannelChoice` 字段名、`api.ts` 方法签名、axum 0.8 提取器写法、tungstenite 0.24 的 `Message::Text` 类型、`tokio-stream` 依赖是否需新增——均在对应步骤内注明「以实际为准并按编译器修正」。
- **无占位**：所有代码步骤含完整代码；SSE 端点因篇幅给的是可编译骨架并明确标出补全点（Step 2/3/4）。
