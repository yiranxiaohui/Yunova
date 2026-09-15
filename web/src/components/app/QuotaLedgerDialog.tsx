import { useCallback, useEffect, useState } from "react"
import { Receipt, RefreshCw } from "lucide-react"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { formatQuota, quotaApi, type LedgerEntry } from "@/lib/quota"
import { StatsView } from "./StatsView"

type Props = {
  open: boolean
  onClose: () => void
}

const PAGE_SIZE = 50 // matches the backend default; if backend changes, "load more" still works

function formatTime(s: string): string {
  // Backend writes either "YYYY-MM-DD HH:MM:SS" (SQLite/MySQL) or ISO (Postgres).
  // Parse both, fall back to raw on error.
  const candidate = s.includes("T") ? s : s.replace(" ", "T")
  const withZ = candidate.endsWith("Z") ? candidate : `${candidate}Z`
  const d = new Date(withZ)
  if (Number.isNaN(d.getTime())) return s
  return d.toLocaleString()
}

const PROTOCOL_LABEL: Record<string, string> = {
  openai: "OpenAI",
  claude: "Claude",
  gemini: "Gemini",
}

/** Map a backend ledger reason string to a Chinese-friendly description.
 *  Falls back to the raw text on unknown shapes. The raw value is still
 *  surfaced via `title=` on the cell, for debugging / cross-referencing. */
function prettifyReason(raw: string): string {
  // signup
  if (raw === "signup_grant") return "注册赠送"

  // invites
  if (raw === "invite_reward_invitee") return "受邀奖励"
  const inviter = raw.match(/^invite_reward_inviter:(.+)$/)
  if (inviter) return `邀请奖励(被邀: ${inviter[1]})`

  // recharge
  if (raw.startsWith("epay_recharge:")) {
    return `充值 · 订单 ${raw.slice("epay_recharge:".length)}`
  }

  // refunds — must come BEFORE chat_/image_ matching since they all start "refund_"
  if (raw === "refund_chain_exhausted") return "退款 · 对话 · 全渠道失败(旧记录)"
  if (raw === "refund_upstream_error") return "退款 · 上游错误(旧记录)"
  if (raw === "refund_job_create_error") return "退款 · 任务创建失败(旧记录)"
  if (raw === "refund_studio_error") return "退款 · 工作室"
  // 对话改为按 token 事后结算后不再产生退款，这些只会出现在历史记录里。
  if (raw.startsWith("refund_chat_")) {
    const rest = raw.slice("refund_chat_".length)
    if (rest.endsWith("_all_failed")) {
      return `退款 · 对话 · ${rest.slice(0, -"_all_failed".length)} · 全渠道失败`
    }
    if (rest.endsWith("_stream_empty")) {
      return `退款 · 对话 · ${rest.slice(0, -"_stream_empty".length)} · 流为空`
    }
    return `退款 · 对话 · ${rest}`
  }
  if (raw.startsWith("refund_video_")) {
    const rest = raw.slice("refund_video_".length)
    if (rest.endsWith("_no_channel")) {
      return `退款 · 视频 · ${rest.slice(0, -"_no_channel".length)} · 无可用渠道`
    }
    return `退款 · 视频 · ${rest}`
  }
  if (raw.startsWith("refund_image_")) {
    const rest = raw.slice("refund_image_".length)
    const SUFFIXES: Array<[string, string]> = [
      ["_upstream_error", "上游错误"],
      ["_job_create_error", "任务创建失败"],
    ]
    for (const [suf, label] of SUFFIXES) {
      if (rest.endsWith(suf)) {
        return `退款 · 图像 · ${rest.slice(0, -suf.length)} · ${label}`
      }
    }
    return `退款 · 图像 · ${rest}`
  }

  // chat / image — after refunds. Two shapes coexist:
  //   "chat_<protocol>" (legacy) and "chat_<model>@<channel>" (current).
  if (raw.startsWith("chat_")) {
    const rest = raw.slice("chat_".length)
    if (rest.includes("@")) {
      const [model, channel] = rest.split("@")
      return `对话 · ${model} · ${channel}`
    }
    return `对话 · ${PROTOCOL_LABEL[rest] ?? rest}`
  }
  if (raw.startsWith("image_")) {
    const rest = raw.slice("image_".length)
    if (rest.includes("@")) {
      const [model, channel] = rest.split("@")
      return `图像 · ${model} · ${channel}`
    }
    return `图像 · ${PROTOCOL_LABEL[rest] ?? rest}`
  }
  if (raw.startsWith("video_")) return `视频 · ${raw.slice("video_".length)}`
  // Legacy rows from the removed worker feature. The feature is gone, but its
  // ledger entries stay, so keep the label instead of showing a raw reason.
  if (raw === "worker_agent") return "远程执行（已下线）"
  if (raw === "worker_compact") return "远程执行 · 压缩上下文（已下线）"
  if (raw === "studio_generate") return "工作室生图"

  // admin manual adjustments (admin-supplied reasons get the literal text)
  if (raw.startsWith("admin_")) return `管理员调整 · ${raw.slice("admin_".length)}`

  // unknown — show raw
  return raw
}

/** 列表里的 token 摘要：没有 token 的条目（充值、赠送、生图）留空。 */
function summarizeTokens(e: LedgerEntry): string {
  if (e.input_tokens <= 0 && e.output_tokens <= 0) return "—"
  const fmt = (n: number) => n.toLocaleString("zh-CN")
  return `${fmt(e.input_tokens)}↑ ${fmt(e.output_tokens)}↓`
}

