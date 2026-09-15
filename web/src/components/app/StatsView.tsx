import { useEffect, useMemo, useState } from "react"
import { RefreshCw } from "lucide-react"
import { Button } from "@/components/ui/button"
import { cn } from "@/lib/utils"
import {
  formatQuota,
  type AdminQuotaStats,
  type DailyPoint,
  type ModelBucket,
  type QuotaStats,
  type StatsPeriod,
  type TopUserRow,
} from "@/lib/quota"

type Loader = (period: StatsPeriod) => Promise<QuotaStats | AdminQuotaStats>

type Props = {
  loader: Loader
  showTopUsers?: boolean
  /** When true (the default), reload whenever this view becomes visible. */
  active?: boolean
}

const PERIODS: { value: StatsPeriod; label: string }[] = [
  { value: "7d", label: "7 天" },
  { value: "30d", label: "30 天" },
  { value: "90d", label: "90 天" },
  { value: "all", label: "全部" },
]

const PROTOCOL_LABEL: Record<string, string> = {
  openai: "OpenAI",
  claude: "Claude",
  gemini: "Gemini",
}

const KIND_LABEL: Record<string, string> = {
  chat: "对话",
  image: "图像",
  video: "视频",
}

export function StatsView({ loader, showTopUsers, active = true }: Props) {
  const [period, setPeriod] = useState<StatsPeriod>("7d")
  const [data, setData] = useState<QuotaStats | AdminQuotaStats | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (!active) return
    let cancelled = false
    setLoading(true)
    setError(null)
    loader(period)
      .then((d) => {
        if (!cancelled) setData(d)
      })
      .catch((e) => {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e))
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [period, active, loader])

  const topUsers = (data as AdminQuotaStats | null)?.top_users ?? []

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center gap-2">
        <div className="inline-flex rounded-md border border-border p-0.5">
          {PERIODS.map((p) => (
            <button
              key={p.value}
              type="button"
              onClick={() => setPeriod(p.value)}
              className={cn(
                "rounded px-3 py-1 text-xs transition-colors",
                period === p.value
                  ? "bg-primary text-primary-foreground"
                  : "text-muted-foreground hover:bg-accent"
              )}
            >
              {p.label}
            </button>
          ))}
        </div>
        <Button
          variant="ghost"
          size="sm"
          onClick={() => setPeriod((p) => p)}
          disabled={loading}
          title="刷新"
        >
          <RefreshCw className={"size-4 " + (loading ? "animate-spin" : "")} />
        </Button>
      </div>

      {error && (
        <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
          {error}
        </div>
      )}

      {!data && !error && (
        <p className="text-sm text-muted-foreground">{loading ? "加载中…" : "暂无数据"}</p>
      )}

      {data && (
        <>
          <SummaryCards data={data} />
          <Section title="每日消费">
            <DailyBarChart daily={data.daily} />
          </Section>
          <Section title={`按模型消费（前 ${data.by_model.length || 0} 名）`}>
            {data.by_model.length === 0 ? (
              <p className="text-xs text-muted-foreground">
                这段时间还没有按模型分类的消费记录。可能是历史数据（v0.22 之前）未记录模型字段，或本期没有走平台共享额度的请求。
              </p>
            ) : (
              <ModelBarList rows={data.by_model} />
            )}
          </Section>
          {showTopUsers && (
            <Section title="消费榜 · Top 20">
              {topUsers.length === 0 ? (
                <p className="text-xs text-muted-foreground">本期暂无用户消费记录</p>
              ) : (
                <TopUserList rows={topUsers} />
              )}
            </Section>
          )}
        </>
      )}
    </div>
  )
}

function SummaryCards({ data }: { data: QuotaStats }) {
  return (
    <div className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-6">
      <StatCard label="净消费" value={data.net_spent} tone="spend" />
      <StatCard label="总扣费" value={data.spent} />
      <StatCard label="退款" value={data.refunded} tone="positive" />
      <StatCard label="充值入账" value={data.recharged} tone="positive" />
      <StatCard label="赠送(注册/邀请)" value={data.granted} tone="positive" />
      <StatCard
        label="Tokens (入/出)"
        value={data.input_tokens + data.output_tokens}
        hint={`输入 ${formatQuota(data.input_tokens)} · 输出 ${formatQuota(data.output_tokens)}`}
      />
    </div>
  )
}

function StatCard({
  label,
  value,
  tone,
  hint,
}: {
  label: string
  value: number
  tone?: "spend" | "positive"
  hint?: string
}) {
  const color =
    tone === "spend"
      ? "text-rose-600 dark:text-rose-400"
      : tone === "positive"
        ? "text-emerald-600 dark:text-emerald-400"
        : "text-foreground"
  return (
    <div className="rounded-lg border border-border bg-card p-3 shadow-sm">
      <p className="text-[11px] uppercase tracking-wide text-muted-foreground">
        {label}
      </p>
      <p
        className={"mt-1 text-xl font-semibold tabular-nums " + color}
        title={hint}
      >
        {formatQuota(value)}
      </p>
      {hint && (
        <p className="mt-0.5 truncate text-[10px] text-muted-foreground">{hint}</p>
      )}
    </div>
  )
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="rounded-xl border border-border bg-card p-4 shadow-sm">
      <h3 className="mb-3 text-sm font-semibold">{title}</h3>
      {children}
    </div>
  )
}

