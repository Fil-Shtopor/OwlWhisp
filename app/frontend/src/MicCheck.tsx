// The level meter, and the switch that gives it something to show.
//
// Two facts shape this component.
//
// First, the meter stream is only alive while something is capturing — dictation, or the test
// stream this switch opens. Without the switch, "is my microphone working?" is unanswerable on a
// screen where nothing is being dictated, which is exactly when the question gets asked.
//
// Second, `setMicTest` returns whether the stream is open *afterwards*, which is not always what
// was asked: a machine can refuse microphone access, and a saved device that is no longer plugged
// in makes the call fail outright. So the checkbox renders the backend's answer, never the
// request, and the reason for a refusal is shown verbatim rather than summarised away.

import { useCallback, useEffect, useRef, useState } from "react";
import { setMicTest, subscribeMicLevel } from "./ipc";

export interface MicCheckProps {
  /**
   * True while dictation owns the microphone. The test stream is closed and the switch is locked:
   * a second capture at that moment is pointless, and on some devices it is worse than pointless.
   */
  readonly busyElsewhere: boolean;
  /** One line naming the input this will open, or null when there is nothing useful to say. */
  readonly deviceNote: string | null;
}

/** Percent of the bar, for a figure the user can compare against what they can see. */
function percent(level: number): string {
  return `${Math.round(level * 100)}%`;
}

export function MicCheck({ busyElsewhere, deviceNote }: MicCheckProps) {
  const [level, setLevel] = useState(0);
  /** The loudest level seen since the test was switched on. Zero means genuinely no signal. */
  const [peak, setPeak] = useState(0);
  /** What the backend reported after the last call — never what was requested. */
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [streamError, setStreamError] = useState<string | null>(null);

  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  // The real RMS of the microphone, mapped to [0, 1] on a -60..0 dBFS scale by the backend. It is
  // pushed to zero whenever capture stops, so there is nothing to tear down here beyond ignoring
  // messages that arrive after this component has gone.
  useEffect(() => {
    let disposed = false;
    void subscribeMicLevel((next) => {
      if (disposed) return;
      setLevel(next);
      setPeak((previous) => (next > previous ? next : previous));
    }).catch((e: unknown) => {
      if (!disposed) setStreamError(String(e));
    });
    return () => {
      disposed = true;
    };
  }, []);

  // Closing the stream on unmount is not optional: leaving a microphone open because someone
  // switched tabs would be indefensible. Deliberately fire-and-forget — the component is already
  // gone, so there is nobody left to tell if it fails.
  useEffect(
    () => () => {
      void setMicTest(false).catch(() => {});
    },
    [],
  );

  const apply = useCallback(async (enabled: boolean) => {
    setBusy(true);
    setError(null);
    if (enabled) setPeak(0);
    try {
      const isOpen = await setMicTest(enabled);
      if (!alive.current) return;
      setOpen(isOpen);
      if (!isOpen) {
        setLevel(0);
        setPeak(0);
      }
      if (enabled && !isOpen) {
        setError(
          "The microphone did not open, so nothing is being captured. This machine may be " +
            "refusing access to it.",
        );
      }
    } catch (e: unknown) {
      if (!alive.current) return;
      setOpen(false);
      setLevel(0);
      setPeak(0);
      setError(String(e));
    } finally {
      if (alive.current) setBusy(false);
    }
  }, []);

  // Dictation starting takes the microphone back. Driven off the reported state, so the switch
  // cannot be left claiming a test stream that was closed underneath it.
  useEffect(() => {
    if (busyElsewhere && open) void apply(false);
  }, [busyElsewhere, open, apply]);

  const width = `${Math.min(100, Math.max(0, level * 100))}%`;

  return (
    <section className="mic-check">
      <div className="mic-check-head">
        <span className="perf-label">Microphone level</span>
        {open && <span className="badge yes">listening</span>}
      </div>

      <div
        className="meter"
        role="meter"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(level * 100)}
        aria-label="Microphone level"
        title="The real RMS of the input, on a -60..0 dBFS scale, so ordinary speech sits mid-bar."
      >
        <div className="meter-fill" style={{ width }} />
      </div>

      <div className="mic-check-row">
        <label className="check-row">
          <input
            type="checkbox"
            checked={open}
            disabled={busy || busyElsewhere}
            onChange={(e) => void apply(e.currentTarget.checked)}
          />
          <span className="radio-label">Test microphone</span>
        </label>
        <span className="sub mic-check-note">
          Opens the microphone only to move this bar. Nothing is transcribed, nothing is written to
          disk, nothing leaves the machine.
        </span>
      </div>

      {deviceNote !== null && <p className="sub">{deviceNote}</p>}

      {busy && <p className="sub">Asking the operating system…</p>}

      {error !== null && <p className="status-err">{error}</p>}

      {streamError !== null && (
        <p className="status-err">
          The level stream could not be registered: {streamError}. The bar will stay at zero even
          when the microphone is working.
        </p>
      )}

      {/*
        A meter stuck at zero while the stream is open is a real finding — no signal is reaching
        the app — so it is reported as one rather than covered up with an idle animation.
      */}
      {open && peak === 0 && (
        <p className="sub">
          No signal yet: the bar has not moved since the microphone opened. Say something. If it
          stays here, this input is reaching the app as silence.
        </p>
      )}
      {open && peak > 0 && (
        <p className="sub mic-peak">
          Loudest so far: {percent(peak)} of the bar. Ordinary speech should reach the middle.
        </p>
      )}
      {!open && !busyElsewhere && error === null && (
        <p className="sub">
          The bar only moves while something is capturing — during dictation, or while this switch
          is on.
        </p>
      )}
      {busyElsewhere && (
        <p className="sub">Dictation has the microphone; the bar is following that.</p>
      )}
    </section>
  );
}
