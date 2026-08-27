//! Integration test: transcribe the repo's WAV fixtures end-to-end through sherpa-onnx.
//!
//! These need real model files, which are environment-provided, so they are `#[ignore]` by default
//! — exactly like `crates/lw-engine-parakeet/tests/transcribe_fixture.rs`:
//!
//! ```text
//! # English (any family: Whisper, Moonshine, a transducer, …)
//! LW_SHERPA_MODEL_DIR=<models>/sherpa-onnx-whisper-tiny.en \
//!   cargo test -p lw-engine-sherpa --features sherpa --test transcribe_fixture -- --ignored --nocapture
//!
//! # Russian, with a multilingual model
//! LW_SHERPA_MULTILINGUAL_MODEL_DIR=<models>/sherpa-onnx-whisper-small \
//!   cargo test -p lw-engine-sherpa --features sherpa --test transcribe_fixture -- --ignored --nocapture
//! ```
//!
//! Optional knobs: `LW_SHERPA_THREADS`, `LW_SHERPA_FP32=1` (skip the `*.int8.onnx` variants).
//!
//! The assertion is a word error rate against the fixture transcript in
//! `tests/fixtures/audio/fixtures.json`, so a pass means the whole path really worked — model
//! detection, feature extraction, decoding and detokenization — not merely that it linked.

mod util;

#[cfg(feature = "sherpa")]
mod sherpa_tests {
    use std::time::Instant;

    use lw_core::engine::SpeechEngine;
    use lw_engine_sherpa::{SherpaConfig, SherpaEngine};

    use super::util;

    /// Reference transcripts, copied from `tests/fixtures/audio/fixtures.json`.
    const EN_1: &str = "There are many beaches, due to Auckland's straddling of two harbours. \
                        The most popular ones are in three areas.";
    const RU_1: &str = "Основной религией в Молдавии является православное христианство.";

    fn config(dir: std::path::PathBuf) -> SherpaConfig {
        let mut cfg = SherpaConfig::new(dir);
        if let Ok(n) = std::env::var("LW_SHERPA_THREADS")
            && let Ok(n) = n.parse()
        {
            cfg.num_threads = n;
        }
        cfg.prefer_quantized = std::env::var("LW_SHERPA_FP32").is_err();
        cfg.whisper_token_timestamps = std::env::var("LW_SHERPA_WHISPER_TIMESTAMPS").is_ok();
        cfg
    }

    #[test]
    #[ignore = "requires LW_SHERPA_MODEL_DIR and the model files"]
    fn transcribes_the_english_fixture() {
        let Some(dir) = util::env_dir("LW_SHERPA_MODEL_DIR") else {
            panic!("set LW_SHERPA_MODEL_DIR to an extracted sherpa-onnx model directory");
        };
        let mut engine = SherpaEngine::open(config(dir)).expect("load model");
        eprintln!(
            "model: {} on {} ({:?})",
            engine.backend_name(),
            engine.device().name,
            engine.device().detail
        );

        let audio = util::load_wav(&util::fixture("fleurs_en_1.wav"));
        let secs = audio.duration_secs();

        // A first pass warms the ORT arenas; report the steady-state number as well as the cold one.
        let cold = Instant::now();
        let first = engine.transcribe(&audio).expect("transcribe");
        util::report("cold", secs, cold.elapsed(), &first.text);

        let warm = Instant::now();
        let transcript = engine.transcribe(&audio).expect("transcribe");
        util::report("warm", secs, warm.elapsed(), &transcript.text);

        let e = util::wer(EN_1, &transcript.text);
        eprintln!("WER: {e:.3}   tokens with timing: {}", transcript.tokens.len());
        assert!(!transcript.text.is_empty(), "empty transcript");
        assert!(e < 0.35, "WER too high: {e:.3} (hyp: {})", transcript.text);
    }

