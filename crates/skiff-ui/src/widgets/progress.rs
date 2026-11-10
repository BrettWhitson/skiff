use crate::geom::Rect;
use crate::ui::Ui;

/// A horizontal progress bar; `None` draws a static striped bar
/// (progress unknown) with no animation, so it costs no redraws.
pub fn progress_bar(ui: &mut Ui, rect: Rect, fraction: Option<f32>) {
    let p = ui.style().palette;
    let radius = (rect.h as f32 / 2.0).min(ui.px(3) as f32);
    let stripe = ui.px(8).max(2);
    let mut painter = ui.painter();
    painter.rect(rect, p.surface_alt, radius);
    match fraction {
        Some(f) => {
            let w = (rect.w as f32 * f.clamp(0.0, 1.0)).round() as i32;
            if w > 0 {
                painter.rect(Rect::new(rect.x, rect.y, w, rect.h), p.accent, radius);
            }
        }
        None => {
            painter.push_clip(rect);
            let mut x = rect.x;
            while x < rect.right() {
                painter.rect(Rect::new(x, rect.y, stripe, rect.h), p.accent.with_alpha(110), 0.0);
                x += 2 * stripe;
            }
            painter.pop_clip();
        }
    }
}
