//! The dictation overlay: a small pill that floats over whatever the user is working in.
//!
//! A port of `app/frontend/src/overlay/overlay.tsx`, down to the colours, which come from
//! `stateVisuals.ts` and are deliberately not the ones the main window uses -- this thing is read
//! out of the corner of the eye, over an unknown background, and needs more contrast than a panel
//! sitting on our own dark card does.
//!
//! Everything that makes it an overlay rather than a window -- transparent, undecorated, always on
//! top, off the taskbar, and click-through -- is set where the window is opened, in `app.rs`. What
//! is here is only what it draws.

use iced::widget::{column, container, row, text, Space};
use iced::{Color, Element, Length};
use lw_app::dictation::RecordingState;

/// The pill's size. Matches the Tauri overlay window so the thing appears in the same place, at the
/// same size, as the build it replaces.
pub const WIDTH: f32 = 240.0;
pub const HEIGHT: f32 = 56.0;
/// How far above the bottom of the work area it sits.
pub const MARGIN: f32 = 72.0;

/// Colour and wording for one state, from `stateVisuals.ts`.
fn visual(state: RecordingState) -> (Color, &'static str, bool) {
    match state {
        RecordingState::Idle => (rgb(0x6b, 0x72, 0x80), "Idle", false),
        RecordingState::Listening => (rgb(0xef, 0x44, 0x44), "Listening", true),
        RecordingState::Processing => (rgb(0x3b, 0x82, 0xf6), "Processing", true),
        RecordingState::Done => (rgb(0x22, 0xc5, 0x5e), "Done", false),
        RecordingState::Error => (rgb(0xb9, 0x1c, 0x1c), "Error", false),
    }
}

fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgb8(r, g, b)
}

/// Draw the pill.
///
/// `phase` runs 0..1 and drives the dot's pulse. The web version animated with CSS; here the
/// caller already ticks a phase for the main window's own pill, so it is passed in rather than
/// timed again.
pub fn view<'a, M: 'a>(state: RecordingState, phase: f32) -> Element<'a, M> {
    let (colour, label, pulse) = visual(state);

    // The same 1.2s ease-in-out breath as the CSS keyframes: full size and opaque at the ends,
    // smaller and dimmer in the middle.
    let (scale, alpha) = if pulse {
        let breath = (phase * std::f32::consts::TAU).cos() * 0.5 + 0.5;
        (0.7 + 0.3 * breath, 0.6 + 0.4 * breath)
    } else {
        (1.0, 1.0)
    };
    let dot_size = 12.0 * scale;
    let dot_colour = Color { a: alpha, ..colour };

    let dot = container(Space::new(dot_size, dot_size)).style(move |_t| container::Style {
        background: Some(dot_colour.into()),
        border: iced::Border {
            radius: (dot_size / 2.0).into(),
            ..Default::default()
        },
        ..Default::default()
    });

    // A fixed-width cell around the dot so the label does not shuffle sideways as the dot breathes.
    let dot_cell = container(dot)
        .width(12.0)
        .height(12.0)
        .center_x(12.0)
        .center_y(12.0);

    let pill = container(
        row![
            dot_cell,
            text(label).size(13).color(rgb(0xe5, 0xe7, 0xee)),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center),
    )
    .padding(iced::Padding::from([0, 16]))
    .height(40)
    .center_y(40)
    .style(|_t| container::Style {
        background: Some(
            Color {
                a: 0.88,
                ..rgb(0x11, 0x13, 0x1a)
            }
            .into(),
        ),
        border: iced::Border {
            radius: 20.0.into(),
            width: 1.0,
            color: Color {
                a: 0.12,
                ..Color::WHITE
            },
        },
        ..Default::default()
    });

    // Centred in the window, with the window itself transparent: the 8px margin the web version had
    // is what keeps the pill's shadowless edge off the window edge.
    container(
        column![Space::new(0, Length::Fill), pill, Space::new(0, Length::Fill)]
            .align_x(iced::Alignment::Center),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .center_x(Length::Fill)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_state_has_its_own_colour_and_word() {
        // The pill is often the only thing on screen saying what is happening, so two states that
        // looked the same would be two states the user cannot tell apart.
        let all = [
            RecordingState::Idle,
            RecordingState::Listening,
            RecordingState::Processing,
            RecordingState::Done,
            RecordingState::Error,
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                let (ca, la, _) = visual(*a);
                let (cb, lb, _) = visual(*b);
                assert_ne!(la, lb, "{a:?} and {b:?} share a label");
                assert!(
                    (ca.r, ca.g, ca.b) != (cb.r, cb.g, cb.b),
                    "{a:?} and {b:?} share a colour"
                );
            }
        }
    }

    #[test]
    fn only_the_states_that_are_still_working_pulse() {
        // A steady dot means "nothing is happening to your audio right now". Pulsing Done or Error
        // would suggest the app was still busy with an utterance it had in fact finished with.
        assert!(visual(RecordingState::Listening).2);
        assert!(visual(RecordingState::Processing).2);
        for quiet in [
            RecordingState::Idle,
            RecordingState::Done,
            RecordingState::Error,
        ] {
            assert!(!visual(quiet).2, "{quiet:?} pulses");
        }
    }
}
