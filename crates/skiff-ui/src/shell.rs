//! The window shell: winit for the window and input, softbuffer to put
//! pixels on screen. Everything an `App` sees is the toolkit's own types,
//! so apps are testable offscreen with `render_offscreen`.

use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseScrollDelta, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::Key as WKey;
use winit::window::{Window, WindowId};

use crate::canvas::Canvas;
use crate::clipboard::{Clipboard, MemoryClipboard, SystemClipboard};
use crate::color::Color;
use crate::display::{DisplayList, TileRenderer};
pub use crate::geom::Size;
use crate::geom::{Point, Rect};
use crate::input::{CursorIcon, Event, Key, Modifiers, MouseButton, map_char_key, map_named_key};
use crate::text::Fonts;
use crate::text::cache::GlyphCache;

/// Everything an app gets (and can ask for) during one frame.
pub struct FrameCtx<'a> {
    /// Input since the last frame, in toolkit form.
    pub events: Vec<Event>,
    /// Window size in physical pixels.
    pub size: Size,
    /// Physical pixels per logical pixel (the window's DPI factor).
    pub scale: f32,
    /// Where the frame's drawing goes; the shell renders it afterwards.
    pub list: &'a mut DisplayList,
    pub fonts: &'a Fonts,
    /// Rasterized glyphs, shared across frames.
    pub cache: &'a mut GlyphCache,
    pub clipboard: &'a mut dyn Clipboard,
    /// Ask for another frame after this long (animations, caret blink).
    pub request_redraw_in: Option<Duration>,
    /// Set to change the window title.
    pub title: Option<String>,
    /// Set to close the window and end the event loop.
    pub quit: bool,
    /// Background painted beneath the display list.
    pub background: Color,
    /// Mouse cursor shape to show.
    pub cursor: CursorIcon,
    /// Set to swap fonts after this frame: the shell drops cached glyphs,
    /// repaints everything and immediately runs another frame.
    pub replace_fonts: Option<Fonts>,
    /// An assistive technology is listening: build the accessibility tree.
    pub a11y_active: bool,
    /// This frame's accessibility tree (set by `Ui::run` when active).
    pub a11y_tree: Option<crate::a11y::Tree>,
}

/// An application driven by the shell: one `frame` call per redraw.
pub trait App {
    /// Builds the frame: reads `ctx.events`, fills `ctx.list`, sets requests.
    fn frame(&mut self, ctx: &mut FrameCtx);
    /// Called once when the event loop is shutting down (save state here).
    fn exit(&mut self) {}
}

/// How to open the main window.
pub struct WindowOptions {
    pub title: String,
    /// Window class (Windows, X11) / app id (Wayland) desktops group windows by.
    pub app_id: String,
    /// Initial size in logical pixels.
    pub size: (u32, u32),
    /// Fonts the app starts with.
    pub fonts: Fonts,
    /// Window / taskbar icon: square straight-RGBA pixels and their side
    /// length (e.g. `logo::rgba(64)`), or none for the platform default.
    pub icon: Option<(Vec<u8>, u32)>,
}

/// Wakes the shell from another thread (e.g. when a transfer progresses).
#[derive(Clone)]
pub struct Waker(EventLoopProxy<UserEvent>);

/// Events the shell's event loop receives from other threads.
#[derive(Debug)]
pub enum UserEvent {
    /// Just run another frame.
    Wake,
    /// The screen reader adapter wants something.
    A11y(accesskit_winit::Event),
}

impl From<accesskit_winit::Event> for UserEvent {
    /// Wraps an AccessKit adapter event (needed by the adapter's event-loop proxy).
    fn from(e: accesskit_winit::Event) -> Self {
        UserEvent::A11y(e)
    }
}

impl Waker {
    /// Queues a frame; harmless if the event loop has already exited.
    pub fn wake(&self) {
        let _ = self.0.send_event(UserEvent::Wake);
    }
}

/// Input-translation state carried between winit events.
#[derive(Default)]
pub struct ShellState {
    /// Modifier keys currently held (winit reports them separately from keys).
    pub mods: Modifiers,
    /// Last pointer position; button events carry it.
    pub cursor: Point,
}

/// Converts a winit mouse button; Back / Forward become `Other(3)` / `Other(4)`.
fn map_button(b: winit::event::MouseButton) -> MouseButton {
    use winit::event::MouseButton as B;
    match b {
        B::Left => MouseButton::Left,
        B::Right => MouseButton::Right,
        B::Middle => MouseButton::Middle,
        B::Back => MouseButton::Other(3),
        B::Forward => MouseButton::Other(4),
        B::Other(n) => MouseButton::Other(n),
    }
}

