//! fontdue-backed `GlyphRasterizer`.

use anyhow::{Result, anyhow};

use super::{GlyphBitmap, GlyphRasterizer, LineMetrics};

/// A parsed TrueType / OpenType font rasterized with fontdue.
pub struct FontdueFont {
    font: fontdue::Font,
}

impl FontdueFont {
    /// Parses font file contents (the first face of a collection).
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        let font = fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).map_err(|e| anyhow!(e))?;
        Ok(Self { font })
    }
}

impl GlyphRasterizer for FontdueFont {
    /// Glyph index 0 is the "missing glyph", so anything else is a real glyph.
    fn has_glyph(&self, c: char) -> bool {
        self.font.lookup_glyph_index(c) != 0
    }

    /// Horizontal advance of `c` at `px`.
    fn advance(&self, c: char, px: f32) -> f32 {
        self.font.metrics(c, px).advance_width
    }

    /// Rasterizes `c` at `px` into a coverage mask.
    fn rasterize(&self, c: char, px: f32) -> GlyphBitmap {
        let (m, coverage) = self.font.rasterize(c, px);
        GlyphBitmap { width: m.width, height: m.height, xmin: m.xmin, ymin: m.ymin, advance: m.advance_width, coverage }
    }

    /// The font's ascent / descent / gap at `px`; a 0.8 / 0.2 split of `px`
    /// if the font has no horizontal metrics.
    fn line_metrics(&self, px: f32) -> LineMetrics {
        match self.font.horizontal_line_metrics(px) {
            Some(m) => LineMetrics { ascent: m.ascent, descent: m.descent, line_gap: m.line_gap },
            None => LineMetrics { ascent: px * 0.8, descent: -px * 0.2, line_gap: 0.0 },
        }
    }
}
