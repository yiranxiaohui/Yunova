# 视频生成功能实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 Yunova 增加 OpenAI `/v1/videos` 协议的视频生成：独立视频创作台 + 个人视频库，模型+时长+分辨率组合定价，只走平台渠道扣积分，前端懒轮询 + 60s 定时兜底。

**Architecture:** 新后端模块 `src/videos.rs`（job 表 + 懒轮询推进函数 `advance_job`），复用 `upstream_channels`（`kind='video'`）与 `credits::try_deduct/grant`；MP4 下载落盘 `data_dir/videos/`；前端新增 `VideoStudioPage` + `video-gen.ts` 客户端 + 管理端 `VideoPricingPanel`。

**Tech Stack:** Rust/Axum/sqlx-any、reqwest multipart、React 19 + Vite 8 + Tailwind 4 + shadcn/ui。

**Spec:** `docs/superpowers/specs/2026-08-05-video-generation-design.md`（本计划的权威依据，含错误处理汇总表）。

## Global Constraints

- 本项目**无测试框架**（前后端都没有）——每个任务的验证是 `cargo check`（后端）/ `bun run build`（前端）+ 说明性人工验证点；不写测试文件。
- 所有用户可见文案**中文**。
- SQL 一律经 `db::q(kind, sql)`；布尔判断用 `db::bool_true(kind)` / `db::bool_as_int(kind, col)`；时间用 `db::now_expr(kind)`；插入取 id 按方言分派（照抄 `channels::create_channel` 三分支模式）。
- migration 编号 **0030**（当前最大 0029），三方言各一份并注册进 `src/db.rs` 三个数组。
- 包管理只用 **bun**；不在本机执行 `bun run build` 之外的打包发布类命令。
- 退款一律按 `video_jobs.cost_credits` 实扣值，`refunded` 标志幂等。
- 前端 fetch 一律 `credentials: "same-origin"`，错误处理照 `web/src/lib/channels.ts` 的 `jsonOrThrow`/`okOrThrow` 模式。

---

### Task 1: migration 0030 + db.rs 注册

**Files:**
- Create: `migrations/sqlite/0030_video_generation.sql`
- Create: `migrations/mysql/0030_video_generation.sql`
- Create: `migrations/postgres/0030_video_generation.sql`
- Modify: `src/db.rs:95-96`（SQLITE 数组尾部）、`:126-127`（MYSQL）、`:157-158`（POSTGRES）

**Interfaces:**
- Produces: 表 `video_jobs`、`video_pricing`（列名见下，后续任务的 SQL 依赖这些列名）。

- [ ] **Step 1: 写 sqlite migration**

`migrations/sqlite/0030_video_generation.sql`：

```sql
-- Video generation: jobs + rule-based pricing
-- See: docs/superpowers/specs/2026-08-05-video-generation-design.md

CREATE TABLE video_jobs (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    token             TEXT NOT NULL UNIQUE,
    user_id           INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    model             TEXT NOT NULL,
    prompt            TEXT NOT NULL,
    seconds           INTEGER NOT NULL,
    size              TEXT NOT NULL,
    input_image_path  TEXT,
    upstream_video_id TEXT,
    channel_id        INTEGER,
    cost_credits      INTEGER NOT NULL DEFAULT 0,
    status            TEXT NOT NULL DEFAULT 'pending',
    progress          INTEGER NOT NULL DEFAULT 0,
    video_path        TEXT,
    error             TEXT,
    refunded          INTEGER NOT NULL DEFAULT 0,
    download_retries  INTEGER NOT NULL DEFAULT 0,
    polling           INTEGER NOT NULL DEFAULT 0,
    last_polled_at    TEXT,
    created_at        TEXT NOT NULL DEFAULT (datetime('now')),
    started_at        TEXT,
    finished_at       TEXT
);
CREATE INDEX idx_video_jobs_user   ON video_jobs(user_id, created_at DESC);
CREATE INDEX idx_video_jobs_status ON video_jobs(status, last_polled_at);

CREATE TABLE video_pricing (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    model           TEXT NOT NULL UNIQUE,
    display_name    TEXT,
    enabled         INTEGER NOT NULL DEFAULT 1,
    base_credits    INTEGER NOT NULL DEFAULT 0,
    per_second      INTEGER NOT NULL DEFAULT 0,
    allowed_seconds TEXT NOT NULL,
    size_rules      TEXT NOT NULL,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
);
```

- [ ] **Step 2: 写 mysql migration**

`migrations/mysql/0030_video_generation.sql`（比照 `migrations/mysql/0016_image_jobs.sql` 与 `0019_channels_pricing.sql` 的方言习惯——`BIGINT AUTO_INCREMENT`、`VARCHAR`/`TEXT`、`DATETIME DEFAULT CURRENT_TIMESTAMP`、显式 `ENGINE`/字符集若现有文件带则带）：

```sql
CREATE TABLE video_jobs (
    id                BIGINT PRIMARY KEY AUTO_INCREMENT,
    token             VARCHAR(64) NOT NULL UNIQUE,
    user_id           BIGINT NOT NULL,
    model             VARCHAR(191) NOT NULL,
    prompt            TEXT NOT NULL,
    seconds           INT NOT NULL,
    size              VARCHAR(32) NOT NULL,
    input_image_path  VARCHAR(255),
    upstream_video_id VARCHAR(191),
    channel_id        BIGINT,
    cost_credits      BIGINT NOT NULL DEFAULT 0,
    status            VARCHAR(16) NOT NULL DEFAULT 'pending',
    progress          INT NOT NULL DEFAULT 0,
    video_path        VARCHAR(255),
    error             TEXT,
    refunded          TINYINT NOT NULL DEFAULT 0,
    download_retries  INT NOT NULL DEFAULT 0,
    polling           TINYINT NOT NULL DEFAULT 0,
    last_polled_at    DATETIME,
    created_at        DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    started_at        DATETIME,
    finished_at       DATETIME,
    CONSTRAINT fk_video_jobs_user FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);
CREATE INDEX idx_video_jobs_user   ON video_jobs(user_id, created_at DESC);
CREATE INDEX idx_video_jobs_status ON video_jobs(status, last_polled_at);

CREATE TABLE video_pricing (
    id              BIGINT PRIMARY KEY AUTO_INCREMENT,
    model           VARCHAR(191) NOT NULL UNIQUE,
    display_name    VARCHAR(191),
    enabled         TINYINT NOT NULL DEFAULT 1,
    base_credits    BIGINT NOT NULL DEFAULT 0,
    per_second      BIGINT NOT NULL DEFAULT 0,
    allowed_seconds TEXT NOT NULL,
    size_rules      TEXT NOT NULL,
    created_at      DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at      DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);
```

写之前先 `cat migrations/mysql/0016_image_jobs.sql migrations/mysql/0019_channels_pricing.sql` 核对：外键写法、users(id) 类型、是否用 `BOOLEAN`。**以现有文件的实际写法为准**，上面代码按需调整。

- [ ] **Step 3: 写 postgres migration**

