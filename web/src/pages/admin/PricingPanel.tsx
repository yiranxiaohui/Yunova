import { useEffect, useRef, useState } from "react"
import {
  Check,
  CheckCircle2,
  ChevronDown,
  CloudDownload,
  CircleAlert,
  LoaderCircle,
  Pencil,
  Plus,
  RefreshCw,
  Search,
  Trash2,
} from "lucide-react"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import {
  channelsAdminApi,
  type AllChannelModel,
  type ChannelKind,
  type ChannelProtocol,
  type ModelPrice,
  type PricingInput,
  type VideoSizeRule,
} from "@/lib/channels"
import { NewApiImportDialog } from "./NewApiImportDialog"
import {
  adminQuotaApi,
  formatQuota,
  microQuotaForMicroUsd,
  microUsdToUsd,
  usdToMicroUsd,
  type AdminSettings,
} from "@/lib/quota"
import {
  defaultVideoSize,
  displayVideoSize,
  effectiveAllowedSeconds,
  isGrokVideoModel,
  videoSizeOptions,
} from "@/lib/video-capabilities"

const KINDS: ChannelKind[] = ["chat", "image", "video"]
const PROTOCOLS: ChannelProtocol[] = ["openai", "claude", "gemini"]
const SIZE_RE = /^\d+x\d+$/
const KIND_LABELS: Record<ChannelKind, string> = {
  chat: "对话",
  image: "图片",
  video: "视频",
}
const PROTOCOL_LABELS: Record<ChannelProtocol, string> = {
  openai: "OpenAI",
  claude: "Claude",
  gemini: "Gemini",
}

/**
 * 美元单价输入框。内部以微美元存储，避免 0.15 这类小数在多次编辑后累积浮点误差；
 * 输入框保留用户原始文本，这样输到一半的 "0." 不会被规整掉。
 */
function UsdPriceField({
  label,
  value,
  onChange,
  usdToCnyRateMicro,
  multiplierPercent,
  placeholder,
}: {
  label: string
  value: number | null
  onChange: (microUsd: number) => void
  usdToCnyRateMicro: number
  multiplierPercent: number
  placeholder?: string
}) {
  const [text, setText] = useState(() => microUsdToUsd(value ?? 0))
  // 外部重置（例如切换编辑对象）时同步回显，用户正在输入时不打断。
  const [lastValue, setLastValue] = useState(value)
  if (value !== lastValue) {
    setLastValue(value)
    setText(microUsdToUsd(value ?? 0))
  }
  const micro = value ?? 0
  const quota = microQuotaForMicroUsd(micro, usdToCnyRateMicro, multiplierPercent)
  return (
    <div>
      <Label>{label}</Label>
      <div className="relative">
        <span className="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-xs text-muted-foreground">
          $
        </span>
        <Input
          type="number"
          min={0}
          step="any"
          className="pl-5"
          value={text}
          placeholder={placeholder ?? "0"}
          onChange={(e) => {
            setText(e.target.value)
            onChange(usdToMicroUsd(e.target.value))
          }}
        />
      </div>
      <p className="mt-1 text-[11px] text-muted-foreground tabular-nums">
        {micro > 0 ? `≈ ${formatQuota(quota)} 元` : "免费"}
      </p>
    </div>
  )
}

/** 列表里的价格摘要：对话显示输入/输出价，图像按次，视频按基础价+每秒。 */
function describePrice(r: ModelPrice): string {
  const usd = (micro: number) => `$${micro / 1_000_000}`
  if (r.kind === "video") {
    return `${usd(r.base_price)}+${usd(r.per_second_price)}/秒`
  }
  if (r.kind === "image") {
    return r.per_call_price > 0 ? `${usd(r.per_call_price)}/次` : "免费"
  }
  if (r.input_price === 0 && r.output_price === 0) return "免费"
  return `${usd(r.input_price)} / ${usd(r.output_price)} 每1M`
}

type DialogMode =
  | { kind: "create" }
  | { kind: "edit"; row: ModelPrice }
  | null

