//! Design tokens, colours and metrics shared by widgets.
//!
//! Metrics are logical pixels; `Ui` multiplies them by the display scale
//! (which includes `Style::zoom`), so zoom scales spacing, radii, control
//! heights, icons and text alike. `tokens` holds the fixed scales (spacing,
//! radius, heights, motion); `TextRole` the type scale; `Palette` the
//! colour roles. Three built-in styles exist: `dark_default`,
//! `light_default` and `high_contrast`.

use crate::color::Color;
use crate::text::FontKind;

/// The design system's fixed scales, in logical pixels (milliseconds for
/// motion). Use them through `Ui::px` so they follow DPI and zoom.
pub mod tokens {
    /// Spacing scale: 2.
    pub const SPACE_2XS: i32 = 2;
    /// Spacing scale: 4.
    pub const SPACE_XS: i32 = 4;
    /// Spacing scale: 8 (the default gap between widgets).
    pub const SPACE_SM: i32 = 8;
    /// Spacing scale: 12.
    pub const SPACE_MD: i32 = 12;
    /// Spacing scale: 16.
    pub const SPACE_LG: i32 = 16;
    /// Spacing scale: 24.
    pub const SPACE_XL: i32 = 24;
    /// Spacing scale: 32.
    pub const SPACE_2XL: i32 = 32;
    /// Spacing scale: 48.
    pub const SPACE_3XL: i32 = 48;
    /// The whole spacing scale, smallest first.
    pub const SPACE: [i32; 8] = [SPACE_2XS, SPACE_XS, SPACE_SM, SPACE_MD, SPACE_LG, SPACE_XL, SPACE_2XL, SPACE_3XL];

    /// No rounding: terminal panes.
    pub const RADIUS_NONE: f32 = 0.0;
    /// Small radius: inputs, buttons.
    pub const RADIUS_SM: f32 = 4.0;
    /// Medium radius: rows, tabs, menu items.
    pub const RADIUS_MD: f32 = 6.0;
    /// Large radius: popovers, menus, the palette, dialogs, toasts.
    pub const RADIUS_LG: f32 = 10.0;

    /// Height of buttons, inputs, steppers and dropdowns.
    pub const CONTROL_H: i32 = 30;
    /// Height of list and table rows, menu items.
    pub const ROW_H: i32 = 28;
    /// Height of rows in compact lists.
    pub const ROW_H_COMPACT: i32 = 24;
    /// Height of the tab strip.
    pub const TAB_H: i32 = 34;
    /// Height of a section or table header.
    pub const HEADER_H: i32 = 28;

    /// Line height of wrapped text, as a multiple of its size.
    pub const LINE_HEIGHT: f32 = 1.4;

    /// Icon grid: icons are drawn on a 16 px square.
    pub const ICON: i32 = 16;
    /// Icon stroke width at 16 px.
    pub const ICON_STROKE: f32 = 1.5;
    /// Diameter of a status dot.
    pub const DOT: i32 = 8;

    /// Width of the keyboard focus ring.
    pub const FOCUS_RING: i32 = 2;
    /// Gap between a widget and its focus ring.
    pub const FOCUS_GAP: i32 = 2;

    /// Overlay shadow: vertical offset.
    pub const SHADOW_Y: i32 = 8;
    /// Overlay shadow: blur radius.
    pub const SHADOW_BLUR: i32 = 24;
    /// Overlay shadow: opacity of its darkest part (30%).
    pub const SHADOW_ALPHA: u8 = 77;

    /// Hover and press transitions.
    pub const MOTION_FAST_MS: u64 = 120;
    /// Popovers, dialogs and toasts appearing.
    pub const MOTION_MED_MS: u64 = 180;
    /// One cycle of the "connecting" status-dot pulse.
    pub const PULSE_MS: u64 = 1200;
}

/// The type scale. Sizes are relative to `Style::font_px` (the body size,
/// 13 by default), so a larger configured font grows every role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextRole {
    /// 12, regular: captions, badges, shortcut hints, column headers.
    Caption,
    /// 13, regular: normal UI text.
    Body,
    /// 13, semibold: emphasised body text (names in rows, button labels).
    Strong,
    /// 15, semibold: dialog and toast titles, section titles.
    Title,
    /// 18, semibold: page headers.
    Heading,
    /// 22, semibold: empty-state titles and other large headlines.
    Display,
}

/// How strongly `Palette::tint` colours a surface (badge backgrounds).
pub const TINT: f32 = 0.12;

/// The body size the type scale is defined against.
pub const BODY_PX: f32 = 13.0;

impl TextRole {
    /// The role's size at a 13 px body size.
    pub const fn base_px(self) -> f32 {
        match self {
            TextRole::Caption => 12.0,
            TextRole::Body | TextRole::Strong => 13.0,
            TextRole::Title => 15.0,
            TextRole::Heading => 18.0,
            TextRole::Display => 22.0,
        }
    }

