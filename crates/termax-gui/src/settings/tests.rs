//! Settings screen tests (temp config files only, no network).

use termax_ui::geom::Point;
use termax_ui::input::{Event, Key, Modifiers};

use crate::app::tests::{CONFIG, Fixture};
use crate::keys::Action;

const CTRL: Modifiers = Modifiers { ctrl: true, shift: false, alt: false, logo: false };
const CTRL_SHIFT: Modifiers = Modifiers { ctrl: true, shift: true, alt: false, logo: false };

fn settings(section: &str) -> Fixture {
    let mut f = Fixture::new();
    f.app.do_action(Action::Settings);
    f.frame(vec![]);
    f.click_text(section);
    f
}

/// Replaces the text of the input labelled `label`.
fn set_field(f: &mut Fixture, label: &str, value: &str) {
    let l = f.find(label);
    f.click(Point::new(l.x + super::LABEL_W + 20, l.y));
    f.frame(vec![Event::KeyDown { key: Key::Char('a'), mods: CTRL, text: None }]);
    f.frame(vec![Event::Text(value.into())]);
    f.frame(vec![]);
}

fn file(f: &Fixture) -> String {
    std::fs::read_to_string(&f.path).unwrap()
}

#[test]
fn settings_save_preserves_comments() {
    let mut f = Fixture::new();
    f.right_click_text("alpha");
    f.click_text("Edit");
    assert!(f.texts().iter().any(|t| t == "Connections"), "settings open: {:?}", f.texts());
    set_field(&mut f, "Port", "2222");
    f.click_text("Save");
    let text = file(&f);
    assert!(text.contains("# my hand-written comment"), "{text}");
    assert!(text.contains("port = 2222"), "{text}");
    assert_eq!(f.app.cfg.connections[0].port, Some(2222));
}

#[test]
fn add_connection_appears_in_sidebar() {
    let mut f = settings("Connections");
    f.click_text("Add");
    set_field(&mut f, "Name", "gamma");
    set_field(&mut f, "Host", "10.9.9.9");
    f.click_text("Save");
    assert!(f.app.hosts.iter().any(|h| h.name == "gamma"));
    assert!(file(&f).contains("gamma"));
    f.frame(vec![]);
    assert!(f.texts().iter().filter(|t| *t == "gamma").count() >= 1);
}

#[test]
fn duplicate_name_rejected() {
    let mut f = settings("Connections");
    f.click_text("Add");
    set_field(&mut f, "Name", "alpha");
    set_field(&mut f, "Host", "10.9.9.9");
    f.click_text("Save");
    assert!(f.texts().iter().any(|t| t.contains("already exists")), "{:?}", f.texts());
    assert_eq!(f.app.cfg.connections.len(), 2);
    assert_eq!(file(&f), CONFIG);
}

#[test]
fn port_out_of_range_rejected() {
    let mut f = settings("Connections");
    f.click_text("Add");
    set_field(&mut f, "Name", "zero");
    set_field(&mut f, "Host", "10.9.9.9");
    set_field(&mut f, "Port", "0");
    f.click_text("Save");
    assert!(f.texts().iter().any(|t| t.contains("Invalid port")), "{:?}", f.texts());
    assert_eq!(f.app.cfg.connections.len(), 2);
}

#[test]
fn font_path_invalid_keeps_old_fonts() {
    let mut f = settings("Appearance");
    set_field(&mut f, "Terminal font file", "C:/termax-no-such-dir/missing.ttf");
    f.click_text("Apply fonts");
    assert!(f.texts().iter().any(|t| t.contains("could not load")), "{:?}", f.texts());
    assert!(f.app.pending_fonts.is_none());
    assert_eq!(f.app.cfg.gui.font_path, None);
    assert!(!file(&f).contains("missing.ttf"));
}

