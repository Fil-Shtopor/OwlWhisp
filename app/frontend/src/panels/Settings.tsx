import { useCallback, useEffect, useRef, useState } from "react";
import {
  activeBackend,
  activeHotkey,
  getAutostart,
  getSettings,
  listBackends,
  listInputDevices,
  listSoundThemes,
  previewSound,
  setAutostart,
  setSettings,
  type AcceleratorReport,
  type ActiveBackend,
  type BackendOption,
  type BackendPreference,
  type HotkeyConfig,
  type HotkeyMode,
  type Settings,
  type SoundThemeId,
  type SoundThemeOption,
} from "../ipc";
import {
  formatHotkey,
  hasModifier,
  hotkeyTrigger,
  triggerLabel,
  MODIFIER_OPTIONS,
} from "../format";
import {
  activeAccelerator,
  activeDisplayText,
  describeActive,
  getLastRun,
  isCoarse,
  preferenceLabel,
  probeAccelerators,
  resolveOption,
  strictnessNote,
  subscribeLastRun,
  type BackendResolution,
  type LastRun,
} from "../backend";
import { MicCheck } from "../MicCheck";
import type { UiState } from "../stateVisuals";

const MODE_OPTIONS: ReadonlyArray<{ id: HotkeyMode; label: string; help: string }> = [
  {
    id: "push_to_talk",
    label: "Push to talk",
    help: "Hold the keys while speaking, release to stop.",
  },
  {
    id: "toggle",
    label: "Toggle",
    help: "Press once to start, press again to stop (no holding).",
  },
  {
    id: "hands_free",
    label: "Hands free",
    help: "Press once; recording stops automatically when you stop speaking.",
  },
];

const LETTERS: readonly string[] = Array.from({ length: 26 }, (_, i) =>
  String.fromCharCode(97 + i),
);
const DIGITS: readonly string[] = Array.from({ length: 10 }, (_, i) => String(i));
const FKEYS: readonly string[] = Array.from({ length: 20 }, (_, i) => `f${i + 1}`);

const TRIGGER_GROUPS: ReadonlyArray<{ label: string; keys: readonly string[] }> = [
  { label: "Common", keys: ["space", "tab", "enter", "esc"] },
  { label: "Letters", keys: LETTERS },
  { label: "Digits", keys: DIGITS },
  { label: "Function keys", keys: FKEYS },
];

/** Everything the dropdown offers. All of these are accepted by lw-core's `key_code_name`. */
const SELECTABLE_TRIGGERS: ReadonlySet<string> = new Set([
  "space",
  "tab",
  "enter",
  "esc",
  ...LETTERS,
  ...DIGITS,
  ...FKEYS,
]);

/**
 * Everything `key_code_name` accepts, including spellings this editor never produces. A settings
 * file written elsewhere may hold one of them, and it would be wrong to call it invalid.
 */
const VALID_TRIGGERS: ReadonlySet<string> = new Set([
  "space",
  "tab",
  "enter",
  "return",
  "esc",
  "escape",
  "capslock",
  "caps_lock",
  "insert",
  "backquote",
  "grave",
  ...LETTERS,
  ...DIGITS,
  ...FKEYS,
]);

/**
 * Mirrors `HotkeyConfig::validate` in crates/lw-core/src/settings/mod.rs: a binding needs a real
 * key. Modifiers on their own are rejected there no matter how many are ticked, because an OS
 * global-shortcut registration has nothing to register without one.
 */
function hotkeyError(hotkey: HotkeyConfig): string | null {
  const trigger = hotkeyTrigger(hotkey);
  if (trigger === "") {
    return "A hotkey needs a non-modifier key — pick a trigger key such as Space, a letter, or a function key. Modifiers on their own cannot be registered as a global shortcut.";
  }
  if (!VALID_TRIGGERS.has(trigger)) {
    return `Unsupported hotkey key: ${hotkey.trigger}`;
  }
  return null;
}

/** Map a `KeyboardEvent.code` to a trigger name, or null for bare modifiers/unsupported keys. */
function triggerFromCode(code: string): string | null {
  if (code === "Space") return "space";
  if (code === "Tab") return "tab";
  if (code === "Enter" || code === "NumpadEnter") return "enter";
  if (code === "Escape") return "esc";
  const letter = /^Key([A-Z])$/.exec(code);
  if (letter !== null) return letter[1].toLowerCase();
  const digit = /^Digit([0-9])$/.exec(code);
  if (digit !== null) return digit[1];
  const fkey = /^F([0-9]{1,2})$/.exec(code);
  if (fkey !== null) {
    const n = Number(fkey[1]);
    if (n >= 1 && n <= 20) return `f${n}`;
  }
  return null;
}

type SaveStatus = { kind: "idle" } | { kind: "saved" } | { kind: "error"; message: string };

