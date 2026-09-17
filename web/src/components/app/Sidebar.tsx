import { useEffect, useMemo, useState, type ReactNode } from "react"
import { Link, useLocation, useNavigate, useParams } from "react-router-dom"
import {
  BookMarked,
  ChevronDown,
  Clapperboard,
  Cloud,
  Download,
  ImageIcon,
  Laptop,
  Library,
  LogIn,
  LogOut,
  MessageSquareText,
  MoreHorizontal,
  Pencil,
  Scissors,
  Search,
  Shield,
  SquarePen,
  Sparkles,
  Trash2,
  User,
  Workflow,
  X,
} from "lucide-react"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { conversationsApi, type Conversation } from "@/lib/conversations"
import { searchApi, type SearchHit } from "@/lib/search"
import { agentApi, type AgentSession, type AgentTarget } from "@/lib/agent"
import { cn } from "@/lib/utils"
import { useAuth } from "@/lib/auth-context"
import { prefetchWorkMode } from "@/lib/mode"
import { capabilities } from "@/lib/platform"
import { useConfirm } from "@/lib/confirm-context"
import { useIsDesktop } from "@/lib/use-media-query"
import { useSidebarCollapsed } from "@/lib/sidebar-collapse"
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

/** The studios, reachable from the "更多" group rather than the top level. */
const TOOLS: Array<{ to: string; label: string; icon: typeof ImageIcon; hint: string }> = [
  { to: "/studio", label: "图像工作室", icon: ImageIcon, hint: "多轮对话式生图（Responses API）" },
  { to: "/videos", label: "视频工作室", icon: Clapperboard, hint: "文生视频 / 图生视频" },
  { to: "/workflows", label: "流水线", icon: Workflow, hint: "图片 / 多视频 / 裁剪 / 合并节点画布" },
  { to: "/editor", label: "在线剪辑", icon: Scissors, hint: "多轨精细剪辑、素材库与服务端导出" },
  { to: "/library", label: "素材库", icon: Library, hint: "管理并浏览公开分享的图片、视频和音频" },
]

/**
 * A single navigation row.
 *
 * One component for both widths: in the rail only the icon survives, but the
 * row keeps its `title`, so a collapsed sidebar is still navigable without
 * guessing what a bare glyph means.
 */
