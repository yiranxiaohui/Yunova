import { afterEach, beforeEach, describe, expect, test } from "bun:test"
import { modePath, readModeDraft } from "../src/lib/mode"

/** `readModeDraft` reads the active history entry, which is how the draft's
 *  lifetime is scoped to one navigation. Stub it per test. */
const original = Object.getOwnPropertyDescriptor(globalThis, "window")

function withHistoryState(state: unknown) {
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: { history: { state } },
  })
}

beforeEach(() => withHistoryState(null))

afterEach(() => {
  if (original) Object.defineProperty(globalThis, "window", original)
  else Reflect.deleteProperty(globalThis, "window")
})

describe("chat/work mode switching", () => {
  test("each mode maps to its own route", () => {
    expect(modePath("chat")).toBe("/")
    expect(modePath("work")).toBe("/t")
  })

  test("carries the half-typed prompt handed over by the other mode", () => {
    withHistoryState({ usr: { modeDraft: "帮我部署这个仓库" } })
    expect(readModeDraft()).toBe("帮我部署这个仓库")
  })

  test("arriving any other way starts with an empty composer", () => {
    // A sidebar link, a fresh visit, or a task-creation navigation carrying an
    // unrelated payload must not resurrect a draft.
    expect(readModeDraft()).toBe("")
    withHistoryState({ usr: {} })
    expect(readModeDraft()).toBe("")
    withHistoryState({ usr: { pending: "已经发出的提示" } })
    expect(readModeDraft()).toBe("")
  })

  test("ignores a non-string draft instead of rendering it", () => {
    // History state is attacker-adjacent (it survives reloads and can be
    // hand-edited), so a wrong type must not reach the textarea.
    withHistoryState({ usr: { modeDraft: { toString: () => "x" } } })
    expect(readModeDraft()).toBe("")
  })
})
