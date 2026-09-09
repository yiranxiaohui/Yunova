# 视频生成功能设计

日期：2026-08-05

## 背景与范围

Yunova 目前支持聊天与图片生成，本设计新增视频生成能力。经调研，各家视频 API（Sora、可灵、火山方舟 Seedance、Veo）全部为"创建任务 → 轮询 → 取结果"的异步制；OpenAI 的 `/v1/videos` 三段式协议已成事实标准，new-api 等中转站用这一个格式即可路由到可灵、即梦、万相、Vidu 等多家上游（非 Sora 参数经 `metadata` 透传）。OpenAI 官方 Sora API 将于 2026-09-24 关停，但其协议形态由中转站生态延续。

已确认的功能范围：

- **协议**：只实现 OpenAI `/v1/videos` 格式（`POST /v1/videos` 创建、`GET /v1/videos/{id}` 查询、`GET /v1/videos/{id}/content` 下载 MP4 流），对接 new-api 等兼容中转站。
- **前端**：独立"视频创作台"页面 + 个人视频库。不做视频广场，不改聊天页。
- **计费**：模型 + 时长 + 分辨率组合定价；只走平台共享渠道，全部扣积分，不支持 BYOK。
- **能力**：文生视频 + 图生视频（`input_reference` 参考图）。
- **轮询模型**：前端驱动的懒轮询为主，服务端 60s 定时器低频兜底。

上游产物为限时 URL 或二进制流（可灵约 30 天、方舟约 24 小时、Veo 约 2 天），后端拿到结果必须立即下载 MP4 落盘。

## 架构

```
前端 VideoStudioPage
  → POST /api/videos/jobs        创建任务（先扣积分，再调上游 POST /v1/videos）
  → GET  /api/videos/jobs/{token} 懒轮询（后端顺手查上游 GET /v1/videos/{id}）
       完成 → 后端 GET /v1/videos/{id}/content 下载 MP4 → 落盘 data_dir/videos/
       失败 → 按实扣金额退款
  → GET  /api/videos/{name}       静态播放（公开路由）
```

新后端模块 `src/videos.rs`，`main.rs` 中 `.merge(videos::routes())` 并注册公开路由；复用 `upstream_channels`/`channel_models`（`kind='video'`）、`credits::try_deduct`/`grant`、`data_dir` 落盘模式。

## 数据模型（migration 0030，sqlite/mysql/postgres 三份）

### video_jobs

```sql
CREATE TABLE video_jobs (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    token             TEXT NOT NULL UNIQUE,        -- 前端轮询凭据
    user_id           INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    model             TEXT NOT NULL,               -- 平台模型名
    prompt            TEXT NOT NULL,
    seconds           INTEGER NOT NULL,
    size              TEXT NOT NULL,               -- 如 1280x720
    input_image_path  TEXT,                        -- 图生视频参考图（本地路径，可空）
    upstream_video_id TEXT,                        -- 上游返回的 video id
    channel_id        INTEGER,                     -- 实际使用的渠道（查询/退款用）
    cost_credits      INTEGER NOT NULL DEFAULT 0,  -- 实际扣费，退款按此值
    status            TEXT NOT NULL DEFAULT 'pending', -- pending/running/completed/failed
    progress          INTEGER NOT NULL DEFAULT 0,  -- 上游进度 0-100
    video_path        TEXT,                        -- 落盘后的 /api/videos/{name}
    error             TEXT,
    refunded          INTEGER NOT NULL DEFAULT 0,  -- 是否已退款（幂等保护 + 前端展示）
    download_retries  INTEGER NOT NULL DEFAULT 0,  -- 下载重试计数
    polling           INTEGER NOT NULL DEFAULT 0,  -- 并发轮询去重锁
    last_polled_at    TEXT,                        -- 节流与兜底扫描依据
    created_at        TEXT NOT NULL DEFAULT (datetime('now')),
    started_at        TEXT,
    finished_at       TEXT
);
CREATE INDEX idx_video_jobs_user   ON video_jobs(user_id, created_at DESC);
CREATE INDEX idx_video_jobs_status ON video_jobs(status, last_polled_at);
```

（MySQL/Postgres 版按现有 migration 的方言习惯改写类型与默认值。）

### video_pricing（规则式定价，不枚举组合）

```sql
CREATE TABLE video_pricing (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    model           TEXT NOT NULL UNIQUE,
    display_name    TEXT,
    enabled         INTEGER NOT NULL DEFAULT 1,
    base_credits    INTEGER NOT NULL DEFAULT 0,   -- 每次任务基础价
    per_second      INTEGER NOT NULL DEFAULT 0,   -- 每秒加价
    allowed_seconds TEXT NOT NULL,                -- JSON 数组，如 [4,8,12]
    size_rules      TEXT NOT NULL,                -- JSON [{"size":"1280x720","multiplier":100},...]
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
);
```

