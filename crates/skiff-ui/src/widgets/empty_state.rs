//! The empty-state block: what an empty list or screen shows instead of
//! nothing (icon, title, one line of help, a primary action, an optional
//! secondary action and a shortcut hint), centred in its area; the buttons
//! stack when the area is too narrow for them side by side.

use crate::geom::Rect;
use crate::icons::Icon;
use crate::style::{TextRole, tokens};
use crate::ui::{Id, Ui};

use super::button::{ButtonKind, ButtonOpts, button_width, button_with};
use super::label::{Align, paragraph, paragraph_height};

/// Contents of an `empty_state`.
#[derive(Debug, Clone, Copy, Default)]
pub struct EmptyState<'a> {
    /// Large muted icon above the title.
    pub icon: Option<Icon>,
    pub title: &'a str,
    /// One line (it wraps) explaining what goes here.
    pub body: &'a str,
    /// Primary button label.
    pub action: Option<&'a str>,
    /// Secondary button label.
    pub secondary: Option<&'a str>,
    /// Caption under the buttons, e.g. "or press Ctrl+K".
    pub hint: Option<&'a str>,
}

/// Which of an empty state's buttons were clicked.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EmptyStateResponse {
    pub action: bool,
    pub secondary: bool,
}

/// Draws `e` centred in `rect` (at most 380 px wide). The buttons are
/// ordinary focusable buttons with ids under `id`.
pub fn empty_state(ui: &mut Ui, id: Id, rect: Rect, e: &EmptyState) -> EmptyStateResponse {
    let p = ui.style().palette;
    let w = rect.w.min(ui.px(380));
    let x = rect.x + (rect.w - w) / 2;
    let icon_box = ui.px(48);
    let gap = ui.px(tokens::SPACE_MD);
    let ctl = ui.px(tokens::CONTROL_H);
    let title_h = paragraph_height(ui, e.title, TextRole::Title, w);
    let body_h = if e.body.is_empty() { 0 } else { paragraph_height(ui, e.body, TextRole::Body, w) };
    let hint_h = e.hint.map_or(0, |h| paragraph_height(ui, h, TextRole::Caption, w));
    let has_buttons = e.action.is_some() || e.secondary.is_some();
    // Buttons side by side, or stacked (same width) when the area is narrow.
    let aw = e.action.map_or(0, |a| button_width(ui, a, 0).max(ui.px(96)));
    let sw = e.secondary.map_or(0, |s| button_width(ui, s, 0).max(ui.px(96)));
    let bgap = if aw > 0 && sw > 0 { ui.px(tokens::SPACE_SM) } else { 0 };
    let stacked = aw + sw + bgap > w;
    let buttons_h = if stacked && bgap > 0 { 2 * ctl + bgap } else { ctl };
    let total = e.icon.map_or(0, |_| icon_box + gap)
        + title_h
        + if body_h > 0 { ui.px(tokens::SPACE_XS) + body_h } else { 0 }
        + if has_buttons { ui.px(tokens::SPACE_LG) + buttons_h } else { 0 }
        + if hint_h > 0 { gap + hint_h } else { 0 };
    // A little above centre reads as centred.
    let mut y = rect.y + ((rect.h - total) * 2 / 5).max(0);
    if let Some(icon) = e.icon {
        let r = Rect::new(rect.x + (rect.w - icon_box) / 2, y, icon_box, icon_box);
        let s = ui.px(tokens::ICON * 2);
        let mut painter = ui.painter();
        painter.rect(r, p.surface_alt, icon_box as f32 / 2.0);
        painter.icon(Rect::new(r.x + (r.w - s) / 2, r.y + (r.h - s) / 2, s, s), icon, p.muted);
        y += icon_box + gap;
    }
    y += paragraph(ui, Rect::new(x, y, w, title_h), e.title, TextRole::Title, p.fg, Align::Center);
    if body_h > 0 {
        y += ui.px(tokens::SPACE_XS);
        y += paragraph(ui, Rect::new(x, y, w, body_h), e.body, TextRole::Body, p.muted, Align::Center);
    }
    let mut out = EmptyStateResponse::default();
    if has_buttons {
        y += ui.px(tokens::SPACE_LG);
        let (ra, rs) = if stacked {
            let bw = aw.max(sw).min(w);
            let bx = rect.x + (rect.w - bw) / 2;
            let second_y = if aw > 0 { y + ctl + bgap } else { y };
            (Rect::new(bx, y, bw, ctl), Rect::new(bx, second_y, bw, ctl))
        } else {
            let bx = rect.x + (rect.w - aw - sw - bgap) / 2;
            (Rect::new(bx, y, aw, ctl), Rect::new(bx + aw + bgap, y, sw, ctl))
        };
        if let Some(a) = e.action {
            out.action = button_with(ui, id.with("action"), ra, a, ButtonOpts::kind(ButtonKind::Primary)).clicked;
        }
        if let Some(s) = e.secondary {
            out.secondary = button_with(ui, id.with("secondary"), rs, s, ButtonOpts::default()).clicked;
        }
        y += buttons_h;
    }
    if let Some(h) = e.hint {
        y += gap;
        paragraph(ui, Rect::new(x, y, w, hint_h), h, TextRole::Caption, p.muted, Align::Center);
    }
    out
}
