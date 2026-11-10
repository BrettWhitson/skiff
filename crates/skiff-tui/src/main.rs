//! `skiff`: the terminal (ratatui) front end of Skiff, an SSH and SFTP client.
//! The package is `skiff-ssh`; the binary is `skiff`.
//!
//! One executable plays three roles:
//! - ssh's `SSH_ASKPASS` helper: when ssh runs it to ask for a password or a
//!   host-key decision, it relays the question to the running skiff and exits.
//! - A command-line tool: `list`, `info`, `completion`, `--config-path` and
//!   `skiff <host> <cmd…>` (runs the command over the system `ssh` and exits
//!   with its status) never start the UI.
//! - The full-screen TUI: a host list plus tabs with terminal sessions (in
//!   optional splits) and dual-pane SFTP browsers, port forwards, snippets and
//!   modal dialogs.
//!
//! Modules:
//! - `app`: all UI state and behavior: tabs, modals, the event and key
//!   handling, and the background `Msg`s (connections, listings, transfers,
//!   askpass questions) drained on each loop turn.
//! - `ui`: draws the app each frame (tab bar, host list, terminals, SFTP
//!   panes and transfers, status line, modals).
//! - `keys`: the global shortcut actions, their default chords and the
//!   `[tui_keys]` config section that remaps them.
//! - `forms`: the host editor and port-forward dialogs.
//! - `sftp_tab`: the SFTP browser tab's state and its background work.
//! - `input`: the single-line text field used by filters, prompts and forms.
//! - `tui_theme`: config themes resolved to ratatui colors and styles.
//!
//! Config, ssh_config parsing and editing, the terminal emulator, SFTP and
//! transfers, askpass, history and forwards all live in `skiff-core`; this
//! crate is the terminal UI over them. `skiff-gui` is the graphical
//! counterpart built on the same core.

mod app;
mod forms;
mod input;
mod keys;
mod sftp_tab;
mod tui_theme;
mod ui;

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture};
use crossterm::execute;

use crate::app::App;
use skiff_core::cli::{self, Cli};
use skiff_core::config::Config;
use skiff_core::history::{self, ConnKind, History};
use skiff_core::hosts;

/// Entry point: askpass helper mode, then CLI parsing; commands that print
/// or run something exit early, otherwise the TUI starts (optionally opening
/// a host given on the command line) and runs until quit.
fn main() -> Result<()> {
    // ssh runs this executable as SSH_ASKPASS (SFTP password and host-key
    // questions): relay the question to the running skiff and exit.
    if skiff_core::askpass::is_helper_invocation() {
        std::process::exit(skiff_core::askpass::helper_main());
    }
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // `--plain` (anywhere) switches the TUI to plain mode for this run.
    let plain = args.iter().any(|a| a == "--plain");
    args.retain(|a| a != "--plain");
    let cmd = match cli::parse(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skiff: {e}");
            std::process::exit(2);
        }
    };
    let cfg_path = Config::default_path();
    // Commands that need no config return here; the rest load (and, the
    // first time, migrate or create) the config.
    let (cfg, migration) = match cmd {
        Cli::Help => {
            print!("{}", cli::HELP);
            return Ok(());
        }
        Cli::Version => {
            println!("skiff {}", env!("CARGO_PKG_VERSION"));
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
        _ => {
            let (cfg, m) = skiff_core::migrate::load_config(&cfg_path)?;
            if let Some(note) = m.note() {
                eprintln!("skiff: {note}");
            }
            (cfg, m)
        }
    };
    let (list, warning) = hosts::collect(&cfg);
    if let Some(w) = &warning {
        eprintln!("skiff: {w}");
    }
    // Looks a host up by name; an unknown name is a usage error (exit 2).
    let found = |name: &str| match cli::resolve(&list, name) {
        Ok(h) => h.clone(),
        Err(e) => {
            eprintln!("skiff: {e}");
            std::process::exit(2);
        }
    };
    // Host commands; only `Cli::Tui` falls through, with an optional host
    // (and whether to open it as files) to open on start.
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
    if let Ok(exe) = std::env::current_exe() {
        app.enable_askpass(exe);
    }
    // The migration note (also printed above) is the TUI's first status line.
    app.show_migration(&migration);
    if let Some((name, files)) = open {
        app.open_named(&name, files);
    }

    // ratatui::init installs a panic hook that restores the terminal.
    let mut terminal = ratatui::init();
    // ratatui's hook restores raw mode and the screen; also give the mouse
    // and paste modes back if skiff panics.
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

/// The UI loop: draw, sync mouse capture, wait up to 30 ms for input, handle
/// it, flush any clipboard copy, then apply background messages. Turns mouse
/// capture back off on the way out.
fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    let mut mouse = false;
    let result = (|| loop {
        terminal.draw(|f| ui::draw(f, app))?;
        // With `tui_mouse = "auto"`, capture the mouse only while a program in
        // the active terminal asks for it, so the user's terminal keeps its
        // own selection otherwise.
        let want = app.capture_mouse();
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
            if let Some(text) = app.clipboard_out.take() {
                // The user's terminal puts it on the clipboard (OSC 52).
                use std::io::Write;
                let mut out = std::io::stdout();
                let _ = out.write_all(skiff_core::termx::osc52(&text).as_bytes());
                let _ = out.flush();
            }
        }
        app.drain_messages();
    })();
    if mouse {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
    }
    result
}
