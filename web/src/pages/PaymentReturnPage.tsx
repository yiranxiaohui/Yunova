import { useEffect, useState } from "react"
import { formatQuota } from "@/lib/quota"
import { Link, useSearchParams } from "react-router-dom"
import { ArrowLeft, CheckCircle2, Loader2, XCircle } from "lucide-react"
import { Button } from "@/components/ui/button"
import { paymentsApi, type PaymentOrder } from "@/lib/payments"

export default function PaymentReturnPage() {
  const [params] = useSearchParams()
  const outTradeNo = params.get("out_trade_no") ?? ""
  const [order, setOrder] = useState<PaymentOrder | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [attempt, setAttempt] = useState(0)

  useEffect(() => {
    if (!outTradeNo) {
      setError("未提供订单号")
      return
    }
    let cancelled = false
    const start = Date.now()
    const maxMs = 5 * 60 * 1000
    async function tick() {
      if (cancelled) return
      try {
        const o = await paymentsApi.get(outTradeNo)
        if (cancelled) return
        setOrder(o)
        if (o.status === "paid" || o.status === "failed") return
      } catch (e) {
        if (cancelled) return
        setError(e instanceof Error ? e.message : String(e))
      }
      if (Date.now() - start > maxMs) return
      setAttempt((n) => n + 1)
      setTimeout(() => void tick(), 3000)
    }
    void tick()
    return () => {
      cancelled = true
    }
  }, [outTradeNo])

  return (
    <div className="grid min-h-svh place-items-center bg-background p-4 text-foreground sm:p-6">
      <div className="flex w-full max-w-md flex-col items-center gap-4 rounded-lg border border-border bg-card p-6 shadow-sm sm:p-8">
        {error && !order && (
          <>
            <XCircle className="size-10 text-destructive" />
            <h1 className="text-lg font-semibold">无法查询订单</h1>
            <p className="text-sm text-muted-foreground">{error}</p>
          </>
        )}

        {!error && !order && (
          <>
            <Loader2 className="size-10 animate-spin text-muted-foreground" />
            <h1 className="text-lg font-semibold">正在查询支付结果…</h1>
            <p className="text-sm text-muted-foreground">
              订单号 <span className="font-mono">{outTradeNo || "-"}</span>
            </p>
          </>
        )}

        {order && order.status === "paid" && (
          <>
            <CheckCircle2 className="size-10 text-emerald-500" />
            <h1 className="text-lg font-semibold">支付成功</h1>
            <p className="text-sm text-muted-foreground">
              已到账 <b>{formatQuota(order.quota)}</b> 元额度（实付 ¥
              {(order.amount_cents / 100).toFixed(2)}）
            </p>
          </>
        )}

        {order && order.status === "pending" && (
          <>
            <Loader2 className="size-10 animate-spin text-amber-500" />
            <h1 className="text-lg font-semibold">等待支付结果</h1>
            <p className="text-sm text-muted-foreground">
              已等待 {attempt * 3} 秒，支付平台确认可能需要几秒到几分钟。
            </p>
          </>
        )}

        {order && order.status === "failed" && (
          <>
            <XCircle className="size-10 text-destructive" />
            <h1 className="text-lg font-semibold">订单已关闭</h1>
            <p className="text-sm text-muted-foreground">如有疑问，请联系管理员。</p>
          </>
        )}

        <Button variant="outline" asChild>
          <Link to="/">
            <ArrowLeft className="size-4" /> 返回首页
          </Link>
        </Button>
      </div>
    </div>
  )
}
