import type { User } from "./api"

async function jsonOrThrow<T>(res: Response): Promise<T> {
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText)
    throw new Error(text || `HTTP ${res.status}`)
  }
  return res.json() as Promise<T>
}

export type ProfileUpdate = {
  display_name?: string
}

export const profileApi = {
  async get(): Promise<User> {
    const res = await fetch("/api/profile", { credentials: "same-origin" })
    return jsonOrThrow<User>(res)
  },
  async update(updates: ProfileUpdate): Promise<User> {
    const res = await fetch("/api/profile", {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(updates),
      credentials: "same-origin",
    })
    return jsonOrThrow<User>(res)
  },
  async uploadAvatar(file: File): Promise<User> {
    const res = await fetch("/api/profile/avatar", {
      method: "POST",
      headers: { "Content-Type": file.type || "application/octet-stream" },
      body: file,
      credentials: "same-origin",
    })
    return jsonOrThrow<User>(res)
  },
  async changePassword(
    current_password: string,
    new_password: string
  ): Promise<void> {
    const res = await fetch("/api/profile/password", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ current_password, new_password }),
      credentials: "same-origin",
    })
    if (!res.ok) {
      const text = await res.text().catch(() => res.statusText)
      throw new Error(text || `HTTP ${res.status}`)
    }
  },
}
