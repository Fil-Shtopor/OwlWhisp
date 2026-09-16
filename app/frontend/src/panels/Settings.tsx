import { useEffect, useState } from "react";
import {
  activeBackend,
  activeHotkey,
  getSettings,
  listBackends,
  setSettings,
  type AcceleratorReport,
  type ActiveBackend,
  type BackendOption,
  type HotkeyConfig,
  type HotkeyMode,
  type Settings,
} from "../ipc";
import {
  formatHotkey,
  hasModifier,
  hotkeyTrigger,
  triggerLabel,
  MODIFIER_OPTIONS,
} from "../format";
import {
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
import { AcceleratorTable } from "../AcceleratorTable";

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

export function SettingsPanel() {
  const [settings, setLocal] = useState<Settings | null>(null);
  // Null until `list_backends` answers: the labels belong to the Rust side, so there is nothing
  // honest to show before it does.
  const [backends, setBackends] = useState<BackendOption[] | null>(null);
  const [backendsError, setBackendsError] = useState<string | null>(null);
  const [accel, setAccel] = useState<AcceleratorReport | null>(null);
  const [accelError, setAccelError] = useState<string | null>(null);
  const [accelLoading, setAccelLoading] = useState(true);
  const [probeNonce, setProbeNonce] = useState(0);
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
        if (!disposed) setLocal(s);
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

  // Probing loads provider libraries, so it is a real call — the shared result is reused, and
  // only "Re-probe" forces a fresh one.
  useEffect(() => {
    let disposed = false;
    setAccelLoading(true);
    void probeAccelerators(probeNonce > 0)
      .then((report) => {
        if (!disposed) {
          setAccel(report);
          setAccelError(null);
        }
      })
      .catch((e: unknown) => {
        if (!disposed) {
          setAccel(null);
          setAccelError(String(e));
        }
      })
      .finally(() => {
        if (!disposed) setAccelLoading(false);
      });
    return () => {
      disposed = true;
    };
  }, [probeNonce]);

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

  /**
   * The disagreement that matters: an engine that is loaded and running on the CPU while this
   * selection resolves to something faster. Deliberately narrow — it compares the engine's own
   * acceleration class against the resolved accelerator's kind, and only in the CPU direction,
   * because a provider may legitimately report a class that is not its accelerator's kind (CoreML
   * reports ANE while its accelerator is classed as a GPU). A warning has to be certain.
   */
  const cpuFallbackWarning: string | null = (() => {
    if (active === null || !active.loaded || active.acceleration !== "CPU") return null;
    const target = selectedResolution?.accelerator ?? null;
    if (target === null || target.kind === "cpu") return null;
    const why = active.notes.length === 0 ? "" : ` The engine's reasons: ${active.notes.join(" · ")}`;
    return `The loaded engine is running on the CPU, although this selection resolves to ${target.label}.${why}`;
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
                Right now the engine is running on <strong>{describeActive(active)}</strong>
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
            {cpuFallbackWarning !== null && <p className="status-err">{cpuFallbackWarning}</p>}
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

      <section className="field accel-section">
        <div className="field-row">
          <label>This machine</label>
          <button
            className="btn secondary small"
            onClick={() => setProbeNonce((n) => n + 1)}
            disabled={accelLoading}
          >
            {accelLoading ? "Probing…" : "Re-probe"}
          </button>
        </div>
        <AcceleratorTable report={accel} loadError={accelError} loading={accelLoading} />
      </section>

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

      <button className="btn" onClick={() => void save()} disabled={saving || hotkeyErr !== null}>
        {saving ? "Saving…" : "Save"}
      </button>

      {status.kind === "saved" && <p className="status-ok">Settings saved.</p>}
      {status.kind === "error" && <p className="status-err">Save failed: {status.message}</p>}
    </div>
  );
}
