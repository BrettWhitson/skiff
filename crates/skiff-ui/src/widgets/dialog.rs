//! Modal dialogs: a scrim over the window, a raised box with a title, a
//! wrapped (never truncated) message and a row of buttons. Focus is
//! trapped inside (`Ui::modal`), Escape cancels, and focus returns to
//! where it was when the dialog closes.
//!
//! The caller decides when a dialog is shown: call `dialog` every frame
//! while it should be open, and stop once it returns a result. To drop
//! one unanswered, call `dialog_dismiss` (it restores focus).

use crate::a11y::Role;
use crate::geom::Rect;
use crate::icons::Icon;
use crate::input::{Key, Modifiers};
use crate::style::{TextRole, tokens};
use crate::ui::{Id, Sense, Ui};

use super::button::{ButtonKind, ButtonOpts, button_width, button_with};
use super::label::{Align, paragraph, paragraph_height};

/// One button of a dialog.
#[derive(Debug, Clone, Copy)]
pub struct DialogButton<'a> {
    pub label: &'a str,
    pub kind: ButtonKind,
}

/// A dialog's contents.
#[derive(Debug, Clone, Copy)]
pub struct Dialog<'a> {
    pub title: &'a str,
    /// The message; wraps to as many lines as it needs.
    pub body: &'a str,
    /// Buttons, laid out left to right at the bottom right.
    pub buttons: &'a [DialogButton<'a>],
    /// The button focused when the dialog opens (the safe choice for a
    /// destructive confirm, e.g. Cancel).
    pub initial: usize,
    /// Icon beside the title (e.g. `Icon::Warning` for a destructive confirm).
    pub icon: Option<Icon>,
    /// Box width in logical pixels (capped by the window).
    pub width: i32,
}

impl Default for Dialog<'_> {
    /// An empty 440 px dialog with no buttons.
    fn default() -> Self {
        Dialog { title: "", body: "", buttons: &[], initial: 0, icon: None, width: 440 }
    }
}

/// How a dialog was answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogResult {
    /// The button at this index was activated.
    Button(usize),
    /// Escape was pressed.
    Cancelled,
}

/// Per-dialog state in widget memory.
#[derive(Debug, Clone, Copy, Default)]
struct DialogState {
    /// The frame number it was last drawn in (a gap means it reopened).
    last_frame: u64,
    /// Focus before it opened, restored when it closes.
    return_to: Option<Id>,
    /// When it (re)opened, for the entrance transition.
    opened: Option<std::time::Instant>,
}

