// 站点额度（quota）API 客户端。
//
// 额度是本站自有单位，直接以整数展示，不带货币符号。模型价格由管理员照抄各家
// 官方美元价目表录入，后端按 `quota_per_usd` 与全局倍率换算成额度。
// 对话按 token 实际用量事后结算，生图按次、视频按秒。

export type QuotaMe = {
  balance: number
  lifetime_used: number
  /** 1 美元上游成本折算多少额度 */
  quota_per_usd: number
  /** 全局加价倍率，100 = 1.0 倍 */
  price_multiplier_percent: number
}

export type LedgerEntry = {
  id: number
  delta: number
  reason: string
  created_at: string
  kind: string | null
  model: string | null
  input_tokens: number
  output_tokens: number
  cached_tokens: number
}

async function jsonOrThrow<T>(res: Response): Promise<T> {
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText)
    throw new Error(text || `HTTP ${res.status}`)
  }
  return res.json() as Promise<T>
}

export type StatsPeriod = "7d" | "30d" | "90d" | "all"

export type DailyPoint = {
  date: string
  spent: number
  refunded: number
}

export type ModelBucket = {
  model: string
  kind: string
  protocol: string | null
  count: number
  spent: number
  refunded: number
  input_tokens: number
  output_tokens: number
}

export type KindBucket = {
  kind: string
  total: number
}

export type QuotaStats = {
  period: StatsPeriod
  start: string
  spent: number
  refunded: number
  net_spent: number
  granted: number
  recharged: number
  input_tokens: number
  output_tokens: number
  daily: DailyPoint[]
  by_model: ModelBucket[]
  by_kind: KindBucket[]
}

export type TopUserRow = {
  user_id: number
  username: string
  spent: number
  refunded: number
}

export type AdminQuotaStats = QuotaStats & { top_users: TopUserRow[] }

export const quotaApi = {
  async me(): Promise<QuotaMe> {
    return jsonOrThrow(
      await fetch("/api/quota/me", { credentials: "same-origin" })
    )
  },
  async ledger(page = 1): Promise<LedgerEntry[]> {
    return jsonOrThrow(
      await fetch(`/api/quota/ledger?page=${page}`, {
        credentials: "same-origin",
      })
    )
  },
  async stats(period: StatsPeriod = "7d"): Promise<QuotaStats> {
    return jsonOrThrow(
      await fetch(`/api/quota/stats?period=${period}`, {
        credentials: "same-origin",
      })
    )
  },
}

export type AdminSettings = {
  registration_enabled: boolean
  signup_grant: number
  quota_per_usd: number
  price_multiplier_percent: number
  invite_grant_inviter: number
  invite_grant_invitee: number
  email_verification_required: boolean
  smtp_host: string
  smtp_port: number
  smtp_username: string
  smtp_from_email: string
  smtp_from_name: string
  smtp_security: string
  smtp_password_set: boolean
}

export type AdminSettingsUpdate = Partial<
  Omit<AdminSettings, "smtp_password_set"> & {
    smtp_password: string
  }
>

export type AdminUserQuota = {
  user_id: number
  username: string
  balance: number
  lifetime_used: number
}

export const adminQuotaApi = {
  async getSettings(): Promise<AdminSettings> {
    return jsonOrThrow(
      await fetch("/api/admin/app-settings", { credentials: "same-origin" })
    )
  },
  async updateSettings(patch: AdminSettingsUpdate): Promise<void> {
    const res = await fetch("/api/admin/app-settings", {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(patch),
      credentials: "same-origin",
    })
    if (!res.ok) {
      throw new Error((await res.text().catch(() => res.statusText)) || `HTTP ${res.status}`)
    }
  },
  async listUserQuota(): Promise<AdminUserQuota[]> {
    return jsonOrThrow(
      await fetch("/api/admin/quota", { credentials: "same-origin" })
    )
  },
  async adjust(
    userId: number,
    body: { balance?: number; delta?: number; reason?: string }
  ): Promise<{ balance: number }> {
    return jsonOrThrow(
      await fetch(`/api/admin/quota/${userId}`, {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
        credentials: "same-origin",
      })
    )
  },
  async sendTestEmail(email: string): Promise<void> {
    const res = await fetch("/api/admin/email/test", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ email }),
      credentials: "same-origin",
    })
    if (!res.ok) {
      throw new Error(
        (await res.text().catch(() => res.statusText)) || `HTTP ${res.status}`
      )
    }
  },
  async stats(period: StatsPeriod = "7d"): Promise<AdminQuotaStats> {
    return jsonOrThrow(
      await fetch(`/api/admin/quota/stats?period=${period}`, {
        credentials: "same-origin",
      })
    )
  },
}

// ---------------------------------------------------------------------------
// 展示与换算helpers
// ---------------------------------------------------------------------------

/** 微美元换算成额度的基准单位：价格字段都以每 100 万 token 计。 */
export const MICRO_USD = 1_000_000

/** 千分位展示，额度是纯数字、不带货币符号。 */
export function formatQuota(n: number): string {
  return Math.round(n).toLocaleString("zh-CN")
}

/** 大额额度的紧凑展示（徽章等空间有限处使用）。 */
export function formatQuotaCompact(n: number): string {
  const v = Math.round(n)
  const abs = Math.abs(v)
  if (abs >= 100_000_000) return `${(v / 100_000_000).toFixed(2)}亿`
  if (abs >= 10_000) return `${(v / 10_000).toFixed(v % 10_000 === 0 ? 0 : 1)}万`
  return v.toLocaleString("zh-CN")
}

/** 微美元 → 美元字符串，用于后台价格输入框的回显。 */
export function microUsdToUsd(micro: number): string {
  if (!micro) return ""
  return String(micro / MICRO_USD)
}

/** 美元输入 → 微美元整数，四舍五入避免浮点误差落库。 */
export function usdToMicroUsd(usd: string | number): number {
  const n = typeof usd === "number" ? usd : Number.parseFloat(usd)
  if (!Number.isFinite(n) || n < 0) return 0
  return Math.round(n * MICRO_USD)
}

/** 把模型的微美元单价按站点汇率折算成额度，用于后台预览。 */
export function quotaForMicroUsd(
  micro: number,
  quotaPerUsd: number,
  multiplierPercent = 100
): number {
  if (micro <= 0 || quotaPerUsd <= 0 || multiplierPercent <= 0) return 0
  return Math.ceil((micro * quotaPerUsd * multiplierPercent) / (MICRO_USD * 100))
}

/** 某次对话的额度花费预估，与后端 `chat_quota_cost` 保持同一套取整规则。 */
export function estimateChatQuota(args: {
  inputPrice: number
  outputPrice: number
  cachedInputPrice: number | null
  inputTokens: number
  outputTokens: number
  cachedTokens?: number
  quotaPerUsd: number
  multiplierPercent?: number
}): number {
  const cached = Math.max(0, args.cachedTokens ?? 0)
  const billableInput = Math.max(0, args.inputTokens - cached)
  const cachedRate = args.cachedInputPrice ?? args.inputPrice
  const micro =
    billableInput * Math.max(0, args.inputPrice) +
    Math.max(0, args.outputTokens) * Math.max(0, args.outputPrice) +
    cached * Math.max(0, cachedRate)
  const microUsd = Math.ceil(micro / MICRO_USD)
  return quotaForMicroUsd(microUsd, args.quotaPerUsd, args.multiplierPercent ?? 100)
}
