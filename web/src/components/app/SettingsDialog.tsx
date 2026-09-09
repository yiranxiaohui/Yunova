import { useEffect, useRef, useState } from "react"
import { Check, Cloud, Copy, Film, ImageIcon, KeyRound, MessageSquare, RefreshCw, Trash2 } from "lucide-react"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import {
  IMAGE_PROTOCOL_META,
  PROTOCOL_META,
  likelyWebSearchCapable,
  type ImageProtocol,
  type Protocol,
  type UpstreamMode,
  type UpstreamSettings,
} from "@/lib/settings"
import { listModels } from "@/lib/models"
import { listPlatformModels, type PlatformModel } from "@/lib/platform-models"
import { cn } from "@/lib/utils"
import { workerApi, type Worker } from "@/lib/worker"

type Props = {
  open: boolean
  initial: UpstreamSettings
  isAuthenticated: boolean
  onClose: () => void
  onLoginRequired: () => void
  onSave: (s: UpstreamSettings) => void
}

type Tab = "chat" | "image" | "video" | "worker"

const PROTOCOL_OPTIONS: Protocol[] = ["openai", "claude", "gemini"]
const IMAGE_PROTOCOL_OPTIONS: ImageProtocol[] = ["openai", "gemini"]

export function SettingsDialog({
  open,
  initial,
  isAuthenticated,
  onClose,
  onLoginRequired,
  onSave,
}: Props) {
  // --- Mode ---
  const [chatMode, setChatMode] = useState<UpstreamMode>(initial.chatMode)
  const [imageMode, setImageMode] = useState<UpstreamMode>(initial.imageMode)

  // --- Chat state ---
  const [protocol, setProtocol] = useState<Protocol>(initial.protocol)
  const [baseUrl, setBaseUrl] = useState(initial.baseUrl)
  const [apiKey, setApiKey] = useState(initial.apiKey)
  const [model, setModel] = useState(initial.model)
  const [useProxy, setUseProxy] = useState(initial.useProxy)
  const [webSearch, setWebSearch] = useState(initial.webSearch)

  // --- Image state ---
  const [imageProtocol, setImageProtocol] = useState<ImageProtocol>(initial.imageProtocol)
  const [imageBaseUrl, setImageBaseUrl] = useState(initial.imageBaseUrl)
  const [imageApiKey, setImageApiKey] = useState(initial.imageApiKey)
  const [imageModel, setImageModel] = useState(initial.imageModel)
  const [imageUseProxy, setImageUseProxy] = useState(initial.imageUseProxy)

  // --- Video state ---
  const [videoMode, setVideoMode] = useState<UpstreamMode>(initial.videoMode)
  const [videoBaseUrl, setVideoBaseUrl] = useState(initial.videoBaseUrl)
  const [videoApiKey, setVideoApiKey] = useState(initial.videoApiKey)
  const [videoModel, setVideoModel] = useState(initial.videoModel)

  const [cloudSync, setCloudSync] = useState(initial.cloudSync)
  const [tab, setTab] = useState<Tab>("chat")

  const [chatModels, setChatModels] = useState<string[]>([])
  const [chatLoading, setChatLoading] = useState(false)
  const [chatError, setChatError] = useState<string | null>(null)
  const [imageModels, setImageModels] = useState<string[]>([])
  const [imageLoading, setImageLoading] = useState(false)
  const [imageError, setImageError] = useState<string | null>(null)
  const chatAbortRef = useRef<AbortController | null>(null)
  const imageAbortRef = useRef<AbortController | null>(null)

  // Platform-mode model catalogues — loaded from /api/channels/models.
  const [platformChat, setPlatformChat] = useState<PlatformModel[] | null>(null)
  const [platformImage, setPlatformImage] = useState<PlatformModel[] | null>(null)
  const [platformChatError, setPlatformChatError] = useState<string | null>(null)
  const [platformImageError, setPlatformImageError] = useState<string | null>(null)

  useEffect(() => {
    if (open) {
      setChatMode(initial.chatMode)
      setImageMode(initial.imageMode)
      setProtocol(initial.protocol)
      setBaseUrl(initial.baseUrl)
      setApiKey(initial.apiKey)
      setModel(initial.model)
      setUseProxy(initial.useProxy)
      setWebSearch(initial.webSearch)
      setImageProtocol(initial.imageProtocol)
      setImageBaseUrl(initial.imageBaseUrl)
      setImageApiKey(initial.imageApiKey)
      setImageModel(initial.imageModel)
      setImageUseProxy(initial.imageUseProxy)
      setVideoMode(initial.videoMode)
      setVideoBaseUrl(initial.videoBaseUrl)
      setVideoApiKey(initial.videoApiKey)
      setVideoModel(initial.videoModel)
      setCloudSync(initial.cloudSync)
      setTab("chat")
      setChatModels([])
      setImageModels([])
      setChatError(null)
      setImageError(null)
      setPlatformChat(null)
      setPlatformImage(null)
      setPlatformChatError(null)
      setPlatformImageError(null)
    }
  }, [open, initial])

  // Fetch platform models when dialog opens and platform mode is active.
  useEffect(() => {
    if (!open || !isAuthenticated || chatMode !== "platform") return
    const ctrl = new AbortController()
    setPlatformChatError(null)
    listPlatformModels("chat", ctrl.signal)
      .then((list) => {
        setPlatformChat(list)
        // If current model not in catalogue, auto-pick first.
        if (list.length > 0 && !list.some((m) => m.model === model)) {
          setModel(list[0].model)
          setProtocol(list[0].protocol)
        }
      })
      .catch((e: unknown) => {
        if ((e as { name?: string }).name !== "AbortError") {
          setPlatformChatError(e instanceof Error ? e.message : String(e))
        }
      })
    return () => ctrl.abort()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, chatMode, isAuthenticated])

  useEffect(() => {
    if (!open || !isAuthenticated || imageMode !== "platform") return
    const ctrl = new AbortController()
    setPlatformImageError(null)
    listPlatformModels("image", ctrl.signal)
      .then((list) => {
        setPlatformImage(list)
        if (list.length > 0 && !list.some((m) => m.model === imageModel)) {
          setImageModel(list[0].model)
          if (list[0].protocol === "openai" || list[0].protocol === "gemini") {
            setImageProtocol(list[0].protocol)
          }
        }
      })
      .catch((e: unknown) => {
        if ((e as { name?: string }).name !== "AbortError") {
          setPlatformImageError(e instanceof Error ? e.message : String(e))
        }
      })
    return () => ctrl.abort()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, imageMode, isAuthenticated])

  useEffect(() => {
    setChatModels([])
    setChatError(null)
  }, [protocol, baseUrl])

  useEffect(() => {
    setImageModels([])
    setImageError(null)
  }, [imageBaseUrl, imageProtocol])


  const meta = PROTOCOL_META[protocol]
  const imgMeta = IMAGE_PROTOCOL_META[imageProtocol]
  const canLoadChat = Boolean(baseUrl.trim() && apiKey.trim())
  const canLoadImage = Boolean(imageBaseUrl.trim() && imageApiKey.trim())

  function pickProtocol(next: Protocol) {
    if (next === protocol) return
    const prev = PROTOCOL_META[protocol]
    setProtocol(next)
    const nextMeta = PROTOCOL_META[next]
    if (!baseUrl || baseUrl === prev.defaultBaseUrl) {
      setBaseUrl(nextMeta.defaultBaseUrl)
    }
    if (!model || model === prev.defaultModel) {
      setModel(nextMeta.defaultModel)
    }
  }

  function pickImageProtocol(next: ImageProtocol) {
    if (next === imageProtocol) return
    const prev = IMAGE_PROTOCOL_META[imageProtocol]
    setImageProtocol(next)
    const nextMeta = IMAGE_PROTOCOL_META[next]
    if (!imageBaseUrl || imageBaseUrl === prev.defaultBaseUrl) {
      setImageBaseUrl(nextMeta.defaultBaseUrl)
    }
    if (!imageModel || imageModel === prev.defaultModel) {
      setImageModel(nextMeta.defaultModel)
    }
  }

  async function loadChatModels() {
    if (!canLoadChat || chatLoading) return
    chatAbortRef.current?.abort()
    const ctrl = new AbortController()
    chatAbortRef.current = ctrl
    setChatLoading(true)
    setChatError(null)
    try {
      const list = await listModels({
        protocol,
        baseUrl: baseUrl.trim(),
        apiKey: apiKey.trim(),
        useProxy,
        signal: ctrl.signal,
      })
      setChatModels(list)
      if (list.length > 0 && !list.includes(model)) setModel(list[0])
    } catch (e) {
      if ((e as { name?: string }).name !== "AbortError") {
        setChatError(e instanceof Error ? e.message : String(e))
      }
    } finally {
      setChatLoading(false)
      chatAbortRef.current = null
    }
  }

  async function loadImageModels() {
    if (!canLoadImage || imageLoading) return
    imageAbortRef.current?.abort()
    const ctrl = new AbortController()
    imageAbortRef.current = ctrl
    setImageLoading(true)
    setImageError(null)
    try {
      const list = await listModels({
        protocol: imageProtocol,
        baseUrl: imageBaseUrl.trim(),
        apiKey: imageApiKey.trim(),
        useProxy: imageUseProxy,
        signal: ctrl.signal,
      })
      const filter =
        imageProtocol === "gemini"
          ? /imagen|image/i
          : /dall-?e|gpt-image|image|flux|sd-?\d/i
      const filtered = list.filter((m) => filter.test(m))
      const pool = filtered.length > 0 ? filtered : list
      setImageModels(pool)
      if (pool.length > 0 && !pool.includes(imageModel)) setImageModel(pool[0])
    } catch (e) {
      if ((e as { name?: string }).name !== "AbortError") {
        setImageError(e instanceof Error ? e.message : String(e))
      }
    } finally {
      setImageLoading(false)
      imageAbortRef.current = null
    }
  }

  function copyChatAsImage() {
    if (!baseUrl && !apiKey) return
    // Only makes sense to copy when the chat protocol is a superset of the
    // image protocol (OpenAI → OpenAI, Gemini → Gemini).
    if (protocol === "openai" && imageProtocol === "openai") {
      if (!imageBaseUrl) setImageBaseUrl(baseUrl)
      if (!imageApiKey) setImageApiKey(apiKey)
      if (!imageModel) setImageModel(IMAGE_PROTOCOL_META.openai.defaultModel)
    } else if (protocol === "gemini" && imageProtocol === "gemini") {
      if (!imageBaseUrl) setImageBaseUrl(baseUrl)
      if (!imageApiKey) setImageApiKey(apiKey)
      if (!imageModel) setImageModel(IMAGE_PROTOCOL_META.gemini.defaultModel)
    } else {
      // Different protocols: copy key only (most common: same vendor, diff URL).
      if (!imageApiKey) setImageApiKey(apiKey)
    }
  }

  return (
    <Dialog open={open} onOpenChange={(next) => !next && onClose()}>
      {/* block：面板内部靠 mt-* 排版，用默认的 grid+gap 会多出一层间距 */}
      <DialogContent
        className="nc-scroll block p-4 sm:max-w-md sm:p-6"
        style={{ maxHeight: "calc(100svh - 1rem)", overflow: "auto" }}
      >
        <DialogHeader>
          <DialogTitle>模型设置</DialogTitle>
          <DialogDescription className="mt-1">
            {cloudSync
              ? "☁️ 已开启云端同步：对话和图像配置会保存到服务器；视频自定义 API 始终只保存在当前浏览器。"
              : "仅保存在当前浏览器 (localStorage)，不会上传到服务器。"}
          </DialogDescription>
        </DialogHeader>

        <Tabs
          value={tab}
          onValueChange={(v) => setTab(v as Tab)}
          className="mt-4"
        >
          <TabsList>
            <TabsTrigger value="chat">
              <MessageSquare className="size-3.5" /> 对话
            </TabsTrigger>
            <TabsTrigger value="image">
              <ImageIcon className="size-3.5" /> 图像
            </TabsTrigger>
            <TabsTrigger value="video">
              <Film className="size-3.5" /> 视频
            </TabsTrigger>
            <TabsTrigger value="worker" disabled={!isAuthenticated}>
              工蜂
            </TabsTrigger>
          </TabsList>

          <TabsContent value="chat">
          <div className="mt-4 flex flex-col gap-3">
            <ModeToggle
              mode={chatMode}
              onChange={(next) => {
                if (next === "platform" && !isAuthenticated) {
                  onLoginRequired()
                  return
                }
                setChatMode(next)
              }}
              platformLabel={isAuthenticated ? "云端积分" : "云端积分（需登录）"}
              byokLabel="自带 API Key"
            />

            {chatMode === "platform" ? (
              <PlatformModelPicker
                label="对话模型"
                models={platformChat}
                error={platformChatError}
                value={model}
                onPick={(m) => {
                  setModel(m.model)
                  setProtocol(m.protocol)
                }}
              />
            ) : (
              <>
            <div className="flex flex-col gap-1.5">
              <Label>协议</Label>
              <div className="grid grid-cols-3 gap-2">
                {PROTOCOL_OPTIONS.map((p) => (
                  <button
                    key={p}
                    type="button"
                    onClick={() => pickProtocol(p)}
                    className={cn(
                      "rounded-md border px-3 py-2 text-sm transition-colors",
                      protocol === p
                        ? "border-primary bg-primary/10 text-foreground"
                        : "border-border bg-background hover:bg-accent"
                    )}
                  >
                    {PROTOCOL_META[p].label}
                  </button>
                ))}
              </div>
            </div>

            <div className="flex flex-col gap-1.5">
              <Label htmlFor="baseUrl">Base URL</Label>
              <Input
                id="baseUrl"
                placeholder={meta.defaultBaseUrl}
                value={baseUrl}
                onChange={(e) => setBaseUrl(e.target.value)}
              />
              <p className="text-xs text-muted-foreground">
                只填主机，路径自动补 <code>{meta.pathHint}</code>。
              </p>
            </div>

            <div className="flex flex-col gap-1.5">
              <Label htmlFor="apiKey">API Key</Label>
              <Input
                id="apiKey"
                type="text"
                autoComplete="off"
                spellCheck={false}
                placeholder={
                  protocol === "gemini"
                    ? "AIza…"
                    : "sk-…"
                }
                value={apiKey}
                onChange={(e) => setApiKey(e.target.value)}
              />
            </div>

            <ModelField
              label="对话模型"
              value={model}
              onChange={setModel}
              models={chatModels}
              loading={chatLoading}
              error={chatError}
              canLoad={canLoadChat}
              onLoad={loadChatModels}
              placeholder={meta.defaultModel}
              datalistId="chat-model-options"
            />

            <label className="flex items-start gap-2 text-sm">
              <input
                type="checkbox"
                className="mt-1"
                checked={useProxy}
                onChange={(e) => setUseProxy(e.target.checked)}
              />
              <span>
                走服务端转发（推荐：规避浏览器 CORS 限制；key 只在浏览器，仅随每次请求发送）
              </span>
            </label>

            <label className="flex items-start gap-2 text-sm">
              <input
                type="checkbox"
                className="mt-1"
                checked={webSearch}
                onChange={(e) => setWebSearch(e.target.checked)}
              />
              <span>
                启用网页搜索（调用各厂商原生工具：OpenAI{" "}
                <code>web_search</code>、Claude <code>web_search</code>、Gemini{" "}
                <code>google_search</code>；需模型支持）
              </span>
            </label>

            {webSearch && !likelyWebSearchCapable(protocol, model) && (
              <div className="rounded-md border border-amber-500/40 bg-amber-500/5 px-3 py-2 text-xs text-muted-foreground">
                ⚠️ 当前模型 <code>{model || "(未填写)"}</code>{" "}
                可能不支持网页搜索,请求可能返回 400。参考可用模型：
                {protocol === "openai" && (
                  <>
                    {" "}
                    <code>gpt-4o-search-preview</code>、
                    <code>gpt-4o-mini-search-preview</code>、<code>gpt-5</code>
                  </>
                )}
                {protocol === "claude" && (
                  <>
                    {" "}
                    Claude 3.5 / 3.7 / 4.x(如{" "}
                    <code>claude-sonnet-4-6</code>)
                  </>
                )}
                {protocol === "gemini" && (
                  <>
                    {" "}
                    Gemini 2.0+(如 <code>gemini-2.0-flash</code>)
                  </>
                )}
                。判断基于模型名启发式,不一定准确;OpenAI / Google 不在{" "}
                <code>/models</code> 接口里返回工具能力元数据。
              </div>
            )}
              </>
            )}
          </div>
          </TabsContent>

          <TabsContent value="image">
          <div className="mt-4 flex flex-col gap-3">
            <ModeToggle
              mode={imageMode}
              onChange={(next) => {
                if (next === "platform" && !isAuthenticated) {
                  onLoginRequired()
                  return
                }
                setImageMode(next)
              }}
              platformLabel={isAuthenticated ? "云端积分" : "云端积分（需登录）"}
              byokLabel="自带 API Key"
            />

            {imageMode === "platform" ? (
              <PlatformModelPicker
                label="图像模型"
                models={platformImage}
                error={platformImageError}
                value={imageModel}
                onPick={(m) => {
                  setImageModel(m.model)
                  if (m.protocol === "openai" || m.protocol === "gemini") {
                    setImageProtocol(m.protocol)
                  }
                }}
              />
            ) : (
              <>
            <div className="rounded-md border border-border bg-muted/30 p-3 text-xs text-muted-foreground">
              图像生成支持 OpenAI（<code>/v1/images/generations</code>）和 Google Imagen（<code>:predict</code>）两种协议。可以独立配置（例如聊天走 Claude、生图走 Imagen）。留空则禁用图像模式。
            </div>

            <div className="flex flex-col gap-1.5">
              <Label>协议</Label>
              <div className="grid grid-cols-2 gap-2">
                {IMAGE_PROTOCOL_OPTIONS.map((p) => (
                  <button
                    key={p}
                    type="button"
                    onClick={() => pickImageProtocol(p)}
                    className={cn(
                      "rounded-md border px-3 py-2 text-sm transition-colors",
                      imageProtocol === p
                        ? "border-primary bg-primary/10 text-foreground"
                        : "border-border bg-background hover:bg-accent"
                    )}
                  >
                    {IMAGE_PROTOCOL_META[p].label}
                  </button>
                ))}
              </div>
            </div>

            <div className="flex flex-col gap-1.5">
              <div className="flex items-center justify-between">
                <Label htmlFor="imageBaseUrl">图像 Base URL</Label>
                {(baseUrl || apiKey) && (
                  <button
                    type="button"
                    onClick={copyChatAsImage}
                    className="text-xs text-muted-foreground underline-offset-2 hover:text-foreground hover:underline"
                  >
                    复制对话配置
                  </button>
                )}
              </div>
              <Input
                id="imageBaseUrl"
                placeholder={imgMeta.defaultBaseUrl}
                value={imageBaseUrl}
                onChange={(e) => setImageBaseUrl(e.target.value)}
              />
              <p className="text-xs text-muted-foreground">
                路径自动补 <code>{imgMeta.pathHint}</code>。
              </p>
            </div>

            <div className="flex flex-col gap-1.5">
              <Label htmlFor="imageApiKey">图像 API Key</Label>
              <Input
                id="imageApiKey"
                type="text"
                autoComplete="off"
                spellCheck={false}
                placeholder={
                  imageProtocol === "gemini"
                    ? "AIza…"
                    : "sk-…"
                }
                value={imageApiKey}
                onChange={(e) => setImageApiKey(e.target.value)}
              />
            </div>

            <ModelField
              label="图像模型"
              value={imageModel}
              onChange={setImageModel}
              models={imageModels}
              loading={imageLoading}
              error={imageError}
              canLoad={canLoadImage}
              onLoad={loadImageModels}
              placeholder={imgMeta.defaultModel}
              datalistId="image-model-options"
            />

            <label className="flex items-start gap-2 text-sm">
              <input
                type="checkbox"
                className="mt-1"
                checked={imageUseProxy}
                onChange={(e) => setImageUseProxy(e.target.checked)}
              />
              <span>走服务端转发（同上）</span>
            </label>
              </>
            )}
          </div>
          </TabsContent>

          <TabsContent value="video">
            <div className="mt-4 flex flex-col gap-3">
              <ModeToggle
                mode={videoMode}
                onChange={(next) => {
                  if (next === "platform" && !isAuthenticated) {
                    onLoginRequired()
                    return
                  }
                  setVideoMode(next)
                }}
                platformLabel={isAuthenticated ? "云端积分" : "云端积分（需登录）"}
                byokLabel="本地 API"
              />

              {videoMode === "platform" ? (
                <div className="rounded-md border border-border bg-muted/30 p-3 text-xs text-muted-foreground">
                  视频页会显示管理员配置的模型、时长和分辨率。切换到“本地 API”后，浏览器会直接访问自己的 OpenAI 兼容视频服务。
                </div>
              ) : (
                <>
                  <div className="flex flex-col gap-1.5">
                    <Label htmlFor="videoBaseUrl">视频 Base URL</Label>
                    <Input
                      id="videoBaseUrl"
                      placeholder="http://127.0.0.1:8000"
                      value={videoBaseUrl}
                      onChange={(e) => setVideoBaseUrl(e.target.value)}
                    />
                    <p className="text-xs text-muted-foreground">
                      浏览器会直接请求 <code>/v1/videos</code> 和 <code>/v1/models</code>；本地服务需允许当前站点跨域访问。
                    </p>
                  </div>

                  <div className="flex flex-col gap-1.5">
                    <Label htmlFor="videoApiKey">视频 API Key</Label>
                    <Input
                      id="videoApiKey"
                      type="text"
                      autoComplete="off"
                      spellCheck={false}
                      placeholder="没有 Key 的本地服务可留空"
                      value={videoApiKey}
                      onChange={(e) => setVideoApiKey(e.target.value)}
                    />
                  </div>

                  <div className="flex flex-col gap-1.5">
                    <Label htmlFor="videoModel">视频模型（可选）</Label>
                    <Input
                      id="videoModel"
                      placeholder="例如 wan2.1 或自建模型 ID"
                      value={videoModel}
                      onChange={(e) => setVideoModel(e.target.value)}
                    />
                    <p className="text-xs text-muted-foreground">
                      视频页也可以刷新模型列表；填写后会优先使用这里的模型。
                    </p>
                  </div>
                </>
              )}
            </div>
          </TabsContent>

          <TabsContent value="worker">
            <div className="mt-4">
              <WorkerSettings />
            </div>
          </TabsContent>
        </Tabs>

        <label className="mt-4 flex items-start gap-2 text-sm">
          <input
            type="checkbox"
            className="mt-1"
            checked={cloudSync}
            disabled={!isAuthenticated}
            onChange={(e) => setCloudSync(e.target.checked)}
          />
          <span>
            {isAuthenticated
              ? "云端同步对话/图像 API Key、Base URL 和模型（视频自定义 API 始终只保存在当前浏览器）"
              : "登录后可开启模型设置云端同步"}
          </span>
        </label>

        <DialogFooter className="mt-6 flex-row justify-end">
          <Button variant="ghost" onClick={onClose}>
            取消
          </Button>
          <Button
            onClick={() =>
              onSave({
                chatMode,
                imageMode,
                protocol,
                baseUrl: baseUrl.trim(),
                apiKey: apiKey.trim(),
                model: model.trim(),
                useProxy,
                webSearch,
                imageProtocol,
                imageBaseUrl: imageBaseUrl.trim(),
                imageApiKey: imageApiKey.trim(),
                imageModel: imageModel.trim(),
                imageUseProxy,
                videoMode,
                videoBaseUrl: videoBaseUrl.trim(),
                videoApiKey: videoApiKey.trim(),
                videoModel: videoModel.trim(),
                cloudSync: isAuthenticated && cloudSync,
              })
            }
          >
            保存
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function WorkerSettings() {
  const [workers, setWorkers] = useState<Worker[]>([])
  const [token, setToken] = useState<string | null>(null)
  const [pairing, setPairing] = useState(false)
  const [copied, setCopied] = useState(false)
  const refresh = () => workerApi.list().then(setWorkers).catch(() => {})
  useEffect(() => {
    refresh()
    const t = setInterval(refresh, 5000)
    return () => clearInterval(t)
  }, [])
  const deployCmd = (tok: string) =>
    `YUNOVA_WORKER_URL=wss://你的域名/api/worker/connect YUNOVA_WORKER_TOKEN=${tok} ./yunova-worker`
  async function pair() {
    if (pairing) return
    setPairing(true)
    try {
      const r = await workerApi.pair()
      setToken(r.token)
      setCopied(false)
      refresh()
    } finally {
      setPairing(false)
    }
  }
  async function copyDeploy() {
    if (!token) return
    try {
      await navigator.clipboard.writeText(deployCmd(token))
      setCopied(true)
      setTimeout(() => setCopied(false), 2000)
    } catch {
      /* 忽略 */
    }
  }
  async function removeWorker(id: number) {
    await workerApi.remove(id).catch(() => {})
    refresh()
  }
  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center gap-2">
        <Button size="sm" onClick={pair} disabled={pairing}>
          {pairing ? "生成中…" : "生成配对码"}
        </Button>
        <span className="text-xs text-muted-foreground">部署到你的服务器，连回本站受你操控</span>
      </div>
      {token && (
        <div className="space-y-2 rounded-md border bg-muted/40 p-3 text-sm">
          <div className="font-medium">配对码（仅显示一次，请妥善保存）</div>
          <code className="block break-all rounded bg-background p-2 font-mono text-xs">{token}</code>
          <div className="text-xs text-muted-foreground">部署命令：</div>
          <div className="flex items-start gap-2">
            <code className="block flex-1 break-all rounded bg-background p-2 font-mono text-xs">
              {deployCmd(token)}
            </code>
            <Button variant="outline" size="icon" className="shrink-0" onClick={copyDeploy} title="复制部署命令">
              {copied ? <Check className="size-4 text-green-500" /> : <Copy className="size-4" />}
            </Button>
          </div>
        </div>
      )}
      <ul className="space-y-1">
        {workers.map((w) => (
          <li key={w.id} className="flex items-center gap-2 rounded-md border px-3 py-2 text-sm">
            <span
              className={cn(
                "size-2.5 shrink-0 rounded-full",
                w.online ? "bg-green-500" : "bg-muted-foreground/50"
              )}
              aria-hidden
            />
            <span className="flex-1 truncate">{w.name}</span>
            <span className="shrink-0 text-xs text-muted-foreground">{w.online ? "在线" : "离线"}</span>
            <Button variant="ghost" size="icon" className="shrink-0" onClick={() => removeWorker(w.id)} title="删除工蜂">
              <Trash2 className="size-4" />
            </Button>
          </li>
        ))}
        {workers.length === 0 && (
          <li className="rounded-md border border-dashed px-3 py-4 text-center text-sm text-muted-foreground">
            还没有工蜂，点上面「生成配对码」并部署。
          </li>
        )}
      </ul>
    </div>
  )
}

function ModelField({
  label,
  value,
  onChange,
  models,
  loading,
  error,
  canLoad,
  onLoad,
  placeholder,
  datalistId,
}: {
  label: string
  value: string
  onChange: (v: string) => void
  models: string[]
  loading: boolean
  error: string | null
  canLoad: boolean
  onLoad: () => void
  placeholder: string
  datalistId: string
}) {
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-center justify-between">
        <Label htmlFor={datalistId + "-input"}>{label}</Label>
        <Button
          type="button"
          size="sm"
          variant="outline"
          onClick={onLoad}
          disabled={!canLoad || loading}
        >
          <RefreshCw className={loading ? "animate-spin" : ""} />
          {loading ? "加载中…" : models.length > 0 ? "刷新" : "加载列表"}
        </Button>
      </div>
      <Input
        id={datalistId + "-input"}
        list={datalistId}
        placeholder={placeholder}
        value={value}
        onChange={(e) => onChange(e.target.value)}
      />
      {models.length > 0 && (
        <>
          <datalist id={datalistId}>
            {models.map((m) => (
              <option key={m} value={m} />
            ))}
          </datalist>
          <div className="max-h-24 overflow-y-auto rounded-md border border-border bg-muted/30 p-1.5">
            <div className="flex flex-wrap gap-1">
              {models.map((m) => (
                <button
                  key={m}
                  type="button"
                  onClick={() => onChange(m)}
                  className={cn(
                    "rounded px-2 py-0.5 text-xs transition-colors",
                    m === value
                      ? "bg-primary text-primary-foreground"
                      : "bg-background hover:bg-accent"
                  )}
                >
                  {m}
                </button>
              ))}
            </div>
          </div>
        </>
      )}
      {error && <p className="text-xs text-destructive">加载失败：{error}</p>}
    </div>
  )
}

