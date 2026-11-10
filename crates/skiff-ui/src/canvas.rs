//! A CPU pixel buffer (`0x00RRGGBB`) with clipping and alpha blending.

use crate::color::Color;
use crate::geom::Rect;

/// A pixel buffer that every drawing primitive ends up on; the shell copies
/// it to the window.
pub struct Canvas {
    pub width: i32,
    pub height: i32,
    /// Row-major `0x00RRGGBB` pixels, `width * height` of them.
    pub pixels: Vec<u32>,
    /// Clip stack; each entry is already intersected with the ones below it.
    clips: Vec<Rect>,
    /// Row of the full picture this canvas's first row is (a band of a
    /// larger canvas, painted on another thread). Coordinates stay global.
    y0: i32,
}

/// Blends `c` over `dst` with extra coverage `cov` (0..=255).
#[inline]
fn blend(dst: u32, c: Color, cov: u32) -> u32 {
    let a = c.a as u32 * cov / 255;
    if a == 0 {
        return dst;
    }
    if a == 255 {
        return c.to_u32();
    }
    // Per channel: d*(1-a) + s*a in 0..=255 integer math, rounded.
    let mix = |d: u32, s: u32| (d * (255 - a) + s * a + 127) / 255;
    let (dr, dg, db) = ((dst >> 16) & 0xff, (dst >> 8) & 0xff, dst & 0xff);
    (mix(dr, c.r as u32) << 16) | (mix(dg, c.g as u32) << 8) | mix(db, c.b as u32)
}

/// Coverage (0..=1) of the pixel at (px, py) by a rounded rect.
fn rounded_coverage(px: i32, py: i32, r: Rect, radius: f32) -> f32 {
    let (fx, fy) = (px as f32 + 0.5, py as f32 + 0.5);
    // Distance from the pixel center to the rect shrunk by `radius`; within
    // `radius` is inside, with a one-pixel antialiased edge.
    let (x0, y0) = (r.x as f32 + radius, r.y as f32 + radius);
    let (x1, y1) = (r.right() as f32 - radius, r.bottom() as f32 - radius);
    let cx = fx.clamp(x0, x1.max(x0));
    let cy = fy.clamp(y0, y1.max(y0));
    let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
    (radius + 0.5 - d).clamp(0.0, 1.0)
}

impl Canvas {
    /// A black canvas; negative sizes become 0.
    pub fn new(width: i32, height: i32) -> Self {
        let (width, height) = (width.max(0), height.max(0));
        Self { width, height, pixels: vec![0; (width * height) as usize], clips: vec![], y0: 0 }
    }

    /// Rows `y0..y0 + height` of a `width`-wide picture.
    pub fn band(width: i32, height: i32, y0: i32) -> Self {
        let mut c = Self::new(width, height);
        c.y0 = y0;
        c
    }

    /// Reuses this canvas (and its allocation) as another band.
    pub fn set_band(&mut self, width: i32, height: i32, y0: i32) {
        if (self.width, self.height) != (width.max(0), height.max(0)) {
            self.resize(width, height);
        }
        self.clips.clear();
        self.y0 = y0;
    }

    /// Changes the size; contents are cleared to black and clips dropped.
    pub fn resize(&mut self, width: i32, height: i32) {
        let (width, height) = (width.max(0), height.max(0));
        self.width = width;
        self.height = height;
        self.pixels.clear();
        self.pixels.resize((width * height) as usize, 0);
        self.clips.clear();
    }

    /// The area this canvas covers, in picture coordinates (`y0`-offset for a band).
    pub fn bounds(&self) -> Rect {
        Rect::new(0, self.y0, self.width, self.height)
    }

    #[inline]
    /// Offset into `pixels` of picture coordinates (x, y).
    fn index(&self, x: i32, y: i32) -> usize {
        ((y - self.y0) * self.width + x) as usize
    }

    /// The current clip (canvas bounds intersected with every pushed clip).
    pub fn clip(&self) -> Rect {
        self.clips.last().copied().unwrap_or(self.bounds())
    }

    /// Narrows the clip to `r` (within the current clip) until `pop_clip`.
    pub fn push_clip(&mut self, r: Rect) {
        let c = self.clip().intersect(r).unwrap_or_default();
        self.clips.push(c);
    }

    /// Restores the clip from before the last `push_clip`.
    pub fn pop_clip(&mut self) {
        self.clips.pop();
    }

    /// The pixel at (x, y); panics outside the canvas.
    pub fn pixel(&self, x: i32, y: i32) -> u32 {
        self.pixels[self.index(x, y)]
    }

    /// Fills every pixel with `c`, ignoring clip and alpha.
    pub fn clear(&mut self, c: Color) {
        self.pixels.fill(c.to_u32());
    }

    /// Fills `r` (clipped) with `c`, blending if it is translucent.
    pub fn fill_rect(&mut self, r: Rect, c: Color) {
        let Some(r) = r.intersect(self.clip()) else { return };
        if c.a == 0 {
            return;
        }
        for y in r.y..r.bottom() {
            let row = self.index(0, y);
            let span = &mut self.pixels[row + r.x as usize..row + r.right() as usize];
            if c.a == 255 {
                span.fill(c.to_u32());
            } else {
                for p in span {
                    *p = blend(*p, c, 255);
                }
            }
        }
    }

