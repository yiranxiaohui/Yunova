import { useCallback } from "react"
import { useNavigate } from "react-router-dom"

/**
 * Chat versus work is a real behavioural split, not a cosmetic one:
 *
 * - chat has no tools and bills tokens only; it stays on the existing
 *   `/api/chat` path, which is cheaper and needs no runtime.
 * - work starts an agent runtime that can run commands, so it additionally
 *   needs an execution target and an approval story.
 *
 * The two live on separate routes because their session models differ, but
 * the *switch itself* must feel like one control in one place. Everything in
 * this module exists to hide the route change: the half-typed prompt travels
 * with the navigation, the code-split work-mode chunk is fetched before it is
 * needed, and the swap is handed to the browser's view transition so the two
 * screens cross-fade instead of blinking.
 */
export type WorkMode = "chat" | "work"

export function modePath(mode: WorkMode): string {
  return mode === "work" ? "/t" : "/"
}

/**
 * The half-typed prompt handed over by the other mode.
 *
 * Carried in the history entry rather than in storage so its lifetime is
 * exactly right without any bookkeeping: it is scoped to this one navigation,
 * survives a remount or a reload of the same entry, and is simply absent when
 * the user arrives any other way (a sidebar link, a fresh visit). A module
 * variable would instead leak the text into the next unrelated new chat.
 */
export function readModeDraft(): string {
  const draft = (
    window.history.state as { usr?: { modeDraft?: unknown } } | null
  )?.usr?.modeDraft
  return typeof draft === "string" ? draft : ""
}

/**
 * Warm the lazily-routed work workspace.
 *
 * `/t` is code-split, so an un-warmed switch renders the route's fallback —
 * the flash that makes the toggle feel like a page load. Importing the same
 * specifier the router uses reuses its chunk, so this only moves the fetch
 * earlier; it never downloads twice.
 */
let warmed = false
export function prefetchWorkMode(): void {
  if (warmed) return
  warmed = true
  void import("@/pages/AgentTaskPage").catch(() => {
    // A failed prefetch must not break the switch: the router retries the
    // import on navigation.
    warmed = false
  })
}

/** Prefetch when the browser is idle, falling back to a short timer. */
export function prefetchWorkModeWhenIdle(): () => void {
  const w = window as unknown as {
    requestIdleCallback?: (cb: () => void) => number
    cancelIdleCallback?: (h: number) => void
  }
  if (w.requestIdleCallback) {
    const h = w.requestIdleCallback(() => prefetchWorkMode())
    return () => w.cancelIdleCallback?.(h)
  }
  const t = window.setTimeout(prefetchWorkMode, 1200)
  return () => window.clearTimeout(t)
}

/**
 * Switch modes from either side.
 *
 * Deliberately a no-op when the mode is unchanged: the segmented control is
 * also the current-state indicator, so tapping the active half must not push
 * a history entry or disturb a running session.
 *
 * No "last mode" is persisted on purpose. The route already records which
 * mode a session is in, and silently reopening the app in work mode would
 * change where the next message executes and what it costs.
 */
export function useModeSwitch(current: WorkMode) {
  const nav = useNavigate()
  return useCallback(
    (next: WorkMode, draft = "") => {
      if (next === current) return
      if (next === "work") prefetchWorkMode()
      // `viewTransition` is ignored where the API is missing, so the switch
      // degrades to an instant swap rather than breaking.
      nav(modePath(next), {
        state: draft.trim() ? { modeDraft: draft } : undefined,
        viewTransition: true,
      })
    },
    [current, nav]
  )
}
