// Typed wrappers around Tauri invoke/listen.
//
// IPC contract (matches app/src-tauri/src/commands.rs):
//   commands: get_settings, set_settings, list_backends, get_diagnostics,
//             get_recording_state, set_recording_state, subscribe_mic_level, active_hotkey
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

/**
 * serde names of lw-core's BackendPreference.
 *
 * The first four are coarse — say what *kind* of hardware to use and let the machine pick the
 * vendor. The rest pin one exact execution provider, which is what you want when comparing them:
 * a run that silently fell back would attribute its numbers to the wrong backend.
 */
export type BackendPreference =
  | "automatic"
  | "force_npu"
  | "force_gpu"
  | "force_cpu"
  | "qnn"
  | "web_gpu"
  | "cuda"
  | "tensor_rt"
  | "direct_ml"
  | "core_ml"
  | "open_vino"
  | "vitis_ai";

/** One selectable entry for the backend picker. */
export interface BackendOption {
  value: BackendPreference;
  label: string;
  /** The accelerator id this pins (matches `AcceleratorStatus.id`), or null for a coarse choice. */
  accelerator: string | null;
  /** True when failing to honour it is an error rather than a fallback. */
  strict: boolean;
}

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

export function listBackends(): Promise<BackendOption[]> {
  return invoke<BackendOption[]>("list_backends");
}

/** Broad class of compute an accelerator provides. */
export type AcceleratorKind = "cpu" | "gpu" | "npu";

/**
 * One accelerator as it stands on this machine.
 *
 * `present`, `registered` and `devices` answer different questions and routinely disagree: a
 * vendor driver can be installed while its provider fails to load, or load and find no device.
 * **`usable` is the only one that means acceleration** — gate the UI on that.
 */
export interface AcceleratorStatus {
  id: string;
  label: string;
  kind: AcceleratorKind;
  kind_label: string;
  vendor: string | null;
  /** Provider library filename, or null when it is built in / unavailable on this OS. */
  library: string | null;
  /** True when this accelerator needs a model artifact compiled for it (NPUs do, GPUs do not). */
  needs_dedicated_artifact: boolean;
  present: boolean;
  registered: boolean;
  devices: number;
  usable: boolean;
  /** One line explaining the verdict, suitable to show verbatim. */
  detail: string;
  /** The preference value to save in order to pin this accelerator exactly. */
  preference: BackendPreference;
}

export interface AcceleratorReport {
  /** Non-null when availability could not be determined at all (e.g. no ONNX Runtime found). */
  error: string | null;
  items: AcceleratorStatus[];
}

