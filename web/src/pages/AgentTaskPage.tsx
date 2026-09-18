import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { useNavigate, useParams } from "react-router-dom"
import { Loader2, Menu, Laptop, Send, Square } from "lucide-react"
import { cn } from "@/lib/utils"
import { Button } from "@/components/ui/button"
import { Textarea } from "@/components/ui/textarea"
import { Sheet, SheetContent, SheetTitle, SheetTrigger } from "@/components/ui/sheet"
import { Sidebar } from "@/components/app/Sidebar"
import { SidebarToggle } from "@/components/app/SidebarToggle"
import { AgentTranscript, ApprovalCard } from "@/components/app/AgentTranscript"
import { DeviceDialog } from "@/components/app/DeviceDialog"
import { WorkspacePicker } from "@/components/app/WorkspacePicker"
import {
  ModeSelector,
  ModeSwitch,
  TargetBadge,
  type DeviceOption,
} from "@/components/app/ModeSelector"
import { readModeDraft, useModeSwitch } from "@/lib/mode"
import { ModelPicker } from "@/components/app/ModelPicker"
import { ThinkingPicker } from "@/components/app/ThinkingPicker"
import { listPlatformModels } from "@/lib/platform-models"
import type { Protocol } from "@/lib/settings"
import {
  agentApi,
  entriesToItems,
  listDevices,
  mergeAgentItems,
  subscribeAgentSession,
  type AgentItem,
  type AgentSession,
  type AgentTarget,
  type ThinkingLevel,
} from "@/lib/agent"
import {
  capabilities,
  notifyApprovalPending,
  onAppStateChange,
  requestNotificationPermission,
} from "@/lib/platform"
import { approvalTitle } from "@/lib/agent"
import { useAuth } from "@/lib/auth-context"
import { toast } from "sonner"

/**
 * Protocol of a stored model.
 *
 * A session records only the model name, so a task reopened later would show
 * the wrong protocol badge until the user touched the picker. Resolving it
 * from the catalogue is one request, shared across calls, and only made when a
 * task actually has a pinned model.
 */
let agentModelProtocols: Promise<Map<string, Protocol>> | null = null
function protocolOf(model: string): Promise<Protocol | undefined> {
  agentModelProtocols ??= listPlatformModels("chat")
    .then(
      (list) =>
        new Map(
          list
            .filter((m) => m.agent_provider != null)
            .map((m) => [m.model, m.protocol as Protocol])
        )
    )
    .catch(() => {
      // Never cache a failure: the badge is cosmetic, but a poisoned cache
      // would keep it wrong for the rest of the session.
      agentModelProtocols = null
      return new Map<string, Protocol>()
    })
  return agentModelProtocols.then((m) => m.get(model))
}

/**
 * Work-mode task workspace.
 *
 * Kept separate from ChatPage rather than folded into it: chat is a
 * request/response exchange owned by the tab, while a task is a long-running
 * session owned by the server that any device can join. Sharing one component
 * would mean one of the two models is always being worked around.
 *
 * The transcript is always derived from the server's mirror. Streaming deltas
 * are rendered as a transient overlay and discarded once the turn settles, so
 * the tab never becomes a second source of truth that can disagree with what
 * another device sees.
 */
