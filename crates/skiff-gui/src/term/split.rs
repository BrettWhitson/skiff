//! Split panes: a terminal tab holds one or more panes, each its own
//! session, laid out by a tree of two-way splits (any number of panes).
//!
//! Dividers drag with the mouse (snapping to 25 / 33 / 50 / 67 / 75%; Alt
//! drags freely; a double-click equalizes) and, focused (F6 reaches them),
//! move with the arrow keys from snap point to snap point. Panes keep at
//! least `MIN_COLS` x `MIN_ROWS` cells. A pane's grip (top centre) or Alt+Shift+drag drags it
//! onto another pane's edge or centre (swap) or onto the tab strip (its own
//! tab); a terminal tab dragged off the strip onto a pane joins this tab.
//! Focus moves between panes by click, F6 and the `focus_next_pane`
//! shortcut; each pane is its own focus region and its terminal is named
//! "Terminal: host, pane n of m" for screen readers.
//!
//! Structure: `TermSplit` owns the `Pane`s (each a `TermTab` with a stable
//! `u64` key) and a `Layout` binary tree whose leaves are pane keys and whose
//! inner nodes are `Split { dir, ratio, a, b }`. A node is addressed by its
//! path (`false` = into `a`, `true` = into `b`); `Layout::arrange` turns the
//! tree into pane rects and `Divider`s each frame. The snap points, key step
//! and minimum sizes are shared with the TUI through `skiff_core::panes`.

use skiff_core::hosts::HostEntry;
use skiff_ui::a11y::{A11yAction, Role};
use skiff_ui::geom::{Point, Rect};
use skiff_ui::input::{CursorIcon, Event, Key, Modifiers, MouseButton};
use skiff_ui::ui::{Id, Sense, Ui};
use skiff_ui::widgets::tooltip;

use super::TermTab;
use crate::engine::WakeFn;
use crate::keys::{Action, Keymap};
use crate::theme::GuiTheme;

/// Where a split puts the new pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    /// Side by side, the new pane on the right.
    Right,
    /// Stacked, the new pane below.
    Down,
}

// Shared with the TUI: snap points (25/33/50/67/75%), the 10% minimum share,
// the 5% key step past the last snap point, and the snapping functions.
pub use skiff_core::panes::{KEY_STEP, MIN_RATIO, SNAPS, snap, step_ratio};

/// Divider thickness, logical px.
pub const GAP: i32 = 5;
/// How close (logical px) a dragged divider must come to snap.
pub const SNAP_PX: i32 = 8;
/// The smallest pane width, in terminal cells.
pub const MIN_COLS: i32 = skiff_core::panes::MIN_COLS as i32;
/// The smallest pane height, in terminal cells.
pub const MIN_ROWS: i32 = skiff_core::panes::MIN_ROWS as i32;

/// A side of a pane (where a dropped pane goes, which way a pane moves),
/// or its centre (a drop there swaps, or just shows a dragged tab).
///
/// `Left` / `Right` split side by side and `Top` / `Bottom` stack; the
/// dropped content lands on the named side of the target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    Left,
    Right,
    Top,
    Bottom,
    Center,
}

impl Zone {
    /// The zone of `rect` under `at`: the middle third (both ways) is the
    /// centre, elsewhere the nearest edge.
    pub fn at(rect: Rect, at: Point) -> Zone {
        if rect.w <= 0 || rect.h <= 0 {
            return Zone::Center;
        }
        // Position as fractions of the rect; outside the centre box, the
        // edge with the smallest distance wins.
        let fx = (at.x - rect.x) as f32 / rect.w as f32;
        let fy = (at.y - rect.y) as f32 / rect.h as f32;
        let mid = 1.0 / 3.0..=2.0 / 3.0;
        if mid.contains(&fx) && mid.contains(&fy) {
            return Zone::Center;
        }
        [(fx, Zone::Left), (1.0 - fx, Zone::Right), (fy, Zone::Top), (1.0 - fy, Zone::Bottom)]
            .into_iter()
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map_or(Zone::Center, |(_, z)| z)
    }

    /// The split direction a drop on this side makes.
    pub fn dir(self) -> Option<Dir> {
        match self {
            Zone::Left | Zone::Right => Some(Dir::Right),
            Zone::Top | Zone::Bottom => Some(Dir::Down),
            Zone::Center => None,
        }
    }

    /// Towards the end of its axis (right or down).
    fn forward(self) -> bool {
        matches!(self, Zone::Right | Zone::Bottom)
    }

    /// The part of `rect` a drop here would take (the preview).
    pub fn preview(self, rect: Rect, gap: i32) -> Rect {
        match self.dir() {
            None => rect,
            Some(d) => {
                let (a, _, b) = split_rect(rect, d, 0.5, gap);
                if self.forward() { b } else { a }
            }
        }
    }

