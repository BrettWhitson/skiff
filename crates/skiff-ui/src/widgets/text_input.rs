//! Single-line text field. Edits happen on a `Vec<char>`, so cursor
//! movement and deletion can never split a UTF-8 sequence.

use crate::a11y::{A11yAction, Role};
use crate::geom::{Point, Rect};
use crate::icons::Icon;
use crate::input::{CursorIcon, Event, Key, MouseButton};
use crate::style::tokens;
use crate::text::FontKind;
use crate::ui::{Id, Response, Sense, Ui};

/// Options for `text_input`; `Default` gives a plain unlabeled field.
#[derive(Debug, Clone, Copy, Default)]
pub struct TextInputOpts<'a> {
    /// Draw "•" per character; copy and cut are disabled.
    pub password: bool,
    /// Shown (muted) while the value is empty.
    pub placeholder: &'a str,
    /// The name screen readers announce; the placeholder when empty.
    pub label: &'a str,
    /// An icon inside the field before the text (e.g. `Icon::Search`).
    pub icon: Option<Icon>,
    /// No box or outline: the field sits on a surface that frames it (a
    /// command palette's panel).
    pub frameless: bool,
}

/// What happened to a text field this frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct TextInputResponse {
    /// The field's own interaction (hover, focus, clicks).
    pub response: Response,
    /// The value was edited this frame.
    pub changed: bool,
    /// Enter was pressed while focused.
    pub submitted: bool,
}

/// Cursor and selection, in chars. `anchor == cursor` means no selection.
#[derive(Debug, Clone, Copy, Default)]
struct EditState {
    /// Insertion point, in chars.
    cursor: usize,
    /// The other end of the selection, in chars.
    anchor: usize,
    /// Horizontal scroll in pixels, so the cursor stays visible in long text.
    scroll: f32,
    /// Focus as of last frame, to tell when focus has just arrived.
    had_focus: bool,
}

/// Word characters for Ctrl+arrow movement and Ctrl+Backspace/Delete.
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The start of the word at or before `i`, skipping separators first.
fn word_left(s: &[char], mut i: usize) -> usize {
    while i > 0 && !is_word(s[i - 1]) {
        i -= 1;
    }
    while i > 0 && is_word(s[i - 1]) {
        i -= 1;
    }
    i
}

/// The end of the word at or after `i`, skipping separators first.
fn word_right(s: &[char], mut i: usize) -> usize {
    while i < s.len() && !is_word(s[i]) {
        i += 1;
    }
    while i < s.len() && is_word(s[i]) {
        i += 1;
    }
    i
}