function ModeToggle({
  mode,
  onChange,
  platformLabel,
  byokLabel,
}: {
  mode: UpstreamMode
  onChange: (m: UpstreamMode) => void
  platformLabel: string
  byokLabel: string
}) {
  return (
    <div>
      <div className="grid grid-cols-2 gap-2">
        <button
          type="button"
          onClick={() => onChange("platform")}
          className={cn(
            "flex items-center justify-center gap-1.5 rounded-md border px-3 py-2 text-sm transition-colors",
            mode === "platform"
              ? "border-primary bg-primary/10 text-foreground"
              : "border-border bg-background hover:bg-accent"
          )}
        >
          <Cloud className="size-3.5" /> {platformLabel}
        </button>
        <button
          type="button"
          onClick={() => onChange("byok")}
          className={cn(
            "flex items-center justify-center gap-1.5 rounded-md border px-3 py-2 text-sm transition-colors",
            mode === "byok"
              ? "border-primary bg-primary/10 text-foreground"
              : "border-border bg-background hover:bg-accent"
          )}
        >
          <KeyRound className="size-3.5" /> {byokLabel}
        </button>
      </div>
      <p className="mt-1.5 text-xs text-muted-foreground">
        {mode === "platform"
          ? "使用管理员配置的上游，按模型扣除账户积分。无需填写 Base URL / API Key。"
          : "使用你自己的上游服务（OpenAI / Anthropic / Gemini 或自建中转），不消耗积分。"}
      </p>
    </div>
  )
}

