//! The Parakeet TDT speech engine: composes the mel front end, an encoder backend (QNN/CPU), and
//! the TDT decoder into a [`lw_core::engine::SpeechEngine`].

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use lw_core::audio::AudioBuffer;
use lw_core::engine::{
    Acceleration, DeviceInfo, EngineInitContext, HealthReport, Language, Provider, SpeechEngine, Transcript,
};
use lw_ort::{CpuSessionConfig, OrtRuntime, QnnSessionConfig, build_cpu_session};

use lw_core::capabilities::{ALL_ACCELERATORS, Accelerator, AcceleratorKind};

use crate::encoder::{CpuEncoder, ENC_DIM, EncoderBackend, SUBSAMPLING, StaticWindowEncoder};
use crate::mel::{MelFrontend, N_MELS, OnnxMel};
use crate::tdt::TdtDecoder;
use crate::vocab::Vocab;
use crate::{Error, Result, merge};

/// Languages supported by Parakeet TDT 0.6B v3 (25 European languages).
pub const LANGUAGES: &[Language] = &[
    Language("en"),
    Language("es"),
    Language("fr"),
    Language("de"),
    Language("it"),
    Language("pt"),
    Language("ru"),
    Language("uk"),
    Language("pl"),
    Language("nl"),
    Language("cs"),
    Language("sk"),
    Language("sl"),
    Language("hr"),
    Language("bg"),
    Language("ro"),
    Language("hu"),
    Language("el"),
    Language("da"),
    Language("sv"),
    Language("fi"),
    Language("et"),
    Language("lv"),
    Language("lt"),
    Language("mt"),
];

/// Which encoder backend the engine should try.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendKind {
    /// Try every usable accelerator best-first, falling back to the CPU.
    Auto,
    /// An NPU only -- whichever vendor's this machine has. Fails rather than falling back.
    ForceNpu,
    /// A GPU only -- whichever vendor's this machine has. Fails rather than falling back.
    ForceGpu,
    /// CPU only.
    ForceCpu,
    /// One exact execution provider. Fails rather than falling back, so that a measurement
    /// attributed to it cannot silently have come from somewhere else.
    Exact(lw_core::capabilities::Accelerator),
}

impl From<lw_core::engine::BackendPreference> for BackendKind {
    /// The one place a user preference becomes an engine plan, so the app, the CLI and any future
    /// front end cannot drift apart on what "Any GPU" means.
    fn from(p: lw_core::engine::BackendPreference) -> Self {
        use lw_core::engine::BackendPreference as P;
        match p {
            P::Automatic => BackendKind::Auto,
            P::ForceNpu => BackendKind::ForceNpu,
            P::ForceGpu => BackendKind::ForceGpu,
            P::ForceCpu => BackendKind::ForceCpu,
            other => match other.accelerator() {
                Some(a) => BackendKind::Exact(a),
                None => BackendKind::Auto,
            },
        }
    }
}

/// Engine configuration.
#[derive(Clone, Debug)]
pub struct ParakeetConfig {
    /// Directory containing the model files.
    pub model_dir: PathBuf,
    /// Cache directory for the QNN context binary.
    pub cache_dir: PathBuf,
    /// CPU thread budget.
    pub cpu_threads: usize,
    /// NPU static window length in mel frames (e.g. 2000 for 20 s).
    pub npu_window_frames: usize,
    /// Overlap (mel frames) when chunking long audio.
    pub chunk_overlap_frames: usize,
    /// Which backend to prefer.
    pub backend: BackendKind,
    /// HTP `htp_arch` override (e.g. 81).
    pub htp_arch: Option<u32>,
    /// HTP `soc_model` override (e.g. 88).
    pub soc_model: Option<u32>,
}

impl ParakeetConfig {
    /// Take the NPU target from detected hardware rather than assuming one SoC.
    ///
    /// The Hexagon generation differs per device (Snapdragon X Elite / X Plus are **V73**,
    /// X2 Elite is **V81**), and a QNN context binary is only valid for the generation it was
    /// prepared for — hard-coding one would silently exclude every other Snapdragon. Leaving both
    /// values `None` is also fine: the QNN EP then picks the target itself.
    pub fn with_capabilities(mut self, caps: &lw_core::capabilities::Capabilities) -> Self {
        if caps.npu.present {
            self.htp_arch = caps.npu.htp_arch.map(|a| a.num());
            self.soc_model = caps.npu.soc_model;
        }
        self
    }

