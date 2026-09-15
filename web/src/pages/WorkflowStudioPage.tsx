import { useEffect, useMemo, useRef, useState } from "react"
import { Link } from "react-router-dom"
import {
  ArrowLeft,
  Boxes,
  Check,
  ChevronDown,
  ChevronUp,
  Clapperboard,
  Crop,
  GitMerge,
  ImageIcon,
  Link2,
  Loader2,
  Menu,
  MousePointer2,
  Play,
  Plus,
  RotateCcw,
  Save,
  Square,
  Terminal,
  Trash2,
  X,
} from "lucide-react"
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
import { listPlatformModels, type PlatformModel } from "@/lib/platform-models"
import {
  computeVideoCost,
  listVideoModels,
  type VideoModel,
} from "@/lib/video-gen"
import {
  workflowApi,
  type Workflow,
  type WorkflowEdge,
  type WorkflowGraph,
  type WorkflowNode,
  type WorkflowNodeData,
  type WorkflowNodeRun,
  type WorkflowNodeType,
  type WorkflowRun,
  type WorkflowRunLog,
} from "@/lib/workflows"
import { cn } from "@/lib/utils"

const NODE_WIDTH = 304
const PORT_Y = 51
const CANVAS_WIDTH = 2200
const CANVAS_HEIGHT = 1500
const DEFAULT_EDGE_PRIORITY = 100
const MAX_EDGE_PRIORITY = 9999

const NODE_META: Record<
  WorkflowNodeType,
  { label: string; hint: string; icon: typeof ImageIcon; color: string }
> = {
  image_generation: {
    label: "图片生成",
    hint: "文生图 / 接图后编辑",
    icon: ImageIcon,
    color: "text-violet-600 dark:text-violet-300",
  },
  video_generation: {
    label: "视频生成",
    hint: "文生视频 / 图生视频",
    icon: Clapperboard,
    color: "text-sky-600 dark:text-sky-300",
  },
  video_trim: {
    label: "视频裁剪",
    hint: "按秒截取片段",
    icon: Crop,
    color: "text-amber-600 dark:text-amber-300",
  },
  video_merge: {
    label: "视频合并",
    hint: "按连线顺序拼接",
    icon: GitMerge,
    color: "text-emerald-600 dark:text-emerald-300",
  },
}

function makeId(prefix: string): string {
  const nonce =
    typeof crypto.randomUUID === "function"
      ? crypto.randomUUID().replaceAll("-", "")
      : `${Date.now()}${Math.random().toString(16).slice(2)}`
  return `${prefix}_${nonce}`
}

function defaultVideoData(models: VideoModel[]): WorkflowNodeData {
  const model = models[0]
  return {
    model: model?.model ?? "",
    prompt: "",
    seconds: model?.allowed_seconds[0] ?? 5,
    size: model?.size_rules[0]?.size ?? "1280x720",
  }
}

function defaultNodeData(
  type: WorkflowNodeType,
  imageModels: PlatformModel[],
  videoModels: VideoModel[]
): WorkflowNodeData {
  switch (type) {
    case "image_generation":
      return {
        model: imageModels[0]?.model ?? "",
        prompt: "",
        size: "1024x1024",
        quality: "auto",
      }
    case "video_generation":
      return defaultVideoData(videoModels)
    case "video_trim":
      return { start: 0, end: 5 }
    case "video_merge":
      return { width: 1280, height: 720 }
  }
}

function outputKind(type: WorkflowNodeType): "image" | "video" {
  return type === "image_generation" ? "image" : "video"
}

type MergeInput = {
  edge: WorkflowEdge
  source: WorkflowNode
  connectionIndex: number
}

function mergeInputsFor(graph: WorkflowGraph, targetId: string): MergeInput[] {
  return graph.edges
    .map((edge, connectionIndex) => ({
      edge,
      connectionIndex,
      source: graph.nodes.find((node) => node.id === edge.source),
    }))
    .filter(
      (input): input is MergeInput =>
        input.edge.target === targetId && input.source !== undefined
    )
    .sort(
      (left, right) =>
        (left.edge.priority ?? DEFAULT_EDGE_PRIORITY) -
          (right.edge.priority ?? DEFAULT_EDGE_PRIORITY) ||
        left.connectionIndex - right.connectionIndex
    )
}

function nextMergeInputPriority(edges: WorkflowEdge[], targetId: string): number {
  const priorities = edges
    .filter((edge) => edge.target === targetId)
    .map((edge) => edge.priority ?? DEFAULT_EDGE_PRIORITY)
  if (priorities.length === 0) return DEFAULT_EDGE_PRIORITY
  return Math.min(MAX_EDGE_PRIORITY, Math.max(...priorities) + 100)
}

function canConnect(
  source: WorkflowNode,
  target: WorkflowNode,
  edges: WorkflowEdge[]
): string | null {
  if (source.id === target.id) return "节点不能连接到自身"
  const kind = outputKind(source.type)
  const incoming = edges.filter((edge) => edge.target === target.id)
  if (edges.some((edge) => edge.source === source.id && edge.target === target.id)) {
    return "这两个节点已经连接"
  }
  const pending = [target.id]
  const visited = new Set<string>()
  while (pending.length > 0) {
    const nodeId = pending.pop()!
    if (nodeId === source.id) return "这条连线会形成循环"
    if (visited.has(nodeId)) continue
    visited.add(nodeId)
    for (const edge of edges) {
      if (edge.source === nodeId) pending.push(edge.target)
    }
  }
  if (target.type === "image_generation" && kind !== "image") {
    return "图片生成节点只能接收图片"
  }
  if (target.type === "video_generation") {
    if (kind !== "image") return "视频生成节点只能接收图片作为参考图"
    if (incoming.length > 0) return "一个视频生成节点最多接收一张图片"
  }
  if (target.type === "video_trim") {
    if (kind !== "video") return "视频裁剪节点只能接收视频"
    if (incoming.length > 0) return "一个裁剪节点只能接收一个视频"
  }
  if (target.type === "video_merge" && kind !== "video") {
    return "视频合并节点只能接收视频"
  }
  if (target.type === "video_merge" && incoming.length >= 12) {
    return "单个合并节点最多接收 12 个视频"
  }
  return null
}

