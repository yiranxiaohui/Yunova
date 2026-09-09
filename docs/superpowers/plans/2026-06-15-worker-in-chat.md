# 工蜂融入对话 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 取消独立 `/worker` 页面，把工蜂作为聊天页内的一个「模式」：用户在对话里开启并配置工蜂，工蜂会话进侧边栏与普通对话混排、可回看，文本走 markdown、工具结果超长可折叠，工蜂管理挪到设置。

**Architecture:** 不动数据库表。`worker_sessions`/`worker_messages` 保持平行于 `conversations`/`messages`。后端只加一个「列工蜂会话」GET 接口。前端抽共享 `Markdown` 组件，新增工蜂事件渲染组件与历史回看解析器，给 `ChatPage` 加工蜂模式（经路由 `/w/:id` 进入），侧边栏合并两个列表来源，工蜂管理迁到 `SettingsDialog`，删除 `WorkerPage` 与 `/worker` 路由。

**Tech Stack:** Rust/Axum + sqlx（后端），React 19 + Vite + Tailwind 4 + shadcn/ui + react-router-dom v6 + react-markdown/remark-gfm（前端）。

**关于测试：** 本仓库无测试运行器（见 CLAUDE.md）。按服务器规约不做发布打包；前端用 `bunx tsc -b` 做类型检查，后端用 `cargo check`（注意它会经 build.rs 触发一次 `web/` 的 bun 构建，属工具链行为）。每个任务以类型检查通过 + 手动验证说明 + 提交收尾。

---

## File Structure

新建：
- `web/src/components/app/Markdown.tsx` — 共享 markdown 渲染：导出 `CodeBlock` 与 `Markdown`。
- `web/src/components/app/WorkerEvents.tsx` — 工蜂事件渲染：`WorkerRow`（text/error/tool_call/tool_result/approval/done）+ `ToolResult`（超长折叠）。

修改：
- `src/worker.rs` — 新增 `list_sessions` GET handler 与路由。
- `web/src/lib/worker.ts` — `WorkerSession` 类型、`workerApi.sessions()`、`replayMessages()` 解析器。
- `web/src/pages/ChatPage.tsx` — 改用共享 `CodeBlock`；新增工蜂模式（路由 `/w/:id`、开关、配置条、SSE 发送、事件渲染、历史回看）。
- `web/src/components/app/Sidebar.tsx` — 合并 conversations + worker_sessions，按 kind 渲染与路由。
- `web/src/components/app/SettingsDialog.tsx` — 新增「工蜂」分区（配对/部署/列表/删除）。
- `web/src/App.tsx` — 新增 `/w/:id` 路由指向 ChatPage；删除 `/worker` 路由与 import。

删除：
- `web/src/pages/WorkerPage.tsx`

---

## Task 1: 后端「列工蜂会话」接口

**Files:**
- Modify: `src/worker.rs`（`routes()` 与新增 `list_sessions` handler）

- [ ] **Step 1: 在 `routes()` 注册 GET /worker/sessions**

在 `src/worker.rs` 的 `routes()` 中，把这一行：

```rust
        .route("/worker/sessions", post(create_session))
```

改为：

```rust
        .route("/worker/sessions", post(create_session).get(list_sessions))
```

- [ ] **Step 2: 新增 `list_sessions` handler**

参考同文件 `list_messages` 的鉴权与查询风格，在 `create_session` 附近新增：

```rust
async fn list_sessions(
    Extension(installed): Extension<crate::db::InstalledState>,
    Extension(user): Extension<crate::auth::CurrentUser>,
) -> impl IntoResponse {
    let rows: Vec<(i64, i64, String, String)> = sqlx::query_as(&crate::db::q(
        installed.kind,
        "SELECT s.id, s.worker_id, s.title, s.updated_at \
         FROM worker_sessions s WHERE s.user_id = ? ORDER BY s.updated_at DESC",
    ))
    .bind(user.id)
    .fetch_all(&installed.pool)
    .await
    .unwrap_or_default();
    let out: Vec<_> = rows
        .into_iter()
        .map(|(id, worker_id, title, updated_at)| {
            json!({"id": id, "worker_id": worker_id, "title": title, "updated_at": updated_at})
        })
        .collect();
    Json(out).into_response()
}
```

> 注：`Extension`/`IntoResponse`/`Json`/`json!`/`CurrentUser`/`InstalledState` 的引入路径与 `list_messages` 完全一致；若该文件顶部已 `use` 则无需重复。确认 `list_messages` 的实际 import 写法并照抄。

- [ ] **Step 3: 类型检查**

Run: `cargo check`
Expected: 编译通过，无关于 `list_sessions` 的错误。

- [ ] **Step 4: 手动验证**

