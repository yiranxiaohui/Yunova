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
需要使用模板连接现有目录时，可指定 `YUNOVA_DATA_PATH` 和
`YUNOVA_POSTGRES_DATA_PATH`；数据库名称和账号仍需匹配原有配置。

旧版本的安装向导还列着 MySQL，现在只剩 SQLite 和 PostgreSQL 两选一，
原因见下文「双库并行 migration」。已在跑的 SQLite 部署可用 `yunova db-copy`
整体搬到 PostgreSQL，见「切换数据库后端」。

## 计费模型（v3，2026-09）

v1 的「全局共享上游 + cost_chat / cost_image 两档定价」和 v2 的「按次积分」已被替换为
**按 token 计费的站点额度制**：

- **额度（quota）以人民币计价**：1 额度 = 1 元，余额读起来就是一笔钱。因为一次对话常只花
  几分甚至几毫，额度以**微额度**（1 额度 = 1_000_000）整数存储，仅在展示时换算成元。
- **模型价格照抄官方美元价目表**：对话填每 100 万 token 的输入/输出（及可选的缓存输入）
  单价，图像填每次调用单价，视频填基础价 + 每秒价。价格以**微美元**（1 美元 = 1_000_000）
  整数存储，`$1.25` 就是 `1250000`，不存在浮点误差。
- **两个全局开关**把上游美元成本换算成额度：`usd_to_cny_rate_micro`（默认 `1000000`，
  即 1 美元 = 1 元）与 `price_multiplier_percent`（默认 100，即不加价）。改价立即生效，
  无需重启。
- **默认汇率是 1:1，不是外汇牌价**。NewAPI / One-API 系的中转站本身就是按列表美元价
  「1 美元收 1 元」卖额度的，所以持平才等于按真实成本计费；填 7.2 之类的牌价会让每个模型
  都按成本的好几倍扣费。要留毛利请用全局加价倍率，而不是抬高汇率。
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
     × 汇率(1 美元 = 1 元) = 0.00625 额度（6250 微额度）
 → ledger: -6250 "chat_openai" (model=gpt-5, in=1000, out=500)
