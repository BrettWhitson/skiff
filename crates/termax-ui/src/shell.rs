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
use crate::geom::{Point, Rect};
pub use crate::geom::Size;
use crate::input::{CursorIcon, Event, Key, Modifiers, MouseButton, map_char_key, map_named_key};
use crate::text::Fonts;
use crate::text::cache::GlyphCache;

/// Everything an app gets (and can ask for) during one frame.
pub struct FrameCtx<'a> {
    pub events: Vec<Event>,
    /// Window size in physical pixels.
    pub size: Size,
    pub scale: f32,
    pub list: &'a mut DisplayList,
    pub fonts: &'a Fonts,
    pub cache: &'a mut GlyphCache,
    pub clipboard: &'a mut dyn Clipboard,
    /// Ask for another frame after this long (animations, caret blink).
    pub request_redraw_in: Option<Duration>,
    pub title: Option<String>,
    pub quit: bool,
    /// Background painted beneath the display list.
    pub background: Color,
    pub cursor: CursorIcon,
    /// Set to swap fonts after this frame: the shell drops cached glyphs,
    /// repaints everything and immediately runs another frame.
    pub replace_fonts: Option<Fonts>,
}

pub trait App {
    fn frame(&mut self, ctx: &mut FrameCtx);
}

pub struct WindowOptions {
    pub title: String,
    /// Initial size in logical pixels.
    pub size: (u32, u32),
    pub fonts: Fonts,
}

/// Wakes the shell from another thread (e.g. when a transfer progresses).
#[derive(Clone)]
pub struct Waker(EventLoopProxy<Wake>);

#[derive(Debug, Clone, Copy)]
pub struct Wake;

impl Waker {
    pub fn wake(&self) {
        let _ = self.0.send_event(Wake);
    }
}

/// Input-translation state carried between winit events.
#[derive(Default)]
pub struct ShellState {
    pub mods: Modifiers,
    pub cursor: Point,
}

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
pub fn from_winit(ev: &WindowEvent, st: &mut ShellState, scale: f32) -> Vec<Event> {
    match ev {
        WindowEvent::ModifiersChanged(m) => {
            let s = m.state();
            st.mods = Modifiers { ctrl: s.control_key(), shift: s.shift_key(), alt: s.alt_key(), logo: s.super_key() };
            vec![]
        }
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
            let text = event.text.as_ref().map(|t| t.to_string()).filter(|t| t.chars().all(|c| !c.is_control()));
            let mut out = vec![Event::KeyDown { key: key.clone(), mods, text: text.clone() }];
            if let Some(t) = text {
                if (!mods.ctrl || mods.alt) && !mods.logo && key != Key::Enter && key != Key::Tab {
                    out.push(Event::Text(t));
                }
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
    };
    app.frame(&mut ctx);
    let (redraw_in, title, quit, bg, cursor, replace_fonts) =
        (ctx.request_redraw_in, ctx.title.take(), ctx.quit, ctx.background, ctx.cursor, ctx.replace_fonts.take());
    let damage = renderer.render(list, canvas, fonts, cache, bg);
    FrameOut { damage, redraw_in, title, quit, cursor, replace_fonts }
}

struct FrameOut {
    damage: Vec<Rect>,
    redraw_in: Option<Duration>,
    title: Option<String>,
    quit: bool,
    cursor: CursorIcon,
    replace_fonts: Option<Fonts>,
}

/// Installs fonts an app asked for: cached glyphs and tile hashes belong
/// to the old fonts, so both are dropped and a new frame is due now.
fn apply_font_swap(out: &mut FrameOut, fonts: &mut Fonts, cache: &mut GlyphCache, renderer: &mut TileRenderer) {
    if let Some(f) = out.replace_fonts.take() {
        *fonts = f;
        cache.clear();
        renderer.invalidate();
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
        );
        apply_font_swap(&mut out, &mut self.fonts, &mut self.cache, &mut self.renderer);
        out
    }
}

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