启动 server，登录后 `curl -s --cookie <session> http://127.0.0.1:3000/api/worker/sessions`，返回 JSON 数组（无会话时为 `[]`）。

- [ ] **Step 5: Commit**

```bash
git add src/worker.rs
git commit -m "feat(worker): 新增 GET /api/worker/sessions 列出工蜂会话"
```

---

## Task 2: 共享 Markdown 组件

**Files:**
- Create: `web/src/components/app/Markdown.tsx`
- Modify: `web/src/pages/ChatPage.tsx`（删除本地 `CodeBlock`，改 import）

- [ ] **Step 1: 创建 `Markdown.tsx`**

把 `ChatPage.tsx` 第 411 行起的 `CodeBlock` 完整搬过来并导出，另加一个通用 `Markdown`。先读取 `ChatPage.tsx` 的 `CodeBlock` 全文（约 411–470 行）确保一字不差搬运。文件内容：

```tsx
import { useEffect, useRef, useState } from "react"
import ReactMarkdown from "react-markdown"
import remarkGfm from "remark-gfm"
import { Check, Copy } from "lucide-react"
import { cn } from "@/lib/utils"

// 从 ChatPage 抽出的代码块（带复制按钮）。粘贴 ChatPage 原 CodeBlock 的完整实现，
// 仅把 `function CodeBlock` 前加 `export`。
export function CodeBlock({ children, ...rest }: React.HTMLAttributes<HTMLPreElement>) {
  const preRef = useRef<HTMLPreElement>(null)
  const [done, setDone] = useState(false)
  const timerRef = useRef<number | null>(null)
  useEffect(
    () => () => {
      if (timerRef.current) window.clearTimeout(timerRef.current)
    },
    []
  )
  async function onCopy(e: React.MouseEvent) {
    e.stopPropagation()
    e.preventDefault()
    const text = preRef.current?.innerText ?? ""
    try {
      await navigator.clipboard.writeText(text)
    } catch {
      const ta = document.createElement("textarea")
      ta.value = text
      ta.style.position = "fixed"
      ta.style.opacity = "0"
      document.body.appendChild(ta)
      ta.select()
      try {
        document.execCommand("copy")
      } catch {
        /* noop */
      }
      document.body.removeChild(ta)
    }
    setDone(true)
    if (timerRef.current) window.clearTimeout(timerRef.current)
    timerRef.current = window.setTimeout(() => setDone(false), 1500)
  }
  return (
    <div className="group/code relative my-2">
      <button
        type="button"
        onClick={onCopy}
        title={done ? "已复制" : "复制代码"}
        aria-label={done ? "已复制" : "复制代码"}
        className={cn(
          "absolute right-2 top-2 z-10 inline-flex items-center gap-1 rounded-md border border-border bg-background/80 px-2 py-1 text-xs backdrop-blur hover:bg-accent",
          "opacity-0 transition-opacity group-hover/code:opacity-100 focus-visible:opacity-100"
        )}
      >
        {done ? <Check className="size-3" /> : <Copy className="size-3" />}
        {done ? "已复制" : "复制"}
      </button>
      <pre ref={preRef} {...rest}>
        {children}
      </pre>
    </div>
  )
}

const PROSE = cn(
  "prose prose-sm dark:prose-invert min-w-0 max-w-none",
  "[&_pre]:max-w-full [&_pre]:overflow-x-auto [&_pre]:whitespace-pre",
  "[&_*]:break-words [&_a]:break-all",
  "prose-pre:bg-muted prose-pre:text-foreground prose-pre:border prose-pre:border-border",
  "[&_pre_code]:!text-foreground [&_pre_code]:!bg-transparent",
  "prose-code:rounded prose-code:bg-muted prose-code:px-1 prose-code:py-0.5 prose-code:text-[0.85em] prose-code:font-normal prose-code:before:content-none prose-code:after:content-none"
)

/** 通用 markdown 渲染（不含聊天页特有的图片发布浮层）。 */
export function Markdown({ children, className }: { children: string; className?: string }) {
  return (
    <div className={cn(PROSE, className)}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          pre: ({ node: _node, ...props }) => <CodeBlock {...props} />,
        }}
      >
        {children || "…"}
      </ReactMarkdown>
    </div>
  )
}
```

> 关键：Step 1 的 `CodeBlock` 必须与 ChatPage 现有实现逐字一致（含 `<pre ref={preRef} {...rest}>` 收尾部分）。读 ChatPage 411 行起确认结尾几行后再粘贴。

- [ ] **Step 2: ChatPage 删除本地 CodeBlock，改用共享的**

