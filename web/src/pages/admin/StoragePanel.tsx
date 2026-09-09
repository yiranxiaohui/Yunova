import { useEffect, useState } from "react"
import { Cloud, HardDrive, Loader2, Save, TestTube2 } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import {
  adminApi,
  type AdminStorageSettings,
  type AdminStorageSettingsUpdate,
} from "@/lib/admin"

type FormState = AdminStorageSettingsUpdate & {
  access_key_id: string
  secret_access_key: string
  session_token: string
  clear_session_token: boolean
}

type Notice = { kind: "success" | "error"; text: string } | null

function formFromSettings(settings: AdminStorageSettings): FormState {
  return {
    backend: settings.backend,
    endpoint: settings.endpoint,
    region: settings.region,
    bucket: settings.bucket,
    prefix: settings.prefix,
    path_style: settings.path_style,
    access_key_id: "",
    secret_access_key: "",
    session_token: "",
    clear_session_token: false,
  }
}

export function StoragePanel() {
  const [settings, setSettings] = useState<AdminStorageSettings | null>(null)
  const [form, setForm] = useState<FormState | null>(null)
  const [loading, setLoading] = useState(true)
  const [testing, setTesting] = useState(false)
  const [saving, setSaving] = useState(false)
  const [notice, setNotice] = useState<Notice>(null)

  useEffect(() => {
    void (async () => {
      setLoading(true)
      try {
        const loaded = await adminApi.storage()
        setSettings(loaded)
        setForm(formFromSettings(loaded))
      } catch (error) {
        setNotice({
          kind: "error",
          text: error instanceof Error ? error.message : String(error),
        })
      } finally {
        setLoading(false)
      }
    })()
  }, [])

  function payload(): AdminStorageSettingsUpdate | null {
    if (!form) return null
    return {
      ...form,
      access_key_id: form.access_key_id.trim() || undefined,
      secret_access_key: form.secret_access_key.trim() || undefined,
      session_token: form.session_token.trim() || undefined,
    }
  }

  async function testConnection() {
    const body = payload()
    if (!body) return
    setTesting(true)
    setNotice(null)
    try {
      await adminApi.testStorage(body)
      setNotice({ kind: "success", text: "连接测试成功，Bucket 可写入和删除对象。" })
    } catch (error) {
      setNotice({
        kind: "error",
        text: error instanceof Error ? error.message : String(error),
      })
    } finally {
      setTesting(false)
    }
  }

  async function save() {
    const body = payload()
    if (!body) return
    setSaving(true)
    setNotice(null)
    try {
      const updated = await adminApi.updateStorage(body)
      setSettings(updated)
      setForm(formFromSettings(updated))
      setNotice({ kind: "success", text: "配置已保存并立即生效，无需重启服务。" })
    } catch (error) {
      setNotice({
        kind: "error",
        text: error instanceof Error ? error.message : String(error),
      })
    } finally {
      setSaving(false)
    }
  }

  if (loading && !form) {
    return <p className="text-sm text-muted-foreground">加载中…</p>
  }
  if (!form) {
    return (
      <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
        {notice?.text || "无法加载媒体存储配置"}
      </div>
    )
  }

  const busy = testing || saving

  return (
    <div className="flex flex-col gap-4">
      {notice && (
        <div
          className={
            notice.kind === "success"
              ? "rounded-md border border-emerald-500/35 bg-emerald-500/10 px-3 py-2 text-sm text-emerald-700 dark:text-emerald-300"
              : "rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive"
          }
        >
          {notice.text}
        </div>
      )}

      {settings && (
        <div className="rounded-xl border border-border bg-card p-4 shadow-sm">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div>
              <h2 className="text-sm font-semibold">当前运行状态</h2>
              <p className="mt-1 text-xs text-muted-foreground">
                保存配置后，新的上传任务会立即使用所选存储。
              </p>
            </div>
            <div className="flex items-center gap-2 rounded-full border border-border bg-muted/40 px-3 py-1.5 text-xs">
              {settings.active_backend === "s3" ? (
                <Cloud className="size-3.5" />
              ) : (
                <HardDrive className="size-3.5" />
              )}
              <span className="font-semibold uppercase">{settings.active_backend}</span>
              <span className="max-w-64 truncate font-mono text-muted-foreground" title={settings.active_location}>
                {settings.active_location}
              </span>
            </div>
          </div>
        </div>
      )}

      <div className="rounded-xl border border-border bg-card p-4 shadow-sm">
        <h2 className="text-sm font-semibold">媒体存储配置</h2>
        <p className="mb-4 mt-1 text-xs text-muted-foreground">
          用于生成图片、视频、剪辑音频和用户头像。普通聊天文档附件仍保存在本地。
        </p>

        <div className="space-y-4">
          <div className="space-y-1.5">
            <Label htmlFor="storage-backend">存储方式</Label>
            <Select
              value={form.backend}
              onValueChange={(value: "local" | "s3") =>
                setForm((current) => current && { ...current, backend: value })
              }
              disabled={busy}
            >
              <SelectTrigger id="storage-backend" className="w-full sm:w-72">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="local">本地磁盘</SelectItem>
                <SelectItem value="s3">S3 兼容对象存储</SelectItem>
              </SelectContent>
            </Select>
          </div>

          {form.backend === "s3" ? (
            <>
              <div className="grid gap-4 sm:grid-cols-2">
                <Field label="Endpoint" htmlFor="s3-endpoint" hint="AWS S3 可留空；R2、MinIO 等填写完整 HTTPS 地址。">
                  <Input
                    id="s3-endpoint"
                    value={form.endpoint}
                    onChange={(event) =>
                      setForm((current) => current && { ...current, endpoint: event.target.value })
                    }
                    placeholder="https://<account-id>.r2.cloudflarestorage.com"
                    disabled={busy}
                  />
                </Field>
                <Field label="Region" htmlFor="s3-region" hint="Cloudflare R2 通常填写 auto。">
                  <Input
                    id="s3-region"
                    value={form.region}
                    onChange={(event) =>
                      setForm((current) => current && { ...current, region: event.target.value })
                    }
                    placeholder="us-east-1"
                    disabled={busy}
                  />
                </Field>
                <Field label="Bucket" htmlFor="s3-bucket">
                  <Input
                    id="s3-bucket"
                    value={form.bucket}
                    onChange={(event) =>
                      setForm((current) => current && { ...current, bucket: event.target.value })
                    }
                    placeholder="yunova-media"
                    disabled={busy}
                  />
                </Field>
                <Field label="对象前缀" htmlFor="s3-prefix" hint="用于与 Bucket 中的其他应用数据隔离。">
                  <Input
                    id="s3-prefix"
                    value={form.prefix}
                    onChange={(event) =>
                      setForm((current) => current && { ...current, prefix: event.target.value })
                    }
                    placeholder="yunova"
                    disabled={busy}
                  />
                </Field>
              </div>

              <label className="flex items-start gap-2 rounded-lg border border-border bg-muted/25 px-3 py-2.5 text-sm">
                <input
                  type="checkbox"
                  className="mt-0.5"
                  checked={form.path_style}
                  onChange={(event) =>
                    setForm((current) => current && { ...current, path_style: event.target.checked })
                  }
                  disabled={busy}
                />
                <span>
                  <span className="font-medium">使用 Path-style 地址</span>
                  <span className="mt-0.5 block text-xs text-muted-foreground">
                    R2 和大多数 MinIO 部署需要开启；AWS S3 通常关闭。
                  </span>
                </span>
              </label>

              <div className="grid gap-4 sm:grid-cols-2">
                <Field
                  label="Access Key ID"
                  htmlFor="s3-access-key"
                  hint={settings?.access_key_id_set ? `已保存 ${settings.access_key_id_hint ?? ""}；留空保持不变。` : undefined}
                >
                  <Input
                    id="s3-access-key"
                    type="password"
                    autoComplete="new-password"
                    value={form.access_key_id}
                    onChange={(event) =>
                      setForm((current) => current && { ...current, access_key_id: event.target.value })
                    }
                    placeholder={settings?.access_key_id_set ? "留空保持现有值" : "Access Key ID"}
                    disabled={busy}
                  />
                </Field>
                <Field
                  label="Secret Access Key"
                  htmlFor="s3-secret-key"
                  hint={settings?.secret_access_key_set ? "已保存；留空保持不变。" : undefined}
                >
                  <Input
                    id="s3-secret-key"
                    type="password"
                    autoComplete="new-password"
                    value={form.secret_access_key}
                    onChange={(event) =>
                      setForm((current) => current && { ...current, secret_access_key: event.target.value })
                    }
                    placeholder={settings?.secret_access_key_set ? "留空保持现有值" : "Secret Access Key"}
                    disabled={busy}
                  />
                </Field>
              </div>

              <Field
                label="Session Token（可选）"
                htmlFor="s3-session-token"
                hint={settings?.session_token_set ? "已保存；留空保持不变。" : "仅临时凭证需要填写。"}
              >
                <Input
                  id="s3-session-token"
                  type="password"
                  autoComplete="new-password"
                  value={form.session_token}
                  onChange={(event) =>
                    setForm((current) =>
                      current && {
                        ...current,
                        session_token: event.target.value,
                        clear_session_token: false,
                      }
                    )
                  }
                  placeholder={settings?.session_token_set ? "留空保持现有值" : "Session Token"}
                  disabled={busy || form.clear_session_token}
                />
              </Field>
              {settings?.session_token_set && (
                <label className="flex items-center gap-2 text-xs text-muted-foreground">
                  <input
                    type="checkbox"
                    checked={form.clear_session_token}
                    onChange={(event) =>
                      setForm((current) =>
                        current && { ...current, clear_session_token: event.target.checked }
                      )
                    }
                    disabled={busy}
                  />
                  清除已保存的 Session Token
                </label>
              )}

              <p className="rounded-lg border border-border bg-muted/25 px-3 py-2 text-xs text-muted-foreground">
                测试和保存会写入并立即删除一个空测试对象，用于验证 Bucket 的写入与删除权限。密钥不会通过接口回显。
              </p>
            </>
          ) : (
            <p className="rounded-lg border border-border bg-muted/25 px-3 py-2 text-sm text-muted-foreground">
              新图片、视频、剪辑音频和头像将写入本机数据目录。切换不会迁移已有文件，已保存的 S3 凭证会保留，方便之后重新启用。
            </p>
          )}

          <div className="flex flex-wrap justify-end gap-2 border-t border-border pt-4">
            {form.backend === "s3" && (
              <Button variant="outline" onClick={() => void testConnection()} disabled={busy}>
                {testing ? <Loader2 className="animate-spin" /> : <TestTube2 />}
                {testing ? "测试中…" : "测试连接"}
              </Button>
            )}
            <Button onClick={() => void save()} disabled={busy}>
              {saving ? <Loader2 className="animate-spin" /> : <Save />}
              {saving ? "保存中…" : "保存并应用"}
            </Button>
          </div>
        </div>
      </div>

      <p className="text-xs text-muted-foreground">
        切换到 S3 后，新媒体只写入对象存储；旧的本地媒体仍可回退读取，但不会自动迁移。
      </p>
    </div>
  )
}

function Field({
  label,
  htmlFor,
  hint,
  children,
}: {
  label: string
  htmlFor: string
  hint?: string
  children: React.ReactNode
}) {
  return (
    <div className="space-y-1.5">
      <Label htmlFor={htmlFor}>{label}</Label>
      {children}
      {hint && <p className="text-xs text-muted-foreground">{hint}</p>}
    </div>
  )
}
