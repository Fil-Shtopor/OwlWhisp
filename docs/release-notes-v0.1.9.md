# OwlWhisp 0.1.9 preview

Adds user-supplied local models and optional online speech recognition in **Models → My models**.
On-device recognition remains the default.

- **Add a local ONNX model:** select a compatible sherpa-onnx export folder, name it and run
  **Check and add**. The application detects the family, with a manual choice for ambiguous
  layouts. A separate process loads the model and performs a short inference before registration.
  Supported families are Whisper, Moonshine, NeMo transducer, SenseVoice, Paraformer, Qwen3-ASR and
  Omnilingual. Custom ONNX models currently run on CPU and require a build with the sherpa engine;
  the Linux ARM64 preview still ships without that engine.
- **Keep ownership of model files:** models run from their selected folder. **Choose folder**
  reconnects a moved model while preserving its registration. **Remove from list** keeps the
  original files. Custom local models are available to single-model and comparative benchmarks.
- **Connect an online transcription API:** enter your own API base URL, model ID and optional
  API key for a service implementing the OpenAI-compatible audio transcription API. Keys are kept
  in the operating system credential store, outside settings and model registrations. Saving a
  connection does not contact the service. **Test connection** uploads one second of generated
  silence and may consume API quota.
- **Make online use explicit:** **Use online** selects a service, and Dictate shows the address
  receiving recordings. Online models are excluded from local benchmarks and automatic fallback.
  Requests use HTTPS except for loopback servers; redirects are refused. Changing an endpoint or
  model creates a separate connection without reusing the old connection's key. API access and
  billing depend on the provider; signing in with an existing chat subscription is not implemented.
- **Report the selected source accurately:** Settings shows CPU recognition for imported ONNX
  models and the destination for online connections. Scratchpad and documentation privacy wording
  now reflect the selected recognition source.

Validation before release: 284 core/application unit tests passed, along with strict Clippy,
build checks with and without the sherpa feature, and an additional end-to-end test importing,
selecting and transcribing real audio with a Whisper ONNX export. Online requests, authorization,
multipart WAV uploads and failure handling were checked against a local test API; no paid provider
account was used. Release runners additionally verify all six native packages, complete asset sets,
public downloads and SHA-256 checksums before the workflow is marked successful.

This remains an unsigned preview release. macOS/Linux desktop integration and RTX Spark support
retain their existing preview limitations.
