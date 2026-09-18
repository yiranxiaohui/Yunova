import { Check, Cloud, FolderOpen, Laptop, MessageSquare, ShieldCheck } from "lucide-react"
import { cn } from "@/lib/utils"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import {
  APPROVAL_DEFAULT_HINT,
  APPROVAL_HINTS,
  APPROVAL_LABELS,
  type AgentTarget,
  type ApprovalMode,
} from "@/lib/agent"
import { prefetchWorkMode, type WorkMode } from "@/lib/mode"

export type { WorkMode } from "@/lib/mode"

export interface DeviceOption {
  id: number
  name: string
  online: boolean
  /** That machine's own approval policy, as it reports it.
   *
   *  Carried here so the approval picker can say which side wins: the machine
   *  keeps the stricter of its setting and the task's, so offering "全部放行"
   *  on a machine configured to ask would be a control that silently does
   *  nothing. Null while it is offline or from a client too old to say. */
  approval?: ApprovalMode | null
}

const MODES: Array<{
  value: WorkMode
  label: string
  icon: React.ReactNode
  hint: string
}> = [
  {
    value: "chat",
    label: "对话",
    icon: <MessageSquare className="size-3.5" />,
    hint: "直接提问、上传文件，只按 token 计费",
  },
  {
    value: "work",
    label: "工作",
    icon: <Cloud className="size-3.5" />,
    hint: "启动可执行命令的 Agent，需要选择运行位置",
  },
]

/**
 * The chat/work switch.
 *
 * One segmented control shared by both pages, because the switch is the only
 * place the two modes meet: if each page drew its own toggle they would drift
 * in position and size, and the change would read as a page jump rather than
 * a state change.
 *
 * The moving part is a single absolutely-positioned thumb rather than a
 * per-button background. Restyling two buttons makes the active half appear
 * to blink at the new location; sliding one element makes the destination
 * visible during the transition, which is what makes the control feel
 * continuous even though a route change happens underneath.
 */
export function ModeSwitch({
  mode,
  onModeChange,
  size = "sm",
  className,
}: {
  mode: WorkMode
  onModeChange: (m: WorkMode) => void
  /** `lg` for the empty-state hero, `sm` next to the composer. */
  size?: "sm" | "lg"
  className?: string
}) {
  const index = mode === "work" ? 1 : 0
  const large = size === "lg"

  return (
    <div
      role="tablist"
      aria-label="模式"
      aria-orientation="horizontal"
      className={cn(
        "mode-switch relative inline-grid grid-cols-2 rounded-full border border-border/70 bg-muted/50 p-1 shadow-sm backdrop-blur",
        large && "w-[19rem] max-w-full",
        className
      )}
    >
      {/* The thumb is driven by a transform so the browser animates it on the
          compositor; animating `left` would relayout the row on every frame
          and stutter next to a streaming transcript. */}
      <span
        aria-hidden
        className={cn(
          "pointer-events-none absolute inset-y-1 left-1 w-[calc(50%-0.25rem)] rounded-full bg-background shadow-[0_2px_10px_-4px_color-mix(in_oklch,var(--foreground)_45%,transparent)] ring-1 ring-border/60",
          "transition-transform duration-300 ease-[cubic-bezier(0.22,1,0.36,1)] motion-reduce:transition-none"
        )}
        style={{ transform: `translateX(${index * 100}%)` }}
      />
      {MODES.map((m) => {
        const active = m.value === mode
        return (
          <button
            key={m.value}
            type="button"
            role="tab"
            aria-selected={active}
            // Roving focus: one stop for the pair, then arrows move within it,
            // which is how a two-state switch is expected to behave.
            tabIndex={active ? 0 : -1}
            title={m.hint}
            onClick={() => onModeChange(m.value)}
            // Warm the work-mode chunk on intent rather than on click: by the
            // time the pointer lands the code is usually already parsed, so
            // the switch does not fall back to a loading screen.
            onPointerEnter={() => {
              if (m.value === "work") prefetchWorkMode()
            }}
            onFocus={() => {
              if (m.value === "work") prefetchWorkMode()
            }}
            onKeyDown={(e) => {
              if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return
              e.preventDefault()
              onModeChange(mode === "chat" ? "work" : "chat")
            }}
            className={cn(
              "tap-target-sm relative z-10 inline-flex items-center justify-center gap-1.5 rounded-full font-medium transition-colors duration-200",
              large ? "px-6 py-2 text-sm" : "px-4 py-1.5 text-xs",
              active
                ? "text-foreground"
                : "text-muted-foreground hover:text-foreground"
            )}
          >
            {m.icon}
            {m.label}
          </button>
        )
      })}
    </div>
  )
}

