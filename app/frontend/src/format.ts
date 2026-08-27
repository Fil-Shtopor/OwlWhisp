// Presentation helpers shared by the panels.
//
// House rule: an estimate is never rendered the way a measurement is. `formatEstimatedRtf` always
// carries the "(estimated)" suffix; `formatRtf` is only ever fed numbers that were measured.
//
// The hotkey helpers live here too, so the editor in Settings and the hint on Dictate spell a
// binding exactly the same way and there is only one place that knows the alias spellings.

import type { HotkeyConfig } from "./ipc";

const BYTE_UNITS = ["KiB", "MiB", "GiB", "TiB"] as const;

/** Humanise a byte count, binary units. `null` becomes an explicit "unknown". */
export function formatBytes(bytes: number | null): string {
  if (bytes === null || !Number.isFinite(bytes) || bytes < 0) return "unknown";
  if (bytes < 1024) return `${Math.round(bytes)} B`;
  let value = bytes / 1024;
  let index = 0;
  while (value >= 1024 && index < BYTE_UNITS.length - 1) {
    value /= 1024;
    index += 1;
  }
  const unit = BYTE_UNITS[index];
  return `${value < 10 ? value.toFixed(1) : Math.round(value).toString()} ${unit}`;
}

/** A measured real-time factor. Never use this for catalog estimates. */
export function formatRtf(rtf: number): string {
  return rtf.toFixed(4);
}

/** A catalog estimate. The "(estimated)" tail is not optional — it is the point. */
export function formatEstimatedRtf(rtf: number | null): string {
  if (rtf === null) return "not estimated";
  return `~${rtf.toFixed(4)} (estimated)`;
}

/** WER arrives as a fraction (0.054 = 5.4%). */
export function formatWer(wer: number | null): string {
  if (wer === null) return "—";
  return `${(wer * 100).toFixed(1)}%`;
}

/** Milliseconds, rounded; seconds once it gets long enough that ms stop reading well. */
export function formatMs(ms: number): string {
  if (ms >= 10_000) return `${(ms / 1000).toFixed(1)} s`;
  return `${Math.round(ms)} ms`;
}

export function formatSeconds(secs: number): string {
  return `${secs.toFixed(2)} s`;
}

// ---------------------------------------------------------------------------------------------
// Hotkey presentation
// ---------------------------------------------------------------------------------------------

export interface ModifierOption {
  /** The canonical name written back to settings. */
  readonly id: string;
  readonly label: string;
  /** Extra wording for a checkbox, e.g. "Win/Cmd". */
  readonly detail: string | null;
  /**
   * Spellings lw-core also accepts in a settings file written elsewhere. A binding holding one of
   * them must still tick the right box and render with the right name.
   */
  readonly aliases: readonly string[];
}

export const MODIFIER_OPTIONS: readonly ModifierOption[] = [
  { id: "ctrl", label: "Ctrl", detail: null, aliases: ["ctrl", "control"] },
  { id: "alt", label: "Alt", detail: null, aliases: ["alt", "option"] },
  { id: "shift", label: "Shift", detail: null, aliases: ["shift"] },
  { id: "meta", label: "Meta", detail: "Win/Cmd", aliases: ["meta", "win", "super", "cmd"] },
];

export function modifierLabel(name: string): string {
  const trimmed = name.trim();
  const known = MODIFIER_OPTIONS.find((m) => m.aliases.includes(trimmed.toLowerCase()));
  if (known !== undefined) return known.label;
  return trimmed.length === 0 ? name : trimmed.charAt(0).toUpperCase() + trimmed.slice(1);
}

/** Whether the binding holds this modifier under any of its accepted spellings. */
export function hasModifier(hotkey: HotkeyConfig, id: string): boolean {
  const option = MODIFIER_OPTIONS.find((m) => m.id === id);
  if (option === undefined) return false;
  return hotkey.modifiers.some((m) => option.aliases.includes(m.trim().toLowerCase()));
}

export function triggerLabel(key: string): string {
  switch (key.trim().toLowerCase()) {
    case "space":
      return "Space";
    case "tab":
      return "Tab";
    case "enter":
    case "return":
      return "Enter";
    case "esc":
    case "escape":
      return "Esc";
    case "capslock":
    case "caps_lock":
      return "Caps Lock";
    case "insert":
      return "Insert";
    case "backquote":
    case "grave":
      return "`";
    case "none":
    case "":
      return "no key";
    default:
      return key.toUpperCase();
  }
}

/**
 * The trigger key, normalised. An empty string means the binding has no real key — which
 * `HotkeyConfig::validate` rejects, because registering an OS global shortcut needs one.
 */
export function hotkeyTrigger(hotkey: HotkeyConfig): string {
  const trigger = hotkey.trigger.trim().toLowerCase();
  return trigger === "none" ? "" : trigger;
}

/** Human key names in press order, e.g. `["Ctrl", "Alt", "Space"]`. */
export function hotkeyParts(hotkey: HotkeyConfig): string[] {
  const parts = hotkey.modifiers.map(modifierLabel);
  const trigger = hotkeyTrigger(hotkey);
  if (trigger !== "") parts.push(triggerLabel(trigger));
  return parts;
}

/** One-line spelling of a binding, e.g. "Ctrl + Alt + Space". */
export function formatHotkey(hotkey: HotkeyConfig): string {
  const parts = hotkeyParts(hotkey);
  if (parts.length === 0) return "nothing bound";
  if (hotkeyTrigger(hotkey) === "") return `${parts.join(" + ")} + (no key)`;
  return parts.join(" + ");
}