    /// Room for a new half here, each at least `min` px (w, h); the centre
    /// always fits (it splits nothing).
    pub fn fits(self, rect: Rect, gap: i32, min: (i32, i32)) -> bool {
        match self.dir() {
            None => true,
            Some(Dir::Right) => rect.w - gap >= 2 * min.0,
            Some(Dir::Down) => rect.h - gap >= 2 * min.1,
        }
    }
}

/// What is being dragged onto panes: a whole terminal tab (by id), or
/// one pane of a tab.
///
/// `Tab(id)` carries the terminal tab's id; `Pane { tab, key }` names the
/// tab the pane is in and its pane key there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneDrag {
    Tab(u64),
    Pane { tab: u64, key: u64 },
}

/// A terminal tab dropped onto pane `target` of the shown tab: the app
/// moves tab `from`'s panes in (or, on the centre, just shows it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TabDrop {
    /// Id of the dragged terminal tab.
    pub from: u64,
    /// Key of the pane it was dropped on.
    pub target: u64,
    /// Which part of that pane: a side (join beside it) or the centre.
    pub zone: Zone,
}

/// The arrangement of a tab's panes (by pane key): a binary tree whose
/// leaves are panes (`Pane(key)`) and whose inner nodes split their area in two.
#[derive(Debug, Clone, PartialEq)]
pub enum Layout {
    Pane(u64),
    /// `a` is left of (or above) `b`; `ratio` is `a`'s share.
    Split {
        dir: Dir,
        ratio: f32,
        a: Box<Layout>,
        b: Box<Layout>,
    },
}

/// A divider as laid out.
#[derive(Debug, Clone, PartialEq)]
pub struct Divider {
    /// The split it belongs to: turns from the root (`false` = into `a`).
    pub path: Vec<bool>,
    /// A key that survives restructuring (unlike `path`): the pane right
    /// after the divider, the first one of the split's second side. Each
    /// pane starts at most one split's second side, so it is unique.
    pub key: u64,
    /// The split's direction (`Right`: a vertical divider between columns).
    pub dir: Dir,
    /// The split's stored share for its first side.
    pub ratio: f32,
    /// The draggable strip between the two sides.
    pub handle: Rect,
    /// The whole area the split divides.
    pub area: Rect,
}

/// `rect` split at `ratio`: the first side, the divider and the second side.
/// The gap comes off first; `ratio` of what is left (rounded) is the first
/// side, and the second side takes the rest, so the three always tile `rect`.
pub fn split_rect(rect: Rect, dir: Dir, ratio: f32, gap: i32) -> (Rect, Rect, Rect) {
    let total = match dir {
        Dir::Right => rect.w,
        Dir::Down => rect.h,
    };
    let gap = gap.clamp(0, total.max(0));
    let first = ((total - gap).max(0) as f32 * ratio).round() as i32;
    let second = (total - first - gap).max(0);
    match dir {
        Dir::Right => (
            Rect::new(rect.x, rect.y, first, rect.h),
            Rect::new(rect.x + first, rect.y, gap, rect.h),
            Rect::new(rect.x + first + gap, rect.y, second, rect.h),
        ),
        Dir::Down => (
            Rect::new(rect.x, rect.y, rect.w, first),
            Rect::new(rect.x, rect.y + first, rect.w, gap),
            Rect::new(rect.x, rect.y + first + gap, rect.w, second),
        ),
    }
}

/// The ratio that puts a divider of `area` under the mouse at `at`: the
/// inverse of `split_rect` with the divider's middle on the pointer, kept
/// within `MIN_RATIO` (0.5 when there is no room at all).
pub fn ratio_at(area: Rect, dir: Dir, gap: i32, at: Point) -> f32 {
    let (pos, start, total) = match dir {
        Dir::Right => (at.x, area.x, area.w),
        Dir::Down => (at.y, area.y, area.h),
    };
    let room = total - gap;
    if room <= 0 {
        return 0.5;
    }
    ((pos - start - gap / 2) as f32 / room as f32).clamp(MIN_RATIO, 1.0 - MIN_RATIO)
}

impl Layout {
    /// The panes' rects (in reading order) and the dividers of `rect`.
    pub fn arrange(&self, rect: Rect, gap: i32) -> (Vec<(u64, Rect)>, Vec<Divider>) {
        let (mut panes, mut dividers) = (vec![], vec![]);
        self.arrange_into(rect, gap, &mut vec![], &mut panes, &mut dividers);
        (panes, dividers)
    }

    /// Recursive half of `arrange`: lays this node out in `rect`, with `path`
    /// the turns to it; a split's divider is pushed before its children's.
    fn arrange_into(&self, rect: Rect, gap: i32, path: &mut Vec<bool>, panes: &mut Vec<(u64, Rect)>, dividers: &mut Vec<Divider>) {
        match self {
            Layout::Pane(k) => panes.push((*k, rect)),
            Layout::Split { dir, ratio, a, b } => {
                let (ra, handle, rb) = split_rect(rect, *dir, *ratio, gap);
                dividers.push(Divider { path: path.clone(), key: b.first_key(), dir: *dir, ratio: *ratio, handle, area: rect });
                path.push(false);
                a.arrange_into(ra, gap, path, panes, dividers);
                path.pop();
                path.push(true);
                b.arrange_into(rb, gap, path, panes, dividers);
                path.pop();
            }
        }
    }

