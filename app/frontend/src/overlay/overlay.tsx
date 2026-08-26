// Entry point for the "overlay" window: a small transparent recording pill.
// The window itself is created hidden in Rust and shown only while dictation
// is active, so this just mirrors the `state_changed` event.

import { StrictMode, useEffect, useState, type CSSProperties } from "react";
import { createRoot } from "react-dom/client";
import { onStateChanged, getRecordingState } from "../ipc";
import { STATE_VISUALS, type UiState } from "../stateVisuals";

function OverlayPill() {
  const [state, setState] = useState<UiState>("loading");

  useEffect(() => {
    let disposed = false;
    const unlisten = onStateChanged((payload) => {
      if (!disposed) setState(payload.state);
    });
    void getRecordingState()
      .then((s) => {
        if (!disposed) setState((prev) => (prev === "loading" ? s : prev));
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      void unlisten.then((un) => un());
    };
  }, []);

  const visual = STATE_VISUALS[state];

  return (
    <div style={styles.pill}>
      <span
        style={{
          ...styles.dot,
          backgroundColor: visual.color,
          boxShadow: `0 0 10px ${visual.color}`,
          animation: visual.pulse ? "pill-pulse 1.2s ease-in-out infinite" : "none",
        }}
      />
      <span style={styles.label}>{visual.label}</span>
      <style>{`
        @keyframes pill-pulse {
          0%, 100% { transform: scale(1); opacity: 1; }
          50% { transform: scale(0.7); opacity: 0.6; }
        }
      `}</style>
    </div>
  );
}

const styles = {
  pill: {
    display: "flex",
    alignItems: "center",
    gap: 10,
    height: 40,
    margin: 8,
    padding: "0 16px",
    borderRadius: 20,
    backgroundColor: "rgba(17, 19, 26, 0.88)",
    border: "1px solid rgba(255, 255, 255, 0.12)",
    color: "#e5e7ee",
    fontFamily: '"Segoe UI Variable Text", "Segoe UI", system-ui, sans-serif',
    fontSize: 13,
    width: "fit-content",
  },
  dot: {
    width: 12,
    height: 12,
    borderRadius: "50%",
    flexShrink: 0,
  },
  label: {
    fontWeight: 600,
    whiteSpace: "nowrap",
  },
} satisfies Record<string, CSSProperties>;

const rootEl = document.getElementById("root");
if (!rootEl) {
  throw new Error("missing #root element");
}

createRoot(rootEl).render(
  <StrictMode>
    <OverlayPill />
  </StrictMode>,
);
