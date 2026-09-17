import { describe, expect, test } from "bun:test"

/** Safe-area padding and the two chat surfaces that depend on it.
 *
 *  Work mode's header used to sit ~5px higher than chat's and its composer sat
 *  flush against the window's bottom edge, purely because `AgentTaskPage`
 *  carried `safe-top` / `safe-bottom` and `ChatPage` did not. The helpers were
 *  plain `padding-top: env(...)` rules declared *outside* `@layer utilities`,
 *  so on a desktop they resolved to `0px` and, being unlayered, still beat the
 *  element's own `py-2` / `pb-3`. The class did not pad the element — it
 *  deleted the padding it already had.
 *
 *  These assertions are on the source rather than on a rendered layout because
 *  the bug was entirely one of CSS authoring: the cascade-layer placement and
 *  the add-versus-replace shape of the declaration. A DOM test would need a
 *  real engine resolving `env()` to catch either. */

const css = await Bun.file(new URL("../src/index.css", import.meta.url)).text()
const chat = await Bun.file(
  new URL("../src/pages/ChatPage.tsx", import.meta.url)
).text()
const work = await Bun.file(
  new URL("../src/pages/AgentTaskPage.tsx", import.meta.url)
).text()

/** The `class="..."` value of the first tag matching `pattern`. */
function classesOf(source: string, pattern: RegExp): string {
  const tag = source.match(pattern)
  expect(tag).not.toBeNull()
  const cls = tag![0].match(/className="([^"]*)"/)
  expect(cls).not.toBeNull()
  return cls![1]!
}

const chatHeader = () => classesOf(chat, /<header\b[^>]*>/)
const chatComposer = () =>
  classesOf(chat, /<div className="[^"]*safe-bottom[^"]*">/)
const workHeader = () =>
  classesOf(work, /<div className="safe-top[^"]*min-h-14[^"]*">/)
const workComposer = () =>
  classesOf(work, /<div className="safe-bottom[^"]*">/)

describe("safe-area helpers", () => {
  test("are declared in the utilities layer, not above it", () => {
    // `@utility` is Tailwind v4's way in; a bare `.safe-top { }` is unlayered
    // and therefore outranks every Tailwind utility regardless of order.
    expect(css).toContain("@utility safe-top")
    expect(css).toContain("@utility safe-bottom")
    expect(css).not.toMatch(/^\.safe-(top|bottom)\s*\{/m)
  })

  test("add the inset to the element's own padding instead of replacing it", () => {
    for (const [side, prop] of [
      ["top", "padding-top"],
      ["bottom", "padding-bottom"],
    ] as const) {
      const block = css.match(
        new RegExp(`@utility safe-${side}\\s*\\{[^}]*\\}`)
      )?.[0]
      expect(block).toBeDefined()
      expect(block).toContain(prop)
      expect(block).toContain("calc(")
      expect(block).toContain(`--safe-area-extra-${side}`)
      expect(block).toContain(`env(safe-area-inset-${side}`)
    }
  })

  test("collapse to the caller's spacing where there is no inset", () => {
    // The fallback is what makes the desktop case correct: no notch means the
    // `env()` term is 0 and only `--safe-area-extra-*` remains.
    expect(css).toMatch(/--safe-area-extra-top,\s*0px/)
    expect(css).toMatch(/--safe-area-extra-bottom,\s*0px/)
    expect(css).toMatch(/env\(safe-area-inset-top,\s*0px\)/)
    expect(css).toMatch(/env\(safe-area-inset-bottom,\s*0px\)/)
  })
})

describe("chat and work mode line up", () => {
  test("both headers reserve the top inset", () => {
    // Chat lacking `safe-top` was half the original discrepancy, and in the
    // packaged app it also put the header under the status bar.
    expect(chatHeader()).toContain("safe-top")
    expect(workHeader()).toContain("safe-top")
  })

  test("both composers reserve the bottom inset", () => {
    expect(chatComposer()).toContain("safe-bottom")
    expect(workComposer()).toContain("safe-bottom")
  })

  test("headers resolve to the same padding and minimum height", () => {
    for (const header of [chatHeader(), workHeader()]) {
      expect(header).toContain("min-h-14")
      expect(header).toContain("[--safe-area-extra-top:0.5rem]")
      expect(header).toContain("pb-2")
      // `py-2` would set `padding-top` too, and lose to the unlayerable-looking
      // helper; splitting it into `pb-2` is what keeps the two in agreement.
      expect(header).not.toMatch(/\bpy-2\b/)
      expect(header).not.toMatch(/\bpt-\d/)
    }
  })

  test("composers resolve to the same padding at both breakpoints", () => {
    for (const composer of [chatComposer(), workComposer()]) {
      expect(composer).toContain("[--safe-area-extra-bottom:0.75rem]")
      expect(composer).toContain("md:[--safe-area-extra-bottom:1rem]")
      expect(composer).toContain("pt-1.5")
      // A `pb-*` utility here is the regression: it reads as intentional
      // spacing but is silently discarded by `safe-bottom`.
      expect(composer).not.toMatch(/(^|[\s:])pb-\d/)
    }
  })
})
