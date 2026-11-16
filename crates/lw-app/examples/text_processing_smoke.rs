//! Explicit, isolated integration experiment; never writes the user's settings.
//! cargo run -p lw-app --example text_processing_smoke -- <scratch-settings.json> cpu|cuda
use lw_core::{
    settings::{Settings, TranslationBackend},
    text::TextLanguage,
};
use serde_json::json;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

fn main() -> Result<(), String> {
    let path = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("Pass an isolated settings path")?,
    );
    let backend = match std::env::args().nth(2).as_deref() {
        Some("cpu") => TranslationBackend::Cpu,
        Some("cuda") => TranslationBackend::Cuda,
        _ => return Err("Choose cpu or cuda".into()),
    };
    if !lw_app::translation::is_ready(&path, backend) {
        let setup = lw_app::translation::start_setup(&path, backend);
        loop {
            if let Some(result) = setup.poll() {
                result?;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    let mut settings = Settings::default();
    settings.text_processing.remove_fillers = true;
    settings.text_processing.format_punctuation = true;
    settings.text_processing.language = TextLanguage::English;
    let pipeline = lw_app::bench::build_pipeline(&settings);
    let sample = "Um, please send the report uh, by Friday . thank you !";
    let clean = pipeline.run(sample);
    assert_eq!(clean, "Please send the report by Friday. Thank you!");
    for _ in 0..20 {
        std::hint::black_box(pipeline.run(sample));
    }
    let start = Instant::now();
    for _ in 0..1000 {
        std::hint::black_box(pipeline.run(sample));
    }
    let cleanup_ms = start.elapsed().as_secs_f64(); // 1,000 iterations: seconds == mean ms
    let start = Instant::now();
    let mut translator = lw_app::translation::Translator::start(&path, backend)?;
    let load_ms = start.elapsed().as_secs_f64() * 1000.0;
    let mut results = Vec::new();
    for (target, input) in [
        (
            TextLanguage::English,
            "Отправь, пожалуйста, отчёт до пятницы. Спасибо!",
        ),
        (
            TextLanguage::Russian,
            "Please send the report by Friday. Thank you!",
        ),
        (
            TextLanguage::French,
            "Please send the report by Friday. Thank you!",
        ),
        (
            TextLanguage::Italian,
            "Please send the report by Friday. Thank you!",
        ),
        (
            TextLanguage::Spanish,
            "Please send the report by Friday. Thank you!",
        ),
        (
            TextLanguage::Chinese,
            "Please send the report by Friday. Thank you!",
        ),
        (
            TextLanguage::English,
            "Установи debug_mode=true в settings.toml и открой /api/v2/users.",
        ),
        (TextLanguage::English, "Скажи «привет» и вернись завтра."),
    ] {
        let start = Instant::now();
        let output = translator.translate(input, target, start + Duration::from_secs(10))?;
        results.push(
            json!({"target":target,"input":input,"output":output,"ms":start.elapsed().as_secs_f64()*1000.0}),
        );
    }
    let too_long = "x".repeat(3001);
    assert!(
        translator
            .translate(
                &too_long,
                TextLanguage::English,
                Instant::now() + Duration::from_secs(1)
            )
            .is_err()
    );
    assert!(
        translator.is_alive(),
        "length rejection must not unload a healthy model"
    );
    let start = Instant::now();
    let timeout = translator.translate("Привет", TextLanguage::English, start);
    assert!(timeout.is_err());
    assert!(
        !translator.is_alive(),
        "a timed-out worker must release its memory"
    );
    let shutdown_ms = start.elapsed().as_secs_f64() * 1000.0;
    // Exercise a real in-flight HTTP deadline as well as the already-expired fast path.
    let mut timeout_worker = lw_app::translation::Translator::start(&path, backend)?;
    let start = Instant::now();
    let request_timeout = timeout_worker.translate(
        "Please send the report by Friday. Thank you!",
        TextLanguage::Russian,
        start + Duration::from_millis(1),
    );
    assert!(request_timeout.is_err());
    assert!(!timeout_worker.is_alive());
    let request_timeout_ms = start.elapsed().as_secs_f64() * 1000.0;
    // Wait for actual OS reclamation only after measuring the source-delivery deadline path.
    lw_app::translation::stop_all();
    println!("{}", serde_json::to_string_pretty(&json!({"backend":translator.backend(),"cleanup_mean_ms":cleanup_ms,"load_ms":load_ms,"results":results,"deadline_shutdown_ms":shutdown_ms,"deadline_error":timeout.unwrap_err(),"http_timeout_ms":request_timeout_ms,"http_timeout_error":request_timeout.unwrap_err()})).unwrap());
    Ok(())
}
