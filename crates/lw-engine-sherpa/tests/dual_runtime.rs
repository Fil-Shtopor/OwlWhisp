//! The coexistence test: **two ONNX Runtimes in one process**.
//!
//! `lw-ort` loads Microsoft's `onnxruntime.dll` at runtime, by full path (`ort`'s `load-dynamic`).
//! sherpa-onnx carries its own ONNX Runtime, linked *statically* into the binary by the `static`
//! feature the workspace pins. This test binary links both, and checks that:
//!
//! 1. it links at all (a duplicate-symbol or `/MT` vs `/MD` CRT clash would fail here),
//! 2. `lw_ort::OrtRuntime::init` succeeds *and* Microsoft's runtime can build a real CPU session,
//! 3. a sherpa transcription runs in the same process, both before and after that session exists,
//! 4. neither runtime is disturbed by the other.
//!
//! ```text
//! LW_RUNTIME_DIR=runtime/win-arm64 LW_SHERPA_MODEL_DIR=<models>/sherpa-onnx-whisper-tiny.en \
//!   cargo test -p lw-engine-sherpa --features sherpa --test dual_runtime -- --ignored --nocapture
//! ```

mod util;

#[cfg(feature = "sherpa")]
mod dual {
    use lw_core::engine::SpeechEngine;
    use lw_engine_sherpa::{SherpaConfig, SherpaEngine};
    use lw_ort::{CpuSessionConfig, OrtRuntime, build_cpu_session};

    use super::util;

    /// Pick an ONNX file out of the sherpa model directory to feed to Microsoft's runtime, so both
    /// runtimes are genuinely executing, not merely loaded.
    fn an_onnx_file(files: &lw_engine_sherpa::ModelFiles) -> std::path::PathBuf {
        files
            .files()
            .into_iter()
            .find(|p| p.extension().is_some_and(|e| e == "onnx"))
            .expect("the detected layout must reference at least one .onnx file")
            .to_path_buf()
    }

    #[test]
    #[ignore = "requires LW_RUNTIME_DIR + LW_SHERPA_MODEL_DIR"]
    fn sherpa_and_lw_ort_coexist_in_one_process() {
        let Some(runtime_dir) = util::env_dir("LW_RUNTIME_DIR") else {
            panic!("set LW_RUNTIME_DIR to the directory holding onnxruntime.dll");
        };
        let Some(model_dir) = util::env_dir("LW_SHERPA_MODEL_DIR") else {
            panic!("set LW_SHERPA_MODEL_DIR to an extracted sherpa-onnx model directory");
        };

        // 1. Microsoft's ONNX Runtime, loaded dynamically by full path.
        let runtime = OrtRuntime::init(&runtime_dir).expect("init Microsoft ONNX Runtime");
        eprintln!("lw-ort devices: {:?}", runtime.device_summary());

        // 2. sherpa-onnx's statically linked ONNX Runtime, in the same process.
        let mut engine = SherpaEngine::open(SherpaConfig::new(model_dir)).expect("load the sherpa model");
        eprintln!(
            "sherpa engine: {} on {}",
            engine.backend_name(),
            engine.device().name
        );

        let audio = util::load_wav(&util::fixture("fleurs_en_1.wav"));
        let before = engine
            .transcribe(&audio)
            .expect("sherpa transcribe (before ORT session)");
        assert!(!before.text.is_empty(), "sherpa produced nothing");
        eprintln!("sherpa (before ORT session): {}", before.text);

        // 3. Now make Microsoft's runtime actually execute: build a CPU session over one of the
        //    very same ONNX graphs sherpa has open. If the two runtimes shared state, this is
        //    where it would show.
        let onnx = an_onnx_file(engine.model_files().expect("engine is initialized"));
        eprintln!("lw-ort loading {}", onnx.display());
        let session = build_cpu_session(&runtime, &onnx, CpuSessionConfig::default())
            .expect("Microsoft ONNX Runtime failed to build a CPU session");
        let inputs: Vec<String> = session.inputs().iter().map(|i| i.name().to_string()).collect();
        eprintln!("lw-ort session inputs: {inputs:?}");
        assert!(!inputs.is_empty(), "the ORT session reported no inputs");

        // 4. sherpa must still work with Microsoft's runtime live in the process.
        let after = engine
            .transcribe(&audio)
            .expect("sherpa transcribe (after ORT session)");
        eprintln!("sherpa (after ORT session): {}", after.text);
        assert_eq!(
            before.text, after.text,
            "sherpa's output changed once Microsoft's ONNX Runtime was in play"
        );

        // 5. …and Microsoft's runtime must still be usable after sherpa ran again.
        drop(session);
        let session2 = build_cpu_session(&runtime, &onnx, CpuSessionConfig::default())
            .expect("second ORT session after a sherpa decode");
        assert!(!session2.inputs().is_empty());

        // 6. The NPU path is the reason `lw-ort` exists, so check sherpa's presence did not cost
        //    us the QNN plugin EP. Reported rather than asserted: not every machine has an NPU.
        if runtime.qnn_available() {
            let registered = runtime.register_qnn();
            eprintln!(
                "QNN after sherpa: registered={registered} npu={} devices={}",
                runtime.has_qnn_npu(),
                runtime.qnn_npu_count()
            );
            assert!(
                registered,
                "QNN failed to register with sherpa-onnx in the process"
            );
            let after_qnn = engine
                .transcribe(&audio)
                .expect("sherpa transcribe after QNN register");
            assert_eq!(before.text, after_qnn.text, "registering QNN disturbed sherpa");
        } else {
            eprintln!(
                "QNN provider not present in {}; skipped the NPU check",
                runtime_dir.display()
            );
        }

        eprintln!("both ONNX Runtimes coexisted: OK");
    }
}

#[test]
fn the_test_binary_links_both_runtimes() {
    // Linking is the thing under test here; if `lw-ort` and a statically linked sherpa-onnx could
    // not live in one binary, this file would not have built. Touch both so neither is dropped.
    assert!(!lw_ort::onnxruntime_lib_name().is_empty());
    assert_eq!(lw_engine_sherpa::is_available(), cfg!(feature = "sherpa"));
    let _ = util::repo_root();
}
