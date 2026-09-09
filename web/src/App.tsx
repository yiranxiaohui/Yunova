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

function Ready({ children }: { children: React.ReactNode }) {
  const { state } = useAuth()
  if (state.status === "loading") return <Loading />
  if (state.status === "setup") return <Navigate to="/setup" replace />
  return <>{children}</>
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
            <Route
              path="/"
              element={
                <Ready>
                  <ChatPage />
                </Ready>
              }
            />
            <Route
              path="/c/:id"
              element={
                <Protected>
                  <ChatPage />
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

            <Route
              path="/w/:id"
              element={
                <Protected>
                  <ChatPage />
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