在 `web/src/pages/ChatPage.tsx`：
1. 删除本地 `function CodeBlock(...) { ... }` 整段定义（约 411 行起）。
2. 顶部 import 区加：`import { CodeBlock } from "@/components/app/Markdown"`
3. 其内联 `<ReactMarkdown components={{ pre: ... => <CodeBlock .../> }}>` 引用保持不变（现在指向 import 的 CodeBlock）。

> ChatPage 自身的内联 `ReactMarkdown`（带 img 发布浮层）**保持不动**，只换 CodeBlock 来源。

- [ ] **Step 3: 类型检查**

Run: `bunx tsc -b` （在 `web/` 目录下）
Expected: 无类型错误；无「CodeBlock 重复定义/未定义」报错。

- [ ] **Step 4: 手动验证**

启动 server，普通对话里发一条含代码块和 `**bold**` 的消息，确认渲染与复制按钮如旧（无回归）。

- [ ] **Step 5: Commit**

```bash
git add web/src/components/app/Markdown.tsx web/src/pages/ChatPage.tsx
git commit -m "refactor(web): 抽出共享 Markdown/CodeBlock 组件，ChatPage 改用"
```

---

## Task 3: worker.ts —— 会话列表类型与历史回看解析器

**Files:**
- Modify: `web/src/lib/worker.ts`

- [ ] **Step 1: 新增 `WorkerSession` 类型与 `sessions()` 客户端**

在 `web/src/lib/worker.ts` 的 `Worker` 接口附近加：

```ts
export interface WorkerSession {
  id: number
  worker_id: number
  title: string
  updated_at: string
}
```

在 `workerApi` 对象里加方法（紧跟 `createSession` 之后）：

```ts
  async sessions(): Promise<WorkerSession[]> {
    return jsonOrThrow(
      await fetch("/api/worker/sessions", { credentials: "same-origin" })
    )
  },
```

- [ ] **Step 2: 新增 `replayMessages` 解析器**

在文件末尾（`sendAgentMessage` 之后）追加。它把存库的 worker_messages 还原成与实时 SSE 一致的 `AgentEvent[]`：

```ts
/**
 * 把存库的 worker_messages 还原为渲染事件，与实时 SSE 的事件类型一致。
 * - user      -> 一条 text 事件（前缀 🧑，与发送时一致）
 * - assistant -> content block 数组：text 块 => text 事件；tool_use 块 => tool_call 事件
 * - tool      -> tool_result 事件（解析出 output 字符串）
 * 单条解析失败时降级为纯文本，不影响整体。
 */
export function replayMessages(rows: WorkerMessage[]): AgentEvent[] {
  const out: AgentEvent[] = []
  for (const m of rows) {
    if (m.role === "user") {
      out.push({ type: "text", data: `🧑 ${m.content}` })
      continue
    }
    if (m.role === "assistant") {
      let blocks: unknown
      try {
        blocks = JSON.parse(m.content)
      } catch {
        out.push({ type: "text", data: m.content })
        continue
      }
      if (Array.isArray(blocks)) {
        for (const b of blocks as Array<Record<string, unknown>>) {
          if (b?.type === "text" && typeof b.text === "string") {
            out.push({ type: "text", data: b.text })
          } else if (b?.type === "tool_use") {
            out.push({
              type: "tool_call",
              data: { tool: b.name, input: b.input, call_id: b.id },
            })
          }
        }
      } else {
        out.push({ type: "text", data: m.content })
      }
      continue
    }
    if (m.role === "tool") {
      let output = m.content
      try {
        const v = JSON.parse(m.content) as Record<string, unknown>
        if (typeof v.output === "string") output = v.output
        else if (typeof v.content === "string") output = v.content
      } catch {
        /* 保留原文 */
      }
      out.push({ type: "tool_result", data: { output } })
      continue
    }
    // 未知 role：降级纯文本
    out.push({ type: "text", data: m.content })
  }
  return out
}
```

> 注：`tool` 行的 JSON 形状以 `src/worker.rs` 落库为准（搜索 `role "tool"` 落库处确认是 `{output}` 还是嵌套 `content`）。本解析器对两种都做了兜底；实现时核对一次实际字段名，若不同则调整 `v.output`/`v.content` 取值。

- [ ] **Step 3: 类型检查**

Run: `bunx tsc -b`（`web/` 下）
Expected: 无类型错误。

- [ ] **Step 4: Commit**

```bash
git add web/src/lib/worker.ts
git commit -m "feat(web): worker 客户端加会话列表与历史回看解析器"
```

---

## Task 4: 工蜂事件渲染组件（含超长折叠）

**Files:**
- Create: `web/src/components/app/WorkerEvents.tsx`

- [ ] **Step 1: 创建 `WorkerEvents.tsx`**

