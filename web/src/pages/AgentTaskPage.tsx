import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { useNavigate, useParams } from "react-router-dom"
import { Loader2, Menu, Send, Square } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Textarea } from "@/components/ui/textarea"
import { Sheet, SheetContent, SheetTitle, SheetTrigger } from "@/components/ui/sheet"
import { Sidebar } from "@/components/app/Sidebar"
import { AgentTranscript, ApprovalCard } from "@/components/app/AgentTranscript"
import { ModeSelector, TargetBadge, type DeviceOption } from "@/components/app/ModeSelector"
import {
  agentApi,
  entriesToItems,
  subscribeAgentSession,
  type AgentItem,
  type AgentSession,
  type AgentTarget,
} from "@/lib/agent"
import { toast } from "sonner"

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
  const [input, setInput] = useState("")
  const [target, setTarget] = useState<AgentTarget>("cloud")
  const [deviceId, setDeviceId] = useState<number | null>(null)
  const [devices] = useState<DeviceOption[]>([])
  const [sidebarOpen, setSidebarOpen] = useState(false)

  const bottomRef = useRef<HTMLDivElement>(null)
  // Latest mirrored entry id, used as the incremental cursor.
  const cursor = useRef<string | undefined>(undefined)

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
   *  all funnel here, so there is a single way the view catches up. */
  const syncEntries = useCallback(async (sid: number, full = false) => {
    try {
      if (full) cursor.current = undefined
      const entries = await agentApi.entries(sid, cursor.current)
      if (entries.length > 0) {
        cursor.current = entries[entries.length - 1]!.id
      } else if (!full) {
        // Nothing new; keep what is rendered.
        return
      }
      const fresh = entriesToItems(entries)
      // A full sync must replace even when it returns nothing, or switching to
      // an empty session would leave the previous transcript on screen.
      setItems((prev) => (full ? fresh : [...prev, ...fresh]))
    } catch (e) {
      toast.error(`读取会话记录失败：${(e as Error).message}`)
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
        })
        // Carry the prompt across the navigation so the user does not retype
        // it after the route changes.
        nav(`/t/${created.id}`, { state: { pending: message } })
      } catch (e) {
        toast.error(`创建任务失败：${(e as Error).message}`)
      }
    },
    [target, deviceId, nav]
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
  const pending = (window.history.state?.usr?.pending ?? null) as string | null
  const pendingSent = useRef(false)
  useEffect(() => {
    if (!pending || pendingSent.current || sessionId == null) return
    pendingSent.current = true
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

  const stop = useCallback(async () => {
    if (sessionId == null) return
    try {
      await agentApi.abort(sessionId)
      setRunning(false)
    } catch (e) {
      toast.error(`中止失败：${(e as Error).message}`)
    }
  }, [sessionId])

  const header = useMemo(
    () => (
      <div className="flex flex-wrap items-center gap-2 border-b px-4 py-2.5">
        <Sheet open={sidebarOpen} onOpenChange={setSidebarOpen}>
          <SheetTrigger asChild>
            <Button variant="ghost" size="icon-sm" className="md:hidden">
              <Menu />
            </Button>
          </SheetTrigger>
          <SheetContent side="left" className="w-[18rem] p-0">
            <SheetTitle className="sr-only">导航</SheetTitle>
            <Sidebar onNavigate={() => setSidebarOpen(false)} />
          </SheetContent>
        </Sheet>
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
    <div className="app-shell flex min-h-svh">
      {/* Sidebar sets its own 18rem width; the wrapper must match or the
          main column starts underneath it. */}
      <aside className="hidden shrink-0 md:block">
        <Sidebar />
      </aside>

      <main className="flex min-w-0 flex-1 flex-col">
        {header}

        <div className="flex-1 overflow-y-auto px-4 py-4">
          <div className="mx-auto w-full max-w-3xl space-y-3">
            {sessionId == null && items.length === 0 && (
              <div className="py-16 text-center">
                <h1 className="text-2xl font-semibold">今天有什么工作要处理？</h1>
                <p className="mt-2 text-sm text-muted-foreground">
                  工作模式会启动一个能执行命令的 Agent，请选择它运行的位置。
                </p>
              </div>
            )}

            <AgentTranscript items={items} streamingText={streamingText} />

            {approvals.map((req) => (
              <ApprovalCard
                key={req?.id}
                request={req}
                busy={answering}
                onAnswer={(body) => void answer(String(req?.id), body)}
              />
            ))}

            {(running || starting) && !streamingText && (
              <div className="flex items-center gap-2 text-xs text-muted-foreground">
                <Loader2 className="size-3.5 animate-spin" />
                {starting ? "正在启动运行时…" : "Agent 正在处理…"}
              </div>
            )}
            <div ref={bottomRef} />
          </div>
        </div>

        <div className="border-t px-4 py-3">
          <div className="mx-auto w-full max-w-3xl space-y-2">
            {/* The target cannot change once a session exists: its runtime and
                transcript already belong to one machine. */}
            <ModeSelector
              mode="work"
              onModeChange={(m) => {
                if (m === "chat") nav("/")
              }}
              target={session?.target ?? target}
              onTargetChange={setTarget}
              devices={devices}
              deviceId={deviceId}
              onDeviceChange={setDeviceId}
              disabled={sessionId != null}
            />
            <form
              className="flex items-end gap-2"
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
                rows={2}
                placeholder={
                  running ? "Agent 正在运行，发送的消息会插入当前轮次…" : "描述要完成的工作…"
                }
                className="min-h-[3.25rem] resize-none"
              />
              <Button type="submit" size="icon" disabled={!input.trim() || starting}>
                <Send />
              </Button>
            </form>
            {sessionId == null && (
              <p className="text-xs text-muted-foreground">
                {target === "cloud"
                  ? "云电脑在隔离容器中运行，仅按 token 计费，不额外收取机时。"
                  : "本地电脑任务在你自己的机器上执行，需要桌面客户端保持在线。"}
              </p>
            )}
          </div>
        </div>
      </main>
    </div>
  )
}
