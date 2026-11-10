//! Toast notifications: a stack of up to three boxes at the bottom right
//! (360 px wide). Info and success toasts dismiss themselves after 4 s,
//! warnings after 8 s, errors stay until closed; the countdown pauses
//! while the pointer is over the stack or focus is inside it. Each toast
//! can carry one action button and always has a close button. Toasts are
//! a polite live region, so screen readers read new ones out.
//!
//! The app owns a `Toasts` queue, `push`es into it from anywhere, and
//! calls `toasts` once per frame (last, so it paints on top).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::a11y::Role;
use crate::geom::Rect;
use crate::icons::Icon;
use crate::style::{TextRole, tokens};
use crate::ui::{Id, Sense, Ui};

use super::button::{ButtonKind, ButtonOpts, button_width, button_with, icon_button};
use super::label::{Align, paragraph, paragraph_height};

/// What a toast is about; sets its icon, colour and lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToastKind {
    #[default]
    Info,
    Success,
    Warning,
    /// Stays until closed.
    Error,
}

impl ToastKind {
    /// How long a toast of this kind stays (counting only unpaused, visible
    /// time); `None` for toasts that stay until closed.
    pub fn timeout(self) -> Option<Duration> {
        match self {
            ToastKind::Info | ToastKind::Success => Some(Duration::from_secs(4)),
            ToastKind::Warning => Some(Duration::from_secs(8)),
            ToastKind::Error => None,
        }
    }

    /// The icon shown at the toast's left.
    fn icon(self) -> Icon {
        match self {
            ToastKind::Info => Icon::Info,
            ToastKind::Success => Icon::Check,
            ToastKind::Warning => Icon::Warning,
            ToastKind::Error => Icon::Error,
        }
    }
}

/// One notification.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Toast {
    pub kind: ToastKind,
    /// The message; wraps.
    pub text: String,
    /// Label of the optional action button.
    pub action: Option<String>,
}

impl Toast {
    /// A toast of `kind` saying `text`.
    pub fn new(kind: ToastKind, text: impl Into<String>) -> Self {
        Toast { kind, text: text.into(), action: None }
    }

    /// With an action button labelled `label`.
    pub fn with_action(mut self, label: impl Into<String>) -> Self {
        self.action = Some(label.into());
        self
    }
}

/// A queued toast and its countdown.
#[derive(Debug, Clone)]
struct Entry {
    id: u64,
    toast: Toast,
    /// Visible, unpaused time so far.
    shown_for: Duration,
    /// When the countdown was last advanced; `None` until first visible.
    last_tick: Option<Instant>,
    /// When it first became visible (for the entrance slide).
    appeared: Option<Instant>,
}

/// The app's toast queue. The first `MAX_VISIBLE` are shown; later ones
/// wait their turn.
#[derive(Debug, Clone, Default)]
pub struct Toasts {
    items: VecDeque<Entry>,
    next_id: u64,
}

/// Something the user did to a toast.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastEvent {
    /// The action button of toast `.0` was clicked (the toast is closed).
    Action(u64),
    /// Toast `.0` was closed by its close button.
    Dismissed(u64),
}

impl Toasts {
    /// At most this many toasts show at once.
    pub const MAX_VISIBLE: usize = 3;

    /// Queues `toast`; returns its id.
    pub fn push(&mut self, toast: Toast) -> u64 {
        self.next_id += 1;
        self.items.push_back(Entry { id: self.next_id, toast, shown_for: Duration::ZERO, last_tick: None, appeared: None });
        self.next_id
    }

    /// Removes toast `id` (shown or waiting).
    pub fn dismiss(&mut self, id: u64) {
        self.items.retain(|e| e.id != id);
    }

    /// Toasts queued (shown and waiting).
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// No toasts queued.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Ids and contents of the toasts on screen, oldest first.
    pub fn visible(&self) -> impl Iterator<Item = (u64, &Toast)> {
        self.items.iter().take(Self::MAX_VISIBLE).map(|e| (e.id, &e.toast))
    }

