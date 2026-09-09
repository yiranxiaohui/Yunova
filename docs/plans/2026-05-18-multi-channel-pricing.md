# 多渠道 + 按模型计费 实现计划

> **For Hermes:** 用 subagent-driven-development 逐任务执行。

**Goal:** 把 Yunova 的"单 shared upstream + 全局 cost_chat/cost_image 扁平定价"重构为"多渠道（channel）+ 按模型独立定价 + 优先级 fallback"。

**Architecture:**
- 新增 `upstream_channels` 表（管理员 CRUD 的上游连接池）
- 新增 `model_pricing` 表（模型白名单 + 每次调用积分）
- 新增 `channel_models` 表（M:N，标记每个渠道能跑哪些模型）
- 用户调用时：`model → 查 pricing.cost & kind → 查 channel_models 筛 channel → 按 priority 选第一个 enabled → 失败 fallback 下一个`
- 老 KV (`shared_chat_*_url/key/model`) 在 migration 里读出来 → 自动种 channel + 该 channel 的 model 行；保留 KV 只读以便回退

**Tech Stack:** Rust + Axum + sqlx（SQLite/MySQL/Postgres 三库兼容）+ React/Vite/Tailwind 前端

**已拍板决策：**
- ✅ 失败自动 fallback 到下一个 channel（new-api 风格）
- ✅ 未定价模型一律拒绝（白名单制）
- ✅ 仍用 i64 "积分/次"（不拆 input/output）

---

## 阶段 1 — Schema & Migration（后端基础）

### Task 1.1：写 migration `0019_channels_pricing.sql`（三库各一份）

**Objective:** 建 3 张新表 + 索引；从老 KV 种数据；保留老表不动。

**Files:**
- Create: `migrations/sqlite/0019_channels_pricing.sql`
- Create: `migrations/postgres/0019_channels_pricing.sql`
- Create: `migrations/mysql/0019_channels_pricing.sql`

**Schema 要点（三库统一字段、各自语法）：**

```sql
-- 上游渠道
CREATE TABLE upstream_channels (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,   -- pg: BIGSERIAL / mysql: BIGINT AUTO_INCREMENT
  name         TEXT NOT NULL UNIQUE,                -- "OpenAI 官方"、"Azure-East"
  protocol     TEXT NOT NULL,                       -- 'openai' | 'claude' | 'gemini'
  kind         TEXT NOT NULL,                       -- 'chat' | 'image'
  base_url     TEXT NOT NULL,
  api_key      TEXT NOT NULL,                       -- 不返回给前端
  enabled      INTEGER NOT NULL DEFAULT 1,          -- pg: BOOLEAN
  priority     INTEGER NOT NULL DEFAULT 100,        -- 数字小=优先；同优先级随机
  created_at   TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at   TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- 模型定价（白名单）
CREATE TABLE model_pricing (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  model        TEXT NOT NULL UNIQUE,                -- "gpt-5"、"claude-sonnet-4"、"imagen-4"
  kind         TEXT NOT NULL,                       -- 'chat' | 'image'
  cost_credits INTEGER NOT NULL,                    -- 每次调用扣多少
  display_name TEXT,                                -- 可选；前端展示
  enabled      INTEGER NOT NULL DEFAULT 1,
  created_at   TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at   TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- 渠道 × 模型 M:N
CREATE TABLE channel_models (
  channel_id   INTEGER NOT NULL,
  model        TEXT    NOT NULL,
  upstream_id  TEXT,                                -- 上游真实 model id（可不同于内部名）；NULL=同名
  PRIMARY KEY (channel_id, model),
  FOREIGN KEY (channel_id) REFERENCES upstream_channels(id) ON DELETE CASCADE
);

CREATE INDEX idx_channel_models_model ON channel_models(model);
CREATE INDEX idx_upstream_channels_enabled ON upstream_channels(enabled, priority);
```

**MySQL 注意**：`AUTOINCREMENT` → `AUTO_INCREMENT`，列后加 `ENGINE=InnoDB DEFAULT CHARSET=utf8mb4`，`TEXT` 主键 → `VARCHAR(191)`。
**Postgres 注意**：`AUTOINCREMENT` → `BIGSERIAL`，`INTEGER NOT NULL DEFAULT 1` for booleans 改成 `BOOLEAN DEFAULT TRUE`，`TIMESTAMP` → `TIMESTAMPTZ DEFAULT NOW()`。

**Step 1：写 SQLite 版（参考 `migrations/sqlite/0011_credits.sql` 风格）**
**Step 2：写 Postgres 版（参考 `migrations/postgres/0011_credits.sql`）**
**Step 3：写 MySQL 版（参考 `migrations/mysql/0011_credits.sql`）**
**Step 4：本地起 SQLite 跑 `cargo run`，检查表创建无报错**
**Step 5：Commit `feat(credits): add upstream_channels + model_pricing schema (0019)`**

---

### Task 1.2：写数据迁移代码 —— 老 KV → 新表

**Objective:** 启动时如果检测到 `shared_chat_openai_url` 等老 KV 存在且新表为空，自动种一行 channel + 它列出的 model。

