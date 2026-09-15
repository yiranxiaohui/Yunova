// Lists models available via admin-configured upstream channels (paid with
// site quota). Backed by `GET /api/channels/models` (channels.rs).
//
// Distinct from `lib/models.ts` which lists models from the *user's* BYOK
// upstream — these two sources never mix.

export type PlatformModel = {
  model: string
  display_name: string | null
  kind: "chat" | "image"
  protocol: "openai" | "claude" | "gemini"
  context_limit: number | null
  /** 额度价格，后端已按站点汇率与倍率换算完毕 */
  input_quota_per_1m: number
  output_quota_per_1m: number
  cached_input_quota_per_1m: number | null
  per_call_quota: number
}

export async function listPlatformModels(
  flavor: "chat" | "image",
  signal?: AbortSignal
): Promise<PlatformModel[]> {
  const res = await fetch(`/api/channels/models?flavor=${flavor}`, {
    credentials: "same-origin",
    signal,
  })
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText)
    throw new Error(`HTTP ${res.status}: ${text}`)
  }
  return (await res.json()) as PlatformModel[]
}

/**
 * 模型价格的简短中文描述，用于模型选择器等空间有限的位置。
 * 对话按 token 计价，所以展示每 100 万 token 的额度，而不是「每次」。
 */
export function describeModelQuota(m: PlatformModel): string {
  const fmt = (n: number) => Math.round(n).toLocaleString("zh-CN")
  if (m.kind === "image") {
    return m.per_call_quota > 0 ? `${fmt(m.per_call_quota)} 额度/次` : "免费"
  }
  if (m.input_quota_per_1m === 0 && m.output_quota_per_1m === 0) return "免费"
  return `${fmt(m.input_quota_per_1m)}/${fmt(m.output_quota_per_1m)} 额度每1M`
}