/// Draws dialog `id` (a scrim, the box, its buttons) on the overlay layer
/// and returns how it was answered, if it was this frame. The first frame
/// it is shown focuses `d.initial`; answering restores the earlier focus.
pub fn dialog(ui: &mut Ui, id: Id, d: &Dialog) -> Option<DialogResult> {
    let frame = ui.frame_no();
    let mut st = *ui.memory::<DialogState>(id);
    let opening = st.last_frame + 1 != frame;
    if opening {
        st.return_to = ui.focused();
        st.opened = Some(ui.now());
    }
    st.last_frame = frame;
    let p = ui.style().palette;
    let win = ui.window_rect();
    let margin = ui.px(tokens::SPACE_LG);
    let w = ui.px(d.width).min(win.w - 2 * margin).max(ui.px(160));
    let pad = ui.px(tokens::SPACE_XL);
    let inner_w = w - 2 * pad;
    let icon_px = ui.px(20);
    let title_indent = d.icon.map_or(0, |_| icon_px + ui.px(tokens::SPACE_SM));
    let title_h = paragraph_height(ui, d.title, TextRole::Title, inner_w - title_indent);
    let body_h = if d.body.is_empty() { 0 } else { paragraph_height(ui, d.body, TextRole::Body, inner_w) };
    let ctl = ui.px(tokens::CONTROL_H);
    let gap = ui.px(tokens::SPACE_SM);
    let h = pad + title_h + if body_h > 0 { gap + body_h } else { 0 } + ui.px(tokens::SPACE_XL) + ctl + pad;
    // Centred, slightly above the middle; slides up a little as it appears.
    let fade = ui.ease_since(st.opened.unwrap_or(ui.now()), tokens::MOTION_MED_MS);
    let rise = 1.0 - fade;
    let y = ((win.h - h) * 2 / 5).max(margin) + (rise * ui.px(8) as f32).round() as i32;
    let bx = Rect::new(win.x + (win.w - w) / 2, y, w, h);
    let radius = tokens::RADIUS_LG * ui.scale();
    let danger = d.buttons.iter().any(|b| b.kind == ButtonKind::Danger);
    let mut result = None;
    ui.overlay(|ui| {
        // The scrim takes every pointer event outside the dialog.
        ui.interact(id.with("scrim"), win, Sense::hover());
        let scrim = p.scrim.with_alpha((p.scrim.a as f32 * fade.max(0.3)).round() as u8);
        ui.painter().rect(win, scrim, 0.0);
        ui.modal(id, |ui| {
            let role = if danger { Role::AlertDialog } else { Role::Dialog };
            ui.interact(id, bx, Sense::hover());
            if let Some(n) = ui.a11y_node(id, role, d.title) {
                n.modal = true;
                n.bounds = bx;
            }
            ui.a11y_group(id.with("content"), Role::Group, "", |ui| {
                ui.elevate(bx, radius);
                let bw = ui.px(1).max(1);
                {
                    let mut painter = ui.painter();
                    painter.rect(bx, p.overlay, radius);
                    painter.border(bx, p.border, bw, radius);
                }
                let mut cy = bx.y + pad;
                if let Some(icon) = d.icon {
                    let tone = if danger { p.danger } else { p.accent };
                    let lh = super::label::paragraph_line_h(ui, TextRole::Title);
                    ui.painter().icon(Rect::new(bx.x + pad, cy + (lh - icon_px) / 2, icon_px, icon_px), icon, tone);
                }
                let title = Rect::new(bx.x + pad + title_indent, cy, inner_w - title_indent, title_h);
                cy += paragraph(ui, title, d.title, TextRole::Title, p.fg, Align::Start);
                if body_h > 0 {
                    cy += gap;
                    let body = Rect::new(bx.x + pad, cy, inner_w, body_h);
                    paragraph(ui, body, d.body, TextRole::Body, p.muted, Align::Start);
                    if let Some(n) = ui.a11y_node(id.with("body"), Role::Label, d.body) {
                        n.bounds = body;
                    }
                }
                // Buttons, right-aligned in the given order.
                let widths: Vec<i32> = d.buttons.iter().map(|b| button_width(ui, b.label, 0).max(ui.px(88))).collect();
                let total: i32 = widths.iter().sum::<i32>() + gap * (widths.len() as i32 - 1).max(0);
                let mut x = bx.right() - pad - total;
                let by = bx.bottom() - pad - ctl;
                for (i, (b, bw)) in d.buttons.iter().zip(&widths).enumerate() {
                    let bid = id.with(("button", i));
                    if opening && i == d.initial {
                        ui.request_focus(bid);
                    }
                    if button_with(ui, bid, Rect::new(x, by, *bw, ctl), b.label, ButtonOpts::kind(b.kind)).clicked {
                        result = Some(DialogResult::Button(i));
                    }
                    x += bw + gap;
                }
            });
            if ui.keys_for(id) && ui.input_mut().take_key(&Key::Escape, Modifiers::NONE) {
                result = Some(DialogResult::Cancelled);
            }
        });
    });
    if result.is_some() {
        ui.release_modal(id);
        if let Some(f) = st.return_to {
            ui.request_focus(f);
        }
        st.last_frame = 0;
    }
    *ui.memory::<DialogState>(id) = st;
    result
}

/// Closes dialog `id` without an answer (the caller stopped showing it,
/// e.g. what it asked about went away): releases its modal scope and
/// gives focus back to where it was when it opened. Does nothing if it
/// is not open.
pub fn dialog_dismiss(ui: &mut Ui, id: Id) {
    let st = *ui.memory::<DialogState>(id);
    if st.last_frame == 0 {
        return;
    }
    ui.release_modal(id);
    if let Some(f) = st.return_to {
        ui.request_focus(f);
    }
    *ui.memory::<DialogState>(id) = DialogState::default();
}
