import { useEffect, useRef, useState } from "react"
import {
  Brain,
  Check,
  ChevronRight,
  Copy,
  FileText,
  FolderSearch,
  Loader2,
  Pencil,
  Search,
  ShieldAlert,
  Terminal,
  X,
} from "lucide-react"
import { Button } from "@/components/ui/button"
import { Markdown } from "@/components/app/Markdown"
import { cn } from "@/lib/utils"
import {
  approvalMessage,
  approvalTitle,
  toAgentBlocks,
  type AgentItem,
} from "@/lib/agent"

const COLLAPSE_LINES = 12

/** The assistant column: same left gutter and right edge as an assistant bubble.
 *
 * The avatar is `size-8` and the gap `gap-2.5`, so the text starts 2.625rem in,
 * and the bubble row is capped at the same width. Steps, approvals and notices
 * all sit in this column, which is what makes a turn read as one thread
 * instead of three unrelated stacks of boxes. */
const COLUMN = "w-full max-w-[96%] sm:max-w-[90%] sm:pl-[2.625rem]"

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
          "overflow-auto whitespace-pre-wrap rounded-lg bg-muted/70 p-2.5 text-xs leading-relaxed",
          collapsed && "max-h-48 overflow-hidden"
        )}
      >
        {text}
      </pre>
      {collapsed && (
        <div className="pointer-events-none absolute inset-x-0 bottom-0 h-10 rounded-b-lg bg-gradient-to-t from-muted/90 to-transparent" />
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

/** Icon per tool family: a run of steps is skimmed by shape before it is read.
 *
 * Returns an element rather than a component type, because binding a component
 * to a local during render makes React treat it as a fresh component on every
 * pass and remount the subtree. */
function toolIcon(name: string) {
  const cls = "size-3"
  if (name === "read") return <FileText className={cls} />
  if (name === "write" || name === "edit") return <Pencil className={cls} />
  if (name === "grep" || name === "search") return <Search className={cls} />
  if (name === "find" || name === "ls") return <FolderSearch className={cls} />
  return <Terminal className={cls} />
}

function ToolCard({ item }: { item: Extract<AgentItem, { kind: "tool" }> }) {
  const [open, setOpen] = useState(false)
  const summary = summarize(item.name, item.args)
  const running = item.output == null
  const failed = item.ok === false
  return (
    <div className={cn(failed && "bg-destructive/5")}>
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="tap-target-sm flex w-full items-center gap-2 px-2.5 py-1.5 text-left transition-colors hover:bg-accent/40"
      >
        <span
          className={cn(
            "grid size-5 shrink-0 place-items-center rounded-md",
            failed
              ? "bg-destructive/15 text-destructive"
              : running
                ? "bg-primary/15 text-primary"
                : "bg-muted text-muted-foreground"
          )}
        >
          {toolIcon(item.name)}
        </span>
        <span className="shrink-0 font-mono text-xs font-medium">{item.name}</span>
        {summary && (
          <span className="truncate font-mono text-xs text-muted-foreground">{summary}</span>
        )}
        <span className="ml-auto flex shrink-0 items-center gap-1.5">
          {running ? (
            <Loader2 className="size-3.5 animate-spin text-primary" />
          ) : failed ? (
            <X className="size-3.5 text-destructive" />
          ) : (
            <Check className="size-3.5 text-emerald-500" />
          )}
          <ChevronRight
            className={cn(
              "size-3.5 text-muted-foreground/70 transition-transform",
              open && "rotate-90"
            )}
          />
        </span>
      </button>
      {open && (
        <div className="space-y-2 border-t border-border/50 bg-muted/20 px-2.5 py-2">
          {item.args != null && (
            <pre className="overflow-auto whitespace-pre-wrap rounded-lg bg-muted/70 p-2.5 text-xs leading-relaxed">
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
    <div>
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="tap-target-sm flex w-full items-center gap-2 px-2.5 py-1.5 text-left text-xs text-muted-foreground transition-colors hover:bg-accent/40"
      >
        <span className="grid size-5 shrink-0 place-items-center rounded-md bg-muted text-muted-foreground">
          <Brain className="size-3" />
        </span>
        思考过程
        <ChevronRight
          className={cn(
            "ml-auto size-3.5 text-muted-foreground/70 transition-transform",
            open && "rotate-90"
          )}
        />
      </button>
      {open && (
        <div className="whitespace-pre-wrap border-t border-border/50 bg-muted/20 px-2.5 py-2 text-xs leading-relaxed text-muted-foreground">
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
 *
 * The body is the whole point of the card. An approval dialog that does not
 * show the command is not a safety feature — it teaches people to press 允许 —
 * so a request that arrives without one says so plainly instead of rendering
 * an empty box that looks like the UI failed.
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
  // `{}` is what the old gate produced for every single request, so it is
  // treated as "nothing to show" rather than rendered as if it were the
  // command: a user who sees it has no more information than with a blank box.
  const detail = message.trim() === "{}" ? "" : message

  return (
    <div className={COLUMN}>
      <div className="rounded-2xl border border-primary/40 bg-primary/5 p-3 text-sm shadow-sm">
        <div className="flex items-center gap-2 font-medium">
          <ShieldAlert className="size-4 shrink-0 text-primary" />
          {title}
        </div>
        {detail ? (
          // Monospace and scrollable: this is a shell command or a diff, and
          // a wrapped proportional font hides exactly the characters — quotes,
          // slashes, redirects — that decide whether it is safe.
          <pre className="mt-2 max-h-72 overflow-auto whitespace-pre-wrap rounded-lg bg-muted/70 p-2.5 font-mono text-xs leading-relaxed">
            {detail}
          </pre>
        ) : (
          <p className="mt-2 rounded-lg bg-muted/70 p-2.5 text-xs leading-relaxed text-muted-foreground">
            这台电脑没有附上操作内容，无法显示具体要执行什么。若不确定，请先拒绝，
            并将桌面客户端升级到最新版本。
          </p>
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

/** Copy control for an assistant turn, mirroring chat's message toolbar. */
function CopyTurn({ text }: { text: string }) {
  const [done, setDone] = useState(false)
  const timer = useRef<number | null>(null)
  useEffect(
    () => () => {
      if (timer.current) window.clearTimeout(timer.current)
    },
    []
  )
  async function copy() {
    try {
      await navigator.clipboard.writeText(text)
    } catch {
      // Non-secure contexts have no clipboard API; the textarea trick still works.
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
    if (timer.current) window.clearTimeout(timer.current)
    timer.current = window.setTimeout(() => setDone(false), 1500)
  }
  return (
    <button
      type="button"
      onClick={() => void copy()}
      title={done ? "已复制" : "复制"}
      aria-label={done ? "已复制" : "复制"}
      className={cn(
        "grid size-7 place-items-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground",
        "opacity-100 sm:opacity-0 sm:group-hover:opacity-100"
      )}
    >
      {done ? <Check className="size-3.5 text-emerald-500" /> : <Copy className="size-3.5" />}
    </button>
  )
}

/** An assistant turn, styled like chat's assistant bubble. */
function AssistantTurn({
  text,
  streaming,
}: {
  text: string
  /** The in-flight turn has no copy button: the text is still moving. */
  streaming?: boolean
}) {
  return (
    <div className="group flex flex-col items-start gap-1">
      <div className="flex w-full max-w-[96%] items-start gap-2.5 sm:max-w-[90%]">
        <img
          src="/logo.svg"
          alt=""
          className="size-8 shrink-0 rounded-xl shadow-sm ring-1 ring-border/70"
        />
        <Markdown
          className={cn(
            "min-w-0 flex-1 rounded-[1.25rem] rounded-tl-md border border-border/60 bg-card/80 px-4 py-3",
            "text-sm leading-relaxed shadow-[0_8px_28px_-22px_rgba(32,22,55,0.45)] backdrop-blur-sm",
            "prose-img:max-w-full prose-img:rounded-xl prose-img:border prose-img:border-border prose-img:shadow-sm"
          )}
        >
          {text}
        </Markdown>
      </div>
      {!streaming && text.trim() && (
        <div className="pl-[2.625rem]">
          <CopyTurn text={text} />
        </div>
      )}
    </div>
  )
}

/** A user turn, styled like chat's user bubble. */
function UserTurn({
  text,
  avatarUrl,
  initial,
}: {
  text: string
  avatarUrl?: string | null
  initial?: string
}) {
  // Which URL failed, rather than a boolean reset from an effect: a new
  // avatar must get a fresh attempt, and a cascading setState in an effect is
  // both slower and flagged by the lint rules.
  const [brokenUrl, setBrokenUrl] = useState<string | null>(null)
  const broken = avatarUrl != null && brokenUrl === avatarUrl
  return (
    <div className="flex justify-end">
      <div className="flex max-w-[94%] items-end gap-2.5 sm:max-w-[82%]">
        <div className="whitespace-pre-wrap rounded-[1.25rem] rounded-br-md bg-gradient-to-br from-primary to-violet-600 px-4 py-2.5 text-sm leading-relaxed text-primary-foreground shadow-md shadow-primary/15">
          {text}
        </div>
        {avatarUrl && !broken ? (
          <img
            src={avatarUrl}
            alt=""
            loading="lazy"
            onError={() => setBrokenUrl(avatarUrl)}
            className="size-8 shrink-0 rounded-xl border border-border object-cover shadow-sm"
          />
        ) : (
          <div className="grid size-8 shrink-0 place-items-center rounded-xl bg-gradient-to-br from-primary to-chart-5 text-[11px] font-semibold text-primary-foreground shadow-sm">
            {(initial || "?").toUpperCase()}
          </div>
        )}
      </div>
    </div>
  )
}

/** The session transcript. */
export function AgentTranscript({
  items,
  streamingText,
  userAvatarUrl,
  userInitial,
}: {
  items: AgentItem[]
  /** Text of the in-flight assistant message, not yet mirrored. */
  streamingText?: string
  userAvatarUrl?: string | null
  userInitial?: string
}) {
  const blocks = toAgentBlocks(items)
  return (
    <div className="space-y-4">
      {blocks.map((block) => {
        if (block.kind === "steps") {
          return (
            <div key={block.id} className={COLUMN}>
              <div className="divide-y divide-border/50 overflow-hidden rounded-2xl border border-border/60 bg-card/50 backdrop-blur-sm">
                {block.items.map((item) =>
                  item.kind === "tool" ? (
                    <ToolCard key={item.id} item={item} />
                  ) : (
                    <Thinking key={item.id} text={item.text} />
                  )
                )}
              </div>
            </div>
          )
        }

        const item = block.item
        if (item.kind === "user")
          return (
            <UserTurn
              key={item.id}
              text={item.text}
              avatarUrl={userAvatarUrl}
              initial={userInitial}
            />
          )
        if (item.kind === "assistant")
          return <AssistantTurn key={item.id} text={item.text} />
        if (item.kind === "error") {
          // A failed turn produces no visible content, so without this the
          // task would look like it ignored the prompt entirely.
          return (
            <div key={item.id} className={COLUMN}>
              <div className="rounded-xl border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive">
                {item.text}
              </div>
            </div>
          )
        }
        return (
          <div key={item.id} className={cn("text-xs text-muted-foreground", COLUMN)}>
            {item.kind === "notice" ? item.text : null}
          </div>
        )
      })}
      {streamingText && <AssistantTurn text={streamingText} streaming />}
    </div>
  )
}
