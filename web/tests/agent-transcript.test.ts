import { describe, expect, test } from "bun:test"
import {
  APPROVAL_HINTS,
  APPROVAL_LABELS,
  approvalMessage,
  approvalTitle,
  asApprovalMode,
  entriesToItems,
  mergeAgentItems,
  toAgentBlocks,
  type AgentBlock,
  type AgentEntry,
  type AgentItem,
} from "../src/lib/agent"

/** A mirrored user turn, in the runtime's own entry shape. */
function userEntry(id: string, text: string): AgentEntry {
  return {
    id,
    parentId: null,
    type: "message",
    message: { role: "user", content: [{ type: "text", text }] },
  }
}

/** A mirrored assistant turn. */
function assistantEntry(id: string, text: string): AgentEntry {
  return {
    id,
    parentId: null,
    type: "message",
    message: { role: "assistant", content: [{ type: "text", text }] },
  }
}

describe("transcript reconciliation", () => {
  test("appends what the view has not rendered yet", () => {
    const prev = entriesToItems([userEntry("a", "hi")])
    const fresh = entriesToItems([assistantEntry("b", "Hi! How can I help?")])
    const merged = mergeAgentItems(prev, fresh)
    expect(merged.map((i) => i.kind)).toEqual(["user", "assistant"])
  })

  test("an overlapping range cannot duplicate a turn", () => {
    // This is the regression: a settle reaches the page twice — once as the
    // runtime's own `agent_settled` frame and once as the server's `settled`
    // after mirroring — so two syncs read the same cursor and fetched the same
    // range. Appending blindly showed the whole turn a second time.
    const entries = [userEntry("a", "hi"), assistantEntry("b", "Hi!")]
    const first = entriesToItems(entries)
    const second = entriesToItems(entries)
    const merged = mergeAgentItems(first, second)
    expect(merged).toHaveLength(2)
    expect(merged.map((i) => i.id)).toEqual(first.map((i) => i.id))
  })

  test("a partially overlapping range adds only the new tail", () => {
    const prev = entriesToItems([userEntry("a", "hi")])
    const fresh = entriesToItems([
      userEntry("a", "hi"),
      assistantEntry("b", "Hi!"),
    ])
    const merged = mergeAgentItems(prev, fresh)
    expect(merged).toHaveLength(2)
    expect((merged[1] as Extract<AgentItem, { kind: "assistant" }>).text).toBe(
      "Hi!"
    )
  })

  test("keeps the same array when nothing is new", () => {
    // Identity matters: returning a fresh array would re-render the whole
    // transcript, which on a long task is visible as a scroll jump.
    const prev = entriesToItems([userEntry("a", "hi")])
    expect(mergeAgentItems(prev, [])).toBe(prev)
    expect(mergeAgentItems(prev, entriesToItems([userEntry("a", "hi")]))).toBe(
      prev
    )
  })

  test("one assistant entry with several blocks stays distinct", () => {
    // Ids are derived per block, so a multi-block turn must not collapse into
    // one item and must not be mistaken for a duplicate of itself.
    const items = entriesToItems([
      {
        id: "x",
        parentId: null,
        type: "message",
        message: {
          role: "assistant",
          content: [
            { type: "thinking", thinking: "let me check" },
            { type: "text", text: "done" },
          ],
        },
      },
    ])
    expect(items.map((i) => i.kind)).toEqual(["thinking", "assistant"])
    expect(new Set(items.map((i) => i.id)).size).toBe(2)
    expect(mergeAgentItems(items, items)).toHaveLength(2)
  })

  test("a tool result merges onto its call rather than trailing it", () => {
    const items = entriesToItems([
      {
        id: "c",
        parentId: null,
        type: "message",
        message: {
          role: "assistant",
          content: [
            { type: "toolCall", id: "t1", name: "bash", arguments: { command: "ls" } },
          ],
        },
      },
      {
        id: "r",
        parentId: "c",
        type: "message",
        message: {
          role: "toolResult",
          toolCallId: "t1",
          content: [{ type: "text", text: "README.md" }],
        },
      },
    ])
    expect(items).toHaveLength(1)
    const tool = items[0] as Extract<AgentItem, { kind: "tool" }>
    expect(tool.kind).toBe("tool")
    expect(tool.output).toBe("README.md")
    expect(tool.ok).toBe(true)
  })
})