/// Lines → pixels for wheel deltas reported in lines.
const WHEEL_LINE_PX: f32 = 48.0;

/// Translates one winit window event into toolkit events.
/// Wheel deltas in lines are scaled to physical pixels.
pub fn from_winit(ev: &WindowEvent, st: &mut ShellState, scale: f32) -> Vec<Event> {
    match ev {
        WindowEvent::ModifiersChanged(m) => {
            let s = m.state();
            st.mods = Modifiers { ctrl: s.control_key(), shift: s.shift_key(), alt: s.alt_key(), logo: s.super_key() };
            vec![Event::Modifiers(st.mods)]
        }
        // Keys: unmapped named keys are dropped; a release becomes KeyUp only.
        WindowEvent::KeyboardInput { event, .. } => {
            let key = match &event.logical_key {
                WKey::Named(n) => match map_named_key(n) {
                    Some(k) => k,
                    None => return vec![],
                },
                WKey::Character(s) => map_char_key(s),
                _ => return vec![],
            };
            let mods = st.mods;
            if event.state == ElementState::Released {
                return vec![Event::KeyUp { key, mods }];
            }
            // Typed text (none if it holds control characters). It also becomes Text
            // unless a shortcut modifier is held (Ctrl+Alt counts as AltGr, so it
            // types) or the key is Enter / Tab, which widgets handle as keys.
            let text = event.text.as_ref().map(|t| t.to_string()).filter(|t| t.chars().all(|c| !c.is_control()));
            let mut out = vec![Event::KeyDown { key: key.clone(), mods, text: text.clone() }];
            if let Some(t) = text
                && (!mods.ctrl || mods.alt)
                && !mods.logo
                && key != Key::Enter
                && key != Key::Tab
            {
                out.push(Event::Text(t));
            }
            out
        }
        WindowEvent::Ime(winit::event::Ime::Commit(s)) => vec![Event::Text(s.clone())],
        WindowEvent::CursorMoved { position, .. } => {
            st.cursor = Point::new(position.x.round() as i32, position.y.round() as i32);
            vec![Event::MouseMove(st.cursor)]
        }
        WindowEvent::MouseInput { state, button, .. } => {
            let b = map_button(*button);
            vec![match state {
                ElementState::Pressed => Event::MouseDown(b, st.cursor),
                ElementState::Released => Event::MouseUp(b, st.cursor),
            }]
        }
        WindowEvent::MouseWheel { delta, .. } => {
            let (dx, dy) = match delta {
                MouseScrollDelta::LineDelta(x, y) => (x * WHEEL_LINE_PX * scale, y * WHEEL_LINE_PX * scale),
                MouseScrollDelta::PixelDelta(p) => (p.x as f32, p.y as f32),
            };
            vec![Event::Wheel { dx, dy }]
        }
        WindowEvent::DroppedFile(p) => vec![Event::FileDropped(p.clone())],
        WindowEvent::Focused(false) => {
            st.mods = Modifiers::NONE;
            vec![Event::FocusLost]
        }
        WindowEvent::Focused(true) => vec![Event::FocusGained],
        // The pointer left the window: park it off-window so nothing stays hovered.
        WindowEvent::CursorLeft { .. } => {
            st.cursor = Point::new(-1, -1);
            vec![Event::MouseMove(st.cursor)]
        }
        _ => vec![],
    }
}

/// Runs one app frame into `list` and renders it. Returns damage.
#[allow(clippy::too_many_arguments)]
fn frame_once(
    app: &mut dyn App,
    events: Vec<Event>,
    canvas: &mut Canvas,
    scale: f32,
    list: &mut DisplayList,
    renderer: &mut TileRenderer,
    fonts: &Fonts,
    cache: &mut GlyphCache,
    clipboard: &mut dyn Clipboard,
    a11y_active: bool,
) -> FrameOut {
    list.clear();
    let mut ctx = FrameCtx {
        events,
        size: Size::new(canvas.width, canvas.height),
        scale,
        list,
        fonts,
        cache,
        clipboard,
        request_redraw_in: None,
        title: None,
        quit: false,
        background: Color::BLACK,
        cursor: CursorIcon::Default,
        replace_fonts: None,
        a11y_active,
        a11y_tree: None,
    };
    app.frame(&mut ctx);
    // Take what the app set before rendering, since `ctx` borrows `list`.
    let (redraw_in, title, quit, bg, cursor, replace_fonts, tree) =
        (ctx.request_redraw_in, ctx.title.take(), ctx.quit, ctx.background, ctx.cursor, ctx.replace_fonts.take(), ctx.a11y_tree.take());
    let damage = renderer.render(list, canvas, fonts, cache, bg);
    FrameOut { damage, redraw_in, title, quit, cursor, replace_fonts, tree }
}