**Files:**
- Modify: `src/credits.rs`（新增 `pub async fn migrate_legacy_shared(...)`）
- Modify: `src/main.rs`（启动 hook 里调一次，紧跟现有 migration 之后）

**逻辑：**
```rust
// 对 5 对 (kind, protocol) 循环：
//   chat/openai, chat/claude, chat/gemini, image/openai, image/gemini
// 读 shared_{kind}_{protocol}_{url,key,model}
// 若 url & key 非空且 upstream_channels 里没同 base_url+protocol 的：
//   INSERT INTO upstream_channels (name='Legacy {kind}/{protocol}', ...)
//   若 model 非空且 model_pricing 没该 model：
//     INSERT INTO model_pricing (model, kind, cost_credits=老 cost_chat or cost_image)
//   INSERT INTO channel_models (channel_id, model)
```

**幂等：** 用 `INSERT ... WHERE NOT EXISTS` 或先 SELECT 判空再插。

**Step 1**：写函数 + 单测（SQLite in-memory）
**Step 2**：`cargo test credits::migrate_legacy`
**Step 3**：commit `feat(credits): seed channels/pricing from legacy KV`

---

## 阶段 2 — Channel 选择器（核心运行时）

### Task 2.1：实现 `pick_channel(model)` 返回有序候选列表

**Files:**
- Create: `src/channels.rs`（新模块）
- Modify: `src/lib.rs` / `src/main.rs` 加 `mod channels;`

**接口：**
```rust
pub struct ChannelPick {
    pub id: i64,
    pub name: String,
    pub protocol: String,   // "openai" | "claude" | "gemini"
    pub kind: String,       // "chat" | "image"
    pub base_url: String,
    pub api_key: String,
    pub upstream_model: String, // 真实传给上游的 model id
}

pub async fn list_channels_for_model(
    pool: &Pool, db_kind: DbKind, model: &str,
) -> Vec<ChannelPick>;

pub async fn lookup_pricing(
    pool: &Pool, db_kind: DbKind, model: &str,
) -> Option<(i64 /*cost*/, String /*kind chat|image*/)>;
```

**SQL 大致：**
```sql
SELECT c.id, c.name, c.protocol, c.kind, c.base_url, c.api_key,
       COALESCE(cm.upstream_id, cm.model) AS upstream_model
  FROM channel_models cm
  JOIN upstream_channels c ON c.id = cm.channel_id
 WHERE cm.model = ? AND c.enabled = 1
 ORDER BY c.priority ASC, c.id ASC;
```

**Step 1-5：** 写函数 + 单测（建临时表插数据校验顺序）+ commit

---

### Task 2.2：聊天调用走新选择器 + fallback

**Files:** Modify: `src/main.rs` 的 chat 路由

**当前逻辑（`main.rs:651` 附近）：**
```rust
if used_shared {
    let cost = get_setting_i64(..., "cost_chat", 1).await;
    try_deduct(..., cost, &format!("chat_{}", protocol.name())).await
    // 然后 read_shared 取 url/key/model，调上游
}
```

**新逻辑：**
1. 取 `body.model`（已存在）
2. `lookup_pricing(model)` —— 没结果 → `400 model_not_priced`
3. 若 `pricing.kind != "chat"` → `400 wrong_model_kind`
4. `try_deduct(cost)`
5. `list_channels_for_model(model)` → 候选 vec
6. 候选为空 → 退款 + `503 no_channel`
7. for each candidate：尝试调用上游 → 成功就 break；失败记录原因继续
8. 全失败 → 退款 + `502 all_channels_failed` 带每个 channel 的简短错误

**注意：**
- 用户自带 API key 模式（非 shared）保持不变，不进新链路
- 退款 reason 沿用 `refund_chat_{protocol}` 但带 channel name 进 ledger 更好排查

**Step 1-6：** TDD（先写 integration test：种 2 个 channel，让第一个 url 404，断言切到第二个 + 余额只扣一次）→ 实现 → commit

---

### Task 2.3：图片调用走新选择器（`images.rs` + `studio.rs`）

同 2.2 模式，针对 `images.rs:124` 和 `studio.rs:356` 两个扣费点改造。

---

## 阶段 3 — Admin API

### Task 3.1：`/api/admin/channels` CRUD

**Files:** Modify: `src/credits.rs` 的 `admin_routes()`

**路由：**
```
GET    /api/admin/channels                   列表（不返回 api_key，返回 api_key_set:bool）
POST   /api/admin/channels                   {name, protocol, kind, base_url, api_key, priority, enabled}
PATCH  /api/admin/channels/{id}              字段全可选；api_key 缺省=不变，""=清空
DELETE /api/admin/channels/{id}              级联删 channel_models
POST   /api/admin/channels/{id}/test         发个 ping 到 /v1/models 验证 base_url+key
POST   /api/admin/channels/{id}/sync-models  拉上游 /v1/models 返回列表（不入库，前端勾选后调下面接口）
PUT    /api/admin/channels/{id}/models       {models: ["gpt-5", "gpt-4o-mini"]} 全量替换该 channel 的 channel_models
```

`sync-models` 复用现有 `admin_list_upstream_models` 的拉取逻辑。