    /// Advances the countdowns of the visible toasts to `now` (unless
    /// `paused`) and drops the expired ones. Returns how long until the
    /// next one expires, for scheduling a repaint.
    pub fn tick(&mut self, now: Instant, paused: bool) -> Option<Duration> {
        let mut next: Option<Duration> = None;
        loop {
            let mut expired = None;
            for e in self.items.iter_mut().take(Self::MAX_VISIBLE) {
                e.appeared.get_or_insert(now);
                if let Some(last) = e.last_tick
                    && !paused
                {
                    e.shown_for += now.saturating_duration_since(last);
                }
                e.last_tick = Some(now);
                if let Some(limit) = e.toast.kind.timeout() {
                    if e.shown_for >= limit {
                        expired = Some(e.id);
                        break;
                    }
                    let left = limit - e.shown_for;
                    next = Some(next.map_or(left, |n| n.min(left)));
                }
            }
            // Removing one may bring a waiting toast on screen: go again.
            match expired {
                Some(id) => {
                    self.dismiss(id);
                    next = None;
                }
                None => return if paused { None } else { next },
            }
        }
    }
}

/// Draws the visible toasts stacked at the bottom right of the window
/// (newest lowest), runs their countdowns and reports clicks. `id` names
/// the stack's widgets.
pub fn toasts(ui: &mut Ui, id: Id, queue: &mut Toasts) -> Option<ToastEvent> {
    let win = ui.window_rect();
    toasts_in(ui, id, win, queue)
}

/// `toasts`, stacked at the bottom right of `area` instead of the window
/// (e.g. above an app's status bar).
pub fn toasts_in(ui: &mut Ui, id: Id, area: Rect, queue: &mut Toasts) -> Option<ToastEvent> {
    let win = area;
    let margin = ui.px(tokens::SPACE_LG);
    let w = ui.px(360).min(win.w - 2 * margin);
    let pad = ui.px(tokens::SPACE_MD);
    let gap = ui.px(tokens::SPACE_SM);
    let icon_px = ui.px(tokens::ICON);
    let close_px = ui.px(tokens::ROW_H - 4);
    let ctl = ui.px(tokens::ROW_H);
    let text_x = pad + icon_px + ui.px(tokens::SPACE_SM + tokens::SPACE_2XS);
    let text_w = w - text_x - close_px - pad / 2 - ui.px(tokens::SPACE_XS);
    // Measure each visible toast, newest at the bottom.
    let shown: Vec<(u64, Toast, Option<Instant>)> =
        queue.items.iter().take(Toasts::MAX_VISIBLE).map(|e| (e.id, e.toast.clone(), e.appeared)).collect();
    let mut boxes = vec![];
    let mut y = win.bottom() - margin;
    for (tid, t, appeared) in shown.iter().rev() {
        let body = paragraph_height(ui, &t.text, TextRole::Body, text_w);
        let action_h = if t.action.is_some() { gap + ctl } else { 0 };
        let h = pad + body.max(close_px - pad / 2) + action_h + pad;
        y -= h;
        boxes.push((*tid, t, *appeared, Rect::new(win.right() - margin - w, y, w, h), body));
        y -= gap;
    }
    // Pause while the pointer is over the stack or focus is inside it.
    let mouse = ui.input().mouse;
    let over = boxes.iter().any(|b| b.3.contains(mouse));
    let focus_inside = ui.focused().is_some_and(|f| boxes.iter().any(|b| is_toast_widget(id, b.0, f)));
    let now = ui.now();
    if let Some(wait) = queue.tick(now, over || focus_inside) {
        ui.request_repaint_in(wait);
    }
    let p = ui.style().palette;
    let radius = tokens::RADIUS_LG * ui.scale();
    let mut event = None;
    ui.overlay(|ui| {
        ui.a11y_group(id, Role::Group, "Notifications", |ui| {
            for (tid, t, appeared, r, body_h) in &boxes {
                // Slide in from the right as it appears.
                let k = ui.ease_since(appeared.unwrap_or(now), tokens::MOTION_MED_MS);
                let r = r.translate(((1.0 - k) * ui.px(tokens::SPACE_XL) as f32).round() as i32, 0);
                let tid = *tid;
                ui.interact(id.with(("toast", tid)), r, Sense::hover());
                if let Some(n) = ui.a11y_node(id.with(("toast", tid)), Role::Status, &t.text) {
                    n.live = true;
                }
                let tone = match t.kind {
                    ToastKind::Info => p.info,
                    ToastKind::Success => p.success,
                    ToastKind::Warning => p.warning,
                    ToastKind::Error => p.danger,
                };
                ui.elevate(r, radius);
                let bw = ui.px(1).max(1);
                {
                    let mut painter = ui.painter();
                    painter.rect(r, p.overlay, radius);
                    painter.border(r, p.border, bw, radius);
                    painter.icon(Rect::new(r.x + pad, r.y + pad + bw, icon_px, icon_px), t.kind.icon(), tone);
                }
                paragraph(ui, Rect::new(r.x + text_x, r.y + pad, text_w, *body_h), &t.text, TextRole::Body, p.fg, Align::Start);
                let close = Rect::new(r.right() - pad / 2 - close_px, r.y + pad / 2, close_px, close_px);
                if icon_button(ui, id.with(("close", tid)), close, Icon::Close, "Dismiss notification", ButtonKind::Ghost).clicked {
                    event = Some(ToastEvent::Dismissed(tid));
                }
                if let Some(a) = &t.action {
                    let aw = button_width(ui, a, 0);
                    let ar = Rect::new(r.x + text_x, r.bottom() - pad - ctl, aw, ctl);
                    if button_with(ui, id.with(("action", tid)), ar, a, ButtonOpts::kind(ButtonKind::Secondary)).clicked {
                        event = Some(ToastEvent::Action(tid));
                    }
                }
            }
        })
    });
    if let Some(ToastEvent::Action(t) | ToastEvent::Dismissed(t)) = event {
        queue.dismiss(t);
    }
    event
}