/** What this machine can actually accelerate with, probed live. */
export function listAccelerators(): Promise<AcceleratorReport> {
  return invoke<AcceleratorReport>("list_accelerators");
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

/**
 * The accelerator the OS shortcut registry currently holds, e.g. `"Control+Alt+Space"`.
 *
 * `null` means no OS-level accelerator is registered, i.e. registration genuinely failed —
 * `HotkeyConfig::validate` rejects modifier-only bindings, so there is no other reason for it.
 */
export function activeHotkey(): Promise<string | null> {
  return invoke<string | null>("active_hotkey");
}

/** Register the mic-level stream (stubbed with synthetic levels in the backend for now). */
export function subscribeMicLevel(handler: (level: number) => void): Promise<void> {
  const channel = new Channel<number>();
  channel.onmessage = handler;
  return invoke<void>("subscribe_mic_level", { channel });
}

/** Payload of the `hotkey_changed` event, emitted on every (re)registration. */
export interface HotkeyChanged {
  /** What the OS holds, or null when registration failed. */
  accelerator: string | null;
  mode: HotkeyMode;
}

/**
 * Subscribe to `hotkey_changed`. The main webview exists before the backend finishes starting,
 * so a first `activeHotkey()` can legitimately answer null; this event carries the correction,
 * and every later rebinding too.
 */
export function onHotkeyChanged(handler: (payload: HotkeyChanged) => void): Promise<UnlistenFn> {
  return listen<HotkeyChanged>("hotkey_changed", (event) => handler(event.payload));
}

// ---------------------------------------------------------------------------------------------
// Hardware capabilities
// ---------------------------------------------------------------------------------------------

/** What the backend detected about this machine. Used to explain model recommendations. */
export interface Capabilities {
  /** One-line human summary, e.g. "Snapdragon X2 Elite ... 18 cores - NPU Hexagon V81 via QNN EP". */
  machine: string;
  cpu: string;
  cores: number;
  os: string;
  arch: string;
  npu_present: boolean;
  /** Null when there is no NPU, or when one is present but unusable. */
  npu_label: string | null;
  /** True only when the QNN execution provider actually loaded - not merely that a driver exists. */
  npu_usable: boolean;
}

export function getCapabilities(): Promise<Capabilities> {
  return invoke<Capabilities>("get_capabilities");
}

// ---------------------------------------------------------------------------------------------
// Model catalog
// ---------------------------------------------------------------------------------------------

export type QualityTier = "basic" | "good" | "better" | "best";
export type SpeedTier = "very_fast" | "fast" | "moderate" | "slow";
export type InstallState = "installed" | "incomplete" | "missing" | "unpinned";

/** A real measurement, tagged with the machine it was taken on. Never an estimate. */
export interface MeasuredPoint {
  hardware: string;
  rtf: number;
  wer: number | null;
  machine: string;
  source: string;
}

export interface ModelEntry {
  id: string;
  name: string;
  description: string;
  engine: string;
  licence: string;
  upstream_url: string | null;
  languages: string[];
  language_summary: string;
  quality: QualityTier;
  quality_label: string;
  speed: SpeedTier;
  speed_label: string;
  download_bytes: number | null;
  disk_bytes: number | null;
  /** Hardware targets this model has artifacts for, as human labels. */
  hardware: string[];
  /** Whether THIS build on THIS machine could run it today. */
  runnable: boolean;
  best_hardware: string | null;
  /** ESTIMATE, never a measurement. Render it as such. */
  estimated_rtf: number | null;
  /** A real measurement from the catalog - read `machine` before applying it to the user. */
  measured_reference: MeasuredPoint | null;
  measurements: MeasuredPoint[];
  wer_estimates: Record<string, number>;
  reason: string;
  blockers: string[];
  install_state: InstallState;
  install_dir: string | null;
  manifest_error: string | null;
  notes: string | null;
}

export interface ModelCatalog {
  machine: string;
  models_root: string;
  manifests_dir: string | null;
  /** Entry id recommended for this machine, if any. */
  recommended: string | null;
  /** Sentence the UI must show next to any estimated number. */
  estimate_disclaimer: string;
  entries: ModelEntry[];
}

export function listModels(): Promise<ModelCatalog> {
  return invoke<ModelCatalog>("list_models");
}

/** Progress events streamed while a model downloads. */
export type InstallEvent =
  | { event: "file_started"; path: string; total: number }
  | {
      event: "progress";
      path: string;
      received: number;
      total: number;
      file_index: number;
      file_count: number;
    }
  | { event: "file_verified"; path: string }
  | { event: "completed"; dir?: string }
  /** The user cancelled. Not an error: partial files stay in staging and resume next time. */
  | { event: "cancelled" }
  | { event: "failed"; message: string };

/**
 * Download and SHA-256-verify a model. Resolves when the download finishes; progress arrives
 * through `onEvent`. A `failed` event is also delivered before the promise rejects. A cancelled
 * download delivers `cancelled` and RESOLVES, so cancelling is never surfaced as an error.
 */
export function installModel(id: string, onEvent: (e: InstallEvent) => void): Promise<void> {
  const channel = new Channel<InstallEvent>();
  channel.onmessage = onEvent;
  return invoke<void>("install_model", { id, channel });
}

/** Ask an in-flight `install_model` to stop. Partial files stay in staging and resume later. */
export function cancelInstall(id: string): Promise<void> {
  return invoke<void>("cancel_install", { id });
}

// ---------------------------------------------------------------------------------------------
// Benchmark
// ---------------------------------------------------------------------------------------------

export interface BenchClip {
  name: string;
  duration_s: number;
  ms: number;
  rtf: number;
  wer: number | null;
}

/** Every number here is MEASURED on this machine by this run. */
export interface BenchReport {
  machine: string;
  /** The backend that actually executed, e.g. "QNN on NPU" or "ONNX Runtime CPU on CPU". */
  backend: string;
  model_id: string;
  model_dir: string;
  /** Where the audio came from, and whether WER was computable. */
  clip_source: string;
  engine_load_ms: number;
  clips: BenchClip[];
  /** First run - includes one-time warm-up. */
  cold_rtf: number;
  /** Mean over the runs after the first, or null when there was no second run. */
  warm_rtf: number | null;
  warm_count: number;
  /** Word-weighted WER, or null when the clips had no reference transcripts. */
  wer: number | null;
  audio_secs: number;
  /**
   * What the engine decided while selecting a backend, in order - including why it fell back.
   * A run that asked for the NPU and ended up on the CPU says so here; show them.
   */
  notes: string[];
}

/**
 * Run a benchmark on this machine. Long-running: the backend does it on a blocking thread.
 * `modelId` defaults to the model in Settings; `backend` defaults to the configured preference.
 */
export function runBenchmark(modelId?: string, backend?: BackendPreference): Promise<BenchReport> {
  return invoke<BenchReport>("run_benchmark", { modelId: modelId ?? null, backend: backend ?? null });
}
