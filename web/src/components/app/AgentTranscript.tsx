import { useState } from "react"
import { Brain, Check, ChevronRight, Terminal, X } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Markdown } from "@/components/app/Markdown"
import { cn } from "@/lib/utils"
import { approvalMessage, approvalTitle, type AgentItem } from "@/lib/agent"

const COLLAPSE_LINES = 12

/** Tool output: short results inline, long ones clamped behind a toggle. */
function Output({ text }: { text: string }) {
  const lines = text ? text.split("\n").length : 0
  const long = lines > COLLAPSE_LINES
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
        {text}
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
          {open ? "收起" : `展开（共 ${lines} 行）`}
        </button>
      )}
    </div>
  )
}

/** One-line summary of a tool call, so the common case needs no expanding. */
function summarize(name: string, args: unknown): string {
  const a = (args ?? {}) as Record<string, unknown>
  const str = (v: unknown) => (typeof v === "string" ? v : "")
  const clip = (s: string) => (s.length > 90 ? `${s.slice(0, 90)}…` : s)
  if (name === "bash" || name === "powershell") return clip(str(a.command))
  if (name === "read") return clip(str(a.path))
  if (name === "write") return clip(str(a.path))
  if (name === "edit") return clip(str(a.path))
  if (name === "grep") return clip(str(a.pattern))
  if (name === "find" || name === "ls") return clip(str(a.path) || str(a.pattern))
  const first = Object.values(a).find((v) => typeof v === "string")
  return clip(str(first))
}

function ToolCard({ item }: { item: Extract<AgentItem, { kind: "tool" }> }) {
  const [open, setOpen] = useState(false)
  const summary = summarize(item.name, item.args)
  const running = item.output == null
  return (
    <div className="rounded-lg border bg-card/60 text-sm">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="tap-target-sm flex w-full items-center gap-2 px-3 py-2 text-left"
      >
        <Terminal className="size-3.5 shrink-0 text-muted-foreground" />
        <span className="font-mono text-xs font-medium">{item.name}</span>
        {summary && (
          <span className="truncate font-mono text-xs text-muted-foreground">{summary}</span>
        )}
        <span className="ml-auto flex shrink-0 items-center gap-1.5">
          {running ? (
            <span className="text-xs text-muted-foreground">执行中…</span>
          ) : item.ok === false ? (
            <X className="size-3.5 text-destructive" />
          ) : (
            <Check className="size-3.5 text-primary" />
          )}
          <ChevronRight
            className={cn("size-3.5 text-muted-foreground transition-transform", open && "rotate-90")}
          />
        </span>
      </button>
      {open && (
        <div className="space-y-2 border-t px-3 py-2">
          {item.args != null && (
            <pre className="overflow-auto whitespace-pre-wrap rounded bg-muted p-2 text-xs">
              {JSON.stringify(item.args, null, 2)}
            </pre>
          )}
          {item.output != null && <Output text={item.output} />}
        </div>
      )}
    </div>
  )
}

function Thinking({ text }: { text: string }) {
  const [open, setOpen] = useState(false)
  return (
    <div className="rounded-lg border border-dashed bg-muted/30 text-sm">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="tap-target-sm flex w-full items-center gap-2 px-3 py-1.5 text-left text-xs text-muted-foreground"
      >
        <Brain className="size-3.5" />
        思考过程
        <ChevronRight
          className={cn("ml-auto size-3.5 transition-transform", open && "rotate-90")}
        />
      </button>
      {open && (
        <div className="border-t px-3 py-2 text-xs whitespace-pre-wrap text-muted-foreground">
          {text}
        </div>
      )}
    </div>
  )
}

/**
 * A blocking approval request.
 *
 * Rendered inline in the transcript rather than as a modal: the request is
 * broadcast to every connected client, so it can appear while the user is
 * looking at another device, and a modal that stole focus on all of them would
 * be worse than a card that waits.
 */
export function ApprovalCard({
  request,
  onAnswer,
  busy,
}: {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  request: any
  onAnswer: (body: { confirmed?: boolean; value?: string; cancelled?: boolean }) => void
  busy?: boolean
}) {
  const method = String(request?.method ?? "")
  const title = approvalTitle(request)
  const message = approvalMessage(request)
  const options: string[] = Array.isArray(request?.options) ? request.options : []

  return (
    <div className="rounded-lg border border-primary/50 bg-primary/5 p-3 text-sm">
      <div className="font-medium">{title}</div>
      {message && (
        <pre className="mt-1.5 overflow-auto whitespace-pre-wrap rounded bg-muted p-2 text-xs">
          {message}
        </pre>
      )}
      <div className="mt-2.5 flex flex-wrap gap-2">
        {method === "select" &&
          options.map((o) => (
            <Button key={o} size="sm" variant="outline" className="tap-target-sm" disabled={busy} onClick={() => onAnswer({ value: o })}>
              {o}
            </Button>
          ))}
        {method === "confirm" && (
          <>
            <Button size="sm" className="tap-target-sm" disabled={busy} onClick={() => onAnswer({ confirmed: true })}>
              允许
            </Button>
            <Button size="sm" variant="outline" className="tap-target-sm" disabled={busy} onClick={() => onAnswer({ confirmed: false })}>
              拒绝
            </Button>
          </>
        )}
        {(method === "input" || method === "editor") && (
          <InlineInput busy={busy} onSubmit={(v) => onAnswer({ value: v })} />
        )}
        <Button size="sm" variant="ghost" className="tap-target-sm" disabled={busy} onClick={() => onAnswer({ cancelled: true })}>
          忽略
        </Button>
      </div>
      <p className="mt-2 text-xs text-muted-foreground">
        任意已登录的设备都可以处理这条请求，最先响应的生效。
      </p>
    </div>
  )
}

function InlineInput({
  busy,
  onSubmit,
}: {
  busy?: boolean
  onSubmit: (v: string) => void
}) {
  const [v, setV] = useState("")
  return (
    <form
      className="flex gap-2"
      onSubmit={(e) => {
        e.preventDefault()
        onSubmit(v)
      }}
    >
      <input
        value={v}
        onChange={(e) => setV(e.target.value)}
        className="tap-target-sm h-8 rounded-md border bg-background px-2 text-xs"
        placeholder="输入内容…"
      />
      <Button size="sm" type="submit" className="tap-target-sm" disabled={busy}>
        提交
      </Button>
    </form>
  )
}

/** The session transcript. */
export function AgentTranscript({
  items,
  streamingText,
}: {
  items: AgentItem[]
  /** Text of the in-flight assistant message, not yet mirrored. */
  streamingText?: string
}) {
  return (
    <div className="space-y-3">
      {items.map((item) => {
        if (item.kind === "user") {
          return (
            <div key={item.id} className="flex justify-end">
              <div className="max-w-[85%] rounded-2xl bg-primary px-3.5 py-2 text-sm text-primary-foreground whitespace-pre-wrap">
                {item.text}
              </div>
            </div>
          )
        }
        if (item.kind === "assistant") {
          return (
            <div key={item.id} className="text-sm">
              <Markdown>{item.text}</Markdown>
            </div>
          )
        }
        if (item.kind === "thinking") return <Thinking key={item.id} text={item.text} />
        if (item.kind === "tool") return <ToolCard key={item.id} item={item} />
        return (
          <div key={item.id} className="text-xs text-muted-foreground">
            {item.text}
          </div>
        )
      })}
      {streamingText && (
        <div className="text-sm">
          <Markdown>{streamingText}</Markdown>
        </div>
      )}
    </div>
  )
}
