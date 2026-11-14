//! Colors for the terminal UI.
//!
//! Panels stay near-black. The accent is a weathered-copper verdigris: light
//! enough to read on the background, and dark text stays readable on top of it.
//! Launch is a slightly brighter green so the primary control is still obvious.

use ratatui::style::{Color, Modifier, Style};

pub const BG: Color = Color::Rgb(14, 18, 18);
pub const SURFACE: Color = Color::Rgb(24, 32, 30);
pub const SURFACE_2: Color = Color::Rgb(36, 50, 46);
pub const BORDER: Color = Color::Rgb(58, 82, 74);
pub const TEXT: Color = Color::Rgb(230, 236, 232);
pub const MUTED: Color = Color::Rgb(140, 158, 150);
pub const ACCENT: Color = Color::Rgb(118, 196, 172);
pub const ON_ACCENT: Color = Color::Rgb(12, 28, 26);
pub const LAUNCH: Color = Color::Rgb(72, 176, 128);
pub const DANGER: Color = Color::Rgb(214, 92, 78);

pub fn base() -> Style {
    Style::default().fg(TEXT).bg(BG)
}

pub fn muted() -> Style {
    Style::default().fg(MUTED).bg(BG)
}

pub fn surface() -> Style {
    Style::default().fg(TEXT).bg(SURFACE)
}

pub fn title() -> Style {
    Style::default()
        .fg(ACCENT)
        .bg(SURFACE)
        .add_modifier(Modifier::BOLD)
}

pub fn selected() -> Style {
    Style::default()
        .fg(ON_ACCENT)
        .bg(ACCENT)
        .add_modifier(Modifier::BOLD)
}

pub fn hover() -> Style {
    Style::default().fg(TEXT).bg(SURFACE_2)
}

/// Keyboard focus on a row button. Warm gold, so it is not the row highlight.
pub fn focused_control() -> Style {
    Style::default()
        .fg(Color::Rgb(24, 16, 4))
        .bg(Color::Rgb(255, 196, 64))
        .add_modifier(Modifier::BOLD)
}

/// A row button that is visible on both the teal selection and a plain row.
pub fn row_button() -> Style {
    Style::default()
        .fg(TEXT)
        .bg(Color::Rgb(8, 12, 12))
        .remove_modifier(Modifier::BOLD)
}

pub fn disabled_button() -> Style {
    Style::default().fg(MUTED).bg(SURFACE_2)
}

pub fn danger() -> Style {
    Style::default().fg(DANGER).bg(BG)
}
