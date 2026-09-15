# Yunova 工蜂 (Worker)

旧版 `NOVACHAT_WORKER_*` 环境变量仍可使用；推荐改为 `YUNOVA_WORKER_*`，
同时存在时新名称优先。升级已有 systemd 服务时保留原配对配置，并更新可执行文件路径。

工蜂是部署到**你自己服务器**上的轻量远程执行器。它主动连回 Yunova，由网页端的 AI agent（Claude）操控，在你的服务器上执行 shell 命令、读写文件——相当于一个远程的、受网页驱动的编码 agent 的「手」。

思考循环跑在 Yunova 后端（复用站点的模型渠道与额度体系），工蜂本身**不持有任何模型 key**，只负责执行下发的工具调用并回传结果。

## 能力（v1）

工蜂只会三件事：

- `shell` —— 执行 shell 命令，回传 stdout/stderr 与退出码
- `read_file` —— 读取指定路径文件
- `write_file` —— 写入/覆盖指定路径文件（自动创建父目录）

单条工具输出超过 64KB 会被截断。

## 安全模型

- **配对码鉴权**：每台工蜂用一个一次性配对码连接，配对码在服务端仅以 SHA-256 哈希存储。
- **归属隔离**：你只能操控自己账号名下的工蜂。
- **分级审批**：`read_file` 自动执行；`shell` 与 `write_file` 默认需要你在网页上逐条「批准」，也可在会话里打开「自动批准」一键放手。
- ⚠️ **工蜂能在它运行的身份下执行任意命令。只把它部署到你自己信任的机器，并强烈建议用非 root 的最小权限用户运行。** 删除工蜂即吊销其配对码并断开连接。

## 获取二进制

**推荐：下载预构建版本。** 每个 `vX.Y.Z` 版本发布时，CI 会构建二进制并附在 [GitHub Releases](../../releases)，Linux 版为静态 musl、无运行时依赖：

- `yunova-worker-x86_64-unknown-linux-musl.tar.gz` —— x86_64 服务器
- `yunova-worker-aarch64-unknown-linux-musl.tar.gz` —— ARM64 服务器
- `yunova-worker-x86_64-pc-windows-msvc.zip` —— Windows（x64，含 `yunova-worker.exe`）

```bash
# Linux
curl -fsSL <release-asset-url> | tar xz
```

Windows：下载 `.zip` 解压得到 `yunova-worker.exe`。其 `shell` 工具在 Windows 上经 `cmd /C` 执行命令（Linux 经 `sh -c`）。

**或自行构建**（按项目规约：构建在本地 / CI 完成，**不在线上机器执行**）：

```bash
cargo build -p yunova-worker --release
# 产物：target/release/yunova-worker（单个可执行文件）
```

## 部署运行

把二进制拷到目标服务器，设三个环境变量后运行：

```bash
YUNOVA_WORKER_URL=wss://你的域名/api/worker/connect \
YUNOVA_WORKER_TOKEN=<在网页「工蜂」页生成的配对码> \
YUNOVA_WORKER_NAME=$(hostname) \
./yunova-worker
```

| 环境变量 | 必填 | 说明 |
|----------|------|------|
| `YUNOVA_WORKER_URL` | 是 | Yunova 的 WS 接入地址，形如 `wss://host/api/worker/connect`（本地调试可用 `ws://127.0.0.1:3000/api/worker/connect`） |
| `YUNOVA_WORKER_TOKEN` | 是 | 配对码。网页登录 → 侧边栏「工蜂」→「生成配对码」获取（仅显示一次） |
| `YUNOVA_WORKER_NAME` | 否 | 工蜂显示名，默认取 `hostname` |

连接成功后日志会打印 `[worker] 鉴权成功，worker_id=…`，网页「工蜂」列表里该机即变为「在线」。断线会自动重连（指数退避，上限 30 秒）。

### 作为 systemd 服务（可选）

```ini
# /etc/systemd/system/yunova-worker.service
[Unit]
Description=Yunova Worker
After=network-online.target

[Service]
# 建议用非 root 的专用用户
User=yunova
Environment=YUNOVA_WORKER_URL=wss://你的域名/api/worker/connect
Environment=YUNOVA_WORKER_TOKEN=粘贴配对码
ExecStart=/opt/yunova-worker/yunova-worker
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

```bash
systemctl daemon-reload && systemctl enable --now yunova-worker
```
