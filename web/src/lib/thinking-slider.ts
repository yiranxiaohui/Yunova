/** Geometry for the discrete reasoning-level slider.
 *
 *  Kept out of the component so the arithmetic is testable without a DOM and
 *  so the component file stays refresh-friendly (components only).
 */

/** Pointer ratio (0–1 across the track) → the tick it lands on.
 *
 *  Rounded rather than floored so a drag settles on the closest level, and
 *  clamped because pointer capture keeps reporting positions after the finger
 *  has left the track. Exported for tests: the arithmetic is the whole
 *  behaviour of a discrete slider. */
export function nearestTick(ratio: number, last: number): number {
  return Math.round(Math.min(1, Math.max(0, ratio)) * last)
}

/** Where an arrow key goes from the current position.
 *
 *  `from === null` is the runtime default, which has no position on the
 *  ladder, so the first press jumps to the end the key points at instead of
 *  nudging a value the user never chose. */
export function stepTick(
  from: number | null,
  step: number,
  last: number
): number {
  if (from === null) return step > 0 ? 0 : last
  return Math.min(last, Math.max(0, from + step))
}