    /// Pane keys in reading order.
    pub fn keys(&self) -> Vec<u64> {
        self.arrange(Rect::default(), 0).0.into_iter().map(|(k, _)| k).collect()
    }

    /// The first pane in reading order (leftmost / topmost leaf).
    fn first_key(&self) -> u64 {
        match self {
            Layout::Pane(k) => *k,
            Layout::Split { a, .. } => a.first_key(),
        }
    }

    /// The last pane in reading order (rightmost / bottommost leaf).
    fn last_key(&self) -> u64 {
        match self {
            Layout::Pane(k) => *k,
            Layout::Split { b, .. } => b.last_key(),
        }
    }

    /// Splits pane `key` in two, `new` after it; false if there is no such pane.
    pub fn split(&mut self, key: u64, dir: Dir, new: u64) -> bool {
        match self {
            Layout::Pane(k) if *k == key => {
                *self = Layout::Split { dir, ratio: 0.5, a: Box::new(Layout::Pane(key)), b: Box::new(Layout::Pane(new)) };
                true
            }
            Layout::Pane(_) => false,
            Layout::Split { a, b, .. } => a.split(key, dir, new) || b.split(key, dir, new),
        }
    }

    /// Removes pane `key` (its neighbor takes the space); returns the pane
    /// that should get focus: the neighbor next to it in reading order.
    pub fn remove(&mut self, key: u64) -> Option<u64> {
        let Layout::Split { a, b, .. } = self else {
            return None;
        };
        // A direct child: the split collapses into the other side (the
        // `Pane(0)` placeholder is dropped with the old node).
        if **a == Layout::Pane(key) {
            let rest = std::mem::replace(&mut **b, Layout::Pane(0));
            let next = rest.first_key();
            *self = rest;
            return Some(next);
        }
        if **b == Layout::Pane(key) {
            let rest = std::mem::replace(&mut **a, Layout::Pane(0));
            let prev = rest.last_key();
            *self = rest;
            return Some(prev);
        }
        a.remove(key).or_else(|| b.remove(key))
    }

    /// The node at `path`.
    fn node(&self, path: &[bool]) -> Option<&Layout> {
        match (self, path.split_first()) {
            (n, None) => Some(n),
            (Layout::Split { a, .. }, Some((false, rest))) => a.node(rest),
            (Layout::Split { b, .. }, Some((true, rest))) => b.node(rest),
            (Layout::Pane(_), _) => None,
        }
    }

    /// The node at `path`, mutably.
    fn node_mut(&mut self, path: &[bool]) -> Option<&mut Layout> {
        match (self, path.split_first()) {
            (n, None) => Some(n),
            (Layout::Split { a, .. }, Some((false, rest))) => a.node_mut(rest),
            (Layout::Split { b, .. }, Some((true, rest))) => b.node_mut(rest),
            (Layout::Pane(_), _) => None,
        }
    }

    /// The share of the split at `path`.
    pub fn ratio(&self, path: &[bool]) -> Option<f32> {
        match self.node(path)? {
            Layout::Split { ratio, .. } => Some(*ratio),
            Layout::Pane(_) => None,
        }
    }

    /// Sets the share of the split at `path` (kept within `MIN_RATIO`).
    pub fn set_ratio(&mut self, path: &[bool], value: f32) {
        if let Some(Layout::Split { ratio, .. }) = self.node_mut(path) {
            *ratio = value.clamp(MIN_RATIO, 1.0 - MIN_RATIO);
        }
    }

    /// Whether pane `key` is somewhere in this tree.
    pub fn contains(&self, key: u64) -> bool {
        match self {
            Layout::Pane(k) => *k == key,
            Layout::Split { a, b, .. } => a.contains(key) || b.contains(key),
        }
    }

    /// The turns from the root to pane `key`.
    fn path_to(&self, key: u64) -> Option<Vec<bool>> {
        match self {
            Layout::Pane(k) => (*k == key).then(Vec::new),
            Layout::Split { a, b, .. } => {
                let (side, rest) = if a.contains(key) { (false, a) } else { (true, b) };
                let mut p = rest.path_to(key)?;
                p.insert(0, side);
                Some(p)
            }
        }
    }

    /// The innermost split along `dir` that pane `key` is in (its path).
    pub fn axis_split(&self, key: u64, dir: Dir) -> Option<Vec<bool>> {
        let path = self.path_to(key)?;
        // Walk the pane's ancestors from the nearest up; first match wins.
        (0..path.len())
            .rev()
            .map(|i| &path[..i])
            .find(|p| matches!(self.node(p), Some(Layout::Split { dir: d, .. }) if *d == dir))
            .map(<[bool]>::to_vec)
    }

