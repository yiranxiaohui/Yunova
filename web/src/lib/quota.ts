// 站点额度（quota）API 客户端。
//
// 额度以人民币计价：1 额度 = 1 元。但一次对话常只花几分甚至几毫，因此后端一律
// 以「微额度」（1 额度 = 1_000_000）存储和传输，前端只在展示时换算成元。
// 整数微额度保证每一笔扣费精确，用浮点保存余额会逐渐产生偏差。
//
// 模型价格由管理员照抄各家官方美元价目表录入，后端按 `usd_to_cny_rate_micro`
// 汇率与全局倍率换算成额度。对话按 token 实际用量事后结算，生图按次、视频按秒。

export type QuotaMe = {
  /** 微额度。除以 1e6 得到元。 */
  balance: number
  lifetime_used: number
  /** 每 1 额度对应多少微额度，避免前端硬编码倍数 */
  micro_per_quota: number
  /** 汇率：1 美元 = 多少微额度（即多少元 × 1e6） */
  usd_to_cny_rate_micro: number
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
  usd_to_cny_rate_micro: number
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
// 展示与换算 helpers
// ---------------------------------------------------------------------------

/** 微美元换算基准：价格字段都以每 100 万 token 计。 */
export const MICRO_USD = 1_000_000

/** 每 1 额度（= 1 元）对应的微额度数。 */
export const MICRO_QUOTA = 1_000_000

/**
 * 微额度 → 元的展示字符串。
 *
 * 去掉无意义的尾随零：整数额显示「5」而不是「5.000000」；不足一分的小额
 * 保留足以看出非零的位数，避免显示成 0 让人以为没扣费。
 */
export function formatQuota(micro: number): string {
  const neg = micro < 0
  const abs = Math.abs(Math.round(micro))
  const whole = Math.floor(abs / MICRO_QUOTA)
  const frac = abs % MICRO_QUOTA
  const sign = neg ? "-" : ""
  if (frac === 0) return `${sign}${whole.toLocaleString("zh-CN")}`
  const fracStr = String(frac).padStart(6, "0").replace(/0+$/, "")
  return `${sign}${whole.toLocaleString("zh-CN")}.${fracStr}`
}

/** 带「元」后缀的展示形式。 */
export function formatQuotaYuan(micro: number): string {
  return `${formatQuota(micro)} 元`
}

/**
 * 徽章等窄处的紧凑展示。金额较大时省略小数，较小时保留 2 位，
 * 这样余额不足时用户仍能看出还剩多少。
 */
export function formatQuotaCompact(micro: number): string {
  const yuanValue = micro / MICRO_QUOTA
  const abs = Math.abs(yuanValue)
  if (abs >= 10_000) return `${(yuanValue / 10_000).toFixed(1)}万`
  if (abs >= 100) return Math.round(yuanValue).toLocaleString("zh-CN")
  if (abs === 0) return "0"
  // 小额保留两位小数，但不要显示成 0.00 掩盖仍有余额的事实
  if (abs < 0.01) return yuanValue > 0 ? "<0.01" : ">-0.01"
  return yuanValue.toFixed(2)
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

/** 元输入 → 微额度整数。 */
export function yuanToMicroQuota(yuan: string | number): number {
  const n = typeof yuan === "number" ? yuan : Number.parseFloat(yuan)
  if (!Number.isFinite(n)) return 0
  return Math.round(n * MICRO_QUOTA)
}

/** 微额度 → 元的输入框回显（不带千分位，便于再次编辑）。 */
export function microQuotaToYuanInput(micro: number): string {
  if (!micro) return "0"
  return String(Number((micro / MICRO_QUOTA).toFixed(6)))
}

/**
 * 模型的微美元单价按汇率折算成微额度，用于后台预览。
 * 与后端 `QuotaRate::micro_quota_for_micro_usd` 保持同一套取整规则。
 */
export function microQuotaForMicroUsd(
  microUsd: number,
  usdToCnyRateMicro: number,
  multiplierPercent = 100
): number {
  if (microUsd <= 0 || usdToCnyRateMicro <= 0 || multiplierPercent <= 0) return 0
  return Math.ceil(
    (microUsd * usdToCnyRateMicro * multiplierPercent) / (MICRO_USD * 100)
  )
}

/** 某次对话的额度花费预估，与后端 `chat_quota_cost` 同一套取整规则。 */
export function estimateChatQuota(args: {
  inputPrice: number
  outputPrice: number
  cachedInputPrice: number | null
  inputTokens: number
  outputTokens: number
  cachedTokens?: number
  usdToCnyRateMicro: number
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
  return microQuotaForMicroUsd(
    microUsd,
    args.usdToCnyRateMicro,
    args.multiplierPercent ?? 100
  )
}
