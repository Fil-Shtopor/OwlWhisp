//! The palette of the window, carried over from the web front end it replaces.
//!
//! The colours are the ones in `app/frontend/src/styles.css`, token for token, so the native
//! window is recognisably the same application rather than a different one that does the same job.
//!
//! Text is the one thing not yet guaranteed identical across platforms. Everything here is
//! rasterised on the CPU by tiny-skia, so shapes and colours come out the same pixel for pixel on
//! Windows, macOS and Linux -- but the glyphs come from whatever font the system offers, and that
//! differs. Fixing it means embedding a font and recording its licence; noted in
//! `docs/architecture.md` rather than left to be discovered.

use iced::{Border, Color, Theme};

/// `--bg`
pub const BG: Color = rgb(0x11, 0x13, 0x1a);
/// `--bg-raised`
pub const BG_RAISED: Color = rgb(0x1a, 0x1d, 0x27);
/// `--bg-inset`
pub const BG_INSET: Color = rgb(0x0c, 0x0e, 0x14);
/// `--border`
pub const BORDER: Color = rgb(0x2a, 0x2e, 0x3d);
/// `--text`
pub const TEXT: Color = rgb(0xe5, 0xe7, 0xee);
/// `--text-dim`
pub const TEXT_DIM: Color = rgb(0x9a, 0xa0, 0xb0);
/// `--accent`
pub const ACCENT: Color = rgb(0x89, 0xb4, 0xfa);
/// `.badge.yes` -- a measurement that exists, a capability that is real.
pub const GOOD: Color = rgb(0x22, 0xc5, 0x5e);
/// `.badge.no` -- cannot run, refused, absent.
pub const BAD: Color = rgb(0xef, 0x44, 0x44);
/// The amber for estimates, which must never look like measurements.
pub const ESTIMATE: Color = rgb(0xf5, 0xa9, 0x7a);

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: 1.0,
    }
}

/// The same colour at a fraction of its opacity, for the tinted fill behind a pill.
pub const fn faded(c: Color, alpha: f32) -> Color {
    Color { a: alpha, ..c }
}

/// The widest a paragraph of running prose is allowed to get.
///
/// `--measure: 78ch` in the stylesheet, and for the same reason: a sentence two thousand pixels
/// wide is measurably harder to read, because the eye loses the line on the way back. Everything
/// else in the window grows with it.
pub const MEASURE: f32 = 620.0;

/// The application's theme.
pub fn theme() -> Theme {
    palette_theme("OwlWhisp", BG)
}

/// The overlay's theme: the same colours, but cleared to nothing.
///
/// The window background comes from the palette, and the overlay's window must not paint one at
/// all -- it is a pill floating over another application, and anything behind the pill would be a
/// grey rectangle following it around. There is no per-window hook for this in iced 0.13; the
/// theme is the only thing the window id reaches, so the transparency travels in the palette.
pub fn overlay_theme() -> Theme {
    palette_theme("OwlWhisp Overlay", iced::Color::TRANSPARENT)
}

fn palette_theme(name: &str, background: iced::Color) -> Theme {
    Theme::custom(
        name.to_string(),
        iced::theme::Palette {
            background,
            text: TEXT,
            primary: ACCENT,
            success: GOOD,
            // iced 0.14 asks for a warning colour of its own. The amber this already uses for an
            // estimate is the same idea -- "true, but not measured" -- so nothing new is invented.
            warning: ESTIMATE,
            danger: BAD,
        },
    )
}

/// What the window is cleared to before anything is drawn. Reads the palette, so the overlay's
/// transparent background is honoured rather than special-cased twice.
pub fn appearance(theme: &Theme) -> iced::theme::Style {
    iced::theme::Style {
        background_color: theme.palette().background,
        text_color: theme.palette().text,
    }
}

/// An action button, with a disabled state that plainly looks disabled.
///
/// iced's default dims a disabled button only slightly, and against this palette the difference is
/// not visible. A button that looks pressable and is not reads as the application ignoring the
/// click -- which is exactly how "I cannot select a model" begins. `danger` colours the label for
/// an action that removes something.
pub fn action(danger: bool) -> impl Fn(&Theme, iced::widget::button::Status) -> iced::widget::button::Style {
    move |_t, status| {
        let disabled = matches!(status, iced::widget::button::Status::Disabled);
        let base = if danger { BAD } else { ACCENT };
        iced::widget::button::Style {
            background: Some(
                if disabled {
                    // A flat, recessed surface: not a button that is merely a different colour,
                    // but one that is plainly not raised.
                    BG_RAISED
                } else if matches!(status, iced::widget::button::Status::Hovered) {
                    faded(base, 0.85)
                } else {
                    base
                }
                .into(),
            ),
            text_color: if disabled {
                faded(TEXT_DIM, 0.55)
            } else if danger {
                Color::WHITE
            } else {
                BG
            },
            border: Border {
                color: if disabled { BORDER } else { Color::TRANSPARENT },
                width: 1.0,
                radius: 8.0.into(),
            },
            ..Default::default()
        }
    }
}

/// A raised surface with a border, matching `.card` and the model table's container.
pub fn card(_t: &Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(BG_RAISED.into()),
        border: Border {
            color: BORDER,
            width: 1.0,
            radius: 10.0.into(),
        },
        ..Default::default()
    }
}

/// A sunken surface, matching `.card-inset` and the expanded row's body.
pub fn inset(_t: &Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(BG_INSET.into()),
        border: Border {
            color: BORDER,
            width: 1.0,
            radius: 8.0.into(),
        },
        ..Default::default()
    }
}

/// A pill: rounded, tinted with its own colour at low opacity.
pub fn pill(colour: Color) -> impl Fn(&Theme) -> iced::widget::container::Style {
    move |_t| iced::widget::container::Style {
        background: Some(faded(colour, 0.15).into()),
        text_color: Some(colour),
        border: Border {
            radius: 999.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// A small square-ish chip, matching `.hw-chip`: an accelerator name beside a number.
pub fn chip(_t: &Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(BG_INSET.into()),
        text_color: Some(TEXT_DIM),
        border: Border {
            radius: 4.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// A horizontal rule, matching the `border-bottom` the table rows carry.
pub fn rule(_t: &Theme) -> iced::widget::rule::Style {
    iced::widget::rule::Style {
        color: BORDER,
        radius: 0.0.into(),
        fill_mode: iced::widget::rule::FillMode::Full,
        // A one-pixel line drawn between two device pixels is two grey ones; snapping puts it on
        // the grid, which is what the border it stands in for always did.
        snap: true,
    }
}
