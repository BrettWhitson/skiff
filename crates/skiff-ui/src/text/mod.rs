//! Text: glyph rasterization behind a trait, font stacks, caching, layout.
//!
//! `GlyphRasterizer` hides the font library; `FontStack` adds fallbacks
//! and a "tofu" box; `Fonts` holds the UI, semibold UI and monospace
//! stacks (semibold is a real face when one is installed, else the UI font
//! emboldened by `Emboldened`); `cache`
//! keeps rasterized glyphs; `layout` measures and draws single lines;
//! `system` finds installed fonts.

pub mod cache;
pub mod font;
pub mod layout;
pub mod system;

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result, bail};

/// A rasterized glyph. `xmin`/`ymin` are the bitmap's offset from the pen
/// position on the baseline (`ymin` is the bottom edge, positive = up).
#[derive(Debug, Clone, PartialEq)]
pub struct GlyphBitmap {
    /// Bitmap width in pixels.
    pub width: usize,
    /// Bitmap height in pixels.
    pub height: usize,
    pub xmin: i32,
    pub ymin: i32,
    /// How far the pen moves after this glyph.
    pub advance: f32,
    /// One byte of coverage (0..=255) per pixel, row-major, `width * height`.
    pub coverage: Vec<u8>,
}

/// Vertical metrics of a font at one size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineMetrics {
    /// Height above the baseline.
    pub ascent: f32,
    /// Negative: distance below the baseline.
    pub descent: f32,
    /// Extra space between lines.
    pub line_gap: f32,
}

impl LineMetrics {
    /// Line-box height: ascent, depth below the baseline, and gap.
    pub fn height(&self) -> f32 {
        self.ascent - self.descent + self.line_gap
    }
}

/// Anything that can turn characters into coverage masks. fontdue is one
/// implementation; the rest of the toolkit only sees this trait.
pub trait GlyphRasterizer: Send + Sync {
    /// The font has a real glyph for `c`.
    fn has_glyph(&self, c: char) -> bool;
    /// Horizontal advance of `c` at `px` pixels.
    fn advance(&self, c: char, px: f32) -> f32;
    /// Coverage mask for `c` at `px` pixels.
    fn rasterize(&self, c: char, px: f32) -> GlyphBitmap;
    /// Vertical metrics at `px` pixels.
    fn line_metrics(&self, px: f32) -> LineMetrics;
}

/// Which of the two font stacks text uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontKind {
    /// Proportional interface font.
    Ui,
    /// The interface font's semibold weight (titles, headings, names).
    UiSemibold,
    /// Monospace font (terminal, code).
    Mono,
}

/// Font index used for the "tofu" box drawn when no font has a glyph.
pub const TOFU: usize = usize::MAX;

/// A primary font plus fallbacks tried in order.
#[derive(Clone)]
pub struct FontStack {
    /// Used for line metrics and the tofu box.
    pub primary: Arc<dyn GlyphRasterizer>,
    /// Tried in order for chars the primary lacks.
    pub fallbacks: Vec<Arc<dyn GlyphRasterizer>>,
}

impl FontStack {
    /// A stack with no fallbacks.
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

    /// Rasterizes `c` with the font `resolve` picked (or draws tofu).
    pub fn rasterize(&self, index: usize, c: char, px: f32) -> GlyphBitmap {
        match index {
            0 => self.primary.rasterize(c, px),
            TOFU => self.tofu(px),
            i => self.fallbacks[i - 1].rasterize(c, px),
        }
    }

    /// Line metrics come from the primary font alone.
    pub fn line_metrics(&self, px: f32) -> LineMetrics {
        self.primary.line_metrics(px)
    }

