//! The transfer queue drawer under the panes: a collapsible card whose
//! header sums up the queue ("2 active · 1 failed"), with one row per copy
//! (name, route, size, rate, ETA, a 4 px progress bar, Cancel / Retry) and
//! inline overwrite questions ("apply to all" across one copy action).
//! Finished copy actions leave a notice for the app to show as a toast.

use std::collections::HashMap;
use std::time::Instant;

use skiff_core::fs::endpoint::Endpoint;
use skiff_core::fs::human_size;
use skiff_core::fs::transfer::{self, OverwriteDecision, OverwritePolicy, TransferEvent, TransferHandle, TransferRequest, TransferSummary};
use skiff_ui::a11y::Role;
use skiff_ui::geom::Rect;
use skiff_ui::icons::Icon;
use skiff_ui::input::{CursorIcon, Key, Modifiers};
use skiff_ui::layout::Size as L;
use skiff_ui::style::{TextRole, tokens};
use skiff_ui::ui::{Id, Sense, Ui};
use skiff_ui::widgets::{
    Align, ButtonKind, ButtonOpts, Elide, TextOpts, ToastKind, button, button_width, button_with, checkbox, icon_button, progress_bar,
    scroll_area, text,
};
use tokio::sync::oneshot;

use super::FilesCtx;
use crate::engine::Msg;

/// A drag of pane rows (the drag-and-drop payload).
#[derive(Debug, Clone)]
pub struct FileDrag {
    /// The files tab the rows came from.
    pub tab: u64,
    /// The pane the rows came from.
    pub side: usize,
    /// (path, is_dir, name)
    pub items: Vec<(String, bool, String)>,
}

/// What to copy and where (kept for Retry).
#[derive(Clone)]
pub struct Spec {
    /// Connection to read from.
    pub src: Endpoint,
    /// Full path of the file or folder to copy.
    pub src_path: String,
    /// The pane the files came from (`None`: dropped from the OS).
    pub src_side: Option<usize>,
    /// Connection to write to.
    pub dst: Endpoint,
    /// Folder the copy lands in.
    pub dst_dir: String,
    /// The pane to refresh when done.
    pub dst_side: usize,
    /// Source location label (host name or "Local").
    pub src_label: String,
    /// Destination location label (host name or "Local").
    pub dst_host: String,
    /// Destination shown in the list: "host:folder".
    pub dst_label: String,
}

impl Spec {
    /// Before a retry: a pane still showing the same location may have
    /// reconnected since, so use its current connection. Returns which
    /// endpoints (source, destination) were replaced.
    pub fn refresh_endpoints(&mut self, panes: [(String, Option<Endpoint>); 2]) -> (bool, bool) {
        // OS drops (no source pane) keep their Local source.
        let mut src = false;
        if let Some(s) = self.src_side
            && let Some((label, Some(ep))) = panes.get(s)
            && *label == self.src_label
        {
            self.src = ep.clone();
            src = true;
        }
        let mut dst = false;
        if let Some((label, Some(ep))) = panes.get(self.dst_side)
            && *label == self.dst_host
        {
            self.dst = ep.clone();
            dst = true;
        }
        (src, dst)
    }
}

/// Transfers running at once per tab; the rest wait in the list.
pub const MAX_RUNNING: usize = 4;

/// An "already exists, replace it?" question from a running transfer.
pub struct Question {
    /// The destination path that already exists.
    pub path: String,
    /// Where the answer goes; taken once answered (dropping it cancels).
    reply: Option<oneshot::Sender<OverwriteDecision>>,
    /// The "Apply to all" box.
    pub apply_all: bool,
}

/// Where one transfer row is in its life.
pub enum RowState {
    /// Waiting for a free slot (at most `MAX_RUNNING` run at once).
    Queued,
    /// Copying.
    Running,
    /// Paused on an overwrite question.
    Waiting(Question),
    /// Finished; the summary counts skipped files.
    Done(TransferSummary),
    /// Failed with this message (can be retried).
    Failed(String),
    /// Stopped by the user (or by closing the tab).
    Cancelled,
}