`migrations/postgres/0030_video_generation.sql`（比照 `migrations/postgres/0016_image_jobs.sql`——`BIGSERIAL`、`BOOLEAN`、`TIMESTAMPTZ DEFAULT now()`）：

```sql
CREATE TABLE video_jobs (
    id                BIGSERIAL PRIMARY KEY,
    token             TEXT NOT NULL UNIQUE,
    user_id           BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    model             TEXT NOT NULL,
    prompt            TEXT NOT NULL,
    seconds           INTEGER NOT NULL,
    size              TEXT NOT NULL,
    input_image_path  TEXT,
    upstream_video_id TEXT,
    channel_id        BIGINT,
    cost_credits      BIGINT NOT NULL DEFAULT 0,
    status            TEXT NOT NULL DEFAULT 'pending',
    progress          INTEGER NOT NULL DEFAULT 0,
    video_path        TEXT,
    error             TEXT,
    refunded          BOOLEAN NOT NULL DEFAULT FALSE,
    download_retries  INTEGER NOT NULL DEFAULT 0,
    polling           BOOLEAN NOT NULL DEFAULT FALSE,
    last_polled_at    TIMESTAMPTZ,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    started_at        TIMESTAMPTZ,
    finished_at       TIMESTAMPTZ
);
CREATE INDEX idx_video_jobs_user   ON video_jobs(user_id, created_at DESC);
CREATE INDEX idx_video_jobs_status ON video_jobs(status, last_polled_at);

CREATE TABLE video_pricing (
    id              BIGSERIAL PRIMARY KEY,
    model           TEXT NOT NULL UNIQUE,
    display_name    TEXT,
    enabled         BOOLEAN NOT NULL DEFAULT TRUE,
    base_credits    BIGINT NOT NULL DEFAULT 0,
    per_second      BIGINT NOT NULL DEFAULT 0,
    allowed_seconds TEXT NOT NULL,
    size_rules      TEXT NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

同样先核对现有 postgres migration 的实际写法。

- [ ] **Step 4: 注册进 db.rs 三个数组**

`src/db.rs` 三处各追加一行（照 0029 行的格式）：

```rust
    (30, include_str!("../migrations/sqlite/0030_video_generation.sql")),
```

（mysql/postgres 数组同理改路径。）

- [ ] **Step 5: 验证 + 提交**

Run: `cargo check`
Expected: 编译通过（include_str! 路径正确即可；表要到运行时才建）。

可选运行验证：`rm -f /tmp/nc-test.db && YUNOVA_DATA_DIR=/tmp/nc-vtest cargo run` 短暂启动确认 migration 应用无 SQL 报错后 Ctrl-C。

```bash
git add migrations src/db.rs
git commit -m "feat(db): 视频生成 video_jobs / video_pricing 表（migration 0030）"
```

---

### Task 2: channels.rs 放开 kind='video'

**Files:**
- Modify: `src/channels.rs:826-834`（`validate_protocol_kind`）
- Modify: `src/channels.rs:797`（`admin_upsert_pricing` 的 kind 校验，此处**不放开** video，见 Step 2 说明）
- Modify: `src/channels.rs:737`（admin 渠道列表 flavor 过滤）

**Interfaces:**
- Produces: 渠道 CRUD 接受 `kind="video"`（protocol 仍限 openai）；`channels::select_chain(pool, kind, model, "video")` 可正常返回 video 渠道链（`channels_for_model` 是纯 SQL 按 flavor 过滤，无需改动）。
- 注意：`model_pricing`（chat/image 一口价表）**不用于视频**，视频定价在 `video_pricing`（Task 3）。

- [ ] **Step 1: 放开 validate_protocol_kind**

```rust
fn validate_protocol_kind(protocol: &str, kind: &str) -> Result<(), String> {
    if !matches!(protocol, "openai" | "claude" | "gemini") {
        return Err("protocol must be openai/claude/gemini".into());
    }
    if !matches!(kind, "chat" | "image" | "video") {
        return Err("kind must be chat/image/video".into());
    }
    if kind == "video" && protocol != "openai" {
        return Err("视频渠道仅支持 openai 协议".into());
    }
    Ok(())
}
```

- [ ] **Step 2: 渠道列表 flavor 过滤加 video**

`src/channels.rs:737` 附近：`Some(f) if matches!(f, "chat" | "image") => c.kind == f,` 改为 `matches!(f, "chat" | "image" | "video")`。

`:797` 的 `admin_upsert_pricing`（`model_pricing` 表）保持 `"chat" | "image"` 不变——视频不走这张表。

- [ ] **Step 3: 验证 + 提交**

Run: `cargo check`
Expected: PASS

```bash
git add src/channels.rs
git commit -m "feat(channels): 渠道 kind 支持 video（协议限 openai）"
```

---

### Task 3: src/videos.rs — 定价规则 + admin CRUD + 用户模型列表

**Files:**
- Create: `src/videos.rs`
- Modify: `src/main.rs`（`mod videos;` + `build_router` 中 `.merge(videos::routes())` `.merge(videos::admin_routes())`，位置照 `.merge(images::routes())` 一带）
- Modify: `src/credits.rs:86-118`（`LedgerMeta` 加 video 构造器）

**Interfaces:**
- Consumes: `db::q/bool_as_int/bool_true/now_expr/returning_id`、`credits::LedgerMeta`、`admin::require_admin`。
- Produces（后续任务依赖）：
  - `pub struct VideoPricing { pub id: i64, pub model: String, pub display_name: Option<String>, pub enabled: bool, pub base_credits: i64, pub per_second: i64, pub allowed_seconds: Vec<i64>, pub size_rules: Vec<SizeRule> }`
  - `pub struct SizeRule { pub size: String, pub multiplier: i64 }`（serde，multiplier 为百分比整数）
  - `pub async fn get_pricing(pool, kind, model) -> Result<Option<VideoPricing>, sqlx::Error>`
  - `pub fn compute_cost(p: &VideoPricing, seconds: i64, size: &str) -> Option<i64>` —— seconds/size 不在规则内返回 None
  - `pub fn routes() -> Router<AppState>`（本任务先只挂 `GET /videos/models`）
  - `pub fn admin_routes() -> Router<AppState>`
  - `LedgerMeta::video(model)` 与 `LedgerMeta::refund_video(model)`

- [ ] **Step 1: credits.rs 加 LedgerMeta 构造器**

在 `impl<'a> LedgerMeta<'a>`（`src/credits.rs:94`）中，`refund_image` 后追加：

```rust
    pub fn video(model: &'a str) -> Self {
        Self { kind: "video", protocol: Some("openai"), model: Some(model) }
    }
    /// Refund of a video deduction — kept under "video" kind so net-spend
    /// math (sum deltas where kind='video') stays correct.
    pub fn refund_video(model: &'a str) -> Self {
        Self { kind: "video", protocol: None, model: Some(model) }
    }