---

### Task 3.2：`/api/admin/pricing` CRUD

```
GET    /api/admin/pricing                    全部模型定价
POST   /api/admin/pricing                    {model, kind, cost_credits, display_name?}
PATCH  /api/admin/pricing/{id}
DELETE /api/admin/pricing/{id}               若有 channel 在用 → 409 + 提示先移除
```

---

### Task 3.3：清理老的 `/api/admin/app-settings` shared 字段

老字段（`shared_chat_*_url/key/model`、`shared_enabled`）从 `AdminSettingsView/Update` 移除；GET 仍返回兼容字段（值固定为空字符串/false），加 deprecated 标记日志。`cost_chat`/`cost_image` 保留作为"未匹配模型的兜底默认"——但运行时不再使用（白名单制），只显示在 UI 里说明历史含义。

---

## 阶段 4 — 用户端 API & 前端

### Task 4.1：`/api/models` 列出用户可见的模型

**Returns:**
```ts
[{
  model: "gpt-5",
  kind: "chat",
  display_name: "GPT-5",
  cost_credits: 3,
  available: true   // 是否至少有一个 enabled channel 提供它
}]
```

只列 `model_pricing.enabled=1` 且有 enabled channel 的。前端模型选择器换源。

---

### Task 4.2：废弃 `/api/shared/status`

- 删除 `shared_enabled` 概念
- `/api/credits/me` 删 `cost_chat`/`cost_image` 字段（前端按模型自己看 `/api/models` 里的 `cost_credits`）
- 老接口保留 1 版本返回兼容数据（`enabled: true, openai_available: 看是否有 openai chat channel`），下版本删

---

### Task 4.3：前端 Admin 页面

**Files:**
- Create: `web/src/admin/Channels.tsx`（渠道列表 + 增删改 + Test + Sync Models）
- Create: `web/src/admin/Pricing.tsx`（模型定价表）
- Modify: `web/src/admin/Settings.tsx`（删除 shared 配置区，加跳转到 Channels/Pricing）
- Modify: 主路由 + admin nav

**UX 要点：**
- Channels 表：name / protocol(badge) / kind / priority / models 数量 / enabled 开关 / 操作
- 新建 channel 流程：填基础 → Save → 在详情页 Sync Models 拉列表 → 勾选要启用的 → Save Models
- Pricing 表：model / kind / cost / display_name / enabled
- 添加 pricing 时下拉提示"哪些 channel 提供过这个 model"（从 channel_models 反查）

---

### Task 4.4：前端聊天模型选择器接 `/api/models`

`web/src/` 内现有 model dropdown 改为：
- 拉 `/api/models?kind=chat`
- 渲染 `display_name (cost · credits)`，禁用 `available=false`
- 余额不够时按钮 disabled + 提示充值

---

## 阶段 5 — 收尾

### Task 5.1：删除 `read_shared` + `SharedFlavor` + `SharedUpstream`
代码搜索 `read_shared|SharedFlavor|SharedUpstream` 应该全部被新链路替换；保留兼容时间窗结束后再删。

### Task 5.2：更新 `AGENTS.md` / README 说明新计费模型

### Task 5.3：写 prod 部署 checklist
1. 先 deploy（migrate 自动跑，老 KV 自动 seed）
2. 管理员检查 Channels 页确认渠道齐 + 给每个 channel sync-models 勾启用的
3. 管理员检查 Pricing 页给每个启用的 model 设价
4. 前端模型选择器应该已经列出新模型；老的硬编码 model 列表（如果有）随版本一起删
5. 观察 ledger reason 出现 `chat_gpt-5@openai-official` 等带渠道标识就算上线成功

---

## 数据流总结图

```
用户 → POST /api/chat { model: "gpt-5", ... }
  ↓
lookup_pricing("gpt-5") → cost=3, kind="chat"
  ↓
try_deduct(user, 3, "chat_gpt-5") → balance -= 3
  ↓
list_channels_for_model("gpt-5")
  → [
      ChannelPick{id:1, name:"OpenAI官方", priority:10, upstream_model:"gpt-5"},
      ChannelPick{id:2, name:"Azure", priority:20, upstream_model:"gpt-5-2025-05"},
    ]
  ↓
try ch1 → 503 quota exceeded
  ↓
try ch2 → 200 OK → stream back
  ↓
ledger: delta=-3, reason="chat_gpt-5@Azure"
失败全部 → grant(+3, "refund_chat_gpt-5_all_failed") → 502 to user
```

---

## 风险与回滚

- **回滚方案：** 老 KV 不动，新表 drop 即可回到 v1（部署前打 snapshot）
- **风险点：**
  - migration 在 prod 已有数据上跑，三库各自语法差异（参考 `0011_credits.sql` 三库版差异作为模板）
  - fallback 重试可能放大请求量 —— 加 per-channel 短路 timeout（5s）
  - `try_deduct` + 多次 attempt 的 race：扣费一次 / 退款一次 / 真实调用 N 次。注意退款必须在「全部失败」分支精确触发一次
- **不在本期范围：** token 计费、用户级别限速、配额组、按用户分组定价（可后续 v2）
