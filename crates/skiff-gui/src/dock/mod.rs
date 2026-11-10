//! The dock: the main area is a split tree of tab groups, each with its own
//! tab strip showing one of its tabs. Any tab (terminal, file transfer,
//! settings, forwards) can be dragged to another group's strip (inserted
//! where the marker shows), onto a group's centre (moved into it) or onto
//! one of its edges (docked beside it as a new group). Empty groups close
//! by themselves; dividers between groups resize (see `view`).
//!
//! Structure: `Dock` holds the `Group`s (each an ordered list of tab ids
//! and the one it shows), a `Layout` tree over the group ids and the
//! focused group. The app owns the tabs themselves (`GuiApp::tabs`, by id)
//! and its focused tab (`GuiApp::active`); `Dock::sync` reconciles the two
//! each frame, so code that opens, closes or picks tabs needs no dock
//! calls: new tabs join the focused group, closed ones leave theirs.

pub mod layout;
pub mod view;

pub use layout::{Dir, Divider, GAP, Layout, Zone, ratio_at, split_rect, step_ratio};

/// The widget id of group `group`'s tab strip.
pub fn strip_id(group: u64) -> skiff_ui::ui::Id {
    skiff_ui::ui::Id::new(("tabs", group))
}

/// What the toolkit drag-and-drop carries while a tab is dragged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DockDrag {
    /// Id of the dragged tab.
    pub tab: u64,
}

/// Where a dragged tab (or a keyboard move) puts a tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drop {
    /// Into group `group` at strip position `index` (the end when `None`).
    Into { group: u64, index: Option<usize> },
    /// Into a new group on side `zone` of group `group`.
    Beside { group: u64, zone: Zone },
}

/// One tab group: a tab strip and the tab it shows.
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    /// Stable for the app's lifetime (the leaf in `Layout`, and part of
    /// the strip's and regions' widget ids).
    pub id: u64,
    /// Its tabs (by tab id), in strip order; never empty.
    pub tabs: Vec<u64>,
    /// The tab it shows.
    pub active: u64,
}

/// The groups, their arrangement and which one has focus.
#[derive(Debug, Default)]
pub struct Dock {
    /// Every group (any order; `order` gives reading order).
    pub groups: Vec<Group>,
    /// The split tree over the group ids (`None` with no tabs open).
    pub layout: Option<Layout>,
    /// The group shortcuts and new tabs go to.
    pub focus: u64,
    /// The id the next group gets.
    next_id: u64,
    /// The last dock change in words, for screen readers (a live region).
    pub announce: String,
}

impl Dock {
    /// Group `id`.
    pub fn group(&self, id: u64) -> Option<&Group> {
        self.groups.iter().find(|g| g.id == id)
    }

    /// Group `id`, mutably.
    fn group_mut(&mut self, id: u64) -> Option<&mut Group> {
        self.groups.iter_mut().find(|g| g.id == id)
    }

    /// The group holding tab `tab`.
    pub fn group_of(&self, tab: u64) -> Option<u64> {
        self.groups.iter().find(|g| g.tabs.contains(&tab)).map(|g| g.id)
    }

    /// Group ids in reading order (left to right, top to bottom).
    pub fn order(&self) -> Vec<u64> {
        self.layout.as_ref().map(Layout::keys).unwrap_or_default()
    }

    /// The focused group.
    pub fn focused(&self) -> Option<&Group> {
        self.group(self.focus)
    }

