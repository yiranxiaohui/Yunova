export type Protocol = "openai" | "claude" | "gemini"
export type ImageProtocol = "openai" | "gemini"
export type UpstreamMode = "platform" | "byok"
// Type-only, so this does not create an import cycle with `chat-stream`, which
// imports `Protocol` and `trimSlash` from here at runtime.
import type { ChatThinkingLevel } from "./chat-stream"

export const GUEST_SETTINGS_ID = "guest"

export type UpstreamSettings = {
  // mode: use admin-configured platform channels (charge site quota), or BYOK
  chatMode: UpstreamMode
  imageMode: UpstreamMode

  // chat (protocol-aware) — only used in BYOK mode for baseUrl/apiKey,
  // but `protocol` and `model` are still meaningful in platform mode so the
  // user can pick which platform model to call.
  protocol: Protocol
  baseUrl: string
  apiKey: string
  model: string
  useProxy: boolean
  webSearch: boolean
  // How hard the model thinks before answering. "auto" reproduces what chat
  // did before this was选-able: ask for thinking without dictating how much.
  // Local-only, like `webSearch` — it is a per-device habit, not account
  // configuration, and cloud sync deliberately does not carry it.
  thinking: ChatThinkingLevel

  // image generation (protocol-aware, independent from chat config)
  imageProtocol: ImageProtocol
  imageBaseUrl: string
  imageApiKey: string
  imageModel: string
  imageUseProxy: boolean

  // Video generation. Custom requests go from the browser directly to the
  // local upstream; these values are never sent to or persisted by Yunova.
  videoMode: UpstreamMode
  videoBaseUrl: string
  videoApiKey: string
  videoModel: string

  cloudSync: boolean
}

export const PROTOCOL_META: Record<
  Protocol,
  { label: string; defaultBaseUrl: string; defaultModel: string; pathHint: string }
> = {
  openai: {
    label: "OpenAI 兼容",
    defaultBaseUrl: "https://api.openai.com",
    defaultModel: "gpt-4o-mini",
    pathHint: "/v1/chat/completions",
  },
  claude: {
    label: "Anthropic Claude",
    defaultBaseUrl: "https://api.anthropic.com",
    defaultModel: "claude-3-5-sonnet-latest",
    pathHint: "/v1/messages",
  },
  gemini: {
    label: "Google Gemini",
    defaultBaseUrl: "https://generativelanguage.googleapis.com",
    defaultModel: "gemini-2.0-flash",
    pathHint: "/v1beta/models/{model}:streamGenerateContent",
  },
}

export const IMAGE_PROTOCOL_META: Record<
  ImageProtocol,
  { label: string; defaultBaseUrl: string; defaultModel: string; pathHint: string }
> = {
  openai: {
    label: "OpenAI 兼容",
    defaultBaseUrl: "https://api.openai.com",
    defaultModel: "dall-e-3",
    pathHint: "/v1/images/generations",
  },
  gemini: {
    label: "Google Imagen",
    defaultBaseUrl: "https://generativelanguage.googleapis.com",
    defaultModel: "imagen-3.0-generate-002",
    pathHint: "/v1beta/models/{model}:predict",
  },
}

// Back-compat alias used in a few places.
export const IMAGE_DEFAULTS = {
  baseUrl: IMAGE_PROTOCOL_META.openai.defaultBaseUrl,
  model: IMAGE_PROTOCOL_META.openai.defaultModel,
  pathHint: IMAGE_PROTOCOL_META.openai.pathHint,
}

const EMPTY: UpstreamSettings = {
  chatMode: "platform",
  imageMode: "platform",
  protocol: "openai",
  baseUrl: "",
  apiKey: "",
  model: "",
  useProxy: true,
  webSearch: false,
  thinking: "auto",
  imageProtocol: "openai",
  imageBaseUrl: "",
  imageApiKey: "",
  imageModel: "",
  imageUseProxy: true,
  videoMode: "platform",
  videoBaseUrl: "",
  videoApiKey: "",
  videoModel: "",
  cloudSync: false,
}

const GUEST_EMPTY: UpstreamSettings = {
  ...EMPTY,
  chatMode: "byok",
  imageMode: "byok",
  videoMode: "byok",
}

function keyFor(userId: number | string) {
  return `yunova:upstream:v2:${userId}`
}

export function loadSettings(userId: number | string): UpstreamSettings {
  const fallback = userId === GUEST_SETTINGS_ID ? GUEST_EMPTY : EMPTY
  try {
    const raw = localStorage.getItem(keyFor(userId))
      ?? localStorage.getItem(`novachat:upstream:v2:${userId}`)
    if (!raw) return fallback
    const parsed = JSON.parse(raw) as Partial<UpstreamSettings>
    const merged: UpstreamSettings = { ...fallback, ...parsed }
    if (userId === GUEST_SETTINGS_ID) {
      merged.chatMode = "byok"
      merged.imageMode = "byok"
      merged.cloudSync = false
    }
    return merged
  } catch {
    return fallback
  }
}

export function saveSettings(userId: number | string, s: UpstreamSettings) {
  localStorage.setItem(keyFor(userId), JSON.stringify(s))
  localStorage.removeItem(`novachat:upstream:v2:${userId}`)
}

export function clearSettings(userId: number | string) {
  localStorage.removeItem(keyFor(userId))
  localStorage.removeItem(`novachat:upstream:v2:${userId}`)
}