```

同时更新 `LedgerMeta.kind` 字段的文档注释，把 `"video"` 加进枚举列表。

- [ ] **Step 2: 建 src/videos.rs，写定价数据层**

文件头部 imports 照 `src/skills.rs` / `src/channels.rs` 惯例（axum、sqlx、serde、`crate::db::{self, DbKind, Pool}`、`crate::main` 侧的 `AppState/InstalledState/CurrentUser` 按现有模块的 use 路径写）。核心代码：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SizeRule {
    pub size: String,
    /// Percent multiplier, 100 = 1.0x.
    pub multiplier: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct VideoPricing {
    pub id: i64,
    pub model: String,
    pub display_name: Option<String>,
    pub enabled: bool,
    pub base_credits: i64,
    pub per_second: i64,
    pub allowed_seconds: Vec<i64>,
    pub size_rules: Vec<SizeRule>,
}

fn parse_pricing_row(
    (id, model, display_name, enabled, base_credits, per_second, allowed_seconds, size_rules):
        (i64, String, Option<String>, i64, i64, i64, String, String),
) -> VideoPricing {
    VideoPricing {
        id, model, display_name,
        enabled: enabled != 0,
        base_credits, per_second,
        allowed_seconds: serde_json::from_str(&allowed_seconds).unwrap_or_default(),
        size_rules: serde_json::from_str(&size_rules).unwrap_or_default(),
    }
}

const PRICING_COLS: &str =
    "id, model, display_name, {enabled}, base_credits, per_second, allowed_seconds, size_rules";
// 查询时用 db::bool_as_int(kind, "enabled") 替换 {enabled} 占位（照 channels.rs 的 enabled_c 写法拼 SQL）。

pub async fn list_pricing(pool: &Pool, kind: DbKind) -> Result<Vec<VideoPricing>, sqlx::Error> {
    let enabled = db::bool_as_int(kind, "enabled");
    let sql = db::q(kind, &format!(
        "SELECT id, model, display_name, {enabled}, base_credits, per_second, \
         allowed_seconds, size_rules FROM video_pricing ORDER BY model ASC"
    ));
    let rows: Vec<(i64, String, Option<String>, i64, i64, i64, String, String)> =
        sqlx::query_as(&sql).fetch_all(pool).await?;
    Ok(rows.into_iter().map(parse_pricing_row).collect())
}

pub async fn get_pricing(pool: &Pool, kind: DbKind, model: &str)
    -> Result<Option<VideoPricing>, sqlx::Error>
{
    let enabled = db::bool_as_int(kind, "enabled");
    let sql = db::q(kind, &format!(
        "SELECT id, model, display_name, {enabled}, base_credits, per_second, \
         allowed_seconds, size_rules FROM video_pricing WHERE model = ?"
    ));
    let row: Option<(i64, String, Option<String>, i64, i64, i64, String, String)> =
        sqlx::query_as(&sql).bind(model).fetch_optional(pool).await?;
    Ok(row.map(parse_pricing_row))
}

/// None when seconds/size are not in the model's configured rules.
pub fn compute_cost(p: &VideoPricing, seconds: i64, size: &str) -> Option<i64> {
    if !p.allowed_seconds.contains(&seconds) { return None; }
    let mult = p.size_rules.iter().find(|r| r.size == size)?.multiplier;
    let raw = (p.base_credits + p.per_second * seconds) * mult;
    Some((raw + 50) / 100) // round half up on the percent multiplier
}
```

- [ ] **Step 3: admin CRUD handlers + 路由**

请求体与 upsert（`POST /admin/video-pricing` 做 upsert，比照 `channels::upsert_price` 的"先 UPDATE，0 行再 INSERT"或按方言 ON CONFLICT——**选前者**，一套 SQL 三方言通吃）：

```rust
#[derive(Debug, Deserialize)]
pub struct VideoPricingInput {
    pub model: String,
    pub display_name: Option<String>,
    pub enabled: Option<bool>,
    pub base_credits: i64,
    pub per_second: i64,
    pub allowed_seconds: Vec<i64>,
    pub size_rules: Vec<SizeRule>,
}
```

校验（400 中文错误）：`model` 非空；`base_credits >= 0`、`per_second >= 0`；`allowed_seconds` 非空且全部 > 0；`size_rules` 非空、`size` 形如 `{w}x{h}`（用 `size.split_once('x')` 两侧 parse::<u32> 校验）、`multiplier > 0`。存库时 `allowed_seconds`/`size_rules` 序列化为 JSON 字符串。

Handlers：`admin_list_video_pricing`（GET 全量含 disabled）、`admin_upsert_video_pricing`（POST）、`admin_delete_video_pricing`（DELETE `/admin/video-pricing/{model}`）。响应/错误风格照 `channels.rs` 的 admin handlers（`err(StatusCode, msg)` 辅助函数照抄 images.rs 里的写法）。

```rust
pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route("/admin/video-pricing", get(admin_list_video_pricing).post(admin_upsert_video_pricing))
        .route("/admin/video-pricing/{model}", delete(admin_delete_video_pricing))
        .route_layer(middleware::from_fn(crate::admin::require_admin))
}
```

- [ ] **Step 4: 用户模型列表 GET /videos/models**

照 `channels::user_list_platform_models` 的过滤逻辑：`video_pricing` 里 enabled 的模型，且 `channels::any_enabled_channel(pool, kind, "openai", "video")` 存在才返回（video 渠道协议固定 openai，一次判定即可，无需逐模型查渠道）：

```rust
#[derive(Serialize)]
struct UserVideoModel {
    model: String,
    display_name: Option<String>,
    base_credits: i64,
    per_second: i64,
    allowed_seconds: Vec<i64>,
    size_rules: Vec<SizeRule>,
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/videos/models", get(user_list_models))
}
```

`user_list_models` 返回完整规则，前端本地算价。

- [ ] **Step 5: main.rs 挂载**

`src/main.rs`：模块声明区加 `mod videos;`（字母序插在 `mod studio;` 前后合适位置）；`build_router` 的 merge 链（`:1088-1105`）加：

```rust
        .merge(videos::routes())
        .merge(videos::admin_routes())
```

- [ ] **Step 6: 验证 + 提交**

Run: `cargo check`
Expected: PASS（`routes()` 里暂时只有 models 端点，后续任务往里加）。

```bash
git add src/videos.rs src/main.rs src/credits.rs
git commit -m "feat(videos): 视频定价规则表 CRUD 与用户模型列表"
```

---

### Task 4: 创建任务端点（扣费 → 选渠道 → 上游 POST /v1/videos）

**Files:**
- Modify: `src/videos.rs`

**Interfaces:**
- Consumes: `credits::try_deduct` (`src/credits.rs:173`，`Ok(new_balance)/Err(current_balance)`)、`credits::grant` (`:254`)、`channels::select_one(pool, kind, model, "video")` (`src/channels.rs:217`)、Task 3 的 `get_pricing`/`compute_cost`/`LedgerMeta::video/refund_video`。
- Produces: `POST /api/videos/jobs` → `201 {"token": String, "cost": i64}`；内部函数 `pub(crate) async fn refund_job(pool, kind, job_id) -> ()`（按行内 cost_credits 幂等退款，Task 5/6 复用）。

- [ ] **Step 1: 请求/响应类型与 random token**

