# 工蜂（Worker）远程 Agent — 设计规格

- 日期：2026-06-14
- 状态：已通过设计评审，待规格评审
- 作者：Claude Code

## 1. 目标与定位

在用户自己的服务器上部署一个轻量 Rust 二进制（**工蜂 / Worker**），它主动连回 Yunova 后端。用户在网页上用自然语言对话，Yunova 后端用 **Claude** 跑 agent 循环（类 Claude Code），把 **shell / 读文件 / 写文件** 这三类工具调用下发给目标工蜂执行，结果回传给 Claude 继续推理，直到任务完成。全程结果流式推送到网页。

**核心定位**：思考循环在 Yunova 后端（复用渠道 / credits / LLM 能力），工蜂是**瘦执行器**。

## 2. 关键决策（已与用户确认）

| # | 维度 | 决策 |
|---|------|------|
| 1 | 智能程度 | AI 驱动的 agent（类 Claude Code），非纯命令中继 |
| 2 | 思考循环位置 | Yunova 后端编排，工蜂只执行工具调用 |
| 3 | 鉴权 | 一次性配对码 token（部署时填入工蜂配置） |
| 4 | 工具集（v1） | 最小集：`shell`、`read_file`、`write_file` |
| 5 | 安全策略 | 分级 + 可切换：read 自动；shell/write 默认审批；会话级「自动批准」开关 |
| 6 | 分发形态 | 单个静态 Rust 二进制 |
| 7 | Agent 协议 | 仅 Anthropic（Claude）tool-use |
| 8 | 网页位置 | 全新独立页面「工蜂」，侧边栏新增入口 |

## 3. 架构组件

### 3.1 工蜂二进制（新 workspace crate `worker/`）
- 单静态可执行文件。启动时读取配置：Yunova WebSocket 地址 + 配对码 token（环境变量或配置文件）。
- 建立一条常驻 WebSocket 连回后端 `/api/worker/connect`，发 `hello{token}` 鉴权。
- 鉴权成功后进入事件循环：接收 `exec` 指令 → 本地执行 → 回传 `tool_result`；定期发 `heartbeat`。
- 断线自动重连（指数退避）。
- 只实现三个工具的执行逻辑，不含任何 LLM 客户端、不持有模型 key。
- 工蜂以「启动它的系统用户身份」运行；文档建议非 root 运行。

### 3.2 后端工蜂模块（`src/worker.rs`）
- **WS 接入端点**：`GET /api/worker/connect`（WebSocket 升级），校验配对 token → 绑定到用户 → 注册进在线表。
- **在线工蜂注册表**：进程内 `Arc<RwLock<HashMap<worker_id, Sender>>>`，挂在 AppState 上（或独立的 WorkerRegistry）。每个工蜂一个 mpsc 发送通道，用于把 `exec` 推给对应 WS 任务。
- **Agent 循环**：见 §5。
- **REST 路由**（`pub fn routes() -> Router<AppState>`，挂在 `require_auth` 下）：
  - `POST /api/worker/pair` — 生成一次性配对码，返回明文一次（仅此一次），库里存哈希。
  - `GET /api/worker/list` — 列出本用户的工蜂（名称、在线状态、最后心跳时间）。
  - `PATCH /api/worker/:id` — 改名。
  - `DELETE /api/worker/:id` — 删除（断开其连接并失效 token）。
  - `POST /api/worker/sessions/:sid/message` — 向某工蜂发起/继续一轮 agent 会话（SSE 流式响应）。
  - `POST /api/worker/sessions/:sid/approve` — 批准/拒绝某个待审批的工具调用（body 含 `call_id` + `decision`）。

### 3.3 网页工蜂页（`web/src/components/app/` + `web/src/lib/worker.ts`）
- 侧边栏新增「工蜂」入口。
- **工蜂管理区**：生成配对码（弹窗展示明文 + 复制 + 部署命令提示）、工蜂列表（在线/离线徽标、改名、删除）。
- **Agent 会话区**：选定一台工蜂后进入对话；渲染 agent 推理文本、工具调用卡片（shell 命令 / 写文件 diff 预览）、工具结果；`shell`/`write_file` 卡片带「批准 / 拒绝」按钮；顶部「自动批准」开关。
- API 客户端 `worker.ts` 封装上述 REST + SSE 解析。

## 4. 连接与消息协议（WebSocket，JSON 文本帧）

**工蜂 → 后端：**
```jsonc
{ "type": "hello", "token": "<pairing-code>", "name": "<hostname>" }
{ "type": "heartbeat" }
{ "type": "tool_result", "call_id": "...", "ok": true, "output": "...", "truncated": false }
```

**后端 → 工蜂：**
```jsonc
{ "type": "exec", "call_id": "...", "tool": "shell|read_file|write_file", "args": { ... } }
{ "type": "hello_ok", "worker_id": 123 }   // 鉴权成功
{ "type": "error", "message": "..." }       // 鉴权失败等
```

**工具参数（args）：**
- `shell`: `{ "command": "...", "cwd": "<可选>", "timeout_ms": <可选> }`
- `read_file`: `{ "path": "..." }`
- `write_file`: `{ "path": "...", "content": "..." }`

输出超过上限（如 64KB）时截断并置 `truncated=true`，避免撑爆。