    /// Puts `sub` beside pane `target`, on side `zone` of it (half each).
    /// False for the centre or when there is no such pane.
    pub fn insert(&mut self, target: u64, zone: Zone, sub: Layout) -> bool {
        let Some(dir) = zone.dir() else { return false };
        match self {
            // The target leaf becomes a split; Right / Bottom put `sub` second.
            Layout::Pane(k) if *k == target => {
                let (old, new) = (Box::new(Layout::Pane(target)), Box::new(sub));
                let (a, b) = if zone.forward() { (old, new) } else { (new, old) };
                *self = Layout::Split { dir, ratio: 0.5, a, b };
                true
            }
            Layout::Pane(_) => false,
            Layout::Split { a, b, .. } => {
                if a.contains(target) {
                    a.insert(target, zone, sub)
                } else {
                    b.insert(target, zone, sub)
                }
            }
        }
    }

    /// Exchanges the places of panes `x` and `y`.
    pub fn swap(&mut self, x: u64, y: u64) {
        self.map_keys(&|k| {
            if k == x {
                y
            } else if k == y {
                x
            } else {
                k
            }
        });
    }

    /// Rewrites every pane key through `f` (swaps, and renumbering an absorbed tab).
    fn map_keys(&mut self, f: &impl Fn(u64) -> u64) {
        match self {
            Layout::Pane(k) => *k = f(*k),
            Layout::Split { a, b, .. } => {
                a.map_keys(f);
                b.map_keys(f);
            }
        }
    }

    /// How many panes sit in a row along `dir` (splits the other way count
    /// their widest side).
    fn span(&self, dir: Dir) -> u32 {
        match self {
            Layout::Pane(_) => 1,
            Layout::Split { dir: d, a, b, .. } if *d == dir => a.span(dir) + b.span(dir),
            Layout::Split { a, b, .. } => a.span(dir).max(b.span(dir)),
        }
    }

    /// Shares so that panes in a row get the same size: each split's ratio is
    /// its first side's span over the total (e.g. 1 vs 2 panes gives 1/3).
    pub fn equalize(&mut self) {
        if let Layout::Split { dir, ratio, a, b } = self {
            a.equalize();
            b.equalize();
            let (sa, sb) = (a.span(*dir) as f32, b.span(*dir) as f32);
            *ratio = sa / (sa + sb);
        }
    }

    /// Equalizes the split at `path` and those inside it.
    pub fn equalize_at(&mut self, path: &[bool]) {
        if let Some(n) = self.node_mut(path) {
            n.equalize();
        }
    }

    /// The least px this needs along `dir`, each pane at least `min` (w, h):
    /// splits along `dir` add their sides plus the gap, others take the larger.
    fn min_extent(&self, dir: Dir, min: (i32, i32), gap: i32) -> i32 {
        match self {
            Layout::Pane(_) if dir == Dir::Right => min.0,
            Layout::Pane(_) => min.1,
            Layout::Split { dir: d, a, b, .. } if *d == dir => a.min_extent(dir, min, gap) + gap + b.min_extent(dir, min, gap),
            Layout::Split { a, b, .. } => a.min_extent(dir, min, gap).max(b.min_extent(dir, min, gap)),
        }
    }

    /// Moves dividers so every pane of `rect` keeps `min` px (w, h) where
    /// there is room; where there is not, the sides share by their needs.
    /// The view applies it to a copy (layout time only).
    pub fn enforce_min(&mut self, rect: Rect, gap: i32, min: (i32, i32)) {
        let Layout::Split { dir, ratio, a, b } = self else { return };
        let total = if *dir == Dir::Right { rect.w } else { rect.h };
        let room = (total - gap) as f32;
        let (ma, mb) = (a.min_extent(*dir, min, gap) as f32, b.min_extent(*dir, min, gap) as f32);
        // Clamp the ratio so side `a` gets at least `ma` px and side `b` at
        // least `mb`; if both can't fit, split the room in proportion ma : mb.
        if room > 0.0 {
            // Half a px of slack each way: `split_rect` rounds.
            let (lo, hi) = ((ma + 0.5) / room, (room - mb - 0.5) / room);
            *ratio = if lo <= hi { ratio.clamp(lo, hi) } else { ma / (ma + mb).max(1.0) };
        }
        let (ra, _, rb) = split_rect(rect, *dir, *ratio, gap);
        a.enforce_min(ra, gap, min);
        b.enforce_min(rb, gap, min);
    }
}

/// One pane of a terminal tab.
pub struct Pane {
    /// Stable within its tab (the leaf in `Layout`, and part of the widget id).
    pub key: u64,
    /// The terminal shown in this pane.
    pub term: TermTab,
}

/// A terminal tab: its panes and how they are arranged.
pub struct TermSplit {
    /// In reading order (left to right, top to bottom).
    pub panes: Vec<Pane>,
    /// The split tree over the panes' keys.
    pub layout: Layout,
    /// Index of the focused pane (the one shortcuts and snippets go to).
    pub focus: usize,
    /// The key the next new pane gets (keys are never reused in a tab).
    next_key: u64,
    /// Keyboard focus moves to the focused pane next frame.
    focus_due: bool,
}

