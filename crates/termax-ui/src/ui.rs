//! Immediate-mode UI core.
//!
//! Widgets are functions that take `&mut Ui`, draw through `ui.painter()`
//! and ask `ui.interact(...)` what the user did to their rectangle. State
//! that must outlive a frame lives in `UiState` (focus, the pressed
//! widget, per-widget memory). Hit-testing uses the rectangles registered
//! during the previous frame, so the topmost widget — last registered,
//! overlays above everything — receives the click even when something
//! drawn later in this frame covers an earlier widget.

use std::any::Any;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use crate::clipboard::Clipboard;
use crate::color::Color;
use crate::display::{DisplayList, Prim};
use crate::geom::{Point, Rect, Size};
use crate::input::{CursorIcon, Event, Key, Modifiers, MouseButton};
use crate::layout;
use crate::shell::FrameCtx;
use crate::style::Style;
use crate::text::cache::GlyphCache;
use crate::text::layout as text_layout;
use crate::text::{FontKind, Fonts};

/// A stable widget identity, derived from anything hashable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Id(pub u64);

impl Id {
    pub fn new(v: impl Hash) -> Id {
        let mut h = DefaultHasher::new();
        v.hash(&mut h);
        Id(h.finish())
    }

    /// A child id, unique within this parent.
    pub fn with(self, v: impl Hash) -> Id {
        let mut h = DefaultHasher::new();
        self.0.hash(&mut h);
        v.hash(&mut h);
        Id(h.finish())
    }
}

/// What a widget wants to know about.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Sense {
    pub click: bool,
    pub drag: bool,
    pub focus: bool,
}

impl Sense {
    /// Blocks hits below, reports hover only.
    pub const fn hover() -> Self {
        Sense { click: false, drag: false, focus: false }
    }
    pub const fn click() -> Self {
        Sense { click: true, drag: false, focus: false }
    }
    pub const fn drag() -> Self {
        Sense { click: false, drag: true, focus: false }
    }
    pub const fn click_and_drag() -> Self {
        Sense { click: true, drag: true, focus: false }
    }
    /// Clickable and takes keyboard focus (on click or Tab).
    pub const fn focusable() -> Self {
        Sense { click: true, drag: false, focus: true }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Response {
    pub rect: Rect,
    pub hovered: bool,
    /// Primary button held down on this widget.
    pub pressed: bool,
    pub clicked: bool,
    pub double_clicked: bool,
    pub right_clicked: bool,
    /// Mouse movement since last frame while dragging this widget.
    pub dragged: Option<Point>,
    pub focused: bool,
}

#[derive(Debug, Clone, Copy)]
struct Hit {
    id: Id,
    rect: Rect,
    layer: u8,
    sense: Sense,
}

fn top_hit(hits: &[Hit], p: Point) -> Option<Hit> {
    hits.iter().enumerate().filter(|(_, h)| h.rect.contains(p)).max_by_key(|(i, h)| (h.layer, *i)).map(|(_, h)| *h)
}

/// This frame's input plus current pointer/keyboard state.
#[derive(Debug, Clone, Default)]
pub struct Input {
    pub events: Vec<Event>,
    pub mouse: Point,
    pub left_down: bool,
    pub right_down: bool,
    pub mods: Modifiers,
}

impl Input {
    /// True if `key` with exactly `mods` was pressed this frame.
    pub fn key_pressed(&self, key: &Key, mods: Modifiers) -> bool {
        self.events.iter().any(|e| matches!(e, Event::KeyDown { key: k, mods: m, .. } if k == key && *m == mods))
    }

    /// Like `key_pressed`, but removes the event so nobody else acts on it.
    pub fn take_key(&mut self, key: &Key, mods: Modifiers) -> bool {
        let pos =
            self.events.iter().position(|e| matches!(e, Event::KeyDown { key: k, mods: m, .. } if k == key && *m == mods));
        pos.map(|i| self.events.remove(i)).is_some()
    }

    /// Text typed this frame.
    pub fn text(&self) -> String {
        self.events.iter().filter_map(|e| if let Event::Text(t) = e { Some(t.as_str()) } else { None }).collect()
    }

    /// Total wheel movement this frame.
    pub fn wheel(&self) -> (f32, f32) {
        self.events.iter().fold((0.0, 0.0), |(x, y), e| match e {
            Event::Wheel { dx, dy } => (x + dx, y + dy),
            _ => (x, y),
        })
    }
}

/// UI state that lives across frames. Keep one per window in the app.
pub struct UiState {
    memory: HashMap<Id, Box<dyn Any>>,
    focus: Option<Id>,
    active: Option<Id>,
    right_active: Option<Id>,
    prev_hits: Vec<Hit>,
    mouse: Point,
    left: bool,
    right: bool,
    mods: Modifiers,
    drag_last: Point,
    last_click: Option<(Id, Instant, Point)>,
    /// Widget that owns the keyboard (an open menu); carried to the next frame.
    capture: Option<Id>,
}

impl Default for UiState {
    fn default() -> Self {
        UiState {
            memory: HashMap::new(),
            focus: None,
            active: None,
            right_active: None,
            prev_hits: vec![],
            mouse: Point::new(-1, -1),
            left: false,
            right: false,
            mods: Modifiers::NONE,
            drag_last: Point::default(),
            last_click: None,
            capture: None,
        }
    }
}

const DOUBLE_CLICK: Duration = Duration::from_millis(450);

/// Interaction results computed once per frame from the events.
#[derive(Default)]
struct FrameHits {
    hovered: Option<Id>,
    clicked: HashSet<Id>,
    double_clicked: HashSet<Id>,
    right_clicked: HashSet<Id>,
    drag: Option<(Id, Point)>,
}

/// Draws into the current layer's display list.
pub struct Painter<'p> {
    list: &'p mut DisplayList,
    fonts: &'p Fonts,
    cache: &'p mut GlyphCache,
}

impl Painter<'_> {
    pub fn rect(&mut self, rect: Rect, color: Color, radius: f32) {
        if !rect.is_empty() && color.a > 0 {
            self.list.push(Prim::Rect { rect, color, radius });
        }
    }

