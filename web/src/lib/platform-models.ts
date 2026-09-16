// Lists models available via admin-configured upstream channels (paid with
// site quota). Backed by `GET /api/channels/models` (channels.rs).
//
// Distinct from `lib/models.ts` which lists models from the *user's* BYOK
// upstream — these two sources never mix.

import { formatQuota } from "./quota"

export type PlatformModel = {
  model: string
  display_name: string | null
  kind: "chat" | "image"
  protocol: "openai" | "claude" | "gemini"
  context_limit: number | null
  /** Provider key inside an agent runtime's generated config, or null when
   *  work mode cannot run this model. Decided by the server so the picker and
   *  the runtime cannot disagree about what is selectable. */
  agent_provider: string | null
  // 价格，后端已按站点汇率与倍率换算完毕，单位为微额度（1 额度 = 1 元 = 1e6）
  input_micro_quota_per_1m: number
  output_micro_quota_per_1m: number
  cached_input_micro_quota_per_1m: number | null
  per_call_micro_quota: number
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
 * 对话按 token 计价，所以展示每 100 万 token 的价格，而不是「每次」。
 */
export function describeModelQuota(m: PlatformModel): string {
  if (m.kind === "image") {
    return m.per_call_micro_quota > 0
      ? `${formatQuota(m.per_call_micro_quota)} 元/次`
      : "免费"
  }
  if (m.input_micro_quota_per_1m === 0 && m.output_micro_quota_per_1m === 0) {
    return "免费"
  }
  return `${formatQuota(m.input_micro_quota_per_1m)}/${formatQuota(
    m.output_micro_quota_per_1m
  )} 元每1M`
}
