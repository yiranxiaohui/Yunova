import { cn } from "@/lib/utils"

export function BrandMark({
  className,
  subtitle,
}: {
  className?: string
  subtitle?: string
}) {
  return (
    <div className={cn("flex items-center gap-3", className)}>
      <div className="relative shrink-0">
        <div className="absolute inset-1 rounded-xl bg-primary/40 blur-md" />
        <img
          src="/logo.png"
          alt="Yunova"
          className="relative size-10 rounded-xl ring-1 ring-white/15 shadow-panel"
        />
      </div>
      <div className="flex flex-col leading-tight">
        <span className="bg-gradient-to-r from-foreground to-foreground/65 bg-clip-text text-xl font-semibold tracking-[-0.035em] text-transparent">
          Yunova
        </span>
        {subtitle && (
          <span className="mt-0.5 text-[11px] tracking-wide text-muted-foreground">
            {subtitle}
          </span>
        )}
      </div>
    </div>
  )
}
