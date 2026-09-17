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
  /** Which cue sound plays. See `listSoundThemes()`. */
  sound_theme: SoundThemeId;
  /** Cue volume in [0, 1]. */
  sound_volume: number;
  /**
   * Mirror of the OS autostart registration. The OS is the source of truth — read it with
   * `getAutostart()` and change it with `setAutostart()`; this field only gives the UI something
   * to render before that probe returns.
   */
  autostart: boolean;
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

/**
 * What the dictation engine is running on right now — read back from the engine, not predicted
 * from settings. A run that asked for the NPU and fell back reports the CPU, and `notes` says why.
 */
export interface ActiveBackend {
  /** False until the first dictation: the engine loads lazily, and before that nothing is running. */
  loaded: boolean;
  provider: string | null;
  /** Display text for the acceleration class. Do not match on this — use `accelerator_kind`. */
  acceleration: string | null;
  /** Stable accelerator id (`"qnn_npu"`, `"web_gpu"`, …), matching `AcceleratorStatus.id`. */
  accelerator: string | null;
  /** That accelerator's class as a stable id: `"cpu"` | `"gpu"` | `"npu"`. */
  accelerator_kind: AcceleratorKind | null;
  device: string | null;
  notes: string[];
  model_id: string;
  /** Why loading failed, when it did. */
  error: string | null;
}

