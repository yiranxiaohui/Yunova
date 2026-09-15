// Agent task API client (work mode).
//
// Distinct from `lib/worker.ts`, which drives the legacy in-house agent loop.
// Here the loop runs in an external `pi` runtime and the backend only relays,
// so the client subscribes to a session rather than awaiting a reply to its
// own request. That is what lets a task started on a phone be watched from the
// browser: every client opens its own stream against the same session.
//
// The transcript is read from the server's mirror, not accumulated in the tab.
// A reload, a second device, or a dropped connection all recover by asking for
// entries after the last id they saw.

export type AgentTarget = "cloud" | "device"

export interface AgentSession {
  id: number
  target: AgentTarget
  device_id: number | null
  title: string
  model: string | null
  /** idle | running | failed */
  status: string
  updated_at: string
  /** Whether a runtime is attached right now. An idle session can still hold
   *  a warm runtime, so this is not the same as `status`. */
  live: boolean
}

/** One entry of the mirrored session tree, in the runtime's own shape. */
export interface AgentEntry {
  id: string
  parentId: string | null
  type: string
  timestamp?: string
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  message?: any
}

async function jsonOrThrow<T>(res: Response): Promise<T> {
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText)
    throw new Error(text || `HTTP ${res.status}`)
  }
  return res.json() as Promise<T>
}

async function okOrThrow(res: Response): Promise<void> {
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText)
    throw new Error(text || `HTTP ${res.status}`)
  }
}

export const agentApi = {
  async createSession(body: {
    target: AgentTarget
    device_id?: number
    title?: string
    model?: string
  }): Promise<{ id: number; target: string; title: string }> {
    return jsonOrThrow(
      await fetch("/api/agent/sessions", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
        credentials: "same-origin",
      })
    )
  },

  async sessions(): Promise<AgentSession[]> {
    return jsonOrThrow(
      await fetch("/api/agent/sessions", { credentials: "same-origin" })
    )
  },

  /** Fetch mirrored history. Pass the last id already rendered to get only
   *  what is missing, which is how a reconnect avoids refetching everything. */
  async entries(sid: number, since?: string): Promise<AgentEntry[]> {
    const q = since ? `?since=${encodeURIComponent(since)}` : ""
    const data = await jsonOrThrow<{ entries: AgentEntry[] }>(
      await fetch(`/api/agent/sessions/${sid}/entries${q}`, {
        credentials: "same-origin",
      })
    )
    return data.entries ?? []
  },

  async start(sid: number): Promise<{ ok: boolean; reused: boolean; sandboxed?: boolean }> {
    return jsonOrThrow(
      await fetch(`/api/agent/sessions/${sid}/start`, {
        method: "POST",
        credentials: "same-origin",
      })
    )
  },

  async stop(sid: number): Promise<void> {
    await okOrThrow(
      await fetch(`/api/agent/sessions/${sid}/stop`, {
        method: "POST",
        credentials: "same-origin",
      })
    )
  },

  /** Send a prompt. `streamingBehavior` is required while the agent is
   *  already streaming; the runtime rejects an unqualified prompt then. */
  async prompt(
    sid: number,
    message: string,
    streamingBehavior?: "steer" | "followUp"
  ): Promise<void> {
    await okOrThrow(
      await fetch(`/api/agent/sessions/${sid}/prompt`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ message, streaming_behavior: streamingBehavior }),
        credentials: "same-origin",
      })
    )
  },

  async abort(sid: number): Promise<void> {
    await okOrThrow(
      await fetch(`/api/agent/sessions/${sid}/abort`, {
        method: "POST",
        credentials: "same-origin",
      })
    )
  },

  /** Answer a blocking approval dialog.
   *
   *  Any connected client may answer; the server accepts only the first, so a
   *  409 here means another device already decided and is not an error worth
   *  surfacing as a failure. */
  async approve(
    sid: number,
    body: { request_id: string; confirmed?: boolean; value?: string; cancelled?: boolean }
  ): Promise<void> {
    await okOrThrow(
      await fetch(`/api/agent/sessions/${sid}/approve`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
        credentials: "same-origin",
      })
    )
  },
}

// ---------------------------------------------------------------------------
// devices
// ---------------------------------------------------------------------------

/** A machine the user has paired for local execution. */
export interface AgentDevice {
  id: number
  name: string
  platform: string | null
  last_seen_at: string | null
  revoked: boolean
  /** Whether the desktop client is connected right now. Liveness comes from
   *  an open socket, not a stored row, so a task cannot start without it. */
  online: boolean
}

export async function listDevices(): Promise<AgentDevice[]> {
  return jsonOrThrow(
    await fetch("/api/agent/devices", { credentials: "same-origin" })
  )
}

/** Issue a pairing code. The plaintext is returned exactly once. */
export async function pairDevice(name?: string): Promise<{ code: string; name: string }> {
  return jsonOrThrow(
    await fetch("/api/agent/devices/pair", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name }),
      credentials: "same-origin",
    })
  )
}

/** Revoke a machine, disconnecting it immediately. */
export async function revokeDevice(id: number): Promise<void> {
  await okOrThrow(
    await fetch(`/api/agent/devices/${id}`, {
      method: "DELETE",
      credentials: "same-origin",
    })
  )
}

// ---------------------------------------------------------------------------
// live event stream
// ---------------------------------------------------------------------------

/** A frame from the runtime, or one of the stream's own control events. */
export interface AgentStreamEvent {
  /** The runtime's event type, or "settled" | "closed" | "resync". */
  type: string
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  data: any
}

