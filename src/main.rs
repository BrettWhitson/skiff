mod app;
mod config;
mod hosts;
mod input;
mod sftp;
mod sshconfig;
mod terminal;
mod theme;
mod ui;

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;

use crate::app::App;
use crate::config::Config;

fn main() -> Result<()> {
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("termax {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let cfg_path = Config::default_path();
    if std::env::args().any(|a| a == "--config-path") {
        println!("{}", cfg_path.display());
        return Ok(());
    }
    let cfg = Config::load_or_create(&cfg_path)?;

    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().worker_threads(2).build()?;
    let mut app = App::new(cfg, cfg_path, rt.handle().clone());

    // ratatui::init installs a panic hook that restores the terminal.
    let mut terminal = ratatui::init();
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
    loop {
        terminal.draw(|f| ui::draw(f, app))?;
        if app.should_quit {
            return Ok(());
        }
        if event::poll(Duration::from_millis(30))? {
            // Handle everything already queued before redrawing (fast typing / pastes).
            loop {
                app.handle_event(event::read()?);
                if app.should_quit || !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
        app.drain_messages();
    }
}
