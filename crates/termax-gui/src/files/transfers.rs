//! The transfer list under the panes: progress, speed, cancel, retry and
//! inline overwrite questions (with "apply to all" across one copy action).

use std::collections::HashMap;
use std::time::Instant;

use termax_core::fs::endpoint::Endpoint;
use termax_core::fs::human_size;
use termax_core::fs::transfer::{
    self, OverwriteDecision, OverwritePolicy, TransferEvent, TransferHandle, TransferRequest, TransferSummary,
};
use termax_ui::geom::Rect;
use termax_ui::layout::Size as L;
use termax_ui::text::FontKind;
use termax_ui::ui::{Id, Ui};
use termax_ui::widgets::{button, checkbox, elide, label, progress_bar, scroll_area};
use tokio::sync::oneshot;

use super::FilesCtx;
use crate::engine::Msg;

/// A drag of pane rows (the drag-and-drop payload).
#[derive(Debug, Clone)]
pub struct FileDrag {
    pub tab: u64,
    pub side: usize,
    /// (path, is_dir, name)
    pub items: Vec<(String, bool, String)>,
}

/// What to copy and where (kept for Retry).
#[derive(Clone)]
pub struct Spec {
    pub src: Endpoint,
    pub src_path: String,
    pub dst: Endpoint,
    pub dst_dir: String,
    /// The pane to refresh when done.
    pub dst_side: usize,
    pub src_label: String,
    pub dst_label: String,
}

pub struct Question {
    pub path: String,
    reply: Option<oneshot::Sender<OverwriteDecision>>,
    pub apply_all: bool,
}

pub enum RowState {
    Running,
    Waiting(Question),
    Done(TransferSummary),
    Failed(String),
    Cancelled,
}

pub struct TransferRow {
    pub id: u64,
    batch: u64,
    pub spec: Spec,
    pub done: u64,
    pub total: Option<u64>,
    pub current: String,
    /// Bytes per second (smoothed).
    pub speed: f64,
    last: Option<(Instant, u64)>,
    pub state: RowState,
    handle: Option<TransferHandle>,
    /// The user pressed Cancel: however it ends, it ends "cancelled".
    cancelled: bool,
}

impl TransferRow {
    pub fn is_active(&self) -> bool {
        matches!(self.state, RowState::Running | RowState::Waiting(_))
    }
}

#[derive(Default)]
pub struct TransferList {
    pub rows: Vec<TransferRow>,
    next_id: u64,
    next_batch: u64,
    /// "Apply to all" answers, per copy action.
    batch_policy: HashMap<u64, OverwriteDecision>,
}

impl TransferList {
    pub fn new_batch(&mut self) -> u64 {
        self.next_batch += 1;
        self.next_batch
    }

    fn launch(ctx: &FilesCtx, id: u64, spec: &Spec) -> TransferHandle {
        let (etx, mut erx) = tokio::sync::mpsc::unbounded_channel();
        let tx = ctx.engine.sender();
        let tab = ctx.tab_id;
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

    pub fn start(&mut self, ctx: &FilesCtx, spec: Spec, batch: u64) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        let handle = Self::launch(ctx, id, &spec);
        self.rows.push(TransferRow {
            id,
            batch,
            spec,
            done: 0,
            total: None,
            current: String::new(),
            speed: 0.0,
            last: None,
            state: RowState::Running,
            handle: Some(handle),
            cancelled: false,
        });
        id
    }

    fn retry(&mut self, i: usize, ctx: &FilesCtx) {
        self.next_id += 1;
        let id = self.next_id;
        let row = &mut self.rows[i];
        row.handle = Some(Self::launch(ctx, id, &row.spec));
        row.id = id;
        row.state = RowState::Running;
        row.cancelled = false;
        row.done = 0;
        row.total = None;
        row.speed = 0.0;
        row.last = None;
    }

    /// Cancels every running transfer (closing the tab).
    pub fn cancel_all(&mut self) {
        for i in 0..self.rows.len() {
            if self.rows[i].is_active() {
                self.cancel(i);
            }
        }
    }

    pub fn running(&self) -> usize {
        self.rows.iter().filter(|r| r.is_active()).count()
    }