export function PricingPanel() {
  const [rows, setRows] = useState<ModelPrice[]>([])
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [query, setQuery] = useState("")
  const [dialog, setDialog] = useState<DialogMode>(null)
  const [importOpen, setImportOpen] = useState(false)

  async function load() {
    setLoading(true)
    setError(null)
    try {
      setRows(await channelsAdminApi.listPricing())
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }
  useEffect(() => {
    let cancelled = false
    channelsAdminApi
      .listPricing()
      .then((nextRows) => {
        if (!cancelled) setRows(nextRows)
      })
      .catch((e) => {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e))
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [])

  const q = query.trim().toLowerCase()
  const filtered = q
    ? rows.filter(
        (r) =>
          r.model.toLowerCase().includes(q) ||
          (r.display_name ?? "").toLowerCase().includes(q) ||
          r.kind.includes(q)
      )
    : rows

  async function onDelete(r: ModelPrice) {
    if (!confirm(`确认删除「${r.model}」的计费规则？删除后该模型将不再被白名单包含。`)) return
    try {
      await channelsAdminApi.deletePricing(r.model)
      await load()
    } catch (e) {
      alert(e instanceof Error ? e.message : String(e))
    }
  }

  async function onToggle(r: ModelPrice) {
    try {
      await channelsAdminApi.upsertPricing({
        model: r.model,
        kind: r.kind,
        display_name: r.display_name,
        enabled: !r.enabled,
        protocol: r.protocol,
        context_limit: r.context_limit,
        input_price: r.input_price,
        output_price: r.output_price,
        cached_input_price: r.cached_input_price,
        per_call_price: r.per_call_price,
        base_price: r.base_price,
        per_second_price: r.per_second_price,
        allowed_seconds: r.allowed_seconds,
        size_rules: r.size_rules,
      })
      await load()
    } catch (e) {
      alert(e instanceof Error ? e.message : String(e))
    }
  }

  return (
    <div className="flex flex-col gap-4">
      <p className="text-sm text-muted-foreground">
        按模型独立的计费白名单：未列出的模型会被拒绝（403）。价格按各家<b>官方美元价目表</b>填写，
        站点按「额度换算」中的汇率与倍率折算成额度。价格填 0 即免费白名单。
        修改 <code>model</code> 字段请先删除再新建。
      </p>

      <div className="flex items-center gap-2">
        <Input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="搜索模型名 / 显示名 / 功能…"
          className="max-w-xs"
        />
        <Button variant="outline" size="sm" onClick={() => void load()}>
          <RefreshCw /> 刷新
        </Button>
        <Button size="sm" onClick={() => setDialog({ kind: "create" })}>
          <Plus /> 新建模型
        </Button>
        <Button
          variant="outline"
          size="sm"
          onClick={() => setImportOpen(true)}
          title="从 NewAPI 站点批量导入模型价格"
        >
          <CloudDownload /> 从 NewAPI 导入
        </Button>
        <span className="ml-auto text-xs text-muted-foreground">
          共 {rows.length} 条
        </span>
      </div>

      {error && (
        <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
          {error}
        </div>
      )}

      <div className="rounded-lg border border-border">
        {/* 表头/单元格的间距在父级统一设置，省得每个 Head/Cell 都写一遍 */}
        <Table className="min-w-[40rem]">
          <TableHeader className="bg-muted/40 text-xs uppercase [&_th]:h-9 [&_th]:px-3 [&_th]:text-muted-foreground">
            <TableRow>
              <TableHead>模型</TableHead>
              <TableHead>显示名</TableHead>
              <TableHead>功能</TableHead>
              <TableHead>协议</TableHead>
              <TableHead className="text-right">价格（美元）</TableHead>
              <TableHead className="text-right">上下文</TableHead>
              <TableHead>启用</TableHead>
              <TableHead className="text-right">操作</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody className="[&_td]:px-3 [&_td]:py-2">
            {loading && (
              <TableRow>
                <TableCell colSpan={8} className="py-6 text-center text-muted-foreground">
                  加载中…
                </TableCell>
              </TableRow>
            )}
            {!loading && filtered.length === 0 && (
              <TableRow>
                <TableCell colSpan={8} className="py-6 text-center text-muted-foreground">
                  暂无计费规则。点击「新建模型」添加白名单条目。
                </TableCell>
              </TableRow>
            )}
            {filtered.map((r) => (
              <TableRow key={r.id}>
                <TableCell className="font-mono text-xs">{r.model}</TableCell>
                <TableCell className="text-muted-foreground">
                  {r.display_name ?? "—"}
                </TableCell>
                <TableCell>
                  <span className="rounded bg-muted px-1.5 py-0.5 font-mono text-xs">
                    {r.kind}
                  </span>
                </TableCell>
                <TableCell>
                  <span className="rounded bg-muted px-1.5 py-0.5 font-mono text-xs">
                    {r.protocol}
                  </span>
                </TableCell>
                <TableCell className="text-right tabular-nums">
                  {describePrice(r)}
                </TableCell>
                <TableCell className="text-right tabular-nums text-muted-foreground">
                  {r.context_limit != null
                    ? r.context_limit >= 1_000_000
                      ? `${(r.context_limit / 1_000_000).toFixed(r.context_limit % 1_000_000 === 0 ? 0 : 1)}M`
                      : r.context_limit >= 1000
                        ? `${(r.context_limit / 1000).toFixed(r.context_limit % 1000 === 0 ? 0 : 1)}K`
                        : String(r.context_limit)
                    : "自动"}
                </TableCell>
                <TableCell>
                  <button
                    onClick={() => void onToggle(r)}
                    className={
                      "rounded px-2 py-0.5 text-xs " +
                      (r.enabled
                        ? "bg-emerald-500/20 text-emerald-600"
                        : "bg-muted text-muted-foreground")
                    }
                  >
                    {r.enabled ? "启用" : "禁用"}
                  </button>
                </TableCell>
                <TableCell className="text-right">
                  <div className="inline-flex items-center gap-1">
                    <Button
                      size="sm"
                      variant="ghost"
                      onClick={() => setDialog({ kind: "edit", row: r })}
                    >
                      <Pencil />
                    </Button>
                    <Button
                      size="sm"
                      variant="ghost"
                      onClick={() => void onDelete(r)}
                    >
                      <Trash2 />
                    </Button>
                  </div>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </div>

      {dialog && (
        <PricingDialog
          mode={dialog}
          onClose={() => setDialog(null)}
          onSaved={async () => {
            setDialog(null)
            await load()
          }}
        />
      )}

      <NewApiImportDialog
        open={importOpen}
        onClose={() => setImportOpen(false)}
        onImported={load}
      />
    </div>
  )
}

type FormState = PricingInput

function PricingDialog({
  mode,
  onClose,
  onSaved,
}: {
  mode: { kind: "create" } | { kind: "edit"; row: ModelPrice }
  onClose: () => void
  onSaved: () => void | Promise<void>
}) {
  const initial: FormState =
    mode.kind === "edit"
      ? {
          model: mode.row.model,
          kind: mode.row.kind,
          display_name: mode.row.display_name,
          enabled: mode.row.enabled,
          protocol: mode.row.protocol,
          context_limit: mode.row.context_limit,
          input_price: mode.row.input_price,
          output_price: mode.row.output_price,
          cached_input_price: mode.row.cached_input_price,
          per_call_price: mode.row.per_call_price,
          base_price: mode.row.base_price,
          per_second_price: mode.row.per_second_price,
          allowed_seconds: mode.row.allowed_seconds ?? [],
          size_rules: mode.row.size_rules ?? [],
        }
      : {
          model: "",
          kind: "chat",
          display_name: null,
          enabled: true,
          protocol: "openai",
          context_limit: null,
          input_price: 0,
          output_price: 0,
          cached_input_price: null,
          per_call_price: 0,
          base_price: 0,
          per_second_price: 0,
          allowed_seconds: [],
          size_rules: [],
        }
  const [form, setForm] = useState<FormState>(initial)
  const [saving, setSaving] = useState(false)
  const [err, setErr] = useState<string | null>(null)
  // 汇率与倍率只用于把美元价预览成额度；取不到时退回默认值，不阻塞保存。
  const [rate, setRate] = useState<Pick<
    AdminSettings,
    "usd_to_cny_rate_micro" | "price_multiplier_percent"
  > | null>(null)
  useEffect(() => {
    let cancelled = false
    adminQuotaApi
      .getSettings()
      .then((s) => {
        if (!cancelled) setRate(s)
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [])
  const usdToCnyRateMicro = rate?.usd_to_cny_rate_micro ?? 7_200_000
  const multiplierPercent = rate?.price_multiplier_percent ?? 100
  const [allModels, setAllModels] = useState<AllChannelModel[]>([])
  const [probeErrors, setProbeErrors] = useState<
    { channel: string; error: string }[]
  >([])
  const [loadingModels, setLoadingModels] = useState(true)
  const [modelPickerOpen, setModelPickerOpen] = useState(false)
  const [activeSuggestionIndex, setActiveSuggestionIndex] = useState(0)
  const dialogTitleRef = useRef<HTMLHeadingElement>(null)

  // Probe channel catalogs for model selection and binding refresh.
  useEffect(() => {
    channelsAdminApi
      .listAllChannelModels()
      .then((res) => {
        setAllModels(res.models)
        setProbeErrors(res.errors)
      })
      .catch(() => {
        setAllModels([])
        setProbeErrors([])
      })
      .finally(() => setLoadingModels(false))
  }, [mode.kind])

  // A model's function is selected below; channel discovery is function-agnostic.
  const suggestions = allModels
  const modelQuery = form.model.trim().toLowerCase()
  const matchingSuggestions = suggestions.filter(
    (m) =>
      !modelQuery ||
      m.model.toLowerCase().includes(modelQuery) ||
      m.channels.some((c) => c.name.toLowerCase().includes(modelQuery))
  )
  const filteredSuggestions = matchingSuggestions.slice(0, 50)
  const currentMatch = suggestions.find((m) => m.model === form.model)
  const compatibleChannels =
    currentMatch?.channels.filter((c) => c.protocol === form.protocol) ?? []

  const isVideo = form.kind === "video"
  const isChat = form.kind === "chat"
  const isImage = form.kind === "image"
  const sizeRules = form.size_rules ?? []
  const isGrokVideo = isGrokVideoModel(form.model)
  const allowedSeconds = effectiveAllowedSeconds(
    form.model,
    form.allowed_seconds ?? []
  )
  const sizeOptions = videoSizeOptions(
    form.model,
    sizeRules.map((rule) => rule.size)
  )
  const nextSizeOption = sizeOptions.find(
    (option) => !sizeRules.some((rule) => rule.size === option.value)
  )

  function selectModel(model: AllChannelModel) {
    setForm({
      ...form,
      model: model.model,
      display_name: form.display_name || model.model,
      ...(form.kind === "video"
        ? {
            allowed_seconds: effectiveAllowedSeconds(model.model, [4]),
            size_rules: [
              {
                size: defaultVideoSize(model.model),
                multiplier: 100,
              },
            ],
          }
        : {}),
    })
    setModelPickerOpen(false)
  }

  function updateSizeRule(idx: number, patch: Partial<VideoSizeRule>) {
    setForm({
      ...form,
      size_rules: sizeRules.map((r, i) => (i === idx ? { ...r, ...patch } : r)),
    })
  }

  function updateAllowedSecond(idx: number, seconds: number) {
    setForm({
      ...form,
      allowed_seconds: allowedSeconds.map((value, index) =>
        index === idx ? seconds : value
      ),
    })
  }

  const sortedAllowedSeconds = [...allowedSeconds].sort((a, b) => a - b)
  const previewSeconds =
    isGrokVideo && sortedAllowedSeconds.includes(8)
      ? 8
      : sortedAllowedSeconds[0]
  const previewRule = sizeRules.find(
    (rule) => SIZE_RE.test(rule.size.trim()) && rule.multiplier > 0
  )
  // 与后端 videos::compute_price 相同的取整规则，保证后台预览与实际扣费一致。
  const previewMicroUsd =
    previewSeconds && previewRule
      ? Math.round(
          (((form.base_price ?? 0) +
            (form.per_second_price ?? 0) * previewSeconds) *
            previewRule.multiplier) /
            100
        )
      : null

  async function submit() {
    setErr(null)
    if (currentMatch && compatibleChannels.length === 0) {
      setErr(`该模型没有 ${form.protocol} 协议的可用渠道`)
      return
    }
    const submittedSeconds = [...allowedSeconds]
    if (isVideo) {
      if (
        submittedSeconds.some(
          (seconds) => !Number.isInteger(seconds) || seconds <= 0
        )
      ) {
        setErr("支持时长必须全部为正整数")
        return
      }
      if (submittedSeconds.length === 0) {
        setErr("允许时长不能为空")
        return
      }
      if (new Set(submittedSeconds).size !== submittedSeconds.length) {
        setErr("支持时长不能重复")
        return
      }
      if (sizeRules.length === 0) {
        setErr("至少需要一个分辨率档位")
        return
      }
      for (const r of sizeRules) {
        if (!SIZE_RE.test(r.size.trim())) {
          setErr(`分辨率格式不合法：「${r.size}」，应形如 1280x720`)
          return
        }
        if (!(r.multiplier > 0) || !Number.isInteger(r.multiplier)) {
          setErr(`倍率必须为正整数（百分比，100=原价），分辨率「${r.size}」`)
          return
        }
      }
    }
    setSaving(true)
    try {
      await channelsAdminApi.upsertPricing({
        ...form,
        channel_ids: currentMatch
          ? compatibleChannels.map((c) => c.id)
          : undefined,
        display_name: form.display_name?.toString().trim() || null,
        allowed_seconds: isVideo
          ? submittedSeconds.sort((a, b) => a - b)
          : null,
        size_rules: isVideo
          ? sizeRules.map((r) => ({ size: r.size.trim(), multiplier: r.multiplier }))
          : null,
      })
      await onSaved()
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    } finally {
      setSaving(false)
    }
  }

  // 父级用 {dialog && <PricingDialog/>} 控制挂载，所以这里常开，关闭走 onClose
  return (
    <Dialog open onOpenChange={(next) => !next && onClose()}>
      <DialogContent
        onOpenAutoFocus={(event) => {
          event.preventDefault()
          dialogTitleRef.current?.focus()
        }}
        className="flex max-h-[min(92vh,52rem)] flex-col gap-0 overflow-hidden rounded-3xl border-border/80 bg-card p-0 shadow-2xl sm:max-w-xl [&_[data-slot=dialog-close]]:right-5 [&_[data-slot=dialog-close]]:top-5 [&_[data-slot=dialog-close]]:rounded-lg [&_[data-slot=dialog-close]]:p-1"
      >
        <DialogHeader className="gap-1 border-b border-border/60 bg-muted/15 px-6 py-5 pr-14">
          <DialogTitle
            ref={dialogTitleRef}
            tabIndex={-1}
            className="text-lg leading-6 outline-none"
          >
            {mode.kind === "create" ? "新建计费规则" : "编辑计费规则"}
          </DialogTitle>
          <DialogDescription className="text-xs leading-5">
            {mode.kind === "create"
              ? "选择上游模型，并设置它的功能、协议与计费价格。"
              : "调整展示信息与计费方式，模型 ID 保持不变。"}
          </DialogDescription>
          {mode.kind === "edit" && (
            <div className="flex flex-wrap items-center gap-2 pt-1.5 text-xs">
              <span className="max-w-full truncate font-mono font-medium text-foreground">
                {form.model}
              </span>
              <span className="rounded-full bg-primary/10 px-2 py-0.5 font-medium text-primary">
                {KIND_LABELS[form.kind]}
              </span>
              <span className="rounded-full bg-muted px-2 py-0.5 text-muted-foreground">
                {PROTOCOL_LABELS[form.protocol]}
              </span>
            </div>
          )}
        </DialogHeader>

        <div className="min-h-0 flex-1 overflow-y-auto px-6 py-5">
          <div className="space-y-6">
            {mode.kind === "create" && (
              <section className="space-y-3.5">
                <div className="flex items-start justify-between gap-4">
                  <div>
                    <Label htmlFor="pricing-model">模型 ID</Label>
                    <p className="mt-1 text-xs leading-5 text-muted-foreground">
                      搜索已启用渠道，也可以直接填写自定义模型 ID。
                    </p>
                  </div>
                  {!loadingModels && suggestions.length > 0 && (
                    <span className="shrink-0 rounded-full bg-primary/8 px-2.5 py-1 text-[11px] font-medium text-primary">
                      {suggestions.length} 个可用
                    </span>
                  )}
                </div>
                <div>
                  <div className="relative">
                    <Search className="pointer-events-none absolute left-3.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
                    <Input
                      id="pricing-model"
                      role="combobox"
                      aria-autocomplete="list"
                      aria-controls="pricing-model-options"
                      aria-expanded={modelPickerOpen}
                      aria-activedescendant={
                        modelPickerOpen && filteredSuggestions[activeSuggestionIndex]
                          ? `pricing-model-${activeSuggestionIndex}`
                          : undefined
                      }
                      value={form.model}
                      onFocus={() => setModelPickerOpen(true)}
                      onBlur={() =>
                        window.setTimeout(() => setModelPickerOpen(false), 160)
                      }
                      onChange={(e) => {
                        setForm({ ...form, model: e.target.value })
                        setActiveSuggestionIndex(0)
                        setModelPickerOpen(true)
                      }}
                      onKeyDown={(e) => {
                        if (e.key === "Escape") {
                          setModelPickerOpen(false)
                        } else if (e.key === "ArrowDown") {
                          e.preventDefault()
                          setModelPickerOpen(true)
                          setActiveSuggestionIndex((index) =>
                            Math.min(index + 1, filteredSuggestions.length - 1)
                          )
                        } else if (e.key === "ArrowUp") {
                          e.preventDefault()
                          setActiveSuggestionIndex((index) =>
                            Math.max(index - 1, 0)
                          )
                        } else if (
                          e.key === "Enter" &&
                          modelPickerOpen &&
                          filteredSuggestions[activeSuggestionIndex]
                        ) {
                          e.preventDefault()
                          selectModel(filteredSuggestions[activeSuggestionIndex])
                        }
                      }}
                      placeholder="搜索模型名称或渠道"
                      autoComplete="off"
                      className="h-11 bg-background pl-10 pr-10 font-mono"
                    />
                    <ChevronDown
                      className={`pointer-events-none absolute right-3.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground transition-transform ${
                        modelPickerOpen ? "rotate-180" : ""
                      }`}
                    />
                  </div>

                  {modelPickerOpen && (
                    <div
                      id="pricing-model-options"
                      role="listbox"
                      className="mt-2 overflow-hidden rounded-xl border border-border/80 bg-popover shadow-lg shadow-black/5"
                    >
                      <div className="flex items-center justify-between border-b border-border/60 bg-muted/30 px-3 py-2 text-[11px] text-muted-foreground">
                        <span>
                          {loadingModels
                            ? "正在读取渠道模型…"
                            : modelQuery
                              ? `${matchingSuggestions.length} 个匹配结果`
                              : "选择一个已探测到的模型"}
                        </span>
                        {matchingSuggestions.length > filteredSuggestions.length && (
                          <span>仅显示前 {filteredSuggestions.length} 个</span>
                        )}
                      </div>
                      {loadingModels ? (
                        <div className="flex items-center justify-center gap-2 px-4 py-8 text-sm text-muted-foreground">
                          <LoaderCircle className="size-4 animate-spin" />
                          正在加载候选模型
                        </div>
                      ) : filteredSuggestions.length > 0 ? (
                        <div className="max-h-56 overflow-y-auto p-1.5">
                          {filteredSuggestions.map((model, index) => {
                            const isSelected = model.model === form.model
                            const protocols = Array.from(
                              new Set(model.channels.map((channel) => channel.protocol))
                            )

                            return (
                              <button
                                id={`pricing-model-${index}`}
                                key={model.model}
                                type="button"
                                role="option"
                                aria-selected={isSelected}
                                className={`group flex w-full items-center gap-3 rounded-lg px-3 py-2.5 text-left transition-colors focus:outline-none ${
                                  index === activeSuggestionIndex
                                    ? "bg-accent"
                                    : "hover:bg-accent/70"
                                }`}
                                onMouseEnter={() => setActiveSuggestionIndex(index)}
                                onMouseDown={(e) => e.preventDefault()}
                                onClick={() => selectModel(model)}
                              >
                                <span className="flex size-7 shrink-0 items-center justify-center rounded-lg border border-border/70 bg-background text-muted-foreground shadow-xs">
                                  {isSelected ? (
                                    <Check className="size-3.5 text-primary" />
                                  ) : (
                                    <span className="size-1.5 rounded-full bg-muted-foreground/45" />
                                  )}
                                </span>
                                <span className="min-w-0 flex-1">
                                  <span className="block truncate font-mono text-sm font-medium text-foreground">
                                    {model.model}
                                  </span>
                                  <span className="mt-0.5 block truncate text-xs text-muted-foreground">
                                    {model.channels.map((channel) => channel.name).join("、")}
                                  </span>
                                </span>
                                <span className="hidden shrink-0 items-center gap-1 sm:flex">
                                  {protocols.map((protocol) => (
                                    <span
                                      key={protocol}
                                      className="rounded-md bg-muted px-1.5 py-0.5 text-[10px] font-medium uppercase text-muted-foreground"
                                    >
                                      {protocol}
                                    </span>
                                  ))}
                                </span>
                              </button>
                            )
                          })}
                        </div>
                      ) : (
                        <div className="px-4 py-7 text-center">
                          <p className="text-sm font-medium">没有找到匹配模型</p>
                          <p className="mt-1 text-xs leading-5 text-muted-foreground">
                            仍可将“{form.model.trim()}”作为自定义模型 ID 保存。
                          </p>
                        </div>
                      )}
                    </div>
                  )}
                </div>

                <div aria-live="polite">
                  {loadingModels ? (
                    <p className="flex items-center gap-1.5 text-xs text-muted-foreground">
                      <LoaderCircle className="size-3.5 animate-spin" />
                      正在探测启用渠道
                    </p>
                  ) : currentMatch ? (
                    compatibleChannels.length > 0 ? (
                      <p className="flex items-center gap-1.5 text-xs text-emerald-600 dark:text-emerald-400">
                        <CheckCircle2 className="size-3.5" />
                        将绑定到：{compatibleChannels.map((c) => c.name).join("、")}
                      </p>
                    ) : (
                      <p className="flex items-center gap-1.5 text-xs text-amber-600 dark:text-amber-400">
                        <CircleAlert className="size-3.5" />
                        该模型没有 {PROTOCOL_LABELS[form.protocol]} 协议的可用渠道
                      </p>
                    )
                  ) : form.model.trim() ? (
                    <p className="flex items-start gap-1.5 text-xs leading-5 text-amber-600 dark:text-amber-400">
                      <CircleAlert className="mt-0.5 size-3.5 shrink-0" />
                      未在渠道中找到；将按自定义模型 ID 保存，请确认上游支持。
                    </p>
                  ) : (
                    <p className="text-xs text-muted-foreground">
                      候选模型来自所有启用渠道的实时探测。
                    </p>
                  )}
                </div>

                {probeErrors.length > 0 && (
                  <div className="flex items-start gap-2 rounded-lg border border-amber-500/25 bg-amber-500/8 px-3 py-2 text-xs leading-5 text-amber-700 dark:text-amber-300">
                    <CircleAlert className="mt-0.5 size-3.5 shrink-0" />
                    <span>
                      {probeErrors.map((e) => e.channel).join("、")} 探测失败，
                      不影响其他渠道。
                    </span>
                  </div>
                )}

                <div className="grid gap-3 sm:grid-cols-2">
                  <div className="space-y-1.5">
                    <Label>功能</Label>
                    <Select
                      value={form.kind}
                      onValueChange={(value) => {
                        const nextKind = value as ChannelKind
                        setForm({
                          ...form,
                          kind: nextKind,
                          ...(nextKind === "video"
                            ? {
                                protocol: "openai" as ChannelProtocol,
                                allowed_seconds: effectiveAllowedSeconds(
                                  form.model,
                                  form.allowed_seconds?.length
                                    ? form.allowed_seconds
                                    : [4]
                                ),
                                size_rules: sizeRules.length
                                  ? sizeRules
                                  : [
                                      {
                                        size: defaultVideoSize(form.model),
                                        multiplier: 100,
                                      },
                                    ],
                              }
                            : {}),
                        })
                      }}
                    >
                      <SelectTrigger className="w-full">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {KINDS.map((kind) => (
                          <SelectItem key={kind} value={kind}>
                            {KIND_LABELS[kind]}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </div>
                  <div className="space-y-1.5">
                    <Label>协议</Label>
                    <Select
                      value={form.protocol}
                      disabled={form.kind === "video"}
                      onValueChange={(value) =>
                        setForm({
                          ...form,
                          protocol: value as ChannelProtocol,
                        })
                      }
                    >
                      <SelectTrigger className="w-full">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {PROTOCOLS.map((protocol) => (
                          <SelectItem key={protocol} value={protocol}>
                            {PROTOCOL_LABELS[protocol]}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </div>
                </div>
              </section>
            )}

            <section className="space-y-1.5">
              <Label htmlFor="pricing-display-name">显示名称</Label>
              <Input
                id="pricing-display-name"
                value={form.display_name ?? ""}
                onChange={(e) =>
                  setForm({ ...form, display_name: e.target.value })
                }
                placeholder={form.model || "例如：GPT-4o mini"}
              />
            </section>

            {isChat && (
              <section className="rounded-xl border border-border/80 bg-muted/20 p-3.5">
                <div className="mb-3">
                  <h3 className="text-sm font-medium">计费设置</h3>
                  <p className="mt-0.5 text-xs text-muted-foreground">
                    按官方价目表填写<b>每 100 万 token 的美元单价</b>，对话结束后按实际用量结算。
                  </p>
                </div>
                <div className="grid grid-cols-2 gap-3">
                  <UsdPriceField
                    label="输入 / 1M tokens"
                    value={form.input_price ?? 0}
                    onChange={(micro) => setForm({ ...form, input_price: micro })}
                    usdToCnyRateMicro={usdToCnyRateMicro}
                    multiplierPercent={multiplierPercent}
                  />
                  <UsdPriceField
                    label="输出 / 1M tokens"
                    value={form.output_price ?? 0}
                    onChange={(micro) => setForm({ ...form, output_price: micro })}
                    usdToCnyRateMicro={usdToCnyRateMicro}
                    multiplierPercent={multiplierPercent}
                  />
                  <UsdPriceField
                    label="缓存输入 / 1M tokens"
                    value={form.cached_input_price ?? null}
                    onChange={(micro) =>
                      setForm({ ...form, cached_input_price: micro > 0 ? micro : null })
                    }
                    usdToCnyRateMicro={usdToCnyRateMicro}
                    multiplierPercent={multiplierPercent}
                    placeholder="留空＝按输入价"
                  />
                  <div>
                    <Label>上下文</Label>
                    <Input
                      type="number"
                      min={0}
                      value={form.context_limit ?? ""}
                      onChange={(e) => {
                        const value = Math.floor(Number(e.target.value))
                        setForm({
                          ...form,
                          context_limit: value > 0 ? value : null,
                        })
                      }}
                      placeholder="自动推断"
                    />
                  </div>
                </div>
              </section>
            )}

            {isImage && (
              <section className="rounded-xl border border-border/80 bg-muted/20 p-3.5">
                <div className="mb-3">
                  <h3 className="text-sm font-medium">计费设置</h3>
                  <p className="mt-0.5 text-xs text-muted-foreground">
                    图像接口不返回 token 用量，按<b>每次调用的美元单价</b>计费。
                  </p>
                </div>
                <div className="grid grid-cols-2 gap-3">
                  <UsdPriceField
                    label="每次调用"
                    value={form.per_call_price ?? 0}
                    onChange={(micro) => setForm({ ...form, per_call_price: micro })}
                    usdToCnyRateMicro={usdToCnyRateMicro}
                    multiplierPercent={multiplierPercent}
                  />
                </div>
              </section>
            )}

            {isVideo && (
              <>
                <section className="rounded-xl border border-border/80 bg-muted/20 p-3.5">
                  <div className="mb-3">
                    <h3 className="text-sm font-medium">计费设置</h3>
                    <p className="mt-0.5 text-xs text-muted-foreground">
                      总价由基础价、生成时长和分辨率倍率共同决定，按美元填写。
                    </p>
                  </div>
                  <div className="grid grid-cols-2 gap-3">
                    <UsdPriceField
                      label="基础价"
                      value={form.base_price ?? 0}
                      onChange={(micro) => setForm({ ...form, base_price: micro })}
                      usdToCnyRateMicro={usdToCnyRateMicro}
                      multiplierPercent={multiplierPercent}
                    />
                    <UsdPriceField
                      label="每秒价格"
                      value={form.per_second_price ?? 0}
                      onChange={(micro) => setForm({ ...form, per_second_price: micro })}
                      usdToCnyRateMicro={usdToCnyRateMicro}
                      multiplierPercent={multiplierPercent}
                    />
                  </div>
                  <div
                    aria-live="polite"
                    className="mt-3 flex items-center justify-between gap-3 rounded-lg border border-primary/10 bg-primary/5 px-3 py-2"
                  >
                    <span className="text-xs text-muted-foreground">价格示例</span>
                    {previewMicroUsd !== null && previewSeconds && previewRule ? (
                      <span className="text-sm font-semibold tabular-nums text-foreground">
                        {previewSeconds} 秒 · {displayVideoSize(previewRule.size)} ={" "}
                        {formatQuota(
                          microQuotaForMicroUsd(
                            previewMicroUsd,
                            usdToCnyRateMicro,
                            multiplierPercent
                          )
                        )}{" "}
                        元
                      </span>
                    ) : (
                      <span className="text-xs text-muted-foreground">
                        完善时长与分辨率后显示
                      </span>
                    )}
                  </div>
                </section>

                <section className="space-y-2">
                  <div>
                    <Label>支持时长</Label>
                    <p className="mt-0.5 text-xs text-muted-foreground">
                      决定用户生成视频时可以选择的秒数。
                    </p>
                  </div>
                  {isGrokVideo ? (
                    <div className="rounded-lg border border-primary/15 bg-primary/5 px-3 py-2.5">
                      <div className="flex items-center justify-between gap-3">
                        <span className="text-sm font-medium">连续时长</span>
                        <span className="text-sm font-semibold tabular-nums text-primary">
                          1–15 秒
                        </span>
                      </div>
                      <p className="mt-1 text-xs text-muted-foreground">
                        Grok 支持用户按 1 秒步长自由选择，保存时自动应用完整范围。
                      </p>
                    </div>
                  ) : (
                    <div className="flex flex-col gap-2">
                      {allowedSeconds.map((seconds, index) => (
                        <div
                          key={index}
                          className="grid grid-cols-[minmax(0,1fr)_2rem] items-center gap-2"
                        >
                          <div className="relative">
                            <Input
                              type="number"
                              min={1}
                              step={1}
                              value={seconds || ""}
                              onChange={(e) =>
                                updateAllowedSecond(index, Number(e.target.value))
                              }
                              className="pr-9"
                              aria-label={`第 ${index + 1} 个支持时长`}
                            />
                            <span className="pointer-events-none absolute inset-y-0 right-3 flex items-center text-xs text-muted-foreground">
                              秒
                            </span>
                          </div>
                          <Button
                            size="icon-sm"
                            variant="ghost"
                            type="button"
                            aria-label={`删除 ${seconds} 秒`}
                            onClick={() =>
                              setForm({
                                ...form,
                                allowed_seconds: allowedSeconds.filter(
                                  (_, itemIndex) => itemIndex !== index
                                ),
                              })
                            }
                          >
                            <Trash2 />
                          </Button>
                        </div>
                      ))}
                      {allowedSeconds.length === 0 && (
                        <div className="rounded-lg border border-dashed border-border px-3 py-4 text-center text-xs text-muted-foreground">
                          还没有支持时长，添加一个即可开始计价。
                        </div>
                      )}
                      <Button
                        size="sm"
                        variant="outline"
                        type="button"
                        onClick={() =>
                          setForm({
                            ...form,
                            allowed_seconds: [
                              ...allowedSeconds,
                              Math.max(3, ...allowedSeconds) + 1,
                            ],
                          })
                        }
                        className="w-full border-dashed text-muted-foreground"
                      >
                        <Plus /> 添加时长
                      </Button>
                    </div>
                  )}
                </section>

                <section className="space-y-3">
                  <div>
                    <Label>分辨率与价格</Label>
                    <p className="mt-0.5 text-xs text-muted-foreground">
                      100% 为原价，例如 150% 表示 1.5 倍价格。
                    </p>
                  </div>
                  <div className="grid grid-cols-[minmax(0,1fr)_7rem_2rem] items-center gap-2 px-1 text-[11px] text-muted-foreground">
                    <span>分辨率</span>
                    <span>价格倍率</span>
                    <span className="sr-only">操作</span>
                  </div>
                  <div className="flex flex-col gap-2">
                    {sizeRules.map((r, idx) => (
                      <div
                        key={idx}
                        className="grid grid-cols-[minmax(0,1fr)_7rem_2rem] items-center gap-2"
                      >
                        <Select
                          value={r.size}
                          onValueChange={(value) =>
                            updateSizeRule(idx, { size: value })
                          }
                        >
                          <SelectTrigger className="w-full">
                            <SelectValue placeholder="选择分辨率" />
                          </SelectTrigger>
                          <SelectContent>
                            {sizeOptions.map((option) => (
                              <SelectItem
                                key={option.value}
                                value={option.value}
                                disabled={sizeRules.some(
                                  (rule, ruleIndex) =>
                                    ruleIndex !== idx && rule.size === option.value
                                )}
                              >
                                {option.label}
                              </SelectItem>
                            ))}
                          </SelectContent>
                        </Select>
                        <div className="relative">
                          <Input
                            type="number"
                            min={1}
                            step="1"
                            value={r.multiplier}
                            onChange={(e) =>
                              updateSizeRule(idx, {
                                multiplier: Number(e.target.value) || 0,
                              })
                            }
                            className="pr-7"
                          />
                          <span className="pointer-events-none absolute inset-y-0 right-2.5 flex items-center text-xs text-muted-foreground">
                            %
                          </span>
                        </div>
                        <Button
                          size="icon-sm"
                          variant="ghost"
                          type="button"
                          aria-label={`删除 ${r.size || "分辨率"}`}
                          onClick={() =>
                            setForm({
                              ...form,
                              size_rules: sizeRules.filter((_, i) => i !== idx),
                            })
                          }
                        >
                          <Trash2 />
                        </Button>
                      </div>
                    ))}
                    {sizeRules.length === 0 && (
                      <div className="rounded-lg border border-dashed border-border px-3 py-4 text-center text-xs text-muted-foreground">
                        还没有分辨率，添加一个即可开始计价。
                      </div>
                    )}
                    <Button
                      size="sm"
                      variant="outline"
                      type="button"
                      disabled={!nextSizeOption}
                      onClick={() =>
                        setForm({
                          ...form,
                          size_rules: [
                            ...sizeRules,
                            {
                              size:
                                nextSizeOption?.value ??
                                defaultVideoSize(form.model),
                              multiplier: 100,
                            },
                          ],
                        })
                      }
                      className="w-full border-dashed text-muted-foreground"
                    >
                      <Plus /> 添加分辨率
                    </Button>
                  </div>
                </section>
              </>
            )}

            {mode.kind === "edit" && (
              <details className="group rounded-xl border border-border/80">
                <summary className="flex cursor-pointer list-none items-center justify-between gap-3 px-3.5 py-3 text-sm font-medium marker:hidden">
                  <span>
                    高级设置
                    <span className="ml-2 text-xs font-normal text-muted-foreground">
                      模型 ID、功能与协议
                    </span>
                  </span>
                  <span
                    aria-hidden="true"
                    className="text-muted-foreground transition-transform group-open:rotate-180"
                  >
                    ⌄
                  </span>
                </summary>
                <div className="space-y-3 border-t border-border/70 px-3.5 py-3">
                  <div>
                    <Label>模型 ID</Label>
                    <Input value={form.model} disabled />
                  </div>
                  <div className="grid grid-cols-2 gap-3">
                    <div>
                      <Label>功能</Label>
                      <Select
                        value={form.kind}
                        onValueChange={(value) =>
                          setForm({
                            ...form,
                            kind: value as ChannelKind,
                            ...(value === "video"
                              ? { protocol: "openai" as ChannelProtocol }
                              : {}),
                          })
                        }
                      >
                        <SelectTrigger className="w-full">
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          {KINDS.map((kind) => (
                            <SelectItem key={kind} value={kind}>
                              {KIND_LABELS[kind]}
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                    </div>
                    <div>
                      <Label>协议</Label>
                      <Select
                        value={form.protocol}
                        disabled={form.kind === "video"}
                        onValueChange={(value) =>
                          setForm({
                            ...form,
                            protocol: value as ChannelProtocol,
                          })
                        }
                      >
                        <SelectTrigger className="w-full">
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          {PROTOCOLS.map((protocol) => (
                            <SelectItem key={protocol} value={protocol}>
                              {PROTOCOL_LABELS[protocol]}
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                    </div>
                  </div>
                  <p className="text-xs text-muted-foreground">
                    修改功能或协议后，保存时会重新匹配可用渠道。
                  </p>
                </div>
              </details>
            )}

            <label className="flex cursor-pointer items-center gap-3 rounded-xl border border-border/80 px-3.5 py-3 transition-colors hover:bg-muted/30 has-[:checked]:border-primary/25 has-[:checked]:bg-primary/[0.04]">
              <input
                type="checkbox"
                checked={form.enabled ?? true}
                onChange={(e) =>
                  setForm({ ...form, enabled: e.target.checked })
                }
                className="size-4 accent-primary"
              />
              <span className="min-w-0">
                <span className="block text-sm font-medium">启用此模型</span>
                <span className="block text-xs text-muted-foreground">
                  启用后模型会出现在对应功能的可用列表中。
                </span>
              </span>
            </label>

            {mode.kind === "edit" && currentMatch && compatibleChannels.length === 0 && (
              <div className="rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-xs text-amber-700 dark:text-amber-300">
                当前没有支持 {PROTOCOL_LABELS[form.protocol]} 协议的可用渠道。
              </div>
            )}

            {err && (
              <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
                {err}
              </div>
            )}
          </div>
        </div>

        <DialogFooter className="flex-row justify-end border-t border-border/60 bg-muted/10 px-6 py-4">
          <Button variant="outline" onClick={onClose} disabled={saving}>
            取消
          </Button>
          <Button
            onClick={() => void submit()}
            disabled={saving || !form.model.trim()}
            className="min-w-24"
          >
            {saving
              ? "保存中…"
              : mode.kind === "create"
                ? "创建规则"
                : "保存更改"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
