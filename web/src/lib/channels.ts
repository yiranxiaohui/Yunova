// Admin channels + pricing API client.
//
// Mirrors src/channels.rs handlers under /api/admin/channels and
// /api/admin/pricing. All routes require admin session.

async function jsonOrThrow<T>(res: Response): Promise<T> {
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText)
    throw new Error(text || `HTTP ${res.status}`)
  }
  return res.json() as Promise<T>
}

async function okOrThrow(res: Response): Promise<void> {
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText)
    throw new Error(text || `HTTP ${res.status}`)
  }
}

// ---------------------------------------------------------------------------
// channels
// ---------------------------------------------------------------------------

export type ChannelProtocol = "openai" | "claude" | "gemini"
export type ChannelKind = "chat" | "image" | "video"

export type Channel = {
  id: number
  name: string
  protocol: ChannelProtocol
  base_url: string
  /** 掩码提示（如 `sk-1…cdef`），后端不再回显明文密钥。 */
  api_key_hint: string
  /** 是否已配置密钥，用于区分「未配置」和「已配置但隐藏」。 */
  has_api_key: boolean
  enabled: boolean
  priority: number
}

export type ChannelInput = {
  name: string
  protocol: ChannelProtocol
  base_url: string
  api_key: string
  enabled?: boolean
  priority?: number
}

export type ChannelPatch = Partial<{
  name: string
  protocol: ChannelProtocol
  base_url: string
  api_key: string
  enabled: boolean
  priority: number
}>

export type ChannelModel = {
  channel_id: number
  model: string
  upstream_id: string | null
}

export type ChannelModelEntry = {
  model: string
  upstream_id?: string | null
}

// ---------------------------------------------------------------------------
// pricing
// ---------------------------------------------------------------------------

export type VideoSizeRule = { size: string; multiplier: number }

/**
 * 价格一律以「微美元」存储（1 美元 = 1_000_000），对应各家官方公布的价目表。
 * chat 按 token 计费（每 100 万 token 的价格），image 按次，video 按秒。
 */
export type ModelPrice = {
  id: number
  model: string
  kind: ChannelKind
  display_name: string | null
  enabled: boolean
  protocol: ChannelProtocol
  context_limit: number | null
  /** 每 100 万 token 的微美元价格 */
  input_price: number
  output_price: number
  /** null 表示该模型没有缓存折扣，缓存 token 按输入价计费 */
  cached_input_price: number | null
  /** 每次生图的微美元价格 */
  per_call_price: number
  // video 计费：(base_price + per_second_price × 秒) × 尺寸倍率
  base_price: number
  per_second_price: number
  allowed_seconds: number[] | null
  size_rules: VideoSizeRule[] | null
}

export type PricingInput = {
  model: string
  kind: ChannelKind
  /** Replace model-to-channel bindings when present; omit to preserve them. */
  channel_ids?: number[]
  display_name?: string | null
  enabled?: boolean
  protocol: ChannelProtocol
  context_limit?: number | null
  input_price?: number
  output_price?: number
  cached_input_price?: number | null
  per_call_price?: number
  base_price?: number
  per_second_price?: number
  allowed_seconds?: number[] | null
  size_rules?: VideoSizeRule[] | null
}

/**
 * 从 NewAPI 站点的 `/api/pricing` 导入模型价格。
 * NewAPI 以倍率存价（1 倍率 = $0.002/1K tokens），后端换算成微美元后入库，
 * 与手工录入的价格完全等价。
 */
export type NewApiSyncRequest = {
  base_url: string
  protocol?: ChannelProtocol
  channel_ids?: number[]
  /** 只导入该 NewAPI 分组开放的模型；留空表示全部 */
  group?: string
  /** 预演：返回将要写入的结果但不落库 */
  dry_run?: boolean
  /**
   * 只同步本地已添加的模型：上游目录里本站没有的模型直接忽略，不会新增。
   * 该模式本身就是为了刷新价格，因此隐含 `overwrite_existing`。
   */
  existing_only?: boolean
  /** 覆盖已存在的模型价格；默认跳过，避免冲掉手工调整 */
  overwrite_existing?: boolean
  /** 导入后直接启用；默认导入为停用，便于先复核再开放。只作用于新增的模型 */
  enable_imported?: boolean
}

export type NewApiSyncedModel = {
  model: string
  kind: ChannelKind
  input_price: number
  output_price: number
  cached_input_price: number | null
  per_call_price: number
  existed: boolean
  applied: boolean
}

