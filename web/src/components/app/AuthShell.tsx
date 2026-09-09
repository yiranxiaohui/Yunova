import type { ReactNode } from "react"
import { Check, ShieldCheck, Sparkles } from "lucide-react"
import { BrandMark } from "./BrandMark"

type Props = {
  eyebrow: string
  title: string
  description: string
  children: ReactNode
  footer: ReactNode
}

const FEATURES = [
  "兼容 OpenAI、Claude 与 Gemini",
  "整合 Agent、工具执行与创作工作流",
  "自托管部署，数据由你掌控",
]

export function AuthShell({
  eyebrow,
  title,
  description,
  children,
  footer,
}: Props) {
  return (
    <div className="bg-auth-grid relative flex min-h-svh items-center overflow-hidden p-4 sm:p-6 lg:p-8">
      <span className="auth-orb -left-24 top-12 size-72 bg-primary/10 blur-3xl" />
      <span className="auth-orb -bottom-28 -right-16 size-80 bg-emerald-400/10 blur-3xl" />

      <div className="glass-surface fade-up relative mx-auto grid w-full max-w-5xl overflow-hidden rounded-[2rem] lg:grid-cols-[1.05fr_0.95fr]">
        <aside className="relative hidden min-h-[660px] overflow-hidden bg-[linear-gradient(145deg,#171029_0%,#2e1760_54%,#5522a5_100%)] p-10 text-white lg:flex lg:flex-col">
          <div className="absolute -right-32 -top-24 size-96 rounded-full border border-white/10 bg-white/5" />
          <div className="absolute -bottom-32 -left-24 size-80 rounded-full border border-violet-300/15 bg-violet-300/5" />
          <div className="absolute inset-0 bg-[radial-gradient(circle_at_65%_35%,rgba(196,151,255,0.22),transparent_32%)]" />

          <div className="relative flex items-center gap-3">
            <img
              src="/logo.png"
              alt="Yunova"
              className="size-11 rounded-2xl ring-1 ring-white/20 shadow-2xl"
            />
            <div>
              <p className="text-xl font-semibold tracking-[-0.035em]">Yunova</p>
              <p className="text-[11px] tracking-[0.16em] text-violet-200/75">AGENT WORKSPACE</p>
            </div>
          </div>

          <div className="relative my-auto max-w-md">
            <span className="mb-5 inline-flex items-center gap-1.5 rounded-full border border-white/15 bg-white/10 px-3 py-1 text-xs text-violet-100 backdrop-blur">
              <Sparkles className="size-3" /> 一个空间，释放所有灵感
            </span>
            <h2 className="text-4xl font-semibold leading-tight tracking-[-0.045em]">
              和更聪明的 AI，
              <span className="bg-gradient-to-r from-violet-200 to-fuchsia-200 bg-clip-text text-transparent">
                完成更好的作品。
              </span>
            </h2>
            <p className="mt-5 text-sm leading-7 text-violet-100/70">
              连接模型与工具，将对话、任务执行和内容创作整合到一个工作空间。
            </p>

            <ul className="mt-9 space-y-3.5">
              {FEATURES.map((feature) => (
                <li key={feature} className="flex items-center gap-3 text-sm text-violet-50/85">
                  <span className="grid size-6 place-items-center rounded-full bg-white/10 ring-1 ring-white/10">
                    <Check className="size-3.5" />
                  </span>
                  {feature}
                </li>
              ))}
            </ul>
          </div>

          <div className="relative flex items-center gap-2 text-xs text-violet-200/65">
            <ShieldCheck className="size-4" /> 安全、私密、完全可控
          </div>
        </aside>

        <main className="flex min-h-[620px] flex-col justify-center bg-card/75 px-6 py-8 backdrop-blur-xl sm:px-10 lg:px-12 lg:py-10">
          <BrandMark subtitle="Agent 工作空间" className="mb-9 lg:hidden" />
          <span className="mb-3 w-fit rounded-full border border-primary/15 bg-primary/5 px-3 py-1 text-[11px] font-semibold tracking-wide text-primary">
            {eyebrow}
          </span>
          <h1 className="text-3xl font-semibold tracking-[-0.04em]">{title}</h1>
          <p className="mt-2 text-sm leading-relaxed text-muted-foreground">
            {description}
          </p>

          {children}

          <div className="mt-7 text-center text-sm text-muted-foreground">
            {footer}
          </div>
        </main>
      </div>
    </div>
  )
}
