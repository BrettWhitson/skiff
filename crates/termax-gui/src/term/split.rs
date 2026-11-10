//! Split panes: a terminal tab holds one or more panes, each its own
//! session to the same host, laid out by a tree of two-way splits.
//!
//! Dividers drag with the mouse and, focused (F6 reaches them), move with
//! the arrow keys. Focus moves between panes by click, F6 and the
//! `focus_next_pane` shortcut; each pane is its own focus region and its
//! terminal is named "Terminal: host, pane n of m" for screen readers.

use termax_core::hosts::HostEntry;
use termax_ui::a11y::{A11yAction, Role};
use termax_ui::geom::{Point, Rect};
use termax_ui::input::{CursorIcon, Event, Key, Modifiers, MouseButton};
use termax_ui::ui::{Id, Sense, Ui};

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

/// The smallest share of a split either side keeps.
pub const MIN_RATIO: f32 = 0.1;
/// How far one arrow key press moves a divider.
pub const KEY_STEP: f32 = 0.05;
/// Divider thickness, logical px.
pub const GAP: i32 = 5;

/// The arrangement of a tab's panes (by pane key).
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
    pub dir: Dir,
    pub ratio: f32,
    /// The draggable strip between the two sides.
    pub handle: Rect,
    /// The whole area the split divides.
    pub area: Rect,
}

/// `rect` split at `ratio`: the first side, the divider and the second side.
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

/// The ratio that puts a divider of `area` under the mouse at `at`.
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

    fn first_key(&self) -> u64 {
        match self {
            Layout::Pane(k) => *k,
            Layout::Split { a, .. } => a.first_key(),
        }
    }

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

    /// The split at `path`.
    fn ratio_mut(&mut self, path: &[bool]) -> Option<&mut f32> {
        match (self, path.split_first()) {
            (Layout::Split { ratio, .. }, None) => Some(ratio),
            (Layout::Split { a, .. }, Some((false, rest))) => a.ratio_mut(rest),
            (Layout::Split { b, .. }, Some((true, rest))) => b.ratio_mut(rest),
            (Layout::Pane(_), _) => None,
        }
    }

    /// Sets the share of the split at `path` (kept within `MIN_RATIO`).
    pub fn set_ratio(&mut self, path: &[bool], value: f32) {
        if let Some(r) = self.ratio_mut(path) {
            *r = value.clamp(MIN_RATIO, 1.0 - MIN_RATIO);
        }
    }
}

pub struct Pane {
    pub key: u64,
    pub term: TermTab,
}

/// A terminal tab: its panes and how they are arranged.
pub struct TermSplit {
    /// In reading order (left to right, top to bottom).
    pub panes: Vec<Pane>,
    pub layout: Layout,
    /// Index of the focused pane (the one shortcuts and snippets go to).
    pub focus: usize,
    next_key: u64,
    /// Keyboard focus moves to the focused pane next frame.
    focus_due: bool,
}

/// The widget id of pane `key`'s terminal in tab `tab`.
pub fn pane_id(tab: u64, key: u64) -> Id {
    Id::new(("term", tab, key))
}

impl TermSplit {
    pub fn new(term: TermTab) -> TermSplit {
        TermSplit { panes: vec![Pane { key: 0, term }], layout: Layout::Pane(0), focus: 0, next_key: 1, focus_due: false }
    }

    pub fn host(&self) -> &HostEntry {
        &self.focused().host
    }

    pub fn focused(&self) -> &TermTab {
        &self.panes[self.focus.min(self.panes.len() - 1)].term
    }

    pub fn focused_mut(&mut self) -> &mut TermTab {
        let i = self.focus.min(self.panes.len() - 1);
        &mut self.panes[i].term
    }

    /// Splits the focused pane: a new session to the same host opens in
    /// the new half, and gets focus.
    pub fn split(&mut self, dir: Dir) {
        let key = self.next_key;
        self.next_key += 1;
        let term = self.focused().sibling();
        let cur = self.panes[self.focus].key;
        if self.layout.split(cur, dir, key) {
            self.focus += 1;
            self.panes.insert(self.focus, Pane { key, term });
            self.focus_due = true;
        }
    }

