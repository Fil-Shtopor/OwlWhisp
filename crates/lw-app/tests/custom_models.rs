//! Optional end-to-end check against a real, externally stored ONNX export.
#![cfg(feature = "sherpa")]

use lw_app::custom_models::{self, LocalModelKind};
use lw_core::engine::{BackendPreference, Provider};
use lw_core::settings::Settings;

#[test]
#[ignore = "requires LW_CUSTOM_MODEL_TEST_DIR pointing to a real Whisper sherpa-onnx export"]
fn imports_selects_and_transcribes_with_an_external_model() {
    let directory = std::env::var_os("LW_CUSTOM_MODEL_TEST_DIR")
        .map(std::path::PathBuf::from)
        .expect("set LW_CUSTOM_MODEL_TEST_DIR to a real Whisper ONNX model folder");
    let validated = custom_models::run_validation_worker(&directory, LocalModelKind::Whisper).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let settings_path = temp.path().join("settings.json");
    let id = custom_models::register_local(&settings_path, "External Whisper", validated, None).unwrap();
    lw_app::install::select(&settings_path, &id).unwrap();
    let settings = Settings::load(&settings_path).unwrap();
    assert_eq!(settings.model_id, id);
    assert_eq!(settings.backend, BackendPreference::ForceCpu);

    let mut loaded = lw_app::bench::load_engine(&settings_path).unwrap();
    assert_eq!(loaded.engine.provider(), Provider::NativeCpu);
    let (clips, _) = lw_core::bench::quick_clips(None, 15).unwrap();
    let clip = clips
        .iter()
        .find(|c| c.language.as_deref() == Some("en"))
        .expect("English fixture");
    let transcript = loaded.engine.transcribe(&clip.audio).unwrap();
    assert!(
        !transcript.text.trim().is_empty(),
        "real speech must produce a transcript"
    );
    assert!(directory.is_dir(), "the source folder must remain user-owned");
}
