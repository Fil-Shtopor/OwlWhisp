//! The LocalWisper window, drawn natively on the CPU.
//!
//! Replaces the Tauri/WebView2 front end. Measured on the target machine: that one costs eight
//! processes and 240 MB of private commit to show a settings form and a table; an equivalent
//! window with a GPU context costs 113 MB in one process; this one costs 18 MB, because nothing
//! here opens a GPU context at all. The rasteriser is tiny-skia, which also means the same pixels
//! on Windows, macOS and Linux rather than whatever each platform's driver produces.

mod app;
mod panels;
mod theme;
mod widgets;

fn main() -> iced::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "lw_gui=info,lw_app=info,lw_core=info".into()),
        )
        .init();

    iced::application(app::App::title, app::App::update, app::App::view)
        .theme(app::App::theme)
        .subscription(app::App::subscription)
        .window(iced::window::Settings {
            size: iced::Size::new(1000.0, 720.0),
            min_size: Some(iced::Size::new(560.0, 420.0)),
            ..Default::default()
        })
        .run()
}
