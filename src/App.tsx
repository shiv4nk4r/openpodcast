import { Navigate, Route, Routes } from "react-router-dom";
import { ErrorBoundary } from "./components/ErrorBoundary";
import Dashboard from "./pages/Dashboard";
import CreateStudio from "./pages/CreateStudio";
import Lobby from "./pages/Lobby";
import Recording from "./pages/Recording";
import Editor from "./pages/Editor";
import Playback from "./pages/Playback";
import Settings from "./pages/Settings";
import Onboarding, { isOnboardingComplete } from "./pages/Onboarding";

function RequireOnboarded({ children }: { children: React.ReactNode }) {
  if (!isOnboardingComplete()) {
    return <Navigate to="/onboarding" replace />;
  }
  return children;
}

export default function App() {
  return (
    <div className="app-shell">
      <ErrorBoundary label="app">
      <Routes>
        <Route path="/onboarding" element={<Onboarding />} />
        <Route
          path="/"
          element={
            <RequireOnboarded>
              <Lobby />
            </RequireOnboarded>
          }
        />
        <Route
          path="/lobby"
          element={
            <RequireOnboarded>
              <Lobby />
            </RequireOnboarded>
          }
        />
        <Route
          path="/join"
          element={
            <RequireOnboarded>
              <Lobby initialPanel="invite" />
            </RequireOnboarded>
          }
        />
        <Route
          path="/studio"
          element={
            <RequireOnboarded>
              <Recording />
            </RequireOnboarded>
          }
        />
        <Route
          path="/recordings"
          element={
            <RequireOnboarded>
              <ErrorBoundary label="recordings">
                <Dashboard />
              </ErrorBoundary>
            </RequireOnboarded>
          }
        />
        <Route
          path="/create"
          element={
            <RequireOnboarded>
              <CreateStudio />
            </RequireOnboarded>
          }
        />
        <Route
          path="/editor/:id"
          element={
            <RequireOnboarded>
              <Editor />
            </RequireOnboarded>
          }
        />
        <Route
          path="/playback/:id"
          element={
            <RequireOnboarded>
              <Playback />
            </RequireOnboarded>
          }
        />
        <Route
          path="/settings"
          element={
            <RequireOnboarded>
              <Settings />
            </RequireOnboarded>
          }
        />
        <Route path="*" element={<Navigate to="/" replace />} />
      </Routes>
      </ErrorBoundary>
    </div>
  );
}