全渠道失败 → 不扣费 → 502
```

### 后台管理

`Admin → Channels`：CRUD 渠道、启用/停用、绑定 model 列表（每行 `model` 或 `model=client_model=upstream_id`）。
`Admin → Pricing`：CRUD model 白名单 + 官方美元单价（录入时实时预览折算后的额度），
或点「从 NewAPI 导入」批量拉取上游价格（见下）。
`Admin → 设置 → 额度与汇率`：调整 `usd_to_cny_rate_micro`、全局加价倍率、注册与邀请赠送额度。

旧的「Shared Backend」面板与 KV (`shared_chat_openai_*` 等) 暂时保留只用于历史 seed，新链路不再读它们。

### 从 NewAPI 导入价格

若上游是 NewAPI / One-API 系的中转站，可直接读它的 `GET /api/pricing`，免去逐个录价：

- **倍率换算**：NewAPI 以倍率存价，源码锚定 `1 === $0.002/1K tokens`，即
  `输入 $/1M = model_ratio × 2`、`输出 = model_ratio × completion_ratio × 2`、
  `缓存 = 输入 × cache_ratio`；`quota_type=1` 时改用 `model_price`（已是美元）。
  换算后统一落成微美元，和手工录入的价格完全等价。
  配合默认的 1:1 汇率，导入完就是「上游列表价多少美元，本站扣多少元」。
- **默认只同步本地已添加的模型**（`existing_only`）：中转站动辅几百个模型，站点只卖其中一小部分。
  筛好一次后，重新同步应该只给这些模型刷价，上游有、本地没有的直接忽略（计入 `skipped_missing`）。
  该模式隐含覆盖（刷价就是目的），但**只改价格**：启用状态、显示名、上下文上限、
  协议与渠道绑定全部保留，因此被管理员停用的模型不会被一次同步重新激活。
  本地有、上游目录里没有的模型以 `not_listed` 回报（价格保留不动），避免「以为全部刷新了」。
- **计费方式不一致的同名模型不会被改写**：本地 `chat` / `image` / `video` 各自读不同价格列，
  若改写成其他 kind 会把原有列清零而变成免费，因此只告警、不写库。
- **不兼容的计费方式会被跳过而不是按 0 导入**：上游「按次计费的对话模型」和
  「按 token 计费的图像模型」在本站没有对应计费模式，若强行导入会变成免费，
  因此只报告原因、不写库，由管理员手动定价。
- **默认停用 + 默认不覆盖**：导入的模型默认 `enabled=false`，便于复核后再开放；
  已存在的模型默认跳过，避免冲掉手工调整过的价格（勾选「覆盖」才更新）。
- **先预演再写入**：`dry_run` 返回与实际导入完全一致的结果，可先看清将要写什么。
- 可按 NewAPI 分组过滤，并一次性绑定到指定渠道。

接口：`POST /api/admin/pricing/sync-newapi`
`{base_url, group?, channel_ids?, dry_run?, existing_only?, overwrite_existing?, enable_imported?}`。
管理员提供的 URL 会走 SSRF 防护，无法用于探测内网。唯一的例外是 fake-ip 段
`198.18.0.0/15`：主机经透明代理（mihomo / sing-box / Clash 的 fake-ip 模式）解析时，
所有公网域名都会拿到该段里的合成地址，真实目标由隧道在连接时解析，因此把它当私网
拒绝只会让每个正常中转站都报「DNS 解析到私网 / 回环地址」。真正的内网域名仍会解析到
RFC1918 / 回环地址并被拦下。

### 从积分升级到额度

迁移 0038 按 1 积分 = 500 额度换算余额与流水，购买力不变。但**旧的对话模型无法自动定价**：
积分制下 chat 只有「每次 N 积分」一个数字，推导不出输入/输出单价。迁移 0039 因此把这类
模型**停用**（原值保留在 `per_call_price` 供参考），避免它们以 0 价继续提供服务。

升级后请在「模型计费」里为这些对话模型补上官方美元单价——手工填写或用「从 NewAPI 导入」——
再重新启用。图像（按次）和视频（按秒）的定价可以直接换算，不受影响。

迁移 0040 把额度改为人民币计价（1 额度 = 1 元），并把旧的两个点数开关折算成一个汇率。
用旧默认值（500000 点/美元 ÷ 50000 点/元）折出来的是 10 元/美元，等于按上游成本的
十倍计费；迁移 0047 把这个默认值改回 1:1，管理员手工调过的汇率则原样保留。

## 双库并行 migration

每次 schema 变更**必须**同时落地两份同号 SQL：
- `migrations/sqlite/NNNN_*.sql` — `INTEGER` + `TEXT DEFAULT (datetime('now'))`
- `migrations/postgres/NNNN_*.sql` — `BIGSERIAL` + `TEXT DEFAULT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS')`

并把文件名追加到 `src/db.rs` 的两个 `*_MIGRATIONS` 数组。

**Postgres 侧不要用 `TIMESTAMPTZ`、`BOOLEAN`、`NUMERIC`。** 连接池是 `sqlx::Any`，
它的类型表只有 bool / 整数 / 浮点 / text / blob；`timestamptz` 这类列是**值**层面
解不出来，一张表里有一个就整表读不出任何一行，跟 SQL 写得对不对无关。所以时间统一存
`YYYY-MM-DD HH:MM:SS`（UTC）文本、布尔存 0/1 整数——和 SQLite 逐字节一致，
因此字典序比较、`substr` 取日期、Rust 侧解析两边同一套代码。
`db::tests::postgres_migrations_avoid_types_the_any_driver_cannot_decode` 会守住这条。

同理，`SUM()` 在 Postgres 上会升成 `numeric`，一律写
`CAST(COALESCE(SUM(x), 0) AS BIGINT)`。

