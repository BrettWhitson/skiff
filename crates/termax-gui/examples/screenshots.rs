//! Renders screenshots of the GUI offscreen (no window) for docs.
//!
//! `cargo build -p termax-gui && cargo run -p termax-gui --example screenshots -- OUT_DIR`
//!
//! Connects only to throwaway `termax-testserver` fixtures on 127.0.0.1 with
//! a scratch config (`~/.ssh/config` is not read). The terminal tab runs a
//! local shell, because the test server's shell only echoes input.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use termax_core::config::{Config, Connection};
use termax_gui::app::{GuiApp, Tab, TabKind, start_broker};
use termax_gui::files::{Location, PaneState};
use termax_gui::term::TermTab;
use termax_testserver::{FixtureAuth, TestFixture};
use termax_ui::input::{Event, Key, Modifiers};
use termax_ui::text::Fonts;
use termax_ui::ui::testing::Harness;

const W: i32 = 1440;
const H: i32 = 900;

struct Shots {
    app: GuiApp,
    h: Harness,
    out: PathBuf,
}

impl Shots {
    fn frame(&mut self, events: Vec<Event>) {
        let app = &mut self.app;
        self.h.frame(events, &mut |ui| app.ui(ui));
    }

    fn until(&mut self, what: &str, cond: impl Fn(&GuiApp) -> bool) {
        let start = Instant::now();
        while !cond(&self.app) {
            assert!(start.elapsed() < Duration::from_secs(30), "timed out waiting for {what}");
            self.frame(vec![]);
            std::thread::sleep(Duration::from_millis(30));
        }
        self.settle();
    }

    /// A few frames so late changes show.
    fn settle(&mut self) {
        for _ in 0..4 {
            self.frame(vec![]);
            std::thread::sleep(Duration::from_millis(30));
        }
    }

    fn typed(&mut self, s: &str) {
        self.frame(vec![Event::Text(s.into())]);
    }

    fn key(&mut self, key: Key) {
        self.frame(vec![Event::KeyDown { key, mods: Modifiers::NONE, text: None }]);
    }