迁移并升级现 `WorkerPage` 的 `Row`：text/error 走 markdown，tool_result 超长折叠。

```tsx
import { useState } from "react"
import { Check, Wrench, X } from "lucide-react"
import { Button } from "@/components/ui/button"
import { cn } from "@/lib/utils"
import { Markdown } from "@/components/app/Markdown"
import type { AgentEvent } from "@/lib/worker"

export type WorkerLogItem = AgentEvent & { id: number; resolved?: boolean }

const COLLAPSE_LINES = 12

/** 工具结果：≤12 行完整展开；超长默认 clamp + 渐隐 + 展开/收起切换。 */
function ToolResult({ output }: { output: string }) {
  const lineCount = output ? output.split("\n").length : 0
  const long = lineCount > COLLAPSE_LINES
  const [open, setOpen] = useState(false)
  const collapsed = long && !open
  return (
    <div className="relative">
      <pre
        className={cn(
          "overflow-auto whitespace-pre-wrap rounded bg-muted p-2 text-xs",
          collapsed && "max-h-48 overflow-hidden"
        )}
      >
        {output}
      </pre>
      {collapsed && (
        <div className="pointer-events-none absolute inset-x-0 bottom-0 h-10 rounded-b bg-gradient-to-t from-muted to-transparent" />
      )}
      {long && (
        <button
          type="button"
          onClick={() => setOpen((v) => !v)}
          className="mt-1 text-xs text-primary hover:underline"
        >
          {open ? "收起" : `展开（共 ${lineCount} 行）`}
        </button>
      )}
    </div>
  )
}

export function WorkerRow({
  item,
  onDecide,
}: {
  item: WorkerLogItem
  onDecide: (i: WorkerLogItem, decision: boolean) => void
}) {
  switch (item.type) {
    case "text":
      return (
        <Markdown>
          {typeof item.data === "string" ? item.data : JSON.stringify(item.data)}
        </Markdown>
      )
    case "tool_call":
      return (
        <div className="flex items-start gap-1.5 text-blue-600 dark:text-blue-400">
          <Wrench className="mt-0.5 size-3.5 shrink-0" />
          <span className="font-medium">{item.data?.tool}</span>
          <code className="break-all text-xs">{JSON.stringify(item.data?.input)}</code>
        </div>
      )
    case "tool_result":
      return <ToolResult output={item.data?.output ?? ""} />
    case "error":
      return (
        <div className="text-red-600 dark:text-red-400">
          <span className="mr-1">⚠</span>
          <Markdown className="inline">
            {typeof item.data === "string" ? item.data : JSON.stringify(item.data)}
          </Markdown>
        </div>
      )
    case "done":
      return <div className="py-1 text-center text-xs text-muted-foreground">— 完成 —</div>
    case "approval_required":
      return (
        <div className="rounded-md border border-yellow-400 bg-yellow-50 p-2 dark:border-yellow-700 dark:bg-yellow-950/30">
          <div className="text-xs">
            需批准 <b>{item.data?.tool}</b>：
            <code className="break-all">{JSON.stringify(item.data?.input)}</code>
          </div>
          <div className="mt-2 flex gap-2">
            <Button
              size="sm"
              variant="outline"
              className="gap-1"
              disabled={item.resolved}
              onClick={() => onDecide(item, true)}
            >
              <Check className="size-3.5" /> 批准
            </Button>
            <Button
              size="sm"
              variant="outline"
              className="gap-1"
              disabled={item.resolved}
              onClick={() => onDecide(item, false)}
            >
              <X className="size-3.5" /> 拒绝
            </Button>
            {item.resolved && (
              <span className="self-center text-xs text-muted-foreground">已处理</span>
            )}
          </div>
        </div>
      )
    default:
      return null
  }
}
```

- [ ] **Step 2: 类型检查**

Run: `bunx tsc -b`（`web/` 下）
Expected: 无类型错误。

- [ ] **Step 3: Commit**

```bash
git add web/src/components/app/WorkerEvents.tsx
git commit -m "feat(web): 工蜂事件渲染组件，工具结果超长折叠 + 文本走 markdown"
```

---

## Task 5: ChatPage 工蜂模式

**Files:**
- Modify: `web/src/pages/ChatPage.tsx`
- Modify: `web/src/App.tsx`（新增 `/w/:id` 路由）

> 本任务最复杂。原则：工蜂逻辑尽量隔离，不污染普通对话路径。普通对话的 state/send 完全不动；工蜂模式用独立 state 与独立渲染分支。

- [ ] **Step 1: App 加 `/w/:id` 路由**

在 `web/src/App.tsx`，复制 `/c/:id` 那段 `<Route>`，新增：

