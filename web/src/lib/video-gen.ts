// Video generation API client. Platform mode uses Yunova's server API;
// local mode talks to the user's OpenAI-compatible service from the browser.

async function jsonOrThrow<T>(res: Response): Promise<T> {
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText)
    throw new Error(text || `HTTP ${res.status}`)
  }
  return res.json() as Promise<T>
}

export type SizeRule = { size: string; multiplier: number }

export type VideoModel = {
  model: string
  display_name: string | null
  base_credits: number
  per_second: number
  allowed_seconds: number[]
  size_rules: SizeRule[]
}

export type VideoJob = {
  token: string
  model: string
  prompt: string
  seconds: number
  size: string
  input_image_path: string | null
  status: "pending" | "running" | "completed" | "failed"
  progress: number
  video_path: string | null
  error: string | null
  cost_credits: number
  refunded: boolean
  created_at: string
  finished_at: string | null
  // Browser-local jobs never exist in Yunova's database.
  local?: true
  local_base_url?: string
  local_upstream_id?: string
  local_reference_name?: string | null
}

export type CreateVideoJobReq = {
  model: string
  prompt: string
  seconds: number
  size: string
  input_image_path?: string
}

export type VideoUpstreamConfig = {
  baseUrl: string
  apiKey?: string
}

type LocalVideoJob = VideoJob & {
  local: true
  local_base_url: string
  local_upstream_id: string
}

const CUSTOM_VIDEO_SECONDS = Array.from({ length: 15 }, (_, index) => index + 1)
const CUSTOM_VIDEO_SIZES: SizeRule[] = [
  { size: "1280x720", multiplier: 100 },
  { size: "720x1280", multiplier: 100 },
  { size: "1920x1080", multiplier: 100 },
  { size: "1080x1920", multiplier: 100 },
]
const LOCAL_JOBS_VERSION = 1
const LOCAL_JOBS_LIMIT = 48

function customModels(models: string[]): VideoModel[] {
  return models.filter(Boolean).map((model) => ({
    model,
    display_name: null,
    base_credits: 0,
    per_second: 0,
    allowed_seconds: CUSTOM_VIDEO_SECONDS,
    size_rules: CUSTOM_VIDEO_SIZES,
  }))
}

/** Same formula as videos::compute_cost: (base + per_second*s) * multiplier / 100. */
export function computeVideoCost(m: VideoModel, seconds: number, size: string): number | null {
  if (!m.allowed_seconds.includes(seconds)) return null
  const rule = m.size_rules.find((r) => r.size === size)
  if (!rule) return null
  return Math.round(((m.base_credits + m.per_second * seconds) * rule.multiplier) / 100)
}

export function isLocalVideoJob(job: VideoJob): job is LocalVideoJob {
  return job.local === true && Boolean(job.local_base_url && job.local_upstream_id)
}

function parseLocalModels(body: unknown): string[] {
  if (!body || typeof body !== "object") return []
  const value = body as { data?: unknown; models?: unknown }
  const fromData = Array.isArray(value.data)
    ? value.data.flatMap((item) => {
        if (typeof item === "string") return [item.trim()]
        if (!item || typeof item !== "object") return []
        const id = (item as { id?: unknown }).id
        return typeof id === "string" ? [id.trim()] : []
      })
    : []
  const fromModels = Array.isArray(value.models)
    ? value.models.flatMap((item) => (typeof item === "string" ? [item.trim()] : []))
    : []
  return [...new Set((fromData.length > 0 ? fromData : fromModels).filter(Boolean))].sort()
}

function normalizeBaseUrl(raw: string): string {
  const value = raw.trim().replace(/\/+$/, "")
  let parsed: URL
  try {
    parsed = new URL(value)
  } catch {
    throw new Error("本地视频 API Base URL 无效")
  }
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
    throw new Error("本地视频 API 仅支持 HTTP 或 HTTPS")
  }
  if (parsed.username || parsed.password) {
    throw new Error("请不要在本地视频 API URL 中填写账号或密码")
  }
  return value
}

function isLoopbackHost(hostname: string): boolean {
  const host = hostname.toLowerCase().replace(/^\[|\]$/g, "")
  return host === "localhost" || host === "::1" || host.startsWith("127.")
}

function localApiUrl(config: VideoUpstreamConfig, path: string): string {
  const base = normalizeBaseUrl(config.baseUrl)
  const parsed = new URL(base)
  if (
    typeof window !== "undefined" &&
    window.location.protocol === "https:" &&
    parsed.protocol === "http:" &&
    !isLoopbackHost(parsed.hostname)
  ) {
    throw new Error(
      "当前页面使用 HTTPS，浏览器会阻止直连 HTTP 局域网 API；请为本地 API 配置 HTTPS"
    )
  }
  const apiBase = /\/v1$/i.test(base) ? base : `${base}/v1`
  return `${apiBase}${path}`
}

