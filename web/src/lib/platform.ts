// Platform adapter.
//
// The same React bundle runs in a browser, inside the desktop shell, and
// inside the packaged mobile app. Capabilities differ — only the packaged app
// has push notifications, only a browser has a URL bar — and scattering those
// checks through components would make every feature aware of every platform.
//
// So platform differences are resolved here, once, behind a capability query.
// Components ask "can I notify?" rather than "am I on iOS?", which keeps them
// honest when a capability arrives on a platform that previously lacked it.
//
// Loading is deliberately lazy and failure-tolerant: the web build must not
// depend on a native bridge being present, and a missing plugin degrades to
// "capability unavailable" rather than breaking the page.

export type Platform = "web" | "ios" | "android" | "desktop"

/** Minimal shape of the Capacitor global the native shell injects. */
interface CapacitorGlobal {
  getPlatform?: () => string
  isNativePlatform?: () => boolean
  Plugins?: Record<string, unknown>
}

function capacitor(): CapacitorGlobal | undefined {
  return (globalThis as { Capacitor?: CapacitorGlobal }).Capacitor
}

/** Whether the code is running inside the packaged native shell. */
export function isNative(): boolean {
  const cap = capacitor()
  return typeof cap?.isNativePlatform === "function" ? cap.isNativePlatform() : false
}

export function platform(): Platform {
  const cap = capacitor()
  const raw = cap?.getPlatform?.()
  if (raw === "ios" || raw === "android") return raw
  // The desktop shell announces itself with a marker injected into the site
  // webview (`DESKTOP_MARKER` in `desktop/src/shell.rs`). That webview is
  // named in no Tauri capability, so it cannot ask the shell anything over
  // IPC; a read-only global is the whole channel, and one bit is all this
  // needs. `__TAURI_INTERNALS__` is accepted too, for the panel window and
  // for older clients that predate the marker.
  const g = globalThis as {
    __YUNOVA_DESKTOP__?: unknown
    __TAURI_INTERNALS__?: unknown
    isTauri?: unknown
  }
  if (g.__YUNOVA_DESKTOP__ === true || g.isTauri === true || g.__TAURI_INTERNALS__) {
    return "desktop"
  }
  return "web"
}

/**
 * What this platform can do.
 *
 * `canExecuteLocally` is the one that carries product meaning: a phone is a
 * remote control, not an execution target, so the task UI must not offer to
 * run work "here" when it cannot.
 */
export interface Capabilities {
  platform: Platform
  native: boolean
  /** Push notifications, so an approval can reach a user who is elsewhere. */
  canNotify: boolean
  /** Whether tasks can run on this device itself. */
  canExecuteLocally: boolean
  /**
   * Whether offering the desktop client still makes sense here.
   *
   * It does not inside the desktop client itself — the user already installed
   * it — and it does not in the packaged mobile app, where a store build must
   * not link out to a binary download.
   */
  canInstallDesktop: boolean
  /** Whether a back gesture/button needs explicit handling. */
  needsBackHandling: boolean
}

export function capabilities(): Capabilities {
  const p = platform()
  const native = isNative()
  return {
    platform: p,
    native,
    // Browsers have the Notification API but it is unreliable for a
    // backgrounded tab on iOS; only claim it in the packaged app.
    canNotify: native,
    // Only the desktop shell hosts a local runtime. A phone drives tasks that
    // run in the cloud or on a paired computer.
    canExecuteLocally: p === "desktop",
    // Only a browser can act on a desktop download: inside the client the app
    // is already installed, and a packaged mobile build must not link out to a
    // binary.
    canInstallDesktop: p === "web",
    needsBackHandling: p === "android",
  }
}

// ---------------------------------------------------------------------------
// push notifications
// ---------------------------------------------------------------------------

interface PushPlugin {
  requestPermissions: () => Promise<{ receive: string }>
  register: () => Promise<void>
  addListener: (
    event: string,
    handler: (data: unknown) => void
  ) => Promise<{ remove: () => Promise<void> }>
}

interface LocalNotificationsPlugin {
  requestPermissions: () => Promise<{ display: string }>
  schedule: (opts: {
    notifications: Array<{ id: number; title: string; body: string }>
  }) => Promise<void>
}

function plugin<T>(name: string): T | undefined {
  return capacitor()?.Plugins?.[name] as T | undefined
}

/**
 * Ask for notification permission.
 *
 * Returns false rather than throwing when unavailable: a denied or missing
 * permission must degrade the feature, never break the screen the user is on.
 */
export async function requestNotificationPermission(): Promise<boolean> {
  if (!isNative()) return false
  try {
    const local = plugin<LocalNotificationsPlugin>("LocalNotifications")
    if (local) {
      const r = await local.requestPermissions()
      return r.display === "granted"
    }
    const push = plugin<PushPlugin>("PushNotifications")
    if (push) {
      const r = await push.requestPermissions()
      if (r.receive !== "granted") return false
      await push.register()
      return true
    }
  } catch (e) {
    console.warn("[platform] notification permission unavailable:", e)
  }
  return false
}

/**
 * Surface an approval request while the app is not in the foreground.
 *
 * This is why the mobile app is worth packaging at all: a blocked agent is
 * useless if the user never learns it is waiting. Local notifications are used
 * rather than server push because the event originates from a socket this
 * client already holds — no server-side push infrastructure is required.
 */
export async function notifyApprovalPending(title: string, body: string): Promise<void> {
  if (!isNative()) return
  try {
    const local = plugin<LocalNotificationsPlugin>("LocalNotifications")
    if (!local) return
    await local.schedule({
      notifications: [
        {
          // A fixed id collapses repeats instead of stacking one per frame.
          id: 1,
          title,
          body: body.slice(0, 160),
        },
      ],
    })
  } catch (e) {
    console.warn("[platform] could not post a notification:", e)
  }
}

// ---------------------------------------------------------------------------
// app lifecycle
// ---------------------------------------------------------------------------

interface AppPlugin {
  addListener: (
    event: string,
    handler: (data: { isActive?: boolean; canGoBack?: boolean }) => void
  ) => Promise<{ remove: () => Promise<void> }>
  exitApp?: () => Promise<void>
}

/**
 * Observe foreground/background transitions.
 *
 * The task view needs this: a phone suspends timers and can drop a socket
 * while backgrounded, so the transcript must be reconciled from the server on
 * resume rather than assumed intact.
 */
export async function onAppStateChange(
  handler: (active: boolean) => void
): Promise<() => void> {
  if (!isNative()) {
    // The browser equivalent, so callers need no platform branch.
    const onVisibility = () => handler(document.visibilityState === "visible")
    document.addEventListener("visibilitychange", onVisibility)
    return () => document.removeEventListener("visibilitychange", onVisibility)
  }
  try {
    const app = plugin<AppPlugin>("App")
    if (!app) return () => {}
    const sub = await app.addListener("appStateChange", (s) => handler(s.isActive === true))
    return () => void sub.remove()
  } catch {
    return () => {}
  }
}

/**
 * Handle the Android hardware back button.
 *
 * Without this, back exits the app from any screen, which loses the user's
 * place in a running task.
 */
export async function onHardwareBack(handler: () => boolean): Promise<() => void> {
  if (platform() !== "android") return () => {}
  try {
    const app = plugin<AppPlugin>("App")
    if (!app) return () => {}
    const sub = await app.addListener("backButton", () => {
      // The handler reports whether it consumed the event; only exit when
      // there is nowhere left to go back to.
      if (!handler()) void app.exitApp?.()
    })
    return () => void sub.remove()
  } catch {
    return () => {}
  }
}