/// The widget id of pane `key`'s terminal in tab `tab`.
pub fn pane_id(tab: u64, key: u64) -> Id {
    Id::new(("term", tab, key))
}

impl TermSplit {
    /// A tab with `term` as its only pane (key 0).
    pub fn new(term: TermTab) -> TermSplit {
        TermSplit { panes: vec![Pane { key: 0, term }], layout: Layout::Pane(0), focus: 0, next_key: 1, focus_due: false }
    }

    /// The focused pane's host (what the tab is labelled with).
    pub fn host(&self) -> &HostEntry {
        &self.focused().host
    }

    /// The focused pane's terminal (focus clamped into range).
    pub fn focused(&self) -> &TermTab {
        &self.panes[self.focus.min(self.panes.len() - 1)].term
    }

    /// The focused pane's terminal, mutably.
    pub fn focused_mut(&mut self) -> &mut TermTab {
        let i = self.focus.min(self.panes.len() - 1);
        &mut self.panes[i].term
    }

    /// The focused pane's key.
    pub fn focused_key(&self) -> u64 {
        self.panes[self.focus.min(self.panes.len() - 1)].key
    }

    /// Puts `panes` back in reading order after the layout changed, with
    /// focus (moving there next frame) on pane `focus_key`.
    fn reorder(&mut self, focus_key: u64) {
        let order = self.layout.keys();
        self.panes.sort_by_key(|p| order.iter().position(|k| *k == p.key).unwrap_or(usize::MAX));
        self.focus = self.panes.iter().position(|p| p.key == focus_key).unwrap_or(0);
        self.focus_due = true;
    }

    /// Splits the focused pane: a new session to the same host opens in
    /// the new half, and gets focus.
    pub fn split(&mut self, dir: Dir) {
        let key = self.next_key;
        self.next_key += 1;
        let term = self.focused().sibling();
        let cur = self.focused_key();
        if self.layout.split(cur, dir, key) {
            self.panes.push(Pane { key, term });
            self.reorder(key);
        }
    }

    /// Takes pane `key` out (its session keeps running); its neighbor gets
    /// its space, and focus if it had it. None for the only pane.
    pub fn take(&mut self, key: u64) -> Option<TermTab> {
        let i = self.panes.iter().position(|p| p.key == key).filter(|_| self.panes.len() > 1)?;
        let was_focused = self.focused_key();
        let next = self.layout.remove(key);
        let pane = self.panes.remove(i);
        self.reorder(if was_focused == key { next.unwrap_or(0) } else { was_focused });
        Some(pane.term)
    }

    /// Closes the focused pane (its session ends); focus goes to its
    /// neighbor. False when it is the only pane (close the tab instead).
    pub fn close_focused(&mut self) -> bool {
        self.take(self.focused_key()).is_some()
    }

    /// Moves all of `other`'s panes (sessions running) beside pane
    /// `target`, on side `zone`; they keep their arrangement and focus.
    /// Gives `other` back when that is not possible.
    pub fn absorb(&mut self, other: TermSplit, target: u64, zone: Zone) -> Result<(), Box<TermSplit>> {
        if zone == Zone::Center || !self.layout.contains(target) {
            return Err(Box::new(other));
        }
        // Shift the incoming keys past ours so none collide.
        let base = self.next_key;
        let focus_key = other.focused_key() + base;
        let TermSplit { panes, mut layout, next_key, .. } = other;
        layout.map_keys(&|k| k + base);
        self.next_key += next_key;
        self.layout.insert(target, zone, layout);
        self.panes.extend(panes.into_iter().map(|p| Pane { key: p.key + base, term: p.term }));
        self.reorder(focus_key);
        Ok(())
    }

    /// Moves pane `key` to side `zone` of pane `target`, or swaps the two
    /// on the centre. It gets focus. False when nothing would change.
    pub fn move_pane(&mut self, key: u64, target: u64, zone: Zone) -> bool {
        if key == target || !self.layout.contains(key) || !self.layout.contains(target) {
            return false;
        }
        if zone == Zone::Center {
            self.layout.swap(key, target);
        } else {
            self.layout.remove(key);
            self.layout.insert(target, zone, Layout::Pane(key));
        }
        self.reorder(key);
        true
    }

