import { useEffect, useMemo, useRef, useState } from "react"
import { Check, RefreshCcw } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { describeModelQuota, type PlatformModel } from "@/lib/platform-models"
import { PROTOCOL_META, type Protocol } from "@/lib/settings"
import { cn } from "@/lib/utils"

/**
 * Model switcher shared by chat and work mode.
 *
 * Extracted rather than duplicated because the two screens must agree on what
 * a model *is*: the same catalogue, the same price line, the same protocol
 * badge. When work mode had no picker at all, its model was whatever the
 * runtime happened to default to, which is exactly the kind of invisible
 * difference this component exists to prevent.
 *
 * Loading is injected. Chat can read either the platform catalogue or the
 * user's own upstream, while work mode is always platform-billed, and neither
 * caller should have to explain that to a popover.
 */
export function ModelPicker({
  protocol,
  model,
  load,
  /** Changing this discards the cached list; pass whatever identifies the
   *  upstream the models were read from. */
  reloadKey = "",
  onChangeModel,
  footer,
  showQuota,
  disabled,
  placeholder = "未配置",
  title = "点击切换模型",
  className,
}: {
  protocol: Protocol
  model: string
  load: () => Promise<PlatformModel[]>
  reloadKey?: string
  onChangeModel: (next: string, protocol?: Protocol) => void
  footer?: string
  showQuota?: boolean
  disabled?: boolean
  placeholder?: string
  title?: string
  className?: string
}) {
  const [open, setOpen] = useState(false)
  // Cached with the key it was loaded under, so a changed upstream invalidates
  // the list by derivation instead of by an effect that clears state after the
  // stale models have already been rendered once.
  const [cache, setCache] = useState<{
    key: string
    models: PlatformModel[]
    error: string | null
  } | null>(null)
  const [loading, setLoading] = useState(false)
  const [query, setQuery] = useState("")
  const popRef = useRef<HTMLDivElement>(null)

  const fresh = cache?.key === reloadKey ? cache : null
  const models = fresh?.models ?? []
  const error = fresh?.error ?? null

  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent) => {
      if (!popRef.current) return
      if (!popRef.current.contains(e.target as Node)) setOpen(false)
    }
    window.addEventListener("mousedown", onDown)
    return () => window.removeEventListener("mousedown", onDown)
  }, [open])

  async function fetchList() {
    setLoading(true)
    try {
      setCache({ key: reloadKey, models: await load(), error: null })
    } catch (e) {
      setCache({
        key: reloadKey,
        models: [],
        error: e instanceof Error ? e.message : String(e),
      })
    } finally {
      setLoading(false)
    }
  }

  // Loaded from the button rather than from an effect on `open`: the catalogue
  // is fetched because the user asked to see it, so the request belongs to the
  // interaction and not to a render pass.
  function toggle() {
    const next = !open
    setOpen(next)
    if (next && !fresh && !loading) void fetchList()
  }

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase()
    if (!q) return models
    return models.filter((m) => {
      const display = m.display_name ?? ""
      return (
        m.model.toLowerCase().includes(q) ||
        display.toLowerCase().includes(q) ||
        m.protocol.toLowerCase().includes(q)
      )
    })
  }, [models, query])

  return (
    <div className={cn("relative min-w-0", className)} ref={popRef}>
      <button
        type="button"
        disabled={disabled}
        onClick={toggle}
        className="inline-flex min-w-0 max-w-[8rem] items-center gap-1.5 rounded-xl border border-border/70 bg-card/70 px-2.5 py-1.5 text-xs shadow-sm backdrop-blur transition-all hover:border-primary/30 hover:bg-card disabled:cursor-not-allowed disabled:opacity-60 sm:max-w-[11rem] md:max-w-none"
        title={title}
      >
        <span
          className={cn(
            "inline-block size-2 shrink-0 rounded-full bg-gradient-to-br",
            PROTOCOL_COLOR[protocol]
          )}
        />
        <span className="hidden font-medium md:inline">
          {PROTOCOL_META[protocol].label.replace(" 兼容", "")}
        </span>
        <span className="hidden text-muted-foreground md:inline">·</span>
        <span className="truncate text-muted-foreground">{model || placeholder}</span>
        <span className="shrink-0 text-muted-foreground">▾</span>
      </button>

      {open && (
        <div className="absolute left-0 top-full z-40 mt-2 w-72 rounded-2xl border border-border bg-popover/95 p-2.5 shadow-panel backdrop-blur-xl">
          <div className="mb-2 flex items-center gap-1">
            <Input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="搜索模型…"
              className="h-8 text-xs"
              autoFocus
            />
            <Button
              type="button"
              size="icon"
              variant="ghost"
              onClick={() => void fetchList()}
              disabled={loading}
              title="重新拉取"
              className="size-8 shrink-0"
            >
              <RefreshCcw className={cn("size-3.5", loading && "animate-spin")} />
            </Button>
          </div>
          <div className="nc-scroll max-h-72 overflow-y-auto">
            {loading && models.length === 0 && (
              <p className="px-2 py-6 text-center text-xs text-muted-foreground">
                加载中…
              </p>
            )}
            {!loading && error && (
              <p className="rounded border border-destructive/40 bg-destructive/10 px-2 py-1.5 text-xs text-destructive">
                {error}
              </p>
            )}
            {!loading && !error && filtered.length === 0 && (
              <p className="px-2 py-6 text-center text-xs text-muted-foreground">
                {models.length === 0 ? "暂无可用模型" : "没有匹配的模型"}
              </p>
            )}
            <ul className="flex flex-col">
              {filtered.map((m) => {
                const active = m.model === model
                return (
                  <li key={`${m.protocol}:${m.model}`}>
                    <button
                      type="button"
                      onClick={() => {
                        onChangeModel(m.model, m.protocol)
                        setOpen(false)
                      }}
                      className={cn(
                        "flex w-full items-center justify-between rounded px-2 py-1.5 text-left text-xs hover:bg-accent",
                        active && "bg-accent text-accent-foreground"
                      )}
                    >
                      <span className="min-w-0">
                        <span className="block truncate font-mono">
                          {m.display_name || m.model}
                        </span>
                        {showQuota && (
                          <span className="block truncate text-[10px] text-muted-foreground">
                            {m.protocol} · {describeModelQuota(m)}
                          </span>
                        )}
                      </span>
                      {active && <Check className="ml-2 size-3.5 shrink-0 text-primary" />}
                    </button>
                  </li>
                )
              })}
            </ul>
          </div>
          {footer && (
            <div className="mt-2 border-t border-border pt-2 text-[10px] text-muted-foreground">
              {footer}
            </div>
          )}
        </div>
      )}
    </div>
  )
}

const PROTOCOL_COLOR: Record<Protocol, string> = {
  openai: "from-emerald-400 to-emerald-600",
  claude: "from-amber-400 to-orange-600",
  gemini: "from-sky-400 to-indigo-600",
}