```tsx
          <Route
            path="/w/:id"
            element={
              <Protected>
                <ChatPage />
              </Protected>
            }
          />
```

- [ ] **Step 2: ChatPage 识别工蜂模式**

在 `ChatPage()` 顶部（`const { id: paramId } = useParams()` 附近）加：

```tsx
import { useLocation } from "react-router-dom" // 合并进已有的 react-router-dom import
// ...
const location = useLocation()
const isWorkerRoute = location.pathname.startsWith("/w/")
const workerSessionId = isWorkerRoute && paramId ? Number(paramId) : null
```

- [ ] **Step 3: 工蜂模式 state**

在组件 state 区加（与普通对话 state 并列，互不干扰）：

```tsx
import { workerApi, sendAgentMessage, replayMessages, type Worker, type WorkerLogItem } from "@/lib/worker"
import { WorkerRow } from "@/components/app/WorkerEvents"
// ...
const [workerMode, setWorkerMode] = useState(false)
const [workers, setWorkers] = useState<Worker[]>([])
const [workerId, setWorkerId] = useState<number | null>(null)
const [workerModel, setWorkerModel] = useState("claude-opus-4-8")
const [autoApprove, setAutoApprove] = useState(false)
const [workerLog, setWorkerLog] = useState<WorkerLogItem[]>([])
const [activeWorkerSession, setActiveWorkerSession] = useState<number | null>(null)
const [workerSending, setWorkerSending] = useState(false)
const workerSeq = useRef(0)
const workerAbort = useRef<(() => void) | null>(null)
const pushWorker = (e: AgentEvent) =>
  setWorkerLog((l) => [...l, { ...e, id: workerSeq.current++ }])
```

> `WorkerLogItem` 从 `@/components/app/WorkerEvents` 导出（Task 4），`AgentEvent` 从 `@/lib/worker`。按需补 import。

- [ ] **Step 4: 进入工蜂模式 + 历史回看**

加一个 effect：当路由是 `/w/:id` 时拉历史并进入工蜂模式。

```tsx
useEffect(() => {
  if (workerSessionId == null) return
  setWorkerMode(true)
  setActiveWorkerSession(workerSessionId)
  setWorkerLog([])
  workerApi
    .messages(workerSessionId)
    .then((rows) => {
      const events = replayMessages(rows)
      setWorkerLog(events.map((e) => ({ ...e, id: workerSeq.current++ })))
    })
    .catch(() => {})
}, [workerSessionId])

// 工蜂模式下加载在线工蜂列表
useEffect(() => {
  if (!workerMode) return
  workerApi.list().then(setWorkers).catch(() => {})
}, [workerMode])

// 离开/切换时中断进行中的工蜂会话
useEffect(() => () => workerAbort.current?.(), [])
```

- [ ] **Step 5: 工蜂发送 + 批准函数**

```tsx
async function sendWorker(text: string) {
  if (workerId == null || !text.trim() || workerSending) return
  let s = activeWorkerSession
  try {
    if (s == null) {
      s = (await workerApi.createSession(workerId)).id
      setActiveWorkerSession(s)
      nav(`/w/${s}`, { replace: true }) // 让侧边栏出现该会话、URL 可回看
    }
  } catch (e) {
    pushWorker({ type: "error", data: `创建会话失败：${String(e)}` })
    return
  }
  pushWorker({ type: "text", data: `🧑 ${text.trim()}` })
  setWorkerSending(true)
  workerAbort.current = sendAgentMessage(
    s,
    { worker_id: workerId, model: workerModel, text: text.trim(), auto_approve: autoApprove },
    (e) => {
      pushWorker(e)
      if (e.type === "done" || e.type === "error") {
        setWorkerSending(false)
        workerAbort.current = null
      }
    }
  )
}

async function decideWorker(item: WorkerLogItem, decision: boolean) {
  if (activeWorkerSession == null) return
  await workerApi.approve(activeWorkerSession, item.data?.call_id, decision).catch(() => {})
  setWorkerLog((l) => l.map((x) => (x.id === item.id ? { ...x, resolved: true } : x)))
}
```

- [ ] **Step 6: 渲染分支 —— 工蜂模式下用工蜂消息区**

在主消息列表渲染处，按 `workerMode` 分支。普通分支保持原样；工蜂分支渲染 `workerLog`：

```tsx
{workerMode ? (
  <div className="space-y-2">
    {workerLog.length === 0 && (
      <div className="text-muted-foreground">发条消息让工蜂开始工作。</div>
    )}
    {workerLog.map((item) => (
      <WorkerRow key={item.id} item={item} onDecide={decideWorker} />
    ))}
  </div>
) : (
  /* 原有普通对话消息渲染，保持不动 */
)}
```

