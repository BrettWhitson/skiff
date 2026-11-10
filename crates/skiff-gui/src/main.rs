//! `skiff-gui`: the desktop GUI binary.
//!
//! Usage: `skiff-gui [<host>] [--files <host>]` opens the window, optionally
//! with a terminal or file-transfer tab for `<host>`. The same executable
//! doubles as the `SSH_ASKPASS` helper that ssh launches for password and
//! host-key prompts. All app logic lives in the `skiff_gui` library; this
//! file only loads config, state and fonts and starts the `skiff-ui` loop.

// No console window on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::Path;

use skiff_core::askpass;
use skiff_core::config::Config;
use skiff_gui::app::GuiApp;
use skiff_ui::shell::{WindowOptions, run_with_waker};
use skiff_ui::text::Fonts;

/// Entry point: runs as the askpass helper if ssh launched us, otherwise
/// parses the command line, loads config and fonts and runs the window.
fn main() -> anyhow::Result<()> {
    // ssh runs this executable as SSH_ASKPASS: answer and exit, no window.
    if askpass::is_helper_invocation() {
        std::process::exit(askpass::helper_main());
    }
    let cfg_path = Config::default_path();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let open = match skiff_core::cli::parse_gui(&args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("skiff-gui: {e}");
            std::process::exit(2);
        }
    };
    // One-time copy of the old (termax) config dir; its note shows in the app.
    // A failed copy loads defaults without writing a config, so the next start retries.
    let (cfg, migration) = skiff_core::migrate::load_config(&cfg_path)?;
    let ui_font = cfg.gui.ui_font_path.as_deref().map(Path::new);
    let mono = cfg.gui.font_path.as_deref().map(Path::new);
    // A configured font file that fails to load falls back to system fonts.
    let fonts = Fonts::load(ui_font, mono).or_else(|_| Fonts::load(None, None))?;
    // Reopen at the last saved window size.
    let size = skiff_core::state::UiState::load(&skiff_core::state::UiState::path_for(&cfg_path)).window_size.unwrap_or((1280, 800));
    run_with_waker(
        move |waker| {
            let mut app = GuiApp::new(cfg, cfg_path, Some(waker));
            app.open = open;
            if let Some(note) = migration.note() {
                app.notify(skiff_ui::widgets::ToastKind::Info, note.to_string());
            }
            app
        },
        WindowOptions { title: "Skiff".into(), app_id: "skiff".into(), size, fonts },
    )
}
