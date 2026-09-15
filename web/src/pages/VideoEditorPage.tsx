import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from "react"
import { Link, useNavigate, useParams } from "react-router-dom"
import { formatQuota } from "@/lib/quota"
import {
  ArrowLeft,
  Captions,
  Check,
  Clapperboard,
  Copy,
  Download,
  Eye,
  EyeOff,
  Film,
  FolderOpen,
  GripVertical,
  ImageIcon,
  Layers3,
  Library,
  Loader2,
  Lock,
  Music2,
  Pause,
  Play,
  Plus,
  Redo2,
  Save,
  Scissors,
  Search,
  Settings2,
  Sparkles,
  Trash2,
  Undo2,
  Unlock,
  Upload,
  Video,
  Volume2,
  VolumeX,
  WandSparkles,
  X,
  ZoomIn,
  ZoomOut,
} from "lucide-react"
import { toast } from "sonner"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectSeparator,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Textarea } from "@/components/ui/textarea"
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet"
import { useConfirm } from "@/lib/confirm-context"
import { cn } from "@/lib/utils"
import {
  editorId,
  emptyTimeline,
  inspectMediaFile,
  timelineDuration,
  videoEditorApi,
  type EditorAsset,
  type EditorClip,
  type EditorExport,
  type EditorProject,
  type EditorTimeline,
  type EditorTrack,
  type EditorTrackKind,
} from "@/lib/video-editor"
import {
  computeVideoCost,
  listVideoModels,
  type VideoModel,
} from "@/lib/video-gen"
import { workflowApi, type WorkflowGraph } from "@/lib/workflows"

const DEFAULT_ZOOM = 64
const MIN_ZOOM = 24
const MAX_ZOOM = 240
const AUTOSAVE_MS = 1400
const TRACK_DRAG_TYPE = "application/x-yunova-editor-track"

type SequenceFormat = {
  width: number
  height: number
  label: string
}

type SequenceFormatGroup = {
  label: string
  formats: readonly SequenceFormat[]
}

const SEQUENCE_FORMAT_GROUPS: readonly SequenceFormatGroup[] = [
  {
    label: "横屏 · 16:9",
    formats: [
      { width: 1280, height: 720, label: "HD 720p" },
      { width: 1920, height: 1080, label: "Full HD 1080p" },
      { width: 2560, height: 1440, label: "QHD 1440p" },
      { width: 3840, height: 2160, label: "Ultra HD 4K" },
    ],
  },
  {
    label: "竖屏 · 9:16",
    formats: [
      { width: 720, height: 1280, label: "HD 720p" },
      { width: 1080, height: 1920, label: "Full HD 1080p" },
      { width: 1440, height: 2560, label: "QHD 1440p" },
      { width: 2160, height: 3840, label: "Ultra HD 4K" },
    ],
  },
  {
    label: "方形 · 1:1",
    formats: [
      { width: 720, height: 720, label: "HD" },
      { width: 1080, height: 1080, label: "Full HD" },
      { width: 1440, height: 1440, label: "QHD" },
      { width: 2160, height: 2160, label: "Ultra HD" },
    ],
  },
]

const STANDARD_SEQUENCE_FORMATS = SEQUENCE_FORMAT_GROUPS.flatMap(
  (group) => group.formats
)

function sequenceFormatValue(width: number, height: number): string {
  return `${width}x${height}`
}

function cloneTimeline(value: EditorTimeline): EditorTimeline {
  return structuredClone(value)
}

function formatTime(value: number, fps = 30): string {
  const safe = Math.max(0, value)
  const hours = Math.floor(safe / 3600)
  const minutes = Math.floor((safe % 3600) / 60)
  const seconds = Math.floor(safe % 60)
  const frames = Math.floor((safe - Math.floor(safe)) * fps)
  return [hours, minutes, seconds, frames]
    .map((part) => String(part).padStart(2, "0"))
    .join(":")
}

function formatExportTime(value: string): string {
  const normalized = /^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}/.test(value)
    ? `${value.replace(" ", "T")}Z`
    : value
  const date = new Date(normalized)
  if (Number.isNaN(date.getTime())) return value
  return date.toLocaleString("zh-CN", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  })
}

function clipAt(
  timeline: EditorTimeline,
  clipId: string | null
): { track: EditorTrack; clip: EditorClip } | null {
  if (!clipId) return null
  for (const track of timeline.tracks) {
    const clip = track.clips.find((item) => item.id === clipId)
    if (clip) return { track, clip }
  }
  return null
}

function trackIcon(kind: EditorTrackKind) {
  if (kind === "audio") return Music2
  if (kind === "text") return Captions
  return Video
}

function sourceLabel(source: EditorAsset["source"]): string {
  switch (source) {
    case "generated":
      return "AI 生成"
    case "workflow":
      return "流水线"
    case "public":
      return "公有素材"
    case "imported":
      return "已收藏"
    default:
      return "本地上传"
  }
}

function videoTrackGap(timeline: EditorTimeline, time: number) {
  const track = timeline.tracks.find((item) => item.kind === "video")
  if (!track) return { start: time, duration: 5 }
  const clips = [...track.clips].sort((a, b) => a.start - b.start)
  const before = clips.filter((clip) => clip.start + clip.duration <= time).at(-1)
  const covering = clips.find(
    (clip) => time >= clip.start && time < clip.start + clip.duration
  )
  const start = covering ? covering.start + covering.duration : before?.start != null
    ? before.start + before.duration
    : time
  const after = clips.find((clip) => clip.start >= start + 0.001)
  return {
    start,
    duration: after ? Math.max(0.1, after.start - start) : 5,
  }
}

