import { lazy, Suspense } from "react"
import { BrowserRouter, Navigate, Route, Routes } from "react-router-dom"
import { AuthProvider, useAuth } from "@/lib/auth-context"
import { ConfirmProvider } from "@/lib/confirm-context"
import LoginPage from "@/pages/LoginPage"
import RegisterPage from "@/pages/RegisterPage"
import ChatPage from "@/pages/ChatPage"
import SetupPage from "@/pages/SetupPage"
import AdminPage from "@/pages/AdminPage"
import PaymentReturnPage from "@/pages/PaymentReturnPage"
import ImageStudioPage from "@/pages/ImageStudioPage"
import VideoStudioPage from "@/pages/VideoStudioPage"
import WorkflowStudioPage from "@/pages/WorkflowStudioPage"
import SharedConversationPage from "@/pages/SharedConversationPage"
import { Toaster } from "@/components/ui/sonner"

const VideoEditorPage = lazy(() => import("@/pages/VideoEditorPage"))
// Downloading the client is a once-per-machine detour, so its page never needs
// to be in the main bundle.
const DownloadPage = lazy(() => import("@/pages/DownloadPage"))
const MediaLibraryPage = lazy(() => import("@/pages/MediaLibraryPage"))
// Approving a CLI login is a once-per-machine detour, like the download page.
const CliLoginPage = lazy(() => import("@/pages/CliLoginPage"))
const AgentTaskPage = lazy(() => import("@/pages/AgentTaskPage"))

function Loading() {
  return (
    <div className="app-shell grid min-h-svh place-items-center text-muted-foreground">
      <div className="fade-up flex flex-col items-center gap-3">
        <div className="relative">
          <div className="absolute inset-1 rounded-2xl bg-primary/25 blur-lg" />
          <img src="/logo.svg" alt="" className="relative size-12 rounded-2xl shadow-panel" />
        </div>
        <span className="text-xs tracking-[0.15em]">正在载入 YUNOVA</span>
      </div>
    </div>
  )
}

/** Fallback for the work-mode chunk.
 *
 *  Switching modes is a state change from the user's point of view, so it must
 *  not flash the branded app-launch screen: that reads as "the app restarted".
 *  This keeps the shell's background and shows nothing but a quiet hint, and in
 *  practice it is rarely seen at all because the switch prefetches the chunk. */
function ModeLoading() {
  return (
    <div className="app-shell grid min-h-svh place-items-center bg-background text-muted-foreground">
      <span className="text-xs tracking-[0.12em]">正在进入工作模式…</span>
    </div>
  )
}

function Protected({ children }: { children: React.ReactNode }) {
  const { state } = useAuth()
  if (state.status === "loading") return <Loading />
  if (state.status === "setup") return <Navigate to="/setup" replace />
  if (state.status === "anon") return <Navigate to="/login" replace />
  return <>{children}</>
}

/** `/` 和 `/c/:id` 必须用同一个组件类型包裹。
 *
 * 在首页发送第一条消息时，ChatPage 会先建会话再 navigate 到
 * `/c/<id>`。若两条路由的 element 包裹类型不同（以前分别是 `Ready`
 * 和 `Protected`），React 会当成不同的子树——卸载并重新挂载 ChatPage，
 * 刚发出的消息、进行中的流以及「跳过首次加载」的 ref 全部丢失，
 * 界面因此变回空会话，只有刷新才能看到已保存的记录。 */
function ChatRoute({ requireAuth = false }: { requireAuth?: boolean }) {
  const { state } = useAuth()
  if (state.status === "loading") return <Loading />
  if (state.status === "setup") return <Navigate to="/setup" replace />
  // 首页允许游客自带密钥对话；具体会话属于账号，必须登录。
  if (requireAuth && state.status === "anon")
    return <Navigate to="/login" replace />
  return <ChatPage />
}

function AnonOnly({ children }: { children: React.ReactNode }) {
  const { state } = useAuth()
  if (state.status === "loading") return <Loading />
  if (state.status === "setup") return <Navigate to="/setup" replace />
  if (state.status === "authed") return <Navigate to="/" replace />
  return <>{children}</>
}

