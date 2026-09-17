// CLI sign-in approval.
//
// The browser's half of the device-code flow: a CLI shows a code, the user
// brings it here, and this page turns an authenticated session into an agent
// token for that CLI. The page never sees the token — it is minted on the
// CLI's own poll — so nothing worth stealing passes through the tab.

async function okOrThrow(res: Response): Promise<void> {
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText)
    throw new Error(text || `HTTP ${res.status}`)
  }
}

export interface CliCodeInfo {
  /** What is asking, e.g. `pi`. */
  client_name: string
  hostname: string | null
  platform: string | null
  expires_at: string
  approved: boolean
  denied: boolean
}

export const cliAuth = {
  /** Describe a pending code so the user can see what they are approving. */
  async describe(userCode: string): Promise<CliCodeInfo> {
    const res = await fetch("/api/cli/auth/code", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ user_code: userCode }),
      credentials: "same-origin",
    })
    if (!res.ok) {
      const text = await res.text().catch(() => res.statusText)
      throw new Error(text || `HTTP ${res.status}`)
    }
    return res.json() as Promise<CliCodeInfo>
  },

  async approve(userCode: string): Promise<void> {
    await okOrThrow(
      await fetch("/api/cli/auth/approve", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ user_code: userCode }),
        credentials: "same-origin",
      })
    )
  },

  async deny(userCode: string): Promise<void> {
    await okOrThrow(
      await fetch("/api/cli/auth/deny", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ user_code: userCode }),
        credentials: "same-origin",
      })
    )
  },
}

/**
 * Normalise a typed code the way the server does.
 *
 * Kept in step with `normalize_user_code` in `src/cli_auth.rs`: the user reads
 * a code off a terminal and types it here, so case and the dash must not be
 * part of the secret. Doing it client-side too means the field can show the
 * canonical form as the user types instead of failing at submit.
 */
export function normalizeUserCode(raw: string): string {
  const compact = raw
    .split("")
    .filter((c) => /[a-zA-Z0-9]/.test(c))
    .join("")
    .toUpperCase()
  return compact.length === 8 ? `${compact.slice(0, 4)}-${compact.slice(4)}` : compact
}

/** Whether a code is complete enough to submit. */
export function isCompleteCode(raw: string): boolean {
  return /^[A-Z0-9]{4}-[A-Z0-9]{4}$/.test(normalizeUserCode(raw))
}
