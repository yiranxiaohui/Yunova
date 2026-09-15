import { useEffect, useMemo, useRef, useState } from "react"
import { useLocation, useNavigate, useParams, useSearchParams } from "react-router-dom"
import {
  ArrowUp,
  ArrowUpRight,
  ArrowDown,
  BookMarked,
  Bot,
  Check,
  ClipboardCheck,
  Copy,
  Code2,
  Download,
  FileText,
  Globe,
  Library,
  Lightbulb,
  Menu,
  MessageSquareText,
  Paperclip,
  PenLine,
  Pencil,
  Plus,
  RefreshCcw,
  Settings,
  Share2,
  Sparkles,
  Square,
  Upload,
  Wand2,
  X,
} from "lucide-react"
import ReactMarkdown from "react-markdown"
import remarkGfm from "remark-gfm"
import { CodeBlock } from "@/components/app/Markdown"
import { ReasoningBlock } from "@/components/app/ReasoningBlock"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Textarea } from "@/components/ui/textarea"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"

// Radix 的 SelectItem 不接受空字符串作为 value，用哨兵值代表「未选择工蜂」。
const WORKER_NONE = "__none__"
import { streamChat, type ChatMessage } from "@/lib/chat-stream"
import { estimateMessagesTokens } from "@/lib/context-limits"
import { listModels } from "@/lib/models"
import {
  describeModelQuota,
  listPlatformModels,
  type PlatformModel,
} from "@/lib/platform-models"
import { cn } from "@/lib/utils"
import { useAuth } from "@/lib/auth-context"
import {
  loadSettings,
  saveSettings,
  loadEffectiveSettings,
  settingsApi,
  likelyWebSearchCapable,
  GUEST_SETTINGS_ID,
  PROTOCOL_META,
  type Protocol,
  type UpstreamSettings,
} from "@/lib/settings"
import { SettingsDialog } from "@/components/app/SettingsDialog"
import { RechargeDialog } from "@/components/app/RechargeDialog"
import { QuotaLedgerDialog } from "@/components/app/QuotaLedgerDialog"
import { Sidebar } from "@/components/app/Sidebar"
import { SystemPromptDialog } from "@/components/app/SystemPromptBar"
import { PromptLibrary } from "@/components/app/PromptLibrary"
import { SkillsDialog } from "@/components/app/SkillsDialog"
import { ShareDialog } from "@/components/app/ShareDialog"
import { ImagePreview } from "@/components/app/ImagePreview"
import { conversationsApi, type StoredMessage } from "@/lib/conversations"
import {
  skillsApi,
  composeSystemPromptWithSkills,
  type Skill,
} from "@/lib/skills"
import { filenameFromPath } from "@/lib/media-path"
import { videoEditorApi } from "@/lib/video-editor"
import {
  formatQuota,
  formatQuotaCompact,
  quotaApi,
  type QuotaMe,
} from "@/lib/quota"
import {
  workerApi,
  sendAgentMessage,
  replayMessages,
  contextLimit,
  type Worker,
  type AgentEvent,
} from "@/lib/worker"
import { WorkerLog, type WorkerLogItem } from "@/components/app/WorkerEvents"

// reasoning / reasoningMs 随消息一起落库（messages.reasoning /
// messages.reasoning_ms），所以刷新页面后仍能看到思考过程。
type UiMessage = ChatMessage & {
  id?: number
  reasoning?: string
  reasoningMs?: number
}

/** 把服务端消息转成前端消息，把 null 归一到 undefined。 */
function toUiMessage(m: StoredMessage): UiMessage {
  return {
    id: m.id,
    role: m.role,
    content: m.content,
    ...(m.reasoning ? { reasoning: m.reasoning } : {}),
    ...(m.reasoning_ms != null ? { reasoningMs: m.reasoning_ms } : {}),
  }
}

const PROTOCOL_COLOR: Record<Protocol, string> = {
  openai: "from-emerald-400 to-emerald-600",
  claude: "from-amber-400 to-orange-600",
  gemini: "from-sky-400 to-indigo-600",
}

function ModelPicker({
  protocol,
  model,
  settings,
  onChangeModel,
}: {
  protocol: Protocol
  model: string
  settings: UpstreamSettings
  onChangeModel: (next: string, protocol?: Protocol) => void
}) {
  const [open, setOpen] = useState(false)
  const [models, setModels] = useState<PlatformModel[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [query, setQuery] = useState("")
  const popRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent) => {
      if (!popRef.current) return
      if (!popRef.current.contains(e.target as Node)) setOpen(false)
    }
    window.addEventListener("mousedown", onDown)
    return () => window.removeEventListener("mousedown", onDown)
  }, [open])

  const baseUrl = settings.baseUrl
  const apiKey = settings.apiKey
  const useProxy = settings.useProxy
  const fetchProtocol = settings.protocol
  const chatMode = settings.chatMode

  async function fetchList() {
    setError(null)
    setLoading(true)
    try {
      const list =
        chatMode === "platform"
          ? await listPlatformModels("chat")
          : (
              await listModels({
                protocol: fetchProtocol as Protocol,
                baseUrl,
                apiKey,
                useProxy,
              })
            ).map((model) => ({
              model,
              display_name: null,
              kind: "chat" as const,
              protocol: fetchProtocol as Protocol,
              context_limit: null,
              // BYOK 模型由用户自己的 Key 付费，站内不计额度。
              input_micro_quota_per_1m: 0,
              output_micro_quota_per_1m: 0,
              cached_input_micro_quota_per_1m: null,
              per_call_micro_quota: 0,
            }))
      setModels(list)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
      setModels([])
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    if (open && models.length === 0 && !loading && !error) void fetchList()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open])

  // Reset model cache when upstream context changes (protocol/baseUrl).
  useEffect(() => {
    setModels([])
    setError(null)
  }, [baseUrl, fetchProtocol, chatMode])

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase()
    if (!q) return models
    return models.filter((m) => {
      const display = m.display_name ?? ""
      return (
        m.model.toLowerCase().includes(q) ||
        display.toLowerCase().includes(q) ||
        m.protocol.toLowerCase().includes(q)
      )
    })
  }, [models, query])

  return (
    <div className="relative min-w-0" ref={popRef}>
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="inline-flex min-w-0 max-w-[8rem] items-center gap-1.5 rounded-xl border border-border/70 bg-card/70 px-2.5 py-1.5 text-xs shadow-sm backdrop-blur transition-all hover:border-primary/30 hover:bg-card sm:max-w-[11rem] md:max-w-none"
        title="点击切换模型"
      >
        <span
          className={cn(
            "inline-block size-2 shrink-0 rounded-full bg-gradient-to-br",
            PROTOCOL_COLOR[protocol]
          )}
        />
        <span className="hidden font-medium md:inline">
          {PROTOCOL_META[protocol].label.replace(" 兼容", "")}
        </span>
        <span className="hidden text-muted-foreground md:inline">·</span>
        <span className="truncate text-muted-foreground">{model || "未配置"}</span>
        <span className="shrink-0 text-muted-foreground">▾</span>
      </button>

      {open && (
        <div className="absolute left-0 top-full z-40 mt-2 w-72 rounded-2xl border border-border bg-popover/95 p-2.5 shadow-panel backdrop-blur-xl">
          <div className="mb-2 flex items-center gap-1">
            <Input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="搜索模型…"
              className="h-8 text-xs"
              autoFocus
            />
            <Button
              type="button"
              size="icon"
              variant="ghost"
              onClick={() => void fetchList()}
              disabled={loading}
              title="重新拉取"
              className="size-8 shrink-0"
            >
              <RefreshCcw className={cn("size-3.5", loading && "animate-spin")} />
            </Button>
          </div>
          <div className="nc-scroll max-h-72 overflow-y-auto">
            {loading && models.length === 0 && (
              <p className="px-2 py-6 text-center text-xs text-muted-foreground">
                加载中…
              </p>
            )}
            {!loading && error && (
              <p className="rounded border border-destructive/40 bg-destructive/10 px-2 py-1.5 text-xs text-destructive">
                {error}
              </p>
            )}
            {!loading && !error && filtered.length === 0 && (
              <p className="px-2 py-6 text-center text-xs text-muted-foreground">
                {models.length === 0
                  ? "暂无可用模型"
                  : "没有匹配的模型"}
              </p>
            )}
            <ul className="flex flex-col">
              {filtered.map((m) => {
                const active = m.model === model
                return (
                  <li key={`${m.protocol}:${m.model}`}>
                    <button
                      type="button"
                      onClick={() => {
                        onChangeModel(m.model, m.protocol)
                        setOpen(false)
                      }}
                      className={cn(
                        "flex w-full items-center justify-between rounded px-2 py-1.5 text-left text-xs hover:bg-accent",
                        active && "bg-accent text-accent-foreground"
                      )}
                    >
                      <span className="min-w-0">
                        <span className="block truncate font-mono">
                          {m.display_name || m.model}
                        </span>
                        {chatMode === "platform" && (
                          <span className="block truncate text-[10px] text-muted-foreground">
                            {m.protocol} · {describeModelQuota(m)}
                          </span>
                        )}
                      </span>
                      {active && <Check className="ml-2 size-3.5 shrink-0 text-primary" />}
                    </button>
                  </li>
                )
              })}
            </ul>
          </div>
          <div className="mt-2 border-t border-border pt-2 text-[10px] text-muted-foreground">
            {chatMode === "platform"
              ? "云端额度 · 从管理员开放的模型获取"
              : "自带 Key · 从你配置的上游获取"}
          </div>
        </div>
      )}
    </div>
  )
}

type BubbleActions = {
  onCopy: () => Promise<void> | void
  copied: boolean
  onRetry?: () => void
  onEdit?: () => void
}

function ActionIcon({
  label,
  onClick,
  children,
}: {
  label: string
  onClick: () => void
  children: React.ReactNode
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      title={label}
      aria-label={label}
      className="grid size-7 place-items-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
    >
      {children}
    </button>
  )
}

