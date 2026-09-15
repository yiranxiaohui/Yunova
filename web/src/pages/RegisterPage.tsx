import { useEffect, useRef, useState } from "react"
import { Link, useNavigate, useSearchParams } from "react-router-dom"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { useAuth } from "@/lib/auth-context"
import { api } from "@/lib/api"
import { AuthShell } from "@/components/app/AuthShell"

export default function RegisterPage() {
  const auth = useAuth()
  const nav = useNavigate()
  const [params] = useSearchParams()
  const refFromUrl = (params.get("ref") ?? "").trim().toUpperCase()
  const [username, setUsername] = useState("")
  const [password, setPassword] = useState("")
  const [confirm, setConfirm] = useState("")
  const [email, setEmail] = useState("")
  const [emailCode, setEmailCode] = useState("")
  const [inviteCode, setInviteCode] = useState(refFromUrl)
  const [emailRequired, setEmailRequired] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [codeMsg, setCodeMsg] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [sendingCode, setSendingCode] = useState(false)
  const [cooldown, setCooldown] = useState(0)
  const tickRef = useRef<ReturnType<typeof setInterval> | null>(null)

  useEffect(() => {
    let cancelled = false
    api
      .authConfig()
      .then((c) => {
        if (!cancelled) setEmailRequired(c.email_verification_required)
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [])

  useEffect(() => {
    return () => {
      if (tickRef.current) clearInterval(tickRef.current)
    }
  }, [])

  function startCooldown(seconds: number) {
    setCooldown(seconds)
    if (tickRef.current) clearInterval(tickRef.current)
    tickRef.current = setInterval(() => {
      setCooldown((n) => {
        if (n <= 1) {
          if (tickRef.current) clearInterval(tickRef.current)
          return 0
        }
        return n - 1
      })
    }, 1000)
  }

  async function sendCode() {
    setError(null)
    setCodeMsg(null)
    const trimmed = email.trim().toLowerCase()
    if (!trimmed || !trimmed.includes("@")) {
      setError("请先填写邮箱")
      return
    }
    setSendingCode(true)
    try {
      await api.sendEmailCode(trimmed)
      setCodeMsg("验证码已发送，请查收邮件（10 分钟内有效）")
      startCooldown(60)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSendingCode(false)
    }
  }

  async function submit(e: React.FormEvent) {
    e.preventDefault()
    setError(null)
    if (password !== confirm) {
      setError("两次输入的密码不一致")
      return
    }
    if (password.length < 6) {
      setError("密码至少 6 位")
      return
    }
    if (emailRequired) {
      if (!email.trim()) {
        setError("请填写邮箱")
        return
      }
      if (!emailCode.trim()) {
        setError("请填写邮箱验证码")
        return
      }
    }
    setBusy(true)
    try {
      await auth.register({
        username: username.trim(),
        password,
        invite_code: inviteCode.trim() || undefined,
        email: email.trim() || undefined,
        email_code: emailCode.trim() || undefined,
      })
      nav("/", { replace: true })
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <AuthShell
      eyebrow="加入 Yunova"
      title="创建你的空间"
      description="只需几步，即可开始与多个领先模型协作。"
      footer={
        <p>
          已有账号？{" "}
          <Link
            to="/login"
            className="font-semibold text-primary underline-offset-4 hover:underline"
          >
            直接登录
          </Link>
        </p>
      }
    >
        <form onSubmit={submit} className="mt-7 flex flex-col gap-3.5">
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="username">用户名</Label>
            <Input
              id="username"
              autoComplete="username"
              value={username}
              onChange={(e) => setUsername(e.target.value)}
              required
              minLength={3}
              maxLength={32}
              pattern="[A-Za-z0-9_\-]+"
              title="3-32 位，字母数字下划线或短横线"
              autoFocus
            />
          </div>

          {emailRequired && (
            <>
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="email">邮箱</Label>
                <div className="flex gap-2">
                  <Input
                    id="email"
                    type="email"
                    autoComplete="email"
                    value={email}
                    onChange={(e) => setEmail(e.target.value)}
                    required
                    className="flex-1"
                    placeholder="you@example.com"
                  />
                  <Button
                    type="button"
                    variant="outline"
                    onClick={() => void sendCode()}
                    disabled={sendingCode || cooldown > 0}
                  >
                    {sendingCode
                      ? "发送中…"
                      : cooldown > 0
                        ? `${cooldown}s`
                        : "发送验证码"}
                  </Button>
                </div>
                {codeMsg && (
                  <p className="text-xs text-emerald-600 dark:text-emerald-400">
                    {codeMsg}
                  </p>
                )}
              </div>
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="email-code">邮箱验证码</Label>
                <Input
                  id="email-code"
                  value={emailCode}
                  onChange={(e) =>
                    setEmailCode(e.target.value.replace(/\D/g, "").slice(0, 6))
                  }
                  required
                  inputMode="numeric"
                  pattern="[0-9]{6}"
                  maxLength={6}
                  placeholder="6 位数字"
                  className="font-mono tracking-[0.4em]"
                />
              </div>
            </>
          )}

          <div className="flex flex-col gap-1.5">
            <Label htmlFor="password">密码</Label>
            <Input
              id="password"
              type="password"
              autoComplete="new-password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              required
              minLength={6}
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="confirm">确认密码</Label>
            <Input
              id="confirm"
              type="password"
              autoComplete="new-password"
              value={confirm}
              onChange={(e) => setConfirm(e.target.value)}
              required
              minLength={6}
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="invite">
              邀请码{" "}
              <span className="text-xs text-muted-foreground">
                （可选，填写可获得额外额度）
              </span>
            </Label>
            <Input
              id="invite"
              value={inviteCode}
              onChange={(e) =>
                setInviteCode(e.target.value.trim().toUpperCase())
              }
              maxLength={16}
              placeholder="ABC1234"
              className="font-mono uppercase tracking-wide"
            />
          </div>
          {error && (
            <div className="rounded-xl border border-destructive/30 bg-destructive/8 px-3 py-2.5 text-sm text-destructive">
              {error}
            </div>
          )}
          <Button type="submit" disabled={busy} className="mt-1 w-full bg-gradient-to-r from-primary to-violet-600" size="lg">
            {busy ? "创建中…" : "创建账号"}
          </Button>
        </form>
    </AuthShell>
  )
}