/// One copy in the transfer list.
pub struct TransferRow {
    /// Engine transfer id; a retry gets a new one.
    pub id: u64,
    /// The copy action this row belongs to (for "apply to all").
    batch: u64,
    pub spec: Spec,
    /// Bytes copied so far.
    pub done: u64,
    /// Total bytes, once known.
    pub total: Option<u64>,
    /// The file being copied now.
    pub current: String,
    /// Bytes per second (smoothed).
    pub speed: f64,
    /// Time and byte count of the last speed sample.
    last: Option<(Instant, u64)>,
    pub state: RowState,
    /// Cancels the running transfer; `None` while queued or finished.
    handle: Option<TransferHandle>,
    /// The user pressed Cancel: however it ends, it ends "cancelled".
    cancelled: bool,
}

impl TransferRow {
    /// Not finished yet (queued, running or asking).
    pub fn is_active(&self) -> bool {
        matches!(self.state, RowState::Queued | RowState::Running | RowState::Waiting(_))
    }

    /// Holds a running slot (running or asking; not queued).
    fn in_flight(&self) -> bool {
        matches!(self.state, RowState::Running | RowState::Waiting(_))
    }
}

/// Most notices kept for the app to pick up (older ones are dropped).
const MAX_NOTICES: usize = 8;

/// A tab's transfers, oldest first, with the running-slot limit and
/// per-batch "apply to all" answers.
#[derive(Default)]
pub struct TransferList {
    pub rows: Vec<TransferRow>,
    /// The drawer shows only its header.
    pub collapsed: bool,
    /// Finished copy actions to tell the user about (as toasts), oldest first.
    pub notices: Vec<(ToastKind, String)>,
    /// Last transfer id handed out.
    next_id: u64,
    /// Last batch id handed out.
    next_batch: u64,
    /// "Apply to all" answers, per copy action.
    batch_policy: HashMap<u64, OverwriteDecision>,
}

impl TransferList {
    /// A fresh batch id for one copy action.
    pub fn new_batch(&mut self) -> u64 {
        self.collapsed = false; // show the new copies
        self.next_batch += 1;
        self.next_batch
    }

    /// Starts transfer `id` on the engine (asking before overwriting) and
    /// forwards its events to the app as `Msg::Transfer`.
    fn launch(ctx: &FilesCtx, id: u64, spec: &Spec) -> TransferHandle {
        let (etx, mut erx) = tokio::sync::mpsc::unbounded_channel();
        let tx = ctx.engine.sender();
        let tab = ctx.tab_id;
        // Relay task: tags each event with this tab.
        ctx.engine.rt.spawn(async move {
            while let Some(ev) = erx.recv().await {
                tx.send(Msg::Transfer { tab, ev });
            }
        });
        let req = TransferRequest {
            id,
            src: spec.src.clone(),
            src_path: spec.src_path.clone(),
            dst: spec.dst.clone(),
            dst_dir: spec.dst_dir.clone(),
        };
        transfer::start(ctx.engine.rt.handle(), req, OverwritePolicy::Ask, etx)
    }