    /// Config from an [`EngineInitContext`] and a backend kind.
    pub fn from_ctx(ctx: &EngineInitContext, backend: BackendKind) -> Self {
        Self {
            model_dir: ctx.model_dir.clone(),
            cache_dir: ctx.cache_dir.clone(),
            cpu_threads: ctx.cpu_threads,
            npu_window_frames: 2000,
            chunk_overlap_frames: 100,
            backend,
            htp_arch: None,
            soc_model: None,
        }
    }
}

/// The Parakeet engine.
pub struct ParakeetEngine {
    runtime: Arc<OrtRuntime>,
    config: ParakeetConfig,
    mel: Option<Box<dyn MelFrontend>>,
    encoder: Option<Box<dyn EncoderBackend>>,
    decoder: Option<TdtDecoder>,
    vocab: Option<Vocab>,
    provider: Provider,
    acceleration: Acceleration,
    device: DeviceInfo,
    notes: Vec<String>,
    /// The accelerator the encoder actually ended up on.
    selected: Option<Accelerator>,
}

impl ParakeetEngine {
    /// Create an engine bound to an initialized [`OrtRuntime`].
    pub fn new(runtime: Arc<OrtRuntime>, config: ParakeetConfig) -> Self {
        Self {
            runtime,
            config,
            mel: None,
            encoder: None,
            decoder: None,
            vocab: None,
            provider: Provider::OnnxCpu,
            acceleration: Acceleration::Cpu,
            device: DeviceInfo::new("uninitialized"),
            notes: Vec::new(),
            selected: None,
        }
    }

    /// Selection notes recorded during initialization (for diagnostics).
    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    fn model_file(&self, names: &[&str]) -> Result<PathBuf> {
        for n in names {
            let p = self.config.model_dir.join(n);
            if p.exists() {
                return Ok(p);
            }
        }
        Err(Error::MissingFile(format!(
            "{} in {}",
            names.join("/"),
            self.config.model_dir.display()
        )))
    }

    fn build_cpu_encoder(&self) -> Result<Box<dyn EncoderBackend>> {
        let path = self.model_file(&[
            "encoder-model.int8.onnx",
            "encoder.int8.onnx",
            "encoder-model.onnx",
        ])?;
        Ok(Box::new(CpuEncoder::load(
            &self.runtime,
            &path,
            self.config.cpu_threads,
        )?))
    }

    /// The accelerator the encoder ended up on, once [`SpeechEngine::initialize`] has run.
    pub fn selected_accelerator(&self) -> Option<Accelerator> {
        self.selected
    }

    /// The accelerators to try, in order, for the configured preference.
    ///
    /// Only accelerators this machine can actually use are included -- the probe registers each
    /// provider and asks it for devices, so an installed driver with no working provider does not
    /// get into the plan and cannot produce a confusing failure later. `Auto` always ends at the
    /// CPU; a forced preference never does, because falling back would misattribute the result.
    fn acceleration_plan(&self) -> Vec<Accelerator> {
        let usable = self.runtime.usable_accelerators();
        let keep = |a: &Accelerator| usable.contains(a);
        match self.config.backend {
            BackendKind::Auto => ALL_ACCELERATORS.iter().copied().filter(keep).collect(),
            BackendKind::ForceCpu => vec![Accelerator::Cpu],
            BackendKind::ForceNpu => ALL_ACCELERATORS
                .iter()
                .copied()
                .filter(|a| a.kind() == AcceleratorKind::Npu)
                .filter(keep)
                .collect(),
            BackendKind::ForceGpu => ALL_ACCELERATORS
                .iter()
                .copied()
                .filter(|a| a.kind() == AcceleratorKind::Gpu)
                .filter(keep)
                .collect(),
            BackendKind::Exact(a) => vec![a],
        }
    }

    fn try_build_npu_encoder(&self) -> Result<Box<dyn EncoderBackend>> {
        // A static-shape encoder ONNX for the configured window, or a prebuilt EPContext wrapper.
        let t = self.config.npu_window_frames;
        let path = self.model_file(&[
            &format!("encoder-static-t{t}.onnx"),
            "encoder-static.onnx",
            "encoder-model.onnx",
        ])?;
        let mut qnn = QnnSessionConfig::new(
            &self.config.cache_dir,
            format!("parakeet-enc-t{t}-arch{}", self.config.htp_arch.unwrap_or(0)),
        );
        qnn.htp_arch = self.config.htp_arch;
        qnn.soc_model = self.config.soc_model;
        let device = format!(
            "Snapdragon Hexagon HTP{}",
            self.config
                .htp_arch
                .map(|a| format!(" (V{a})"))
                .unwrap_or_default()
        );
        Ok(Box::new(StaticWindowEncoder::qnn(
            &self.runtime,
            &path,
            t,
            qnn,
            device,
        )?))
    }

