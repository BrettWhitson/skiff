mod app;
mod forms;
mod input;
mod sftp_tab;
mod tui_theme;
mod ui;

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture};
use crossterm::execute;

use crate::app::App;
use termax_core::cli::{self, Cli};
use termax_core::config::Config;
use termax_core::history::{self, ConnKind, History};
use termax_core::hosts;

fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // `--plain` (anywhere) switches the TUI to plain mode for this run.
    let plain = args.iter().any(|a| a == "--plain");
    args.retain(|a| a != "--plain");
    let cmd = match cli::parse(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("termax: {e}");
            std::process::exit(2);
        }
    };
    let cfg_path = Config::default_path();
    let cfg = match cmd {
        Cli::Help => {
            print!("{}", cli::HELP);
            return Ok(());
        }
        Cli::Version => {
            println!("termax {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Cli::ConfigPath => {
            println!("{}", cfg_path.display());
            return Ok(());
        }
        Cli::Completion(s) => {
            print!("{}", cli::completion_script(s));
            return Ok(());
        }
        _ => Config::load_or_create(&cfg_path)?,
    };
    let (list, warning) = hosts::collect(&cfg);
    if let Some(w) = &warning {
        eprintln!("termax: {w}");
    }
    let found = |name: &str| match cli::resolve(&list, name) {
        Ok(h) => h.clone(),
        Err(e) => {
            eprintln!("termax: {e}");
            std::process::exit(2);
        }
    };
    let open = match cmd {
        Cli::List { json, all } => {
            print!("{}", if json { cli::list_json(&list, all) } else { cli::list_text(&list, all) });
            return Ok(());
        }
        Cli::Info { host, json } => {
            let h = found(&host);
            print!("{}", if json { cli::info_json(&h) } else { cli::info_text(&h) });
            return Ok(());
        }
        Cli::Exec { host, cmd, tty } => {
            let h = found(&host);
            let mut args = h.shell_args();
            if tty {
                args.insert(0, "-t".into());
            }
            args.extend(cmd);
            let status = std::process::Command::new(&cfg.settings.ssh_binary)
                .args(&args)
                .status()
                .map_err(|e| anyhow::anyhow!("starting `{}`: {e}", cfg.settings.ssh_binary))?;
            let code = status.code().unwrap_or(255);
            // 255 is ssh's own failure (could not connect or log in).
            if code != 255 {
                let path = History::path_for(&cfg_path);
                let _ = History::load(&path).update(&path, |hist| hist.record(&h.name, ConnKind::Command, history::now()));
            }
            std::process::exit(code);
        }
        Cli::Tui { open } => open.map(|(name, files)| (found(&name).name, files)),
        _ => None,
    };

    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().worker_threads(2).build()?;
    let mut app = App::new(cfg, cfg_path, rt.handle().clone());
    app.plain |= plain;
    if let Some((name, files)) = open {
        app.open_named(&name, files);
    }

    // ratatui::init installs a panic hook that restores the terminal.
    let mut terminal = ratatui::init();
    // ratatui's hook restores raw mode and the screen; also give the mouse
    // and paste modes back if termax panics.
    let restore = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(std::io::stdout(), DisableMouseCapture, DisableBracketedPaste);
        restore(info);
    }));
    let _ = execute!(std::io::stdout(), EnableBracketedPaste);
    let result = run(&mut terminal, &mut app);
    let _ = execute!(std::io::stdout(), DisableBracketedPaste);
    ratatui::restore();

    // Close sessions (kills child ssh processes) before the runtime goes away.
    drop(app);
    rt.shutdown_timeout(Duration::from_millis(500));
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    let mut mouse = false;
    let result = (|| loop {
        terminal.draw(|f| ui::draw(f, app))?;
        // Capture the mouse only while a program in the active terminal asked
        // for it, so the user's terminal keeps its own selection otherwise.
        let want = app.wants_mouse();
        if want != mouse {
            mouse = want;
            let _ = if want { execute!(std::io::stdout(), EnableMouseCapture) } else { execute!(std::io::stdout(), DisableMouseCapture) };
        }
        if app.should_quit {
            return Ok(());
        }
        if event::poll(Duration::from_millis(30))? {
            // Handle everything already queued before redrawing (fast typing / pastes).
            let mut batch = vec![event::read()?];
            while event::poll(Duration::ZERO)? {
                batch.push(event::read()?);
            }
            app.handle_batch(batch);
        }
        app.drain_messages();
    })();
    if mouse {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
    }
    result
}
