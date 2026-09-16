# Yunova

Yunova 是基于 Rust Axum + React 的自托管 Agent 整合平台，将多模型对话、
Agent 任务执行、图片与视频创作、媒体剪辑和工作流放在统一工作空间中，内置按 token 计费的额度体系。

源码仓库：[yiranxiaohui/Yunova](https://github.com/yiranxiaohui/Yunova)。

## 从旧版本升级

项目品牌由 NovaChat 更名为 Yunova，服务端程序名为 `yunova`，
镜像为 `ghcr.io/yiranxiaohui/yunova`。旧 GitHub 仓库地址会重定向，
建议将 Git remote 更新为 `git@github.com:yiranxiaohui/Yunova.git`。

- 新配置使用 `YUNOVA_*` 环境变量，仍兼容对应的 `NOVACHAT_*` 旧名称；同时设置时新名称优先。
- 新安装默认使用 `yunova.toml` 和 `yunova.db`。如果数据目录只有旧的 `novachat.toml`，
  服务会继续读取和保存该文件，并使用其中原有数据库地址；不会移动、重命名或创建替代数据库。
  也可用 `YUNOVA_CONFIG` 指向现有配置。
- 浏览器继续读取旧品牌下的本地设置和视频任务，保存时写入 Yunova 名称。用户和访客数据仍分别隔离。
- S3 已配置的 bucket/prefix 保持有效；未指定 prefix 时继续使用历史命名空间 `novachat`，
  避免丢失对已有对象的访问。新配置建议显式指定 `prefix = "yunova"`。
- 迁移 `0037` 只更新仍等于旧默认值的邮件发件人名称和充值商品名称，保留管理员自定义值。
  已发布迁移中的旧名称用于升级识别，不改写历史迁移。

仓库中的 Compose 文件用于新安装，默认服务名为 `yunova`，挂载目录为 `./yunova_data`。
升级已有部署时，应保留其 Compose 项目名、服务名、端口、配置和数据挂载，先只切换到已发布的
Yunova 镜像。不要直接用新模板覆盖旧部署，否则可能创建一个使用空数据目录的新实例。
需要使用模板连接现有目录时，可指定 `YUNOVA_DATA_PATH`、`YUNOVA_MYSQL_DATA_PATH` 和
`YUNOVA_POSTGRES_DATA_PATH`；数据库名称和账号仍需匹配原有配置。

## 计费模型（v3，2026-09）

v1 的「全局共享上游 + cost_chat / cost_image 两档定价」和 v2 的「按次积分」已被替换为
**按 token 计费的站点额度制**：

- **额度（quota）是本站自有单位**，界面直接显示整数，不带货币符号。
- **模型价格照抄官方美元价目表**：对话填每 100 万 token 的输入/输出（及可选的缓存输入）
  单价，图像填每次调用单价，视频填基础价 + 每秒价。价格以**微美元**（1 美元 = 1_000_000）
  整数存储，`$1.25` 就是 `1250000`，不存在浮点误差。
- **两个全局开关**把上游美元成本换算成额度：`quota_per_usd`（默认 500000）
  与 `price_multiplier_percent`（默认 100，即不加价）。改价立即生效，无需重启。
- **对话事后结算**：请求前只做白名单校验和「余额是否为正」的准入判断，不预扣。
  代理在转发 SSE 的同时旁路解析上游返回的 usage，流结束后按真实 token 扣费。
  上游失败或没返回 usage 就**不计费**，因此不再需要退款链路。
  图像按次、视频按秒仍是先扣后退（这些接口不返回 token）。
- **多渠道（Channel）路由**：每种协议可配置 N 个上游 channel；按 `priority` 升序 fallback。
  渠道的 `api_key` **只写不读**：`GET /api/admin/channels` 只返回 `api_key_hint`
  （如 `sk-p…6789`）与 `has_api_key`，明文不离开服务端。编辑时该字段留空即表示
  不修改，只能整个替换——一次管理员会话泄露不应等于全部上游 key 泄露。
- **按模型定价白名单**：`model_pricing` 里没有的 model 直接 403 `NotWhitelisted`；
  价格填 0 表示放行不扣费。
- **Ledger**：每条流水都带 `kind` / `protocol` / `model` 和
  `input_tokens` / `output_tokens` / `cached_tokens`，统计面板据此做按模型聚合。

### 数据流

```
POST /api/proxy/openai {model:"gpt-5",...}
 → authorize_chat("gpt-5")     → 白名单通过 && 余额 > 0（不扣费）
 → list_channels_for_model     → [ch1(p=10), ch2(p=20)]
 → try ch1 → 503 → try ch2 → 200 OK stream
 → MeteredStream 边转发边解析 usage
 → 流结束 settle_chat(input=1000, output=500)
     $1.25/1M × 1000 + $10/1M × 500 = $0.00625
     × quota_per_usd(500000) = 3125 额度
 → ledger: -3125 "chat_openai" (model=gpt-5, in=1000, out=500)
全渠道失败 → 不扣费 → 502
```

### 后台管理

`Admin → Channels`：CRUD 渠道、启用/停用、绑定 model 列表（每行 `model` 或 `model=client_model=upstream_id`）。
`Admin → Pricing`：CRUD model 白名单 + 官方美元单价（录入时实时预览折算后的额度），
或点「从 NewAPI 导入」批量拉取上游价格（见下）。
`Admin → 设置 → 额度换算`：调整 `quota_per_usd`、全局加价倍率、注册与邀请赠送额度。

旧的「Shared Backend」面板与 KV (`shared_chat_openai_*` 等) 暂时保留只用于历史 seed，新链路不再读它们。

### 从 NewAPI 导入价格

若上游是 NewAPI / One-API 系的中转站，可直接读它的 `GET /api/pricing`，免去逐个录价：

- **倍率换算**：NewAPI 以倍率存价，源码锚定 `1 === $0.002/1K tokens`，即
  `输入 $/1M = model_ratio × 2`、`输出 = model_ratio × completion_ratio × 2`、
  `缓存 = 输入 × cache_ratio`；`quota_type=1` 时改用 `model_price`（已是美元）。
  换算后统一落成微美元，和手工录入的价格完全等价。
  顺带一提，NewAPI 的 `QuotaPerUnit = 500000` 与本站 `quota_per_usd` 默认值天然一致。
- **不兼容的计费方式会被跳过而不是按 0 导入**：上游「按次计费的对话模型」和
  「按 token 计费的图像模型」在本站没有对应计费模式，若强行导入会变成免费，
  因此只报告原因、不写库，由管理员手动定价。
- **默认停用 + 默认不覆盖**：导入的模型默认 `enabled=false`，便于复核后再开放；
  已存在的模型默认跳过，避免冲掉手工调整过的价格（勾选「覆盖」才更新）。
- **先预演再写入**：`dry_run` 返回与实际导入完全一致的结果，可先看清将要写什么。
- 可按 NewAPI 分组过滤，并一次性绑定到指定渠道。

接口：`POST /api/admin/pricing/sync-newapi`
`{base_url, group?, channel_ids?, dry_run?, overwrite_existing?, enable_imported?}`。
管理员提供的 URL 会走 SSRF 防护，无法用于探测内网。

### 从积分升级到额度

迁移 0038 按 1 积分 = 500 额度换算余额与流水，购买力不变。但**旧的对话模型无法自动定价**：
积分制下 chat 只有「每次 N 积分」一个数字，推导不出输入/输出单价。迁移 0039 因此把这类
模型**停用**（原值保留在 `per_call_price` 供参考），避免它们以 0 价继续提供服务。

升级后请在「模型计费」里为这些对话模型补上官方美元单价——手工填写或用「从 NewAPI 导入」——
再重新启用。图像（按次）和视频（按秒）的定价可以直接换算，不受影响。

## 三库并行 migration

每次 schema 变更**必须**同时落地三份同号 SQL：
- `migrations/sqlite/NNNN_*.sql` — `INTEGER` + `TEXT DEFAULT (datetime('now'))`
- `migrations/postgres/NNNN_*.sql` — `BIGSERIAL / BOOLEAN / TIMESTAMPTZ DEFAULT NOW()`
- `migrations/mysql/NNNN_*.sql` — `BIGINT AUTO_INCREMENT / TINYINT(1) / DATETIME(3) DEFAULT CURRENT_TIMESTAMP(3) ENGINE=InnoDB utf8mb4_unicode_ci`

并把文件名追加到 `src/db.rs` 三个 `MIGRATIONS_*` 数组。

跨方言 SQL 助手：
- `db::q(kind, sql)` — Postgres 自动 `?` → `$1..$N`
- `db::bool_as_int(kind, col)` / `db::bool_true(kind)` — bool 列读 / 写

## 本地开发

```bash
cargo run                # 后端 :3001
cd web && bun run dev    # 前端 :5173 → /api 走 vite proxy
```

测试在本地跑：`cargo test` + `cd web && npx tsc -b && bun test`。CI 只负责构建镜像、不跑测试。

## 创作流水线

`/workflows` 提供可拖拽、可连线的媒体节点画布，内置图片生成、视频生成、
视频裁剪和视频合并节点。同一张图片可以分支到多个视频节点，合并节点会按输入
优先级从小到大拼接视频，同优先级保持连线创建顺序。运行状态和节点产物持久化到
数据库，页面关闭或服务重启后可
继续跟踪；失败节点支持从该节点向下重试。

裁剪和合并依赖运行环境中的 `ffmpeg` / `ffprobe`，官方容器镜像已内置。
`YUNOVA_FFMPEG`、`YUNOVA_FFPROBE` 可指定自定义可执行文件路径，
`YUNOVA_MEDIA_CONCURRENCY` 控制全局并发媒体处理数（默认 `2`，范围 `1～8`），
`YUNOVA_MEDIA_TIMEOUT_SECONDS` 控制单次处理超时（默认 `7200` 秒）。

### 自定义视频 API

视频工作室支持切换到「本地 API」模式，填写 OpenAI 兼容服务的 Base URL、
API Key（可选）和模型。模型查询、创建、轮询和 MP4 下载都由当前浏览器直接
访问本地服务，不经过 Yunova 后端；自定义任务不扣平台额度，凭据和任务记录
只保存在浏览器，不会写入数据库或云端同步。服务提供 `/v1/models` 时可在页面
加载模型列表，否则可以手动填写模型、时长和分辨率。

本地服务必须允许 Yunova 页面来源的 CORS 请求；浏览器要求局域网访问授权时
也需要允许该权限。HTTPS 页面不能调用普通局域网地址上的 HTTP 服务，此时需要
给本地服务配置 HTTPS，或从 HTTP 页面使用。若填写 API Key，本地服务的 CORS
预检还需允许 `Authorization` 请求头。

当前本地模式对接的是 OpenAI 兼容的三段式视频接口；原生 ComfyUI 的
`/prompt` 工作流协议尚未直接接入，需要先使用兼容层或适配器暴露
`/v1/videos`、`/v1/models`。

## 在线视频剪辑

`/editor` 提供面向桌面的多轨精细剪辑台：视频、音频和文字轨道支持按帧吸附、
拖移、修边、切割、复制、撤销/重做，并可精确设置入点、速度、音量、透明度、
位置、缩放、旋转与淡入淡出。项目时间线自动保存为版本化快照，服务端使用
FFmpeg 合成 MP4，因此无需让浏览器长时间承担最终编码。

素材面板同时聚合用户上传、历史图片/视频生成、流水线产物和公有素材库；个人
素材可以发布为公有素材或收藏公有素材。时间线缺少镜头时，可以在当前空隙直接
启动视频生成流水线，完成后自动回填时间线并进入个人素材库。导出任务、进度和
结果都会持久化，页面刷新后仍可查看。

## S3 媒体存储

Yunova 默认把图片、视频、音频和头像保存在 `YUNOVA_DATA_DIR`。可在
管理员后台的「媒体存储」页面中启用 AWS S3、Cloudflare R2、MinIO 等 S3 兼容
存储。页面支持连接测试，保存后立即生效，无需重启；Access Key 和 Secret Key
不会通过管理接口回显。

配置保存在数据目录下的 `yunova.toml`。也可以直接维护该文件：

```toml
database_url = "sqlite:///data/yunova.db"

[storage]
backend = "s3"
endpoint = "https://<account-id>.r2.cloudflarestorage.com" # AWS S3 可省略
region = "auto"                                            # AWS 填实际 region
bucket = "yunova-media"
access_key_id = "..."
secret_access_key = "..."
prefix = "yunova"
path_style = true                                           # 自定义 endpoint 默认 true
```

手动修改配置文件后需要重启 Yunova。为兼容已有 Docker/Kubernetes 部署，仍支持
以下环境变量作为未配置 `[storage]` 时的后备方式；新部署推荐使用管理页面：

| 环境变量 | 说明 |
| --- | --- |
| `YUNOVA_STORAGE_BACKEND` | 设为 `s3` 启用 S3；默认 `local` |
| `YUNOVA_S3_ENDPOINT` | S3 兼容 endpoint；AWS S3 可省略 |
| `YUNOVA_S3_REGION` | 区域，默认 `us-east-1` |
| `YUNOVA_S3_BUCKET` | bucket 名称 |
| `YUNOVA_S3_ACCESS_KEY_ID` | Access Key ID |
| `YUNOVA_S3_SECRET_ACCESS_KEY` | Secret Access Key |
| `YUNOVA_S3_SESSION_TOKEN` | 临时凭证的 session token（可选） |
| `YUNOVA_S3_PREFIX` | 对象 key 前缀，新配置建议 `yunova`；未指定时兼容历史命名空间 |
| `YUNOVA_S3_PATH_STYLE` | 是否使用 path-style，支持 `true/false` |

网页或 `yunova.toml` 中保存的配置优先于环境变量。凭证和区域也兼容标准的
`AWS_ACCESS_KEY_ID`、`AWS_SECRET_ACCESS_KEY`、
`AWS_SESSION_TOKEN`、`AWS_REGION` / `AWS_DEFAULT_REGION`。bucket 可以保持私有，
Yunova 会代理读取并保留原有 `/api/images/...`、`/api/videos/...` 地址以及视频
Range 播放。启用后新图片、视频和头像只写入 S3；切换前的本地媒体仍可回退读取，
但不会自动上传或删除。普通聊天文档附件仍保存在本地 `files/` 目录。

## Agent 令牌（外部 Agent 运行时接入）

浏览器之外的 Agent 运行时——桌面客户端里内嵌的 `pi`、云沙箱容器里的 `pi`——
需要访问平台的模型网关，但它既没有 `nc_session` cookie，也**不能**拿到管理员
配置的上游渠道 key（那会绕过模型白名单、渠道 fallback 和 token 计量，
并把管理员的 key 放到用户自己的机器上）。

Agent 令牌是为此提供的 bearer 凭据：只绑定一个用户，数据库里只存 SHA-256
哈希，明文仅在创建时返回一次。它解析出的用户与 cookie 路径完全一致，因此
现有的「白名单 → 授权 → 按 token 计费」链路一行都不用改。

令牌分两类，区别只在生命周期：

- **会话凭据**：启动云电脑/设备任务时由服务端自动签发（名称 `session-<id>`），
  带 `expires_at`（默认 12 小时）。会话一结束就吊销——用户点停止、运行时退出、
  设备断线、沙箱到达寿命上限、服务端重启后的孤儿清理，每条路径都会吊销。
  TTL 只是兜底，正常路径不依赖它。
- **手动令牌**：用户在令牌页自己创建，`expires_at` 为空，由该页面的撤销按钮管理。

设备任务会把明文令牌下发到用户自己的机器上，这是设计使然（运行时在那边）。
正因如此，它必须随会话消亡：否则用户可以把它抄出来长期使用。

| 接口 | 说明 |
| --- | --- |
| `GET /api/agent/tokens` | 列出本人令牌（只返回前缀与 `expires_at`/`expired`，不回显明文） |
| `POST /api/agent/tokens` | 创建令牌，响应中的 `token` 是唯一一次明文 |
| `DELETE /api/agent/tokens/{id}` | 撤销（保留记录以便追溯，不物理删除） |
| `POST /api/agent/runtime-config` | 生成运行时用的 `models.json` |

令牌以 `yna_` 开头，可通过 `Authorization: Bearer`、`x-api-key` 或
`x-goog-api-key` 提交——`pi` 会按 provider 的 `api` 类型选择请求头。用户自己的
上游 key（BYOK）不带这个前缀，因此不会被误认成 Agent 令牌，BYOK 仍然完全绕过
平台计费。

`POST /api/agent/runtime-config` 按 `model_pricing` 生成配置，所以运行时只能
看到管理员已启用并定价的模型，`baseUrl` 指向本站的 `/api/proxy/*` 而不是真实
上游：

```bash
curl -X POST https://yunnet.top/api/agent/runtime-config \
  -H 'content-type: application/json' \
  -d '{"base_url":"https://yunnet.top","token":"yna_..."}' \
  > ~/.pi/agent/models.json
```

生成的 provider 形如：

```json
{
  "providers": {
    "yunova-claude": {
      "baseUrl": "https://yunnet.top/api/proxy/claude",
      "api": "anthropic-messages",
      "apiKey": "yna_...",
      "models": [{ "id": "claude-opus-4-5", "contextWindow": 200000 }]
    }
  }
}
```

运行时的 SDK 会自己在 `baseUrl` 后拼接厂商的固定路径（OpenAI 拼 `/responses`，
Anthropic 拼 `/v1/messages`），因此网关额外暴露了
`/api/proxy/openai/responses` 和 `/api/proxy/claude/v1/messages` 这两个别名，
让未经改造的客户端可以直接指向 `/api/proxy/<protocol>`。

## Agent 任务（云电脑 / 本地电脑）

### 前端入口

侧边栏的「新工作任务」进入 `/t`，已有任务在 `/t/:id`。对话与工作是并列的两种模式，
而不是同一页面的开关：工作模式会启动一个能执行命令的 Agent，选择它等于决定
「代码在哪里跑」，必须是一个显式决定。

侧边栏本身按「常用在上、工具收起、下载在底」排布：新对话与新工作任务常驻顶部，
图像/视频/流水线/剪辑/素材库收进可折叠的「更多」，桌面端还可以收成只剩图标的窄栏
（偏好存在 `localStorage`，刷新后保持）。这样会话列表拿到绝大部分竖向空间，
而不是被一堆入口卡片挤到屏幕下半截。

工作模式下再选执行位置：**云电脑**（隔离容器）或**本地电脑**（桌面客户端）。
未安装桌面客户端时不会把本地电脑列成可选项，而是直接说明原因；离线设备也不可选，
避免把一个前置条件变成发送时才报的错。会话创建后执行位置不再可改——它的运行时和
记录已经属于那台机器。

几个前端设计决定：

- **记录始终以服务端镜像为准**。流式增量只作为临时覆盖层渲染，轮次结束即丢弃，
  因此标签页不会变成第二个可能与其他设备不一致的数据源。刷新、重连、`resync`
  提示都走同一条恢复路径。
- **审批用卡片而不是弹窗**。请求会广播到所有在线端，可能在你看着另一台设备时弹出，
  在所有端上抢焦点比等待更糟。任一端处理后，其余端的卡片会通过
  `approval_resolved` 事件自动消失。
- **不改 ChatPage**。对话是标签页拥有的一问一答，任务是服务端拥有、多端可加入的
  长会话；合成一个组件意味着总有一方在被绕过。

### 移动端（遥控）

手机是**遥控器，不是执行目标**：它驱动跑在云电脑或已配对电脑上的任务。
所以移动端用 Capacitor 打包**同一份** React 构建产物，只补上浏览器做不到的事——
主要是在 Agent 被审批阶住时通知用户。

平台差异全部收在 `web/src/lib/platform.ts` 一层里。组件问的是「能不能通知」而不是
「是不是 iOS」，因此某个能力日后出现在新平台上时不需要改组件。其中
`canExecuteLocally` 有产品含义：只有桌面外壳为 `true`，所以手机不会声称
能在本机跑任务。

```bash
cd web
bun run mobile:sync      # 构建 + cap sync
bun run mobile:android   # 并打开 Android Studio
bun run mobile:ios       # 并打开 Xcode（需 macOS）
```

`web/android` 和 `web/ios` **不入版本库**：它们由 `capacitor.config.ts` 完整生成，
且无需手改（通知权限由插件通过 manifest merge 注入）。克隆后跑一次
`bun run mobile:sync` 即可。

几个实现要点：

- **审批本地通知**。请求来自客户端已经持有的 socket，所以用本地通知即可，
  **不需要服务端推送基础设施**。固定通知 id 让重复请求折叠而不是堆满通知栏。
- **切回前景时从服务端重对账**。手机会挂起定时器并可能掉连接，所以恢复时
  重拉镜像，而不是假定屏上内容仍然完整。
- **安全区垫边**。`viewport-fit=cover` 配合 `.safe-top` / `.safe-bottom`，
  否则底部输入框会压在 Home 指示条下面变得部分不可点。
- **触摸点击区**。`.tap-target` / `.tap-target-sm` 只在 `pointer: coarse` 下生效，
  因此不会把桌面端的控件撑大。
- **不允许明文流量**。自托管实例必须走 HTTPS，否则会话 cookie 和配对码
  会在网络上裸奔。

iOS 出包仍需 macOS 与开发者账号，Android 出包需 Android SDK。

### 本地电脑（桌面客户端）

`desktop/` 是独立的 `yunova-desktop` 二进制：把用户自己的电脑变成执行目标。
它**主动**连回服务器并保持 WebSocket——个人电脑通常没有可达地址，主动出连是
唯一不需要端口映射的做法。

走的是与云电脑**完全相同**的 pi RPC JSONL：客户端在本机跑 `pi --mode rpc`，把它的
stdio 经由这条转发通道与服务器对接。因此 `DeviceTransport` 只是一根管子，
镜像、广播、审批、计费全部复用已经在沙箱上验证过的代码。

使用流程：在 `/t` 页面点「本地电脑」生成配对码（只显示一次，服务端只存哈希），
然后在目标机器上运行：

```bash
YUNOVA_DEVICE_URL=https://yunnet.top \
YUNOVA_DEVICE_TOKEN=ynd_... \
YUNOVA_DEVICE_WORKSPACE=/path/to/project \
./yunova-desktop
```

二进制从 `/download` 页面下载。它不由本服务分发：镜像没有理由塞进五个平台的构建，
自托管实例也不该为了让用户装客户端而去镜像这些文件。页面在浏览时读 GitHub Release
的资产列表，读不到（无出网、限流、内网部署）就退化成「最新发布页」链接，
而不是渲染出死链。资产名由 `.github/workflows/desktop-release.yml` 产生，
必须与 `web/src/lib/downloads.ts` 里的 `yunova-desktop-<target>` 对齐，
`web/tests/downloads.test.ts` 盯着这条约定。

| 环境变量 | 默认 | 说明 |
| --- | --- | --- |
| `YUNOVA_DEVICE_URL` | 必填 | 站点地址，自动推导 WebSocket 端点 |
| `YUNOVA_DEVICE_TOKEN` | 必填 | 网页生成的配对码 |
| `YUNOVA_DEVICE_WORKSPACE` | 当前目录 | **Agent 可操作的范围**，请指向具体项目 |
| `YUNOVA_DEVICE_NAME` | 主机名 | 列表里显示的名字 |
| `YUNOVA_DEVICE_AUTO_APPROVE` | 关 | 设为 `1` 放开审批，谨慎使用 |
| `YUNOVA_PI_BIN` | `pi` | 运行时可执行文件 |
| `YUNOVA_DEVICE_GATEWAY_URL` | 服务端配置 | 设备端运行时回调的网关地址（服务器上设置） |

安全模型与云电脑**有本质区别**，协议设计也因此不同：沙箱是一次性且隔离的，
个人电脑不是。所以约束 Agent 的策略**由客户端拥有**，而不是交给服务器：

- **审批默认开启**。客户端自己写入 `tool_call` 门禁 extension，拦住
  `bash`/`powershell`/`write`/`edit`。指令可能来自手机，也可能受 Agent 读到的
  网页内容影响，所以在个人机器上「询问」才是安全默认值。
- **工作目录限定范围**，默认当前目录而非 `$HOME`；不配置也不会默认暂开整个用户目录。
- **服务器不下发要执行的命令**，只转发提示词，由本机运行时自己判断。
- **设备拿不到上游 key**，只收到指向本站网关的会话级令牌（文件权限 `0600`）。
- **移除设备立即断开**并使配对码失效；离线设备无法启动任务（返回 409）。

审批请求会广播到所有在线端，因此可以在网页或手机上处理跑在家里电脑上的任务；
拒绝后 Agent 会收到被阻止的原因，而不是默默卡住。

目前只有无界面的命令行客户端（可直接用于服务器和开发机）；GUI 外壳可以直接
嵌入这套连接器与策略代码，它不依赖窗口。

### 云电脑会话的审批配置

沙箱会把 `<数据目录>/agent-runtimes/s<id>/agent/extensions` 以**只读**方式挂载并
拷入运行时配置目录。这是给云电脑加 `tool_call` 审批门禁的唯一途径：运行时只从
自己的配置目录发现 extension，而那个目录在容器内是 tmpfs。只读是必要的——
能拦工具调用的 extension 不能被 Agent 自己改掉。

示例（需重启该会话的运行时生效）：

```ts
// <数据目录>/agent-runtimes/s1/agent/extensions/approval.ts
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent"
export default function (pi: ExtensionAPI) {
  pi.on("tool_call", async (event, ctx) => {
    if (event.toolName !== "bash") return
    const ok = await ctx.ui.confirm({ title: "允许执行命令？" })
    if (!ok) return { block: true, reason: "用户拒绝" }
  })
}
```

不配置时云电脑全自动执行，这正是容器隔离存在的理由；本地电脑则应始终保留审批。

### 后端接口

工作模式的任务由外部 Agent 运行时（`pi --mode rpc`）执行，而不再由服务端自己写思考循环。
两种执行目标说的是**同一套 JSONL 协议**，因此“目标”只是传输层的差异：

| target | 执行位置 | 接入方式 |
| --- | --- | --- |
| `cloud` | 本服务器分配的沙箱 | 服务端直接拉起子进程 |
| `device` | 用户自己的电脑 | 桌面客户端主动连回并保持连接 |

于是会话存储、事件广播、审批路由和计费只写一份，网页、桌面端和手机看到的是同一份记录。

| 接口 | 说明 |
| --- | --- |
| `POST /api/agent/sessions` | 创建会话（`target` 为 `cloud` 或 `device`） |
| `GET /api/agent/sessions` | 列出会话，`live` 表示当前是否挂着运行时 |
| `POST /api/agent/sessions/{id}/start` | 启动运行时（仅 `cloud`），已启动时复用 |
| `POST /api/agent/sessions/{id}/stop` | 停止运行时，不删记录 |
| `GET /api/agent/sessions/{id}/events` | SSE 订阅；多端可同时订阅同一会话 |
| `GET /api/agent/sessions/{id}/entries?since=<entry_id>` | 读取镜像的历史，`since` 为增量游标 |
| `POST /api/agent/sessions/{id}/prompt` | 发消息；流式中需带 `streaming_behavior` |
| `POST /api/agent/sessions/{id}/abort` | 中止当前轮次 |
| `POST /api/agent/sessions/{id}/approve` | 应答审批对话框 |

几个关键设计：

- **订阅而非应答**。事件流是独立的订阅，不是某次 prompt 的响应。所以手机发指令、
  网页看过程这种用法天然成立，刷新页面也不丢上下文。
- **运行时持有权威会话树**，服务端在每次 `agent_settled` 时用 `get_entries {since}`
  增量镜像到 `agent_entries`。`(session_id, entry_id)` 唯一索引使重连后的重叠拉取幂等。
- **只有 `agent_settled` 算结束**，`agent_end` 之后还可能有自动重试、压缩重试和排队消息。
- **审批广播到所有在线端，任一端批准即生效**；重复应答返回 409，避免两个设备
  同时点“允许”时向运行时发两次答案。新订阅者会先收到待处理的审批请求，
  不会看到一个原因不明的停顿。
- **运行时拿不到上游 key**。子进程的 `models.json` 由 `model_pricing` 生成，
  指向本站 `/api/proxy/*` 并带一个会话级 Agent 令牌（文件权限 `0600`）。
- **重启后 `running` 会话重置为 `idle`**。活会话绑定在子进程或设备套接字上，
  不可能跨重启存活；不重置的话那些会话会永远拒绝新消息。

相关环境变量：

| 环境变量 | 说明 |
| --- | --- |
| `YUNOVA_PI_BIN` | Agent 运行时可执行文件，默认 `pi` |
| `YUNOVA_AGENT_GATEWAY_URL` | 运行时回调的网关地址；容器化后需填容器内可解析的地址 |

旧的工蜂（`/api/worker/*`）已下线，其数据表由 migration 44 删除；远程执行全部由
上述 Agent 任务链路接管。已发生的额度流水仍保留在额度明细里。

## 云电脑沙箱

工作模式的 Agent 有 shell，所以云电脑任务跑在**每会话一个容器**里。隔离不是附加项：
没有它就不能开自动批准，而逐条手动确认工具调用等于没有 Agent。

镜像由 `docker/sandbox.Dockerfile` 构建，pi 版本在镜像里固定：

```bash
docker build -f docker/sandbox.Dockerfile -t yunova-sandbox:latest .
```

传输层没有变。`docker run -i` 的 stdio 与本地子进程一致，所以沙箱直接复用同一个
`AgentTransport`，分帧、stderr 排水和关停行为完全相同——容器化是部署变更，不是协议变更。

隔离策略（均已在 `docker inspect` 中验证生效）：

| 措施 | 作用 |
| --- | --- |
| `--cap-drop ALL` + `--security-opt no-new-privileges` | 容器内无任何 capability，也无法重新获得 |
| `--read-only` + tmpfs | 根文件系统只读；只有 `/workspace`、`/tmp`、`/home/agent` 可写且随容器销毁 |
| `--pids-limit` | 挡 fork 炸弹——内存上限拦不住 |
| `--memory` / `--cpus` | 资源上限 |
| `--network yunova-sandbox`（`--internal`） | **无公网出口**，只能访问本站模型网关 |
| `--user <服务进程 uid>` | 非 root；也让容器能读到 `0600` 的凭据文件 |
| `models.json:ro` | Agent 可读但不能改写，无法把请求指到其他上游 |
| 无 TTY | 防止 pty 控制字符污染 JSONL 流 |

关于出网：**仅用 `--add-host` 是不够的**。它只给网关起了个名字，不限制路由；
在默认 bridge 上沙箱仍能访问公网（实测过，返回 200），可用于外传数据或当代理。
因此沙箱挂在专用的 `--internal` 网络上，它没有 masquerade 路由，但宿主仍可通过
桥网关访问。

生命周期：

- 容器按会话命名（`yunova-agent-s<id>`），`--rm` 退出即删
- `stop` 会强制清除容器并立即吊销会话凭据，幂等且可自愈
- 启动时回收上次进程崩溃留下的孤儿容器，并吊销其遗留的会话凭据
- 超过 `YUNOVA_SANDBOX_MAX_LIFETIME` 后自动停止并吊销会话凭据

目前云电脑**不按机时计费**，只按 token 计费（与对话模式一致）。

| 环境变量 | 默认 | 说明 |
| --- | --- | --- |
| `YUNOVA_SANDBOX` | 开启 | 设为 `0` 退回宿主进程（仅开发用，无隔离） |
| `YUNOVA_SANDBOX_IMAGE` | `yunova-sandbox:latest` | 沙箱镜像 |
| `YUNOVA_DOCKER_BIN` | `docker` | 可写 `sudo -n docker` 等包装命令 |
| `YUNOVA_SANDBOX_CPUS` | `1` | CPU 上限 |
| `YUNOVA_SANDBOX_MEMORY` | `1g` | 内存上限 |
| `YUNOVA_SANDBOX_PIDS` | `256` | 进程数上限 |
| `YUNOVA_SANDBOX_WORKSPACE_SIZE` | `1g` | 工作目录 tmpfs 大小 |
| `YUNOVA_SANDBOX_MAX_LIFETIME` | `3600` | 沙箱最长存活秒数 |
| `YUNOVA_SANDBOX_GATEWAY_URL` | `http://yunova-gateway:<port>` | 容器内看到的网关地址 |
| `YUNOVA_SANDBOX_GATEWAY_CONTAINER` | 空 | Yunova 自身所在容器名；容器化部署必填 |
| `YUNOVA_HOST_DATA_DIR` | 空 | `YUNOVA_DATA_DIR` 对应的宿主路径；容器化部署必填 |

### 容器化部署要补的三件事

Yunova 自己跑在容器里、却要驱动**宿主**的 Docker daemon，因此有两处会静默出错：

- **挂载路径**：`-v` 的源路径由 daemon 在宿主文件系统上解析。直接传容器内的
  `/data/...`，daemon 会在宿主同名路径下建一个空目录，沙箱于是读不到凭据。
  用 `YUNOVA_HOST_DATA_DIR` 声明宿主路径即可自动换算。
- **网关地址**：桥网关指向宿主，但服务监听在自己的网络命名空间里，宿主那个端口
  上没人listen。设 `YUNOVA_SANDBOX_GATEWAY_CONTAINER=<自身容器名>`，启动时会把
  该容器接入 `yunova-sandbox` 网络并改用它在该网络上的地址。
- **socket 权限**：挂载 `/var/run/docker.sock` 后，entrypoint 会按 socket 的属组
  把 `yunova` 用户加进去（不改 socket 属主，避免影响宿主其他客户端）。

对应的 Compose 片段：

```yaml
services:
  yunova:
    volumes:
      - ./data:/data
      - /var/run/docker.sock:/var/run/docker.sock
    environment:
      YUNOVA_HOST_DATA_DIR: /opt/yunova/data   # ./data 的宿主绝对路径
      YUNOVA_SANDBOX_GATEWAY_CONTAINER: yunova
```

部署注意：Docker socket 等于 root 权限。挂载它就等于把宿主 root 等价权限交给本服务
进程；不挂载则云电脑不可用，工作模式会在启动时报错。要么挂 socket，要么用
`YUNOVA_DOCKER_BIN="sudo -n docker"`；两者都等于信任该进程。生产环境建议把沙箱
宿主与主服务分开，并在宿主防火墙上叠加出网限制。

## 部署

正式版本镜像 tag：`docker.yunnet.top/github/yiranxiaohui/yunova:X.Y.Z`。

- push `main` → GitHub Actions 构建开发镜像
- push `vX.Y.Z` → GitHub Actions 构建正式镜像
- 两个发布工作流成功后，由 Codex 从可信服务器 SSH 部署生产环境
- migration 在容器启动时自动跑
- 默认版本策略只递增最后一位：`vX.Y.Z` → `vX.Y.(Z+1)`
