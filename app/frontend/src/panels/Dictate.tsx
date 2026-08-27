import { useEffect, useState } from "react";
import {
  activeHotkey,
  getSettings,
  setRecordingState,
  subscribeMicLevel,
  type HotkeyConfig,
  type RecordingState,
} from "../ipc";
import { hotkeyParts, hotkeyTrigger } from "../format";
import { STATE_VISUALS, type UiState } from "../stateVisuals";

const SIMULATED: readonly RecordingState[] = ["idle", "listening", "processing", "done", "error"];

/** What the OS shortcut registry answered. `unknown` covers "not asked yet" and "did not answer". */
type Registration = "unknown" | "held" | "none";

/** The bound keys as separate caps: Ctrl + Alt + Space. */
function Keys({ parts }: { parts: readonly string[] }) {
  return (
    <>
      {parts.map((part, i) => (
        <span key={`${part}-${i}`}>
          {i > 0 ? " + " : null}
          <span className="kbd">{part}</span>
        </span>
      ))}
    </>
  );
}

/**
 * One line telling the user how to dictate — read from the real binding, never hardcoded, because
 * the hotkey and its mode are both editable in Settings. App unmounts this panel on a tab switch,
 * so returning from Settings re-reads it and the line cannot go stale.
 */
function HotkeyHint({
  hotkey,
  failed,
  registration,
}: {
  hotkey: HotkeyConfig | null;
  failed: boolean;
  registration: Registration;
}) {
  if (failed) {
    return <p className="hint">Could not read your hotkey — open Settings to check it.</p>;
  }
  if (hotkey === null) {
    return <p className="hint">Checking your hotkey…</p>;
  }

  const parts = hotkeyParts(hotkey);
  if (hotkeyTrigger(hotkey) === "" || parts.length === 0) {
    return <p className="hint">No hotkey is set — pick one in Settings to dictate.</p>;
  }

  const keys = <Keys parts={parts} />;
  const sentence =
    hotkey.mode === "toggle" ? (
      <>Press {keys} to start, press again to stop</>
    ) : hotkey.mode === "hands_free" ? (
      <>Press {keys} and speak; it stops when you do</>
    ) : (
      <>Hold {keys} to dictate</>
    );

  return (
    <p className="hint">
      {sentence}
      {registration === "none" && (
        <span className="status-err" title="Another app may already own this combination">
          {" "}
          — not registered with the OS
        </span>
      )}
    </p>
  );
}

export function DictatePanel({ state }: { state: UiState }) {
  const [micLevel, setMicLevel] = useState(0);
  const [hotkey, setHotkey] = useState<HotkeyConfig | null>(null);
  const [hotkeyFailed, setHotkeyFailed] = useState(false);
  const [registration, setRegistration] = useState<Registration>("unknown");
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

  useEffect(() => {
    let disposed = false;
    void getSettings()
      .then((s) => {
        if (!disposed) setHotkey(s.hotkey);
      })
      .catch(() => {
        if (!disposed) setHotkeyFailed(true);
      });
    // Settings say what should be bound; this says what the OS actually holds. A null answer means
    // registration failed, so the hint warns instead of promising a key that does nothing.
    void activeHotkey()
      .then((accelerator) => {
        if (!disposed) setRegistration(accelerator === null ? "none" : "held");
      })
      .catch(() => {
        // Leave it "unknown": a silent line beats a warning we cannot stand behind.
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
      <HotkeyHint hotkey={hotkey} failed={hotkeyFailed} registration={registration} />
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
