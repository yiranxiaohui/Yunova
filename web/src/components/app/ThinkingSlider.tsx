import { useCallback, useRef } from "react"
import { Brain } from "lucide-react"
import { nearestTick, stepTick } from "@/lib/thinking-slider"
import { cn } from "@/lib/utils"

/**
 * The reasoning ladder as a slider rather than a row of pills.
 *
 * A ladder is ordered — "极简 → 低 → 中 → 高 → 最大" is one axis, not a set of
 * unrelated choices — and pills said nothing about that ordering while
 * wrapping onto two lines inside a 288px panel. A slider shows the position on
 * the axis, costs one line whatever the ladder's length, and keeps the
 * per-level hit areas (each tick is still clickable) that made the pills
 * usable on touch.
 *
 * `value === null` means "whatever the runtime defaults to", which is not a
 * synonym for the weakest level; it renders as an unfilled track with a muted
 * knob parked at the left so nothing claims to be selected until the user
 * actually moves it. `autoValue` is the same idea for ladders that spell that
 * default as a real level (chat's `auto`): it is lifted out of the axis into
 * its own toggle, because a slider can only show an ordered scale and
 * "automatic" has no place on one.
 */
export function ThinkingSlider<T extends string>({
  value,
  options,
  onChange,
  label,
  hint,
  autoValue,
  autoLabel = "自动",
  disabled,
  className,
}: {
  value: T | null
  options: { value: T; label: string }[]
  onChange: (next: T) => void
  label?: string
  hint?: string
  /** Level meaning "let the model decide", shown as a toggle beside the
   *  ladder instead of as a stop on it. */
  autoValue?: T
  autoLabel?: string
  disabled?: boolean
  className?: string
}) {
  const trackRef = useRef<HTMLDivElement>(null)
  const last = Math.max(options.length - 1, 1)
  const index = options.findIndex((o) => o.value === value)
  const unset = index < 0
  const pos = unset ? 0 : index
  const percent = (pos / last) * 100
  const auto = autoValue != null && value === autoValue

  // Pointer position → nearest tick. Rounding rather than flooring means the
  // knob follows the finger to the level it is closest to, which is what a
  // discrete slider is expected to do when dragged between two stops.
  const pick = useCallback(
    (clientX: number) => {
      const rect = trackRef.current?.getBoundingClientRect()
      if (!rect || rect.width === 0) return
      const next = nearestTick((clientX - rect.left) / rect.width, last)
      const option = options[next]
      if (option && option.value !== value) onChange(option.value)
    },
    [last, onChange, options, value]
  )

  function onPointerDown(e: React.PointerEvent<HTMLDivElement>) {
    if (disabled) return
    // Captured so a drag that leaves the panel keeps updating instead of
    // stopping dead — the panel is only 288px wide and overshooting is normal.
    e.currentTarget.setPointerCapture(e.pointerId)
    pick(e.clientX)
  }

  function onPointerMove(e: React.PointerEvent<HTMLDivElement>) {
    if (disabled || !e.currentTarget.hasPointerCapture(e.pointerId)) return
    pick(e.clientX)
  }

  function onKeyDown(e: React.KeyboardEvent<HTMLDivElement>) {
    if (disabled) return
    const step =
      e.key === "ArrowLeft" || e.key === "ArrowDown"
        ? -1
        : e.key === "ArrowRight" || e.key === "ArrowUp"
          ? 1
          : 0
    if (step === 0 && e.key !== "Home" && e.key !== "End") return
    e.preventDefault()
    // From "default" the first keypress lands on an end of the ladder rather
    // than nudging a position that was never chosen.
    const next =
      e.key === "Home"
        ? 0
        : e.key === "End"
          ? last
          : stepTick(unset ? null : pos, step, last)
    const option = options[next]
    if (option && option.value !== value) onChange(option.value)
  }

  return (
    <div className={cn("select-none", className)}>
      <div className="mb-2 flex items-center justify-between gap-2 text-[10px]">
        <span
          className="flex items-center gap-1.5 text-muted-foreground"
          title={hint}
        >
          <Brain className="size-3" />
          {label ?? "思考程度"}
        </span>
        <span className="flex items-center gap-1.5">
          {autoValue != null && (
            <button
              type="button"
              disabled={disabled}
              onClick={() => onChange(autoValue)}
              className={cn(
                "rounded-full border px-1.5 py-px transition-colors disabled:cursor-not-allowed disabled:opacity-60",
                auto
                  ? "border-primary/40 bg-primary/10 font-medium text-primary"
                  : "border-border/70 text-muted-foreground hover:bg-accent"
              )}
            >
              {autoLabel}
            </button>
          )}
          {/* When the auto toggle is lit it already names the state; a second
              copy of the same word beside it is just noise. */}
          {!auto && (
            <span
              className={cn(
                "font-medium",
                unset ? "text-muted-foreground" : "text-primary"
              )}
            >
              {unset ? "跟随默认" : options[pos]!.label}
            </span>
          )}
        </span>
      </div>
      <div
        role="slider"
        tabIndex={disabled ? -1 : 0}
        aria-label={label ?? "思考程度"}
        aria-valuemin={0}
        aria-valuemax={last}
        aria-valuenow={unset ? undefined : pos}
        aria-valuetext={
          unset ? (auto ? autoLabel : "跟随默认") : options[pos]!.label
        }
        aria-disabled={disabled}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onKeyDown={onKeyDown}
        className={cn(
          // Padded to a comfortable target: the visible track is 6px tall but
          // the grab area has to survive a fingertip.
          "group relative -mx-1 cursor-pointer touch-none px-1 py-2.5 outline-none",
          disabled && "cursor-not-allowed opacity-60"
        )}
      >
        <div
          ref={trackRef}
          className="relative h-1.5 w-full rounded-full bg-muted ring-offset-2 ring-offset-popover transition-shadow group-focus-visible:ring-2 group-focus-visible:ring-primary/50"
        >
          <div
            className={cn(
              "absolute inset-y-0 left-0 rounded-full bg-primary transition-[width] duration-150",
              unset && "opacity-0"
            )}
            style={{ width: `${percent}%` }}
          />
          {options.map((o, i) => {
            const passed = !unset && i <= pos
            return (
              <span
                key={o.value}
                title={o.label}
                className={cn(
                  "absolute top-1/2 size-1 -translate-x-1/2 -translate-y-1/2 rounded-full transition-colors",
                  passed ? "bg-primary-foreground/70" : "bg-foreground/25"
                )}
                style={{ left: `${(i / last) * 100}%` }}
              />
            )
          })}
          <span
            className={cn(
              "absolute top-1/2 size-4 -translate-x-1/2 -translate-y-1/2 rounded-full border bg-background shadow-sm transition-[left,border-color] duration-150",
              unset ? "border-border" : "border-primary"
            )}
            style={{ left: `${percent}%` }}
          />
        </div>
      </div>
      {/* Only the two ends are labelled: naming every stop needs more width
          than the panel has, and the chosen level is already spelled out
          above. */}
      <div className="flex items-center justify-between text-[10px] text-muted-foreground">
        <span>{options[0]?.label}</span>
        <span>{options[last]?.label}</span>
      </div>
    </div>
  )
}
