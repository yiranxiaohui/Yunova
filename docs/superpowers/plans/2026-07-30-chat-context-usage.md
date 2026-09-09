# 普通聊天上下文占用率显示 — 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 普通聊天模式在输入栏上方显示 `上下文 X / Y (pct%)`，真实 usage 优先、字符估算兜底。

**Architecture:** 纯前端三个改动点：① 新建 `context-limits.ts`（模型上限表 + token 估算），工蜂 `worker.ts` 复用上限表；② `chat-stream.ts` 三协议解析 usage 并经新回调 `onUsage` 上报；③ `ChatPage.tsx` 持有真实 usage 状态、估算兜底、渲染文字行。后端零改动。

**Tech Stack:** React 19 + TypeScript + Vite 8 + Tailwind 4，包管理用 bun。

**Spec:** `docs/superpowers/specs/2026-07-30-chat-context-usage-design.md`。与 spec 的一处偏差（以本计划为准）：OpenAI 协议走的是 **Responses API**（`/v1/responses`），usage 在流末尾的 `response.completed` 事件里（`response.usage.input_tokens/output_tokens`），**不需要** `stream_options.include_usage`；同时兼容中转站回落的 chat-completions 风格 chunk（顶层 `usage.prompt_tokens/completion_tokens`）。

## Global Constraints

- 本项目**没有测试框架**（CLAUDE.md 明示），不新增测试基建；每个任务的验证 = `bun run build` + `bun run lint`（在 `web/` 目录下）通过，最后整体跑服务人工验证。
- UI 文案用中文，与现有风格一致。
- 不改后端、不加迁移。
- lint 注意：组件内局部函数不能叫 `useXxx`（会被 rules-of-hooks 误判）。
- 构建/lint 用 bun：`cd /opt/yunova/web && bun run build`、`bun run lint`。不跑 `cargo build`（本地只做检查类命令；`cargo check` 会触发 bun build，可以跑但非必需）。

---

### Task 1: 共享上限表与估算函数 `context-limits.ts`

**Files:**
- Create: `web/src/lib/context-limits.ts`
- Modify: `web/src/lib/worker.ts:33-41`（`CONTEXT_LIMITS`/`contextLimit` 改为复用新文件）

**Interfaces:**
- Produces:
  - `contextLimit(model: string): number` — 按模型名关键词返回上下文上限
  - `estimateTokens(text: string): number` — 单段文本估算
  - `estimateMessagesTokens(texts: string[]): number` — 多段文本估算求和
  - `DEFAULT_CONTEXT_LIMIT = 128_000`

- [ ] **Step 1: 新建 `web/src/lib/context-limits.ts`**

```ts
// 模型上下文上限与 token 估算。真实 usage 拿不到时用估算兜底。

/** 按模型名关键词匹配上下文窗口上限（从上到下首个命中）。 */
const LIMIT_PATTERNS: Array<[RegExp, number]> = [
  [/gemini/i, 1_000_000],
  [/gpt-5/i, 400_000],
  [/claude/i, 200_000],
  [/gpt-4o|(^|[^a-z])o[34]([^a-z]|$)/i, 128_000],
  [/deepseek/i, 128_000],
]

export const DEFAULT_CONTEXT_LIMIT = 128_000

export function contextLimit(model: string): number {
  for (const [re, limit] of LIMIT_PATTERNS) {
    if (re.test(model)) return limit
  }
  return DEFAULT_CONTEXT_LIMIT
}

/** 粗略估算：CJK 字符按 1 token/字，其余按 4 字符/token。误差 ±20% 可接受。 */
export function estimateTokens(text: string): number {
  if (!text) return 0
  const cjk = text.match(/[　-鿿豈-﫿＀-￯]/g)?.length ?? 0
  const rest = text.length - cjk
  return cjk + Math.ceil(rest / 4)
}

export function estimateMessagesTokens(texts: string[]): number {
  return texts.reduce((sum, t) => sum + estimateTokens(t), 0)
}
```

- [ ] **Step 2: `worker.ts` 复用上限表**

`web/src/lib/worker.ts` 删掉本地 `CONTEXT_LIMITS` / `DEFAULT_CONTEXT_LIMIT` / `contextLimit`（第 33–41 行），改为 re-export，保持既有 import 路径不破：