/// What one frame produced: changed screen areas plus the app's requests.
struct FrameOut {
    /// Canvas areas repainted this frame.
    damage: Vec<Rect>,
    redraw_in: Option<Duration>,
    title: Option<String>,
    quit: bool,
    cursor: CursorIcon,
    replace_fonts: Option<Fonts>,
    tree: Option<crate::a11y::Tree>,
}

/// Installs fonts an app asked for: cached glyphs and tile hashes belong
/// to the old fonts, so both are dropped and a new frame is due now.
fn apply_font_swap(out: &mut FrameOut, fonts: &mut Fonts, cache: &mut GlyphCache, renderer: &mut TileRenderer) {
    if let Some(f) = out.replace_fonts.take() {
        *fonts = f;
        cache.clear();
        renderer.invalidate();
        renderer.clear_glyphs();
        out.redraw_in = Some(Duration::ZERO);
    }
}

/// A headless shell: runs app frames into an in-memory canvas.
pub(crate) struct Offscreen {
    pub canvas: Canvas,
    pub scale: f32,
    pub fonts: Fonts,
    pub cache: GlyphCache,
    list: DisplayList,
    renderer: TileRenderer,
    clip: MemoryClipboard,
}

impl Offscreen {
    /// A `size` canvas at `scale`, with a memory clipboard.
    pub fn new(size: Size, scale: f32, fonts: Fonts) -> Self {
        Self {
            canvas: Canvas::new(size.w, size.h),
            scale,
            fonts,
            cache: GlyphCache::default(),
            list: DisplayList::default(),
            renderer: TileRenderer::default(),
            clip: MemoryClipboard::default(),
        }
    }

    /// Runs one frame (accessibility off) and applies a font swap.
    fn frame(&mut self, app: &mut dyn App, events: Vec<Event>) -> FrameOut {
        let mut out = frame_once(
            app,
            events,
            &mut self.canvas,
            self.scale,
            &mut self.list,
            &mut self.renderer,
            &self.fonts,
            &mut self.cache,
            &mut self.clip,
            false,
        );
        apply_font_swap(&mut out, &mut self.fonts, &mut self.cache, &mut self.renderer);
        out
    }
}

/// Converts the toolkit cursor to winit's.
fn winit_cursor(c: CursorIcon) -> winit::window::CursorIcon {
    use winit::window::CursorIcon as W;
    match c {
        CursorIcon::Default => W::Default,
        CursorIcon::Pointer => W::Pointer,
        CursorIcon::Text => W::Text,
        CursorIcon::ResizeH => W::EwResize,
        CursorIcon::ResizeV => W::NsResize,
        CursorIcon::Grab => W::Grab,
        CursorIcon::Grabbing => W::Grabbing,
        CursorIcon::Wait => W::Wait,
    }
}

/// Runs `app` headlessly for one frame per entry in `frames` and returns
/// the final canvas.
pub fn render_offscreen(app: &mut impl App, size: Size, scale: f32, frames: Vec<Vec<Event>>, fonts: &Fonts) -> Canvas {
    let mut off = Offscreen::new(size, scale, fonts.clone());
    for events in frames {
        off.frame(app, events);
    }
    off.canvas
}

/// The window and its presentation surface, created on `resumed`.
struct Gfx {
    window: Rc<Window>,
    surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
    /// Talks to the platform screen reader.
    adapter: accesskit_winit::Adapter,
}