**计价公式**：`总价 = round((base_credits + per_second × seconds) × multiplier / 100)`，`multiplier` 为百分比整数。`seconds` 不在 `allowed_seconds`、或 `size` 不在 `size_rules` 中的请求直接 400 拒单，保证价格总是显式配置过的。

视频模型不复用 `model_pricing`（其 `cost_credits` 为一口价，无法表达组合定价），独立成表。

### 渠道与账本

- `upstream_channels.kind` 允许 `'video'`（协议固定 `openai`），`channel_models` 照常映射上游模型名；`channels::select_chain` 增加对 `kind="video"` 的支持（无 schema 变更）。
- `credit_ledger` 无表变更：`LedgerMeta.kind` 新增 `"video"`，reason 形如 `video_{model}`、`refund_video_{model}_{suffix}`。

### 与图片实现的刻意差异

- **退款按 `cost_credits` 实扣值退**，不在退款时重读当前价——视频任务长命（可能跨越管理员改价）且单价高，必须精确退。
- **无 BYOK 分支**：`used_shared` 概念不存在，所有任务都扣费。

## 后端 API（src/videos.rs）

### 受保护路由（require_auth）

| 端点 | 作用 |
|---|---|
| `GET /api/videos/models` | 启用的视频模型列表，含 `allowed_seconds`、`size_rules` 完整规则与基础价/每秒价，供前端渲染选项并本地实时算价 |
| `POST /api/videos/jobs` | 创建任务 |
| `GET /api/videos/jobs/{token}` | 轮询（懒推进） |
| `GET /api/videos/jobs` | 个人视频库列表（分页，含各状态） |
| `DELETE /api/videos/jobs/{token}` | 删除记录 + 本地 MP4（仅本人；pending/running 状态拒绝删除） |

### 管理端路由（require_admin）

`GET/POST/PATCH/DELETE /api/admin/video-pricing` —— 定价规则 CRUD。渠道管理复用现有渠道端点（前端渠道表单 kind 下拉增加"视频"）。

### 公开路由

`GET /api/videos/{name}` —— 静态 MP4，`Content-Type: video/mp4` + HTTP Range 支持（`Accept-Ranges: bytes`，播放器拖进度条必需）。实现时优先看图片静态路由现状，选最小改法（tower-http ServeDir 或手写 Range）。

### 创建任务流程（POST /api/videos/jobs）

请求体：`{ model, prompt, seconds, size, input_image_path? }`。参考图先经现有 `POST /api/images/save` 上传获得路径。

1. 校验模型存在且启用、`seconds ∈ allowed_seconds`、`size ∈ size_rules`，计算 `cost`；
2. `credits::try_deduct(cost)`，余额不足返回 402（中文提示）；
3. `channels::select_chain(kind="video", model)` 选渠道；无可用渠道 → 退款（`refund_video_{model}_no_channel`）+ 400；
4. 插入 `video_jobs`（status=pending，记录 `cost_credits`、`channel_id`）；
5. 调上游 `POST {base}/v1/videos`（multipart：映射后的模型名、prompt、seconds、size；图生视频附 `input_reference` 文件）；
6. 成功 → 写 `upstream_video_id`，status=running，返回 `{token, cost}`；失败 → 退款（`refund_video_{model}_create_error`），status=failed，透传上游错误。

先扣费再调上游，与图片一致；上游调用是唯一外部依赖，失败路径单一。

### 轮询推进（advance_job，前端轮询与兜底定时器共用）

`GET /api/videos/jobs/{token}`：

1. 查本地行；status 为 completed/failed → 直接返回，不打上游；
2. **节流**：`last_polled_at` 距今 < 3 秒 → 返回本地缓存状态；
3. **抢锁**：`UPDATE video_jobs SET polling=1, last_polled_at=<now> WHERE token=? AND polling=0`；没抢到 → 返回本地状态；
4. 用 job 的 `channel_id` 取渠道 base/key（渠道已删 → 标 failed + 退款），调上游 `GET /v1/videos/{id}`：
   - `queued`/`in_progress` → 更新 `progress`；
   - `failed` → 标 failed + 按 `cost_credits` 退款（`refund_video_{model}_upstream_failed`，`refunded` 置 1 保证幂等）；
   - `completed` → `GET /v1/videos/{id}/content` 下载 MP4 流，落盘 `data_dir/videos/{16字节hex}.mp4`，status=completed + `video_path`；
5. 所有分支收尾释放锁（`polling=0`）。

**下载失败不立即判死**：视频已生成成功，单次下载失败只记 error、`download_retries += 1`，保持 running 由下次轮询重试；累计 5 次失败才标 failed + 退款（`refund_video_{model}_download_failed`）。

### 兜底定时器（挂入 main.rs 现有 60s 循环）

