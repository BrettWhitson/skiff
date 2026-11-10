//! File-transfer tabs: two panes, each Local or any host.

use std::path::PathBuf;

use termax_core::askpass::{AskRequest, Broker};
use termax_core::fs::endpoint::Endpoint;
use termax_core::fs::transfer::TransferEvent;
use termax_core::fs::FileEntry;
use termax_core::fs::connect::RemoteConn;
use termax_core::hosts::HostEntry;
use termax_ui::geom::Rect;
use termax_ui::layout::Size as L;
use termax_ui::ui::Ui;

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
    Connected { seq: u64, result: Result<RemoteConn, String> },
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
    /// `ssh -V`; `None` if unknown (assumed new enough).
    pub ssh_version: Option<(u32, u32)>,
}

pub struct FilesTab {
    pub panes: [Pane; 2],
    pub transfers: TransferList,
}

impl FilesTab {
    pub fn new(left: Location, right: Location) -> Self {
        FilesTab { panes: [Pane::new(left), Pane::new(right)], transfers: TransferList::default() }
    }

    pub fn title(&self) -> String {
        format!("\u{21C5} {} \u{2194} {}", self.panes[0].loc.label(), self.panes[1].loc.label())
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

    pub fn on_transfer(&mut self, ev: TransferEvent, ctx: &FilesCtx) {
        if let Some(side) = self.transfers.on_event(ev)
            && let Some(p) = self.panes.get_mut(side)
            && p.ep.is_some()
        {
            p.refresh(ctx, side);
        }
        self.transfers.pump(ctx);
    }

    /// Each pane's location label and current connection (for retries).
    pub fn pane_endpoints(&self) -> [(String, Option<Endpoint>); 2] {
        [0, 1].map(|i| (self.panes[i].loc.label().to_string(), self.panes[i].ep.clone()))
    }

    /// Copies `items` (from pane `from`) into the other pane's folder.
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

    /// Files dropped from the OS onto pane `to`.
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

    pub fn on_event(&mut self, side: usize, ev: FilesEvent, ctx: &FilesCtx) {
        if let Some(p) = self.panes.get_mut(side) {
            p.on_event(ev, ctx, side);
        }
    }
}

/// Both panes side by side, the transfer list below.
pub fn view(ui: &mut Ui, rect: Rect, tab: &mut FilesTab, ctx: &FilesCtx) {
    let list_h = transfers::wanted_height(&tab.transfers);
    let rows = ui.split_v(rect, &[L::Fill, L::Px(list_h)], if list_h > 0 { 6 } else { 0 });
    let cols = ui.split_h(rows[0], &[L::Frac(0.5), L::Frac(0.5)], 6);
    let dropped: Vec<PathBuf> = ui
        .input()
        .events
        .iter()
        .filter_map(|e| if let termax_ui::input::Event::FileDropped(p) = e { Some(p.clone()) } else { None })
        .collect();
    for side in 0..2 {
        let out = pane::view(ui, cols[side], &mut tab.panes[side], side, ctx);
        if out.copy {
            let items = tab.panes[side].selected();
            tab.copy(side, items, ctx);
        }
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
