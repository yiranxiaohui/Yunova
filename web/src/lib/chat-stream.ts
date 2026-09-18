import type { Protocol } from "./settings"
import { trimSlash } from "./settings"

export type ChatMessage = { role: "system" | "user" | "assistant"; content: string }

/**
 * How hard the model should think before answering, in chat mode.
 *
 * `auto` is what chat did before the level could be chosen: ask for thinking
 * without saying how much, and let the provider decide. It stays the default
 * because it is the only value that behaves sensibly on every model — the
 * three vendors disagree on what a "level" even is (an effort enum, a token
 * budget, a dynamic budget), so the explicit levels below are a mapping, not a
 * promise of identical behaviour across providers.
 *
 * `off` is the one level worth being literal about: it suppresses the thinking
 * request entirely, which is also what the automatic downgrade falls back to
 * when a model rejects the parameters.
 */
export type ChatThinkingLevel = "off" | "auto" | "low" | "medium" | "high"

export const CHAT_THINKING_LEVELS: ChatThinkingLevel[] = [
  "off",
  "auto",
  "low",
  "medium",
  "high",
]

export const CHAT_THINKING_LABELS: Record<ChatThinkingLevel, string> = {
  off: "不思考",
  auto: "自动",
  low: "低",
  medium: "中",
  high: "高",
}

// --- attachment handling -------------------------------------------------
// User messages can embed markdown image refs like `![](/api/images/x.png)`
// or data URLs. We strip them out for the text sent to the model and
// upload each referenced image as a base64 `input_image` part.

type ExtractedImages = { text: string; images: string[] }

function extractImages(md: string): ExtractedImages {
  const images: string[] = []
  const text = md
    .replace(/!\[[^\]]*\]\(([^)]+)\)/g, (_m, p1: string) => {
      if (p1) images.push(p1)
      return ""
    })
    .trim()
  return { text, images }
}

// Document attachments are embedded in user messages as ordinary markdown
// links whose URL points at our own `/api/files/` store, e.g.
// `[report.pdf](/api/files/ab12.pdf)`, or a guest-mode data URL. We strip them
// from the prompt text and re-attach each as a native document block.
type FileRef = { name: string; url: string }
type ExtractedFiles = { text: string; files: FileRef[] }

function extractFiles(md: string): ExtractedFiles {
  const files: FileRef[] = []
  const text = md
    .replace(
      /\[([^\]]*)\]\(((?:\/api\/files\/|data:)[^)\s]+)\)/g,
      (_m, name: string, url: string) => {
        if (url) files.push({ name: name || "file", url })
        return ""
      }
    )
    .trim()
  return { text, files }
}

function isPdf(mime: string): boolean {
  return mime === "application/pdf" || mime.startsWith("application/pdf")
}

function isTextual(mime: string): boolean {
  return (
    mime.startsWith("text/") ||
    mime === "application/json" ||
    mime === "application/xml" ||
    mime === "application/x-yaml" ||
    mime === "application/javascript"
  )
}

async function fileRefToText(url: string): Promise<string | null> {
  try {
    const res = await fetch(url, { credentials: "same-origin" })
    if (!res.ok) return null
    return await res.text()
  } catch {
    return null
  }
}

/** Assistant content sometimes embeds markdown image refs (notably the OpenAI
 * Responses `image_generation` tool dumps `![alt](/api/images/…png)` into the
 * assistant message after it generates an image). None of OpenAI / Claude /
 * Gemini accept image content blocks under the assistant role on a follow-up
 * turn — sending them back causes a 400 and effectively bricks the
 * conversation. Strip the refs and replace with a short text marker so the
 * model still knows "I generated an image here" without re-uploading the
 * bitmap. */
function sanitizeAssistantContent(content: string): string {
  const { text, images } = extractImages(content)
  if (images.length === 0) return content
  const marker =
    images.length === 1
      ? "[已生成图像]"
      : `[已生成 ${images.length} 张图像]`
  return text ? `${text}\n\n${marker}` : marker
}

function mimeFromPath(path: string): string {
  const ext = path.split(".").pop()?.toLowerCase() ?? ""
  if (ext === "jpg" || ext === "jpeg") return "image/jpeg"
  if (ext === "webp") return "image/webp"
  if (ext === "gif") return "image/gif"
  return "image/png"
}