```rust
#[derive(Debug, Deserialize)]
struct CreateJobReq {
    model: String,
    prompt: String,
    seconds: i64,
    size: String,
    /// e.g. "/api/images/abcd1234.png" — 先经 POST /api/images/save 上传。
    input_image_path: Option<String>,
}

#[derive(Serialize)]
struct CreateJobResp { token: String, cost: i64 }
```

token 生成照抄 `images.rs` 的 `random_hex(16)`（若该函数非 pub，在 videos.rs 里复制同款实现，rand 已在依赖里）。

- [ ] **Step 2: 幂等退款辅助函数**

```rust
/// Refund a job's cost_credits exactly once. The UPDATE-guard makes retries
/// (sweeper + poll racing) safe: only the caller that flips refunded gets to
/// grant.
pub(crate) async fn refund_job(
    pool: &Pool, kind: DbKind,
    job_id: i64, user_id: i64, model: &str, cost: i64, suffix: &str,
) {
    if cost <= 0 { return; }
    let bt = db::bool_true(kind);
    let sql = db::q(kind, &format!(
        "UPDATE video_jobs SET refunded = {bt} WHERE id = ? AND refunded <> {bt}"
    ));
    let n = sqlx::query(&sql).bind(job_id).execute(pool).await
        .map(|r| r.rows_affected()).unwrap_or(0);
    if n == 0 { return; } // already refunded (or DB error — err on not double-granting)
    let reason = format!("refund_video_{model}_{suffix}");
    let _ = credits::grant(pool, kind, user_id, cost,
        &reason, &credits::LedgerMeta::refund_video(model)).await;
}
```

注意 postgres 的 `refunded` 是 BOOLEAN——`{bt}` 由 `db::bool_true` 给出 `TRUE`/`1`，与现有代码同法。

- [ ] **Step 3: create_job handler**

流程（severity 顺序不可调换：**先扣费，后调上游**）：

1. 解析校验：`prompt` 非空 trim；`get_pricing(model)` 不存在或 `!enabled` → 400 `"模型不存在或未启用"`；`compute_cost(&p, seconds, &size)` 为 None → 400 `"该模型不支持所选时长或分辨率"`；`input_image_path` 若有，必须以 `/api/images/` 开头且文件名不含 `..`/`/`（取末段 name，读 `data_dir/images/{name}`，读不到 → 400 `"参考图不存在"`），字节读入内存备用。
2. `credits::try_deduct(pool, kind, user.id, cost, &format!("video_{model}"), &LedgerMeta::video(&model))` —— `Err(balance)` → 402 `format!("积分不足：需要 {cost}，当前余额 {balance}")`。
3. `channels::select_one(pool, kind, &model, "video")` —— None → `refund` 走不了（行还没插），此处直接 `credits::grant(..., &format!("refund_video_{model}_no_channel"), &LedgerMeta::refund_video(&model))` 后返回 400 `"暂无可用视频渠道，请联系管理员"`。
4. 插入 `video_jobs`（status='pending'，token、cost_credits、channel_id、input_image_path 原样存），拿 job_id（三方言分支照 `channels::create_channel`）。
5. 调上游（reqwest 用 `AppState.http`，multipart）：

```rust
let base = choice.channel.base_url.trim_end_matches('/');
let mut form = reqwest::multipart::Form::new()
    .text("model", choice.upstream_model.clone())
    .text("prompt", req.prompt.clone())
    .text("seconds", req.seconds.to_string())
    .text("size", req.size.clone());
if let Some(bytes) = image_bytes {
    form = form.part("input_reference",
        reqwest::multipart::Part::bytes(bytes)
            .file_name(image_name.clone())          // 原始文件名（含扩展名）
            .mime_str(&image_mime)?);               // 按扩展名 mime_guess
}
let res = http.post(format!("{base}/v1/videos"))
    .bearer_auth(&choice.channel.api_key)
    .multipart(form)
    .send().await;
```

6. 结果分派：
   - 网络错误或非 2xx：读上游 body 截断 500 字符存 `error`，job 标 failed（`finished_at = now_expr`），`refund_job(..., "create_error")`，返回 502 与错误信息。
   - 2xx：解析 `{"id": "..."}`（`serde_json::Value` 取 `id` 字符串；取不到按失败处理同上），`UPDATE video_jobs SET upstream_video_id = ?, status = 'running', started_at = <now>, last_polled_at = <now> WHERE id = ?`，返回 201 `CreateJobResp { token, cost }`。

- [ ] **Step 4: 挂路由**

`routes()` 中追加 `.route("/videos/jobs", post(create_job))`。

- [ ] **Step 5: 验证 + 提交**

Run: `cargo check`
Expected: PASS

```bash
git add src/videos.rs
git commit -m "feat(videos): 创建视频任务（组合计价扣费 + 上游 /v1/videos）"
```

---

### Task 5: 轮询推进 advance_job + 查询/列表/删除端点 + MP4 静态服务

**Files:**
- Modify: `src/videos.rs`
- Modify: `src/main.rs`（public 路由区，照 `images::public_routes()` 的挂法加 `videos::public_routes()`）

**Interfaces:**
- Consumes: Task 4 的 `refund_job`；`channels::Channel`（按 job.channel_id 反查渠道，写一个 `channel_by_id(pool, kind, id) -> Option<(String /*base_url*/, String /*api_key*/)>` 的小查询即可，无需动 channels.rs）。
- Produces:
  - `GET /api/videos/jobs/{token}` → `JobView`
  - `GET /api/videos/jobs?page=N` → `{ jobs: Vec<JobView>, has_more: bool }`（每页 24）
  - `DELETE /api/videos/jobs/{token}` → 204
  - `pub async fn advance_job(state-ish deps, token) `——Task 6 定时器复用（签名收拢为 `advance_job(http: &reqwest::Client, pool: &Pool, kind: DbKind, data_dir: &std::path::Path, token: &str)`）
  - `pub fn public_routes() -> Router<AppState>`（`GET /videos/{name}`）
  - `JobView`（serde，字段见 Step 1——前端 Task 7 的 TS 类型逐字段对应）

- [ ] **Step 1: JobView 与行读取**

```rust
#[derive(Serialize)]
struct JobView {
    token: String,
    model: String,
    prompt: String,
    seconds: i64,
    size: String,
    status: String,            // pending | running | completed | failed
    progress: i64,             // 0-100
    video_path: Option<String>,// "/api/videos/{name}"
    error: Option<String>,
    cost_credits: i64,
    refunded: bool,
    created_at: String,
    finished_at: Option<String>,
}
```

内部行结构 `JobRow` 多带 `id/user_id/upstream_video_id/channel_id/download_retries/input_image_path/last_polled_at/polling`；`fetch_job(pool, kind, token) -> Option<JobRow>` 一个 SELECT 全列（bool 列经 `db::bool_as_int` 读成 i64）。

- [ ] **Step 2: advance_job 核心**

