# LocalWisper — Using the app

_What the desktop app does, and how to drive it. For the CLI see [`models.md`](models.md) and
[`benchmarks.md`](benchmarks.md); for build instructions see [`build.md`](build.md)._

The window has five tabs: **Dictate**, **Settings**, **Models**, **Diagnostics** and **Benchmark**.
Closing the window hides it to the tray; "Quit" in the tray menu exits.

---

## 1. Dictating

Hold the hotkey, speak, release. The default binding is **Ctrl + Alt + Space**, and the hint on the
Dictate tab always shows the binding that is actually registered with the OS — if registration
failed (another app owning the combination, most often), it says so rather than promising a key
that does nothing.

Transcribed text is injected into the focused application. The overlay window shows recording
state without stealing focus.

---

## 2. Hotkey and modes

Settings → **Hotkey**. Pick modifiers (Ctrl / Alt / Shift / Win) and a trigger key, or use the
capture affordance and press the combination you want.

| Mode | Behaviour |
|---|---|
| **Push to talk** | Hold the keys while speaking; release to stop. The default. |
| **Toggle** | Press once to start, press again to stop. Nothing to hold. |
| **Hands-free** | Press once and speak; recording ends by itself when you stop talking. |

Hands-free uses the same VAD endpoint detector as the CLI (Silero + energy, see
[`architecture.md`](architecture.md)), so the trailing-silence and hangover settings under
Settings → VAD change when it decides you have finished.

**A hotkey needs a real key, not only modifiers.** A modifiers-only binding (say Ctrl + Win) cannot
be registered through the OS global-shortcut API — it needs a low-level keyboard hook, which this
build does not ship. Settings rejects such a binding with that message instead of accepting it and
quietly listening on the default keys.

After saving, Settings shows what the OS actually registered, so you can see the change took.

---

## 3. Choosing a model

The **Models** tab lists the catalog with, for each entry: size, languages, a quality tier, a speed
tier, and an **estimated** real-time factor for your machine. The entry recommended for your
hardware is marked.

Two kinds of number appear there, and the app never mixes them up:

- **Estimated RTF** — derived from the model's speed tier and your detected hardware. It is a
  guess, labelled as one, so you can shortlist before downloading 640 MB.
- **Measured** — a real run. Every measurement carries the machine it was taken on, because a
  number from a Snapdragon X2 says nothing about your laptop. Use **Benchmark** for numbers from
  *your* machine.

Download shows per-file progress and a file counter. Every file is checked against a pinned
SHA-256 before it is used, staged in a temporary directory and promoted atomically, so an
interrupted download never leaves a half-installed model. Cancelling is not an error: partial
files stay in staging and the next attempt resumes them.

Entries with no pinned manifest cannot be downloaded from here. That is deliberate — see
[`models.md`](models.md) §4.

Some models need a build with the optional `sherpa` engine feature; the list says so and disables
download rather than offering something this build cannot run. See [`licenses.md`](licenses.md)
for why that feature is off by default.

Models live in the app's own data directory. To make the CLI agree about what is installed, point
it there with `LW_MODELS_ROOT` ([`models.md`](models.md)).

---

## 4. Benchmarking your own machine

The **Benchmark** tab measures the selected model here and now. It reports:

- the **backend that actually executed** — read back from the engine, not from what was requested,
  so a run that asked for the NPU and fell back to the CPU says CPU;
- **notes** explaining the choice, including the reason for any fallback;
- per-clip wall time, RTF and (when the clips have reference transcripts) WER;
- a **cold** RTF for the first run and a **warm** mean for the rest.

It takes tens of seconds. The **first NPU run takes minutes**: the encoder's Hexagon context binary
is prepared on-device and cached (~1.2 GB), after which it reloads in about two seconds.

Benchmarks run on the dictation worker, so dictation pauses while one is in flight. That is
deliberate: two engines would mean two QNN sessions competing for one Hexagon context, and the
failure would look like a benchmark result.

The equivalent from a terminal is `lw bench --quick`, which shares the same measurement code — see
[`benchmarks.md`](benchmarks.md).

---

## 5. Diagnostics and logs

The **Diagnostics** tab reports the ONNX Runtime it found, whether the QNN provider DLL is present,
whether an NPU device actually enumerated, and the app/OS versions. "Present" and "usable" are
reported separately: a driver package can be installed while the execution provider fails to load,
and only the second one means acceleration.

Logs are written under the app data directory in `logs/localwisper.log.<date>`, at `info` by
default. Raise it with `LW_LOG=debug`.

**Nothing private is logged.** No audio, transcript text, clipboard contents or keys are ever
written — the logs carry device, backend and version facts only.

---

## 6. Where things live

| What | Windows | macOS | Linux |
|---|---|---|---|
| Settings | `%APPDATA%\ai.localwisper.app\settings.json` | `~/Library/Application Support/ai.localwisper.app/settings.json` | `~/.local/share/ai.localwisper.app/settings.json` |
| Models | `…\ai.localwisper.app\models` | `…/ai.localwisper.app/models` | `…/ai.localwisper.app/models` |
| NPU context cache | `…\ai.localwisper.app\cache` | — | — |
| Logs | `…\ai.localwisper.app\logs` | `…/ai.localwisper.app/logs` | `…/ai.localwisper.app/logs` |

Deleting `settings.json` resets to defaults; the app recreates it on the next save.