async function refToBase64(ref: string): Promise<{ mime: string; b64: string } | null> {
  // data URL: pass the base64 payload through.
  if (ref.startsWith("data:")) {
    const m = /^data:([^;]+);base64,(.*)$/.exec(ref)
    if (!m) return null
    return { mime: m[1], b64: m[2] }
  }
  try {
    const res = await fetch(ref, { credentials: "same-origin" })
    if (!res.ok) return null
    const blob = await res.blob()
    const mime = blob.type || mimeFromPath(ref)
    const buf = await blob.arrayBuffer()
    const bytes = new Uint8Array(buf)
    let bin = ""
    const chunk = 0x8000
    for (let i = 0; i < bytes.length; i += chunk) {
      bin += String.fromCharCode(...bytes.subarray(i, i + chunk))
    }
    return { mime, b64: btoa(bin) }
  } catch {
    return null
  }
}

async function refToDataUrl(ref: string): Promise<string | null> {
  if (ref.startsWith("data:")) return ref
  const enc = await refToBase64(ref)
  return enc ? `data:${enc.mime};base64,${enc.b64}` : null
}

export type ChatStreamOptions = {
  protocol: Protocol
  baseUrl: string
  apiKey: string
  model: string
  messages: ChatMessage[]
  useProxy?: boolean
  /**
   * When true, route through `/api/proxy/*` WITHOUT sending
   * `X-Upstream-Url/Key` headers — the server resolves an admin-configured
   * channel chain for the model and charges site quota. When false (default),
   * BYOK: the client supplies its own upstream creds via headers.
   */
  usePlatform?: boolean
  webSearch?: boolean
  /** How hard to think. Defaults to `auto`, which is the behaviour chat had
   *  before this could be chosen. */
  thinking?: ChatThinkingLevel
  // OpenAI protocol only. Declares the hosted `image_generation` tool on
  // the /v1/responses request so the model can emit images inline during
  // a normal chat turn. When an `image_generation_call` completes, its
  // base64 bytes are uploaded to /api/images/save and a markdown image
  // reference is spliced into the assistant message.
  imageGen?: boolean
  // Guests have no server media store. Keep generated images as data URLs in
  // the in-memory conversation instead of calling the protected save route.
  persistGeneratedImages?: boolean
  temperature?: number
  maxTokens?: number
  signal?: AbortSignal
  onDelta: (delta: string) => void
  // 模型的思考过程增量（推理模型才有）。上游不回传时一次也不会调用。
  // 我们不主动向上游索要思考内容，只解析它自愿回传的部分。
  onReasoning?: (delta: string) => void
  // Apply a transformation to the current assistant message content. Used
  // by the hosted image_generation flow to show a ticking "生成图像中…"
  // placeholder without persisting it into the saved message content.
  patchAssistant?: (update: (prev: string) => string) => void
  // 上游回传的真实 token 用量（prompt/completion）。流中可能被多次调用，
  // 最后一次为最终值；部分中转站不回传 usage，则一次也不会调用。
  onUsage?: (promptTokens: number, completionTokens: number) => void
}

type PreparedRequest = {
  url: string
  body: unknown
  directHeaders: Record<string, string>
}

/** Gemini takes a token budget rather than an effort enum, so the levels are
 *  mapped to absolute numbers. Chosen inside the range every thinking-capable
 *  2.x/3.x model accepts, so one mapping works across the family; a model that
 *  rejects the field at all is caught by the downgrade in `streamChat`. */
const GEMINI_BUDGET: Record<"low" | "medium" | "high", number> = {
  low: 1024,
  medium: 8192,
  high: 24576,
}

