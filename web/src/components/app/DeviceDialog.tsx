import { useCallback, useEffect, useState } from "react"
import { Check, Copy, Laptop, Loader2, Trash2 } from "lucide-react"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { useConfirm } from "@/lib/confirm-context"
import { listDevices, pairDevice, revokeDevice, type AgentDevice } from "@/lib/agent"
import { toast } from "sonner"

/**
 * Manage the machines that can run tasks locally.
 *
 * Pairing is code-based and the code is shown exactly once: the server only
 * stores its hash, so it cannot be redisplayed later. The dialog therefore
 * treats copying it as the primary action rather than something the user can
 * come back for.
 */
export function DeviceDialog({
  open,
  onOpenChange,
}: {
  open: boolean
  onOpenChange: (v: boolean) => void
}) {
  const { confirm } = useConfirm()
  const [devices, setDevices] = useState<AgentDevice[]>([])
  const [loading, setLoading] = useState(true)
  const [pairing, setPairing] = useState(false)
  const [name, setName] = useState("")
  const [code, setCode] = useState<string | null>(null)
  const [copied, setCopied] = useState(false)

  const load = useCallback(async () => {
    try {
      const next = await listDevices()
      setDevices(next)
    } catch (e) {
      toast.error(`读取设备列表失败：${(e as Error).message}`)
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    if (!open) return
    // Liveness comes from an open socket, so poll while the dialog is visible
    // rather than showing a stale online state. Kicked off via the timer's
    // first tick pattern so no setState runs synchronously in the effect.
    let cancelled = false
    const tick = () => {
      if (!cancelled) void load()
    }
    const initial = setTimeout(tick, 0)
    const t = setInterval(tick, 5000)
    return () => {
      cancelled = true
      clearTimeout(initial)
      clearInterval(t)
    }
  }, [open, load])

  const pair = async () => {
    setPairing(true)
    try {
      const r = await pairDevice(name.trim() || undefined)
      setCode(r.code)
      setName("")
      await load()
    } catch (e) {
      toast.error(`创建配对码失败：${(e as Error).message}`)
    } finally {
      setPairing(false)
    }
  }

  const remove = async (d: AgentDevice) => {
    const ok = await confirm({
      title: `移除「${d.name}」？`,
      description: "该电脑会立即断开，配对码失效；已有任务记录会保留。",
      confirmText: "移除",
      destructive: true,
    })
    if (!ok) return
    try {
      await revokeDevice(d.id)
      await load()
    } catch (e) {
      toast.error(`移除失败：${(e as Error).message}`)
    }
  }

  const copy = async () => {
    if (!code) return
    try {
      await navigator.clipboard.writeText(code)
      setCopied(true)
      setTimeout(() => setCopied(false), 2000)
    } catch {
      toast.error("复制失败，请手动选择文本")
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle>本地电脑</DialogTitle>
          <DialogDescription>
            在自己的电脑上运行桌面客户端，即可把工作任务派到那台机器执行。
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4">
          <div className="flex gap-2">
            <Input
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="设备名称（可留空）"
              className="h-9"
            />
            <Button onClick={() => void pair()} disabled={pairing} className="shrink-0">
              {pairing && <Loader2 className="size-3.5 animate-spin" />}
              生成配对码
            </Button>
          </div>

          {code && (
            <div className="rounded-lg border border-primary/40 bg-primary/5 p-3">
              <p className="text-xs text-muted-foreground">
                配对码只显示一次，请立即复制并填入桌面客户端：
              </p>
              <div className="mt-2 flex items-center gap-2">
                <code className="min-w-0 flex-1 truncate rounded bg-muted px-2 py-1.5 font-mono text-xs">
                  {code}
                </code>
                <Button size="sm" variant="outline" onClick={() => void copy()}>
                  {copied ? <Check className="size-3.5" /> : <Copy className="size-3.5" />}
                  {copied ? "已复制" : "复制"}
                </Button>
              </div>
              <pre className="mt-2 overflow-auto rounded bg-muted p-2 text-[11px] leading-relaxed">
{`YUNOVA_DEVICE_URL=${window.location.origin}
YUNOVA_DEVICE_TOKEN=${code}
YUNOVA_DEVICE_WORKSPACE=<Agent 可操作的目录>
./yunova-desktop`}
              </pre>
            </div>
          )}

          <div className="space-y-2">
            {loading && (
              <div className="flex items-center gap-2 text-xs text-muted-foreground">
                <Loader2 className="size-3.5 animate-spin" />
                加载中…
              </div>
            )}
            {!loading && devices.filter((d) => !d.revoked).length === 0 && (
              <p className="text-xs text-muted-foreground">
                还没有绑定电脑。生成配对码后在目标机器上运行桌面客户端即可。
              </p>
            )}
            {devices
              .filter((d) => !d.revoked)
              .map((d) => (
                <div
                  key={d.id}
                  className="flex items-center gap-2 rounded-lg border px-3 py-2 text-sm"
                >
                  <Laptop className="size-4 shrink-0 text-muted-foreground" />
                  <span className="min-w-0 flex-1 truncate">
                    {d.name}
                    {d.platform && (
                      <span className="ml-1.5 text-xs text-muted-foreground">{d.platform}</span>
                    )}
                  </span>
                  <span
                    className={
                      d.online
                        ? "text-xs text-primary"
                        : "text-xs text-muted-foreground"
                    }
                  >
                    {d.online ? "在线" : "离线"}
                  </span>
                  <Button
                    size="icon-xs"
                    variant="ghost"
                    onClick={() => void remove(d)}
                    title="移除"
                  >
                    <Trash2 />
                  </Button>
                </div>
              ))}
          </div>

          <p className="text-xs text-muted-foreground">
            桌面客户端默认逐条确认命令，审批请求会推送到所有登录端，任一端处理即生效。
            工作目录决定 Agent 能改动的范围，请指向具体项目而不是整个用户目录。
          </p>
        </div>
      </DialogContent>
    </Dialog>
  )
}