    #[test]
    #[ignore = "requires LW_SHERPA_MULTILINGUAL_MODEL_DIR and the model files"]
    fn transcribes_the_russian_fixture() {
        // This one needs a *different* model from the other tests, so it announces a skip rather
        // than failing a `-- --ignored` run that only set LW_SHERPA_MODEL_DIR.
        let Some(dir) = util::env_dir("LW_SHERPA_MULTILINGUAL_MODEL_DIR") else {
            eprintln!(
                "SKIPPED: set LW_SHERPA_MULTILINGUAL_MODEL_DIR to a multilingual model \
                 (e.g. sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8) to run the Russian fixture"
            );
            return;
        };
        let mut engine = SherpaEngine::open(config(dir).with_language("ru")).expect("load model");
        let audio = util::load_wav(&util::fixture("fleurs_ru_1.wav"));
        let secs = audio.duration_secs();

        let t0 = Instant::now();
        let transcript = engine.transcribe(&audio).expect("transcribe");
        util::report("ru", secs, t0.elapsed(), &transcript.text);

        let e = util::wer(RU_1, &transcript.text);
        eprintln!("WER: {e:.3}");
        assert!(!transcript.text.is_empty(), "empty transcript");
        assert!(e < 0.5, "WER too high: {e:.3} (hyp: {})", transcript.text);
    }

    #[test]
    #[ignore = "requires LW_SHERPA_MODEL_DIR and the model files"]
    fn health_check_reports_the_real_device() {
        let Some(dir) = util::env_dir("LW_SHERPA_MODEL_DIR") else {
            panic!("set LW_SHERPA_MODEL_DIR");
        };
        let mut engine = SherpaEngine::open(config(dir)).expect("load model");
        let health = engine.health_check();
        eprintln!("{health:?}");
        assert!(health.ok, "probe failed: {}", health.message);
        assert_eq!(health.provider, lw_core::engine::Provider::NativeCpu);
        assert!(health.probe_latency_ms.is_some_and(|ms| ms > 0.0));
        assert!(
            health.message.contains(engine.backend_name()),
            "the health message should name the detected model kind: {}",
            health.message
        );
    }

    #[test]
    #[ignore = "requires LW_SHERPA_MODEL_DIR and the model files"]
    fn initialize_is_idempotent_and_shutdown_releases() {
        use lw_core::engine::EngineInitContext;

        let Some(dir) = util::env_dir("LW_SHERPA_MODEL_DIR") else {
            panic!("set LW_SHERPA_MODEL_DIR");
        };
        let ctx = EngineInitContext {
            model_dir: dir.clone(),
            cache_dir: std::env::temp_dir().join("lw-sherpa-test-cache"),
            cpu_threads: 2,
        };
        let mut engine = SherpaEngine::new(config(dir));
        engine.initialize(&ctx).expect("first initialize");
        let kind = engine.model_kind();

        let again = Instant::now();
        engine.initialize(&ctx).expect("second initialize");
        // Reloading would take seconds; a no-op takes microseconds.
        assert!(
            again.elapsed().as_millis() < 100,
            "initialize reloaded the model instead of no-opping"
        );
        assert_eq!(engine.model_kind(), kind);

        engine.shutdown();
        assert!(!engine.is_initialized());
        assert_eq!(engine.backend_name(), "sherpa-onnx");
    }
}

#[test]
fn wer_helper_is_sane() {
    assert_eq!(util::wer("a b c", "a b c"), 0.0);
    assert!((util::wer("a b c", "a x c") - 1.0 / 3.0).abs() < 1e-6);
    assert_eq!(util::wer("", ""), 0.0);
}

#[test]
fn the_english_fixture_is_present_and_is_16khz_mono() {
    let audio = util::load_wav(&util::fixture("fleurs_en_1.wav"));
    assert_eq!(audio.sample_rate, 16_000);
    assert!(
        (audio.duration_secs() - 6.0).abs() < 0.5,
        "{}",
        audio.duration_secs()
    );
}
