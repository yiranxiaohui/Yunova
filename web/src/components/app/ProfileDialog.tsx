import { useEffect, useRef, useState } from "react"
import { formatQuota } from "@/lib/quota"
import { Check, Copy, Users } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { useAuth } from "@/lib/auth-context"
import { profileApi } from "@/lib/profile"
import {
  buildInviteUrl,
  invitesApi,
  type InviteInfo,
} from "@/lib/invites"

type Props = {
  open: boolean
  onClose: () => void
}

export function ProfileDialog({ open, onClose }: Props) {
  const auth = useAuth()
  const user = auth.state.status === "authed" ? auth.state.user : null

  const [displayName, setDisplayName] = useState("")
  const [avatarUrl, setAvatarUrl] = useState("")
  const [avatarBroken, setAvatarBroken] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [saving, setSaving] = useState(false)

  const fileInputRef = useRef<HTMLInputElement | null>(null)
  const [uploading, setUploading] = useState(false)

  const [pwOpen, setPwOpen] = useState(false)
  const [currentPw, setCurrentPw] = useState("")
  const [newPw, setNewPw] = useState("")
  const [confirmPw, setConfirmPw] = useState("")
  const [pwSaving, setPwSaving] = useState(false)
  const [pwMsg, setPwMsg] = useState<string | null>(null)

  const [invite, setInvite] = useState<InviteInfo | null>(null)
  const [inviteCopied, setInviteCopied] = useState<"code" | "link" | null>(null)

  useEffect(() => {
    if (!open || !user) return
    setDisplayName(user.display_name ?? "")
    setAvatarUrl(user.avatar_url ?? "")
    setAvatarBroken(false)
    setError(null)
    setPwOpen(false)
    setCurrentPw("")
    setNewPw("")
    setConfirmPw("")
    setPwMsg(null)
    setInviteCopied(null)
    let cancelled = false
    invitesApi
      .me()
      .then((info) => {
        if (!cancelled) setInvite(info)
      })
      .catch(() => {
        if (!cancelled) setInvite(null)
      })
    return () => {
      cancelled = true
    }
  }, [open, user])

  async function copyInvite(kind: "code" | "link") {
    if (!invite) return
    const text =
      kind === "code" ? invite.invite_code : buildInviteUrl(invite.invite_code)
    try {
      await navigator.clipboard.writeText(text)
      setInviteCopied(kind)
      setTimeout(() => {
        setInviteCopied((x) => (x === kind ? null : x))
      }, 1200)
    } catch {
      /* ignore */
    }
  }

  useEffect(() => {
    setAvatarBroken(false)
  }, [avatarUrl])

  // open 交给 Dialog 管；user 为空时整个组件不渲染，内部可放心解引用
  if (!user) return null

  const trimmedAvatar = avatarUrl.trim()
  const initial = (
    (displayName.trim() || user.username || "?").slice(0, 1) || "?"
  ).toUpperCase()
  const dirty = (displayName.trim() || null) !== (user.display_name ?? null)

  async function save() {
    setError(null)
    setSaving(true)
    try {
      const updated = await profileApi.update({
        display_name: displayName,
      })
      auth.updateUser(updated)
      onClose()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setSaving(false)
    }
  }

  async function uploadFile(file: File) {
    setError(null)
    if (!file.type.startsWith("image/")) {
      setError("请选择图片文件")
      return
    }
    if (file.size > 2 * 1024 * 1024) {
      setError("图片不能超过 2MB")
      return
    }
    setUploading(true)
    try {
      const updated = await profileApi.uploadAvatar(file)
      auth.updateUser(updated)
      setAvatarUrl(updated.avatar_url ?? "")
      setAvatarBroken(false)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setUploading(false)
    }
  }

  async function changePassword() {
    setPwMsg(null)
    if (!currentPw) {
      setPwMsg("请输入当前密码")
      return
    }
    if (newPw.length < 6 || newPw.length > 256) {
      setPwMsg("新密码需为 6-256 位")
      return
    }
    if (newPw !== confirmPw) {
      setPwMsg("两次输入的新密码不一致")
      return
    }
    setPwSaving(true)
    try {
      await profileApi.changePassword(currentPw, newPw)
      setPwMsg("密码已更新")
      setCurrentPw("")
      setNewPw("")
      setConfirmPw("")
    } catch (e) {
      setPwMsg(e instanceof Error ? e.message : String(e))
    } finally {
      setPwSaving(false)
    }
  }

  const showImage = trimmedAvatar && !avatarBroken

  return (
    <Dialog open={open} onOpenChange={(next) => !next && onClose()}>
      <DialogContent
        className="nc-scroll flex flex-col gap-4"
        style={{ maxHeight: "calc(100svh - 2rem)", overflowY: "auto" }}
      >
        <DialogHeader>
          <DialogTitle>个人资料</DialogTitle>
          <DialogDescription>
            修改昵称、头像或密码。用户名一旦注册后不可更改。
          </DialogDescription>
        </DialogHeader>

        {error && (
          <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
            {error}
          </div>
        )}

        <input
          ref={fileInputRef}
          type="file"
          accept="image/png,image/jpeg,image/webp,image/gif"
          className="hidden"
          onChange={(e) => {
            const f = e.target.files?.[0]
            e.target.value = ""
            if (f) void uploadFile(f)
          }}
        />

        <div className="flex items-center gap-4">
          <button
            type="button"
            className="group relative size-16 shrink-0 overflow-hidden rounded-full border border-border"
            onClick={() => fileInputRef.current?.click()}
            disabled={uploading}
            title="点击上传头像"
          >
            {showImage ? (
              <img
                src={trimmedAvatar}
                alt=""
                className="size-full object-cover"
                onError={() => setAvatarBroken(true)}
              />
            ) : (
              <div className="grid size-full place-items-center bg-gradient-to-br from-primary to-chart-5 text-xl font-semibold text-primary-foreground">
                {initial}
              </div>
            )}
            <div className="absolute inset-0 grid place-items-center bg-black/50 text-[10px] font-medium text-white opacity-0 transition-opacity group-hover:opacity-100">
              {uploading ? "上传中…" : "更换"}
            </div>
          </button>
          <div className="min-w-0 flex-1">
            <p className="text-xs text-muted-foreground">用户名</p>
            <p className="truncate font-medium">{user.username}</p>
            <p className="mt-0.5 text-xs text-muted-foreground">
              点击头像上传图片（最大 2MB）
            </p>
          </div>
        </div>

        <div className="flex flex-col gap-1.5">
          <Label htmlFor="p-name">昵称</Label>
          <Input
            id="p-name"
            value={displayName}
            onChange={(e) => setDisplayName(e.target.value)}
            maxLength={64}
            placeholder="留空则显示用户名"
          />
        </div>

        <div className="flex flex-col gap-2 rounded-md border border-border bg-muted/30 p-3">
          <button
            type="button"
            className="flex items-center justify-between text-sm font-medium"
            onClick={() => setPwOpen((o) => !o)}
          >
            <span>修改密码</span>
            <span className="text-xs text-muted-foreground">
              {pwOpen ? "收起" : "展开"}
            </span>
          </button>
          {pwOpen && (
            <div className="flex flex-col gap-2">
              <Input
                type="password"
                autoComplete="current-password"
                placeholder="当前密码"
                value={currentPw}
                onChange={(e) => setCurrentPw(e.target.value)}
              />
              <Input
                type="password"
                autoComplete="new-password"
                placeholder="新密码（6-256 位）"
                value={newPw}
                onChange={(e) => setNewPw(e.target.value)}
              />
              <Input
                type="password"
                autoComplete="new-password"
                placeholder="确认新密码"
                value={confirmPw}
                onChange={(e) => setConfirmPw(e.target.value)}
              />
              {pwMsg && (
                <p className="text-xs text-muted-foreground">{pwMsg}</p>
              )}
              <div className="flex justify-end">
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => void changePassword()}
                  disabled={
                    pwSaving || !currentPw || !newPw || !confirmPw
                  }
                >
                  {pwSaving ? "更新中…" : "更新密码"}
                </Button>
              </div>
            </div>
          )}
        </div>

        {invite && (
          <div className="flex flex-col gap-2 rounded-md border border-border bg-muted/30 p-3">
            <div className="flex items-center gap-2">
              <Users className="size-4 text-primary" />
              <span className="text-sm font-medium">邀请好友</span>
              <span className="ml-auto text-xs text-muted-foreground">
                已邀请 <b className="text-foreground">{invite.invited_count}</b>{" "}
                · 获得 <b className="text-foreground">{invite.total_earned}</b>{" "}
                奖励
              </span>
            </div>
            <p className="text-xs text-muted-foreground">
              好友用你的邀请码注册后，你获得{" "}
              <b className="text-foreground">{formatQuota(invite.grant_inviter)}</b> 元，
              对方额外获得{" "}
              <b className="text-foreground">{formatQuota(invite.grant_invitee)}</b> 元。
            </p>
            <div className="flex items-center gap-2">
              <Input
                value={invite.invite_code}
                readOnly
                className="font-mono tracking-widest"
                onFocus={(e) => e.currentTarget.select()}
              />
              <Button
                size="sm"
                variant="outline"
                onClick={() => void copyInvite("code")}
                title="复制邀请码"
              >
                {inviteCopied === "code" ? (
                  <Check className="text-emerald-500" />
                ) : (
                  <Copy />
                )}
                码
              </Button>
              <Button
                size="sm"
                variant="outline"
                onClick={() => void copyInvite("link")}
                title="复制邀请链接"
              >
                {inviteCopied === "link" ? (
                  <Check className="text-emerald-500" />
                ) : (
                  <Copy />
                )}
                链接
              </Button>
            </div>
          </div>
        )}

        <DialogFooter>
          <Button variant="outline" onClick={onClose} disabled={saving}>
            取消
          </Button>
          <Button onClick={() => void save()} disabled={saving || !dirty}>
            {saving ? "保存中…" : "保存"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