    /// Adds a row for `spec` in `batch`, running it now if a slot is free
    /// and queueing it otherwise; returns its id.
    pub fn start(&mut self, ctx: &FilesCtx, spec: Spec, batch: u64) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        let free = self.in_flight() < MAX_RUNNING;
        let handle = free.then(|| Self::launch(ctx, id, &spec));
        self.rows.push(TransferRow {
            id,
            batch,
            spec,
            done: 0,
            total: None,
            current: String::new(),
            speed: 0.0,
            last: None,
            state: if free { RowState::Running } else { RowState::Queued },
            handle,
            cancelled: false,
        });
        id
    }

    /// How many rows hold a running slot.
    fn in_flight(&self) -> usize {
        self.rows.iter().filter(|r| r.in_flight()).count()
    }

    /// Starts queued transfers while there are free slots.
    pub fn pump(&mut self, ctx: &FilesCtx) {
        let mut free = MAX_RUNNING.saturating_sub(self.in_flight());
        for row in self.rows.iter_mut().filter(|r| matches!(r.state, RowState::Queued)) {
            if free == 0 {
                break;
            }
            row.handle = Some(Self::launch(ctx, row.id, &row.spec));
            row.state = RowState::Running;
            free -= 1;
        }
    }

    /// Restarts row `i` under a new id (so late events from the old run are
    /// ignored), using the panes' current connections where they still match.
    fn retry(&mut self, i: usize, ctx: &FilesCtx, panes: [(String, Option<Endpoint>); 2]) {
        self.next_id += 1;
        let id = self.next_id;
        let free = self.in_flight() < MAX_RUNNING;
        let row = &mut self.rows[i];
        row.spec.refresh_endpoints(panes);
        row.handle = free.then(|| Self::launch(ctx, id, &row.spec));
        row.id = id;
        row.state = if free { RowState::Running } else { RowState::Queued };
        row.cancelled = false;
        row.done = 0;
        row.total = None;
        row.speed = 0.0;
        row.last = None;
    }

    /// Cancels every unfinished transfer, queued ones included (closing the tab).
    pub fn cancel_all(&mut self) {
        for i in 0..self.rows.len() {
            if self.rows[i].is_active() {
                self.cancel(i);
            }
        }
    }

    /// How many rows are unfinished (queued, running or asking).
    pub fn running(&self) -> usize {
        self.rows.iter().filter(|r| r.is_active()).count()
    }

    /// Applies an engine event; returns the pane to refresh when one finished.
    pub fn on_event(&mut self, ev: TransferEvent) -> Option<usize> {
        match ev {
            TransferEvent::Progress { id, done, total, current } => {
                let row = self.rows.iter_mut().find(|r| r.id == id)?;
                let now = Instant::now();
                // Speed: sample at most every 50 ms, smoothed with an
                // exponential moving average (30% new sample).
                if let Some((t, d)) = row.last {
                    let dt = now.duration_since(t).as_secs_f64();
                    if dt >= 0.05 {
                        let inst = done.saturating_sub(d) as f64 / dt;
                        row.speed = if row.speed == 0.0 { inst } else { 0.7 * row.speed + 0.3 * inst };
                        row.last = Some((now, done));
                    }
                } else {
                    row.last = Some((now, done));
                }
                row.done = done;
                row.total = total;
                row.current = current;
                None
            }
            TransferEvent::NeedsDecision { id, path, reply } => {
                let row = self.rows.iter_mut().find(|r| r.id == id)?;
                // An earlier "apply to all" in this batch answers at once.
                match self.batch_policy.get(&row.batch) {
                    Some(d) => {
                        let _ = reply.send(*d);
                    }
                    None => row.state = RowState::Waiting(Question { path, reply: Some(reply), apply_all: false }),
                }
                None
            }
            TransferEvent::Done { id, result } => {
                let row = self.rows.iter_mut().find(|r| r.id == id)?;
                row.handle = None;
                // A user cancel wins over however the transfer reported ending.
                row.state = match result {
                    _ if row.cancelled => RowState::Cancelled,
                    Ok(s) if s.cancelled => RowState::Cancelled,
                    Ok(s) => RowState::Done(s),
                    Err(e) => RowState::Failed(e),
                };
                let (side, batch) = (row.spec.dst_side, row.batch);
                self.notify_if_finished(batch);
                // Refresh the destination once its last transfer ends.
                let more = self.rows.iter().any(|r| r.is_active() && r.spec.dst_side == side);
                (!more).then_some(side)
            }
        }
    }

    /// When copy action `batch` has nothing left running, queues one notice
    /// for it: the failures if any copy failed, else what was copied where
    /// (an action that was only cancelled says nothing).
    fn notify_if_finished(&mut self, batch: u64) {
        let rows: Vec<&TransferRow> = self.rows.iter().filter(|r| r.batch == batch).collect();
        if rows.iter().any(|r| r.is_active()) {
            return;
        }
        let failed: Vec<&&TransferRow> = rows.iter().filter(|r| matches!(r.state, RowState::Failed(_))).collect();
        let done = rows.iter().filter(|r| matches!(r.state, RowState::Done(_))).count();
        let dst = rows.first().map(|r| r.spec.dst_label.clone()).unwrap_or_default();
        let notice = match (failed.as_slice(), done) {
            ([one], 0) => {
                let why = if let RowState::Failed(e) = &one.state { e.as_str() } else { "" };
                Some((ToastKind::Error, format!("Copying {} failed: {why}", file_name(&one.spec.src_path))))
            }
            ([], 0) => None,
            ([], 1) => {
                let one = rows.iter().find(|r| matches!(r.state, RowState::Done(_))).map_or("", |r| file_name(&r.spec.src_path));
                Some((ToastKind::Success, format!("Copied {one} to {dst}")))
            }
            ([], n) => Some((ToastKind::Success, format!("Copied {n} items to {dst}"))),
            (f, _) => Some((ToastKind::Error, format!("{} of {} copies to {dst} failed", f.len(), f.len() + done))),
        };
        if let Some(n) = notice {
            self.notices.push(n);
            if self.notices.len() > MAX_NOTICES {
                self.notices.remove(0);
            }
        }
    }

    /// Answers row `i`'s question; with "apply to all" the answer also goes
    /// to every other question (now or later) from the same copy action.
    fn decide(&mut self, i: usize, overwrite: bool) {
        let batch = self.rows[i].batch;
        let RowState::Waiting(q) = &mut self.rows[i].state else { return };
        let all = q.apply_all;
        let d = match (overwrite, all) {
            (true, false) => OverwriteDecision::Overwrite,
            (false, false) => OverwriteDecision::Skip,
            (true, true) => OverwriteDecision::OverwriteAll,
            (false, true) => OverwriteDecision::SkipAll,
        };
        if let Some(r) = q.reply.take() {
            let _ = r.send(d);
        }
        self.rows[i].state = RowState::Running;
        // Remember the answer for later questions and answer the open ones.
        if all {
            self.batch_policy.insert(batch, d);
            for r in self.rows.iter_mut().filter(|r| r.batch == batch) {
                if let RowState::Waiting(q) = &mut r.state {
                    if let Some(rep) = q.reply.take() {
                        let _ = rep.send(d);
                    }
                    r.state = RowState::Running;
                }
            }
        }
    }

    /// Cancels row `i`: a queued row ends at once; a running one is told to
    /// stop and shows "cancelled" when its Done event arrives.
    fn cancel(&mut self, i: usize) {
        let r = &mut self.rows[i];
        if matches!(r.state, RowState::Queued) {
            r.state = RowState::Cancelled;
            return;
        }
        r.cancelled = true;
        if let Some(h) = &r.handle {
            h.cancel();
        }
        if let RowState::Waiting(q) = &mut r.state {
            q.reply.take(); // dropping the answer cancels too
            r.state = RowState::Running; // until the Done event arrives
        }
    }
}

