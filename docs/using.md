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

Three things on this screen answer the questions people actually ask first.

**Try dictating here** is a scratchpad. Click it, press the hotkey and speak: the text arrives by
the same focus path it would use in any other application, so it tests the real mechanism rather
than a preview of it. Nothing typed or dictated there is saved, logged or sent anywhere, and it is
gone when the window closes.

**Last transcript** says what happened to the text — typed into the focused window, or written to
the clipboard because typing was refused. Those are different outcomes with different next steps,
so they are labelled differently rather than both reading as success.

**Microphone level** is the real RMS of the capture, mapped onto the usual −60…0 dBFS voice range.
It only moves while something is capturing, which is why **Test microphone** exists: it opens the
microphone, moves the bar, and transcribes nothing. The checkbox shows what the application
*managed* to do, not what was asked — if the machine refuses access, or a saved device is gone, it
stays off and says why. If the bar does not move it says the input is reaching the app as silence,
which is a real diagnosis; it never animates to look busy.

A failure from the dictation worker appears here as a banner carrying the worker's own message,
and stays until it is dismissed or the next transcript arrives.

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

The **Models** tab lists the catalog as one row per model, so the whole catalog fits on a screen:
size, family, languages, a quality tier, an **estimated** real-time factor for your machine and,
where one exists, a measured WER. The arrow at the left of a row expands it for everything else.
The entry recommended for your hardware is marked.

Rows are grouped by who made the model — a heading, a count, and a gap, in one continuous list.
The recommended model's maker leads, then makers by how many models they have, with anything
whose maker is unknown under *Other*. Sorting by a column drops the grouping, and says so.

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

Fourteen of the fifteen entries run on the portable `sherpa` engine, which **the installed
application includes** — the Windows build script links it, having first staged a GPL-free copy
(see [`licenses.md`](licenses.md) for what that means and why it is not automatic). If you are
running a build made without it — a plain `cargo build`, or `-NoSherpa` — the list says so on each
affected row and disables download rather than offering a model the build cannot load. That state
is a property of the binary, not of your machine or your licence: nothing can be installed to
repair it, only a build with the feature.

Models live in the app's own data directory. To make the CLI agree about what is installed, point
it there with `LW_MODELS_ROOT` ([`models.md`](models.md)).

---

## 4. Choosing where it runs

Settings → **Backend** picks the accelerator. Four coarse choices plus one entry per execution
provider:

| Choice | Meaning |
|---|---|
| **Automatic** | Try every usable accelerator best-first — NPU → CPU → GPU — and fall back. The default. |
| **Any NPU** | Only an NPU, whichever vendor this machine has. |
| **Any GPU** | Only a GPU, whichever vendor this machine has. |
| **CPU** | The CPU only. |
| *a named provider* | Exactly that one — Qualcomm NPU, WebGPU, CUDA, TensorRT, DirectML, CoreML, OpenVINO or Vitis AI. |

The CPU comes ahead of the GPU in **Automatic** on purpose: the only GPU measurement this project
has shows an integrated GPU losing to a strong CPU. A discrete GPU very likely wins — measure it
with the Benchmark tab and then pick **Any GPU** if it does.

Everything except **Automatic** is strict: if the chosen backend cannot be used, the engine says
so instead of quietly running somewhere else. That is deliberate — when you are comparing
backends, a silent fallback would attribute the numbers to the wrong one.

Providers this machine cannot use are listed with the reason. The three facts are kept apart
because they routinely disagree:

- **present** — the provider library is in the runtime directory;
- **registered** — ONNX Runtime accepted it;
- **devices** — it found hardware.

Only the last one means acceleration. A vendor driver can be installed and still enumerate
nothing. From a terminal the same table is `lw diagnose`.

**What is actually running** is read back from the engine rather than from what was requested, so
a run that asked for the NPU and fell back to the CPU reports the CPU — visible in Diagnostics and
in the benchmark report, whose notes also say *why* it fell back.

To add a provider this build does not bundle (CUDA, TensorRT, DirectML, OpenVINO, Vitis AI), drop
its library into the runtime directory; it appears by itself. [`hardware.md`](hardware.md) §5 lists
what each one needs.

---

## 5. Microphone, sounds, and starting with the computer

All three live in Settings.

**Microphone.** A picker over the input devices the machine reports, with **System default** as an
explicit first choice rather than an empty one. *Re-scan* re-reads the list without restarting.

A saved device that is not present now stays selected, under *Saved, but not present now*, and
says that dictation will **fail** until it is reconnected or another input is chosen. That is the
truth about what the application does: it refuses rather than quietly recording from whatever the
system happens to call default, because a silent substitution is how you end up dictating into the
wrong microphone for a week. The saved setting is left alone so reconnecting the device is enough.

A **Test microphone** switch sits beside the picker so a device can be chosen and heard in one
place. It opens the *saved* device — the one the worker will use — and says so when the picker has
changes that have not been saved yet.