```ts
export { contextLimit, DEFAULT_CONTEXT_LIMIT } from "./context-limits"
```

注意：工蜂三个模型名都含 `claude` → 仍是 200k，行为不变。

- [ ] **Step 3: 验证**

Run: `cd /opt/yunova/web && bun run build && bun run lint`
Expected: 两者通过，无新增 error（存量 `react-hooks/set-state-in-effect` warning 不管）。

- [ ] **Step 4: Commit**

```bash
git add web/src/lib/context-limits.ts web/src/lib/worker.ts
git commit -m "feat(web): 新增模型上下文上限表与 token 估算工具"
```

---

### Task 2: `chat-stream.ts` 三协议解析真实 usage

**Files:**
- Modify: `web/src/lib/chat-stream.ts`

**Interfaces:**
- Produces: `ChatStreamOptions` 新增可选字段 `onUsage?: (promptTokens: number, completionTokens: number) => void`。流中每次拿到更新的 usage 都调用（取最后一次即为最终值）。

- [ ] **Step 1: `ChatStreamOptions` 加回调**

在 `chat-stream.ts` 的 `ChatStreamOptions` 类型中（`patchAssistant` 之后）追加：

```ts
  // 上游回传的真实 token 用量（prompt/completion）。流中可能被多次调用，
  // 最后一次为最终值；部分中转站不回传 usage，则一次也不会调用。
  onUsage?: (promptTokens: number, completionTokens: number) => void
```

- [ ] **Step 2: 在 SSE 解析循环中提取 usage**

在 `streamChat` 内、`while` 循环之前声明状态：

```ts
  let usageIn = 0
  let usageOut = 0
  const reportUsage = () => {
    if (o.onUsage && (usageIn > 0 || usageOut > 0)) o.onUsage(usageIn, usageOut)
  }
```

在解析出 `json` 后（现有 `const json = JSON.parse(data)` 之后、`extractDelta` 之前）插入按协议的 usage 提取：

```ts
          if (o.protocol === "openai") {
            // Responses API：完成事件带整段 usage。
            const resp = (json as { response?: { usage?: { input_tokens?: number; output_tokens?: number } } }).response
            if (json.type === "response.completed" && resp?.usage) {
              usageIn = resp.usage.input_tokens ?? usageIn
              usageOut = resp.usage.output_tokens ?? usageOut
              reportUsage()
            }
            // 兼容 chat-completions 风格中转站：顶层 usage 字段。
            const cc = json.usage as { prompt_tokens?: number; completion_tokens?: number } | undefined
            if (cc && (cc.prompt_tokens != null || cc.completion_tokens != null)) {
              usageIn = cc.prompt_tokens ?? usageIn
              usageOut = cc.completion_tokens ?? usageOut
              reportUsage()
            }
          } else if (o.protocol === "claude") {
            if (json.type === "message_start") {
              const msg = json.message as { usage?: { input_tokens?: number; output_tokens?: number } } | undefined
              if (msg?.usage) {
                usageIn = msg.usage.input_tokens ?? usageIn
                usageOut = msg.usage.output_tokens ?? usageOut
                reportUsage()
              }
            } else if (json.type === "message_delta") {
              const u = json.usage as { output_tokens?: number } | undefined
              if (u?.output_tokens != null) {
                usageOut = u.output_tokens
                reportUsage()
              }
            }
          } else if (o.protocol === "gemini") {
            const um = json.usageMetadata as { promptTokenCount?: number; candidatesTokenCount?: number } | undefined
            if (um) {
              usageIn = um.promptTokenCount ?? usageIn
              usageOut = um.candidatesTokenCount ?? usageOut
              reportUsage()
            }
          }
```

注意插入位置在现有 OpenAI `image_generation` 分支之后即可（那些分支 `continue` 的路径不携带 usage，互不影响）。

- [ ] **Step 3: 验证**

Run: `cd /opt/yunova/web && bun run build && bun run lint`
Expected: 通过。

- [ ] **Step 4: Commit**

```bash
git add web/src/lib/chat-stream.ts
git commit -m "feat(web): 聊天流解析三协议真实 usage 并回调 onUsage"
```

---

### Task 3: ChatPage 状态接线与占用率 UI

