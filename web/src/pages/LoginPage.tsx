import { useState } from "react"
import { Link, useNavigate } from "react-router-dom"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { useAuth } from "@/lib/auth-context"
import { AuthShell } from "@/components/app/AuthShell"

export default function LoginPage() {
  const auth = useAuth()
  const nav = useNavigate()
  const [username, setUsername] = useState("")
  const [password, setPassword] = useState("")
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  async function submit(e: React.FormEvent) {
    e.preventDefault()
    setError(null)
    setBusy(true)
    try {
      await auth.login(username.trim(), password)
      nav("/", { replace: true })
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <AuthShell
      eyebrow="欢迎回来"
      title="继续你的创作"
      description="登录 Yunova，回到你的对话与工作空间。"
      footer={
        <p>
          还没有账号？{" "}
          <Link
            to="/register"
            className="font-semibold text-primary underline-offset-4 hover:underline"
          >
            免费注册
          </Link>
        </p>
      }
    >
        <form onSubmit={submit} className="mt-8 flex flex-col gap-4.5">
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="username">用户名</Label>
            <Input
              id="username"
              autoComplete="username"
              value={username}
              onChange={(e) => setUsername(e.target.value)}
              required
              autoFocus
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="password">密码</Label>
            <Input
              id="password"
              type="password"
              autoComplete="current-password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              required
            />
          </div>
          {error && (
            <div className="rounded-xl border border-destructive/30 bg-destructive/8 px-3 py-2.5 text-sm text-destructive">
              {error}
            </div>
          )}
          <Button type="submit" disabled={busy} className="mt-1 w-full bg-gradient-to-r from-primary to-violet-600" size="lg">
            {busy ? "登录中…" : "登录"}
          </Button>
        </form>
    </AuthShell>
  )
}