**Start at login.** One checkbox, working the same way on Windows, macOS and Linux. It applies
immediately rather than waiting for Save, and it reports **what the operating system says
afterwards** — a managed or locked-down machine can refuse the registration, and a checkbox that
claimed otherwise would be lying. If you remove the entry outside the app (Task Manager,
`launchctl`, your desktop's startup list), the app notices at next launch and follows the OS.

**Cue sounds.** A short sound when recording starts and another when it stops. Four themes:

| Theme | What it is |
|---|---|
| **Chime** | Two soft tones, rising to start and falling to stop. The default. |
| **Blip** | A single short pip. The most discreet. |
| **Click** | A percussive tick with almost no pitch. |
| **Marimba** | A struck-bar tone with a fast decay. |

Every theme's start and stop are mirror images — rising means "listening", falling means "done" —
so that is the only thing to learn, and it survives changing the theme. There is a preview button:
it plays whatever the controls on screen say right now, not the last thing you saved.

The sounds are **synthesized, not shipped**: generated from a formula at your output device's own
sample rate. Nothing to bundle, nothing to license, no resampling, and every cue is under 200 ms
and fades in and out so it never clicks.

Cues are deliberately limited to the two moments you can act on. Finishing and failing are silent:
by then the text has appeared, or it has not, which is feedback enough.

---

## 6. Benchmarking your own machine

The **Benchmark** tab measures the selected model here and now. It reports:

- the **backend that actually executed** — read back from the engine, not from what was requested,
  so a run that asked for the NPU and fell back to the CPU says CPU;
- **notes** explaining the choice, including the reason for any fallback;
- per-clip wall time, RTF and (when the clips have reference transcripts) WER, over the whole
  twelve-clip fixture set — three each in English, Russian, Spanish and Ukrainian;
- a **cold** RTF for the first run and a **warm** mean for the rest.

**WER is scored only on languages the model claims.** An English-only model is judged on the
English clips and the rest are transcribed, timed, and marked *not scored* — a word error rate
against a language a model never advertised measures the question, not the model. (Moonshine tiny
en: 0.092 on English, 0.850 if you score it on all four.)

It takes tens of seconds. The **first NPU run takes minutes**: the encoder's Hexagon context binary
is prepared on-device and cached (~1.2 GB), after which it reloads in about two seconds.

**How this is measured** at the top of the tab spells the method out in the app itself — the clip
set, cold versus warm, how WER is computed, and why unclaimed languages are excluded.

A run is not tied to the tab being visible. Start a benchmark, switch to Models or Settings, come
back: it is still running, with the clock at true elapsed time, and a run that finishes while you
are elsewhere is waiting when you return. A model download behaves the same way, keeping its
progress bar, its file counter and its Cancel button across a tab switch.

Benchmarks run on the dictation worker, so dictation pauses while one is in flight. That is
deliberate: two engines would mean two QNN sessions competing for one Hexagon context, and the
failure would look like a benchmark result.

### Comparing every backend at once

**Compare all backends** measures every accelerator this machine can use, one after another, and
puts them side by side — instead of running each one and writing the numbers down yourself.

All runs use **one clip set, loaded once**. That is the point: numbers measured on different audio
would not be comparable. The result marks two winners, and they are often not the same row:

- **fastest** — lowest *warm* RTF, because the cold run carries one-time setup that says nothing
  about steady-state dictation;
- **most accurate** — lowest word error rate.

On the X2, for example, the NPU is fastest while the CPU is the most accurate (its int8 encoder
happens to win on these clips). Accelerators that could not run are listed with the reason rather
than hidden.

It takes **minutes**: each backend loads its own engine, and a first NPU run also prepares a
context binary. Progress shows which one is being measured.

The equivalent from a terminal is `lw bench --quick`, which shares the same measurement code — see
[`benchmarks.md`](benchmarks.md).

---

## 7. Diagnostics and logs

The **Diagnostics** tab reports the ONNX Runtime it found, whether the QNN provider DLL is present,
whether an NPU device actually enumerated, and the app/OS versions. "Present" and "usable" are
reported separately: a driver package can be installed while the execution provider fails to load,
and only the second one means acceleration.

It also carries the full accelerator table for this machine — the provider library, whether ONNX
Runtime registered it, how many devices it then found, and the verdict — which is four separate
facts rather than one. *Refresh* forces a fresh probe. Settings keeps only the choice itself and a
pointer here, so there is one place that answers "what can this machine actually do?".

Logs are written under the app data directory in `logs/localwisper.log.<date>`, at `info` by
default. Raise it with `LW_LOG=debug`.

**Nothing private is logged.** No audio, transcript text, clipboard contents or keys are ever
written — the logs carry device, backend and version facts only.

---

## 8. Where things live

| What | Windows | macOS | Linux |
|---|---|---|---|
| Settings | `%APPDATA%\ai.localwisper.app\settings.json` | `~/Library/Application Support/ai.localwisper.app/settings.json` | `~/.local/share/ai.localwisper.app/settings.json` |
| Models | `…\ai.localwisper.app\models` | `…/ai.localwisper.app/models` | `…/ai.localwisper.app/models` |
| NPU context cache | `…\ai.localwisper.app\cache` | — | — |
| Logs | `…\ai.localwisper.app\logs` | `…/ai.localwisper.app/logs` | `…/ai.localwisper.app/logs` |

Deleting `settings.json` resets to defaults; the app recreates it on the next save.