function PlatformModelPicker({
  label,
  models,
  error,
  value,
  onPick,
}: {
  label: string
  models: PlatformModel[] | null
  error: string | null
  value: string
  onPick: (m: PlatformModel) => void
}) {
  if (error) {
    return (
      <div className="rounded-md border border-destructive/40 bg-destructive/5 p-3 text-xs text-destructive">
        加载平台模型失败：{error}
      </div>
    )
  }
  if (!models) {
    return (
      <div className="rounded-md border border-border bg-muted/30 p-3 text-xs text-muted-foreground">
        加载平台模型中…
      </div>
    )
  }
  if (models.length === 0) {
    return (
      <div className="rounded-md border border-amber-500/40 bg-amber-500/5 p-3 text-xs text-muted-foreground">
        管理员尚未配置可用的平台模型。请联系管理员或切换到「自带 API Key」模式。
      </div>
    )
  }
  return (
    <div className="flex flex-col gap-1.5">
      <Label>{label}</Label>
      <div className="flex flex-col gap-1">
        {models.map((m) => (
          <button
            key={m.model}
            type="button"
            onClick={() => onPick(m)}
            className={cn(
              "flex items-center justify-between rounded-md border px-3 py-2 text-left text-sm transition-colors",
              m.model === value
                ? "border-primary bg-primary/10"
                : "border-border bg-background hover:bg-accent"
            )}
          >
            <div className="flex flex-col">
              <span className="font-medium">{m.display_name || m.model}</span>
              {m.display_name && (
                <span className="text-xs text-muted-foreground">{m.model}</span>
              )}
            </div>
            <span className="text-xs text-muted-foreground">
              {m.cost_credits} 积分/次
            </span>
          </button>
        ))}
      </div>
    </div>
  )
}