> 实现时定位现有消息 `.map` 渲染块，把它包进 `workerMode ? (...) : (原块)`。不要改动原块内部。

- [ ] **Step 7: 工蜂开关 + 配置条**

在输入框上方加开关与配置条（仅在非工蜂会话回看时允许切换；从 `/w/:id` 进来的固定为工蜂模式）：

```tsx
<div className="flex flex-wrap items-center gap-3 px-1 text-sm">
  <label className="flex cursor-pointer items-center gap-1.5">
    <input
      type="checkbox"
      className="size-4 accent-primary"
      checked={workerMode}
      onChange={(e) => setWorkerMode(e.target.checked)}
      disabled={workerSessionId != null}
    />
    <Bot className="size-4" /> <span>工蜂模式</span>
  </label>
  {workerMode && (
    <>
      <select
        className="h-8 rounded-md border bg-background px-2 text-sm"
        value={workerId ?? ""}
        onChange={(e) => setWorkerId(e.target.value ? Number(e.target.value) : null)}
      >
        <option value="">选择工蜂…</option>
        {workers
          .filter((w) => w.online)
          .map((w) => (
            <option key={w.id} value={w.id}>
              {w.name}
            </option>
          ))}
      </select>
      <Input
        className="h-8 w-44"
        value={workerModel}
        onChange={(e) => setWorkerModel(e.target.value)}
        placeholder="模型"
      />
      <label className="flex cursor-pointer items-center gap-1.5">
        <input
          type="checkbox"
          className="size-4 accent-primary"
          checked={autoApprove}
          onChange={(e) => setAutoApprove(e.target.checked)}
        />
        <span>自动批准</span>
      </label>
      {workers.filter((w) => w.online).length === 0 && (
        <span className="text-xs text-muted-foreground">没有在线工蜂，去设置里配对</span>
      )}
    </>
  )}
</div>
```

> `Bot` 从 `lucide-react` import；`Input` 已在 ChatPage 用到。

- [ ] **Step 8: 发送入口分流**

找到现有发送处理（`send()` 与输入框的回车/发送按钮 onClick）。在工蜂模式下改走 `sendWorker`：

```tsx
// 在 send() 开头：
if (workerMode) {
  const text = input // 用 ChatPage 现有的输入 state 名
  setInput("")
  await sendWorker(text)
  return
}
// ...原有普通对话发送逻辑不变
```

发送按钮的 `disabled` 在工蜂模式下用 `workerSending || workerId == null || !input.trim()`；非工蜂模式保持原条件。

> 实现时确认 ChatPage 输入框 state 的真实变量名（搜索 `setInput`/`value=` 于 textarea），用真名替换 `input`/`setInput`。

- [ ] **Step 9: 类型检查**

Run: `bunx tsc -b`（`web/` 下）
Expected: 无类型错误。

- [ ] **Step 10: 手动验证**

启动 server（需有一个在线工蜂；没有则先在设置里配对并本地跑 `yunova-worker` 连上）：
1. 普通对话：发消息、切换会话、回看——全部如旧（无回归）。
2. 开「工蜂模式」→ 选在线工蜂 → 发消息：看到 text/tool_call/tool_result 渲染，markdown 生效，超长工具结果折叠可展开。
3. 触发一次需批准的工具，点「批准/拒绝」生效。
4. 发完后 URL 变为 `/w/:id`，刷新页面能回看该会话历史。

- [ ] **Step 11: Commit**

```bash
git add web/src/pages/ChatPage.tsx web/src/App.tsx
git commit -m "feat(web): 聊天页内置工蜂模式（开关/配置/SSE/回看），路由 /w/:id"
```

---

## Task 6: 侧边栏混排

**Files:**
- Modify: `web/src/components/app/Sidebar.tsx`

- [ ] **Step 1: 拉取并合并两个列表**

定位现有拉 conversations 的 effect（约 75 行 `conversationsApi.list().then(...)`）。改为同时拉 worker_sessions，合并为带 `kind` 的统一列表。新增本地类型：

```tsx
import { workerApi, type WorkerSession } from "@/lib/worker"

type SidebarItem =
  | { kind: "chat"; id: number; title: string; updated_at: string }
  | { kind: "worker"; id: number; title: string; updated_at: string }
```

合并逻辑（替换原 setItems 流程）：

```tsx
Promise.all([
  conversationsApi.list().catch(() => [] as Conversation[]),
  workerApi.sessions().catch(() => [] as WorkerSession[]),
]).then(([convs, sessions]) => {
  const merged: SidebarItem[] = [
    ...convs.map((c) => ({ kind: "chat" as const, id: c.id, title: c.title, updated_at: c.updated_at })),
    ...sessions.map((s) => ({ kind: "worker" as const, id: s.id, title: s.title, updated_at: s.updated_at })),
  ].sort((a, b) => (a.updated_at < b.updated_at ? 1 : -1))
  setItems(merged)
})
```

