import { useEffect, useState } from "react";
import { setRecordingState, subscribeMicLevel, type RecordingState } from "../ipc";
import { STATE_VISUALS, type UiState } from "../stateVisuals";

const SIMULATED: readonly RecordingState[] = ["idle", "listening", "processing", "done", "error"];

export function DictatePanel({ state }: { state: UiState }) {
  const [micLevel, setMicLevel] = useState(0);
  const visual = STATE_VISUALS[state];

  useEffect(() => {
    let disposed = false;
    void subscribeMicLevel((level) => {
      if (!disposed) setMicLevel(level);
    }).catch(() => {
      // Channel registration failing is non-fatal; the meter just stays at 0.
    });
    return () => {
      disposed = true;
    };
  }, []);

  return (
    <div className="dictate">
      <div
        className={visual.pulse ? "state-orb pulse" : "state-orb"}
        style={{
          backgroundColor: visual.color,
          boxShadow: `0 0 48px ${visual.color}55`,
        }}
      />
      <div className="state-label">{visual.label}</div>
      <p className="hint">
        Hold <span className="kbd">Ctrl</span> + <span className="kbd">Alt</span> +{" "}
        <span className="kbd">Space</span> to dictate
      </p>
      <div className="meter" title="Microphone level (synthetic until audio capture is wired)">
        <div className="meter-fill" style={{ width: `${Math.round(micLevel * 100)}%` }} />
      </div>
      <div className="simulate">
        {SIMULATED.map((s) => (
          <button key={s} onClick={() => void setRecordingState(s)}>
            {s}
          </button>
        ))}
      </div>
    </div>
  );
}