/// Copy an image URL's bytes onto the clipboard as a PNG ClipboardItem so
/// the user can paste it into Word / Slack / image editors.
/// Non-PNG sources (JPEG/WebP/data:) are re-encoded via canvas since the
/// Clipboard API requires image/png on all browsers.
async function copyImageToClipboard(url: string): Promise<void> {
  if (!navigator.clipboard || typeof window.ClipboardItem === "undefined") {
    throw new Error("当前浏览器不支持剪贴板复制图片")
  }
  const res = await fetch(url, { credentials: "same-origin" })
  if (!res.ok) throw new Error(`HTTP ${res.status}`)
  const src = await res.blob()
  let png: Blob = src
  if (src.type !== "image/png") {
    png = await new Promise<Blob>((resolve, reject) => {
      const img = new Image()
      img.crossOrigin = "anonymous"
      const objectUrl = URL.createObjectURL(src)
      img.onload = () => {
        const canvas = document.createElement("canvas")
        canvas.width = img.naturalWidth
        canvas.height = img.naturalHeight
        const ctx = canvas.getContext("2d")
        if (!ctx) {
          URL.revokeObjectURL(objectUrl)
          reject(new Error("画布 2D 上下文不可用"))
          return
        }
        ctx.drawImage(img, 0, 0)
        canvas.toBlob((b) => {
          URL.revokeObjectURL(objectUrl)
          if (!b) reject(new Error("PNG 编码失败"))
          else resolve(b)
        }, "image/png")
      }
      img.onerror = () => {
        URL.revokeObjectURL(objectUrl)
        reject(new Error("图片加载失败"))
      }
      img.src = objectUrl
    })
  }
  await navigator.clipboard.write([new ClipboardItem({ "image/png": png })])
}

function CopyImageButton({
  url,
  variant = "inline",
}: {
  url: string
  variant?: "inline" | "circle"
}) {
  const [state, setState] = useState<"idle" | "busy" | "done" | "err">("idle")
  const resetTimerRef = useRef<number | null>(null)
  useEffect(
    () => () => {
      if (resetTimerRef.current) window.clearTimeout(resetTimerRef.current)
    },
    []
  )
  async function onClick(e: React.MouseEvent) {
    e.stopPropagation()
    e.preventDefault()
    if (state === "busy") return
    setState("busy")
    try {
      await copyImageToClipboard(url)
      setState("done")
    } catch {
      setState("err")
    } finally {
      if (resetTimerRef.current) window.clearTimeout(resetTimerRef.current)
      resetTimerRef.current = window.setTimeout(() => setState("idle"), 1800)
    }
  }
  const icon =
    state === "done" ? (
      <ClipboardCheck className={variant === "circle" ? "size-5 text-emerald-400" : "size-3 text-emerald-500"} />
    ) : (
      <Copy className={variant === "circle" ? "size-5" : "size-3"} />
    )
  const title =
    state === "done"
      ? "已复制到剪贴板"
      : state === "err"
        ? "复制失败（浏览器或协议限制）"
        : state === "busy"
          ? "复制中…"
          : "复制图片到剪贴板"
  if (variant === "circle") {
    return (
      <button
        type="button"
        onClick={onClick}
        aria-label={title}
        title={title}
        className="inline-flex size-9 items-center justify-center rounded-full bg-black/60 text-white hover:bg-black/80"
      >
        {icon}
      </button>
    )
  }
  return (
    <button
      type="button"
      onClick={onClick}
      title={title}
      className="inline-flex items-center gap-1 rounded-md border border-border bg-background/80 px-2 py-1 text-xs backdrop-blur hover:bg-accent"
    >
      {icon}
      {state === "done" ? "已复制" : "复制"}
    </button>
  )
}

// Document attachments accepted alongside images. PDFs plus text/code files —
// the types OpenAI / Claude / Gemini accept as native document blocks.
const DOC_EXTS = [
  "pdf", "txt", "md", "markdown", "csv", "json", "log", "xml", "yaml", "yml",
  "html", "htm", "css", "ts", "tsx", "js", "jsx", "py", "rs", "go", "java",
  "c", "h", "cpp", "hpp", "cc", "sh", "rb", "php", "sql", "toml", "ini",
  "conf", "env", "text",
]
const DOC_ACCEPT =
  "application/pdf," + DOC_EXTS.map((e) => `.${e}`).join(",")

function isAcceptedDoc(file: File): boolean {
  const m = file.type
  if (
    m === "application/pdf" ||
    m.startsWith("text/") ||
    m === "application/json" ||
    m === "application/xml"
  )
    return true
  const ext = file.name.split(".").pop()?.toLowerCase() ?? ""
  return DOC_EXTS.includes(ext)
}

type UserSegment =
  | { type: "text"; value: string }
  | { type: "image"; url: string; alt: string }
  | { type: "file"; url: string; name: string }

function splitUserContent(content: string): UserSegment[] {
  // Match markdown image syntax `![alt](url)` and our document-link syntax
  // `[name](/api/files/…)`. The leading `!?` distinguishes the two.
  const re = /(!?)\[([^\]]*)\]\(([^)\s]+)\)/g
  const segments: UserSegment[] = []
  let lastIndex = 0
  let m: RegExpExecArray | null
  while ((m = re.exec(content)) !== null) {
    const bang = m[1] === "!"
    const url = m[3] ?? ""
    const isFile = !bang && url.startsWith("/api/files/")
    // A plain markdown link that isn't one of our file refs: leave it in text.
    if (!bang && !isFile) continue
    const text = content.slice(lastIndex, m.index).replace(/\n+$/, "")
    if (text.length > 0) segments.push({ type: "text", value: text })
    if (isFile) {
      segments.push({ type: "file", name: m[2] || "file", url })
    } else {
      segments.push({ type: "image", alt: m[2] ?? "", url })
    }
    lastIndex = m.index + m[0].length
    // Swallow a single trailing newline so consecutive refs stack cleanly
    if (content[lastIndex] === "\n") lastIndex += 1
  }
  const tail = content.slice(lastIndex)
  if (tail.length > 0) segments.push({ type: "text", value: tail })
  if (segments.length === 0) segments.push({ type: "text", value: content })
  return segments
}

