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
  DESKTOP_REPO,
  OS_LABEL,
  RELEASES_URL,
  assetName,
  downloadUrl,
  fetchLatestRelease,
  guessOs,
  installerAsset,
  preferredBuild,
  type DesktopBuild,
  type DesktopOs,
  type ReleaseInfo,
} from "@/lib/downloads"

/**
 * Desktop app download page.
 *
 * The client is a desktop app that embeds this site and turns the user's
 * machine into an execution target, so the page has two jobs. It hands over
 * the right file — an installer for a person, the bare binary for a server —
 * and it states what the app is allowed to do, because that is the question
 * anyone asks before installing something that can run commands at home.
 *
 * The release listing is fetched from GitHub and may be unavailable; every
 * button therefore falls back to the "latest release" page instead of
 * rendering a dead link or an empty page.
 */
export default function DownloadPage() {
  const [release, setRelease] = useState<ReleaseInfo | null>(null)
  const [loading, setLoading] = useState(true)
  const [copied, setCopied] = useState(false)
  const [copiedCli, setCopiedCli] = useState(false)
  const [macCopied, setMacCopied] = useState(false)

  const os = useMemo(() => guessOs(), [])
  const primary = useMemo(() => preferredBuild(os), [os])
  const primaryInstaller = useMemo(
    () => (primary ? installerAsset(primary, release) : null),
    [primary, release]
  )

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
YUNOVA_DEVICE_WORKSPACE=/path/to/project
./yunova-desktop --headless`

  // What turns `pi` into this platform's CLI: install it, add the provider
  // package, then `/login yunova` signs in by browser approval rather than by
  // pasting a token. Built from the current origin so a self-hosted instance
  // shows its own address instead of the public one.
  const cliSnippet = `npm install -g --ignore-scripts @earendil-works/pi-coding-agent
pi install git:github.com/${DESKTOP_REPO}@main
YUNOVA_BASE_URL=${window.location.origin} pi`

  // The builds carry no Apple Developer ID, so macOS quarantines them and
  // offers the "unidentified developer" prompt. Some machines refuse outright
  // instead, and the wording it uses -- "damaged" -- reads as a corrupt
  // download and points at the Trash, so people retry the download or give
  // up. Naming the real cause next to the button is the honest fix until the
  // app is signed and notarized.
  const macQuarantineSnippet = "sudo xattr -rd com.apple.quarantine /Applications/Yunova.app"

  const copyMacSnippet = async () => {
    try {
      await navigator.clipboard.writeText(macQuarantineSnippet)
      setMacCopied(true)
      setTimeout(() => setMacCopied(false), 2000)
    } catch {
      toast.error("复制失败，请手动选择文本")
    }
  }

  const copySnippet = async () => {
    try {
      await navigator.clipboard.writeText(runSnippet)
      setCopied(true)
      setTimeout(() => setCopied(false), 2000)
    } catch {
      toast.error("复制失败，请手动选择文本")
    }
  }

  const copyCli = async () => {
    try {
      await navigator.clipboard.writeText(cliSnippet)
      setCopiedCli(true)
      setTimeout(() => setCopiedCli(false), 2000)
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
            <h1 className="truncate text-sm font-semibold">下载电脑版</h1>
            <p className="hidden text-[11px] text-muted-foreground sm:block">
              桌面应用，并把任务跑在自己的电脑上
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
              src="/logo.svg"
              alt=""
              className="relative size-16 rounded-[1.35rem] ring-1 ring-white/15 shadow-panel"
            />
          </div>
          <div>
            <h2 className="text-2xl font-semibold tracking-[-0.03em] md:text-3xl">
              Yunova 桌面端
            </h2>
            <p className="mx-auto mt-2 max-w-xl text-sm leading-relaxed text-muted-foreground">
              完整的桌面应用：在自己的窗口里开对话、跑任务，同时把这台电脑变成执行目标——
              工作模式的任务可以直接读写你指定的项目目录、运行命令，结果实时同步回网页和手机。
              关窗不断连，任务在后台继续跑，需要审批时会系统通知你。
            </p>
          </div>

          <div className="flex flex-col items-center gap-2">
            {primary ? (
              <div className="flex flex-wrap items-center justify-center gap-2">
                <Button asChild size="lg" className="gap-2">
                  <a
                    href={primaryInstaller?.url ?? downloadUrl(primary, release)}
                    rel="noreferrer"
                    title={primaryInstaller?.name ?? assetName(primary)}
                  >
                    <Download className="size-4" />
                    下载 {OS_LABEL[primary.os]} 版（{primary.arch}）
                  </a>
                </Button>
                {/* The bare binary stays one click away rather than hidden:
                    it is what a server needs, and someone looking for it
                    should not have to read the release page to find it. */}
                {primaryInstaller && (
                  <Button asChild size="lg" variant="outline" className="gap-2">
                    <a
                      href={downloadUrl(primary, release)}
                      rel="noreferrer"
                      title={assetName(primary)}
                    >
                      <Terminal className="size-4" /> 仅命令行版
                    </a>
                  </Button>
                )}
              </div>
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
            {!loading && release && !primaryInstaller && primary && (
              <p className="text-xs text-muted-foreground">
                这个版本未提供 {OS_LABEL[primary.os]} 安装包，下载的是可直接运行的二进制。
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
                  const inst = installerAsset(b, release)
                  const binary = downloadUrl(b, release)
                  const known = inst != null || binary !== RELEASES_URL
                  const href = inst?.url ?? binary
                  return (
                    <div
                      key={b.target}
                      className={cn(
                        "rounded-xl border border-border/70 px-3 py-2 text-sm transition-colors hover:border-primary/40",
                        primary?.target === b.target && "border-primary/45 bg-primary/5"
                      )}
                    >
                      <a
                        href={href}
                        rel="noreferrer"
                        target={known ? undefined : "_blank"}
                        className="flex items-center justify-between gap-2"
                        title={
                          inst?.name ??
                          (binary !== RELEASES_URL
                            ? assetName(b)
                            : "该架构暂未发布，前往发布页查看")
                        }
                      >
                        <span className="min-w-0">
                          <span className="block truncate font-medium">{b.arch}</span>
                          <span className="block truncate text-[11px] text-muted-foreground">
                            {inst ? `安装包 · ${inst.ext}` : b.target}
                          </span>
                        </span>
                        <Download className="size-4 shrink-0 text-muted-foreground" />
                      </a>
                      {/* Shown only when an installer exists, so a row never
                          offers the same file twice. */}
                      {inst && binary !== RELEASES_URL && (
                        <a
                          href={binary}
                          rel="noreferrer"
                          title={assetName(b)}
                          className="mt-1 inline-flex items-center gap-1 text-[11px] text-muted-foreground underline-offset-2 hover:text-primary hover:underline"
                        >
                          <Terminal className="size-3" /> 命令行版（服务器）
                        </a>
                      )}
                    </div>
                  )
                })}
              </div>
              {/* Only under macOS: the other platforms have no equivalent
                  step, and a warning shown to everyone would be noise that
                  makes the app look broken. */}
              {group === "macos" && (
                <div className="rounded-xl bg-muted/70 p-3">
                  <p className="text-[11px] leading-relaxed text-muted-foreground">
                    <span className="font-medium text-foreground">
                      首次打开提示「已损坏」或「无法验证开发者」？
                    </span>{" "}
                    文件是完整的。安装包尚未经 Apple 签名公证，系统会拦下从网络
                    下载的应用。先试右键应用图标 →「打开」；若仍被拦，把应用拖到
                    「应用程序」后在终端执行：
                  </p>
                  <div className="mt-2 flex items-start justify-between gap-2">
                    <code className="min-w-0 flex-1 overflow-auto text-[11px] leading-relaxed">
                      {macQuarantineSnippet}
                    </code>
                    <Button
                      size="sm"
                      variant="ghost"
                      className="shrink-0"
                      onClick={() => void copyMacSnippet()}
                    >
                      {macCopied ? (
                        <Check className="size-3.5" />
                      ) : (
                        <Copy className="size-3.5" />
                      )}
                      {macCopied ? "已复制" : "复制"}
                    </Button>
                  </div>
                </div>
              )}
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
                <span className="font-medium text-foreground">1. 安装并打开：</span>
                首次启动会该问本站地址，填好后窗口里直接就是 Yunova。
              </li>
              <li>
                <span className="font-medium text-foreground">2. 登录账号：</span>
                在「本机设置」里登录，登录成功即自动绑定这台电脑，无需配对码。
              </li>
              <li>
                <span className="font-medium text-foreground">3. 派发任务：</span>
                把任务的执行位置选成这台电脑即可；网页和手机上看到的是同一份记录。
              </li>
            </ol>
            <div className="mt-4 rounded-xl bg-muted/70 p-3">
              <div className="flex items-center justify-between gap-2">
                <span className="text-[11px] text-muted-foreground">
                  服务器上无界面运行
                </span>
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
                只有「本机设置」里选定的目录可被改动，默认不是整个用户目录。
              </li>
              <li>
                <span className="font-medium text-foreground">网页无法改本机设置。</span>
                站点界面跑在单独的窗口里，拿不到本地控制权限，因此服务器不能悄悄放宽
                审批或改工作目录。
              </li>
              <li>
                <span className="font-medium text-foreground">不保存密码。</span>
                登录后只在本机保存一枚设备令牌（仅当前用户可读），密码不落盘。
              </li>
              <li>
                <span className="font-medium text-foreground">不下发上游密钥。</span>
                客户端只拿到指向本站网关的会话级令牌。
              </li>
              <li>
                <span className="font-medium text-foreground">随时可撤销。</span>
                在设备列表移除后立即断开，设备令牌同时失效；重新登录可再次绑定。
              </li>
            </ul>
            <p className="mt-4 text-xs text-muted-foreground">
              不想在自己的电脑上执行？工作模式的「云电脑」跑在隔离容器里，无需安装任何客户端。
            </p>
          </div>
        </section>

        <section className="mt-10 rounded-2xl border border-border/70 bg-card/60 p-5">
          <div className="flex items-center gap-2 text-sm font-semibold">
            <Terminal className="size-4 text-primary" /> 命令行（用 pi 直接调用本站模型）
          </div>
          <p className="mt-3 text-sm leading-relaxed text-muted-foreground">
            和 Claude Code、Codex CLI 一样在终端里干活，但模型额度走你的 Yunova 账号。
            安装后在 pi 里运行{" "}
            <code className="rounded bg-muted px-1.5 py-0.5 text-[12px]">/login yunova</code>，
            会弹出一个登录码；在浏览器里确认后，{" "}
            <code className="rounded bg-muted px-1.5 py-0.5 text-[12px]">/model</code>{" "}
            里就能选到本站已开放的模型。全程不需要在终端输入密码，也拿不到上游密钥。
          </p>
          <div className="mt-4 rounded-xl bg-muted/70 p-3">
            <div className="flex items-center justify-between gap-2">
              <span className="text-[11px] text-muted-foreground">安装并登录</span>
              <Button size="sm" variant="ghost" onClick={() => void copyCli()}>
                {copiedCli ? <Check className="size-3.5" /> : <Copy className="size-3.5" />}
                {copiedCli ? "已复制" : "复制"}
              </Button>
            </div>
            <pre className="mt-1.5 overflow-auto text-[11px] leading-relaxed">
              {cliSnippet}
            </pre>
          </div>
          <p className="mt-3 text-xs text-muted-foreground">
            已经拿到登录码了？直接去{" "}
            <Link
              to="/cli/login"
              className="text-primary underline-offset-2 hover:underline"
            >
              授权页面
            </Link>
            。授权可随时在「Agent 令牌」里撤销。
          </p>
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
