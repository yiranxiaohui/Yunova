import { Check, Cloud, Laptop, MessageSquare } from "lucide-react"
import { cn } from "@/lib/utils"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import type { AgentTarget } from "@/lib/agent"

/**
 * Chat versus work is a real behavioural split, not a cosmetic one, so it is
 * surfaced as a mode rather than hidden in settings:
 *
 * - chat has no tools and bills tokens only; it stays on the existing
 *   `/api/chat` path, which is cheaper and needs no runtime.
 * - work starts an agent runtime that can run commands, so it additionally
 *   needs an execution target and an approval story.
 *
 * Presenting them as one continuum would mislead: picking "work" decides
 * *where code runs*, which the user must choose deliberately.
 */
export type WorkMode = "chat" | "work"

export interface DeviceOption {
  id: number
  name: string
  online: boolean
}

export function ModeSelector({
  mode,
  onModeChange,
  target,
  onTargetChange,
  devices,
  deviceId,
  onDeviceChange,
  disabled,
  className,
}: {
  mode: WorkMode
  onModeChange: (m: WorkMode) => void
  target: AgentTarget
  onTargetChange: (t: AgentTarget) => void
  devices: DeviceOption[]
  deviceId: number | null
  onDeviceChange: (id: number | null) => void
  disabled?: boolean
  className?: string
}) {
  return (
    <div className={cn("flex flex-wrap items-center gap-2", className)}>
      <div className="inline-flex rounded-lg border bg-muted/40 p-0.5">
        <ModeTab
          active={mode === "chat"}
          disabled={disabled}
          onClick={() => onModeChange("chat")}
          icon={<MessageSquare className="size-3.5" />}
          label="对话"
        />
        <ModeTab
          active={mode === "work"}
          disabled={disabled}
          onClick={() => onModeChange("work")}
          icon={<Cloud className="size-3.5" />}
          label="工作"
        />
      </div>

      {mode === "work" && (
        <TargetPicker
          target={target}
          onTargetChange={onTargetChange}
          devices={devices}
          deviceId={deviceId}
          onDeviceChange={onDeviceChange}
          disabled={disabled}
        />
      )}
    </div>
  )
}

function ModeTab({
  active,
  disabled,
  onClick,
  icon,
  label,
}: {
  active: boolean
  disabled?: boolean
  onClick: () => void
  icon: React.ReactNode
  label: string
}) {
  return (
    <button
      type="button"
      disabled={disabled}
      onClick={onClick}
      aria-pressed={active}
      className={cn(
        "inline-flex items-center gap-1.5 rounded-md px-3 py-1.5 text-xs font-medium transition-colors",
        active
          ? "bg-background text-foreground shadow-sm"
          : "text-muted-foreground hover:text-foreground",
        disabled && "pointer-events-none opacity-50"
      )}
    >
      {icon}
      {label}
    </button>
  )
}

/**
 * Where the task executes.
 *
 * A device entry is offered only when that machine is connected: a task
 * pointed at an offline computer cannot start, and showing it as selectable
 * would turn a precondition into a confusing failure at send time.
 */
function TargetPicker({
  target,
  onTargetChange,
  devices,
  deviceId,
  onDeviceChange,
  disabled,
}: {
  target: AgentTarget
  onTargetChange: (t: AgentTarget) => void
  devices: DeviceOption[]
  deviceId: number | null
  onDeviceChange: (id: number | null) => void
  disabled?: boolean
}) {
  const value = target === "cloud" ? "cloud" : `device:${deviceId ?? ""}`

  return (
    <Select
      value={value}
      disabled={disabled}
      onValueChange={(v) => {
        if (v === "cloud") {
          onTargetChange("cloud")
          onDeviceChange(null)
          return
        }
        const id = Number(v.slice("device:".length))
        if (Number.isFinite(id)) {
          onTargetChange("device")
          onDeviceChange(id)
        }
      }}
    >
      <SelectTrigger size="sm" className="h-8 w-[10.5rem] text-xs">
        <SelectValue placeholder="选择执行位置" />
      </SelectTrigger>
      <SelectContent>
        <SelectItem value="cloud">
          <span className="flex items-center gap-2">
            <Cloud className="size-3.5" />
            云电脑
          </span>
        </SelectItem>
        {devices.map((d) => (
          <SelectItem key={d.id} value={`device:${d.id}`} disabled={!d.online}>
            <span className="flex items-center gap-2">
              <Laptop className="size-3.5" />
              {d.name}
              {!d.online && <span className="text-muted-foreground">（离线）</span>}
            </span>
          </SelectItem>
        ))}
        {devices.length === 0 && (
          <div className="px-2 py-1.5 text-xs text-muted-foreground">
            暂无本地电脑，安装桌面客户端后可在此选择
          </div>
        )}
      </SelectContent>
    </Select>
  )
}

/** Compact indicator of what a running session is attached to. */
export function TargetBadge({
  target,
  live,
  sandboxed,
  className,
}: {
  target: AgentTarget
  live: boolean
  sandboxed?: boolean
  className?: string
}) {
  const Icon = target === "cloud" ? Cloud : Laptop
  return (
    <span
      className={cn(
        "inline-flex items-center gap-1.5 rounded-md border px-2 py-0.5 text-xs",
        live ? "border-primary/40 text-foreground" : "text-muted-foreground",
        className
      )}
    >
      <Icon className="size-3" />
      {target === "cloud" ? "云电脑" : "本地电脑"}
      {live && <Check className="size-3 text-primary" />}
      {/* Isolation is the reason automatic approval is acceptable, so say so
          rather than leaving the user to assume it. */}
      {target === "cloud" && live && sandboxed === false && (
        <span className="text-amber-600 dark:text-amber-500">未隔离</span>
      )}
    </span>
  )
}