async function prepareOpenAi(
  o: ChatStreamOptions,
  thinking: ChatThinkingLevel
): Promise<PreparedRequest> {
  const system = o.messages
    .filter((m) => m.role === "system")
    .map((m) => m.content)
    .join("\n\n")
  const nonSystem = o.messages.filter((m) => m.role !== "system")
  const input = await Promise.all(
    nonSystem.map(async (m) => {
      const content =
        m.role === "assistant" ? sanitizeAssistantContent(m.content) : m.content
      const { text: t0, images } = extractImages(content)
      const { text, files } =
        m.role === "user" ? extractFiles(t0) : { text: t0, files: [] }
      if (images.length === 0 && files.length === 0) {
        return { role: m.role, content }
      }
      const parts: unknown[] = []
      if (text) {
        parts.push({
          type: m.role === "assistant" ? "output_text" : "input_text",
          text,
        })
      }
      for (const ref of images) {
        const durl = await refToDataUrl(ref)
        if (durl) parts.push({ type: "input_image", image_url: durl })
      }
      for (const f of files) {
        const durl = await refToDataUrl(f.url)
        if (durl)
          parts.push({ type: "input_file", filename: f.name, file_data: durl })
      }
      return { role: m.role, content: parts }
    })
  )
  const tools: unknown[] = []
  if (o.webSearch) tools.push({ type: "web_search" })
  if (o.imageGen) tools.push({ type: "image_generation" })
  return {
    url: `${trimSlash(o.baseUrl)}/v1/responses`,
    body: {
      model: o.model,
      input,
      stream: true,
      ...(system ? { instructions: system } : {}),
      ...(o.temperature !== undefined ? { temperature: o.temperature } : {}),
      // 索要推理摘要。非推理模型（gpt-4o 等）会 400，由 streamChat 的降级
      // 重试兜底。`effort` 只在用户明确选了级别时才传：省略它等于用
      // 模型自己的默认强度，而中转站对这个字段的支持参差不齐。
      ...(thinking === "off"
        ? {}
        : {
            reasoning: {
              summary: "auto",
              ...(thinking === "auto" ? {} : { effort: thinking }),
            },
          }),
      ...(tools.length ? { tools } : {}),
    },
    directHeaders: {
      Authorization: `Bearer ${o.apiKey}`,
    },
  }
}

async function prepareClaude(
  o: ChatStreamOptions,
  thinking: ChatThinkingLevel
): Promise<PreparedRequest> {
  const system = o.messages
    .filter((m) => m.role === "system")
    .map((m) => m.content)
    .join("\n\n")
  const nonSystem = o.messages.filter((m) => m.role !== "system")
  const rest = await Promise.all(
    nonSystem.map(async (m) => {
      const content =
        m.role === "assistant" ? sanitizeAssistantContent(m.content) : m.content
      const { text: t0, images } = extractImages(content)
      const { text, files } =
        m.role === "user" ? extractFiles(t0) : { text: t0, files: [] }
      if (images.length === 0 && files.length === 0) {
        return { role: m.role, content }
      }
      const parts: unknown[] = []
      if (text) parts.push({ type: "text", text })
      for (const ref of images) {
        const enc = await refToBase64(ref)
        if (enc) {
          parts.push({
            type: "image",
            source: { type: "base64", media_type: enc.mime, data: enc.b64 },
          })
        }
      }
      for (const f of files) {
        const enc = await refToBase64(f.url)
        if (!enc) continue
        if (isPdf(enc.mime)) {
          parts.push({
            type: "document",
            source: { type: "base64", media_type: "application/pdf", data: enc.b64 },
            title: f.name,
          })
        } else if (isTextual(enc.mime)) {
          const raw = await fileRefToText(f.url)
          if (raw != null) {
            parts.push({
              type: "document",
              source: { type: "text", media_type: "text/plain", data: raw },
              title: f.name,
            })
          }
        }
      }
      return { role: m.role, content: parts }
    })
  )
  // extended thinking 的硬性约束：budget_tokens 至少 1024 且必须小于
  // max_tokens，同时 temperature 只能是 1（传别的值直接 400）。级别越高需要
  // 越大的窗口，所以上限跟着级别抬：只抬 budget 不抬 max_tokens 会把正文
  // 的空间挤完，模型会在思考完之后直接被截断。
  const thinkingOn = thinking !== "off"
  const maxTokens = thinkingOn
    ? Math.max(o.maxTokens ?? 4096, thinking === "high" ? 8192 : 2048)
    : (o.maxTokens ?? 4096)
  // 余下的都留给正文：1024 是 API 的硬下限，maxTokens-1024 保证回答本身
  // 至少还有 1024 token 可用。
  const budget = (ratio: number) =>
    Math.min(
      Math.max(Math.floor(maxTokens * ratio), 1024),
      Math.max(maxTokens - 1024, 1024)
    )
  const BUDGET_RATIO: Record<Exclude<ChatThinkingLevel, "off">, number> = {
    auto: 0.5,
    low: 0.25,
    medium: 0.5,
    high: 0.75,
  }

  return {
    url: `${trimSlash(o.baseUrl)}/v1/messages`,
    body: {
      model: o.model,
      max_tokens: maxTokens,
      messages: rest,
      stream: true,
      ...(system ? { system } : {}),
      // 开思考时不传 temperature —— API 要求它必须为 1，省略即取默认值 1。
      ...(!thinkingOn && o.temperature !== undefined
        ? { temperature: o.temperature }
        : {}),
      ...(thinkingOn
        ? {
            thinking: {
              type: "enabled",
              budget_tokens: budget(BUDGET_RATIO[thinking]),
            },
          }
        : {}),
      ...(o.webSearch
        ? {
            tools: [
              {
                type: "web_search_20250305",
                name: "web_search",
                max_uses: 5,
              },
            ],
          }
        : {}),
    },
    directHeaders: {
      "x-api-key": o.apiKey,
      "anthropic-version": "2023-06-01",
      "anthropic-dangerous-direct-browser-access": "true",
    },
  }
}

