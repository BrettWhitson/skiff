//! The Settings button at the end of the tab strip: a ghost icon button
//! (the toolkit's gear icon) with the shortcut in its tooltip.

use skiff_ui::geom::Rect;
use skiff_ui::icons::Icon;
use skiff_ui::style::tokens;
use skiff_ui::ui::{Id, Ui};
use skiff_ui::widgets::{ButtonKind, icon_button_tip};

/// The button's widget id.
pub fn id() -> Id {
    Id::new("settings-button")
}

/// The tooltip: "Settings (Ctrl+Shift+,)", or just "Settings" when unbound.
pub fn tip(shortcut: Option<String>) -> String {
    match shortcut {
        Some(s) => format!("Settings ({s})"),
        None => "Settings".into(),
    }
}

/// Where the button sits in the tab strip `strip`: a square at its right
/// end, level with the tabs (below their top margin).
pub fn rect(ui: &Ui, strip: Rect) -> Rect {
    let top = ui.px(tokens::SPACE_XS + tokens::SPACE_2XS).min(strip.h / 4);
    let side = strip.h - top;
    Rect::new(strip.right() - side - ui.px(tokens::SPACE_XS), strip.y + top, side, side).inset(ui.px(tokens::SPACE_2XS))
}

/// A focusable icon button named "Settings"; true when activated (click,
/// Space / Enter, or the screen reader's click).
pub fn button(ui: &mut Ui, rect: Rect, tip: &str) -> bool {
    icon_button_tip(ui, id(), rect, Icon::Settings, "Settings", tip, ButtonKind::Ghost).clicked
}