跨方言 SQL 助手：
- `db::q(kind, sql)` — Postgres 自动 `?` → `$1..$N`
- `db::now_expr(kind)` — 两边都产出同格式 UTC 文本
- `db::day_bucket(col)` / `db::ci_eq(kind, col)`

MySQL 曾经也在列表里，但 `Any` 把 `MEDIUMTEXT` 当成 BLOB、`TINYINT` 直接不支持，
而 `system_prompt` / `content` / `graph_json` 全是 `MEDIUMTEXT`；它连启动都过不去，
说明没有实际用户，因此整套移除，不留「能选但一定坏」的选项。

## 切换数据库后端

`yunova db-copy` 把一个已装好的库整体搬到另一个后端，SQLite → PostgreSQL 是主要用途：

```bash
yunova db-copy --from sqlite:///data/yunova.db \
               --to   postgres://user:pass@host:5432/yunova
```

它在目标库跑完 migration，按外键拓扑序逐表整行复制（`users.invited_by` 自引用留到
第二遍补），最后把 Postgres 的 id sequence 推到 `MAX(id)`，新插入才不会撞上已复制的行。
源库只读不改，配置文件也不动——验证完再把 `YUNOVA_DATABASE_URL` 或 `yunova.toml`
指向新库。目标库非空时默认拒绝执行（确认无误才加 `--allow-nonempty`）。

源库里出现 `TABLE_ORDER` 不认识的表时会直接报错而不是静默跳过，所以新增表要同时更新
`src/db_copy.rs` 里的那份列表。

## 本地开发

```bash
cargo run                # 后端 :3001
cd web && bun run dev    # 前端 :5173 → /api 走 vite proxy
```

测试在本地跑：`cargo test` + `cd web && npx tsc -b && bun test`。CI 只负责构建镜像、不跑测试。

## 品牌图标

图标由 `brand/generate.py` 参数化生成（三角新星，向下光芒兼作字母 Y 的竖笔），
favicon、站内 logo、Tauri 磁贴与 `.ico`/`.icns` 全部出自同一段定义，避免各自漂移。
修改图形后执行 `cd brand && python3 generate.py` 重新生成全部资源，
细节见 [`brand/README.md`](brand/README.md)。

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

手机是**遥控器，不是执行目标**：它驱动跑在云电脑或已绑定电脑上的任务。
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
- **不允许明文流量**。自托管实例必须走 HTTPS，否则会话 cookie 和设备令牌
  会在网络上裸奔；桌面客户端在非加密连接上会直接拒绝发送密码。

iOS 出包仍需 macOS 与开发者账号，Android 出包需 Android SDK。

### 本地电脑（桌面端应用）

`desktop/` 是独立的 `yunova-desktop`：一个**真正的桌面应用**，而不是只能在终端里
盯着的命令行程序。窗口里直接就是本站界面（Tauri + 系统 WebView 加载内置的站点
地址，默认 `https://chat.yunnet.top`），因此聊天、发任务、看执行过程都在这个窗口
里完成；同时它把这台电脑**变成执行目标**。

为什么是「装网页」而不是重写一遍前端：产品界面只有一份，桌面端跟着服务端一起
更新，不会落后一个版本。桌面端负责的恰好是浏览器做不到的那部分——保持连接器
在线、监管本机运行时、Agent 卡在审批时发系统通知、关掉窗口后仍留在托盘里继续跑。

它**主动**连回服务器并保持 WebSocket——个人电脑通常没有可达地址，主动出连是
唯一不需要端口映射的做法。

走的是与云电脑**完全相同**的 pi RPC JSONL：客户端在本机跑 `pi --mode rpc`，把它的
stdio 经由这条转发通道与服务器对接。因此 `DeviceTransport` 只是一根管子，
镜像、广播、审批、计费全部复用已经在沙箱上验证过的代码。

#### 两个前端，一个连接器