/// `f` is the close or action button of toast `tid` in stack `id`.
fn is_toast_widget(id: Id, tid: u64, f: Id) -> bool {
    f == id.with(("close", tid)) || f == id.with(("action", tid))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Info toasts expire after 4 s of visible time, errors persist, and a
    /// paused stretch does not count.
    #[test]
    fn toast_timing_and_pause() {
        let t0 = Instant::now();
        let s = |n: u64| t0 + Duration::from_millis(n);
        let mut q = Toasts::default();
        let info = q.push(Toast::new(ToastKind::Info, "saved"));
        let err = q.push(Toast::new(ToastKind::Error, "failed"));
        assert_eq!(q.tick(s(0), false), Some(Duration::from_secs(4)));
        q.tick(s(3000), false);
        // Two seconds paused (hovered) do not count.
        q.tick(s(5000), true);
        q.tick(s(5500), false);
        assert!(q.visible().any(|(i, _)| i == info), "expired while paused");
        q.tick(s(6600), false);
        assert!(!q.visible().any(|(i, _)| i == info), "should have expired");
        q.tick(s(600_000), false);
        assert_eq!(q.visible().map(|(i, _)| i).collect::<Vec<_>>(), [err]);
    }

    /// At most three show; a waiting toast's clock starts only when it
    /// comes on screen.
    #[test]
    fn toast_queue_limits_visible() {
        let t0 = Instant::now();
        let s = |n: u64| t0 + Duration::from_millis(n);
        let mut q = Toasts::default();
        let ids: Vec<u64> = (0..4).map(|i| q.push(Toast::new(ToastKind::Info, format!("t{i}")))).collect();
        q.tick(s(0), false);
        assert_eq!(q.visible().count(), 3);
        q.dismiss(ids[0]);
        q.tick(s(3900), false);
        // The fourth became visible at 3.9 s; the others expire at 4 s.
        q.tick(s(4100), false);
        assert_eq!(q.visible().map(|(i, _)| i).collect::<Vec<_>>(), [ids[3]]);
        q.tick(s(7800), false);
        assert_eq!(q.len(), 1);
        q.tick(s(8000), false);
        assert!(q.is_empty());
    }
}
