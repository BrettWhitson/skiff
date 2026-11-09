//! Text: glyph rasterization behind a trait, font stacks, caching, layout.

pub mod cache;
pub mod font;
pub mod layout;
pub mod system;

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, bail};

/// A rasterized glyph. `xmin`/`ymin` are the bitmap's offset from the pen
/// position on the baseline (`ymin` is the bottom edge, positive = up).
#[derive(Debug, Clone, PartialEq)]
pub struct GlyphBitmap {
    pub width: usize,
    pub height: usize,
    pub xmin: i32,
    pub ymin: i32,
    pub advance: f32,
    pub coverage: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineMetrics {
    pub ascent: f32,
    /// Negative: distance below the baseline.
    pub descent: f32,
    pub line_gap: f32,
}

impl LineMetrics {
    pub fn height(&self) -> f32 {
        self.ascent - self.descent + self.line_gap
    }
}

/// Anything that can turn characters into coverage masks. fontdue is one
/// implementation; the rest of the toolkit only sees this trait.
pub trait GlyphRasterizer: Send + Sync {
    fn has_glyph(&self, c: char) -> bool;
    fn advance(&self, c: char, px: f32) -> f32;
    fn rasterize(&self, c: char, px: f32) -> GlyphBitmap;
    fn line_metrics(&self, px: f32) -> LineMetrics;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontKind {
    Ui,
    Mono,
}

/// Font index used for the "tofu" box drawn when no font has a glyph.
pub const TOFU: usize = usize::MAX;

/// A primary font plus fallbacks tried in order.
#[derive(Clone)]
pub struct FontStack {
    pub primary: Arc<dyn GlyphRasterizer>,
    pub fallbacks: Vec<Arc<dyn GlyphRasterizer>>,
}

impl FontStack {
    pub fn new(primary: Arc<dyn GlyphRasterizer>) -> Self {
        Self { primary, fallbacks: vec![] }
    }

    /// Which font draws `c`: 0 = primary, 1.. = fallbacks, `TOFU` = none.
    pub fn resolve(&self, c: char) -> usize {
        if self.primary.has_glyph(c) {
            return 0;
        }
        self.fallbacks.iter().position(|f| f.has_glyph(c)).map_or(TOFU, |i| i + 1)
    }

    pub fn rasterize(&self, index: usize, c: char, px: f32) -> GlyphBitmap {
        match index {
            0 => self.primary.rasterize(c, px),
            TOFU => self.tofu(px),
            i => self.fallbacks[i - 1].rasterize(c, px),
        }
    }

    pub fn line_metrics(&self, px: f32) -> LineMetrics {
        self.primary.line_metrics(px)
    }

    /// A hollow box as wide as the primary font's '0'.
    fn tofu(&self, px: f32) -> GlyphBitmap {
        let advance = self.primary.advance('0', px).max(1.0);
        let m = self.primary.line_metrics(px);
        let w = ((advance * 0.8).round() as usize).max(3);
        let h = ((m.ascent * 0.8).round() as usize).max(3);
        let mut coverage = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..w {
                if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
                    coverage[y * w + x] = 255;
                }
            }
        }
        let xmin = ((advance - w as f32) / 2.0).round() as i32;
        GlyphBitmap { width: w, height: h, xmin, ymin: 0, advance, coverage }
    }
}

pub struct Fonts {
    pub ui: FontStack,
    pub mono: FontStack,
}

impl Fonts {
    pub fn new(ui: FontStack, mono: FontStack) -> Self {
        Self { ui, mono }
    }

    pub fn stack(&self, kind: FontKind) -> &FontStack {
        match kind {
            FontKind::Ui => &self.ui,
            FontKind::Mono => &self.mono,
        }
    }

    /// Embedded fonts (for bundling later). Each falls back to the other.
    pub fn from_bytes(ui: Vec<u8>, mono: Vec<u8>) -> Result<Fonts> {
        let ui: Arc<dyn GlyphRasterizer> = Arc::new(font::FontdueFont::from_bytes(ui)?);
        let mono: Arc<dyn GlyphRasterizer> = Arc::new(font::FontdueFont::from_bytes(mono)?);
        let mut ui_stack = FontStack::new(ui.clone());
        ui_stack.fallbacks.push(mono.clone());
        let mut mono_stack = FontStack::new(mono);
        mono_stack.fallbacks.push(ui);
        Ok(Fonts::new(ui_stack, mono_stack))
    }

    /// An explicit path wins; otherwise the first system candidate that
    /// loads. Remaining candidates and symbol fonts become fallbacks.
    pub fn load(ui_path: Option<&Path>, mono_path: Option<&Path>) -> Result<Fonts> {
        let ui = load_stack(FontKind::Ui, ui_path)?;
        let mono = load_stack(FontKind::Mono, mono_path)?;
        Ok(Fonts::new(ui, mono))
    }

    /// Monospace cell: the mono font's 'M' advance × its line height.
    pub fn cell_size(&self, px: f32) -> (f32, f32) {
        (self.mono.primary.advance('M', px), layout::line_height(self, FontKind::Mono, px))
    }
}

fn load_file(path: &Path) -> Result<Arc<dyn GlyphRasterizer>> {
    let bytes = std::fs::read(path).with_context(|| format!("reading font {}", path.display()))?;
    let f = font::FontdueFont::from_bytes(bytes).with_context(|| format!("parsing font {}", path.display()))?;
    Ok(Arc::new(f))
}

fn load_stack(kind: FontKind, explicit: Option<&Path>) -> Result<FontStack> {
    let mut fonts: Vec<Arc<dyn GlyphRasterizer>> = vec![];
    if let Some(p) = explicit {
        fonts.push(load_file(p)?);
    }
    for p in system::candidates(kind).into_iter().chain(system::fallbacks()) {
        if p.is_file() {
            if let Ok(f) = load_file(&p) {
                fonts.push(f);
            }
        }
    }
    if fonts.is_empty() {
        bail!("no {kind:?} font found; set a font path in the config");
    }
    let primary = fonts.remove(0);
    Ok(FontStack { primary, fallbacks: fonts })
}

/// A deterministic stand-in font (every glyph a 6×10 block, 8px advance;
/// U+10FFFD missing) so toolkit and app tests never depend on installed fonts.
pub struct BlockFont;

impl GlyphRasterizer for BlockFont {
    fn has_glyph(&self, c: char) -> bool {
        c != '\u{10FFFD}'
    }
    fn advance(&self, _: char, _: f32) -> f32 {
        8.0
    }
    fn rasterize(&self, _: char, _: f32) -> GlyphBitmap {
        GlyphBitmap { width: 6, height: 10, xmin: 1, ymin: 0, advance: 8.0, coverage: vec![200; 60] }
    }
    fn line_metrics(&self, _: f32) -> LineMetrics {
        LineMetrics { ascent: 12.0, descent: -4.0, line_gap: 0.0 }
    }
}

/// `Fonts` built from `BlockFont` (for tests).
pub fn test_fonts() -> Fonts {
    let f: Arc<dyn GlyphRasterizer> = Arc::new(BlockFont);
    Fonts::new(FontStack::new(f.clone()), FontStack::new(f))
}

#[cfg(test)]
mod tests;