> 把 `items`/`filtered` 的类型从 `Conversation[]` 改为 `SidebarItem[]`。搜索过滤逻辑（`filtered`）按 `title` 过滤，对两种 kind 都适用，无需改判断条件——但若过滤里访问了 `system_prompt` 等 chat 专有字段，需去掉。

- [ ] **Step 2: 渲染按 kind 区分**

在列表 `.map((c) => ...)` 内：
- `to` 改为按 kind：`c.kind === "worker" ? /w/${c.id} : /c/${c.id}`。
- 工蜂条目标题前加 `Bot` 图标区分：

```tsx
import { Bot } from "lucide-react" // 合并进已有 lucide import
// 标题行：
<div className="flex items-center gap-1 truncate text-sm font-medium">
  {c.kind === "worker" && <Bot className="size-3.5 shrink-0 text-primary" />}
  <span className="truncate">{c.title}</span>
</div>
```

`active` 判定改为同时比 kind：`activeId === c.id`（若不同 kind 可能 id 撞号，建议改成比较当前路由——用 `useLocation` 判断 `/w/` 前缀 + id）。

- [ ] **Step 3: 删除/重命名按 kind 分流**

现有菜单的删除/重命名调用 `conversationsApi`。改为：

```tsx
// 删除：
c.kind === "worker"
  ? await workerApi.remove(c.id).catch(() => {})  // 注意：这是删 worker session 还是 worker？见下注
  : await conversationsApi.remove(c.id)
```

> ⚠️ 注意 `workerApi.remove(id)` 删的是**工蜂设备**（`DELETE /worker/{id}`），不是会话。侧边栏要删的是**会话**。当前后端无「删 worker session」接口。实现选项：
> - (推荐) 本任务先只支持工蜂会话的「打开/回看」，菜单里对 worker 条目隐藏删除/重命名（或置灰），把「删会话」接口留到后续；
> - 或在 Task 1 一并加 `DELETE /worker/sessions/{sid}` 与 `PATCH`，前端再接。
> 选其一并在提交信息注明。默认走推荐项（worker 条目暂不提供删/改名）。

- [ ] **Step 4: 类型检查**

Run: `bunx tsc -b`（`web/` 下）
Expected: 无类型错误。

- [ ] **Step 5: 手动验证**

启动 server：侧边栏同时出现普通对话与工蜂会话（工蜂带图标），按时间混排；点工蜂条目进入工蜂模式并回看历史；点普通条目如旧。

- [ ] **Step 6: Commit**

```bash
git add web/src/components/app/Sidebar.tsx
git commit -m "feat(web): 侧边栏混排普通对话与工蜂会话"
```

---

## Task 7: 工蜂管理挪到设置

**Files:**
- Modify: `web/src/components/app/SettingsDialog.tsx`

- [ ] **Step 1: 加「工蜂」Tab**

把 `type Tab = "chat" | "image"` 改为 `type Tab = "chat" | "image" | "worker"`。在 Tabs 区（约 281 行）`image` TabBtn 之后加：

```tsx
<TabBtn active={tab === "worker"} onClick={() => setTab("worker")}>
  工蜂
</TabBtn>
```

- [ ] **Step 2: 工蜂分区内容**

从 `WorkerPage.tsx` 迁移配对/部署/列表/删除逻辑到一个分区组件。在 SettingsDialog 内（与 chat/image 面板并列）新增 `{tab === "worker" && <WorkerSettings />}`，并定义：