    /// The same stack with every font synthetically emboldened.
    pub fn emboldened(&self) -> FontStack {
        let e = |f: &Arc<dyn GlyphRasterizer>| Arc::new(Emboldened(f.clone())) as Arc<dyn GlyphRasterizer>;
        FontStack { primary: e(&self.primary), fallbacks: self.fallbacks.iter().map(e).collect() }
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

/// The UI, semibold UI and monospace font stacks.
#[derive(Clone)]
pub struct Fonts {
    pub ui: FontStack,
    /// Semibold UI text; `ui` emboldened unless a real face was loaded.
    pub ui_semibold: FontStack,
    /// `ui_semibold` is a real semibold (or bold) face, not emboldened.
    pub semibold_is_real: bool,
    pub mono: FontStack,
}

impl Fonts {
    /// Fonts from two prepared stacks; semibold is `ui` emboldened.
    pub fn new(ui: FontStack, mono: FontStack) -> Self {
        let ui_semibold = ui.emboldened();
        Self { ui, ui_semibold, semibold_is_real: false, mono }
    }

    /// The same fonts with `stack` as the real semibold face.
    pub fn with_semibold(mut self, stack: FontStack) -> Self {
        self.ui_semibold = stack;
        self.semibold_is_real = true;
        self
    }

    /// The stack for `kind`.
    pub fn stack(&self, kind: FontKind) -> &FontStack {
        match kind {
            FontKind::Ui => &self.ui,
            FontKind::UiSemibold => &self.ui_semibold,
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
    /// loads. Remaining candidates and symbol fonts become fallbacks, read
    /// only when a character first needs one (startup parses two fonts).
    /// With the system UI font, its installed semibold face (Segoe UI
    /// Semibold, DejaVu Sans Bold) is loaded lazily for `UiSemibold`; an
    /// explicit UI font, or none installed, is emboldened instead.
    pub fn load(ui_path: Option<&Path>, mono_path: Option<&Path>) -> Result<Fonts> {
        // One shared lazy font per symbol file, for both stacks.
        let symbols: Vec<Arc<dyn GlyphRasterizer>> =
            system::fallbacks().into_iter().map(|p| Arc::new(LazyFont::file(p)) as Arc<dyn GlyphRasterizer>).collect();
        // Load the two primaries in parallel.
        let (ui, mono) = std::thread::scope(|s| {
            let ui = s.spawn(|| load_stack(FontKind::Ui, ui_path, &symbols));
            let mono = load_stack(FontKind::Mono, mono_path, &symbols);
            (ui.join().expect("font loading panicked"), mono)
        });
        let fonts = Fonts::new(ui?, mono?);
        let semibold = if ui_path.is_some() { None } else { system::semibold_candidates().into_iter().find(|p| p.is_file()) };
        Ok(match semibold {
            Some(p) => {
                // The real face first; chars it lacks use the emboldened UI stack.
                let mut fallbacks = vec![fonts.ui_semibold.primary.clone()];
                fallbacks.extend(fonts.ui_semibold.fallbacks.iter().cloned());
                let stack = FontStack { primary: Arc::new(LazyFont::file(p)), fallbacks };
                fonts.with_semibold(stack)
            }
            None => fonts,
        })
    }

    /// Monospace cell: the mono font's 'M' advance × its line height.
    pub fn cell_size(&self, px: f32) -> (f32, f32) {
        (self.mono.primary.advance('M', px), layout::line_height(self, FontKind::Mono, px))
    }
}

/// Reads and parses one font file.
fn load_file(path: &Path) -> Result<Arc<dyn GlyphRasterizer>> {
    let bytes = std::fs::read(path).with_context(|| format!("reading font {}", path.display()))?;
    let f = font::FontdueFont::from_bytes(bytes).with_context(|| format!("parsing font {}", path.display()))?;
    Ok(Arc::new(f))
}

/// The stack for `kind`: the explicit font or the first loadable system
/// candidate as primary, other candidates and `symbols` as lazy fallbacks.
fn load_stack(kind: FontKind, explicit: Option<&Path>, symbols: &[Arc<dyn GlyphRasterizer>]) -> Result<FontStack> {
    let mut primary = match explicit {
        Some(p) => Some(load_file(p)?),
        None => None,
    };
    let mut fallbacks: Vec<Arc<dyn GlyphRasterizer>> = vec![];
    for p in system::candidates(kind) {
        if !p.is_file() {
            continue;
        }
        if primary.is_none() {
            primary = load_file(&p).ok();
        } else {
            fallbacks.push(Arc::new(LazyFont::file(p)));
        }
    }
    let Some(primary) = primary else {
        bail!("no {kind:?} font found; set a font path in the config");
    };
    fallbacks.extend(symbols.iter().cloned());
    Ok(FontStack { primary, fallbacks })
}

/// Loads a font when first needed.
type FontLoader = Box<dyn Fn() -> Result<Arc<dyn GlyphRasterizer>> + Send + Sync>;

/// A font read on first use (fallbacks: most text never needs them). A
/// font that fails to load has no glyphs.
pub struct LazyFont {
    load: FontLoader,
    font: OnceLock<Option<Arc<dyn GlyphRasterizer>>>,
}

impl LazyFont {
    /// A font produced by `load` on first use.
    pub fn new(load: FontLoader) -> Self {
        Self { load, font: OnceLock::new() }
    }

    /// A font read from `path` on first use.
    pub fn file(path: PathBuf) -> Self {
        Self::new(Box::new(move || load_file(&path)))
    }

    /// Loads the font the first time (once, even across threads); `None` if loading failed.
    fn get(&self) -> Option<&Arc<dyn GlyphRasterizer>> {
        self.font.get_or_init(|| (self.load)().ok()).as_ref()
    }
}

impl GlyphRasterizer for LazyFont {
    /// False if the font failed to load.
    fn has_glyph(&self, c: char) -> bool {
        self.get().is_some_and(|f| f.has_glyph(c))
    }

    /// 0 if the font failed to load.
    fn advance(&self, c: char, px: f32) -> f32 {
        self.get().map_or(0.0, |f| f.advance(c, px))
    }

    /// An empty bitmap if the font failed to load.
    fn rasterize(&self, c: char, px: f32) -> GlyphBitmap {
        match self.get() {
            Some(f) => f.rasterize(c, px),
            None => GlyphBitmap { width: 0, height: 0, xmin: 0, ymin: 0, advance: 0.0, coverage: vec![] },
        }
    }

    /// All zeros if the font failed to load.
    fn line_metrics(&self, px: f32) -> LineMetrics {
        match self.get() {
            Some(f) => f.line_metrics(px),
            None => LineMetrics { ascent: 0.0, descent: 0.0, line_gap: 0.0 },
        }
    }
}

/// A font made heavier by smearing each glyph right by a fraction of a
/// pixel (synthetic semibold, used when no real semibold face exists).
pub struct Emboldened(pub Arc<dyn GlyphRasterizer>);

impl Emboldened {
    /// How far (0.35..=1 px) glyphs at `px` are smeared.
    fn strength(px: f32) -> f32 {
        (px / 20.0).clamp(0.35, 1.0)
    }
}

impl GlyphRasterizer for Emboldened {
    /// Same glyphs as the inner font.
    fn has_glyph(&self, c: char) -> bool {
        self.0.has_glyph(c)
    }

    /// The inner advance plus the smear width.
    fn advance(&self, c: char, px: f32) -> f32 {
        self.0.advance(c, px) + Self::strength(px)
    }

    /// The inner glyph one pixel wider, each pixel "screen"-blended with
    /// `strength` of its left neighbour.
    fn rasterize(&self, c: char, px: f32) -> GlyphBitmap {
        let g = self.0.rasterize(c, px);
        let s = Self::strength(px);
        if g.width == 0 || g.height == 0 {
            return GlyphBitmap { advance: g.advance + s, ..g };
        }
        let (w, nw) = (g.width, g.width + 1);
        let mut coverage = vec![0u8; nw * g.height];
        for y in 0..g.height {
            let row = &g.coverage[y * w..(y + 1) * w];
            for x in 0..nw {
                let a = if x < w { row[x] as f32 } else { 0.0 };
                let b = if x > 0 { row[x - 1] as f32 * s } else { 0.0 };
                coverage[y * nw + x] = (a + b * (1.0 - a / 255.0)).round().min(255.0) as u8;
            }
        }
        GlyphBitmap { width: nw, coverage, advance: g.advance + s, ..g }
    }

    /// The inner font's metrics.
    fn line_metrics(&self, px: f32) -> LineMetrics {
        self.0.line_metrics(px)
    }
}

/// A deterministic stand-in font (every glyph a 6×10 block, 8px advance;
/// U+10FFFD missing) so toolkit and app tests never depend on installed fonts.
pub struct BlockFont;

impl GlyphRasterizer for BlockFont {
    /// Every char except U+10FFFD.
    fn has_glyph(&self, c: char) -> bool {
        c != '\u{10FFFD}'
    }
    /// Fixed 8px advance.
    fn advance(&self, _: char, _: f32) -> f32 {
        8.0
    }
    /// A 6x10 block at coverage 200, one pixel right of the pen.
    fn rasterize(&self, _: char, _: f32) -> GlyphBitmap {
        GlyphBitmap { width: 6, height: 10, xmin: 1, ymin: 0, advance: 8.0, coverage: vec![200; 60] }
    }
    /// Ascent 12, descent -4, no gap (16px lines).
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