function localHeaders(config: VideoUpstreamConfig): Headers {
  const headers = new Headers()
  const key = config.apiKey?.trim()
  if (key) headers.set("Authorization", `Bearer ${key}`)
  return headers
}

async function localFetch(
  config: VideoUpstreamConfig,
  path: string,
  init: RequestInit = {},
  timeoutMs = 30_000
): Promise<Response> {
  const url = localApiUrl(config, path)
  const controller = new AbortController()
  const timer = globalThis.setTimeout(() => controller.abort(), timeoutMs)
  try {
    return await fetch(url, {
      ...init,
      credentials: "omit",
      cache: "no-store",
      headers: init.headers ?? localHeaders(config),
      signal: controller.signal,
    })
  } catch (cause) {
    if (cause instanceof DOMException && cause.name === "AbortError") {
      throw new Error("本地视频 API 请求超时")
    }
    const origin = new URL(url).origin
    throw new Error(
      `浏览器无法直连 ${origin}；请确认服务已启动，并允许当前站点的 CORS 和局域网访问`
    )
  } finally {
    globalThis.clearTimeout(timer)
  }
}

async function localResponseError(response: Response): Promise<Error> {
  const text = await response.text().catch(() => "")
  let detail = text.trim()
  try {
    const parsed = JSON.parse(text) as { error?: unknown; message?: unknown }
    const error = parsed.error
    if (typeof error === "string") detail = error
    else if (error && typeof error === "object") {
      const message = (error as { message?: unknown }).message
      if (typeof message === "string") detail = message
    } else if (typeof parsed.message === "string") detail = parsed.message
  } catch {
    // Plain-text upstream errors are useful as-is.
  }
  const suffix = detail ? `：${detail.slice(0, 300)}` : ""
  return new Error(`本地视频 API 返回 ${response.status}${suffix}`)
}

function upstreamId(body: unknown): string | null {
  if (!body || typeof body !== "object") return null
  const id = (body as { id?: unknown }).id
  if (typeof id === "string" && id.trim()) return id.trim()
  if (typeof id === "number" && Number.isFinite(id)) return String(id)
  return null
}

function progressValue(body: unknown): number {
  if (!body || typeof body !== "object") return 0
  const progress = (body as { progress?: unknown }).progress
  return typeof progress === "number" && Number.isFinite(progress)
    ? Math.max(0, Math.min(100, Math.round(progress)))
    : 0
}

function localStatus(body: unknown): VideoJob["status"] {
  if (!body || typeof body !== "object") return "running"
  const raw = (body as { status?: unknown }).status
  if (typeof raw !== "string") return "running"
  switch (raw.toLowerCase()) {
    case "queued":
    case "pending":
      return "pending"
    case "in_progress":
    case "processing":
    case "running":
      return "running"
    case "completed":
    case "succeeded":
    case "success":
      return "completed"
    case "failed":
    case "error":
    case "cancelled":
    case "canceled":
      return "failed"
    default:
      return "running"
  }
}

function localFailure(body: unknown): string {
  if (!body || typeof body !== "object") return "本地视频 API 生成失败"
  const error = (body as { error?: unknown }).error
  if (typeof error === "string" && error.trim()) return error.trim()
  if (error && typeof error === "object") {
    const message = (error as { message?: unknown }).message
    if (typeof message === "string" && message.trim()) return message.trim()
  }
  return "本地视频 API 生成失败"
}

export async function listVideoModels(upstream?: VideoUpstreamConfig): Promise<VideoModel[]> {
  if (!upstream) {
    return jsonOrThrow(
      await fetch("/api/videos/models", {
        credentials: "same-origin",
      })
    )
  }
  const response = await localFetch(upstream, "/models", {
    headers: localHeaders(upstream),
  })
  if (!response.ok) throw await localResponseError(response)
  return customModels(parseLocalModels(await response.json()))
}

export async function createVideoJob(req: CreateVideoJobReq): Promise<{ token: string; cost: number }> {
  return jsonOrThrow(
    await fetch("/api/videos/jobs", {
      method: "POST",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(req),
    })
  )
}

export async function getVideoJob(token: string): Promise<VideoJob> {
  return jsonOrThrow(
    await fetch(`/api/videos/jobs/${encodeURIComponent(token)}`, {
      credentials: "same-origin",
    })
  )
}

