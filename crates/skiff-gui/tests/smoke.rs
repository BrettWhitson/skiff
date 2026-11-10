//! Launch-level checks. No test here connects anywhere: the config disables
//! `~/.ssh/config` and the one connection points at a closed local port.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use skiff_gui::app::GuiApp;
use skiff_ui::display::Prim;
use skiff_ui::ui::testing::Harness;

/// A temp config with one connection to a closed local port; keep the
/// returned dir alive for the test.
fn temp_config() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[settings]\nload_ssh_config = false\ntheme = \"midnight\"\n\n[[connection]]\nname = \"smoke-host\"\nhost = \"127.0.0.1\"\nport = 1\n",
    )
    .unwrap();
    (dir, path)
}

/// The app draws its first frame offscreen: the host is in the sidebar and
/// the theme's background is painted.
#[test]
fn renders_first_frame_offscreen() {
    let (_dir, path) = temp_config();
    let cfg = skiff_core::config::Config::load_or_create(&path).unwrap();
    let mut app = GuiApp::new(cfg, path, None);
    let mut h = Harness::new(1280, 800, 1.0);
    h.frame(vec![], &mut |ui| app.ui(ui));
    let sidebar_text =
        h.list().prims.iter().any(|p| matches!(p, Prim::Text { text, origin, .. } if text.contains("smoke-host") && origin.x < 400));
    assert!(sidebar_text, "connection listed in the sidebar");
    let c = h.render();
    assert_eq!(c.pixel(640, 400) & 0xffffff, 0x0f1117, "midnight background");
}

/// Started with the askpass environment set, the binary acts as the helper:
/// it fails fast on an unreachable broker instead of opening a window.
#[test]
fn askpass_helper_mode_exits_without_window() {
    let start = Instant::now();
    let mut child = Command::new(env!("CARGO_BIN_EXE_skiff-gui"))
        .env("SKIFF_ASKPASS_ENDPOINT", "skiff-no-such-endpoint-for-test")
        .env("SKIFF_ASKPASS_TAG", "t")
        .env("SKIFF_ASKPASS_TOKEN", "x")
        .arg("Password: ")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(!status.success());
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(5), "helper did not exit");
        std::thread::sleep(Duration::from_millis(50));
    }
}