    /// The font face: regular for caption and body, semibold for the rest.
    pub const fn font(self) -> FontKind {
        match self {
            TextRole::Caption | TextRole::Body => FontKind::Ui,
            _ => FontKind::UiSemibold,
        }
    }
}

/// The named colours every widget draws with. Design role names in parentheses
/// where the field name differs: `surface`/`surface_alt`/`overlay` are
/// surfaces 1–3, `fg` is text, `muted` text-muted, `control` border-strong.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    /// Window background.
    pub bg: Color,
    /// (surface1) Panels, sidebars, inputs.
    pub surface: Color,
    /// (surface2) Raised areas: button faces, chips, the drag label.
    pub surface_alt: Color,
    /// (surface3) Overlays: menus, popovers, dialogs, toasts, tooltips.
    pub overlay: Color,
    /// (text) Normal text.
    pub fg: Color,
    /// (text_muted) Secondary text and scrollbar thumbs (still 4.5:1).
    pub muted: Color,
    /// Disabled text and icons (3:1; WCAG exempts disabled controls).
    pub text_disabled: Color,
    /// Brand / primary-action colour.
    pub accent: Color,
    /// Text drawn on top of `accent`.
    pub accent_fg: Color,
    /// The keyboard focus ring (3:1 on every surface).
    pub focus: Color,
    /// Dividers and subtle outlines.
    pub border: Color,
    /// (border_strong) Outline of inputs, checkboxes and buttons: at least
    /// 3:1 on `bg` and `surface` (WCAG 1.4.11), where `border` may be a
    /// subtle divider.
    pub control: Color,
    /// Selected rows, highlighted menu items, selected text.
    pub selection: Color,
    /// Hovered rows and ghost buttons.
    pub hover: Color,
    /// Errors and destructive actions.
    pub danger: Color,
    /// Success / OK state.
    pub success: Color,
    /// Warnings.
    pub warning: Color,
    /// Informational state.
    pub info: Color,
    /// Translucent layer behind modal dialogs.
    pub scrim: Color,
}

impl Palette {
    /// The text colour with the best contrast on `bg` among `fg`,
    /// `accent_fg` and `bg` (for labels on coloured fills).
    pub fn text_on(&self, bg: Color) -> Color {
        [self.fg, self.accent_fg, self.bg].into_iter().max_by(|a, b| a.contrast(bg).total_cmp(&b.contrast(bg))).unwrap_or(self.fg)
    }

    /// A soft fill of `tone` over `on` (badges, tinted backgrounds).
    pub fn tint(&self, tone: Color, on: Color) -> Color {
        on.mix(tone, TINT)
    }
}

/// The full look of the interface: palette plus sizes (logical pixels).
#[derive(Debug, Clone, PartialEq)]
pub struct Style {
    pub palette: Palette,
    /// Body size of the proportional UI font (the type scale follows it).
    pub font_px: f32,
    /// Size of the monospace font.
    pub mono_px: f32,
    /// Corner radius of buttons and inputs (`tokens::RADIUS_SM`).
    pub radius: f32,
    /// Gap between neighbouring widgets.
    pub spacing: i32,
    /// Inner padding of widgets.
    pub padding: i32,
    /// Interface zoom: multiplies the display scale for layout and text
    /// alike, so rows grow with their text.
    pub zoom: f32,
    /// Skip animations (the OS "show animations" setting is off): pulses
    /// become static and transitions instant.
    pub reduce_motion: bool,
}

impl Style {
    /// Size of `role` in logical pixels.
    pub fn text_px(&self, role: TextRole) -> f32 {
        role.base_px() * self.font_px / BODY_PX
    }

    /// Sizes shared by the built-in styles, with `palette`.
    fn with_palette(palette: Palette) -> Self {
        Style {
            palette,
            font_px: BODY_PX,
            mono_px: 14.0,
            radius: tokens::RADIUS_SM,
            spacing: tokens::SPACE_SM,
            padding: tokens::SPACE_SM,
            zoom: 1.0,
            reduce_motion: false,
        }
    }

    /// The built-in dark theme at 13px text and zoom 1.
    pub fn dark_default() -> Self {
        Self::with_palette(Palette {
            bg: Color::hex(0x1b1c24),
            surface: Color::hex(0x22242e),
            surface_alt: Color::hex(0x2c2e3a),
            overlay: Color::hex(0x2a2c38),
            fg: Color::hex(0xe6e7ef),
            muted: Color::hex(0xa9adc2),
            text_disabled: Color::hex(0x777b92),
            accent: Color::hex(0x7c8cff),
            accent_fg: Color::hex(0x10121a),
            focus: Color::hex(0x8e9bff),
            border: Color::hex(0x343746),
            control: Color::hex(0x777b96),
            selection: Color::hex(0x343b66),
            hover: Color::hex(0x292b37),
            danger: Color::hex(0xff8a8a),
            success: Color::hex(0x5fd38d),
            warning: Color::hex(0xf2c06b),
            info: Color::hex(0x74bcff),
            scrim: Color::rgba(6, 7, 12, 150),
        })
    }