    /// Applies an engine event; returns the pane to refresh when one finished.
    pub fn on_event(&mut self, ev: TransferEvent) -> Option<usize> {
        match ev {
            TransferEvent::Progress { id, done, total, current } => {
                let row = self.rows.iter_mut().find(|r| r.id == id)?;
                let now = Instant::now();
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
                row.state = match result {
                    _ if row.cancelled => RowState::Cancelled,
                    Ok(s) if s.cancelled => RowState::Cancelled,
                    Ok(s) => RowState::Done(s),
                    Err(e) => RowState::Failed(e),
                };
                Some(row.spec.dst_side)
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

    fn cancel(&mut self, i: usize) {
        let r = &mut self.rows[i];
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

/// Height the transfer list wants (0 when empty), logical px.
pub fn wanted_height(list: &TransferList) -> i32 {
    if list.rows.is_empty() { 0 } else { (34 + 32 * list.rows.len() as i32).min(200) }
}

pub fn view(ui: &mut Ui, rect: Rect, list: &mut TransferList, ctx: &FilesCtx) {
    if list.rows.is_empty() || rect.h <= 0 {
        return;
    }
    let p = ui.style().palette;
    let id = Id::new(("transfers", ctx.tab_id));
    let radius = ui.px(6) as f32;
    ui.painter().rect(rect, p.surface, radius);
    let inner = rect.inset(ui.px(6));
    let parts = ui.split_v(inner, &[L::Px(26), L::Fill], 4);
    let head = ui.split_h(parts[0], &[L::Fill, L::Px(150)], 6);
    let running = list.running();
    let title = if running > 0 { format!("Transfers \u{00B7} {running} running") } else { "Transfers".to_string() };
    label(ui, head[0], &title, Some(p.muted));
    if button(ui, id.with("clear"), head[1], "Clear finished").clicked {
        list.rows.retain(|r| r.is_active());
    }
    let row_h = ui.px(32);
    let content = row_h * list.rows.len() as i32;
    let mut actions: Vec<(usize, &'static str)> = vec![];
    scroll_area(ui, id.with("scroll"), parts[1], content, |ui, vis, off| {
        let px = ui.font_px();
        for (i, row) in list.rows.iter_mut().enumerate() {
            let r = Rect::new(vis.x, vis.y + i as i32 * row_h - off, vis.w, row_h - ui.px(4));
            if r.bottom() < vis.y || r.y > vis.bottom() {
                continue;
            }
            let rid = id.with(("row", i));
            let name = row.spec.src_path.rsplit(['/', '\\']).next().unwrap_or("").to_string();
            let what = format!("{name}  {} \u{2192} {}", row.spec.src_label, row.spec.dst_label);
            match &mut row.state {
                RowState::Waiting(q) => {
                    let cols = ui.split_h(r, &[L::Fill, L::Px(90), L::Px(64), L::Px(140), L::Px(76)], 6);
                    let qname = q.path.rsplit(['/', '\\']).next().unwrap_or(&q.path).to_string();
                    label(ui, cols[0], &format!("{qname} exists \u{2014} replace it?"), Some(p.warning));
                    if button(ui, rid.with("ow"), cols[1], "Overwrite").clicked {
                        actions.push((i, "overwrite"));
                    }
                    if button(ui, rid.with("skip"), cols[2], "Skip").clicked {
                        actions.push((i, "skip"));
                    }
                    checkbox(ui, rid.with("all"), cols[3], &mut q.apply_all, "Apply to all");
                    if button(ui, rid.with("cancel"), cols[4], "Cancel").clicked {
                        actions.push((i, "cancel"));
                    }
                }
                state => {
                    let cols = ui.split_h(r, &[L::Fill, L::Px(150), L::Px(90), L::Px(76)], 6);
                    let shown = elide(ui, &what, FontKind::Ui, px, cols[0].w);
                    label(ui, cols[0], &shown, None);
                    let bar = Rect::new(cols[1].x, cols[1].y + cols[1].h / 2 - ui.px(4), cols[1].w, ui.px(8));
                    match state {
                        RowState::Running => {
                            let f = row.total.filter(|t| *t > 0).map(|t| row.done as f32 / t as f32);
                            progress_bar(ui, bar, f);
                            label(ui, cols[2], &format!("{}/s", human_size(row.speed as u64)), Some(p.muted));
                            if button(ui, rid.with("cancel"), cols[3], "Cancel").clicked {
                                actions.push((i, "cancel"));
                            }
                        }
                        RowState::Done(s) => {
                            progress_bar(ui, bar, Some(1.0));
                            let t = if s.skipped > 0 { format!("done, {} skipped", s.skipped) } else { "done".into() };
                            label(ui, cols[2], &t, Some(p.success));
                        }
                        RowState::Cancelled => {
                            label(ui, Rect::new(cols[1].x, cols[1].y, cols[1].w + cols[2].w, cols[1].h), "cancelled", Some(p.muted));
                        }
                        RowState::Failed(e) => {
                            let span = Rect::new(cols[1].x, cols[1].y, cols[2].right() - cols[1].x, cols[1].h);
                            let shown = elide(ui, e, FontKind::Ui, px, span.w);
                            label(ui, span, &shown, Some(p.danger));
                            if button(ui, rid.with("retry"), cols[3], "Retry").clicked {
                                actions.push((i, "retry"));
                            }
                        }
                        RowState::Waiting(_) => {}
                    }
                }
            }
        }
    });
    for (i, a) in actions {
        match a {
            "overwrite" => list.decide(i, true),
            "skip" => list.decide(i, false),
            "cancel" => list.cancel(i),
            "retry" => list.retry(i, ctx),
            _ => {}
        }
    }
}
