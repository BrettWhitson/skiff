//! Badges (small tinted pills of text), status dots and status pills.
//!
//! A status is never shown by colour alone: dots take a text alternative
//! for screen readers (and callers usually show it as a tooltip), and the
//! "connecting" dot pulses (a static ring under reduced motion).

use std::time::Duration;

use crate::a11y::Role;
use crate::color::Color;
use crate::geom::{Point, Rect};
use crate::style::{Palette, TextRole, tokens};
use crate::ui::{Id, Ui};

/// The colour family of a badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tone {
    /// Grey: tags, counts.
    #[default]
    Neutral,
    /// Accent tint: kinds, highlights.
    Accent,
    Info,
    Success,
    Warning,
    Danger,
}

/// A connection-like state shown by `status_dot` and `status_pill`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    /// Not connected / unknown: a filled grey dot.
    #[default]
    Idle,
    /// Connecting: a pulsing accent dot.
    Connecting,
    /// Connected / running: green.
    Ok,
    /// Degraded: amber.
    Warning,
    /// Failed: red.
    Error,
}

impl Status {
    /// The badge tone for this status.
    pub fn tone(self) -> Tone {
        match self {
            Status::Idle => Tone::Neutral,
            Status::Connecting => Tone::Accent,
            Status::Ok => Tone::Success,
            Status::Warning => Tone::Warning,
            Status::Error => Tone::Danger,
        }
    }
}

/// The (tint, text) colours of a `tone` badge: the tint is translucent so
/// it reads on any surface; the text meets 4.5:1 on it.
pub fn tone_colors(p: &Palette, tone: Tone) -> (Color, Color) {
    let (base, text) = match tone {
        Tone::Neutral => (p.muted, p.fg),
        Tone::Accent => (p.accent, p.fg),
        Tone::Info => (p.info, p.info),
        Tone::Success => (p.success, p.success),
        Tone::Warning => (p.warning, p.warning),
        Tone::Danger => (p.danger, p.danger),
    };
    // The same as `Palette::tint` once composited over the surface.
    (base.with_alpha((crate::style::TINT * 255.0).round() as u8), text)
}

/// The colour of a dot for `status`.
pub fn status_color(p: &Palette, status: Status) -> Color {
    match status {
        Status::Idle => p.muted,
        Status::Connecting => p.accent,
        Status::Ok => p.success,
        Status::Warning => p.warning,
        Status::Error => p.danger,
    }
}

/// Height of a badge (caption text plus padding).
fn badge_h(ui: &Ui) -> i32 {
    let (font, px) = ui.text_style(TextRole::Caption);
    ui.line_height(font, px) + ui.px(tokens::SPACE_2XS) * 2
}

/// The width a badge for `text` takes.
pub fn badge_width(ui: &mut Ui, text: &str) -> i32 {
    let (font, px) = ui.text_style(TextRole::Caption);
    ui.measure(text, font, px).ceil() as i32 + 2 * ui.px(tokens::SPACE_SM)
}

/// A pill with `text` in caption size, tinted by `tone`, at the left of
/// `rect` and vertically centred in it. Returns the pill's rect.
pub fn badge(ui: &mut Ui, rect: Rect, text: &str, tone: Tone) -> Rect {
    let p = ui.style().palette;
    let (tint, fg) = tone_colors(&p, tone);
    let (font, px) = ui.text_style(TextRole::Caption);
    let h = badge_h(ui).min(rect.h);
    let w = badge_width(ui, text).min(rect.w);
    let pill = Rect::new(rect.x, rect.y + (rect.h - h) / 2, w, h);
    let lh = ui.line_height(font, px);
    let pad = ui.px(tokens::SPACE_SM);
    let shown = super::label::elide(ui, text, font, px, w - 2 * pad);
    let mut painter = ui.painter();
    painter.rect(pill, tint, h as f32 / 2.0);
    painter.text(Point::new(pill.x + pad, pill.y + (h - lh) / 2), &shown, font, px, fg);
    pill
}

/// Time for one pulse of the connecting dot.
const PULSE: Duration = Duration::from_millis(tokens::PULSE_MS);

/// A status dot (`tokens::DOT` across) centred in `rect`, with `alt` as
/// its accessible text (`Role::Image`). Connecting pulses a ring around
/// the dot (static under reduced motion, still telling it from idle).
pub fn status_dot(ui: &mut Ui, id: Id, rect: Rect, status: Status, alt: &str) {
    let p = ui.style().palette;
    let c = status_color(&p, status);
    let d = ui.px(tokens::DOT).max(4);
    let center = rect.center();
    let dot = Rect::new(center.x - d / 2, center.y - d / 2, d, d);
    if let Some(n) = ui.a11y_node(id, Role::Image, alt) {
        n.bounds = dot;
    }
    if status == Status::Connecting {
        // Phase 0..1 of the pulse: the ring grows and fades.
        let t = if ui.reduce_motion() {
            0.45
        } else {
            ui.request_repaint_in(Duration::from_millis(33));
            (ui.anim_time().as_millis() % PULSE.as_millis()) as f32 / PULSE.as_millis() as f32
        };
        let grow = (d as f32 * (0.6 + 0.6 * t)).round() as i32;
        let ring = dot.inset(-grow / 2);
        let alpha = (150.0 * (1.0 - t)).round() as u8;
        let bw = ui.px(1).max(1) + 1;
        ui.painter().border(ring, c.with_alpha(alpha), bw, ring.w as f32 / 2.0);
    }
    ui.painter().rect(dot, c, d as f32 / 2.0);
}

/// A badge with a status dot before `text`, tinted by the status. Returns
/// the pill's rect. The text is the accessible description.
pub fn status_pill(ui: &mut Ui, id: Id, rect: Rect, status: Status, text: &str) -> Rect {
    let p = ui.style().palette;
    let (tint, fg) = tone_colors(&p, status.tone());
    let (font, px) = ui.text_style(TextRole::Caption);
    let h = badge_h(ui).min(rect.h);
    let pad = ui.px(tokens::SPACE_SM);
    let gap = ui.px(tokens::SPACE_XS + tokens::SPACE_2XS);
    let d = ui.px(tokens::DOT).max(4);
    let tw = ui.measure(text, font, px).ceil() as i32;
    let w = (pad + d + gap + tw + pad).min(rect.w);
    let pill = Rect::new(rect.x, rect.y + (rect.h - h) / 2, w, h);
    let lh = ui.line_height(font, px);
    ui.painter().rect(pill, tint, h as f32 / 2.0);
    status_dot(ui, id, Rect::new(pill.x + pad, pill.y, d, h), status, text);
    ui.painter().text(Point::new(pill.x + pad + d + gap, pill.y + (h - lh) / 2), text, font, px, fg);
    pill
}