    /// The pane next to `key` on side `zone`: the one sharing the most of
    /// that edge (the first of equals).
    pub fn neighbor(&self, key: u64, zone: Zone) -> Option<u64> {
        // Lay out on a large gapless square so touching edges compare exactly.
        const BIG: i32 = 1 << 20;
        let (rects, _) = self.layout.arrange(Rect::new(0, 0, BIG, BIG), 0);
        let r = rects.iter().find(|(k, _)| *k == key)?.1;
        let overlap = |a0: i32, a1: i32, b0: i32, b1: i32| a1.min(b1) - a0.max(b0);
        rects
            .iter()
            .filter(|(k, _)| *k != key)
            .filter_map(|(k, c)| {
                let shared = match zone {
                    Zone::Left if c.right() == r.x => overlap(r.y, r.bottom(), c.y, c.bottom()),
                    Zone::Right if c.x == r.right() => overlap(r.y, r.bottom(), c.y, c.bottom()),
                    Zone::Top if c.bottom() == r.y => overlap(r.x, r.right(), c.x, c.right()),
                    Zone::Bottom if c.y == r.bottom() => overlap(r.x, r.right(), c.x, c.right()),
                    _ => 0,
                };
                (shared > 0).then_some((*k, shared))
            })
            .max_by_key(|(_, shared)| *shared)
            .map(|(k, _)| k)
    }

    /// Swaps the focused pane with its neighbor on side `zone`.
    pub fn move_focused(&mut self, zone: Zone) -> bool {
        let key = self.focused_key();
        self.neighbor(key, zone).is_some_and(|n| self.move_pane(key, n, Zone::Center))
    }

    /// Moves the divider beside the focused pane (the innermost one along
    /// that axis) one step towards `zone`.
    pub fn resize_focused(&mut self, zone: Zone) -> bool {
        let Some(dir) = zone.dir() else { return false };
        let Some(path) = self.layout.axis_split(self.focused_key(), dir) else { return false };
        let cur = self.layout.ratio(&path).unwrap_or(0.5);
        self.layout.set_ratio(&path, step_ratio(cur, zone.forward()));
        true
    }

    /// Gives panes in a row the same size; false for a single pane.
    pub fn equalize(&mut self) -> bool {
        self.layout.equalize();
        matches!(self.layout, Layout::Split { .. })
    }

    /// Moves focus to the next (or previous) pane, wrapping around.
    pub fn focus_next(&mut self, forward: bool) {
        let n = self.panes.len();
        self.focus = if forward { (self.focus + 1) % n } else { (self.focus + n - 1) % n };
        self.focus_due = true;
    }

    /// The tab came to the front: its panes' screens are taken as read.
    pub fn shown(&mut self) {
        for p in &mut self.panes {
            p.term.shown();
        }
    }
}

/// The panes of a terminal tab, with their dividers. `actions` (app
/// shortcuts for this view) go to the focused pane. Returns a terminal tab
/// dropped onto one of the panes (the app moves it in).
#[allow(clippy::too_many_arguments)]
pub fn view(
    ui: &mut Ui,
    tab: u64,
    rect: Rect,
    split: &mut TermSplit,
    theme: &GuiTheme,
    keymap: &Keymap,
    wake: Option<WakeFn>,
    actions: &mut Vec<Action>,
) -> Option<TabDrop> {
    let n = split.panes.len();
    split.focus = split.focus.min(n - 1);
    let gap = ui.px(GAP).max(1);
    let pad = ui.px(super::PAD);
    // Minimum pane size in px: MIN_COLS x MIN_ROWS cells plus the padding.
    let (cw, ch) = split.panes[0].term.cell_size();
    let min = (cw * MIN_COLS + 2 * pad, ch * MIN_ROWS + 2 * pad);
    // Clamped for this frame only: the stored shares come back when the
    // window grows again.
    let mut shown = split.layout.clone();
    shown.enforce_min(rect, gap, min);
    let (rects, dividers) = shown.arrange(rect, gap);
    if std::mem::take(&mut split.focus_due) {
        ui.request_focus(pane_id(tab, split.focused_key()));
    }
    if n > 1 {
        alt_drag(ui, tab, split, &rects);
    }
    // Each pane: the focused one gets the shortcuts; with several panes, each
    // is its own focus region with a grip.
    let mut focused = None;
    for (i, p) in split.panes.iter_mut().enumerate() {
        let r = rects.iter().find(|(k, _)| *k == p.key).map_or(Rect::default(), |(_, r)| *r);
        let id = pane_id(tab, p.key);
        let mut mine = if i == split.focus { std::mem::take(actions) } else { vec![] };
        if n == 1 {
            p.term.pane_label = None;
            super::view(ui, id, r, &mut p.term, theme, keymap, wake.clone(), &mut mine);
            continue;
        }
        p.term.pane_label = Some(format!("pane {} of {n}", i + 1));
        let region = id.with("region");
        let name = format!("Pane {} of {n}: {}", i + 1, p.term.host.name);
        ui.region(region, &name, |ui| super::view(ui, id, r, &mut p.term, theme, keymap, wake.clone(), &mut mine));
        if ui.focus_in_region(region) {
            focused = Some(i);
        }
        grip(ui, tab, p.key, i + 1, r, &p.term.host.name);
    }
    if let Some(i) = focused {
        split.focus = i;
    }
    run_requests(ui, split);
    if n > 1 {
        // Dividers are keyed by `Divider::key` (not path), so a focused
        // divider keeps focus when the tree around it changes.
        ui.region(Id::new(("pane-dividers", tab)), "Pane dividers", |ui| {
            for d in &dividers {
                divider(ui, Id::new(("pane-divider", tab)).with(d.key), d, gap, &mut split.layout);
            }
        });
        // Accent line around the focused pane, so it shows which one types.
        let focus_rect = rects.iter().find(|(k, _)| *k == split.focused_key()).map(|(_, r)| *r);
        if let Some(r) = focus_rect {
            let w = ui.px(1).max(1);
            ui.painter().border(r, theme.palette.accent, w, 0.0);
        }
    }
    drops(ui, tab, split, &rects, gap, min, theme)
}