/// The winit application handler: owns the app, the window and the frame state.
struct Shell<A: App> {
    app: A,
    /// Current title (follows `FrameCtx::title` changes).
    opts_title: String,
    opts_app_id: String,
    /// The window icon from `WindowOptions`.
    opts_icon: Option<(Vec<u8>, u32)>,
    opts_size: (u32, u32),
    fonts: Fonts,
    gfx: Option<Gfx>,
    state: ShellState,
    /// Events queued since the last frame.
    pending: Vec<Event>,
    /// The rendered frame; copied to the window surface where damaged.
    canvas: Canvas,
    list: DisplayList,
    renderer: TileRenderer,
    cache: GlyphCache,
    clipboard: SystemClipboard,
    /// When the app asked to be redrawn next.
    redraw_at: Option<Instant>,
    /// We asked for the pending redraw (vs. the OS asking).
    requested: bool,
    /// The OS asked for a redraw: present even if nothing changed.
    os_redraw: bool,
    /// The cursor currently set on the window.
    cursor: CursorIcon,
    /// A fatal error; ends the loop and is returned from `run`.
    error: Option<anyhow::Error>,
    /// Hands the AccessKit adapter a way back into the event loop.
    proxy: EventLoopProxy<UserEvent>,
    /// A screen reader asked for the tree and hasn't gone away.
    a11y_active: bool,
}

/// Sets the window class / app id where the platform has one.
fn with_app_id(attrs: winit::window::WindowAttributes, id: &str) -> winit::window::WindowAttributes {
    if id.is_empty() {
        return attrs;
    }
    #[cfg(windows)]
    let attrs = winit::platform::windows::WindowAttributesExtWindows::with_class_name(attrs, id);
    #[cfg(all(unix, not(any(target_os = "macos", target_os = "ios", target_os = "android"))))]
    let attrs = {
        let attrs = winit::platform::x11::WindowAttributesExtX11::with_name(attrs, id, id);
        winit::platform::wayland::WindowAttributesExtWayland::with_name(attrs, id, id)
    };
    attrs
}

impl<A: App> Shell<A> {
    /// Asks winit for a redraw and remembers that we asked.
    fn request_redraw(&mut self) {
        self.requested = true;
        if let Some(g) = &self.gfx {
            g.window.request_redraw();
        }
    }

    /// Creates the window, the AccessKit adapter and the softbuffer surface.
    fn create_window(&mut self, el: &ActiveEventLoop) -> Result<()> {
        // Hidden until the screen reader adapter is attached (AccessKit's rule).
        let attrs = Window::default_attributes()
            .with_title(self.opts_title.clone())
            .with_inner_size(LogicalSize::new(self.opts_size.0, self.opts_size.1))
            .with_visible(false);
        let attrs = with_app_id(attrs, &self.opts_app_id);
        // A bad icon only costs the icon, never the window.
        let icon = self.opts_icon.clone().and_then(|(rgba, size)| winit::window::Icon::from_rgba(rgba, size, size).ok());
        // Windows keeps a separate (large) taskbar icon.
        #[cfg(windows)]
        let attrs = winit::platform::windows::WindowAttributesExtWindows::with_taskbar_icon(attrs, icon.clone());
        let attrs = attrs.with_window_icon(icon);
        let window = Rc::new(el.create_window(attrs).context("creating window")?);
        let adapter = accesskit_winit::Adapter::with_event_loop_proxy(el, &window, self.proxy.clone());
        window.set_visible(true);
        let context = softbuffer::Context::new(window.clone()).map_err(|e| anyhow::anyhow!("softbuffer context: {e}"))?;
        let surface = softbuffer::Surface::new(&context, window.clone()).map_err(|e| anyhow::anyhow!("softbuffer surface: {e}"))?;
        window.request_redraw();
        self.gfx = Some(Gfx { window, surface, adapter });
        Ok(())
    }