async function prepareGemini(
  o: ChatStreamOptions,
  thinking: ChatThinkingLevel
): Promise<PreparedRequest> {
  const system = o.messages
    .filter((m) => m.role === "system")
    .map((m) => m.content)
    .join("\n\n")
  const nonSystem = o.messages.filter((m) => m.role !== "system")
  const contents = await Promise.all(
    nonSystem.map(async (m) => {
      const content =
        m.role === "assistant" ? sanitizeAssistantContent(m.content) : m.content
      const { text: t0, images } = extractImages(content)
      const { text, files } =
        m.role === "user" ? extractFiles(t0) : { text: t0, files: [] }
      const parts: unknown[] = []
      if (text || (images.length === 0 && files.length === 0)) {
        parts.push({ text: text || content })
      }
      for (const ref of images) {
        const enc = await refToBase64(ref)
        if (enc) {
          parts.push({ inline_data: { mime_type: enc.mime, data: enc.b64 } })
        }
      }
      for (const f of files) {
        const enc = await refToBase64(f.url)
        if (enc && (isPdf(enc.mime) || isTextual(enc.mime))) {
          parts.push({ inline_data: { mime_type: enc.mime, data: enc.b64 } })
        }
      }
      return {
        role: m.role === "assistant" ? "model" : "user",
        parts,
      }
    })
  )
  const generationConfig =
    o.temperature !== undefined || o.maxTokens !== undefined || thinking !== "off"
      ? {
          ...(o.temperature !== undefined ? { temperature: o.temperature } : {}),
          ...(o.maxTokens !== undefined ? { maxOutputTokens: o.maxTokens } : {}),
          // 不加 includeThoughts，parts 里就不会出现 thought:true 的片段。
          // 老模型不认这个字段会 400，由 streamChat 的降级重试兜底。
          // thinkingBudget 是 token 数而不是枚举，且各模型的合法区间不同，所以
          // 只在用户明确选了级别时才传；`auto` 省略它，等于交给模型动态
          // 决定，这也是降级重试的第一级。
          ...(thinking === "off"
            ? {}
            : {
                thinkingConfig: {
                  includeThoughts: true,
                  ...(thinking === "auto"
                    ? {}
                    : { thinkingBudget: GEMINI_BUDGET[thinking] }),
                },
              }),
        }
      : undefined
  return {
    url: `${trimSlash(o.baseUrl)}/v1beta/models/${encodeURIComponent(
      o.model
    )}:streamGenerateContent?alt=sse`,
    body: {
      contents,
      ...(system ? { systemInstruction: { parts: [{ text: system }] } } : {}),
      ...(generationConfig ? { generationConfig } : {}),
      ...(o.webSearch ? { tools: [{ google_search: {} }] } : {}),
    },
    directHeaders: {
      "x-goog-api-key": o.apiKey,
    },
  }
}

function prepare(
  o: ChatStreamOptions,
  thinking: ChatThinkingLevel
): Promise<PreparedRequest> {
  switch (o.protocol) {
    case "openai":
      return prepareOpenAi(o, thinking)
    case "claude":
      return prepareClaude(o, thinking)
    case "gemini":
      return prepareGemini(o, thinking)
  }
}

