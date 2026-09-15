import { useEffect, useRef, useState } from "react"
import { Link } from "react-router-dom"
import { formatQuota } from "@/lib/quota"
import {
  ArrowLeft,
  Clapperboard,
  Download,
  ImagePlus,
  Loader2,
  Menu,
  RefreshCw,
  Scissors,
  Trash2,
  X,
} from "lucide-react"
import { cn } from "@/lib/utils"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Textarea } from "@/components/ui/textarea"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { useConfirm } from "@/lib/confirm-context"
import {
  computeVideoCost,
  createLocalVideoJob,
  createVideoJob,
  deleteVideoJob,
  getLocalVideoJob,
  getVideoJob,
  isLocalVideoJob,
  loadLocalVideoJobs,
  listVideoJobs,
  listVideoModels,
  saveLocalVideoJobs,
  type CreateVideoJobReq,
  type VideoJob,
  type VideoModel,
  type VideoUpstreamConfig,
} from "@/lib/video-gen"
import {
  GUEST_SETTINGS_ID,
  loadEffectiveSettings,
  loadSettings,
  saveSettings,
  type UpstreamMode,
  type UpstreamSettings,
} from "@/lib/settings"
import { useAuth } from "@/lib/auth-context"
import {
  consecutiveSecondsRange,
  videoSizeLabel,
} from "@/lib/video-capabilities"

const POLL_MS = 5000

function fileToBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const r = new FileReader()
    r.onload = () => {
      const result = r.result as string
      // data:<mime>;base64,<payload> — /api/images/save 只要 payload。
      const idx = result.indexOf(",")
      resolve(idx >= 0 ? result.slice(idx + 1) : result)
    }
    r.onerror = () => reject(r.error ?? new Error("读取失败"))
    r.readAsDataURL(file)
  })
}

async function uploadRefImage(file: File): Promise<string> {
  const b64 = await fileToBase64(file)
  const res = await fetch("/api/images/save", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ b64, mime: file.type || "image/png" }),
    credentials: "same-origin",
  })
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText)
    throw new Error(text || `HTTP ${res.status}`)
  }
  const j = (await res.json()) as { path: string }
  return j.path
}

async function downloadRefImage(path: string): Promise<File> {
  const response = await fetch(path, { credentials: "same-origin" })
  if (!response.ok) throw new Error("读取原参考图失败")
  const blob = await response.blob()
  const name = path.split("/").pop()?.split("?")[0] || "reference-image"
  return new File([blob], name, { type: blob.type || "image/png" })
}

function firstSecondsAndSize(m: VideoModel): { seconds: number | null; size: string | null } {
  const range = consecutiveSecondsRange(m.allowed_seconds)
  const preferredSeconds =
    range && range.min <= 8 && range.max >= 8 ? 8 : m.allowed_seconds[0]
  return {
    seconds: preferredSeconds ?? null,
    size: m.size_rules[0]?.size ?? null,
  }
}

function customModel(model: string): VideoModel {
  return {
    model,
    display_name: null,
    base_micro_quota: 0,
    per_second_micro_quota: 0,
    allowed_seconds: Array.from({ length: 15 }, (_, index) => index + 1),
    size_rules: [
      { size: "1280x720", multiplier: 100 },
      { size: "720x1280", multiplier: 100 },
      { size: "1920x1080", multiplier: 100 },
      { size: "1080x1920", multiplier: 100 },
    ],
  }
}

function statusLabel(job: VideoJob): string {
  switch (job.status) {
    case "pending":
      return "排队中"
    case "running":
      return "生成中"
    case "completed":
      return "已完成"
    case "failed":
      return "失败"
    default:
      return job.status
  }
}

