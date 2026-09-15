import { useEffect, useState } from "react"
import { Link, Navigate } from "react-router-dom"
import {
  ArrowLeft,
  BarChart3,
  Cloud,
  Coins,
  CreditCard,
  Database,
  Key,
  Mail,
  Menu,
  RefreshCw,
  Route,
  Send,
  Server,
  Shield,
  Tag,
  Ticket,
  Trash2,
  UserCog,
  Users,
} from "lucide-react"
import { cn } from "@/lib/utils"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table"

// Radix 的 SelectItem 不接受空字符串作为 value，用哨兵值代表「不过滤」。
const STATUS_ALL = "__all__"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { useAuth } from "@/lib/auth-context"
import { useConfirm } from "@/lib/confirm-context"
import {
  adminApi,
  type AdminInviteRow,
  type AdminStats,
  type AdminSystemInfo,
  type AdminUser,
} from "@/lib/admin"
import {
  adminQuotaApi,
  type AdminSettings,
  type AdminSettingsUpdate,
  type AdminUserQuota,
} from "@/lib/quota"
import { ChannelsPanel } from "./admin/ChannelsPanel"
import { PricingPanel } from "./admin/PricingPanel"
import { StoragePanel } from "./admin/StoragePanel"
import { StatsView } from "@/components/app/StatsView"
import {
  adminPaymentsApi,
  type AdminOrder,
  type AdminPaymentConfig,
  type AdminPaymentConfigUpdate,
} from "@/lib/payments"

type Section =
  | "overview"
  | "users"
  | "quota"
  | "payments"
  | "invites"
  | "channels"
  | "pricing"
  | "shared"
  | "email"
  | "storage"
  | "system"