    /// Build the encoder on `accel` (a GPU provider), using the same static-shape graph the NPU
    /// path uses.
    ///
    /// GPU providers consume the ordinary graph. TensorRT additionally compiles a cached engine
    /// for a bounded profile, so different dictation lengths do not trigger repeated builds.
    fn try_build_gpu_encoder(
        &self,
        accel: lw_core::capabilities::Accelerator,
    ) -> Result<Box<dyn EncoderBackend>> {
        // Prefer the dynamic graph. The engine already bounds long audio to `t` frames below,
        // which gives TensorRT an explicit range covering every chunk, including a short tail.
        let t = self.config.npu_window_frames;
        if let Ok(path) = self.model_file(&["encoder-model.onnx", "encoder.onnx"]) {
            let tensorrt = if accel == Accelerator::TensorRt {
                let max = t.max(1);
                lw_ort::TensorRtSessionConfig {
                    fp16: true,
                    profile: Some(lw_ort::TensorRtShapeProfile {
                        min_shapes: "audio_signal:1x128x1,length:1".into(),
                        opt_shapes: format!("audio_signal:1x128x{},length:1", 600.min(max)),
                        max_shapes: format!("audio_signal:1x128x{max},length:1"),
                    }),
                }
            } else {
                lw_ort::TensorRtSessionConfig::default()
            };
            return Ok(Box::new(CpuEncoder::on_accelerator_with_tensorrt_config(
                &self.runtime,
                accel,
                &path,
                self.config.cpu_threads,
                accel.label(),
                &tensorrt,
            )?));
        }
        if let Ok(path) = self.model_file(&[&format!("encoder-static-t{t}.onnx"), "encoder-static.onnx"]) {
            return Ok(Box::new(StaticWindowEncoder::on_accelerator(
                &self.runtime,
                accel,
                &path,
                t,
                self.config.cpu_threads,
                accel.label(),
            )?));
        }
        // Last resort: the quantized graph. Support for int8 operators varies a lot between GPU
        // providers, so this may legitimately fail -- and then Auto falls back to the CPU, which
        // runs this exact file well.
        let path = self.model_file(&["encoder-model.int8.onnx", "encoder.int8.onnx"])?;
        Ok(Box::new(CpuEncoder::on_accelerator(
            &self.runtime,
            accel,
            &path,
            self.config.cpu_threads,
            accel.label(),
        )?))
    }

    /// Split mel features into windows and run encoder+decoder per window, merging text.
    fn transcribe_features(&mut self, feats: &[f32], n_frames: usize) -> Result<Transcript> {
        let vocab = self
            .vocab
            .as_ref()
            .ok_or_else(|| Error::Other("vocab not loaded".into()))?
            .clone();
        // Dynamic CPU/GPU graphs can accept a whole clip, but multi-minute attention grows
        // prohibitively large. Use the same bounded windows as the NPU for long dictation.
        let window = self.config.npu_window_frames.max(1);

        let mut text = String::new();
        if n_frames <= window {
            let (enc, t_out) = self.run_encoder(feats, n_frames)?;
            let emissions = self.run_decoder(&enc, t_out)?;
            let ids: Vec<usize> = emissions.iter().map(|e| e.token).collect();
            text = vocab.detokenize(&ids);
        } else {
            let stride = window.saturating_sub(self.config.chunk_overlap_frames).max(1);
            let mut start = 0usize;
            while start < n_frames {
                let end = (start + window).min(n_frames);
                let chunk_frames = end - start;
                let mut chunk = vec![0.0f32; N_MELS * chunk_frames];
                for m in 0..N_MELS {
                    for f in 0..chunk_frames {
                        chunk[m * chunk_frames + f] = feats[m * n_frames + start + f];
                    }
                }
                let (enc, t_out) = self.run_encoder(&chunk, chunk_frames)?;
                let emissions = self.run_decoder(&enc, t_out)?;
                let ids: Vec<usize> = emissions.iter().map(|e| e.token).collect();
                let chunk_text = vocab.detokenize(&ids);
                text = merge::merge(&text, &chunk_text);
                if end == n_frames {
                    break;
                }
                start += stride;
            }
        }
        Ok(Transcript::from_text(text))
    }

    fn run_encoder(&mut self, feats: &[f32], n_frames: usize) -> Result<(Vec<f32>, usize)> {
        let enc = self
            .encoder
            .as_mut()
            .ok_or_else(|| Error::Other("encoder not loaded".into()))?;
        enc.run(feats, n_frames)
    }

