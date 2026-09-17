import { useCallback, useEffect, useState } from "react"
import { Link, useSearchParams } from "react-router-dom"
import {
  ArrowLeft,
  Check,
  Loader2,
  ShieldCheck,
  Terminal,
  X,
} from "lucide-react"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import {
  cliAuth,
  isCompleteCode,
  normalizeUserCode,
  type CliCodeInfo,
} from "@/lib/cli-auth"

/**
 * Approve a CLI sign-in.
 *
 * This page exists so a command-line tool never has to ask for an account
 * password. The CLI shows a short code, the user brings it to a browser that
 * is already signed in, and the approval here is what lets the CLI collect a
 * scoped token on its next poll.
 *
 * Two deliberate properties:
 *
 * * The token is never rendered here. It is minted on the CLI's own poll, so
 *   a screenshot of this page, or a bystander reading it, reveals nothing.
 * * What is being approved is shown before the button, not after. A bare
 *   "approve?" prompt trains people to click yes; naming the tool, the machine
 *   and what it will be allowed to do is the difference between consent and a
 *   reflex.
 */
export default function CliLoginPage() {
  const [params] = useSearchParams()
  // Prefilled from the link the CLI prints, so the common path is one click.
  const initial = params.get("code") ?? ""
  const [code, setCode] = useState(normalizeUserCode(initial))
  const [info, setInfo] = useState<CliCodeInfo | null>(null)
  const [state, setState] = useState<
    "idle" | "checking" | "ready" | "approved" | "denied" | "error"
  >("idle")
  const [error, setError] = useState<string | null>(null)

  const lookup = useCallback(async (raw: string) => {
    setError(null)
    setState("checking")
    try {
      const found = await cliAuth.describe(raw)
      setInfo(found)
      // A code that was already handled must not offer the buttons again:
      // the CLI has moved on, and a second approval would be a no-op the user
      // would read as broken.
      setState(found.approved ? "approved" : found.denied ? "denied" : "ready")
    } catch (e) {
      setInfo(null)
      setError(e instanceof Error ? e.message : String(e))
      setState("error")
    }
  }, [])

  useEffect(() => {
    if (!isCompleteCode(initial)) return
    // Deferred rather than called in the effect body: the lookup sets state,
    // and doing that synchronously during the effect cascades renders. Same
    // first-tick pattern the device list uses.
    const t = setTimeout(() => void lookup(normalizeUserCode(initial)), 0)
    return () => clearTimeout(t)
    // Only on mount: re-running on every keystroke would look up half-typed
    // codes and flash errors while the user is still typing.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const submit = async (e: React.FormEvent) => {
    e.preventDefault()
    if (!isCompleteCode(code)) return
    await lookup(normalizeUserCode(code))
  }

  const approve = async () => {
    setError(null)
    try {
      await cliAuth.approve(normalizeUserCode(code))
      setState("approved")
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }

  const deny = async () => {
    setError(null)
    try {
      await cliAuth.deny(normalizeUserCode(code))
      setState("denied")
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }

  return (
    <div className="min-h-svh bg-background text-foreground">
      <header className="sticky top-0 z-30 border-b border-border/70 bg-background/85 backdrop-blur-xl">
        <div className="mx-auto flex h-14 max-w-2xl items-center gap-3 px-4 md:px-7">
          <Button asChild variant="ghost" size="icon-sm">
            <Link to="/" aria-label="返回">
              <ArrowLeft />
            </Link>
          </Button>
          <span className="grid size-8 shrink-0 place-items-center rounded-xl bg-primary/10 text-primary">
            <Terminal className="size-4" />
          </span>
          <div className="min-w-0">
            <h1 className="truncate text-sm font-semibold">授权命令行登录</h1>
            <p className="hidden text-[11px] text-muted-foreground sm:block">
              让 pi 等命令行工具使用你的 Yunova 模型额度
            </p>
          </div>
        </div>
      </header>

      <main className="mx-auto w-full max-w-2xl px-4 py-8 md:px-7 md:py-12">
        {state === "approved" ? (
          <section className="rounded-2xl border border-emerald-500/30 bg-emerald-500/5 p-6 text-center">
            <span className="mx-auto grid size-12 place-items-center rounded-2xl bg-emerald-500/15 text-emerald-500">
              <Check className="size-6" />
            </span>
            <h2 className="mt-4 text-lg font-semibold">已授权</h2>
            <p className="mx-auto mt-2 max-w-md text-sm leading-relaxed text-muted-foreground">
              可以回到命令行了，它会在几秒内自动完成登录。这个页面不会显示令牌，
              令牌由命令行自己取走并保存在你的电脑上。
            </p>
            <p className="mt-4 text-xs text-muted-foreground">
              随时可以在「设置 → Agent 令牌」里撤销这次授权。
            </p>
          </section>
        ) : state === "denied" ? (
          <section className="rounded-2xl border border-border/70 bg-card/60 p-6 text-center">
            <span className="mx-auto grid size-12 place-items-center rounded-2xl bg-muted text-muted-foreground">
              <X className="size-6" />
            </span>
            <h2 className="mt-4 text-lg font-semibold">已拒绝</h2>
            <p className="mx-auto mt-2 max-w-md text-sm leading-relaxed text-muted-foreground">
              这个登录码已作废，命令行不会拿到任何凭据。如果这不是你发起的登录，
              不需要再做别的事。
            </p>
          </section>
        ) : (
          <>
            <form onSubmit={submit} className="flex flex-col gap-4">
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="code">命令行里显示的登录码</Label>
                <Input
                  id="code"
                  value={code}
                  // Normalised as the user types: the dash and the case are
                  // formatting, not secret, so the field shows the canonical
                  // form instead of rejecting a valid code at submit.
                  onChange={(e) => setCode(normalizeUserCode(e.target.value))}
                  placeholder="ACDE-FGHJ"
                  autoFocus
                  spellCheck={false}
                  className="text-center font-mono text-lg tracking-[0.3em]"
                />
              </div>
              <Button
                type="submit"
                disabled={!isCompleteCode(code) || state === "checking"}
                className="gap-2"
              >
                {state === "checking" ? (
                  <Loader2 className="size-4 animate-spin" />
                ) : null}
                查询登录码
              </Button>
            </form>

            {info && state === "ready" && (
              <section className="mt-6 rounded-2xl border border-border/70 bg-card/60 p-5">
                <div className="flex items-center gap-2 text-sm font-semibold">
                  <ShieldCheck className="size-4 text-primary" /> 确认是你发起的登录
                </div>
                <dl className="mt-4 grid gap-2 text-sm">
                  <div className="flex items-center justify-between gap-3">
                    <dt className="text-muted-foreground">请求方</dt>
                    <dd className="font-medium">{info.client_name}</dd>
                  </div>
                  {info.hostname && (
                    <div className="flex items-center justify-between gap-3">
                      <dt className="text-muted-foreground">设备</dt>
                      <dd className="font-medium">{info.hostname}</dd>
                    </div>
                  )}
                  {info.platform && (
                    <div className="flex items-center justify-between gap-3">
                      <dt className="text-muted-foreground">系统</dt>
                      <dd className="font-medium">{info.platform}</dd>
                    </div>
                  )}
                </dl>
                <p className="mt-4 rounded-xl bg-muted/70 p-3 text-xs leading-relaxed text-muted-foreground">
                  授权后，这个命令行工具可以用你的账号调用平台模型并消耗额度。
                  它拿不到你的密码，也拿不到上游渠道密钥；授权可随时在
                  「Agent 令牌」里撤销。
                  <br />
                  如果这个登录码不是你刚才在自己的电脑上看到的，请选择拒绝。
                </p>
                <div className="mt-4 flex flex-wrap gap-2">
                  <Button onClick={() => void approve()} className="gap-2">
                    <Check className="size-4" /> 授权登录
                  </Button>
                  <Button
                    onClick={() => void deny()}
                    variant="outline"
                    className="gap-2"
                  >
                    <X className="size-4" /> 拒绝
                  </Button>
                </div>
              </section>
            )}

            {error && (
              <p className="mt-4 rounded-xl border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive">
                {error}
              </p>
            )}

            <section className="mt-8 rounded-2xl border border-border/70 bg-card/60 p-5">
              <div className="flex items-center gap-2 text-sm font-semibold">
                <Terminal className="size-4 text-primary" /> 还没安装命令行？
              </div>
              <p className="mt-3 text-sm leading-relaxed text-muted-foreground">
                在「
                <Link
                  to="/download"
                  className="text-primary underline-offset-2 hover:underline"
                >
                  下载
                </Link>
                」页面可以安装命令行工具，装好后运行 <code>/login</code>{" "}
                就会显示这里需要的登录码。
              </p>
            </section>
          </>
        )}
      </main>
    </div>
  )
}