/// Line breaks and tabs become spaces; other control chars are dropped.
fn sanitize(t: &str) -> Vec<char> {
    t.replace("\r\n", " ")
        .chars()
        .filter_map(|c| match c {
            '\n' | '\r' | '\t' => Some(' '),
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect()
}

/// One frame's editing session over the value's chars.
struct Editor {
    chars: Vec<char>,
    st: EditState,
    /// Set by any edit, so the caller's `String` is rebuilt only when needed.
    changed: bool,
}

impl Editor {
    /// The selection as an ordered `(start, end)` range.
    fn selection(&self) -> (usize, usize) {
        (self.st.cursor.min(self.st.anchor), self.st.cursor.max(self.st.anchor))
    }

    /// Whether any text is selected.
    fn has_selection(&self) -> bool {
        self.st.cursor != self.st.anchor
    }

    /// The selected text.
    fn selected(&self) -> String {
        let (a, b) = self.selection();
        self.chars[a..b].iter().collect()
    }

    /// Removes chars `a..b` and collapses the cursor and anchor to `a`.
    fn delete_range(&mut self, a: usize, b: usize) {
        if a < b {
            self.chars.drain(a..b);
            self.changed = true;
        }
        self.st.cursor = a;
        self.st.anchor = a;
    }

    /// Removes the selected text, if any.
    fn delete_selection(&mut self) {
        let (a, b) = self.selection();
        self.delete_range(a, b);
    }

    /// Replaces the selection with sanitized `t` and puts the cursor after it.
    /// Text that sanitizes to nothing leaves the selection alone.
    fn insert(&mut self, t: &str) {
        let ins = sanitize(t);
        if ins.is_empty() {
            return;
        }
        self.delete_selection();
        let at = self.st.cursor;
        self.chars.splice(at..at, ins.iter().copied());
        self.st.cursor = at + ins.len();
        self.st.anchor = self.st.cursor;
        self.changed = true;
    }

    /// Moves the cursor to `i` (clamped); `select` keeps the anchor, extending
    /// the selection.
    fn move_to(&mut self, i: usize, select: bool) {
        self.st.cursor = i.min(self.chars.len());
        if !select {
            self.st.anchor = self.st.cursor;
        }
    }
}

/// A single-line editable text field.
/// Handles mouse, keyboard, clipboard and screen-reader SetValue, writes
/// edits back to `value`, and keeps cursor state in widget memory.
pub fn text_input(ui: &mut Ui, id: Id, rect: Rect, value: &mut String, opts: TextInputOpts) -> TextInputResponse {
    let r = ui.interact(id, rect, Sense::focusable());
    if r.hovered {
        ui.set_cursor_icon(CursorIcon::Text);
    }
    // The value may have changed outside the widget: clamp the stored cursor.
    let mut st = *ui.memory::<EditState>(id);
    let mut ed = Editor { chars: value.chars().collect(), st, changed: false };
    let n = ed.chars.len();
    ed.st.cursor = ed.st.cursor.min(n);
    ed.st.anchor = ed.st.anchor.min(n);

    let px = ui.font_px();
    let pad = ui.px(6);
    // A leading icon takes its width plus a gap from the text area.
    let icon_px = ui.px(tokens::ICON);
    let lead = if opts.icon.is_some() { icon_px + ui.px(tokens::SPACE_SM) } else { 0 };
    let inner = Rect::new(rect.x + pad + lead, rect.y, rect.w - 2 * pad - lead, rect.h);
    // What is drawn and measured: bullets for passwords so the caret lines up.
    let display = |chars: &[char]| -> String { if opts.password { "•".repeat(chars.len()) } else { chars.iter().collect() } };

    let mut submitted = false;
    // A screen reader replaced the value: cursor to the end.
    if let Some(A11yAction::SetValue(v)) = ui.take_a11y_action(id, |a| matches!(a, A11yAction::SetValue(_))) {
        ed.chars = sanitize(&v);
        ed.st.cursor = ed.chars.len();
        ed.st.anchor = ed.st.cursor;
        ed.changed = true;
    }
    if r.focused {
        if !ed.st.had_focus && !r.pressed && !r.clicked {
            // Focus arrived by keyboard: cursor to the end.
            ed.move_to(n, false);
        }
        // Mouse: a press inside places the cursor; dragging extends.
        let mouse_down_here = ui.input().events.iter().any(|e| matches!(e, Event::MouseDown(MouseButton::Left, p) if rect.contains(*p)));
        // On the press frame place the cursor; while held, extend the selection.
        if r.pressed || mouse_down_here {
            let shown = display(&ed.chars);
            let x = (ui.input().mouse.x - inner.x) as f32 + ed.st.scroll;
            let ci = ui.char_index_at_x(&shown, FontKind::Ui, px, x);
            ed.move_to(ci, !mouse_down_here);
        }
        if ui.keys_for(id) {
            // Handled events are consumed; everything else (Tab, app
            // shortcuts such as Ctrl+Shift+X) stays for others.
            let events = std::mem::take(&mut ui.input_mut().events);
            let mut rest = Vec::with_capacity(events.len());
            for e in events {
                let handled = match &e {
                    Event::Text(t) | Event::Paste(t) => {
                        ed.insert(t);
                        true
                    }
                    // Logo/Cmd chords are never handled here.
                    Event::KeyDown { key, mods, .. } if !mods.logo => {
                        let (ctrl, shift) = (mods.ctrl && !mods.alt, mods.shift);
                        // Clipboard/select-all only on plain Ctrl: Ctrl+Shift is the app's.
                        let cmd = ctrl && !shift;
                        let cur = ed.st.cursor;
                        match key {
                            // With a selection, Left/Right without Shift collapse it to that side.
                            Key::Left if ed.has_selection() && !shift => {
                                let a = ed.selection().0;
                                ed.move_to(a, false);
                                true
                            }
                            Key::Right if ed.has_selection() && !shift => {
                                let b = ed.selection().1;
                                ed.move_to(b, false);
                                true
                            }
                            Key::Left => {
                                let to = if ctrl { word_left(&ed.chars, cur) } else { cur.saturating_sub(1) };
                                ed.move_to(to, shift);
                                true
                            }
                            Key::Right => {
                                let to = if ctrl { word_right(&ed.chars, cur) } else { cur + 1 };
                                ed.move_to(to, shift);
                                true
                            }
                            Key::Home => {
                                ed.move_to(0, shift);
                                true
                            }
                            Key::End => {
                                ed.move_to(ed.chars.len(), shift);
                                true
                            }
                            Key::Backspace if ed.has_selection() => {
                                ed.delete_selection();
                                true
                            }
                            Key::Backspace => {
                                let from = if ctrl { word_left(&ed.chars, cur) } else { cur.saturating_sub(1) };
                                ed.delete_range(from, cur);
                                true
                            }
                            Key::Delete if ed.has_selection() => {
                                ed.delete_selection();
                                true
                            }
                            Key::Delete => {
                                let to = if ctrl { word_right(&ed.chars, cur) } else { (cur + 1).min(ed.chars.len()) };
                                ed.delete_range(cur, to);
                                true
                            }
                            Key::Enter => {
                                submitted = true;
                                true
                            }
                            Key::Char('a') if cmd => {
                                ed.st.anchor = 0;
                                ed.st.cursor = ed.chars.len();
                                true
                            }
                            // Copy and cut need a selection and never expose a password; paste
                            // is always taken.
                            Key::Char('c') if cmd && !opts.password && ed.has_selection() => {
                                let s = ed.selected();
                                ui.clipboard().set(&s);
                                true
                            }
                            Key::Char('x') if cmd && !opts.password && ed.has_selection() => {
                                let s = ed.selected();
                                ui.clipboard().set(&s);
                                ed.delete_selection();
                                true
                            }
                            Key::Char('v') if cmd => {
                                if let Some(t) = ui.clipboard().get() {
                                    ed.insert(&t);
                                }
                                true
                            }
                            _ => false,
                        }
                    }
                    _ => false,
                };
                if !handled {
                    rest.push(e);
                }
            }
            ui.input_mut().events = rest;
        }
    }
    ed.st.had_focus = r.focused;
    if ed.changed {
        *value = ed.chars.iter().collect();
    }
    // A password field's value is never published to assistive technology.
    let name = if opts.label.is_empty() { opts.placeholder } else { opts.label };
    let role = if opts.password { Role::PasswordInput } else { Role::TextInput };
    if let Some(n) = ui.a11y_node(id, role, name) {
        n.value = (!opts.password).then(|| value.clone());
    }

    // Layout: keep the cursor visible.
    let shown = display(&ed.chars);
    let prefix = |ui: &mut Ui, i: usize| -> f32 {
        let s: String = shown.chars().take(i).collect();
        ui.measure(&s, FontKind::Ui, px)
    };
    // Scroll just enough to keep the caret within the field (2px for the caret),
    // and never past the end of the text.
    let cursor_x = prefix(ui, ed.st.cursor);
    let total = ui.measure(&shown, FontKind::Ui, px);
    let room = (inner.w - 2).max(0) as f32;
    if r.focused {
        if cursor_x - ed.st.scroll > room {
            ed.st.scroll = cursor_x - room;
        }
        if cursor_x < ed.st.scroll {
            ed.st.scroll = cursor_x;
        }
    }
    ed.st.scroll = ed.st.scroll.min((total - room).max(0.0)).max(0.0);
    let (sa, sb) = ed.selection();
    let (sel_a, sel_b) = (prefix(ui, sa), prefix(ui, sb));
    st = ed.st;
    *ui.memory::<EditState>(id) = st;

    // Paint.
    let p = ui.style().palette;
    let radius = ui.style().radius * ui.scale();
    let bw = ui.px(1).max(1);
    let lh = ui.line_height(FontKind::Ui, px);
    let ty = rect.y + (rect.h - lh) / 2;
    let x0 = inner.x - st.scroll.round() as i32;
    {
        let mut painter = ui.painter();
        if !opts.frameless {
            painter.rect(rect, p.surface, radius);
            painter.border(rect, if r.focused { p.accent } else { p.control }, bw, radius);
        }
        if let Some(icon) = opts.icon {
            painter.icon(Rect::new(rect.x + pad, rect.y + (rect.h - icon_px) / 2, icon_px, icon_px), icon, p.muted);
        }
    }
    ui.with_clip(inner, |ui| {
        let mut painter = ui.painter();
        if r.focused && sa != sb {
            let sel = Rect::new(x0 + sel_a.round() as i32, ty, (sel_b - sel_a).round() as i32, lh);
            painter.rect(sel, p.selection, 0.0);
        }
        if shown.is_empty() {
            if !opts.placeholder.is_empty() {
                painter.text(Point::new(inner.x, ty), opts.placeholder, FontKind::Ui, px, p.muted);
            }
        } else {
            painter.text(Point::new(x0, ty), &shown, FontKind::Ui, px, p.fg);
        }
        if r.focused {
            let caret = Rect::new(x0 + cursor_x.round() as i32, ty, bw, lh);
            painter.rect(caret, p.accent, 0.0);
        }
    });
    TextInputResponse { response: r, changed: ed.changed, submitted }
}
