// No console window on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};

use termax_core::askpass;
use termax_core::config::Config;
use termax_gui::app::GuiApp;
use termax_ui::shell::{WindowOptions, run_with_waker};
use termax_ui::text::Fonts;

fn main() -> anyhow::Result<()> {
    // ssh runs this executable as SSH_ASKPASS: answer and exit, no window.
    if askpass::is_helper_invocation() {
        std::process::exit(askpass::helper_main());
    }
    let cfg_path = std::env::var_os("TERMAX_CONFIG").map(PathBuf::from).unwrap_or_else(Config::default_path);
    let args: Vec<String> = std::env::args().skip(1).collect();
    let open = match termax_core::cli::parse_gui(&args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("termax-gui: {e}");
            std::process::exit(2);
        }
    };
    let cfg = Config::load_or_create(&cfg_path)?;
    let ui_font = cfg.gui.ui_font_path.as_deref().map(Path::new);
    let mono = cfg.gui.font_path.as_deref().map(Path::new);
    let fonts = Fonts::load(ui_font, mono).or_else(|_| Fonts::load(None, None))?;
    let size = termax_core::state::UiState::load(&termax_core::state::UiState::path_for(&cfg_path))
        .window_size
        .unwrap_or((1280, 800));
    run_with_waker(
        move |waker| {
            let mut app = GuiApp::new(cfg, cfg_path, Some(waker));
            app.open = open;
            app
        },
        WindowOptions { title: "termax".into(), size, fonts },
    )
}
