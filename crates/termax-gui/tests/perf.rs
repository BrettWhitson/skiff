//! Performance checks. The timing tests are `#[ignore]`d; run them in
//! release: `cargo test -p termax-gui --release --test perf -- --ignored --nocapture`.
//! Nothing here connects anywhere: terminals run a local batch file / shell
//! in a pty, and the config disables `~/.ssh/config`.

use std::path::Path;
use std::time::{Duration, Instant};

use termax_core::config::{Config, Connection};
use termax_core::hosts::collect;
use termax_gui::app::{GuiApp, Tab, TabKind};
use termax_gui::term::TermTab;
use termax_ui::input::{Event, Key, Modifiers};
use termax_ui::text::Fonts;
use termax_ui::ui::testing::Harness;

const CONFIG: &str = "[settings]\nload_ssh_config = false\ntheme = \"midnight\"\nssh_binary = \"termax-test-no-such-ssh\"\n";

fn app(dir: &Path) -> GuiApp {
    let path = dir.join("config.toml");
    std::fs::write(&path, CONFIG).unwrap();
    GuiApp::new(Config::load_or_create(&path).unwrap(), path, None)
}

/// A terminal tab running `program args` locally.
fn local_terminal(app: &mut GuiApp, program: &str, args: Vec<String>) {
    let mut cfg = Config::default();
    cfg.settings.load_ssh_config = false;
    cfg.connections.push(Connection { name: "local".into(), host: "127.0.0.1".into(), ..Default::default() });
    let host = collect(&cfg).0.remove(0);
    let tab = TermTab::with_command(host, program.into(), args, 1000);
    app.tabs.push(Tab { id: 9000, kind: TabKind::Terminal(Box::new(tab)) });
    app.active = app.tabs.len() - 1;
}

fn screen_text(app: &GuiApp) -> String {
    match &app.tabs[app.active].kind {
        TabKind::Terminal(t) => t.session.as_ref().map(|s| s.parser.lock().unwrap().screen().contents()).unwrap_or_default(),
        _ => String::new(),
    }
}

/// Mixed output: SGR colours, wide characters, long and short lines.
fn mixed_output(bytes: usize) -> String {
    let mut s = String::with_capacity(bytes + 200);
    let mut i = 0u32;
    while s.len() < bytes {
        let c = 31 + i % 7;
        s.push_str(&format!("\x1b[{c}m{i:>8}\x1b[0m \x1b[1mbold\x1b[0m plain text \u{4e2d}\u{6587} \x1b[4munder\x1b[0m"));
        if i.is_multiple_of(5) {
            s.push_str(&"-".repeat(150));
        }
        s.push_str("\r\n");
        i += 1;
    }
    s
}

fn real_fonts() -> Fonts {
    Fonts::load(None, None).expect("system fonts")
}

#[test]
fn idle_requests_no_redraw() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = app(dir.path());
    let mut h = Harness::new(1280, 800, 1.0);
    h.frame(vec![], &mut |ui| a.ui(ui));
    h.frame(vec![Event::FocusLost], &mut |ui| a.ui(ui));
    h.frame(vec![], &mut |ui| a.ui(ui));
    assert_eq!(h.repaint_in, None, "unfocused and idle: no timer");
}

#[test]
#[ignore]
fn first_frame_under_300ms() {
    let dir = tempfile::tempdir().unwrap();
    let start = Instant::now();
    let fonts = real_fonts();
    let t_fonts = start.elapsed();
    let mut a = app(dir.path());
    let t_app = start.elapsed();
    let mut h = Harness::with_fonts(1280, 800, 1.0, fonts);
    h.frame(vec![], &mut |ui| a.ui(ui));
    let _ = h.render();
    let took = start.elapsed();
    println!("first frame: {took:?} (fonts {t_fonts:?}, app {:?})", t_app - t_fonts);
    assert!(took < Duration::from_millis(300), "{took:?}");
}

#[cfg(windows)]
fn printer(dir: &Path, text: &str, then_read: bool) -> (String, Vec<String>) {
    std::fs::write(dir.join("out.txt"), text).unwrap();
    let tail = if then_read { "set /p X=\r\necho GOT:%X%\r\n" } else { "pause >nul\r\n" };
    let bat = dir.join("print.cmd");
    std::fs::write(&bat, format!("@echo off\r\ntype \"%~dp0out.txt\"\r\n{tail}")).unwrap();
    ("cmd.exe".into(), vec!["/c".into(), bat.display().to_string()])
}

