import { describe, expect, test } from "bun:test"
import {
  estimateChatQuota,
  formatQuota,
  formatQuotaCompact,
  microQuotaForMicroUsd,
  microQuotaToYuanInput,
  microUsdToUsd,
  usdToMicroUsd,
  yuanToMicroQuota,
  MICRO_QUOTA,
} from "../src/lib/quota"
import { describeModelQuota, type PlatformModel } from "../src/lib/platform-models"

/** 站点默认汇率：1 美元上游成本折算 ¥7.2，不加价。 */
const USD_TO_CNY = 7_200_000

/** 便于阅读的期望值：元 → 微额度。 */
const yuan = (n: number) => Math.round(n * MICRO_QUOTA)

function chatModel(patch: Partial<PlatformModel> = {}): PlatformModel {
  return {
    model: "gpt-5",
    display_name: null,
    kind: "chat",
    protocol: "openai",
    context_limit: null,
    input_micro_quota_per_1m: yuan(9),
    output_micro_quota_per_1m: yuan(72),
    cached_input_micro_quota_per_1m: null,
    per_call_micro_quota: 0,
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

  test("美元成本按汇率折算成人民币额度", () => {
    // $1 = ¥7.2
    expect(microQuotaForMicroUsd(1_000_000, USD_TO_CNY)).toBe(yuan(7.2))
    // $1.25 = ¥9
    expect(microQuotaForMicroUsd(1_250_000, USD_TO_CNY)).toBe(yuan(9))
  })

  test("全局倍率统一作用于所有模型", () => {
    const base = microQuotaForMicroUsd(1_000_000, USD_TO_CNY, 100)
    expect(microQuotaForMicroUsd(1_000_000, USD_TO_CNY, 150)).toBe(base * 1.5)
    expect(microQuotaForMicroUsd(1_000_000, USD_TO_CNY, 200)).toBe(base * 2)
  })

  test("极小额消耗向上取整，避免高频调用变成免费", () => {
    expect(microQuotaForMicroUsd(1, USD_TO_CNY)).toBeGreaterThan(0)
    // 真正免费的模型仍然是 0。
    expect(microQuotaForMicroUsd(0, USD_TO_CNY)).toBe(0)
  })
})

describe("元与微额度的互转", () => {
  test("1 元就是 1 额度", () => {
    expect(yuanToMicroQuota(1)).toBe(MICRO_QUOTA)
    expect(formatQuota(yuanToMicroQuota(1))).toBe("1")
  })

  test("管理员输入的小数金额不产生浮点误差", () => {
    // 0.1 + 0.2 式的误差不能落库。
    expect(yuanToMicroQuota("0.1")).toBe(100_000)
    expect(yuanToMicroQuota("12.34")).toBe(12_340_000)
    expect(yuanToMicroQuota("0.045")).toBe(45_000)
  })

  test("回显能被再次编辑而不漂移", () => {
    for (const v of [0, 1, 1_500_000, 45_000, 12_340_000]) {
      expect(yuanToMicroQuota(microQuotaToYuanInput(v))).toBe(v)
    }
  })

  test("非法输入按 0 处理", () => {
    expect(yuanToMicroQuota("abc")).toBe(0)
    expect(yuanToMicroQuota("")).toBe(0)
  })
})