const NAV: { key: Section; label: string; icon: React.ComponentType<{ className?: string }> }[] = [
  { key: "overview", label: "概览", icon: BarChart3 },
  { key: "users", label: "用户管理", icon: Users },
  { key: "quota", label: "额度", icon: Coins },
  { key: "payments", label: "支付 / 充值", icon: CreditCard },
  { key: "invites", label: "邀请", icon: Ticket },
  { key: "channels", label: "上游渠道", icon: Route },
  { key: "pricing", label: "模型计费", icon: Tag },
  { key: "email", label: "邮箱 / SMTP", icon: Mail },
  { key: "storage", label: "媒体存储", icon: Cloud },
  { key: "system", label: "系统信息", icon: Server },
]

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} GB`
}

export default function AdminPage() {
  const { state } = useAuth()
  const currentUser = state.status === "authed" ? state.user : null
  const [section, setSection] = useState<Section>("overview")
  const [mobileNavOpen, setMobileNavOpen] = useState(false)

  if (state.status === "loading") {
    return (
      <div className="grid min-h-svh place-items-center text-muted-foreground">
        加载中…
      </div>
    )
  }
  if (!currentUser || !currentUser.is_admin) {
    return <Navigate to="/" replace />
  }

  return (
    <div className="flex h-svh bg-background text-foreground">
      {mobileNavOpen && (
        <button
          type="button"
          aria-label="关闭侧栏"
          onClick={() => setMobileNavOpen(false)}
          className="fixed inset-0 z-30 bg-black/50 md:hidden"
        />
      )}
      <aside
        className={cn(
          "fixed inset-y-0 left-0 z-40 flex h-full w-60 max-w-[85vw] flex-col border-r border-sidebar-border bg-sidebar transition-transform",
          mobileNavOpen ? "translate-x-0 shadow-2xl" : "-translate-x-full",
          "md:static md:w-60 md:max-w-none md:shrink-0 md:translate-x-0 md:shadow-none"
        )}
      >
        <div className="flex items-center gap-2 px-4 pb-3 pt-4">
          <div className="grid size-8 place-items-center rounded-md bg-gradient-to-br from-primary to-chart-5 text-primary-foreground">
            <Shield className="size-4" />
          </div>
          <div className="min-w-0">
            <p className="text-sm font-semibold">管理控制台</p>
            <p className="truncate text-[11px] text-muted-foreground">
              {currentUser.display_name || currentUser.username}
            </p>
          </div>
        </div>

        <nav className="flex flex-col gap-0.5 px-2 py-2">
          {NAV.map((n) => {
            const active = section === n.key
            const Icon = n.icon
            return (
              <button
                key={n.key}
                type="button"
                onClick={() => {
                  setSection(n.key)
                  setMobileNavOpen(false)
                }}
                className={
                  "flex items-center gap-2 rounded-md px-3 py-2 text-left text-sm transition-colors " +
                  (active
                    ? "bg-sidebar-accent text-sidebar-accent-foreground"
                    : "text-muted-foreground hover:bg-sidebar-accent/50 hover:text-sidebar-accent-foreground")
                }
              >
                <Icon className="size-4" />
                {n.label}
              </button>
            )
          })}
        </nav>

        <div className="mt-auto border-t border-sidebar-border px-2 py-2">
          <Link
            to="/"
            className="flex items-center gap-2 rounded-md px-3 py-2 text-sm text-muted-foreground transition-colors hover:bg-sidebar-accent/60 hover:text-sidebar-accent-foreground"
          >
            <ArrowLeft className="size-4" /> 返回聊天
          </Link>
        </div>
      </aside>

      <div className="nc-scroll flex min-w-0 flex-1 flex-col overflow-y-auto">
        <header className="flex items-center justify-between gap-2 border-b border-border bg-background/70 px-3 py-3 backdrop-blur md:px-6 md:py-4">
          <div className="flex min-w-0 items-center gap-2">
            <Button
              variant="ghost"
              size="icon"
              className="md:hidden"
              onClick={() => setMobileNavOpen(true)}
              aria-label="打开侧栏"
            >
              <Menu />
            </Button>
            <div className="min-w-0">
              <h1 className="truncate text-lg font-semibold tracking-tight md:text-xl">
                {NAV.find((n) => n.key === section)?.label}
              </h1>
              <p className="hidden text-sm text-muted-foreground sm:block">
                仅管理员可见。对系统全局生效。
              </p>
            </div>
          </div>
        </header>

        <main className="mx-auto w-full max-w-5xl px-3 py-4 md:px-6 md:py-6">
          {section === "overview" && <OverviewPanel />}
          {section === "users" && <UsersPanel currentUserId={currentUser.id} />}
          {section === "quota" && <QuotaPanel />}
          {section === "payments" && <PaymentsPanel />}
          {section === "invites" && <InvitesPanel />}
          {section === "channels" && <ChannelsPanel />}
          {section === "pricing" && <PricingPanel />}
          {section === "email" && <EmailPanel />}
          {section === "storage" && <StoragePanel />}
          {section === "system" && <SystemPanel />}
        </main>
      </div>
    </div>
  )
}

function StatCard({
  label,
  value,
  hint,
}: {
  label: string
  value: string | number
  hint?: string
}) {
  return (
    <div className="rounded-xl border border-border bg-card p-4 shadow-sm">
      <p className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
        {label}
      </p>
      <p className="mt-1 text-2xl font-semibold tabular-nums">{value}</p>
      {hint && <p className="mt-1 text-xs text-muted-foreground">{hint}</p>}
    </div>
  )
}

function OverviewPanel() {
  const [stats, setStats] = useState<AdminStats | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)

  async function load() {
    setLoading(true)
    setError(null)
    try {
      setStats(await adminApi.stats())
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }
  useEffect(() => {
    void load()
  }, [])

  return (
    <div className="flex flex-col gap-4">
      <div className="flex items-center justify-between">
        <p className="text-sm text-muted-foreground">
          系统整体使用情况。数据实时来自数据库。
        </p>
        <Button variant="outline" size="sm" onClick={() => void load()}>
          <RefreshCw /> 刷新
        </Button>
      </div>

      {error && (
        <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
          {error}
        </div>
      )}
      {loading && !stats && (
        <p className="text-sm text-muted-foreground">加载中…</p>
      )}
      {stats && (
        <>
          <div className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4">
            <StatCard
              label="用户"
              value={stats.users}
              hint={`其中管理员 ${stats.admins}`}
            />
            <StatCard label="会话" value={stats.conversations} />
            <StatCard label="消息" value={stats.messages} />
            <StatCard label="活跃会话凭证" value={stats.sessions} />
            <StatCard
              label="Skills"
              value={stats.skills}
              hint={`公开 ${stats.public_skills}`}
            />
            <StatCard
              label="提示词"
              value={stats.prompts}
              hint={`公开 ${stats.public_prompts}`}
            />
            <StatCard label="素材库" value={stats.library_assets} />
          </div>
        </>
      )}
    </div>
  )
}

function UsersPanel({ currentUserId }: { currentUserId: number }) {
  const { confirm } = useConfirm()
  const [users, setUsers] = useState<AdminUser[]>([])
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [query, setQuery] = useState("")
  const [editing, setEditing] = useState<AdminUser | null>(null)

  async function load() {
    setLoading(true)
    setError(null)
    try {
      setUsers(await adminApi.listUsers())
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }
  useEffect(() => {
    void load()
  }, [])

  const q = query.trim().toLowerCase()
  const filtered = q
    ? users.filter(
        (u) =>
          u.username.toLowerCase().includes(q) ||
          (u.display_name ?? "").toLowerCase().includes(q)
      )
    : users

  async function toggleAdmin(u: AdminUser) {
    const next = !u.is_admin
    const verb = next ? "提升为管理员" : "撤销管理员"
    const ok = await confirm({
      title: `确认${verb} ${u.username}？`,
      confirmText: next ? "提升" : "撤销",
    })
    if (!ok) return
    try {
      await adminApi.updateUser(u.id, { is_admin: next })
      await load()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }

  async function removeUser(u: AdminUser) {
    const ok = await confirm({
      title: `删除用户 "${u.username}"？`,
      description: "其全部会话、消息、Skills 都会一并删除，此操作不可撤销。",
      confirmText: "删除",
      destructive: true,
    })
    if (!ok) return
    try {
      await adminApi.deleteUser(u.id)
      await load()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }

  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-center gap-2">
        <Input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="搜索用户名或昵称…"
          className="max-w-xs"
        />
        <Button variant="outline" size="sm" onClick={() => void load()}>
          <RefreshCw /> 刷新
        </Button>
      </div>

      {error && (
        <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
          {error}
        </div>
      )}

      <div className="flex flex-col gap-2 md:hidden">
        {loading && (
          <p className="px-3 py-6 text-center text-sm text-muted-foreground">加载中…</p>
        )}
        {!loading && filtered.length === 0 && (
          <p className="px-3 py-6 text-center text-sm text-muted-foreground">
            没有匹配的用户
          </p>
        )}
        {filtered.map((u) => {
          const isSelf = u.id === currentUserId
          return (
            <div
              key={u.id}
              className="flex flex-col gap-2 rounded-lg border border-border bg-card p-3 shadow-sm"
            >
              <div className="flex items-start gap-2.5">
                {u.avatar_url ? (
                  <img
                    src={u.avatar_url}
                    alt=""
                    className="size-9 shrink-0 rounded-full border border-border object-cover"
                  />
                ) : (
                  <div className="grid size-9 shrink-0 place-items-center rounded-full bg-gradient-to-br from-primary to-chart-5 text-sm font-semibold text-primary-foreground">
                    {(u.display_name?.trim() || u.username).slice(0, 1).toUpperCase()}
                  </div>
                )}
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-1.5">
                    <p className="truncate text-sm font-medium">{u.username}</p>
                    <span className="shrink-0 text-[10px] tabular-nums text-muted-foreground">
                      #{u.id}
                    </span>
                    {isSelf && (
                      <span className="shrink-0 text-[10px] text-muted-foreground">
                        （你）
                      </span>
                    )}
                  </div>
                  {u.display_name && (
                    <p className="truncate text-xs text-muted-foreground">
                      {u.display_name}
                    </p>
                  )}
                  <div className="mt-1 flex flex-wrap items-center gap-1.5 text-xs">
                    {u.is_admin ? (
                      <span className="inline-flex items-center gap-1 rounded-full bg-primary/10 px-2 py-0.5 font-medium text-primary">
                        <Shield className="size-3" /> 管理员
                      </span>
                    ) : (
                      <span className="text-muted-foreground">普通用户</span>
                    )}
                    {u.email && (
                      <span className="truncate text-muted-foreground" title={u.email}>
                        {u.email}
                      </span>
                    )}
                  </div>
                </div>
              </div>
              <div className="grid grid-cols-3 gap-2 rounded-md bg-muted/40 px-2 py-1.5 text-xs">
                <div className="flex flex-col items-center">
                  <span className="text-muted-foreground">会话</span>
                  <span className="tabular-nums">{u.conversations}</span>
                </div>
                <div className="flex flex-col items-center">
                  <span className="text-muted-foreground">消息</span>
                  <span className="tabular-nums">{u.messages}</span>
                </div>
                <div className="flex flex-col items-center">
                  <span className="text-muted-foreground">Skills</span>
                  <span className="tabular-nums">{u.skills}</span>
                </div>
              </div>
              <div className="flex items-center justify-between gap-2">
                <span className="text-[10px] text-muted-foreground">
                  {u.created_at.replace("T", " ").replace("Z", "")}
                </span>
                <div className="flex gap-1">
                  <Button
                    size="xs"
                    variant="outline"
                    onClick={() => setEditing(u)}
                    title="编辑"
                  >
                    <UserCog /> 编辑
                  </Button>
                  <Button
                    size="xs"
                    variant="outline"
                    onClick={() => void toggleAdmin(u)}
                    disabled={isSelf && u.is_admin}
                    title={u.is_admin ? "撤销管理员" : "提升为管理员"}
                  >
                    <Shield />
                    {u.is_admin ? "撤销" : "提升"}
                  </Button>
                  <Button
                    size="xs"
                    variant="outline"
                    onClick={() => void removeUser(u)}
                    disabled={isSelf}
                    className="text-destructive hover:bg-destructive/10 hover:text-destructive"
                    title="删除用户"
                  >
                    <Trash2 />
                  </Button>
                </div>
              </div>
            </div>
          )
        })}
      </div>

      <div className="hidden rounded-lg border border-border md:block">
        {/* 表头/单元格的间距在父级统一设置，省得每个 Head/Cell 都写一遍 */}
        <Table className="min-w-[36rem]">
          <TableHeader className="bg-muted/40 text-xs uppercase [&_th]:h-9 [&_th]:px-3 [&_th]:text-muted-foreground">
            <TableRow>
              <TableHead>ID</TableHead>
              <TableHead>用户</TableHead>
              <TableHead>角色</TableHead>
              <TableHead>邮箱</TableHead>
              <TableHead className="text-right">会话</TableHead>
              <TableHead className="text-right">消息</TableHead>
              <TableHead className="text-right">Skills</TableHead>
              <TableHead>注册时间</TableHead>
              <TableHead className="text-right">操作</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody className="[&_td]:px-3 [&_td]:py-2">
            {loading && (
              <TableRow>
                <TableCell
                  colSpan={9}
                  className="py-6 text-center text-muted-foreground"
                >
                  加载中…
                </TableCell>
              </TableRow>
            )}
            {!loading && filtered.length === 0 && (
              <TableRow>
                <TableCell
                  colSpan={9}
                  className="py-6 text-center text-muted-foreground"
                >
                  没有匹配的用户
                </TableCell>
              </TableRow>
            )}
            {filtered.map((u) => {
              const isSelf = u.id === currentUserId
              return (
                <TableRow key={u.id}>
                  <TableCell className="tabular-nums text-muted-foreground">
                    {u.id}
                  </TableCell>
                  <TableCell>
                    <div className="flex items-center gap-2">
                      {u.avatar_url ? (
                        <img
                          src={u.avatar_url}
                          alt=""
                          className="size-6 rounded-full border border-border object-cover"
                        />
                      ) : (
                        <div className="grid size-6 place-items-center rounded-full bg-gradient-to-br from-primary to-chart-5 text-[10px] font-semibold text-primary-foreground">
                          {(u.display_name?.trim() || u.username)
                            .slice(0, 1)
                            .toUpperCase()}
                        </div>
                      )}
                      <div className="min-w-0">
                        <p className="truncate font-medium">{u.username}</p>
                        {u.display_name && (
                          <p className="truncate text-xs text-muted-foreground">
                            {u.display_name}
                          </p>
                        )}
                      </div>
                    </div>
                  </TableCell>
                  <TableCell>
                    {u.is_admin ? (
                      <span className="inline-flex items-center gap-1 rounded-full bg-primary/10 px-2 py-0.5 text-xs font-medium text-primary">
                        <Shield className="size-3" /> 管理员
                      </span>
                    ) : (
                      <span className="text-xs text-muted-foreground">
                        普通用户
                      </span>
                    )}
                    {isSelf && (
                      <span className="ml-1 text-[10px] text-muted-foreground">
                        （你）
                      </span>
                    )}
                  </TableCell>
                  <TableCell
                    className="max-w-[14rem] truncate text-xs text-muted-foreground"
                    title={u.email ?? ""}
                  >
                    {u.email || "—"}
                  </TableCell>
                  <TableCell className="text-right tabular-nums">
                    {u.conversations}
                  </TableCell>
                  <TableCell className="text-right tabular-nums">
                    {u.messages}
                  </TableCell>
                  <TableCell className="text-right tabular-nums">
                    {u.skills}
                  </TableCell>
                  <TableCell className="text-xs text-muted-foreground">
                    {u.created_at.replace("T", " ").replace("Z", "")}
                  </TableCell>
                  <TableCell>
                    <div className="flex justify-end gap-1">
                      <Button
                        size="xs"
                        variant="outline"
                        onClick={() => setEditing(u)}
                        title="编辑"
                      >
                        <UserCog /> 编辑
                      </Button>
                      <Button
                        size="xs"
                        variant="outline"
                        onClick={() => void toggleAdmin(u)}
                        disabled={isSelf && u.is_admin}
                        title={u.is_admin ? "撤销管理员" : "提升为管理员"}
                      >
                        <Shield />
                        {u.is_admin ? "撤销" : "提升"}
                      </Button>
                      <Button
                        size="xs"
                        variant="outline"
                        onClick={() => void removeUser(u)}
                        disabled={isSelf}
                        className="text-destructive hover:bg-destructive/10 hover:text-destructive"
                        title="删除用户"
                      >
                        <Trash2 />
                      </Button>
                    </div>
                  </TableCell>
                </TableRow>
              )
            })}
          </TableBody>
        </Table>
      </div>

      {editing && (
        <EditUserDialog
          user={editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null)
            void load()
          }}
        />
      )}
    </div>
  )
}

function EditUserDialog({
  user,
  onClose,
  onSaved,
}: {
  user: AdminUser
  onClose: () => void
  onSaved: () => void
}) {
  const [displayName, setDisplayName] = useState(user.display_name ?? "")
  const [password, setPassword] = useState("")
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)

  async function save() {
    setError(null)
    const updates: {
      display_name?: string
      password?: string
    } = {}
    if ((displayName.trim() || "") !== (user.display_name ?? "")) {
      updates.display_name = displayName
    }
    if (password) {
      if (password.length < 6 || password.length > 256) {
        setError("新密码需为 6-256 位")
        return
      }
      updates.password = password
    }
    if (Object.keys(updates).length === 0) {
      onClose()
      return
    }
    setSaving(true)
    try {
      await adminApi.updateUser(user.id, updates)
      onSaved()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setSaving(false)
    }
  }

  // 父级用 {editing && <EditUserDialog/>} 控制挂载，这里常开，关闭走 onClose
  return (
    <Dialog open onOpenChange={(next) => !next && onClose()}>
      <DialogContent className="flex flex-col gap-4 sm:max-w-md">
        <DialogHeader>
          <DialogTitle>编辑用户 · {user.username}</DialogTitle>
          <DialogDescription>
            修改昵称或重置密码。改密码会使该用户已有的登录全部失效。
          </DialogDescription>
        </DialogHeader>

        {error && (
          <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
            {error}
          </div>
        )}

        <div className="flex flex-col gap-1.5">
          <Label htmlFor="eu-name">昵称</Label>
          <Input
            id="eu-name"
            value={displayName}
            onChange={(e) => setDisplayName(e.target.value)}
            maxLength={64}
            placeholder="留空则显示用户名"
          />
        </div>

        <div className="flex flex-col gap-1.5">
          <Label htmlFor="eu-pw">重置密码</Label>
          <Input
            id="eu-pw"
            type="password"
            autoComplete="new-password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            placeholder="留空不修改（6-256 位）"
          />
        </div>

        <DialogFooter className="flex-row justify-end">
          <Button variant="outline" onClick={onClose} disabled={saving}>
            取消
          </Button>
          <Button onClick={() => void save()} disabled={saving}>
            {saving ? "保存中…" : "保存"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function SystemPanel() {
  const [info, setInfo] = useState<AdminSystemInfo | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [pruneMsg, setPruneMsg] = useState<string | null>(null)
  const [pruning, setPruning] = useState(false)

  async function load() {
    setLoading(true)
    setError(null)
    try {
      setInfo(await adminApi.system())
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }
  useEffect(() => {
    void load()
  }, [])

  async function prune() {
    setPruneMsg(null)
    setPruning(true)
    try {
      const r = await adminApi.pruneSessions()
      setPruneMsg(`已清理 ${r.removed} 条过期登录凭证`)
    } catch (e) {
      setPruneMsg(e instanceof Error ? e.message : String(e))
    } finally {
      setPruning(false)
    }
  }

  return (
    <div className="flex flex-col gap-4">
      {error && (
        <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
          {error}
        </div>
      )}

      {loading && !info && (
        <p className="text-sm text-muted-foreground">加载中…</p>
      )}
      {info && (
        <div className="rounded-xl border border-border bg-card p-4 shadow-sm">
          <h2 className="mb-3 flex items-center gap-2 text-sm font-semibold">
            <Database className="size-4" /> 运行信息
          </h2>
          <dl className="grid grid-cols-1 gap-x-6 gap-y-2 text-sm sm:grid-cols-2">
            <InfoRow label="Yunova 版本" value={info.version} />
            <InfoRow label="数据库类型" value={info.db_kind} />
            <InfoRow label="监听地址" value={info.bind_addr} />
            <InfoRow
              label="本地图片目录占用"
              value={formatBytes(info.images_dir_bytes)}
            />
            <InfoRow label="媒体存储" value={info.storage_backend.toUpperCase()} />
            <InfoRow label="媒体位置" value={info.storage_location} mono />
            <InfoRow label="数据目录" value={info.data_dir} mono />
            <InfoRow label="配置文件" value={info.config_path} mono />
          </dl>
        </div>
      )}

      <div className="rounded-xl border border-border bg-card p-4 shadow-sm">
        <h2 className="mb-3 flex items-center gap-2 text-sm font-semibold">
          <Key className="size-4" /> 登录会话
        </h2>
        <p className="mb-3 text-sm text-muted-foreground">
          清理数据库中已过期但尚未被删除的登录凭证。仅影响已过期的会话。
        </p>
        <div className="flex items-center gap-3">
          <Button
            variant="outline"
            size="sm"
            onClick={() => void prune()}
            disabled={pruning}
          >
            <Trash2 /> {pruning ? "清理中…" : "清理过期会话"}
          </Button>
          {pruneMsg && (
            <span className="text-xs text-muted-foreground">{pruneMsg}</span>
          )}
        </div>
      </div>
    </div>
  )
}

function InfoRow({
  label,
  value,
  mono,
}: {
  label: string
  value: string
  mono?: boolean
}) {
  return (
    <div className="flex flex-col">
      <dt className="text-xs text-muted-foreground">{label}</dt>
      <dd
        className={
          "truncate " + (mono ? "font-mono text-xs" : "text-sm font-medium")
        }
        title={value}
      >
        {value}
      </dd>
    </div>
  )
}

function QuotaPanel() {
  const [tab, setTab] = useState<"stats" | "users">("stats")
  const [rows, setRows] = useState<AdminUserQuota[]>([])
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [query, setQuery] = useState("")
  const [editing, setEditing] = useState<AdminUserQuota | null>(null)

  async function load() {
    setLoading(true)
    setError(null)
    try {
      setRows(await adminQuotaApi.listUserQuota())
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }
  useEffect(() => {
    void load()
  }, [])

  const q = query.trim().toLowerCase()
  const filtered = q
    ? rows.filter((r) => r.username.toLowerCase().includes(q))
    : rows

  return (
    <div className="flex flex-col gap-3">
      <p className="text-sm text-muted-foreground">
        用户未填写自己的 API Key 时消耗站点额度：对话按上游返回的真实 token 用量结算，生图按次、视频按秒。
        对话失败或上游未返回用量时不计费；生图、视频失败会自动退还。
      </p>

      <Tabs
        value={tab}
        onValueChange={(v) => setTab(v as "stats" | "users")}
        className="gap-3"
      >
        <TabsList className="self-start">
          <TabsTrigger value="stats">全站统计</TabsTrigger>
          <TabsTrigger value="users">用户余额</TabsTrigger>
        </TabsList>

        <TabsContent value="stats">
          <StatsView loader={adminQuotaApi.stats} showTopUsers active />
        </TabsContent>

        <TabsContent value="users" className="flex flex-col gap-3">
      <div className="flex items-center gap-2">
        <Input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="按用户名搜索…"
          className="max-w-xs"
        />
        <Button variant="outline" size="sm" onClick={() => void load()}>
          <RefreshCw /> 刷新
        </Button>
      </div>

      {error && (
        <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
          {error}
        </div>
      )}

      <div className="flex flex-col gap-2 md:hidden">
        {loading && (
          <p className="px-3 py-6 text-center text-sm text-muted-foreground">加载中…</p>
        )}
        {!loading && filtered.length === 0 && (
          <p className="px-3 py-6 text-center text-sm text-muted-foreground">
            没有匹配的用户
          </p>
        )}
        {filtered.map((r) => (
          <div
            key={r.user_id}
            className="flex items-center justify-between gap-2 rounded-lg border border-border bg-card p-3 shadow-sm"
          >
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-1.5">
                <p className="truncate text-sm font-medium">{r.username}</p>
                <span className="shrink-0 text-[10px] tabular-nums text-muted-foreground">
                  #{r.user_id}
                </span>
              </div>
              <div className="mt-0.5 flex gap-3 text-xs">
                <span>
                  <span className="text-muted-foreground">余额 </span>
                  <span className="tabular-nums font-medium">{r.balance}</span>
                </span>
                <span className="text-muted-foreground">
                  消耗 <span className="tabular-nums">{r.lifetime_used}</span>
                </span>
              </div>
            </div>
            <Button size="xs" variant="outline" onClick={() => setEditing(r)}>
              <Coins /> 调整
            </Button>
          </div>
        ))}
      </div>

      <div className="hidden rounded-lg border border-border md:block">
        {/* 表头/单元格的间距在父级统一设置，省得每个 Head/Cell 都写一遍 */}
        <Table className="min-w-[36rem]">
          <TableHeader className="bg-muted/40 text-xs uppercase [&_th]:h-9 [&_th]:px-3 [&_th]:text-muted-foreground">
            <TableRow>
              <TableHead>ID</TableHead>
              <TableHead>用户</TableHead>
              <TableHead className="text-right">余额</TableHead>
              <TableHead className="text-right">历史消耗</TableHead>
              <TableHead className="text-right">操作</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody className="[&_td]:px-3 [&_td]:py-2">
            {loading && (
              <TableRow>
                <TableCell colSpan={5} className="py-6 text-center text-muted-foreground">
                  加载中…
                </TableCell>
              </TableRow>
            )}
            {!loading && filtered.length === 0 && (
              <TableRow>
                <TableCell colSpan={5} className="py-6 text-center text-muted-foreground">
                  没有匹配的用户
                </TableCell>
              </TableRow>
            )}
            {filtered.map((r) => (
              <TableRow key={r.user_id}>
                <TableCell className="tabular-nums text-muted-foreground">{r.user_id}</TableCell>
                <TableCell className="font-medium">{r.username}</TableCell>
                <TableCell className="text-right tabular-nums">{r.balance}</TableCell>
                <TableCell className="text-right tabular-nums text-muted-foreground">
                  {r.lifetime_used}
                </TableCell>
                <TableCell>
                  <div className="flex justify-end">
                    <Button
                      size="xs"
                      variant="outline"
                      onClick={() => setEditing(r)}
                    >
                      <Coins /> 调整
                    </Button>
                  </div>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </div>
        </TabsContent>
      </Tabs>

      {editing && (
        <AdjustCreditsDialog
          row={editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null)
            void load()
          }}
        />
      )}
    </div>
  )
}

function AdjustCreditsDialog({
  row,
  onClose,
  onSaved,
}: {
  row: AdminUserQuota
  onClose: () => void
  onSaved: () => void
}) {
  const [mode, setMode] = useState<"delta" | "balance">("delta")
  const [delta, setDelta] = useState("100")
  const [balance, setBalance] = useState(String(row.balance))
  const [reason, setReason] = useState("")
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)

  async function save() {
    setError(null)
    setSaving(true)
    try {
      if (mode === "delta") {
        const n = Number(delta)
        if (!Number.isFinite(n) || n === 0) {
          setError("增减值需为非零整数（正数充值，负数扣减）")
          setSaving(false)
          return
        }
        await adminQuotaApi.adjust(row.user_id, {
          delta: Math.trunc(n),
          reason: reason.trim() || undefined,
        })
      } else {
        const n = Number(balance)
        if (!Number.isFinite(n) || n < 0) {
          setError("余额需为 >= 0 的整数")
          setSaving(false)
          return
        }
        await adminQuotaApi.adjust(row.user_id, {
          balance: Math.trunc(n),
          reason: reason.trim() || undefined,
        })
      }
      onSaved()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setSaving(false)
    }
  }

  // 父级用 {editing && <AdjustCreditsDialog/>} 控制挂载，这里常开，关闭走 onClose
  return (
    <Dialog open onOpenChange={(next) => !next && onClose()}>
      <DialogContent className="flex flex-col gap-4 sm:max-w-md">
        <DialogHeader>
          <DialogTitle>调整额度 · {row.username}</DialogTitle>
          <DialogDescription>
            当前余额 {row.balance}，累计消耗 {row.lifetime_used}。所有改动都会写入流水。
          </DialogDescription>
        </DialogHeader>

        {error && (
          <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
            {error}
          </div>
        )}

        <div className="flex items-center rounded-md border border-border p-0.5">
          <Button
            size="sm"
            variant={mode === "delta" ? "secondary" : "ghost"}
            className="flex-1"
            onClick={() => setMode("delta")}
          >
            增减
          </Button>
          <Button
            size="sm"
            variant={mode === "balance" ? "secondary" : "ghost"}
            className="flex-1"
            onClick={() => setMode("balance")}
          >
            设置余额
          </Button>
        </div>

        {mode === "delta" ? (
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="adj-delta">增减值（正数充值、负数扣减）</Label>
            <Input
              id="adj-delta"
              type="number"
              value={delta}
              onChange={(e) => setDelta(e.target.value)}
              placeholder="例如 500 或 -100"
            />
          </div>
        ) : (
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="adj-bal">新余额</Label>
            <Input
              id="adj-bal"
              type="number"
              min="0"
              value={balance}
              onChange={(e) => setBalance(e.target.value)}
            />
          </div>
        )}

        <div className="flex flex-col gap-1.5">
          <Label htmlFor="adj-reason">备注（可选）</Label>
          <Input
            id="adj-reason"
            value={reason}
            onChange={(e) => setReason(e.target.value)}
            maxLength={80}
            placeholder="例：活动奖励、异常扣减补偿"
          />
        </div>

        <DialogFooter className="flex-row justify-end">
          <Button variant="outline" onClick={onClose} disabled={saving}>
            取消
          </Button>
          <Button onClick={() => void save()} disabled={saving}>
            {saving ? "保存中…" : "保存"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function InvitesPanel() {
  const [rows, setRows] = useState<AdminInviteRow[]>([])
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [query, setQuery] = useState("")

  async function load() {
    setLoading(true)
    setError(null)
    try {
      setRows(await adminApi.listInvites())
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }
  useEffect(() => {
    void load()
  }, [])

  const q = query.trim().toLowerCase()
  const filtered = q
    ? rows.filter(
        (r) =>
          r.inviter_username.toLowerCase().includes(q) ||
          r.invitee_username.toLowerCase().includes(q) ||
          (r.inviter_code ?? "").toLowerCase().includes(q)
      )
    : rows

  const leaderboard = (() => {
    const m = new Map<
      number,
      { id: number; username: string; code: string | null; count: number }
    >()
    for (const r of rows) {
      const cur = m.get(r.inviter_id)
      if (cur) cur.count += 1
      else
        m.set(r.inviter_id, {
          id: r.inviter_id,
          username: r.inviter_username,
          code: r.inviter_code,
          count: 1,
        })
    }
    return Array.from(m.values())
      .sort((a, b) => b.count - a.count || a.username.localeCompare(b.username))
      .slice(0, 10)
  })()

  return (
    <div className="flex flex-col gap-4">
      <p className="text-sm text-muted-foreground">
        全站所有成功触发邀请奖励的注册关系。最多展示最近 500 条。
      </p>

      <div className="flex items-center gap-2">
        <Input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="搜索邀请人 / 被邀请人 / 邀请码…"
          className="max-w-xs"
        />
        <Button variant="outline" size="sm" onClick={() => void load()}>
          <RefreshCw /> 刷新
        </Button>
        <span className="ml-auto text-xs text-muted-foreground">
          共 {rows.length} 条
        </span>
      </div>

      {error && (
        <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
          {error}
        </div>
      )}

      {leaderboard.length > 0 && (
        <div className="rounded-xl border border-border bg-card p-4 shadow-sm">
          <h2 className="mb-3 text-sm font-semibold">邀请榜 · Top 10</h2>
          <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
            {leaderboard.map((l, idx) => (
              <div
                key={l.id}
                className="flex items-center justify-between rounded-md border border-border/60 bg-background px-3 py-2 text-sm"
              >
                <div className="flex min-w-0 items-center gap-2">
                  <span className="w-5 text-right font-mono text-xs text-muted-foreground">
                    {idx + 1}
                  </span>
                  <span className="truncate font-medium">{l.username}</span>
                  {l.code && (
                    <span className="truncate font-mono text-[11px] text-muted-foreground">
                      {l.code}
                    </span>
                  )}
                </div>
                <span className="tabular-nums text-sm">
                  <b>{l.count}</b>{" "}
                  <span className="text-xs text-muted-foreground">人</span>
                </span>
              </div>
            ))}
          </div>
        </div>
      )}

      <div className="rounded-lg border border-border">
        {/* 表头/单元格的间距在父级统一设置，省得每个 Head/Cell 都写一遍 */}
        <Table className="min-w-[36rem]">
          <TableHeader className="bg-muted/40 text-xs uppercase [&_th]:h-9 [&_th]:px-3 [&_th]:text-muted-foreground">
            <TableRow>
              <TableHead>邀请人</TableHead>
              <TableHead>邀请码</TableHead>
              <TableHead>被邀请人</TableHead>
              <TableHead>注册时间</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody className="[&_td]:px-3 [&_td]:py-2">
            {loading && (
              <TableRow>
                <TableCell colSpan={4} className="py-6 text-center text-muted-foreground">
                  加载中…
                </TableCell>
              </TableRow>
            )}
            {!loading && filtered.length === 0 && (
              <TableRow>
                <TableCell colSpan={4} className="py-6 text-center text-muted-foreground">
                  暂无邀请记录
                </TableCell>
              </TableRow>
            )}
            {filtered.map((r) => (
              <TableRow key={`${r.inviter_id}-${r.invitee_id}`}>
                <TableCell>
                  <span className="font-medium">{r.inviter_username}</span>
                  <span className="ml-1 text-xs text-muted-foreground">
                    #{r.inviter_id}
                  </span>
                </TableCell>
                <TableCell className="font-mono text-xs text-muted-foreground">
                  {r.inviter_code ?? "—"}
                </TableCell>
                <TableCell>
                  <span className="font-medium">{r.invitee_username}</span>
                  <span className="ml-1 text-xs text-muted-foreground">
                    #{r.invitee_id}
                  </span>
                </TableCell>
                <TableCell className="text-xs text-muted-foreground">
                  {r.invitee_created_at.replace("T", " ").replace("Z", "")}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </div>
    </div>
  )
}

function EmailPanel() {
  const [cfg, setCfg] = useState<AdminSettings | null>(null)
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [saveMsg, setSaveMsg] = useState<string | null>(null)
  const [patch, setPatch] = useState<AdminSettingsUpdate>({})
  const [testEmail, setTestEmail] = useState("")
  const [testing, setTesting] = useState(false)
  const [testMsg, setTestMsg] = useState<string | null>(null)
  const [testErr, setTestErr] = useState<string | null>(null)

  async function load() {
    setLoading(true)
    setError(null)
    try {
      const c = await adminQuotaApi.getSettings()
      setCfg(c)
      setPatch({})
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }
  useEffect(() => {
    void load()
  }, [])

  function set<K extends keyof AdminSettingsUpdate>(
    k: K,
    v: AdminSettingsUpdate[K]
  ) {
    setPatch((p) => ({ ...p, [k]: v }))
  }

  async function save() {
    setSaveMsg(null)
    setError(null)
    setSaving(true)
    try {
      await adminQuotaApi.updateSettings(patch)
      setSaveMsg("已保存")
      await load()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setSaving(false)
    }
  }

  async function sendTest() {
    setTestMsg(null)
    setTestErr(null)
    if (!testEmail.trim()) {
      setTestErr("请输入收件邮箱")
      return
    }
    setTesting(true)
    try {
      await adminQuotaApi.sendTestEmail(testEmail.trim())
      setTestMsg("已发送，请检查收件箱（可能在垃圾邮件中）")
    } catch (e) {
      setTestErr(e instanceof Error ? e.message : String(e))
    } finally {
      setTesting(false)
    }
  }

  if (loading && !cfg) {
    return <p className="text-sm text-muted-foreground">加载中…</p>
  }
  if (!cfg) {
    return (
      <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
        {error ?? "无法加载设置"}
      </div>
    )
  }

  return (
    <div className="flex flex-col gap-4">
      <p className="text-sm text-muted-foreground">
        配置注册策略、额度换算与发信 SMTP。SMTP 密码留空不修改；填 <b>&ldquo;&ndash;&ldquo;</b> 可清除。
      </p>

      {error && (
        <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
          {error}
        </div>
      )}

      <div className="rounded-xl border border-border bg-card p-4 shadow-sm">
        <h2 className="mb-3 text-sm font-semibold">注册策略</h2>
        <div className="flex items-center gap-2">
          <input
            type="checkbox"
            id="registration-enabled"
            className="size-4 accent-primary"
            defaultChecked={cfg.registration_enabled}
            onChange={(e) => set("registration_enabled", e.target.checked)}
          />
          <Label htmlFor="registration-enabled" className="cursor-pointer">
            允许新用户注册
          </Label>
        </div>
        <p className="mt-2 mb-4 text-xs text-muted-foreground">
          关闭后，注册接口返回 403；仅管理员可在「用户管理」为新成员手动创建账号时才能加人。建议公开部署在配置完初始管理员后关闭。
        </p>
        <div className="flex items-center gap-2">
          <input
            type="checkbox"
            id="email-required"
            className="size-4 accent-primary"
            defaultChecked={cfg.email_verification_required}
            onChange={(e) =>
              set("email_verification_required", e.target.checked)
            }
          />
          <Label htmlFor="email-required" className="cursor-pointer">
            注册时要求邮箱验证码
          </Label>
        </div>
        <p className="mt-2 text-xs text-muted-foreground">
          开启后，注册页会要求填写邮箱并输入接收到的 6 位验证码（10 分钟有效，60 秒冷却）。
        </p>
      </div>

      <div className="rounded-xl border border-border bg-card p-4 shadow-sm">
        <h2 className="mb-3 text-sm font-semibold">额度换算</h2>
        <p className="mb-3 text-xs text-muted-foreground">
          模型价格在「模型计费」中按各家<b>官方美元价目表</b>填写，这里决定美元如何折算成站内额度。
          对话按上游返回的真实 token 用量结算，生图按次、视频按秒。
        </p>
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          <div className="flex flex-col gap-1.5">
            <Label className="text-xs">1 美元 = 多少额度</Label>
            <Input
              type="number"
              min={1}
              defaultValue={cfg.quota_per_usd}
              onChange={(e) => set("quota_per_usd", Number(e.target.value) || 1)}
            />
            <p className="text-[11px] text-muted-foreground">
              上游每花费 1 美元，从用户余额扣除的额度数。数值越大，额度单位越细。
            </p>
          </div>
          <div className="flex flex-col gap-1.5">
            <Label className="text-xs">全局加价（%）</Label>
            <Input
              type="number"
              min={0}
              defaultValue={cfg.price_multiplier_percent}
              onChange={(e) =>
                set("price_multiplier_percent", Number(e.target.value) || 0)
              }
            />
            <p className="text-[11px] text-muted-foreground">
              100 为按原价，150 即在官方价基础上加价 50%，对所有模型统一生效。
            </p>
          </div>
          <div className="flex flex-col gap-1.5">
            <Label className="text-xs">注册赠送额度</Label>
            <Input
              type="number"
              min={0}
              defaultValue={cfg.signup_grant}
              onChange={(e) => set("signup_grant", Number(e.target.value) || 0)}
            />
          </div>
          <div className="grid grid-cols-2 gap-3">
            <div className="flex flex-col gap-1.5">
              <Label className="text-xs">邀请人奖励</Label>
              <Input
                type="number"
                min={0}
                defaultValue={cfg.invite_grant_inviter}
                onChange={(e) =>
                  set("invite_grant_inviter", Number(e.target.value) || 0)
                }
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label className="text-xs">被邀请人奖励</Label>
              <Input
                type="number"
                min={0}
                defaultValue={cfg.invite_grant_invitee}
                onChange={(e) =>
                  set("invite_grant_invitee", Number(e.target.value) || 0)
                }
              />
            </div>
          </div>
        </div>
      </div>

      <div className="rounded-xl border border-border bg-card p-4 shadow-sm">
        <h2 className="mb-3 text-sm font-semibold">SMTP 服务器</h2>
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          <div className="flex flex-col gap-1.5 sm:col-span-2">
            <Label className="text-xs">主机</Label>
            <Input
              defaultValue={cfg.smtp_host}
              placeholder="smtp.example.com"
              onChange={(e) => set("smtp_host", e.target.value)}
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label className="text-xs">端口</Label>
            <Input
              type="number"
              min="1"
              max="65535"
              defaultValue={String(cfg.smtp_port)}
              onChange={(e) => {
                const n = Number(e.target.value)
                if (Number.isFinite(n) && n > 0 && n < 65536)
                  set("smtp_port", Math.trunc(n))
              }}
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label className="text-xs">加密方式</Label>
            <Select
              defaultValue={cfg.smtp_security || "starttls"}
              onValueChange={(v) => set("smtp_security", v)}
            >
              <SelectTrigger className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="starttls">STARTTLS（推荐，端口 587）</SelectItem>
                <SelectItem value="tls">TLS / SSL（端口 465）</SelectItem>
                <SelectItem value="none">不加密（仅局域网测试）</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div className="flex flex-col gap-1.5">
            <Label className="text-xs">用户名</Label>
            <Input
              defaultValue={cfg.smtp_username}
              placeholder="通常为发件邮箱"
              onChange={(e) => set("smtp_username", e.target.value)}
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label className="text-xs">
              密码{" "}
              <span className="text-muted-foreground">
                （当前 {cfg.smtp_password_set ? "已设置" : "未设置"}；留空不改，填 &ldquo;&ndash;&ldquo; 清除）
              </span>
            </Label>
            <Input
              type="password"
              placeholder={
                cfg.smtp_password_set ? "••••（已设置，留空不修改）" : ""
              }
              onChange={(e) =>
                set("smtp_password", e.target.value === "-" ? "" : e.target.value)
              }
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label className="text-xs">发件人邮箱</Label>
            <Input
              type="email"
              defaultValue={cfg.smtp_from_email}
              placeholder="noreply@example.com"
              onChange={(e) => set("smtp_from_email", e.target.value)}
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label className="text-xs">发件人名称</Label>
            <Input
              defaultValue={cfg.smtp_from_name}
              placeholder="Yunova"
              onChange={(e) => set("smtp_from_name", e.target.value)}
            />
          </div>
        </div>
      </div>

      <div className="flex items-center gap-3">
        <Button onClick={() => void save()} disabled={saving}>
          {saving ? "保存中…" : "保存设置"}
        </Button>
        <Button variant="outline" onClick={() => void load()} disabled={saving}>
          <RefreshCw /> 重载
        </Button>
        {saveMsg && (
          <span className="text-xs text-emerald-600 dark:text-emerald-400">
            {saveMsg}
          </span>
        )}
      </div>

      <div className="rounded-xl border border-border bg-card p-4 shadow-sm">
        <h2 className="mb-3 text-sm font-semibold">发送测试邮件</h2>
        <p className="mb-3 text-xs text-muted-foreground">
          保存 SMTP 配置后，向指定邮箱发送一封测试邮件验证连通性。
        </p>
        <div className="flex flex-col gap-2 sm:flex-row">
          <Input
            type="email"
            placeholder="收件邮箱"
            value={testEmail}
            onChange={(e) => setTestEmail(e.target.value)}
            className="flex-1"
          />
          <Button onClick={() => void sendTest()} disabled={testing}>
            <Send /> {testing ? "发送中…" : "发送测试邮件"}
          </Button>
        </div>
        {testMsg && (
          <p className="mt-2 text-xs text-emerald-600 dark:text-emerald-400">
            {testMsg}
          </p>
        )}
        {testErr && <p className="mt-2 text-xs text-destructive">{testErr}</p>}
      </div>
    </div>
  )
}

function PaymentsPanel() {
  const [cfg, setCfg] = useState<AdminPaymentConfig | null>(null)
  const [patch, setPatch] = useState<AdminPaymentConfigUpdate>({})
  const [orders, setOrders] = useState<AdminOrder[]>([])
  const [statusFilter, setStatusFilter] = useState<"" | "pending" | "paid" | "failed">("")
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [saveMsg, setSaveMsg] = useState<string | null>(null)

  async function loadAll() {
    setLoading(true)
    setError(null)
    try {
      const [c, o] = await Promise.all([
        adminPaymentsApi.getConfig(),
        adminPaymentsApi.listOrders(
          statusFilter ? { status: statusFilter } : {}
        ),
      ])
      setCfg(c)
      setOrders(o)
      setPatch({})
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }
  useEffect(() => {
    void loadAll()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [statusFilter])

  function set<K extends keyof AdminPaymentConfigUpdate>(
    k: K,
    v: AdminPaymentConfigUpdate[K]
  ) {
    setPatch((p) => ({ ...p, [k]: v }))
  }

  async function save() {
    setSaveMsg(null)
    setError(null)
    setSaving(true)
    try {
      await adminPaymentsApi.updateConfig(patch)
      setSaveMsg("已保存")
      await loadAll()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setSaving(false)
    }
  }

  if (loading && !cfg) {
    return <p className="text-sm text-muted-foreground">加载中…</p>
  }
  if (!cfg) {
    return (
      <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
        {error ?? "无法加载支付配置"}
      </div>
    )
  }

  return (
    <div className="flex flex-col gap-4">
      <p className="text-sm text-muted-foreground">
        当前对接 <b>易支付</b>（epay）通用协议。用户在充值弹窗下单后会跳转到
        易支付完成支付，支付平台会向回调地址发送异步通知；回调里的
        <code>MD5</code> 签名用「商户密钥」校验，成功即按「1 元 = N 额度」比例入账。
      </p>

      {error && (
        <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
          {error}
        </div>
      )}

      <div className="rounded-xl border border-border bg-card p-4 shadow-sm">
        <h2 className="mb-3 text-sm font-semibold">网关配置</h2>
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          <div className="flex items-center gap-2 sm:col-span-2">
            <input
              type="checkbox"
              id="epay-on"
              className="size-4 accent-primary"
              defaultChecked={cfg.enabled}
              onChange={(e) => set("enabled", e.target.checked)}
            />
            <Label htmlFor="epay-on" className="cursor-pointer">
              启用充值入口（关闭时，前端不显示「充值」按钮，API 拒绝下单）
            </Label>
          </div>
          <div className="flex flex-col gap-1.5 sm:col-span-2">
            <Label className="text-xs">
              易支付地址{" "}
              <span className="text-muted-foreground">
                （填写网关域名或完整的 submit.php 地址）
              </span>
            </Label>
            <Input
              defaultValue={cfg.api_url}
              onChange={(e) => set("api_url", e.target.value.trim())}
              placeholder="https://pay.example.com"
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label className="text-xs">商户 ID (pid)</Label>
            <Input
              defaultValue={cfg.pid}
              onChange={(e) => set("pid", e.target.value.trim())}
              placeholder="1000"
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label className="text-xs">
              商户密钥{" "}
              <span className="text-muted-foreground">
                （当前 {cfg.key_set ? "已设置" : "未设置"}；留空不改，填
                &ldquo;&ndash;&ldquo; 清除）
              </span>
            </Label>
            <Input
              type="password"
              onChange={(e) =>
                set("key", e.target.value === "-" ? "" : e.target.value)
              }
              placeholder={cfg.key_set ? "••••（已设置，留空不修改）" : ""}
            />
          </div>
          <div className="flex flex-col gap-1.5 sm:col-span-2">
            <Label className="text-xs">
              回调地址{" "}
              <span className="text-muted-foreground">
                （只填写公网域名，例如 https://chat.example.com，系统自动补全通知路径）
              </span>
            </Label>
            <Input
              defaultValue={cfg.callback_url}
              onChange={(e) => set("callback_url", e.target.value.trim())}
              placeholder="https://your-domain.com"
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label className="text-xs">1 元 = 多少额度</Label>
            <Input
              type="number"
              min="1"
              defaultValue={cfg.quota_per_yuan}
              onChange={(e) => {
                const n = Number(e.target.value)
                if (Number.isFinite(n) && n >= 1) set("quota_per_yuan", Math.trunc(n))
              }}
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label className="text-xs">商品名称</Label>
            <Input
              defaultValue={cfg.product_name}
              onChange={(e) => set("product_name", e.target.value)}
              placeholder="Yunova 额度充值"
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label className="text-xs">最小金额（元）</Label>
            <Input
              type="number"
              min="1"
              defaultValue={cfg.min_yuan}
              onChange={(e) => {
                const n = Number(e.target.value)
                if (Number.isFinite(n) && n >= 1) set("min_yuan", Math.trunc(n))
              }}
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label className="text-xs">最大金额（元）</Label>
            <Input
              type="number"
              min="1"
              defaultValue={cfg.max_yuan}
              onChange={(e) => {
                const n = Number(e.target.value)
                if (Number.isFinite(n) && n >= 1) set("max_yuan", Math.trunc(n))
              }}
            />
          </div>
        </div>

        <div className="mt-4 flex items-center gap-3">
          <Button onClick={() => void save()} disabled={saving}>
            {saving ? "保存中…" : "保存配置"}
          </Button>
          <Button variant="outline" onClick={() => void loadAll()} disabled={saving}>
            <RefreshCw /> 重载
          </Button>
          {saveMsg && (
            <span className="text-xs text-emerald-600 dark:text-emerald-400">
              {saveMsg}
            </span>
          )}
        </div>
      </div>

      <div className="rounded-xl border border-border bg-card p-4 shadow-sm">
        <div className="mb-3 flex items-center justify-between">
          <h2 className="text-sm font-semibold">充值订单</h2>
          <div className="flex items-center gap-2">
            <Select
              value={statusFilter || STATUS_ALL}
              onValueChange={(v) =>
                setStatusFilter(
                  (v === STATUS_ALL ? "" : v) as typeof statusFilter
                )
              }
            >
              <SelectTrigger size="sm" className="text-xs">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={STATUS_ALL} className="text-xs">
                  全部状态
                </SelectItem>
                <SelectItem value="pending" className="text-xs">
                  待支付
                </SelectItem>
                <SelectItem value="paid" className="text-xs">
                  已支付
                </SelectItem>
                <SelectItem value="failed" className="text-xs">
                  已关闭
                </SelectItem>
              </SelectContent>
            </Select>
            <Button variant="outline" size="sm" onClick={() => void loadAll()}>
              <RefreshCw /> 刷新
            </Button>
          </div>
        </div>

        <div className="rounded-lg border border-border">
          {/* 表头/单元格的间距在父级统一设置，省得每个 Head/Cell 都写一遍 */}
          <Table className="min-w-[32rem] text-xs">
            <TableHeader className="bg-muted/40 text-[10px] uppercase [&_th]:h-8 [&_th]:px-2 [&_th]:text-[10px] [&_th]:text-muted-foreground">
              <TableRow>
                <TableHead>订单号</TableHead>
                <TableHead>用户</TableHead>
                <TableHead>方式</TableHead>
                <TableHead className="text-right">金额</TableHead>
                <TableHead className="text-right">额度</TableHead>
                <TableHead>状态</TableHead>
                <TableHead>创建</TableHead>
                <TableHead>支付时间</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody className="[&_td]:px-2 [&_td]:py-1.5">
              {orders.length === 0 && (
                <TableRow>
                  <TableCell colSpan={8} className="py-6 text-center text-muted-foreground">
                    暂无订单
                  </TableCell>
                </TableRow>
              )}
              {orders.map((o) => (
                <TableRow key={o.id}>
                  <TableCell className="font-mono text-[11px]">
                    {o.out_trade_no}
                  </TableCell>
                  <TableCell>{o.username}</TableCell>
                  <TableCell>{o.payway}</TableCell>
                  <TableCell className="text-right tabular-nums">
                    ¥{(o.amount_cents / 100).toFixed(2)}
                  </TableCell>
                  <TableCell className="text-right tabular-nums">
                    +{o.quota}
                  </TableCell>
                  <TableCell>
                    <OrderStatus status={o.status} />
                  </TableCell>
                  <TableCell className="text-muted-foreground">
                    {o.created_at}
                  </TableCell>
                  <TableCell className="text-muted-foreground">
                    {o.paid_at ?? "-"}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      </div>
    </div>
  )
}

function OrderStatus({ status }: { status: "pending" | "paid" | "failed" }) {
  const map = {
    pending: { label: "待支付", cls: "bg-amber-500/10 text-amber-600 dark:text-amber-400" },
    paid: { label: "已支付", cls: "bg-emerald-500/10 text-emerald-600 dark:text-emerald-400" },
    failed: { label: "已关闭", cls: "bg-destructive/10 text-destructive" },
  } as const
  const m = map[status]
  return (
    <span className={`inline-block rounded px-1.5 py-0.5 text-[10px] ${m.cls}`}>
      {m.label}
    </span>
  )
}
