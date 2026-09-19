//! The LocalWisper window, drawn natively.
//!
//! Replaces the Tauri/WebView2 front end. Measured on the target machine, that one costs seven
//! WebView2 processes and 506 MB of working set to show a settings form and a table; this one is
//! the same process that already holds the engine.

mod app;
mod panels;
mod theme;
mod widgets;

fn main() -> eframe::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "lw_gui=info,lw_app=info,lw_core=info".into()),
        )
        .init();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1000.0, 720.0])
            .with_min_inner_size([560.0, 420.0])
            .with_title("LocalWisper"),
        ..Default::default()
    };
    eframe::run_native(
        "LocalWisper",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