export async function createLocalVideoJob(
  req: CreateVideoJobReq,
  upstream: VideoUpstreamConfig,
  inputReference?: File
): Promise<LocalVideoJob> {
  const form = new FormData()
  form.set("model", req.model)
  form.set("prompt", req.prompt)
  form.set("seconds", String(req.seconds))
  form.set("size", req.size)
  if (inputReference) {
    form.set("input_reference", inputReference, inputReference.name)
  }
  const response = await localFetch(
    upstream,
    "/videos",
    { method: "POST", headers: localHeaders(upstream), body: form },
    180_000
  )
  if (!response.ok) throw await localResponseError(response)
  const body: unknown = await response.json()
  const id = upstreamId(body)
  if (!id) throw new Error("本地视频 API 响应缺少 id")
  const status = localStatus(body)
  const now = new Date().toISOString()
  return {
    token: `local-${crypto.randomUUID()}`,
    model: req.model,
    prompt: req.prompt,
    seconds: req.seconds,
    size: req.size,
    input_image_path: null,
    status: status === "completed" ? "running" : status,
    progress: progressValue(body),
    video_path: null,
    error: status === "failed" ? localFailure(body) : null,
    cost_credits: 0,
    refunded: false,
    created_at: now,
    finished_at: status === "failed" ? now : null,
    local: true,
    local_base_url: normalizeBaseUrl(upstream.baseUrl),
    local_upstream_id: id,
    local_reference_name: inputReference?.name ?? null,
  }
}

export async function getLocalVideoJob(
  job: LocalVideoJob,
  upstream: VideoUpstreamConfig
): Promise<LocalVideoJob> {
  const id = encodeURIComponent(job.local_upstream_id)
  const response = await localFetch(upstream, `/videos/${id}`, {
    headers: localHeaders(upstream),
  })
  if (!response.ok) throw await localResponseError(response)
  const body: unknown = await response.json()
  const status = localStatus(body)
  const progress = progressValue(body)
  if (status === "failed") {
    return {
      ...job,
      status,
      progress,
      error: localFailure(body),
      finished_at: new Date().toISOString(),
    }
  }
  if (status !== "completed") {
    return { ...job, status, progress, error: null }
  }

  const content = await localFetch(
    upstream,
    `/videos/${id}/content`,
    { headers: localHeaders(upstream) },
    180_000
  )
  if (!content.ok) throw await localResponseError(content)
  const blob = await content.blob()
  if (blob.size === 0) throw new Error("本地视频 API 返回了空视频")
  return {
    ...job,
    status: "completed",
    progress: 100,
    video_path: URL.createObjectURL(blob),
    error: null,
    finished_at: new Date().toISOString(),
  }
}

export async function listVideoJobs(page: number): Promise<{ jobs: VideoJob[]; has_more: boolean }> {
  const res = await fetch(`/api/videos/jobs?page=${page}`, { credentials: "same-origin" })
  return jsonOrThrow(res)
}

export async function deleteVideoJob(token: string): Promise<void> {
  const res = await fetch(`/api/videos/jobs/${encodeURIComponent(token)}`, {
    method: "DELETE",
    credentials: "same-origin",
  })
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText)
    throw new Error(text || `HTTP ${res.status}`)
  }
}

function localJobsKey(owner: number | string): string {
  return `yunova:local-video-jobs:v${LOCAL_JOBS_VERSION}:${owner}`
}

export function loadLocalVideoJobs(owner: number | string): VideoJob[] {
  if (typeof localStorage === "undefined") return []
  try {
    const raw = localStorage.getItem(localJobsKey(owner))
      ?? localStorage.getItem(`novachat:local-video-jobs:v${LOCAL_JOBS_VERSION}:${owner}`)
    const parsed = JSON.parse(raw ?? "[]") as unknown
    if (!Array.isArray(parsed)) return []
    return parsed
      .filter((item): item is VideoJob => {
        if (!item || typeof item !== "object") return false
        const job = item as Partial<VideoJob>
        return (
          job.local === true &&
          typeof job.token === "string" &&
          typeof job.local_base_url === "string" &&
          typeof job.local_upstream_id === "string"
        )
      })
      .slice(0, LOCAL_JOBS_LIMIT)
      .map((job) =>
        job.status === "completed"
          ? { ...job, status: "running", progress: 99, video_path: null, finished_at: null }
          : { ...job, video_path: null }
      )
  } catch {
    return []
  }
}

export function saveLocalVideoJobs(owner: number | string, jobs: VideoJob[]): void {
  if (typeof localStorage === "undefined") return
  const localJobs = jobs
    .filter(isLocalVideoJob)
    .slice(0, LOCAL_JOBS_LIMIT)
    .map((job) => ({ ...job, video_path: null }))
  localStorage.setItem(localJobsKey(owner), JSON.stringify(localJobs))
  localStorage.removeItem(`novachat:local-video-jobs:v${LOCAL_JOBS_VERSION}:${owner}`)
}