/**
 * Work mode's control strip: the switch plus where the task runs.
 *
 * The switch stays live even inside a running task: leaving for chat is
 * always allowed, because it starts a separate conversation and does not
 * touch the task. Only the execution target locks once a session exists,
 * since its runtime and transcript already belong to one machine.
 */
export function ModeSelector({
  onModeChange,
  target,
  onTargetChange,
  devices,
  deviceId,
  onDeviceChange,
  targetLocked,
  workspace,
  onPickWorkspace,
  approval,
  onApprovalChange,
  deviceApproval,
  approvalPending,
  /** Hidden when the page already shows the large empty-state switch, so the
   *  control never appears twice on one screen. */
  hideSwitch,
  className,
}: {
  onModeChange: (m: WorkMode) => void
  target: AgentTarget
  onTargetChange: (t: AgentTarget) => void
  devices: DeviceOption[]
  deviceId: number | null
  onDeviceChange: (id: number | null) => void
  targetLocked?: boolean
  /** Directory the task runs in; null means the machine's default. */
  workspace?: string | null
  /** Absent for a task that already exists: its runtime and transcript belong
   *  to one directory, so changing it mid-task would silently move the work. */
  onPickWorkspace?: () => void
  /** What this task asked the gate to stop for; null means the target's own
   *  policy. */
  approval?: ApprovalMode | null
  onApprovalChange?: (next: ApprovalMode | null) => void
  /** The chosen machine's own setting, as it reports it. Shown because the
   *  machine keeps the stricter of the two, so a task asking for less than
   *  this gets the machine's answer and the user should see that before
   *  walking away. Null while offline or from a client too old to say. */
  deviceApproval?: ApprovalMode | null
  /** Whether the change waits for the next start. True once a runtime is
   *  live: the gate is loaded when the runtime starts, so saying nothing
   *  would let the user believe a running agent had just been reined in. */
  approvalPending?: boolean
  hideSwitch?: boolean
  className?: string
}) {
  return (
    <div className={cn("flex flex-wrap items-center gap-2", className)}>
      {!hideSwitch && <ModeSwitch mode="work" onModeChange={onModeChange} />}
      <TargetPicker
        target={target}
        onTargetChange={onTargetChange}
        devices={devices}
        deviceId={deviceId}
        onDeviceChange={onDeviceChange}
        disabled={targetLocked}
      />
      {/* Only for a local machine: a cloud task runs in a disposable sandbox
          whose directory nothing can usefully choose. */}
      {target === "device" && (
        <WorkspaceChip workspace={workspace} onPick={onPickWorkspace} />
      )}
      {/* Offered for both targets, and meaning different things in each: on a
          machine it can only tighten what its owner configured, while a
          sandbox has no policy of its own and gets exactly this. */}
      {onApprovalChange && (
        <ApprovalPicker
          target={target}
          approval={approval ?? null}
          onChange={onApprovalChange}
          deviceApproval={deviceApproval ?? null}
          pending={approvalPending}
        />
      )}
    </div>
  )
}

/**
 * How much this task stops to ask.
 *
 * A per-task control rather than only a per-machine one, because the right
 * answer changes with the task and not with the computer: the same laptop runs
 * a mechanical rename that should not ask twenty times and an unfamiliar
 * script that should ask about everything. Before this, changing either meant
 * opening the desktop client's settings and changing it for every task.
 *
 * What it is careful not to imply is authority. On a local machine the setting
 * there is a floor — the machine keeps the stricter of the two — so when this
 * asks for less than the machine requires, the menu says which one wins rather
 * than showing a choice that quietly does nothing.
 */