export function trimSlash(url: string): string {
  return url.replace(/\/+$/, "")
}

export function isImageConfigured(s: UpstreamSettings): boolean {
  if (s.imageMode === "platform") return Boolean(s.imageModel)
  return Boolean(s.imageBaseUrl && s.imageApiKey && s.imageModel)
}

/**
 * Heuristic — returns `true` when the chosen model is likely to accept a
 * native web-search tool call. Name-based, not authoritative (each vendor's
 * /models endpoint does not expose tool-capability metadata). If this returns
 * `false` but the user leaves the toggle on, the request will still be sent
 * and the provider may return 400.
 */
export function likelyWebSearchCapable(
  protocol: Protocol,
  model: string
): boolean {
  const m = model.toLowerCase()
  if (!m) return false
  switch (protocol) {
    case "openai":
      // chat completions: *-search-preview family and gpt-5.x
      return m.includes("search-preview") || /^gpt-5(\b|-)/.test(m)
    case "claude":
      // all Claude 3.5+ / 4.x accept the web_search_20250305 server tool
      return (
        /claude-3-5/.test(m) ||
        /claude-3-7/.test(m) ||
        /claude-(sonnet|opus|haiku)-4/.test(m) ||
        /claude-4/.test(m)
      )
    case "gemini":
      // Gemini 2.x uses `google_search`; 1.5 uses a different field we don't send
      return /gemini-(2|3)/.test(m)
  }
}

// ---------------------------------------------------------------------------
// cloud sync
// ---------------------------------------------------------------------------

type CloudPayload = {
  protocol: Protocol
  base_url: string
  api_key: string
  model: string
  use_proxy: boolean
  image_protocol?: ImageProtocol | null
  image_base_url?: string | null
  image_api_key?: string | null
  image_model?: string | null
  image_use_proxy?: boolean | null
}

function toCloud(s: UpstreamSettings): CloudPayload {
  const hasImage = s.imageBaseUrl || s.imageApiKey || s.imageModel
  return {
    protocol: s.protocol,
    base_url: s.baseUrl,
    api_key: s.apiKey,
    model: s.model,
    use_proxy: s.useProxy,
    ...(hasImage
      ? {
          image_protocol: s.imageProtocol,
          image_base_url: s.imageBaseUrl || null,
          image_api_key: s.imageApiKey || null,
          image_model: s.imageModel || null,
          image_use_proxy: s.imageUseProxy,
        }
      : {}),
  }
}

function fromCloud(p: CloudPayload): Omit<UpstreamSettings, "cloudSync"> {
  const ip = p.image_protocol === "gemini" ? "gemini" : "openai"
  return {
    // Mode is a client-only preference; default to "platform" when cloud sync
    // omits it. Users can flip per-device.
    chatMode: "platform",
    imageMode: "platform",
    protocol: p.protocol,
    baseUrl: p.base_url ?? "",
    apiKey: p.api_key ?? "",
    model: p.model ?? "",
    useProxy: Boolean(p.use_proxy),
    imageProtocol: ip,
    imageBaseUrl: p.image_base_url ?? "",
    imageApiKey: p.image_api_key ?? "",
    imageModel: p.image_model ?? "",
    imageUseProxy: p.image_use_proxy == null ? true : Boolean(p.image_use_proxy),
    // Video custom API credentials intentionally remain local to this device.
    videoMode: "platform",
    videoBaseUrl: "",
    videoApiKey: "",
    videoModel: "",
    webSearch: false,
    thinking: "auto",
  }
}

export const settingsApi = {
  async fetch(): Promise<Omit<UpstreamSettings, "cloudSync"> | null> {
    const res = await fetch("/api/settings", { credentials: "same-origin" })
    if (res.status === 404) return null
    if (!res.ok) {
      throw new Error((await res.text().catch(() => res.statusText)) || `HTTP ${res.status}`)
    }
    const data = (await res.json()) as CloudPayload
    return fromCloud(data)
  },
  async save(s: UpstreamSettings): Promise<void> {
    const res = await fetch("/api/settings", {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(toCloud(s)),
      credentials: "same-origin",
    })
    if (!res.ok) {
      throw new Error((await res.text().catch(() => res.statusText)) || `HTTP ${res.status}`)
    }
  },
  async remove(): Promise<void> {
    const res = await fetch("/api/settings", {
      method: "DELETE",
      credentials: "same-origin",
    })
    if (!res.ok && res.status !== 404) {
      throw new Error((await res.text().catch(() => res.statusText)) || `HTTP ${res.status}`)
    }
  },
}

export async function loadEffectiveSettings(
  userId: number | string
): Promise<UpstreamSettings> {
  const local = loadSettings(userId)
  if (!local.cloudSync) return local
  try {
    const remote = await settingsApi.fetch()
    if (!remote) return local
    // Preserve local-only flags when merging cloud data back in.
    const merged: UpstreamSettings = {
      ...remote,
      chatMode: local.chatMode,
      imageMode: local.imageMode,
      webSearch: local.webSearch,
      thinking: local.thinking,
      videoMode: local.videoMode,
      videoBaseUrl: local.videoBaseUrl,
      videoApiKey: local.videoApiKey,
      videoModel: local.videoModel,
      cloudSync: true,
    }
    saveSettings(userId, merged)
    return merged
  } catch {
    return local
  }
}
