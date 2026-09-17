import { useEffect, useState } from "react"
import { CloudDownload, LoaderCircle, TriangleAlert } from "lucide-react"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import {
  channelsAdminApi,
  type Channel,
  type NewApiSyncResult,
} from "@/lib/channels"
import {
  adminQuotaApi,
  formatQuota,
  microQuotaForMicroUsd,
  DEFAULT_USD_TO_CNY_RATE_MICRO,
} from "@/lib/quota"

/**
 * 从 NewAPI 站点导入模型价格。
 *
 * 默认走「预演 → 复核 → 写入」三步：先 dry-run 看清会写什么，确认后再落库。
 * 导入的模型默认<b>停用</b>，避免一次拉进上百个模型后立刻开始计费。
 *
 * 中转站通常列出几百个模型，而站点只卖其中一小部分，因此默认只同步<b>本地已添加</b>
 * 的模型：上游有、本地没有的直接忽略，已添加的按上游价重新计价。取消勾选才会把
 * 整个目录拉进来。
 */
export function NewApiImportDialog({
  open,
  onClose,
  onImported,
}: {
  open: boolean
  onClose: () => void
  onImported: () => void | Promise<void>
}) {
  const [baseUrl, setBaseUrl] = useState("")
  const [group, setGroup] = useState("")
  const [channelId, setChannelId] = useState<number | null>(null)
  const [existingOnly, setExistingOnly] = useState(true)
  const [overwrite, setOverwrite] = useState(false)
  const [enableImported, setEnableImported] = useState(false)
  const [channels, setChannels] = useState<Channel[]>([])
  const [preview, setPreview] = useState<NewApiSyncResult | null>(null)
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState<string | null>(null)
  const [done, setDone] = useState<NewApiSyncResult | null>(null)
  const [usdToCnyRateMicro, setUsdToCnyRateMicro] = useState(
    DEFAULT_USD_TO_CNY_RATE_MICRO
  )
  const [multiplier, setMultiplier] = useState(100)

  useEffect(() => {
    if (!open) return
    let cancelled = false
    void channelsAdminApi
      .listChannels()
      .then((rows) => {
        if (!cancelled) setChannels(rows.filter((c) => c.enabled))
      })
      .catch(() => {})
    void adminQuotaApi
      .getSettings()
      .then((s) => {
        if (cancelled) return
        setUsdToCnyRateMicro(s.usd_to_cny_rate_micro)
        setMultiplier(s.price_multiplier_percent)
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [open])

  function reset() {
    setPreview(null)
    setDone(null)
    setErr(null)
  }

  async function run(dryRun: boolean) {
    if (!baseUrl.trim()) {
      setErr("请填写 NewAPI 站点地址")
      return
    }
    setBusy(true)
    setErr(null)
    try {
      const result = await channelsAdminApi.syncNewApiPricing({
        base_url: baseUrl.trim(),
        group: group.trim() || undefined,
        channel_ids: channelId != null ? [channelId] : undefined,
        dry_run: dryRun,
        existing_only: existingOnly,
        overwrite_existing: overwrite,
        enable_imported: enableImported,
      })
      if (dryRun) {
        setPreview(result)
        setDone(null)
      } else {
        setDone(result)
        setPreview(null)
        await onImported()
      }
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  const shown = done ?? preview

  return (
    <Dialog
      open={open}
      onOpenChange={(v) => {
        if (!v) {
          reset()
          onClose()
        }
      }}
    >
      <DialogContent className="max-h-[90vh] gap-0 overflow-hidden p-0 sm:max-w-2xl">
        <DialogHeader className="border-b border-border px-5 py-4">
          <DialogTitle className="flex items-center gap-2">
            <CloudDownload className="size-4" /> 从 NewAPI 导入价格
          </DialogTitle>
          <DialogDescription>
            读取上游站点的 <code>/api/pricing</code>，把倍率换算成官方美元价后写入模型计费。
            默认<b>只刷新本地已添加的模型</b>，上游多出来的模型不会被拉进来。
          </DialogDescription>
        </DialogHeader>

        <div className="nc-scroll max-h-[60vh] overflow-y-auto px-5 py-4">
          <div className="flex flex-col gap-3">
            <div>
              <Label>NewAPI 站点地址</Label>
              <Input
                value={baseUrl}
                onChange={(e) => {
                  setBaseUrl(e.target.value)
                  reset()
                }}
                placeholder="https://your-newapi.example.com"
              />
              <p className="mt-1 text-[11px] text-muted-foreground">
                填站点根地址即可，会自动补 <code>/api/pricing</code>。该接口通常允许匿名访问。
              </p>
            </div>

            <div className="grid grid-cols-2 gap-3">
              <div>
                <Label>分组（可选）</Label>
                <Input
                  value={group}
                  onChange={(e) => {
                    setGroup(e.target.value)
                    reset()
                  }}
                  placeholder="留空＝全部分组"
                />
                <p className="mt-1 text-[11px] text-muted-foreground">
                  只导入该分组开放的模型，例如 <code>Claude</code>。
                </p>
              </div>
              <div>
                <Label>绑定渠道（可选）</Label>
                <select
                  className="h-9 w-full rounded-md border border-input bg-transparent px-3 text-sm"
                  value={channelId ?? ""}
                  onChange={(e) =>
                    setChannelId(e.target.value ? Number(e.target.value) : null)
                  }
                >
                  <option value="">不绑定</option>
                  {channels.map((c) => (
                    <option key={c.id} value={c.id}>
                      {c.name}（{c.protocol}）
                    </option>
                  ))}
                </select>
                <p className="mt-1 text-[11px] text-muted-foreground">
                  把导入的模型绑定到该渠道，省去逐个绑定。
                </p>
              </div>
            </div>

            <label className="flex items-start gap-2 text-sm">
              <input
                type="checkbox"
                className="mt-0.5 size-4 accent-primary"
                checked={existingOnly}
                onChange={(e) => {
                  setExistingOnly(e.target.checked)
                  reset()
                }}
              />
              <span>
                只同步本地已添加的模型（推荐）
                <span className="block text-[11px] text-muted-foreground">
                  上游有、本站未添加的模型直接忽略；已添加的按上游价重新计价，
                  启用状态、显示名与渠道绑定保持不变。取消勾选会导入整个目录。
                </span>
              </span>
            </label>
            <label
              className={
                "flex items-center gap-2 text-sm " +
                (existingOnly ? "opacity-45" : "")
              }
            >
              <input
                type="checkbox"
                className="size-4 accent-primary"
                checked={overwrite || existingOnly}
                disabled={existingOnly}
                onChange={(e) => {
                  setOverwrite(e.target.checked)
                  reset()
                }}
              />
              覆盖已存在的模型价格（默认跳过，保护手工调整过的价格）
            </label>
            <label
              className={
                "flex items-center gap-2 text-sm " +
                (existingOnly ? "opacity-45" : "")
              }
            >
              <input
                type="checkbox"
                className="size-4 accent-primary"
                checked={enableImported}
                disabled={existingOnly}
                onChange={(e) => {
                  setEnableImported(e.target.checked)
                  reset()
                }}
              />
              导入后立即启用（默认停用，便于先复核价格）
            </label>

            {err && (
              <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
                {err}
              </div>
            )}

            {shown && (
              <div className="rounded-lg border border-border">
                <div className="flex flex-wrap items-center gap-3 border-b border-border bg-muted/30 px-3 py-2 text-xs">
                  <span className="font-medium">
                    {shown.dry_run
                      ? "预演结果"
                      : existingOnly
                        ? "同步完成"
                        : "导入完成"}
                  </span>
                  <span className="text-muted-foreground">
                    上游 {shown.fetched} 个
                  </span>
                  <span className="text-emerald-600 dark:text-emerald-400">
                    新增 {shown.imported}
                  </span>
                  <span className="text-amber-600 dark:text-amber-400">
                    更新 {shown.updated}
                  </span>
                  <span className="text-muted-foreground">
                    跳过 {shown.skipped_existing}
                  </span>
                  {shown.skipped_missing > 0 && (
                    <span className="text-muted-foreground">
                      未添加·忽略 {shown.skipped_missing}
                    </span>
                  )}
                  {shown.skipped_unsupported > 0 && (
                    <span className="text-amber-600 dark:text-amber-400">
                      不兼容 {shown.skipped_unsupported}
                    </span>
                  )}
                </div>

                {shown.not_listed.length > 0 && (
                  <div className="border-b border-border bg-muted/20 px-3 py-2 text-[11px] text-muted-foreground">
                    上游目录里没有以下已添加模型，价格保持不变：
                    <span className="font-mono">
                      {" "}
                      {shown.not_listed.slice(0, 12).join("、")}
                    </span>
                    {shown.not_listed.length > 12 &&
                      ` …另有 ${shown.not_listed.length - 12} 个`}
                  </div>
                )}

                {shown.warnings.length > 0 && (
                  <div className="border-b border-border bg-amber-500/10 px-3 py-2 text-[11px] text-amber-700 dark:text-amber-400">
                    <p className="mb-1 flex items-center gap-1 font-medium">
                      <TriangleAlert className="size-3" /> 注意
                    </p>
                    <ul className="list-inside list-disc">
                      {shown.warnings.slice(0, 8).map((w) => (
                        <li key={w}>{w}</li>
                      ))}
                      {shown.warnings.length > 8 && (
                        <li>…另有 {shown.warnings.length - 8} 条</li>
                      )}
                    </ul>
                  </div>
                )}

                <div className="nc-scroll max-h-56 overflow-y-auto">
                  {shown.models.length === 0 ? (
                    <p className="px-3 py-3 text-xs text-muted-foreground">
                      上游目录与本地已添加的模型没有任何交集，没有需要同步的价格。
                    </p>
                  ) : (
                  <table className="w-full text-xs">
                    <thead className="sticky top-0 bg-muted/50 text-muted-foreground">
                      <tr>
                        <th className="px-3 py-1.5 text-left font-medium">模型</th>
                        <th className="px-3 py-1.5 text-left font-medium">功能</th>
                        <th className="px-3 py-1.5 text-right font-medium">
                          价格（美元）
                        </th>
                        <th className="px-3 py-1.5 text-right font-medium">
                          折算（元）
                        </th>
                      </tr>
                    </thead>
                    <tbody>
                      {shown.models.map((m) => (
                        <tr
                          key={m.model}
                          className={
                            "border-t border-border " +
                            (m.applied ? "" : "opacity-45")
                          }
                        >
                          <td className="px-3 py-1 font-mono">
                            {m.model}
                            {!m.applied && (
                              <span className="ml-1 text-[10px] text-muted-foreground">
                                已存在·跳过
                              </span>
                            )}
                          </td>
                          <td className="px-3 py-1 text-muted-foreground">
                            {m.kind}
                          </td>
                          <td className="px-3 py-1 text-right tabular-nums">
                            {describeUsd(m)}
                          </td>
                          <td className="px-3 py-1 text-right tabular-nums text-muted-foreground">
                            {describeQuota(m, usdToCnyRateMicro, multiplier)}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                  )}
                </div>
              </div>
            )}
          </div>
        </div>

        <DialogFooter className="border-t border-border px-5 py-3">
          <Button variant="outline" onClick={onClose} disabled={busy}>
            关闭
          </Button>
          <Button
            variant="outline"
            onClick={() => void run(true)}
            disabled={busy || !baseUrl.trim()}
          >
            {busy && <LoaderCircle className="animate-spin" />} 预演
          </Button>
          <Button
            onClick={() => void run(false)}
            disabled={busy || !baseUrl.trim()}
          >
            {busy && <LoaderCircle className="animate-spin" />}{" "}
            {existingOnly ? "同步" : "导入"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function usd(micro: number): string {
  return `$${Number((micro / 1_000_000).toFixed(6))}`
}

function describeUsd(m: {
  kind: string
  input_price: number
  output_price: number
  per_call_price: number
}): string {
  if (m.kind === "image") {
    return m.per_call_price > 0 ? `${usd(m.per_call_price)}/次` : "免费"
  }
  if (m.input_price === 0 && m.output_price === 0) return "免费"
  return `${usd(m.input_price)} / ${usd(m.output_price)} 每1M`
}

function describeQuota(
  m: { kind: string; input_price: number; output_price: number; per_call_price: number },
  usdToCnyRateMicro: number,
  multiplier: number
): string {
  const q = (micro: number) =>
    formatQuota(microQuotaForMicroUsd(micro, usdToCnyRateMicro, multiplier))
  if (m.kind === "image") {
    return m.per_call_price > 0 ? `${q(m.per_call_price)}/次` : "—"
  }
  if (m.input_price === 0 && m.output_price === 0) return "—"
  return `${q(m.input_price)} / ${q(m.output_price)}`
}