/// The last part of a path (either separator).
fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// The drawer header's summary: "2 active · 1 failed", or "3 done" when
/// nothing is active or failed ("" when every row was cancelled).
pub fn summary(list: &TransferList) -> String {
    let active = list.running();
    let failed = list.rows.iter().filter(|r| matches!(r.state, RowState::Failed(_))).count();
    let done = list.rows.iter().filter(|r| matches!(r.state, RowState::Done(_))).count();
    let mut parts = vec![];
    if active > 0 {
        parts.push(format!("{active} active"));
    }
    if failed > 0 {
        parts.push(format!("{failed} failed"));
    }
    if parts.is_empty() && done > 0 {
        parts.push(format!("{done} done"));
    }
    parts.join(SEP)
}

/// Separator between the parts of a summary or status line.
const SEP: &str = " \u{00B7} ";

/// Time left as "8s left", "3m 12s left" or "1h 5m left".
pub fn format_eta(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{s}s left"),
        s if s < 3600 => format!("{}m {}s left", s / 60, s % 60),
        s => format!("{}h {}m left", s / 3600, s % 3600 / 60),
    }
}

/// A running row's progress line: "1.2M of 3.4M · 5.1M/s · 8s left" (parts
/// that are not known yet are left out).
pub fn progress_text(row: &TransferRow) -> String {
    let mut parts = vec![match row.total {
        Some(t) => format!("{} of {}", human_size(row.done), human_size(t)),
        None => human_size(row.done),
    }];
    if row.speed >= 1.0 {
        parts.push(format!("{}/s", human_size(row.speed as u64)));
        if let Some(t) = row.total {
            parts.push(format_eta((t.saturating_sub(row.done) as f64 / row.speed).ceil() as u64));
        }
    }
    parts.join(SEP)
}