function Bubble({
  message,
  actions,
  onPublishImage,
  publishedFilenames,
  publishingFilename,
  userAvatarUrl,
  userInitial,
}: {
  message: UiMessage
  actions: BubbleActions
  onPublishImage?: (filename: string, alt: string) => void
  publishedFilenames?: Set<string>
  publishingFilename?: string | null
  userAvatarUrl?: string | null
  userInitial?: string
}) {
  const isUser = message.role === "user"
  const [preview, setPreview] = useState<{ src: string; alt: string } | null>(
    null
  )
  const [avatarBroken, setAvatarBroken] = useState(false)
  useEffect(() => {
    setAvatarBroken(false)
  }, [userAvatarUrl])

  const toolbar = (
    <div
      className={cn(
        "flex items-center gap-0.5 opacity-100 transition-opacity sm:opacity-0 sm:group-hover:opacity-100",
        isUser ? "justify-end" : "justify-start"
      )}
    >
      <ActionIcon label={actions.copied ? "已复制" : "复制"} onClick={() => void actions.onCopy()}>
        {actions.copied ? (
          <Check className="size-3.5 text-emerald-500" />
        ) : (
          <Copy className="size-3.5" />
        )}
      </ActionIcon>
      {actions.onEdit && (
        <ActionIcon label="编辑并重发" onClick={actions.onEdit}>
          <Pencil className="size-3.5" />
        </ActionIcon>
      )}
      {actions.onRetry && (
        <ActionIcon label="重新生成" onClick={actions.onRetry}>
          <RefreshCcw className="size-3.5" />
        </ActionIcon>
      )}
    </div>
  )

  if (isUser) {
    const segments = splitUserContent(message.content)
    return (
      <div className="group flex flex-col items-end gap-1">
        <div className="flex max-w-[94%] items-end gap-2.5 sm:max-w-[82%]">
          <div className="rounded-[1.25rem] rounded-br-md bg-gradient-to-br from-primary to-violet-600 px-4 py-2.5 text-sm leading-relaxed text-primary-foreground shadow-md shadow-primary/15">
            {segments.map((seg, i) =>
              seg.type === "text" ? (
                <p key={i} className="whitespace-pre-wrap">
                  {seg.value}
                </p>
              ) : seg.type === "image" ? (
                <img
                  key={i}
                  src={seg.url}
                  alt={seg.alt}
                  loading="lazy"
                  onClick={() => setPreview({ src: seg.url, alt: seg.alt })}
                  className="my-1 max-h-80 w-auto cursor-zoom-in rounded-xl border border-primary-foreground/20"
                />
              ) : (
                <a
                  key={i}
                  href={seg.url}
                  target="_blank"
                  rel="noreferrer"
                  className="my-1 flex items-center gap-2 rounded-xl border border-primary-foreground/20 bg-primary-foreground/10 px-3 py-2 text-xs no-underline hover:bg-primary-foreground/20"
                  title={seg.name}
                >
                  <FileText className="size-4 shrink-0" />
                  <span className="truncate">{seg.name}</span>
                </a>
              )
            )}
          </div>
          {userAvatarUrl && !avatarBroken ? (
            <img
              src={userAvatarUrl}
              alt=""
              loading="lazy"
              onError={() => setAvatarBroken(true)}
              className="size-8 shrink-0 rounded-xl border border-border object-cover shadow-sm"
            />
          ) : (
            <div className="grid size-8 shrink-0 place-items-center rounded-xl bg-gradient-to-br from-primary to-chart-5 text-[11px] font-semibold text-primary-foreground shadow-sm">
              {(userInitial || "?").toUpperCase()}
            </div>
          )}
        </div>
        <div className="pr-10">{toolbar}</div>
        {preview && (
          <ImagePreview
            src={preview.src}
            alt={preview.alt}
            onClose={() => setPreview(null)}
            extraActions={<CopyImageButton url={preview.src} variant="circle" />}
          />
        )}
      </div>
    )
  }

  return (
    <div className="group flex flex-col items-start gap-1">
      <div className="flex max-w-[96%] items-start gap-2.5 sm:max-w-[90%]">
        <img
          src="/logo.png"
          alt=""
          className="size-8 shrink-0 rounded-xl ring-1 ring-border/70 shadow-sm"
        />
        <div
          className={cn(
            "prose prose-sm dark:prose-invert min-w-0 max-w-none",
            "rounded-[1.25rem] rounded-tl-md bg-card/80 px-4 py-3 text-sm leading-relaxed backdrop-blur-sm",
            "border border-border/60 shadow-[0_8px_28px_-22px_rgba(32,22,55,0.45)]",
            "[&_pre]:max-w-full [&_pre]:overflow-x-auto [&_pre]:whitespace-pre",
            "[&_*]:break-words [&_a]:break-all",
            "prose-pre:bg-muted prose-pre:text-foreground prose-pre:border prose-pre:border-border",
            "[&_pre_code]:!text-foreground [&_pre_code]:!bg-transparent",
            "prose-img:max-w-full prose-img:rounded-xl prose-img:border prose-img:border-border prose-img:shadow-sm",
            "prose-code:rounded prose-code:bg-muted prose-code:px-1 prose-code:py-0.5 prose-code:text-[0.85em] prose-code:font-normal prose-code:before:content-none prose-code:after:content-none"
          )}
        >
          {message.reasoning && (
            <ReasoningBlock
              reasoning={message.reasoning}
              elapsedMs={message.reasoningMs}
              // 已落库的消息肯定不在生成中，就算旧数据没有耗时也不能显示
              // 成「思考中…」。
              done={message.id !== undefined ? true : undefined}
            />
          )}
          <ReactMarkdown
            remarkPlugins={[remarkGfm]}
            components={{
              pre: ({ node: _node, ...props }) => <CodeBlock {...props} />,
              img: ({ src, alt }) => {
                const url = typeof src === "string" ? src : ""
                const altText = alt ?? ""
                const fname = filenameFromPath(url)
                const published = fname
                  ? publishedFilenames?.has(fname)
                  : false
                const busy = fname ? publishingFilename === fname : false
                const imgEl = (
                  <img
                    src={url}
                    alt={altText}
                    loading="lazy"
                    onClick={() => setPreview({ src: url, alt: altText })}
                    className="!my-0 max-h-80 w-auto cursor-zoom-in"
                  />
                )
                return (
                  <span className="group/img relative inline-block">
                    {imgEl}
                    <div
                      className={cn(
                        "absolute bottom-2 right-2 z-10 flex items-center gap-1",
                        "opacity-0 transition-opacity group-hover/img:opacity-100",
                        published && "opacity-100"
                      )}
                    >
                      {url && (
                        <>
                          <CopyImageButton url={url} />
                          <a
                            href={url}
                            download={fname || "image"}
                            title="下载图片"
                            onClick={(e) => e.stopPropagation()}
                            className="inline-flex items-center gap-1 rounded-md border border-border bg-background/80 px-2 py-1 text-xs backdrop-blur hover:bg-accent"
                          >
                            <Download className="size-3" /> 下载
                          </a>
                        </>
                      )}
                      {onPublishImage && fname && (
                        <button
                          type="button"
                          onClick={() => onPublishImage(fname, altText)}
                          disabled={busy || published}
                          title={
                            published
                              ? "已分享到素材库"
                              : busy
                                ? "发布中…"
                                : "分享到公有素材库"
                          }
                          className={cn(
                            "inline-flex items-center gap-1 rounded-md border border-border bg-background/80 px-2 py-1 text-xs backdrop-blur",
                            published && "cursor-default",
                            !published && !busy && "hover:bg-accent"
                          )}
                        >
                          {published ? (
                            <>
                              <Check className="size-3 text-emerald-500" /> 已发布
                            </>
                          ) : (
                            <>
                              <Upload className="size-3" />{" "}
                              {busy ? "分享中…" : "分享到素材库"}
                            </>
                          )}
                        </button>
                      )}
                    </div>
                  </span>
                )
              },
            }}
          >
            {/* 思考块已经表明正在工作，此时不必再挂一个孤零零的省略号 */}
            {message.content || (message.reasoning ? "" : "…")}
          </ReactMarkdown>
        </div>
      </div>
      <div className="pl-10">{toolbar}</div>
      {preview && (
        <ImagePreview
          src={preview.src}
          alt={preview.alt}
          onClose={() => setPreview(null)}
          extraActions={<CopyImageButton url={preview.src} variant="circle" />}
        />
      )}
    </div>
  )
}

const SAMPLE_PROMPTS = [
  { title: "解释概念", body: "用通俗比喻解释「向量数据库」是什么。", icon: Lightbulb },
  { title: "写代码", body: "用 Rust 写一个简单的 HTTP 客户端示例。", icon: Code2 },
  { title: "头脑风暴", body: "帮我想 5 个给副业独立开发者的产品点子。", icon: Sparkles },
  { title: "改写润色", body: "把这段话改得更简洁、更专业：", icon: PenLine },
]