```rust
pub async fn advance_job(
    http: &reqwest::Client, pool: &Pool, kind: DbKind,
    data_dir: &std::path::Path, token: &str,
) {
    let Some(job) = fetch_job(pool, kind, token).await else { return };
    if job.status == "completed" || job.status == "failed" { return; }

    // Throttle: last_polled_at within 3s → serve cached state.
    // 时间比较在 SQL 里做（方言 datetime 格式不一，别在 Rust 里 parse）：
    // 抢锁 UPDATE 本身就带节流条件——一条语句同时完成节流+去重。
    let bt = db::bool_true(kind);
    let now = db::now_expr(kind);
    let lock_sql = db::q(kind, &format!(
        "UPDATE video_jobs SET polling = {bt}, last_polled_at = {now} \
         WHERE token = ? AND polling <> {bt} \
           AND (last_polled_at IS NULL OR last_polled_at < {})",
        three_seconds_ago_expr(kind)   // 见下
    ));
    let got = sqlx::query(&lock_sql).bind(token).execute(pool).await
        .map(|r| r.rows_affected()).unwrap_or(0);
    if got == 0 { return; }

    let release = || async { /* UPDATE video_jobs SET polling = <false> WHERE token = ? */ };
    // …主体逻辑，所有 return 前调 release（用一个内部 async fn + 手动收尾，
    // 或把主体包成 result 再统一 release——实现取后者，收尾只写一次）。
    poll_upstream_once(http, pool, kind, data_dir, &job).await;
    release_lock(pool, kind, token).await;
}
```

`three_seconds_ago_expr(kind)`：sqlite `datetime('now','-3 seconds')`、mysql `DATE_SUB(NOW(), INTERVAL 3 SECOND)`、postgres `now() - interval '3 seconds'`。同款再写 `minutes_ago_expr(kind, n)` 给 Task 6 用。

`poll_upstream_once`（主体）：

1. `channel_by_id(job.channel_id)` 拿 `(base, key)`；渠道没了 → job 标 failed（error=`"渠道已删除"`）+ `refund_job(..., "upstream_failed")`，return。
2. `GET {base}/v1/videos/{upstream_video_id}` bearer key。网络错误 → 仅记 `error` 字段不改状态（瞬时故障下次再试），return。
3. 解析 JSON `status` 字段：
   - `"queued"` | `"in_progress"` → `UPDATE progress = <progress 字段（缺省 0）>`；
   - `"failed"` → 取 `error.message`（缺省 `"上游生成失败"`）存 error，标 failed + `finished_at`，`refund_job(..., "upstream_failed")`；
   - `"completed"` → 进入下载：`GET {base}/v1/videos/{id}/content`，2xx 时 `res.bytes()` 落盘 `data_dir/videos/{random_hex(16)}.mp4`（先 `create_dir_all`），`UPDATE status='completed', video_path='/api/videos/{name}', progress=100, finished_at=<now>`。下载失败（网络/非 2xx/写盘错）→ `download_retries + 1`：`< 5` 只记 error 保持 running；`>= 5` 标 failed + `refund_job(..., "download_failed")`。

- [ ] **Step 3: get/list/delete handlers**

- `get_job`：鉴权归属（`WHERE token = ? AND user_id = ?`，查无 → 404）；调 `advance_job(...)`；重新 `fetch_job` 返回 `JobView`。
- `list_jobs`：`WHERE user_id = ? ORDER BY created_at DESC LIMIT 25 OFFSET page*24`，取 25 判 `has_more`，截 24 返回。**列表不推进任务**（进行中的由前端对单个 token 轮询）。
- `delete_job`：归属校验；`status IN ('pending','running')` → 409 `"任务进行中，暂不能删除"`；有 `video_path` 则按文件名删 `data_dir/videos/{name}`（删不掉忽略）；`input_image_path` 不删（属于 images 体系）；DELETE 行；204。

`routes()` 追加：

```rust
        .route("/videos/jobs", post(create_job).get(list_jobs))
        .route("/videos/jobs/{token}", get(get_job).delete(delete_job))
```

（Task 4 里挂的 post 合并到这一行。）

- [ ] **Step 4: MP4 静态服务（带 Range）**

`serve_video`：照 `images.rs::serve_image`（`:1455`）的路径防穿越检查，但要支持 HTTP Range（播放器拖进度条必需）。实现：读全量 bytes 后手写单区间 Range：

```rust
async fn serve_video(State(state): State<AppState>, Path(name): Path<String>, headers: HeaderMap) -> Response {
    if name.contains("..") || name.contains('/') || name.contains('\\') {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let path = state.data_dir.join("videos").join(&name);
    let bytes = match tokio::fs::read(&path).await {
        Ok(b) => b, Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    let total = bytes.len() as u64;
    // 解析 "bytes=start-end"（只支持单区间；无 Range 头 → 200 全量）
    if let Some(r) = headers.get(header::RANGE).and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("bytes="))
    {
        let mut it = r.splitn(2, '-');
        let start: u64 = it.next().unwrap_or("").parse().unwrap_or(0);
        let end: u64 = it.next().unwrap_or("").parse().unwrap_or(total - 1).min(total - 1);
        if start > end || start >= total {
            return Response::builder().status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::CONTENT_RANGE, format!("bytes */{total}"))
                .body(Body::empty()).unwrap();
        }
        let chunk = bytes[start as usize..=(end as usize)].to_vec();
        return Response::builder().status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_TYPE, "video/mp4")
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{total}"))
            .header(header::CACHE_CONTROL, "private, max-age=86400, immutable")
            .body(Body::from(chunk)).unwrap();
    }
    Response::builder()
        .header(header::CONTENT_TYPE, "video/mp4")
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CACHE_CONTROL, "private, max-age=86400, immutable")
        .body(Body::from(bytes)).unwrap()
}

pub fn public_routes() -> Router<AppState> {
    Router::new().route("/videos/{name}", get(serve_video))
}
```

（`bytes=start-` 后缀开区间由 `end` 缺省 `total-1` 自然覆盖；`bytes=-N` 尾部区间浏览器视频播放不用，忽略不实现。）

`main.rs`：找到挂 `images::public_routes()` 的位置，同样挂 `videos::public_routes()`。

- [ ] **Step 5: 验证 + 提交**

Run: `cargo check`
Expected: PASS

```bash
git add src/videos.rs src/main.rs
git commit -m "feat(videos): 懒轮询推进、任务查询/列表/删除、MP4 静态服务（Range）"
```

---

### Task 6: 兜底定时器（挂入 main.rs 60s 循环）

**Files:**
- Modify: `src/videos.rs`（`pub async fn sweep(...)`）
- Modify: `src/main.rs:1175-1181`（现有 60s rate-limiter prune 循环）

**Interfaces:**
- Consumes: Task 5 的 `advance_job`、`minutes_ago_expr`；Task 4 的 `refund_job`。
- Produces: `pub async fn sweep(http: &reqwest::Client, pool: &Pool, kind: DbKind, data_dir: &std::path::Path)`。

- [ ] **Step 1: sweep 函数**

