// Typed wrappers around Tauri invoke/listen.
//
// IPC contract (matches app/src-tauri/src/commands.rs):
//   commands: get_settings, set_settings, list_backends, get_diagnostics,
//             get_recording_state, set_recording_state, subscribe_mic_level
//   events:   state_changed  { state: RecordingState }
//             open_tab       "settings" | "diagnostics"
//   channel:  subscribe_mic_level(channel: Channel<number>) — mic level in [0, 1]

import { Channel, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** Recording states emitted by the backend (lowercase serde form). */
export type RecordingState = "idle" | "listening" | "processing" | "done" | "error";

/** Payload of the `state_changed` event. */
export interface StatePayload {
  readonly state: RecordingState;
}

/** serde names of lw-core's BackendPreference. */
export type BackendPreference = "automatic" | "force_npu" | "force_cpu";

/** serde names of lw-core's HotkeyMode. */
export type HotkeyMode = "push_to_talk" | "toggle" | "hands_free";

/** Any JSON value (used for fields the UI treats opaquely). */
export type JsonValue =
  | string
  | number
  | boolean
  | null
  | JsonValue[]
  | { [key: string]: JsonValue };

export interface HotkeyConfig {
  modifiers: string[];
  trigger: string;
  mode: HotkeyMode;
}

export interface AudioConfig {
  input_device: string;
  min_record_secs: number;
  max_record_secs: number;
}

export interface VadConfig {
  threshold: number;
  frame_ms: number;
  min_speech_ms: number;
  hangover_ms: number;
  pre_roll_ms: number;
  trailing_pad_ms: number;
  max_segment_ms: number;
}

export interface LlmConfig {
  enabled: boolean;
  base_url: string;
  model: string;
  key_in_keychain: boolean;
}

/** Mirror of lw_core::settings::Settings (unknown/extra fields survive at runtime). */
export interface Settings {
  version: number;
  hotkey: HotkeyConfig;
  audio: AudioConfig;
  backend: BackendPreference;
  model_id: string;
  vad: VadConfig;
  overlay_enabled: boolean;
  sounds_enabled: boolean;
  llm: LlmConfig;
  dictionary: JsonValue;
  profiles: JsonValue;
  log_level: string;
}

/** get_diagnostics returns a flat-ish JSON object; render it generically. */
export type Diagnostics = Record<string, JsonValue>;

export function getSettings(): Promise<Settings> {
  return invoke<Settings>("get_settings");
}

export function setSettings(settings: Settings): Promise<Settings> {
  return invoke<Settings>("set_settings", { settings });
}

export function listBackends(): Promise<BackendPreference[]> {
  return invoke<BackendPreference[]>("list_backends");
}

export function getDiagnostics(): Promise<Diagnostics> {
  return invoke<Diagnostics>("get_diagnostics");
}

export function getRecordingState(): Promise<RecordingState> {
  return invoke<RecordingState>("get_recording_state");
}

export function setRecordingState(next: RecordingState): Promise<void> {
  return invoke<void>("set_recording_state", { next });
}

/** Subscribe to `state_changed`. Returns the unlisten function. */
export function onStateChanged(handler: (payload: StatePayload) => void): Promise<UnlistenFn> {
  return listen<StatePayload>("state_changed", (event) => handler(event.payload));
}

/** Tray menu items ask the frontend to switch tab via `open_tab`. */
export function onOpenTab(handler: (tab: string) => void): Promise<UnlistenFn> {
  return listen<string>("open_tab", (event) => handler(event.payload));
}

/** Register the mic-level stream (stubbed with synthetic levels in the backend for now). */
export function subscribeMicLevel(handler: (level: number) => void): Promise<void> {
  const channel = new Channel<number>();
  channel.onmessage = handler;
  return invoke<void>("subscribe_mic_level", { channel });
}