/// Alt+Shift+press on a pane starts dragging it (instead of selecting text);
/// the press event is removed so the terminal underneath never sees it.
/// Plain Alt+click stays the terminal's (programs reading the mouse get it).
fn alt_drag(ui: &mut Ui, tab: u64, split: &TermSplit, rects: &[(u64, Rect)]) {
    let mods = ui.input().mods;
    if !(mods.alt && mods.shift) || ui.dragging() {
        return;
    }
    let Some((i, at)) = ui.input().events.iter().enumerate().find_map(|(i, e)| match e {
        Event::MouseDown(MouseButton::Left, p) => Some((i, *p)),
        _ => None,
    }) else {
        return;
    };
    // Only a press over a pane counts (not over a divider).
    let Some(&(key, _)) = rects.iter().find(|(_, r)| r.contains(at)) else { return };
    ui.input_mut().events.remove(i);
    let label = split.panes.iter().find(|p| p.key == key).map_or_else(String::new, |p| p.term.host.name.clone());
    ui.start_drag(Box::new(PaneDrag::Pane { tab, key }), label);
}

/// The handle at the top centre of a pane that drags the pane (drawn as
/// two rows of five dots).
fn grip(ui: &mut Ui, tab: u64, key: u64, n: usize, pane: Rect, host: &str) {
    // In the padding above the cells, centred (the find bar sits top right).
    let (w, h) = (ui.px(32), ui.px(super::PAD + 2));
    let r = Rect::new(pane.x + (pane.w - w) / 2, pane.y, w, h);
    let id = Id::new(("pane-grip", tab, key));
    let resp = ui.interact(id, r, Sense::drag());
    let started = ui.memory::<bool>(id);
    if !resp.pressed {
        *started = false;
    } else if resp.dragged.is_some() && !*started {
        *started = true; // once per press: Esc cancels for good
        ui.start_drag(Box::new(PaneDrag::Pane { tab, key }), format!("Pane {n}: {host}"));
    }
    if resp.hovered {
        ui.set_cursor_icon(CursorIcon::Grab);
    }
    let p = ui.style().palette;
    let c = if resp.hovered || resp.pressed { p.fg } else { p.muted };
    let dot = ui.px(2).max(1);
    let mut painter = ui.painter();
    for row in 0..2 {
        for col in 0..5 {
            let x = r.x + r.w / 2 - 7 * dot + col * 3 * dot;
            let y = r.y + (r.h - 3 * dot) / 2 + row * 2 * dot;
            painter.rect(Rect::new(x, y, dot, dot), c, 0.0);
        }
    }
}

/// Drop previews over the panes while a pane or terminal tab is dragged,
/// and the drop itself: a pane moves here; a tab is handed to the app.
///
/// Drop mapping: a pane of this tab on another pane's side moves there
/// (`move_pane`, a split), on its centre swaps; another terminal tab on a
/// side or centre returns a `TabDrop` (the app absorbs it, or shows it).
/// Sides too small for two `min` halves get no target.
fn drops(
    ui: &mut Ui,
    tab: u64,
    split: &mut TermSplit,
    rects: &[(u64, Rect)],
    gap: i32,
    min: (i32, i32),
    theme: &GuiTheme,
) -> Option<TabDrop> {
    let payload = *ui.drag_payload::<PaneDrag>()?;
    let moving = match payload {
        PaneDrag::Tab(t) if t != tab => None,
        PaneDrag::Pane { tab: t, key } if t == tab => Some(key),
        _ => return None, // its own tab, or a pane of another tab
    };
    let mouse = ui.input().mouse;
    let mut out = None;
    for &(key, r) in rects {
        let zone = Zone::at(r, mouse);
        // Not onto itself; a side only where both halves would keep `min`.
        let useful = match (moving, zone) {
            (Some(k), _) if k == key => false,
            (_, Zone::Center) => true,
            _ => zone.fits(r, gap, min),
        };
        if !useful || !r.contains(mouse) {
            continue;
        }
        let d = ui.drop_target::<PaneDrag>(r);
        let p = theme.palette;
        let show = zone.preview(r, gap);
        let w = ui.px(2).max(1);
        let mut painter = ui.painter();
        painter.rect(show, p.accent.with_alpha(48), 0.0);
        painter.border(show, p.accent, w, 0.0);
        if d.dropped.is_some() {
            match moving {
                Some(k) => {
                    split.move_pane(k, key, zone);
                }
                None => {
                    if let PaneDrag::Tab(from) = payload {
                        out = Some(TabDrop { from, target: key, zone });
                    }
                }
            }
            ui.request_repaint_in(std::time::Duration::ZERO);
        }
    }
    out
}

