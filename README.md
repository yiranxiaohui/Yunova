# Yunova

Yunova 是基于 Rust Axum + React 的自托管 Agent 整合平台，将多模型对话、
工蜂工具执行、图片与视频创作、媒体剪辑和工作流放在统一工作空间中，内置积分计费。

源码仓库：[yiranxiaohui/Yunova](https://github.com/yiranxiaohui/Yunova)。

## 从旧版本升级

项目品牌由 NovaChat 更名为 Yunova，服务端程序名为 `yunova`，工蜂程序名为
`yunova-worker`，镜像为 `ghcr.io/yiranxiaohui/yunova`。旧 GitHub 仓库地址会重定向，
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

## 计费模型（v2，2026-05）

v1 的「全局共享上游 + cost_chat / cost_image 两档定价」已被替换为：

- **多渠道（Channel）路由**：每种协议（OpenAI / Claude / Gemini）可配置 N 个上游 channel；按 `priority` 升序 fallback，第一个成功的 channel 处理请求。
- **按模型定价（ModelPricing）白名单**：`model_pricing` 表里没有的 model 直接 403 `NotWhitelisted`；`cost_credits=0` 表示放行不扣费。
- **扣费链路**：`try_deduct_for_model` 在请求前查 `model_pricing` 一次性扣；失败全部 channel 后通过 `cost_for_model` 二次查价精确退款。
- **Ledger reason 格式**：成功 `chat_<model>@<channel_name>`，全失败退款 `refund_chat_<model>_all_failed`。

### 数据流

```
POST /api/chat {model:"gpt-5",...}
 → lookup_pricing("gpt-5")    → cost=3, kind="chat"
 → try_deduct_for_model       → balance -= 3
 → list_channels_for_model    → [ch1(p=10), ch2(p=20)]
 → try ch1 → 503 → try ch2 → 200 OK stream
 → ledger: -3 "chat_gpt-5@Azure"
全失败 → grant(+3 "refund_chat_gpt-5_all_failed") → 502
```

### 后台管理

`Admin → Channels`：CRUD 渠道、启用/停用、绑定 model 列表（每行 `model` 或 `model=client_model=upstream_id`）。
`Admin → Pricing`：CRUD model 白名单 + 单价。

旧的「Shared Backend」面板与 KV (`shared_chat_openai_*` 等) 暂时保留只用于历史 seed，新链路不再读它们。

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

测试在本地跑：`cargo test` + `cd web && npx tsc -b`。CI 只负责构建镜像、不跑测试。

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
访问本地服务，不经过 Yunova 后端；自定义任务不扣平台积分，凭据和任务记录
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

## 部署

正式版本镜像 tag：`docker.yunnet.top/github/yiranxiaohui/yunova:X.Y.Z`。

- push `main` → GitHub Actions 构建开发镜像
- push `vX.Y.Z` → GitHub Actions 构建正式镜像与 Worker 多平台附件
- 两个发布工作流成功后，由 Codex 从可信服务器 SSH 部署生产环境
- migration 在容器启动时自动跑
- 默认版本策略只递增最后一位：`vX.Y.Z` → `vX.Y.(Z+1)`