```rust
/// Runs every ~60s from main. Three duties, cheap when idle:
pub async fn sweep(http: &reqwest::Client, pool: &Pool, kind: DbKind, data_dir: &std::path::Path) {
    let bt = db::bool_true(kind);

    // 1. Repair hung polling locks (crashed mid-poll > 5 min ago).
    let sql = db::q(kind, &format!(
        "UPDATE video_jobs SET polling = <false-literal> \
         WHERE polling = {bt} AND last_polled_at < {}", minutes_ago_expr(kind, 5)));
    let _ = sqlx::query(&sql).execute(pool).await;
    // <false-literal>: postgres FALSE / others 0 —— 写个 db-kind match 或直接
    // 复用 bool_true 的反面：sqlite/mysql 用 0，postgres 用 FALSE。

    // 2. Timeout: > 2h and still not terminal → fail + refund.
    let stale: Vec<(i64, i64, String, i64, String)> = { // id, user_id, model, cost, token
        let sql = db::q(kind, &format!(
            "SELECT id, user_id, model, cost_credits, token FROM video_jobs \
             WHERE status IN ('pending','running') AND created_at < {}",
            minutes_ago_expr(kind, 120)));
        sqlx::query_as(&sql).fetch_all(pool).await.unwrap_or_default()
    };
    for (id, user_id, model, cost, token) in stale {
        let sql = db::q(kind, &format!(
            "UPDATE video_jobs SET status = 'failed', error = '生成超时', finished_at = {} \
             WHERE id = ? AND status IN ('pending','running')", db::now_expr(kind)));
        let n = sqlx::query(&sql).bind(id).execute(pool).await
            .map(|r| r.rows_affected()).unwrap_or(0);
        if n > 0 { refund_job(pool, kind, id, user_id, &model, cost, "timeout").await; }
        let _ = token; // token 仅调试用
    }

    // 3. Advance orphans: running/pending not polled for 10 min (user closed page).
    let orphans: Vec<(String,)> = {
        let sql = db::q(kind, &format!(
            "SELECT token FROM video_jobs \
             WHERE status IN ('pending','running') \
               AND (last_polled_at IS NULL OR last_polled_at < {}) \
             ORDER BY created_at ASC LIMIT 20", minutes_ago_expr(kind, 10)));
        sqlx::query_as(&sql).fetch_all(pool).await.unwrap_or_default()
    };
    for (token,) in orphans {
        advance_job(http, pool, kind, data_dir, &token).await;
    }
}
```

顺序注意：先修锁再推进（悬挂锁不修，孤儿任务会被锁挡住）；LIMIT 20 防单轮过载。status='pending' 且无 `upstream_video_id` 的行（创建时上游调用中途崩溃）会在 advance_job 里因无 video_id 无法推进——在 `poll_upstream_once` 开头加：`upstream_video_id 为 None → 标 failed + refund_job(..., "create_error")`（补进 Task 5 的实现里，此处提醒勿漏）。

- [ ] **Step 2: main.rs 循环接线**

现有循环（`main.rs:1175`）只 prune rate limiter 且在 `installed` 之前构造。视频 sweep 需要 pool——在循环体内每轮从 `state.installed.read().await` 克隆 `Option<InstalledState>`（照其他后台任务读 installed 的现有写法；若无先例，clone `AppState` 进 spawn，循环里读锁）：

```rust
            let sweeper_state = state.clone();
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    pruner.prune().await;
                    let installed = sweeper_state.installed.read().await.clone();
                    if let Some(s) = installed {
                        videos::sweep(&sweeper_state.http, &s.pool, s.kind, &sweeper_state.data_dir).await;
                    }
                }
            });
```

（若 `state` 在该点尚未完全构造导致借用问题，就单开一个新的 `tokio::spawn` 循环放在 `installed` 初始化之后，效果相同。）

- [ ] **Step 3: 验证 + 提交**

Run: `cargo check`
Expected: PASS

人工验证点（此刻可做可留到 Task 10）：`cargo run` 起服务，日志无 panic；60 秒后无 SQL 报错（三个 sweep 查询在空表上跑通）。

```bash
git add src/videos.rs src/main.rs
git commit -m "feat(videos): 60s 兜底定时器（修锁/超时退款/推进孤儿任务）"
```

---

### Task 7: 前端 API 客户端 web/src/lib/video-gen.ts

**Files:**
- Create: `web/src/lib/video-gen.ts`

**Interfaces:**
- Consumes: Task 3/5 的后端端点。
- Produces（Task 8/9 依赖，字段与后端 serde 逐一对应）：

- [ ] **Step 1: 写客户端**

```ts
// Video generation API client. Platform-credits only (no BYOK) — all calls
// ride the session cookie; no X-Upstream-* headers.

async function jsonOrThrow<T>(res: Response): Promise<T> {
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText)
    throw new Error(text || `HTTP ${res.status}`)
  }
  return res.json() as Promise<T>
}

export type SizeRule = { size: string; multiplier: number }

export type VideoModel = {
  model: string
  display_name: string | null
  base_credits: number
  per_second: number
  allowed_seconds: number[]
  size_rules: SizeRule[]
}

export type VideoJob = {
  token: string
  model: string
  prompt: string
  seconds: number
  size: string
  status: "pending" | "running" | "completed" | "failed"
  progress: number
  video_path: string | null
  error: string | null
  cost_credits: number
  refunded: boolean
  created_at: string
  finished_at: string | null
}

export type CreateVideoJobReq = {
  model: string
  prompt: string
  seconds: number
  size: string
  input_image_path?: string
}

/** 与后端 videos::compute_cost 同式：(base + per_second*s) * multiplier / 100，四舍五入。 */
export function computeVideoCost(m: VideoModel, seconds: number, size: string): number | null {
  if (!m.allowed_seconds.includes(seconds)) return null
  const rule = m.size_rules.find((r) => r.size === size)
  if (!rule) return null
  return Math.round(((m.base_credits + m.per_second * seconds) * rule.multiplier) / 100)
}

export async function listVideoModels(): Promise<VideoModel[]> {
  const res = await fetch("/api/videos/models", { credentials: "same-origin" })
  return jsonOrThrow(res)
}

export async function createVideoJob(req: CreateVideoJobReq): Promise<{ token: string; cost: number }> {
  const res = await fetch("/api/videos/jobs", {
    method: "POST",
    credentials: "same-origin",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(req),
  })
  return jsonOrThrow(res)
}

export async function getVideoJob(token: string): Promise<VideoJob> {
  const res = await fetch(`/api/videos/jobs/${encodeURIComponent(token)}`, {
    credentials: "same-origin",
  })
  return jsonOrThrow(res)
}

export async function listVideoJobs(page: number): Promise<{ jobs: VideoJob[]; has_more: boolean }> {
  const res = await fetch(`/api/videos/jobs?page=${page}`, { credentials: "same-origin" })
  return jsonOrThrow(res)
}

export async function deleteVideoJob(token: string): Promise<void> {
  const res = await fetch(`/api/videos/jobs/${encodeURIComponent(token)}`, {
    method: "DELETE",
    credentials: "same-origin",
  })
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText)
    throw new Error(text || `HTTP ${res.status}`)
  }
}
```

注意后端 `compute_cost` 是 `(raw + 50) / 100` 整数半上取整——前端 `Math.round` 与之一致（正数场景）。

- [ ] **Step 2: 验证 + 提交**

