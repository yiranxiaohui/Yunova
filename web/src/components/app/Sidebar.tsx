import { useEffect, useMemo, useState, type ReactNode } from "react"
import { Link, useLocation, useNavigate, useParams } from "react-router-dom"
import {
  BookMarked,
  Clapperboard,
  ImageIcon,
  Library,
  LogIn,
  LogOut,
  MessageSquareText,
  MoreHorizontal,
  Pencil,
  Plus,
  Cloud,
  Laptop,
  Search,
  Scissors,
  Shield,
  Sparkles,
  Trash2,
  User,
  Workflow,
} from "lucide-react"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { conversationsApi, type Conversation } from "@/lib/conversations"
import { searchApi, type SearchHit } from "@/lib/search"
import { agentApi, type AgentSession, type AgentTarget } from "@/lib/agent"
import { cn } from "@/lib/utils"
import { useAuth } from "@/lib/auth-context"
import { useConfirm } from "@/lib/confirm-context"
import { BrandMark } from "./BrandMark"
import { ProfileDialog } from "./ProfileDialog"

type Props = {
  /** Bump to force the session list to reload. Optional so pages that do not
   *  mutate the list (for example the work-mode task page) can omit it. */
  reloadKey?: number
  onCreated?: (c: Conversation) => void
  onOpenLibrary?: () => void
  onNewGuest?: () => void
  /** Called after the user picks a navigation target (link or button).
   *  Parent uses this to close the mobile drawer. */
  onNavigate?: () => void
}

type SidebarItem =
  | { kind: "chat"; id: number; title: string; updated_at: string }
  | { kind: "agent"; id: number; title: string; updated_at: string; target: AgentTarget }

function relativeTime(iso: string): string {
  // Timestamps arrive in two shapes: SQLite's `datetime('now')` ("2026-09-15
  // 09:03:43", implicitly UTC) and RFC 3339 with an explicit offset. Appending
  // "Z" unconditionally corrupts the latter into an unparseable string, which
  // surfaced as "Invalid Date" in the list.
  const hasZone = /(?:Z|[+-]\d{2}:?\d{2})$/.test(iso)
  const d = new Date(iso.replace(" ", "T") + (hasZone ? "" : "Z"))
  if (Number.isNaN(d.getTime())) return ""
  const diff = Date.now() - d.getTime()
  const m = Math.floor(diff / 60000)
  if (m < 1) return "刚刚"
  if (m < 60) return `${m} 分钟前`
  const h = Math.floor(m / 60)
  if (h < 24) return `${h} 小时前`
  const days = Math.floor(h / 24)
  if (days < 7) return `${days} 天前`
  return d.toLocaleDateString()
}