    pub fn border(&mut self, rect: Rect, color: Color, width: i32, radius: f32) {
        if !rect.is_empty() && color.a > 0 {
            self.list.push(Prim::Border { rect, color, width, radius });
        }
    }

    /// Draws one line with its line box's top-left at `pos`; returns its width.
    pub fn text(&mut self, pos: Point, text: &str, font: FontKind, px: f32, color: Color) -> i32 {
        let w = text_layout::measure(self.fonts, self.cache, font, text, px).ceil() as i32;
        let ink = text_layout::ink_bounds(self.fonts, self.cache, font, text, px, pos);
        let h = text_layout::line_height(self.fonts, font, px) as i32;
        if !text.is_empty() {
            self.list.push(Prim::Text {
                origin: pos,
                text: text.to_owned(),
                font,
                px,
                color,
                bounds: Rect::new(pos.x, pos.y, w, h).union(ink),
            });
        }
        w
    }

    pub fn push_clip(&mut self, r: Rect) {
        self.list.push(Prim::PushClip(r));
    }

    pub fn pop_clip(&mut self) {
        self.list.push(Prim::PopClip);
    }

    pub fn prim(&mut self, p: Prim) {
        self.list.push(p);
    }
}

pub struct Ui<'a> {
    st: &'a mut UiState,
    list: &'a mut DisplayList,
    overlay_list: DisplayList,
    fonts: &'a Fonts,
    cache: &'a mut GlyphCache,
    clipboard: &'a mut dyn Clipboard,
    style: Style,
    size: Size,
    scale: f32,
    rect: Rect,
    clip: Rect,
    layer: u8,
    input: Input,
    frame: FrameHits,
    hits: Vec<Hit>,
    focusables: Vec<Id>,
    focus_move: Option<bool>,
    capture: Option<Id>,
    next_capture: Option<Id>,
    repaint_in: Option<Duration>,
    cursor: CursorIcon,
}

impl<'a> Ui<'a> {
    /// Runs one UI frame inside an app's `frame` callback.
    pub fn run(state: &mut UiState, ctx: &mut FrameCtx, style: &Style, f: impl FnOnce(&mut Ui)) {
        let events = std::mem::take(&mut ctx.events);
        let (size, scale) = (ctx.size, ctx.scale);
        let (repaint, cursor) = {
            let mut ui = Ui::begin(
                state,
                &mut *ctx.list,
                ctx.fonts,
                &mut *ctx.cache,
                &mut *ctx.clipboard,
                style.clone(),
                size,
                scale,
                events,
            );
            f(&mut ui);
            ui.finish()
        };
        ctx.request_redraw_in = match (ctx.request_redraw_in, repaint) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        ctx.cursor = cursor;
        ctx.background = style.palette.bg;
    }

