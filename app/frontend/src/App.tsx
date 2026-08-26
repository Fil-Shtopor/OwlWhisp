import { useEffect, useState } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { getRecordingState, onOpenTab, onStateChanged } from "./ipc";
import type { UiState } from "./stateVisuals";
import { DictatePanel } from "./panels/Dictate";
import { SettingsPanel } from "./panels/Settings";
import { DiagnosticsPanel } from "./panels/Diagnostics";

type Tab = "dictate" | "settings" | "diagnostics";

const TABS: ReadonlyArray<{ id: Tab; label: string }> = [
  { id: "dictate", label: "Dictate" },
  { id: "settings", label: "Settings" },
  { id: "diagnostics", label: "Diagnostics" },
];

function isTab(value: string): value is Tab {
  return value === "dictate" || value === "settings" || value === "diagnostics";
}

export function App() {
  const [tab, setTab] = useState<Tab>("dictate");
  const [state, setState] = useState<UiState>("loading");

  useEffect(() => {
    let disposed = false;
    const unlisteners: UnlistenFn[] = [];

    // Subscribe first so no transition is missed, then read the current state.
    void onStateChanged((payload) => {
      if (!disposed) setState(payload.state);
    }).then((un) => unlisteners.push(un));

    void getRecordingState()
      .then((s) => {
        if (!disposed) setState((prev) => (prev === "loading" ? s : prev));
      })
      .catch(() => {
        if (!disposed) setState("error");
      });

    void onOpenTab((requested) => {
      if (!disposed && isTab(requested)) setTab(requested);
    }).then((un) => unlisteners.push(un));

    return () => {
      disposed = true;
      for (const un of unlisteners) un();
    };
  }, []);

  return (
    <div className="app">
      <nav className="tabs">
        {TABS.map((t) => (
          <button
            key={t.id}
            className={t.id === tab ? "tab active" : "tab"}
            onClick={() => setTab(t.id)}
          >
            {t.label}
          </button>
        ))}
      </nav>
      <main className="panel">
        {tab === "dictate" && <DictatePanel state={state} />}
        {tab === "settings" && <SettingsPanel />}
        {tab === "diagnostics" && <DiagnosticsPanel />}
      </main>
    </div>
  );
}