function starterGraph(
  imageModels: PlatformModel[],
  videoModels: VideoModel[]
): WorkflowGraph {
  const imageId = makeId("image")
  const videoAId = makeId("video")
  const videoBId = makeId("video")
  const mergeId = makeId("merge")
  const imageData = defaultNodeData("image_generation", imageModels, videoModels)
  const videoData = defaultNodeData("video_generation", imageModels, videoModels)
  return {
    version: 1,
    nodes: [
      {
        id: imageId,
        type: "image_generation",
        x: 80,
        y: 180,
        data: { ...imageData, prompt: "一座雨夜中的未来城市，电影感灯光" },
      },
      {
        id: videoAId,
        type: "video_generation",
        x: 500,
        y: 50,
        data: { ...videoData, prompt: "镜头缓慢向前推进，霓虹倒影流动" },
      },
      {
        id: videoBId,
        type: "video_generation",
        x: 500,
        y: 430,
        data: { ...videoData, prompt: "镜头向上摇摄，飞行器从画面掠过" },
      },
      {
        id: mergeId,
        type: "video_merge",
        x: 930,
        y: 230,
        data: { width: 1280, height: 720 },
      },
    ],
    edges: [
      { id: makeId("edge"), source: imageId, target: videoAId },
      { id: makeId("edge"), source: imageId, target: videoBId },
      { id: makeId("edge"), source: videoAId, target: mergeId, priority: 100 },
      { id: makeId("edge"), source: videoBId, target: mergeId, priority: 200 },
    ],
  }
}

function statusText(status?: WorkflowNodeRun["status"]): string {
  switch (status) {
    case "waiting":
      return "等待上游"
    case "starting":
      return "正在创建"
    case "running":
      return "执行中"
    case "completed":
      return "已完成"
    case "failed":
      return "失败"
    case "blocked":
      return "已阻断"
    case "cancelled":
      return "已停止"
    default:
      return "未运行"
  }
}

function runStatusText(status: WorkflowRun["status"]): string {
  return {
    running: "运行中",
    completed: "已完成",
    failed: "部分失败",
    cancelled: "已停止",
  }[status]
}

function formatLogTime(value: string): string {
  const normalized = /^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}(?:\.\d+)?$/.test(value)
    ? `${value.replace(" ", "T")}Z`
    : value
  const date = new Date(normalized)
  if (Number.isNaN(date.getTime())) return value
  return new Intl.DateTimeFormat(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  }).format(date)
}

function logLevelClass(level: WorkflowRunLog["level"]): string {
  switch (level) {
    case "success":
      return "text-emerald-600 dark:text-emerald-400"
    case "warning":
      return "text-amber-600 dark:text-amber-400"
    case "error":
      return "text-destructive"
    default:
      return "text-foreground"
  }
}

