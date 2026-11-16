//! Real settings controls in an isolated window, without global hotkeys, tray or microphone capture.
//! Set LW_APP_DATA_DIR to a scratch directory before launching.
#![cfg_attr(windows, windows_subsystem = "windows")]
#![allow(dead_code)]
#[path = "../src/panels/settings.rs"]
mod settings;
#[path = "../src/theme.rs"]
mod theme;
#[path = "../src/widgets.rs"]
mod widgets;
use iced::widget::{button, column, container, scrollable};
use iced::{Element, Length};

#[derive(Default)]
struct Preview {
    state: Option<settings::State>,
    saved: bool,
}
impl Preview {
    fn new() -> Self {
        Self {
            state: Some(settings::State::new()),
            saved: false,
        }
    }
    fn update(&mut self, message: settings::Message) {
        if self.state.as_mut().unwrap().update(message) {
            self.saved = true;
        }
    }
    fn view(&self) -> Element<'_, settings::Message> {
        let content = column![
            self.state.as_ref().unwrap().text_processing_card(),
            button(widgets::button_label(if self.saved {
                "Saved - save changes again"
            } else {
                "Save settings"
            }))
            .on_press(settings::Message::Save)
            .style(theme::action(false))
        ]
        .spacing(16)
        .padding(20)
        .width(Length::Fill);
        container(scrollable(content)).into()
    }
    fn subscription(&self) -> iced::Subscription<settings::Message> {
        self.state.as_ref().unwrap().subscription()
    }
}
fn main() -> iced::Result {
    assert!(
        std::env::var_os("LW_APP_DATA_DIR").is_some(),
        "Set an isolated LW_APP_DATA_DIR"
    );
    iced::application(Preview::new, Preview::update, Preview::view)
        .title("OwlWhisp - Text settings preview")
        .theme(|_: &Preview| theme::theme())
        .subscription(Preview::subscription)
        .window_size((650.0, 900.0))
        .run()
}