    /// The built-in light theme.
    pub fn light_default() -> Self {
        Self::with_palette(Palette {
            bg: Color::hex(0xf1f2f6),
            surface: Color::hex(0xf9fafc),
            surface_alt: Color::hex(0xe7e9ef),
            overlay: Color::hex(0xfcfcfd),
            fg: Color::hex(0x1d2130),
            muted: Color::hex(0x50566a),
            text_disabled: Color::hex(0x7a8092),
            accent: Color::hex(0x3653d4),
            accent_fg: Color::hex(0xfafbff),
            focus: Color::hex(0x3653d4),
            border: Color::hex(0xd9dce5),
            control: Color::hex(0x7a8092),
            selection: Color::hex(0xd5dcf8),
            hover: Color::hex(0xe9ebf1),
            danger: Color::hex(0xa31f24),
            success: Color::hex(0x1a6230),
            warning: Color::hex(0x6b4600),
            info: Color::hex(0x164f9c),
            scrim: Color::rgba(16, 18, 28, 100),
        })
    }

    /// The built-in high-contrast theme (7:1 text).
    pub fn high_contrast() -> Self {
        Self::with_palette(Palette {
            bg: Color::hex(0x0b0b0b),
            surface: Color::hex(0x101010),
            surface_alt: Color::hex(0x1c1c1c),
            overlay: Color::hex(0x141414),
            fg: Color::hex(0xfafafa),
            muted: Color::hex(0xcdcdcd),
            text_disabled: Color::hex(0x8c8c8c),
            accent: Color::hex(0xffd700),
            accent_fg: Color::hex(0x0b0b0b),
            focus: Color::hex(0xffd700),
            border: Color::hex(0x8a8a8a),
            control: Color::hex(0xc8c8c8),
            selection: Color::hex(0x1f3170),
            hover: Color::hex(0x1e1e1e),
            danger: Color::hex(0xffb0b0),
            success: Color::hex(0x6cff6c),
            warning: Color::hex(0xffd700),
            info: Color::hex(0xa6d4ff),
            scrim: Color::rgba(0, 0, 0, 190),
        })
    }
}

impl Default for Style {
    /// Same as `Style::dark_default`.
    fn default() -> Self {
        Self::dark_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every built-in style meets WCAG on every surface its roles are drawn
    /// on: text and muted text 4.5:1 (7:1 high contrast), status colours as
    /// text and on their tinted badges, disabled text / outlines / focus
    /// ring 3:1, labels on accent and danger fills. No pure black or white.
    #[test]
    fn builtin_styles_meet_contrast() {
        let mut bad = vec![];
        let styles = [("dark", Style::dark_default(), 4.5), ("light", Style::light_default(), 4.5), ("hc", Style::high_contrast(), 7.0)];
        for (name, s, text) in styles {
            let p = s.palette;
            let surfaces = [("bg", p.bg), ("surface", p.surface), ("raised", p.surface_alt), ("overlay", p.overlay), ("hover", p.hover)];
            let mut checks: Vec<(String, f32, f32)> = vec![];
            for (on, bg) in surfaces.iter().copied().chain([("selection", p.selection)]) {
                checks.push((format!("fg/{on}"), p.fg.contrast(bg), text));
                checks.push((format!("muted/{on}"), p.muted.contrast(bg), text));
            }
            for (on, bg) in surfaces {
                for (what, c) in [("danger", p.danger), ("warning", p.warning), ("success", p.success), ("info", p.info)] {
                    checks.push((format!("{what}/{on}"), c.contrast(bg), text));
                    checks.push((format!("{what}-badge/{on}"), c.contrast(p.tint(c, bg)), text));
                }
                checks.push((format!("disabled/{on}"), p.text_disabled.contrast(bg), 3.0));
                checks.push((format!("focus/{on}"), p.focus.contrast(bg), 3.0));
                checks.push((format!("control/{on}"), p.control.contrast(bg), 3.0));
            }
            checks.push(("focus/selection".into(), p.focus.contrast(p.selection), 3.0));
            checks.push(("accent_fg/accent".into(), p.accent_fg.contrast(p.accent), text));
            checks.push(("label/danger".into(), p.text_on(p.danger).contrast(p.danger), text));
            for (what, got, min) in checks {
                if got < min {
                    bad.push(format!("{name} {what}: {got:.2} < {min}"));
                }
            }
            for c in [p.bg, p.surface, p.surface_alt, p.overlay, p.fg] {
                assert!(c != Color::BLACK && c != Color::WHITE, "{name}: pure {c:?}");
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }

    /// The type scale follows the configured body size.
    #[test]
    fn type_scale_follows_body_size() {
        let mut s = Style::dark_default();
        assert_eq!(s.text_px(TextRole::Display), 22.0);
        s.font_px = 26.0;
        assert_eq!(s.text_px(TextRole::Caption), 24.0);
        assert_eq!(TextRole::Title.font(), FontKind::UiSemibold);
    }
}
