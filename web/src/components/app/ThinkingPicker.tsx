import { Brain } from "lucide-react"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import {
  THINKING_LABELS,
  THINKING_LEVELS,
  type ThinkingLevel,
} from "@/lib/agent"
import {
  CHAT_THINKING_LABELS,
  CHAT_THINKING_LEVELS,
  type ChatThinkingLevel,
} from "@/lib/chat-stream"
import { cn } from "@/lib/utils"

/**
 * How hard the agent reasons before answering.
 *
 * Worth a control of its own because the cost of the wrong level runs in both
 * directions: a one-line edit pays for reasoning it does not need, and a
 * refactor across ten files silently gets less than it deserves. Until this
 * existed, every task ran on whatever the runtime defaulted to and nothing on
 * screen said so.
 *
 * `levels` is what the *selected model* supports, as reported by a live
 * runtime — the ladder is model-dependent, and `xhigh`/`max` exist only on
 * some. An empty list means nothing authoritative is known yet (no runtime
 * attached, or it could not answer), in which case the whole ladder is offered
 * rather than a control that hides working options. A level pi cannot do on
 * the current model is clamped by pi rather than rejected, so an optimistic
 * offer degrades instead of failing.
 */
export function ThinkingPicker({
  level,
  levels,
  onChange,
  disabled,
  className,
}: {
  /** Null means the runtime's own default, which is what the label says: it
   *  is not a synonym for `medium`, because the default is pi's to change. */
  level: ThinkingLevel | null
  levels: ThinkingLevel[]
  onChange: (next: ThinkingLevel) => void
  disabled?: boolean
  className?: string
}) {
  const offered = levels.length > 0 ? levels : THINKING_LEVELS

  return (
    <Select
      value={level ?? ""}
      disabled={disabled}
      onValueChange={(v) => {
        if (isThinkingLevel(v)) onChange(v)
      }}
    >
      <SelectTrigger
        size="sm"
        className={cn(
          "tap-target-sm h-8 w-[7.5rem] rounded-full text-xs",
          className
        )}
        title="选择推理级别：级别越高，Agent 在回答前思考得越久，消耗的 token 也越多"
      >
        <span className="flex min-w-0 items-center gap-1.5">
          <Brain className="size-3.5 shrink-0" />
          <SelectValue placeholder="默认思考" />
        </span>
      </SelectTrigger>
      <SelectContent>
        {offered.map((l) => (
          <SelectItem key={l} value={l} className="tap-target-sm">
            {THINKING_LABELS[l]}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  )
}

function isThinkingLevel(v: string): v is ThinkingLevel {
  return (THINKING_LEVELS as string[]).includes(v)
}

/**
 * The same choice in chat mode, against a different ladder.
 *
 * Chat talks to the three vendor APIs directly rather than through a runtime
 * that normalises them, and they disagree on what a level is: an effort enum,
 * a token budget, a dynamic budget. So the chat ladder is deliberately shorter
 * and includes `auto` — the one value that behaves sensibly everywhere, and
 * the behaviour chat had before the level could be chosen at all.
 *
 * Never disabled by model capability: `chat-stream` downgrades a rejected
 * level (to `auto`, then to no thinking at all) rather than failing, so a
 * model that cannot do this still answers.
 */
export function ChatThinkingPicker({
  level,
  onChange,
  disabled,
  className,
}: {
  level: ChatThinkingLevel
  onChange: (next: ChatThinkingLevel) => void
  disabled?: boolean
  className?: string
}) {
  return (
    <Select
      value={level}
      disabled={disabled}
      onValueChange={(v) => {
        if (isChatThinkingLevel(v)) onChange(v)
      }}
    >
      <SelectTrigger
        size="sm"
        className={cn(
          "tap-target-sm h-8 w-[6.5rem] rounded-full text-xs",
          className
        )}
        title="选择思考强度：级别越高，回答前思考得越久，消耗的 token 也越多。不支持的模型会自动回退。"
      >
        <span className="flex min-w-0 items-center gap-1.5">
          <Brain className="size-3.5 shrink-0" />
          <SelectValue />
        </span>
      </SelectTrigger>
      <SelectContent>
        {CHAT_THINKING_LEVELS.map((l) => (
          <SelectItem key={l} value={l} className="tap-target-sm">
            {CHAT_THINKING_LABELS[l]}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  )
}

function isChatThinkingLevel(v: string): v is ChatThinkingLevel {
  return (CHAT_THINKING_LEVELS as string[]).includes(v)
}