/** 悬停时展示完整分解，含命中缓存的输入 token。 */
function describeTokens(e: LedgerEntry): string {
  if (e.input_tokens <= 0 && e.output_tokens <= 0) return ""
  const parts = [`输入 ${e.input_tokens}`, `输出 ${e.output_tokens}`]
  if (e.cached_tokens > 0) parts.push(`其中缓存命中 ${e.cached_tokens}`)
  return parts.join(" · ")
}

type Tab = "stats" | "ledger"

export function QuotaLedgerDialog({ open, onClose }: Props) {
  const [tab, setTab] = useState<Tab>("stats")
  const [entries, setEntries] = useState<LedgerEntry[]>([])
  const [page, setPage] = useState(1)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [hasMore, setHasMore] = useState(true)

  const statsLoader = useCallback(quotaApi.stats, [])

  useEffect(() => {
    if (!open) return
    setEntries([])
    setError(null)
    setPage(1)
    setHasMore(true)
    setTab("stats")
    void loadPage(1, true)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open])

  async function loadPage(p: number, reset: boolean) {
    setLoading(true)
    setError(null)
    try {
      const rows = await quotaApi.ledger(p)
      setEntries((prev) => (reset ? rows : [...prev, ...rows]))
      // If the server returned fewer than a typical page, assume we hit the end.
      setHasMore(rows.length >= PAGE_SIZE)
      setPage(p)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={(next) => !next && onClose()}>
      {/* 右上角已有刷新按钮，关掉自带的 X 免得两者叠在一起；底部另有「关闭」 */}
      <DialogContent
        showCloseButton={false}
        className="flex max-h-[90vh] flex-col gap-3 p-4 sm:max-h-[80vh] sm:max-w-2xl sm:p-6"
      >
        <DialogHeader className="flex-row items-start justify-between gap-2 text-left">
          <div>
            <DialogTitle className="flex items-center gap-2">
              <Receipt className="size-5" /> 额度中心
            </DialogTitle>
            <DialogDescription className="text-xs">
              {tab === "stats"
                ? "看一眼这段时间花在哪里、剩下多少。"
                : "每次扣费 / 退款 / 充值 / 邀请奖励都会在这里记录。"}
            </DialogDescription>
          </div>
          {tab === "ledger" && (
            <Button
              variant="ghost"
              size="sm"
              onClick={() => void loadPage(1, true)}
              disabled={loading}
              title="刷新"
            >
              <RefreshCw className={"size-4 " + (loading ? "animate-spin" : "")} />
            </Button>
          )}
        </DialogHeader>

        <Tabs
          value={tab}
          onValueChange={(v) => setTab(v as "stats" | "ledger")}
          className="min-h-0 flex-1 gap-3"
        >
          <TabsList className="self-start">
            <TabsTrigger value="stats">统计</TabsTrigger>
            <TabsTrigger value="ledger">明细</TabsTrigger>
          </TabsList>

        {error && tab === "ledger" && (
          <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
            {error}
          </div>
        )}

          <TabsContent
            value="stats"
            className="nc-scroll min-h-[200px] overflow-y-auto"
          >
            <StatsView loader={statsLoader} active={open && tab === "stats"} />
          </TabsContent>

          <TabsContent
            value="ledger"
            className="nc-scroll min-h-[200px] overflow-y-auto"
          >
            <div className="rounded-md border border-border">
              {entries.length === 0 && !loading ? (
                <p className="px-3 py-10 text-center text-sm text-muted-foreground">
                  {error ? "加载失败" : "还没有额度变动记录"}
                </p>
              ) : (
                <table className="w-full text-sm">
                  <thead className="sticky top-0 bg-muted/50 text-xs text-muted-foreground">
                    <tr>
                      <th className="px-3 py-2 text-left font-medium">时间</th>
                      <th className="px-3 py-2 text-right font-medium">变动</th>
                      <th className="px-3 py-2 text-right font-medium">Tokens</th>
                      <th className="px-3 py-2 text-left font-medium">原因</th>
                    </tr>
                  </thead>
                  <tbody>
                    {entries.map((e) => (
                      <tr key={e.id} className="border-t border-border">
                        <td className="whitespace-nowrap px-3 py-1.5 text-xs tabular-nums text-muted-foreground">
                          {formatTime(e.created_at)}
                        </td>
                        <td
                          className={
                            "whitespace-nowrap px-3 py-1.5 text-right tabular-nums font-medium " +
                            (e.delta > 0
                              ? "text-emerald-600 dark:text-emerald-400"
                              : e.delta < 0
                                ? "text-rose-600 dark:text-rose-400"
                                : "text-muted-foreground")
                          }
                        >
                          {e.delta > 0 ? "+" : ""}
                          {formatQuota(e.delta)}
                        </td>
                        <td
                          className="whitespace-nowrap px-3 py-1.5 text-right text-xs tabular-nums text-muted-foreground"
                          title={describeTokens(e)}
                        >
                          {summarizeTokens(e)}
                        </td>
                        <td
                          className="break-all px-3 py-1.5 text-xs"
                          title={e.reason}
                        >
                          {prettifyReason(e.reason)}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
            </div>
          </TabsContent>
        </Tabs>

        <div className="flex items-center justify-between gap-2">
          <span className="text-xs text-muted-foreground">
            {tab === "ledger" ? `已加载 ${entries.length} 条` : ""}
          </span>
          <div className="flex gap-2">
            {tab === "ledger" && hasMore && (
              <Button
                variant="outline"
                size="sm"
                onClick={() => void loadPage(page + 1, false)}
                disabled={loading}
              >
                {loading ? "加载中…" : "加载更多"}
              </Button>
            )}
            <Button variant="outline" size="sm" onClick={onClose}>
              关闭
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  )
}