    /// Fills `r` with antialiased corners of `radius`; plain fill under half a pixel.
    pub fn fill_rounded_rect(&mut self, r: Rect, radius: f32, c: Color) {
        let radius = radius.min(r.w as f32 / 2.0).min(r.h as f32 / 2.0);
        if radius < 0.5 {
            return self.fill_rect(r, c);
        }
        let band = radius.ceil() as i32;
        // Straight middle band: plain fill.
        self.fill_rect(Rect::new(r.x, r.y + band, r.w, r.h - 2 * band), c);
        let Some(clip) = r.intersect(self.clip()) else { return };
        // Top and bottom bands hold the corners: blend by per-pixel coverage.
        for y in (r.y..r.y + band).chain(r.bottom() - band..r.bottom()) {
            if y < clip.y || y >= clip.bottom() {
                continue;
            }
            for x in clip.x..clip.right() {
                let cov = rounded_coverage(x, y, r, radius);
                if cov > 0.0 {
                    let i = self.index(x, y);
                    self.pixels[i] = blend(self.pixels[i], c, (cov * 255.0) as u32);
                }
            }
        }
    }

    /// Draws a `width`-pixel outline inside `r`, with rounded corners when
    /// `radius` is at least half a pixel.
    pub fn stroke_rect(&mut self, r: Rect, width: i32, radius: f32, c: Color) {
        if r.is_empty() || width <= 0 {
            return;
        }
        // Square corners: four filled strips.
        if radius < 0.5 {
            let w = width.min(r.w).min(r.h);
            self.fill_rect(Rect::new(r.x, r.y, r.w, w), c);
            self.fill_rect(Rect::new(r.x, r.bottom() - w, r.w, w), c);
            self.fill_rect(Rect::new(r.x, r.y + w, w, r.h - 2 * w), c);
            self.fill_rect(Rect::new(r.right() - w, r.y + w, w, r.h - 2 * w), c);
            return;
        }
        // Rounded: coverage of the outer shape minus coverage of the inset shape.
        let radius = radius.min(r.w as f32 / 2.0).min(r.h as f32 / 2.0);
        let inner = r.inset(width);
        let inner_radius = (radius - width as f32).max(0.0);
        let Some(clip) = r.intersect(self.clip()) else { return };
        for y in clip.y..clip.bottom() {
            for x in clip.x..clip.right() {
                let outer = rounded_coverage(x, y, r, radius);
                let inn = if inner.is_empty() { 0.0 } else { rounded_coverage(x, y, inner, inner_radius) };
                let cov = (outer - inn).max(0.0);
                if cov > 0.0 {
                    let i = self.index(x, y);
                    self.pixels[i] = blend(self.pixels[i], c, (cov * 255.0) as u32);
                }
            }
        }
    }

    /// Like `blend_mask`, with each mask row shifted right by
    /// `(h - 1 - row) * shear` pixels (synthetic italic).
    #[allow(clippy::too_many_arguments)]
    pub fn blend_mask_sheared(&mut self, x: i32, y: i32, w: i32, h: i32, mask: &[u8], c: Color, shear: f32) {
        for row in 0..h.max(0) {
            let dx = ((h - 1 - row) as f32 * shear).round() as i32;
            let s = (row * w) as usize;
            self.blend_mask(x + dx, y + row, w, 1, &mask[s..s + w as usize], c);
        }
    }

