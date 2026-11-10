//! Rasterized-glyph cache.
//!
//! Glyphs are keyed by font, char and size, so each is rasterized once.
//! Icon masks (`crate::icons`) are cached here too, keyed by icon and size.

use std::collections::HashMap;

use super::{FontKind, Fonts, GlyphBitmap};
use crate::icons::{self, Icon};

/// Key: (font kind, font index in its stack, char, size in quarter pixels).
type Key = (FontKind, usize, char, u32);

#[derive(Default)]
/// Rasterized glyphs, filled on demand. Not thread-safe: each painting
/// thread keeps its own.
pub struct GlyphCache {
    map: HashMap<Key, GlyphBitmap>,
    /// Icon coverage masks by (icon, edge length in pixels).
    icons: HashMap<(Icon, i32), Vec<u8>>,
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

    /// The `size`×`size` coverage mask of `icon`, rasterizing on first use.
    pub fn icon(&mut self, icon: Icon, size: i32) -> &[u8] {
        self.icons.entry((icon, size)).or_insert_with(|| icons::rasterize(icon, size))
    }

    /// Number of cached icon masks.
    pub fn icon_count(&self) -> usize {
        self.icons.len()
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
        self.icons.clear();
    }
}