/** Ask the dictation worker what its engine actually selected. Does not load one. */
export function activeBackend(): Promise<ActiveBackend> {
  return invoke<ActiveBackend>("active_backend");
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

/**
 * Register the mic-level stream.
 *
 * Levels are the **real** RMS of the microphone, mapped to `[0, 1]` on a -60..0 dBFS scale so
 * ordinary speech uses the middle of the bar. The stream is alive while dictating, and while
 * `setMicTest(true)` holds a microphone open; it is pushed to zero when either ends. Before this
 * existed the backend generated a sine wave, which moved convincingly and meant nothing.
 */
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
  /**
   * Who made the model ("NVIDIA", "OpenAI", …), for grouping the list by maker.
   *
   * Null when the catalog does not record one; such entries belong under a trailing "Other"
   * group rather than being hidden. Deliberately not derived from `engine`: an engine is a
   * decoder architecture, and Whisper and Distil-Whisper share one while coming from different
   * organisations.
   */
  vendor: string | null;
  /**
   * The jobs this entry is here to do, so the catalog can be read by need rather than by name.
   * Editorial, like the quality and speed tiers. Empty means the entry earns its place some other
   * way — usually by covering languages nothing else here covers.
   */
  roles: ModelRole[];
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

/** One of the jobs an entry can be tagged with; the vocabulary comes from the backend. */
export interface ModelRole {
  id: string;
  label: string;
  /** One line saying what picking this role gets you. */
  blurb: string;
}

export interface ModelCatalog {
  machine: string;
  models_root: string;
  manifests_dir: string | null;
  /** Entry id recommended for this machine, if any. */
  recommended: string | null;
  /** Sentence the UI must show next to any estimated number. */
  estimate_disclaimer: string;
  /** Every role this build knows about, in the order they should be offered. */
  roles: ModelRole[];
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
  /** WER against the reference, when the clip had one. Computed even when it was not scored. */
  wer: number | null;
  /** The clip's language tag (`"en"`, `"ru"`, …), when the fixture set names one. */
  language: string | null;
  /**
   * Whether this clip's WER counted towards `BenchReport.wer`.
   *
   * False when the model does not claim the clip's language: it was still transcribed and timed,
   * so it contributes to RTF, but scoring it would measure the question rather than the model.
   */
  scored: boolean;
}

/** Every number here is MEASURED on this machine by this run. */
export interface BenchReport {
  machine: string;
  /** The backend that actually executed, e.g. "QNN on NPU" or "ONNX Runtime CPU on CPU". */
  backend: string;
  /** That backend's stable accelerator id, for comparing runs without matching display prose. */
  accelerator: string | null;
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

// ---------------------------------------------------------------------------------------------
// Cue sounds
// ---------------------------------------------------------------------------------------------

export type SoundThemeId = "chime" | "blip" | "click" | "marimba";

export interface SoundThemeOption {
  id: SoundThemeId;
  label: string;
  description: string;
}

/** The cue sounds this build offers. */
export function listSoundThemes(): Promise<SoundThemeOption[]> {
  return invoke<SoundThemeOption[]>("list_sound_themes");
}

/**
 * Play one cue so the user can hear a theme before committing to it.
 *
 * Takes the theme and volume explicitly rather than reading settings, so a preview follows the
 * controls being moved right now instead of the last saved value.
 */
export function previewSound(
  theme: SoundThemeId,
  volume?: number,
  cue?: "start" | "stop",
): Promise<void> {
  return invoke<void>("preview_sound", { theme, volume: volume ?? null, cue: cue ?? null });
}

// ---------------------------------------------------------------------------------------------
// Start at login
// ---------------------------------------------------------------------------------------------

/** Whether the OS is currently set to start LocalWisper at login. */
export function getAutostart(): Promise<boolean> {
  return invoke<boolean>("get_autostart");
}

/**
 * Register or unregister start-at-login, and return **what the OS reports afterwards**.
 *
 * That is not always what was asked: a managed machine can refuse. Render the returned value, so
 * the checkbox can never claim an autostart that does not exist.
 */
export function setAutostart(enabled: boolean): Promise<boolean> {
  return invoke<boolean>("set_autostart", { enabled });
}

// ---------------------------------------------------------------------------------------------
// Comparing every backend
// ---------------------------------------------------------------------------------------------

/** An accelerator that was offered but not measured, and why. */
export interface BenchSkipped {
  accelerator: string;
  label: string;
  reason: string;
}

/** Every usable accelerator measured on the same clips, so the numbers can honestly be compared. */
export interface BenchSuite {
  machine: string;
  model_id: string;
  /** One clip set for all runs — that is what makes this a comparison rather than a list. */
  clip_source: string;
  runs: BenchReport[];
  skipped: BenchSkipped[];
  /** Accelerator id of the fastest run by WARM RTF, or null if nothing ran. */
  fastest: string | null;
  /** Accelerator id of the most accurate run by WER, when WER was computable. */
  most_accurate: string | null;
}

/**
 * Measure the model on every usable accelerator, in one action.
 *
 * Long-running — minutes, since each backend loads its own engine and a first NPU run prepares a
 * context binary. Subscribe with `onBenchmarkProgress` to show which one is running.
 */
export function runBenchmarkAll(modelId?: string): Promise<BenchSuite> {
  return invoke<BenchSuite>("run_benchmark_all", { modelId: modelId ?? null });
}

/** Payload of `benchmark_progress`, emitted as each backend starts and finishes. */
export interface BenchmarkProgress {
  index: number;
  total: number;
  accelerator: string;
  label: string;
  stage: "running" | "done" | "failed";
  /** Present on "done". */
  warm_rtf?: number | null;
  /** Present on "done". */
  wer?: number | null;
  /** Present on "failed". */
  reason?: string;
}

/** Subscribe to per-backend progress during `runBenchmarkAll`. */
export function onBenchmarkProgress(
  handler: (p: BenchmarkProgress) => void,
): Promise<UnlistenFn> {
  return listen<BenchmarkProgress>("benchmark_progress", (event) => handler(event.payload));
}

// ---------------------------------------------------------------------------------------------
// Microphone check, and the text that comes out
// ---------------------------------------------------------------------------------------------

/**
 * Open or close a microphone stream that only drives the level meter.
 *
 * Nothing is transcribed and nothing is kept — the audio is read for its amplitude and dropped.
 * Returns whether the stream is open afterwards, which is not always what was asked: a machine
 * that refuses microphone access reports false.
 */
export function setMicTest(enabled: boolean): Promise<boolean> {
  return invoke<boolean>("set_mic_test", { enabled });
}

/**
 * The microphone input devices this machine offers.
 *
 * Matching is by substring, case-insensitive: `settings.audio.input_device` holds a fragment of
 * a device name, and an empty string means the system default. A saved name that matches nothing
 * falls back to the default — worth surfacing, since the alternative is a user wondering why
 * their chosen microphone is not being used.
 */
export function listInputDevices(): Promise<string[]> {
  return invoke<string[]>("list_input_devices");
}

/** Payload of the `transcript` event, emitted once an utterance has been delivered. */
export interface TranscriptPayload {
  /** The final text, after the cleanup pipeline. */
  text: string;
  /** True if it was typed into the focused application; false if it went to the clipboard. */
  injected: boolean;
  /** The backend that produced it, e.g. "QNN". */
  provider: string;
}

/**
 * Subscribe to dictation failures reported by the worker.
 *
 * These are the things that stop an utterance: the microphone refusing to open (including a
 * configured device that is no longer present), the engine failing to load, capture failing
 * mid-utterance. The state machine flashes through `error` back to `idle`, so without this the
 * user sees a blink and no reason.
 */
export function onWorkerError(handler: (message: string) => void): Promise<UnlistenFn> {
  return listen<string>("worker_error", (event) => handler(event.payload));
}

/** Subscribe to finished transcripts. Returns the unlisten function. */
export function onTranscript(handler: (payload: TranscriptPayload) => void): Promise<UnlistenFn> {
  return listen<TranscriptPayload>("transcript", (event) => handler(event.payload));
}