export default function VideoStudioPage() {
  const { confirm } = useConfirm()
  const auth = useAuth()
  const user = auth.state.status === "authed" ? auth.state.user : null
  const settingsOwner = user?.id ?? GUEST_SETTINGS_ID
  const initialVideoSettings = loadSettings(GUEST_SETTINGS_ID)
  const settingsRef = useRef<UpstreamSettings>(initialVideoSettings)
  const [settingsReady, setSettingsReady] = useState(false)
  const [videoMode, setVideoMode] = useState<UpstreamMode>(initialVideoSettings.videoMode)
  const [videoBaseUrl, setVideoBaseUrl] = useState(initialVideoSettings.videoBaseUrl)
  const [videoApiKey, setVideoApiKey] = useState(initialVideoSettings.videoApiKey)
  const [videoModel, setVideoModel] = useState(initialVideoSettings.videoModel)

  const [models, setModels] = useState<VideoModel[]>([])
  const [modelsLoading, setModelsLoading] = useState(false)
  const [model, setModel] = useState<string>("")
  const [seconds, setSeconds] = useState<number | null>(null)
  const [size, setSize] = useState<string | null>(null)
  const [prompt, setPrompt] = useState("")
  const [refImage, setRefImage] = useState<string | null>(null)
  const [refImageFile, setRefImageFile] = useState<File | null>(null)
  const [refImageUploading, setRefImageUploading] = useState(false)
  const fileRef = useRef<HTMLInputElement>(null)
  const referencePreviewUrlRef = useRef<string | null>(null)
  const localJobFilesRef = useRef(new Map<string, File>())
  const localVideoUrlsRef = useRef(new Set<string>())
  const hydratedOwnerRef = useRef<number | string | null>(null)

  const [jobs, setJobs] = useState<VideoJob[]>([])
  const [hasMore, setHasMore] = useState(false)
  const [page, setPage] = useState(0)
  const [loadingMore, setLoadingMore] = useState(false)

  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [mobileNavOpen, setMobileNavOpen] = useState(false)

  const [deletingToken, setDeletingToken] = useState<string | null>(null)
  const [regeneratingToken, setRegeneratingToken] = useState<string | null>(null)

  useEffect(() => {
    const videoUrls = localVideoUrlsRef.current
    const referenceUrl = referencePreviewUrlRef
    return () => {
      if (referenceUrl.current) URL.revokeObjectURL(referenceUrl.current)
      for (const url of videoUrls) URL.revokeObjectURL(url)
      videoUrls.clear()
    }
  }, [])

  useEffect(() => {
    let cancelled = false
    void loadEffectiveSettings(settingsOwner).then((settings) => {
      if (cancelled) return
      settingsRef.current = settings
      setVideoMode(settings.videoMode)
      setVideoBaseUrl(settings.videoBaseUrl)
      setVideoApiKey(settings.videoApiKey)
      setVideoModel(settings.videoModel)
      hydratedOwnerRef.current = settingsOwner
      const localJobs = loadLocalVideoJobs(settingsOwner)
      setJobs((previous) => [
        ...localJobs,
        ...previous.filter((job) => !isLocalVideoJob(job)),
      ])
      setSettingsReady(true)
    })
    return () => {
      cancelled = true
    }
  }, [settingsOwner])

  useEffect(() => {
    if (settingsReady && hydratedOwnerRef.current === settingsOwner) {
      saveLocalVideoJobs(settingsOwner, jobs)
    }
  }, [jobs, settingsOwner, settingsReady])

  function updateVideoSetting<K extends keyof Pick<UpstreamSettings, "videoMode" | "videoBaseUrl" | "videoApiKey" | "videoModel">>(
    key: K,
    value: UpstreamSettings[K]
  ) {
    const next = { ...settingsRef.current, [key]: value }
    settingsRef.current = next
    saveSettings(settingsOwner, next)
    if (key === "videoMode") setVideoMode(value as UpstreamMode)
    if (key === "videoBaseUrl") setVideoBaseUrl(value as string)
    if (key === "videoApiKey") setVideoApiKey(value as string)
    if (key === "videoModel") setVideoModel(value as string)
  }

  function clearReferenceImage() {
    if (referencePreviewUrlRef.current) {
      URL.revokeObjectURL(referencePreviewUrlRef.current)
      referencePreviewUrlRef.current = null
    }
    setRefImage(null)
    setRefImageFile(null)
  }

  function setLocalReferenceImage(file: File) {
    clearReferenceImage()
    const url = URL.createObjectURL(file)
    referencePreviewUrlRef.current = url
    setRefImage(url)
    setRefImageFile(file)
  }

  function customUpstream(): VideoUpstreamConfig | undefined {
    if (videoMode !== "byok" || !videoBaseUrl.trim()) return undefined
    return {
      baseUrl: videoBaseUrl,
      apiKey: videoApiKey,
    }
  }

  async function reloadModels() {
    setModelsLoading(true)
    setError(null)
    try {
      const upstream = customUpstream()
      const list =
        videoMode === "byok" && !upstream ? [] : await listVideoModels(upstream)
      setModels(list)
      if (list.length > 0) {
        const selected = list.find((item) => item.model === model) ?? list[0]!
        const defaults = firstSecondsAndSize(selected)
        setModel(selected.model)
        setSeconds((previous) =>
          previous != null && selected.allowed_seconds.includes(previous)
            ? previous
            : defaults.seconds
        )
        setSize((previous) =>
          previous != null &&
          selected.size_rules.some((rule) => rule.size === previous)
            ? previous
            : defaults.size
        )
      } else {
        if (videoMode === "byok" && videoModel.trim()) {
          const fallback = customModel(videoModel.trim())
          setModels([fallback])
          setModel(fallback.model)
          const defaults = firstSecondsAndSize(fallback)
          setSeconds(defaults.seconds)
          setSize(defaults.size)
        } else {
          setModel("")
          setSeconds(null)
          setSize(null)
        }
      }
    } catch (e) {
      if (videoMode === "byok" && videoModel.trim()) {
        const fallback = customModel(videoModel.trim())
        setModels([fallback])
        setModel(fallback.model)
        const defaults = firstSecondsAndSize(fallback)
        setSeconds(defaults.seconds)
        setSize(defaults.size)
      }
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setModelsLoading(false)
    }
  }

  async function loadJobs(p: number, append: boolean) {
    try {
      const { jobs: rows, has_more } = await listVideoJobs(p)
      setJobs((prev) => {
        const localJobs = prev.filter(isLocalVideoJob)
        const serverJobs = prev.filter((job) => !isLocalVideoJob(job))
        return append ? [...localJobs, ...serverJobs, ...rows] : [...localJobs, ...rows]
      })
      setHasMore(has_more)
      setPage(p)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }

  useEffect(() => {
    if (!settingsReady) return
    // Initial model/job loading is an external synchronization kicked off
    // after the authenticated settings have hydrated.
    // eslint-disable-next-line react-hooks/set-state-in-effect
    void reloadModels()
    void loadJobs(0, false)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [settingsReady])

  // 切换模型时把时长/分辨率重置为该模型规则里的第一项。
  function handleModelChange(next: string) {
    setModel(next)
    if (videoMode === "byok") updateVideoSetting("videoModel", next)
    const m = models.find((x) => x.model === next)
    if (m) {
      const { seconds: s, size: sz } = firstSecondsAndSize(m)
      setSeconds(s)
      setSize(sz)
    }
  }

  async function handlePickFile(e: React.ChangeEvent<HTMLInputElement>) {
    const f = e.target.files?.[0]
    e.target.value = ""
    if (!f) return
    setRefImageUploading(true)
    setError(null)
    try {
      if (videoMode === "byok") {
        setLocalReferenceImage(f)
        return
      }
      const path = await uploadRefImage(f)
      clearReferenceImage()
      setRefImage(path)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setRefImageUploading(false)
    }
  }

  const activeModel = models.find((m) => m.model === model) ??
    (videoMode === "byok" && model.trim() ? customModel(model.trim()) : undefined)
  const durationRange = activeModel
    ? consecutiveSecondsRange(activeModel.allowed_seconds)
    : null
  const cost =
    activeModel && seconds != null && size?.trim()
      ? videoMode === "byok"
        ? 0
        : computeVideoCost(activeModel, seconds, size)
      : null

  function rememberLocalVideo(job: VideoJob) {
    if (isLocalVideoJob(job) && job.video_path?.startsWith("blob:")) {
      localVideoUrlsRef.current.add(job.video_path)
    }
  }

  async function enqueueVideoJob(
    request: CreateVideoJobReq,
    upstream?: VideoUpstreamConfig,
    inputReference?: File
  ) {
    if (upstream) {
      const job = await createLocalVideoJob(request, upstream, inputReference)
      if (inputReference) localJobFilesRef.current.set(job.token, inputReference)
      setJobs((prev) => [job, ...prev.filter((item) => item.token !== job.token)])
      return
    }
    const { token } = await createVideoJob(request)
    const job = await getVideoJob(token)
    setJobs((prev) => [job, ...prev.filter((item) => item.token !== job.token)])
  }

  async function handleSubmit() {
    if (!activeModel || seconds == null || size == null) return
    const upstream = customUpstream()
    if (videoMode === "byok" && !upstream) {
      setError("请先填写自定义视频 API Base URL")
      return
    }
    const p = prompt.trim()
    if (!p) return
    setSubmitting(true)
    setError(null)
    try {
      await enqueueVideoJob({
        model: activeModel.model,
        prompt: p,
        seconds,
        size,
        input_image_path: videoMode === "platform" ? refImage ?? undefined : undefined,
      }, upstream, videoMode === "byok" ? refImageFile ?? undefined : undefined)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setSubmitting(false)
    }
  }

  // 轮询：只要 jobs 里还有 pending/running 就每 5s 拉一次状态，合并回列表；
  // 没有 in-flight 任务或组件卸载即清 interval。
  useEffect(() => {
    const inFlight = jobs.filter((j) => j.status === "pending" || j.status === "running")
    if (inFlight.length === 0) return
    const id = window.setInterval(() => {
      void (async () => {
        for (const j of inFlight) {
          try {
            const updated = isLocalVideoJob(j)
              ? await getLocalVideoJob(j, {
                  baseUrl: j.local_base_url,
                  apiKey: settingsRef.current.videoApiKey,
                })
              : await getVideoJob(j.token)
            rememberLocalVideo(updated)
            setJobs((prev) => prev.map((x) => (x.token === updated.token ? updated : x)))
          } catch (pollError) {
            if (isLocalVideoJob(j)) {
              const message = pollError instanceof Error ? pollError.message : String(pollError)
              setJobs((prev) => {
                const current = prev.find((item) => item.token === j.token)
                if (current?.error === message) return prev
                return prev.map((item) =>
                  item.token === j.token ? { ...item, error: message } : item
                )
              })
            }
            // Polling failures are transient; retry this job on the next tick.
          }
        }
      })()
    }, POLL_MS)
    return () => window.clearInterval(id)
  }, [jobs])

  async function regenerate(job: VideoJob) {
    if (submitting || regeneratingToken !== null) return

    // Keep the form in sync with the exact request being retried. In
    // particular, restoring input_image_path makes this a real image-to-video
    // retry instead of silently degrading it to text-to-video.
    setModel(job.model)
    setSeconds(job.seconds)
    setSize(job.size)
    setPrompt(job.prompt)
    clearReferenceImage()

    if (isLocalVideoJob(job)) {
      updateVideoSetting("videoMode", "byok")
      updateVideoSetting("videoBaseUrl", job.local_base_url)
      updateVideoSetting("videoModel", job.model)
      const inputReference = localJobFilesRef.current.get(job.token)
      if (inputReference) setLocalReferenceImage(inputReference)
    } else {
      setRefImage(job.input_image_path)
    }

    setRegeneratingToken(job.token)
    setError(null)
    try {
      const localJob = isLocalVideoJob(job)
      const upstream = localJob
        ? { baseUrl: job.local_base_url, apiKey: settingsRef.current.videoApiKey }
        : customUpstream()
      if ((localJob || videoMode === "byok") && !upstream) {
        throw new Error("请先填写自定义视频 API Base URL")
      }
      const inputReference = localJob
        ? localJobFilesRef.current.get(job.token)
        : videoMode === "byok" && job.input_image_path
          ? await downloadRefImage(job.input_image_path)
          : undefined
      if (!localJob && inputReference) setLocalReferenceImage(inputReference)
      if (localJob && job.local_reference_name && !inputReference) {
        throw new Error("本地参考图未保留，请重新选择参考图后再生成")
      }
      await enqueueVideoJob({
        model: job.model,
        prompt: job.prompt,
        seconds: job.seconds,
        size: job.size,
        input_image_path: upstream ? undefined : job.input_image_path ?? undefined,
      }, upstream, inputReference)
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e)
      if (!isLocalVideoJob(job)) await loadJobs(0, false)
      setError(message)
    } finally {
      setRegeneratingToken(null)
    }
  }

  async function removeJob(job: VideoJob) {
    const ok = await confirm({
      title: "删除这条视频记录？",
      description: isLocalVideoJob(job)
        ? "只会删除当前浏览器中的记录。"
        : "此操作不可撤销。",
      confirmText: "删除",
      destructive: true,
    })
    if (!ok) return
    setDeletingToken(job.token)
    try {
      if (isLocalVideoJob(job)) {
        if (job.video_path?.startsWith("blob:")) {
          URL.revokeObjectURL(job.video_path)
          localVideoUrlsRef.current.delete(job.video_path)
        }
        localJobFilesRef.current.delete(job.token)
      } else {
        await deleteVideoJob(job.token)
      }
      setJobs((prev) => prev.filter((x) => x.token !== job.token))
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setDeletingToken(null)
    }
  }

  return (
    <div className="flex h-svh bg-background text-foreground">
      {mobileNavOpen && (
        <button
          type="button"
          aria-label="关闭参数面板"
          onClick={() => setMobileNavOpen(false)}
          className="fixed inset-0 z-30 bg-black/50 md:hidden"
        />
      )}
      <aside
        className={cn(
          "fixed inset-y-0 left-0 z-40 flex h-full w-80 max-w-[85vw] flex-col border-r border-sidebar-border bg-sidebar text-sidebar-foreground transition-transform",
          mobileNavOpen ? "translate-x-0 shadow-2xl" : "-translate-x-full",
          "md:static md:w-80 md:max-w-none md:shrink-0 md:translate-x-0 md:shadow-none"
        )}
      >
        <div className="flex flex-col gap-1 px-3 pb-2 pt-4">
          <Link
            to="/"
            className="flex items-center gap-2 rounded-md px-2 py-1.5 text-xs text-muted-foreground transition-colors hover:bg-sidebar-accent/60 hover:text-sidebar-accent-foreground"
          >
            <ArrowLeft className="size-3.5" /> 返回对话
          </Link>
        </div>

        <div className="nc-scroll flex-1 overflow-y-auto px-4 pb-4">
          <h2 className="mb-2 flex items-center gap-2 text-sm font-semibold">
            <Clapperboard className="size-4 text-primary" /> 生成参数
          </h2>

          <div className="mb-4 flex flex-col gap-2 rounded-md border border-border bg-muted/20 p-3">
            <Label className="text-xs">生成来源</Label>
            <div className="grid grid-cols-2 gap-1.5">
              {(["platform", "byok"] as const).map((next) => (
                <button
                  key={next}
                  type="button"
                  onClick={() => {
                    if (next === videoMode) return
                    if (next === "platform" && !user) {
                      setError("登录后才能使用云端额度模式")
                      return
                    }
                    clearReferenceImage()
                    updateVideoSetting("videoMode", next)
                    setModels([])
                    setModel(next === "byok" ? videoModel : "")
                  }}
                  className={cn(
                    "rounded-md border px-2 py-1.5 text-xs transition-colors",
                    videoMode === next
                      ? "border-primary bg-primary/10 text-foreground"
                      : "border-border bg-background hover:bg-accent"
                  )}
                >
                  {next === "platform" ? "云端额度" : "本地 API"}
                </button>
              ))}
            </div>
            <p className="text-[11px] leading-relaxed text-muted-foreground">
              {videoMode === "platform"
                ? "使用管理员配置的 OpenAI 兼容视频渠道，按定价规则扣额度。"
                : "当前浏览器直接请求本地视频服务，不经过 Yunova 服务器。"}
            </p>
            {videoMode === "byok" && (
              <div className="mt-1 flex flex-col gap-2">
                <Input
                  aria-label="自定义视频 API Base URL"
                  placeholder="http://127.0.0.1:8000"
                  value={videoBaseUrl}
                  onChange={(e) => updateVideoSetting("videoBaseUrl", e.target.value)}
                  className="h-8 text-xs"
                />
                <Input
                  aria-label="自定义视频 API Key"
                  type="text"
                  autoComplete="off"
                  spellCheck={false}
                  placeholder="API Key（本地服务可留空）"
                  value={videoApiKey}
                  onChange={(e) => updateVideoSetting("videoApiKey", e.target.value)}
                  className="h-8 text-xs"
                />
                <div className="flex gap-1.5">
                  <Input
                    aria-label="自定义视频模型"
                    placeholder="模型 ID，例如 wan2.1"
                    value={videoModel}
                    onChange={(e) => {
                      const nextModel = e.target.value
                      updateVideoSetting("videoModel", nextModel)
                      setModel(nextModel)
                      if (nextModel.trim()) {
                        const defaults = firstSecondsAndSize(customModel(nextModel.trim()))
                        setSeconds((previous) => previous ?? defaults.seconds)
                        setSize((previous) => previous ?? defaults.size)
                      }
                    }}
                    className="h-8 min-w-0 flex-1 text-xs"
                  />
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    onClick={() => void reloadModels()}
                    disabled={modelsLoading || !videoBaseUrl.trim()}
                    className="h-8 shrink-0 px-2 text-xs"
                  >
                    <RefreshCw className={cn("size-3", modelsLoading && "animate-spin")} />
                    加载模型
                  </Button>
                </div>
                <p className="text-[10px] leading-relaxed text-muted-foreground">
                  本地服务需提供 <code>/v1/videos</code> 并允许当前站点跨域访问；模型列表不可用时可手动填写模型。原生 ComfyUI 需先配置 OpenAI 兼容层。
                </p>
              </div>
            )}
          </div>

          {models.length === 0 && !modelsLoading && (
            <p className="mb-3 rounded-md border border-dashed border-border bg-muted/30 px-3 py-4 text-center text-xs text-muted-foreground">
              {videoMode === "byok"
                ? "没有可用的本地模型，请手动填写模型 ID"
                : "管理员尚未配置视频模型"}
            </p>
          )}

          <div className="mb-3 flex flex-col gap-1.5">
            <div className="flex items-center justify-between">
              <Label className="text-xs">模型</Label>
              <button
                type="button"
                onClick={() => void reloadModels()}
                disabled={modelsLoading}
                className="inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[11px] text-muted-foreground hover:bg-accent hover:text-foreground"
              >
                <RefreshCw className={cn("size-3", modelsLoading && "animate-spin")} />
                {modelsLoading ? "加载中…" : "刷新"}
              </button>
            </div>
            <Select value={model} onValueChange={handleModelChange} disabled={models.length === 0}>
              <SelectTrigger className="w-full">
                <SelectValue placeholder="选择模型" />
              </SelectTrigger>
              <SelectContent>
                {models.map((m) => (
                  <SelectItem key={m.model} value={m.model}>
                    {m.display_name ?? m.model}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>

          {activeModel && (
            <>
              <div className="mb-3 flex flex-col gap-1.5">
                <div className="flex items-center justify-between gap-2">
                  <Label className="text-xs">时长</Label>
                  {durationRange && (
                    <span className="text-xs font-medium tabular-nums text-primary">
                      {seconds ?? durationRange.min} 秒
                    </span>
                  )}
                </div>
                {videoMode === "byok" ? (
                  <Input
                    type="number"
                    min={1}
                    max={3600}
                    step={1}
                    value={seconds ?? ""}
                    onChange={(e) => {
                      const value = Number(e.target.value)
                      setSeconds(Number.isFinite(value) ? value : null)
                    }}
                    aria-label="视频时长（秒）"
                    placeholder="1 到 3600 秒"
                  />
                ) : durationRange ? (
                  <>
                    <input
                      type="range"
                      min={durationRange.min}
                      max={durationRange.max}
                      step={1}
                      value={seconds ?? durationRange.min}
                      onChange={(e) => setSeconds(Number(e.target.value))}
                      aria-label="视频时长（秒）"
                      className="h-2 w-full cursor-pointer accent-primary"
                    />
                    <div className="flex justify-between text-[11px] tabular-nums text-muted-foreground">
                      <span>{durationRange.min} 秒</span>
                      <span>可逐秒选择</span>
                      <span>{durationRange.max} 秒</span>
                    </div>
                  </>
                ) : (
                  <div className="flex flex-wrap gap-1.5">
                    {activeModel.allowed_seconds.map((s) => (
                      <Button
                        key={s}
                        type="button"
                        size="sm"
                        variant={seconds === s ? "default" : "outline"}
                        onClick={() => setSeconds(s)}
                      >
                        {s} 秒
                      </Button>
                    ))}
                  </div>
                )}
              </div>

              <div className="mb-3 flex flex-col gap-1.5">
                <Label className="text-xs">分辨率</Label>
                {videoMode === "byok" ? (
                  <Input
                    value={size ?? ""}
                    onChange={(e) => setSize(e.target.value)}
                    placeholder="例如 1280x720 或 512x512"
                    aria-label="视频分辨率"
                  />
                ) : (
                  <Select
                    value={size ?? undefined}
                    onValueChange={setSize}
                    disabled={activeModel.size_rules.length === 0}
                  >
                    <SelectTrigger className="w-full">
                      <SelectValue placeholder="选择分辨率" />
                    </SelectTrigger>
                    <SelectContent>
                      {activeModel.size_rules.map((r) => (
                        <SelectItem key={r.size} value={r.size}>
                          {videoSizeLabel(activeModel.model, r.size)}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                )}
              </div>
            </>
          )}

          <div className="mb-3 flex flex-col gap-1.5">
            <Label className="text-xs">
              参考图 <span className="text-muted-foreground">(可选，图生视频)</span>
            </Label>
            {refImage ? (
              <div className="group relative w-24">
                <img
                  src={refImage}
                  alt=""
                  className="aspect-square w-24 rounded-md border border-border object-cover"
                />
                <button
                  type="button"
                  onClick={clearReferenceImage}
                  aria-label="移除参考图"
                  className="absolute -right-1 -top-1 grid size-5 place-items-center rounded-full bg-background/90 text-foreground shadow ring-1 ring-border hover:bg-destructive hover:text-destructive-foreground"
                >
                  <X className="size-3" />
                </button>
              </div>
            ) : (
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => fileRef.current?.click()}
                disabled={refImageUploading}
                className="justify-start gap-2"
              >
                {refImageUploading ? (
                  <Loader2 className="size-4 animate-spin" />
                ) : (
                  <ImagePlus className="size-4" />
                )}
                {refImageUploading
                  ? "上传中…"
                  : videoMode === "byok"
                    ? "选择参考图"
                    : "上传参考图"}
              </Button>
            )}
            <input
              ref={fileRef}
              type="file"
              accept="image/*"
              className="hidden"
              onChange={(e) => void handlePickFile(e)}
            />
          </div>

          <div className="mb-3 flex flex-col gap-1.5">
            <Label className="text-xs">Prompt</Label>
            <Textarea
              value={prompt}
              onChange={(e) => setPrompt(e.target.value)}
              rows={4}
              placeholder="描述想要的视频画面、镜头运动、氛围…"
              className="text-sm"
            />
          </div>
        </div>

        <div className="shrink-0 border-t border-sidebar-border px-4 py-3">
          <Button
            onClick={() => void handleSubmit()}
            disabled={submitting || !prompt.trim() || cost == null}
            className="w-full"
          >
            {submitting ? (
              <>
                <Loader2 className="size-4 animate-spin" /> 提交中…
              </>
            ) : (
              <>
                <Clapperboard className="size-4" />
            {videoMode === "byok"
              ? "生成视频（本地 API）"
              : cost != null
                ? `生成视频（消耗 ${formatQuota(cost)} 元）`
                : "生成视频"}
              </>
            )}
          </Button>
        </div>
      </aside>

      <div className="flex min-w-0 flex-1 flex-col">
        <header className="flex items-center justify-between gap-2 border-b border-border bg-background/70 px-3 py-3 backdrop-blur md:px-6">
          <div className="flex min-w-0 items-center gap-2">
            <Button
              variant="ghost"
              size="icon"
              className="md:hidden"
              onClick={() => setMobileNavOpen(true)}
              aria-label="打开参数面板"
            >
              <Menu />
            </Button>
            <Clapperboard className="size-4 shrink-0 text-muted-foreground" />
            <h1 className="truncate text-base font-semibold">视频工作室</h1>
            <span className="hidden text-xs text-muted-foreground sm:inline">
              文生视频 · 图生视频
            </span>
          </div>
          <div className="flex shrink-0 items-center gap-1">
            <Button asChild variant="outline" size="sm">
              <Link to="/editor">
                <Scissors className="size-3.5" />
                <span className="hidden sm:inline">精细剪辑</span>
              </Link>
            </Button>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => void loadJobs(0, false)}
              title="刷新列表"
            >
              <RefreshCw className="size-3.5" />
              <span className="hidden sm:inline">刷新</span>
            </Button>
          </div>
        </header>

        <main className="nc-scroll flex min-h-0 flex-1 flex-col overflow-y-auto">
          {error && (
            <div className="mx-auto my-3 w-full max-w-3xl rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
              <button
                type="button"
                onClick={() => setError(null)}
                className="float-right"
                aria-label="关闭"
              >
                <X className="size-3.5" />
              </button>
              {error}
            </div>
          )}

          <div className="mx-auto flex w-full max-w-5xl flex-col gap-4 px-3 py-4 md:px-6 md:py-6">
            {jobs.length === 0 ? (
              <div className="flex h-80 flex-col items-center justify-center gap-2 text-muted-foreground">
                <Clapperboard className="size-8" />
                <p className="text-sm">在左侧填写参数后点「生成视频」</p>
              </div>
            ) : (
              <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
                {jobs.map((job) => (
                  <VideoJobCard
                    key={job.token}
                    job={job}
                    deleting={deletingToken === job.token}
                    regenerating={regeneratingToken === job.token}
                    regenerateDisabled={submitting || regeneratingToken !== null}
                    onRegenerate={() => void regenerate(job)}
                    onRemove={() => void removeJob(job)}
                  />
                ))}
              </div>
            )}

            {hasMore && (
              <div className="flex justify-center py-2">
                <Button
                  variant="outline"
                  size="sm"
                  disabled={loadingMore}
                  onClick={() => {
                    setLoadingMore(true)
                    void loadJobs(page + 1, true).finally(() => setLoadingMore(false))
                  }}
                >
                  {loadingMore ? (
                    <>
                      <Loader2 className="size-3.5 animate-spin" /> 加载中…
                    </>
                  ) : (
                    "加载更多"
                  )}
                </Button>
              </div>
            )}
          </div>
        </main>
      </div>
    </div>
  )
}

function VideoJobCard({
  job,
  deleting,
  regenerating,
  regenerateDisabled,
  onRegenerate,
  onRemove,
}: {
  job: VideoJob
  deleting: boolean
  regenerating: boolean
  regenerateDisabled: boolean
  onRegenerate: () => void
  onRemove: () => void
}) {
  const inFlight = job.status === "pending" || job.status === "running"

  if (job.status === "failed") {
    return (
      <div className="flex flex-col gap-2 rounded-lg border border-destructive/40 bg-destructive/5 p-3">
        <div className="flex items-center gap-2 text-sm font-medium text-destructive">
          <X className="size-4" /> 生成失败
        </div>
        <p className="line-clamp-3 whitespace-pre-wrap break-words text-xs text-foreground/80">
          {job.error || "未知错误"}
        </p>
        {job.refunded && (
          <p className="text-[11px] text-muted-foreground">
            已退还 {formatQuota(job.cost_quota)} 元
          </p>
        )}
        <div className="mt-1 flex flex-wrap gap-2">
          <Button
            variant="outline"
            size="sm"
            onClick={onRegenerate}
            disabled={regenerateDisabled}
          >
            <RefreshCw className={cn("size-3.5", regenerating && "animate-spin")} />
            {regenerating ? "重试中…" : "重新生成"}
          </Button>
          <Button variant="ghost" size="sm" onClick={onRemove} disabled={deleting}>
            <Trash2 className="size-3.5" /> 删除
          </Button>
        </div>
      </div>
    )
  }

  if (inFlight) {
    return (
      <div className="flex flex-col items-center justify-center gap-2 rounded-lg border border-border bg-card p-6 text-center">
        <Loader2 className="size-6 animate-spin text-primary" />
        <p className="text-sm font-medium">
          {statusLabel(job)} · 进度 {job.progress}%
        </p>
        <p className="line-clamp-2 text-xs text-muted-foreground">{job.prompt}</p>
        {job.error && (
          <p className="line-clamp-2 text-[11px] text-destructive">{job.error}</p>
        )}
        <p className="text-[11px] text-muted-foreground">
          {isLocalVideoJob(job)
            ? "本地服务会继续生成；保持此页面打开可自动获取结果"
            : "可离开页面，任务将在后台继续，稍后回来查看"}
        </p>
      </div>
    )
  }

  return (
    <div className="flex flex-col gap-2 rounded-lg border border-border bg-card p-3">
      {job.video_path && (
        <video
          controls
          preload="metadata"
          src={job.video_path}
          className="w-full rounded-lg"
        />
      )}
      <p className="line-clamp-2 text-xs text-foreground/90">{job.prompt}</p>
      <div className="flex flex-wrap gap-1.5 text-[11px] text-muted-foreground">
        <span className="rounded bg-muted px-1.5 py-0.5">{job.model}</span>
        <span className="rounded bg-muted px-1.5 py-0.5">{job.seconds} 秒</span>
        <span className="rounded bg-muted px-1.5 py-0.5">{job.size}</span>
        <span className="rounded bg-muted px-1.5 py-0.5">
          {isLocalVideoJob(job)
            ? "本地 API"
            : job.cost_quota > 0
              ? `消耗 ${formatQuota(job.cost_quota)} 元`
              : "云端任务"}
        </span>
      </div>
      <div className="mt-1 flex flex-wrap gap-2">
        {job.video_path && (
          <Button asChild variant="outline" size="sm">
            <a href={job.video_path} download>
              <Download className="size-3.5" /> 下载
            </a>
          </Button>
        )}
        <Button
          variant="outline"
          size="sm"
          onClick={onRegenerate}
          disabled={regenerateDisabled}
        >
          <RefreshCw className={cn("size-3.5", regenerating && "animate-spin")} />
          {regenerating ? "生成中…" : "重新生成"}
        </Button>
        <Button variant="ghost" size="sm" onClick={onRemove} disabled={deleting}>
          <Trash2 className="size-3.5" /> 删除
        </Button>
      </div>
    </div>
  )
}
