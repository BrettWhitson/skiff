//! File-transfer tabs: two panes, each Local or any host.

use termax_core::askpass::Broker;
use termax_core::fs::FileEntry;
use termax_core::hosts::HostEntry;
use termax_ui::geom::Rect;
use termax_ui::layout::Size as L;
use termax_ui::ui::Ui;

use crate::engine::Engine;

pub mod ops;
pub mod pane;

pub use pane::{Confirm, Pane, PaneState, format_time};

#[derive(Debug, Clone)]
pub enum Location {
    Local,
    Host(HostEntry),
}

impl Location {
    pub fn label(&self) -> &str {
        match self {
            Location::Local => "Local",
            Location::Host(h) => &h.name,
        }
    }
}

/// Results of background file work for one pane.
pub enum FilesEvent {
    Listed { seq: u64, cwd: String, result: Result<Vec<FileEntry>, String> },
    OpDone { result: Result<(), String> },
}

/// What the file views need from the app for one frame.
pub struct FilesCtx<'a> {
    pub tab_id: u64,
    pub engine: &'a Engine,
    /// Where Local panes start.
    pub start_dir: &'a str,
    /// For the location pickers.
    pub hosts: &'a [HostEntry],
    pub askpass: Option<&'a Broker>,
    pub ssh_binary: &'a str,
}

pub struct FilesTab {
    pub panes: [Pane; 2],
}

impl FilesTab {
    pub fn new(left: Location, right: Location) -> Self {
        FilesTab { panes: [Pane::new(left), Pane::new(right)] }
    }

    pub fn title(&self) -> String {
        format!("\u{21C5} {} \u{2194} {}", self.panes[0].loc.label(), self.panes[1].loc.label())
    }

    pub fn on_event(&mut self, side: usize, ev: FilesEvent, ctx: &FilesCtx) {
        if let Some(p) = self.panes.get_mut(side) {
            p.on_event(ev, ctx, side);
        }
    }
}

/// Both panes side by side.
pub fn view(ui: &mut Ui, rect: Rect, tab: &mut FilesTab, ctx: &FilesCtx) {
    let cols = ui.split_h(rect, &[L::Frac(0.5), L::Frac(0.5)], 6);
    for side in 0..2 {
        pane::view(ui, cols[side], &mut tab.panes[side], side, ctx);
    }
}

#[cfg(test)]
pub(crate) mod tests;