#[cfg(unix)]
fn printer(dir: &Path, text: &str, then_read: bool) -> (String, Vec<String>) {
    std::fs::write(dir.join("out.txt"), text).unwrap();
    let tail = if then_read { "read X; echo GOT:$X" } else { "sleep 600" };
    ("sh".into(), vec!["-c".into(), format!("cat '{}'; {tail}", dir.join("out.txt").display())])
}

#[test]
#[ignore]
fn full_terminal_frame_4k_under_16ms() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = app(dir.path());
    let (prog, args) = printer(dir.path(), &mixed_output(200_000), false);
    local_terminal(&mut a, &prog, args);
    let mut h = Harness::with_fonts(3840, 2160, 1.0, real_fonts());
    let start = Instant::now();
    // Filled: at least 100 rows, unchanged for half a second.
    let (mut last, mut since) = (String::new(), Instant::now());
    loop {
        assert!(start.elapsed() < Duration::from_secs(60), "screen never filled");
        h.frame(vec![], &mut |ui| a.ui(ui));
        let now = screen_text(&a);
        if now != last {
            (last, since) = (now, Instant::now());
        } else if last.lines().count() >= 100 && since.elapsed() > Duration::from_millis(500) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    h.frame(vec![], &mut |ui| a.ui(ui));
    // The shell's path: tile renderer, everything dirty (as when every
    // row of a full screen scrolls).
    let mut canvas = termax_ui::canvas::Canvas::new(3840, 2160);
    let mut tiles = termax_ui::display::TileRenderer::default();
    let mut cache = termax_ui::text::cache::GlyphCache::default();
    let bg = h.style.palette.bg;
    let mut best = Duration::MAX;
    for _ in 0..5 {
        tiles.invalidate();
        let t = Instant::now();
        h.frame(vec![], &mut |ui| a.ui(ui));
        let built = t.elapsed();
        tiles.render(h.list(), &mut canvas, &h.fonts, &mut cache, bg);
        best = best.min(t.elapsed());
        println!("build {built:?}, build + paint {:?}", t.elapsed());
        let t2 = Instant::now();
        tiles.render(h.list(), &mut canvas, &h.fonts, &mut cache, bg);
        println!("  hash only (nothing dirty): {:?}", t2.elapsed());
    }
    println!("4k full repaint: {best:?}");
    assert!(best < Duration::from_millis(16), "{best:?}");
}

#[test]
#[ignore]
fn flood_output_frame_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = app(dir.path());
    let (prog, args) = printer(dir.path(), &mixed_output(10 << 20), true);
    local_terminal(&mut a, &prog, args);
    let mut h = Harness::with_fonts(1280, 800, 1.0, real_fonts());
    h.frame(vec![], &mut |ui| a.ui(ui)); // spawns the session
    let start = Instant::now();
    let mut worst = Duration::ZERO;
    let mut frames = 0u32;
    let mut typed = false;
    while !screen_text(&a).contains("GOT:ok") {
        assert!(start.elapsed() < Duration::from_secs(300), "flood never finished; worst frame {worst:?}");
        let events = if !typed && start.elapsed() > Duration::from_millis(500) {
            typed = true;
            vec![
                Event::KeyDown { key: Key::Char('o'), mods: Modifiers::NONE, text: Some("o".into()) },
                Event::Text("o".into()),
                Event::KeyDown { key: Key::Char('k'), mods: Modifiers::NONE, text: Some("k".into()) },
                Event::Text("k".into()),
                Event::KeyDown { key: Key::Enter, mods: Modifiers::NONE, text: None },
            ]
        } else {
            vec![]
        };
        let t = Instant::now();
        h.frame(events, &mut |ui| a.ui(ui));
        let _ = h.render();
        let took = t.elapsed();
        worst = worst.max(took);
        frames += 1;
        std::thread::sleep(Duration::from_millis(16u64.saturating_sub(took.as_millis() as u64)));
    }
    println!("flood: {frames} frames in {:?}, worst frame {worst:?}", start.elapsed());
    assert!(worst < Duration::from_millis(16), "{worst:?}");
}
