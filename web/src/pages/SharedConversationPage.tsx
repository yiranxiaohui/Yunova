import { useEffect, useState } from "react"
import { Link, useParams } from "react-router-dom"
import { Sparkles, User } from "lucide-react"
import ReactMarkdown from "react-markdown"
import remarkGfm from "remark-gfm"
import { BrandMark } from "@/components/app/BrandMark"
import { sharingApi, type PublicSnapshot, type SnapshotMessage } from "@/lib/sharing"

function formatStamp(s: string): string {
  const candidate = s.includes("T") ? s : s.replace(" ", "T")
  const withZ = candidate.endsWith("Z") ? candidate : `${candidate}Z`
  const d = new Date(withZ)
  if (Number.isNaN(d.getTime())) return s
  return d.toLocaleString()
}

export default function SharedConversationPage() {
  const { token } = useParams<{ token: string }>()
  const [data, setData] = useState<PublicSnapshot | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [gone, setGone] = useState(false)

  useEffect(() => {
    if (!token) {
      setError("链接无效")
      setLoading(false)
      return
    }
    let cancelled = false
    setLoading(true)
    setError(null)
    setGone(false)
    sharingApi
      .getPublic(token)
      .then((d) => {
        if (!cancelled) setData(d)
      })
      .catch((e) => {
        if (cancelled) return
        const msg = e instanceof Error ? e.message : String(e)
        if (/HTTP 410|过期/.test(msg)) setGone(true)
        setError(msg)
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [token])

  return (
    <div className="flex min-h-svh flex-col bg-background text-foreground">
      <header className="sticky top-0 z-10 flex items-center justify-between gap-2 border-b border-border bg-background/80 px-3 py-3 backdrop-blur md:px-6">
        <Link to="/" className="contents">
          <BrandMark />
        </Link>
        <Link
          to="/"
          className="rounded-md px-3 py-1 text-xs text-muted-foreground transition-colors hover:bg-accent"
        >
          打开自己的 Yunova →
        </Link>
      </header>

      <main className="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-4 px-3 py-5 md:px-5 md:py-8">
        {loading && (
          <p className="text-center text-sm text-muted-foreground">加载中…</p>
        )}

        {!loading && gone && (
          <div className="rounded-xl border border-border bg-card p-6 text-center">
            <p className="text-lg font-semibold">分享已过期</p>
            <p className="mt-2 text-sm text-muted-foreground">
              这个链接的有效期已经结束，作者也可能主动撤销了它。
            </p>
          </div>
        )}

        {!loading && !gone && error && (
          <div className="rounded-xl border border-border bg-card p-6 text-center">
            <p className="text-lg font-semibold">无法加载分享</p>
            <p className="mt-2 text-sm text-muted-foreground">
              链接可能已被作者撤销，或者你复制了错误的地址。
            </p>
          </div>
        )}

        {!loading && data && (
          <>
            <section className="rounded-xl border border-border bg-card p-4 shadow-sm">
              <h1 className="text-xl font-semibold tracking-tight">
                {data.title}
              </h1>
              <p className="mt-1 text-xs text-muted-foreground">
                由 <b>{data.creator_name || "匿名"}</b> 分享 ·{" "}
                {formatStamp(data.created_at)} · 浏览 {data.view_count} 次
              </p>
              {data.system_prompt && (
                <details className="mt-3 text-sm">
                  <summary className="cursor-pointer text-xs text-muted-foreground hover:text-foreground">
                    系统提示词
                  </summary>
                  <pre className="mt-2 whitespace-pre-wrap rounded-md bg-muted/50 p-3 text-xs">
                    {data.system_prompt}
                  </pre>
                </details>
              )}
            </section>

            <section className="flex flex-col gap-4">
              {data.messages.map((m, i) => (
                <ReadonlyBubble key={i} message={m} />
              ))}
            </section>

            <footer className="mt-4 rounded-xl border border-border bg-muted/30 p-4 text-center text-xs text-muted-foreground">
              想自己也体验？{" "}
              <Link to="/" className="font-medium text-primary hover:underline">
                打开 Yunova
              </Link>
            </footer>
          </>
        )}
      </main>
    </div>
  )
}

function ReadonlyBubble({ message }: { message: SnapshotMessage }) {
  if (message.role === "user") {
    return (
      <div className="flex flex-col items-end gap-1">
        <div className="flex max-w-[92%] items-end gap-2 sm:max-w-[82%]">
          <div className="rounded-2xl rounded-tr-md bg-primary px-4 py-2.5 text-sm leading-relaxed text-primary-foreground shadow-sm">
            <p className="whitespace-pre-wrap">{message.content}</p>
          </div>
          <div className="grid size-7 shrink-0 place-items-center rounded-full bg-gradient-to-br from-primary to-chart-5 text-primary-foreground">
            <User className="size-3.5" />
          </div>
        </div>
      </div>
    )
  }

  if (message.role === "system") {
    // System rows are folded into the header details block above; if any
    // sneaks into the messages array, render it as a muted card.
    return (
      <div className="rounded-md bg-muted/40 px-3 py-2 text-xs text-muted-foreground">
        系统：{message.content}
      </div>
    )
  }

  return (
    <div className="flex flex-col items-start gap-1">
      <div className="flex max-w-[94%] items-start gap-2.5 sm:max-w-[88%]">
        <div className="grid size-7 shrink-0 place-items-center rounded-full bg-gradient-to-br from-primary to-chart-5 text-primary-foreground shadow-sm">
          <Sparkles className="size-3.5" />
        </div>
        <div className="prose prose-sm min-w-0 max-w-none rounded-2xl rounded-tl-md border border-border/70 bg-card px-4 py-2.5 text-sm leading-relaxed shadow-sm dark:prose-invert [&_*]:break-words [&_a]:break-all [&_pre]:max-w-full [&_pre]:overflow-x-auto prose-pre:border prose-pre:border-border prose-pre:bg-muted prose-img:max-w-full prose-img:rounded-xl prose-img:border prose-img:border-border prose-code:rounded prose-code:bg-muted prose-code:px-1 prose-code:py-0.5 prose-code:text-[0.85em] prose-code:font-normal prose-code:before:content-none prose-code:after:content-none [&_pre_code]:!bg-transparent [&_pre_code]:!text-foreground">
          <ReactMarkdown remarkPlugins={[remarkGfm]}>{message.content}</ReactMarkdown>
        </div>
      </div>
    </div>
  )
}
