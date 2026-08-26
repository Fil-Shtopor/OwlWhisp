import { useEffect, useState } from "react";
import {
  getSettings,
  listBackends,
  setSettings,
  type BackendPreference,
  type Settings,
} from "../ipc";

const BACKEND_LABELS: Record<BackendPreference, string> = {
  automatic: "Automatic",
  force_npu: "NPU",
  force_cpu: "CPU",
};

function formatHotkey(settings: Settings): string {
  const parts = [...settings.hotkey.modifiers];
  if (settings.hotkey.trigger !== "none") parts.push(settings.hotkey.trigger);
  return parts.map((p) => p.charAt(0).toUpperCase() + p.slice(1)).join(" + ");
}

type SaveStatus = { kind: "idle" } | { kind: "saved" } | { kind: "error"; message: string };

export function SettingsPanel() {
  const [settings, setLocal] = useState<Settings | null>(null);
  const [backends, setBackends] = useState<BackendPreference[]>([
    "automatic",
    "force_npu",
    "force_cpu",
  ]);
  const [status, setStatus] = useState<SaveStatus>({ kind: "idle" });
  const [loadError, setLoadError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let disposed = false;
    void getSettings()
      .then((s) => {
        if (!disposed) setLocal(s);
      })
      .catch((e: unknown) => {
        if (!disposed) setLoadError(String(e));
      });
    void listBackends()
      .then((b) => {
        if (!disposed && b.length > 0) setBackends(b);
      })
      .catch(() => {
        // keep the static fallback list
      });
    return () => {
      disposed = true;
    };
  }, []);

  if (loadError !== null) {
    return <p className="status-err">Failed to load settings: {loadError}</p>;
  }
  if (settings === null) {
    return <p className="hint">Loading settings…</p>;
  }

  const update = (patch: Partial<Settings>) => {
    setLocal({ ...settings, ...patch });
    setStatus({ kind: "idle" });
  };

  const save = async () => {
    setSaving(true);
    try {
      const normalized = await setSettings(settings);
      setLocal(normalized);
      setStatus({ kind: "saved" });
    } catch (e: unknown) {
      setStatus({ kind: "error", message: String(e) });
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="form">
      <div className="field">
        <label>Hotkey</label>
        <div>
          <span className="kbd">{formatHotkey(settings)}</span>{" "}
          <span className="sub">
            ({settings.hotkey.mode.replaceAll("_", " ")}) — active shortcut is currently fixed to
            Ctrl + Alt + Space; the editor comes later.
          </span>
        </div>
      </div>

      <div className="field">
        <label htmlFor="backend">Backend</label>
        <select
          id="backend"
          value={settings.backend}
          onChange={(e) => {
            const v = e.currentTarget.value;
            if (v === "automatic" || v === "force_npu" || v === "force_cpu") {
              update({ backend: v });
            }
          }}
        >
          {backends.map((b) => (
            <option key={b} value={b}>
              {BACKEND_LABELS[b]}
            </option>
          ))}
        </select>
        <span className="sub">Automatic prefers the NPU and falls back to CPU.</span>
      </div>

      <div className="field">
        <label htmlFor="vad-threshold">VAD threshold: {settings.vad.threshold.toFixed(2)}</label>
        <input
          id="vad-threshold"
          type="range"
          min={0}
          max={1}
          step={0.01}
          value={settings.vad.threshold}
          onChange={(e) =>
            update({ vad: { ...settings.vad, threshold: Number(e.currentTarget.value) } })
          }
        />
        <span className="sub">Higher values require louder/clearer speech to trigger.</span>
      </div>

      <div className="field">
        <div className="field-row">
          <label htmlFor="overlay-enabled">Show recording overlay</label>
          <input
            id="overlay-enabled"
            type="checkbox"
            checked={settings.overlay_enabled}
            onChange={(e) => update({ overlay_enabled: e.currentTarget.checked })}
          />
        </div>
        <div className="field-row">
          <label htmlFor="sounds-enabled">Start/stop sounds</label>
          <input
            id="sounds-enabled"
            type="checkbox"
            checked={settings.sounds_enabled}
            onChange={(e) => update({ sounds_enabled: e.currentTarget.checked })}
          />
        </div>
      </div>

      <button className="btn" onClick={() => void save()} disabled={saving}>
        {saving ? "Saving…" : "Save"}
      </button>

      {status.kind === "saved" && <p className="status-ok">Settings saved.</p>}
      {status.kind === "error" && <p className="status-err">Save failed: {status.message}</p>}
    </div>
  );
}