**Files:**
- Modify: `web/src/pages/ChatPage.tsx`

**Interfaces:**
- Consumes: Task 1 的 `contextLimit` / `estimateMessagesTokens`（从 `@/lib/context-limits` import）；Task 2 的 `onUsage` 回调。
- 复用组件内已有 `formatTokens(n)`（约 1556 行）与 `effectiveSystemPrompt`、`messages`、`settings.model`、`workerMode`。

- [ ] **Step 1: import 与状态**

顶部 import（注意 `contextLimit` 目前从 `@/lib/worker` import，保持不动即可——它已 re-export；另加）：

```ts
import { estimateMessagesTokens } from "@/lib/context-limits"
```

在普通聊天状态区（`const [messages, setMessages] = ...` 附近，约 773 行）加：

```ts
  // 普通聊天上下文占用：null = 本会话尚未拿到真实 usage，显示估算值。
  const [chatUsageTokens, setChatUsageTokens] = useState<number | null>(null)
```

- [ ] **Step 2: 会话切换时重置**

在加载会话的 `useEffect`（约 880 行，`[conversationId, nav]` 依赖那个）里：`if (!conversationId)` 分支和成功加载 `.then` 分支各加一行 `setChatUsageTokens(null)`。

- [ ] **Step 3: 两处 `streamChat` 调用传 onUsage**

`send()`（约 1321 行）与 `regenerateLastAssistant()`（约 1427 行）的 `streamChat({ ... })` 参数都加：

```ts
        onUsage: (p, c) => setChatUsageTokens(p + c),
```

- [ ] **Step 4: 计算显示值**

在 `effectiveSystemPrompt` 的 `useMemo` 之后加：

```ts
  // 展示用上下文 token：真实 usage 优先，没有则按消息文本估算。
  const displayContextTokens = useMemo(() => {
    if (chatUsageTokens != null) return chatUsageTokens
    return estimateMessagesTokens([
      effectiveSystemPrompt,
      ...messages.map((m) => m.content),
    ])
  }, [chatUsageTokens, effectiveSystemPrompt, messages])
```

- [ ] **Step 5: 渲染文字行**

在输入栏上方那行 flex 容器内（约 1888 行 `<div className="mb-2 flex flex-wrap items-center gap-3 px-1 text-sm">`），工蜂条件块 `{workerMode && (...)}` 之后追加：

```tsx
              {!workerMode && (() => {
                const limit = contextLimit(settings.model)
                const pct = Math.min(
                  100,
                  Math.round((displayContextTokens / limit) * 100)
                )
                const cls =
                  pct >= 95
                    ? "text-red-500"
                    : pct >= 80
                      ? "text-orange-500"
                      : "text-muted-foreground"
                return (
                  <span className={`ml-auto text-xs ${cls}`}>
                    上下文 {formatTokens(displayContextTokens)} /{" "}
                    {formatTokens(limit)} ({pct}%)
                    {pct >= 95 ? "，建议新开会话" : ""}
                  </span>
                )
              })()}
```

说明：`ml-auto` 让它靠右，与左侧工蜂勾选框同行；始终渲染（含 0%）。工蜂模式下不渲染（工蜂有自己的进度条块）。

- [ ] **Step 6: 验证**

Run: `cd /opt/yunova/web && bun run build && bun run lint`
Expected: 通过，无新增 lint error。

- [ ] **Step 7: Commit**

```bash
git add web/src/pages/ChatPage.tsx
git commit -m "feat(web): 普通聊天输入栏上方显示上下文占用率"
```

---

### Task 4: 端到端人工验证

**Files:** 无代码改动。

- [ ] **Step 1: 起服务**

Run: `cargo run`（或已有运行实例），浏览器开聊天页。

- [ ] **Step 2: 验证矩阵**

- 新会话未发消息：显示 `上下文 0 / …(0%)`（或极小估算值）。
- 发一条消息（Anthropic 或 OpenAI 上游）：回复结束后数字跳为真实 usage（明显区别于估算，如带 system prompt 时）。
- 切换到另一个会话：数字重置为该会话的估算值。
- 勾选工蜂模式：该文字行消失，工蜂自己的进度条正常。
- （可选）Gemini 上游同样验证。

- [ ] **Step 3: 确认后按 finishing-a-development-branch 流程收尾**
