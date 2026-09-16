import { useEffect, useState } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import {
  activeBackend,
  activeHotkey,
  getSettings,
  listBackends,
  onHotkeyChanged,
  setRecordingState,
  subscribeMicLevel,
  type AcceleratorReport,
  type ActiveBackend,
  type BackendOption,
  type BackendPreference,
  type HotkeyConfig,
  type RecordingState,
} from "../ipc";
import { hotkeyParts, hotkeyTrigger } from "../format";
import {
  getLastRun,
  probeAccelerators,
  runningBackendLine,
  subscribeLastRun,
  type BackendLine,
  type LastRun,
} from "../backend";
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

/**
 * Which backend dictation is on, in one line.
 *
 * The badge is the honesty marker: `running` is the loaded engine's own answer, `measured` is a
 * benchmark from this session, and `predicted` is an inference from settings plus a probe — which
 * is all there is before the first dictation, because the engine loads lazily.
 */
function BackendHint({ line, detail }: { line: BackendLine; detail: string }) {
  const className = line.tone === "warning" ? "status-err backend-line" : "hint backend-line";
  const badge =
    line.tone === "running" ? (
      <span className="badge yes">running</span>
    ) : line.tone === "measured" ? (
      <span className="badge measured-badge">measured</span>
    ) : line.tone === "probed" ? (
      <span className="badge">predicted</span>
    ) : null;
  return (
    <p className={className} title={detail}>
      {badge}
      {badge === null ? null : " "}
      {line.text}
    </p>
  );
}

export function DictatePanel({ state }: { state: UiState }) {
  const [micLevel, setMicLevel] = useState(0);
  const [hotkey, setHotkey] = useState<HotkeyConfig | null>(null);
  const [settingsFailed, setSettingsFailed] = useState(false);
  const [registration, setRegistration] = useState<Registration>("unknown");
  const [preference, setPreference] = useState<BackendPreference | null>(null);
  const [backendOptions, setBackendOptions] = useState<readonly BackendOption[]>([]);
  const [accel, setAccel] = useState<AcceleratorReport | null>(null);
  const [lastRun, setLastRun] = useState<LastRun | null>(getLastRun);
  const [active, setActive] = useState<ActiveBackend | null>(null);
  const [activeError, setActiveError] = useState<string | null>(null);
  const [activeNonce, setActiveNonce] = useState(0);
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
        if (!disposed) {
          setHotkey(s.hotkey);
          setPreference(s.backend);
        }
      })
      .catch(() => {
        if (!disposed) setSettingsFailed(true);
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
    // The backend announces every (re)registration. That corrects the answer above when this
    // panel asked before startup finished, and keeps the line accurate if the binding changes
    // while the panel is open.
    let unlisten: UnlistenFn | undefined;
    void onHotkeyChanged((payload) => {
      if (disposed) return;
      setRegistration(payload.accelerator === null ? "none" : "held");
    }).then((un) => {
      if (disposed) un();
      else unlisten = un;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  // The backend labels and the live accelerator probe, for the "what is running" line. Both are
  // best-effort: a failure leaves the line saying it does not know, never claiming acceleration.
  useEffect(() => {
    let disposed = false;
    void listBackends()
      .then((options) => {
        if (!disposed) setBackendOptions(options);
      })
      .catch(() => {
        // The raw preference name is still shown; only the pretty label is lost.
      });
    void probeAccelerators()
      .then((report) => {
        if (!disposed) setAccel(report);
      })
      .catch(() => {
        // Leaves availability "not established", which is the honest state.
      });
    return () => {
      disposed = true;
    };
  }, []);

  // What the worker's engine actually selected. Asked once on mount, and again whenever that
  // answer can have changed — it is a message to the worker thread, not something to poll.
  useEffect(() => {
    let disposed = false;
    void activeBackend()
      .then((report) => {
        if (!disposed) {
          setActive(report);
          setActiveError(null);
        }
      })
      .catch((e: unknown) => {
        if (!disposed) {
          setActive(null);
          setActiveError(String(e));
        }
      });
    return () => {
      disposed = true;
    };
  }, [activeNonce]);

  // The engine loads lazily on the first dictation, so the first real answer only exists once one
  // has finished (or failed). Both transitions are worth re-asking on.
  useEffect(() => {
    if (state === "done" || state === "error") setActiveNonce((n) => n + 1);
  }, [state]);

  // A benchmark drops the dictation engine and loads its own, so the previous answer is stale the
  // moment one finishes: take the measurement and re-ask the worker.
  useEffect(
    () =>
      subscribeLastRun((run) => {
        setLastRun(run);
        setActiveNonce((n) => n + 1);
      }),
    [],
  );

  const backendLine: BackendLine = settingsFailed
    ? {
        tone: "warning",
        text: "Backend: unknown — settings could not be read.",
        detail: "Open Settings to check the accelerator preference.",
      }
    : runningBackendLine(preference, backendOptions, accel, lastRun, active);
  // A failed `active_backend` never silences the line; it degrades it to the prediction below and
  // says so in the tooltip, rather than claiming the engine reported anything.
  const backendDetail =
    activeError === null
      ? backendLine.detail
      : `${backendLine.detail} (The worker could not be asked what is running: ${activeError})`;

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
      <HotkeyHint hotkey={hotkey} failed={settingsFailed} registration={registration} />
      <BackendHint line={backendLine} detail={backendDetail} />
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