/** The badge that says how far a choice has been verified. `usable` is the only green one. */
function AvailabilityBadge({ resolution }: { resolution: BackendResolution }) {
  switch (resolution.availability) {
    case "usable":
      return <span className="badge yes">usable</span>;
    case "unusable":
      return <span className="badge no">unavailable</span>;
    case "unsupported":
      return <span className="badge no">not on this platform</span>;
    case "unknown":
      return <span className="badge warn">unknown</span>;
  }
}

/**
 * One selectable backend, with the reason it can or cannot be used.
 *
 * The reason is the backend's own `detail` string, shown verbatim: it already names the missing
 * file or the missing device, which is the part that tells the user what to do about it.
 */
function BackendRow({
  option,
  resolution,
  selected,
  disabled,
  onPick,
}: {
  option: BackendOption;
  resolution: BackendResolution;
  selected: boolean;
  disabled: boolean;
  onPick: () => void;
}) {
  const classes = ["accel-option"];
  if (selected) classes.push("selected");
  if (disabled) classes.push("unavailable");
  return (
    <label className={classes.join(" ")}>
      <input
        type="radio"
        name="backend"
        value={option.value}
        checked={selected}
        disabled={disabled}
        onChange={onPick}
      />
      <span className="accel-option-body">
        <span className="accel-option-head">
          <span className="radio-label">{option.label}</span>
          <AvailabilityBadge resolution={resolution} />
        </span>
        <span className="sub">{resolution.reason}</span>
      </span>
    </label>
  );
}

/**
 * "Start at login", read from the operating system rather than from the settings file.
 *
 * Two rules govern this control, and both exist because the OS can disagree with us:
 *
 * 1. The state on screen comes from `getAutostart()`, never from `settings.autostart` — that field
 *    is only a mirror, and the registration can be removed in Task Manager, `launchctl` or a
 *    desktop environment's startup list without this app ever hearing about it. Until the probe
 *    answers, the box is indeterminate: an unverified state is not a boolean.
 * 2. A toggle renders what `setAutostart()` *returns*, not what it was asked for. A managed machine
 *    can refuse, and a checkbox that claimed an autostart that does not exist would be worse than
 *    no checkbox at all.
 *
 * `onVerified` mirrors the confirmed value back into the edited settings, so the Save button
 * cannot write a stale mirror over a value the OS has since corrected.
 */
function AutostartField({
  mirror,
  onVerified,
}: {
  /** `settings.autostart`, shown only while the probe is in flight — and never as a fact. */
  mirror: boolean;
  onVerified: (value: boolean) => void;
}) {
  const [known, setKnown] = useState<boolean | null>(null);
  const [probeError, setProbeError] = useState<string | null>(null);
  const [changeError, setChangeError] = useState<string | null>(null);
  const [refused, setRefused] = useState<{ requested: boolean; actual: boolean } | null>(null);
  const [busy, setBusy] = useState(false);
  const box = useRef<HTMLInputElement | null>(null);
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  useEffect(() => {
    let disposed = false;
    void getAutostart()
      .then((value) => {
        if (disposed) return;
        setKnown(value);
        setProbeError(null);
        onVerified(value);
      })
      .catch((e: unknown) => {
        if (disposed) return;
        setKnown(null);
        setProbeError(String(e));
      });
    return () => {
      disposed = true;
    };
  }, [onVerified]);

  // Deliberately on every render: clicking the box clears `indeterminate` in the DOM, and after a
  // change that could not be confirmed the state is still unknown and must look it.
  useEffect(() => {
    if (box.current !== null) box.current.indeterminate = known === null;
  });

  const toggle = async (requested: boolean) => {
    setBusy(true);
    setChangeError(null);
    setRefused(null);
    try {
      const actual = await setAutostart(requested);
      if (!alive.current) return;
      setKnown(actual);
      setProbeError(null);
      onVerified(actual);
      if (actual !== requested) setRefused({ requested, actual });
    } catch (e: unknown) {
      if (alive.current) setChangeError(String(e));
    } finally {
      if (alive.current) setBusy(false);
    }
  };

  return (
    <section className="field autostart-field">
      <div className="field-row">
        <label htmlFor="autostart">Start at login</label>
        <input
          id="autostart"
          ref={box}
          type="checkbox"
          checked={known ?? mirror}
          disabled={busy}
          onChange={(e) => void toggle(e.currentTarget.checked)}
        />
      </div>
      <span className="sub">
        Launch LocalWisper when you sign in, so the hotkey is live without starting it by hand.
        Windows, macOS and Linux each register this differently; the app asks the operating system
        and reports what it says.
      </span>
      <span className="sub">
        <strong>This applies immediately</strong> — it is not part of the Save button below.
      </span>
      {busy && <span className="sub">Asking the operating system…</span>}
      {!busy && known === null && probeError === null && (
        <span className="sub">Checking with the operating system…</span>
      )}
      {!busy && known !== null && (
        <span className="sub">
          The operating system reports start at login is{" "}
          <strong>{known ? "on" : "off"}</strong>.
        </span>
      )}
      {probeError !== null && (
        <p className="status-err">
          Could not read the autostart registration: {probeError}. The box above is shown
          indeterminate because this app does not know the real state — toggling it will ask the
          operating system and report what it answers.
        </p>
      )}
      {refused !== null && (
        <p className="status-err">
          The operating system did not accept that. You asked to{" "}
          {refused.requested ? "turn start at login on" : "turn start at login off"}, and it reports{" "}
          <strong>{refused.actual ? "on" : "off"}</strong> afterwards. The box shows what it
          reports, not what was asked. A managed or locked-down machine can refuse this.
        </p>
      )}
      {changeError !== null && (
        <p className="status-err">
          Could not change start at login: {changeError}. Nothing on screen has been changed to
          claim otherwise.
        </p>
      )}
    </section>
  );
}