    fn save(&mut self, name: &str) {
        self.frame(vec![]);
        let c = self.h.render();
        let mut ppm = format!("P6 {} {} 255\n", c.width, c.height).into_bytes();
        for p in &c.pixels {
            ppm.extend([(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
        }
        let path = self.out.join(format!("{name}.ppm"));
        std::fs::write(&path, ppm).unwrap();
        println!("wrote {}", path.display());
    }

    fn files_tab(&self) -> Option<&termax_gui::files::FilesTab> {
        match &self.app.tabs.get(self.app.active)?.kind {
            TabKind::Files(f) => Some(f),
            _ => None,
        }
    }
}

fn conn(name: &str, fx: &TestFixture, tags: &[&str]) -> Connection {
    Connection {
        name: name.into(),
        host: fx.destination(),
        extra_args: fx.ssh_options(),
        tags: tags.iter().map(|t| t.to_string()).collect(),
        ..Default::default()
    }
}

fn write_files(dir: &Path, files: &[(&str, usize)], dirs: &[&str]) {
    for d in dirs {
        std::fs::create_dir_all(dir.join(d)).unwrap();
    }
    for (f, size) in files {
        std::fs::write(dir.join(f), vec![b'x'; *size]).unwrap();
    }
}

fn main() {
    let out = PathBuf::from(std::env::args().nth(1).expect("usage: screenshots OUT_DIR"));
    std::fs::create_dir_all(&out).unwrap();
    let helper = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().join(if cfg!(windows) {
        "termax-gui.exe"
    } else {
        "termax-gui"
    });
    assert!(helper.exists(), "build the binary first: cargo build -p termax-gui ({})", helper.display());

    // Throwaway servers on 127.0.0.1.
    let files_fx = TestFixture::new(FixtureAuth::Key).unwrap();
    write_files(
        files_fx.root(),
        &[("app-1.4.2.tar.gz", 18_400_000), ("config.yaml", 2_300), ("deploy.sh", 1_100), ("CHANGELOG.md", 9_800)],
        &["releases", "logs", "backups"],
    );
    let pw_fx = TestFixture::new(FixtureAuth::Password("demo-password".into())).unwrap();

    let local = tempfile::tempdir().unwrap();
    write_files(
        local.path(),
        &[("report-q3.pdf", 1_520_000), ("notes.md", 4_200), ("build.log", 88_000), ("screenshot.png", 640_000)],
        &["Documents", "Projects", "Downloads"],
    );

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut cfg = Config::load_or_create(&path).unwrap();
    let baseline = cfg.clone();
    cfg.settings.load_ssh_config = false;
    cfg.settings.theme = "midnight".into();
    cfg.settings.local_start_dir = Some(local.path().display().to_string());
    cfg.connections = vec![
        conn("file-server", &files_fx, &["prod", "storage"]),
        conn("staging-api", &pw_fx, &["staging"]),
        Connection { name: "local-shell".into(), host: "localhost".into(), tags: vec!["demo".into()], ..Default::default() },
    ];
    cfg.save(&baseline, &path).unwrap();
    let cfg = Config::load_or_create(&path).unwrap();

    let mut app = GuiApp::new(cfg, path, None);
    app.askpass = Some(start_broker(&app.engine, helper).unwrap());
    let fonts = Fonts::load(None, None).expect("system fonts");
    let mut s = Shots { app, h: Harness::with_fonts(W, H, 1.0, fonts), out };
    s.frame(vec![]);
    let host = |s: &Shots, name: &str| s.app.hosts.iter().find(|h| h.name == name).unwrap().clone();

    // 1. Terminal tab (a local shell stands in for ssh).
    let lh = host(&s, "local-shell");
    let (prog, args) = if cfg!(windows) { ("cmd.exe", vec!["/k".into(), "prompt $P$G".into()]) } else { ("sh", vec![]) };
    let mut t = TermTab::with_command(lh, prog.into(), args, 1000);
    t.font_px = 0.0;
    s.app.tabs.push(Tab { id: 900, kind: TabKind::Terminal(Box::new(t)) });
    s.app.active = s.app.tabs.len() - 1;
    s.settle();
    std::thread::sleep(Duration::from_millis(800));
    s.settle();
    let cd = format!("cd /d {}", local.path().display());
    let cmds: Vec<&str> = if cfg!(windows) { vec![&cd, "ver", "dir", "echo Hello from termax"] } else { vec!["uname -a", "ls -la"] };
    for c in cmds {
        s.typed(c);
        s.key(Key::Enter);
        std::thread::sleep(Duration::from_millis(400));
        s.settle();
    }
    std::thread::sleep(Duration::from_millis(500));
    s.settle();
    s.save("1-terminal");

    // 2. File transfer: Local <-> file-server, one copy finished.
    let fs = host(&s, "file-server");
    s.app.open_files(Location::Local, Location::Host(fs));
    s.until("both panes listed", |a| match &a.tabs[a.active].kind {
        TabKind::Files(f) => f.panes.iter().all(|p| p.ep.is_some() && p.state == PaneState::Idle && !p.cwd.is_empty()),
        _ => false,
    });
    let report = local.path().join("report-q3.pdf").display().to_string();
    let notes = local.path().join("notes.md").display().to_string();
    {
        let id = s.app.tabs[s.app.active].id;
        let names = [("report-q3.pdf", report), ("notes.md", notes)];
        let ctx = termax_gui::files::FilesCtx {
            tab_id: id,
            engine: &s.app.engine,
            start_dir: &s.app.start_dir,
            hosts: &s.app.hosts,
            askpass: s.app.askpass.as_ref(),
            ssh_binary: &s.app.cfg.settings.ssh_binary,
            ssh_version: None,
        };
        let a = s.app.active;
        if let TabKind::Files(f) = &mut s.app.tabs[a].kind {
            let items = names.into_iter().map(|(n, p)| (p, false, n.to_string())).collect();
            f.copy(0, items, &ctx);
            f.panes[0].sel.set = [3usize].into_iter().collect();
        }
    }
    s.until("copies done", |a| match &a.tabs[a.active].kind {
        TabKind::Files(f) => !f.transfers.rows.is_empty() && f.transfers.running() == 0,
        _ => false,
    });
    std::thread::sleep(Duration::from_millis(300));
    s.settle();
    s.save("2-file-transfer");

    // 3. Quick switcher over it.
    s.app.switcher_open = true;
    s.settle();
    s.typed("st");
    s.settle();
    s.save("3-quick-switcher");
    s.key(Key::Escape);
    s.settle();

    // 4. Inline password prompt from ssh (askpass relay).
    let pw = host(&s, "staging-api");
    s.app.open_files(Location::Local, Location::Host(pw));
    s.until("password prompt", |a| match &a.tabs[a.active].kind {
        TabKind::Files(f) => f.panes[1].prompt.is_some(),
        _ => false,
    });
    s.typed("demo-pa");
    s.settle();
    assert!(s.files_tab().is_some());
    s.save("4-password-prompt");

    // 5. Settings: editing a connection.
    s.app.edit_target = Some("file-server".into());
    s.app.open_settings();
    s.settle();
    s.save("5-settings");

    // 6. Forwards tab, with status dots in the sidebar (fixtures only).
    for name in ["file-server", "staging-api"] {
        s.app.check_status(Some(name));
    }
    let listen = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let fwd = termax_core::forward::Forward {
        name: Some("admin panel".into()),
        host: "file-server".into(),
        kind: termax_core::forward::ForwardKind::Local,
        bind: None,
        listen,
        target: Some(format!("127.0.0.1:{}", files_fx.port())),
    };
    s.app.cfg.forwards.push(fwd.clone());
    let mut st = std::mem::take(&mut s.app.forwards);
    s.app.start_forward(&mut st, &fwd);
    s.app.forwards = st;
    s.app.open_forwards();
    s.until("forward running and statuses in", |a| {
        a.forwards.state_of(&fwd) == Some(termax_core::forward::ForwardState::Running)
            && ["file-server", "staging-api"].iter().all(|n| matches!(a.statuses.get(*n), Some(termax_core::status::Status::Online { .. })))
    });
    s.save("6-forwards");
    s.app.forwards.stop_all();

    // Close everything (kills the ssh children) before the fixtures stop.
    for id in s.app.tabs.iter().map(|t| t.id).collect::<Vec<_>>() {
        s.app.close_tab(id);
    }
}