/** 一个 SSE 帧解析出的两路内容：`text` 进正文气泡，`reasoning` 进折叠的
 * 思考区。两者可能同时为空（心跳、usage 等无关帧）。 */
type Chunk = { text?: string; reasoning?: string }

function extractDelta(protocol: Protocol, json: unknown): Chunk {
  const j = json as Record<string, unknown>
  switch (protocol) {
    case "openai": {
      // Responses API stream events carry a `type` field; text chunks come
      // through as `response.output_text.delta` with a string `delta`.
      if (j.type === "response.output_text.delta") {
        return { text: typeof j.delta === "string" ? j.delta : undefined }
      }
      // 官方 Responses API 的推理摘要流。
      if (j.type === "response.reasoning_summary_text.delta") {
        return { reasoning: typeof j.delta === "string" ? j.delta : undefined }
      }
      // Tolerate relays that still emit chat-completions-style chunks via
      // the /v1/responses endpoint. 中转站的推理模型（DeepSeek-R1 等）把思考
      // 放在 `reasoning_content`；OpenRouter 系用 `reasoning`。
      const choices = (j.choices ?? []) as Array<{
        delta?: { content?: string; reasoning_content?: string; reasoning?: string }
      }>
      const d = choices[0]?.delta
      return {
        text: d?.content,
        reasoning: d?.reasoning_content ?? d?.reasoning,
      }
    }
    case "claude": {
      if (j.type === "content_block_delta") {
        const delta = j.delta as
          | { type?: string; text?: string; thinking?: string }
          | undefined
        if (delta?.type === "text_delta") return { text: delta.text }
        // extended thinking；`signature_delta` 是签名校验数据，不展示。
        if (delta?.type === "thinking_delta") return { reasoning: delta.thinking }
      }
      return {}
    }
    case "gemini": {
      // 思考片段是带 `thought: true` 的 part，必须与正文分开——否则会被当成
      // 正常回答混进气泡里。
      const cands = (j.candidates ?? []) as Array<{
        content?: { parts?: Array<{ text?: string; thought?: boolean }> }
      }>
      const parts = cands[0]?.content?.parts ?? []
      let text = ""
      let reasoning = ""
      for (const p of parts) {
        if (p.thought) reasoning += p.text ?? ""
        else text += p.text ?? ""
      }
      return { text: text || undefined, reasoning: reasoning || undefined }
    }
  }
}

async function sendRequest(
  o: ChatStreamOptions,
  prepared: PreparedRequest
): Promise<Response> {
  const payload = JSON.stringify(prepared.body)

  // Platform mode MUST go through the backend proxy: the server resolves the
  // admin-configured shared channel and charges site quota, and the browser can
  // never reach that upstream directly. Force proxy whenever usePlatform is set
  // so a stale `useProxy=false` BYOK setting can't leak us onto a direct fetch.
  if (o.useProxy || o.usePlatform) {
    const headers: Record<string, string> = {
      "Content-Type": "application/json",
    }
    // BYOK: forward upstream creds via headers. Platform mode: omit creds —
    // the server resolves an admin channel and charges site quota — but always
    // declare the model. Gemini bodies have no `model` field (it lives in the
    // URL, which platform mode doesn't send), so the header is the only way
    // the server can route the request.
    if (o.usePlatform) {
      headers["X-Upstream-Model"] = o.model
    } else {
      headers["X-Upstream-Url"] = prepared.url
      headers["X-Upstream-Key"] = o.apiKey
    }
    return fetch(`/api/proxy/${o.protocol}`, {
      method: "POST",
      headers,
      body: payload,
      credentials: "same-origin",
      signal: o.signal,
    })
  }
  return fetch(prepared.url, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Accept: "text/event-stream",
      ...prepared.directHeaders,
    },
    body: payload,
    signal: o.signal,
  })
}

/** 上游报的错是不是「不认识思考参数」。不支持思考的模型（gpt-4o、Claude 3.5、
 * 老 Gemini）会在 400 里点名这些字段，据此决定要不要去掉参数重试。选了具体
 * 级别时还会命中 effort / thinking_budget：部分中转站支持思考却不支持控制
 * 强度，那应该退到「思考，但不指定强度」而不是直接不思考。 */