窗口是默认形态（用户双击图标运行的就是它）；`--headless` 保留原来的终端行为，
给服务器和开发机用——那里没有显示器可以开窗口。两者都驱动
`desktop/src/connector.rs`，所以协议与本机策略只存在一份实现：

```bash
./yunova-desktop              # 桌面应用（默认）
./yunova-desktop --headless   # 无界面，读环境变量，行为与旧版一致
```

桌面模式的设置存在 OS 配置目录（与设备令牌同一个目录，卸载时一并清理），
可在应用内的「本机设置」窗口里改：设备名、工作目录（带系统目录选择器）、
是否放开审批、是否开机自动连接；站点地址折叠在「改用其他站点（自托管）」里，
清空即恢复内置地址。环境变量若存在则**优先**，这样预置 `YUNOVA_DEVICE_URL`
的分发包不会被一份过期的设置文件悄悄覆盖。自托管构建可以用
`YUNOVA_DEFAULT_SITE_URL` 在编译期换掉内置地址。

#### 装完就在线：不填地址，也不再登录第二次

使用流程只剩两步：**安装、打开应用，在窗口里登录本站账号**——这台电脑随即出现在
「运行位置」里，既不用填站点地址，也**没有配对码**。

之所以能做到，是因为原先那两步问的都是应用自己已经知道、或用户刚刚已经回答过的
事情：

- **地址是产品自己的**。让用户填一个他没有理由知道的 URL，唯一的结果就是装完之后
  网页端显示「本地电脑（离线）」而客户端一切正常。所以地址内置、可覆盖，
  `is_configured()` 默认为真，启动即连。
- **窗口已经登录过了**。连接器于是从**自己这个 webview** 里读出站点的会话 cookie，
  用 `attach` 帧向服务器证明这台机器属于该账号（`src/agent_device.rs` 的
  `FromDevice::Attach`），换回一枚设备令牌。会话令牌比密码**更弱**：它本来就在那个
  webview 里、会过期、在网页端退出即失效，因此这条路换不到密码换不到的任何东西。
- **边界没有变宽**。cookie 是应用**读**自己的 webview，不是页面**递**进来的；远程
  页面依旧不在任何 capability 里，一行 IPC 也调不到。
- **登录状态是会变的**，所以首启时若还没登录，连接器停在「需要登录」，由
  `watch_site_login` 每两秒看一次窗口里是否已经有会话——用户登完页面，机器自己就
  上线了，不需要重启应用。显式点过「断开」或「解除绑定」会记住，watcher 不会把它
  们撤销。

账号回答了「这是谁的电脑」，剩下的只有「哪一台」，由客户端算出的机器指纹回答。
只有窗口里登不进去（或无界面模式）时，才需要在「本机设置」里用账号密码登录，
那个表单因此折叠了起来。

```bash
# 无界面模式（服务器/容器）：没有窗口可以读会话，所以这里仍然用账号登录
YUNOVA_DEVICE_WORKSPACE=/path/to/project \
./yunova-desktop --headless
# 首次运行会提示输入账号和密码；无人值守可用 YUNOVA_USERNAME / YUNOVA_PASSWORD
# 自托管实例用 YUNOVA_DEVICE_URL=https://your.site 覆盖内置地址
```

登录后客户端在本机保存一枚**设备令牌**（`0600`，位于 OS 配置目录而非工作目录，
因为工作目录正是 Agent 可以改写的地方），之后重连与重启都不再需要密码。
令牌每次登录都会轮换，服务器只存哈希；密码不落盘。指纹只决定**复用哪一行设备记录**，
永远不参与鉴权，所以猜中指纹不会拿到任何东西。

#### 远程页面拿不到本机控制权

这是整个桌面端设计围绕的边界，也是「显示服务器的页面」与「能改本机策略」这两件事
能同时成立的唯一原因：

- **站点页面单独一个 webview**，不出现在任何 capability 里，因此它的 IPC 调用会被
  Tauri 直接拒绝。服务器再怎么被攻破，也无法调用本地命令去放开审批或改工作目录。