    /// Runs a frame at the window's current size and scale, applies the app's
    /// requests, and copies the damaged canvas areas to the screen.
    fn redraw(&mut self, el: &ActiveEventLoop) -> Result<()> {
        let Some(gfx) = self.gfx.as_mut() else { return Ok(()) };
        let phys = gfx.window.inner_size();
        let scale = gfx.window.scale_factor() as f32;
        let (w, h) = (phys.width as i32, phys.height as i32);
        if self.canvas.width != w || self.canvas.height != h {
            self.canvas.resize(w, h);
        }
        let events = std::mem::take(&mut self.pending);
        let mut out = frame_once(
            &mut self.app,
            events,
            &mut self.canvas,
            scale,
            &mut self.list,
            &mut self.renderer,
            &self.fonts,
            &mut self.cache,
            &mut self.clipboard,
            self.a11y_active,
        );
        apply_font_swap(&mut out, &mut self.fonts, &mut self.cache, &mut self.renderer);
        let FrameOut { damage, redraw_in, title, quit, cursor, tree, .. } = out;
        // Apply what the app asked for: title, accessibility tree, cursor, quit.
        if let Some(t) = title {
            gfx.window.set_title(&t);
            self.opts_title = t;
        }
        if let Some(tree) = tree {
            let title = &self.opts_title;
            gfx.adapter.update_if_active(|| crate::a11y_kit::to_update(&tree, title));
        }
        if cursor != self.cursor {
            self.cursor = cursor;
            gfx.window.set_cursor(winit_cursor(cursor));
        }
        if quit {
            el.exit();
        }
        self.redraw_at = redraw_in.map(|d| Instant::now() + d);
        let (Some(nw), Some(nh)) = (NonZeroU32::new(phys.width), NonZeroU32::new(phys.height)) else {
            return Ok(()); // minimized / zero-size: nothing to present
        };
        if damage.is_empty() && !self.os_redraw {
            return Ok(()); // nothing changed on screen
        }
        self.os_redraw = false;
        gfx.surface.resize(nw, nh).map_err(|e| anyhow::anyhow!("surface resize: {e}"))?;
        let mut buf = gfx.surface.buffer_mut().map_err(|e| anyhow::anyhow!("surface buffer: {e}"))?;
        // A buffer that does not hold the previous frame gets a full copy.
        let full = buf.age() != 1;
        let rects: Vec<Rect> = if full { vec![self.canvas.bounds()] } else { damage };
        // Copy damaged rows from the canvas into the window buffer.
        let width = w as usize;
        for r in &rects {
            for y in r.y..r.bottom() {
                let s = y as usize * width + r.x as usize;
                let e = s + r.w as usize;
                buf[s..e].copy_from_slice(&self.canvas.pixels[s..e]);
            }
        }
        let sb: Vec<softbuffer::Rect> = rects
            .iter()
            .filter_map(|r| {
                Some(softbuffer::Rect {
                    x: r.x as u32,
                    y: r.y as u32,
                    width: NonZeroU32::new(r.w as u32)?,
                    height: NonZeroU32::new(r.h as u32)?,
                })
            })
            .collect();
        buf.present_with_damage(&sb).map_err(|e| anyhow::anyhow!("present: {e}"))
    }

    /// Stores a fatal error and ends the event loop.
    fn fail(&mut self, el: &ActiveEventLoop, e: anyhow::Error) {
        self.error = Some(e);
        el.exit();
    }
}

impl<A: App> ApplicationHandler<UserEvent> for Shell<A> {
    /// Lets the app save state before the loop ends.
    fn exiting(&mut self, _el: &ActiveEventLoop) {
        self.app.exit();
    }

    /// A `redraw_in` deadline passed: run a frame.
    fn new_events(&mut self, _el: &ActiveEventLoop, cause: StartCause) {
        if let StartCause::ResumeTimeReached { .. } = cause {
            self.redraw_at = None;
            self.request_redraw();
        }
    }

    /// Creates the window the first time the app is resumed.
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.gfx.is_none()
            && let Err(e) = self.create_window(el)
        {
            self.fail(el, e);
        }
    }

    /// Handles wakeups and screen-reader requests (activation, actions).
    fn user_event(&mut self, _el: &ActiveEventLoop, ev: UserEvent) {
        use accesskit_winit::WindowEvent as A;
        match ev {
            UserEvent::Wake => {}
            UserEvent::A11y(e) => match e.window_event {
                A::InitialTreeRequested => self.a11y_active = true,
                A::ActionRequested(req) => self.pending.extend(crate::a11y_kit::to_event(req)),
                A::AccessibilityDeactivated => self.a11y_active = false,
            },
        }
        self.request_redraw();
    }

    /// Feeds winit events to the AccessKit adapter, then handles close,
    /// redraw, resize, or translates them into input for the next frame.
    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if let Some(g) = self.gfx.as_mut() {
            g.adapter.process_event(&g.window, &event);
        }
        match &event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::RedrawRequested => {
                // A redraw we did not ask for comes from the OS (window
                // uncovered, restored): the screen needs every pixel.
                if !std::mem::take(&mut self.requested) {
                    self.os_redraw = true;
                    self.renderer.invalidate();
                }
                if let Err(e) = self.redraw(el) {
                    self.fail(el, e);
                }
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                self.renderer.invalidate();
                self.request_redraw();
            }
            _ => {
                let scale = self.gfx.as_ref().map_or(1.0, |g| g.window.scale_factor() as f32);
                let evs = from_winit(&event, &mut self.state, scale);
                if !evs.is_empty() {
                    self.pending.extend(evs);
                    self.request_redraw();
                }
            }
        }
    }

    /// Sleeps until the next event, or until the app's requested redraw time.
    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        el.set_control_flow(match self.redraw_at {
            Some(t) => ControlFlow::WaitUntil(t),
            None => ControlFlow::Wait,
        });
    }
}

