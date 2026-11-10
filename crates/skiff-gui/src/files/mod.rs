//! File-transfer tabs: two panes, each Local or any host, with the
//! transfer queue drawer below. `pane` draws one side, `ops` runs
//! listing/connect/file operations on the engine, and `transfers` queues
//! and shows copies.

use std::path::PathBuf;

use skiff_core::askpass::{AskRequest, Broker};
use skiff_core::fs::FileEntry;
use skiff_core::fs::connect::RemoteConn;
use skiff_core::fs::endpoint::Endpoint;
use skiff_core::fs::transfer::TransferEvent;
use skiff_core::hosts::HostEntry;
use skiff_ui::geom::Rect;
use skiff_ui::layout::Size as L;
use skiff_ui::style::tokens;
use skiff_ui::ui::Ui;
use skiff_ui::widgets::ToastKind;

use crate::engine::Engine;

pub mod ops;
pub mod pane;
pub mod transfers;

use transfers::{Spec, TransferList};

pub use pane::{Confirm, Pane, PaneState, Prompt, format_time};

/// The askpass tag naming pane `side` of tab `tab`.
pub fn tag(tab: u64, side: usize) -> String {
    format!("{tab}:{side}")
}

/// Splits an askpass tag made by `tag` back into (tab id, side); `None` if
/// it is not in that form.
pub fn parse_tag(tag: &str) -> Option<(u64, usize)> {
    let (t, s) = tag.split_once(':')?;
    Some((t.parse().ok()?, s.parse().ok()?))
}

/// Shows an askpass question in the pane it belongs to; refuses it if
/// that pane is gone.
pub fn route_ask(tab: &mut FilesTab, tab_id: u64, req: AskRequest) {
    match parse_tag(&req.tag) {
        Some((t, side)) if t == tab_id && side < 2 => tab.panes[side].ask(req),
        _ => {
            let _ = req.reply.send(None);
        }
    }
}

/// Where a pane browses: this machine or a configured host.
#[derive(Debug, Clone)]
pub enum Location {
    /// The local file system.
    Local,
    /// A host reached over SFTP.
    Host(Box<HostEntry>),
}

impl Location {
    /// The name shown for this location: "Local" or the host's name.
    pub fn label(&self) -> &str {
        match self {
            Location::Local => "Local",
            Location::Host(h) => &h.name,
        }
    }
}

/// Results of background file work for one pane.
pub enum FilesEvent {
    /// A folder listing for request `seq` of folder `cwd`.
    Listed { seq: u64, cwd: String, result: Result<Vec<FileEntry>, String> },
    /// A mkdir / rename / delete finished.
    OpDone { result: Result<(), String> },
    /// The SFTP connection attempt `seq` finished.
    Connected { seq: u64, result: Result<Box<RemoteConn>, String> },
}

/// What the file views need from the app for one frame.
pub struct FilesCtx<'a> {
    /// The id of the tab being drawn.
    pub tab_id: u64,
    /// Runs background work and delivers its results.
    pub engine: &'a Engine,
    /// Where Local panes start.
    pub start_dir: &'a str,
    /// For the location pickers.
    pub hosts: &'a [HostEntry],
    /// Routes ssh password/host-key questions to the GUI; `None` if unavailable.
    pub askpass: Option<&'a Broker>,
    /// The ssh program to launch.
    pub ssh_binary: &'a str,
    /// `ssh -V`; `None` if unknown (assumed new enough).
    pub ssh_version: Option<(u32, u32)>,
}

/// One file-transfer tab: a left and right pane plus their transfers.
pub struct FilesTab {
    /// Left (0) and right (1) panes.
    pub panes: [Pane; 2],
    /// Copies started from this tab.
    pub transfers: TransferList,
}

impl FilesTab {
    /// A tab with its two panes at `left` and `right` (they connect on first draw).
    pub fn new(left: Location, right: Location) -> Self {
        FilesTab { panes: [Pane::new(left), Pane::new(right)], transfers: TransferList::default() }
    }

    /// Tab title: "left ↔ right" from the two location labels (the tab strip adds the icon).
    pub fn title(&self) -> String {
        format!("{} \u{2194} {}", self.panes[0].loc.label(), self.panes[1].loc.label())
    }

    /// The tab is closing: cancel its transfers, stop its connections and
    /// withdraw its ssh questions (`retire` = the broker and this tab's id).
    pub fn shutdown(&mut self, retire: Option<(&Broker, u64)>) {
        self.transfers.cancel_all();
        for side in 0..2 {
            self.panes[side].cancel_connect();
            if let Some((b, id)) = retire {
                b.retire(&tag(id, side));
            }
        }
    }

    /// Feeds a transfer event to the list, refreshes the pane whose folder
    /// it changed, and starts queued transfers that can now run.
    pub fn on_transfer(&mut self, ev: TransferEvent, ctx: &FilesCtx) {
        if let Some(side) = self.transfers.on_event(ev)
            && let Some(p) = self.panes.get_mut(side)
            && p.ep.is_some()
        {
            p.refresh(ctx, side);
        }
        self.transfers.pump(ctx);
    }