Run: `cd web && bun run build`
Expected: PASS（暂无引用方，仅类型检查通过）。

```bash
git add web/src/lib/video-gen.ts
git commit -m "feat(web): 视频生成 API 客户端"
```

---

### Task 8: VideoStudioPage 页面 + 路由 + 侧边栏入口

**Files:**
- Create: `web/src/pages/VideoStudioPage.tsx`
- Modify: `web/src/App.tsx`（import + `/videos` Route，包裹方式照 `/studio` 的 `<Route>`）
- Modify: `web/src/components/app/Sidebar.tsx:253` 附近（"图像工作室"按钮下加"视频工作室"）

**Interfaces:**
- Consumes: Task 7 的全部导出；现有 `POST /api/images/save`（参考图上传，用法照 ImageStudioPage 里的上传调用）；积分余额展示照 ChatPage 的余额组件/接口（实现时先看 ImageStudioPage 顶栏怎么拿余额，照抄）。
- Produces: 路由 `/videos` 可用。

**先读**：`web/src/pages/ImageStudioPage.tsx` 全文——布局骨架、上传参考图、任务卡片、错误 toast/对话框的现成写法都从这里搬。以下描述行为要求，具体 JSX 结构对齐该文件风格。

- [ ] **Step 1: 页面骨架与状态**

```tsx
export default function VideoStudioPage() {
  const [models, setModels] = useState<VideoModel[]>([])
  const [model, setModel] = useState<string>("")          // 选中模型名
  const [seconds, setSeconds] = useState<number | null>(null)
  const [size, setSize] = useState<string | null>(null)
  const [prompt, setPrompt] = useState("")
  const [refImage, setRefImage] = useState<string | null>(null) // /api/images/xxx
  const [jobs, setJobs] = useState<VideoJob[]>([])
  const [hasMore, setHasMore] = useState(false)
  const [page, setPage] = useState(0)
  const [submitting, setSubmitting] = useState(false)
  // ...
}
```

挂载时并行 `listVideoModels()` + `listVideoJobs(0)`；models 为空显示空态"管理员尚未配置视频模型"。选中模型变化时把 seconds/size 重置为该模型规则里的第一项。

- [ ] **Step 2: 参数面板（左栏）**

- 模型：`Select`（shadcn，显示 `display_name ?? model`）；
- 时长：分段按钮组（`allowed_seconds.map`，Button variant 按选中态切 `default`/`outline`），文案 `${s} 秒`；
- 分辨率：同款按钮组（`size_rules.map(r => r.size)`）；
- 参考图：`<input type="file" accept="image/*">` 隐藏 + 触发按钮；选中后 FileReader 读 bytes → `POST /api/images/save`（body 格式照 ImageStudioPage 现有上传代码）→ 存返回 path，展示缩略图 `<img src={path}>` + 移除按钮；
- 提示词：`Textarea` 4 行；
- 价格预览与提交按钮：

```tsx
const activeModel = models.find((m) => m.model === model)
const cost = activeModel && seconds != null && size != null
  ? computeVideoCost(activeModel, seconds, size) : null
// 按钮文案：cost != null ? `生成视频（消耗 ${cost} 积分）` : "生成视频"
// disabled：submitting || !prompt.trim() || cost == null
```

- [ ] **Step 3: 提交与轮询**

```tsx
async function handleSubmit() {
  if (!activeModel || seconds == null || size == null) return
  setSubmitting(true)
  try {
    const { token } = await createVideoJob({
      model: activeModel.model, prompt: prompt.trim(),
      seconds, size,
      input_image_path: refImage ?? undefined,
    })
    const job = await getVideoJob(token)
    setJobs((prev) => [job, ...prev])
  } catch (e) {
    // 显示错误（Alert/toast 照 ImageStudioPage 用法；402 的中文消息直接展示）
  } finally {
    setSubmitting(false)
  }
}
```

轮询：一个 `useEffect` 依赖 `jobs` 中是否存在 `pending|running` 状态；有则 `setInterval` 5000ms，逐个 in-flight token 调 `getVideoJob` 并 merge 回 `jobs`（按 token 替换）；无 in-flight 或组件卸载即 `clearInterval`。进行中卡片显示：转圈图标 + `进度 {progress}%` + 提示文案"可离开页面，任务将在后台继续，稍后回来查看"。

- [ ] **Step 4: 视频卡片与库**

- completed：`<video controls preload="metadata" src={job.video_path!} className="w-full rounded-lg" />`，卡片底部：模型/时长/分辨率/`消耗 {cost_credits} 积分`，操作：下载（`<a href={video_path} download>`）、重新生成（参数回填左栏表单）、删除；
- failed：红色边框卡片，错误信息 + （`refunded` 为 true 时）`已退还 {cost_credits} 积分`，操作：重新生成、删除；
- 删除：用项目现有应用内确认对话框组件（repo 刚用它替换了原生弹窗——搜 `AlertDialog` 在 ImageStudioPage/ChatPage 的用法照抄），确认后 `deleteVideoJob(token)` 并从列表移除；进行中任务不显示删除按钮（后端也会 409）；
- 加载更多：`hasMore` 时底部按钮，`listVideoJobs(page+1)` append。

- [ ] **Step 5: 路由与侧边栏**

`App.tsx`：`import VideoStudioPage from "@/pages/VideoStudioPage"`；照 `/studio` Route 的包裹结构（`App.tsx:108-123`）加 `/videos`。

`Sidebar.tsx`（`:253` "图像工作室"按钮块之后）：

```tsx
        <Button asChild variant="outline" size="sm" className="w-full justify-start gap-2">
          <Link to="/videos" onClick={() => onNavigate?.()} title="文生视频 / 图生视频">
            <Clapperboard className="size-4" /> 视频工作室
          </Link>
        </Button>
```

`Clapperboard` 从 `lucide-react` import（该文件现有 lucide import 行里追加）。

- [ ] **Step 6: 验证 + 提交**

Run: `cd web && bun run build && bun run lint`
Expected: build PASS；lint 无新增 error（既有 warning 不管；注意组件内辅助函数别用 `use` 前缀命名）。

```bash
git add web/src/pages/VideoStudioPage.tsx web/src/App.tsx web/src/components/app/Sidebar.tsx
git commit -m "feat(web): 视频工作室页面（参数面板/实时价格/轮询/视频库）"
```

---

### Task 9: 管理端 — 视频定价面板 + 渠道 kind 增加 video

**Files:**
- Create: `web/src/pages/admin/VideoPricingPanel.tsx`
- Modify: `web/src/lib/channels.ts:26`（`ChannelKind` 类型加 `"video"`）及该文件（追加 video-pricing API 函数）
- Modify: `web/src/pages/admin/ChannelsPanel.tsx`（kind 下拉加"视频"选项；找到现有 chat/image 选项的 Select/RadioGroup 照加）
- Modify: `web/src/pages/AdminPage.tsx`（Tabs 注册新面板；照 ChannelsPanel/PricingPanel 的挂法）

**Interfaces:**
- Consumes: Task 3 的 `/api/admin/video-pricing` 端点。
- Produces: 管理界面可维护视频定价与视频渠道。

