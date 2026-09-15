import { describe, expect, test } from "bun:test"
import {
  estimateChatQuota,
  formatQuota,
  formatQuotaCompact,
  microUsdToUsd,
  quotaForMicroUsd,
  usdToMicroUsd,
} from "../src/lib/quota"
import { describeModelQuota, type PlatformModel } from "../src/lib/platform-models"

/** 站点默认：1 美元上游成本 = 500_000 额度，不加价。 */
const QUOTA_PER_USD = 500_000

function chatModel(patch: Partial<PlatformModel> = {}): PlatformModel {
  return {
    model: "gpt-5",
    display_name: null,
    kind: "chat",
    protocol: "openai",
    context_limit: null,
    input_quota_per_1m: 625_000,
    output_quota_per_1m: 5_000_000,
    cached_input_quota_per_1m: null,
    per_call_quota: 0,
    ...patch,
  }
}

describe("美元价与额度的换算", () => {
  test("官方价目表里的小数美元价不丢精度", () => {
    // $1.25 / 1M tokens 必须落成整数 1_250_000 微美元，而不是浮点近似值。
    expect(usdToMicroUsd("1.25")).toBe(1_250_000)
    expect(usdToMicroUsd("0.15")).toBe(150_000)
    expect(usdToMicroUsd("0.075")).toBe(75_000)
    // 反向回显不应出现 0.30000000000000004 这类尾数。
    expect(microUsdToUsd(300_000)).toBe("0.3")
    expect(microUsdToUsd(1_250_000)).toBe("1.25")
  })

  test("非法或负数输入按 0 处理，不会写入负价格", () => {
    expect(usdToMicroUsd("")).toBe(0)
    expect(usdToMicroUsd("abc")).toBe(0)
    expect(usdToMicroUsd("-3")).toBe(0)
  })

  test("美元成本按站点汇率折算成额度", () => {
    expect(quotaForMicroUsd(1_000_000, QUOTA_PER_USD)).toBe(500_000)
    expect(quotaForMicroUsd(1_250_000, QUOTA_PER_USD)).toBe(625_000)
  })

  test("全局倍率统一作用于所有模型", () => {
    const base = quotaForMicroUsd(1_000_000, QUOTA_PER_USD, 100)
    expect(quotaForMicroUsd(1_000_000, QUOTA_PER_USD, 150)).toBe(base * 1.5)
    expect(quotaForMicroUsd(1_000_000, QUOTA_PER_USD, 200)).toBe(base * 2)
  })

  test("极小额消耗向上取整，避免高频调用变成免费", () => {
    expect(quotaForMicroUsd(1, QUOTA_PER_USD)).toBe(1)
    // 真正免费的模型仍然是 0。
    expect(quotaForMicroUsd(0, QUOTA_PER_USD)).toBe(0)
  })
})

describe("对话按 token 计费的预估", () => {
  test("与后端同一套公式：分别按输入价和输出价计费", () => {
    // $1.25 输入 / $10 输出，100 万输入 + 100 万输出。
    const quota = estimateChatQuota({
      inputPrice: 1_250_000,
      outputPrice: 10_000_000,
      cachedInputPrice: null,
      inputTokens: 1_000_000,
      outputTokens: 1_000_000,
      quotaPerUsd: QUOTA_PER_USD,
    })
    // $11.25 × 500_000 = 5_625_000 额度
    expect(quota).toBe(5_625_000)
  })

  test("命中缓存的 token 走缓存价，且不会被重复计费", () => {
    const cached = estimateChatQuota({
      inputPrice: 1_250_000,
      outputPrice: 0,
      cachedInputPrice: 125_000,
      inputTokens: 1_000_000,
      outputTokens: 0,
      cachedTokens: 900_000,
      quotaPerUsd: QUOTA_PER_USD,
    })
    const uncached = estimateChatQuota({
      inputPrice: 1_250_000,
      outputPrice: 0,
      cachedInputPrice: 125_000,
      inputTokens: 1_000_000,
      outputTokens: 0,
      quotaPerUsd: QUOTA_PER_USD,
    })
    // 缓存价是输入价的 1/10，命中 90% 后应显著便宜。
    expect(cached).toBeLessThan(uncached)
    // 10 万 × $1.25 + 90 万 × $0.125 = $0.2375
    expect(cached).toBe(Math.ceil(0.2375 * QUOTA_PER_USD))
  })

  test("未配置缓存价时，缓存 token 按普通输入价计费", () => {
    const withCacheHits = estimateChatQuota({
      inputPrice: 1_250_000,
      outputPrice: 0,
      cachedInputPrice: null,
      inputTokens: 1_000_000,
      outputTokens: 0,
      cachedTokens: 400_000,
      quotaPerUsd: QUOTA_PER_USD,
    })
    const withoutCacheHits = estimateChatQuota({
      inputPrice: 1_250_000,
      outputPrice: 0,
      cachedInputPrice: null,
      inputTokens: 1_000_000,
      outputTokens: 0,
      quotaPerUsd: QUOTA_PER_USD,
    })
    expect(withCacheHits).toBe(withoutCacheHits)
  })

  test("免费模型不产生任何额度消耗", () => {
    expect(
      estimateChatQuota({
        inputPrice: 0,
        outputPrice: 0,
        cachedInputPrice: null,
        inputTokens: 50_000,
        outputTokens: 20_000,
        quotaPerUsd: QUOTA_PER_USD,
      })
    ).toBe(0)
  })
})

describe("额度展示", () => {
  test("额度是纯数字，不带货币符号", () => {
    expect(formatQuota(1234567)).toBe("1,234,567")
    expect(formatQuota(0)).toBe("0")
    expect(formatQuota(-2500)).toBe("-2,500")
  })

  test("徽章用紧凑写法展示大额余额", () => {
    expect(formatQuotaCompact(9_999)).toBe("9,999")
    expect(formatQuotaCompact(120_000)).toBe("12万")
    expect(formatQuotaCompact(125_000)).toBe("12.5万")
    expect(formatQuotaCompact(300_000_000)).toBe("3.00亿")
  })

  test("对话模型展示每百万 token 的额度，而不是每次", () => {
    expect(describeModelQuota(chatModel())).toBe("625,000/5,000,000 额度每1M")
  })

  test("图像模型按次展示", () => {
    const image = chatModel({ kind: "image", per_call_quota: 20_000 })
    expect(describeModelQuota(image)).toBe("20,000 额度/次")
  })

  test("零价模型显示为免费", () => {
    expect(
      describeModelQuota(
        chatModel({ input_quota_per_1m: 0, output_quota_per_1m: 0 })
      )
    ).toBe("免费")
    expect(
      describeModelQuota(chatModel({ kind: "image", per_call_quota: 0 }))
    ).toBe("免费")
  })
})