    /// Takes the notices queued since the last call (finished or failed
    /// copy actions), for the app to show as toasts.
    pub fn take_notices(&mut self) -> Vec<(ToastKind, String)> {
        std::mem::take(&mut self.transfers.notices)
    }

    /// Each pane's location label and current connection (for retries).
    pub fn pane_endpoints(&self) -> [(String, Option<Endpoint>); 2] {
        [0, 1].map(|i| (self.panes[i].loc.label().to_string(), self.panes[i].ep.clone()))
    }

    /// Copies `items` (from pane `from`) into the other pane's folder, as
    /// one batch. Items are (path, is_dir, name) as from `Pane::selected`.
    pub fn copy(&mut self, from: usize, items: Vec<(String, bool, String)>, ctx: &FilesCtx) {
        let to = 1 - from;
        let (Some(src), Some(dst)) = (self.panes[from].ep.clone(), self.panes[to].ep.clone()) else { return };
        if items.is_empty() {
            return;
        }
        // Two panes on the same host each have their own connection, so the
        // engine cannot tell they are one file system: refuse here.
        if self.panes[from].loc.label() == self.panes[to].loc.label() && self.panes[from].cwd == self.panes[to].cwd {
            self.panes[to].banner = Some("Same folder: cannot copy onto itself".into());
            return;
        }
        let batch = self.transfers.new_batch();
        for (path, _, _) in items {
            let spec = Spec {
                src: src.clone(),
                src_path: path,
                src_side: Some(from),
                dst: dst.clone(),
                dst_dir: self.panes[to].cwd.clone(),
                dst_side: to,
                src_label: self.panes[from].loc.label().to_string(),
                dst_host: self.panes[to].loc.label().to_string(),
                dst_label: format!("{}:{}", self.panes[to].loc.label(), self.panes[to].cwd),
            };
            self.transfers.start(ctx, spec, batch);
        }
    }

    /// Uploads files dropped from the OS onto pane `to`, as one batch.
    pub fn upload(&mut self, to: usize, paths: Vec<PathBuf>, ctx: &FilesCtx) {
        let Some(dst) = self.panes[to].ep.clone() else { return };
        let batch = self.transfers.new_batch();
        for p in paths {
            let spec = Spec {
                src: Endpoint::Local,
                src_path: p.display().to_string(),
                src_side: None,
                dst: dst.clone(),
                dst_dir: self.panes[to].cwd.clone(),
                dst_side: to,
                src_label: "Local".into(),
                dst_host: self.panes[to].loc.label().to_string(),
                dst_label: format!("{}:{}", self.panes[to].loc.label(), self.panes[to].cwd),
            };
            self.transfers.start(ctx, spec, batch);
        }
    }

    /// Hands a background result to pane `side` (ignored if out of range).
    pub fn on_event(&mut self, side: usize, ev: FilesEvent, ctx: &FilesCtx) {
        if let Some(p) = self.panes.get_mut(side) {
            p.on_event(ev, ctx, side);
        }
    }
}

/// Draws both panes side by side, the transfer drawer below, and acts on
/// their copy requests, pane-to-pane drags and OS file drops.
pub fn view(ui: &mut Ui, rect: Rect, tab: &mut FilesTab, ctx: &FilesCtx) {
    // The drawer takes at most 40% of the tab (its rows scroll), never less
    // than its header and one row.
    let room = (rect.h as f32 / ui.scale() * 0.4) as i32;
    let list_h = transfers::wanted_height(&tab.transfers).min(room.max(transfers::MIN_OPEN_H));
    let gap = tokens::SPACE_SM;
    let rows = ui.split_v(rect, &[L::Fill, L::Px(list_h)], if list_h > 0 { gap } else { 0 });
    let cols = ui.split_h(rows[0], &[L::Frac(0.5), L::Frac(0.5)], gap);
    // Files dropped from the OS this frame.
    let dropped: Vec<PathBuf> = ui
        .input()
        .events
        .iter()
        .filter_map(|e| if let skiff_ui::input::Event::FileDropped(p) = e { Some(p.clone()) } else { None })
        .collect();
    for (side, &col) in cols.iter().enumerate().take(2) {
        let out = pane::view(ui, col, &mut tab.panes[side], side, ctx);
        if out.copy {
            let items = tab.panes[side].selected();
            tab.copy(side, items, ctx);
        }
        // Rows dragged from the other pane of this same tab: copy them here.
        if let Some(d) = out.dropped
            && d.tab == ctx.tab_id
            && d.side != side
        {
            tab.copy(d.side, d.items, ctx);
        }
    }
    if !dropped.is_empty() {
        // onto the pane under the pointer (the right pane if unknown)
        let m = ui.input().mouse;
        let to = if cols[0].contains(m) { 0 } else { 1 };
        tab.upload(to, dropped, ctx);
    }
    let eps = tab.pane_endpoints();
    transfers::view(ui, rows[1], &mut tab.transfers, ctx, eps);
}

#[cfg(test)]
pub(crate) mod tests;

#[cfg(test)]
mod tests_transfer;