function isThinkingRejected(status: number, body: string): boolean {
  if (status !== 400 && status !== 422) return false
  return /thinking|reasoning|budget_tokens|includeThoughts|effort|thinking_budget|thinkingBudget/i.test(
    body
  )
}

export async function streamChat(o: ChatStreamOptions): Promise<void> {
  const requested: ChatThinkingLevel = o.thinking ?? "auto"
  // 降级链：先按用户选的级别请求；被拒就退到 `auto`（只要思考、不说多
  // 少），因为不少中转站支持 thinking 却不认 effort / thinkingBudget；再被拒
  // 才彻底去掉思考参数。这样选了级别不会把原本能思考的模型降成不思考。
  // 对话按实际 token 用量事后计费，非 2xx 不产生扣费，重试不会重复计费。
  const chain: ChatThinkingLevel[] =
    requested === "off"
      ? ["off"]
      : requested === "auto"
        ? ["auto", "off"]
        : [requested, "auto", "off"]

  let res = await sendRequest(o, await prepare(o, chain[0]))
  for (let i = 1; i < chain.length && !res.ok; i++) {
    const text = await res.text().catch(() => "")
    if (!isThinkingRejected(res.status, text)) {
      throw new Error(`HTTP ${res.status}: ${text || res.statusText}`)
    }
    const next = await sendRequest(o, await prepare(o, chain[i]))
    // 最后一级也失败就把它的错误报出去：逗留第一次的 400 只会把“不支持
    // 思考”当成真正的失败原因。
    if (!next.ok && i === chain.length - 1) {
      const retryText = await next.text().catch(() => "")
      throw new Error(`HTTP ${next.status}: ${retryText || next.statusText}`)
    }
    res = next
  }
  if (!res.ok) {
    const text = await res.text().catch(() => "")
    throw new Error(`HTTP ${res.status}: ${text || res.statusText}`)
  }
  if (!res.body) throw new Error(`HTTP ${res.status}: 响应没有可读的流`)

  const reader = res.body.getReader()
  const decoder = new TextDecoder()
  let buffer = ""

  // State for the ticking "生成图像中…" placeholder shown while the hosted
  // image_generation tool is working. `imgPrefix` is the assistant content
  // snapshot taken at the moment generation started; the placeholder is
  // appended to it every tick and wiped when the tool completes.
  let imgPrefix: string | null = null
  let imgStartedAt = 0
  let imgTicker: ReturnType<typeof setInterval> | null = null

  let usageIn = 0
  let usageOut = 0
  const reportUsage = () => {
    if (o.onUsage && (usageIn > 0 || usageOut > 0)) o.onUsage(usageIn, usageOut)
  }

  const placeholderFor = (prefix: string, secs: number): string => {
    const sep = prefix ? "\n\n" : ""
    return `${prefix}${sep}🎨 生成图像中…（已等 ${secs}s）`
  }

  const startImgPlaceholder = () => {
    if (imgTicker != null || !o.patchAssistant) return
    imgStartedAt = Date.now()
    o.patchAssistant((prev) => {
      imgPrefix = prev
      return placeholderFor(prev, 0)
    })
    imgTicker = setInterval(() => {
      if (imgPrefix == null || !o.patchAssistant) return
      const secs = Math.floor((Date.now() - imgStartedAt) / 1000)
      const prefix = imgPrefix
      o.patchAssistant(() => placeholderFor(prefix, secs))
    }, 1000)
  }

  const stopImgPlaceholder = () => {
    if (imgTicker != null) {
      clearInterval(imgTicker)
      imgTicker = null
    }
    if (imgPrefix != null && o.patchAssistant) {
      const prefix = imgPrefix
      o.patchAssistant(() => prefix)
    }
    imgPrefix = null
  }

  while (true) {
    const { value, done } = await reader.read()
    if (done) break
    buffer += decoder.decode(value, { stream: true })

    let sep: number
    while ((sep = buffer.indexOf("\n\n")) !== -1) {
      const rawEvent = buffer.slice(0, sep)
      buffer = buffer.slice(sep + 2)
      for (const line of rawEvent.split("\n")) {
        if (!line.startsWith("data:")) continue
        const data = line.slice(5).trim()
        if (!data || data === "[DONE]") continue
        try {
          const json = JSON.parse(data) as Record<string, unknown>
          if (o.protocol === "openai") {
            const ty = json.type as string | undefined
            // Generation kicking off — show the ticking placeholder.
            if (ty === "response.output_item.added") {
              const item = json.item as { type?: string } | undefined
              if (item?.type === "image_generation_call") {
                startImgPlaceholder()
                continue
              }
            }
            // Generation finished — stop the ticker, persist the image,
            // and splice its markdown reference into the assistant bubble.
            if (ty === "response.output_item.done") {
              const item = json.item as
                | {
                    type?: string
                    result?: string
                    revised_prompt?: string
                  }
                | undefined
              if (item?.type === "image_generation_call") {
                stopImgPlaceholder()
                if (item.result) {
                  await persistImageGenerationCall(
                    item,
                    o.onDelta,
                    o.persistGeneratedImages !== false
                  )
                }
                continue
              }
            }
          }
          if (o.protocol === "openai") {
            // Responses API：完成事件带整段 usage。
            const resp = (json as { response?: { usage?: { input_tokens?: number; output_tokens?: number } } }).response
            if (json.type === "response.completed" && resp?.usage) {
              usageIn = resp.usage.input_tokens ?? usageIn
              usageOut = resp.usage.output_tokens ?? usageOut
              reportUsage()
            }
            // 兼容 chat-completions 风格中转站：顶层 usage 字段。
            const cc = json.usage as { prompt_tokens?: number; completion_tokens?: number } | undefined
            if (cc && (cc.prompt_tokens != null || cc.completion_tokens != null)) {
              usageIn = cc.prompt_tokens ?? usageIn
              usageOut = cc.completion_tokens ?? usageOut
              reportUsage()
            }
          } else if (o.protocol === "claude") {
            if (json.type === "message_start") {
              const msg = json.message as { usage?: { input_tokens?: number; output_tokens?: number } } | undefined
              if (msg?.usage) {
                usageIn = msg.usage.input_tokens ?? usageIn
                usageOut = msg.usage.output_tokens ?? usageOut
                reportUsage()
              }
            } else if (json.type === "message_delta") {
              const u = json.usage as { output_tokens?: number } | undefined
              if (u?.output_tokens != null) {
                usageOut = u.output_tokens
                reportUsage()
              }
            }
          } else if (o.protocol === "gemini") {
            const um = json.usageMetadata as { promptTokenCount?: number; candidatesTokenCount?: number } | undefined
            if (um) {
              usageIn = um.promptTokenCount ?? usageIn
              usageOut = um.candidatesTokenCount ?? usageOut
              reportUsage()
            }
          }
          const chunk = extractDelta(o.protocol, json)
          // 思考先于正文推送，保证 UI 能在正文开始前就展示思考流。
          if (chunk.reasoning) o.onReasoning?.(chunk.reasoning)
          if (chunk.text) o.onDelta(chunk.text)
        } catch {
          // tolerate keep-alive / non-json frames
        }
      }
    }
  }

  // If the stream ends mid-generation (upstream closed, aborted, etc.),
  // make sure the placeholder gets cleaned up.
  stopImgPlaceholder()
}

async function persistImageGenerationCall(
  item: { result?: string; revised_prompt?: string },
  onDelta: (delta: string) => void,
  persist: boolean
): Promise<void> {
  if (!item.result) return
  const alt = (item.revised_prompt || "image").replace(/[[\]]/g, "")
  if (!persist) {
    onDelta(`\n\n![${alt}](data:image/png;base64,${item.result})`)
    return
  }
  try {
    const res = await fetch("/api/images/save", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ b64: item.result, mime: "image/png" }),
      credentials: "same-origin",
    })
    if (res.ok) {
      const j = (await res.json()) as { path: string }
      onDelta(`\n\n![${alt}](${j.path})`)
    } else {
      const text = await res.text().catch(() => res.statusText)
      onDelta(`\n\n> 图像保存失败：${text || `HTTP ${res.status}`}`)
    }
  } catch (e) {
    onDelta(
      `\n\n> 图像保存失败：${e instanceof Error ? e.message : String(e)}`
    )
  }
}
