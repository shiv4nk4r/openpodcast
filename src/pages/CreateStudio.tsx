import { useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { IconArrowLeft } from "@tabler/icons-react";
import { api } from "../api";
import { Segmented, Select } from "../components/Select";
import { useApp } from "../state";
import type { StudioMode } from "../types";

export default function CreateStudio() {
  const { settings, setStudio } = useApp();
  const navigate = useNavigate();
  const [name, setName] = useState("Episode 1");
  const [mode, setMode] = useState<StudioMode>("audio_video");
  const [frameRate, setFrameRate] = useState("30");
  const [resolution, setResolution] = useState("1080p");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function onCreate() {
    setBusy(true);
    setError(null);
    try {
      const studioSettings = {
        name,
        mode,
        frame_rate: Number(frameRate),
        resolution,
      };
      const res = await api.createStudio(studioSettings, settings.display_name);
      setStudio({
        role: "host",
        sessionId: res.session_id,
        participantId: "host",
        invite: res.invite,
        settings: studioSettings,
        displayName: settings.display_name,
      });
      navigate("/lobby");
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="dashboard">
      <aside className="side-nav">
        <div className="brand">OpenPodcast</div>
        <Link className="nav-item" to="/">
          <IconArrowLeft size={16} stroke={1.75} />
          Back
        </Link>
      </aside>
      <main className="main-pane">
        <h1 className="page-title">Create studio</h1>
        <p className="page-sub">Choose mode and quality. Guests inherit these settings.</p>
        <div className="form-card">
          <div className="field">
            <label>Studio name</label>
            <input value={name} onChange={(e) => setName(e.target.value)} />
          </div>
          <Segmented
            label="Mode"
            value={mode}
            onChange={(v) => setMode(v as StudioMode)}
            options={[
              { value: "audio_only", label: "Audio only" },
              { value: "audio_video", label: "Audio + video" },
            ]}
          />
          {mode === "audio_video" && (
            <>
              <Select
                label="Frame rate"
                value={frameRate}
                onChange={setFrameRate}
                options={[24, 25, 30, 60].map((f) => ({
                  value: String(f),
                  label: `${f} fps`,
                  description: f === 30 ? "Default for most podcasts" : undefined,
                }))}
              />
              <Select
                label="Resolution"
                value={resolution}
                onChange={setResolution}
                options={[
                  { value: "720p", label: "720p", description: "Lighter files" },
                  { value: "1080p", label: "1080p", description: "Recommended" },
                ]}
              />
            </>
          )}
          {error && (
            <p style={{ color: "var(--record)", fontSize: "0.9rem" }}>{error}</p>
          )}
          <button className="btn btn-primary" disabled={busy} onClick={() => void onCreate()}>
            {busy ? "Creating…" : "Enter lobby"}
          </button>
        </div>
      </main>
    </div>
  );
}
