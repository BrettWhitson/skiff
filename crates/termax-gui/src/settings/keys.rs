//! One row per action; click a chord to record a new one.

use termax_ui::geom::Rect;
use termax_ui::input::{Event, Key, Modifiers};
use termax_ui::ui::{Id, Ui};
use termax_ui::widgets::{button, label};

use super::{Rows, split_row};
use crate::app::GuiApp;
use crate::keys::{Action, Chord};

#[derive(Default)]
pub struct KeysState {
    /// The action whose chord is being recorded.
    pub capturing: Option<Action>,
}

pub fn view(ui: &mut Ui, rect: Rect, st: &mut KeysState, app: &mut GuiApp) {
    let id = Id::new("settings-keys");
    if let Some(a) = st.capturing {
        ui.capture_keyboard(id);
        record(ui, st, a, app);
    }
    let mut rows = Rows::new(rect);
    let muted = ui.style().palette.muted;
    let danger = ui.style().palette.danger;
    label(ui, rows.next(ui), "Click a shortcut to change it. Esc cancels, Backspace restores the default.", Some(muted));
    for a in Action::ALL {
        let (l, c) = split_row(ui, rows.next(ui));
        label(ui, l, a.label(), None);
        let chords = app.keymap.chords(a);
        let text = if st.capturing == Some(a) {
            "Press keys\u{2026}".to_string()
        } else if chords.is_empty() {
            "(none)".to_string()
        } else {
            chords.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(", ")
        };
        let b = Rect::new(c.x, c.y, ui.px(220).min(c.w), c.h);
        if button(ui, Id::new(("settings-key", a.config_name())), b, &text).clicked {
            st.capturing = Some(a);
        }
        let clash = Action::ALL.into_iter().find(|&o| o != a && app.keymap.chords(o).iter().any(|ch| chords.contains(ch)));
        if let Some(o) = clash {
            let r = Rect::new(b.x + b.w + ui.px(10), c.y, (c.x + c.w - b.x - b.w - ui.px(10)).max(0), c.h);
            label(ui, r, &format!("also {}", o.label()), Some(danger));
        }
    }
}

fn record(ui: &mut Ui, st: &mut KeysState, a: Action, app: &mut GuiApp) {
    let presses: Vec<(Key, Modifiers)> = ui
        .input()
        .events
        .iter()
        .filter_map(|e| if let Event::KeyDown { key, mods, .. } = e { Some((key.clone(), *mods)) } else { None })
        .collect();
    for (key, mods) in presses {
        ui.input_mut().take_key(&key, mods);
        if st.capturing.is_none() {
            continue;
        }
        let name = a.config_name().to_string();
        match (&key, mods == Modifiers::NONE) {
            (Key::Escape, true) => st.capturing = None,
            (Key::Backspace, true) => {
                app.cfg.keys.remove(&name);
                app.save_config();
                st.capturing = None;
            }
            _ => {
                if let Some(chord) = Chord::from_press(&key, mods) {
                    app.cfg.keys.insert(name, chord);
                    app.save_config();
                    st.capturing = None;
                }
            }
        }
    }
    // Ignore what the key press would also type.
    ui.input_mut().events.retain(|e| !matches!(e, Event::Text(_)));
}
