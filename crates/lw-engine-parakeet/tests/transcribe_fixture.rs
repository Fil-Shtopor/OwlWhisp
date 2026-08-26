//! Integration test: transcribe a known WAV fixture end-to-end on the CPU backend.
//!
//! Requires the model files and the ONNX Runtime; both are environment-provided, so the test is
//! `#[ignore]` by default and runs only when `LW_MODEL_DIR` and `LW_RUNTIME_DIR` are set:
//!
//!   LW_RUNTIME_DIR=runtime/win-arm64 LW_MODEL_DIR=<models>/parakeet-tdt-0.6b-v3 \
//!     cargo test -p lw-engine-parakeet --test transcribe_fixture -- --ignored
//!
//! It asserts a low word error rate against the fixture transcript, proving the whole Rust
//! pipeline (mel -> encoder -> TDT decode -> detokenize) works, not just that it compiles.

use std::path::PathBuf;

use lw_core::audio::{AudioBuffer, downmix_to_mono};
use lw_core::engine::{EngineInitContext, SpeechEngine};
use lw_engine_parakeet::{BackendKind, ParakeetConfig, ParakeetEngine};
use lw_ort::OrtRuntime;

fn wer(reference: &str, hypothesis: &str) -> f32 {
    let norm = |s: &str| -> Vec<String> {
        s.to_lowercase()
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c.is_whitespace() {
                    c
                } else {
                    ' '
                }
            })
            .collect::<String>()
            .split_whitespace()
            .map(str::to_string)
            .collect()
    };
    let r = norm(reference);
    let h = norm(hypothesis);
    if r.is_empty() {
        return if h.is_empty() { 0.0 } else { 1.0 };
    }
    let mut prev: Vec<usize> = (0..=h.len()).collect();
    let mut cur = vec![0usize; h.len() + 1];
    for (i, rw) in r.iter().enumerate() {
        cur[0] = i + 1;
        for (j, hw) in h.iter().enumerate() {
            let cost = if rw == hw { 0 } else { 1 };
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[h.len()] as f32 / r.len() as f32
}

fn load_wav(path: &std::path::Path) -> AudioBuffer {
    let mut reader = hound::WavReader::open(path).expect("open wav");
    let spec = reader.spec();
    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().map(|s| s.unwrap()).collect(),
        hound::SampleFormat::Int => {
            let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader.samples::<i32>().map(|s| s.unwrap() as f32 / max).collect()
        }
    };
    AudioBuffer::new(downmix_to_mono(&interleaved, spec.channels), spec.sample_rate)
}

#[test]
#[ignore = "requires LW_MODEL_DIR + LW_RUNTIME_DIR and the model files"]
fn transcribes_english_fixture_cpu() {
    let runtime_dir = std::env::var("LW_RUNTIME_DIR").expect("set LW_RUNTIME_DIR");
    let model_dir = PathBuf::from(std::env::var("LW_MODEL_DIR").expect("set LW_MODEL_DIR"));
    let runtime = OrtRuntime::init(std::path::Path::new(&runtime_dir)).expect("init ort");

    let cache = std::env::temp_dir().join("lw-test-cache");
    let config = ParakeetConfig::from_ctx(
        &EngineInitContext {
            model_dir: model_dir.clone(),
            cache_dir: cache.clone(),
            cpu_threads: 0,
        },
        BackendKind::ForceCpu,
    );
    let mut engine = ParakeetEngine::new(runtime, config);
    engine
        .initialize(&EngineInitContext {
            model_dir,
            cache_dir: cache,
            cpu_threads: 0,
        })
        .expect("initialize engine");

    // The fixture lives at the repo root under tests/fixtures/audio.
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    let wav = repo_root.join("tests/fixtures/audio/fleurs_en_1.wav");
    let audio = load_wav(&wav);
    let transcript = engine.transcribe(&audio).expect("transcribe");
    let reference = "There are many beaches, due to Auckland's straddling of two harbours. \
        The most popular ones are in three areas.";
    let e = wer(reference, &transcript.text);
    eprintln!("hyp: {}\nWER: {e:.2}", transcript.text);
    assert!(!transcript.text.is_empty(), "empty transcript");
    assert!(e < 0.35, "WER too high: {e:.2} (hyp: {})", transcript.text);
}

#[test]
fn wer_helper_is_sane() {
    assert_eq!(wer("a b c", "a b c"), 0.0);
    assert!((wer("a b c", "a x c") - 1.0 / 3.0).abs() < 1e-6);
}
