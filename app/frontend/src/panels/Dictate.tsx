import { useEffect, useRef, useState } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import {
  activeBackend,
  activeHotkey,
  getSettings,
  listBackends,
  onHotkeyChanged,
  onTranscript,
  onWorkerError,
  type AcceleratorReport,
  type ActiveBackend,
  type BackendOption,
  type BackendPreference,
  type HotkeyConfig,
  type TranscriptPayload,
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

/**
 * Readable ink for a solid fill, chosen from the fill's own luminance.
 *
 * The state colours span a bright amber and a dark red, so one fixed text colour is unreadable on
 * one end or the other. sRGB relative luminance with the usual coefficients, thresholded at 0.55.
 */
function inkOn(hex: string): string {
  const n = parseInt(hex.replace("#", ""), 16);
  const [r, g, b] = [(n >> 16) & 255, (n >> 8) & 255, n & 255].map((c) => {
    const s = c / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b > 0.55 ? "#101218" : "#ffffff";
}

/**
 * The state, as the same pill the recording overlay shows — solid colour, pulsing while something
 * is happening, and the binding on the same line.
 *
 * It replaces a 180px circle with a 44px bar. The circle was the tallest thing on the first screen
 * by a wide margin and said exactly one word; at the default 920x660 window it pushed the
 * scratchpad and everything under it below the fold. Matching the overlay is not only for space:
 * the overlay is what the user sees while actually dictating, so the same shape and the same
 * colours mean one thing has to be learned rather than two.
 */
function StatusPill({
  state,
  hotkey,
  failed,
  registration,
}: {
  state: UiState;
  hotkey: HotkeyConfig | null;
  failed: boolean;
  registration: Registration;
}) {
  const visual = STATE_VISUALS[state];
  return (
    <div
      className="status-pill"
      style={{ backgroundColor: visual.color, color: inkOn(visual.color) }}
      role="status"
    >
      <span className={visual.pulse ? "status-pill-dot pulsing" : "status-pill-dot"} />
      <span className="status-pill-label">{visual.label}</span>
      <span className="status-pill-hint">
        <HotkeyHint hotkey={hotkey} failed={failed} registration={registration} />
      </span>
    </div>
  );
}

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

/**
 * Why the last utterance produced nothing.
 *
 * The state machine flashes `error` and drops straight back to `idle`, so the orb cannot carry
 * this: by the time anyone looks up it is grey again. The message is the worker's own, shown
 * verbatim — "input device matching "…" not found" tells the user exactly what to fix, and any
 * generic rewording of it would throw that away. It stays until a transcript proves dictation is
 * working again, or until the user dismisses it.
 */
function FailureBanner({ message, onDismiss }: { message: string; onDismiss: () => void }) {
  return (
    <div className="failure" role="alert">
      <span className="badge no">failed</span>
      <span className="failure-text">{message}</span>
      <button className="btn secondary small" onClick={onDismiss}>
        Dismiss
      </button>
    </div>
  );
}

/**
 * A place to try dictation without leaving the app.
 *
 * LocalWisper types into whatever window has focus, so a focused textarea receives dictated text
 * by the ordinary path — there is no special wiring here, and that is the point: what happens in
 * this box is what happens in any other application.
 *
 * The panel below it exists for the case where the text went somewhere else. `injected: false`
 * means it could not be typed and landed on the clipboard instead, which is a genuinely different
 * outcome and not one the user should have to deduce from an empty box.
 */
function Scratchpad({ last }: { last: TranscriptPayload | null }) {
  const [text, setText] = useState("");
  const [copyState, setCopyState] = useState<"idle" | "copied" | "failed">("idle");
  const area = useRef<HTMLTextAreaElement | null>(null);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopyState("copied");
    } catch {
      // No clipboard permission, or no clipboard API in this webview. Selecting the text is
      // still possible, so say that rather than pretending the copy worked.
      setCopyState("failed");
    }
  };

  return (
    <section className="scratchpad">
      <div className="scratchpad-head">
        <span className="perf-label">Try dictating here</span>
        <div className="scratch-actions">
          <button
            className="btn secondary small"
            onClick={() => void copy()}
            disabled={text === ""}
          >
            Copy
          </button>
          <button
            className="btn secondary small"
            onClick={() => {
              setText("");
              setCopyState("idle");
              area.current?.focus();
            }}
            disabled={text === ""}
          >
            Clear
          </button>
        </div>
      </div>

      <textarea
        ref={area}
        className="scratch-area"
        value={text}
        spellCheck={false}
        placeholder="Click here first, then use your hotkey and speak. The text arrives the same way it would in any other app."
        onChange={(e) => {
          setText(e.currentTarget.value);
          setCopyState("idle");
        }}
        aria-label="Scratchpad for trying dictation"
      />

      <p className="sub">
        A scratchpad. Nothing typed or dictated here is saved, logged or sent anywhere — it is gone
        when this window closes.
      </p>
      {copyState === "copied" && <p className="sub status-ok">Copied to the clipboard.</p>}
      {copyState === "failed" && (
        <p className="sub status-err">
          This webview would not give up the clipboard. Select the text and copy it by hand.
        </p>
      )}

      <div className="scratch-last">
        <span className="perf-label">Last transcript</span>
        {last === null ? (
          <p className="sub">Nothing has been dictated yet in this session.</p>
        ) : (
          <>
            <div className="scratch-last-head">
              {last.injected ? (
                <span className="badge yes">typed into the focused window</span>
              ) : (
                <span className="badge warn">sent to the clipboard</span>
              )}
              <span className="sub mono">via {last.provider}</span>
            </div>
            <p className="scratch-last-text">
              {last.text.trim() === "" ? (
                <span className="sub">The transcript was empty.</span>
              ) : (
                last.text
              )}
            </p>
            {!last.injected && (
              <p className="sub">
                It could not be typed into the focused application, so it was copied instead. Paste
                it with <span className="kbd">Ctrl</span> + <span className="kbd">V</span>.
              </p>
            )}
          </>
        )}
      </div>
    </section>
  );
}

export function DictatePanel({ state }: { state: UiState }) {
  const [hotkey, setHotkey] = useState<HotkeyConfig | null>(null);
  const [inputDevice, setInputDevice] = useState<string | null>(null);
  const [settingsFailed, setSettingsFailed] = useState(false);
  const [registration, setRegistration] = useState<Registration>("unknown");
  const [preference, setPreference] = useState<BackendPreference | null>(null);
  const [backendOptions, setBackendOptions] = useState<readonly BackendOption[]>([]);
  const [accel, setAccel] = useState<AcceleratorReport | null>(null);
  const [lastRun, setLastRun] = useState<LastRun | null>(getLastRun);
  const [active, setActive] = useState<ActiveBackend | null>(null);
  const [activeError, setActiveError] = useState<string | null>(null);
  const [activeNonce, setActiveNonce] = useState(0);
  const [lastTranscript, setLastTranscript] = useState<TranscriptPayload | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [eventsError, setEventsError] = useState<string | null>(null);

  useEffect(() => {
    let disposed = false;
    void getSettings()
      .then((s) => {
        if (!disposed) {
          setHotkey(s.hotkey);
          setPreference(s.backend);
          setInputDevice(s.audio.input_device);
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

  // What came out, and what stopped it coming out. One effect for both, because they are two
  // halves of the same question and a transcript is what clears a failure.
  useEffect(() => {
    let disposed = false;
    const unlisteners: UnlistenFn[] = [];
    const keep = (un: UnlistenFn) => {
      if (disposed) un();
      else unlisteners.push(un);
    };

    void onTranscript((payload) => {
      if (disposed) return;
      setLastTranscript(payload);
      // A finished utterance is proof that whatever failed before is no longer failing.
      setFailure(null);
    })
      .then(keep)
      .catch((e: unknown) => {
        if (!disposed) setEventsError(String(e));
      });

    void onWorkerError((message) => {
      if (!disposed) setFailure(message);
    })
      .then(keep)
      .catch((e: unknown) => {
        if (!disposed) setEventsError(String(e));
      });

    return () => {
      disposed = true;
      for (const un of unlisteners) un();
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

  // Which input dictation will open, read from the saved settings because that is what the worker
  // reads too. Only worth a sentence when a specific device is configured: "uses the system
  // default" adds nothing to a line that already says to go and pick one.
  const deviceNote =
    inputDevice === null || inputDevice.trim() === ""
      ? null
      : `It is currently set to the input matching “${inputDevice.trim()}”.`;

  return (
    <div className="dictate">
      <StatusPill
        state={state}
        hotkey={hotkey}
        failed={settingsFailed}
        registration={registration}
      />
      <BackendHint line={backendLine} detail={backendDetail} />

      {failure !== null && (
        <FailureBanner message={failure} onDismiss={() => setFailure(null)} />
      )}
      {eventsError !== null && (
        <p className="sub status-err">
          This panel could not subscribe to the worker's events ({eventsError}), so it cannot report
          transcripts or failures. Dictation itself is unaffected.
        </p>
      )}

      <Scratchpad last={lastTranscript} />

      {/*
        No microphone meter here any more. The same control lives in Settings, next to the device
        picker that fixes the problem it reports, and duplicating it cost most of the first
        screen's height. What is left is the sentence that sends you there, which is the only part
        a user needs on this tab -- and it names the device the worker will actually open.
      */}
      <p className="sub dictate-mic-note">
        If the bar above does not turn red when you press the hotkey, or nothing is transcribed,
        check the microphone: <b>Settings</b> has the input picker and a <b>Test microphone</b>{" "}
        switch that shows whether sound is reaching the app.
        {deviceNote !== null && <> {deviceNote}</>}
      </p>

    </div>
  );
}
