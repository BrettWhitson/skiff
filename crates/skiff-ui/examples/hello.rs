//! Smallest skiff-ui app: a window showing its size and the last event.
//! Draws straight into the display list, without `Ui` or widgets.
//! `cargo run -p skiff-ui --example hello`

use skiff_ui::color::Color;
use skiff_ui::display::Prim;
use skiff_ui::geom::{Point, Rect};
use skiff_ui::shell::{App, FrameCtx, WindowOptions, run};
use skiff_ui::text::FontKind;
use skiff_ui::text::layout::{line_height, measure};

/// The example app's state.
#[derive(Default)]
struct Hello {
    /// Debug text of the last event received.
    last: String,
    /// Frames run so far.
    frames: u64,
}

impl App for Hello {
    /// Draws a card with the window size, scale, frame count and last event.
    fn frame(&mut self, ctx: &mut FrameCtx) {
        self.frames += 1;
        if let Some(e) = ctx.events.last() {
            self.last = format!("{e:?}");
        }
        ctx.background = Color::hex(0x1e1f29);
        // Logical sizes times the DPI scale give physical pixels.
        let s = ctx.scale;
        let px = 16.0 * s;
        let pad = (24.0 * s) as i32;
        let card = Rect::new(pad, pad, ctx.size.w - 2 * pad, (120.0 * s) as i32);
        ctx.list.push(Prim::Rect { rect: card, color: Color::hex(0x2b2d3a), radius: 8.0 * s });
        ctx.list.push(Prim::Border { rect: card, color: Color::hex(0x6272a4), width: 1, radius: 8.0 * s });
        let lh = line_height(ctx.fonts, FontKind::Ui, px) as i32;
        let lines = [
            format!("skiff-ui · {}×{} px · scale {:.2} · frame {}", ctx.size.w, ctx.size.h, ctx.scale, self.frames),
            format!("last event: {}", self.last),
        ];
        // One clipped text prim per line; the bounds come from measuring the text.
        for (i, text) in lines.iter().enumerate() {
            let origin = Point::new(card.x + pad / 2, card.y + pad / 2 + i as i32 * (lh + 6));
            let font = if i == 0 { FontKind::Ui } else { FontKind::Mono };
            let w = measure(ctx.fonts, ctx.cache, font, text, px).ceil() as i32;
            ctx.list.push(Prim::PushClip(card.inset(2)));
            ctx.list.push(Prim::Text {
                origin,
                text: text.clone(),
                font,
                px,
                color: Color::hex(0xf8f8f2),
                bounds: Rect::new(origin.x, origin.y, w.max(1), lh),
            });
            ctx.list.push(Prim::PopClip);
        }
    }
}

/// Loads system fonts and opens the window.
fn main() -> anyhow::Result<()> {
    let fonts = skiff_ui::text::Fonts::load(None, None)?;
    run(Hello::default(), WindowOptions { title: "skiff-ui hello".into(), app_id: "skiff-ui-hello".into(), size: (720, 240), fonts })
}
