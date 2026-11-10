// No console window on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::Path;

use skiff_core::askpass;
use skiff_core::config::Config;
use skiff_gui::app::GuiApp;
use skiff_ui::shell::{WindowOptions, run_with_waker};
use skiff_ui::text::Fonts;

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
    let migrated = skiff_core::migrate::migrate_legacy(&cfg_path);
    let cfg = Config::load_or_create(&cfg_path)?;
    let ui_font = cfg.gui.ui_font_path.as_deref().map(Path::new);
    let mono = cfg.gui.font_path.as_deref().map(Path::new);
    let fonts = Fonts::load(ui_font, mono).or_else(|_| Fonts::load(None, None))?;
    let size = skiff_core::state::UiState::load(&skiff_core::state::UiState::path_for(&cfg_path)).window_size.unwrap_or((1280, 800));
    run_with_waker(
        move |waker| {
            let mut app = GuiApp::new(cfg, cfg_path, Some(waker));
            app.open = open;
            if let Some(note) = migrated {
                app.warn(note);
            }
            app
        },
        WindowOptions { title: "Skiff".into(), app_id: "skiff".into(), size, fonts },
    )
}
