//! The toolkit's color type: 8-bit sRGB with straight alpha, plus the
//! conversions to and from the `0x00RRGGBB` pixels the framebuffer uses.

/// An sRGB color with straight (non-premultiplied) alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    /// Opacity: 0 is fully transparent, 255 fully opaque.
    pub a: u8,
}

impl Color {
    pub const TRANSPARENT: Color = Color { r: 0, g: 0, b: 0, a: 0 };
    pub const BLACK: Color = Color::rgb(0, 0, 0);
    pub const WHITE: Color = Color::rgb(255, 255, 255);

    /// An opaque color from its three channels.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// A color with explicit alpha.
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// From `0xRRGGBB`.
    pub const fn hex(v: u32) -> Self {
        Self::rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
    }

    /// The same color with its alpha replaced by `a`.
    pub const fn with_alpha(self, a: u8) -> Self {
        Self { a, ..self }
    }

    /// `0x00RRGGBB`, the pixel format softbuffer expects.
    pub const fn to_u32(self) -> u32 {
        ((self.r as u32) << 16) | ((self.g as u32) << 8) | self.b as u32
    }

    /// An opaque color from a `0x00RRGGBB` pixel (the top byte is ignored).
    pub const fn from_u32(v: u32) -> Self {
        Self::rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
    }

    /// WCAG relative luminance (0 black .. 1 white), ignoring alpha.
    pub fn luminance(self) -> f32 {
        let lin = |v: u8| {
            let c = v as f32 / 255.0;
            if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * lin(self.r) + 0.7152 * lin(self.g) + 0.0722 * lin(self.b)
    }

    /// WCAG contrast ratio with `other` (1..=21), ignoring alpha.
    pub fn contrast(self, other: Color) -> f32 {
        let (a, b) = (self.luminance(), other.luminance());
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    /// This (possibly translucent) colour composited over opaque `bg`.
    pub fn over(self, bg: Color) -> Color {
        bg.mix(self.with_alpha(bg.a), self.a as f32 / 255.0)
    }

    /// Linear mix towards `other` by `t` (0..=1), ignoring alpha.
    pub fn mix(self, other: Color, t: f32) -> Color {
        let t = t.clamp(0.0, 1.0);
        let m = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Color { r: m(self.r, other.r), g: m(self.g, other.g), b: m(self.b, other.b), a: self.a }
    }
}