- **本机控制面板是应用自带的本地页面**（`desktop/ui/`，无依赖手写 HTML），
  只有它出现在 `capabilities/local-panel.json` 的 `windows` 里。面板不由服务端提供，
  因此在还没配置任何服务器时（首次启动）也能打开——那恰恰是最需要它的时候。
- **自动绑定只读不写**。应用从自己的站点 webview 里**读**会话 cookie 来绑定本机；
  页面不能把任何东西递进这个进程，也不知道有人在读。方向是单向的，因此「装完即
  在线」没有换来一条新的攻击面。
- 这条边界由 `desktop/tests/capability_boundary.rs` 盯着：任何把站点窗口加进
  capability、去掉窗口白名单、或改用通配符的改动都会让测试失败；
  `desktop/tests/session_cookie_single_source.rs` 则盯着客户端读的 cookie 名与
  服务端 `auth::SESSION_COOKIE` 一致——两边一旦对不上，界面一切正常，机器却永远
  不会上线，是这套机制唯一会静默失败的地方。

二进制与安装包从 `/download` 页面下载。它不由本服务分发：镜像没有理由塞进五个平台的
构建，自托管实例也不该为了让用户装客户端而去镜像这些文件。页面在浏览时读 GitHub
Release 的资产列表，读不到（无出网、限流、内网部署）就退化成「最新发布页」链接，
而不是渲染出死链。安装包名由打包器生成并带版本号，因此页面按**扩展名 + 架构**匹配
而不是拼出一个可能不存在的文件名；独立二进制名仍由
`.github/workflows/desktop-release.yml` 产生，必须与 `web/src/lib/downloads.ts` 里的
`yunova-desktop-<target>` 对齐，`web/tests/downloads.test.ts` 盯着这两条约定。
安装包里的版本号来自根 `Cargo.toml` 的 `[workspace.package] version`（见「部署」），
所以它与服务端自报的版本总是同一个。

| 环境变量 | 默认 | 说明 |
| --- | --- | --- |
| `YUNOVA_DEVICE_URL` | 内置站点地址 | 站点地址，自动推导 WebSocket 端点；桌面模式下作为设置的初值/覆盖值 |
| `YUNOVA_USERNAME` / `YUNOVA_PASSWORD` | 交互输入 | 无人值守启动用；设置后跳过登录提示 |
| `YUNOVA_DEVICE_CONFIG_DIR` | OS 配置目录 | 设备令牌与桌面设置的存放位置 |
| `YUNOVA_DEVICE_WORKSPACE` | 无界面：当前目录；桌面：`~/Yunova` | **Agent 可操作的范围**，请指向具体项目 |
| `YUNOVA_DEVICE_NAME` | 主机名 | 列表里显示的名字 |
| `YUNOVA_DEVICE_AUTO_APPROVE` | 关 | 设为 `1` 放开审批，谨慎使用 |
| `YUNOVA_PI_BIN` | `pi` | 运行时可执行文件 |
| `YUNOVA_DEVICE_GATEWAY_URL` | 服务端配置 | 设备端运行时回调的网关地址（服务器上设置） |

构建桌面端需要系统 WebView。macOS 与 Windows 自带（WebKit / WebView2）；
Linux 上需要 `libwebkit2gtk-4.1-dev`、`libgtk-3-dev`、`librsvg2-dev`、`patchelf`、
`libayatana-appindicator3-dev`、`libsoup-3.0-dev`、`libxdo-dev`；打 `.deb`/`.AppImage`
还需要 `xdg-utils`（Tauri 的 Linux 打包器要找 `/usr/bin/xdg-open`，缺了会在
**打包阶段**失败，而不是编译阶段）。
打包用 `cd desktop && cargo tauri build`。

#### macOS 上的「已损坏」

