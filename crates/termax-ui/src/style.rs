//! Colors and metrics shared by widgets. Metrics are logical pixels.

use crate::color::Color;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub bg: Color,
    pub surface: Color,
    pub surface_alt: Color,
    pub fg: Color,
    pub muted: Color,
    pub accent: Color,
    pub accent_fg: Color,
    pub border: Color,
    /// Outline of inputs, checkboxes and buttons: at least 3:1 on `bg` and
    /// `surface` (WCAG 1.4.11), where `border` may be a subtle divider.
    pub control: Color,
    pub selection: Color,
    pub danger: Color,
    pub success: Color,
    pub warning: Color,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Style {
    pub palette: Palette,
    pub font_px: f32,
    pub mono_px: f32,
    pub radius: f32,
    pub spacing: i32,
    pub padding: i32,
    /// Interface zoom: multiplies the display scale for layout and text
    /// alike, so rows grow with their text.
    pub zoom: f32,
}

impl Style {
    pub fn dark_default() -> Self {
        Style {
            palette: Palette {
                bg: Color::hex(0x1e1f29),
                surface: Color::hex(0x262835),
                surface_alt: Color::hex(0x313343),
                fg: Color::hex(0xe6e6ef),
                muted: Color::hex(0x8b8fa7),
                accent: Color::hex(0x7c8cff),
                accent_fg: Color::hex(0x10121a),
                border: Color::hex(0x3c3f52),
                control: Color::hex(0x6e7290),
                selection: Color::hex(0x3d4470),
                danger: Color::hex(0xff6b6b),
                success: Color::hex(0x5fd38d),
                warning: Color::hex(0xf5c26b),
            },
            font_px: 14.0,
            mono_px: 14.0,
            radius: 6.0,
            spacing: 8,
            padding: 8,
            zoom: 1.0,
        }
    }
}

impl Default for Style {
    fn default() -> Self {
        Self::dark_default()
    }
}
