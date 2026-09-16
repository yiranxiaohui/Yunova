import { afterEach, beforeEach, describe, expect, test } from "bun:test"
import {
  readSidebarCollapsed,
  setSidebarCollapsed,
} from "../src/lib/sidebar-collapse"

/** The toggle lives in the page header while the column that reacts to it is a
 *  separate component, so the preference is read from storage and announced on
 *  a DOM event. Both halves are stubbed here. */
const originalWindow = Object.getOwnPropertyDescriptor(globalThis, "window")

type Listener = (e: Event) => void

function installWindow(store: Map<string, string> | null) {
  const listeners = new Map<string, Listener[]>()
  const fake = {
    localStorage: store
      ? {
          getItem: (k: string) => store.get(k) ?? null,
          setItem: (k: string, v: string) => void store.set(k, v),
        }
      : {
          // Private mode: storage exists but refuses to answer.
          getItem: () => {
            throw new Error("denied")
          },
          setItem: () => {
            throw new Error("denied")
          },
        },
    addEventListener: (type: string, fn: Listener) => {
      listeners.set(type, [...(listeners.get(type) ?? []), fn])
    },
    removeEventListener: () => {},
    dispatchEvent: (e: Event) => {
      for (const fn of listeners.get(e.type) ?? []) fn(e)
      return true
    },
  }
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: fake,
  })
  return listeners
}

beforeEach(() => {
  installWindow(new Map())
})

afterEach(() => {
  if (originalWindow) Object.defineProperty(globalThis, "window", originalWindow)
  else Reflect.deleteProperty(globalThis, "window")
})

describe("sidebar collapse preference", () => {
  test("starts expanded and remembers both directions", () => {
    expect(readSidebarCollapsed()).toBe(false)
    setSidebarCollapsed(true)
    expect(readSidebarCollapsed()).toBe(true)
    setSidebarCollapsed(false)
    expect(readSidebarCollapsed()).toBe(false)
  })

  test("announces every change so the header and the column agree", () => {
    const listeners = installWindow(new Map())
    const seen: boolean[] = []
    listeners.set("yunova:sidebar-collapsed", [
      (e) => seen.push(Boolean((e as CustomEvent<boolean>).detail)),
    ])
    setSidebarCollapsed(true)
    setSidebarCollapsed(false)
    expect(seen).toEqual([true, false])
  })

  test("a storage refusal leaves the UI usable rather than throwing", () => {
    installWindow(null)
    expect(readSidebarCollapsed()).toBe(false)
    expect(() => setSidebarCollapsed(true)).not.toThrow()
    // The preference is simply not remembered, which is the acceptable
    // outcome: the toggle still works for the current page.
    expect(readSidebarCollapsed()).toBe(false)
  })
})
