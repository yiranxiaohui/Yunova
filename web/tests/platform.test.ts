import { afterEach, describe, expect, test } from "bun:test"
import { capabilities, platform } from "../src/lib/platform"

/** The page runs in three hosts from one bundle, and the only evidence of
 *  which one is a global the host injected. Each test installs that evidence
 *  and removes it again, because leaking it would make every later test look
 *  like it runs inside the desktop client. */
const KEYS = ["__YUNOVA_DESKTOP__", "__TAURI_INTERNALS__", "isTauri", "Capacitor"] as const

function set(key: (typeof KEYS)[number], value: unknown) {
  Object.defineProperty(globalThis, key, { configurable: true, value })
}

afterEach(() => {
  for (const k of KEYS) Reflect.deleteProperty(globalThis, k)
})

describe("where the page is running", () => {
  test("a plain browser is web, and web is the only place worth offering an install", () => {
    expect(platform()).toBe("web")
    expect(capabilities().canInstallDesktop).toBe(true)
    expect(capabilities().canExecuteLocally).toBe(false)
  })

  test("the desktop client is recognised from the marker it injects", () => {
    // What the shell actually sets; see `DESKTOP_MARKER` in shell.rs.
    set("__YUNOVA_DESKTOP__", true)
    expect(platform()).toBe("desktop")
  })

  test("…and from Tauri's own globals, for clients that predate the marker", () => {
    // An installed copy does not update in step with the server, so a page
    // served to an older client must still recognise it.
    set("__TAURI_INTERNALS__", { plugins: {} })
    expect(platform()).toBe("desktop")
    Reflect.deleteProperty(globalThis, "__TAURI_INTERNALS__")
    set("isTauri", true)
    expect(platform()).toBe("desktop")
  })

  test("the desktop client never offers its own download", () => {
    set("__YUNOVA_DESKTOP__", true)
    const caps = capabilities()
    expect(caps.canInstallDesktop).toBe(false)
    // The same host is the one that can run a task here; that is why the
    // install prompt is redundant rather than merely untidy.
    expect(caps.canExecuteLocally).toBe(true)
  })

  test("the packaged mobile app offers neither install nor local execution", () => {
    set("Capacitor", {
      getPlatform: () => "ios",
      isNativePlatform: () => true,
    })
    const caps = capabilities()
    expect(caps.platform).toBe("ios")
    expect(caps.canInstallDesktop).toBe(false)
    expect(caps.canExecuteLocally).toBe(false)
  })

  test("a truthy-looking marker from the page itself is not enough", () => {
    // Guard against a stray global (an analytics shim, a userscript) hiding
    // the download for ordinary browser users.
    set("__YUNOVA_DESKTOP__", "yes")
    expect(platform()).toBe("web")
    set("isTauri", "yes")
    expect(platform()).toBe("web")
  })
})
