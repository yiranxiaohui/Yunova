import { useEffect, useMemo, useState } from "react"
import { useNavigate } from "react-router-dom"
import { CheckCircle2, Loader2 } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { setupApi, type ConnectionForm } from "@/lib/setup"
import { BrandMark } from "@/components/app/BrandMark"
import { cn } from "@/lib/utils"

type Kind = "sqlite" | "postgres"

const KIND_META: Record<Kind, { label: string; badge: string; port: number; placeholder: string; note: string }> = {
  sqlite: {
    label: "SQLite",
    badge: "零依赖",
    port: 0,
    placeholder: "yunova.db",
    note: "最简单。单文件、本机即跑，适合个人或小团队。",
  },
  postgres: {
    label: "PostgreSQL",
    badge: "推荐",
    port: 5432,
    placeholder: "yunova",
    note: "多用户或云部署的更稳选择。",
  },
}

export default function SetupPage() {
  const nav = useNavigate()
  const [step, setStep] = useState<1 | 2>(1)
  const [kind, setKind] = useState<Kind>("sqlite")
  const [sqlitePath, setSqlitePath] = useState("yunova.db")
  const [host, setHost] = useState("localhost")
  const [port, setPort] = useState<number>(5432)
  const [dbUser, setDbUser] = useState("")
  const [dbPass, setDbPass] = useState("")
  const [database, setDatabase] = useState("")
  const [tls, setTls] = useState(false)

  const [adminUser, setAdminUser] = useState("")
  const [adminPass, setAdminPass] = useState("")

  const [testing, setTesting] = useState(false)
  const [testOk, setTestOk] = useState(false)
  const [installing, setInstalling] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    setupApi.status().then((s) => {
      if (s.installed) nav("/login", { replace: true })
    }).catch(() => {})
  }, [nav])

  useEffect(() => {
    setTestOk(false)
    if (kind === "postgres") setPort(5432)
  }, [kind])

  const connection = useMemo<ConnectionForm>(() => {
    if (kind === "sqlite") {
      return { kind: "sqlite", sqlite_path: sqlitePath.trim() || "yunova.db" }
    }
    return {
      kind: "postgres",
      host: host.trim(),
      port,
      user: dbUser,
      password: dbPass,
      database: database.trim(),
      tls,
    }
  }, [kind, sqlitePath, host, port, dbUser, dbPass, database, tls])

  async function runTest() {
    setError(null)
    setTesting(true)
    setTestOk(false)
    try {
      await setupApi.test(connection)
      setTestOk(true)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setTesting(false)
    }
  }

  async function runInstall(e: React.FormEvent) {
    e.preventDefault()
    setError(null)
    if (adminPass.length < 6) {
      setError("管理员密码至少 6 位")
      return
    }
    setInstalling(true)
    try {
      await setupApi.install({
        connection,
        admin_username: adminUser.trim(),
        admin_password: adminPass,
      })
      nav("/login", { replace: true })
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setInstalling(false)
    }
  }

  const step1Complete = kind === "sqlite" || Boolean(database.trim())

  return (
    <div className="bg-auth-grid min-h-svh px-3 py-6 sm:px-4 sm:py-10">
      <div className="mx-auto max-w-2xl">
        <div className="mb-6 flex flex-wrap items-center justify-between gap-3 sm:mb-8">
          <BrandMark subtitle="首次初始化" />
          <Stepper step={step} />
        </div>

        {step === 1 && (
          <div className="rounded-2xl border border-border bg-card p-4 shadow-panel sm:p-6">
            <h2 className="text-lg font-semibold">选择数据库</h2>
            <p className="mt-1 text-sm text-muted-foreground">
              两者都可以随时切换，数据不会跟着走。生产环境推荐 PostgreSQL。
            </p>

            <div className="mt-4 grid gap-2 sm:grid-cols-2">
              {(Object.keys(KIND_META) as Kind[]).map((k) => {
                const m = KIND_META[k]
                const active = kind === k
                return (
                  <button
                    key={k}
                    type="button"
                    onClick={() => setKind(k)}
                    className={cn(
                      "group flex flex-col items-start gap-1 rounded-xl border p-3 text-left transition-colors",
                      active
                        ? "border-primary/50 bg-primary/5 shadow-sm"
                        : "border-border bg-background hover:border-primary/30 hover:bg-accent"
                    )}
                  >
                    <div className="flex w-full items-center justify-between">
                      <span className="text-sm font-medium">{m.label}</span>
                      <span
                        className={cn(
                          "rounded-full px-1.5 py-0.5 text-[10px]",
                          active
                            ? "bg-primary/15 text-primary"
                            : "bg-muted text-muted-foreground"
                        )}
                      >
                        {m.badge}
                      </span>
                    </div>
                    <span className="text-xs text-muted-foreground">{m.note}</span>
                  </button>
                )
              })}
            </div>

            <div className="mt-5 flex flex-col gap-3">
              {kind === "sqlite" ? (
                <div className="flex flex-col gap-1.5">
                  <Label htmlFor="path">数据库文件路径</Label>
                  <Input
                    id="path"
                    value={sqlitePath}
                    onChange={(e) => setSqlitePath(e.target.value)}
                    placeholder={KIND_META.sqlite.placeholder}
                  />
                  <p className="text-xs text-muted-foreground">
                    相对路径放到服务端 <code>data/</code> 下；不存在会自动创建。
                  </p>
                </div>
              ) : (
                <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
                  <div className="flex flex-col gap-1.5">
                    <Label htmlFor="host">主机</Label>
                    <Input id="host" value={host} onChange={(e) => setHost(e.target.value)} />
                  </div>
                  <div className="flex flex-col gap-1.5">
                    <Label htmlFor="port">端口</Label>
                    <Input id="port" type="number" value={port} onChange={(e) => setPort(Number(e.target.value))} />
                  </div>
                  <div className="flex flex-col gap-1.5">
                    <Label htmlFor="db">数据库名</Label>
                    <Input
                      id="db"
                      value={database}
                      onChange={(e) => setDatabase(e.target.value)}
                      placeholder={KIND_META[kind].placeholder}
                      required
                    />
                  </div>
                  <div className="flex flex-col gap-1.5">
                    <Label htmlFor="dbuser">用户名</Label>
                    <Input id="dbuser" value={dbUser} onChange={(e) => setDbUser(e.target.value)} />
                  </div>
                  <div className="col-span-2 flex flex-col gap-1.5">
                    <Label htmlFor="dbpass">密码</Label>
                    <Input id="dbpass" type="password" value={dbPass} onChange={(e) => setDbPass(e.target.value)} />
                  </div>
                  <label className="col-span-2 flex items-center gap-2 text-sm">
                    <input type="checkbox" checked={tls} onChange={(e) => setTls(e.target.checked)} />
                    <span>启用 TLS</span>
                  </label>
                </div>
              )}

              <div className="mt-1 flex items-center gap-3">
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={runTest}
                  disabled={testing}
                >
                  {testing ? <Loader2 className="animate-spin" /> : null}
                  {testing ? "测试中…" : "测试连接"}
                </Button>
                {testOk && (
                  <span className="inline-flex items-center gap-1 text-sm text-emerald-600 dark:text-emerald-400">
                    <CheckCircle2 className="size-4" /> 连接成功
                  </span>
                )}
              </div>
            </div>

            {error && (
              <div className="mt-4 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
                {error}
              </div>
            )}

            <div className="mt-6 flex justify-end">
              <Button
                type="button"
                size="lg"
                onClick={() => setStep(2)}
                disabled={!step1Complete}
              >
                下一步：创建管理员
              </Button>
            </div>
          </div>
        )}

        {step === 2 && (
          <form
            onSubmit={runInstall}
            className="rounded-2xl border border-border bg-card p-4 shadow-panel sm:p-6"
          >
            <h2 className="text-lg font-semibold">创建管理员账号</h2>
            <p className="mt-1 text-sm text-muted-foreground">
              这是首位登录 Yunova 的账号。之后可以在界面里再注册更多用户。
            </p>

            <div className="mt-5 grid grid-cols-1 gap-3 sm:grid-cols-2">
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="au">用户名</Label>
                <Input
                  id="au"
                  value={adminUser}
                  onChange={(e) => setAdminUser(e.target.value)}
                  minLength={3}
                  maxLength={32}
                  pattern="[A-Za-z0-9_\-]+"
                  required
                  autoFocus
                />
              </div>
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="ap">密码</Label>
                <Input
                  id="ap"
                  type="password"
                  value={adminPass}
                  onChange={(e) => setAdminPass(e.target.value)}
                  minLength={6}
                  required
                />
              </div>
            </div>

            {error && (
              <div className="mt-4 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
                {error}
              </div>
            )}

            <div className="mt-6 flex justify-between">
              <Button type="button" variant="ghost" onClick={() => setStep(1)}>
                上一步
              </Button>
              <Button type="submit" size="lg" disabled={installing}>
                {installing ? <Loader2 className="animate-spin" /> : null}
                {installing ? "初始化中…" : "完成安装"}
              </Button>
            </div>
          </form>
        )}
      </div>
    </div>
  )
}

function Stepper({ step }: { step: 1 | 2 }) {
  return (
    <div className="flex items-center gap-2 text-xs text-muted-foreground">
      <StepDot active={step >= 1} label="1 · 数据库" />
      <div className="h-px w-8 bg-border" />
      <StepDot active={step >= 2} label="2 · 管理员" />
    </div>
  )
}

function StepDot({ active, label }: { active: boolean; label: string }) {
  return (
    <span
      className={cn(
        "rounded-full border px-2 py-0.5 transition-colors",
        active
          ? "border-primary/40 bg-primary/10 text-foreground"
          : "border-border bg-background"
      )}
    >
      {label}
    </span>
  )
}
