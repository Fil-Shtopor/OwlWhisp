# OwlWhisp 0.1.8 preview

Reduces the wait for text after a first dictation or a dictation following idle model unloading.
In 0.1.7, model initialization started after recording stopped. It now starts when dictation
is activated and runs in a background thread while the microphone records.

- **Load during recording:** the microphone starts first, then the configured speech engine
  initializes alongside capture. When recording stops, transcription waits only for any
  unfinished initialization. If loading completed during speech, recognition can begin
  immediately. This applies to both first use and loading after the idle timeout.
- **Reuse the loaded engine:** an engine already in memory is reused, and a pending load is
  shared rather than started twice. Automatic unloading after inactivity keeps its existing
  behavior; microphone tests and status requests do not trigger loading.
- **Recover from initialization failures:** failed loads report their error for the current
  utterance and retry on the next activation. Changing settings discards obsolete load results
  and releases their engine. An initialization thread that exits unexpectedly returns an error
  instead of leaving dictation waiting for a result forever.

Validation: full Rust workspace tests, clippy and Python packaging tests passed on Windows x64.
Automated coverage checks background initialization, reuse, retry and discarded loads. Release
runners also run the dictation lifecycle and hardware tests on all six native platforms before
publishing, then verify packaged runtimes, public downloads and SHA-256 checksums.

This remains an unsigned preview release. Qualcomm NPU model preparation from 0.1.7 is included;
RTX Spark support remains experimental.