/** Clamp a stored volume into the range the slider and the backend accept. */
function clampVolume(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.min(1, Math.max(0, value));
}

/**
 * The cue sounds: on/off, which one, how loud, and — the point of the section — hearing one.
 *
 * A preview plays the controls *on this screen*, not the saved settings, so choosing a sound is a
 * matter of clicking around until one sounds right rather than saving and dictating to find out.
 */
function SoundCues({
  settings,
  disabled,
  onChange,
}: {
  settings: Settings;
  /** A save is in flight, so nothing here should move. */
  disabled: boolean;
  onChange: (patch: Partial<Settings>) => void;
}) {
  const [themes, setThemes] = useState<SoundThemeOption[] | null>(null);
  const [themesError, setThemesError] = useState<string | null>(null);
  const [playing, setPlaying] = useState<{ theme: SoundThemeId; cue: "start" | "stop" } | null>(
    null,
  );
  const [playError, setPlayError] = useState<string | null>(null);
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  useEffect(() => {
    let disposed = false;
    void listSoundThemes()
      .then((list) => {
        if (!disposed) {
          setThemes(list);
          setThemesError(null);
        }
      })
      .catch((e: unknown) => {
        if (!disposed) {
          setThemes(null);
          setThemesError(String(e));
        }
      });
    return () => {
      disposed = true;
    };
  }, []);

  const volume = clampVolume(settings.sound_volume);
  const enabled = settings.sounds_enabled;
  // The theme, volume and previews all depend on there being a sound to play at all.
  const locked = disabled || !enabled;

  const preview = async (theme: SoundThemeId, cue: "start" | "stop") => {
    setPlaying({ theme, cue });
    setPlayError(null);
    try {
      await previewSound(theme, volume, cue);
    } catch (e: unknown) {
      if (alive.current) setPlayError(String(e));
    } finally {
      if (alive.current) setPlaying(null);
    }
  };

  const isPlaying = (theme: SoundThemeId, cue: "start" | "stop") =>
    playing !== null && playing.theme === theme && playing.cue === cue;

  const savedIsOffered =
    themes === null || themes.some((theme) => theme.id === settings.sound_theme);

  return (
    <section className="field sound-section">
      <div className="field-row">
        <label htmlFor="sounds-enabled">Start/stop sounds</label>
        <input
          id="sounds-enabled"
          type="checkbox"
          checked={enabled}
          disabled={disabled}
          onChange={(e) => onChange({ sounds_enabled: e.currentTarget.checked })}
        />
      </div>
      <span className="sub">
        A short cue when recording starts and another when it stops, so you know the hotkey landed
        without looking at the overlay.
      </span>
      {!enabled && (
        <p className="sub">
          Sounds are off, so the sound, volume and previews below are disabled — there would be
          nothing to play. Tick the box above to choose one.
        </p>
      )}

      {themes === null && themesError === null && <p className="hint">Loading sounds…</p>}
      {themesError !== null && (
        <p className="status-err">
          Could not read the list of sounds: {themesError}. The saved sound{" "}
          <span className="mono">{settings.sound_theme}</span> is still in force; it just cannot be
          changed or previewed from here until the backend answers.
        </p>
      )}
      {themes !== null && themes.length === 0 && (
        <p className="hint">This build offers no cue sounds to choose from.</p>
      )}

      {themes !== null && themes.length > 0 && (
        <div className="accel-group-block">
          <span className="perf-label">Sound</span>
          {!savedIsOffered && (
            <p className="status-err">
              The saved sound <span className="mono">{settings.sound_theme}</span> is not one this
              build offers, so none is selected below. Pick one to replace it.
            </p>
          )}
          {themes.map((theme) => {
            const selected = settings.sound_theme === theme.id;
            const classes = ["accel-option", "sound-option"];
            if (selected) classes.push("selected");
            if (locked) classes.push("unavailable");
            return (
              <div key={theme.id} className={classes.join(" ")}>
                <label className="sound-option-label">
                  <input
                    type="radio"
                    name="sound-theme"
                    value={theme.id}
                    checked={selected}
                    disabled={locked}
                    onChange={() => onChange({ sound_theme: theme.id })}
                  />
                  <span className="accel-option-body">
                    <span className="accel-option-head">
                      <span className="radio-label">{theme.label}</span>
                    </span>
                    <span className="sub">{theme.description}</span>
                  </span>
                </label>
                <button
                  className="btn secondary small"
                  disabled={locked || playing !== null}
                  onClick={() => void preview(theme.id, "start")}
                  title={
                    locked
                      ? "Turn start/stop sounds on to hear this"
                      : `Play the ${theme.label} start cue at the volume set below`
                  }
                >
                  {isPlaying(theme.id, "start") ? "Playing…" : "Preview"}
                </button>
              </div>
            );
          })}
        </div>
      )}

      <div className="subfield">
        <label htmlFor="sound-volume" className="perf-label">
          Volume — {Math.round(volume * 100)}%
        </label>
        <input
          id="sound-volume"
          type="range"
          min={0}
          max={1}
          step={0.05}
          value={volume}
          disabled={locked}
          onChange={(e) => onChange({ sound_volume: clampVolume(Number(e.currentTarget.value)) })}
        />
        <div className="sound-preview-row">
          <button
            className="btn secondary small"
            disabled={locked || playing !== null}
            onClick={() => void preview(settings.sound_theme, "start")}
          >
            {isPlaying(settings.sound_theme, "start") ? "Playing…" : "Hear the start cue"}
          </button>
          <button
            className="btn secondary small"
            disabled={locked || playing !== null}
            onClick={() => void preview(settings.sound_theme, "stop")}
          >
            {isPlaying(settings.sound_theme, "stop") ? "Playing…" : "Hear the stop cue"}
          </button>
        </div>
        <span className="sub">
          A preview plays the sound and volume selected <strong>here, now</strong> — not the last
          saved ones — so you can hear a change before committing to it.
        </span>
        {playError !== null && <p className="status-err">Could not play that: {playError}</p>}
      </div>

      <span className="sub">
        The sound and volume are part of the <strong>Save</strong> button below; previewing changes
        nothing on disk.
      </span>
    </section>
  );
}

