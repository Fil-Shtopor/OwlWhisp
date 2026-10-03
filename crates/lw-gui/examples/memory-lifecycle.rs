//! Manual memory QA without opening a microphone, injecting text or changing user settings.
//! Each JSON phase waits for a newline, letting a sampler measure the whole process tree.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::time::Instant;

fn phase(name: &str, cycle: usize, extra: serde_json::Value) -> Result<(), String> {
    println!(
        "{}",
        serde_json::json!({"phase":name, "cycle":cycle, "pid":std::process::id(), "details":extra})
    );
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    let mut line = String::new();
    if std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| e.to_string())?
        == 0
    {
        return Err("sampler disconnected".into());
    }
    Ok(())
}

fn run() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    let settings = PathBuf::from(args.next().ok_or("usage: memory-lifecycle SETTINGS WAV")?);
    let wav = PathBuf::from(args.next().ok_or("missing fixture WAV")?);
    let audio = lw_core::bench::load_wav(&wav).map_err(|e| e.to_string())?;
    phase(
        "baseline",
        0,
        serde_json::json!({"audio_seconds":audio.duration_secs()}),
    )?;
    let diagnostics = lw_app::diagnostics::collect(env!("CARGO_PKG_VERSION"));
    phase(
        "after_diagnostics",
        0,
        serde_json::json!({"accelerators":diagnostics.accelerators}),
    )?;
    let mut previous_text = None;
    for cycle in 1..=2 {
        let start = Instant::now();
        let mut loaded = lw_app::bench::load_engine(&settings)?;
        phase(
            "loaded",
            cycle,
            serde_json::json!({"load_ms":start.elapsed().as_millis(), "provider":loaded.engine.provider().to_string()}),
        )?;
        let start = Instant::now();
        let transcript = loaded.engine.transcribe(&audio).map_err(|e| e.to_string())?;
        if transcript.text.trim().is_empty() {
            return Err("speech fixture produced no text".into());
        }
        if previous_text
            .as_ref()
            .is_some_and(|text| text != &transcript.text)
        {
            return Err("transcript changed after releasing and reloading the engine".into());
        }
        previous_text = Some(transcript.text.clone());
        phase(
            "transcribed",
            cycle,
            serde_json::json!({"transcribe_ms":start.elapsed().as_millis(), "text_chars":transcript.text.chars().count()}),
        )?;
        drop(loaded);
        phase("released", cycle, serde_json::json!({}))?;
    }
    Ok(())
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).is_some_and(|arg| arg == "--provider-probe") {
        let result = lw_core::capabilities::Accelerator::from_id(&args[3])
            .ok_or_else(|| "invalid accelerator".to_string())
            .and_then(|accel| {
                lw_app::provider_worker::run_provider_probe(std::path::Path::new(&args[2]), accel)
            });
        println!("{}", serde_json::to_string(&result).unwrap());
        lw_ort::exit_without_teardown(i32::from(result.is_err()));
    }
    if args.get(1).is_some_and(|arg| arg == "--provider-worker") {
        let result = lw_app::provider_worker::run_provider_worker();
        lw_ort::exit_without_teardown(i32::from(result.is_err()));
    }
    let result = run();
    if let Err(error) = &result {
        eprintln!("{error}");
    }
    lw_ort::exit_without_teardown(i32::from(result.is_err()));
}