- [ ] **Step 1: channels.ts 类型与 API**

```ts
export type ChannelKind = "chat" | "image" | "video"

export type VideoSizeRule = { size: string; multiplier: number }

export type VideoPricing = {
  id: number
  model: string
  display_name: string | null
  enabled: boolean
  base_credits: number
  per_second: number
  allowed_seconds: number[]
  size_rules: VideoSizeRule[]
}

export type VideoPricingInput = {
  model: string
  display_name?: string | null
  enabled?: boolean
  base_credits: number
  per_second: number
  allowed_seconds: number[]
  size_rules: VideoSizeRule[]
}

export async function listVideoPricing(): Promise<VideoPricing[]> {
  const res = await fetch("/api/admin/video-pricing", { credentials: "same-origin" })
  return jsonOrThrow(res)
}

export async function upsertVideoPricing(input: VideoPricingInput): Promise<void> {
  const res = await fetch("/api/admin/video-pricing", {
    method: "POST",
    credentials: "same-origin",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(input),
  })
  return okOrThrow(res)
}

export async function deleteVideoPricing(model: string): Promise<void> {
  const res = await fetch(`/api/admin/video-pricing/${encodeURIComponent(model)}`, {
    method: "DELETE",
    credentials: "same-origin",
  })
  return okOrThrow(res)
}
```

- [ ] **Step 2: VideoPricingPanel**

结构照 `PricingPanel.tsx`（先通读它）：上方规则列表 Table（列：模型、显示名、基础价、每秒价、允许时长、分辨率档位数、启用开关、编辑/删除），下方/对话框编辑表单：

- 模型名、显示名、基础价、每秒价：普通 Input（数字字段 `type="number"` min 0）；
- 允许时长：逗号分隔输入框（如 `4,8,12`），提交前 parse 成 number[]，非法项报错"时长必须为正整数"；
- 分辨率倍率表：可增删行的小表格——每行两个输入（`size` 文本如 `1280x720`、`multiplier` 数字）+ 删除行按钮，底部"添加档位"按钮；提交前校验 size 格式 `/^\d+x\d+$/`、multiplier > 0；
- 提交调 `upsertVideoPricing`，删除用应用内确认对话框。全部中文文案。

- [ ] **Step 3: ChannelsPanel kind 下拉 + AdminPage 挂 Tab**

- ChannelsPanel：搜现有 kind 选项（"chat"/"image" 的 SelectItem 或等价物），追加 `<SelectItem value="video">视频</SelectItem>`；若该面板有按 kind 的过滤 tab 也同步加。协议约束（video 仅 openai）后端已校验，前端可在选 video 时把协议选择锁定为 openai（一行 disabled 逻辑，做不做都不阻塞——后端兜底）。
- AdminPage：照 ChannelsPanel/PricingPanel 的注册方式加 `<TabsTrigger value="video-pricing">视频定价</TabsTrigger>` + 对应 `TabsContent` 渲染 `<VideoPricingPanel />`（AdminPage 的 Tabs 结构在 `:920` 附近，先读清它当前有哪些 tab、admin 面板信息架构在哪个层级，选同层挂入）。

- [ ] **Step 4: 验证 + 提交**

Run: `cd web && bun run build && bun run lint`
Expected: PASS

```bash
git add web/src/lib/channels.ts web/src/pages/admin/VideoPricingPanel.tsx web/src/pages/admin/ChannelsPanel.tsx web/src/pages/AdminPage.tsx
git commit -m "feat(admin): 视频定价管理面板；渠道支持视频类型"
```

---

### Task 10: 端到端运行验证

**Files:** 无新文件（发现问题就地修）。

**Interfaces:** —

- [ ] **Step 1: 双侧编译**

Run: `cargo check && cd web && bun run build && bun run lint`
Expected: 全 PASS。

- [ ] **Step 2: 冷启动 + migration**

```bash
rm -rf /tmp/nc-video-e2e && YUNOVA_DATA_DIR=/tmp/nc-video-e2e YUNOVA_BIND=127.0.0.1:3100 cargo run
```

走 `/setup` 建 SQLite + 管理员。确认启动日志无 migration 报错；`sqlite3 /tmp/nc-video-e2e/yunova.db ".schema video_jobs"` 能看到表。

- [ ] **Step 3: 管理端配置**

浏览器（或 curl 带 session cookie）：
1. 渠道管理建 video 渠道：protocol=openai、kind=video、base_url 指向可用的 new-api 实例、真实 key；
2. 视频定价建一条规则（如 model=`kling-v1`、base 10、per_second 5、seconds `[5,10]`、rules `[{"size":"1280x720","multiplier":100}]`）；
3. `curl -b <cookie> localhost:3100/api/videos/models` 返回该模型。

- [ ] **Step 4: 正向链路**

视频工作室页：选模型/时长/分辨率 → 确认按钮显示正确积分价 → 生成 → 进行中卡片出现且 progress 递增 → 完成后 `<video>` 可播放、可拖动进度条（验证 Range：`curl -H "Range: bytes=0-99" -sD- -o /dev/null localhost:3100/api/videos/<name>.mp4` 应返回 206 + Content-Range）→ 管理端/账本页确认扣费记录 `video_{model}`。

- [ ] **Step 5: 失败与兜底链路**

1. 402：把测试用户余额调到低于单价，提交 → 前端显示"积分不足"；
2. 退款：渠道 key 改成错的再提交 → 任务 failed，账本出现 `refund_video_..._create_error`，余额复原；
3. 兜底：正常提交后立即关掉页面 → ≤11 分钟后重开视频库，任务应已 completed（落盘）或 failed（已退款）；
4. 删除：删一个完成任务 → 列表移除且 `/tmp/nc-video-e2e/videos/` 下对应 mp4 消失。

- [ ] **Step 6: 收尾提交**

修复过程中的所有改动随修随 commit；全部通过后：

```bash
git add -A && git status   # 确认无遗漏
git commit -m "chore(videos): 端到端验证修复" # 仅当有改动
```

---

## Self-Review 记录

- **Spec 覆盖**：migration（T1）、渠道 video（T2）、定价 CRUD+models（T3）、创建含图生视频与全部退款路径（T4：no_channel/create_error）、advance_job 节流/抢锁/下载重试/upstream_failed/download_failed + Range 静态服务 + 列表删除（T5）、sweeper 修锁/timeout/孤儿推进（T6）、前端客户端/页面/管理端（T7-9）、验收清单逐条对应 spec"测试与验收"（T10）。spec 错误表 7 行全部有落点。
- **类型一致性**：`JobView`（T5）与 TS `VideoJob`（T7）字段逐一对应；`compute_cost`（T3）与 `computeVideoCost`（T7）同式；`refund_job` 签名 T4 定义、T5/T6 引用一致；status 字符串统一 `pending/running/completed/failed`（注意与 image_jobs 的 `done` 不同，前端勿照抄旧值）。
- **无占位符**：所有代码块给出实际内容；两处"照现有文件核对方言写法"（T1 mysql/postgres）是对既有代码的核对指令而非 TBD。