```tsx
function WorkerSettings() {
  const [workers, setWorkers] = useState<Worker[]>([])
  const [token, setToken] = useState<string | null>(null)
  const [pairing, setPairing] = useState(false)
  const [copied, setCopied] = useState(false)
  const refresh = () => workerApi.list().then(setWorkers).catch(() => {})
  useEffect(() => {
    refresh()
    const t = setInterval(refresh, 5000)
    return () => clearInterval(t)
  }, [])
  const deployCmd = (tok: string) =>
    `YUNOVA_WORKER_URL=wss://你的域名/api/worker/connect YUNOVA_WORKER_TOKEN=${tok} ./yunova-worker`
  async function pair() {
    if (pairing) return
    setPairing(true)
    try {
      const r = await workerApi.pair()
      setToken(r.token)
      setCopied(false)
      refresh()
    } finally {
      setPairing(false)
    }
  }
  async function copyDeploy() {
    if (!token) return
    try {
      await navigator.clipboard.writeText(deployCmd(token))
      setCopied(true)
      setTimeout(() => setCopied(false), 2000)
    } catch {
      /* 忽略 */
    }
  }
  async function removeWorker(id: number) {
    await workerApi.remove(id).catch(() => {})
    refresh()
  }
  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center gap-2">
        <Button size="sm" onClick={pair} disabled={pairing}>
          {pairing ? "生成中…" : "生成配对码"}
        </Button>
        <span className="text-xs text-muted-foreground">部署到你的服务器，连回本站受你操控</span>
      </div>
      {token && (
        <div className="space-y-2 rounded-md border bg-muted/40 p-3 text-sm">
          <div className="font-medium">配对码（仅显示一次，请妥善保存）</div>
          <code className="block break-all rounded bg-background p-2 font-mono text-xs">{token}</code>
          <div className="text-xs text-muted-foreground">部署命令：</div>
          <div className="flex items-start gap-2">
            <code className="block flex-1 break-all rounded bg-background p-2 font-mono text-xs">
              {deployCmd(token)}
            </code>
            <Button variant="outline" size="icon" className="shrink-0" onClick={copyDeploy} title="复制部署命令">
              {copied ? <Check className="size-4 text-green-500" /> : <Copy className="size-4" />}
            </Button>
          </div>
        </div>
      )}
      <ul className="space-y-1">
        {workers.map((w) => (
          <li key={w.id} className="flex items-center gap-2 rounded-md border px-3 py-2 text-sm">
            <span
              className={cn(
                "size-2.5 shrink-0 rounded-full",
                w.online ? "bg-green-500" : "bg-muted-foreground/50"
              )}
              aria-hidden
            />
            <span className="flex-1 truncate">{w.name}</span>
            <span className="shrink-0 text-xs text-muted-foreground">{w.online ? "在线" : "离线"}</span>
            <Button variant="ghost" size="icon" className="shrink-0" onClick={() => removeWorker(w.id)} title="删除工蜂">
              <Trash2 className="size-4" />
            </Button>
          </li>
        ))}
        {workers.length === 0 && (
          <li className="rounded-md border border-dashed px-3 py-4 text-center text-sm text-muted-foreground">
            还没有工蜂，点上面「生成配对码」并部署。
          </li>
        )}
      </ul>
    </div>
  )
}
```

> 按需补 import：`workerApi, type Worker` from `@/lib/worker`；`Check, Copy, Trash2` from `lucide-react`；`Button`、`cn` 应已在文件内。

- [ ] **Step 3: 类型检查**

Run: `bunx tsc -b`（`web/` 下）
Expected: 无类型错误。

- [ ] **Step 4: 手动验证**

打开设置 →「工蜂」分区：生成配对码、复制部署命令、列表显示在线状态、删除工蜂均可用。

- [ ] **Step 5: Commit**

```bash
git add web/src/components/app/SettingsDialog.tsx
git commit -m "feat(web): 工蜂配对/管理迁入设置弹窗"
```

---

## Task 8: 清理独立工蜂页

**Files:**
- Modify: `web/src/App.tsx`
- Modify: `web/src/components/app/Sidebar.tsx`
- Delete: `web/src/pages/WorkerPage.tsx`

- [ ] **Step 1: 删除 `/worker` 路由与 import**

`web/src/App.tsx`：删除 `import WorkerPage from "@/pages/WorkerPage"` 与 `<Route path="/worker" ...>` 整段。

- [ ] **Step 2: 删除侧边栏「工蜂」入口**

`web/src/components/app/Sidebar.tsx`：删除约 210 行 `<Link to="/worker" ...>工蜂</Link>` 整段。

- [ ] **Step 3: 删除 WorkerPage 文件**

```bash
git rm web/src/pages/WorkerPage.tsx
```

- [ ] **Step 4: 类型检查**

Run: `bunx tsc -b`（`web/` 下）
Expected: 无类型错误、无对 `WorkerPage`/`/worker` 的悬空引用。

- [ ] **Step 5: 手动验证**

应用启动正常；不再有独立工蜂入口；访问 `/worker` 落到通配 `Navigate to="/"`。

- [ ] **Step 6: Commit**

```bash
git add web/src/App.tsx web/src/components/app/Sidebar.tsx
git commit -m "chore(web): 删除独立工蜂页与入口，功能并入对话"
```

---

## Final Verification

- [ ] `cargo check` 通过（后端）。
- [ ] `bunx tsc -b` 通过（前端）。
- [ ] 全链路手动走查：普通对话无回归；工蜂模式发消息/工具渲染/超长折叠/批准/回看/侧边栏混排/设置配对全部可用。
- [ ] 使用 superpowers:verification-before-completion 复核后再交付。