/// Height of the drawer header, logical px.
const HEAD_H: i32 = 36;
/// Height of one transfer row, logical px.
const ROW_H: i32 = 48;
/// Rows shown before the list scrolls.
const MAX_ROWS: i32 = 4;
/// The smallest open drawer: the header and one row, logical px.
pub const MIN_OPEN_H: i32 = HEAD_H + ROW_H + 2;

/// Height the drawer wants (0 when there are no transfers), logical px.
pub fn wanted_height(list: &TransferList) -> i32 {
    match list.rows.len() as i32 {
        0 => 0,
        _ if list.collapsed => HEAD_H,
        n => HEAD_H + ROW_H * n.min(MAX_ROWS) + 2,
    }
}

/// Draws the drawer (header with the collapse toggle, summary and "Clear
/// finished", then one scrolling row per transfer) and applies the buttons
/// pressed. `panes` is used by Retry.
pub fn view(ui: &mut Ui, rect: Rect, list: &mut TransferList, ctx: &FilesCtx, panes: [(String, Option<Endpoint>); 2]) {
    if list.rows.is_empty() || rect.h <= 0 {
        return;
    }
    let p = ui.style().palette;
    let id = Id::new(("transfers", ctx.tab_id));
    let radius = tokens::RADIUS_MD * ui.scale();
    let bw = ui.px(1).max(1);
    {
        let mut painter = ui.painter();
        painter.rect(rect, p.surface, radius);
        painter.border(rect, p.border, bw, radius);
    }
    let pad = ui.px(tokens::SPACE_SM);
    let head = Rect::new(rect.x + bw, rect.y + bw, rect.w - 2 * bw, ui.px(HEAD_H) - bw);

    // Header: the strip left of "Clear finished" toggles the drawer (a
    // focusable button for keyboards, announced with the summary).
    let tid = id.with("toggle");
    let finished = list.rows.iter().any(|r| !r.is_active());
    let clear_w = button_width(ui, "Clear finished", 0);
    let ch = ui.px(tokens::ROW_H);
    let clear_r = Rect::new(head.right() - pad - clear_w, head.y + (head.h - ch) / 2, clear_w, ch);
    let toggle_r = Rect::new(head.x, head.y, clear_r.x - head.x - pad, head.h);
    let mut tr = ui.interact(tid, toggle_r, Sense::focusable());
    if tr.focused && ui.keys_for(tid) {
        let enter = ui.input_mut().take_key(&Key::Enter, Modifiers::NONE);
        tr.clicked |= enter || ui.input_mut().take_key(&Key::Space, Modifiers::NONE);
    }
    let sum = summary(list);
    let name = if sum.is_empty() { "Transfers".to_string() } else { format!("Transfers, {sum}") };
    if let Some(n) = ui.a11y_node(tid, Role::Button, &name) {
        n.expanded = Some(!list.collapsed);
    }
    if tr.hovered {
        ui.set_cursor_icon(CursorIcon::Pointer);
    }
    if tr.clicked {
        list.collapsed = !list.collapsed;
    }
    let s = ui.px(tokens::ICON);
    let mut x = head.x + pad;
    let chevron = if list.collapsed { Icon::ChevronUp } else { Icon::ChevronDown };
    ui.painter().icon(Rect::new(x, head.y + (head.h - s) / 2, s, s), chevron, if tr.hovered { p.fg } else { p.muted });
    x += s + pad;
    let strong = TextOpts { role: Some(TextRole::Strong), ..Default::default() };
    x += text(ui, Rect::new(x, head.y, (toggle_r.right() - x).max(0), head.h), "Transfers", strong) + pad;
    // Summary pieces; failures in the danger colour.
    let caption = |c| TextOpts { role: Some(TextRole::Caption), color: Some(c), ..Default::default() };
    for (k, piece) in sum.split(SEP).filter(|s| !s.is_empty()).enumerate() {
        if k > 0 {
            x += text(ui, Rect::new(x, head.y, (toggle_r.right() - x).max(0), head.h), SEP, caption(p.muted));
        }
        let c = if piece.ends_with("failed") { p.danger } else { p.muted };
        x += text(ui, Rect::new(x, head.y, (toggle_r.right() - x).max(0), head.h), piece, caption(c));
    }
    let ghost = ButtonOpts { kind: ButtonKind::Ghost, enabled: finished, ..Default::default() };
    if button_with(ui, id.with("clear"), clear_r, "Clear finished", ghost).clicked {
        list.rows.retain(|r| r.is_active());
    }
    if list.collapsed || list.rows.is_empty() {
        return;
    }
    ui.painter().rect(Rect::new(head.x, head.bottom(), head.w, bw), p.border, 0.0);
    let body = Rect::new(head.x, head.bottom() + bw, head.w, rect.bottom() - bw - head.bottom() - bw);

    let row_h = ui.px(ROW_H);
    let content = row_h * list.rows.len() as i32;
    // Button presses are collected while the rows are borrowed and applied after.
    let mut actions: Vec<(usize, &'static str)> = vec![];
    scroll_area(ui, id.with("scroll"), body, content, |ui, vis, off| {
        for (i, row) in list.rows.iter_mut().enumerate() {
            let r = Rect::new(vis.x, vis.y + i as i32 * row_h - off, vis.w, row_h);
            // Skip rows scrolled out of view.
            if r.bottom() < vis.y || r.y > vis.bottom() {
                continue;
            }
            if i > 0 {
                ui.painter().rect(Rect::new(r.x + pad, r.y, r.w - 2 * pad, bw), p.border, 0.0);
            }
            let inner = Rect::new(r.x + pad + ui.px(tokens::SPACE_XS), r.y, r.w - 2 * pad - ui.px(tokens::SPACE_XS), r.h);
            row_view(ui, id.with(("row", i)), inner, row, i, &mut actions);
        }
    });
    for (i, a) in actions {
        match a {
            "overwrite" => list.decide(i, true),
            "skip" => list.decide(i, false),
            "cancel" => list.cancel(i),
            "retry" => list.retry(i, ctx, panes.clone()),
            _ => {}
        }
    }
}

/// One transfer row in `r`: icon, name and route on the first line with the
/// status at the right, a 4 px progress bar below, and Cancel / Retry. An
/// overwrite question takes the whole row instead.
fn row_view(ui: &mut Ui, rid: Id, r: Rect, row: &mut TransferRow, i: usize, actions: &mut Vec<(usize, &'static str)>) {
    let p = ui.style().palette;
    let pad = ui.px(tokens::SPACE_SM);
    let s = ui.px(tokens::ICON);
    let ctl = ui.px(tokens::CONTROL_H);
    let by = r.y + (r.h - ctl) / 2;
    let name = file_name(&row.spec.src_path).to_string();
    let caption = |c| TextOpts { role: Some(TextRole::Caption), color: Some(c), ..Default::default() };
    // The overwrite question replaces the whole line.
    if let RowState::Waiting(q) = &mut row.state {
        let cols = ui.split_h(r, &[L::Fill, L::Px(96), L::Px(64), L::Px(124), L::Px(76)], tokens::SPACE_XS);
        let line = |c: Rect| Rect::new(c.x, by, c.w, ctl);
        ui.painter().icon(Rect::new(cols[0].x, r.y + (r.h - s) / 2, s, s), Icon::Warning, p.warning);
        let qname = file_name(&q.path).to_string();
        let tr = Rect::new(cols[0].x + s + pad, r.y, cols[0].w - s - pad, r.h);
        text(ui, tr, &format!("{qname} exists \u{2014} replace it?"), TextOpts { elide: Elide::Middle, ..Default::default() });
        if button_with(ui, rid.with("ow"), line(cols[1]), "Overwrite", ButtonOpts::kind(ButtonKind::Primary)).clicked {
            actions.push((i, "overwrite"));
        }
        if button(ui, rid.with("skip"), line(cols[2]), "Skip").clicked {
            actions.push((i, "skip"));
        }
        checkbox(ui, rid.with("all"), line(cols[3]), &mut q.apply_all, "Apply to all");
        if button_with(ui, rid.with("cancel"), line(cols[4]), "Cancel", ButtonOpts::kind(ButtonKind::Ghost)).clicked {
            actions.push((i, "cancel"));
        }
        return;
    }
    // Right end: Retry (failed) and/or a Cancel icon (unfinished).
    let mut right = r.right();
    if row.is_active() {
        right -= ctl;
        let label = format!("Cancel copying {name}");
        if icon_button(ui, rid.with("cancel"), Rect::new(right, by, ctl, ctl), Icon::Close, &label, ButtonKind::Ghost).clicked {
            actions.push((i, "cancel"));
        }
    }
    if matches!(row.state, RowState::Failed(_)) {
        let w = button_width(ui, "Retry", 1);
        right -= w;
        let opts = ButtonOpts { icon: Some(Icon::Refresh), ..Default::default() };
        if button_with(ui, rid.with("retry"), Rect::new(right, by, w, ctl), "Retry", opts).clicked {
            actions.push((i, "retry"));
        }
    }
    right -= pad;
    // Line 1: icon, name, route; status on the right.
    let lh = ui.px(20);
    let y1 = r.y + (r.h - lh - ui.px(10)) / 2;
    let (icon, icon_c) = match row.state {
        RowState::Failed(_) => (Icon::Error, p.danger),
        RowState::Done(_) => (Icon::Check, p.success),
        _ => (Icon::Transfer, p.muted),
    };
    ui.painter().icon(Rect::new(r.x, y1 + (lh - s) / 2, s, s), icon, icon_c);
    let tx = r.x + s + pad + ui.px(tokens::SPACE_XS);
    let (status, sc) = match &row.state {
        RowState::Running => (progress_text(row), p.muted),
        RowState::Queued => ("Queued".to_string(), p.muted),
        RowState::Done(sum) if sum.skipped > 0 => (format!("Done, {} skipped", sum.skipped), p.success),
        RowState::Done(_) => (row.total.map_or("Done".to_string(), |t| format!("Done{SEP}{}", human_size(t))), p.success),
        RowState::Cancelled => ("Cancelled".to_string(), p.muted),
        RowState::Failed(e) => (e.clone(), p.danger),
        RowState::Waiting(_) => (String::new(), p.muted),
    };
    // The status gets up to half the line; the name and route take the rest.
    let avail = (right - tx).max(0);
    let (sfont, spx) = ui.text_style(TextRole::Caption);
    let sw = (ui.measure(&status, sfont, spx).ceil() as i32).min(avail / 2);
    let status_r = Rect::new(right - sw, y1, sw, lh);
    text(ui, status_r, &status, TextOpts { align: Align::End, elide: Elide::End, ..caption(sc) });
    let name_room = (status_r.x - pad - tx).max(0);
    let strong = TextOpts { role: Some(TextRole::Strong), elide: Elide::Middle, ..Default::default() };
    let nw = text(ui, Rect::new(tx, y1, name_room, lh), &name, strong);
    let route = format!("{} \u{2192} {}", row.spec.src_label, row.spec.dst_label);
    let rx = tx + nw + pad;
    if rx < status_r.x - pad {
        text(ui, Rect::new(rx, y1, status_r.x - pad - rx, lh), &route, TextOpts { elide: Elide::Middle, ..caption(p.muted) });
    }
    // Line 2: the 4 px bar (striped while the size is unknown).
    let bar = Rect::new(tx, y1 + lh + ui.px(6), (right - tx).max(0), ui.px(4).max(2));
    let track = p.surface_alt.mix(p.fg, 0.08);
    let radius = bar.h as f32 / 2.0;
    let fill = |ui: &mut Ui, f: f32, c| {
        ui.painter().rect(bar, track, radius);
        let w = (bar.w as f32 * f.clamp(0.0, 1.0)).round() as i32;
        if w > 0 {
            ui.painter().rect(Rect::new(bar.x, bar.y, w, bar.h), c, radius);
        }
    };
    match &row.state {
        RowState::Running => match row.total.filter(|t| *t > 0) {
            Some(t) => fill(ui, row.done as f32 / t as f32, p.accent),
            None => progress_bar(ui, bar, None),
        },
        RowState::Queued => fill(ui, 0.0, p.accent),
        RowState::Done(_) => fill(ui, 1.0, p.success),
        RowState::Failed(_) => fill(ui, 1.0, p.danger),
        _ => fill(ui, 0.0, p.muted),
    }
    if let Some(n) = ui.a11y_node(rid, Role::Label, &format!("{name}, {route}: {status}")) {
        n.bounds = r;
    }
}