    /// A new group holding `tab` (not yet in the layout); returns its id.
    fn new_group(&mut self, tab: u64) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.groups.push(Group { id, tabs: vec![tab], active: tab });
        id
    }

    /// Brings the dock in line with the app's tabs: closed tabs leave their
    /// groups, tabs in no group join the focused one (at the end), and
    /// `active` (the app's focused tab) is shown with its group focused.
    pub fn sync(&mut self, tabs: &[u64], active: Option<u64>) {
        let gone: Vec<u64> = self.groups.iter().flat_map(|g| g.tabs.iter().copied()).filter(|t| !tabs.contains(t)).collect();
        for t in gone {
            self.remove_tab(t);
        }
        for &t in tabs {
            if self.group_of(t).is_some() {
                continue;
            }
            match self.group_mut(self.focus) {
                Some(g) => g.tabs.push(t),
                None => {
                    // No group yet (or the focused one is gone): start one.
                    let id = self.new_group(t);
                    match &mut self.layout {
                        Some(l) => {
                            let last = *l.keys().last().expect("a layout has a group");
                            l.insert(last, Zone::Right, Layout::Group(id));
                        }
                        None => self.layout = Some(Layout::Group(id)),
                    }
                    self.focus = id;
                }
            }
        }
        if let Some(a) = active
            && let Some(g) = self.group_of(a)
        {
            self.focus = g;
            self.group_mut(g).expect("found above").active = a;
        }
        if self.group(self.focus).is_none()
            && let Some(first) = self.order().first()
        {
            self.focus = *first;
        }
    }

    /// Takes tab `tab` out of its group. A group left empty closes (its
    /// neighbor takes the space, and focus if it had it). Returns the tab
    /// shown in its place when it was its group's shown tab: the one to
    /// its left in the strip, else the neighbor group's.
    pub fn remove_tab(&mut self, tab: u64) -> Option<u64> {
        let gi = self.groups.iter().position(|g| g.tabs.contains(&tab))?;
        let g = &mut self.groups[gi];
        let pos = g.tabs.iter().position(|t| *t == tab).expect("found above");
        g.tabs.remove(pos);
        if g.tabs.is_empty() {
            let gid = g.id;
            self.groups.remove(gi);
            let next = match &mut self.layout {
                Some(Layout::Group(_)) | None => {
                    self.layout = None;
                    None
                }
                Some(l) => l.remove(gid),
            };
            if self.focus == gid {
                self.focus = next.unwrap_or(0);
            }
            return next.and_then(|n| self.group(n)).map(|g| g.active);
        }
        if g.active != tab {
            return None;
        }
        g.active = g.tabs[pos.saturating_sub(1).min(g.tabs.len() - 1)];
        Some(g.active)
    }

    /// Moves the tab at strip position `from` of group `group` to `to`.
    pub fn reorder(&mut self, group: u64, from: usize, to: usize) {
        if let Some(g) = self.group_mut(group)
            && from < g.tabs.len()
            && to < g.tabs.len()
        {
            let t = g.tabs.remove(from);
            g.tabs.insert(to, t);
        }
    }

    /// Whether `to` would change anything for tab `tab` (a drop onto its own
    /// place, or docking the only tab of a group beside itself, would not).
    pub fn can_drop(&self, tab: u64, to: Drop) -> bool {
        let Some(src) = self.group_of(tab).and_then(|g| self.group(g)) else { return false };
        match to {
            Drop::Into { group, .. } if self.group(group).is_none() => false,
            Drop::Into { group, index: None } => group != src.id,
            Drop::Into { group, index: Some(i) } if group == src.id => {
                let pos = src.tabs.iter().position(|t| *t == tab).unwrap_or(0);
                i != pos && i != pos + 1
            }
            Drop::Into { .. } => true,
            Drop::Beside { group, zone } => zone != Zone::Center && self.group(group).is_some() && (group != src.id || src.tabs.len() > 1),
        }
    }

    /// Moves tab `tab` as `to` says; it is shown and its group focused.
    /// Returns the group it ended in (`None`, and nothing changes, when
    /// `can_drop` says no).
    pub fn apply(&mut self, tab: u64, to: Drop) -> Option<u64> {
        if !self.can_drop(tab, to) {
            return None;
        }
        match to {
            Drop::Into { group, index } => {
                let src = self.group_of(tab)?;
                let pos = self.group(src)?.tabs.iter().position(|t| *t == tab)?;
                self.remove_tab(tab);
                // Within the same strip the removal shifted later positions.
                let index = index.map(|i| if src == group && i > pos { i - 1 } else { i });
                let g = self.group_mut(group)?;
                let at = index.unwrap_or(g.tabs.len()).min(g.tabs.len());
                g.tabs.insert(at, tab);
                g.active = tab;
                self.focus = group;
                Some(group)
            }
            Drop::Beside { group, zone } => {
                self.remove_tab(tab);
                Some(self.place_beside(tab, group, zone))
            }
        }
    }

    /// Puts tab `tab` (in no group) into a new group on side `zone` of
    /// group `target` (or as the only group when there is none); focuses it.
    pub fn place_beside(&mut self, tab: u64, target: u64, zone: Zone) -> u64 {
        let id = self.new_group(tab);
        match &mut self.layout {
            Some(l) if l.contains(target) => {
                l.insert(target, zone, Layout::Group(id));
            }
            Some(l) => {
                let last = *l.keys().last().expect("a layout has a group");
                l.insert(last, zone, Layout::Group(id));
            }
            None => self.layout = Some(Layout::Group(id)),
        }
        self.focus = id;
        id
    }

    /// The group on side `zone` of group `group`.
    pub fn neighbor(&self, group: u64, zone: Zone) -> Option<u64> {
        self.layout.as_ref()?.neighbor(group, zone)
    }

    /// Where the keyboard's "move tab to group <side>" puts tab `tab`: into
    /// the neighbor group on that side, else (when its group has other tabs)
    /// a new group on that side; `None` when it is alone at that edge.
    pub fn move_target(&self, tab: u64, zone: Zone) -> Option<Drop> {
        let src = self.group_of(tab)?;
        let to = match self.neighbor(src, zone) {
            Some(n) => Drop::Into { group: n, index: None },
            None => Drop::Beside { group: src, zone },
        };
        self.can_drop(tab, to).then_some(to)
    }

    /// Focuses the group on side `zone` of the focused one; false if none.
    pub fn focus_toward(&mut self, zone: Zone) -> bool {
        match self.neighbor(self.focus, zone) {
            Some(n) => {
                self.focus = n;
                true
            }
            None => false,
        }
    }

    /// Focuses the next (or previous) group in reading order, wrapping;
    /// false with a single group.
    pub fn focus_next(&mut self, forward: bool) -> bool {
        let order = self.order();
        let n = order.len();
        if n < 2 {
            return false;
        }
        let i = order.iter().position(|g| *g == self.focus).unwrap_or(0);
        self.focus = order[if forward { (i + 1) % n } else { (i + n - 1) % n }];
        true
    }

    /// Moves the divider beside the focused group (the innermost one along
    /// that axis) one step towards `zone`; false if there is none.
    pub fn resize_focused(&mut self, zone: Zone) -> bool {
        let (Some(dir), Some(l)) = (zone.dir(), self.layout.as_mut()) else { return false };
        let Some(path) = l.axis_split(self.focus, dir) else { return false };
        let cur = l.ratio(&path).unwrap_or(0.5);
        l.set_ratio(&path, step_ratio(cur, zone.forward()));
        true
    }

    /// Gives groups in a row the same size; false with a single group.
    pub fn equalize(&mut self) -> bool {
        match &mut self.layout {
            Some(l @ Layout::Split { .. }) => {
                l.equalize();
                true
            }
            _ => false,
        }
    }

    /// "Group 2 of 3" for group `id` (reading order), or "" with one group.
    pub fn position_words(&self, id: u64) -> String {
        let order = self.order();
        match order.iter().position(|g| *g == id) {
            Some(i) if order.len() > 1 => format!("Group {} of {}", i + 1, order.len()),
            _ => String::new(),
        }
    }
}

#[cfg(test)]
mod tests;
