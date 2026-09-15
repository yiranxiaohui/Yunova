import { useEffect, useState } from "react"
import { Coins, ExternalLink, RefreshCw } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table"
import { paymentsApi, type PaymentOrder } from "@/lib/payments"

type Props = {
  open: boolean
  onClose: () => void
  onPaid?: () => void
}

type Payway = "alipay" | "wxpay"

const PRESET_YUAN = [10, 30, 50, 100, 200]

export function RechargeDialog({ open, onClose, onPaid }: Props) {
  const [yuan, setYuan] = useState<number>(10)
  const [payway, setPayway] = useState<Payway>("alipay")
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [history, setHistory] = useState<PaymentOrder[]>([])
  const [historyLoading, setHistoryLoading] = useState(false)

  useEffect(() => {
    if (!open) return
    setError(null)
    setSubmitting(false)
    void reloadHistory()
  }, [open])

  async function reloadHistory() {
    setHistoryLoading(true)
    try {
      setHistory(await paymentsApi.listMine())
    } catch {
      /* non-fatal */
    } finally {
      setHistoryLoading(false)
    }
  }

  async function submit() {
    setError(null)
    const n = Math.trunc(Number(yuan))
    if (!Number.isFinite(n) || n < 1) {
      setError("请填写有效金额")
      return
    }
    setSubmitting(true)
    try {
      const r = await paymentsApi.create(n, payway)
      // open the pay URL in a new tab, start polling for this order
      window.open(r.pay_url, "_blank", "noopener,noreferrer")
      void pollOrder(r.out_trade_no)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setSubmitting(false)
    }
  }

  async function pollOrder(outTradeNo: string) {
    const start = Date.now()
    const maxMs = 10 * 60 * 1000
    while (Date.now() - start < maxMs) {
      await new Promise((r) => setTimeout(r, 3000))
      try {
        const o = await paymentsApi.get(outTradeNo)
        if (o.status === "paid") {
          onPaid?.()
          void reloadHistory()
          return
        }
        if (o.status === "failed") return
      } catch {
        /* keep trying */
      }
    }
  }

  return (
    <Dialog open={open} onOpenChange={(next) => !next && onClose()}>
      <DialogContent className="flex flex-col gap-4">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <Coins className="size-5" /> 充值额度
          </DialogTitle>
          <DialogDescription>
            支付完成后，额度将在几秒内到账。若长时间未到账请联系管理员。
          </DialogDescription>
        </DialogHeader>

        {error && (
          <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
            {error}
          </div>
        )}

        <div className="flex flex-col gap-2">
          <Label>支付方式</Label>
          <div className="flex items-center rounded-md border border-border p-0.5">
            <Button
              size="sm"
              variant={payway === "alipay" ? "secondary" : "ghost"}
              className="flex-1"
              onClick={() => setPayway("alipay")}
            >
              支付宝
            </Button>
            <Button
              size="sm"
              variant={payway === "wxpay" ? "secondary" : "ghost"}
              className="flex-1"
              onClick={() => setPayway("wxpay")}
            >
              微信
            </Button>
          </div>
        </div>

        <div className="flex flex-col gap-2">
          <Label htmlFor="rc-yuan">充值金额（元）</Label>
          <div className="flex flex-wrap gap-2">
            {PRESET_YUAN.map((n) => (
              <Button
                key={n}
                size="sm"
                variant={yuan === n ? "secondary" : "outline"}
                onClick={() => setYuan(n)}
              >
                ¥{n}
              </Button>
            ))}
          </div>
          <Input
            id="rc-yuan"
            type="number"
            min="1"
            step="1"
            value={String(yuan)}
            onChange={(e) => {
              const n = Number(e.target.value)
              setYuan(Number.isFinite(n) ? Math.trunc(n) : 0)
            }}
          />
        </div>

        <div className="flex justify-end gap-2">
          <Button variant="outline" onClick={onClose} disabled={submitting}>
            关闭
          </Button>
          <Button onClick={() => void submit()} disabled={submitting || yuan < 1}>
            {submitting ? "下单中…" : (
              <>
                <ExternalLink className="size-4" /> 前往支付
              </>
            )}
          </Button>
        </div>

        <div className="border-t border-border pt-3">
          <div className="mb-2 flex items-center justify-between">
            <p className="text-xs font-semibold text-muted-foreground">最近订单</p>
            <Button
              size="sm"
              variant="ghost"
              onClick={() => void reloadHistory()}
              disabled={historyLoading}
            >
              <RefreshCw className="size-3" /> 刷新
            </Button>
          </div>
          {history.length === 0 ? (
            <p className="text-xs text-muted-foreground">暂无订单</p>
          ) : (
            <div className="nc-scroll max-h-48 overflow-y-auto rounded border border-border">
              {/* 表头/单元格的间距在父级统一设置，省得每个 Head/Cell 都写一遍 */}
              <Table className="text-xs">
                <TableHeader className="bg-muted/40 text-[10px] uppercase [&_th]:h-7 [&_th]:px-2 [&_th]:py-1 [&_th]:text-[10px] [&_th]:text-muted-foreground">
                  <TableRow>
                    <TableHead>订单号</TableHead>
                    <TableHead className="text-right">金额</TableHead>
                    <TableHead className="text-right">额度</TableHead>
                    <TableHead>状态</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody className="[&_td]:px-2 [&_td]:py-1">
                  {history.slice(0, 20).map((o) => (
                    <TableRow key={o.id}>
                      <TableCell className="font-mono text-[10px] text-muted-foreground">
                        {o.out_trade_no}
                      </TableCell>
                      <TableCell className="text-right tabular-nums">
                        ¥{(o.amount_cents / 100).toFixed(2)}
                      </TableCell>
                      <TableCell className="text-right tabular-nums">
                        +{o.quota}
                      </TableCell>
                      <TableCell>
                        <StatusPill status={o.status} />
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </div>
          )}
        </div>
      </DialogContent>
    </Dialog>
  )
}

function StatusPill({ status }: { status: PaymentOrder["status"] }) {
  const map: Record<PaymentOrder["status"], { label: string; cls: string }> = {
    pending: {
      label: "待支付",
      cls: "bg-amber-500/10 text-amber-600 dark:text-amber-400",
    },
    paid: {
      label: "已完成",
      cls: "bg-emerald-500/10 text-emerald-600 dark:text-emerald-400",
    },
    failed: {
      label: "已关闭",
      cls: "bg-destructive/10 text-destructive",
    },
  }
  const m = map[status]
  return (
    <span className={`inline-block rounded px-1.5 py-0.5 text-[10px] ${m.cls}`}>
      {m.label}
    </span>
  )
}