/// Opens a window and runs `app` until it quits or the window closes.
pub fn run(app: impl App + 'static, opts: WindowOptions) -> Result<()> {
    run_with_waker(|_| app, opts)
}

/// Like `run`, but hands the app constructor a `Waker` first.
pub fn run_with_waker<A: App + 'static>(make: impl FnOnce(Waker) -> A, opts: WindowOptions) -> Result<()> {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build().context("creating event loop")?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let app = make(Waker(proxy.clone()));
    let mut shell = Shell {
        app,
        opts_title: opts.title,
        opts_app_id: opts.app_id,
        opts_size: opts.size,
        opts_icon: opts.icon,
        fonts: opts.fonts,
        gfx: None,
        state: ShellState::default(),
        pending: vec![],
        canvas: Canvas::new(0, 0),
        list: DisplayList::default(),
        renderer: TileRenderer::default(),
        cache: GlyphCache::default(),
        clipboard: SystemClipboard::default(),
        redraw_at: None,
        requested: false,
        os_redraw: false,
        cursor: CursorIcon::Default,
        error: None,
        proxy,
        a11y_active: false,
    };
    event_loop.run_app(&mut shell).context("event loop")?;
    match shell.error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Color;
    use crate::display::Prim;
    use crate::geom::Rect;
    use crate::text::test_fonts;

    /// A test app that fills the window red and counts its frames.
    struct Red {
        frames: usize,
    }
    impl App for Red {
        /// Pushes a full-window red rect.
        fn frame(&mut self, ctx: &mut FrameCtx) {
            self.frames += 1;
            ctx.list.push(Prim::Rect { rect: Rect::from_size(ctx.size), color: Color::hex(0xff0000), radius: 0.0 });
        }
    }

    /// `render_offscreen` runs one frame per event batch and returns the pixels.
    #[test]
    fn offscreen_renders_app_frames() {
        let mut app = Red { frames: 0 };
        let c = render_offscreen(&mut app, Size::new(100, 80), 1.0, vec![vec![], vec![Event::FocusLost]], &test_fonts());
        assert_eq!(app.frames, 2);
        assert_eq!(c.pixel(50, 40), 0xff0000);
    }

    /// A test app that draws text and swaps fonts on its second frame.
    struct Swapper {
        n: usize,
    }
    impl App for Swapper {
        /// Draws "abc"; asks for new fonts on frame 2.
        fn frame(&mut self, ctx: &mut FrameCtx) {
            self.n += 1;
            ctx.list.push(Prim::Text {
                origin: crate::geom::Point::new(0, 0),
                text: "abc".into(),
                font: crate::text::FontKind::Ui,
                px: 14.0,
                color: Color::WHITE,
                bounds: Rect::new(0, 0, 30, 16),
            });
            if self.n == 2 {
                ctx.replace_fonts = Some(test_fonts());
            }
        }
    }

    /// A font swap clears the glyph cache, asks for an immediate redraw and
    /// repaints the whole window.
    #[test]
    fn replace_fonts_clears_cache_and_repaints() {
        let mut app = Swapper { n: 0 };
        let mut off = Offscreen::new(Size::new(100, 80), 1.0, test_fonts());
        off.frame(&mut app, vec![]);
        assert!(!off.cache.is_empty());
        let out = off.frame(&mut app, vec![]);
        assert!(off.cache.is_empty(), "glyph cache cleared after a font swap");
        assert_eq!(out.redraw_in, Some(Duration::ZERO), "swap requests an immediate redraw");
        let out = off.frame(&mut app, vec![]);
        let area: i32 = out.damage.iter().map(|r| r.w * r.h).sum();
        assert_eq!(area, 100 * 80, "whole window repainted with the new fonts");
    }

    /// A 0x0 window renders without panicking and has no pixels.
    #[test]
    fn zero_size_frame_is_noop() {
        let mut app = Red { frames: 0 };
        let c = render_offscreen(&mut app, Size::new(0, 0), 1.0, vec![vec![]], &test_fonts());
        assert!(c.pixels.is_empty());
    }
}