export function Sidebar({
  reloadKey,
  onCreated,
  onOpenLibrary,
  onNewGuest,
  onNavigate,
}: Props) {
  const { confirm, prompt } = useConfirm()
  const { id: paramId } = useParams()
  const activeId = paramId ? Number(paramId) : null
  const location = useLocation()
  const activeAgent = location.pathname.startsWith("/t/")
  const nav = useNavigate()
  const auth = useAuth()
  const user = auth.state.status === "authed" ? auth.state.user : null

  const [items, setItems] = useState<SidebarItem[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [menuFor, setMenuFor] = useState<string | null>(null)
  const [query, setQuery] = useState("")
  const [profileOpen, setProfileOpen] = useState(false)
  const [avatarBroken, setAvatarBroken] = useState(false)
  const [searchHits, setSearchHits] = useState<SearchHit[] | null>(null)
  const [searching, setSearching] = useState(false)
  const [searchError, setSearchError] = useState<string | null>(null)

  useEffect(() => {
    setAvatarBroken(false)
  }, [user?.avatar_url])

  useEffect(() => {
    if (!user) {
      setItems([])
      setError(null)
      setLoading(false)
      return
    }
    let cancelled = false
    setLoading(true)
    Promise.all([
      conversationsApi.list().catch(() => [] as Conversation[]),
      agentApi.sessions().catch(() => [] as AgentSession[]),
    ])
      .then(([convs, agents]) => {
        if (cancelled) return
        const merged: SidebarItem[] = [
          ...convs.map((c) => ({
            kind: "chat" as const,
            id: c.id,
            title: c.title,
            updated_at: c.updated_at,
          })),
          ...agents.map((s) => ({
            kind: "agent" as const,
            id: s.id,
            title: s.title,
            updated_at: s.updated_at,
            target: s.target,
          })),
        ].sort((a, b) => (a.updated_at < b.updated_at ? 1 : -1))
        setItems(merged)
      })
      .catch((e) => {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e))
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [reloadKey, user])

  const trimmedQuery = query.trim()
  const isApiSearch = trimmedQuery.length >= 2
  const filtered = useMemo(() => {
    if (isApiSearch) return items
    const q = trimmedQuery.toLowerCase()
    if (!q) return items
    return items.filter((c) => c.title.toLowerCase().includes(q))
  }, [items, trimmedQuery, isApiSearch])

  useEffect(() => {
    if (!user || !isApiSearch) {
      setSearchHits(null)
      setSearching(false)
      setSearchError(null)
      return
    }
    let cancelled = false
    setSearchError(null)
    setSearching(true)
    const handle = window.setTimeout(() => {
      searchApi
        .conversations(trimmedQuery)
        .then((hits) => {
          if (!cancelled) setSearchHits(hits)
        })
        .catch((e) => {
          if (!cancelled) {
            setSearchHits([])
            setSearchError(e instanceof Error ? e.message : String(e))
          }
        })
        .finally(() => {
          if (!cancelled) setSearching(false)
        })
    }, 280)
    return () => {
      cancelled = true
      window.clearTimeout(handle)
    }
  }, [trimmedQuery, isApiSearch, user])

  async function createNew() {
    if (!user) {
      onNewGuest?.()
      nav("/")
      onNavigate?.()
      return
    }
    try {
      const c = await conversationsApi.create()
      setItems((s) => [
        { kind: "chat" as const, id: c.id, title: c.title, updated_at: c.updated_at },
        ...s,
      ])
      onCreated?.(c)
      nav(`/c/${c.id}`)
      onNavigate?.()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }

  async function rename(c: SidebarItem) {
    const next = await prompt({
      title: "重命名会话",
      defaultValue: c.title,
      placeholder: "会话标题",
    })
    if (next == null) return
    const title = next.trim()
    if (!title || title === c.title) return
    try {
      if (c.kind === "agent") {
        // Agent tasks have no rename endpoint yet; their title comes from the
        // first prompt. Skip rather than show a misleading success.
        setError("工作任务暂不支持重命名")
        return
      } else {
        await conversationsApi.update(c.id, { title })
      }
      setItems((s) =>
        s.map((x) => (x.kind === c.kind && x.id === c.id ? { ...x, title } : x))
      )
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }

  async function remove(c: SidebarItem) {
    const ok = await confirm({
      title: `删除会话 "${c.title}"？`,
      description: "此操作不可撤销。",
      confirmText: "删除",
      destructive: true,
    })
    if (!ok) return
    try {
      if (c.kind === "agent") {
        // Stopping releases the runtime and its container. The transcript is
        // kept: there is no delete endpoint yet, and silently dropping the
        // row from the list would hide work the user can still open.
        await agentApi.stop(c.id)
        setError("已停止该任务的运行时；记录仍可查看")
        return
      } else {
        await conversationsApi.remove(c.id)
      }
      setItems((s) => s.filter((x) => !(x.kind === c.kind && x.id === c.id)))
      // `agent` returned above, so only chat reaches here.
      if (!activeAgent && activeId === c.id) nav("/")
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }

  async function removeAll() {
    const chatCount = items.filter((x) => x.kind === "chat").length
    const ok = await confirm({
      title: `确定要清空全部 ${chatCount} 个会话吗？`,
      description: "此操作不可撤销。",
      confirmText: "清空",
      destructive: true,
    })
    if (!ok) return
    try {
      await conversationsApi.removeAll()
      setItems((s) => s.filter((x) => x.kind !== "chat"))
      nav("/")
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }

  return (
    <aside className="flex h-full w-[18rem] shrink-0 flex-col border-r border-sidebar-border bg-sidebar/95 text-sidebar-foreground shadow-[12px_0_40px_-32px_rgba(37,24,70,0.45)] backdrop-blur-xl">
      <div className="flex items-center justify-between px-4 pb-4 pt-5">
        <BrandMark subtitle="Agent 工作空间" />
      </div>

      <div className="flex flex-col gap-3 px-3 pb-3">
        <Button
          onClick={createNew}
          className="h-11 w-full justify-start gap-2.5 rounded-xl bg-gradient-to-r from-primary to-chart-5 px-3.5 shadow-md shadow-primary/20 hover:shadow-lg hover:shadow-primary/25"
        >
          <span className="grid size-6 place-items-center rounded-md bg-white/15">
            <Plus className="size-4" />
          </span>
          <span className="font-semibold">开启新对话</span>
        </Button>

        {/* Work mode is a peer of chat, not a setting inside it: it starts an
            agent that can run commands, so it gets its own entry point. */}
        <Button
          asChild
          variant="outline"
          className="h-10 w-full justify-start gap-2.5 rounded-xl px-3.5"
        >
          <Link to="/t" onClick={() => onNavigate?.()} title="启动可执行命令的 Agent 任务">
            <span className="grid size-6 place-items-center rounded-md bg-primary/10 text-primary">
              <Cloud className="size-4" />
            </span>
            <span className="font-semibold">新工作任务</span>
          </Link>
        </Button>

        <div className="grid grid-cols-2 gap-2">
          <Link
            to="/studio"
            onClick={() => onNavigate?.()}
            title="多轮对话式生图（Responses API）"
            className="group flex min-h-20 flex-col justify-between rounded-xl border border-sidebar-border bg-background/45 p-3 shadow-sm transition-all hover:-translate-y-0.5 hover:border-primary/25 hover:bg-background/80 hover:shadow-md"
          >
            <span className="grid size-8 place-items-center rounded-lg bg-violet-500/10 text-violet-600 transition-colors group-hover:bg-violet-500/15 dark:text-violet-300">
              <ImageIcon className="size-4" />
            </span>
            <span className="text-xs font-medium">图像工作室</span>
          </Link>
          <Link
            to="/videos"
            onClick={() => onNavigate?.()}
            title="文生视频 / 图生视频"
            className="group flex min-h-20 flex-col justify-between rounded-xl border border-sidebar-border bg-background/45 p-3 shadow-sm transition-all hover:-translate-y-0.5 hover:border-primary/25 hover:bg-background/80 hover:shadow-md"
          >
            <span className="grid size-8 place-items-center rounded-lg bg-sky-500/10 text-sky-600 transition-colors group-hover:bg-sky-500/15 dark:text-sky-300">
              <Clapperboard className="size-4" />
            </span>
            <span className="text-xs font-medium">视频工作室</span>
          </Link>
          <Link
            to="/workflows"
            onClick={() => onNavigate?.()}
            title="图片 / 多视频 / 裁剪 / 合并节点画布"
            className="group flex min-h-20 flex-col justify-between rounded-xl border border-sidebar-border bg-background/45 p-3 shadow-sm transition-all hover:-translate-y-0.5 hover:border-primary/25 hover:bg-background/80 hover:shadow-md"
          >
            <span className="grid size-8 place-items-center rounded-lg bg-emerald-500/10 text-emerald-600 transition-colors group-hover:bg-emerald-500/15 dark:text-emerald-300">
              <Workflow className="size-4" />
            </span>
            <span className="text-xs font-medium">流水线</span>
          </Link>
          <Link
            to="/editor"
            onClick={() => onNavigate?.()}
            title="多轨精细剪辑、素材库与服务端导出"
            className="group flex min-h-20 flex-col justify-between rounded-xl border border-sidebar-border bg-background/45 p-3 shadow-sm transition-all hover:-translate-y-0.5 hover:border-primary/25 hover:bg-background/80 hover:shadow-md"
          >
            <span className="grid size-8 place-items-center rounded-lg bg-amber-500/10 text-amber-600 transition-colors group-hover:bg-amber-500/15 dark:text-amber-300">
              <Scissors className="size-4" />
            </span>
            <span className="text-xs font-medium">在线剪辑</span>
          </Link>
        </div>

        <div className="relative">
          <Search className="pointer-events-none absolute left-3 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
          <Input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={user ? "搜索会话…" : "登录后查看历史会话"}
            disabled={!user}
            className="h-9 rounded-xl border-sidebar-border bg-background/45 pl-9 text-sm shadow-none"
          />
        </div>
      </div>

      <div className="flex items-center justify-between px-4 pb-1 pt-1">
        <span className="text-[11px] font-semibold uppercase tracking-[0.14em] text-muted-foreground">
          最近对话
        </span>
        {items.length > 0 && !trimmedQuery && (
          <span className="rounded-full bg-sidebar-accent/60 px-2 py-0.5 text-[10px] tabular-nums text-muted-foreground">
            {items.length}
          </span>
        )}
      </div>

      <div className="nc-scroll flex-1 overflow-y-auto px-2.5 pb-2">
        {isApiSearch ? (
          <SearchResultsView
            query={trimmedQuery}
            hits={searchHits}
            searching={searching}
            error={searchError}
            onNavigate={() => {
              setMenuFor(null)
              onNavigate?.()
            }}
          />
        ) : (
          <>
        {loading && (
          <p className="px-2 py-1 text-xs text-muted-foreground">加载中…</p>
        )}
        {!loading && filtered.length === 0 && (
          <p className="px-2 py-6 text-center text-xs text-muted-foreground">
            {!user
              ? "游客对话仅保留在当前页面"
              : items.length === 0
                ? "还没有会话"
                : "没有匹配结果"}
          </p>
        )}
        {error && (
          <p className="mx-2 my-1 rounded-md border border-destructive/40 bg-destructive/10 px-2 py-1 text-xs text-destructive">
            {error}
          </p>
        )}
        <ul className="flex flex-col gap-1">
          {filtered.map((c) => {
            const active =
              activeAgent
                ? c.kind === "agent" && activeId === c.id
                : c.kind === "chat" && activeId === c.id
            const itemKey = `${c.kind}-${c.id}`
            return (
              <li key={itemKey} className="relative">
                <div
                  className={cn(
                    "group relative flex items-center rounded-xl border border-transparent transition-all",
                    active
                      ? "border-primary/10 bg-sidebar-accent/80 text-sidebar-accent-foreground shadow-sm"
                      : "hover:border-sidebar-border hover:bg-background/45"
                  )}
                >
                  {active && (
                    <span className="absolute left-1 top-1/2 h-5 w-0.5 -translate-y-1/2 rounded-full bg-sidebar-primary" />
                  )}
                  <Link
                    to={c.kind === "agent" ? `/t/${c.id}` : `/c/${c.id}`}
                    className="min-w-0 flex-1 px-3 py-2.5"
                    title={c.title}
                    onClick={() => {
                      setMenuFor(null)
                      onNavigate?.()
                    }}
                  >
                    <div className="flex items-center gap-1.5 truncate text-[13px] font-medium">
                      {c.kind === "agent" &&
                        (c.target === "cloud" ? (
                          <Cloud className="size-3.5 shrink-0 text-primary" />
                        ) : (
                          <Laptop className="size-3.5 shrink-0 text-primary" />
                        ))}
                      <span className="truncate">{c.title}</span>
                    </div>
                    <div className="mt-1 truncate text-[10px] text-muted-foreground">
                      {relativeTime(c.updated_at)}
                    </div>
                  </Link>
                  <button
                    type="button"
                    className="mr-1 rounded p-1 text-muted-foreground opacity-0 transition-opacity hover:bg-background/60 hover:text-foreground group-hover:opacity-100 data-[open=true]:opacity-100"
                    data-open={menuFor === itemKey}
                    onClick={(e) => {
                      e.stopPropagation()
                      setMenuFor(menuFor === itemKey ? null : itemKey)
                    }}
                    aria-label="菜单"
                  >
                    <MoreHorizontal className="size-4" />
                  </button>
                </div>
                {menuFor === itemKey && (
                  <div
                    className="absolute right-1 top-full z-10 mt-0.5 flex min-w-36 flex-col rounded-md border border-border bg-popover p-1 text-sm shadow-panel"
                    onMouseLeave={() => setMenuFor(null)}
                  >
                    <button
                      className="flex items-center gap-2 rounded px-2 py-1 text-left hover:bg-accent"
                      onClick={() => {
                        setMenuFor(null)
                        void rename(c)
                      }}
                    >
                      <Pencil className="size-3.5" /> 重命名
                    </button>
                    <button
                      className="flex items-center gap-2 rounded px-2 py-1 text-left text-destructive hover:bg-destructive/10"
                      onClick={() => {
                        setMenuFor(null)
                        void remove(c)
                      }}
                    >
                      <Trash2 className="size-3.5" /> 删除
                    </button>
                  </div>
                )}
              </li>
            )
          })}
        </ul>
        {!loading && items.some((x) => x.kind === "chat") && !trimmedQuery && (
          <button
            type="button"
            onClick={() => void removeAll()}
            className="mx-2 mt-2 flex w-[calc(100%-1rem)] items-center justify-center gap-1.5 rounded-md px-2 py-1.5 text-xs text-muted-foreground transition-colors hover:bg-destructive/10 hover:text-destructive"
            title="删除全部会话"
          >
            <Trash2 className="size-3.5" /> 清空全部会话
          </button>
        )}
          </>
        )}
      </div>

      <div className="flex flex-col gap-1 border-t border-sidebar-border bg-background/20 px-2.5 py-2.5">
        <Link
          to="/library"
          onClick={() => onNavigate?.()}
          className="flex items-center gap-2.5 rounded-lg px-2.5 py-2 text-sm text-muted-foreground transition-colors hover:bg-sidebar-accent/60 hover:text-sidebar-accent-foreground"
          title="管理并浏览公开分享的图片、视频和音频"
        >
          <Library className="size-4" /> 素材库
        </Link>
        {user?.is_admin && (
          <Link
            to="/admin"
            onClick={() => onNavigate?.()}
            className="flex items-center gap-2.5 rounded-lg px-2.5 py-2 text-sm text-muted-foreground transition-colors hover:bg-sidebar-accent/60 hover:text-sidebar-accent-foreground"
            title="进入管理控制台"
          >
            <Shield className="size-4" /> 管理控制台
          </Link>
        )}
        {user && onOpenLibrary && (
          <button
            type="button"
            onClick={() => {
              onOpenLibrary()
              onNavigate?.()
            }}
            className="flex items-center gap-2.5 rounded-lg px-2.5 py-2 text-sm text-muted-foreground transition-colors hover:bg-sidebar-accent/60 hover:text-sidebar-accent-foreground"
          >
            <BookMarked className="size-4" /> 提示词库
          </button>
        )}
        {user ? (
          <div className="mt-1 flex items-center justify-between gap-2 rounded-xl border border-sidebar-border bg-background/45 p-1.5 shadow-sm">
            <button
              type="button"
              onClick={() => setProfileOpen(true)}
              className="flex min-w-0 flex-1 items-center gap-2.5 rounded-lg px-1 py-0.5 text-left transition-colors hover:bg-sidebar-accent/70"
              title="个人资料"
            >
              {user.avatar_url && !avatarBroken ? (
                <img
                  src={user.avatar_url}
                  alt=""
                  className="size-7 shrink-0 rounded-full border border-border object-cover"
                  onError={() => setAvatarBroken(true)}
                />
              ) : (
                <div className="grid size-7 shrink-0 place-items-center rounded-full bg-gradient-to-br from-primary to-chart-5 text-[11px] font-semibold text-primary-foreground">
                  {((user.display_name?.trim() || user.username)
                    .slice(0, 1)).toUpperCase()}
                </div>
              )}
              <span className="truncate text-xs">
                {user.display_name?.trim() || user.username}
              </span>
            </button>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => {
                void auth.logout().then(() => {
                  nav("/")
                  onNavigate?.()
                })
              }}
              title="退出登录"
            >
              <LogOut className="size-4" />
            </Button>
          </div>
        ) : (
          <Button asChild className="mt-1 w-full justify-start gap-2.5">
            <Link to="/login?next=/" onClick={() => onNavigate?.()}>
              <LogIn className="size-4" /> 登录使用云端模型
            </Link>
          </Button>
        )}
      </div>

      {user && (
        <ProfileDialog open={profileOpen} onClose={() => setProfileOpen(false)} />
      )}
    </aside>
  )
}

function highlightTerm(text: string, term: string): ReactNode[] {
  if (!term) return [text]
  const escaped = term.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")
  const re = new RegExp(`(${escaped})`, "ig")
  const parts = text.split(re)
  return parts.map((p, i) =>
    i % 2 === 1 ? (
      <mark
        key={i}
        className="rounded-sm bg-yellow-300/60 px-0.5 text-foreground dark:bg-yellow-400/40"
      >
        {p}
      </mark>
    ) : (
      <span key={i}>{p}</span>
    )
  )
}

function SearchResultsView({
  query,
  hits,
  searching,
  error,
  onNavigate,
}: {
  query: string
  hits: SearchHit[] | null
  searching: boolean
  error: string | null
  onNavigate: () => void
}) {
  if (error) {
    return (
      <p className="mx-2 my-2 rounded-md border border-destructive/40 bg-destructive/10 px-2 py-1.5 text-xs text-destructive">
        搜索失败：{error}
      </p>
    )
  }
  if (hits == null) {
    return (
      <p className="px-2 py-3 text-xs text-muted-foreground">
        {searching ? "搜索中…" : "输入关键词搜索"}
      </p>
    )
  }
  if (hits.length === 0) {
    return (
      <p className="px-2 py-6 text-center text-xs text-muted-foreground">
        {searching ? "搜索中…" : "没有匹配结果"}
      </p>
    )
  }
  return (
    <>
      {searching && (
        <p className="px-2 py-1 text-[11px] text-muted-foreground">搜索中…</p>
      )}
      <ul className="flex flex-col gap-0.5">
        {hits.map((h, idx) => {
          const target =
            h.kind === "content" && h.message_id != null
              ? `/c/${h.conversation_id}?msg=${h.message_id}`
              : `/c/${h.conversation_id}`
          const key = `${h.kind}-${h.conversation_id}-${h.message_id ?? "t"}-${idx}`
          const Icon =
            h.kind === "title"
              ? MessageSquareText
              : h.role === "user"
                ? User
                : Sparkles
          return (
            <li key={key}>
              <Link
                to={target}
                onClick={onNavigate}
                className="flex flex-col gap-1 rounded-lg px-3 py-2 transition-colors hover:bg-sidebar-accent/60"
                title={h.conversation_title}
              >
                <div className="flex items-center gap-1.5 text-[11px] text-muted-foreground">
                  <Icon className="size-3 shrink-0" />
                  <span className="truncate">
                    {highlightTerm(h.conversation_title, query)}
                  </span>
                </div>
                {h.kind === "content" && (
                  <div className="line-clamp-2 text-xs leading-snug text-foreground/90">
                    {highlightTerm(h.snippet, query)}
                  </div>
                )}
              </Link>
            </li>
          )
        })}
      </ul>
    </>
  )
}
