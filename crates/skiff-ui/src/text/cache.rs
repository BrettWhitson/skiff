//! Rasterized-glyph cache.

use std::collections::HashMap;

use super::{FontKind, Fonts, GlyphBitmap};

/// Key: (font kind, font index in its stack, char, size in quarter pixels).
type Key = (FontKind, usize, char, u32);

#[derive(Default)]
pub struct GlyphCache {
    map: HashMap<Key, GlyphBitmap>,
}

impl GlyphCache {
    pub fn get(&mut self, fonts: &Fonts, kind: FontKind, c: char, px: f32) -> &GlyphBitmap {
        let stack = fonts.stack(kind);
        let index = stack.resolve(c);
        let key = (kind, index, c, (px * 4.0) as u32);
        self.map.entry(key).or_insert_with(|| stack.rasterize(index, c, px))
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn clear(&mut self) {
        self.map.clear();
    }
}
