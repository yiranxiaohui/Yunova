/**
 * Yunova as a pi provider.
 *
 * This is what makes `pi` a Yunova CLI in the same sense that Claude Code and
 * Codex CLI are clients of their own platforms: `/login yunova` signs in, and
 * the account's models then appear in `/model` and bill through the platform's
 * own gateway.
 *
 * The design follows from one constraint: the CLI runs on the user's computer,
 * so it must never hold anything worth stealing beyond its own revocable
 * credential.
 *
 * * **No password.** Login is a device code — the CLI shows a short code, the
 *   user approves it in a browser that is already signed in, and the CLI polls
 *   until a scoped agent token exists. The account credential never enters a
 *   terminal or a shell history.
 * * **No upstream keys.** The token addresses Yunova's own `/api/proxy/*`
 *   gateway, so the model whitelist, channel failover and token metering all
 *   still apply. A user who reads the token out of `auth.json` gains only what
 *   their own account already allows, and the token can be revoked alone.
 * * **No hardcoded catalog.** Models come from `/api/cli/models` at refresh
 *   time, because the platform's model list is an admin decision that changes
 *   without anyone reinstalling this package.
 *
 * The provider is registered per protocol (`yunova-openai`, `yunova-claude`)
 * rather than as one provider, because pi selects the request shape from the
 * provider's `api` and the gateway speaks both. Registering one provider would
 * force every model through a single wire format and break the other half.
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent"
import type {
  OAuthCredentials,
  OAuthLoginCallbacks,
} from "@earendil-works/pi-ai"

/** Where the site lives, when the user has not said otherwise. */
const DEFAULT_BASE_URL = "https://chat.yunnet.top"

/**
 * Environment override for self-hosted instances.
 *
 * Read at login *and* at refresh, so pointing the CLI at another deployment
 * does not require editing a stored credential by hand.
 */
const BASE_URL_ENV = "YUNOVA_BASE_URL"

/** How long to keep polling before giving up, matching the server's code TTL. */
const LOGIN_TIMEOUT_MS = 10 * 60 * 1000

/** Fallback poll spacing when the server does not state one. */
const DEFAULT_POLL_MS = 3000

/**
 * The gateway protocols this package exposes, and how each maps to pi.
 *
 * Kept in step with `RUNTIME_PROVIDERS` in `src/agent_token.rs`: the server
 * decides which protocols an agent may use, and a provider registered here for
 * a protocol the gateway does not serve would show models that fail on first
 * use.
 */
const PROTOCOLS = [
  {
    protocol: "openai",
    providerId: "yunova-openai",
    label: "Yunova（OpenAI 协议）",
    api: "openai-responses",
    path: "/api/proxy/openai",
  },
  {
    protocol: "claude",
    providerId: "yunova-claude",
    label: "Yunova（Claude 协议）",
    api: "anthropic-messages",
    path: "/api/proxy/claude",
  },
] as const

interface PlatformModel {
  id: string
  name?: string
  protocol: string
  contextWindow?: number
}

function baseUrl(): string {
  const raw = process.env[BASE_URL_ENV]?.trim()
  return (raw && raw.length > 0 ? raw : DEFAULT_BASE_URL).replace(/\/+$/, "")
}

function platform(): string {
  return `${process.platform}-${process.arch}`
}

async function hostname(): Promise<string> {
  try {
    const os = await import("node:os")
    return os.hostname()
  } catch {
    return "unknown"
  }
}