每轮执行三件事：

1. **推进孤儿任务**：`status IN ('pending','running') AND last_polled_at < now - 10min` → 逐个 `advance_job`（用户关页面后任务最终仍会落盘或退款）；
2. **修复悬挂锁**：`polling=1 AND last_polled_at < now - 5min` → 强制 `polling=0`（进程崩溃残留）；
3. **超时止损**：`created_at < now - 2h` 且未完成 → 标 failed + 退款（`refund_video_{model}_timeout`）。

启动时不做 `cleanup_stale_jobs` 式的粗暴标失败——状态全在表里，重启后由定时器与用户轮询自然接管。

## 前端设计

### 页面与导航

- 新页面 `web/src/pages/VideoStudioPage.tsx`，路由 `/videos`，侧边栏"图片创作台"旁新增"视频创作台"（lucide `Clapperboard` 或 `Film` 图标）。布局对齐 ImageStudioPage：左侧参数面板、右侧作品区。

### API 客户端 web/src/lib/video-gen.ts

```ts
listVideoModels(): Promise<VideoModel[]>
createVideoJob(req): Promise<{ token, cost }>
getVideoJob(token): Promise<VideoJob>
listVideoJobs(page): Promise<VideoJob[]>
deleteVideoJob(token): Promise<void>
```

无 BYOK，无 `X-Upstream-*` 头，全走站内 cookie 鉴权。

### 参数面板

- 模型下拉（来自 models 接口）；时长与分辨率均为分段按钮，选项来自所选模型的规则；
- 参考图可选上传（复用 `POST /api/images/save`），缩略图预览可移除；
- 提示词多行输入；
- **价格实时预览**：参数一变即本地按公式算价，生成按钮文案"生成（消耗 N 积分）"；余额不足按钮置灰并提示。顶部复用积分余额徽章。

### 生成与轮询体验

- 创建成功后作品区顶部插入"进行中"卡片：转圈 + 上游 `progress` + 已用时，注明"可离开页面，稍后回来查看"；
- 前端 `setInterval` 5 秒轮询（后端 3 秒节流兜底）；页面卸载即停，任务由后端定时器接管；
- 完成 → `<video controls preload="metadata">` 播放器（src 为 `/api/videos/{name}`）+ 下载按钮；
- 失败 → 错误信息 + "已退还 N 积分"（依据 job 的 `cost_credits` 与 `refunded`）。

### 视频库

进页面加载 `listVideoJobs`：完成的显示视频卡片（悬停显示 prompt/模型/时长/消耗积分），进行中的自动恢复轮询，失败的支持"重新生成"（参数回填左栏）。删除走应用内确认对话框。分页或"加载更多"。

### 管理后台

- AdminPage 新增"视频定价"标签页：规则列表 + 编辑表单（模型名、显示名、基础价、每秒价、允许时长、分辨率倍率表——倍率表用可增删行的表格编辑，不让管理员手写 JSON）；
- 渠道管理 kind 下拉增加"视频"选项，其余复用。

### 文案

全站中文；后端错误消息（含 402 余额不足）中文透传，与现有约定一致。

## 错误处理汇总

| 场景 | 行为 | 退款 reason 后缀 |
|---|---|---|
| 余额不足 | 402，不创建任务 | —（未扣费）|
| 无可用渠道 | 400，已扣积分退回 | `no_channel` |
| 上游创建失败 | 任务标 failed | `create_error` |
| 上游生成失败 | 任务标 failed | `upstream_failed` |
| MP4 下载失败 ×5 | 任务标 failed | `download_failed` |
| 渠道被删除 | 任务标 failed | `upstream_failed` |
| 2 小时超时 | 任务标 failed | `timeout` |

所有退款按 `video_jobs.cost_credits` 实扣值执行，`refunded` 标志保证幂等（不重复退）。

## 测试与验收

项目无测试框架，按现有约定以运行验证为准：

1. `cargo check` + `bun run build` 双侧编译通过；
2. 配一个指向 new-api 的 video 渠道 + 一条定价规则，走通：创建 → 轮询进度 → 完成落盘 → 播放器可播可拖动 → 积分账本扣费记录正确；
3. 失败路径：余额不足 402、上游错误退款入账、删除记录后 MP4 文件消失；
4. 兜底路径：创建任务后关闭页面，≤11 分钟后任务被定时器推进完成；
5. 三方言 migration 在 SQLite（默认）至少实测，MySQL/Postgres 语法比照现有 migration 风格。

## 非目标（本期不做）

- 视频广场（发布/点赞/评论）；
- BYOK 用户自带上游；
- 聊天页内的视频模式；
- `/v1/video/generations`（new-api 私有格式）等第二协议；
- 上游 webhook 回调（懒轮询 + 兜底已覆盖需求）。