/**
 * Subscribe to a session's live events.
 *
 * Uses `EventSource` rather than a streamed POST: the subscription is a plain
 * GET, so the browser reconnects on its own. The caller still needs `onResync`
 * because a reconnect (or a subscriber that fell behind) can miss frames, and
 * the authoritative history is the server's mirror.
 */
export function subscribeAgentSession(
  sid: number,
  handlers: {
    onEvent: (e: AgentStreamEvent) => void
    onResync?: () => void
    onClosed?: (reason: string) => void
    onError?: () => void
  }
): () => void {
  const es = new EventSource(`/api/agent/sessions/${sid}/events`, {
    withCredentials: true,
  })

  // The server names each SSE event after the runtime's own event type, and
  // new pi releases add types. Listening per-name would silently drop those,
  // so take whatever arrives and let the renderer decide.
  const forward = (name: string) => (ev: MessageEvent) => {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    let data: any = ev.data
    try {
      data = JSON.parse(ev.data)
    } catch {
      /* keep raw text */
    }
    if (name === "resync") {
      handlers.onResync?.()
      return
    }
    if (name === "closed") {
      handlers.onClosed?.(typeof data?.reason === "string" ? data.reason : "运行时已退出")
      return
    }
    handlers.onEvent({ type: name, data })
  }

  const names = [
    "agent_start",
    "agent_end",
    "agent_settled",
    "turn_start",
    "turn_end",
    "message_start",
    "message_update",
    "message_end",
    "tool_execution_start",
    "tool_execution_update",
    "tool_execution_end",
    "extension_ui_request",
    "approval_resolved",
    "queue_update",
    "compaction_start",
    "compaction_end",
    "auto_retry_start",
    "auto_retry_end",
    "settled",
    "closed",
    "resync",
  ]
  for (const n of names) es.addEventListener(n, forward(n) as EventListener)
  // Unnamed frames arrive as "message".
  es.onmessage = forward("message") as (ev: MessageEvent) => void
  es.onerror = () => handlers.onError?.()

  return () => es.close()
}

// ---------------------------------------------------------------------------
// rendering helpers
// ---------------------------------------------------------------------------

/** A flattened, render-ready view of a session. */
export type AgentItem =
  | { kind: "user"; id: string; text: string }
  | { kind: "assistant"; id: string; text: string }
  | { kind: "thinking"; id: string; text: string }
  | { kind: "tool"; id: string; name: string; args: unknown; output?: string; ok?: boolean }
  | { kind: "notice"; id: string; text: string }

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function contentText(content: any): string {
  if (typeof content === "string") return content
  if (!Array.isArray(content)) return ""
  return content
    .filter((c) => c?.type === "text" && typeof c.text === "string")
    .map((c) => c.text as string)
    .join("")
}

/**
 * Convert mirrored entries into render-ready items.
 *
 * Tool calls and their results arrive as separate entries, so results are
 * merged back onto the call by `toolCallId`. Without that the UI would show a
 * command and its output as two unrelated blocks.
 */
export function entriesToItems(entries: AgentEntry[]): AgentItem[] {
  const out: AgentItem[] = []
  const toolIndex = new Map<string, number>()

  for (const e of entries) {
    if (e.type !== "message" || !e.message) continue
    const m = e.message
    if (m.role === "user") {
      const text = contentText(m.content)
      if (text.trim()) out.push({ kind: "user", id: e.id, text })
      continue
    }
    if (m.role === "assistant") {
      const blocks = Array.isArray(m.content) ? m.content : []
      for (const [i, b] of blocks.entries()) {
        if (b?.type === "text" && typeof b.text === "string") {
          if (b.text.trim()) out.push({ kind: "assistant", id: `${e.id}-${i}`, text: b.text })
        } else if (b?.type === "thinking" && typeof b.thinking === "string") {
          if (b.thinking.trim())
            out.push({ kind: "thinking", id: `${e.id}-${i}`, text: b.thinking })
        } else if (b?.type === "toolCall") {
          const id = `${e.id}-${i}`
          toolIndex.set(String(b.id), out.length)
          out.push({ kind: "tool", id, name: String(b.name ?? ""), args: b.arguments })
        }
      }
      continue
    }
    if (m.role === "toolResult") {
      const at = toolIndex.get(String(m.toolCallId))
      const output = contentText(m.content)
      if (at != null && out[at]?.kind === "tool") {
        const t = out[at] as Extract<AgentItem, { kind: "tool" }>
        out[at] = { ...t, output, ok: !m.isError }
      } else {
        // The call is missing (compacted away, or history starts mid-turn).
        // Show the output rather than dropping it.
        out.push({
          kind: "tool",
          id: e.id,
          name: String(m.toolName ?? "tool"),
          args: undefined,
          output,
          ok: !m.isError,
        })
      }
      continue
    }
  }
  return out
}

/** Human-readable label for an approval request. */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function approvalTitle(req: any): string {
  // pi's ui.confirm passes {title, message} through as `title`, so accept
  // both the flat and the nested shape.
  const t = req?.title
  if (typeof t === "string") return t
  if (t && typeof t === "object" && typeof t.title === "string") return t.title
  return "需要确认"
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function approvalMessage(req: any): string {
  const t = req?.title
  if (t && typeof t === "object" && typeof t.message === "string") return t.message
  if (typeof req?.message === "string") return req.message
  return ""
}
