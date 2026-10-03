# OwlWhisp 0.1.1 preview

OwlWhisp now releases its speech model after an idle timeout, reducing background memory after
dictation. **Settings → Background memory** lets you choose 1, 5, 15 or 30 minutes, or keep the
model loaded for faster responses. The default is 5 minutes; existing settings receive it
automatically. The next dictation reloads the model, and prepared TensorRT caches remain on disk.

- Explicit GPU backends and bundled GPU selection use isolated inference workers. Unloading
  ends the worker, releasing its model, provider libraries and driver allocations.
- GPU diagnostics run in short-lived probe processes. CPU/exact-provider startup avoids probing
  unrelated GPU providers into the application's process-global ONNX Runtime environment.
- The Dictate tab reports when the model is unloaded. Worker backend notes remain visible.
- The README includes a measured comparison with OpenWhispr 1.10.1 and Superwhisper 1.6.5. In
  one 60-second Windows x64 snapshot, the updated full OwlWhisp client used 43 MiB private
  resident RAM / 88 MiB working set with its model unloaded. Model states differed between
  clients; this is a current-configuration snapshot, not a matched-model benchmark.
- Memory documentation records model-loading costs, CPU/TensorRT release-and-reload checks,
  measurement conditions and samples. A Windows sampler and an inference-lifecycle example
  support repeatable measurements.

Reloading measured approximately 2.6 seconds on CPU and 4.3 seconds with a prepared TensorRT
cache on the test machine. First TensorRT preparation can take much longer. Keep the model
loaded or choose a longer timeout when response time matters more than background RAM.

Native packages cover Windows x64/ARM64, macOS Apple Silicon/Intel and Linux x64/ARM64. Windows
includes an installer and portable ZIP; macOS includes an `.app` ZIP; Linux includes a portable
tarball. Each platform has SHA-256 checksums. Models are downloaded separately in the application.

Windows is the primary desktop platform. macOS/Linux remain previews with incomplete global
hotkeys, automatic text injection and foreground-app detection. Linux ARM64 supports Parakeet
on CPU without the optional sherpa engine. Windows installers and macOS bundles are unsigned;
Linux packages require compatible GTK 3/ALSA system libraries.

Every archive is verified on its native runner before publication. The release requires all six
platforms and all 14 assets; publication is followed by unauthenticated download and SHA-256
verification of every asset.
