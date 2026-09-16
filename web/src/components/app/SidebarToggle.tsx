import { PanelLeft } from "lucide-react"
import { useSidebarCollapsed } from "@/lib/sidebar-collapse"
import { useIsDesktop } from "@/lib/use-media-query"
import { cn } from "@/lib/utils"

/**
 * Hides and restores the navigation column.
 *
 * Lives in the page header rather than inside the sidebar: once the column is
 * a rail there is no obvious place in it for the control that brings it back,
 * and Doubao's layout — one square button at the top-left of the working
 * area — keeps the affordance in the same pixel whether the column is open or
 * closed. Desktop only, because the mobile sidebar is a drawer with its own
 * dismissal and a rail there would be unusable.
 */
export function SidebarToggle({ className }: { className?: string }) {
  const [collapsed, toggle] = useSidebarCollapsed()
  const isDesktop = useIsDesktop()
  if (!isDesktop) return null
  return (
    <button
      type="button"
      onClick={toggle}
      title={collapsed ? "展开侧栏" : "收起侧栏"}
      aria-label={collapsed ? "展开侧栏" : "收起侧栏"}
      aria-expanded={!collapsed}
      className={cn(
        "hidden size-8 shrink-0 place-items-center rounded-lg text-muted-foreground transition-colors hover:bg-accent/70 hover:text-foreground md:grid",
        className
      )}
    >
      <PanelLeft className="size-4" />
    </button>
  )
}