    /// Closes the focused pane (its session ends); focus goes to its
    /// neighbor. False when it is the only pane (close the tab instead).
    pub fn close_focused(&mut self) -> bool {
        if self.panes.len() < 2 {
            return false;
        }
        let key = self.panes[self.focus].key;
        let next = self.layout.remove(key);
        self.panes.remove(self.focus);
        self.focus = next.and_then(|k| self.panes.iter().position(|p| p.key == k)).unwrap_or(0);
        self.focus_due = true;
        true
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
/// shortcuts for this view) go to the focused pane.
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
) {
    let n = split.panes.len();
    split.focus = split.focus.min(n - 1);
    if n == 1 {
        let p = &mut split.panes[0];
        p.term.pane_label = None;
        let id = pane_id(tab, p.key);
        if std::mem::take(&mut split.focus_due) {
            ui.request_focus(id);
        }
        super::view(ui, id, rect, &mut p.term, theme, keymap, wake, actions);
        run_requests(ui, split);
        return;
    }
    let gap = ui.px(GAP).max(1);
    let (rects, dividers) = split.layout.arrange(rect, gap);
    if std::mem::take(&mut split.focus_due) {
        ui.request_focus(pane_id(tab, split.panes[split.focus].key));
    }
    let mut focused = None;
    for (i, p) in split.panes.iter_mut().enumerate() {
        let r = rects.iter().find(|(k, _)| *k == p.key).map_or(Rect::default(), |(_, r)| *r);
        let id = pane_id(tab, p.key);
        p.term.pane_label = Some(format!("pane {} of {n}", i + 1));
        let mut mine = if i == split.focus { std::mem::take(actions) } else { vec![] };
        let region = id.with("region");
        let name = format!("Pane {} of {n}: {}", i + 1, p.term.host.name);
        ui.region(region, &name, |ui| super::view(ui, id, r, &mut p.term, theme, keymap, wake.clone(), &mut mine));
        if ui.focus_in_region(region) {
            focused = Some(i);
        }
    }
    if let Some(i) = focused {
        split.focus = i;
    }
    run_requests(ui, split);
    ui.region(Id::new(("pane-dividers", tab)), "Pane dividers", |ui| {
        for d in &dividers {
            divider(ui, Id::new(("pane-divider", tab)).with(d.key), d, gap, &mut split.layout);
        }
    });
    // Accent line around the focused pane, so it shows which one types.
    let focus_rect = rects.iter().find(|(k, _)| *k == split.panes[split.focus].key).map(|(_, r)| *r);
    if let Some(r) = focus_rect {
        let w = ui.px(1).max(1);
        ui.painter().border(r, theme.palette.accent, w, 0.0);
    }
}

/// Runs a split or close a pane's menu asked for, on that pane.
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

/// One divider: drag it, or focus it and use the arrow keys (also the
/// screen reader's increment / decrement).
fn divider(ui: &mut Ui, id: Id, d: &Divider, gap: i32, layout: &mut Layout) {
    let r = ui.interact(id, d.handle, Sense { click: false, drag: true, focus: true });
    let mut ratio = d.ratio;
    let down_now = ui.input().events.iter().any(|e| matches!(e, Event::MouseDown(MouseButton::Left, _)));
    if r.pressed && !down_now {
        ratio = ratio_at(d.area, d.dir, gap, ui.input().mouse);
    }
    if r.focused {
        let (less, more) = match d.dir {
            Dir::Right => (Key::Left, Key::Right),
            Dir::Down => (Key::Up, Key::Down),
        };
        if ui.input_mut().take_key(&less, Modifiers::NONE) {
            ratio -= KEY_STEP;
        }
        if ui.input_mut().take_key(&more, Modifiers::NONE) {
            ratio += KEY_STEP;
        }
    }
    if ui.take_a11y_action(id, |a| matches!(a, A11yAction::Increment)).is_some() {
        ratio += KEY_STEP;
    }
    if ui.take_a11y_action(id, |a| matches!(a, A11yAction::Decrement)).is_some() {
        ratio -= KEY_STEP;
    }
    if ratio != d.ratio {
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
    let line = ui.px(1).max(1);
    let h = d.handle;
    let mark = match d.dir {
        Dir::Right => Rect::new(h.x + (h.w - line) / 2, h.y, line, h.h),
        Dir::Down => Rect::new(h.x, h.y + (h.h - line) / 2, h.w, line),
    };
    ui.painter().rect(mark, c, 0.0);
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
