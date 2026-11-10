//! Rasterized-glyph cache.
//!
//! Glyphs are keyed by font, char and size, so each is rasterized once.

use std::collections::HashMap;

use super::{FontKind, Fonts, GlyphBitmap};

/// Key: (font kind, font index in its stack, char, size in quarter pixels).
type Key = (FontKind, usize, char, u32);

#[derive(Default)]
/// Rasterized glyphs, filled on demand. Not thread-safe: each painting
/// thread keeps its own.
pub struct GlyphCache {
    map: HashMap<Key, GlyphBitmap>,
}

impl GlyphCache {
    /// The bitmap for `c` at `px` in the font of `kind` that has it (or tofu),
    /// rasterizing on first use.
    pub fn get(&mut self, fonts: &Fonts, kind: FontKind, c: char, px: f32) -> &GlyphBitmap {
        let stack = fonts.stack(kind);
        let index = stack.resolve(c);
        let key = (kind, index, c, (px * 4.0) as u32);
        self.map.entry(key).or_insert_with(|| stack.rasterize(index, c, px))
    }

    /// Number of cached glyphs.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// No glyphs cached.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Drops every glyph (after the fonts change).
    pub fn clear(&mut self) {
        self.map.clear();
    }
}