/**
 * Which microphone dictation opens.
 *
 * Three things about `audio.input_device` decide how this has to behave.
 *
 * It is a *fragment*, matched case-insensitively against the device names the OS reports — so the
 * value stored here is the full name, which trivially contains itself, and a shorter value written
 * by hand keeps working.
 *
 * An empty string means the system default. That is a choice, so it is offered as one rather than
 * shown as a blank field.
 *
 * And a saved name that matches nothing is **refused**, not quietly replaced: capture fails with
 * `input device matching "…" not found`. That is the right call — someone who picks a specific
 * microphone does so because the default is wrong for them, and substituting it would produce bad
 * transcripts with no clue why — but it means this screen must say "dictation will fail", not
 * "the default will be used". The setting is left exactly as saved: the device may simply be
 * unplugged, and rewriting it would lose the user's choice for them.
 */
function MicrophoneField({
  value,
  savedValue,
  disabled,
  dictating,
  onChange,
}: {
  value: string;
  /** The value as last saved. The test stream opens that one, not the edited one. */
  savedValue: string | null;
  disabled: boolean;
  /** True while dictation owns the microphone, so the test releases it. */
  dictating: boolean;
  onChange: (device: string) => void;
}) {
  const [devices, setDevices] = useState<string[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [nonce, setNonce] = useState(0);

  useEffect(() => {
    let disposed = false;
    setDevices(null);
    void listInputDevices()
      .then((list) => {
        if (!disposed) {
          setDevices(list);
          setError(null);
        }
      })
      .catch((e: unknown) => {
        if (!disposed) {
          setDevices([]);
          setError(String(e));
        }
      });
    return () => {
      disposed = true;
    };
  }, [nonce]);

  const wanted = value.trim();
  const matched =
    wanted === "" || devices === null
      ? null
      : (devices.find((d) => d.toLowerCase().includes(wanted.toLowerCase())) ?? null);
  // Only a loaded list can prove something is missing; before that, "not found" is not yet true.
  const orphaned = devices !== null && wanted !== "" && matched === null;
  const selectValue = wanted === "" ? "" : (matched ?? value);
  const hasChoices = (devices?.length ?? 0) > 0;
  const unsaved = savedValue !== null && savedValue.trim() !== wanted;

  const deviceNote =
    savedValue === null
      ? null
      : savedValue.trim() === ""
        ? "This test opens the system default input."
        : `This test opens the saved input, matching “${savedValue.trim()}”.`;

  return (
    <section className="field mic-field">
      <div className="field-row">
        <label htmlFor="input-device">Microphone</label>
        <button
          className="btn secondary small"
          onClick={() => setNonce((n) => n + 1)}
          disabled={disabled || devices === null}
        >
          {devices === null ? "Scanning…" : "Re-scan"}
        </button>
      </div>

      <select
        id="input-device"
        value={selectValue}
        disabled={disabled || devices === null || (!hasChoices && !orphaned)}
        onChange={(e) => onChange(e.currentTarget.value)}
      >
        <option value="">System default</option>
        {orphaned && (
          <optgroup label="Saved, but not present now">
            <option value={value}>{value} — unavailable</option>
          </optgroup>
        )}
        {hasChoices && (
          <optgroup label="Inputs on this machine">
            {(devices ?? []).map((device, i) => (
              <option key={`${device}-${i}`} value={device}>
                {device}
              </option>
            ))}
          </optgroup>
        )}
      </select>

      {devices === null && error === null && <span className="sub">Asking for input devices…</span>}

      {error !== null && (
        <p className="status-err">
          Could not list input devices: {error}. The saved value is still in force and can only be
          changed from here once this succeeds.
        </p>
      )}

      {devices !== null && !hasChoices && error === null && (
        <p className="status-err">
          This machine reports no audio inputs at all. Dictation has nothing to record from until
          one is connected.
        </p>
      )}

      {orphaned && (
        <p className="status-err">
          Nothing on this machine matches “{wanted}” right now — it is probably unplugged or
          renamed. Dictation will <strong>fail</strong> until it is reconnected or another input is
          chosen here; it is not silently replaced by the default. The setting is left as you saved
          it.
        </p>
      )}

      {wanted === "" && (
        <span className="sub">
          Whatever the operating system calls default when recording starts.
        </span>
      )}
      {matched !== null && matched !== value && (
        <span className="sub">
          Matches <strong>{matched}</strong> — the stored value is a fragment, matched
          case-insensitively.
        </span>
      )}

      {unsaved && (
        <span className="sub">
          Not saved yet. Dictation and the test below both open the saved device, so press Save
          before checking this one.
        </span>
      )}

      <MicCheck busyElsewhere={dictating} deviceNote={deviceNote} />
    </section>
  );
}

export function SettingsPanel({ state }: { state: UiState }) {
  const [settings, setLocal] = useState<Settings | null>(null);
  // The preference as *saved*, which is what the running engine was built from. The edited copy
  // above can differ the moment a radio is clicked, and comparing the engine against an unsaved
  // edit would accuse it of a fallback it never made.
  const [savedBackend, setSavedBackend] = useState<BackendPreference | null>(null);
  // Same reasoning for the microphone: the worker opens the *saved* device, so the test meter and
  // the "not saved yet" note both have to compare against this rather than the edited copy.
  const [savedDevice, setSavedDevice] = useState<string | null>(null);
  // Null until `list_backends` answers: the labels belong to the Rust side, so there is nothing
  // honest to show before it does.
  const [backends, setBackends] = useState<BackendOption[] | null>(null);
  const [backendsError, setBackendsError] = useState<string | null>(null);
  // Probed only to resolve what each backend choice would land on. The table that used to render
  // this lives on Diagnostics now, so a failed probe simply leaves availability "unknown" here.
  const [accel, setAccel] = useState<AcceleratorReport | null>(null);
  const [lastRun, setLastRun] = useState<LastRun | null>(getLastRun);
  const [active, setActive] = useState<ActiveBackend | null>(null);
  const [activeNonce, setActiveNonce] = useState(0);
  const [status, setStatus] = useState<SaveStatus>({ kind: "idle" });
  const [loadError, setLoadError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [capturing, setCapturing] = useState(false);
  const [registered, setRegistered] = useState<string | null>(null);
  const [registeredStatus, setRegisteredStatus] = useState<"loading" | "known" | "unavailable">(
    "loading",
  );

  useEffect(() => {
    let disposed = false;
    void getSettings()
      .then((s) => {
        if (!disposed) {
          setLocal(s);
          setSavedBackend(s.backend);
          setSavedDevice(s.audio.input_device);
        }
      })
      .catch((e: unknown) => {
        if (!disposed) setLoadError(String(e));
      });
    void listBackends()
      .then((b) => {
        if (!disposed) setBackends(b);
      })
      .catch((e: unknown) => {
        if (!disposed) setBackendsError(String(e));
      });
    void activeHotkey()
      .then((a) => {
        if (!disposed) {
          setRegistered(a);
          setRegisteredStatus("known");
        }
      })
      .catch(() => {
        // Non-fatal: we just cannot show what the OS holds.
        if (!disposed) setRegisteredStatus("unavailable");
      });
    return () => {
      disposed = true;
    };
  }, []);

  // Probing loads provider libraries, so it is a real call; the session-wide result is reused.
  useEffect(() => {
    let disposed = false;
    void probeAccelerators()
      .then((report) => {
        if (!disposed) setAccel(report);
      })
      .catch(() => {
        // Leaves every choice's availability "unknown", which is the honest state — and is what
        // the picker already renders when it cannot establish one.
      });
    return () => {
      disposed = true;
    };
  }, []);

  // What the dictation worker's engine actually selected — the answer that outranks every
  // prediction on this screen. Null until it answers; a failure leaves it null and the screen
  // simply says nothing about a running engine rather than guessing.
  useEffect(() => {
    let disposed = false;
    void activeBackend()
      .then((report) => {
        if (!disposed) setActive(report);
      })
      .catch(() => {
        if (!disposed) setActive(null);
      });
    return () => {
      disposed = true;
    };
  }, [activeNonce]);

  // A benchmark run in another tab both measures this machine and drops the dictation engine, so
  // it is worth showing and it invalidates the answer above.
  useEffect(
    () =>
      subscribeLastRun((run) => {
        setLastRun(run);
        setActiveNonce((n) => n + 1);
      }),
    [],
  );

  // "Capture keystroke": one keydown fills both modifiers and trigger. Esc on its own cancels.
  useEffect(() => {
    if (!capturing) return;
    const onKeyDown = (event: KeyboardEvent) => {
      event.preventDefault();
      event.stopPropagation();
      const bare = !event.ctrlKey && !event.altKey && !event.shiftKey && !event.metaKey;
      if (event.code === "Escape" && bare) {
        setCapturing(false);
        return;
      }
      const trigger = triggerFromCode(event.code);
      if (trigger === null) return; // still holding modifiers — wait for the real key
      const modifiers: string[] = [];
      if (event.ctrlKey) modifiers.push("ctrl");
      if (event.altKey) modifiers.push("alt");
      if (event.shiftKey) modifiers.push("shift");
      if (event.metaKey) modifiers.push("meta");
      setLocal((prev) =>
        prev === null ? prev : { ...prev, hotkey: { ...prev.hotkey, modifiers, trigger } },
      );
      setStatus({ kind: "idle" });
      setCapturing(false);
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => {
      window.removeEventListener("keydown", onKeyDown, true);
    };
  }, [capturing]);

  /**
   * Keep `settings.autostart` in step with what the OS confirmed.
   *
   * The field is a mirror, and the OS owns the truth; writing the confirmed value back means the
   * Save button cannot later persist a stale mirror. It deliberately does not touch the save
   * status: this is not an edit the user made, so it must not clear a "Settings saved" line or
   * present itself as unsaved work.
   */
  const mirrorAutostart = useCallback((value: boolean) => {
    setLocal((prev) =>
      prev === null || prev.autostart === value ? prev : { ...prev, autostart: value },
    );
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

  const updateHotkey = (patch: Partial<HotkeyConfig>) => {
    update({ hotkey: { ...settings.hotkey, ...patch } });
  };

  const toggleModifier = (id: string, checked: boolean) => {
    // Rebuild in a stable canonical order, dropping any alias spelling from an older file.
    const ticked = MODIFIER_OPTIONS.filter((m) =>
      m.id === id ? checked : hasModifier(settings.hotkey, m.id),
    ).map((m) => m.id);
    updateHotkey({ modifiers: ticked });
  };

  const hotkeyErr = hotkeyError(settings.hotkey);
  // "" when the saved file holds `none` (or nothing): no longer offered, but still shown so the
  // error below explains why Save is disabled.
  const triggerValue = hotkeyTrigger(settings.hotkey);

  const save = async () => {
    setSaving(true);
    try {
      const normalized = await setSettings(settings);
      setLocal(normalized);
      setSavedBackend(normalized.backend);
      setSavedDevice(normalized.audio.input_device);
      setStatus({ kind: "saved" });
      // Saving makes the worker drop its engine and reload it on the next utterance, so whatever
      // it reported a moment ago is stale: ask again rather than leave a dead engine on screen.
      setActiveNonce((n) => n + 1);
      try {
        const accelerator = await activeHotkey();
        setRegistered(accelerator);
        setRegisteredStatus("known");
      } catch {
        setRegisteredStatus("unavailable");
      }
    } catch (e: unknown) {
      setStatus({ kind: "error", message: String(e) });
    } finally {
      setSaving(false);
    }
  };

  const options = backends ?? [];
  const coarseOptions = options.filter(isCoarse);
  const exactOptions = options.filter((o) => !isCoarse(o));
  const selectedOption = options.find((o) => o.value === settings.backend) ?? null;
  const selectedResolution =
    selectedOption === null ? null : resolveOption(selectedOption, accel);

  // What the *saved* preference resolves to, which is what the loaded engine was built from.
  const savedOption =
    savedBackend === null ? null : (options.find((o) => o.value === savedBackend) ?? null);
  const savedResolution = savedOption === null ? null : resolveOption(savedOption, accel);

  /**
   * The engine ran somewhere other than where the saved preference points.
   *
   * Both sides are now stable ids — `active.accelerator_kind` against the resolved accelerator's
   * `kind` — so this compares vocabulary, not wording, and cannot go quiet if a Display impl is
   * reworded. That also retires the one-directional narrowing the old string test needed: the
   * CoreML case is a non-issue here, because its accelerator is classed `gpu` on both sides. Any
   * direction of mismatch is now reported, since an engine on the NPU while the setting says CPU
   * is just as much a surprise as the reverse.
   *
   * It is compared against the *saved* preference on purpose. Clicking a radio changes the edited
   * copy immediately, and an engine still faithfully running the saved choice must not be accused
   * of a fallback because of an edit that has not been applied yet.
   */
  const acceleratorMismatch: string | null = (() => {
    if (active === null || !active.loaded) return null;
    const runningKind = active.accelerator_kind;
    const target = savedResolution?.accelerator ?? null;
    // A null on either side is "cannot tell", which is not a disagreement.
    if (runningKind === null || target === null || runningKind === target.kind) return null;
    const runningLabel = activeAccelerator(active, accel)?.label ?? activeDisplayText(active);
    const why = active.notes.length === 0 ? "" : ` The engine's reasons: ${active.notes.join(" · ")}`;
    return `The loaded engine is running on ${runningLabel}, although the saved preference resolves to ${target.label}.${why}`;
  })();

  const renderRow = (option: BackendOption) => {
    const resolution = resolveOption(option, accel);
    // Only exact providers are locked out: a coarse choice stays selectable so the user can say
    // "any NPU" on a machine where none is usable *yet*, and read the warning about it.
    const blocked =
      !isCoarse(option) &&
      (resolution.availability === "unusable" || resolution.availability === "unsupported");
    return (
      <BackendRow
        key={option.value}
        option={option}
        resolution={resolution}
        selected={settings.backend === option.value}
        disabled={saving || blocked}
        onPick={() => update({ backend: option.value })}
      />
    );
  };

  return (
    <div className="form">
      <section className="field hotkey-editor">
        <label>Hotkey</label>
        <div className="hotkey-preview">
          <span className="kbd">{formatHotkey(settings.hotkey)}</span>
          <button
            className="btn secondary small"
            onClick={() => setCapturing((c) => !c)}
            disabled={saving}
          >
            {capturing ? "Cancel capture" : "Capture keystroke"}
          </button>
        </div>
        {capturing && (
          <p className="sub capture-hint">
            Press the combination you want to use. Esc on its own cancels.
          </p>
        )}

        <div className="subfield">
          <span className="perf-label">Mode</span>
          {MODE_OPTIONS.map((mode) => (
            <label key={mode.id} className="radio-row">
              <input
                type="radio"
                name="hotkey-mode"
                value={mode.id}
                checked={settings.hotkey.mode === mode.id}
                onChange={() => updateHotkey({ mode: mode.id })}
              />
              <span>
                <span className="radio-label">{mode.label}</span>
                <span className="sub"> — {mode.help}</span>
              </span>
            </label>
          ))}
        </div>

        <div className="subfield">
          <span className="perf-label">Modifiers</span>
          <div className="modifier-row">
            {MODIFIER_OPTIONS.map((mod) => (
              <label key={mod.id} className="check-row">
                <input
                  type="checkbox"
                  checked={hasModifier(settings.hotkey, mod.id)}
                  onChange={(e) => toggleModifier(mod.id, e.currentTarget.checked)}
                />
                <span>{mod.detail === null ? mod.label : `${mod.label} (${mod.detail})`}</span>
              </label>
            ))}
          </div>
        </div>

        <div className="subfield">
          <label htmlFor="hotkey-trigger" className="perf-label">
            Trigger key
          </label>
          <select
            id="hotkey-trigger"
            value={triggerValue}
            onChange={(e) => updateHotkey({ trigger: e.currentTarget.value })}
          >
            {!SELECTABLE_TRIGGERS.has(triggerValue) && (
              <optgroup label={triggerValue === "" ? "Not set" : "Saved value"}>
                <option value={triggerValue}>
                  {triggerValue === "" ? "No key — pick one below" : triggerLabel(triggerValue)}
                </option>
              </optgroup>
            )}
            {TRIGGER_GROUPS.map((group) => (
              <optgroup key={group.label} label={group.label}>
                {group.keys.map((key) => (
                  <option key={key} value={key}>
                    {triggerLabel(key)}
                  </option>
                ))}
              </optgroup>
            ))}
          </select>
          <span className="sub">
            A global shortcut needs a real key. Modifiers on their own cannot be registered with
            the OS, so every binding pairs them with one of these.
          </span>
        </div>

        {hotkeyErr !== null && <p className="status-err">{hotkeyErr}</p>}

        <div className="subfield">
          <span className="perf-label">Registered with the OS</span>
          {registeredStatus === "loading" ? (
            <span className="sub">Checking…</span>
          ) : registeredStatus === "unavailable" ? (
            <span className="sub">Not known — the backend did not answer.</span>
          ) : registered !== null ? (
            <span>
              <span className="kbd">{registered}</span>{" "}
              <span className="sub">is the accelerator the OS currently holds.</span>
            </span>
          ) : (
            <span className="sub">
              The OS holds no accelerator for this app right now. If you just saved, registration
              did not take — another app may already own the combination.
            </span>
          )}
        </div>
      </section>

      <section className="field accel-picker">
        <label>Accelerator</label>
        <span className="sub">
          Which hardware transcription runs on. <strong>Automatic</strong> picks the best usable
          accelerator and falls back to the CPU; every other choice is strict, so if it cannot be
          honoured the run fails instead of quietly running somewhere else.
        </span>
        <span className="sub">
          The full picture of what this machine can accelerate with — each provider library, the
          devices it found and the verdict — is on the <strong>Diagnostics</strong> tab.
        </span>

        {backendsError !== null && (
          <p className="status-err">
            Could not read the list of backends: {backendsError}. The saved preference is{" "}
            <span className="mono">{settings.backend}</span>; it is still in force, but it cannot be
            changed from here until the backend answers.
          </p>
        )}
        {backends === null && backendsError === null && (
          <p className="hint">Loading backends…</p>
        )}
        {backends !== null && options.length === 0 && (
          <p className="hint">The backend offered no choices at all.</p>
        )}

        {coarseOptions.length > 0 && (
          <div className="accel-group-block">
            <span className="perf-label">By kind of hardware</span>
            {coarseOptions.map(renderRow)}
          </div>
        )}
        {exactOptions.length > 0 && (
          <div className="accel-group-block">
            <span className="perf-label">One exact provider</span>
            {exactOptions.map(renderRow)}
          </div>
        )}
        {backends !== null && selectedOption === null && (
          <p className="status-err">
            The saved preference <span className="mono">{settings.backend}</span> is not one this
            build offers. Pick one above to replace it.
          </p>
        )}

        {selectedOption !== null && selectedResolution !== null && (
          <div className="accel-selected">
            <span className="perf-label">In force</span>
            <span>
              {selectedOption.label}
              {selectedResolution.accelerator !== null &&
                selectedResolution.accelerator.label !== selectedOption.label && (
                  <> → {selectedResolution.accelerator.label}</>
                )}
            </span>
            <span className="sub">{strictnessNote(selectedOption)}</span>
            {(selectedResolution.availability === "unusable" ||
              selectedResolution.availability === "unsupported") && (
              <p className="status-err">
                This machine cannot serve it: {selectedResolution.reason}
                {selectedOption.strict
                  ? " Because this choice is strict, transcription will fail rather than run somewhere else."
                  : ""}
              </p>
            )}
            {selectedResolution.availability === "unknown" && (
              <p className="sub">
                Availability could not be established: {selectedResolution.reason}
              </p>
            )}
            {active !== null && active.loaded && (
              <span className="sub">
                Right now the engine is running on <strong>{describeActive(active, accel)}</strong>
                {active.model_id === "" ? "" : `, model ${active.model_id}`}. That is read back
                from the engine, not predicted from this screen.
              </span>
            )}
            {active !== null && !active.loaded && active.error !== null && (
              <p className="status-err">The dictation engine could not load: {active.error}</p>
            )}
            {active !== null && !active.loaded && active.error === null && (
              <span className="sub">
                No engine is loaded yet — the first dictation loads one, and this line then reports
                what it chose.
              </span>
            )}
            {acceleratorMismatch !== null && (
              <p className="status-err">{acceleratorMismatch}</p>
            )}
            {lastRun !== null && (
              <span className="sub">
                Last benchmark in this session ran on{" "}
                <strong>{lastRun.backend}</strong>
                {lastRun.requested === null
                  ? " (with the preference from Settings)."
                  : ` (asked for ${preferenceLabel(options, lastRun.requested)}).`}
              </span>
            )}
            <span className="sub">
              Saving applies it to the next transcription. Only a benchmark measures what actually
              executed.
            </span>
          </div>
        )}
      </section>

      <MicrophoneField
        value={settings.audio.input_device}
        savedValue={savedDevice}
        disabled={saving}
        dictating={state === "listening" || state === "processing"}
        onChange={(device) =>
          update({ audio: { ...settings.audio, input_device: device } })
        }
      />

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
      </div>

      <SoundCues settings={settings} disabled={saving} onChange={update} />

      <AutostartField mirror={settings.autostart} onVerified={mirrorAutostart} />

      <button className="btn" onClick={() => void save()} disabled={saving || hotkeyErr !== null}>
        {saving ? "Saving…" : "Save"}
      </button>

      {status.kind === "saved" && <p className="status-ok">Settings saved.</p>}
      {status.kind === "error" && <p className="status-err">Save failed: {status.message}</p>}
    </div>
  );
}