## 5. Agent 循环（核心新逻辑）

`POST /api/worker/sessions/:sid/message` 处理流程：
1. 校验会话归属 + 目标工蜂在线。
2. 加载会话历史 → 组装 Anthropic Messages 请求体，附带 3 个工具定义（tools schema）。
3. 通过 `channels::resolve_route(pool, kind, headers, "chat", "claude", model)` 解析渠道链，调 Claude **Messages API（非流式或流式皆可，v1 用非流式拿完整 tool_use 更简单）**。
4. **扣费**：每次调 Claude 按现有 `cost_chat` / `try_deduct_for_model` 扣 credits；失败按现有约定退款。
5. 解析响应：
   - 若 `stop_reason == "tool_use"`：对每个 tool_use 块——
     - `read_file` → 直接下发工蜂执行（自动）。
     - `shell` / `write_file` → 若会话「自动批准」开则直接下发；否则经 SSE 发 `approval_required` 事件并**暂停**，等 `/approve` 调用决定。
       - 批准 → 下发工蜂执行；拒绝 → 构造一个表示「用户拒绝」的 tool_result 喂回 Claude。
     - 收集所有 tool_result → 作为新一轮 user turn 喂回 Claude，回到第 3 步。
   - 若 `stop_reason == "end_turn"`：输出最终文本，结束本轮。
6. 全程通过 SSE 向网页推送事件：`text`（增量/整段）、`tool_call`、`approval_required`、`tool_result`、`done`、`error`。

**暂停等审批的实现（v1）**：agent 循环驻留在内存、挂在该 SSE 连接上，用一个 `oneshot`/`Notify` 等待 `/approve` 唤醒。网页刷新/断线会中断当前那一轮（已与用户确认 v1 可接受）。对话历史照常落库，刷新后可看到历史，但进行中的那一轮需重发。

**工具定义（传给 Claude 的 tools）：**
- `shell` — 在远程服务器执行 shell 命令，返回 stdout/stderr/exit code。
- `read_file` — 读取远程服务器上指定路径文件内容。
- `write_file` — 写入/覆盖远程服务器上指定路径文件。

## 6. 数据模型（三方言各一份迁移，注册进 `db.rs` 三数组）

### `workers`
| 列 | 类型 | 说明 |
|----|------|------|
| id | PK | |
| user_id | FK | 归属用户 |
| name | text | 工蜂名（默认取 hello 的 hostname） |
| token_hash | text | 配对码哈希（仅存哈希） |
| status | text | `online` / `offline`（由心跳维护，亦可纯内存判断） |
| last_seen_at | datetime | 最后心跳 |
| created_at | datetime | |

### `worker_sessions`
| 列 | 类型 | 说明 |
|----|------|------|
| id | PK | |
| user_id | FK | |
| worker_id | FK | 目标工蜂 |
| title | text | 会话标题 |
| created_at / updated_at | datetime | |

### `worker_messages`
| 列 | 类型 | 说明 |
|----|------|------|
| id | PK | |
| session_id | FK | |
| role | text | `user` / `assistant` / `tool` |
| content | text | 文本或 JSON（工具调用 / 结果序列化） |
| created_at | datetime | |

迁移遵循 `NNNN_name.sql` 三方言规范，所有 SQL 过 `db::q`，布尔用 `db::bool_as_int` / `db::bool_true`，时间用 `db::now_expr`，自增 id 用 §架构里 skills.rs 的 RETURNING / LAST_INSERT_ID 模式。

## 7. 安全

- 配对码：高熵随机串，**仅哈希入库**，生成时明文只返回一次。
- 授权：所有 REST + WS 操作校验工蜂 / 会话归属当前用户；用户只能操控自己名下工蜂。
- 工蜂运行身份由部署者决定，文档强调非 root + 最小权限；Yunova 不对工蜂能跑什么做沙箱（v1 信任部署者自己的机器）。
- shell/write 默认审批即第一道防线。
- token 失效：删除工蜂即吊销其 token 并断开连接。

## 8. 计费

复用现有 credits：agent 循环每次调 Claude 走 `try_deduct_for_model`，按 `cost_chat` / 模型定价扣费；上游失败按现有约定退款（用扣费时返回的 cost 退，不重新读价）。

## 9. 范围外（v1 不做，后续迭代）

- 进程 / 后台任务管理工具、git 工具、包管理封装。
- OpenAI / Gemini 协议的 agent 循环。
- Docker 镜像分发、systemd 安装脚本（v1 先给手动运行说明）。
- 断线可恢复的审批暂停（持久化 agent 状态、断线续跑）。
- 单工蜂多会话并发编排的复杂调度。

## 10. 落地步骤（概览，详见后续实现计划）

1. 三方言迁移（workers / worker_sessions / worker_messages）+ 注册进 `db.rs`。
2. `src/worker.rs`：WS 接入端点 + 注册表 + REST 路由 + agent 循环；`main.rs` 挂载。
3. 新建 `worker/` crate（workspace member）：配置、WS 客户端、三工具执行、重连。
4. `web/src/lib/worker.ts` + 工蜂页组件 + 侧边栏入口。
5. 两端构建验证（`cargo check`、`bun run build`）。