async function readJson(res: Response): Promise<Record<string, unknown>> {
  const text = await res.text()
  try {
    return JSON.parse(text) as Record<string, unknown>
  } catch {
    // The gateway answers errors as plain text, so a parse failure here is an
    // error body rather than a bug. Surfacing the text is what makes a
    // misconfigured base URL diagnosable instead of "unexpected token <".
    throw new Error(text.slice(0, 200) || `HTTP ${res.status}`)
  }
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

/**
 * Run the device-code flow to completion.
 *
 * `access` carries the agent token and `refresh` is set to the same value:
 * pi's credential shape expects both, and a Yunova token is not refreshable —
 * it is revoked and replaced rather than rotated. `expires` is what the server
 * reported, so pi stops using a credential the gateway would reject anyway.
 */
async function login(callbacks: OAuthLoginCallbacks): Promise<OAuthCredentials> {
  const base = baseUrl()
  const started = await fetch(`${base}/api/cli/auth/start`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      client_name: "pi",
      hostname: await hostname(),
      platform: platform(),
    }),
    signal: callbacks.signal,
  })
  if (!started.ok) {
    throw new Error(
      `无法连接 Yunova（${base}）：${(await started.text()).slice(0, 200)}`
    )
  }
  const body = await readJson(started)
  const deviceCode = String(body.device_code ?? "")
  const userCode = String(body.user_code ?? "")
  if (!deviceCode || !userCode) throw new Error("服务端未返回登录码")

  const verificationUri = `${base}/cli/login`
  const intervalSeconds = Number(body.interval ?? DEFAULT_POLL_MS / 1000)
  const expiresIn = Number(body.expires_in ?? LOGIN_TIMEOUT_MS / 1000)

  // Both are shown: the URL with the code embedded is one click for anyone on
  // a desktop, and the bare code is what someone on a headless box types into
  // their phone.
  callbacks.onDeviceCode({
    userCode,
    verificationUri: `${verificationUri}?code=${encodeURIComponent(userCode)}`,
    intervalSeconds,
    expiresInSeconds: expiresIn,
  })

  const deadline = Date.now() + Math.min(expiresIn * 1000, LOGIN_TIMEOUT_MS)
  let waitMs = Math.max(intervalSeconds * 1000, 1000)

  while (Date.now() < deadline) {
    callbacks.signal?.throwIfAborted()
    await sleep(waitMs)

    const res = await fetch(`${base}/api/cli/auth/poll`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ device_code: deviceCode }),
      signal: callbacks.signal,
    })
    if (res.status === 429) {
      // Backing off rather than failing: the limiter is shared with other auth
      // traffic, so a busy moment must not end an otherwise valid login.
      waitMs = Math.min(waitMs * 2, 30_000)
      continue
    }
    const payload = await readJson(res)
    const status = String(payload.status ?? "")

    if (status === "authorization_pending") {
      callbacks.onProgress?.("等待在浏览器中确认…")
      continue
    }
    if (status === "access_denied") throw new Error("登录已被拒绝")
    if (status === "expired_token") {
      throw new Error("登录码已过期，请重新运行 /login yunova")
    }
    if (status === "ok") {
      const token = String(payload.token ?? "")
      if (!token) throw new Error("服务端未返回令牌")
      const ttlSeconds = Number(payload.expires_in ?? 0)
      const who = payload.username ? `（${String(payload.username)}）` : ""
      callbacks.onProgress?.(`已登录 Yunova${who}`)
      return {
        access: token,
        // Not refreshable by design; stored so pi's credential shape is
        // satisfied and a future rotation has somewhere to live.
        refresh: token,
        expires:
          ttlSeconds > 0 ? Date.now() + ttlSeconds * 1000 : Date.now() + 30 * 86_400_000,
        baseUrl: base,
      }
    }
    throw new Error(`未知的登录状态：${status}`)
  }
  throw new Error("登录超时，请重新运行 /login yunova")
}

/**
 * Fetch the account's available models for one protocol.
 *
 * Returns an empty list instead of throwing when the catalog cannot be read:
 * a network blip at startup should leave `/model` without Yunova entries, not
 * break the session the user is already in.
 */
async function fetchModels(
  protocol: string,
  credential: unknown,
  signal: AbortSignal
): Promise<PlatformModel[]> {
  const token = tokenOf(credential)
  if (!token) return []
  const base = storedBaseUrl(credential) ?? baseUrl()
  try {
    const res = await fetch(`${base}/api/cli/models`, {
      headers: { authorization: `Bearer ${token}` },
      signal,
    })
    if (!res.ok) return []
    const body = (await res.json()) as { models?: PlatformModel[] }
    return (body.models ?? []).filter((m) => m.protocol === protocol)
  } catch {
    return []
  }
}

/** The bearer token inside whatever pi handed us, if there is one. */
function tokenOf(credential: unknown): string | undefined {
  if (!credential || typeof credential !== "object") return undefined
  const c = credential as Record<string, unknown>
  const access = typeof c.access === "string" ? c.access : undefined
  const key = typeof c.key === "string" ? c.key : undefined
  return access ?? key
}

/**
 * The site this credential was minted against.
 *
 * Stored at login so a user with one self-hosted instance and one public
 * account does not have to keep an environment variable set correctly for the
 * rest of time.
 */
function storedBaseUrl(credential: unknown): string | undefined {
  if (!credential || typeof credential !== "object") return undefined
  const raw = (credential as Record<string, unknown>).baseUrl
  return typeof raw === "string" && raw.length > 0 ? raw.replace(/\/+$/, "") : undefined
}

export default function (pi: ExtensionAPI) {
  for (const entry of PROTOCOLS) {
    pi.registerProvider(entry.providerId, {
      name: entry.label,
      baseUrl: `${baseUrl()}${entry.path}`,
      api: entry.api,
      // No `models` here on purpose: the catalog is an admin decision on the
      // server, so a list compiled into this package would go stale the moment
      // a model is enabled or priced differently.
      models: [],
      async refreshModels(context) {
        const models = await fetchModels(
          entry.protocol,
          context.credential,
          context.signal
        )
        return models.map((m) => ({
          id: m.id,
          name: m.name ?? m.id,
          reasoning: true,
          input: ["text", "image"] as ("text" | "image")[],
          // Billing is the platform's, and reporting a second price here would
          // only ever disagree with the ledger the user is actually charged
          // against.
          cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
          contextWindow: m.contextWindow ?? 200_000,
          maxTokens: 32_000,
        }))
      },
      oauth: {
        name: "Yunova（浏览器确认登录）",
        // The quota behind it is a platform balance, not a per-token API key
        // the user tops up at a vendor, which is what this flag describes.
        isSubscription: true,
        login,
        async refreshToken(credentials, signal) {
          signal.throwIfAborted()
          // A Yunova token is revoked and re-issued, never rotated in place.
          // Returning it unchanged keeps pi from inventing a refresh cycle
          // that the server has no endpoint for; when it does expire, the
          // gateway rejects it and the user runs /login again.
          return credentials
        },
        getApiKey(credentials) {
          return credentials.access
        },
      },
    })
  }
}