    #[allow(clippy::too_many_arguments)]
    fn begin(
        st: &'a mut UiState,
        list: &'a mut DisplayList,
        fonts: &'a Fonts,
        cache: &'a mut GlyphCache,
        clipboard: &'a mut dyn Clipboard,
        style: Style,
        size: Size,
        scale: f32,
        events: Vec<Event>,
    ) -> Ui<'a> {
        let mut frame = FrameHits::default();
        for e in &events {
            match e {
                Event::MouseMove(p) => st.mouse = *p,
                Event::MouseDown(b, p) => {
                    st.mouse = *p;
                    let top = top_hit(&st.prev_hits, *p);
                    match b {
                        MouseButton::Left => {
                            st.left = true;
                            st.active = top.map(|h| h.id);
                            st.drag_last = *p;
                            st.focus = top.filter(|h| h.sense.focus).map(|h| h.id);
                        }
                        MouseButton::Right => {
                            st.right = true;
                            st.right_active = top.map(|h| h.id);
                        }
                        _ => {}
                    }
                }
                Event::MouseUp(b, p) => {
                    st.mouse = *p;
                    let top = top_hit(&st.prev_hits, *p).map(|h| h.id);
                    match b {
                        MouseButton::Left => {
                            st.left = false;
                            if let Some(id) = st.active.take().filter(|id| Some(*id) == top) {
                                frame.clicked.insert(id);
                                let now = Instant::now();
                                let double = st.last_click.is_some_and(|(lid, t, lp)| {
                                    lid == id
                                        && now.duration_since(t) <= DOUBLE_CLICK
                                        && (lp.x - p.x).abs() <= 4
                                        && (lp.y - p.y).abs() <= 4
                                });
                                if double {
                                    frame.double_clicked.insert(id);
                                    st.last_click = None;
                                } else {
                                    st.last_click = Some((id, now, *p));
                                }
                            }
                        }
                        MouseButton::Right => {
                            st.right = false;
                            if let Some(id) = st.right_active.take().filter(|id| Some(*id) == top) {
                                frame.right_clicked.insert(id);
                            }
                        }
                        _ => {}
                    }
                }
                Event::KeyDown { mods, .. } | Event::KeyUp { mods, .. } => st.mods = *mods,
                Event::FocusLost => {
                    st.left = false;
                    st.right = false;
                    st.active = None;
                    st.right_active = None;
                    st.mods = Modifiers::NONE;
                }
                _ => {}
            }
        }
        if let (Some(id), true) = (st.active, st.left) {
            let d = Point::new(st.mouse.x - st.drag_last.x, st.mouse.y - st.drag_last.y);
            if d != Point::default() {
                frame.drag = Some((id, d));
            }
            st.drag_last = st.mouse;
        }
        frame.hovered = top_hit(&st.prev_hits, st.mouse).map(|h| h.id);
        let capture = st.capture;
        let input = Input { events, mouse: st.mouse, left_down: st.left, right_down: st.right, mods: st.mods };
        let window = Rect::new(0, 0, size.w, size.h);
        Ui {
            st,
            list,
            overlay_list: DisplayList::default(),
            fonts,
            cache,
            clipboard,
            style,
            size,
            scale,
            rect: window,
            clip: window,
            layer: 0,
            input,
            frame,
            hits: vec![],
            focusables: vec![],
            focus_move: None,
            capture,
            next_capture: None,
            repaint_in: None,
            cursor: CursorIcon::Default,
        }
    }

    fn finish(mut self) -> (Option<Duration>, CursorIcon) {
        // Tab / Shift+Tab nobody took moves focus.
        let mut mv = self.focus_move;
        let captured = self.capture.is_some();
        for e in &self.input.events {
            if let Event::KeyDown { key: Key::Tab, mods, .. } = e {
                if !captured && !mods.ctrl && !mods.alt && !mods.logo {
                    mv = Some(!mods.shift);
                }
            }
        }
        let order = &self.focusables;
        if let Some(forward) = mv {
            if !order.is_empty() {
                let cur = self.st.focus.and_then(|f| order.iter().position(|&o| o == f));
                let n = order.len();
                let next = match (cur, forward) {
                    (None, true) => 0,
                    (None, false) => n - 1,
                    (Some(i), true) => (i + 1) % n,
                    (Some(i), false) => (i + n - 1) % n,
                };
                self.st.focus = Some(order[next]);
            }
        } else if let Some(f) = self.st.focus {
            // Focus on a widget that no longer exists is dropped.
            if !self.hits.iter().any(|h| h.id == f) {
                self.st.focus = None;
            }
        }
        let overlay = std::mem::take(&mut self.overlay_list);
        self.list.prims.extend(overlay.prims);
        self.st.prev_hits = std::mem::take(&mut self.hits);
        self.st.capture = self.next_capture;
        (self.repaint_in, self.cursor)
    }

    /// Registers `rect` for this frame and reports what happened to it.
    pub fn interact(&mut self, id: Id, rect: Rect, sense: Sense) -> Response {
        let area = rect.intersect(self.clip).unwrap_or_default();
        self.hits.push(Hit { id, rect: area, layer: self.layer, sense });
        if sense.focus {
            self.focusables.push(id);
        }
        Response {
            rect,
            hovered: self.frame.hovered == Some(id),
            pressed: self.st.active == Some(id) && self.st.left,
            clicked: sense.click && self.frame.clicked.contains(&id),
            double_clicked: sense.click && self.frame.double_clicked.contains(&id),
            right_clicked: self.frame.right_clicked.contains(&id),
            dragged: self.frame.drag.filter(|(d, _)| sense.drag && *d == id).map(|(_, p)| p),
            focused: self.st.focus == Some(id),
        }
    }

    /// The area this code is laying out into.
    pub fn rect(&self) -> Rect {
        self.rect
    }

    pub fn window_rect(&self) -> Rect {
        Rect::new(0, 0, self.size.w, self.size.h)
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Logical → physical pixels.
    pub fn px(&self, logical: i32) -> i32 {
        (logical as f32 * self.scale).round() as i32
    }

    pub fn font_px(&self) -> f32 {
        self.style.font_px * self.scale
    }

    pub fn mono_px(&self) -> f32 {
        self.style.mono_px * self.scale
    }

    pub fn style(&self) -> &Style {
        &self.style
    }

    pub fn style_mut(&mut self) -> &mut Style {
        &mut self.style
    }

    pub fn input(&self) -> &Input {
        &self.input
    }

    pub fn input_mut(&mut self) -> &mut Input {
        &mut self.input
    }

    pub fn fonts(&self) -> &Fonts {
        self.fonts
    }

    pub fn measure(&mut self, text: &str, font: FontKind, px: f32) -> f32 {
        text_layout::measure(self.fonts, self.cache, font, text, px)
    }

    pub fn line_height(&self, font: FontKind, px: f32) -> i32 {
        text_layout::line_height(self.fonts, font, px) as i32
    }

    pub fn char_index_at_x(&mut self, text: &str, font: FontKind, px: f32, x: f32) -> usize {
        text_layout::char_index_at_x(self.fonts, self.cache, font, text, px, x)
    }

    pub fn painter(&mut self) -> Painter<'_> {
        let list = if self.layer > 0 { &mut self.overlay_list } else { &mut *self.list };
        Painter { list, fonts: self.fonts, cache: &mut *self.cache }
    }

    /// `layout::split_h` with `Px` sizes and `gap` in logical pixels.
    pub fn split_h(&self, r: Rect, sizes: &[layout::Size], gap: i32) -> Vec<Rect> {
        layout::split_h(r, &self.scaled(sizes), self.px(gap))
    }

    pub fn split_v(&self, r: Rect, sizes: &[layout::Size], gap: i32) -> Vec<Rect> {
        layout::split_v(r, &self.scaled(sizes), self.px(gap))
    }

    fn scaled(&self, sizes: &[layout::Size]) -> Vec<layout::Size> {
        sizes
            .iter()
            .map(|s| match s {
                layout::Size::Px(p) => layout::Size::Px(self.px(*p)),
                other => *other,
            })
            .collect()
    }

    /// Runs `f` with `rect` as the layout area.
    pub fn with_rect<R>(&mut self, rect: Rect, f: impl FnOnce(&mut Ui) -> R) -> R {
        let saved = std::mem::replace(&mut self.rect, rect);
        let r = f(self);
        self.rect = saved;
        r
    }

    /// Runs `f` with drawing and hit-testing clipped to `rect`.
    pub fn with_clip<R>(&mut self, rect: Rect, f: impl FnOnce(&mut Ui) -> R) -> R {
        let saved = self.clip;
        self.clip = saved.intersect(rect).unwrap_or_default();
        self.painter().push_clip(rect);
        let r = f(self);
        self.painter().pop_clip();
        self.clip = saved;
        r
    }

    /// Runs `f` on the overlay layer: drawn after everything else and hit
    /// before everything else (popups, menus, tooltips).
    pub fn overlay<R>(&mut self, f: impl FnOnce(&mut Ui) -> R) -> R {
        let saved = (self.clip, self.rect);
        self.clip = self.window_rect();
        self.rect = self.window_rect();
        self.layer += 1;
        let r = f(self);
        self.layer -= 1;
        (self.clip, self.rect) = saved;
        r
    }

    pub fn memory<T: Default + 'static>(&mut self, id: Id) -> &mut T {
        let e = self.st.memory.entry(id).or_insert_with(|| Box::new(T::default()));
        if !(**e).is::<T>() {
            *e = Box::new(T::default());
        }
        (**e).downcast_mut::<T>().expect("type checked above")
    }

    pub fn request_focus(&mut self, id: Id) {
        self.st.focus = Some(id);
    }

    pub fn surrender_focus(&mut self, id: Id) {
        if self.st.focus == Some(id) {
            self.st.focus = None;
        }
    }

    pub fn has_focus(&self, id: Id) -> bool {
        self.st.focus == Some(id)
    }

    pub fn focused(&self) -> Option<Id> {
        self.st.focus
    }

    /// Routes keyboard input to `id` for this frame and the next (call it
    /// every frame while e.g. a menu is open). Other widgets check
    /// `keys_for` and leave keys alone meanwhile.
    pub fn capture_keyboard(&mut self, id: Id) {
        self.capture = Some(id);
        self.next_capture = Some(id);
    }

    /// Whether widget `id` may act on keyboard input this frame.
    pub fn keys_for(&self, id: Id) -> bool {
        self.capture.is_none_or(|c| c == id)
    }

    pub fn focus_next(&mut self) {
        self.focus_move = Some(true);
    }

    pub fn focus_prev(&mut self) {
        self.focus_move = Some(false);
    }

    /// The widget currently held down by the primary button, if any.
    pub fn active(&self) -> Option<Id> {
        self.st.active.filter(|_| self.st.left)
    }

    pub fn set_cursor_icon(&mut self, c: CursorIcon) {
        self.cursor = c;
    }

    pub fn request_repaint_in(&mut self, d: Duration) {
        self.repaint_in = Some(self.repaint_in.map_or(d, |r| r.min(d)));
    }

    pub fn clipboard(&mut self) -> &mut dyn Clipboard {
        &mut *self.clipboard
    }
}