export default function VideoEditorPage() {
  const { id: routeId } = useParams()
  const navigate = useNavigate()
  const { confirm } = useConfirm()
  const initialId = routeId && Number.isFinite(Number(routeId)) ? Number(routeId) : null

  const [projectId, setProjectId] = useState<number | null>(initialId)
  const [projectName, setProjectName] = useState("未命名剪辑")
  const [projects, setProjects] = useState<EditorProject[]>([])
  const [timeline, setTimeline] = useState<EditorTimeline>(() => emptyTimeline())
  const timelineRef = useRef(timeline)
  const [selectedClipId, setSelectedClipId] = useState<string | null>(null)
  const [selectedTrackId, setSelectedTrackId] = useState<string | null>(null)
  const [playhead, setPlayhead] = useState(0)
  const playheadRef = useRef(0)
  const [playing, setPlaying] = useState(false)
  const [zoom, setZoom] = useState(DEFAULT_ZOOM)
  const [snap, setSnap] = useState(true)
  const [dirty, setDirty] = useState(false)
  const editRevision = useRef(0)
  const [saveState, setSaveState] = useState<"idle" | "saving" | "saved" | "error">("idle")
  const [loading, setLoading] = useState(true)
  const [mobilePanel, setMobilePanel] = useState<"assets" | "inspector" | null>(null)

  const [undoStack, setUndoStack] = useState<EditorTimeline[]>([])
  const [redoStack, setRedoStack] = useState<EditorTimeline[]>([])
  const continuousOrigin = useRef<EditorTimeline | null>(null)

  const [mineAssets, setMineAssets] = useState<EditorAsset[]>([])
  const [publicAssets, setPublicAssets] = useState<EditorAsset[]>([])
  const [assetScope, setAssetScope] = useState<"mine" | "public">("mine")
  const [assetQuery, setAssetQuery] = useState("")
  const [assetKind, setAssetKind] = useState<"all" | EditorAsset["kind"]>("all")
  const [assetsLoading, setAssetsLoading] = useState(false)
  const [uploading, setUploading] = useState(false)
  const uploadRef = useRef<HTMLInputElement>(null)

  const [videoModels, setVideoModels] = useState<VideoModel[]>([])
  const [generatorOpen, setGeneratorOpen] = useState(false)
  const [generationPrompt, setGenerationPrompt] = useState("")
  const [generationModel, setGenerationModel] = useState("")
  const [generationSeconds, setGenerationSeconds] = useState(5)
  const [generating, setGenerating] = useState(false)
  const [generationStatus, setGenerationStatus] = useState("")

  const [exports, setExports] = useState<EditorExport[]>([])
  const [exportSubmitting, setExportSubmitting] = useState(false)
  const [exportQueueOpen, setExportQueueOpen] = useState(false)
  const [deletingExportToken, setDeletingExportToken] = useState<string | null>(null)
  const exportStatusesRef = useRef(new Map<string, EditorExport["status"]>())
  const exportPollErrorShownRef = useRef(false)

  useEffect(() => {
    timelineRef.current = timeline
  }, [timeline])

  useEffect(() => {
    playheadRef.current = playhead
  }, [playhead])

  const reloadAssets = useCallback(async () => {
    setAssetsLoading(true)
    try {
      const [mine, shared] = await Promise.all([
        videoEditorApi.assets("mine"),
        videoEditorApi.assets("public"),
      ])
      setMineAssets(mine)
      setPublicAssets(shared)
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error))
    } finally {
      setAssetsLoading(false)
    }
  }, [])

  useEffect(() => {
    let cancelled = false
    Promise.all([
      videoEditorApi.projects(),
      videoEditorApi.assets("mine"),
      videoEditorApi.assets("public"),
      videoEditorApi.exports(),
      listVideoModels().catch(() => [] as VideoModel[]),
    ])
      .then(([savedProjects, mine, shared, recentExports, models]) => {
        if (cancelled) return
        setProjects(savedProjects)
        setMineAssets(mine)
        setPublicAssets(shared)
        setExports(recentExports)
        exportStatusesRef.current = new Map(
          recentExports.map((item) => [item.token, item.status])
        )
        setVideoModels(models)
        const first = models[0]
        if (first) {
          setGenerationModel(first.model)
          setGenerationSeconds(first.allowed_seconds[0] ?? 5)
        }
        if (initialId != null) {
          const project = savedProjects.find((item) => item.id === initialId)
          if (project) loadProjectState(project)
          else void loadProjectById(initialId)
        }
      })
      .catch((error) => {
        if (!cancelled) toast.error(error instanceof Error ? error.message : String(error))
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
    // The route id is only an initial load target. Project switching is handled
    // explicitly to avoid replacing an in-progress timeline after navigation.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  async function loadProjectById(id: number) {
    try {
      loadProjectState(await videoEditorApi.project(id))
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error))
      navigate("/editor", { replace: true })
    }
  }

  function loadProjectState(project: EditorProject) {
    const nextTimeline = cloneTimeline(project.timeline)
    setProjectId(project.id)
    setProjectName(project.name)
    timelineRef.current = nextTimeline
    setTimeline(nextTimeline)
    setSelectedClipId(null)
    setSelectedTrackId(project.timeline.tracks[0]?.id ?? null)
    setPlayhead(0)
    setPlaying(false)
    setDirty(false)
    editRevision.current = 0
    setSaveState("saved")
    setUndoStack([])
    setRedoStack([])
    navigate(`/editor/${project.id}`, { replace: true })
  }

  function newProject() {
    const next = emptyTimeline()
    setProjectId(null)
    setProjectName("未命名剪辑")
    timelineRef.current = next
    setTimeline(next)
    setSelectedClipId(null)
    setSelectedTrackId(next.tracks[0]?.id ?? null)
    setPlayhead(0)
    setPlaying(false)
    setDirty(false)
    editRevision.current = 0
    setSaveState("idle")
    setUndoStack([])
    setRedoStack([])
    navigate("/editor", { replace: true })
  }

  const commitTimeline = useCallback(
    (updater: EditorTimeline | ((value: EditorTimeline) => EditorTimeline)) => {
      setTimeline((previous) => {
        const next = typeof updater === "function" ? updater(cloneTimeline(previous)) : updater
        setUndoStack((stack) => [...stack.slice(-49), cloneTimeline(previous)])
        setRedoStack([])
        editRevision.current += 1
        setDirty(true)
        setSaveState("idle")
        timelineRef.current = next
        return next
      })
    },
    []
  )

  function beginContinuousEdit() {
    if (!continuousOrigin.current) continuousOrigin.current = cloneTimeline(timelineRef.current)
  }

  function previewTimeline(updater: (value: EditorTimeline) => EditorTimeline) {
    setTimeline((previous) => {
      const next = updater(cloneTimeline(previous))
      timelineRef.current = next
      return next
    })
    editRevision.current += 1
    setDirty(true)
    setSaveState("idle")
  }

  function endContinuousEdit() {
    const origin = continuousOrigin.current
    continuousOrigin.current = null
    if (!origin || JSON.stringify(origin) === JSON.stringify(timelineRef.current)) return
    setUndoStack((stack) => [...stack.slice(-49), origin])
    setRedoStack([])
  }

  const undo = useCallback(() => {
    setUndoStack((stack) => {
      const previous = stack.at(-1)
      if (!previous) return stack
      setRedoStack((redo) => [...redo.slice(-49), cloneTimeline(timelineRef.current)])
      const next = cloneTimeline(previous)
      timelineRef.current = next
      setTimeline(next)
      editRevision.current += 1
      setDirty(true)
      return stack.slice(0, -1)
    })
  }, [])

  const redo = useCallback(() => {
    setRedoStack((stack) => {
      const next = stack.at(-1)
      if (!next) return stack
      setUndoStack((undoItems) => [
        ...undoItems.slice(-49),
        cloneTimeline(timelineRef.current),
      ])
      const restored = cloneTimeline(next)
      timelineRef.current = restored
      setTimeline(restored)
      editRevision.current += 1
      setDirty(true)
      return stack.slice(0, -1)
    })
  }, [])

  async function saveProject(silent = false): Promise<number | null> {
    const savingRevision = editRevision.current
    setSaveState("saving")
    try {
      const result = await videoEditorApi.save(projectId, projectName, timelineRef.current)
      const savedId = result.id
      setProjectId(savedId)
      if (editRevision.current === savingRevision) {
        setDirty(false)
        setSaveState("saved")
      } else {
        setSaveState("idle")
      }
      const saved = await videoEditorApi.projects()
      setProjects(saved)
      navigate(`/editor/${savedId}`, { replace: true })
      if (!silent) toast.success("项目已保存")
      return savedId
    } catch (error) {
      setSaveState("error")
      if (!silent) toast.error(error instanceof Error ? error.message : String(error))
      return null
    }
  }

  useEffect(() => {
    if (!projectId || !dirty || continuousOrigin.current) return
    const timer = window.setTimeout(() => void saveProject(true), AUTOSAVE_MS)
    return () => window.clearTimeout(timer)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dirty, projectId, projectName, timeline, undoStack.length])

  useEffect(() => {
    if (!playing) return
    const startedAt = performance.now()
    const startTime = playheadRef.current
    let frame = 0
    const tick = (now: number) => {
      const next = startTime + (now - startedAt) / 1000
      const end = Math.max(0.1, timelineDuration(timelineRef.current))
      if (next >= end) {
        setPlayhead(end)
        setPlaying(false)
        return
      }
      setPlayhead(next)
      frame = requestAnimationFrame(tick)
    }
    frame = requestAnimationFrame(tick)
    return () => cancelAnimationFrame(frame)
  }, [playing])

  useEffect(() => {
    const handle = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null
      if (
        target?.matches("input, textarea, select") ||
        target?.isContentEditable
      ) {
        return
      }
      const command = event.metaKey || event.ctrlKey
      if (command && event.key.toLowerCase() === "s") {
        event.preventDefault()
        void saveProject()
      } else if (command && event.key.toLowerCase() === "z") {
        event.preventDefault()
        if (event.shiftKey) redo()
        else undo()
      } else if (command && event.key.toLowerCase() === "y") {
        event.preventDefault()
        redo()
      } else if (event.key === " ") {
        event.preventDefault()
        setPlaying((value) => !value)
      } else if (event.key === "Delete" || event.key === "Backspace") {
        if (selectedClipId) {
          event.preventDefault()
          removeSelectedClip()
        }
      } else if (event.key.toLowerCase() === "s" && selectedClipId) {
        event.preventDefault()
        splitSelectedClip()
      } else if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
        event.preventDefault()
        const direction = event.key === "ArrowLeft" ? -1 : 1
        setPlayhead((time) => Math.max(0, time + direction / timelineRef.current.fps))
      }
    }
    window.addEventListener("keydown", handle)
    return () => window.removeEventListener("keydown", handle)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedClipId, undo, redo])

  const selected = useMemo(
    () => clipAt(timeline, selectedClipId),
    [timeline, selectedClipId]
  )
  const duration = timelineDuration(timeline)

  function changeSelectedClip(patch: Partial<EditorClip>) {
    if (!selectedClipId) return
    commitTimeline((next) => {
      for (const track of next.tracks) {
        const clip = track.clips.find((item) => item.id === selectedClipId)
        if (clip) Object.assign(clip, patch)
      }
      return next
    })
  }

  function removeSelectedClip() {
    if (!selectedClipId) return
    commitTimeline((next) => {
      for (const track of next.tracks) {
        track.clips = track.clips.filter((clip) => clip.id !== selectedClipId)
      }
      return next
    })
    setSelectedClipId(null)
  }

  function splitSelectedClip() {
    const found = clipAt(timelineRef.current, selectedClipId)
    if (!found) return
    const { clip, track } = found
    if (playhead <= clip.start + 0.01 || playhead >= clip.start + clip.duration - 0.01) {
      toast.info("将播放头放在片段内部再切割")
      return
    }
    const firstDuration = playhead - clip.start
    const second: EditorClip = {
      ...structuredClone(clip),
      id: editorId("clip"),
      start: playhead,
      duration: clip.duration - firstDuration,
      source_in: clip.source_in + firstDuration * clip.speed,
      fade_in: 0,
    }
    commitTimeline((next) => {
      const nextTrack = next.tracks.find((item) => item.id === track.id)
      const original = nextTrack?.clips.find((item) => item.id === clip.id)
      if (nextTrack && original) {
        original.duration = firstDuration
        original.fade_out = 0
        nextTrack.clips.push(second)
      }
      return next
    })
    setSelectedClipId(second.id)
  }

  function duplicateSelectedClip() {
    const found = clipAt(timelineRef.current, selectedClipId)
    if (!found) return
    const copy: EditorClip = {
      ...structuredClone(found.clip),
      id: editorId("clip"),
      start: found.clip.start + found.clip.duration + 0.1,
      name: `${found.clip.name} 副本`,
    }
    commitTimeline((next) => {
      next.tracks.find((track) => track.id === found.track.id)?.clips.push(copy)
      return next
    })
    setSelectedClipId(copy.id)
  }

  function addAsset(asset: EditorAsset, trackId?: string, at = playhead) {
    const targetKind = asset.kind === "audio" ? "audio" : "video"
    let newTrackId: string | null = null
    const clip: EditorClip = {
      id: editorId("clip"),
      name: asset.title || "未命名素材",
      asset_path: asset.path,
      asset_kind: asset.kind,
      start: Math.max(0, at),
      duration: asset.kind === "image" ? 5 : Math.max(0.1, asset.duration ?? 5),
      source_in: 0,
      speed: 1,
      opacity: 1,
      volume: 1,
      fade_in: 0,
      fade_out: 0,
      transform: { x: 0, y: 0, scale: 1, rotation: 0 },
      font_size: 64,
      color: "#ffffff",
    }
    commitTimeline((next) => {
      let target = next.tracks.find(
        (track) => track.id === trackId && track.kind === targetKind && !track.locked
      )
      target ??= next.tracks.find(
        (track) => track.kind === targetKind && !track.locked
      )
      if (!target) {
        target = {
          id: editorId("track"),
          name: `${targetKind === "video" ? "V" : "A"}${
            next.tracks.filter((item) => item.kind === targetKind).length + 1
          }`,
          kind: targetKind,
          hidden: false,
          muted: false,
          locked: false,
          clips: [],
        }
        next.tracks.push(target)
      }
      newTrackId = target.id
      target.clips.push(clip)
      return next
    })
    setSelectedClipId(clip.id)
    setSelectedTrackId(newTrackId)
  }

  function addTextClip() {
    const clip: EditorClip = {
      id: editorId("clip"),
      name: "文字",
      start: playhead,
      duration: 4,
      source_in: 0,
      speed: 1,
      opacity: 1,
      volume: 1,
      fade_in: 0,
      fade_out: 0,
      transform: { x: 0, y: 260, scale: 1, rotation: 0 },
      text: "双击右侧属性编辑文字",
      font_size: 64,
      color: "#ffffff",
    }
    commitTimeline((next) => {
      let target = next.tracks.find((track) => track.kind === "text" && !track.locked)
      if (!target) {
        target = {
          id: editorId("track"),
          name: `T${next.tracks.filter((item) => item.kind === "text").length + 1}`,
          kind: "text",
          hidden: false,
          muted: false,
          locked: false,
          clips: [],
        }
        next.tracks.unshift(target)
      }
      target.clips.push(clip)
      return next
    })
    setSelectedClipId(clip.id)
  }

  function addTrack(kind: EditorTrackKind) {
    commitTimeline((next) => {
      const prefix = kind === "video" ? "V" : kind === "audio" ? "A" : "T"
      const track: EditorTrack = {
        id: editorId("track"),
        name: `${prefix}${next.tracks.filter((item) => item.kind === kind).length + 1}`,
        kind,
        hidden: false,
        muted: false,
        locked: false,
        clips: [],
      }
      next.tracks.push(track)
      setSelectedTrackId(track.id)
      return next
    })
  }

  function updateTrack(id: string, patch: Partial<EditorTrack>) {
    commitTimeline((next) => {
      const track = next.tracks.find((item) => item.id === id)
      if (track) Object.assign(track, patch)
      return next
    })
  }

  function reorderTrack(
    sourceId: string,
    targetId: string,
    edge: "before" | "after"
  ) {
    const tracks = timelineRef.current.tracks
    const sourceIndex = tracks.findIndex((track) => track.id === sourceId)
    const targetIndex = tracks.findIndex((track) => track.id === targetId)
    if (sourceIndex < 0 || targetIndex < 0 || sourceIndex === targetIndex) return

    let insertIndex = targetIndex + (edge === "after" ? 1 : 0)
    if (sourceIndex < insertIndex) insertIndex -= 1
    if (sourceIndex === insertIndex) return

    commitTimeline((next) => {
      const nextSourceIndex = next.tracks.findIndex((track) => track.id === sourceId)
      if (nextSourceIndex < 0) return next
      const [track] = next.tracks.splice(nextSourceIndex, 1)
      next.tracks.splice(insertIndex, 0, track)
      return next
    })
    setSelectedTrackId(sourceId)
  }

  async function deleteCurrentProject() {
    if (!projectId) return
    const ok = await confirm({
      title: "删除这个剪辑项目？",
      description: "素材与已导出视频不会被删除，项目时间线将无法恢复。",
      confirmText: "删除项目",
      destructive: true,
    })
    if (!ok) return
    try {
      await videoEditorApi.removeProject(projectId)
      setProjects((items) => items.filter((item) => item.id !== projectId))
      newProject()
      toast.success("项目已删除")
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error))
    }
  }

  async function uploadFiles(files: FileList | File[]) {
    setUploading(true)
    try {
      for (const file of Array.from(files)) {
        const metadata = await inspectMediaFile(file)
        await videoEditorApi.uploadAsset(file, metadata)
      }
      await reloadAssets()
      toast.success("素材已加入个人素材库")
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error))
    } finally {
      setUploading(false)
      if (uploadRef.current) uploadRef.current.value = ""
    }
  }

  async function importPublicAsset(asset: EditorAsset) {
    try {
      await videoEditorApi.importAsset(asset)
      await reloadAssets()
      toast.success("已收藏到个人素材库")
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error))
    }
  }

  async function toggleAssetPublic(asset: EditorAsset) {
    try {
      let libraryId = asset.library_id
      if (!libraryId) {
        libraryId = (await videoEditorApi.importAsset(asset)).id
      }
      await videoEditorApi.setVisibility(libraryId, !asset.is_public)
      await reloadAssets()
      toast.success(asset.is_public ? "已设为私有" : "已发布到公有素材库")
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error))
    }
  }

  async function removeLibraryAsset(asset: EditorAsset) {
    if (!asset.library_id) return
    const ok = await confirm({
      title: "从个人素材库移除？",
      description: "已放入时间线的片段仍可使用，原始媒体文件不会立即删除。",
      confirmText: "移除",
      destructive: true,
    })
    if (!ok) return
    try {
      await videoEditorApi.removeAsset(asset.library_id)
      await reloadAssets()
      toast.success("已从个人素材库移除")
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error))
    }
  }

  async function generateMissingShot() {
    const model = videoModels.find((item) => item.model === generationModel)
    if (!model || !generationPrompt.trim()) return
    const gap = videoTrackGap(timelineRef.current, playhead)
    const size = model.size_rules[0]?.size ?? "1280x720"
    const nodeId = editorId("video")
    const graph: WorkflowGraph = {
      version: 1,
      nodes: [
        {
          id: nodeId,
          type: "video_generation",
          x: 120,
          y: 120,
          data: {
            model: model.model,
            prompt: generationPrompt.trim(),
            seconds: generationSeconds,
            size,
          },
        },
      ],
      edges: [],
    }
    setGenerating(true)
    setGenerationStatus("正在创建补片流水线…")
    try {
      const saved = await workflowApi.save(
        null,
        `剪辑补片 · ${generationPrompt.trim().slice(0, 24)}`,
        graph
      )
      const started = await workflowApi.start(saved.id)
      for (let attempt = 0; attempt < 360; attempt += 1) {
        const run = await workflowApi.run(started.token)
        const node = run.nodes.find((item) => item.node_id === nodeId)
        setGenerationStatus(
          node?.status === "running" || node?.status === "starting"
            ? "AI 正在生成镜头，可继续编辑其他轨道…"
            : node?.status === "waiting"
              ? "补片已进入队列…"
              : "正在整理生成结果…"
        )
        if (run.status === "completed" && node?.output_paths[0]) {
          const output = node.output_paths[0]
          addAsset(
            {
              id: `workflow:${nodeId}`,
              title: generationPrompt.trim().slice(0, 48),
              kind: "video",
              path: output,
              duration: generationSeconds,
              source: "workflow",
              is_public: false,
              created_at: new Date().toISOString(),
            },
            undefined,
            gap.start
          )
          setPlayhead(gap.start)
          await reloadAssets()
          setGeneratorOpen(false)
          setGenerationPrompt("")
          toast.success("补片已生成并放入时间线")
          return
        }
        if (run.status === "failed" || run.status === "cancelled") {
          throw new Error(run.error || node?.error || "补片流水线执行失败")
        }
        await new Promise((resolve) => window.setTimeout(resolve, 3000))
      }
      throw new Error("补片生成等待超时，可前往流水线查看运行状态")
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error))
    } finally {
      setGenerating(false)
      setGenerationStatus("")
    }
  }

  async function startExport() {
    setExportSubmitting(true)
    try {
      const savedId = dirty || !projectId ? await saveProject(true) : projectId
      if (!savedId) throw new Error("请先保存项目")
      const result = await videoEditorApi.startExport(savedId)
      const queued: EditorExport = {
        token: result.token,
        project_id: savedId,
        status: "pending",
        progress: 0,
        video_path: null,
        error: null,
        created_at: new Date().toISOString(),
        started_at: null,
        finished_at: null,
      }
      exportStatusesRef.current.set(result.token, "pending")
      setExports((items) => [queued, ...items])
      setExportQueueOpen(true)
      toast.success("已加入导出队列，可继续编辑或再次导出")
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error))
    } finally {
      setExportSubmitting(false)
    }
  }

  async function removeExport(item: EditorExport) {
    const ok = await confirm({
      title: "删除这条导出记录？",
      description:
        item.status === "completed"
          ? "导出记录及对应的 MP4 文件都将被永久删除。"
          : "失败的导出记录将被永久删除。",
      confirmText: "删除",
      destructive: true,
    })
    if (!ok) return

    setDeletingExportToken(item.token)
    try {
      await videoEditorApi.removeExport(item.token)
      exportStatusesRef.current.delete(item.token)
      setExports((items) => items.filter((entry) => entry.token !== item.token))
      toast.success("导出记录已删除")
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error))
    } finally {
      setDeletingExportToken(null)
    }
  }

  const activeExportKey = exports
    .filter((item) => item.status === "pending" || item.status === "running")
    .map((item) => item.token)
    .join(",")

  useEffect(() => {
    if (!activeExportKey) return
    const tokens = activeExportKey.split(",")
    let cancelled = false
    const poll = async () => {
      const results = await Promise.allSettled(
        tokens.map((token) => videoEditorApi.export(token))
      )
      if (cancelled) return

      const refreshed = new Map<string, EditorExport>()
      let firstError: unknown = null
      for (const result of results) {
        if (result.status === "rejected") {
          firstError ??= result.reason
          continue
        }
        const item = result.value
        refreshed.set(item.token, item)
        const previous = exportStatusesRef.current.get(item.token)
        exportStatusesRef.current.set(item.token, item.status)
        if (previous && previous !== item.status) {
          if (item.status === "completed") {
            toast.success("视频导出完成，可在导出队列下载")
          } else if (item.status === "failed") {
            toast.error(item.error || "视频导出失败")
          }
        }
      }
      setExports((items) =>
        items.map((item) => refreshed.get(item.token) ?? item)
      )

      if (!firstError) {
        exportPollErrorShownRef.current = false
      } else if (!exportPollErrorShownRef.current) {
        exportPollErrorShownRef.current = true
        toast.error(
          `导出队列暂时无法刷新，将自动重试：${
            firstError instanceof Error ? firstError.message : String(firstError)
          }`
        )
      }
    }
    void poll()
    const timer = window.setInterval(() => void poll(), 2500)
    return () => {
      cancelled = true
      window.clearInterval(timer)
    }
  }, [activeExportKey])

  const shownAssets = useMemo(() => {
    const source = assetScope === "mine" ? mineAssets : publicAssets
    const query = assetQuery.trim().toLocaleLowerCase()
    return source.filter(
      (asset) =>
        (assetKind === "all" || asset.kind === assetKind) &&
        (!query ||
          asset.title.toLocaleLowerCase().includes(query) ||
          asset.author?.toLocaleLowerCase().includes(query))
    )
  }, [assetKind, assetQuery, assetScope, mineAssets, publicAssets])

  const currentModel = videoModels.find((item) => item.model === generationModel)
  const generationCost = currentModel
    ? computeVideoCost(
        currentModel,
        generationSeconds,
        currentModel.size_rules[0]?.size ?? "1280x720"
      )
    : null
  const activeExportCount = exports.filter(
    (item) => item.status === "pending" || item.status === "running"
  ).length

  function returnToPreviousPage() {
    const historyIndex = window.history.state?.idx
    if (typeof historyIndex === "number" && historyIndex > 0) {
      navigate(-1)
      return
    }
    navigate("/videos", { replace: true })
  }

  if (loading) {
    return (
      <div className="grid min-h-svh place-items-center bg-background">
        <div className="flex items-center gap-2 text-sm text-muted-foreground">
          <Loader2 className="size-4 animate-spin" /> 正在载入剪辑台…
        </div>
      </div>
    )
  }

  return (
    <div className="flex h-svh min-h-0 flex-col overflow-hidden bg-[#090a0d] text-zinc-100">
      <header className="flex h-14 shrink-0 items-center gap-2 border-b border-white/10 bg-[#111217] px-2.5 md:px-4">
        <Button
          size="icon-sm"
          variant="ghost"
          className="text-zinc-300 hover:bg-white/10 hover:text-white"
          onClick={returnToPreviousPage}
          aria-label="返回进入剪辑页前的页面"
          title="返回"
        >
          <ArrowLeft />
        </Button>
        <div className="hidden items-center gap-2 sm:flex">
          <span className="grid size-8 place-items-center rounded-lg bg-sky-500/15 text-sky-300">
            <Clapperboard className="size-4" />
          </span>
          <div>
            <p className="text-xs font-semibold leading-none">NovaCut</p>
            <p className="mt-1 text-[9px] tracking-[0.14em] text-zinc-500">PRO EDITOR</p>
          </div>
        </div>
        <div className="mx-1 h-5 w-px bg-white/10" />
        <Select
          value={projectId ? String(projectId) : "new"}
          onValueChange={(value) => {
            if (value === "new") newProject()
            else {
              const project = projects.find((item) => item.id === Number(value))
              if (project) loadProjectState(project)
            }
          }}
        >
          <SelectTrigger size="sm" className="w-36 border-white/10 bg-white/5 text-xs text-zinc-200 md:w-48">
            <FolderOpen className="size-3.5" /><SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="new">新建剪辑项目</SelectItem>
            {projects.map((project) => (
              <SelectItem key={project.id} value={String(project.id)}>{project.name}</SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Input
          value={projectName}
          onChange={(event) => {
            setProjectName(event.target.value)
            editRevision.current += 1
            setDirty(true)
          }}
          className="h-8 min-w-0 max-w-52 border-transparent bg-transparent px-2 text-xs text-zinc-200 shadow-none hover:bg-white/5 focus-visible:border-white/10 focus-visible:ring-0"
          aria-label="项目名称"
        />
        <span className="hidden items-center gap-1 text-[10px] text-zinc-500 lg:flex">
          {saveState === "saving" && <><Loader2 className="size-3 animate-spin" /> 保存中</>}
          {saveState === "saved" && <><Check className="size-3 text-emerald-400" /> 已保存</>}
          {saveState === "error" && "保存失败"}
          {saveState === "idle" && dirty && "有未保存更改"}
        </span>
        <div className="ml-auto flex items-center gap-1">
          <Button size="icon-sm" variant="ghost" className="text-zinc-400 hover:bg-white/10" onClick={undo} disabled={!undoStack.length} title="撤销 Ctrl/⌘ Z"><Undo2 /></Button>
          <Button size="icon-sm" variant="ghost" className="text-zinc-400 hover:bg-white/10" onClick={redo} disabled={!redoStack.length} title="重做 Ctrl/⌘ Shift Z"><Redo2 /></Button>
          <Button size="icon-sm" variant="ghost" className="text-zinc-400 hover:bg-white/10 md:hidden" onClick={() => setMobilePanel(mobilePanel === "assets" ? null : "assets")} title="素材库"><Library /></Button>
          <Button size="icon-sm" variant="ghost" className="text-zinc-400 hover:bg-white/10 xl:hidden" onClick={() => setMobilePanel(mobilePanel === "inspector" ? null : "inspector")} title="属性"><Settings2 /></Button>
          <Button size="sm" variant="ghost" className="hidden text-zinc-300 hover:bg-white/10 sm:flex" onClick={() => void saveProject()}><Save /> 保存</Button>
          {projectId && <Button size="icon-sm" variant="ghost" className="hidden text-zinc-500 hover:bg-red-500/10 hover:text-red-300 lg:inline-flex" onClick={() => void deleteCurrentProject()} title="删除项目"><Trash2 /></Button>}
          <Button size="sm" variant="ghost" className="relative text-zinc-300 hover:bg-white/10" onClick={() => setExportQueueOpen(true)} title="打开导出队列">
            <Layers3 />
            <span className="hidden md:inline">导出队列</span>
            {activeExportCount > 0 && <span className="min-w-4 rounded-full bg-sky-500 px-1 text-center text-[9px] leading-4 text-white">{activeExportCount}</span>}
          </Button>
          <Button size="sm" className="bg-sky-500 text-white hover:bg-sky-400" disabled={exportSubmitting || duration <= 0} onClick={() => void startExport()}>
            {exportSubmitting ? <Loader2 className="animate-spin" /> : <Download />}
            <span className="hidden sm:inline">{exportSubmitting ? "提交中" : "导出"}</span>
          </Button>
        </div>
      </header>

      <div className="relative flex min-h-0 flex-1">
        <AssetPanel
          className={cn(
            "absolute inset-y-0 left-0 z-40 w-80 shadow-2xl md:static md:z-auto md:block md:w-72 md:shadow-none",
            mobilePanel === "assets" ? "block" : "hidden"
          )}
          scope={assetScope}
          onScope={setAssetScope}
          query={assetQuery}
          onQuery={setAssetQuery}
          kind={assetKind}
          onKind={setAssetKind}
          assets={shownAssets}
          loading={assetsLoading}
          uploading={uploading}
          uploadRef={uploadRef}
          onUpload={(files) => void uploadFiles(files)}
          onAdd={addAsset}
          onImport={(asset) => void importPublicAsset(asset)}
          onTogglePublic={(asset) => void toggleAssetPublic(asset)}
          onRemove={(asset) => void removeLibraryAsset(asset)}
          onGenerate={() => setGeneratorOpen(true)}
          onClose={() => setMobilePanel(null)}
        />

        <main className="flex min-w-0 flex-1 flex-col">
          <div className="flex min-h-0 flex-1 flex-col bg-[#090a0d]">
            <div className="flex h-10 shrink-0 items-center justify-between border-b border-white/10 px-3 text-[11px] text-zinc-400">
              <span className="flex items-center gap-2"><Film className="size-3.5" /> 节目监视器</span>
              <span>{timeline.width} × {timeline.height} · {timeline.fps} FPS</span>
            </div>
            <div className="relative flex min-h-0 flex-1 items-center justify-center overflow-hidden bg-[radial-gradient(circle_at_center,#1b1d24_0,#090a0d_65%)] p-4">
              <PreviewStage timeline={timeline} time={playhead} playing={playing} />
            </div>
            <div className="flex h-12 shrink-0 items-center justify-center gap-2 border-t border-white/10 bg-[#111217] px-3">
              <span className="w-24 text-right font-mono text-[11px] text-zinc-400">{formatTime(playhead, timeline.fps)}</span>
              <Button size="icon-sm" variant="ghost" className="text-zinc-200 hover:bg-white/10" onClick={() => setPlaying((value) => !value)}>
                {playing ? <Pause className="fill-current" /> : <Play className="fill-current" />}
              </Button>
              <span className="w-24 font-mono text-[11px] text-zinc-500">{formatTime(duration, timeline.fps)}</span>
            </div>
          </div>

          <TimelinePanel
            timeline={timeline}
            selectedClipId={selectedClipId}
            selectedTrackId={selectedTrackId}
            playhead={playhead}
            zoom={zoom}
            snap={snap}
            assets={[...mineAssets, ...publicAssets]}
            onTime={(time) => {
              setPlaying(false)
              setPlayhead(time)
            }}
            onSelectClip={setSelectedClipId}
            onSelectTrack={setSelectedTrackId}
            onZoom={setZoom}
            onSnap={setSnap}
            onBeginEdit={beginContinuousEdit}
            onPreview={previewTimeline}
            onEndEdit={endContinuousEdit}
            onSplit={splitSelectedClip}
            onDelete={removeSelectedClip}
            onDuplicate={duplicateSelectedClip}
            onAddAsset={addAsset}
            onAddText={addTextClip}
            onAddTrack={addTrack}
            onUpdateTrack={updateTrack}
            onReorderTrack={reorderTrack}
          />
        </main>

        <InspectorPanel
          className={cn(
            "absolute inset-y-0 right-0 z-40 w-80 shadow-2xl xl:static xl:z-auto xl:block xl:w-72 xl:shadow-none",
            mobilePanel === "inspector" ? "block" : "hidden"
          )}
          timeline={timeline}
          selected={selected}
          onProject={(patch) => commitTimeline((next) => Object.assign(next, patch))}
          onClip={changeSelectedClip}
          onClose={() => setMobilePanel(null)}
        />
      </div>

      {mobilePanel && (
        <button type="button" className="absolute inset-0 z-30 bg-black/55 md:hidden" onClick={() => setMobilePanel(null)} aria-label="关闭面板" />
      )}

      {generatorOpen && (
        <GenerationDialog
          models={videoModels}
          model={generationModel}
          seconds={generationSeconds}
          prompt={generationPrompt}
          cost={generationCost}
          generating={generating}
          status={generationStatus}
          gap={videoTrackGap(timeline, playhead)}
          onModel={(value) => {
            setGenerationModel(value)
            const next = videoModels.find((item) => item.model === value)
            setGenerationSeconds(next?.allowed_seconds[0] ?? 5)
          }}
          onSeconds={setGenerationSeconds}
          onPrompt={setGenerationPrompt}
          onClose={() => !generating && setGeneratorOpen(false)}
          onGenerate={() => void generateMissingShot()}
        />
      )}

      <ExportQueueSheet
        open={exportQueueOpen}
        onOpenChange={setExportQueueOpen}
        exports={exports}
        projects={projects}
        deletingToken={deletingExportToken}
        onDelete={(item) => void removeExport(item)}
      />
    </div>
  )
}

function AssetPanel({
  className,
  scope,
  onScope,
  query,
  onQuery,
  kind,
  onKind,
  assets,
  loading,
  uploading,
  uploadRef,
  onUpload,
  onAdd,
  onImport,
  onTogglePublic,
  onRemove,
  onGenerate,
  onClose,
}: {
  className?: string
  scope: "mine" | "public"
  onScope: (value: "mine" | "public") => void
  query: string
  onQuery: (value: string) => void
  kind: "all" | EditorAsset["kind"]
  onKind: (value: "all" | EditorAsset["kind"]) => void
  assets: EditorAsset[]
  loading: boolean
  uploading: boolean
  uploadRef: { current: HTMLInputElement | null }
  onUpload: (files: FileList) => void
  onAdd: (asset: EditorAsset) => void
  onImport: (asset: EditorAsset) => void
  onTogglePublic: (asset: EditorAsset) => void
  onRemove: (asset: EditorAsset) => void
  onGenerate: () => void
  onClose: () => void
}) {
  return (
    <aside className={cn("flex min-h-0 shrink-0 flex-col border-r border-white/10 bg-[#111217]", className)}>
      <div className="flex h-10 shrink-0 items-center justify-between border-b border-white/10 px-3">
        <span className="flex items-center gap-2 text-xs font-medium text-zinc-300"><Library className="size-3.5" /> 素材库</span>
        <Button size="icon-xs" variant="ghost" className="text-zinc-500 hover:bg-white/10 md:hidden" onClick={onClose}><X /></Button>
      </div>
      <div className="space-y-2 border-b border-white/10 p-3">
        <div className="grid grid-cols-2 rounded-lg bg-black/25 p-0.5">
          <button type="button" className={cn("rounded-md px-2 py-1.5 text-[11px] transition-colors", scope === "mine" ? "bg-white/10 text-white" : "text-zinc-500 hover:text-zinc-300")} onClick={() => onScope("mine")}>我的素材</button>
          <button type="button" className={cn("rounded-md px-2 py-1.5 text-[11px] transition-colors", scope === "public" ? "bg-white/10 text-white" : "text-zinc-500 hover:text-zinc-300")} onClick={() => onScope("public")}>公有素材</button>
        </div>
        <div className="relative">
          <Search className="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-zinc-600" />
          <Input value={query} onChange={(event) => onQuery(event.target.value)} placeholder="搜索素材…" className="h-8 rounded-lg border-white/10 bg-black/20 pl-8 text-xs text-zinc-200 placeholder:text-zinc-600" />
        </div>
        <div className="flex gap-1">
          {(["all", "video", "image", "audio"] as const).map((value) => (
            <button key={value} type="button" onClick={() => onKind(value)} className={cn("rounded-md px-2 py-1 text-[10px] transition-colors", kind === value ? "bg-sky-500/15 text-sky-300" : "text-zinc-500 hover:bg-white/5 hover:text-zinc-300")}>
              {value === "all" ? "全部" : value === "video" ? "视频" : value === "image" ? "图片" : "音频"}
            </button>
          ))}
        </div>
        <div className="grid grid-cols-2 gap-2">
          <input ref={uploadRef} type="file" accept="video/*,image/*,audio/*" multiple hidden onChange={(event) => event.target.files && onUpload(event.target.files)} />
          <Button size="xs" variant="outline" className="border-white/10 bg-white/5 text-zinc-300 hover:bg-white/10" disabled={uploading} onClick={() => uploadRef.current?.click()}>
            {uploading ? <Loader2 className="animate-spin" /> : <Upload />} 上传素材
          </Button>
          <Button size="xs" className="bg-violet-500/90 text-white hover:bg-violet-400" onClick={onGenerate}><WandSparkles /> 流水线补片</Button>
        </div>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto p-2.5">
        {loading && <div className="flex justify-center py-8 text-zinc-600"><Loader2 className="size-4 animate-spin" /></div>}
        {!loading && assets.length === 0 && (
          <div className="flex flex-col items-center px-4 py-12 text-center text-zinc-600">
            <Layers3 className="mb-3 size-8 opacity-60" />
            <p className="text-xs">没有匹配的素材</p>
            <p className="mt-1 text-[10px] leading-relaxed">上传文件，或用流水线生成缺少的镜头</p>
          </div>
        )}
        <div className="grid grid-cols-2 gap-2">
          {assets.map((asset) => (
            <div
              key={asset.id}
              draggable
              onDragStart={(event) => {
                event.dataTransfer.setData("application/x-yunova-asset", asset.id)
                event.dataTransfer.effectAllowed = "copy"
              }}
              className="group overflow-hidden rounded-lg border border-white/10 bg-black/20 transition-colors hover:border-sky-400/40"
            >
              <button type="button" className="relative block aspect-video w-full overflow-hidden bg-black/40" onDoubleClick={() => onAdd(asset)} title="双击放入播放头位置，也可拖入时间线">
                {asset.kind === "image" ? (
                  <img src={asset.thumbnail_path ?? asset.path} alt="" loading="lazy" className="size-full object-cover" />
                ) : asset.kind === "video" ? (
                  asset.thumbnail_path ? <img src={asset.thumbnail_path} alt="" loading="lazy" className="size-full object-cover" /> : <video src={asset.path} preload="metadata" muted className="size-full object-cover" />
                ) : (
                  <span className="grid size-full place-items-center bg-gradient-to-br from-fuchsia-500/10 to-sky-500/10"><Music2 className="size-7 text-fuchsia-300/70" /></span>
                )}
                <span className="absolute bottom-1 left-1 rounded bg-black/75 px-1 py-0.5 text-[8px] uppercase tracking-wide text-zinc-300">{asset.kind === "image" ? "IMG" : asset.kind === "video" ? "VIDEO" : "AUDIO"}</span>
                {asset.duration != null && <span className="absolute bottom-1 right-1 rounded bg-black/75 px-1 py-0.5 font-mono text-[8px] text-zinc-300">{asset.duration.toFixed(1)}s</span>}
                <span className="absolute inset-0 grid place-items-center bg-black/45 opacity-0 transition-opacity group-hover:opacity-100"><Plus className="size-5 text-white" /></span>
              </button>
              <div className="p-1.5">
                <p className="truncate text-[10px] font-medium text-zinc-300" title={asset.title}>{asset.title}</p>
                <div className="mt-1 flex items-center justify-between gap-1 text-[8px] text-zinc-600">
                  <span className="truncate">{asset.author ? `@${asset.author}` : sourceLabel(asset.source)}</span>
                  <span className="flex shrink-0 items-center gap-0.5">
                    {scope === "public" ? (
                      <button type="button" className="rounded p-0.5 hover:bg-white/10 hover:text-sky-300" onClick={() => onImport(asset)} title="收藏到我的素材"><Library className="size-3" /></button>
                    ) : (
                      <>
                        <button type="button" className={cn("rounded p-0.5 hover:bg-white/10", asset.is_public ? "text-emerald-400" : "hover:text-emerald-300")} onClick={() => onTogglePublic(asset)} title={asset.is_public ? "设为私有" : "发布到公有素材库"}>{asset.is_public ? <Eye className="size-3" /> : <EyeOff className="size-3" />}</button>
                        {asset.library_id && <button type="button" className="rounded p-0.5 hover:bg-red-500/10 hover:text-red-300" onClick={() => onRemove(asset)} title="从素材库移除"><Trash2 className="size-3" /></button>}
                      </>
                    )}
                    <button type="button" className="rounded p-0.5 hover:bg-white/10 hover:text-white" onClick={() => onAdd(asset)} title="添加到时间线"><Plus className="size-3" /></button>
                  </span>
                </div>
              </div>
            </div>
          ))}
        </div>
      </div>
      <div className="border-t border-white/10 px-3 py-2 text-[9px] text-zinc-600">双击素材添加 · 拖放到指定轨道</div>
    </aside>
  )
}

function PreviewStage({
  timeline,
  time,
  playing,
}: {
  timeline: EditorTimeline
  time: number
  playing: boolean
}) {
  const mediaRefs = useRef(new Map<string, HTMLMediaElement>())
  const visuals = timeline.tracks.flatMap((track, trackIndex) =>
    track.kind === "video" && !track.hidden
      ? track.clips
          .filter((clip) => time >= clip.start && time < clip.start + clip.duration)
          .map((clip) => ({ track, trackIndex, clip }))
      : []
  )
  const texts = timeline.tracks.flatMap((track, trackIndex) =>
    track.kind === "text" && !track.hidden
      ? track.clips
          .filter((clip) => time >= clip.start && time < clip.start + clip.duration)
          .map((clip) => ({ trackIndex, clip }))
      : []
  )
  const audio = timeline.tracks.flatMap((track) =>
    track.kind === "audio" && !track.muted
      ? track.clips.filter(
          (clip) => time >= clip.start && time < clip.start + clip.duration
        )
      : []
  )

  useEffect(() => {
    for (const [id, media] of mediaRefs.current) {
      const found = [...visuals.map((item) => item.clip), ...audio].find((clip) => clip.id === id)
      if (!found) {
        media.pause()
        continue
      }
      const desired = found.source_in + (time - found.start) * found.speed
      if (Number.isFinite(media.duration) && Math.abs(media.currentTime - desired) > (playing ? 0.22 : 0.035)) {
        media.currentTime = Math.min(Math.max(0, desired), Math.max(0, media.duration - 0.01))
      }
      media.playbackRate = found.speed
      media.volume = Math.min(1, found.volume)
      if (playing) void media.play().catch(() => {})
      else media.pause()
    }
  }, [audio, playing, time, visuals])

  return (
    <div
      className="relative max-h-full max-w-full overflow-hidden bg-black shadow-[0_18px_80px_rgba(0,0,0,0.65)] ring-1 ring-white/10"
      style={{
        aspectRatio: `${timeline.width}/${timeline.height}`,
        width: `min(100%, calc((100vh - 390px) * ${timeline.width / timeline.height}))`,
        backgroundColor: timeline.background,
      }}
    >
      {visuals.length === 0 && texts.length === 0 && (
        <div className="absolute inset-0 grid place-items-center text-center text-zinc-700">
          <div><Film className="mx-auto mb-3 size-10 opacity-60" /><p className="text-xs">将素材放入时间线开始剪辑</p></div>
        </div>
      )}
      {[...visuals].reverse().map(({ clip, track, trackIndex }) => {
        const style = {
          zIndex: 100 - trackIndex,
          opacity: clip.opacity,
          transform: `translate(calc(-50% + ${clip.transform.x / timeline.width * 100}%), calc(-50% + ${clip.transform.y / timeline.height * 100}%)) scale(${clip.transform.scale}) rotate(${clip.transform.rotation}deg)`,
        }
        if (clip.asset_kind === "image") {
          return <img key={clip.id} src={clip.asset_path} alt="" className="absolute left-1/2 top-1/2 max-h-full max-w-full object-contain" style={style} />
        }
        return (
          <video
            key={clip.id}
            ref={(node) => {
              if (node) mediaRefs.current.set(clip.id, node)
              else mediaRefs.current.delete(clip.id)
            }}
            src={clip.asset_path}
            preload="auto"
            muted={track.muted}
            playsInline
            className="absolute left-1/2 top-1/2 max-h-full max-w-full object-contain"
            style={style}
          />
        )
      })}
      {[...texts].reverse().map(({ clip, trackIndex }) => (
        <div
          key={clip.id}
          className="pointer-events-none absolute left-1/2 top-1/2 max-w-[90%] -translate-x-1/2 -translate-y-1/2 whitespace-pre-wrap text-center font-semibold leading-tight drop-shadow-[0_2px_5px_rgba(0,0,0,.95)]"
          style={{
            zIndex: 200 - trackIndex,
            color: clip.color,
            opacity: clip.opacity,
            fontSize: `${Math.max(10, clip.font_size / timeline.height * 520)}px`,
            transform: `translate(calc(-50% + ${clip.transform.x / timeline.width * 100}%), calc(-50% + ${clip.transform.y / timeline.height * 100}%)) scale(${clip.transform.scale}) rotate(${clip.transform.rotation}deg)`,
          }}
        >
          {clip.text}
        </div>
      ))}
      {audio.map((clip) => (
        <audio
          key={clip.id}
          ref={(node) => {
            if (node) mediaRefs.current.set(clip.id, node)
            else mediaRefs.current.delete(clip.id)
          }}
          src={clip.asset_path}
          preload="auto"
        />
      ))}
    </div>
  )
}

function TimelinePanel({
  timeline,
  selectedClipId,
  selectedTrackId,
  playhead,
  zoom,
  snap,
  assets,
  onTime,
  onSelectClip,
  onSelectTrack,
  onZoom,
  onSnap,
  onBeginEdit,
  onPreview,
  onEndEdit,
  onSplit,
  onDelete,
  onDuplicate,
  onAddAsset,
  onAddText,
  onAddTrack,
  onUpdateTrack,
  onReorderTrack,
}: {
  timeline: EditorTimeline
  selectedClipId: string | null
  selectedTrackId: string | null
  playhead: number
  zoom: number
  snap: boolean
  assets: EditorAsset[]
  onTime: (value: number) => void
  onSelectClip: (value: string | null) => void
  onSelectTrack: (value: string) => void
  onZoom: (value: number) => void
  onSnap: (value: boolean) => void
  onBeginEdit: () => void
  onPreview: (updater: (value: EditorTimeline) => EditorTimeline) => void
  onEndEdit: () => void
  onSplit: () => void
  onDelete: () => void
  onDuplicate: () => void
  onAddAsset: (asset: EditorAsset, trackId?: string, at?: number) => void
  onAddText: () => void
  onAddTrack: (kind: EditorTrackKind) => void
  onUpdateTrack: (id: string, patch: Partial<EditorTrack>) => void
  onReorderTrack: (
    sourceId: string,
    targetId: string,
    edge: "before" | "after"
  ) => void
}) {
  const [draggingTrackId, setDraggingTrackId] = useState<string | null>(null)
  const [trackDropTarget, setTrackDropTarget] = useState<{
    id: string
    edge: "before" | "after"
  } | null>(null)
  const duration = Math.max(30, Math.ceil(timelineDuration(timeline) + 10))
  const contentWidth = Math.max(900, duration * zoom)
  const rulerStep = zoom >= 160 ? 0.5 : zoom >= 80 ? 1 : zoom >= 40 ? 2 : 5
  const rulerMarks = Array.from(
    { length: Math.floor(duration / rulerStep) + 1 },
    (_, index) => index * rulerStep
  )

  function updateClip(trackId: string, clipId: string, patch: Partial<EditorClip>) {
    onPreview((next) => {
      const clip = next.tracks
        .find((track) => track.id === trackId)
        ?.clips.find((item) => item.id === clipId)
      if (clip) Object.assign(clip, patch)
      return next
    })
  }

  function timeFromEvent(event: React.MouseEvent<HTMLElement>) {
    const rect = event.currentTarget.getBoundingClientRect()
    return Math.max(0, Math.min(duration, (event.clientX - rect.left) / zoom))
  }

  function hasTrackDrag(event: React.DragEvent<HTMLElement>) {
    return event.dataTransfer.types.includes(TRACK_DRAG_TYPE)
  }

  function updateTrackDropTarget(
    event: React.DragEvent<HTMLElement>,
    targetId: string
  ) {
    if (!hasTrackDrag(event) || draggingTrackId === targetId) return false
    event.preventDefault()
    event.dataTransfer.dropEffect = "move"
    const rect = event.currentTarget.getBoundingClientRect()
    const edge = event.clientY < rect.top + rect.height / 2 ? "before" : "after"
    if (trackDropTarget?.id !== targetId || trackDropTarget.edge !== edge) {
      setTrackDropTarget({ id: targetId, edge })
    }
    return true
  }

  function dropTrack(event: React.DragEvent<HTMLElement>, targetId: string) {
    if (!hasTrackDrag(event)) return false
    event.preventDefault()
    event.stopPropagation()
    const sourceId = event.dataTransfer.getData(TRACK_DRAG_TYPE)
    const rect = event.currentTarget.getBoundingClientRect()
    const edge = event.clientY < rect.top + rect.height / 2 ? "before" : "after"
    setDraggingTrackId(null)
    setTrackDropTarget(null)
    if (sourceId && sourceId !== targetId) onReorderTrack(sourceId, targetId, edge)
    return true
  }

  function trackDropIndicator(trackId: string) {
    if (trackDropTarget?.id !== trackId) return null
    return (
      <span
        className={cn(
          "pointer-events-none absolute inset-x-0 z-20 h-0.5 bg-sky-400 shadow-[0_0_6px_rgba(56,189,248,.9)]",
          trackDropTarget.edge === "before" ? "top-0" : "bottom-0"
        )}
      />
    )
  }

  return (
    <section className="flex h-[18rem] shrink-0 flex-col border-t border-white/10 bg-[#111217]">
      <div className="flex h-10 shrink-0 items-center gap-1 border-b border-white/10 px-2.5">
        <Button size="icon-xs" variant="ghost" className="text-zinc-400 hover:bg-white/10" onClick={onSplit} disabled={!selectedClipId} title="在播放头切割 S"><Scissors /></Button>
        <Button size="icon-xs" variant="ghost" className="text-zinc-400 hover:bg-white/10" onClick={onDuplicate} disabled={!selectedClipId} title="复制片段"><Copy /></Button>
        <Button size="icon-xs" variant="ghost" className="text-zinc-400 hover:bg-red-500/10 hover:text-red-300" onClick={onDelete} disabled={!selectedClipId} title="删除片段"><Trash2 /></Button>
        <div className="mx-1 h-4 w-px bg-white/10" />
        <Button size="xs" variant="ghost" className="text-zinc-400 hover:bg-white/10" onClick={onAddText}><Captions /> 文字</Button>
        <div className="group relative">
          <Button size="xs" variant="ghost" className="text-zinc-400 hover:bg-white/10"><Plus /> 轨道</Button>
          <div className="invisible absolute bottom-full left-0 z-50 mb-1 w-32 rounded-lg border border-white/10 bg-[#1a1b21] p-1 opacity-0 shadow-xl transition-all group-hover:visible group-hover:opacity-100">
            {(["video", "audio", "text"] as const).map((kind) => {
              const Icon = trackIcon(kind)
              return <button key={kind} type="button" className="flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-[10px] text-zinc-400 hover:bg-white/10 hover:text-white" onClick={() => onAddTrack(kind)}><Icon className="size-3" /> {kind === "video" ? "视频轨道" : kind === "audio" ? "音频轨道" : "文字轨道"}</button>
            })}
          </div>
        </div>
        <button type="button" className={cn("ml-1 flex items-center gap-1 rounded px-1.5 py-1 text-[10px]", snap ? "bg-sky-500/15 text-sky-300" : "text-zinc-500 hover:bg-white/5")} onClick={() => onSnap(!snap)} title="按帧吸附；拖动时按 Shift 临时关闭">
          <span className="text-xs">⌁</span> 吸附
        </button>
        <span className="ml-auto hidden font-mono text-[10px] text-zinc-600 sm:inline">{formatTime(playhead, timeline.fps)}</span>
        <ZoomOut className="ml-2 size-3 text-zinc-600" />
        <input type="range" min={MIN_ZOOM} max={MAX_ZOOM} value={zoom} onChange={(event) => onZoom(Number(event.target.value))} className="h-1 w-20 accent-sky-500" aria-label="时间线缩放" />
        <ZoomIn className="size-3 text-zinc-600" />
      </div>

      <div className="flex min-h-0 flex-1 overflow-y-auto">
        <div className="sticky left-0 z-20 w-32 shrink-0 border-r border-white/10 bg-[#14151a]">
          <div className="h-7 border-b border-white/10 px-2 text-[9px] leading-7 text-zinc-600">轨道控制</div>
          {timeline.tracks.map((track) => {
            const Icon = trackIcon(track.kind)
            return (
              <button
                key={track.id}
                type="button"
                draggable
                className={cn(
                  "relative flex h-12 w-full cursor-grab items-center gap-1 border-b border-white/5 px-1.5 text-left active:cursor-grabbing",
                  selectedTrackId === track.id ? "bg-sky-500/10" : "hover:bg-white/[.03]",
                  draggingTrackId === track.id && "opacity-45"
                )}
                onClick={() => onSelectTrack(track.id)}
                onDragStart={(event) => {
                  const target = event.target as HTMLElement
                  if (target.closest("[data-track-action]")) {
                    event.preventDefault()
                    return
                  }
                  event.dataTransfer.setData(TRACK_DRAG_TYPE, track.id)
                  event.dataTransfer.effectAllowed = "move"
                  setDraggingTrackId(track.id)
                  setTrackDropTarget(null)
                  onSelectTrack(track.id)
                }}
                onDragOver={(event) => updateTrackDropTarget(event, track.id)}
                onDrop={(event) => dropTrack(event, track.id)}
                onDragEnd={() => {
                  setDraggingTrackId(null)
                  setTrackDropTarget(null)
                }}
                title="拖动调整轨道顺序"
              >
                {trackDropIndicator(track.id)}
                <GripVertical className="size-3 shrink-0 text-zinc-700" />
                <Icon className={cn("size-3.5", track.kind === "video" ? "text-sky-400" : track.kind === "audio" ? "text-fuchsia-400" : "text-amber-400")} />
                <span className="min-w-0 flex-1 truncate text-[10px] font-medium text-zinc-400">{track.name}</span>
                {track.kind !== "audio" && <span role="button" tabIndex={0} data-track-action className={cn("rounded p-1", track.hidden ? "text-red-400" : "text-zinc-600 hover:text-zinc-300")} onClick={(event) => { event.stopPropagation(); onUpdateTrack(track.id, { hidden: !track.hidden }) }} onKeyDown={() => {}}>{track.hidden ? <EyeOff className="size-3" /> : <Eye className="size-3" />}</span>}
                {track.kind !== "text" && <span role="button" tabIndex={0} data-track-action className={cn("rounded p-1", track.muted ? "text-red-400" : "text-zinc-600 hover:text-zinc-300")} onClick={(event) => { event.stopPropagation(); onUpdateTrack(track.id, { muted: !track.muted }) }} onKeyDown={() => {}}>{track.muted ? <VolumeX className="size-3" /> : <Volume2 className="size-3" />}</span>}
                <span role="button" tabIndex={0} data-track-action className={cn("rounded p-1", track.locked ? "text-amber-400" : "text-zinc-600 hover:text-zinc-300")} onClick={(event) => { event.stopPropagation(); onUpdateTrack(track.id, { locked: !track.locked }) }} onKeyDown={() => {}}>{track.locked ? <Lock className="size-3" /> : <Unlock className="size-3" />}</span>
              </button>
            )
          })}
        </div>

        <div className="min-w-0 flex-1 overflow-x-auto">
          <div className="relative" style={{ width: contentWidth }}>
            <div className="relative h-7 cursor-crosshair border-b border-white/10 bg-[#14151a]" onPointerDown={(event) => onTime(timeFromEvent(event))}>
              {rulerMarks.map((mark) => (
                <div key={mark} className="absolute inset-y-0 border-l border-white/15" style={{ left: mark * zoom }}>
                  <span className="ml-1 font-mono text-[8px] text-zinc-600">{mark >= 60 ? `${Math.floor(mark / 60)}:${String(Math.floor(mark % 60)).padStart(2, "0")}` : `${mark.toFixed(mark % 1 ? 1 : 0)}s`}</span>
                </div>
              ))}
            </div>
            {timeline.tracks.map((track) => (
              <div
                key={track.id}
                className={cn("relative h-12 border-b border-white/5 bg-[linear-gradient(90deg,rgba(255,255,255,.025)_1px,transparent_1px)]", selectedTrackId === track.id && "bg-sky-500/[.035]", track.locked && "opacity-65")}
                style={{ backgroundSize: `${zoom}px 100%` }}
                onPointerDown={(event) => {
                  if (event.target === event.currentTarget) {
                    onSelectClip(null)
                    onSelectTrack(track.id)
                    onTime(timeFromEvent(event))
                  }
                }}
                onDragOver={(event) => {
                  if (updateTrackDropTarget(event, track.id)) return
                  event.preventDefault()
                  event.dataTransfer.dropEffect = "copy"
                }}
                onDrop={(event) => {
                  if (dropTrack(event, track.id)) return
                  event.preventDefault()
                  const assetId = event.dataTransfer.getData("application/x-yunova-asset")
                  const asset = assets.find((item) => item.id === assetId)
                  if (asset && !track.locked) onAddAsset(asset, track.id, timeFromEvent(event))
                }}
              >
                {trackDropIndicator(track.id)}
                {track.clips.map((clip) => (
                  <TimelineClip
                    key={clip.id}
                    track={track}
                    clip={clip}
                    selected={selectedClipId === clip.id}
                    zoom={zoom}
                    fps={timeline.fps}
                    snap={snap}
                    onSelect={() => {
                      onSelectClip(clip.id)
                      onSelectTrack(track.id)
                    }}
                    onBegin={onBeginEdit}
                    onChange={(patch) => updateClip(track.id, clip.id, patch)}
                    onEnd={onEndEdit}
                  />
                ))}
              </div>
            ))}
            <div className="pointer-events-none absolute bottom-0 top-0 z-10 w-px bg-red-500" style={{ left: playhead * zoom }}>
              <span className="absolute -left-[4px] top-0 size-2 rotate-45 bg-red-500" />
            </div>
          </div>
        </div>
      </div>
    </section>
  )
}

function TimelineClip({
  track,
  clip,
  selected,
  zoom,
  fps,
  snap,
  onSelect,
  onBegin,
  onChange,
  onEnd,
}: {
  track: EditorTrack
  clip: EditorClip
  selected: boolean
  zoom: number
  fps: number
  snap: boolean
  onSelect: () => void
  onBegin: () => void
  onChange: (patch: Partial<EditorClip>) => void
  onEnd: () => void
}) {
  const drag = useRef<{
    mode: "move" | "trim-start" | "trim-end"
    x: number
    original: EditorClip
  } | null>(null)

  function startDrag(
    event: ReactPointerEvent<HTMLDivElement>,
    mode: "move" | "trim-start" | "trim-end"
  ) {
    if (track.locked || event.button !== 0) return
    event.stopPropagation()
    event.currentTarget.setPointerCapture(event.pointerId)
    drag.current = { mode, x: event.clientX, original: structuredClone(clip) }
    onSelect()
    onBegin()
  }

  function snapped(value: number, shift: boolean) {
    if (!snap || shift) return Math.max(0, value)
    const frame = 1 / fps
    return Math.max(0, Math.round(value / frame) * frame)
  }

  function move(event: ReactPointerEvent<HTMLDivElement>) {
    if (!drag.current) return
    const delta = (event.clientX - drag.current.x) / zoom
    const original = drag.current.original
    if (drag.current.mode === "move") {
      onChange({ start: snapped(original.start + delta, event.shiftKey) })
    } else if (drag.current.mode === "trim-start") {
      const nextStart = Math.min(
        original.start + original.duration - 0.05,
        snapped(original.start + delta, event.shiftKey)
      )
      const trimmed = nextStart - original.start
      onChange({
        start: nextStart,
        duration: Math.max(0.05, original.duration - trimmed),
        source_in: Math.max(0, original.source_in + trimmed * original.speed),
      })
    } else {
      onChange({
        duration: Math.max(
          0.05,
          snapped(original.duration + delta, event.shiftKey)
        ),
      })
    }
  }

  function end(event: ReactPointerEvent<HTMLDivElement>) {
    if (!drag.current) return
    event.currentTarget.releasePointerCapture(event.pointerId)
    drag.current = null
    onEnd()
  }

  const color = track.kind === "video"
    ? "border-sky-300/35 bg-sky-500/50"
    : track.kind === "audio"
      ? "border-fuchsia-300/35 bg-fuchsia-500/45"
      : "border-amber-300/35 bg-amber-500/45"
  return (
    <div
      className={cn("absolute bottom-1 top-1 overflow-hidden rounded border text-white shadow-sm", color, selected && "z-[2] ring-2 ring-white ring-offset-1 ring-offset-[#111217]")}
      style={{ left: clip.start * zoom, width: Math.max(8, clip.duration * zoom) }}
      onPointerDown={(event) => startDrag(event, "move")}
      onPointerMove={move}
      onPointerUp={end}
      onPointerCancel={end}
      title={`${clip.name}\n${clip.start.toFixed(3)}s · ${clip.duration.toFixed(3)}s`}
    >
      <div className="pointer-events-none flex h-full min-w-0 items-center gap-1 px-2">
        {track.kind === "video" ? clip.asset_kind === "image" ? <ImageIcon className="size-3 shrink-0" /> : <Film className="size-3 shrink-0" /> : track.kind === "audio" ? <Music2 className="size-3 shrink-0" /> : <Captions className="size-3 shrink-0" />}
        <span className="truncate text-[9px] font-medium drop-shadow-sm">{clip.name}</span>
      </div>
      {selected && !track.locked && (
        <>
          <div className="absolute inset-y-0 left-0 w-2 cursor-ew-resize bg-white/40" onPointerDown={(event) => startDrag(event, "trim-start")} />
          <div className="absolute inset-y-0 right-0 w-2 cursor-ew-resize bg-white/40" onPointerDown={(event) => startDrag(event, "trim-end")} />
        </>
      )}
    </div>
  )
}

function InspectorPanel({
  className,
  timeline,
  selected,
  onProject,
  onClip,
  onClose,
}: {
  className?: string
  timeline: EditorTimeline
  selected: { track: EditorTrack; clip: EditorClip } | null
  onProject: (patch: Partial<EditorTimeline>) => void
  onClip: (patch: Partial<EditorClip>) => void
  onClose: () => void
}) {
  const sequenceFormat = sequenceFormatValue(timeline.width, timeline.height)
  const standardSequenceFormat = STANDARD_SEQUENCE_FORMATS.find(
    (format) => sequenceFormatValue(format.width, format.height) === sequenceFormat
  )

  return (
    <aside className={cn("flex min-h-0 shrink-0 flex-col border-l border-white/10 bg-[#111217]", className)}>
      <div className="flex h-10 shrink-0 items-center justify-between border-b border-white/10 px-3">
        <span className="flex items-center gap-2 text-xs font-medium text-zinc-300"><Settings2 className="size-3.5" /> {selected ? "片段属性" : "项目属性"}</span>
        <Button size="icon-xs" variant="ghost" className="text-zinc-500 hover:bg-white/10 xl:hidden" onClick={onClose}><X /></Button>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto p-3">
        {selected ? (
          <div className="space-y-5">
            <InspectorSection title="基本">
              <DarkField label="片段名称"><DarkInput value={selected.clip.name} onChange={(value) => onClip({ name: value })} /></DarkField>
              {selected.track.kind === "text" && <DarkField label="文字内容"><Textarea value={selected.clip.text ?? ""} onChange={(event) => onClip({ text: event.target.value, name: event.target.value.slice(0, 20) || "文字" })} className="min-h-24 rounded-lg border-white/10 bg-black/20 text-xs text-zinc-200" /></DarkField>}
            </InspectorSection>

            <InspectorSection title="精确时间">
              <div className="grid grid-cols-2 gap-2">
                <NumberField label="开始" value={selected.clip.start} step={1 / timeline.fps} min={0} suffix="s" onChange={(start) => onClip({ start })} />
                <NumberField label="时长" value={selected.clip.duration} step={1 / timeline.fps} min={0.05} suffix="s" onChange={(duration) => onClip({ duration })} />
                {selected.track.kind !== "text" && <NumberField label="入点" value={selected.clip.source_in} step={1 / timeline.fps} min={0} suffix="s" onChange={(source_in) => onClip({ source_in })} />}
                {selected.track.kind !== "text" && <NumberField label="速度" value={selected.clip.speed} step={0.05} min={0.25} max={4} suffix="×" onChange={(speed) => onClip({ speed })} />}
              </div>
              <p className="text-[9px] leading-relaxed text-zinc-600">每帧 {(1 / timeline.fps).toFixed(4)} 秒 · 时间线支持按帧输入与吸附</p>
            </InspectorSection>

            {selected.track.kind !== "audio" && (
              <InspectorSection title="运动与画面">
                <div className="grid grid-cols-2 gap-2">
                  <NumberField label="位置 X" value={selected.clip.transform.x} step={1} suffix="px" onChange={(x) => onClip({ transform: { ...selected.clip.transform, x } })} />
                  <NumberField label="位置 Y" value={selected.clip.transform.y} step={1} suffix="px" onChange={(y) => onClip({ transform: { ...selected.clip.transform, y } })} />
                  <NumberField label="缩放" value={selected.clip.transform.scale * 100} step={1} min={5} max={400} suffix="%" onChange={(scale) => onClip({ transform: { ...selected.clip.transform, scale: scale / 100 } })} />
                  <NumberField label="旋转" value={selected.clip.transform.rotation} step={0.1} suffix="°" onChange={(rotation) => onClip({ transform: { ...selected.clip.transform, rotation } })} />
                  <NumberField label="不透明度" value={selected.clip.opacity * 100} step={1} min={0} max={100} suffix="%" onChange={(opacity) => onClip({ opacity: opacity / 100 })} />
                  {selected.track.kind === "text" && <NumberField label="字号" value={selected.clip.font_size} step={1} min={8} max={400} suffix="px" onChange={(font_size) => onClip({ font_size })} />}
                </div>
                {selected.track.kind === "text" && <DarkField label="文字颜色"><input type="color" value={selected.clip.color} onChange={(event) => onClip({ color: event.target.value })} className="h-8 w-full rounded-lg border border-white/10 bg-black/20 p-1" /></DarkField>}
              </InspectorSection>
            )}

            <InspectorSection title="声音与渐变">
              <div className="grid grid-cols-2 gap-2">
                {selected.track.kind !== "text" && <NumberField label="音量" value={selected.clip.volume * 100} step={1} min={0} max={400} suffix="%" onChange={(volume) => onClip({ volume: volume / 100 })} />}
                <NumberField label="淡入" value={selected.clip.fade_in} step={0.1} min={0} max={selected.clip.duration} suffix="s" onChange={(fade_in) => onClip({ fade_in })} />
                <NumberField label="淡出" value={selected.clip.fade_out} step={0.1} min={0} max={selected.clip.duration} suffix="s" onChange={(fade_out) => onClip({ fade_out })} />
              </div>
            </InspectorSection>
          </div>
        ) : (
          <div className="space-y-5">
            <InspectorSection title="序列设置">
              <DarkField label="输出规格">
                <Select
                  value={sequenceFormat}
                  onValueChange={(value) => {
                    const format = STANDARD_SEQUENCE_FORMATS.find(
                      (item) => sequenceFormatValue(item.width, item.height) === value
                    )
                    if (format) onProject({ width: format.width, height: format.height })
                  }}
                >
                  <SelectTrigger size="sm" className="w-full border-white/10 bg-black/20 text-xs text-zinc-200">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent className="border-white/10 bg-[#1b1c22] text-zinc-200">
                    {!standardSequenceFormat && (
                      <>
                        <SelectGroup>
                          <SelectLabel>旧项目规格</SelectLabel>
                          <SelectItem value={sequenceFormat} className="text-xs">
                            当前自定义 · {timeline.width} × {timeline.height}
                          </SelectItem>
                        </SelectGroup>
                        <SelectSeparator className="bg-white/10" />
                      </>
                    )}
                    {SEQUENCE_FORMAT_GROUPS.map((group) => (
                      <SelectGroup key={group.label}>
                        <SelectLabel>{group.label}</SelectLabel>
                        {group.formats.map((format) => (
                          <SelectItem
                            key={sequenceFormatValue(format.width, format.height)}
                            value={sequenceFormatValue(format.width, format.height)}
                            className="text-xs focus:bg-white/10 focus:text-zinc-100"
                          >
                            {format.label} · {format.width} × {format.height}
                          </SelectItem>
                        ))}
                      </SelectGroup>
                    ))}
                  </SelectContent>
                </Select>
              </DarkField>
              <p className="text-[9px] leading-relaxed text-zinc-600">
                导出使用所选标准规格；切换横竖屏会同步调整画布。
              </p>
              <div className="grid grid-cols-2 gap-2">
                <NumberField label="帧率" value={timeline.fps} step={1} min={1} max={120} suffix="fps" onChange={(fps) => onProject({ fps })} />
                <DarkField label="背景颜色"><input type="color" value={timeline.background} onChange={(event) => onProject({ background: event.target.value })} className="h-8 w-full rounded-lg border border-white/10 bg-black/20 p-1" /></DarkField>
              </div>
            </InspectorSection>

            <InspectorSection title="项目统计">
              <div className="grid grid-cols-2 gap-2">
                <Stat label="轨道" value={timeline.tracks.length} />
                <Stat label="片段" value={timeline.tracks.reduce((sum, track) => sum + track.clips.length, 0)} />
                <Stat label="总时长" value={`${timelineDuration(timeline).toFixed(1)}s`} />
                <Stat label="总帧数" value={Math.ceil(timelineDuration(timeline) * timeline.fps)} />
              </div>
            </InspectorSection>
          </div>
        )}
      </div>
    </aside>
  )
}

function ExportQueueSheet({
  open,
  onOpenChange,
  exports,
  projects,
  deletingToken,
  onDelete,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  exports: EditorExport[]
  projects: EditorProject[]
  deletingToken: string | null
  onDelete: (item: EditorExport) => void
}) {
  const activeCount = exports.filter(
    (item) => item.status === "pending" || item.status === "running"
  ).length

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent className="gap-0 border-white/10 bg-[#111217] text-zinc-100 sm:max-w-md">
        <SheetHeader className="border-b border-white/10 pr-12">
          <SheetTitle className="flex items-center gap-2 text-zinc-100">
            <Layers3 className="size-4 text-sky-400" /> 导出队列
          </SheetTitle>
          <SheetDescription className="text-xs text-zinc-500">
            {activeCount > 0
              ? `${activeCount} 个任务正在排队或处理。导出期间可以继续编辑并再次导出。`
              : exports.length > 0
                ? "导出已完成，点击视频卡片即可下载 MP4。"
                : "提交导出后，任务进度和结果会显示在这里。"}
          </SheetDescription>
        </SheetHeader>

        <div className="min-h-0 flex-1 overflow-y-auto p-4">
          {exports.length === 0 ? (
            <div className="grid min-h-52 place-items-center rounded-xl border border-dashed border-white/10 bg-black/10 text-center">
              <div>
                <Film className="mx-auto mb-3 size-8 text-zinc-700" />
                <p className="text-xs text-zinc-500">暂无导出任务</p>
                <p className="mt-1 text-[10px] text-zinc-700">点击编辑器右上角“导出”开始</p>
              </div>
            </div>
          ) : (
            <div className="space-y-3">
              {exports.map((item) => {
                const project = projects.find((entry) => entry.id === item.project_id)
                const projectLabel = project?.name || (item.project_id ? `剪辑项目 #${item.project_id}` : "已删除的剪辑项目")
                const filename = `NovaCut-${item.token.slice(0, 8)}.mp4`

                if (item.status === "completed" && item.video_path) {
                  return (
                    <div
                      key={item.token}
                      className="group block overflow-hidden rounded-xl border border-white/10 bg-black/20 transition-colors hover:border-emerald-400/35 hover:bg-white/[.04]"
                    >
                      <a href={item.video_path} download={filename} title={`下载 ${filename}`}>
                        <div className="relative aspect-video overflow-hidden bg-black">
                          <video
                            src={item.video_path}
                            preload="metadata"
                            muted
                            playsInline
                            className="pointer-events-none size-full object-contain"
                          />
                          <div className="absolute inset-0 grid place-items-center bg-black/15 opacity-0 transition-opacity group-hover:opacity-100">
                            <span className="flex items-center gap-1.5 rounded-full bg-black/75 px-3 py-1.5 text-[11px] font-medium text-white shadow-lg backdrop-blur-sm">
                              <Download className="size-3.5" /> 下载 MP4
                            </span>
                          </div>
                          <span className="absolute left-2 top-2 flex items-center gap-1 rounded-full bg-emerald-500/90 px-2 py-1 text-[9px] font-medium text-white shadow">
                            <Check className="size-3" /> 已完成
                          </span>
                        </div>
                      </a>
                      <div className="flex items-center justify-between gap-3 px-3 py-2.5">
                        <div className="min-w-0">
                          <p className="truncate text-xs font-medium text-zinc-200">{projectLabel}</p>
                          <p className="mt-0.5 text-[9px] text-zinc-600">{formatExportTime(item.finished_at || item.created_at)}</p>
                        </div>
                        <div className="flex shrink-0 items-center gap-1">
                          <a
                            href={item.video_path}
                            download={filename}
                            className="rounded-md p-1.5 text-zinc-500 transition-colors hover:bg-emerald-500/10 hover:text-emerald-400"
                            title={`下载 ${filename}`}
                            aria-label={`下载 ${filename}`}
                          >
                            <Download className="size-4" />
                          </a>
                          <button
                            type="button"
                            className="rounded-md p-1.5 text-zinc-500 transition-colors hover:bg-red-500/10 hover:text-red-300 disabled:pointer-events-none disabled:opacity-50"
                            onClick={() => onDelete(item)}
                            disabled={deletingToken === item.token}
                            title="删除导出记录"
                            aria-label="删除导出记录"
                          >
                            {deletingToken === item.token ? (
                              <Loader2 className="size-4 animate-spin" />
                            ) : (
                              <Trash2 className="size-4" />
                            )}
                          </button>
                        </div>
                      </div>
                    </div>
                  )
                }

                const pending = item.status === "pending"
                return (
                  <div key={item.token} className={cn(
                    "rounded-xl border p-3",
                    item.status === "failed"
                      ? "border-red-500/20 bg-red-500/[.04]"
                      : "border-sky-500/20 bg-sky-500/[.04]"
                  )}>
                    <div className="flex items-start gap-3">
                      <span className={cn(
                        "grid size-10 shrink-0 place-items-center rounded-lg",
                        item.status === "failed" ? "bg-red-500/10 text-red-400" : "bg-sky-500/10 text-sky-400"
                      )}>
                        {item.status === "failed" ? <X className="size-4" /> : <Loader2 className="size-4 animate-spin" />}
                      </span>
                      <div className="min-w-0 flex-1">
                        <div className="flex items-center justify-between gap-3">
                          <p className="truncate text-xs font-medium text-zinc-200">{projectLabel}</p>
                          <div className="flex shrink-0 items-center gap-1.5">
                            <span className={cn(
                              "text-[9px] font-medium",
                              item.status === "failed" ? "text-red-400" : "text-sky-400"
                            )}>
                              {item.status === "failed" ? "导出失败" : pending ? "排队中" : `处理中 ${item.progress}%`}
                            </span>
                            {item.status === "failed" && (
                              <button
                                type="button"
                                className="rounded p-1 text-zinc-500 transition-colors hover:bg-red-500/10 hover:text-red-300 disabled:pointer-events-none disabled:opacity-50"
                                onClick={() => onDelete(item)}
                                disabled={deletingToken === item.token}
                                title="删除导出记录"
                                aria-label="删除导出记录"
                              >
                                {deletingToken === item.token ? (
                                  <Loader2 className="size-3.5 animate-spin" />
                                ) : (
                                  <Trash2 className="size-3.5" />
                                )}
                              </button>
                            )}
                          </div>
                        </div>
                        <p className="mt-1 text-[9px] text-zinc-600">{formatExportTime(item.created_at)} · {item.token.slice(0, 8)}</p>
                        {item.status === "failed" ? (
                          <p className="mt-2 break-words text-[10px] leading-relaxed text-red-300/80">{item.error || "视频导出失败，请重新导出"}</p>
                        ) : (
                          <>
                            <div className="mt-2 h-1.5 overflow-hidden rounded-full bg-white/5">
                              <div
                                className={cn("h-full rounded-full bg-sky-500 transition-[width] duration-500", !pending && "animate-pulse")}
                                style={{ width: `${pending ? 4 : Math.max(5, item.progress)}%` }}
                              />
                            </div>
                            <p className="mt-1.5 text-[9px] text-zinc-600">
                              {pending
                                ? "等待可用的媒体处理资源…"
                                : item.progress < 25
                                  ? "正在准备导出素材…"
                                  : item.progress < 40
                                    ? "正在合成时间线…"
                                    : item.progress < 90
                                      ? "FFmpeg 正在编码，耗时取决于视频长度和分辨率…"
                                      : "正在保存导出视频…"}
                            </p>
                          </>
                        )}
                      </div>
                    </div>
                  </div>
                )
              })}
            </div>
          )}
        </div>
      </SheetContent>
    </Sheet>
  )
}

function InspectorSection({ title, children }: { title: string; children: React.ReactNode }) {
  return <section><h3 className="mb-2 text-[10px] font-semibold uppercase tracking-[0.12em] text-zinc-500">{title}</h3><div className="space-y-2.5">{children}</div></section>
}

function DarkField({ label, children }: { label: string; children: React.ReactNode }) {
  return <Label className="block text-[9px] font-normal text-zinc-500"><span className="mb-1 block">{label}</span>{children}</Label>
}

function DarkInput({ value, onChange }: { value: string; onChange: (value: string) => void }) {
  return <Input value={value} onChange={(event) => onChange(event.target.value)} className="h-8 rounded-lg border-white/10 bg-black/20 px-2.5 text-xs text-zinc-200" />
}

function NumberField({
  label,
  value,
  onChange,
  step,
  min,
  max,
  suffix,
}: {
  label: string
  value: number
  onChange: (value: number) => void
  step: number
  min?: number
  max?: number
  suffix?: string
}) {
  return (
    <Label className="block text-[9px] font-normal text-zinc-500">
      <span className="mb-1 block">{label}</span>
      <span className="relative block">
        <Input
          type="number"
          value={Number.isFinite(value) ? Number(value.toFixed(4)) : 0}
          step={step}
          min={min}
          max={max}
          onChange={(event) => {
            const next = Number(event.target.value)
            if (Number.isFinite(next)) onChange(Math.min(max ?? Infinity, Math.max(min ?? -Infinity, next)))
          }}
          className="h-8 rounded-lg border-white/10 bg-black/20 px-2.5 pr-8 font-mono text-[10px] text-zinc-200"
        />
        {suffix && <span className="pointer-events-none absolute right-2 top-1/2 -translate-y-1/2 text-[8px] text-zinc-600">{suffix}</span>}
      </span>
    </Label>
  )
}

function Stat({ label, value }: { label: string; value: string | number }) {
  return <div className="rounded-lg border border-white/10 bg-black/15 p-2"><p className="text-[8px] text-zinc-600">{label}</p><p className="mt-1 font-mono text-xs text-zinc-300">{value}</p></div>
}

function GenerationDialog({
  models,
  model,
  seconds,
  prompt,
  cost,
  generating,
  status,
  gap,
  onModel,
  onSeconds,
  onPrompt,
  onClose,
  onGenerate,
}: {
  models: VideoModel[]
  model: string
  seconds: number
  prompt: string
  cost: number | null
  generating: boolean
  status: string
  gap: { start: number; duration: number }
  onModel: (value: string) => void
  onSeconds: (value: number) => void
  onPrompt: (value: string) => void
  onClose: () => void
  onGenerate: () => void
}) {
  const active = models.find((item) => item.model === model)
  return (
    <div className="absolute inset-0 z-[70] grid place-items-center bg-black/70 p-4 backdrop-blur-sm" onPointerDown={(event) => event.target === event.currentTarget && onClose()}>
      <div className="w-full max-w-lg overflow-hidden rounded-2xl border border-white/10 bg-[#181920] shadow-2xl">
        <div className="flex items-center justify-between border-b border-white/10 px-5 py-4">
          <div className="flex items-center gap-3"><span className="grid size-9 place-items-center rounded-xl bg-violet-500/15 text-violet-300"><WandSparkles className="size-4" /></span><div><h2 className="text-sm font-semibold text-zinc-100">流水线生成缺失镜头</h2><p className="mt-0.5 text-[10px] text-zinc-500">生成结果会自动落在 {gap.start.toFixed(2)}s 的空隙并进入素材库</p></div></div>
          <Button size="icon-sm" variant="ghost" className="text-zinc-500 hover:bg-white/10" disabled={generating} onClick={onClose}><X /></Button>
        </div>
        <div className="space-y-4 p-5">
          {models.length === 0 ? (
            <div className="rounded-xl border border-amber-400/20 bg-amber-400/5 p-4 text-xs text-amber-200/80">管理员尚未配置视频生成模型。你仍可前往完整流水线编排图片生成、视频生成、裁剪和合并节点。<Button asChild size="xs" variant="outline" className="mt-3 border-white/10 bg-white/5"><Link to="/workflows">打开流水线</Link></Button></div>
          ) : (
            <>
              <div className="grid grid-cols-2 gap-3">
                <DarkField label="生成模型">
                  <Select value={model} onValueChange={onModel}><SelectTrigger size="sm" className="w-full border-white/10 bg-black/20 text-xs text-zinc-200"><SelectValue /></SelectTrigger><SelectContent>{models.map((item) => <SelectItem key={item.model} value={item.model}>{item.display_name ?? item.model}</SelectItem>)}</SelectContent></Select>
                </DarkField>
                <DarkField label={`镜头时长 · 当前空隙约 ${gap.duration.toFixed(1)}s`}>
                  <Select value={String(seconds)} onValueChange={(value) => onSeconds(Number(value))}><SelectTrigger size="sm" className="w-full border-white/10 bg-black/20 text-xs text-zinc-200"><SelectValue /></SelectTrigger><SelectContent>{(active?.allowed_seconds ?? [5]).map((value) => <SelectItem key={value} value={String(value)}>{value} 秒</SelectItem>)}</SelectContent></Select>
                </DarkField>
              </div>
              <DarkField label="镜头描述与运镜">
                <Textarea value={prompt} onChange={(event) => onPrompt(event.target.value)} placeholder="例如：雨夜霓虹街道，人物从画面左侧走入，镜头缓慢向前推进，电影感，保持角色与上一镜一致…" className="min-h-32 rounded-xl border-white/10 bg-black/20 text-xs leading-relaxed text-zinc-200 placeholder:text-zinc-600" maxLength={4000} />
              </DarkField>
              <div className="rounded-xl border border-white/10 bg-black/15 p-3 text-[10px] leading-relaxed text-zinc-500">
                系统会创建一个可追溯的视频生成流水线。生成期间可以继续剪辑；完成后镜头自动加入主视频轨，若生成时长大于空隙，可用修边手柄精确裁掉。
              </div>
            </>
          )}
        </div>
        <div className="flex items-center justify-between border-t border-white/10 bg-black/10 px-5 py-3">
          <span className="text-[10px] text-zinc-500">{generating ? <span className="flex items-center gap-2 text-violet-300"><Loader2 className="size-3 animate-spin" /> {status}</span> : cost != null ? `预计消耗 ${formatQuota(cost)} 元` : "使用平台视频生成额度"}</span>
          <Button size="sm" className="bg-violet-500 text-white hover:bg-violet-400" disabled={!active || !prompt.trim() || generating} onClick={onGenerate}>{generating ? <Loader2 className="animate-spin" /> : <Sparkles />} {generating ? "生成中" : "启动补片流水线"}</Button>
        </div>
      </div>
    </div>
  )
}
