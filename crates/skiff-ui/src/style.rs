//! Colors and metrics shared by widgets. Metrics are logical pixels; `Ui`
//! multiplies them by the display scale (which includes `Style::zoom`).

use crate::color::Color;

/// The named colors every widget draws with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    /// Window background.
    pub bg: Color,
    /// Panels, inputs and other raised areas.
    pub surface: Color,
    /// A second surface shade: button faces, hovered rows, menus, the drag label.
    pub surface_alt: Color,
    /// Normal text.
    pub fg: Color,
    /// Secondary text and scrollbar thumbs.
    pub muted: Color,
    /// Highlight color, e.g. the keyboard focus ring.
    pub accent: Color,
    /// Text drawn on top of `accent`.
    pub accent_fg: Color,
    /// Dividers and subtle outlines.
    pub border: Color,
    /// Outline of inputs, checkboxes and buttons: at least 3:1 on `bg` and
    /// `surface` (WCAG 1.4.11), where `border` may be a subtle divider.
    pub control: Color,
    /// Selected rows, highlighted menu items, selected text.
    pub selection: Color,
    /// Errors and destructive actions.
    pub danger: Color,
    /// Success / OK state.
    pub success: Color,
    /// Warnings.
    pub warning: Color,
}

/// The full look of the interface: palette plus sizes (logical pixels).
#[derive(Debug, Clone, PartialEq)]
pub struct Style {
    pub palette: Palette,
    /// Size of the proportional UI font.
    pub font_px: f32,
    /// Size of the monospace font.
    pub mono_px: f32,
    /// Corner radius of buttons, inputs and panels.
    pub radius: f32,
    /// Gap between neighbouring widgets.
    pub spacing: i32,
    /// Inner padding of widgets.
    pub padding: i32,
    /// Interface zoom: multiplies the display scale for layout and text
    /// alike, so rows grow with their text.
    pub zoom: f32,
}

impl Style {
    /// The built-in dark theme at 14px text and zoom 1.
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
    /// Same as `Style::dark_default`.
    fn default() -> Self {
        Self::dark_default()
    }
}