发布的安装包**没有** Apple Developer ID 签名也没有公证，因此 macOS 会拦下来。
`bundle.macOS.signingIdentity` 设为 `-`（ad-hoc 临时签名）**不是**为了绕过它，
而是为了决定用户看到哪一种弹窗：

- 不写 `signingIdentity` 时，打包器**根本不跑 `codesign`**，`.app` 对资源和
  `Info.plist` 没有任何封封，只剩链接器给 Mach-O 加的那层 ad-hoc 签名。
  在 Apple Silicon 上这不够过 Gatekeeper：文件一旦带上浏览器下载的
  `com.apple.quarantine`，系统就报**「已损坏，无法打开」**——这条文案会把人
  直接引向废纸篓，而不是引向那个其实可以放行的「身份不明的开发者」对话框。
- 写成 `-` 后 Tauri 会对整个 bundle 执行 `codesign --force -s -`，封封成立，
  于是退回正常的「身份不明的开发者」路径，用户可以右键→打开放行。

ad-hoc 签名不证明任何身份，也**不会**去掉那个提示；要做到双击即装，只有真实
证书签名 + 公证一条路。CI 里若提供了 `APPLE_CERTIFICATE` / `APPLE_ID` 等凭据，
会**优先**于这里的 `-`，无需改配置。

那为什么不干脉去掉 `quarantine`？因为那是让用户关掉 Gatekeeper 对这个应用的
检查，不应该写成推荐做法；但在没有证书之前，它确实是唯一的应急手段，
所以 `/download` 页面在 macOS 卡片里直接写明了这条命令。

安全模型与云电脑**有本质区别**，协议设计也因此不同：沙箱是一次性且隔离的，
个人电脑不是。所以约束 Agent 的策略**由客户端拥有**，而不是交给服务器：

- **审批默认开启**。客户端自己写入 `tool_call` 门禁 extension，拦住
  `bash`/`powershell`/`write`/`edit`。指令可能来自手机，也可能受 Agent 读到的
  网页内容影响，所以在个人机器上「询问」才是安全默认值。
- **工作目录限定范围**，默认当前目录而非 `$HOME`；不配置也不会默认暂开整个用户目录。
- **服务器不下发要执行的命令**，只转发提示词，由本机运行时自己判断。
- **设备拿不到上游 key**，只收到指向本站网关的会话级令牌（文件权限 `0600`）。
- **移除设备立即断开**并使设备令牌失效；在那台电脑上重新登录即可再次绑定，
  因为权限的源头是账号而不是某一枚令牌。离线设备无法启动任务（返回 409）。

审批请求会广播到所有在线端，因此可以在网页或手机上处理跑在家里电脑上的任务；
拒绝后 Agent 会收到被阻止的原因，而不是默默卡住。

桌面应用还会在本机弹系统通知：任务卡在审批上而没人知道，等同于任务永远不会完成。
只有会阻塞运行时的对话（`confirm`/`select`/`input`/`editor`）会触发通知；
`notify` 这类发完就走的 UI 事件不会，否则用户很快就会学会忽略通知，
连唯一重要的那一条也一起丢掉。

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

版本号只写在一处：根 `Cargo.toml` 的 `[workspace.package] version`。服务端与桌面端
都用 `version.workspace = true` 继承它，因此 `/api/health`、管理后台的系统信息、
桌面端 `--version` 以及安装包文件名（含 Windows 文件版本资源和 macOS
`CFBundleShortVersionString`）全都是同一个数字。`desktop/tauri.conf.json` **故意不写**
`version`——Tauri 在缺省时回落到 crate 的 Cargo 版本；写上反而会覆盖它，而且不会
报错，只是把安装包命名成一个没人记得改的旧数字（配置里曾长期停在 `0.1.0`，而项目
已经在发 `v0.4.x`）。这条约束由 `desktop/tests/version_single_source.rs` 盯着。

打 tag 前先把 `[workspace.package] version` 改成同一个数字并提交，否则镜像和安装包
的自报版本会落后于 tag。
