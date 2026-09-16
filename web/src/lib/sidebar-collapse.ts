import { useCallback, useEffect, useState } from "react"

/**
 * Whether the navigation column is hidden.
 *
 * Doubao puts the collapse control in the main header rather than inside the
 * sidebar, so the toggle and the column are two different components and the
 * state cannot live in either of them. It is kept in `localStorage` (so the
 * column does not spring back open on every navigation) and broadcast on a
 * DOM event, which is enough for the handful of readers a single page has and
 * avoids threading a provider through pages that render the sidebar directly.
 */
const KEY = "yunova.sidebar.collapsed"
const EVENT = "yunova:sidebar-collapsed"

export function readSidebarCollapsed(): boolean {
  try {
    return window.localStorage.getItem(KEY) === "1"
  } catch {
    // Private mode: the preference is simply not remembered.
    return false
  }
}

export function setSidebarCollapsed(next: boolean): void {
  try {
    window.localStorage.setItem(KEY, next ? "1" : "0")
  } catch {
    /* ignore */
  }
  window.dispatchEvent(new CustomEvent<boolean>(EVENT, { detail: next }))
}

/** `[collapsed, toggle]`, shared by every mounted reader. */
export function useSidebarCollapsed(): [boolean, () => void] {
  const [collapsed, setCollapsed] = useState(readSidebarCollapsed)

  useEffect(() => {
    const onEvent = (e: Event) => {
      setCollapsed(Boolean((e as CustomEvent<boolean>).detail))
    }
    // Another tab of the same app is a legitimate second writer, and a stale
    // column there would disagree with its own header button.
    const onStorage = (e: StorageEvent) => {
      if (e.key === KEY) setCollapsed(e.newValue === "1")
    }
    window.addEventListener(EVENT, onEvent)
    window.addEventListener("storage", onStorage)
    return () => {
      window.removeEventListener(EVENT, onEvent)
      window.removeEventListener("storage", onStorage)
    }
  }, [])

  // Reads the stored value rather than the local one so two toggles rendered
  // on the same screen cannot drift out of phase with each other.
  const toggle = useCallback(() => {
    setSidebarCollapsed(!readSidebarCollapsed())
  }, [])

  return [collapsed, toggle]
}
