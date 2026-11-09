//! fontdue-backed `GlyphRasterizer`.

use anyhow::{Result, anyhow};

use super::{GlyphBitmap, GlyphRasterizer, LineMetrics};

pub struct FontdueFont {
    font: fontdue::Font,
}

impl FontdueFont {
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        let font = fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).map_err(|e| anyhow!(e))?;
        Ok(Self { font })
    }
}

impl GlyphRasterizer for FontdueFont {
    fn has_glyph(&self, c: char) -> bool {
        self.font.lookup_glyph_index(c) != 0
    }

    fn advance(&self, c: char, px: f32) -> f32 {
        self.font.metrics(c, px).advance_width
    }

    fn rasterize(&self, c: char, px: f32) -> GlyphBitmap {
        let (m, coverage) = self.font.rasterize(c, px);
        GlyphBitmap { width: m.width, height: m.height, xmin: m.xmin, ymin: m.ymin, advance: m.advance_width, coverage }
    }

    fn line_metrics(&self, px: f32) -> LineMetrics {
        match self.font.horizontal_line_metrics(px) {
            Some(m) => LineMetrics { ascent: m.ascent, descent: m.descent, line_gap: m.line_gap },
            None => LineMetrics { ascent: px * 0.8, descent: -px * 0.2, line_gap: 0.0 },
        }
    }
}
