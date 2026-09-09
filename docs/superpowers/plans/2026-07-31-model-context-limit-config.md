# 平台模型可配置上下文窗口 — 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 管理员可为平台模式的每个模型配置上下文窗口大小，聊天页占用率显示优先使用配置值，未配置回落关键词表。

**Architecture:** `model_pricing` 加 `context_limit` 可空列（迁移 0029×3 方言），channels.rs 的 CRUD 与用户侧 `/api/channels/models` 透传；管理端 PricingPanel 加输入框与列；ChatPage 平台聊天模式拉平台模型列表建 `model → context_limit` 映射，取上限时优先查映射。

**Tech Stack:** Rust/Axum + sqlx-any（三方言）、React 19 + TS、bun。

**Spec:** `docs/superpowers/specs/2026-07-31-model-context-limit-config-design.md`

## Global Constraints

- 迁移必须三方言齐备并在 `src/db.rs` 三个数组注册 id 29（当前最新 28）。
- 无测试框架：后端验证 = 仓库根 `cargo check`；前端验证 = `web/` 下 `bun run build` + `bun run lint`（lint 有 55 个存量 error，要求不新增）。
- 规整规则：`context_limit` 仅 `> 0` 有效，空/0/负数一律存 NULL / 前端回落。
- UI 文案中文；组件内局部函数不得以 `useXxx` 命名。
- 用 bun；`cargo check` 首次会顺带跑 bun build，正常。
- commit message 末尾统一追加：

  Generated with [Claude Code](https://claude.ai/code)
  via [Happy](https://happy.engineering)

  Co-Authored-By: Claude <noreply@anthropic.com>
  Co-Authored-By: Happy <yesreply@happy.engineering>

---

### Task 1: 后端 — 迁移 + channels.rs 全链路

**Files:**
- Create: `migrations/sqlite/0029_model_pricing_context.sql`
- Create: `migrations/mysql/0029_model_pricing_context.sql`
- Create: `migrations/postgres/0029_model_pricing_context.sql`
- Modify: `src/db.rs`（三个数组各加一行 id 29）
- Modify: `src/channels.rs`（`ModelPrice`、`list_pricing`、`get_price`、`PricingInput`、`upsert_price`、`PlatformModel`、`user_list_platform_models`）

**Interfaces:**
- Produces（后续任务依赖的 JSON 形状）：
  - `GET /api/admin/pricing` 每行多 `"context_limit": number|null`
  - `POST /api/admin/pricing` 接受可选 `"context_limit": number|null`
  - `GET /api/channels/models` 每行多 `"context_limit": number|null`

- [ ] **Step 1: 三个迁移文件**

`migrations/sqlite/0029_model_pricing_context.sql`：
```sql
ALTER TABLE model_pricing ADD COLUMN context_limit INTEGER NULL;
```
`migrations/mysql/0029_model_pricing_context.sql` 与 `migrations/postgres/0029_model_pricing_context.sql`（两个文件内容相同）：
```sql
ALTER TABLE model_pricing ADD COLUMN context_limit BIGINT NULL;
```

- [ ] **Step 2: 注册进 `src/db.rs`**

三个数组（sqlite 约 95 行附近、mysql 约 125 行、postgres 约 155 行，各自 `(28, …)` 之后）各追加：
```rust
    (29, include_str!("../migrations/sqlite/0029_model_pricing_context.sql")),
```
（mysql/postgres 数组里路径分别换成 `mysql`/`postgres`。）

- [ ] **Step 3: `ModelPrice` 结构加字段**

`src/channels.rs:42-50`：
```rust
pub struct ModelPrice {
    pub id: i64,
    pub model: String,
    pub kind: String,
    pub cost_credits: i64,
    pub display_name: Option<String>,
    pub enabled: bool,
    pub protocol: String,
    pub context_limit: Option<i64>,
}
```

- [ ] **Step 4: `list_pricing` / `get_price` 查询带出新列**

两处 SELECT 都改为（保持 `{enabled_col}` 写法）：
```rust
            "SELECT id, model, kind, cost_credits, display_name, {enabled_col}, protocol, context_limit \
             FROM model_pricing ORDER BY kind, model"
```
（`get_price` 对应 `… WHERE model = ?`。）行元组类型改为
```rust
(i64, String, String, i64, Option<String>, i64, String, Option<i64>)
```
map 闭包解构加 `context_limit` 并填入结构体（两处同样处理）：
```rust
        .map(|(id, model, kind_, cost_credits, display_name, enabled, protocol, context_limit)| ModelPrice {
            id,
            model,
            kind: kind_,
            cost_credits,
            display_name,
            enabled: enabled != 0,
            protocol,
            context_limit,
        })
```

- [ ] **Step 5: `PricingInput` + `upsert_price`**

`PricingInput` 加字段：
```rust
    #[serde(default)]
    pub context_limit: Option<i64>,
```
`upsert_price` 三条 SQL 的列清单与 VALUES 各加一列（`protocol` 之后、`updated_at` 之前）：
- Sqlite：`… enabled, protocol, context_limit, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, {now})`，UPDATE 子句加 `context_limit = excluded.context_limit,`
- Postgres：同上，`context_limit = EXCLUDED.context_limit,`
- Mysql：`context_limit = VALUES(context_limit),`

绑定链在 `.bind(&input.protocol)` 之后加规整绑定：
```rust
    let ctx: Option<i64> = input.context_limit.filter(|n| *n > 0);
    let q = q.bind(ctx);
```
（`let ctx` 放在函数体前部声明即可，位置不拘，编译通过为准。）

- [ ] **Step 6: 用户侧 `PlatformModel` 透传**

`src/channels.rs` 约 850 行的响应结构与填充处：
```rust
struct PlatformModel {
    model: String,
    display_name: Option<String>,
    kind: String, // "chat" | "image"
    cost_credits: i64,
    protocol: String, // top-priority channel's protocol
    context_limit: Option<i64>,
}
```
`out.push(PlatformModel { … })` 里加 `context_limit: p.context_limit,`。

- [ ] **Step 7: 验证**

Run: `cargo check`
Expected: 编译通过无 error（warning 不新增）。

- [ ] **Step 8: Commit**

```bash
git add migrations/sqlite/0029_model_pricing_context.sql migrations/mysql/0029_model_pricing_context.sql migrations/postgres/0029_model_pricing_context.sql src/db.rs src/channels.rs
git commit -m "feat(server): model_pricing 支持按模型配置 context_limit"
```

---

### Task 2: 管理端 — 类型与 PricingPanel 表单/列表

**Files:**
- Modify: `web/src/lib/channels.ts:75-92`（`ModelPrice` / `PricingInput` 类型）
- Modify: `web/src/pages/admin/PricingPanel.tsx`

**Interfaces:**
- Consumes: Task 1 的 admin pricing JSON（`context_limit: number|null`）。
- Produces: 无（终端 UI）。

- [ ] **Step 1: `channels.ts` 类型加字段**

`ModelPrice` 加 `context_limit: number | null`；`PricingInput` 加 `context_limit?: number | null`。

- [ ] **Step 2: PricingPanel 编辑对话框**

`initial` 两个分支分别加：
- edit 分支：`context_limit: mode.row.context_limit,`
- create 分支：`context_limit: null,`

表单里「积分/次」输入框（约 407-417 行）同一个 grid 内、其后追加一格：
```tsx
          <div>
            <Label>上下文 (tokens)</Label>
            <Input
              type="number"
              min={0}
              value={form.context_limit ?? ""}
              onChange={(e) =>
                setForm({
                  ...form,
                  context_limit: Number(e.target.value) > 0 ? Number(e.target.value) : null,
                })
              }
              placeholder="留空自动推断"
            />
          </div>
```

- [ ] **Step 3: `onToggle` 透传**

`onToggle`（约 65-79 行）的 `upsertPricing({...})` 参数加 `context_limit: r.context_limit,`（否则切换启用状态会把配置冲掉）。

- [ ] **Step 4: 列表加列**

表头「积分/次」列后加：
```tsx
              <th className="px-3 py-2 text-right font-medium">上下文</th>
```
行内对应位置（`{r.cost_credits}` 那个 td 之后）加：
```tsx
                <td className="px-3 py-2 text-right tabular-nums text-muted-foreground">
                  {r.context_limit != null
                    ? r.context_limit >= 1_000_000
                      ? `${(r.context_limit / 1_000_000).toFixed(r.context_limit % 1_000_000 === 0 ? 0 : 1)}M`
                      : r.context_limit >= 1000
                        ? `${(r.context_limit / 1000).toFixed(r.context_limit % 1000 === 0 ? 0 : 1)}K`
                        : String(r.context_limit)
                    : "自动"}
                </td>
```
两个 `colSpan={7}`（loading 与空态）改为 `colSpan={8}`。

- [ ] **Step 5: 验证**

Run: `cd web && bun run build && bun run lint`
Expected: build 通过；lint 不新增 error（基线 55）。

- [ ] **Step 6: Commit**

```bash
git add web/src/lib/channels.ts web/src/pages/admin/PricingPanel.tsx
git commit -m "feat(admin): 模型计费规则支持配置上下文窗口"
```

---

### Task 3: 聊天页 — 平台模型上下文映射

**Files:**
- Modify: `web/src/lib/platform-models.ts:7-13`（`PlatformModel` 类型）
- Modify: `web/src/pages/ChatPage.tsx`

**Interfaces:**
- Consumes: Task 1 的 `GET /api/channels/models` JSON；既有 `listPlatformModels("chat")`（ChatPage 已 import）；既有 `contextLimit`（来自 `@/lib/worker` re-export）与占用率显示块（约 1955-1975 行，`{!workerMode && (() => { const limit = contextLimit(settings.model) … })()}`）。

- [ ] **Step 1: `platform-models.ts` 类型加字段**

`PlatformModel` 加 `context_limit: number | null`。

- [ ] **Step 2: ChatPage 拉平台模型建映射**

在 ChatPage 组件顶层状态区（`chatUsageTokens` 声明附近，约 775 行）加：
```ts
  // 平台模式下按模型配置的上下文上限（model → context_limit），拉取失败静默回落关键词表。
  const [platformContextMap, setPlatformContextMap] = useState<Map<string, number>>(new Map())
```
在组件内（其他 useEffect 附近）加：
```ts
  useEffect(() => {
    if (settings.chatMode !== "platform") return
    let cancelled = false
    listPlatformModels("chat")
      .then((list) => {
        if (cancelled) return
        const map = new Map<string, number>()
        for (const m of list) {
          if (m.context_limit != null && m.context_limit > 0) map.set(m.model, m.context_limit)
        }
        setPlatformContextMap(map)
      })
      .catch(() => {
        // 静默：回落关键词表
      })
    return () => {
      cancelled = true
    }
  }, [settings.chatMode])
```
注意：`listPlatformModels` 已在文件顶部 import（第 38 行），无需新增 import。

- [ ] **Step 3: 显示块取上限时优先映射**

占用率显示块里的
```ts
                const limit = contextLimit(settings.model)
```
改为：
```ts
                const limit =
                  (settings.chatMode === "platform"
                    ? platformContextMap.get(settings.model)
                    : undefined) ?? contextLimit(settings.model)
```

- [ ] **Step 4: 验证**

Run: `cd web && bun run build && bun run lint`
Expected: build 通过；lint 不新增 error。

- [ ] **Step 5: Commit**

```bash
git add web/src/lib/platform-models.ts web/src/pages/ChatPage.tsx
git commit -m "feat(web): 聊天页上下文上限优先用平台模型配置值"
```

---

### Task 4: 端到端验证与发布

**Files:** 无代码改动。

- [ ] **Step 1: 合并 → 推送 → CI 构建镜像**（沿用仓库 merge 风格，push main 触发 docker workflow）

- [ ] **Step 2: 生产部署**：114.66.55.93 `/opt/Yunova/docker-compose.yml` 改 `image:` 为新 `sha-<merge短哈希>` tag → `docker compose pull yunova && docker compose up -d yunova`。启动时迁移 0029 自动应用。

- [ ] **Step 3: 运行时验证矩阵**
- 管理控制台 → 计费规则：新列「上下文」出现，编辑某 chat 模型填 1000000 保存，列表显示 `1M`。
- 聊天页平台模式选该模型：占用率显示 `/ 1M`。
- 清空该配置（输入框清空保存）：回落关键词表值。
- 切换启用/禁用开关：`context_limit` 不丢失。
- BYOK 模式：显示不受影响（仍关键词表）。
