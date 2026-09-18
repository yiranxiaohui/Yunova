import type { AgentTarget } from "./agent"

/**
 * Grouping the sidebar's recent list by the directory a task ran in.
 *
 * Work-mode tasks are tied to a directory on a machine, and a user who works
 * on three projects ends up with three interleaved streams of tasks in one
 * flat, time-ordered list — the same title ("hi", "修 bug") appearing once per
 * project with nothing to tell them apart. Grouping by workspace restores the
 * one piece of context that distinguishes them, without asking the user to
 * name or file anything.
 *
 * Chats have no directory, so they keep a group of their own rather than being
 * forced under a fake one.
 */

export type SidebarItem =
  | { kind: "chat"; id: number; title: string; updated_at: string }
  | {
      kind: "agent"
      id: number
      title: string
      updated_at: string
      target: AgentTarget
      /** Directory on the device. Null means the runtime's default. */
      workspace: string | null
      device_id: number | null
    }

export interface SidebarGroup {
  /** Stable identity, used as the React key and as the collapse preference. */
  key: string
  label: string
  /** Shown as the row's tooltip: the full path, which the label truncates. */
  hint: string
  /** What the header's icon should say this group is. */
  kind: "chat" | "workspace" | "cloud" | "device"
  items: SidebarItem[]
}

/** Last path segment, for both POSIX and Windows paths.
 *
 *  Trailing separators are dropped first, so `/home/u/app/` and `/home/u/app`
 *  are the same directory rather than two groups, one of them unlabelled. */
export function directoryLabel(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "")
  if (!trimmed) return path.startsWith("/") ? "/" : path
  const parts = trimmed.split(/[\\/]/)
  const last = parts[parts.length - 1]
  return last || trimmed
}

/** Two spellings of the same directory must not become two groups. */
function normalizeWorkspace(path: string): string {
  const trimmed = path.trim().replace(/[\\/]+$/, "")
  return trimmed || path.trim()
}

function keyOf(item: SidebarItem): string {
  if (item.kind === "chat") return "chat"
  if (item.workspace) return `ws:${normalizeWorkspace(item.workspace)}`
  // A cloud task's directory lives on the server and is per-session, so there
  // is nothing to group it by other than "cloud". Device tasks without a
  // chosen path share that machine's default directory, which is one place —
  // but a different one per machine, hence the id in the key.
  return item.target === "cloud" ? "cloud" : `device:${item.device_id ?? "?"}`
}

function describe(item: SidebarItem, key: string): Pick<SidebarGroup, "label" | "hint" | "kind"> {
  if (item.kind === "chat") {
    return { label: "对话", hint: "普通对话", kind: "chat" }
  }
  if (key.startsWith("ws:")) {
    const path = key.slice(3)
    return { label: directoryLabel(path), hint: path, kind: "workspace" }
  }
  if (key === "cloud") {
    return { label: "云端任务", hint: "运行在云电脑上的任务", kind: "cloud" }
  }
  return {
    label: "默认目录",
    hint: "本地电脑上未指定目录的任务",
    kind: "device",
  }
}

/**
 * Split a time-ordered list into groups, newest group first.
 *
 * Group order follows the newest item inside it rather than a fixed ranking,
 * so whatever the user touched last stays at the top — the property that made
 * the flat list worth using in the first place. Items keep the order they
 * arrived in, which the caller has already sorted by recency.
 */
export function groupSidebarItems(items: SidebarItem[]): SidebarGroup[] {
  const groups = new Map<string, SidebarGroup>()
  for (const item of items) {
    const key = keyOf(item)
    const existing = groups.get(key)
    if (existing) {
      existing.items.push(item)
    } else {
      groups.set(key, { key, ...describe(item, key), items: [item] })
    }
  }
  return [...groups.values()]
}

const COLLAPSED_KEY = "yunova.sidebar.collapsedGroups"

/** Which groups the user folded away, remembered across navigations. */
export function readCollapsedGroups(): Set<string> {
  try {
    const raw = window.localStorage.getItem(COLLAPSED_KEY)
    if (!raw) return new Set()
    const parsed: unknown = JSON.parse(raw)
    if (!Array.isArray(parsed)) return new Set()
    return new Set(parsed.filter((x): x is string => typeof x === "string"))
  } catch {
    // Private mode or a hand-edited value: every group simply starts open.
    return new Set()
  }
}

export function writeCollapsedGroups(keys: Set<string>): void {
  try {
    window.localStorage.setItem(COLLAPSED_KEY, JSON.stringify([...keys]))
  } catch {
    /* ignore */
  }
}