struct Gfx {
    window: Rc<Window>,
    surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
}

struct Shell<A: App> {
    app: A,
    opts_title: String,
    opts_size: (u32, u32),
    fonts: Fonts,
    gfx: Option<Gfx>,
    state: ShellState,
    pending: Vec<Event>,
    canvas: Canvas,
    list: DisplayList,
    renderer: TileRenderer,
    cache: GlyphCache,
    clipboard: SystemClipboard,
    redraw_at: Option<Instant>,
    requested: bool,
    os_redraw: bool,
    cursor: CursorIcon,
    error: Option<anyhow::Error>,
}

impl<A: App> Shell<A> {
    fn request_redraw(&mut self) {
        self.requested = true;
        if let Some(g) = &self.gfx {
            g.window.request_redraw();
        }
    }

    fn create_window(&mut self, el: &ActiveEventLoop) -> Result<()> {
        let attrs = Window::default_attributes()
            .with_title(self.opts_title.clone())
            .with_inner_size(LogicalSize::new(self.opts_size.0, self.opts_size.1));
        let window = Rc::new(el.create_window(attrs).context("creating window")?);
        let context = softbuffer::Context::new(window.clone()).map_err(|e| anyhow::anyhow!("softbuffer context: {e}"))?;
        let surface =
            softbuffer::Surface::new(&context, window.clone()).map_err(|e| anyhow::anyhow!("softbuffer surface: {e}"))?;
        window.request_redraw();
        self.gfx = Some(Gfx { window, surface });
        Ok(())
    }

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
        );
        apply_font_swap(&mut out, &mut self.fonts, &mut self.cache, &mut self.renderer);
        let FrameOut { damage, redraw_in, title, quit, cursor, .. } = out;
        if let Some(t) = title {
            gfx.window.set_title(&t);
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

    fn fail(&mut self, el: &ActiveEventLoop, e: anyhow::Error) {
        self.error = Some(e);
        el.exit();
    }
}

impl<A: App> ApplicationHandler<Wake> for Shell<A> {
    fn new_events(&mut self, _el: &ActiveEventLoop, cause: StartCause) {
        if let StartCause::ResumeTimeReached { .. } = cause {
            self.redraw_at = None;
            self.request_redraw();
        }
    }

    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.gfx.is_none() {
            if let Err(e) = self.create_window(el) {
                self.fail(el, e);
            }
        }
    }

    fn user_event(&mut self, _el: &ActiveEventLoop, _ev: Wake) {
        self.request_redraw();
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
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
    let event_loop = EventLoop::<Wake>::with_user_event().build().context("creating event loop")?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let app = make(Waker(event_loop.create_proxy()));
    let mut shell = Shell {
        app,
        opts_title: opts.title,
        opts_size: opts.size,
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

    struct Red {
        frames: usize,
    }
    impl App for Red {
        fn frame(&mut self, ctx: &mut FrameCtx) {
            self.frames += 1;
            ctx.list.push(Prim::Rect { rect: Rect::from_size(ctx.size), color: Color::hex(0xff0000), radius: 0.0 });
        }
    }

    #[test]
    fn offscreen_renders_app_frames() {
        let mut app = Red { frames: 0 };
        let c = render_offscreen(&mut app, Size::new(100, 80), 1.0, vec![vec![], vec![Event::FocusLost]], &test_fonts());
        assert_eq!(app.frames, 2);
        assert_eq!(c.pixel(50, 40), 0xff0000);
    }

    struct Swapper {
        n: usize,
    }
    impl App for Swapper {
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

    #[test]
    fn zero_size_frame_is_noop() {
        let mut app = Red { frames: 0 };
        let c = render_offscreen(&mut app, Size::new(0, 0), 1.0, vec![vec![]], &test_fonts());
        assert!(c.pixels.is_empty());
    }
}
