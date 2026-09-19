//! The handful of shapes the web front end used everywhere, redrawn.
//!
//! Pills, chips and dimmed sub-lines are not decoration: the project's rule is that an estimate
//! and a measurement must never look alike, and these shapes are how that rule stays visible. They
//! live in one place so the rule is enforced once rather than remembered at every call site.

use iced::widget::{container, text, Container, Text};
use iced::{Color, Element, Length, Padding};

use crate::theme;

/// A rounded pill with a tinted background, matching `.badge`.
pub fn badge<'a, M: 'a>(label: impl text::IntoFragment<'a>, colour: Color) -> Element<'a, M> {
    // Never wrapped. A pill is a label, and a label broken across four lines reads as damage:
    // "Recommended" came out as "* Reco / mme / nded" the first time this was drawn.
    container(
        text(label)
            .size(12)
            .wrapping(text::Wrapping::None),
    )
    .padding(Padding::from([1, 9]))
    .style(theme::pill(colour))
    .into()
}

/// A badge in the palette's "yes" green.
pub fn badge_yes<'a, M: 'a>(label: impl text::IntoFragment<'a>) -> Element<'a, M> {
    badge(label, theme::GOOD)
}

/// A badge in the palette's "no" red.
pub fn badge_no<'a, M: 'a>(label: impl text::IntoFragment<'a>) -> Element<'a, M> {
    badge(label, theme::BAD)
}

/// A small neutral chip, matching `.hw-chip`: an accelerator name beside a number.
pub fn chip<'a, M: 'a>(label: impl text::IntoFragment<'a>) -> Element<'a, M> {
    container(
        text(label)
            .size(10)
            .font(iced::Font::MONOSPACE)
            .wrapping(text::Wrapping::None),
    )
    .padding(Padding::from([1, 5]))
    .style(theme::chip)
    .into()
}

/// Dimmed secondary text, matching `.sub`.
pub fn sub<'a>(s: impl text::IntoFragment<'a>) -> Text<'a> {
    text(s).size(12).color(theme::TEXT_DIM)
}

/// Ordinary body text.
pub fn body<'a>(s: impl text::IntoFragment<'a>) -> Text<'a> {
    text(s).size(14).color(theme::TEXT)
}

/// A number that was measured: green, because a measurement is the thing worth trusting.
pub fn measured<'a>(label: &str, rate: f32) -> Text<'a> {
    text(format!("{label} {:.1}%", rate * 100.0))
        .size(13)
        .font(iced::Font::MONOSPACE)
        .color(theme::GOOD)
}

/// A number that was estimated: amber, and prefixed with the tilde used throughout the project.
pub fn estimated<'a>(rtf: f32) -> Text<'a> {
    text(format!("~{rtf:.4}"))
        .size(13)
        .font(iced::Font::MONOSPACE)
        .color(theme::ESTIMATE)
}

/// Monospace, for paths and figures that should line up.
pub fn mono<'a>(s: impl text::IntoFragment<'a>) -> Text<'a> {
    text(s).size(13).font(iced::Font::MONOSPACE).color(theme::TEXT)
}

/// A section heading.
pub fn heading<'a>(s: impl text::IntoFragment<'a>) -> Text<'a> {
    text(s).size(17).color(theme::TEXT)
}

/// A paragraph of running prose, held to a readable measure.
pub fn prose<'a, M: 'a>(s: impl text::IntoFragment<'a>) -> Element<'a, M> {
    container(text(s).size(13).color(theme::TEXT_DIM))
        .max_width(theme::MEASURE)
        .into()
}

/// A raised card with a border.
pub fn card<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> Container<'a, M> {
    container(content)
        .padding(14)
        .width(Length::Fill)
        .style(theme::card)
}

/// A sunken panel, for an expanded row's body.
pub fn inset<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> Container<'a, M> {
    container(content)
        .padding(12)
        .width(Length::Fill)
        .style(theme::inset)
}
