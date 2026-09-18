import { afterEach, beforeEach, describe, expect, test } from "bun:test"
import {
  directoryLabel,
  groupSidebarItems,
  readCollapsedGroups,
  writeCollapsedGroups,
  type SidebarItem,
} from "../src/lib/sidebar-groups"

const originalWindow = Object.getOwnPropertyDescriptor(globalThis, "window")

function installStorage(store: Map<string, string> | null) {
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: {
      localStorage: store
        ? {
            getItem: (k: string) => store.get(k) ?? null,
            setItem: (k: string, v: string) => void store.set(k, v),
          }
        : {
            getItem: () => {
              throw new Error("denied")
            },
            setItem: () => {
              throw new Error("denied")
            },
          },
    },
  })
  return store
}

function chat(id: number, at: string): SidebarItem {
  return { kind: "chat", id, title: `chat ${id}`, updated_at: at }
}

function task(
  id: number,
  at: string,
  workspace: string | null,
  extra: Partial<Extract<SidebarItem, { kind: "agent" }>> = {}
): SidebarItem {
  return {
    kind: "agent",
    id,
    title: `task ${id}`,
    updated_at: at,
    target: "device",
    device_id: 1,
    workspace,
    ...extra,
  }
}

afterEach(() => {
  if (originalWindow) Object.defineProperty(globalThis, "window", originalWindow)
  else Reflect.deleteProperty(globalThis, "window")
})

describe("directoryLabel", () => {
  test("names a group after its last path segment", () => {
    expect(directoryLabel("/home/u/projects/Yunova")).toBe("Yunova")
    expect(directoryLabel("C:\\work\\api")).toBe("api")
  })

  test("a trailing separator is not a nameless directory", () => {
    expect(directoryLabel("/home/u/projects/Yunova/")).toBe("Yunova")
    expect(directoryLabel("/")).toBe("/")
  })
})

describe("groupSidebarItems", () => {
  test("tasks from one directory land in one group", () => {
    const groups = groupSidebarItems([
      task(1, "2026-09-20 10:00:00", "/home/u/a"),
      task(2, "2026-09-20 09:00:00", "/home/u/b"),
      task(3, "2026-09-20 08:00:00", "/home/u/a"),
    ])
    expect(groups.map((g) => g.label)).toEqual(["a", "b"])
    expect(groups[0].items.map((i) => i.id)).toEqual([1, 3])
    // The full path is what disambiguates two projects sharing a basename,
    // so it must survive into the tooltip.
    expect(groups[0].hint).toBe("/home/u/a")
  })

  test("group order follows the most recent item, not the path", () => {
    const groups = groupSidebarItems([
      task(1, "2026-09-20 10:00:00", "/home/u/zzz"),
      task(2, "2026-09-20 09:00:00", "/home/u/aaa"),
    ])
    expect(groups.map((g) => g.label)).toEqual(["zzz", "aaa"])
  })

  test("two spellings of one directory are one group", () => {
    const groups = groupSidebarItems([
      task(1, "2026-09-20 10:00:00", "/home/u/a/"),
      task(2, "2026-09-20 09:00:00", "/home/u/a"),
    ])
    expect(groups).toHaveLength(1)
    expect(groups[0].items).toHaveLength(2)
  })

  test("chats keep their own group instead of a fabricated directory", () => {
    const groups = groupSidebarItems([
      chat(1, "2026-09-20 10:00:00"),
      task(2, "2026-09-20 09:00:00", "/home/u/a"),
      chat(3, "2026-09-20 08:00:00"),
    ])
    expect(groups.map((g) => g.kind)).toEqual(["chat", "workspace"])
    expect(groups[0].items.map((i) => i.id)).toEqual([1, 3])
  })

  test("cloud tasks and per-machine defaults do not merge", () => {
    const groups = groupSidebarItems([
      task(1, "2026-09-20 10:00:00", null, { target: "cloud", device_id: null }),
      task(2, "2026-09-20 09:00:00", null, { device_id: 7 }),
      task(3, "2026-09-20 08:00:00", null, { device_id: 9 }),
      task(4, "2026-09-20 07:00:00", null, { device_id: 7 }),
    ])
    expect(groups.map((g) => g.kind)).toEqual(["cloud", "device", "device"])
    expect(groups[1].items.map((i) => i.id)).toEqual([2, 4])
    expect(groups[2].items.map((i) => i.id)).toEqual([3])
  })

  test("an empty list has no groups rather than one empty one", () => {
    expect(groupSidebarItems([])).toEqual([])
  })
})

describe("collapsed group preference", () => {
  beforeEach(() => installStorage(new Map()))

  test("round-trips the folded keys", () => {
    writeCollapsedGroups(new Set(["ws:/home/u/a", "chat"]))
    expect([...readCollapsedGroups()].sort()).toEqual(["chat", "ws:/home/u/a"])
  })

  test("nothing is folded before the user folds anything", () => {
    expect(readCollapsedGroups().size).toBe(0)
  })

  test("a corrupted value opens every group instead of throwing", () => {
    const store = installStorage(new Map())!
    store.set("yunova.sidebar.collapsedGroups", "{not json")
    expect(readCollapsedGroups().size).toBe(0)
    store.set("yunova.sidebar.collapsedGroups", JSON.stringify({ a: 1 }))
    expect(readCollapsedGroups().size).toBe(0)
    // A hand-edited array can hold anything; only strings are keys.
    store.set("yunova.sidebar.collapsedGroups", JSON.stringify(["ok", 3, null]))
    expect([...readCollapsedGroups()]).toEqual(["ok"])
  })

  test("a storage refusal leaves the sidebar usable", () => {
    installStorage(null)
    expect(readCollapsedGroups().size).toBe(0)
    expect(() => writeCollapsedGroups(new Set(["chat"]))).not.toThrow()
  })
})