/** Vertical bar chart over up to 90 days. Bars share equal width; height
 *  scales to the largest "spent" value in the series. Hover shows a tooltip
 *  with the raw counts. Returns a helpful message when the series is empty. */
function DailyBarChart({ daily }: { daily: DailyPoint[] }) {
  const max = useMemo(
    () => daily.reduce((acc, d) => Math.max(acc, d.spent, d.refunded), 0),
    [daily]
  )
  if (daily.length === 0) {
    return <p className="text-xs text-muted-foreground">本期暂无消费</p>
  }

  const width = 100 // viewBox width in %; rely on container to size
  const height = 100
  const barW = width / daily.length
  // Squeeze the bar a bit so adjacent bars have visible gap.
  const innerW = barW * 0.75
  const offset = (barW - innerW) / 2

  return (
    <div className="flex flex-col gap-2">
      <svg
        viewBox={`0 0 ${width} ${height}`}
        className="h-32 w-full"
        preserveAspectRatio="none"
      >
        {daily.map((d, i) => {
          const x = i * barW + offset
          const spentH = max > 0 ? (d.spent / max) * height : 0
          const refundH = max > 0 ? (d.refunded / max) * height : 0
          return (
            <g key={d.date}>
              <title>{`${d.date}\n扣费 ${formatQuota(d.spent)}\n退款 ${formatQuota(d.refunded)}`}</title>
              <rect
                x={x}
                y={height - spentH}
                width={innerW}
                height={spentH}
                className="fill-rose-500/70"
              />
              {refundH > 0 && (
                <rect
                  x={x}
                  y={height - spentH - refundH}
                  width={innerW}
                  height={refundH}
                  className="fill-emerald-500/70"
                />
              )}
            </g>
          )
        })}
      </svg>
      <div className="flex items-center justify-between text-[10px] text-muted-foreground">
        <span>{daily[0]?.date}</span>
        <span>
          <span className="inline-block size-2 rounded-sm bg-rose-500/70" /> 扣费{" "}
          <span className="ml-2 inline-block size-2 rounded-sm bg-emerald-500/70" />{" "}
          退款
        </span>
        <span>{daily[daily.length - 1]?.date}</span>
      </div>
    </div>
  )
}

function ModelBarList({ rows }: { rows: ModelBucket[] }) {
  const max = rows.reduce((acc, r) => Math.max(acc, r.spent), 0) || 1
  return (
    <ul className="flex flex-col gap-1.5">
      {rows.map((r, i) => {
        const pct = (r.spent / max) * 100
        return (
          <li key={`${r.model}-${r.kind}-${i}`} className="text-sm">
            <div className="flex items-center justify-between gap-2">
              <div className="flex min-w-0 items-center gap-1.5">
                <span className="inline-block rounded-sm bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground">
                  {KIND_LABEL[r.kind] ?? r.kind}
                </span>
                {r.protocol && (
                  <span className="text-[10px] text-muted-foreground">
                    {PROTOCOL_LABEL[r.protocol] ?? r.protocol}
                  </span>
                )}
                <span className="truncate font-mono text-xs">{r.model}</span>
              </div>
              <div
                className="flex shrink-0 items-center gap-2 text-xs tabular-nums"
                title={
                  r.input_tokens > 0 || r.output_tokens > 0
                    ? `输入 ${formatQuota(r.input_tokens)} tokens · 输出 ${formatQuota(r.output_tokens)} tokens`
                    : undefined
                }
              >
                <span className="text-muted-foreground">{r.count} 次</span>
                <span className="font-medium">{formatQuota(r.spent)}</span>
                {r.refunded > 0 && (
                  <span className="text-emerald-600 dark:text-emerald-400">
                    -{formatQuota(r.refunded)}
                  </span>
                )}
              </div>
            </div>
            <div className="mt-1 h-1.5 overflow-hidden rounded bg-muted">
              <div
                className="h-full bg-rose-500/70"
                style={{ width: `${pct}%` }}
              />
            </div>
          </li>
        )
      })}
    </ul>
  )
}

function TopUserList({ rows }: { rows: TopUserRow[] }) {
  const max = rows.reduce((acc, r) => Math.max(acc, r.spent), 0) || 1
  return (
    <ul className="flex flex-col gap-1.5">
      {rows.map((r, i) => {
        const pct = (r.spent / max) * 100
        return (
          <li key={r.user_id} className="text-sm">
            <div className="flex items-center justify-between gap-2">
              <div className="flex min-w-0 items-center gap-1.5">
                <span className="w-5 text-right font-mono text-[10px] text-muted-foreground">
                  {i + 1}
                </span>
                <span className="truncate font-medium">{r.username}</span>
                <span className="text-[10px] text-muted-foreground">
                  #{r.user_id}
                </span>
              </div>
              <div className="flex shrink-0 items-center gap-2 text-xs tabular-nums">
                <span className="font-medium">{formatQuota(r.spent)}</span>
                {r.refunded > 0 && (
                  <span className="text-emerald-600 dark:text-emerald-400">
                    -{formatQuota(r.refunded)}
                  </span>
                )}
              </div>
            </div>
            <div className="mt-1 h-1.5 overflow-hidden rounded bg-muted">
              <div
                className="h-full bg-rose-500/70"
                style={{ width: `${pct}%` }}
              />
            </div>
          </li>
        )
      })}
    </ul>
  )
}
