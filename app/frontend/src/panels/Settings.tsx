import { useEffect, useState } from "react";
import {
  activeHotkey,
  getSettings,
  listBackends,
  setSettings,
  type BackendPreference,
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

const BACKEND_LABELS: Record<BackendPreference, string> = {
  automatic: "Automatic",
  force_npu: "NPU",
  force_cpu: "CPU",
};

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

function triggerLabel(key: string): string {
  switch (key) {
    case "space":
      return "Space";
    case "tab":
      return "Tab";
    case "enter":
      return "Enter";
    case "esc":
      return "Esc";
    case "none":
      return "None (modifiers only)";
    default:
      return key.toUpperCase();
  }
}

function modifierLabel(name: string): string {
  const known = MODIFIER_OPTIONS.find((m) => m.aliases.includes(name.toLowerCase()));
  return known?.label.split(" ")[0] ?? name.charAt(0).toUpperCase() + name.slice(1);
}

/** Same rule as `HotkeyConfig::is_modifier_only`, including its case-insensitivity. */
function isModifierOnly(hotkey: HotkeyConfig): boolean {
  const trigger = hotkey.trigger.trim().toLowerCase();
  return trigger === "none" || trigger === "";
}

function hasModifier(hotkey: HotkeyConfig, id: string): boolean {
  const option = MODIFIER_OPTIONS.find((m) => m.id === id);
  if (option === undefined) return false;
  return hotkey.modifiers.some((m) => option.aliases.includes(m.toLowerCase()));
}

/** Mirrors `HotkeyConfig::validate` in crates/lw-core/src/settings/mod.rs. */
function hotkeyError(hotkey: HotkeyConfig): string | null {
  if (isModifierOnly(hotkey)) {
    if (hotkey.modifiers.length < 2) {
      return "A modifiers-only hotkey needs at least two modifiers — otherwise a single Ctrl press would trigger it.";
    }
    return null;
  }
  if (!VALID_TRIGGERS.has(hotkey.trigger.trim().toLowerCase())) {
    return `Unsupported hotkey key: ${hotkey.trigger}`;
  }
  return null;
}

function formatHotkey(hotkey: HotkeyConfig): string {
  const parts = hotkey.modifiers.map(modifierLabel);
  if (!isModifierOnly(hotkey)) parts.push(triggerLabel(hotkey.trigger));
  return parts.length > 0 ? parts.join(" + ") : "nothing bound";
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
        if (!disposed && b.length > 0) setBackends(b);
      })
      .catch(() => {
        // keep the static fallback list
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
  const triggerValue = isModifierOnly(settings.hotkey) ? "none" : settings.hotkey.trigger;

  const save = async () => {
    setSaving(true);
    try {
      const normalized = await setSettings(settings);
      setLocal(normalized);
      setStatus({ kind: "saved" });
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
                <span>{mod.label}</span>
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
              <optgroup label="Saved value">
                <option value={triggerValue}>{triggerValue}</option>
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
            &quot;None&quot; makes a modifiers-only binding, which runs on the low-level keyboard
            hook instead of the OS shortcut registry.
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
              {isModifierOnly(settings.hotkey)
                ? "No OS accelerator — a modifiers-only binding is served by the low-level keyboard hook, which this field cannot see."
                : "The OS holds no accelerator for this app right now. If you just saved, registration did not take — another app may already own the combination."}
            </span>
          )}
        </div>
      </section>

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

      <button className="btn" onClick={() => void save()} disabled={saving || hotkeyErr !== null}>
        {saving ? "Saving…" : "Save"}
      </button>

      {status.kind === "saved" && <p className="status-ok">Settings saved.</p>}
      {status.kind === "error" && <p className="status-err">Save failed: {status.message}</p>}
    </div>
  );
}
