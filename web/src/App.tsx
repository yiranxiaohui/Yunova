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
const MediaLibraryPage = lazy(() => import("@/pages/MediaLibraryPage"))
const AgentTaskPage = lazy(() => import("@/pages/AgentTaskPage"))

function Loading() {
  return (
    <div className="app-shell grid min-h-svh place-items-center text-muted-foreground">
      <div className="fade-up flex flex-col items-center gap-3">
        <div className="relative">
          <div className="absolute inset-1 rounded-2xl bg-primary/25 blur-lg" />
          <img src="/logo.png" alt="" className="relative size-12 rounded-2xl shadow-panel" />
        </div>
        <span className="text-xs tracking-[0.15em]">正在载入 YUNOVA</span>
      </div>
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

/** `/`、`/c/:id` 和 `/w/:id` 必须用同一个组件类型包裹。
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
                  <AgentTaskPage />
                </Protected>
              }
            />
            <Route
              path="/t/:id"
              element={
                <Protected>
                  <AgentTaskPage />
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

            <Route path="/w/:id" element={<ChatRoute requireAuth />} />
            <Route path="/s/:token" element={<SharedConversationPage />} />
            <Route path="*" element={<Navigate to="/" replace />} />
          </Routes>
          <Toaster position="top-center" richColors />
        </ConfirmProvider>
      </AuthProvider>
    </BrowserRouter>
  )
}