export default function AgentTaskPage() {
  const { id } = useParams()
  const nav = useNavigate()
  const auth = useAuth()
  const user = auth.state.status === "authed" ? auth.state.user : null
  const sessionId = id ? Number(id) : null

  const [session, setSession] = useState<AgentSession | null>(null)
  const [items, setItems] = useState<AgentItem[]>([])
  const [streamingText, setStreamingText] = useState("")
  const [running, setRunning] = useState(false)
  const [starting, setStarting] = useState(false)
  const [sandboxed, setSandboxed] = useState<boolean | undefined>(undefined)
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const [approvals, setApprovals] = useState<any[]>([])
  const [answering, setAnswering] = useState(false)
  // Seeded from the prompt carried over from chat mode, and only on the
  // compose route: dropping unrelated text in front of a live agent would be
  // worse than losing it. Seeded rather than adopted in an effect so the
  // composer never renders empty and then visibly fills itself in.
  const [input, setInput] = useState(() =>
    sessionId == null ? readModeDraft() : ""
  )
  const [target, setTarget] = useState<AgentTarget>("cloud")
  const [deviceId, setDeviceId] = useState<number | null>(null)
  // Directory the task will run in on the chosen machine. Null means "let the
  // client use its default", which is what every task did before this could
  // be chosen. Reset with the machine: a path from one computer is meaningless
  // on another, and carrying it over would produce a refusal at start.
  const [workspace, setWorkspace] = useState<string | null>(null)
  const [workspaceOpen, setWorkspaceOpen] = useState(false)
  // The model the task runs on. Empty means "whatever the runtime defaults
  // to", which is what every task did before this control existed; the label
  // says so rather than naming a model the server never actually pinned.
  const [model, setModel] = useState("")
  const [modelProtocol, setModelProtocol] = useState<Protocol>("claude")
  // How hard the agent reasons. Null means the runtime's own default, which is
  // what every task did before this control existed; the label says "default"
  // rather than naming a level the server never actually pinned.
  const [thinking, setThinking] = useState<ThinkingLevel | null>(null)
  // Which levels the *selected model* supports, as reported by a live runtime.
  // Empty until one answers, in which case the picker offers the whole ladder
  // rather than hiding options that would in fact work.
  const [thinkingLevels, setThinkingLevels] = useState<ThinkingLevel[]>([])
  const [devices, setDevices] = useState<DeviceOption[]>([])
  const [sidebarOpen, setSidebarOpen] = useState(false)
  const [devicesOpen, setDevicesOpen] = useState(false)

  // ── 对话 / 工作模式切换 ──
  const switchMode = useModeSwitch("work")

  const bottomRef = useRef<HTMLDivElement>(null)
  // Latest mirrored entry id, used as the incremental cursor.
  const cursor = useRef<string | undefined>(undefined)
  // Serializes reconciliation, see `syncEntries`.
  const syncChain = useRef<Promise<unknown>>(Promise.resolve())
  // The session the transcript on screen belongs to, so a sync that was
  // queued for the previous task can tell that it came back too late.
  const activeSid = useRef<number | null>(sessionId)
  // Declared before every effect that syncs: React runs effects in
  // declaration order, so the guard is current by the time the loading effect
  // below asks for entries.
  useEffect(() => {
    activeSid.current = sessionId
  }, [sessionId])

  // The session list itself is rendered by the sidebar, which loads it
  // independently; this only refreshes the *current* session's status so the
  // header stops showing "running" after a turn settles.
  const refreshSessions = useCallback(async () => {
    if (sessionId == null) return
    try {
      const all = await agentApi.sessions()
      setSession(all.find((x) => x.id === sessionId) ?? null)
    } catch {
      /* status refresh is non-critical */
    }
  }, [sessionId])

  /** Reconcile the transcript from the server.
   *
   *  Always the recovery path: initial load, a reconnect, and a `resync` hint
   *  all funnel here, so there is a single way the view catches up.
   *
   *  Serialized through `syncChain`, because the cursor is read before the
   *  request and written after it: two overlapping incremental syncs would
   *  both start from the same id, fetch the same range and append it twice.
   *  That is what duplicated a whole turn on screen — a settle arrives as the
   *  runtime's own `agent_settled` frame *and* as the server's `settled` once
   *  mirroring finished, so two syncs raced across the same mirror write. */
  const syncEntries = useCallback((sid: number, full = false) => {
    const run = async () => {
      // The route moved on while this call waited its turn; the transcript on
      // screen belongs to another session now, so appending would corrupt it.
      if (activeSid.current !== sid) return
      try {
        if (full) cursor.current = undefined
        const entries = await agentApi.entries(sid, cursor.current)
        if (activeSid.current !== sid) return
        if (entries.length > 0) {
          cursor.current = entries[entries.length - 1]!.id
        } else if (!full) {
          // Nothing new; keep what is rendered.
          return
        }
        const fresh = entriesToItems(entries)
        // A full sync must replace even when it returns nothing, or switching
        // to an empty session would leave the previous transcript on screen.
        setItems((prev) => (full ? fresh : mergeAgentItems(prev, fresh)))
      } catch (e) {
        toast.error(`读取会话记录失败：${(e as Error).message}`)
      }
    }
    // Chained on settle *and* rejection so one failed pass cannot wedge every
    // later sync behind it.
    const next = syncChain.current.then(run, run)
    syncChain.current = next
    return next
  }, [])

  // Load the user's machines so the picker can offer them.
  //
  // Refreshed on a timer because liveness comes from an open socket, not from
  // a stored row: a laptop that just woke up should become selectable without
  // the user reloading the page.
  useEffect(() => {
    let cancelled = false
    const load = () => {
      listDevices()
        .then((ds) => {
          if (cancelled) return
          setDevices(
            ds
              .filter((d) => !d.revoked)
              .map((d) => ({ id: d.id, name: d.name, online: d.online }))
          )
        })
        .catch(() => {
          /* the picker still works with cloud only */
        })
    }
    load()
    const timer = setInterval(load, 15000)
    return () => {
      cancelled = true
      clearInterval(timer)
    }
  }, [])

  // Load the session and its history.
  //
  // The per-session reset happens inside the async body rather than in the
  // effect body: clearing synchronously triggers a cascading render, and the
  // stale transcript is replaced in the same update as the fresh one anyway.
  useEffect(() => {
    if (sessionId == null) return
    let cancelled = false
    cursor.current = undefined
    ;(async () => {
      const all = await agentApi.sessions().catch(() => [] as AgentSession[])
      if (cancelled) return
      const s = all.find((x) => x.id === sessionId) ?? null
      setStreamingText("")
      setApprovals([])
      setSession(s)
      // Adopt the stored model so the picker reflects what this task actually
      // runs on, not what was selected on the previous screen.
      setModel(s?.model ?? "")
      setThinking(s?.thinking_level ?? null)
      setRunning(s?.status === "running")
      await syncEntries(sessionId, true)
    })()
    return () => {
      cancelled = true
    }
  }, [sessionId, syncEntries])

  // Subscribe to live events while a runtime is attached.
  useEffect(() => {
    if (sessionId == null || !session?.live) return
    const stop = subscribeAgentSession(sessionId, {
      onEvent: (e) => {
        switch (e.type) {
          case "agent_start":
            setRunning(true)
            break
          case "message_update": {
            const ev = e.data?.assistantMessageEvent
            if (ev?.type === "text_delta" && typeof ev.delta === "string") {
              setStreamingText((t) => t + ev.delta)
            }
            break
          }
          case "tool_execution_start":
          case "tool_execution_end":
            // Tool activity is authoritative only once mirrored; flush the
            // partial text so it is not shown twice around the tool card.
            setStreamingText("")
            void syncEntries(sessionId)
            break
          case "extension_ui_request":
            setApprovals((prev) =>
              prev.some((p) => p?.id === e.data?.id) ? prev : [...prev, e.data]
            )
            // A blocked agent is useless if the user never learns it is
            // waiting, which is the main reason to package a mobile app.
            void notifyApprovalPending(
              approvalTitle(e.data),
              "Agent 正在等待你确认"
            )
            break
          case "approval_resolved":
            // Answered here or on another device; either way the card is no
            // longer actionable and must not linger.
            setApprovals((prev) => prev.filter((p) => p?.id !== e.data?.id))
            break
          case "settled":
          case "agent_settled":
            setRunning(false)
            setStreamingText("")
            void syncEntries(sessionId)
            void refreshSessions()
            break
          default:
            break
        }
      },
      onResync: () => {
        // This subscriber fell behind, so the stream is no longer a complete
        // record. Rebuild from the mirror instead of rendering a gap.
        setStreamingText("")
        void syncEntries(sessionId, true)
      },
      onClosed: (reason) => {
        setRunning(false)
        setStreamingText("")
        setSession((s) => (s ? { ...s, live: false } : s))
        toast.info(reason)
      },
    })
    return stop
  }, [sessionId, session?.live, syncEntries, refreshSessions])

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ block: "end" })
  }, [items.length, streamingText])

  // Colour the picker's badge for a model restored from the server.
  useEffect(() => {
    if (!model) return
    let cancelled = false
    void protocolOf(model).then((p) => {
      if (!cancelled && p) setModelProtocol(p)
    })
    return () => {
      cancelled = true
    }
  }, [model])

  // Ask the runtime which reasoning levels its current model supports.
  //
  // Only a live runtime can answer, and the answer changes with the model, so
  // this re-runs on both. A failure is silent on purpose: the picker falls
  // back to the full ladder, and pi clamps a level the model cannot do rather
  // than rejecting it.
  useEffect(() => {
    if (sessionId == null || !session?.live) return
    let cancelled = false
    void agentApi
      .thinkingLevels(sessionId)
      .then((r) => {
        if (cancelled) return
        setThinkingLevels(r.levels ?? [])
      })
      .catch(() => {
        /* the picker still works with the full ladder */
      })
    return () => {
      cancelled = true
    }
  }, [sessionId, session?.live, model])

  // Ask for notification permission once, and only where it means something.
  //
  // Requested on entering the task workspace rather than at launch: the prompt
  // is self-explanatory here, because this is the screen whose approvals the
  // user would want to be told about.
  useEffect(() => {
    if (!capabilities().canNotify) return
    void requestNotificationPermission()
  }, [])

  // Reconcile after returning to the foreground.
  //
  // A phone suspends timers and can drop the event stream while backgrounded,
  // so the transcript on screen may be stale. The mirror is authoritative, so
  // resync instead of trusting what survived.
  useEffect(() => {
    if (sessionId == null) return
    let dispose: (() => void) | undefined
    void onAppStateChange((active) => {
      if (active) void syncEntries(sessionId, true)
    }).then((off) => {
      dispose = off
    })
    return () => dispose?.()
  }, [sessionId, syncEntries])

  const ensureRuntime = useCallback(async () => {
    if (sessionId == null) return false
    if (session?.live) return true
    setStarting(true)
    try {
      const r = await agentApi.start(sessionId)
      setSandboxed(r.sandboxed)
      setSession((s) => (s ? { ...s, live: true } : s))
      return true
    } catch (e) {
      toast.error(`启动运行时失败：${(e as Error).message}`)
      return false
    } finally {
      setStarting(false)
    }
  }, [sessionId, session?.live])

  /** Create a task, then send the first prompt into it. */
  const createAndSend = useCallback(
    async (message: string) => {
      if (target === "device" && deviceId == null) {
        toast.error("请选择一台本地电脑")
        return
      }
      try {
        const created = await agentApi.createSession({
          target,
          device_id: deviceId ?? undefined,
          title: message.slice(0, 40),
          model: model || undefined,
          workspace: workspace ?? undefined,
          thinking_level: thinking ?? undefined,
        })
        // Carry the prompt across the navigation so the user does not retype
        // it after the route changes.
        nav(`/t/${created.id}`, { state: { pending: message } })
      } catch (e) {
        toast.error(`创建任务失败：${(e as Error).message}`)
      }
    },
    [target, deviceId, model, workspace, thinking, nav]
  )

  /** Switch the model this task runs on.
   *
   *  Before a task exists there is nothing to tell, so the choice is just held
   *  and passed to `createSession`. Once it does, the server owns it: it
   *  applies the change to a live runtime and stores it for the next start. */
  const changeModel = useCallback(
    async (next: string, protocol?: Protocol) => {
      const previous = model
      setModel(next)
      if (protocol) setModelProtocol(protocol)
      if (sessionId == null) return
      try {
        await agentApi.setModel(sessionId, next)
      } catch (e) {
        // Roll back rather than leave the picker claiming a model the agent is
        // not on; a silent mismatch would misattribute the next answer.
        setModel(previous)
        toast.error(`切换模型失败：${(e as Error).message}`)
      }
    },
    [sessionId, model]
  )

  /** Switch how hard this task reasons.
   *
   *  Mirrors `changeModel` deliberately: held locally before a task exists,
   *  owned by the server once it does, and rolled back on failure so the
   *  control never claims a level the agent is not actually running at. */
  const changeThinking = useCallback(
    async (next: ThinkingLevel) => {
      const previous = thinking
      setThinking(next)
      if (sessionId == null) return
      try {
        await agentApi.setThinkingLevel(sessionId, next)
      } catch (e) {
        setThinking(previous)
        toast.error(`切换推理级别失败：${(e as Error).message}`)
      }
    },
    [sessionId, thinking]
  )

  const send = useCallback(
    async (message: string) => {
      const text = message.trim()
      if (!text) return
      if (sessionId == null) {
        await createAndSend(text)
        return
      }
      if (!(await ensureRuntime())) return
      setInput("")
      try {
        // While streaming, a bare prompt is rejected by the runtime; queue it
        // as steering so the user's correction is not lost.
        await agentApi.prompt(sessionId, text, running ? "steer" : undefined)
        setRunning(true)
        // Show the user's own turn immediately; the mirror confirms it later.
        void syncEntries(sessionId)
      } catch (e) {
        toast.error(`发送失败：${(e as Error).message}`)
      }
    },
    [sessionId, running, ensureRuntime, createAndSend, syncEntries]
  )

  // Deliver a prompt carried over from task creation.
  //
  // Consumed from the history entry as well as guarded by the ref: the ref
  // only survives as long as this mount, so a reload of `/t/:id` would replay
  // the same prompt and the task really would hold the turn twice.
  const pending = (window.history.state?.usr?.pending ?? null) as string | null
  const pendingSent = useRef(false)
  useEffect(() => {
    if (!pending || pendingSent.current || sessionId == null) return
    pendingSent.current = true
    const state = window.history.state
    window.history.replaceState(
      { ...state, usr: { ...(state?.usr ?? {}), pending: undefined } },
      ""
    )
    void send(pending)
  }, [pending, sessionId, send])

  const answer = useCallback(
    async (
      requestId: string,
      body: { confirmed?: boolean; value?: string; cancelled?: boolean }
    ) => {
      if (sessionId == null) return
      setAnswering(true)
      try {
        await agentApi.approve(sessionId, { request_id: requestId, ...body })
      } catch (e) {
        // A 409 means another device answered first, which is expected in a
        // multi-client session and is not a failure.
        const msg = (e as Error).message
        if (!msg.includes("已处理") && !msg.includes("409")) toast.error(`处理失败：${msg}`)
      } finally {
        setApprovals((prev) => prev.filter((p) => p?.id !== requestId))
        setAnswering(false)
      }
    },
    [sessionId]
  )

  const stop = useCallback(async () => {    if (sessionId == null) return
    try {
      await agentApi.abort(sessionId)
      setRunning(false)
    } catch (e) {
      toast.error(`中止失败：${(e as Error).message}`)
    }
  }, [sessionId])

  // Exactly one switch on screen: the large one in the empty state, otherwise
  // the compact one in the composer strip.
  const heroSwitch = sessionId == null && items.length === 0

  const header = useMemo(
    () => (
      // Same height and treatment as chat's header, so the content below —
      // including the mode switch — starts at the same y on both screens and
      // the switch does not jump when modes change. The vertical padding is
      // handed to `safe-top` as `--safe-area-extra-top` rather than written as
      // `py-2`: the helper owns `padding-top`, so a competing utility would be
      // dropped and the header would sit higher here than in chat.
      <div className="safe-top [--safe-area-extra-top:0.5rem] relative z-30 flex min-h-14 flex-wrap items-center gap-2 bg-background/65 px-2.5 pb-2 backdrop-blur-xl md:px-4">
        <Sheet open={sidebarOpen} onOpenChange={setSidebarOpen}>
          <SheetTrigger asChild>
            <Button variant="ghost" size="icon-sm" className="tap-target md:hidden">
              <Menu />
            </Button>
          </SheetTrigger>
          <SheetContent side="left" className="w-[16rem] p-0">
            <SheetTitle className="sr-only">导航</SheetTitle>
            <Sidebar onNavigate={() => setSidebarOpen(false)} />
          </SheetContent>
        </Sheet>
        {/* Same thin header as chat, same panel toggle in the same pixel, so
            switching modes does not move the chrome under the pointer. */}
        <SidebarToggle />
        <span className="truncate text-sm font-medium">
          {session?.title ?? "新工作任务"}
        </span>
        {session && (
          <TargetBadge
            target={session.target}
            live={session.live}
            sandboxed={sandboxed}
            className="ml-auto"
          />
        )}
        {running && (
          <Button variant="outline" size="sm" onClick={stop}>
            <Square className="size-3" />
            中止
          </Button>
        )}
      </div>
    ),
    [session, sandboxed, running, stop, sidebarOpen]
  )

  return (
    // `h-svh` rather than `min-h-svh`: the composer is bottom-anchored, so a
    // growing page would push it past the viewport instead of scrolling the
    // transcript. Matches chat, which is why the two screens line up.
    <div className="app-shell flex h-svh bg-background text-foreground">
      {/* The sidebar owns its own width (and collapses to a rail), so the
          wrapper must not pin one or the two disagree while collapsing. */}
      <aside className="hidden shrink-0 md:block">
        <Sidebar />
      </aside>

      <main className="flex min-w-0 flex-1 flex-col">
        {header}

        <div className="nc-scroll flex-1 overflow-y-auto px-3 py-4 md:px-6 md:py-6">
          <div
            className={cn(
              "mx-auto w-full max-w-4xl",
              // Centred while empty, top-aligned once there is a transcript —
              // the same rule chat uses, so the greeting sits at the same
              // height in both modes.
              heroSwitch ? "flex min-h-full flex-col justify-center" : "space-y-4"
            )}
          >
            {sessionId == null && items.length === 0 && (
              <div className="fade-up mx-auto flex w-full max-w-2xl flex-col items-center gap-6 text-center">
                {/* Same hero skeleton as chat's empty state — mark, heading,
                    switch — so the switch lands in the same place on screen
                    before and after the route change. Shifting it by a hundred
                    pixels is what made the toggle feel like a page load. */}
                <div className="relative">
                  <div className="absolute inset-2 rounded-3xl bg-primary/30 blur-2xl" />
                  <img
                    src="/logo.svg"
                    alt=""
                    className="relative size-14 rounded-[1.15rem] ring-1 ring-white/15 shadow-panel md:size-16"
                  />
                </div>
                <div>
                  <p className="text-[1.6rem] font-semibold tracking-[-0.035em] md:text-[2rem]">
                    今天有什么工作要处理？
                  </p>
                  <p className="mx-auto mt-2 max-w-xl text-sm leading-relaxed text-muted-foreground">
                    工作模式会启动一个能执行命令的 Agent，请先选择它运行的位置。
                  </p>
                </div>
                <ModeSwitch
                  mode="work"
                  size="lg"
                  onModeChange={(m) => switchMode(m, input)}
                />
              </div>
            )}

            <AgentTranscript
              items={items}
              streamingText={streamingText}
              userAvatarUrl={user?.avatar_url ?? null}
              userInitial={(user?.display_name?.trim() || user?.username || "?").slice(0, 1)}
            />

            {approvals.map((req) => (
              <ApprovalCard
                key={req?.id}
                request={req}
                busy={answering}
                onAnswer={(body) => void answer(String(req?.id), body)}
              />
            ))}

            {(running || starting) && !streamingText && (
              // Aligned with the assistant column so the pending turn reads as
              // the next bubble rather than a stray line under the transcript.
              <div className="flex items-center gap-2.5">
                <span className="relative grid size-8 shrink-0 place-items-center">
                  <span className="absolute inset-0 animate-ping rounded-xl bg-primary/20" />
                  <img
                    src="/logo.svg"
                    alt=""
                    className="relative size-8 rounded-xl shadow-sm ring-1 ring-border/70"
                  />
                </span>
                <span className="flex items-center gap-2 rounded-[1.25rem] rounded-tl-md border border-border/60 bg-card/80 px-3.5 py-2 text-xs text-muted-foreground shadow-sm backdrop-blur-sm">
                  <Loader2 className="size-3.5 animate-spin" />
                  {starting ? "正在启动运行时…" : "Agent 正在处理…"}
                </span>
              </div>
            )}
            <div ref={bottomRef} />
          </div>
        </div>

        {/* `safe-bottom` keeps the composer clear of the home indicator; a
            bottom-anchored control would otherwise be partly untappable in
            the packaged app. It owns `padding-bottom`, so the desktop spacing
            travels in `--safe-area-extra-bottom` instead of `pb-3`/`md:pb-4`,
            which the helper would otherwise override — that is what pinned
            this composer to the very bottom edge of the window. */}
        <div className="safe-bottom [--safe-area-extra-bottom:0.75rem] md:[--safe-area-extra-bottom:1rem] bg-background/70 px-3 pt-1.5 backdrop-blur-xl md:px-6">
          <div className="mx-auto w-full max-w-4xl">
            {/* One rounded box, text above and controls below, matching chat's
                composer: work mode carries more controls than chat, and beside
                the textarea they left it a narrow slot in a very wide box. */}
            <form
              className="glass-surface flex flex-col gap-1 rounded-[1.35rem] px-2.5 py-2 transition-all focus-within:border-primary/35 focus-within:ring-2 focus-within:ring-ring"
              onSubmit={(e) => {
                e.preventDefault()
                void send(input)
              }}
            >
              <Textarea
                value={input}
                onChange={(e) => setInput(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && !e.shiftKey) {
                    e.preventDefault()
                    void send(input)
                  }
                }}
                rows={1}
                placeholder={
                  running ? "Agent 正在运行，发送的消息会插入当前轮次…" : "描述要完成的工作…"
                }
                className="max-h-60 min-h-[40px] w-full resize-none border-0 bg-transparent px-1.5 py-2 shadow-none focus-visible:ring-0"
              />
              <div className="flex flex-wrap items-center gap-1.5">
                <ModeSelector
                  onModeChange={(m) => switchMode(m, input)}
                  target={session?.target ?? target}
                  onTargetChange={setTarget}
                  devices={devices}
                  deviceId={deviceId}
                  onDeviceChange={(id) => {
                    setDeviceId(id)
                    // A path belongs to one machine; keeping it across a switch
                    // would aim the task at a directory the new machine has
                    // never heard of.
                    setWorkspace(null)
                  }}
                  targetLocked={sessionId != null}
                  workspace={session ? session.workspace : workspace}
                  onPickWorkspace={
                    // Only before the task exists: its runtime and transcript
                    // belong to the directory it started in.
                    sessionId == null && deviceId != null
                      ? () => setWorkspaceOpen(true)
                      : undefined
                  }
                  hideSwitch={heroSwitch}
                />
                {/* Work mode is always platform-billed, so only models the
                    admin priced *and* an agent runtime can address are
                    offered. Filtering on `agent_provider` keeps a model the
                    runtime has no provider for — Gemini today — out of a
                    picker that would otherwise fail on selection. */}
                <ModelPicker
                  protocol={modelProtocol}
                  model={model}
                  placeholder="默认模型"
                  title="点击切换工作模型"
                  showQuota
                  footer="云端额度 · 仅列出可用于 Agent 的模型"
                  onChangeModel={(next, protocol) => void changeModel(next, protocol)}
                  load={async () =>
                    (await listPlatformModels("chat")).filter(
                      (m) => m.agent_provider != null
                    )
                  }
                />
                {/* Next to the model, because the two are one decision: the
                    ladder a level means depends on which model is selected,
                    and the cost of the pair is what the user is choosing. */}
                <ThinkingPicker
                  level={thinking}
                  levels={thinkingLevels}
                  onChange={(next) => void changeThinking(next)}
                />
                {/* Pairing lives next to the picker: "no local computers" is
                    only actionable if the fix is one click away. */}
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  className="tap-target-sm shrink-0 rounded-full px-2.5 text-xs"
                  onClick={() => setDevicesOpen(true)}
                >
                  <Laptop className="size-3.5" />
                  本地电脑
                </Button>
                <span className="ml-auto" />
                <Button
                  type="submit"
                  size="icon"
                  className="size-9 shrink-0 rounded-full shadow-md shadow-primary/20"
                  disabled={!input.trim() || starting}
                >
                  <Send />
                </Button>
              </div>
            </form>
            {sessionId == null && (
              <p className="mt-1.5 text-center text-[10px] tracking-wide text-muted-foreground/80">
                {target === "cloud"
                  ? "云电脑在隔离容器中运行，仅按 token 计费，不额外收取机时。"
                  : "本地电脑任务在你自己的机器上执行，需要那台电脑上的桌面客户端登录同一账号并保持打开。"}
              </p>
            )}
          </div>
        </div>
      </main>

      <DeviceDialog open={devicesOpen} onOpenChange={setDevicesOpen} />
      <WorkspacePicker
        deviceId={deviceId}
        open={workspaceOpen}
        onOpenChange={setWorkspaceOpen}
        onPick={setWorkspace}
      />
    </div>
  )
}
