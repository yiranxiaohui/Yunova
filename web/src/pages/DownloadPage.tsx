import { useEffect, useMemo, useState } from "react"
import { Link } from "react-router-dom"
import {
  ArrowLeft,
  Apple,
  Check,
  Copy,
  Download,
  Laptop,
  Loader2,
  MonitorSmartphone,
  ShieldCheck,
  Terminal,
} from "lucide-react"
import { toast } from "sonner"
import { Button } from "@/components/ui/button"
import { cn } from "@/lib/utils"
import {
  DESKTOP_BUILDS,
  OS_LABEL,
  RELEASES_URL,
  assetName,
  downloadUrl,
  fetchLatestRelease,
  guessOs,
  preferredBuild,
  type DesktopBuild,
  type DesktopOs,
  type ReleaseInfo,
} from "@/lib/downloads"

/**
 * Desktop client download page.
 *
 * The client is what turns the user's own machine into an execution target for
 * work mode, so this page's job is not just to hand over a file: it must also
 * say what the binary is allowed to do, because that is the question a user
 * asks before running something that can execute commands at home.
 *
 * The release listing is fetched from GitHub and may be unavailable; every
 * button therefore falls back to the "latest release" page instead of
 * rendering a dead link or an empty page.
 */
export default function DownloadPage() {
  const [release, setRelease] = useState<ReleaseInfo | null>(null)
  const [loading, setLoading] = useState(true)
  const [copied, setCopied] = useState(false)

  const os = useMemo(() => guessOs(), [])
  const primary = useMemo(() => preferredBuild(os), [os])

  useEffect(() => {
    const ctrl = new AbortController()
    fetchLatestRelease(ctrl.signal)
      .then((r) => setRelease(r))
      .finally(() => setLoading(false))
    return () => ctrl.abort()
  }, [])

  const grouped = useMemo(() => {
    const out: Array<[DesktopOs, DesktopBuild[]]> = []
    for (const b of DESKTOP_BUILDS) {
      const row = out.find(([k]) => k === b.os)
      if (row) row[1].push(b)
      else out.push([b.os, [b]])
    }
    return out
  }, [])

  const runSnippet = `YUNOVA_DEVICE_URL=${window.location.origin}
YUNOVA_DEVICE_TOKEN=<网页生成的配对码>
YUNOVA_DEVICE_WORKSPACE=/path/to/project
./yunova-desktop`

  const copySnippet = async () => {
    try {
      await navigator.clipboard.writeText(runSnippet)
      setCopied(true)
      setTimeout(() => setCopied(false), 2000)
    } catch {
      toast.error("复制失败，请手动选择文本")
    }
  }

  return (
    <div className="min-h-svh bg-background text-foreground">
      <header className="sticky top-0 z-30 border-b border-border/70 bg-background/85 backdrop-blur-xl">
        <div className="mx-auto flex h-14 max-w-5xl items-center gap-3 px-4 md:px-7">
          <Button asChild variant="ghost" size="icon-sm">
            <Link to="/" aria-label="返回对话">
              <ArrowLeft />
            </Link>
          </Button>
          <span className="grid size-8 shrink-0 place-items-center rounded-xl bg-primary/10 text-primary">
            <Download className="size-4" />
          </span>
          <div className="min-w-0">
            <h1 className="truncate text-sm font-semibold">下载客户端</h1>
            <p className="hidden text-[11px] text-muted-foreground sm:block">
              把工作任务跑在自己的电脑上
            </p>
          </div>
          <span className="ml-auto text-xs text-muted-foreground">
            {loading ? (
              <Loader2 className="size-3.5 animate-spin" />
            ) : release ? (
              <span className="rounded-full border border-border bg-card px-2 py-1 tabular-nums">
                {release.tag}
              </span>
            ) : null}
          </span>
        </div>
      </header>

      <main className="mx-auto w-full max-w-5xl px-4 py-8 md:px-7 md:py-12">
        <section className="flex flex-col items-center gap-5 text-center">
          <div className="relative">
            <div className="absolute inset-2 rounded-3xl bg-primary/25 blur-2xl" />
            <img
              src="/logo.png"
              alt=""
              className="relative size-16 rounded-[1.35rem] ring-1 ring-white/15 shadow-panel"
            />
          </div>
          <div>
            <h2 className="text-2xl font-semibold tracking-[-0.03em] md:text-3xl">
              Yunova 桌面客户端
            </h2>
            <p className="mx-auto mt-2 max-w-xl text-sm leading-relaxed text-muted-foreground">
              在自己的电脑上运行客户端，工作模式就能把任务派到这台机器执行：读写你指定的项目目录、
              运行命令，结果实时同步回网页和手机。
            </p>
          </div>

          <div className="flex flex-col items-center gap-2">
            {primary ? (
              <Button asChild size="lg" className="gap-2">
                <a href={downloadUrl(primary, release)} rel="noreferrer">
                  <Download className="size-4" />
                  下载 {OS_LABEL[primary.os]} 版（{primary.arch}）
                </a>
              </Button>
            ) : (
              <Button asChild size="lg" className="gap-2">
                <a href={RELEASES_URL} target="_blank" rel="noreferrer">
                  <Download className="size-4" /> 前往下载页
                </a>
              </Button>
            )}
            {!loading && !release && (
              <p className="text-xs text-muted-foreground">
                无法读取版本信息，按钮将跳转到 GitHub 最新发布页。
              </p>
            )}
          </div>
        </section>

        <section className="mt-10 grid gap-4 md:grid-cols-3">
          {grouped.map(([group, builds]) => (
            <div
              key={group}
              className="flex flex-col gap-3 rounded-2xl border border-border/70 bg-card/60 p-4 shadow-sm"
            >
              <div className="flex items-center gap-2">
                <span className="grid size-8 place-items-center rounded-xl bg-primary/10 text-primary">
                  {group === "macos" ? (
                    <Apple className="size-4" />
                  ) : group === "windows" ? (
                    <MonitorSmartphone className="size-4" />
                  ) : (
                    <Terminal className="size-4" />
                  )}
                </span>
                <span className="text-sm font-semibold">{OS_LABEL[group]}</span>
              </div>
              <div className="flex flex-col gap-2">
                {builds.map((b) => {
                  const url = downloadUrl(b, release)
                  const known = url !== RELEASES_URL
                  return (
                    <a
                      key={b.target}
                      href={url}
                      rel="noreferrer"
                      target={known ? undefined : "_blank"}
                      className={cn(
                        "flex items-center justify-between gap-2 rounded-xl border border-border/70 px-3 py-2 text-sm transition-colors hover:border-primary/40 hover:bg-primary/5",
                        primary?.target === b.target && "border-primary/45 bg-primary/5"
                      )}
                      title={known ? assetName(b) : "该架构暂未发布，前往发布页查看"}
                    >
                      <span className="min-w-0">
                        <span className="block truncate font-medium">{b.arch}</span>
                        <span className="block truncate text-[11px] text-muted-foreground">
                          {b.target}
                        </span>
                      </span>
                      <Download className="size-4 shrink-0 text-muted-foreground" />
                    </a>
                  )
                })}
              </div>
            </div>
          ))}
        </section>

        <section className="mt-10 grid gap-4 md:grid-cols-2">
          <div className="rounded-2xl border border-border/70 bg-card/60 p-5">
            <div className="flex items-center gap-2 text-sm font-semibold">
              <Laptop className="size-4 text-primary" /> 三步接入
            </div>
            <ol className="mt-3 space-y-2.5 text-sm leading-relaxed text-muted-foreground">
              <li>
                <span className="font-medium text-foreground">1. 生成配对码：</span>
                打开「新工作任务」，选择本地电脑并生成配对码（只显示一次）。
              </li>
              <li>
                <span className="font-medium text-foreground">2. 运行客户端：</span>
                解压后带上配对码与工作目录启动，客户端会主动连回本站。
              </li>
              <li>
                <span className="font-medium text-foreground">3. 派发任务：</span>
                回到网页或手机，把任务的执行位置选成这台电脑即可。
              </li>
            </ol>
            <div className="mt-4 rounded-xl bg-muted/70 p-3">
              <div className="flex items-center justify-between gap-2">
                <span className="text-[11px] text-muted-foreground">启动命令</span>
                <Button size="sm" variant="ghost" onClick={() => void copySnippet()}>
                  {copied ? <Check className="size-3.5" /> : <Copy className="size-3.5" />}
                  {copied ? "已复制" : "复制"}
                </Button>
              </div>
              <pre className="mt-1.5 overflow-auto text-[11px] leading-relaxed">
                {runSnippet}
              </pre>
            </div>
          </div>

          <div className="rounded-2xl border border-border/70 bg-card/60 p-5">
            <div className="flex items-center gap-2 text-sm font-semibold">
              <ShieldCheck className="size-4 text-primary" /> 安全边界
            </div>
            <ul className="mt-3 space-y-2.5 text-sm leading-relaxed text-muted-foreground">
              <li>
                <span className="font-medium text-foreground">默认逐条审批。</span>
                运行命令、写文件前都会弹出确认，任一登录端处理即生效。
              </li>
              <li>
                <span className="font-medium text-foreground">工作目录限定范围。</span>
                只有 <code className="rounded bg-muted px-1">YUNOVA_DEVICE_WORKSPACE</code>{" "}
                指向的目录可被改动，默认当前目录而非整个用户目录。
              </li>
              <li>
                <span className="font-medium text-foreground">不下发上游密钥。</span>
                客户端只拿到指向本站网关的会话级令牌。
              </li>
              <li>
                <span className="font-medium text-foreground">随时可撤销。</span>
                在设备列表移除后立即断开，配对码同时失效。
              </li>
            </ul>
            <p className="mt-4 text-xs text-muted-foreground">
              不想在自己的电脑上执行？工作模式的「云电脑」跑在隔离容器里，无需安装任何客户端。
            </p>
          </div>
        </section>

        <p className="mt-8 text-center text-xs text-muted-foreground">
          需要历史版本或校验文件？前往{" "}
          <a
            href={RELEASES_URL}
            target="_blank"
            rel="noreferrer"
            className="text-primary underline-offset-2 hover:underline"
          >
            GitHub 发布页
          </a>
          。
        </p>
      </main>
    </div>
  )
}