/// Offscreen driver for widget tests.
pub mod testing {
    use super::*;
    use crate::canvas::Canvas;
    use crate::clipboard::MemoryClipboard;
    use crate::display::render_full;
    use crate::text::test_fonts;

    pub struct Harness {
        pub state: UiState,
        pub fonts: Fonts,
        pub clipboard: MemoryClipboard,
        pub style: Style,
        pub repaint_in: Option<Duration>,
        pub cursor: CursorIcon,
        size: Size,
        scale: f32,
        list: DisplayList,
        cache: GlyphCache,
    }

    impl Harness {
        /// A `w`×`h` physical-pixel window using the deterministic test font.
        pub fn new(w: i32, h: i32, scale: f32) -> Self {
            Self::with_fonts(w, h, scale, test_fonts())
        }

        pub fn with_fonts(w: i32, h: i32, scale: f32, fonts: Fonts) -> Self {
            Harness {
                state: UiState::default(),
                fonts,
                clipboard: MemoryClipboard::default(),
                style: Style::dark_default(),
                repaint_in: None,
                cursor: CursorIcon::Default,
                size: Size::new(w, h),
                scale,
                list: DisplayList::default(),
                cache: GlyphCache::default(),
            }
        }

        /// Runs one frame with `events`.
        pub fn frame(&mut self, events: Vec<Event>, f: &mut impl FnMut(&mut Ui)) {
            self.list.clear();
            let mut ctx = FrameCtx {
                events,
                size: self.size,
                scale: self.scale,
                list: &mut self.list,
                fonts: &self.fonts,
                cache: &mut self.cache,
                clipboard: &mut self.clipboard,
                request_redraw_in: None,
                title: None,
                quit: false,
                background: Color::BLACK,
                cursor: CursorIcon::Default,
                replace_fonts: None,
            };
            Ui::run(&mut self.state, &mut ctx, &self.style, |ui| f(ui));
            self.repaint_in = ctx.request_redraw_in;
            self.cursor = ctx.cursor;
            if let Some(fonts) = ctx.replace_fonts.take() {
                self.fonts = fonts;
                self.cache.clear();
            }
        }

        pub fn list(&self) -> &DisplayList {
            &self.list
        }

        pub fn scale(&self) -> f32 {
            self.scale
        }

        pub fn resize(&mut self, w: i32, h: i32) {
            self.size = Size::new(w, h);
        }

        /// Rasterizes the last frame.
        pub fn render(&mut self) -> Canvas {
            let mut c = Canvas::new(self.size.w, self.size.h);
            render_full(&self.list, &mut c, &self.fonts, &mut self.cache, self.style.palette.bg);
            c
        }
    }
}

#[cfg(test)]
#[path = "ui_tests.rs"]
mod tests;