describe("transcript grouping", () => {
  const user = (id: string): AgentItem => ({ kind: "user", id, text: "hi" })
  const assistant = (id: string): AgentItem => ({ kind: "assistant", id, text: "ok" })
  const tool = (id: string): AgentItem => ({ kind: "tool", id, name: "bash", args: {} })
  const thinking = (id: string): AgentItem => ({ kind: "thinking", id, text: "…" })

  test("merges a run of steps into one panel", () => {
    const blocks = toAgentBlocks([tool("t1"), thinking("k1"), tool("t2")])
    expect(blocks).toHaveLength(1)
    expect(blocks[0]!.kind).toBe("steps")
    expect((blocks[0] as Extract<AgentBlock, { kind: "steps" }>).items).toHaveLength(3)
  })

  test("a message between two runs splits them", () => {
    const blocks = toAgentBlocks([tool("t1"), assistant("a1"), tool("t2")])
    expect(blocks.map((b) => b.kind)).toEqual(["steps", "single", "steps"])
  })

  test("messages are never folded into a step panel", () => {
    const blocks = toAgentBlocks([user("u1"), assistant("a1")])
    expect(blocks.map((b) => b.kind)).toEqual(["single", "single"])
  })

  test("a block is keyed by its first item, so keys stay stable as steps arrive", () => {
    expect(toAgentBlocks([tool("t1"), tool("t2")])[0]!.id).toBe("t1")
    expect(toAgentBlocks([tool("t1"), tool("t2"), tool("t3")])[0]!.id).toBe("t1")
  })

  test("an empty transcript produces no blocks", () => {
    expect(toAgentBlocks([])).toEqual([])
  })
})

describe("approval requests", () => {
  // pi's RPC mode sends `{type, id, method, title, message}` for a confirm.
  const confirm = (title: string, message?: string) => ({
    type: "extension_ui_request",
    id: "u1",
    method: "confirm",
    title,
    message,
  })

  test("shows the command the user is being asked to approve", () => {
    // The regression this covers reached users: the gate called confirm with
    // an options object and read the wrong argument key, so every card said
    // "允许在本机执行 bash?" over a body of `{}`. Approving something you
    // cannot see is not a decision.
    const req = confirm("允许在本机执行命令？", "目录：/home/u/app\n\nrm -rf build")
    expect(approvalTitle(req)).toBe("允许在本机执行命令？")
    expect(approvalMessage(req)).toContain("rm -rf build")
  })

  test("still reads a request whose fields arrived nested", () => {
    // An older client passed `{title: {title, message}}`. Accepting both keeps
    // a mixed fleet answerable instead of showing "需要确认" with no body.
    const nested = {
      method: "confirm",
      title: { title: "允许执行？", message: "ls -la" },
    }
    expect(approvalTitle(nested)).toBe("允许执行？")
    expect(approvalMessage(nested)).toBe("ls -la")
  })

  test("falls back to a label rather than rendering an empty card", () => {
    expect(approvalTitle({ method: "confirm" })).toBe("需要确认")
    expect(approvalMessage({ method: "confirm" })).toBe("")
  })
})

describe("device approval modes", () => {
  test("labels every mode this build can render", () => {
    for (const mode of ["always", "commands", "never"] as const) {
      expect(asApprovalMode(mode)).toBe(mode)
      expect(APPROVAL_LABELS[mode]).toBeTruthy()
      expect(APPROVAL_HINTS[mode]).toBeTruthy()
    }
  })

  test("an unknown or absent mode renders nothing rather than a guess", () => {
    // The gate lives on the user's machine and the server cannot read it, so
    // an offline machine — or one running a client too old to report — has no
    // policy to display. Claiming one would assert a boundary nobody checked.
    expect(asApprovalMode(null)).toBeNull()
    expect(asApprovalMode(undefined)).toBeNull()
    expect(asApprovalMode("auto")).toBeNull()
    expect(asApprovalMode("本机已关闭安全限制")).toBeNull()
  })
})