export default function WorkflowStudioPage() {
  const { confirm } = useConfirm()
  const [workflows, setWorkflows] = useState<Workflow[]>([])
  const [runs, setRuns] = useState<WorkflowRun[]>([])
  const [workflowId, setWorkflowId] = useState<number | null>(null)
  const [name, setName] = useState("未命名流水线")
  const [graph, setGraph] = useState<WorkflowGraph>({ version: 1, nodes: [], edges: [] })
  const [currentRun, setCurrentRun] = useState<WorkflowRun | null>(null)
  const [imageModels, setImageModels] = useState<PlatformModel[]>([])
  const [videoModels, setVideoModels] = useState<VideoModel[]>([])
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [starting, setStarting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [connectingFrom, setConnectingFrom] = useState<string | null>(null)
  const [mobilePanel, setMobilePanel] = useState(false)
  const canvasViewportRef = useRef<HTMLElement>(null)
  const [logsOpen, setLogsOpen] = useState(true)
  const logScrollRef = useRef<HTMLDivElement>(null)
  const [drag, setDrag] = useState<{
    nodeId: string
    clientX: number
    clientY: number
    originX: number
    originY: number
  } | null>(null)
  const [pan, setPan] = useState<{
    pointerId: number
    clientX: number
    clientY: number
    scrollLeft: number
    scrollTop: number
  } | null>(null)

  const runNodes = useMemo(
    () => new Map(currentRun?.nodes.map((node) => [node.node_id, node]) ?? []),
    [currentRun]
  )
  const nodeNames = useMemo(
    () =>
      new Map(
        graph.nodes.map((node) => [node.id, NODE_META[node.type].label])
      ),
    [graph.nodes]
  )
  const currentRunToken = currentRun?.token
  const currentRunStatus = currentRun?.status
  const estimatedQuota = useMemo(
    () =>
      graph.nodes.reduce((total, node) => {
        if (node.type === "image_generation") {
          return (
            total +
            (imageModels.find((model) => model.model === node.data.model)
              ?.per_call_quota ?? 0)
          )
        }
        if (node.type === "video_generation") {
          const model = videoModels.find(
            (item) => item.model === node.data.model
          )
          if (!model) return total
          return (
            total +
            (computeVideoCost(
              model,
              Number(node.data.seconds),
              String(node.data.size)
            ) ?? 0)
          )
        }
        return total
      }, 0),
    [graph.nodes, imageModels, videoModels]
  )

  async function reloadLibrary() {
    const [saved, recent] = await Promise.all([
      workflowApi.list(),
      workflowApi.listRuns(),
    ])
    setWorkflows(saved)
    setRuns(recent)
  }

  useEffect(() => {
    void (async () => {
      try {
        const [images, videos, saved, recent] = await Promise.all([
          listPlatformModels("image"),
          listVideoModels(),
          workflowApi.list(),
          workflowApi.listRuns(),
        ])
        setImageModels(images)
        setVideoModels(videos)
        setWorkflows(saved)
        setRuns(recent)
        if (saved[0]) {
          setWorkflowId(saved[0].id)
          setName(saved[0].name)
          setGraph(saved[0].graph)
        }
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
      } finally {
        setLoading(false)
      }
    })()
  }, [])

  useEffect(() => {
    if (!currentRunToken || currentRunStatus !== "running") return
    const timer = window.setInterval(() => {
      void workflowApi
        .run(currentRunToken)
        .then((run) => {
          setCurrentRun(run)
          if (run.status !== "running") void reloadLibrary()
        })
        .catch((e) => setError(e instanceof Error ? e.message : String(e)))
    }, 3000)
    return () => window.clearInterval(timer)
    // Poll by token; status changes terminate the interval on the next render.
  }, [currentRunToken, currentRunStatus])

  useEffect(() => {
    if (!logsOpen || !logScrollRef.current) return
    logScrollRef.current.scrollTop = logScrollRef.current.scrollHeight
  }, [currentRun?.logs.length, logsOpen])

  useEffect(() => {
    if (!drag) return
    function move(event: PointerEvent) {
      const nextX = Math.max(20, Math.min(CANVAS_WIDTH - NODE_WIDTH - 20, drag!.originX + event.clientX - drag!.clientX))
      const nextY = Math.max(20, Math.min(CANVAS_HEIGHT - 240, drag!.originY + event.clientY - drag!.clientY))
      setGraph((current) => ({
        ...current,
        nodes: current.nodes.map((node) =>
          node.id === drag!.nodeId ? { ...node, x: nextX, y: nextY } : node
        ),
      }))
    }
    function up() {
      setDrag(null)
    }
    window.addEventListener("pointermove", move)
    window.addEventListener("pointerup", up, { once: true })
    return () => {
      window.removeEventListener("pointermove", move)
      window.removeEventListener("pointerup", up)
    }
  }, [drag])

  useEffect(() => {
    if (!pan) return
    function move(event: PointerEvent) {
      if (event.pointerId !== pan!.pointerId) return
      const viewport = canvasViewportRef.current
      if (!viewport) return
      viewport.scrollLeft = pan!.scrollLeft - (event.clientX - pan!.clientX)
      viewport.scrollTop = pan!.scrollTop - (event.clientY - pan!.clientY)
    }
    function stop(event: PointerEvent) {
      if (event.pointerId === pan!.pointerId) setPan(null)
    }
    window.addEventListener("pointermove", move)
    window.addEventListener("pointerup", stop)
    window.addEventListener("pointercancel", stop)
    return () => {
      window.removeEventListener("pointermove", move)
      window.removeEventListener("pointerup", stop)
      window.removeEventListener("pointercancel", stop)
    }
  }, [pan])

  function createBlank() {
    setWorkflowId(null)
    setName("未命名流水线")
    setGraph({ version: 1, nodes: [], edges: [] })
    setCurrentRun(null)
    setConnectingFrom(null)
    setError(null)
  }

  function loadWorkflow(workflow: Workflow) {
    setWorkflowId(workflow.id)
    setName(workflow.name)
    setGraph(workflow.graph)
    setCurrentRun(null)
    setConnectingFrom(null)
    setMobilePanel(false)
  }

  function addNode(type: WorkflowNodeType) {
    const index = graph.nodes.length
    const node: WorkflowNode = {
      id: makeId(type.split("_")[0]!),
      type,
      x: 80 + (index % 4) * 360,
      y: 80 + Math.floor(index / 4) * 390,
      data: defaultNodeData(type, imageModels, videoModels),
    }
    setGraph((current) => ({ ...current, nodes: [...current.nodes, node] }))
    setMobilePanel(false)
  }

  function removeNode(id: string) {
    setGraph((current) => ({
      ...current,
      nodes: current.nodes.filter((node) => node.id !== id),
      edges: current.edges.filter((edge) => edge.source !== id && edge.target !== id),
    }))
    if (connectingFrom === id) setConnectingFrom(null)
  }

  function updateNodeData(id: string, patch: WorkflowNodeData) {
    setGraph((current) => ({
      ...current,
      nodes: current.nodes.map((node) =>
        node.id === id ? { ...node, data: { ...node.data, ...patch } } : node
      ),
    }))
  }

  function updateEdgePriority(id: string, priority: number) {
    if (!Number.isFinite(priority)) return
    const nextPriority = Math.max(
      0,
      Math.min(MAX_EDGE_PRIORITY, Math.trunc(priority))
    )
    setGraph((current) => ({
      ...current,
      edges: current.edges.map((edge) =>
        edge.id === id ? { ...edge, priority: nextPriority } : edge
      ),
    }))
  }

  function connectTo(targetId: string) {
    if (!connectingFrom) return
    const source = graph.nodes.find((node) => node.id === connectingFrom)
    const target = graph.nodes.find((node) => node.id === targetId)
    if (!source || !target) return
    const invalid = canConnect(source, target, graph.edges)
    if (invalid) {
      setError(invalid)
      return
    }
    setGraph((current) => ({
      ...current,
      edges: [
        ...current.edges,
        {
          id: makeId("edge"),
          source: source.id,
          target: target.id,
          ...(target.type === "video_merge"
            ? { priority: nextMergeInputPriority(current.edges, target.id) }
            : {}),
        },
      ],
    }))
    setConnectingFrom(null)
    setError(null)
  }

  async function save(): Promise<number | null> {
    const cleanName = name.trim()
    if (!cleanName) {
      setError("请填写流水线名称")
      return null
    }
    if (graph.nodes.length === 0) {
      setError("请先添加至少一个节点")
      return null
    }
    setSaving(true)
    setError(null)
    try {
      const saved = await workflowApi.save(workflowId, cleanName, graph)
      setWorkflowId(saved.id)
      await reloadLibrary()
      return saved.id
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
      return null
    } finally {
      setSaving(false)
    }
  }

  async function startRun() {
    if (starting) return
    setStarting(true)
    setError(null)
    try {
      const id = await save()
      if (id == null) return
      const { token } = await workflowApi.start(id)
      const run = await workflowApi.run(token)
      setCurrentRun(run)
      setLogsOpen(true)
      setRuns((current) => [run, ...current.filter((item) => item.token !== token)])
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setStarting(false)
    }
  }

  async function removeCurrentWorkflow() {
    if (workflowId == null) return
    const ok = await confirm({
      title: "删除这条流水线？",
      description: "历史运行记录和已生成媒体会保留。",
      confirmText: "删除",
      destructive: true,
    })
    if (!ok) return
    try {
      await workflowApi.remove(workflowId)
      createBlank()
      await reloadLibrary()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }

  async function cancelCurrentRun() {
    if (!currentRun || currentRun.status !== "running") return
    try {
      await workflowApi.cancel(currentRun.token)
      setCurrentRun(await workflowApi.run(currentRun.token))
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }

  async function retryNode(nodeId: string) {
    if (!currentRun) return
    try {
      await workflowApi.retryNode(currentRun.token, nodeId)
      setCurrentRun(await workflowApi.run(currentRun.token))
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }

  function openRun(run: WorkflowRun) {
    setCurrentRun(run)
    setGraph(run.graph)
    setName(run.name)
    setWorkflowId(run.workflow_id)
    setMobilePanel(false)
    setLogsOpen(true)
    void workflowApi
      .run(run.token)
      .then((loaded) => {
        setCurrentRun((current) =>
          current?.token === loaded.token ? loaded : current
        )
      })
      .catch((e) => setError(e instanceof Error ? e.message : String(e)))
  }

  const panel = (
    <aside className="flex h-full w-72 shrink-0 flex-col border-r border-border bg-card/95 shadow-xl backdrop-blur-xl">
      <div className="flex items-center justify-between border-b border-border px-4 py-3">
        <Link
          to="/"
          className="inline-flex items-center gap-2 text-xs text-muted-foreground hover:text-foreground"
        >
          <ArrowLeft className="size-3.5" /> 返回对话
        </Link>
        <Button variant="ghost" size="icon-sm" onClick={() => setMobilePanel(false)} className="md:hidden">
          <X />
        </Button>
      </div>
      <div className="nc-scroll flex-1 space-y-5 overflow-y-auto p-4">
        <section>
          <div className="mb-2 flex items-center justify-between">
            <h2 className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">节点</h2>
            <span className="text-[10px] text-muted-foreground">点击新增</span>
          </div>
          <div className="grid grid-cols-2 gap-2">
            {(Object.keys(NODE_META) as WorkflowNodeType[]).map((type) => {
              const meta = NODE_META[type]
              const Icon = meta.icon
              return (
                <button
                  type="button"
                  key={type}
                  onClick={() => addNode(type)}
                  className="group rounded-xl border border-border bg-background/65 p-2.5 text-left transition hover:-translate-y-0.5 hover:border-primary/30 hover:shadow-sm"
                >
                  <Icon className={cn("mb-2 size-4", meta.color)} />
                  <span className="block text-xs font-medium">{meta.label}</span>
                  <span className="mt-0.5 block text-[10px] leading-tight text-muted-foreground">{meta.hint}</span>
                </button>
              )
            })}
          </div>
          <Button
            variant="outline"
            size="sm"
            className="mt-2 w-full"
            onClick={() => {
              setGraph(starterGraph(imageModels, videoModels))
              setCurrentRun(null)
              setMobilePanel(false)
            }}
          >
            <Boxes /> 载入双视频合并示例
          </Button>
        </section>

        <section>
          <div className="mb-2 flex items-center justify-between">
            <h2 className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">我的流水线</h2>
            <Button variant="ghost" size="icon-xs" onClick={createBlank} title="新建流水线">
              <Plus />
            </Button>
          </div>
          <div className="space-y-1">
            {workflows.length === 0 ? (
              <p className="rounded-lg border border-dashed border-border p-3 text-center text-xs text-muted-foreground">尚未保存流水线</p>
            ) : (
              workflows.map((workflow) => (
                <button
                  type="button"
                  key={workflow.id}
                  onClick={() => loadWorkflow(workflow)}
                  className={cn(
                    "flex w-full items-center gap-2 rounded-lg px-2.5 py-2 text-left text-xs transition hover:bg-accent",
                    workflow.id === workflowId && !currentRun && "bg-accent text-accent-foreground"
                  )}
                >
                  <Link2 className="size-3.5 shrink-0" />
                  <span className="truncate">{workflow.name}</span>
                </button>
              ))
            )}
          </div>
        </section>

        <section>
          <h2 className="mb-2 text-xs font-semibold uppercase tracking-wider text-muted-foreground">最近运行</h2>
          <div className="space-y-1">
            {runs.slice(0, 10).map((run) => (
              <button
                type="button"
                key={run.token}
                onClick={() => openRun(run)}
                className={cn(
                  "flex w-full items-center justify-between gap-2 rounded-lg px-2.5 py-2 text-left text-xs transition hover:bg-accent",
                  currentRun?.token === run.token && "bg-accent text-accent-foreground"
                )}
              >
                <span className="min-w-0 truncate">{run.name}</span>
                <span
                  className={cn(
                    "shrink-0 text-[10px]",
                    run.status === "completed" && "text-emerald-600",
                    run.status === "failed" && "text-destructive",
                    run.status === "running" && "text-primary"
                  )}
                >
                  {runStatusText(run.status)}
                </span>
              </button>
            ))}
          </div>
        </section>
      </div>
    </aside>
  )

  return (
    <div className="flex h-svh overflow-hidden bg-background text-foreground">
      <div className="hidden md:block">{panel}</div>
      {mobilePanel && (
        <>
          <button type="button" aria-label="关闭节点面板" className="fixed inset-0 z-40 bg-black/45 md:hidden" onClick={() => setMobilePanel(false)} />
          <div className="fixed inset-y-0 left-0 z-50 md:hidden">{panel}</div>
        </>
      )}

      <div className="flex min-w-0 flex-1 flex-col">
        <header className="flex h-16 shrink-0 items-center gap-2 border-b border-border bg-card/85 px-3 backdrop-blur-xl sm:px-5">
          <Button variant="ghost" size="icon-sm" className="md:hidden" onClick={() => setMobilePanel(true)}>
            <Menu />
          </Button>
          <div className="hidden size-9 place-items-center rounded-xl bg-primary/10 text-primary sm:grid">
            <Boxes className="size-4" />
          </div>
          <Input
            value={name}
            onChange={(event) => setName(event.target.value)}
            className="h-9 max-w-72 border-transparent bg-transparent px-2 font-semibold shadow-none hover:border-input focus-visible:bg-background"
            aria-label="流水线名称"
          />
          <div className="ml-auto flex items-center gap-1.5">
            {connectingFrom && (
              <button
                type="button"
                onClick={() => setConnectingFrom(null)}
                className="hidden items-center gap-1.5 rounded-lg bg-primary/10 px-2.5 py-1.5 text-xs text-primary sm:flex"
              >
                <MousePointer2 className="size-3.5" /> 点击目标节点左侧端口 · 取消
              </button>
            )}
            {workflowId != null && (
              <Button variant="ghost" size="icon-sm" onClick={() => void removeCurrentWorkflow()} title="删除流水线">
                <Trash2 />
              </Button>
            )}
            <Button variant="outline" size="sm" onClick={() => void save()} disabled={saving || loading}>
              {saving ? <Loader2 className="animate-spin" /> : <Save />}
              <span className="hidden sm:inline">保存</span>
            </Button>
            {currentRun?.status === "running" ? (
              <Button variant="outline" size="sm" onClick={() => void cancelCurrentRun()}>
                <Square />
                <span className="hidden sm:inline">停止后续</span>
              </Button>
            ) : (
              <Button size="sm" onClick={() => void startRun()} disabled={starting || loading}>
                {starting ? <Loader2 className="animate-spin" /> : <Play />}
                <span className="sm:hidden">运行</span>
                <span className="hidden sm:inline">
                  运行流水线{estimatedQuota > 0 ? ` · 预计 ${estimatedQuota} 额度` : ""}
                </span>
              </Button>
            )}
          </div>
        </header>

        {error && (
          <div className="flex shrink-0 items-start gap-2 border-b border-destructive/25 bg-destructive/5 px-4 py-2 text-xs text-destructive">
            <X className="mt-0.5 size-3.5 shrink-0" />
            <span className="min-w-0 flex-1 break-words">{error}</span>
            <button type="button" onClick={() => setError(null)} aria-label="关闭错误"><X className="size-3.5" /></button>
          </div>
        )}

        <main
          ref={canvasViewportRef}
          className={cn(
            "nc-scroll relative flex-1 overflow-auto",
            pan ? "cursor-grabbing select-none" : "cursor-grab"
          )}
          onPointerDown={(event) => {
            if (event.pointerType !== "mouse" || event.button !== 0 || drag || pan) return
            const target = event.target
            if (
              target instanceof Element &&
              target.closest("[data-workflow-pan-block]")
            ) {
              return
            }
            event.preventDefault()
            event.currentTarget.setPointerCapture(event.pointerId)
            setPan({
              pointerId: event.pointerId,
              clientX: event.clientX,
              clientY: event.clientY,
              scrollLeft: event.currentTarget.scrollLeft,
              scrollTop: event.currentTarget.scrollTop,
            })
          }}
          style={{
            backgroundImage:
              "radial-gradient(circle, color-mix(in oklch, var(--foreground) 14%, transparent) 1px, transparent 1px)",
            backgroundSize: "24px 24px",
          }}
        >
          <div className="relative" style={{ width: CANVAS_WIDTH, height: CANVAS_HEIGHT }}>
            <svg
              className="absolute inset-0 z-0 overflow-visible"
              width={CANVAS_WIDTH}
              height={CANVAS_HEIGHT}
              aria-hidden="true"
            >
              {graph.edges.map((edge) => {
                const source = graph.nodes.find((node) => node.id === edge.source)
                const target = graph.nodes.find((node) => node.id === edge.target)
                if (!source || !target) return null
                const sx = source.x + NODE_WIDTH
                const sy = source.y + PORT_Y
                const tx = target.x
                const ty = target.y + PORT_Y
                const bend = Math.max(70, Math.abs(tx - sx) * 0.45)
                const d = `M ${sx} ${sy} C ${sx + bend} ${sy}, ${tx - bend} ${ty}, ${tx} ${ty}`
                const sourceRun = runNodes.get(source.id)
                const active = sourceRun?.status === "completed"
                return (
                  <g key={edge.id} className="group/edge" data-workflow-pan-block>
                    <path d={d} fill="none" stroke="transparent" strokeWidth="16" className="pointer-events-auto cursor-pointer" onClick={() => setGraph((current) => ({ ...current, edges: current.edges.filter((item) => item.id !== edge.id) }))} />
                    <path
                      d={d}
                      fill="none"
                      stroke="currentColor"
                      strokeWidth="2.5"
                      className={cn(
                        "pointer-events-none text-border transition-colors group-hover/edge:text-destructive",
                        active && "text-primary"
                      )}
                    />
                  </g>
                )
              })}
            </svg>

            {graph.nodes.map((node) => (
              <CanvasNodeCard
                key={node.id}
                node={node}
                run={runNodes.get(node.id)}
                incomingCount={graph.edges.filter((edge) => edge.target === node.id).length}
                mergeInputs={
                  node.type === "video_merge" ? mergeInputsFor(graph, node.id) : []
                }
                imageModels={imageModels}
                videoModels={videoModels}
                connecting={connectingFrom === node.id}
                onStartDrag={(event) => {
                  event.preventDefault()
                  setDrag({
                    nodeId: node.id,
                    clientX: event.clientX,
                    clientY: event.clientY,
                    originX: node.x,
                    originY: node.y,
                  })
                }}
                onConnectFrom={() => {
                  setConnectingFrom((current) => (current === node.id ? null : node.id))
                  setError(null)
                }}
                onConnectTo={() => connectTo(node.id)}
                onRemove={() => removeNode(node.id)}
                onChange={(patch) => updateNodeData(node.id, patch)}
                onEdgePriorityChange={updateEdgePriority}
                onRetry={() => void retryNode(node.id)}
              />
            ))}

            {graph.nodes.length === 0 && !loading && (
              <div data-workflow-pan-block className="absolute left-1/2 top-1/3 flex w-80 -translate-x-1/2 flex-col items-center rounded-2xl border border-dashed border-border bg-card/80 p-8 text-center shadow-sm backdrop-blur">
                <div className="mb-3 grid size-12 place-items-center rounded-2xl bg-primary/10 text-primary">
                  <Plus className="size-5" />
                </div>
                <p className="text-sm font-semibold">从左侧新增第一个节点</p>
                <p className="mt-1 text-xs leading-relaxed text-muted-foreground">拖动节点标题调整位置；点击输出端口，再点击另一个节点的输入端口完成连线。</p>
                <Button variant="outline" size="sm" className="mt-4" onClick={() => setGraph(starterGraph(imageModels, videoModels))}>
                  载入示例
                </Button>
              </div>
            )}
          </div>
        </main>

        {currentRun && (
          <section
            className={cn(
              "shrink-0 border-t border-border bg-card/95 transition-[height]",
              logsOpen ? "h-48 sm:h-56" : "h-10"
            )}
            aria-label="运行日志"
          >
            <button
              type="button"
              className="flex h-10 w-full items-center gap-2 px-4 text-left text-xs hover:bg-accent/45"
              onClick={() => setLogsOpen((open) => !open)}
              aria-expanded={logsOpen}
            >
              <Terminal className="size-3.5 text-primary" />
              <span className="font-medium">运行日志</span>
              <span className="rounded-full bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground">
                {currentRun.logs.length}
              </span>
              {currentRun.status === "running" && (
                <span className="ml-auto inline-flex items-center gap-1 text-[10px] text-primary">
                  <Loader2 className="size-3 animate-spin" /> 实时刷新
                </span>
              )}
              {currentRun.status !== "running" && <span className="ml-auto" />}
              {logsOpen ? (
                <ChevronDown className="size-3.5 text-muted-foreground" />
              ) : (
                <ChevronUp className="size-3.5 text-muted-foreground" />
              )}
            </button>
            {logsOpen && (
              <div
                ref={logScrollRef}
                className="nc-scroll h-[calc(100%-2.5rem)] overflow-y-auto border-t border-border/60 bg-background/55 px-4 py-2 font-mono text-[11px]"
              >
                {currentRun.logs.length === 0 ? (
                  <p className="py-5 text-center font-sans text-xs text-muted-foreground">
                    {currentRun.status === "running"
                      ? "等待运行事件…"
                      : "这次运行没有可用日志"}
                  </p>
                ) : (
                  <div className="space-y-1.5">
                    {currentRun.logs.map((log) => (
                      <div key={log.id} className="flex min-w-0 items-start gap-2 leading-relaxed">
                        <time className="shrink-0 tabular-nums text-muted-foreground">
                          {formatLogTime(log.created_at)}
                        </time>
                        <span
                          className={cn(
                            "mt-1.5 size-1.5 shrink-0 rounded-full bg-current",
                            logLevelClass(log.level)
                          )}
                        />
                        <span className="w-16 shrink-0 truncate text-muted-foreground sm:w-20">
                          {log.node_id
                            ? nodeNames.get(log.node_id) ?? log.node_id
                            : "流水线"}
                        </span>
                        <span className={cn("min-w-0 break-words", logLevelClass(log.level))}>
                          {log.message}
                        </span>
                      </div>
                    ))}
                  </div>
                )}
              </div>
            )}
          </section>
        )}

        <footer className="flex h-8 shrink-0 items-center justify-between border-t border-border bg-card/80 px-4 text-[10px] text-muted-foreground">
          <span>{graph.nodes.length} 个节点 · {graph.edges.length} 条连线 · 拖动画布空白处平移 · 点击连线可删除</span>
          {currentRun && (
            <span className={cn(currentRun.status === "running" && "text-primary", currentRun.status === "failed" && "text-destructive", currentRun.status === "completed" && "text-emerald-600")}>
              本次运行：{runStatusText(currentRun.status)}
            </span>
          )}
        </footer>
      </div>
    </div>
  )
}

function CanvasNodeCard({
  node,
  run,
  incomingCount,
  mergeInputs,
  imageModels,
  videoModels,
  connecting,
  onStartDrag,
  onConnectFrom,
  onConnectTo,
  onRemove,
  onChange,
  onEdgePriorityChange,
  onRetry,
}: {
  node: WorkflowNode
  run?: WorkflowNodeRun
  incomingCount: number
  mergeInputs: MergeInput[]
  imageModels: PlatformModel[]
  videoModels: VideoModel[]
  connecting: boolean
  onStartDrag: (event: React.PointerEvent) => void
  onConnectFrom: () => void
  onConnectTo: () => void
  onRemove: () => void
  onChange: (patch: WorkflowNodeData) => void
  onEdgePriorityChange: (edgeId: string, priority: number) => void
  onRetry: () => void
}) {
  const meta = NODE_META[node.type]
  const Icon = meta.icon
  const selectedVideoModel =
    node.type === "video_generation"
      ? videoModels.find((model) => model.model === node.data.model)
      : undefined
  const failed = run?.status === "failed" || run?.status === "blocked"
  const busy = run?.status === "starting" || run?.status === "running"

  return (
    <article
      data-workflow-pan-block
      className={cn(
        "absolute z-10 rounded-2xl border bg-card/95 shadow-lg shadow-black/5 backdrop-blur-xl transition-shadow",
        connecting && "border-primary ring-4 ring-primary/10",
        failed && "border-destructive/45",
        run?.status === "completed" && "border-emerald-500/35"
      )}
      style={{ left: node.x, top: node.y, width: NODE_WIDTH }}
    >
      <button
        type="button"
        aria-label={`连接到${meta.label}`}
        onClick={onConnectTo}
        className="absolute -left-3 top-10 z-20 grid size-6 place-items-center rounded-full border-2 border-card bg-muted text-muted-foreground shadow transition hover:scale-110 hover:bg-primary hover:text-primary-foreground"
        title="输入端口"
      >
        <span className="size-2 rounded-full bg-current" />
      </button>
      <button
        type="button"
        aria-label={`从${meta.label}开始连线`}
        onClick={onConnectFrom}
        className={cn(
          "absolute -right-3 top-10 z-20 grid size-6 place-items-center rounded-full border-2 border-card bg-muted text-muted-foreground shadow transition hover:scale-110 hover:bg-primary hover:text-primary-foreground",
          connecting && "animate-pulse bg-primary text-primary-foreground"
        )}
        title="输出端口"
      >
        <span className="size-2 rounded-full bg-current" />
      </button>

      <header
        onPointerDown={onStartDrag}
        className="flex cursor-grab touch-none items-center gap-2 border-b border-border px-3 py-3 active:cursor-grabbing"
      >
        <span className={cn("grid size-8 place-items-center rounded-lg bg-muted", meta.color)}>
          <Icon className="size-4" />
        </span>
        <div className="min-w-0 flex-1">
          <h3 className="text-sm font-semibold">{meta.label}</h3>
          <p className="text-[10px] text-muted-foreground">{meta.hint}</p>
        </div>
        {run && (
          <span
            className={cn(
              "rounded-full bg-muted px-2 py-1 text-[10px] text-muted-foreground",
              busy && "bg-primary/10 text-primary",
              run.status === "completed" && "bg-emerald-500/10 text-emerald-600",
              failed && "bg-destructive/10 text-destructive"
            )}
          >
            {busy && <Loader2 className="mr-1 inline size-3 animate-spin" />}
            {run.status === "completed" && <Check className="mr-1 inline size-3" />}
            {statusText(run.status)}
          </span>
        )}
        <button
          type="button"
          onPointerDown={(event) => event.stopPropagation()}
          onClick={onRemove}
          className="rounded-md p-1 text-muted-foreground hover:bg-destructive/10 hover:text-destructive"
          title="删除节点"
        >
          <Trash2 className="size-3.5" />
        </button>
      </header>

      <div className="space-y-3 p-3">
        {node.type === "image_generation" && (
          <>
            <Field label="模型">
              <Select value={String(node.data.model ?? "")} onValueChange={(model) => onChange({ model })}>
                <SelectTrigger size="sm" className="w-full"><SelectValue placeholder="选择图片模型" /></SelectTrigger>
                <SelectContent>
                  {imageModels.map((model) => <SelectItem key={model.model} value={model.model}>{model.display_name ?? model.model}</SelectItem>)}
                </SelectContent>
              </Select>
            </Field>
            <Field label={incomingCount ? "编辑指令" : "图片提示词"}>
              <Textarea value={String(node.data.prompt ?? "")} onChange={(event) => onChange({ prompt: event.target.value })} className="min-h-20 resize-y text-xs" placeholder={incomingCount ? "描述如何修改输入图片…" : "描述希望生成的画面…"} />
            </Field>
            <div className="grid grid-cols-2 gap-2">
              <Field label="尺寸">
                <Select value={String(node.data.size ?? "1024x1024")} onValueChange={(size) => onChange({ size })}>
                  <SelectTrigger size="sm" className="w-full"><SelectValue /></SelectTrigger>
                  <SelectContent>
                    <SelectItem value="1024x1024">1024×1024</SelectItem>
                    <SelectItem value="1024x1792">1024×1792</SelectItem>
                    <SelectItem value="1792x1024">1792×1024</SelectItem>
                    <SelectItem value="auto">自动</SelectItem>
                  </SelectContent>
                </Select>
              </Field>
              <Field label="质量">
                <Select value={String(node.data.quality ?? "auto")} onValueChange={(quality) => onChange({ quality })}>
                  <SelectTrigger size="sm" className="w-full"><SelectValue /></SelectTrigger>
                  <SelectContent>
                    <SelectItem value="auto">自动</SelectItem>
                    <SelectItem value="low">低</SelectItem>
                    <SelectItem value="medium">中</SelectItem>
                    <SelectItem value="high">高</SelectItem>
                  </SelectContent>
                </Select>
              </Field>
            </div>
          </>
        )}

        {node.type === "video_generation" && (
          <>
            <Field label="模型">
              <Select
                value={String(node.data.model ?? "")}
                onValueChange={(modelName) => {
                  const model = videoModels.find((item) => item.model === modelName)
                  onChange({
                    model: modelName,
                    seconds: model?.allowed_seconds[0] ?? 5,
                    size: model?.size_rules[0]?.size ?? "1280x720",
                  })
                }}
              >
                <SelectTrigger size="sm" className="w-full"><SelectValue placeholder="选择视频模型" /></SelectTrigger>
                <SelectContent>
                  {videoModels.map((model) => <SelectItem key={model.model} value={model.model}>{model.display_name ?? model.model}</SelectItem>)}
                </SelectContent>
              </Select>
            </Field>
            <Field label={incomingCount ? "运动提示词（已接参考图）" : "视频提示词"}>
              <Textarea value={String(node.data.prompt ?? "")} onChange={(event) => onChange({ prompt: event.target.value })} className="min-h-20 resize-y text-xs" placeholder="描述镜头运动和画面变化…" />
            </Field>
            <div className="grid grid-cols-2 gap-2">
              <Field label="时长">
                <Select value={String(node.data.seconds ?? 5)} onValueChange={(seconds) => onChange({ seconds: Number(seconds) })}>
                  <SelectTrigger size="sm" className="w-full"><SelectValue /></SelectTrigger>
                  <SelectContent>
                    {(selectedVideoModel?.allowed_seconds ?? [5]).map((seconds) => <SelectItem key={seconds} value={String(seconds)}>{seconds} 秒</SelectItem>)}
                  </SelectContent>
                </Select>
              </Field>
              <Field label="尺寸">
                <Select value={String(node.data.size ?? "1280x720")} onValueChange={(size) => onChange({ size })}>
                  <SelectTrigger size="sm" className="w-full"><SelectValue /></SelectTrigger>
                  <SelectContent>
                    {(selectedVideoModel?.size_rules ?? [{ size: "1280x720", multiplier: 100 }]).map((rule) => <SelectItem key={rule.size} value={rule.size}>{rule.size.replace("x", "×")}</SelectItem>)}
                  </SelectContent>
                </Select>
              </Field>
            </div>
          </>
        )}

        {node.type === "video_trim" && (
          <>
            <p className="rounded-lg bg-muted/60 px-2.5 py-2 text-[10px] leading-relaxed text-muted-foreground">接入一个视频，精确到 0.1 秒重新编码裁剪。</p>
            <div className="grid grid-cols-2 gap-2">
              <Field label="开始（秒）">
                <Input type="number" min={0} step={0.1} value={Number(node.data.start ?? 0)} onChange={(event) => onChange({ start: Number(event.target.value) })} className="h-8" />
              </Field>
              <Field label="结束（秒）">
                <Input type="number" min={0.1} step={0.1} value={Number(node.data.end ?? 5)} onChange={(event) => onChange({ end: Number(event.target.value) })} className="h-8" />
              </Field>
            </div>
          </>
        )}

        {node.type === "video_merge" && (
          <>
            <p className="rounded-lg bg-muted/60 px-2.5 py-2 text-[10px] leading-relaxed text-muted-foreground">
              已连接 {incomingCount} 个视频。优先级数字越小越先拼接，相同时按连线创建顺序。
            </p>
            {mergeInputs.length > 0 && (
              <Field label="拼接顺序">
                <div className="nc-scroll max-h-40 space-y-1.5 overflow-y-auto rounded-lg border border-border bg-background/50 p-1.5">
                  {mergeInputs.map(({ edge, source }, index) => {
                    const prompt = String(source.data.prompt ?? "").trim()
                    const sourceLabel = prompt || source.id
                    return (
                      <div
                        key={edge.id}
                        className="flex items-center gap-2 rounded-md bg-muted/55 px-2 py-1.5"
                      >
                        <span className="grid size-5 shrink-0 place-items-center rounded-full bg-primary/10 text-[10px] font-semibold text-primary">
                          {index + 1}
                        </span>
                        <div className="min-w-0 flex-1" title={sourceLabel}>
                          <p className="text-[10px] font-medium">
                            {NODE_META[source.type].label}
                          </p>
                          <p className="truncate text-[9px] text-muted-foreground">
                            {sourceLabel}
                          </p>
                        </div>
                        <Input
                          type="number"
                          min={0}
                          max={MAX_EDGE_PRIORITY}
                          step={1}
                          value={edge.priority ?? DEFAULT_EDGE_PRIORITY}
                          onChange={(event) =>
                            onEdgePriorityChange(edge.id, event.target.valueAsNumber)
                          }
                          className="h-7 w-[4.5rem] px-2 text-right text-xs"
                          aria-label={`${NODE_META[source.type].label}优先级`}
                          title="优先级，数字越小越靠前"
                        />
                      </div>
                    )
                  })}
                </div>
              </Field>
            )}
            <Field label="输出画幅">
              <Select
                value={`${node.data.width ?? 1280}x${node.data.height ?? 720}`}
                onValueChange={(value) => {
                  const [width, height] = value.split("x").map(Number)
                  onChange({ width: width!, height: height! })
                }}
              >
                <SelectTrigger size="sm" className="w-full"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="1280x720">1280×720 横版</SelectItem>
                  <SelectItem value="720x1280">720×1280 竖版</SelectItem>
                  <SelectItem value="1024x1024">1024×1024 方形</SelectItem>
                  <SelectItem value="1920x1080">1920×1080 高清</SelectItem>
                </SelectContent>
              </Select>
            </Field>
          </>
        )}

        {run?.output_paths[0] && (
          <div className="overflow-hidden rounded-xl border border-border bg-black">
            {outputKind(node.type) === "image" ? (
              <a href={run.output_paths[0]} target="_blank" rel="noreferrer">
                <img src={run.output_paths[0]} alt="节点生成结果" className="max-h-52 w-full object-contain" />
              </a>
            ) : (
              <video controls preload="metadata" src={run.output_paths[0]} className="max-h-52 w-full" />
            )}
          </div>
        )}

        {run?.error && (
          <div className="rounded-lg border border-destructive/25 bg-destructive/5 p-2 text-[10px] leading-relaxed text-destructive">
            <p className="line-clamp-4 break-words">{run.error}</p>
            {(run.status === "failed" || run.status === "blocked") && (
              <Button variant="outline" size="xs" className="mt-2" onClick={onRetry}>
                <RotateCcw /> 从此节点重试
              </Button>
            )}
          </div>
        )}
      </div>
    </article>
  )
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="space-y-1.5">
      <Label className="text-[10px] text-muted-foreground">{label}</Label>
      {children}
    </div>
  )
}
