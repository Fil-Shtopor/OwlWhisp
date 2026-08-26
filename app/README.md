# LocalWisper desktop app

Tauri 2 shell (`src-tauri/`, crate `localwisper`) + React 19/TypeScript frontend (`frontend/`).

## Development

The ARM64 MSVC toolset is portable — source its env before anything that compiles Rust
(Git Bash):

```bash
source ~/.msvc-arm64/env-arm64.sh
export PATH="$HOME/.cargo/bin:$PATH"
```

Run the app in dev mode (starts Vite on :5173 automatically via `beforeDevCommand`):

```bash
cd app/frontend
npm install          # first time only
npm run tauri dev    # uses @tauri-apps/cli; equivalent: npx tauri dev
```

Frontend-only iteration: `npm run dev` (Vite) / `npm run build` (tsc + Vite, outputs `dist/`).
Backend-only check from the repo root: `cargo check -p localwisper`
(the frontend `dist/` must exist first — `tauri::generate_context!` reads it).

Release bundle (NSIS): `npm run tauri build`.

## What exists vs. what is stubbed

Working: tray (Settings / Diagnostics / Quit), main settings window (hides to tray on
close), hidden transparent click-through overlay window, global shortcut
**Ctrl+Alt+Space** (push-to-talk) driving the recording-state machine, settings
persistence via `lw_core::settings::Settings` (`app_data_dir/settings.json`),
ORT/QNN/NPU diagnostics via `lw_ort::OrtRuntime`.

Stubbed (marked `TODO: wire to lw-platform AudioCapture + engine` in
`src-tauri/src/lib.rs` and `commands.rs`): audio capture, VAD, transcription, text
injection. Releasing the hotkey plays a **simulated** processing → done → idle sequence,
and the mic-level channel emits synthetic values, purely so the UI/overlay can be
developed against the real event/channel plumbing.

## IPC contract

Commands (`invoke`):

| Command | Args | Returns |
| --- | --- | --- |
| `get_settings` | – | `Settings` JSON (defaults if no file yet) |
| `set_settings` | `settings: Settings` | normalized `Settings` JSON (validated + saved atomically) |
| `list_backends` | – | `["automatic", "force_npu", "force_cpu"]` |
| `get_diagnostics` | – | JSON object: `app_version`, `core_version`, `os`, `os_version`, `arch`, then `runtime_dir`, `qnn_dll_present`, `npu_available`, `qnn_registered`, `qnn_npu_count`, `devices[]` — or `runtime_error` |
| `get_recording_state` | – | `"idle" \| "listening" \| "processing" \| "done" \| "error"` |
| `set_recording_state` | `next: RecordingState` | – (also emits `state_changed`) |
| `subscribe_mic_level` | `channel: Channel<number>` | – (levels in `[0, 1]` streamed while listening) |

Events (`listen`):

| Event | Payload | Emitted when |
| --- | --- | --- |
| `state_changed` | `{ state: RecordingState }` | every recording-state transition (hotkey or `set_recording_state`) |
| `open_tab` | `"settings" \| "diagnostics"` | tray menu asks the main window to switch tab |

Windows: `main` (settings UI, defined in `tauri.conf.json`) and `overlay`
(created hidden in Rust: transparent, undecorated, always-on-top, skip-taskbar,
non-focusable, cursor events ignored; shown while state ≠ idle when
`settings.overlay_enabled`). Typed frontend wrappers live in `frontend/src/ipc.ts`.
