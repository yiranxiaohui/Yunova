export type PaymentOrder = {
  id: number
  out_trade_no: string
  payway: string
  amount_cents: number
  quota: number
  status: "pending" | "paid" | "failed"
  trade_no: string | null
  created_at: string
  paid_at: string | null
}

export type CreateOrderResp = {
  out_trade_no: string
  pay_url: string
  amount_cents: number
  quota: number
  payway: string
}

export type AdminPaymentConfig = {
  enabled: boolean
  api_url: string
  pid: string
  key_set: boolean
  sign_type: string
  quota_per_yuan: number
  product_name: string
  min_yuan: number
  max_yuan: number
  callback_url: string
  // Kept for compatibility with older API responses.
  return_url: string
  notify_url: string
}

export type AdminPaymentConfigUpdate = Partial<
  Omit<AdminPaymentConfig, "key_set" | "return_url" | "notify_url"> & {
    key: string
    return_url: string
    notify_url: string
  }
>

export type AdminOrder = PaymentOrder & { user_id: number; username: string }

async function jsonOrThrow<T>(res: Response): Promise<T> {
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText)
    throw new Error(text || `HTTP ${res.status}`)
  }
  return res.json() as Promise<T>
}

export const paymentsApi = {
  async create(yuan: number, payway: string): Promise<CreateOrderResp> {
    return jsonOrThrow(
      await fetch("/api/payments/orders", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ yuan, payway }),
        credentials: "same-origin",
      })
    )
  },
  async get(outTradeNo: string): Promise<PaymentOrder> {
    return jsonOrThrow(
      await fetch(
        `/api/payments/orders/${encodeURIComponent(outTradeNo)}`,
        { credentials: "same-origin" }
      )
    )
  },
  async listMine(): Promise<PaymentOrder[]> {
    return jsonOrThrow(
      await fetch("/api/payments/orders", { credentials: "same-origin" })
    )
  },
}

export const adminPaymentsApi = {
  async getConfig(): Promise<AdminPaymentConfig> {
    return jsonOrThrow(
      await fetch("/api/admin/payments/config", { credentials: "same-origin" })
    )
  },
  async updateConfig(patch: AdminPaymentConfigUpdate): Promise<void> {
    const res = await fetch("/api/admin/payments/config", {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(patch),
      credentials: "same-origin",
    })
    if (!res.ok) {
      throw new Error(
        (await res.text().catch(() => res.statusText)) || `HTTP ${res.status}`
      )
    }
  },
  async listOrders(
    params: { page?: number; status?: "pending" | "paid" | "failed" } = {}
  ): Promise<AdminOrder[]> {
    const qs = new URLSearchParams()
    if (params.page) qs.set("page", String(params.page))
    if (params.status) qs.set("status", params.status)
    const suffix = qs.toString() ? `?${qs.toString()}` : ""
    return jsonOrThrow(
      await fetch(`/api/admin/payments/orders${suffix}`, {
        credentials: "same-origin",
      })
    )
  },
}