/// Runs a split or close a pane's menu asked for, on that pane (which
/// takes focus first, so the split / close applies to it).
fn run_requests(ui: &mut Ui, split: &mut TermSplit) {
    let Some((i, a)) = split.panes.iter_mut().enumerate().find_map(|(i, p)| p.term.pane_request.take().map(|a| (i, a))) else {
        return;
    };
    split.focus = i;
    match a {
        Action::SplitRight => split.split(Dir::Right),
        Action::SplitDown => split.split(Dir::Down),
        Action::ClosePane => {
            split.close_focused();
        }
        _ => {}
    }
    ui.request_repaint_in(std::time::Duration::ZERO);
}

/// One divider: drag it (snapping unless Alt is held; double-click
/// equalizes), or focus it and use the arrow keys (also the screen
/// reader's increment / decrement), which step through the snap points.
fn divider(ui: &mut Ui, id: Id, d: &Divider, gap: i32, layout: &mut Layout) {
    let r = ui.interact(id, d.handle, Sense { click: true, drag: true, focus: true });
    let mut ratio = d.ratio;
    let mut snapped = None;
    // Dragging (not on the press frame itself, so a click doesn't jump the
    // divider): follow the pointer, then snap when within SNAP_PX of a snap
    // point, measured in px of the split's room.
    let down_now = ui.input().events.iter().any(|e| matches!(e, Event::MouseDown(MouseButton::Left, _)));
    if r.pressed && !down_now && r.dragged.is_some() {
        ratio = ratio_at(d.area, d.dir, gap, ui.input().mouse);
        let room = if d.dir == Dir::Right { d.area.w } else { d.area.h } - gap;
        if !ui.input().mods.alt
            && let Some(s) = snap(ratio, room, ui.px(SNAP_PX))
        {
            ratio = s;
            snapped = Some(s);
        }
    }
    // Keyboard and screen-reader steps: to the next snap point, else KEY_STEP.
    if r.focused {
        let (less, more) = match d.dir {
            Dir::Right => (Key::Left, Key::Right),
            Dir::Down => (Key::Up, Key::Down),
        };
        if ui.input_mut().take_key(&less, Modifiers::NONE) {
            ratio = step_ratio(ratio, false);
        }
        if ui.input_mut().take_key(&more, Modifiers::NONE) {
            ratio = step_ratio(ratio, true);
        }
    }
    if ui.take_a11y_action(id, |a| matches!(a, A11yAction::Increment)).is_some() {
        ratio = step_ratio(ratio, true);
    }
    if ui.take_a11y_action(id, |a| matches!(a, A11yAction::Decrement)).is_some() {
        ratio = step_ratio(ratio, false);
    }
    // Write back to the stored layout (the drawn one may be min-clamped).
    if r.double_clicked {
        layout.equalize_at(&d.path);
        ratio = layout.ratio(&d.path).unwrap_or(ratio);
    } else if ratio != d.ratio {
        layout.set_ratio(&d.path, ratio);
    }
    let shown = ratio.clamp(MIN_RATIO, 1.0 - MIN_RATIO);
    if r.hovered || r.pressed {
        ui.set_cursor_icon(match d.dir {
            Dir::Right => CursorIcon::ResizeH,
            Dir::Down => CursorIcon::ResizeV,
        });
    }
    let p = ui.style().palette;
    let c = if r.hovered || r.pressed || r.focused { p.accent } else { p.border };
    // Snapped: the line thickens and the share shows by the pointer.
    let line = if snapped.is_some() { ui.px(3).max(2) } else { ui.px(1).max(1) };
    let h = d.handle;
    let mark = match d.dir {
        Dir::Right => Rect::new(h.x + (h.w - line) / 2, h.y, line, h.h),
        Dir::Down => Rect::new(h.x, h.y + (h.h - line) / 2, h.w, line),
    };
    ui.painter().rect(mark, c, 0.0);
    if let Some(s) = snapped {
        let m = ui.input().mouse;
        let at = Rect::new(m.x, m.y + ui.px(8), 1, 1);
        tooltip(ui, at, &format!("{}%", (s * 100.0).round()));
    }
    let name = match d.dir {
        Dir::Right => "Pane divider, left and right",
        Dir::Down => "Pane divider, top and bottom",
    };
    if let Some(node) = ui.a11y_node(id, Role::Splitter, name) {
        node.value = Some(format!("{}%", (shown * 100.0).round()));
    }
}

#[cfg(test)]
#[path = "tests_split.rs"]
mod tests;