    /// Blends `c` through an alpha mask (`w`×`h` bytes, row-major) at (x, y).
    pub fn blend_mask(&mut self, x: i32, y: i32, w: i32, h: i32, mask: &[u8], c: Color) {
        let Some(area) = Rect::new(x, y, w, h).intersect(self.clip()) else { return };
        for py in area.y..area.bottom() {
            let mrow = ((py - y) * w) as usize;
            let row = self.index(0, py);
            for px in area.x..area.right() {
                let cov = mask[mrow + (px - x) as usize] as u32;
                if cov != 0 {
                    let i = row + px as usize;
                    self.pixels[i] = blend(self.pixels[i], c, cov);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Splits a pixel into its (r, g, b) bytes.
    fn rgb(v: u32) -> (u8, u8, u8) {
        ((v >> 16) as u8, (v >> 8) as u8, v as u8)
    }

    /// Each channel differs by at most 1 (rounding slack).
    fn close(a: u32, b: u32) -> bool {
        let (a, b) = (rgb(a), rgb(b));
        (a.0 as i32 - b.0 as i32).abs() <= 1 && (a.1 as i32 - b.1 as i32).abs() <= 1 && (a.2 as i32 - b.2 as i32).abs() <= 1
    }

    /// Fills are clipped to the canvas and to the clip stack.
    #[test]
    fn fill_rect_clips_to_bounds_and_clip_stack() {
        let mut c = Canvas::new(10, 10);
        c.fill_rect(Rect::new(-5, -5, 8, 8), Color::hex(0xff0000));
        assert_eq!(c.pixel(0, 0), 0xff0000);
        assert_eq!(c.pixel(2, 2), 0xff0000);
        assert_eq!(c.pixel(3, 3), 0);
        c.push_clip(Rect::new(5, 5, 2, 2));
        c.fill_rect(Rect::new(0, 0, 10, 10), Color::hex(0x00ff00));
        c.pop_clip();
        assert_eq!(c.pixel(5, 5), 0x00ff00);
        assert_eq!(c.pixel(7, 7), 0);
        assert_eq!(c.pixel(4, 5), 0);
        c.fill_rect(Rect::new(9, 9, 5, 5), Color::hex(0x0000ff));
        assert_eq!(c.pixel(9, 9), 0x0000ff);
    }

    /// Nested clips paint only their intersection.
    #[test]
    fn nested_clips_intersect() {
        let mut c = Canvas::new(10, 10);
        c.push_clip(Rect::new(0, 0, 6, 6));
        c.push_clip(Rect::new(4, 4, 6, 6));
        c.fill_rect(Rect::new(0, 0, 10, 10), Color::WHITE);
        c.pop_clip();
        c.pop_clip();
        assert_eq!(c.pixel(5, 5), 0xffffff);
        assert_eq!(c.pixel(6, 6), 0);
        assert_eq!(c.pixel(3, 3), 0);
    }

    /// Half-transparent red over blue blends to about half each.
    #[test]
    fn alpha_blend_half_red_over_blue() {
        let mut c = Canvas::new(1, 1);
        c.clear(Color::hex(0x0000ff));
        c.fill_rect(Rect::new(0, 0, 1, 1), Color::rgba(255, 0, 0, 0x80));
        assert!(close(c.pixel(0, 0), 0x80007f), "{:06x}", c.pixel(0, 0));
    }

    /// Rounded corners are antialiased: the corner pixel mostly outside, the next one partial.
    #[test]
    fn rounded_rect_corner_is_antialiased() {
        let mut c = Canvas::new(10, 10);
        c.fill_rounded_rect(Rect::new(0, 0, 10, 10), 4.0, Color::WHITE);
        let corner = rgb(c.pixel(0, 0)).0;
        assert!(corner < 128, "corner should be mostly outside, got {corner}");
        let edge = rgb(c.pixel(1, 1)).0;
        assert!(edge > 0 && edge < 255, "edge pixel should be partial, got {edge}");
        assert_eq!(c.pixel(5, 5), 0xffffff);
        assert_eq!(c.pixel(5, 0), 0xffffff);
    }

    /// A stroke paints the border band and leaves the inside untouched.
    #[test]
    fn stroke_rect_draws_only_border() {
        let mut c = Canvas::new(10, 10);
        c.stroke_rect(Rect::new(0, 0, 10, 10), 2, 0.0, Color::WHITE);
        assert_eq!(c.pixel(0, 0), 0xffffff);
        assert_eq!(c.pixel(1, 5), 0xffffff);
        assert_eq!(c.pixel(9, 9), 0xffffff);
        assert_eq!(c.pixel(2, 2), 0);
        assert_eq!(c.pixel(5, 5), 0);
    }

    /// Mask bytes act as coverage, and masks partly off-canvas are clipped.
    #[test]
    fn blend_mask_applies_coverage() {
        let mut c = Canvas::new(4, 1);
        c.blend_mask(1, 0, 2, 1, &[255, 128], Color::WHITE);
        assert_eq!(c.pixel(0, 0), 0);
        assert_eq!(c.pixel(1, 0), 0xffffff);
        assert!(close(c.pixel(2, 0), 0x808080));
        assert_eq!(c.pixel(3, 0), 0);
        // Partly off-canvas masks are clipped, not panicking.
        c.blend_mask(-1, 0, 2, 1, &[255, 255], Color::hex(0xff0000));
        c.blend_mask(3, 0, 3, 1, &[255, 255, 255], Color::hex(0xff0000));
        assert_eq!(c.pixel(0, 0), 0xff0000);
    }

    /// Empty or negative rects draw nothing; a zero-size canvas can be resized.
    #[test]
    fn zero_and_negative_rects_are_noops() {
        let mut c = Canvas::new(4, 4);
        c.fill_rect(Rect::new(1, 1, 0, 3), Color::WHITE);
        c.fill_rect(Rect::new(1, 1, -2, 3), Color::WHITE);
        c.fill_rounded_rect(Rect::new(1, 1, 0, 0), 3.0, Color::WHITE);
        c.stroke_rect(Rect::new(1, 1, 0, 0), 1, 0.0, Color::WHITE);
        assert!(c.pixels.iter().all(|&p| p == 0));
        let mut empty = Canvas::new(0, 0);
        empty.fill_rect(Rect::new(0, 0, 5, 5), Color::WHITE);
        empty.resize(3, 2);
        assert_eq!(empty.pixels.len(), 6);
    }
}
