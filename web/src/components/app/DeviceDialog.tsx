import { useCallback, useEffect, useState } from "react"
import { Check, Copy, Laptop, Loader2, Pencil, Trash2 } from "lucide-react"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { useConfirm } from "@/lib/confirm-context"
import {
  listDevices,
  renameDevice,
  revokeDevice,
  type AgentDevice,
} from "@/lib/agent"
import { toast } from "sonner"

/**
 * The machines that can run tasks locally.
 *
 * There is nothing to create here. A computer attaches by signing in from the
 * desktop client, which is why this dialog is a list rather than a wizard: the
 * account already says whose machine it is, so the only things left are naming
 * it and taking it away again.
 */
export function DeviceDialog({
  open,
  onOpenChange,
}: {
  open: boolean
  onOpenChange: (v: boolean) => void
}) {
  const { confirm, prompt } = useConfirm()
  const [devices, setDevices] = useState<AgentDevice[]>([])
  const [loading, setLoading] = useState(true)
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
    // rather than showing a stale online state. This is also what makes the
    // list fill in by itself moments after the user signs in on the other
    // machine, with no button to press here. Kicked off via the timer's first
    // tick pattern so no setState runs synchronously in the effect.
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

  const rename = async (d: AgentDevice) => {
    const next = await prompt({
      title: "重命名设备",
      defaultValue: d.name,
      placeholder: "设备名称",
    })
    if (next == null) return
    const name = next.trim()
    if (!name || name === d.name) return
    try {
      await renameDevice(d.id, name)
      await load()
    } catch (e) {
      toast.error(`重命名失败：${(e as Error).message}`)
    }
  }

  const remove = async (d: AgentDevice) => {
    const ok = await confirm({
      title: `移除「${d.name}」？`,
      description:
        "该电脑会立即断开，本地保存的凭证失效；在那台电脑上重新登录即可再次绑定。已有任务记录会保留。",
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

  // The desktop app needs none of this: it knows its own address and binds
  // itself from the sign-in in its own window. What is left here is the
  // headless case — a server or a container with no window to sign in from —
  // which is the only place an address and an account still have to be given.
  const runSnippet = `YUNOVA_DEVICE_WORKSPACE=<Agent 可操作的目录> \\
YUNOVA_DEVICE_URL=${window.location.origin} \\
./yunova-desktop --headless`

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(runSnippet)
      setCopied(true)
      setTimeout(() => setCopied(false), 2000)
    } catch {
      toast.error("复制失败，请手动选择文本")
    }
  }

  const active = devices.filter((d) => !d.revoked)

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle>本地电脑</DialogTitle>
          <DialogDescription>
            在自己的电脑上安装并打开桌面客户端，在它的窗口里登录本账号，这台机器就会
            自动出现在下面的列表里——不用填地址，也不用配对码。还没有客户端？
            <a
              href="/download"
              target="_blank"
              rel="noreferrer"
              className="text-primary underline-offset-2 hover:underline"
            >
              前往下载
            </a>
            。
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4">
          <div className="rounded-lg border border-border bg-muted/50 p-3">
            <div className="flex items-center justify-between gap-2">
              <span className="text-xs text-muted-foreground">
                没有桌面环境的机器（服务器、容器）用无界面模式：
              </span>
              <Button size="sm" variant="ghost" onClick={() => void copy()}>
                {copied ? <Check className="size-3.5" /> : <Copy className="size-3.5" />}
                {copied ? "已复制" : "复制"}
              </Button>
            </div>
            <pre className="mt-2 overflow-auto rounded bg-background/70 p-2 text-[11px] leading-relaxed">
              {runSnippet}
            </pre>
          </div>

          <div className="space-y-2">
            {loading && (
              <div className="flex items-center gap-2 text-xs text-muted-foreground">
                <Loader2 className="size-3.5 animate-spin" />
                加载中…
              </div>
            )}
            {!loading && active.length === 0 && (
              <p className="text-xs text-muted-foreground">
                还没有绑定电脑。在目标机器上运行客户端并登录本账号，这里会自动出现。
              </p>
            )}
            {active.map((d) => (
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
                    d.online ? "text-xs text-primary" : "text-xs text-muted-foreground"
                  }
                >
                  {d.online ? "在线" : "离线"}
                </span>
                <Button
                  size="icon-xs"
                  variant="ghost"
                  onClick={() => void rename(d)}
                  title="重命名"
                >
                  <Pencil />
                </Button>
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
            客户端登录后只保存一枚设备令牌，不保存密码；移除设备即刻失效。
            桌面客户端默认逐条确认命令，审批请求会推送到所有登录端，任一端处理即生效。
            工作目录决定 Agent 能改动的范围，请指向具体项目而不是整个用户目录。
          </p>
        </div>
      </DialogContent>
    </Dialog>
  )
}
