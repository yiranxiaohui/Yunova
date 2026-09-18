import { describe, expect, test } from "bun:test"
import { nearestTick, stepTick } from "../src/lib/thinking-slider"

/** The chat ladder has 5 stops, work mode's has 7; both index 0..last. */
const LAST_CHAT = 4
const LAST_WORK = 6

describe("thinking slider position", () => {
  test("snaps to the closest stop, not the one before it", () => {
    // Just past the midpoint between stop 2 and 3 on a 5-stop ladder.
    expect(nearestTick(0.63, LAST_CHAT)).toBe(3)
    expect(nearestTick(0.6, LAST_CHAT)).toBe(2)
  })

  test("both ends are reachable and overshoot is clamped", () => {
    expect(nearestTick(0, LAST_WORK)).toBe(0)
    expect(nearestTick(1, LAST_WORK)).toBe(LAST_WORK)
    // Pointer capture keeps reporting after the finger leaves the track.
    expect(nearestTick(-0.4, LAST_WORK)).toBe(0)
    expect(nearestTick(2.5, LAST_WORK)).toBe(LAST_WORK)
  })

  test("arrow keys move one stop and stop at the ends", () => {
    expect(stepTick(2, 1, LAST_CHAT)).toBe(3)
    expect(stepTick(2, -1, LAST_CHAT)).toBe(1)
    expect(stepTick(LAST_CHAT, 1, LAST_CHAT)).toBe(LAST_CHAT)
    expect(stepTick(0, -1, LAST_CHAT)).toBe(0)
  })

  test("from the runtime default a key jumps to the end it points at", () => {
    // Null has no position on the ladder, so nudging it would be a lie.
    expect(stepTick(null, 1, LAST_WORK)).toBe(0)
    expect(stepTick(null, -1, LAST_WORK)).toBe(LAST_WORK)
  })
})
