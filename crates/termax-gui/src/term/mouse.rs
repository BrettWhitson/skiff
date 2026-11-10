//! Reporting the mouse to programs that turned mouse mode on.

use termax_core::termx::{MouseEv, encode_mouse};
use termax_ui::geom::{Point, Rect, Size};
use termax_ui::input::{Event, Modifiers, MouseButton};
use vt100::{MouseProtocolEncoding, MouseProtocolMode};

/// The button held down and the last cell reported (motion is sent once per cell).
#[derive(Debug, Default)]
pub struct MouseState {
    held: Option<u8>,
    last: Option<(u16, u16)>,
}

impl MouseState {
    /// Whether this frame's mouse goes to the program. A program that turned
    /// mouse mode on gets it, except with Shift (to select, as in xterm),
    /// while a selection is being made, and while the view is scrolled back
    /// (the program can't see that text, so its cells would be wrong); a
    /// button it was told about is followed until released. Turning mouse
    /// mode off forgets the held button.
    pub fn reporting(
        &mut self,
        mode: MouseProtocolMode,
        shift: bool,
        selecting: bool,
        scrolled_back: bool,
    ) -> bool {
        if mode == MouseProtocolMode::None {
            *self = MouseState::default();
            return false;
        }
        self.held.is_some() || !(shift || selecting || scrolled_back)
    }
}

fn button(b: &MouseButton) -> Option<u8> {
    match b {
        MouseButton::Left => Some(0),
        MouseButton::Middle => Some(1),
        MouseButton::Right => Some(2),
        MouseButton::Other(_) => None,
    }
}

/// The 0-based (col, row) under `p`, clamped to the grid.
pub fn cell_of(p: Point, inner: Rect, cell: Size) -> (u16, u16) {
    let cols = (inner.w / cell.w.max(1)).max(1) - 1;
    let rows = (inner.h / cell.h.max(1)).max(1) - 1;
    let col = ((p.x - inner.x).max(0) / cell.w.max(1)).min(cols);
    let row = ((p.y - inner.y).max(0) / cell.h.max(1)).min(rows);
    (col as u16, row as u16)
}

/// The bytes this frame's mouse events report to the program. Presses
/// count only inside `inner` and when the terminal is the topmost widget
/// under the pointer (`on_top`: not under the find bar or a popup); a held
/// button is followed outside it.
pub fn reports(
    events: &[Event],
    inner: Rect,
    cell: Size,
    mode: MouseProtocolMode,
    enc: MouseProtocolEncoding,
    mods: Modifiers,
    on_top: bool,
    st: &mut MouseState,
) -> Vec<Vec<u8>> {
    let mut out = vec![];
    let mut send = |ev, (c, r): (u16, u16)| {
        if let Some(b) = encode_mouse(mode, enc, ev, c, r, mods.shift, mods.alt, mods.ctrl) {
            out.push(b);
        }
    };
    for e in events {
        match e {
            Event::MouseDown(b, p) if on_top && inner.contains(*p) => {
                let Some(b) = button(b) else { continue };
                let at = cell_of(*p, inner, cell);
                st.held = Some(b);
                st.last = Some(at);
                send(MouseEv::Press(b), at);
            }
            Event::MouseUp(b, p) if st.held.is_some() && button(b) == st.held => {
                let at = cell_of(*p, inner, cell);
                send(MouseEv::Release(st.held.take().unwrap_or(0)), at);
                st.last = Some(at);
            }
            Event::MouseMove(p) => {
                let at = cell_of(*p, inner, cell);
                if st.last == Some(at) || (st.held.is_none() && (!on_top || !inner.contains(*p))) {
                    continue;
                }
                st.last = Some(at);
                send(st.held.map_or(MouseEv::Move, MouseEv::Drag), at);
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use termax_ui::geom::{Point, Rect, Size};
    use termax_ui::input::{Event, MouseButton};
    use vt100::{MouseProtocolEncoding as E, MouseProtocolMode as M};

    #[test]
    fn mouse_reports_cells() {
        let inner = Rect::new(10, 10, 800, 400);
        let cell = Size::new(8, 16);
        let mut st = MouseState::default();
        let ev = [
            Event::MouseDown(
                MouseButton::Left,
                Point::new(10 + 8 * 4 + 1, 10 + 16 * 2 + 1),
            ),
            Event::MouseMove(Point::new(10 + 8 * 5 + 1, 10 + 16 * 2 + 1)),
            Event::MouseUp(
                MouseButton::Left,
                Point::new(10 + 8 * 5 + 1, 10 + 16 * 2 + 1),
            ),
        ];
        let out = reports(
            &ev,
            inner,
            cell,
            M::ButtonMotion,
            E::Sgr,
            Default::default(),
            true,
            &mut st,
        );
        assert_eq!(
            out,
            vec![
                b"\x1b[<0;5;3M".to_vec(),
                b"\x1b[<32;6;3M".to_vec(),
                b"\x1b[<0;6;3m".to_vec()
            ]
        );
        let outside = [Event::MouseDown(MouseButton::Left, Point::new(0, 0))];
        assert!(
            reports(
                &outside,
                inner,
                cell,
                M::ButtonMotion,
                E::Sgr,
                Default::default(),
                true,
                &mut st
            )
            .is_empty()
        );
    }

    #[test]
    fn reporting_follows_a_held_button_and_forgets_it_when_mode_ends() {
        let inner = Rect::new(0, 0, 800, 400);
        let cell = Size::new(8, 16);
        let mut st = MouseState::default();
        assert!(st.reporting(M::ButtonMotion, false, false, false));
        assert!(
            !st.reporting(M::ButtonMotion, false, false, true),
            "scrolled back: the view isn't the program's screen"
        );
        let down = [Event::MouseDown(MouseButton::Left, Point::new(20, 20))];
        reports(
            &down,
            inner,
            cell,
            M::ButtonMotion,
            E::Sgr,
            Default::default(),
            true,
            &mut st,
        );
        assert!(
            st.reporting(M::ButtonMotion, true, false, true),
            "a drag the program saw starting is followed to its release"
        );
        assert!(!st.reporting(M::None, false, false, false));
        let up = [Event::MouseUp(MouseButton::Left, Point::new(20, 20))];
        assert!(
            reports(
                &up,
                inner,
                cell,
                M::ButtonMotion,
                E::Sgr,
                Default::default(),
                true,
                &mut st
            )
            .is_empty(),
            "mode off mid-drag: no stale button"
        );
    }

    #[test]
    fn presses_under_an_overlay_are_not_reported() {
        let inner = Rect::new(0, 0, 800, 400);
        let cell = Size::new(8, 16);
        let mut st = MouseState::default();
        let at = Point::new(20, 20);
        let press = [
            Event::MouseDown(MouseButton::Left, at),
            Event::MouseUp(MouseButton::Left, at),
        ];
        // `false`: something (the find bar, a popup) is on top of the terminal there.
        assert!(
            reports(
                &press,
                inner,
                cell,
                M::PressRelease,
                E::Sgr,
                Default::default(),
                false,
                &mut st
            )
            .is_empty()
        );
    }
}