    fn run_decoder(&mut self, enc: &[f32], t_out: usize) -> Result<Vec<crate::tdt::Emission>> {
        let dec = self
            .decoder
            .as_mut()
            .ok_or_else(|| Error::Other("decoder not loaded".into()))?;
        dec.decode(enc, ENC_DIM, t_out)
    }
}

impl SpeechEngine for ParakeetEngine {
    fn backend_name(&self) -> &str {
        "parakeet-tdt-0.6b-v3"
    }
    fn provider(&self) -> Provider {
        self.provider
    }
    fn device(&self) -> DeviceInfo {
        self.device.clone()
    }
    fn acceleration(&self) -> Acceleration {
        self.acceleration
    }
    fn supported_languages(&self) -> &[Language] {
        LANGUAGES
    }
    fn supports_streaming(&self) -> bool {
        false
    }

    fn initialize(&mut self, ctx: &EngineInitContext) -> lw_core::Result<()> {
        // ctx overrides paths if provided.
        if !ctx.model_dir.as_os_str().is_empty() {
            self.config.model_dir = ctx.model_dir.clone();
        }
        if !ctx.cache_dir.as_os_str().is_empty() {
            self.config.cache_dir = ctx.cache_dir.clone();
        }
        if ctx.cpu_threads > 0 {
            self.config.cpu_threads = ctx.cpu_threads;
        }

        // Vocab + mel + decoder (all required, all CPU).
        let vocab_path = self.model_file(&["vocab.txt", "tokens.txt"])?;
        self.vocab = Some(Vocab::load(&vocab_path)?);

        let mel_path = self.model_file(&["nemo128.onnx"])?;
        self.mel = Some(Box::new(OnnxMel::load(&self.runtime, &mel_path)?));

        let dec_path = self.model_file(&["decoder_joint-model.int8.onnx", "decoder_joint-model.onnx"])?;
        let dec_session = build_cpu_session(&self.runtime, &dec_path, CpuSessionConfig::default())
            .map_err(|e| Error::Ort(e.to_string()))?;
        self.decoder = Some(TdtDecoder::new(dec_session, self.vocab.as_ref().unwrap()));

        // Encoder backend selection: walk the plan best-first and take the first that builds.
        let plan = self.acceleration_plan();
        let strict = !matches!(self.config.backend, BackendKind::Auto);
        for accel in plan {
            let built = match accel {
                Accelerator::Cpu => self
                    .build_cpu_encoder()
                    .map(|e| (e, Provider::OnnxCpu, Acceleration::Cpu)),
                Accelerator::QnnNpu => self
                    .try_build_npu_encoder()
                    .map(|e| (e, Provider::QnnHtp, Acceleration::Npu)),
                other => self.try_build_gpu_encoder(other).map(|e| {
                    let provider = match other {
                        Accelerator::CoreMl => Provider::CoreMl,
                        Accelerator::DirectMl => Provider::DirectMl,
                        Accelerator::Cuda => Provider::Cuda,
                        Accelerator::TensorRt => Provider::TensorRt,
                        Accelerator::OpenVino => Provider::OpenVino,
                        Accelerator::VitisAi => Provider::VitisAi,
                        _ => Provider::Gpu,
                    };
                    let acc = if other == Accelerator::CoreMl {
                        Acceleration::Ane
                    } else if other.kind() == lw_core::capabilities::AcceleratorKind::Npu {
                        Acceleration::Npu
                    } else {
                        Acceleration::Gpu
                    };
                    (e, provider, acc)
                }),
            };
            match built {
                Ok((enc, provider, acceleration)) => {
                    let label = enc.label();
                    self.notes.push(format!("encoder: {}", accel.label()));
                    self.encoder = Some(enc);
                    self.provider = provider;
                    self.acceleration = acceleration;
                    self.device = DeviceInfo::with_detail(acceleration.to_string(), label);
                    self.selected = Some(accel);
                    break;
                }
                Err(e) => {
                    // A forced backend must fail loudly: silently answering with a different one
                    // would make every number reported against it a lie.
                    if strict {
                        return Err(Error::Other(format!(
                            "{} was requested but cannot be used: {e}",
                            accel.label()
                        ))
                        .into());
                    }
                    self.notes.push(format!("{} unavailable ({e})", accel.label()));
                }
            }
        }
        if self.encoder.is_none() {
            let usable = self.runtime.usable_accelerators();
            return Err(Error::Ort(format!(
                "no encoder backend matched {:?}; usable here: {}",
                self.config.backend,
                if usable.is_empty() {
                    "none".to_string()
                } else {
                    usable.iter().map(|a| a.label()).collect::<Vec<_>>().join(", ")
                }
            ))
            .into());
        }
        Ok(())
    }