export type NewApiSyncResult = {
  dry_run: boolean
  fetched: number
  imported: number
  updated: number
  skipped_existing: number
  /** 上游有、本站未添加，因而被「只同步已添加」忽略的模型数 */
  skipped_missing: number
  /** 上游计费方式与本站不兼容、已跳过的模型数（详见 warnings） */
  skipped_unsupported: number
  /** 本地已添加但上游目录里没有的模型，价格保持不变 */
  not_listed: string[]
  models: NewApiSyncedModel[]
  warnings: string[]
}

export type AllChannelModel = {
  model: string
  /** channels that advertise this model — function is chosen in model pricing */
  channels: { id: number; name: string; protocol: ChannelProtocol }[]
}

/** per-channel probe failure (timeout, 4xx, parse error) */
export type ChannelProbeError = { channel: string; error: string }

export type AllChannelModelsResponse = {
  models: AllChannelModel[]
  errors: ChannelProbeError[]
}

/**
 * 计费规则 + 它当前是否还有上游渠道提供。
 * 没有上游的模型会被视为禁用：用户端列表不再展示，调用也会被拒绝，
 * 否则只会在真正请求时收到上游报错。
 */
export type AdminModelPrice = ModelPrice & {
  upstream_available: boolean
  /** 当前提供该模型的渠道名，用于后台展示 */
  upstream_channels: string[]
}

export type AdminPricingResponse = {
  models: AdminModelPrice[]
  errors: ChannelProbeError[]
}

/** 一键清理无上游模型的结果。`models` 是被删掉的模型 ID。 */
export type PrunePricingResult = {
  deleted: number
  models: string[]
  errors: ChannelProbeError[]
}

// ---------------------------------------------------------------------------
// API
// ---------------------------------------------------------------------------

export const channelsAdminApi = {
  // channels
  async listChannels(): Promise<Channel[]> {
    return jsonOrThrow(
      await fetch("/api/admin/channels", { credentials: "same-origin" })
    )
  },
  async createChannel(input: ChannelInput): Promise<{ id: number }> {
    return jsonOrThrow(
      await fetch("/api/admin/channels", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(input),
        credentials: "same-origin",
      })
    )
  },
  async patchChannel(id: number, patch: ChannelPatch): Promise<void> {
    await okOrThrow(
      await fetch(`/api/admin/channels/${id}`, {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(patch),
        credentials: "same-origin",
      })
    )
  },
  async deleteChannel(id: number): Promise<void> {
    await okOrThrow(
      await fetch(`/api/admin/channels/${id}`, {
        method: "DELETE",
        credentials: "same-origin",
      })
    )
  },

  // channel models (bound model list)
  async getChannelModels(id: number): Promise<ChannelModel[]> {
    return jsonOrThrow(
      await fetch(`/api/admin/channels/${id}/models`, {
        credentials: "same-origin",
      })
    )
  },
  async setChannelModels(
    id: number,
    models: ChannelModelEntry[]
  ): Promise<void> {
    await okOrThrow(
      await fetch(`/api/admin/channels/${id}/models`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ models }),
        credentials: "same-origin",
      })
    )
  },

  // aggregated model list across all enabled channels, by live-probing each
  // upstream's /models endpoint (no DB whitelist required). Catalogs are
  // cached server-side; `refresh` forces a re-probe.
  async listAllChannelModels(
    refresh = false
  ): Promise<AllChannelModelsResponse> {
    return jsonOrThrow(
      await fetch(
        `/api/admin/channels/all-models${refresh ? "?refresh=1" : ""}`,
        { credentials: "same-origin" }
      )
    )
  },

  // pricing
  async listPricing(refresh = false): Promise<AdminPricingResponse> {
    return jsonOrThrow(
      await fetch(`/api/admin/pricing${refresh ? "?refresh=1" : ""}`, {
        credentials: "same-origin",
      })
    )
  },
  async upsertPricing(input: PricingInput): Promise<void> {
    await okOrThrow(
      await fetch("/api/admin/pricing", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(input),
        credentials: "same-origin",
      })
    )
  },
  async deletePricing(model: string): Promise<void> {
    await okOrThrow(
      await fetch(`/api/admin/pricing/${encodeURIComponent(model)}`, {
        method: "DELETE",
        credentials: "same-origin",
      })
    )
  },
  async prunePricing(refresh = false): Promise<PrunePricingResult> {
    return jsonOrThrow(
      await fetch(
        `/api/admin/pricing/prune-unavailable${refresh ? "?refresh=1" : ""}`,
        { method: "POST", credentials: "same-origin" }
      )
    )
  },
  async syncNewApiPricing(req: NewApiSyncRequest): Promise<NewApiSyncResult> {
    return jsonOrThrow(
      await fetch("/api/admin/pricing/sync-newapi", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(req),
        credentials: "same-origin",
      })
    )
  },
}
