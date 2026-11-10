//! Split-pane divider math shared by the front-ends: the shares a divider
//! snaps to, keyboard steps between them, and the smallest pane. The TUI
//! (`skiff-ssh`) and the GUI (`skiff-gui`) both use it, so a split behaves
//! the same in either. A "share" or ratio is the first pane's fraction of
//! the split, from 0.0 to 1.0.

/// Shares a dragged divider snaps to, and divider keys step through.
pub const SNAPS: [f32; 5] = [0.25, 1.0 / 3.0, 0.5, 2.0 / 3.0, 0.75];
/// The smallest share of a split either side keeps.
pub const MIN_RATIO: f32 = 0.1;
/// How far one key press moves a divider past the last snap point.
pub const KEY_STEP: f32 = 0.05;
/// The smallest pane width, in terminal cells (inside any border).
pub const MIN_COLS: u16 = 10;
/// The smallest pane height, in terminal cells (inside any border).
pub const MIN_ROWS: u16 = 3;

/// The snap point nearest `ratio`, if it lies within `within` units of it
/// when the split is `room` units long (px in the GUI, cells in the TUI).
pub fn snap(ratio: f32, room: i32, within: i32) -> Option<f32> {
    SNAPS
        .iter()
        .copied()
        .filter(|s| ((s - ratio) * room as f32).abs() <= within as f32)
        .min_by(|a, b| (a - ratio).abs().total_cmp(&(b - ratio).abs()))
}

/// One keyboard step from `cur`: the next snap point that way, else
/// `KEY_STEP` further (within `MIN_RATIO`).
pub fn step_ratio(cur: f32, forward: bool) -> f32 {
    // A divider sitting on a snap point (give or take rounding) moves past it.
    const EPS: f32 = 0.005;
    let next = if forward { SNAPS.iter().copied().find(|s| *s > cur + EPS) } else { SNAPS.iter().rev().copied().find(|s| *s < cur - EPS) };
    next.unwrap_or(if forward { cur + KEY_STEP } else { cur - KEY_STEP }).clamp(MIN_RATIO, 1.0 - MIN_RATIO)
}