    fn accelerator(&self) -> Option<Accelerator> {
        self.selected
    }

    fn notes(&self) -> &[String] {
        &self.notes
    }

    fn health_check(&mut self) -> HealthReport {
        // Probe with 0.5 s of near-silence through the whole pipeline.
        let probe = vec![0.0f32; 8000];
        let start = Instant::now();
        let result = (|| -> Result<()> {
            let mel = self
                .mel
                .as_mut()
                .ok_or_else(|| Error::Other("mel not loaded".into()))?;
            let (feats, n) = mel.extract(&probe)?;
            let enc = self
                .encoder
                .as_mut()
                .ok_or_else(|| Error::Other("encoder not loaded".into()))?;
            let _ = enc.run(&feats, n)?;
            Ok(())
        })();
        let ms = start.elapsed().as_secs_f32() * 1000.0;
        match result {
            Ok(()) => HealthReport {
                ok: true,
                provider: self.provider,
                probe_latency_ms: Some(ms),
                message: format!("{} on {}", self.provider, self.device.name),
            },
            Err(e) => HealthReport {
                ok: false,
                provider: self.provider,
                probe_latency_ms: None,
                message: e.to_string(),
            },
        }
    }

    fn transcribe(&mut self, audio: &AudioBuffer) -> lw_core::Result<Transcript> {
        let canonical = audio.to_target()?;
        if canonical.is_empty() {
            return Ok(Transcript::default());
        }
        let (feats, n_frames) = {
            let mel = self
                .mel
                .as_mut()
                .ok_or_else(|| lw_core::Error::Engine("mel not loaded".into()))?;
            mel.extract(&canonical.samples).map_err(lw_core::Error::from)?
        };
        if n_frames == 0 {
            return Ok(Transcript::default());
        }
        let t = self
            .transcribe_features(&feats, n_frames)
            .map_err(lw_core::Error::from)?;
        Ok(t)
    }

    fn shutdown(&mut self) {
        self.encoder = None;
        self.decoder = None;
        self.mel = None;
    }
}

/// The mel subsampling factor, re-exported for callers computing frame budgets.
pub const ENCODER_SUBSAMPLING: usize = SUBSAMPLING;

/// Resolve the standard model file set present in a directory (for diagnostics/model checks).
pub fn model_files_present(dir: &Path) -> Vec<String> {
    let names = [
        "vocab.txt",
        "tokens.txt",
        "nemo128.onnx",
        "decoder_joint-model.int8.onnx",
        "encoder-model.int8.onnx",
        "encoder-model.onnx",
    ];
    names
        .iter()
        .filter(|n| dir.join(n).exists())
        .map(|s| s.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lw_core::capabilities::AcceleratorKind;
    use lw_core::engine::BackendPreference as P;

    #[test]
    fn every_preference_maps_to_a_plan() {
        assert_eq!(BackendKind::from(P::Automatic), BackendKind::Auto);
        assert_eq!(BackendKind::from(P::ForceCpu), BackendKind::ForceCpu);
        assert_eq!(BackendKind::from(P::ForceNpu), BackendKind::ForceNpu);
        assert_eq!(BackendKind::from(P::ForceGpu), BackendKind::ForceGpu);
        for a in ALL_ACCELERATORS {
            let kind = BackendKind::from(P::for_accelerator(a));
            if a == Accelerator::Cpu {
                // The CPU has two spellings that mean the same plan; both are CPU-only.
                assert_eq!(kind, BackendKind::ForceCpu);
            } else {
                assert_eq!(kind, BackendKind::Exact(a), "{a:?}");
            }
        }
    }

    #[test]
    fn only_auto_is_allowed_to_fall_back() {
        // `strict` in `initialize` is derived from this, and it is what stops a forced run
        // quietly reporting a different backend's numbers.
        for p in P::all() {
            let strict = !matches!(BackendKind::from(p), BackendKind::Auto);
            assert_eq!(strict, p.is_strict(), "{p:?}");
        }
    }

    #[test]
    fn a_gpu_preference_never_maps_to_an_npu_plan() {
        for a in ALL_ACCELERATORS
            .iter()
            .filter(|a| a.kind() == AcceleratorKind::Gpu)
        {
            match BackendKind::from(P::for_accelerator(*a)) {
                BackendKind::Exact(got) => assert_eq!(got.kind(), AcceleratorKind::Gpu),
                other => panic!("{a:?} mapped to {other:?}"),
            }
        }
    }
}
