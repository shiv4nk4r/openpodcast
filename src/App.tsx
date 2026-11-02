import { Navigate, Route, Routes } from "react-router-dom";
import { ErrorBoundary } from "./components/ErrorBoundary";
import Dashboard from "./pages/Dashboard";
import CreateStudio from "./pages/CreateStudio";
import Lobby from "./pages/Lobby";
import Recording from "./pages/Recording";
import Editor from "./pages/Editor";
import Playback from "./pages/Playback";
import Settings from "./pages/Settings";

export default function App() {
  return (
    <div className="app-shell">
      <ErrorBoundary label="app">
        <Routes>
          <Route path="/" element={<Lobby />} />
          <Route path="/lobby" element={<Lobby />} />
          <Route path="/join" element={<Lobby initialPanel="invite" />} />
          <Route path="/studio" element={<Recording />} />
          <Route
            path="/recordings"
            element={
              <ErrorBoundary label="recordings">
                <Dashboard />
              </ErrorBoundary>
            }
          />
          <Route path="/create" element={<CreateStudio />} />
          <Route path="/editor/:id" element={<Editor />} />
          <Route path="/playback/:id" element={<Playback />} />
          <Route path="/settings" element={<Settings />} />
          <Route path="/onboarding" element={<Navigate to="/" replace />} />
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
      </ErrorBoundary>
    </div>
  );
}