describe("对话按 token 计费的预估", () => {
  test("与后端同一套公式：分别按输入价和输出价计费", () => {
    // $1.25 输入 / $10 输出，100 万输入 + 100 万输出 = $11.25 = ¥81
    const quota = estimateChatQuota({
      inputPrice: 1_250_000,
      outputPrice: 10_000_000,
      cachedInputPrice: null,
      inputTokens: 1_000_000,
      outputTokens: 1_000_000,
      usdToCnyRateMicro: USD_TO_CNY,
    })
    expect(quota).toBe(yuan(81))
  })

  test("一次普通请求只花几分钱，且不会被取整成 0", () => {
    // 1000 输入 + 500 输出 = $0.00625 = ¥0.045
    const quota = estimateChatQuota({
      inputPrice: 1_250_000,
      outputPrice: 10_000_000,
      cachedInputPrice: null,
      inputTokens: 1_000,
      outputTokens: 500,
      usdToCnyRateMicro: USD_TO_CNY,
    })
    expect(quota).toBe(45_000)
    expect(formatQuota(quota)).toBe("0.045")
  })

  test("命中缓存的 token 走缓存价，且不会被重复计费", () => {
    const cached = estimateChatQuota({
      inputPrice: 1_250_000,
      outputPrice: 0,
      cachedInputPrice: 125_000,
      inputTokens: 1_000_000,
      outputTokens: 0,
      cachedTokens: 900_000,
      usdToCnyRateMicro: USD_TO_CNY,
    })
    const uncached = estimateChatQuota({
      inputPrice: 1_250_000,
      outputPrice: 0,
      cachedInputPrice: 125_000,
      inputTokens: 1_000_000,
      outputTokens: 0,
      usdToCnyRateMicro: USD_TO_CNY,
    })
    expect(cached).toBeLessThan(uncached)
    // 10 万 × $1.25 + 90 万 × $0.125 = $0.2375 = ¥1.71
    expect(cached).toBe(yuan(1.71))
  })

  test("未配置缓存价时，缓存 token 按普通输入价计费", () => {
    const withCacheHits = estimateChatQuota({
      inputPrice: 1_250_000,
      outputPrice: 0,
      cachedInputPrice: null,
      inputTokens: 1_000_000,
      outputTokens: 0,
      cachedTokens: 400_000,
      usdToCnyRateMicro: USD_TO_CNY,
    })
    const withoutCacheHits = estimateChatQuota({
      inputPrice: 1_250_000,
      outputPrice: 0,
      cachedInputPrice: null,
      inputTokens: 1_000_000,
      outputTokens: 0,
      usdToCnyRateMicro: USD_TO_CNY,
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
        usdToCnyRateMicro: USD_TO_CNY,
      })
    ).toBe(0)
  })
})

describe("额度展示", () => {
  test("按元展示，去掉无意义的尾随零", () => {
    expect(formatQuota(0)).toBe("0")
    expect(formatQuota(MICRO_QUOTA)).toBe("1")
    expect(formatQuota(5 * MICRO_QUOTA)).toBe("5")
    expect(formatQuota(1_500_000)).toBe("1.5")
    expect(formatQuota(45_000)).toBe("0.045")
    expect(formatQuota(12_340_000)).toBe("12.34")
  })

  test("大额带千分位，负数余额如实显示", () => {
    expect(formatQuota(1_234_567 * MICRO_QUOTA)).toBe("1,234,567")
    expect(formatQuota(-1_500_000)).toBe("-1.5")
  })

  test("不足一分的余额不会显示成 0，掩盖仍有余额的事实", () => {
    expect(formatQuota(2)).toBe("0.000002")
    expect(formatQuotaCompact(2)).toBe("<0.01")
  })

  test("徽章紧凑展示", () => {
    expect(formatQuotaCompact(0)).toBe("0")
    expect(formatQuotaCompact(yuan(12.5))).toBe("12.50")
    expect(formatQuotaCompact(yuan(1234))).toBe("1,234")
    expect(formatQuotaCompact(yuan(25_000))).toBe("2.5万")
  })

  test("对话模型展示每百万 token 的价格，而不是每次", () => {
    expect(describeModelQuota(chatModel())).toBe("9/72 元每1M")
  })

  test("图像模型按次展示", () => {
    const image = chatModel({ kind: "image", per_call_micro_quota: yuan(0.3) })
    expect(describeModelQuota(image)).toBe("0.3 元/次")
  })

  test("零价模型显示为免费", () => {
    expect(
      describeModelQuota(
        chatModel({ input_micro_quota_per_1m: 0, output_micro_quota_per_1m: 0 })
      )
    ).toBe("免费")
    expect(
      describeModelQuota(chatModel({ kind: "image", per_call_micro_quota: 0 }))
    ).toBe("免费")
  })
})
