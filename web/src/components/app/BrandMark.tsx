import { cn } from "@/lib/utils"

export function BrandMark({
  className,
  subtitle,
  size = "md",
}: {
  className?: string
  subtitle?: string
  /** `sm` for the sidebar header, where vertical space belongs to the
   *  conversation list rather than to branding. */
  size?: "sm" | "md"
}) {
  const small = size === "sm"
  return (
    <div className={cn("flex items-center", small ? "gap-2" : "gap-3", className)}>
      <div className="relative shrink-0">
        <div className="absolute inset-1 rounded-xl bg-primary/40 blur-md" />
        <img
          src="/logo.svg"
          alt="Yunova"
          className={cn(
            "relative rounded-xl ring-1 ring-white/15 shadow-panel",
            small ? "size-7" : "size-10"
          )}
        />
      </div>
      <div className="flex flex-col leading-tight">
        <span
          className={cn(
            "bg-gradient-to-r from-foreground to-foreground/65 bg-clip-text font-semibold tracking-[-0.035em] text-transparent",
            small ? "text-base" : "text-xl"
          )}
        >
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