function SetupOnly({ children }: { children: React.ReactNode }) {
  const { state } = useAuth()
  if (state.status === "loading") return <Loading />
  if (state.status !== "setup") return <Navigate to="/" replace />
  return <>{children}</>
}

export default function App() {
  return (
    <BrowserRouter>
      <AuthProvider>
        <ConfirmProvider>
          <Routes>
            <Route
              path="/setup"
              element={
                <SetupOnly>
                  <SetupPage />
                </SetupOnly>
              }
            />
            <Route
              path="/login"
              element={
                <AnonOnly>
                  <LoginPage />
                </AnonOnly>
              }
            />
            <Route
              path="/register"
              element={
                <AnonOnly>
                  <RegisterPage />
                </AnonOnly>
              }
            />
            <Route path="/" element={<ChatRoute />} />
            <Route path="/c/:id" element={<ChatRoute requireAuth />} />
            {/* Work mode. `/t` composes a new task, `/t/:id` opens one; both
                are wrapped identically so creating a task from `/t` does not
                remount the page and lose the prompt in flight. */}
            <Route
              path="/t"
              element={
                <Protected>
                  <Suspense fallback={<ModeLoading />}>
                    <AgentTaskPage />
                  </Suspense>
                </Protected>
              }
            />
            <Route
              path="/t/:id"
              element={
                <Protected>
                  <Suspense fallback={<ModeLoading />}>
                    <AgentTaskPage />
                  </Suspense>
                </Protected>
              }
            />
            <Route
              path="/admin"
              element={
                <Protected>
                  <AdminPage />
                </Protected>
              }
            />
            <Route
              path="/payments/return"
              element={
                <Protected>
                  <PaymentReturnPage />
                </Protected>
              }
            />
            <Route
              path="/studio"
              element={
                <Protected>
                  <ImageStudioPage />
                </Protected>
              }
            />
            <Route
              path="/studio/:id"
              element={
                <Protected>
                  <ImageStudioPage />
                </Protected>
              }
            />
            <Route
              path="/videos"
              element={
                <Protected>
                  <VideoStudioPage />
                </Protected>
              }
            />
            <Route
              path="/editor"
              element={
                <Protected>
                  <Suspense fallback={<Loading />}>
                    <VideoEditorPage />
                  </Suspense>
                </Protected>
              }
            />
            <Route
              path="/editor/:id"
              element={
                <Protected>
                  <Suspense fallback={<Loading />}>
                    <VideoEditorPage />
                  </Suspense>
                </Protected>
              }
            />
            <Route
              path="/workflows"
              element={
                <Protected>
                  <WorkflowStudioPage />
                </Protected>
              }
            />
            <Route
              path="/library"
              element={
                <Protected>
                  <Suspense fallback={<Loading />}>
                    <MediaLibraryPage />
                  </Suspense>
                </Protected>
              }
            />
            <Route path="/plaza" element={<Navigate to="/library" replace />} />

            {/* Public on purpose: a user who has not signed in yet should
                still be able to fetch the client and read what it is allowed
                to do on their machine. */}
            <Route
              path="/download"
              element={
                <Suspense fallback={<Loading />}>
                  <DownloadPage />
                </Suspense>
              }
            />

            {/* Approving a CLI sign-in requires an authenticated session:
                that requirement is what makes the short user code safe, since
                knowing a code is useless without an account to approve it
                with. */}
            <Route
              path="/cli/login"
              element={
                <Protected>
                  <Suspense fallback={<Loading />}>
                    <CliLoginPage />
                  </Suspense>
                </Protected>
              }
            />

            <Route path="/s/:token" element={<SharedConversationPage />} />
            <Route path="*" element={<Navigate to="/" replace />} />
          </Routes>
          <Toaster position="top-center" richColors />
        </ConfirmProvider>
      </AuthProvider>
    </BrowserRouter>
  )
}
