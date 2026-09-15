export type Conversation = {
  id: number
  title: string
  system_prompt: string
  created_at: string
  updated_at: string
}

export type StoredMessage = {
  id: number
  role: "system" | "user" | "assistant"
  content: string
  /** 推理模型的思考过程，仅 assistant 消息可能有值。 */
  reasoning?: string | null
  /** 思考耗时（毫秒）。 */
  reasoning_ms?: number | null
  created_at: string
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

export const conversationsApi = {
  async list(): Promise<Conversation[]> {
    return jsonOrThrow(
      await fetch("/api/conversations", { credentials: "same-origin" })
    )
  },
  async create(opts: { title?: string; system_prompt?: string } = {}): Promise<Conversation> {
    return jsonOrThrow(
      await fetch("/api/conversations", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(opts),
        credentials: "same-origin",
      })
    )
  },
  async update(
    id: number,
    patch: { title?: string; system_prompt?: string }
  ): Promise<void> {
    await okOrThrow(
      await fetch(`/api/conversations/${id}`, {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(patch),
        credentials: "same-origin",
      })
    )
  },
  async remove(id: number): Promise<void> {
    await okOrThrow(
      await fetch(`/api/conversations/${id}`, {
        method: "DELETE",
        credentials: "same-origin",
      })
    )
  },
  async removeAll(): Promise<{ deleted: number }> {
    return jsonOrThrow(
      await fetch("/api/conversations", {
        method: "DELETE",
        credentials: "same-origin",
      })
    )
  },
  async messages(id: number): Promise<StoredMessage[]> {
    return jsonOrThrow(
      await fetch(`/api/conversations/${id}/messages`, {
        credentials: "same-origin",
      })
    )
  },
  async append(
    id: number,
    messages: Array<{
      role: "system" | "user" | "assistant"
      content: string
      reasoning?: string
      reasoning_ms?: number
    }>
  ): Promise<void> {
    await okOrThrow(
      await fetch(`/api/conversations/${id}/messages`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ messages }),
        credentials: "same-origin",
      })
    )
  },
  async truncate(id: number, fromMessageId: number): Promise<void> {
    await okOrThrow(
      await fetch(
        `/api/conversations/${id}/messages?from=${fromMessageId}`,
        {
          method: "DELETE",
          credentials: "same-origin",
        }
      )
    )
  },
}
