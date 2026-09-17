export type SetupStatus = {
  installed: boolean
  supported: string[]
}

export type ConnectionForm =
  | {
      kind: "sqlite"
      sqlite_path: string
    }
  | {
      kind: "postgres"
      host: string
      port: number
      user: string
      password: string
      database: string
      tls: boolean
    }

async function okOrThrow(res: Response): Promise<void> {
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText)
    throw new Error(text || `HTTP ${res.status}`)
  }
}

async function jsonOrThrow<T>(res: Response): Promise<T> {
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText)
    throw new Error(text || `HTTP ${res.status}`)
  }
  return res.json() as Promise<T>
}

export const setupApi = {
  async status(): Promise<SetupStatus> {
    return jsonOrThrow(await fetch("/api/setup/status"))
  },
  async test(conn: ConnectionForm): Promise<void> {
    await okOrThrow(
      await fetch("/api/setup/test", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(conn),
      })
    )
  },
  async install(payload: {
    connection: ConnectionForm
    admin_username: string
    admin_password: string
  }): Promise<{ ok: boolean; admin_id: number }> {
    return jsonOrThrow(
      await fetch("/api/setup/install", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
      })
    )
  },
}