function NavRow({
  icon: Icon,
  label,
  collapsed,
  active,
  hint,
  trailing,
  ...rest
}: {
  icon: typeof ImageIcon
  label: string
  collapsed: boolean
  active?: boolean
  hint?: string
  trailing?: ReactNode
} & (
  | { to: string; onClick?: () => void; onPointerEnter?: () => void; onFocus?: () => void }
  | { onClick: () => void; to?: undefined }
)) {
  const className = cn(
    "group/nav flex items-center rounded-lg text-sm transition-colors",
    collapsed ? "h-9 w-9 justify-center" : "h-9 w-full gap-2.5 px-2.5",
    active
      ? "bg-sidebar-accent/80 font-medium text-sidebar-accent-foreground"
      : "text-sidebar-foreground/85 hover:bg-sidebar-accent/55 hover:text-sidebar-accent-foreground"
  )
  const body = (
    <>
      <Icon className="size-4 shrink-0" />
      {!collapsed && <span className="min-w-0 flex-1 truncate text-left">{label}</span>}
      {!collapsed && trailing}
    </>
  )
  if ("to" in rest && rest.to) {
    const { to, ...linkProps } = rest
    return (
      <Link to={to} title={hint ?? label} className={className} viewTransition {...linkProps}>
        {body}
      </Link>
    )
  }
  const { onClick } = rest as { onClick: () => void }
  return (
    <button type="button" onClick={onClick} title={hint ?? label} className={className}>
      {body}
    </button>
  )
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
  const [searchOpen, setSearchOpen] = useState(false)
  const [toolsOpen, setToolsOpen] = useState(() =>
    TOOLS.some((t) => location.pathname.startsWith(t.to))
  )
  const [profileOpen, setProfileOpen] = useState(false)
  const [avatarBroken, setAvatarBroken] = useState(false)
  const [searchHits, setSearchHits] = useState<SearchHit[] | null>(null)
  const [searching, setSearching] = useState(false)
  const [searchError, setSearchError] = useState<string | null>(null)
  // Collapsing is driven from the page header, the way Doubao keeps the panel
  // toggle in the working area rather than inside the panel it hides.
  const [collapsedPref, toggleCollapsed] = useSidebarCollapsed()

  // The rail only exists on desktop: the mobile drawer is already an overlay,
  // and a 4rem strip of icons inside it would be a worse version of nothing.
  const isDesktop = useIsDesktop()
  const collapsed = isDesktop && collapsedPref

  // Whether to offer the client at all. Read once per mount because the host
  // cannot change under a running page.
  const canInstallDesktop = useMemo(() => capabilities().canInstallDesktop, [])

  useEffect(() => {
    setAvatarBroken(false)
  }, [user?.avatar_url])

  // Collapsing hides the input, so the filter must stop applying with it:
  // leaving a query attached to an invisible search box makes the recent list
  // look truncated. Derived rather than cleared in an effect so the rail never
  // renders one frame of filtered results first.
  const trimmedQuery = collapsed ? "" : query.trim()

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
      title: `删除${c.kind === "agent" ? "工作任务" : "会话"} "${c.title}"？`,
      description:
        c.kind === "agent"
          ? "将停止其运行时并删除任务记录与工作目录，此操作不可撤销。"
          : "此操作不可撤销。",
      confirmText: "删除",
      destructive: true,
    })
    if (!ok) return
    try {
      if (c.kind === "agent") {
        // Deleting releases the runtime and its container as well as the
        // transcript; `stop` is the operation that keeps the history.
        await agentApi.remove(c.id)
      } else {
        await conversationsApi.remove(c.id)
      }
      setItems((s) => s.filter((x) => !(x.kind === c.kind && x.id === c.id)))
      // Leave the page whose session just disappeared, for either kind.
      if (activeAgent === (c.kind === "agent") && activeId === c.id) nav("/")
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

  const onChatRoute = location.pathname === "/" || location.pathname.startsWith("/c/")
  const onWorkRoute = location.pathname.startsWith("/t")

  return (
    // Width is the sidebar's own business — both the drawer and the desktop
    // column follow it — so collapsing is a single class swap here rather than
    // a prop every page has to thread through.
    <aside
      data-collapsed={collapsed}
      className={cn(
        "flex h-full shrink-0 flex-col border-r border-sidebar-border bg-sidebar/95 text-sidebar-foreground backdrop-blur-xl transition-[width] duration-200",
        collapsed ? "w-[4.25rem] items-center px-2" : "w-[16rem] px-3"
      )}
    >
      {/* Brand only. The collapse control lives in the page header, so the
          sidebar's own top row is free to be what Doubao's is: the product
          name and nothing else. */}
      <div
        className={cn(
          "flex min-h-14 items-center",
          collapsed ? "justify-center" : "px-1"
        )}
      >
        {collapsed ? (
          <img
            src="/logo.svg"
            alt="Yunova"
            title="Yunova"
            className="size-7 rounded-lg ring-1 ring-white/15"
          />
        ) : (
          <BrandMark size="sm" />
        )}
      </div>

      <nav className="flex flex-col gap-0.5 pb-1">
        <NavRow
          icon={SquarePen}
          label="新对话"
          collapsed={collapsed}
          active={onChatRoute}
          onClick={() => void createNew()}
        />
        {/* Work mode is a peer of chat, not a setting inside it: it starts an
            agent that can run commands, so it gets its own entry point. */}
        <NavRow
          icon={Cloud}
          label="新工作任务"
          hint="启动可执行命令的 Agent 任务"
          collapsed={collapsed}
          active={onWorkRoute}
          to="/t"
          onClick={() => onNavigate?.()}
          // Same warming as the in-page switch, so whichever entry point the
          // user takes, work mode does not open on a loading screen.
          onPointerEnter={prefetchWorkMode}
          onFocus={prefetchWorkMode}
        />

        {collapsed ? (
          // In the rail the group cannot expand (there is nowhere to put the
          // labels), so each tool keeps its own icon row.
          TOOLS.map((t) => (
            <NavRow
              key={t.to}
              icon={t.icon}
              label={t.label}
              hint={t.hint}
              collapsed
              active={location.pathname.startsWith(t.to)}
              to={t.to}
              onClick={() => onNavigate?.()}
            />
          ))
        ) : (
          <>
            <NavRow
              icon={Sparkles}
              label="更多"
              hint="图像、视频、流水线、剪辑与素材库"
              collapsed={false}
              onClick={() => setToolsOpen((v) => !v)}
              trailing={
                <ChevronDown
                  className={cn(
                    "size-3.5 shrink-0 text-muted-foreground transition-transform",
                    toolsOpen && "rotate-180"
                  )}
                />
              }
            />
            {toolsOpen && (
              <div className="ml-3 flex flex-col gap-0.5 border-l border-sidebar-border/70 pl-2">
                {TOOLS.map((t) => (
                  <NavRow
                    key={t.to}
                    icon={t.icon}
                    label={t.label}
                    hint={t.hint}
                    collapsed={false}
                    active={location.pathname.startsWith(t.to)}
                    to={t.to}
                    onClick={() => onNavigate?.()}
                  />
                ))}
                {user && onOpenLibrary && (
                  <NavRow
                    icon={BookMarked}
                    label="提示词库"
                    collapsed={false}
                    onClick={() => {
                      onOpenLibrary()
                      onNavigate?.()
                    }}
                  />
                )}
                {user?.is_admin && (
                  <NavRow
                    icon={Shield}
                    label="管理控制台"
                    collapsed={false}
                    active={location.pathname.startsWith("/admin")}
                    to="/admin"
                    onClick={() => onNavigate?.()}
                  />
                )}
              </div>
            )}
          </>
        )}
      </nav>

      {collapsed ? (
        // No input fits in the rail, so the icon expands the sidebar and hands
        // focus straight to the search field — one click, not two.
        <NavRow
          icon={Search}
          label="搜索会话"
          collapsed
          onClick={() => {
            setSearchOpen(true)
            toggleCollapsed()
          }}
        />
      ) : (
        <div className="flex items-center justify-between gap-2 px-1 pb-1 pt-2">
          {searchOpen ? (
            <div className="relative w-full">
              <Search className="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
              <Input
                autoFocus
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder={user ? "搜索会话…" : "登录后查看历史会话"}
                disabled={!user}
                className="h-8 rounded-lg border-sidebar-border bg-background/45 pl-8 pr-7 text-xs shadow-none"
              />
              <button
                type="button"
                onClick={() => {
                  setSearchOpen(false)
                  setQuery("")
                }}
                aria-label="关闭搜索"
                className="absolute right-1.5 top-1/2 -translate-y-1/2 rounded p-0.5 text-muted-foreground hover:text-foreground"
              >
                <X className="size-3.5" />
              </button>
            </div>
          ) : (
            <>
              <span className="text-[11px] font-medium tracking-wide text-muted-foreground">
                最近
              </span>
              <button
                type="button"
                onClick={() => setSearchOpen(true)}
                title="搜索会话"
                aria-label="搜索会话"
                className="rounded-md p-1 text-muted-foreground transition-colors hover:bg-sidebar-accent/60 hover:text-sidebar-accent-foreground"
              >
                <Search className="size-3.5" />
              </button>
            </>
          )}
        </div>
      )}

      {/* The recent list is what the sidebar is for, so it owns the leftover
          height; everything above it is fixed. */}
      <div
        className={cn(
          "nc-scroll flex-1 overflow-y-auto pb-2",
          collapsed && "w-full"
        )}
      >
        {collapsed ? null : isApiSearch ? (
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
              <p className="mx-1 my-1 rounded-md border border-destructive/40 bg-destructive/10 px-2 py-1 text-xs text-destructive">
                {error}
              </p>
            )}
            <ul className="flex flex-col gap-0.5">
              {filtered.map((c) => {
                const active = activeAgent
                  ? c.kind === "agent" && activeId === c.id
                  : c.kind === "chat" && activeId === c.id
                const itemKey = `${c.kind}-${c.id}`
                return (
                  <li key={itemKey} className="relative">
                    <div
                      className={cn(
                        "group relative flex items-center rounded-lg transition-colors",
                        active
                          ? "bg-sidebar-accent/80 text-sidebar-accent-foreground"
                          : "hover:bg-sidebar-accent/55"
                      )}
                    >
                      <Link
                        to={c.kind === "agent" ? `/t/${c.id}` : `/c/${c.id}`}
                        className="min-w-0 flex-1 px-2.5 py-2"
                        title={`${c.title}　·　${relativeTime(c.updated_at)}`}
                        onClick={() => {
                          setMenuFor(null)
                          onNavigate?.()
                        }}
                      >
                        {/* One line per session, Doubao-style: the timestamp
                            moved into the tooltip so twice as many titles fit
                            without scrolling. */}
                        <div className="flex items-center gap-1.5 truncate text-[13px]">
                          {c.kind === "agent" &&
                            (c.target === "cloud" ? (
                              <Cloud className="size-3.5 shrink-0 text-primary" />
                            ) : (
                              <Laptop className="size-3.5 shrink-0 text-primary" />
                            ))}
                          <span className="truncate">{c.title}</span>
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
                className="mt-2 flex w-full items-center justify-center gap-1.5 rounded-md px-2 py-1.5 text-xs text-muted-foreground transition-colors hover:bg-destructive/10 hover:text-destructive"
                title="删除全部会话"
              >
                <Trash2 className="size-3.5" /> 清空全部会话
              </button>
            )}
          </>
        )}
      </div>

      <div
        className={cn(
          "flex flex-col gap-1 border-t border-sidebar-border py-2",
          collapsed && "w-full items-center"
        )}
      >
        {/* Downloads live at the bottom of the sidebar, the way Doubao keeps
            "下载电脑版" out of the working area: it is a one-time action, not
            something the user returns to mid-task. Inside the desktop client
            it is not an action at all — the app is already installed — so the
            row is dropped rather than shown pointing at itself. */}
        {canInstallDesktop && (
          <NavRow
            icon={Download}
            label="下载电脑版"
            hint="安装桌面应用，并把任务跑在自己的电脑上"
            collapsed={collapsed}
            active={location.pathname.startsWith("/download")}
            to="/download"
            onClick={() => onNavigate?.()}
          />
        )}
        {user ? (
          collapsed ? (
            <button
              type="button"
              onClick={() => setProfileOpen(true)}
              title={user.display_name?.trim() || user.username}
              className="grid size-9 place-items-center rounded-lg transition-colors hover:bg-sidebar-accent/60"
            >
              <Avatar user={user} broken={avatarBroken} onBroken={() => setAvatarBroken(true)} />
            </button>
          ) : (
            <div className="flex items-center gap-1">
              <button
                type="button"
                onClick={() => setProfileOpen(true)}
                className="flex min-w-0 flex-1 items-center gap-2.5 rounded-lg px-2 py-1.5 text-left transition-colors hover:bg-sidebar-accent/60"
                title="个人资料"
              >
                <Avatar user={user} broken={avatarBroken} onBroken={() => setAvatarBroken(true)} />
                <span className="truncate text-xs">
                  {user.display_name?.trim() || user.username}
                </span>
              </button>
              <Button
                variant="ghost"
                size="icon-sm"
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
          )
        ) : (
          <NavRow
            icon={LogIn}
            label="登录"
            hint="登录使用云端模型"
            collapsed={collapsed}
            to="/login?next=/"
            onClick={() => onNavigate?.()}
          />
        )}
      </div>

      {user && (
        <ProfileDialog open={profileOpen} onClose={() => setProfileOpen(false)} />
      )}
    </aside>
  )
}

function Avatar({
  user,
  broken,
  onBroken,
}: {
  user: { avatar_url?: string | null; display_name?: string | null; username: string }
  broken: boolean
  onBroken: () => void
}) {
  if (user.avatar_url && !broken) {
    return (
      <img
        src={user.avatar_url}
        alt=""
        className="size-7 shrink-0 rounded-full border border-border object-cover"
        onError={onBroken}
      />
    )
  }
  return (
    <div className="grid size-7 shrink-0 place-items-center rounded-full bg-gradient-to-br from-primary to-chart-5 text-[11px] font-semibold text-primary-foreground">
      {(user.display_name?.trim() || user.username).slice(0, 1).toUpperCase()}
    </div>
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
      <p className="mx-1 my-2 rounded-md border border-destructive/40 bg-destructive/10 px-2 py-1.5 text-xs text-destructive">
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
                className="flex flex-col gap-1 rounded-lg px-2.5 py-2 transition-colors hover:bg-sidebar-accent/60"
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
