//! The dock's layout math: a binary tree of two-way splits whose leaves
//! are tab groups (`Layout::Group(id)`), with the drop-zone geometry used
//! while a tab is dragged (`Zone`).
//!
//! A node is addressed by its path (`false` = into `a`, `true` = into `b`);
//! `Layout::arrange` turns the tree into group rects and `Divider`s each
//! frame, `enforce_min` keeps every group a minimum size for one frame
//! without touching the stored shares, and `neighbor` finds the group on a
//! given side. The snap points, key step and minimum share are shared with
//! the TUI through `skiff_core::panes`.

use skiff_ui::geom::{Point, Rect};

/// Where a split puts the new group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    /// Side by side, the new group on the right.
    Right,
    /// Stacked, the new group below.
    Down,
}

// Shared with the TUI: snap points (25/33/50/67/75%), the 10% minimum share,
// the 5% key step past the last snap point, and the snapping functions.
pub use skiff_core::panes::{KEY_STEP, MIN_RATIO, SNAPS, snap, step_ratio};

/// Divider hit zone between groups, logical px (the line drawn is 1 px).
pub const GAP: i32 = 7;
/// How close (logical px) a dragged divider must come to snap.
pub const SNAP_PX: i32 = 8;

/// A side of a group (where a dropped tab docks, which way a tab moves),
/// or its centre (a drop there moves the tab into the group).
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
    pub fn forward(self) -> bool {
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

/// The arrangement of the dock's groups (by group id): a binary tree whose
/// leaves are groups (`Group(key)`) and whose inner nodes split their area in two.
#[derive(Debug, Clone, PartialEq)]
pub enum Layout {
    Group(u64),
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
    /// A key that survives restructuring (unlike `path`): the group right
    /// after the divider, the first one of the split's second side. Each
    /// group starts at most one split's second side, so it is unique.
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
    /// The groups' rects (in reading order) and the dividers of `rect`.
    pub fn arrange(&self, rect: Rect, gap: i32) -> (Vec<(u64, Rect)>, Vec<Divider>) {
        let (mut groups, mut dividers) = (vec![], vec![]);
        self.arrange_into(rect, gap, &mut vec![], &mut groups, &mut dividers);
        (groups, dividers)
    }

    /// Recursive half of `arrange`: lays this node out in `rect`, with `path`
    /// the turns to it; a split's divider is pushed before its children's.
    fn arrange_into(&self, rect: Rect, gap: i32, path: &mut Vec<bool>, groups: &mut Vec<(u64, Rect)>, dividers: &mut Vec<Divider>) {
        match self {
            Layout::Group(k) => groups.push((*k, rect)),
            Layout::Split { dir, ratio, a, b } => {
                let (ra, handle, rb) = split_rect(rect, *dir, *ratio, gap);
                dividers.push(Divider { path: path.clone(), key: b.first_key(), dir: *dir, ratio: *ratio, handle, area: rect });
                path.push(false);
                a.arrange_into(ra, gap, path, groups, dividers);
                path.pop();
                path.push(true);
                b.arrange_into(rb, gap, path, groups, dividers);
                path.pop();
            }
        }
    }

    /// Group keys in reading order.
    pub fn keys(&self) -> Vec<u64> {
        self.arrange(Rect::default(), 0).0.into_iter().map(|(k, _)| k).collect()
    }

    /// The first group in reading order (leftmost / topmost leaf).
    fn first_key(&self) -> u64 {
        match self {
            Layout::Group(k) => *k,
            Layout::Split { a, .. } => a.first_key(),
        }
    }

    /// The last group in reading order (rightmost / bottommost leaf).
    fn last_key(&self) -> u64 {
        match self {
            Layout::Group(k) => *k,
            Layout::Split { b, .. } => b.last_key(),
        }
    }

    /// Splits group `key` in two, `new` after it; false if there is no such group.
    pub fn split(&mut self, key: u64, dir: Dir, new: u64) -> bool {
        match self {
            Layout::Group(k) if *k == key => {
                *self = Layout::Split { dir, ratio: 0.5, a: Box::new(Layout::Group(key)), b: Box::new(Layout::Group(new)) };
                true
            }
            Layout::Group(_) => false,
            Layout::Split { a, b, .. } => a.split(key, dir, new) || b.split(key, dir, new),
        }
    }

    /// Removes group `key` (its neighbor takes the space); returns the group
    /// that should get focus: the neighbor next to it in reading order.
    pub fn remove(&mut self, key: u64) -> Option<u64> {
        let Layout::Split { a, b, .. } = self else {
            return None;
        };
        // A direct child: the split collapses into the other side (the
        // `Group(0)` placeholder is dropped with the old node).
        if **a == Layout::Group(key) {
            let rest = std::mem::replace(&mut **b, Layout::Group(0));
            let next = rest.first_key();
            *self = rest;
            return Some(next);
        }
        if **b == Layout::Group(key) {
            let rest = std::mem::replace(&mut **a, Layout::Group(0));
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
            (Layout::Group(_), _) => None,
        }
    }

    /// The node at `path`, mutably.
    fn node_mut(&mut self, path: &[bool]) -> Option<&mut Layout> {
        match (self, path.split_first()) {
            (n, None) => Some(n),
            (Layout::Split { a, .. }, Some((false, rest))) => a.node_mut(rest),
            (Layout::Split { b, .. }, Some((true, rest))) => b.node_mut(rest),
            (Layout::Group(_), _) => None,
        }
    }

    /// The share of the split at `path`.
    pub fn ratio(&self, path: &[bool]) -> Option<f32> {
        match self.node(path)? {
            Layout::Split { ratio, .. } => Some(*ratio),
            Layout::Group(_) => None,
        }
    }

    /// Sets the share of the split at `path` (kept within `MIN_RATIO`).
    pub fn set_ratio(&mut self, path: &[bool], value: f32) {
        if let Some(Layout::Split { ratio, .. }) = self.node_mut(path) {
            *ratio = value.clamp(MIN_RATIO, 1.0 - MIN_RATIO);
        }
    }

    /// Whether group `key` is somewhere in this tree.
    pub fn contains(&self, key: u64) -> bool {
        match self {
            Layout::Group(k) => *k == key,
            Layout::Split { a, b, .. } => a.contains(key) || b.contains(key),
        }
    }

    /// The turns from the root to group `key`.
    fn path_to(&self, key: u64) -> Option<Vec<bool>> {
        match self {
            Layout::Group(k) => (*k == key).then(Vec::new),
            Layout::Split { a, b, .. } => {
                let (side, rest) = if a.contains(key) { (false, a) } else { (true, b) };
                let mut p = rest.path_to(key)?;
                p.insert(0, side);
                Some(p)
            }
        }
    }

    /// The innermost split along `dir` that group `key` is in (its path).
    pub fn axis_split(&self, key: u64, dir: Dir) -> Option<Vec<bool>> {
        let path = self.path_to(key)?;
        // Walk the group's ancestors from the nearest up; first match wins.
        (0..path.len())
            .rev()
            .map(|i| &path[..i])
            .find(|p| matches!(self.node(p), Some(Layout::Split { dir: d, .. }) if *d == dir))
            .map(<[bool]>::to_vec)
    }

    /// Puts `sub` beside group `target`, on side `zone` of it (half each).
    /// False for the centre or when there is no such group.
    pub fn insert(&mut self, target: u64, zone: Zone, sub: Layout) -> bool {
        let Some(dir) = zone.dir() else { return false };
        match self {
            // The target leaf becomes a split; Right / Bottom put `sub` second.
            Layout::Group(k) if *k == target => {
                let (old, new) = (Box::new(Layout::Group(target)), Box::new(sub));
                let (a, b) = if zone.forward() { (old, new) } else { (new, old) };
                *self = Layout::Split { dir, ratio: 0.5, a, b };
                true
            }
            Layout::Group(_) => false,
            Layout::Split { a, b, .. } => {
                if a.contains(target) {
                    a.insert(target, zone, sub)
                } else {
                    b.insert(target, zone, sub)
                }
            }
        }
    }

    /// Exchanges the places of groups `x` and `y`.
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

    /// Rewrites every group key through `f` (swaps).
    fn map_keys(&mut self, f: &impl Fn(u64) -> u64) {
        match self {
            Layout::Group(k) => *k = f(*k),
            Layout::Split { a, b, .. } => {
                a.map_keys(f);
                b.map_keys(f);
            }
        }
    }

    /// How many groups sit in a row along `dir` (splits the other way count
    /// their widest side).
    fn span(&self, dir: Dir) -> u32 {
        match self {
            Layout::Group(_) => 1,
            Layout::Split { dir: d, a, b, .. } if *d == dir => a.span(dir) + b.span(dir),
            Layout::Split { a, b, .. } => a.span(dir).max(b.span(dir)),
        }
    }

    /// Shares so that groups in a row get the same size: each split's ratio is
    /// its first side's span over the total (e.g. 1 vs 2 groups gives 1/3).
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

    /// The least px this needs along `dir`, each group at least `min` (w, h):
    /// splits along `dir` add their sides plus the gap, others take the larger.
    fn min_extent(&self, dir: Dir, min: (i32, i32), gap: i32) -> i32 {
        match self {
            Layout::Group(_) if dir == Dir::Right => min.0,
            Layout::Group(_) => min.1,
            Layout::Split { dir: d, a, b, .. } if *d == dir => a.min_extent(dir, min, gap) + gap + b.min_extent(dir, min, gap),
            Layout::Split { a, b, .. } => a.min_extent(dir, min, gap).max(b.min_extent(dir, min, gap)),
        }
    }

    /// Moves dividers so every group of `rect` keeps `min` px (w, h) where
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

    /// The group next to `key` on side `zone`: the one sharing the most of
    /// that edge (the first of equals); `None` for the centre or the window edge.
    pub fn neighbor(&self, key: u64, zone: Zone) -> Option<u64> {
        // Lay out on a large gapless square so touching edges compare exactly.
        const BIG: i32 = 1 << 20;
        let (rects, _) = self.arrange(Rect::new(0, 0, BIG, BIG), 0);
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
}