function ApprovalPicker({
  target,
  approval,
  onChange,
  deviceApproval,
  pending,
}: {
  target: AgentTarget
  approval: ApprovalMode | null
  onChange: (next: ApprovalMode | null) => void
  deviceApproval: ApprovalMode | null
  pending?: boolean
}) {
  const strictness: Record<ApprovalMode, number> = {
    never: 0,
    commands: 1,
    always: 2,
  }
  // Only a local machine has a policy of its own to be overruled by.
  const floor = target === "device" ? deviceApproval : null
  const overruled = (m: ApprovalMode) =>
    floor != null && strictness[floor] > strictness[m]
  const effective = approval ?? floor

  return (
    <Select
      value={approval ?? "default"}
      onValueChange={(v) => onChange(v === "default" ? null : (v as ApprovalMode))}
    >
      <SelectTrigger
        size="sm"
        className="tap-target-sm h-8 w-[8.5rem] rounded-full text-xs"
        title={
          approval
            ? `审批方式：${APPROVAL_HINTS[approval]}`
            : `审批方式：${APPROVAL_DEFAULT_HINT[target]}`
        }
      >
        <span className="flex min-w-0 items-center gap-1.5">
          <ShieldCheck className="size-3.5 shrink-0" />
          <span className="truncate">
            {approval ? APPROVAL_LABELS[approval] : "默认审批"}
          </span>
        </span>
      </SelectTrigger>
      <SelectContent>
        <SelectItem value="default" className="tap-target-sm">
          <span className="flex flex-col items-start">
            <span>默认审批</span>
            <span className="text-[10px] text-muted-foreground">
              {APPROVAL_DEFAULT_HINT[target]}
            </span>
          </span>
        </SelectItem>
        {(Object.keys(APPROVAL_LABELS) as ApprovalMode[]).map((m) => (
          <SelectItem key={m} value={m} className="tap-target-sm">
            <span className="flex flex-col items-start">
              <span>{APPROVAL_LABELS[m]}</span>
              <span className="text-[10px] text-muted-foreground">
                {/* Saying so beats offering a choice that silently does
                    nothing: the machine keeps the stricter policy, so this
                    option would not actually loosen anything. */}
                {overruled(m)
                  ? `该电脑自身要求「${APPROVAL_LABELS[floor!]}」，以更严的为准`
                  : APPROVAL_HINTS[m]}
              </span>
            </span>
          </SelectItem>
        ))}
        {/* The gate is an extension the runtime loads at startup, so a change
            made mid-task cannot reach the agent that is already running. */}
        {pending && (
          <div className="px-2 py-1.5 text-[10px] text-muted-foreground">
            修改下次启动运行时生效，当前运行中的任务仍用原设置
            {effective && `（${APPROVAL_LABELS[effective]}）`}
          </div>
        )}
      </SelectContent>
    </Select>
  )
}

/**
 * Which directory a local task runs in.
 *
 * Shown as a chip rather than folded into the target dropdown: the machine and
 * the directory are two decisions, and a task aimed at the wrong directory is
 * the more expensive mistake of the two. Reads "默认目录" when nothing was
 * picked, which is what the client falls back to.
 */
function WorkspaceChip({
  workspace,
  onPick,
}: {
  workspace?: string | null
  onPick?: () => void
}) {
  const label = workspace ? leafOf(workspace) : "默认目录"
  const shared =
    "inline-flex h-8 max-w-[12rem] items-center gap-1.5 rounded-full border px-2.5 text-xs"

  // A started task keeps its directory, so the chip becomes a label. Still
  // rendered, because "where did this run" is part of reading a transcript.
  if (!onPick) {
    return (
      <span className={cn(shared, "text-muted-foreground")} title={workspace ?? undefined}>
        <FolderOpen className="size-3.5 shrink-0" />
        <span className="truncate">{label}</span>
      </span>
    )
  }

  return (
    <button
      type="button"
      onClick={onPick}
      // The full path in the tooltip: the chip shows the leaf so the strip
      // stays usable, but the leaf alone is ambiguous across projects.
      title={workspace ? `工作目录：${workspace}` : "选择工作目录"}
      className={cn(shared, "tap-target-sm hover:border-primary/40")}
    >
      <FolderOpen className="size-3.5 shrink-0" />
      <span className="truncate">{label}</span>
    </button>
  )
}

/** Last path segment, for a chip that cannot fit a whole path. */
function leafOf(path: string): string {
  // Both separators, because the machine may be Windows while the browser is
  // not: splitting on the browser's idea of a separator would show the whole
  // path for exactly the users who need the short form most.
  const parts = path.split(/[/\\]/).filter(Boolean)
  return parts[parts.length - 1] ?? path
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
      <SelectTrigger
        size="sm"
        className="tap-target-sm h-8 w-[10.5rem] rounded-full text-xs"
      >
        <SelectValue placeholder="选择执行位置" />
      </SelectTrigger>
      <SelectContent>
        <SelectItem value="cloud" className="tap-target-sm">
          <span className="flex items-center gap-2">
            <Cloud className="size-3.5" />
            云电脑
          </span>
        </SelectItem>
        {devices.map((d) => (
          <SelectItem
            key={d.id}
            value={`device:${d.id}`}
            disabled={!d.online}
            className="tap-target-sm"
          >
            <span className="flex items-center gap-2">
              <Laptop className="size-3.5" />
              {d.name}
              {!d.online && <span className="text-muted-foreground">（离线）</span>}
            </span>
          </SelectItem>
        ))}
        {devices.length === 0 && (
          <div className="px-2 py-1.5 text-xs text-muted-foreground">
            暂无本地电脑：在那台电脑上打开桌面客户端并登录本账号即可
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
