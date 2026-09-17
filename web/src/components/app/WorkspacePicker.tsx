import { useCallback, useEffect, useState } from "react"
import { ChevronRight, CornerLeftUp, Folder, FolderGit2, Loader2 } from "lucide-react"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { browseDevice, type DeviceListing } from "@/lib/agent"
import { toast } from "sonner"

/**
 * Pick the directory a work-mode task runs in.
 *
 * Browses the *machine*, not the server. Every listing is answered by the
 * desktop client over its own socket and only for directories the user
 * authorized there, so this dialog cannot show — or select — anything outside
 * that scope. That is also why there is no free-text path field: a typed path
 * would invite the user to aim at a directory the client is then obliged to
 * refuse, and the refusal would read as a bug.
 *
 * Choosing is what makes work mode usable on a real machine: without it every
 * task ran in the one directory configured in the client, so switching
 * projects meant editing a local setting and reconnecting.
 */
export function WorkspacePicker({
  deviceId,
  open,
  onOpenChange,
  onPick,
}: {
  deviceId: number | null
  open: boolean
  onOpenChange: (open: boolean) => void
  /** Called with the chosen absolute path. */
  onPick: (path: string) => void
}) {
  const [listing, setListing] = useState<DeviceListing | null>(null)
  const [loading, setLoading] = useState(false)

  const load = useCallback(
    async (path?: string) => {
      if (deviceId == null) return
      setLoading(true)
      try {
        setListing(await browseDevice(deviceId, path))
      } catch (e) {
        // Cleared on failure rather than left in place: a stale listing under
        // an error toast reads as a directory that is still selectable.
        setListing(null)
        toast.error(`读取目录失败：${(e as Error).message}`)
      } finally {
        setLoading(false)
      }
    },
    [deviceId]
  )

  // Reloaded on every open rather than cached: directories are created and
  // renamed between two uses of this dialog, and a stale list would offer a
  // path the machine now refuses. Kicked off from a timer's first tick, which
  // is how the rest of the app keeps a fetch out of the effect body, and
  // cancelled on close so a listing that arrives late cannot repopulate a
  // dialog the user already dismissed.
  useEffect(() => {
    if (!open) return
    let cancelled = false
    const initial = setTimeout(() => {
      if (!cancelled) void load()
    }, 0)
    return () => {
      cancelled = true
      clearTimeout(initial)
    }
  }, [open, load])

  const here = listing?.path ?? null

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>选择工作目录</DialogTitle>
          <DialogDescription>
            只列出那台电脑已授权的目录。要加入新的项目目录，请在客户端的「本机设置」里添加。
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-col gap-2">
          {/* The current path is shown in full: it is what the agent will be
              able to change, so an abbreviated form would hide the one detail
              that matters before confirming. */}
          <div className="flex items-center gap-2 rounded-md border bg-muted/40 px-2 py-1.5 text-xs">
            <Folder className="size-3.5 shrink-0 text-muted-foreground" />
            <code className="min-w-0 flex-1 break-all">
              {here ?? "已授权的目录"}
            </code>
            {loading && <Loader2 className="size-3.5 animate-spin" />}
          </div>

          <div className="max-h-72 overflow-y-auto rounded-md border">
            {/* "Up" disappears at a root, because the client stops answering
                there; offering it would produce a refusal instead of a move. */}
            {listing?.parent && (
              <button
                type="button"
                className="flex w-full items-center gap-2 border-b px-3 py-2 text-left text-sm hover:bg-accent"
                onClick={() => void load(listing.parent ?? undefined)}
              >
                <CornerLeftUp className="size-3.5 text-muted-foreground" />
                上一级
              </button>
            )}
            {here && !listing?.parent && (
              <button
                type="button"
                className="flex w-full items-center gap-2 border-b px-3 py-2 text-left text-sm hover:bg-accent"
                onClick={() => void load()}
              >
                <CornerLeftUp className="size-3.5 text-muted-foreground" />
                返回已授权目录
              </button>
            )}

            {listing?.entries.map((entry) => (
              <div
                key={entry.path}
                className="flex items-center gap-1 border-b px-1 last:border-b-0"
              >
                {/* Two actions per row, because "go into" and "use this" are
                    both wanted: a project directory is usually the answer,
                    while its parent is usually just a step. */}
                <button
                  type="button"
                  className="flex min-w-0 flex-1 items-center gap-2 px-2 py-2 text-left text-sm hover:underline"
                  onClick={() => void load(entry.path)}
                >
                  {entry.repo ? (
                    <FolderGit2 className="size-3.5 shrink-0 text-primary" />
                  ) : (
                    <Folder className="size-3.5 shrink-0 text-muted-foreground" />
                  )}
                  <span className="truncate">{entry.name}</span>
                  {entry.repo && (
                    <span className="shrink-0 text-[10px] text-muted-foreground">
                      Git
                    </span>
                  )}
                  <ChevronRight className="ml-auto size-3.5 shrink-0 text-muted-foreground" />
                </button>
                <Button
                  size="sm"
                  variant="ghost"
                  className="shrink-0 text-xs"
                  onClick={() => {
                    onPick(entry.path)
                    onOpenChange(false)
                  }}
                >
                  使用
                </Button>
              </div>
            ))}

            {!loading && listing?.entries.length === 0 && (
              <p className="px-3 py-6 text-center text-xs text-muted-foreground">
                {here
                  ? "这个目录下没有子目录，可直接「使用当前目录」。"
                  : "那台电脑还没有授权任何目录，请在客户端的「本机设置」里添加。"}
              </p>
            )}
          </div>

          <div className="flex items-center justify-end gap-2">
            <Button variant="ghost" size="sm" onClick={() => onOpenChange(false)}>
              取消
            </Button>
            {/* Enabled only inside a directory: at the root screen there is no
                single "current directory" to mean. */}
            <Button
              size="sm"
              disabled={!here}
              onClick={() => {
                if (!here) return
                onPick(here)
                onOpenChange(false)
              }}
            >
              使用当前目录
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  )
}