#[test]
fn rebind_action_saves_keys_table() {
    let mut f = settings("Keybindings");
    let row = f.find("Close tab");
    f.click(Point::new(row.x + super::LABEL_W + 60, row.y));
    assert!(f.texts().iter().any(|t| t.contains("Press keys")), "{:?}", f.texts());
    f.frame(vec![Event::KeyDown { key: Key::Char('q'), mods: CTRL_SHIFT, text: None }]);
    f.frame(vec![]);
    assert!(file(&f).contains("close_tab = \"ctrl+shift+q\""), "{}", file(&f));
    assert_eq!(f.app.keymap.match_key(&Key::Char('q'), CTRL_SHIFT, false), Some(Action::CloseTab));
    assert!(f.app.tabs.len() == 1, "the captured chord did not close the tab");
}

#[test]
fn rebind_escape_cancels_backspace_resets() {
    let mut f = settings("Keybindings");
    let row = f.find("Close tab");
    let at = Point::new(row.x + super::LABEL_W + 60, row.y);
    f.click(at);
    f.frame(vec![Event::KeyDown { key: Key::Escape, mods: Modifiers::NONE, text: None }]);
    f.frame(vec![]);
    assert!(!f.texts().iter().any(|t| t.contains("Press keys")));
    assert_eq!(file(&f), CONFIG, "Esc saves nothing");
    f.click(at);
    f.frame(vec![Event::KeyDown { key: Key::Char('q'), mods: CTRL_SHIFT, text: None }]);
    f.frame(vec![]);
    f.click(at);
    f.frame(vec![Event::KeyDown { key: Key::Backspace, mods: Modifiers::NONE, text: None }]);
    f.frame(vec![]);
    assert!(!file(&f).contains("close_tab ="), "{}", file(&f));
    assert_eq!(f.app.keymap.match_key(&Key::Char('w'), CTRL_SHIFT, false), Some(Action::CloseTab));
}

#[test]
fn external_edit_reloads() {
    let mut f = Fixture::new();
    assert_eq!(f.app.theme.name, "midnight");
    std::fs::write(&f.path, CONFIG.replace("midnight", "nord")).unwrap();
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
    std::fs::File::options().write(true).open(&f.path).unwrap().set_modified(later).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    f.frame(vec![]);
    assert_eq!(f.app.theme.name, "nord");
}

#[test]
fn unfocused_window_does_not_poll() {
    let mut f = Fixture::new();
    f.frame(vec![Event::FocusLost]);
    f.frame(vec![]);
    assert_eq!(f.h.repaint_in, None, "idle and unfocused: no timer");
    std::fs::write(&f.path, CONFIG.replace("midnight", "nord")).unwrap();
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
    std::fs::File::options().write(true).open(&f.path).unwrap().set_modified(later).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    f.frame(vec![]);
    assert_eq!(f.app.theme.name, "midnight");
    f.frame(vec![Event::FocusGained]);
    assert_eq!(f.app.theme.name, "nord", "focus gain polls at once");
}

#[test]
fn unsaved_form_survives_reload() {
    let mut f = settings("General");
    set_field(&mut f, "ssh program", "my-ssh");
    f.app.reload_config();
    f.frame(vec![]);
    assert!(f.texts().iter().any(|t| t == "my-ssh"), "{:?}", f.texts());
}

#[test]
fn form_follows_its_connection_when_others_are_deleted() {
    let mut f = Fixture::new();
    f.right_click_text("beta");
    f.click_text("Edit");
    set_field(&mut f, "Port", "3333"); // unsaved edit
    f.app.cfg.connections.remove(0); // alpha deleted elsewhere (sidebar, outside edit)
    f.app.save_config();
    f.frame(vec![]);
    f.click_text("Save"); // used to overwrite whatever sat at beta's old index
    f.click_text("Duplicate"); // used to index past the end
    let names: Vec<&str> = f.app.cfg.connections.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["beta", "beta-copy"], "nothing else overwritten");
    assert_eq!(f.app.cfg.connections[0].port, Some(3333));
}