export default function ChatPage() {
  const auth = useAuth()
  const user = auth.state.status === "authed" ? auth.state.user : null
  const settingsOwnerId = user?.id ?? GUEST_SETTINGS_ID
  const nav = useNavigate()
  const { id: paramId } = useParams()
  const location = useLocation()
  const isWorkerRoute = location.pathname.startsWith("/w/")
  const workerSessionId = isWorkerRoute && paramId ? Number(paramId) : null
  const conversationId = !isWorkerRoute && paramId ? Number(paramId) : null
  const [searchParams] = useSearchParams()
  const msgAnchorParam = searchParams.get("msg")
  const targetMsgId = msgAnchorParam ? Number(msgAnchorParam) : null
  const [highlightedMsgId, setHighlightedMsgId] = useState<number | null>(null)
  const msgAnchorScrolledRef = useRef<string | null>(null)

  const [settings, setSettings] = useState<UpstreamSettings>(() =>
    loadSettings(settingsOwnerId)
  )
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [rechargeOpen, setRechargeOpen] = useState(false)
  const [ledgerOpen, setLedgerOpen] = useState(false)
  const [libraryOpen, setLibraryOpen] = useState(false)
  const [skillsOpen, setSkillsOpen] = useState(false)
  const [shareOpen, setShareOpen] = useState(false)
  const [currentConversation, setCurrentConversation] =
    useState<import("@/lib/conversations").Conversation | null>(null)
  const [mobileNavOpen, setMobileNavOpen] = useState(false)
  const [publishedFilenames, setPublishedFilenames] = useState<Set<string>>(
    new Set()
  )
  const [publishingFilename, setPublishingFilename] = useState<string | null>(
    null
  )
  const [quotaMe, setQuotaMe] = useState<QuotaMe | null>(null)
  const [attachedSkills, setAttachedSkills] = useState<Skill[]>([])
  const [systemPromptOpen, setSystemPromptOpen] = useState(false)
  const [messages, setMessages] = useState<UiMessage[]>([])
  // 普通聊天上下文占用：null = 本会话尚未拿到真实 usage，显示估算值。
  const [chatUsageTokens, setChatUsageTokens] = useState<number | null>(null)
  // 平台模式下按模型配置的上下文上限（model → context_limit），拉取失败静默回落关键词表。
  const [platformContextMap, setPlatformContextMap] = useState<Map<string, number>>(new Map())
  const [systemPrompt, setSystemPrompt] = useState("")
  const [loadingMessages, setLoadingMessages] = useState(false)
  const [input, setInput] = useState("")
  const [streaming, setStreaming] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [sidebarReload, setSidebarReload] = useState(0)
  const [copiedKey, setCopiedKey] = useState<string | null>(null)
  const [attachments, setAttachments] = useState<
    Array<{
      id: string
      file: File
      kind: "image" | "file"
      previewUrl: string | null
    }>
  >([])
  const abortRef = useRef<AbortController | null>(null)
  // Set to a freshly-created conversation id so the conversation-load effect
  // skips fetching it: send() already owns the message state and is about to
  // stream into it; loading the (empty) server list would clobber the stream.
  const skipLoadRef = useRef<number | null>(null)
  const bottomRef = useRef<HTMLDivElement>(null)
  const scrollRef = useRef<HTMLDivElement>(null)
  const atBottomRef = useRef(true)
  const [showJumpToBottom, setShowJumpToBottom] = useState(false)
  const textareaRef = useRef<HTMLTextAreaElement>(null)
  const attachInputRef = useRef<HTMLInputElement>(null)

  // ── 工蜂模式（与普通对话完全隔离，仅在 /w/:id 或手动开关时启用） ──
  const [workerMode, setWorkerMode] = useState(false)
  const [workers, setWorkers] = useState<Worker[]>([])
  const [workerId, setWorkerId] = useState<number | null>(null)
  const [workerModel, setWorkerModel] = useState("claude-opus-4-8")
  const [autoApprove, setAutoApprove] = useState(false)
  const [workerLog, setWorkerLog] = useState<WorkerLogItem[]>([])
  const [activeWorkerSession, setActiveWorkerSession] = useState<number | null>(
    null
  )
  const [workerSending, setWorkerSending] = useState(false)
  const [contextTokens, setContextTokens] = useState(0)
  const workerSeq = useRef(0)
  const workerAbort = useRef<(() => void) | null>(null)
  const pushWorker = (e: AgentEvent) =>
    setWorkerLog((l) => [...l, { ...e, id: workerSeq.current++ }])

  // Release object URLs for removed / unmounted previews.
  useEffect(() => {
    return () => {
      for (const a of attachments) if (a.previewUrl) URL.revokeObjectURL(a.previewUrl)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  useEffect(() => {
    if (!user) {
      setSettings(loadSettings(GUEST_SETTINGS_ID))
      setQuotaMe(null)
      setPublishedFilenames(new Set())
      return
    }
    setSettings(loadSettings(user.id))
    let cancelled = false
    loadEffectiveSettings(user.id).then((s) => {
      if (!cancelled) setSettings(s)
    })
    quotaApi
      .me()
      .then((m) => {
        if (cancelled) return
        setQuotaMe(m)
      })
      .catch(() => {
        /* non-fatal */
      })
    videoEditorApi
      .assets("mine")
      .then((rows) => {
        if (cancelled) return
        setPublishedFilenames((prev) => {
          const next = new Set(prev)
          for (const row of rows) {
            if (!row.is_public) continue
            const filename = filenameFromPath(row.path)
            if (filename) next.add(filename)
          }
          return next
        })
      })
      .catch(() => {
        /* non-fatal */
      })
    return () => {
      cancelled = true
    }
  }, [user])

  useEffect(() => {
    if (!user || settings.chatMode !== "platform" || workerMode) return
    let cancelled = false
    listPlatformModels("chat")
      .then((list) => {
        if (cancelled) return
        const map = new Map<string, number>()
        for (const m of list) {
          if (m.context_limit != null && m.context_limit > 0) map.set(m.model, m.context_limit)
        }
        setPlatformContextMap(map)
      })
      .catch(() => {
        // 静默：回落关键词表
      })
    return () => {
      cancelled = true
    }
  }, [settings.chatMode, workerMode, user])

  async function refreshCredits() {
    if (!user) return
    try {
      const me = await quotaApi.me()
      setQuotaMe(me)
    } catch {
      /* ignore */
    }
  }

  function toggleWebSearch() {
    const next = { ...settings, webSearch: !settings.webSearch }
    setSettings(next)
    saveSettings(settingsOwnerId, next)
    if (next.cloudSync) {
      settingsApi.save(next).catch(() => {
        /* non-fatal */
      })
    }
  }

  useEffect(() => {
    if (!conversationId) {
      setMessages([])
      setSystemPrompt("")
      setAttachedSkills([])
      setCurrentConversation(null)
      setChatUsageTokens(null)
      return
    }
    // A conversation just created by send() that we're about to stream into:
    // skip the fetch — send() owns the message state, and loading the (still
    // empty) server list here would clobber the in-flight stream.
    if (skipLoadRef.current === conversationId) {
      skipLoadRef.current = null
      return
    }
    let cancelled = false
    setLoadingMessages(true)
    setError(null)
    Promise.all([
      conversationsApi.list(),
      conversationsApi.messages(conversationId),
      skillsApi.listForConversation(conversationId).catch(() => [] as Skill[]),
    ])
      .then(([convs, rows, skills]) => {
        if (cancelled) return
        const current = convs.find((c) => c.id === conversationId)
        setSystemPrompt(current?.system_prompt ?? "")
        setCurrentConversation(current ?? null)
        setMessages(rows.map(toUiMessage))
        setAttachedSkills(skills)
        setChatUsageTokens(null)
      })
      .catch((e) => {
        if (cancelled) return
        const msg = e instanceof Error ? e.message : String(e)
        setError(msg)
        if (/404/.test(msg)) nav("/", { replace: true })
      })
      .finally(() => {
        if (!cancelled) setLoadingMessages(false)
      })
    return () => {
      cancelled = true
    }
  }, [conversationId, nav])

  // Auto-delete a conversation when the user leaves it without ever sending
  // a message. Guarded by `loaded` so a slow-loading existing conversation
  // doesn't get nuked if the user clicks away mid-fetch.
  const emptyTrackRef = useRef<{
    id: number
    loaded: boolean
    hadMessages: boolean
  }>({ id: 0, loaded: false, hadMessages: false })
  useEffect(() => {
    if (conversationId == null) return
    if (emptyTrackRef.current.id !== conversationId) {
      emptyTrackRef.current = {
        id: conversationId,
        loaded: false,
        hadMessages: false,
      }
    }
    if (!loadingMessages) emptyTrackRef.current.loaded = true
    if (messages.length > 0) emptyTrackRef.current.hadMessages = true
  }, [conversationId, loadingMessages, messages])
  useEffect(() => {
    if (conversationId == null) return
    const id = conversationId
    return () => {
      const ref = emptyTrackRef.current
      if (ref.id === id && ref.loaded && !ref.hadMessages) {
        conversationsApi
          .remove(id)
          .then(() => setSidebarReload((x) => x + 1))
          .catch(() => {})
      }
    }
  }, [conversationId])

  // Scroll to the anchored message when arriving via Sidebar search (?msg=<id>).
  // Guarded by a per-(convId,msgId) ref so the effect doesn't re-scroll on
  // every messages re-render (e.g. mid-stream token updates).
  useEffect(() => {
    if (targetMsgId == null || conversationId == null) return
    if (loadingMessages) return
    if (!messages.some((m) => m.id === targetMsgId)) return
    const key = `${conversationId}-${targetMsgId}`
    if (msgAnchorScrolledRef.current === key) return
    const handle = requestAnimationFrame(() => {
      const el = document.getElementById(`msg-${targetMsgId}`)
      if (el) {
        el.scrollIntoView({ behavior: "smooth", block: "center" })
        setHighlightedMsgId(targetMsgId)
        msgAnchorScrolledRef.current = key
      }
    })
    return () => cancelAnimationFrame(handle)
  }, [targetMsgId, conversationId, loadingMessages, messages])

  useEffect(() => {
    if (highlightedMsgId == null) return
    const t = window.setTimeout(() => setHighlightedMsgId(null), 2200)
    return () => window.clearTimeout(t)
  }, [highlightedMsgId])

  // 进入工蜂会话：拉历史回看并进入工蜂模式
  useEffect(() => {
    if (workerSessionId == null) return
    setWorkerMode(true)
    if (workerSessionId === activeWorkerSession) return // 自己 nav 进来的当前会话，别清空在跑的日志
    // 切到另一个工蜂会话：先中断上一个会话在跑的流，避免事件串台
    workerAbort.current?.()
    workerAbort.current = null
    setWorkerSending(false)
    setActiveWorkerSession(workerSessionId)
    setWorkerLog([])
    setContextTokens(0)
    workerApi
      .messages(workerSessionId)
      .then((rows) => {
        const events = replayMessages(rows)
        setWorkerLog(events.map((ev) => ({ ...ev, id: workerSeq.current++ })))
      })
      .catch(() => {})
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workerSessionId])

  // 离开工蜂路由：退出工蜂模式并中断在跑的流
  useEffect(() => {
    if (isWorkerRoute) return
    workerAbort.current?.()
    workerAbort.current = null
    setWorkerMode(false)
    setWorkerSending(false)
    setActiveWorkerSession(null)
    setWorkerLog([])
    setContextTokens(0)
  }, [isWorkerRoute])

  // 工蜂模式下加载在线工蜂
  useEffect(() => {
    if (!workerMode) return
    workerApi.list().then(setWorkers).catch(() => {})
  }, [workerMode])

  // 卸载时中断进行中的工蜂会话
  useEffect(() => () => workerAbort.current?.(), [])

  // 工蜂日志增长时跟随到底部（与普通消息一致的 sticky-bottom）
  useEffect(() => {
    if (!workerMode) return
    if (!atBottomRef.current) return
    bottomRef.current?.scrollIntoView({ behavior: "smooth" })
  }, [workerLog, workerMode])

  async function refreshAttachedSkills(convId: number) {
    try {
      const list = await skillsApi.listForConversation(convId)
      setAttachedSkills(list)
    } catch {
      // leave existing state — dialog will show its own errors
    }
  }

  async function publishImage(filename: string, alt: string) {
    if (publishedFilenames.has(filename) || publishingFilename) return
    setPublishingFilename(filename)
    setError(null)
    try {
      const path = `/api/images/${filename}`
      const imported = await videoEditorApi.importAsset({
        id: `chat:${filename}`,
        title: alt || filename,
        kind: "image",
        path,
        thumbnail_path: path,
        source: "generated",
        is_public: false,
        created_at: new Date().toISOString(),
      })
      await videoEditorApi.setVisibility(imported.id, true)
      setPublishedFilenames((s) => new Set(s).add(filename))
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e)
      setError(`分享失败：${msg}`)
    } finally {
      setPublishingFilename(null)
    }
  }

  const effectiveSystemPrompt = useMemo(
    () => composeSystemPromptWithSkills(systemPrompt, attachedSkills),
    [systemPrompt, attachedSkills]
  )

  // 展示用上下文 token：真实 usage 优先，没有则按消息文本估算。
  const displayContextTokens = useMemo(() => {
    if (chatUsageTokens != null) return chatUsageTokens
    return estimateMessagesTokens([
      effectiveSystemPrompt,
      ...messages.map((m) => m.content),
    ])
  }, [chatUsageTokens, effectiveSystemPrompt, messages])

  // Sticky-bottom auto-scroll: only follow the stream when the user is already
  // pinned to the bottom. As soon as they scroll up to read earlier content we
  // stop fighting them — `atBottomRef` is updated by the container's onScroll.
  useEffect(() => {
    if (!atBottomRef.current) return
    bottomRef.current?.scrollIntoView({ behavior: "smooth" })
  }, [messages, streaming])

  function handleScroll(e: React.UIEvent<HTMLDivElement>) {
    const el = e.currentTarget
    const distanceFromBottom = el.scrollHeight - el.scrollTop - el.clientHeight
    const atBottom = distanceFromBottom < 50
    atBottomRef.current = atBottom
    setShowJumpToBottom(!atBottom)
  }

  function jumpToBottom() {
    atBottomRef.current = true
    setShowJumpToBottom(false)
    bottomRef.current?.scrollIntoView({ behavior: "smooth" })
  }

  const configured =
    settings.chatMode === "platform"
      ? Boolean(settings.model)
      : Boolean(settings.baseUrl && settings.apiKey && settings.model)
  const canSend =
    (input.trim().length > 0 || attachments.length > 0) &&
    !streaming &&
    configured

  function addAttachments(files: FileList | File[]) {
    const next = Array.from(files)
      .filter((f) => f.type.startsWith("image/") || isAcceptedDoc(f))
      .map((file) => {
        const kind: "image" | "file" = file.type.startsWith("image/")
          ? "image"
          : "file"
        return {
          id: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
          file,
          kind,
          previewUrl: kind === "image" ? URL.createObjectURL(file) : null,
        }
      })
    if (next.length === 0) return
    setAttachments((prev) => [...prev, ...next])
  }

  function removeAttachment(id: string) {
    setAttachments((prev) => {
      const hit = prev.find((a) => a.id === id)
      if (hit?.previewUrl) URL.revokeObjectURL(hit.previewUrl)
      return prev.filter((a) => a.id !== id)
    })
  }

  function clearAttachments() {
    setAttachments((prev) => {
      for (const a of prev) if (a.previewUrl) URL.revokeObjectURL(a.previewUrl)
      return []
    })
  }

  function bytesToB64(bytes: Uint8Array): string {
    let bin = ""
    const chunk = 0x8000
    for (let i = 0; i < bytes.length; i += chunk) {
      bin += String.fromCharCode(...bytes.subarray(i, i + chunk))
    }
    return btoa(bin)
  }

  // Returns the markdown ref for the uploaded attachment: images use
  // `![](url)` (rebuilt as input_image downstream); documents use a link
  // `[name](/api/files/…)` that the chat-stream layer turns into a native
  // document block per protocol.
  async function uploadAttachment(att: {
    file: File
    kind: "image" | "file"
  }): Promise<string> {
    const b64 = bytesToB64(new Uint8Array(await att.file.arrayBuffer()))
    if (!user) {
      const mime =
        att.file.type ||
        (att.kind === "image" ? "image/png" : "application/octet-stream")
      const dataUrl = `data:${mime};base64,${b64}`
      if (att.kind === "image") return `![](${dataUrl})`
      const safeName = att.file.name.replace(/[[\]()]/g, "_")
      return `[${safeName}](${dataUrl})`
    }
    if (att.kind === "image") {
      const res = await fetch("/api/images/save", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ b64, mime: att.file.type || "image/png" }),
        credentials: "same-origin",
      })
      if (!res.ok) {
        const text = await res.text().catch(() => res.statusText)
        throw new Error(text || `HTTP ${res.status}`)
      }
      const j = (await res.json()) as { path: string }
      return `![](${j.path})`
    }
    const res = await fetch("/api/files/save", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        b64,
        mime: att.file.type || "application/octet-stream",
        filename: att.file.name,
      }),
      credentials: "same-origin",
    })
    if (!res.ok) {
      const text = await res.text().catch(() => res.statusText)
      throw new Error(text || `HTTP ${res.status}`)
    }
    const j = (await res.json()) as { path: string }
    const safeName = att.file.name.replace(/[[\]()]/g, "_")
    return `[${safeName}](${j.path})`
  }

  const banner = useMemo(() => {
    if (!configured) {
      return (
        <div className="rounded-xl border border-amber-500/40 bg-amber-500/5 px-4 py-3">
          <p className="text-sm">
            {settings.chatMode === "platform"
              ? <>尚未选择模型。点击右上角 <b>设置</b> 在「云端额度」模式下选择一个模型。</>
              : <>尚未配置模型。点击右上角 <b>设置</b> 填入 Base URL、Key 和模型名。</>}
          </p>
          <Button
            size="sm"
            variant="outline"
            className="mt-2"
            onClick={() => setSettingsOpen(true)}
          >
            <Settings /> 打开设置
          </Button>
        </div>
      )
    }
    return null
  }, [configured])

  async function ensureConversation(): Promise<number | null> {
    if (conversationId) return conversationId
    try {
      const c = await conversationsApi.create()
      setSystemPrompt(c.system_prompt)
      setSidebarReload((x) => x + 1)
      skipLoadRef.current = c.id
      nav(`/c/${c.id}`, { replace: true })
      return c.id
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
      return null
    }
  }

  async function saveSystemPrompt(next: string) {
    if (!conversationId) {
      setSystemPrompt(next)
      return
    }
    try {
      await conversationsApi.update(conversationId, { system_prompt: next })
      setSystemPrompt(next)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }

  async function refetchMessages(convId: number) {
    try {
      const rows = await conversationsApi.messages(convId)
      setMessages(rows.map(toUiMessage))
    } catch {
      // tolerate; IDs can't be refreshed, retry/edit just won't work
    }
  }

  /** 兜底定格思考耗时。正文首字到达时 onDelta 已经定格过；这里覆盖的是
   * 「思考完但正文一直没来」的情况（用户点了停止 / 上游断流），否则思考区
   * 会永远停在「思考中…」。 */
  function freezeReasoning(startedAt: number) {
    if (!startedAt) return
    const ms = Date.now() - startedAt
    setMessages((prev) => {
      const copy = prev.slice()
      const last = copy[copy.length - 1]
      if (
        last?.role === "assistant" &&
        last.reasoning &&
        last.reasoningMs === undefined
      ) {
        copy[copy.length - 1] = { ...last, reasoningMs: ms }
      }
      return copy
    })
  }

  async function send() {
    if (workerMode) {
      const text = input
      setInput("")
      if (text.trim() === "/compact") {
        await compactWorker()
        return
      }
      await sendWorker(text)
      return
    }
    if (!canSend) return
    if (!user && settings.chatMode === "platform") {
      nav("/login?next=/")
      return
    }
    const text = input.trim()

    const convId = user ? await ensureConversation() : null
    if (user && !convId) return

    // Upload attachments first — fail early so the user message never gets
    // added if saving breaks.
    const pending = attachments
    let uploadedRefs: string[] = []
    if (pending.length > 0) {
      try {
        uploadedRefs = await Promise.all(
          pending.map((a) => uploadAttachment(a))
        )
      } catch (e) {
        setError(`附件上传失败：${e instanceof Error ? e.message : String(e)}`)
        return
      }
    }

    setInput("")
    setError(null)
    clearAttachments()
    // User just hit send — they definitely want to see their own message and
    // the incoming response, so re-arm sticky-bottom regardless of where they
    // were scrolled before.
    atBottomRef.current = true
    setShowJumpToBottom(false)

    // Compose the user message. Text first, then each uploaded attachment as
    // a markdown ref — the chat-stream layer extracts these and re-encodes
    // them as input_image / native document parts.
    const attachMd = uploadedRefs.join("\n")
    const composed = text
      ? attachMd
        ? `${text}\n\n${attachMd}`
        : text
      : attachMd

    const userMsg: UiMessage = { role: "user", content: composed }
    const baseHistory: UiMessage[] = [...messages, userMsg]
    setMessages([...baseHistory, { role: "assistant", content: "" }])
    setStreaming(true)

    const ctrl = new AbortController()
    abortRef.current = ctrl

    let assistantContent = ""
    let assistantReasoning = ""
    let streamError: Error | null = null
    // 思考计时：首个思考增量开始计，正文首字定格。
    let reasoningStart = 0
    let reasoningMs: number | undefined

    const toModel: ChatMessage[] = (effectiveSystemPrompt
      ? [
          { role: "system", content: effectiveSystemPrompt } as ChatMessage,
          ...baseHistory,
        ]
      : (baseHistory as ChatMessage[])
    ).map((m) => ({ role: m.role, content: m.content }))

    try {
      await streamChat({
        protocol: settings.protocol,
        baseUrl: settings.baseUrl,
        apiKey: settings.apiKey,
        model: settings.model,
        useProxy: settings.useProxy,
        usePlatform: settings.chatMode === "platform",
        webSearch: settings.webSearch,
        imageGen: settings.protocol === "openai",
        persistGeneratedImages: Boolean(user),
        messages: toModel,
        signal: ctrl.signal,
        onReasoning: (delta) => {
          if (!reasoningStart) reasoningStart = Date.now()
          assistantReasoning += delta
          setMessages((prev) => {
            const copy = prev.slice()
            const last = copy[copy.length - 1]
            if (last?.role === "assistant") {
              copy[copy.length - 1] = {
                ...last,
                reasoning: (last.reasoning ?? "") + delta,
              }
            }
            return copy
          })
        },
        onDelta: (delta) => {
          // 正文首字到达 = 思考结束，定格耗时供折叠标题显示。
          if (reasoningStart && reasoningMs === undefined) {
            reasoningMs = Date.now() - reasoningStart
          }
          assistantContent += delta
          setMessages((prev) => {
            const copy = prev.slice()
            const last = copy[copy.length - 1]
            if (last?.role === "assistant") {
              copy[copy.length - 1] = {
                ...last,
                content: last.content + delta,
                ...(reasoningMs !== undefined ? { reasoningMs } : {}),
              }
            }
            return copy
          })
        },
        patchAssistant: (update) => {
          setMessages((prev) => {
            const last = prev[prev.length - 1]
            if (last?.role !== "assistant") return prev
            const copy = prev.slice()
            copy[copy.length - 1] = { ...last, content: update(last.content) }
            return copy
          })
        },
        onUsage: (p, c) => setChatUsageTokens(p + c),
      })
    } catch (e) {
      if ((e as { name?: string }).name !== "AbortError") {
        streamError = e instanceof Error ? e : new Error(String(e))
      }
    } finally {
      setStreaming(false)
      abortRef.current = null
      void refreshCredits()
      freezeReasoning(reasoningStart)
      // 中途停止 / 上游断流时 onDelta 没来得及定格，这里补上，好让落库的
      // 耗时和界面显示一致。
      if (reasoningStart && reasoningMs === undefined) {
        reasoningMs = Date.now() - reasoningStart
      }
    }

    if (streamError) {
      setError(streamError.message)
      setMessages((prev) => {
        const copy = prev.slice()
        const last = copy[copy.length - 1]
        if (last?.role === "assistant" && last.content === "") copy.pop()
        return copy
      })
      return
    }

    const toSave: Array<{
      role: "user" | "assistant"
      content: string
      reasoning?: string
      reasoning_ms?: number
    }> = [{ role: "user", content: composed }]
    if (assistantContent) {
      toSave.push({
        role: "assistant",
        content: assistantContent,
        ...(assistantReasoning ? { reasoning: assistantReasoning } : {}),
        ...(assistantReasoning && reasoningMs !== undefined
          ? { reasoning_ms: reasoningMs }
          : {}),
      })
    }
    try {
      if (!user || !convId) return
      await conversationsApi.append(convId, toSave)
      setSidebarReload((x) => x + 1)
      await refetchMessages(convId)
    } catch (e) {
      setError(
        "已生成但保存失败：" + (e instanceof Error ? e.message : String(e))
      )
    }
  }

  async function regenerateLastAssistant() {
    if (streaming) return
    const last = messages[messages.length - 1]
    if (!last || last.role !== "assistant") return
    const prevUser = messages[messages.length - 2]
    if (!prevUser || prevUser.role !== "user") return

    if (user) {
      if (!conversationId || last.id === undefined) return
      try {
        await conversationsApi.truncate(conversationId, last.id)
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
        return
      }
    }

    const trimmed = messages.slice(0, -1)
    setMessages([...trimmed, { role: "assistant", content: "" }])
    setStreaming(true)
    setError(null)

    const ctrl = new AbortController()
    abortRef.current = ctrl
    let assistantContent = ""
    let assistantReasoning = ""
    let streamError: Error | null = null
    // 思考计时：首个思考增量开始计，正文首字定格。
    let reasoningStart = 0
    let reasoningMs: number | undefined

    const history: ChatMessage[] = trimmed.map((m) => ({
      role: m.role,
      content: m.content,
    }))
    const toModel: ChatMessage[] = effectiveSystemPrompt
      ? [{ role: "system", content: effectiveSystemPrompt }, ...history]
      : history

    try {
      await streamChat({
        protocol: settings.protocol,
        baseUrl: settings.baseUrl,
        apiKey: settings.apiKey,
        model: settings.model,
        useProxy: settings.useProxy,
        usePlatform: settings.chatMode === "platform",
        webSearch: settings.webSearch,
        imageGen: settings.protocol === "openai",
        persistGeneratedImages: Boolean(user),
        messages: toModel,
        signal: ctrl.signal,
        onReasoning: (delta) => {
          if (!reasoningStart) reasoningStart = Date.now()
          assistantReasoning += delta
          setMessages((prev) => {
            const copy = prev.slice()
            const tail = copy[copy.length - 1]
            if (tail?.role === "assistant") {
              copy[copy.length - 1] = {
                ...tail,
                reasoning: (tail.reasoning ?? "") + delta,
              }
            }
            return copy
          })
        },
        onDelta: (delta) => {
          // 正文首字到达 = 思考结束，定格耗时供折叠标题显示。
          if (reasoningStart && reasoningMs === undefined) {
            reasoningMs = Date.now() - reasoningStart
          }
          assistantContent += delta
          setMessages((prev) => {
            const copy = prev.slice()
            const tail = copy[copy.length - 1]
            if (tail?.role === "assistant") {
              copy[copy.length - 1] = {
                ...tail,
                content: tail.content + delta,
                ...(reasoningMs !== undefined ? { reasoningMs } : {}),
              }
            }
            return copy
          })
        },
        patchAssistant: (update) => {
          setMessages((prev) => {
            const tail = prev[prev.length - 1]
            if (tail?.role !== "assistant") return prev
            const copy = prev.slice()
            copy[copy.length - 1] = { ...tail, content: update(tail.content) }
            return copy
          })
        },
        onUsage: (p, c) => setChatUsageTokens(p + c),
      })
    } catch (e) {
      if ((e as { name?: string }).name !== "AbortError") {
        streamError = e instanceof Error ? e : new Error(String(e))
      }
    } finally {
      setStreaming(false)
      abortRef.current = null
      freezeReasoning(reasoningStart)
      // 同 send()：中途停止时补上耗时，保证落库值与界面一致。
      if (reasoningStart && reasoningMs === undefined) {
        reasoningMs = Date.now() - reasoningStart
      }
    }

    if (streamError) {
      setError(streamError.message)
      setMessages((prev) => {
        const copy = prev.slice()
        const tail = copy[copy.length - 1]
        if (tail?.role === "assistant" && tail.content === "") copy.pop()
        return copy
      })
      return
    }

    if (assistantContent && user && conversationId) {
      try {
        await conversationsApi.append(conversationId, [
          {
            role: "assistant",
            content: assistantContent,
            ...(assistantReasoning ? { reasoning: assistantReasoning } : {}),
            ...(assistantReasoning && reasoningMs !== undefined
              ? { reasoning_ms: reasoningMs }
              : {}),
          },
        ])
        setSidebarReload((x) => x + 1)
        await refetchMessages(conversationId)
      } catch (e) {
        setError(
          "已生成但保存失败：" + (e instanceof Error ? e.message : String(e))
        )
      }
    }
  }

  async function editLastUser() {
    if (streaming) return
    const last = messages[messages.length - 1]
    let target: UiMessage | undefined
    if (last?.role === "user") target = last
    else if (last?.role === "assistant" && messages[messages.length - 2]?.role === "user")
      target = messages[messages.length - 2]
    if (!target) return

    if (user) {
      if (!conversationId || target.id === undefined) return
      try {
        await conversationsApi.truncate(conversationId, target.id)
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
        return
      }
    }

    const keepUntil = messages.indexOf(target)
    setMessages(keepUntil < 0 ? messages : messages.slice(0, keepUntil))
    setInput(target.content)
    setTimeout(() => textareaRef.current?.focus(), 0)
  }

  async function sendWorker(text: string) {
    if (workerId == null || !text.trim() || workerSending) return
    let s = activeWorkerSession
    try {
      if (s == null) {
        s = (await workerApi.createSession(workerId)).id
        setActiveWorkerSession(s)
        nav(`/w/${s}`, { replace: true })
        setSidebarReload((x) => x + 1)
      }
    } catch (e) {
      pushWorker({ type: "error", data: `创建会话失败：${String(e)}` })
      return
    }
    pushWorker({ type: "text", data: `🧑 ${text.trim()}` })
    setWorkerSending(true)
    workerAbort.current = sendAgentMessage(
      s,
      {
        worker_id: workerId,
        model: workerModel,
        text: text.trim(),
        auto_approve: autoApprove,
      },
      (ev) => {
        if (ev.type === "usage") {
          const it = Number(ev.data?.input_tokens ?? 0)
          const ot = Number(ev.data?.output_tokens ?? 0)
          setContextTokens(it + ot)
          return // usage 不入日志
        }
        pushWorker(ev)
        if (ev.type === "done" || ev.type === "error") {
          setWorkerSending(false)
          workerAbort.current = null
        }
      }
    )
  }

  function formatTokens(n: number): string {
    if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(n % 1_000_000 === 0 ? 0 : 1)}M`
    return n >= 1000 ? `${(n / 1000).toFixed(1)}K` : String(n)
  }

  async function compactWorker() {
    if (workerId == null || activeWorkerSession == null) {
      pushWorker({ type: "error", data: "请先在会话中发起对话再压缩" })
      return
    }
    if (workerSending) return
    pushWorker({ type: "text", data: "🗜️ 正在压缩上下文…" })
    try {
      const r = await workerApi.compact(activeWorkerSession, {
        model: workerModel,
      })
      if (!r.ok) {
        pushWorker({ type: "error", data: r.message ?? "压缩失败" })
        return
      }
      const after = Number(r.after_estimate ?? 0)
      pushWorker({
        type: "text",
        data: `✅ 已压缩，上下文约 ${formatTokens(contextTokens)} → ${formatTokens(after)}`,
      })
      setContextTokens(after)
    } catch (e) {
      pushWorker({ type: "error", data: `压缩失败：${String(e)}` })
    }
  }

  async function decideWorker(item: WorkerLogItem, decision: boolean) {
    if (activeWorkerSession == null) return
    await workerApi
      .approve(activeWorkerSession, String(item.data?.call_id ?? ""), decision)
      .catch(() => {})
    setWorkerLog((l) =>
      l.map((x) => (x.id === item.id ? { ...x, resolved: true } : x))
    )
  }

  async function copyText(content: string, key: string) {
    try {
      await navigator.clipboard.writeText(content)
      setCopiedKey(key)
      setTimeout(() => setCopiedKey((k) => (k === key ? null : k)), 1500)
    } catch {
      setError("复制失败：浏览器不允许剪贴板访问")
    }
  }

  function stop() {
    abortRef.current?.abort()
  }

  function fillSample(text: string) {
    setInput((v) => (v ? v : text))
    textareaRef.current?.focus()
  }

  const lastIdx = messages.length - 1

  return (
    <div className="app-shell flex h-svh bg-background text-foreground">
      {mobileNavOpen && (
        <button
          type="button"
          aria-label="关闭侧栏"
          onClick={() => setMobileNavOpen(false)}
          className="fixed inset-0 z-30 bg-slate-950/45 backdrop-blur-[2px] md:hidden"
        />
      )}
      <div
        className={cn(
          "fixed inset-y-0 left-0 z-40 transition-transform",
          mobileNavOpen ? "translate-x-0 shadow-2xl" : "-translate-x-full",
          "md:static md:translate-x-0 md:shadow-none"
        )}
      >
        <Sidebar
          reloadKey={sidebarReload}
          onOpenLibrary={() => setLibraryOpen(true)}
          onNewGuest={() => {
            if (streaming) stop()
            setMessages([])
            setSystemPrompt("")
            setAttachedSkills([])
            setCurrentConversation(null)
            setChatUsageTokens(null)
            setInput("")
            clearAttachments()
            setError(null)
          }}
          onNavigate={() => setMobileNavOpen(false)}
        />
      </div>

      <div className="flex min-w-0 flex-1 flex-col bg-background/25">
        <header className="relative z-30 flex min-h-16 items-center justify-between gap-2 border-b border-border/60 bg-background/65 px-2.5 py-2.5 backdrop-blur-xl md:gap-3 md:px-6">
          <div className="flex min-w-0 flex-1 items-center gap-1.5 md:gap-3">
            <Button
              variant="ghost"
              size="icon"
              className="shrink-0 md:hidden"
              onClick={() => setMobileNavOpen(true)}
              aria-label="打开侧栏"
            >
              <Menu />
            </Button>
            <div className="hidden min-w-0 md:block">
              <h1 className="truncate text-sm font-semibold tracking-tight">
                {conversationId ? `会话 #${conversationId}` : "新对话"}
              </h1>
              <p className="mt-0.5 text-[10px] text-muted-foreground">AI 智能对话</p>
            </div>
            <ModelPicker
              protocol={settings.protocol}
              model={settings.model}
              settings={settings}
              onChangeModel={(next, nextProtocol) => {
                const updated: UpstreamSettings = {
                  ...settings,
                  model: next,
                  ...(nextProtocol ? { protocol: nextProtocol } : {}),
                }
                setSettings(updated)
                saveSettings(settingsOwnerId, updated)
                if (updated.cloudSync) {
                  settingsApi.save(updated).catch(() => {
                    /* non-fatal */
                  })
                }
              }}
            />
          </div>
          <div className="flex shrink-0 items-center">
            {quotaMe && (
              <div className="mr-1 inline-flex items-center overflow-hidden rounded-xl border border-border/70 bg-card/65 text-xs tabular-nums shadow-sm backdrop-blur transition-colors hover:border-primary/30">
                <button
                  type="button"
                  onClick={() => setLedgerOpen(true)}
                  className="inline-flex items-center gap-1 px-2 py-1.5 hover:bg-primary/10 md:px-2.5"
                  title={`剩余额度 ${formatQuota(quotaMe.balance)} 元｜点击查看额度明细`}
                >
                  <span className="hidden text-muted-foreground md:inline">¥</span>
                  <span className="font-medium">{formatQuotaCompact(quotaMe.balance)}</span>
                </button>
                <button
                  type="button"
                  onClick={() => setRechargeOpen(true)}
                  className="inline-flex items-center border-l border-border px-2 py-1 text-muted-foreground hover:bg-primary/10 hover:text-primary"
                  title="充值额度"
                  aria-label="充值额度"
                >
                  <Plus className="size-3" />
                </button>
              </div>
            )}
            <Button
              variant="ghost"
              size="icon"
              onClick={() => setSystemPromptOpen(true)}
              title={systemPrompt ? "系统提示词（已设置）" : "系统提示词"}
              className="relative size-8 md:size-9"
            >
              <MessageSquareText />
              {systemPrompt && (
                <span className="absolute right-1.5 top-1.5 size-1.5 rounded-full bg-primary" />
              )}
            </Button>
            {user && (
              <>
                <Button
                  variant="ghost"
                  size="icon"
                  onClick={() => setLibraryOpen(true)}
                  title="提示词库"
                  className="hidden md:inline-flex"
                >
                  <BookMarked />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  onClick={() => setSkillsOpen(true)}
                  title={
                    attachedSkills.length > 0
                      ? `Skills（已挂载 ${attachedSkills.length}）`
                      : "Skills"
                  }
                  className="relative size-8 md:size-9"
                >
                  <Wand2 />
                  {attachedSkills.length > 0 && (
                    <span className="absolute right-1 top-1 min-w-4 rounded-full bg-primary px-1 text-[10px] font-semibold leading-4 text-primary-foreground">
                      {attachedSkills.length}
                    </span>
                  )}
                </Button>
              </>
            )}
            <Button
              variant="ghost"
              size="icon"
              onClick={() => nav("/library")}
              title="素材库"
              className="hidden size-8 sm:inline-flex md:size-9"
            >
              <Library />
            </Button>
            <Button
              variant="ghost"
              size="icon"
              onClick={() => setShareOpen(true)}
              title="分享 / 导出"
              disabled={!currentConversation || messages.length === 0}
              className="size-8 md:size-9"
            >
              <Share2 />
            </Button>
            <Button
              variant="ghost"
              size="icon"
              onClick={() => setSettingsOpen(true)}
              title="设置"
              className="size-8 md:size-9"
            >
              <Settings />
            </Button>
          </div>
        </header>

        <div
          ref={scrollRef}
          onScroll={handleScroll}
          className="nc-scroll relative flex-1 overflow-y-auto"
        >
          <div className="mx-auto flex w-full max-w-4xl flex-col gap-5 px-3 py-5 md:px-6 md:py-8">
            {banner}
            {loadingMessages && (
              <p className="text-center text-sm text-muted-foreground">加载中…</p>
            )}
            {workerMode ? (
              <WorkerLog items={workerLog} onDecide={decideWorker} />
            ) : (
              <>
                {!loadingMessages && messages.length === 0 && configured && (
              <div className="fade-up mx-auto mt-8 flex w-full max-w-2xl flex-col items-center gap-7 text-center md:mt-14">
                <div className="relative">
                  <div className="absolute inset-2 rounded-3xl bg-primary/30 blur-2xl" />
                  <img
                    src="/logo.png"
                    alt=""
                    className="relative size-16 rounded-[1.35rem] ring-1 ring-white/15 shadow-panel md:size-[4.5rem]"
                  />
                </div>
                <div>
                  <div className="mb-3 inline-flex items-center gap-1.5 rounded-full border border-primary/15 bg-primary/5 px-3 py-1 text-[11px] font-medium text-primary">
                    <Sparkles className="size-3" /> Yunova AI 助手
                  </div>
                  <p className="text-2xl font-semibold tracking-[-0.035em] md:text-3xl">
                    {user?.display_name?.trim() || user?.username
                      ? `你好，${user?.display_name?.trim() || user?.username}`
                      : "开始一段对话"}
                  </p>
                  <p className="mx-auto mt-2 max-w-xl text-sm leading-relaxed text-muted-foreground">
                    今天想一起完成什么？你可以直接提问、上传文件，或从下面的灵感开始。
                    {settings.protocol === "openai"
                      ? " 当前模型还支持直接用文字描述生成图片。"
                      : ""}
                  </p>
                </div>
                <div className="grid w-full grid-cols-1 gap-2.5 sm:grid-cols-2">
                  {SAMPLE_PROMPTS.map((p) => {
                    const Icon = p.icon
                    return (
                      <button
                        key={p.title}
                        type="button"
                        onClick={() => fillSample(p.body)}
                        className="group glass-surface flex min-h-24 items-start gap-3 rounded-2xl p-4 text-left transition-all hover:-translate-y-0.5 hover:border-primary/25 hover:shadow-lg"
                      >
                        <span className="grid size-9 shrink-0 place-items-center rounded-xl bg-primary/8 text-primary transition-colors group-hover:bg-primary/12">
                          <Icon className="size-4" />
                        </span>
                        <span className="min-w-0 flex-1">
                          <span className="flex items-center justify-between text-xs font-semibold text-foreground">
                            {p.title}
                            <ArrowUpRight className="size-3.5 text-muted-foreground transition-transform group-hover:-translate-y-0.5 group-hover:translate-x-0.5 group-hover:text-primary" />
                          </span>
                          <span className="mt-1.5 line-clamp-2 text-sm leading-relaxed text-muted-foreground">
                            {p.body}
                          </span>
                        </span>
                      </button>
                    )
                  })}
                </div>
              </div>
            )}
            {messages.map((m, i) => {
              const key = m.id !== undefined ? `m-${m.id}` : `t-${i}`
              const isLast = i === lastIdx
              const isLastAssistant =
                isLast &&
                m.role === "assistant" &&
                (!user || m.id !== undefined) &&
                !streaming
              const isLastUser =
                isLast &&
                m.role === "user" &&
                (!user || m.id !== undefined) &&
                !streaming
              // Edit should also be available when last assistant follows a last user
              const isSecondLastUser =
                i === lastIdx - 1 &&
                m.role === "user" &&
                (!user || m.id !== undefined) &&
                !streaming &&
                messages[lastIdx]?.role === "assistant"
              const isHighlighted =
                m.id !== undefined && highlightedMsgId === m.id
              return (
                <div
                  key={key}
                  id={m.id != null ? `msg-${m.id}` : undefined}
                  className={cn(
                    "scroll-mt-24 rounded-2xl transition-colors duration-700",
                    isHighlighted
                      ? "bg-yellow-300/20 dark:bg-yellow-400/15"
                      : "bg-transparent"
                  )}
                >
                  <Bubble
                    message={m}
                    actions={{
                      onCopy: () => copyText(m.content, key),
                      copied: copiedKey === key,
                      onRetry: isLastAssistant ? regenerateLastAssistant : undefined,
                      onEdit:
                        isLastUser || isSecondLastUser ? editLastUser : undefined,
                    }}
                    onPublishImage={
                      m.role === "assistant" && user ? publishImage : undefined
                    }
                    publishedFilenames={publishedFilenames}
                    publishingFilename={publishingFilename}
                    userAvatarUrl={user?.avatar_url ?? null}
                    userInitial={(
                      user?.display_name?.trim() || user?.username || "?"
                    ).slice(0, 1)}
                  />
                </div>
              )
            })}
              </>
            )}
            {error && (
              <div className="rounded-xl border border-destructive/40 bg-destructive/10 px-4 py-2.5 text-sm text-destructive">
                {error}
              </div>
            )}
            <div ref={bottomRef} />
          </div>
          {showJumpToBottom && (
            <button
              type="button"
              onClick={jumpToBottom}
              aria-label="回到底部"
              className="sticky bottom-4 ml-auto mr-4 flex size-9 items-center justify-center rounded-full border border-border bg-background/90 text-foreground shadow-panel backdrop-blur-sm transition hover:bg-accent"
            >
              <ArrowDown className="size-4" />
            </button>
          )}
        </div>

        <div className="border-t border-border/40 bg-background/70 px-3 pb-3 pt-2.5 backdrop-blur-xl md:px-6 md:pb-4">
          <div className="mx-auto max-w-4xl">
            <div className="mb-2 flex flex-wrap items-center gap-2.5 px-1 text-sm">
              <label className="flex cursor-pointer items-center gap-1.5 rounded-full border border-border/70 bg-card/55 px-2.5 py-1 text-xs shadow-sm transition-colors hover:border-primary/20">
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
                  <Select
                    value={workerId === null ? WORKER_NONE : String(workerId)}
                    onValueChange={(v) =>
                      setWorkerId(v === WORKER_NONE ? null : Number(v))
                    }
                  >
                    <SelectTrigger size="sm">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value={WORKER_NONE}>选择工蜂…</SelectItem>
                      {workers
                        .filter((w) => w.online)
                        .map((w) => (
                          <SelectItem key={w.id} value={String(w.id)}>
                            {w.name}
                          </SelectItem>
                        ))}
                    </SelectContent>
                  </Select>
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
                    <span className="text-xs text-muted-foreground">
                      没有在线工蜂，去设置里配对
                    </span>
                  )}
                </>
              )}
              {!workerMode && (() => {
                const limit =
                  (settings.chatMode === "platform"
                    ? platformContextMap.get(settings.model)
                    : undefined) ?? contextLimit(settings.model)
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
            </div>
            {attachments.length > 0 && (
              <div className="mb-2 flex flex-wrap gap-2 rounded-xl border border-border bg-card p-2">
                {attachments.map((a) =>
                  a.kind === "image" && a.previewUrl ? (
                    <div
                      key={a.id}
                      className="group relative size-16 overflow-hidden rounded-lg border border-border bg-muted"
                    >
                      <img
                        src={a.previewUrl}
                        alt={a.file.name}
                        className="size-full object-cover"
                      />
                      <button
                        type="button"
                        onClick={() => removeAttachment(a.id)}
                        className="absolute right-0.5 top-0.5 grid size-5 place-items-center rounded-full bg-background/80 text-muted-foreground opacity-0 transition-opacity hover:bg-background hover:text-foreground group-hover:opacity-100"
                        aria-label="移除附件"
                        title="移除附件"
                        disabled={streaming}
                      >
                        <X className="size-3" />
                      </button>
                    </div>
                  ) : (
                    <div
                      key={a.id}
                      className="group relative flex h-16 max-w-44 items-center gap-2 overflow-hidden rounded-lg border border-border bg-muted px-3 pr-7"
                      title={a.file.name}
                    >
                      <FileText className="size-5 shrink-0 text-muted-foreground" />
                      <span className="truncate text-xs">{a.file.name}</span>
                      <button
                        type="button"
                        onClick={() => removeAttachment(a.id)}
                        className="absolute right-0.5 top-0.5 grid size-5 place-items-center rounded-full bg-background/80 text-muted-foreground opacity-0 transition-opacity hover:bg-background hover:text-foreground group-hover:opacity-100"
                        aria-label="移除附件"
                        title="移除附件"
                        disabled={streaming}
                      >
                        <X className="size-3" />
                      </button>
                    </div>
                  )
                )}
              </div>
            )}
            <input
              ref={attachInputRef}
              type="file"
              accept={`image/png,image/jpeg,image/webp,image/gif,${DOC_ACCEPT}`}
              multiple
              className="hidden"
              onChange={(e) => {
                if (e.target.files) addAttachments(e.target.files)
                e.target.value = ""
              }}
            />
            {workerMode && (() => {
              const limit = contextLimit(workerModel)
              const pct = Math.min(100, Math.round((contextTokens / limit) * 100))
              const warn = pct >= 80
              return (
                <div className="mb-1.5 px-1">
                  <div
                    className={`flex items-center justify-between text-xs ${
                      warn ? "text-orange-500" : "text-muted-foreground"
                    }`}
                  >
                    <span>
                      {warn
                        ? `上下文已用 ${pct}%，输入 /compact 压缩历史`
                        : `上下文 ${formatTokens(contextTokens)} / ${formatTokens(limit)} (${pct}%)`}
                    </span>
                  </div>
                  <div className="mt-1 h-1 w-full overflow-hidden rounded-full bg-muted">
                    <div
                      className={`h-full rounded-full transition-all ${
                        warn ? "bg-orange-500" : "bg-primary/50"
                      }`}
                      style={{ width: `${pct}%` }}
                    />
                  </div>
                </div>
              )
            })()}
            <div className="glass-surface flex items-end gap-1.5 rounded-[1.35rem] p-2.5 transition-all focus-within:border-primary/35 focus-within:shadow-[0_18px_48px_-24px_color-mix(in_oklch,var(--primary)_45%,transparent)] focus-within:ring-2 focus-within:ring-ring">
              <Button
                type="button"
                variant="ghost"
                size="icon"
                className="shrink-0"
                aria-label="附加图片或文件"
                title="附加图片或文件（PDF / 文本，支持多选）"
                disabled={streaming}
                onClick={() => attachInputRef.current?.click()}
              >
                <Paperclip />
              </Button>
              <Button
                type="button"
                variant={settings.webSearch ? "default" : "ghost"}
                size="icon"
                className="shrink-0"
                aria-label={
                  settings.webSearch ? "关闭联网搜索" : "开启联网搜索"
                }
                title={
                  !likelyWebSearchCapable(settings.protocol, settings.model)
                    ? `当前模型可能不支持联网搜索（${
                        settings.model || "未选择"
                      }）；点击仍可切换，请求失败请改选支持的模型`
                    : settings.webSearch
                      ? "联网搜索：开启（点击关闭）"
                      : "联网搜索：关闭（点击开启）"
                }
                disabled={streaming}
                onClick={toggleWebSearch}
              >
                <Globe />
              </Button>
              <Textarea
                ref={textareaRef}
                value={input}
                onChange={(e) => setInput(e.target.value)}
                onPaste={(e) => {
                  const items = e.clipboardData?.items
                  if (!items) return
                  const images: File[] = []
                  for (const item of items) {
                    if (item.kind !== "file") continue
                    if (!item.type.startsWith("image/")) continue
                    const f = item.getAsFile()
                    if (f) {
                      const ext = (f.type.split("/")[1] || "png").split("+")[0]
                      const named =
                        f.name && f.name !== "image.png"
                          ? f
                          : new File([f], `pasted-${Date.now()}.${ext}`, {
                              type: f.type,
                            })
                      images.push(named)
                    }
                  }
                  if (images.length > 0) {
                    e.preventDefault()
                    addAttachments(images)
                  }
                }}
                onKeyDown={(e) => {
                  if (
                    e.key === "Enter" &&
                    !e.shiftKey &&
                    !e.nativeEvent.isComposing
                  ) {
                    e.preventDefault()
                    void send()
                  }
                }}
                placeholder={
                  !configured ? "先在设置中配置 API…" : "输入消息，或粘贴图片…"
                }
                rows={1}
                className="max-h-60 min-h-[42px] flex-1 resize-none border-0 bg-transparent px-2.5 py-2.5 shadow-none focus-visible:ring-0"
              />
              {streaming ? (
                <Button
                  onClick={stop}
                  variant="secondary"
                  size="icon"
                  className="size-10 shrink-0 rounded-xl"
                  aria-label="停止"
                >
                  <Square />
                </Button>
              ) : (
                <Button
                  onClick={() => void send()}
                  disabled={
                    workerMode
                      ? workerSending || workerId == null || !input.trim()
                      : !canSend
                  }
                  size="icon"
                  className="size-10 shrink-0 rounded-xl shadow-md shadow-primary/20"
                  aria-label="发送"
                  title="发送"
                >
                  <ArrowUp />
                </Button>
              )}
            </div>
            <p className="mt-2 text-center text-[10px] tracking-wide text-muted-foreground/80">
              Yunova 可能会生成不准确的信息，请核对重要内容
            </p>
          </div>
        </div>
      </div>

      <SettingsDialog
        open={settingsOpen}
        initial={settings}
        isAuthenticated={Boolean(user)}
        onClose={() => setSettingsOpen(false)}
        onLoginRequired={() => {
          setSettingsOpen(false)
          nav("/login?next=/")
        }}
        onSave={(s) => {
          const prevCloud = settings.cloudSync
          const next: UpstreamSettings = user
            ? s
            : {
                ...s,
                chatMode: "byok",
                imageMode: "byok",
                cloudSync: false,
              }
          saveSettings(settingsOwnerId, next)
          setSettings(next)
          setSettingsOpen(false)
          if (user && next.cloudSync) {
            settingsApi.save(next).catch((e) => {
              setError(`云端同步失败：${e instanceof Error ? e.message : String(e)}`)
            })
          } else if (user && prevCloud) {
            settingsApi.remove().catch(() => {
              // ignore — user turned cloud off, best-effort cleanup
            })
          }
        }}
      />

      <RechargeDialog
        open={rechargeOpen}
        onClose={() => setRechargeOpen(false)}
        onPaid={() => void refreshCredits()}
      />

      <QuotaLedgerDialog
        open={ledgerOpen}
        onClose={() => setLedgerOpen(false)}
      />

      <PromptLibrary
        open={libraryOpen}
        onClose={() => setLibraryOpen(false)}
        onApplyToCurrent={(content) => {
          void saveSystemPrompt(content)
          setLibraryOpen(false)
        }}
      />

      <SystemPromptDialog
        open={systemPromptOpen}
        value={systemPrompt}
        onClose={() => setSystemPromptOpen(false)}
        onSave={saveSystemPrompt}
      />

      <ShareDialog
        open={shareOpen}
        conversation={currentConversation}
        onClose={() => setShareOpen(false)}
      />

      <SkillsDialog
        open={skillsOpen}
        conversationId={conversationId}
        attachedIds={attachedSkills.map((s) => s.id)}
        onClose={() => {
          setSkillsOpen(false)
          if (conversationId) void refreshAttachedSkills(conversationId)
        }}
        onAttachedChange={(ids) => {
          setAttachedSkills((prev) => {
            const byId = new Map(prev.map((s) => [s.id, s]))
            return ids
              .map((id) => byId.get(id))
              .filter((x): x is Skill => Boolean(x))
          })
          if (conversationId) void refreshAttachedSkills(conversationId)
        }}
      />

    </div>
  )
}
