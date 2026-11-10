//! Settings screen tests (temp config files only, no network).

use skiff_ui::geom::Point;
use skiff_ui::input::{Event, Key, Modifiers};

use crate::app::tests::{CONFIG, Fixture};
use crate::keys::Action;

const CTRL: Modifiers = Modifiers { ctrl: true, shift: false, alt: false, logo: false };
const CTRL_SHIFT: Modifiers = Modifiers { ctrl: true, shift: true, alt: false, logo: false };

/// A fixture with the settings tab open on `section`.
fn settings(section: &str) -> Fixture {
    let mut f = Fixture::new();
    f.app.do_action(Action::Settings);
    f.frame(vec![]);
    f.click_text(section);
    f
}

/// Replaces the text of the input labelled `label`.
fn set_field(f: &mut Fixture, label: &str, value: &str) {
    let l = f.find_last(label);
    f.click(Point::new(l.x + super::LABEL_W + 20, l.y));
    f.frame(vec![Event::KeyDown { key: Key::Char('a'), mods: CTRL, text: None }]);
    f.frame(vec![Event::Text(value.into())]);
    f.frame(vec![]);
}

/// Activates the control named `name` the way a screen reader does.
fn activate(f: &mut Fixture, name: &str) {
    let id = f.h.tree().find(name).map(|n| n.id).unwrap_or_else(|| panic!("no control named {name:?}"));
    f.frame(vec![Event::A11y { target: id, action: skiff_ui::a11y::A11yAction::Click }]);
    f.frame(vec![]);
}

/// The config file's current text on disk.
fn file(f: &Fixture) -> String {
    std::fs::read_to_string(&f.path).unwrap()
}

/// Saving a connection edited from the sidebar keeps the file's comments and writes the new port.
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

/// A host added in settings is saved and shows in the sidebar.
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

/// Adding a host with a taken name fails with a message and leaves the file untouched.
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

/// Port 0 is refused and nothing is added.
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

/// A missing font file shows an error and changes neither the fonts nor the config.
#[test]
fn font_path_invalid_keeps_old_fonts() {
    let mut f = settings("Appearance");
    set_field(&mut f, "Terminal font", "C:/skiff-no-such-dir/missing.ttf");
    f.frame(vec![Event::KeyDown { key: Key::Enter, mods: Modifiers::NONE, text: None }]);
    f.frame(vec![]);
    assert!(f.texts().iter().any(|t| t.contains("could not load")), "{:?}", f.texts());
    assert!(f.app.pending_fonts.is_none());
    assert_eq!(f.app.cfg.gui.font_path, None);
    assert!(!file(&f).contains("missing.ttf"));
}

/// Recording a chord saves it under `[keys]`, takes effect, and is not acted on while recording.
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

/// A changed binding shows the "changed" dot and Reset restores the
/// default; the filter narrows the list by action name.
#[test]
fn keybindings_reset_and_filter() {
    let mut f = Fixture::with_config(&format!("{CONFIG}\n[keys]\nclose_tab = \"ctrl+shift+q\"\n"));
    f.app.do_action(Action::Settings);
    f.frame(vec![]);
    f.click_text("Keybindings");
    assert!(f.h.tree().find("Changed from the default").is_some());
    activate(&mut f, "Reset Close tab to the default");
    assert!(!file(&f).contains("close_tab"), "{}", file(&f));
    assert!(f.h.tree().find("Changed from the default").is_none());
    f.app.settings.keys.filter = "split".into();
    f.frame(vec![]);
    assert!(f.try_find("Split pane right").is_some() && f.try_find("Close tab").is_none(), "{:?}", f.texts());
    assert_eq!(super::keys::keycaps("Ctrl++"), ["Ctrl", "+"]);
}

/// Esc while recording saves nothing; Backspace removes the override and restores the default chord.
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

/// A config file changed on disk is picked up (here: the theme).
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

/// An unfocused window stops polling the config file and checks again as soon as it regains focus.
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

/// A config reload does not overwrite a form with unsaved edits.
#[test]
fn unsaved_form_survives_reload() {
    let mut f = settings("General");
    set_field(&mut f, "ssh program", "my-ssh");
    f.app.reload_config();
    f.frame(vec![]);
    assert!(f.texts().iter().any(|t| t == "my-ssh"), "{:?}", f.texts());
    // Enter applies it; an empty program is refused with the reason.
    f.frame(vec![Event::KeyDown { key: Key::Enter, mods: Modifiers::NONE, text: None }]);
    f.frame(vec![]);
    assert_eq!(f.app.cfg.settings.ssh_binary, "my-ssh");
    set_field(&mut f, "ssh program", " ");
    f.frame(vec![Event::KeyDown { key: Key::Enter, mods: Modifiers::NONE, text: None }]);
    f.frame(vec![]);
    assert!(f.try_find("The ssh program is required").is_some(), "{:?}", f.texts());
    assert_eq!(f.app.cfg.settings.ssh_binary, "my-ssh");
}

/// The scrollback stepper applies and saves at once, clamped to its range.
#[test]
fn scrollback_stepper_applies_at_once() {
    let mut f = settings("General");
    let before = f.app.cfg.settings.scrollback;
    activate(&mut f, "Increase");
    assert_eq!(f.app.cfg.settings.scrollback, before + 1000);
    assert!(file(&f).contains(&format!("scrollback = {}", before + 1000)), "{}", file(&f));
    assert_eq!(f.h.tree().find("Scrollback lines").map(|n| n.role), Some(skiff_ui::a11y::Role::SpinButton));
}

/// The search finds settings on every page (and hosts by name); picking a
/// result opens its page with the control focused.
#[test]
fn search_filters_and_opens_settings() {
    let mut f = settings("Connections");
    let all = super::index(&f.app);
    assert_eq!(super::search(&all, "scroll").first().map(|e| e.title.as_str()), Some("Scrollback lines"));
    assert!(super::search(&all, "beta").iter().any(|e| e.section == super::Section::Connections), "hosts by name");
    assert!(super::search(&all, "zzz-nothing").is_empty());
    f.click_text("Search settings");
    f.frame(vec![Event::Text("scroll".into())]);
    f.frame(vec![]);
    assert!(f.try_find("Scrollback lines").is_some(), "{:?}", f.texts());
    f.click_text("Scrollback lines");
    assert_eq!(f.app.settings.section, super::Section::General);
    assert!(f.app.settings.search.is_empty());
    assert_eq!(f.h.state.focused(), Some(skiff_ui::ui::Id::new(super::general::ID_SCROLL).with("field")));
}

/// An open connection form saves and duplicates the right host after an earlier one is deleted elsewhere.
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

/// One host-specific, two-line snippet, appended to the test config.
const SNIPPET: &str = r#"
[[snippet]]
name = "greet"
command = "echo one\necho two"
host = "beta"
"#;

/// A fixture with `extra` appended to the config and the Snippets page open.
fn snippets_page(extra: &str) -> Fixture {
    let mut f = Fixture::with_config(&format!("{CONFIG}{extra}"));
    f.app.do_action(Action::Settings);
    f.frame(vec![]);
    f.click_text("Snippets");
    f
}

/// Adding a snippet focuses Name, saves a `[[snippet]]` entry, and lists it with an accessible name.
#[test]
fn add_snippet_saves_to_config() {
    let mut f = snippets_page("");
    f.click_text("Add");
    assert_eq!(f.h.state.focused(), Some(skiff_ui::ui::Id::new("settings-snip-name")), "Add starts at the name");
    f.frame(vec![Event::Text("logs".into())]);
    set_field(&mut f, "Command", "journalctl -f");
    f.click_text("Run it (press Enter after the command)");
    f.click_text("Save");
    let text = file(&f);
    assert!(text.contains("# my hand-written comment"), "{text}");
    assert!(text.contains("[[snippet]]\nname = \"logs\"\ncommand = \"journalctl -f\"\nrun = true"), "{text}");
    let t = f.h.tree();
    assert_eq!(t.find("logs").map(|n| n.role), Some(skiff_ui::a11y::Role::ListItem), "listed by name");
    let host = t.find("Host").expect("host field");
    assert_eq!((host.role, host.value.as_deref()), (skiff_ui::a11y::Role::ComboBox, Some("Every host")));
}

/// Missing name, duplicate name and blank command are each refused with a message; nothing is saved.
#[test]
fn snippet_form_validates() {
    let mut f = snippets_page(SNIPPET);
    f.click_text("Add");
    f.click_text("Save");
    assert!(f.texts().iter().any(|t| t == "A snippet needs a name"), "{:?}", f.texts());
    set_field(&mut f, "Name", "greet");
    set_field(&mut f, "Command", "ls");
    f.click_text("Save");
    assert!(f.texts().iter().any(|t| t == "A snippet named greet already exists"), "{:?}", f.texts());
    set_field(&mut f, "Name", "other");
    set_field(&mut f, "Command", "   ");
    f.click_text("Save");
    assert!(f.texts().iter().any(|t| t == "The command is empty"), "{:?}", f.texts());
    assert_eq!(f.app.cfg.snippets.len(), 1);
    assert_eq!(file(&f), format!("{CONFIG}{SNIPPET}"));
}

/// Renaming a multi-line, host-specific snippet keeps its command lines and host.
#[test]
fn edit_snippet_keeps_lines_and_host() {
    let mut f = snippets_page(SNIPPET);
    f.click_text("greet");
    assert!(f.texts().iter().any(|t| t == "echo two"), "one input per line: {:?}", f.texts());
    set_field(&mut f, "Name", "hello");
    f.click_text("Save");
    let s = &f.app.cfg.snippets;
    assert_eq!(
        (s.len(), s[0].name.as_str(), s[0].command.as_str(), s[0].host.as_deref()),
        (1, "hello", "echo one\necho two", Some("beta"))
    );
    assert!(!file(&f).contains("greet"), "{}", file(&f));
}

/// Delete asks first with focus on Cancel; Esc keeps the snippet, confirming removes it.
#[test]
fn delete_snippet_asks_first() {
    let mut f = snippets_page(SNIPPET);
    f.click_text("greet");
    f.frame(vec![Event::KeyDown { key: Key::Delete, mods: Modifiers::NONE, text: None }]);
    f.frame(vec![]);
    assert!(f.try_find("Delete snippet greet?").is_some(), "{:?}", f.texts());
    let cancel = skiff_ui::ui::Id::new("settings-snip-delete").with(("button", 0usize));
    assert_eq!(f.h.state.focused(), Some(cancel), "focus on Cancel");
    f.frame(vec![Event::KeyDown { key: Key::Escape, mods: Modifiers::NONE, text: None }]);
    f.frame(vec![]);
    assert!(f.try_find("Delete snippet greet?").is_none());
    assert_eq!(f.app.cfg.snippets.len(), 1);
    f.click_text("Delete");
    let at = f.find_last("Delete"); // the dialog's button
    f.click(at);
    assert!(f.app.cfg.snippets.is_empty());
    assert_eq!(file(&f), CONFIG);
}

/// Settings lists and dropdowns expose accessible names and roles.
#[test]
fn settings_lists_and_dropdowns_are_named() {
    let f = settings("Connections");
    let t = f.h.tree();
    assert_eq!(t.find("Connections").map(|n| n.role), Some(skiff_ui::a11y::Role::Button), "the nav button comes first");
    assert!(t.nodes.iter().any(|n| n.role == skiff_ui::a11y::Role::List && n.name == "Connections"));
    assert_eq!(t.find("alpha").map(|n| n.role), Some(skiff_ui::a11y::Role::ListItem));
    let mut f = settings("General");
    f.frame(vec![]);
    let t = f.h.tree();
    assert_eq!(t.find("Check host status").map(|n| n.role), Some(skiff_ui::a11y::Role::ComboBox));
}

/// Focusing a keybinding row below the fold scrolls it fully into view.
#[test]
fn keybindings_scroll_to_the_focused_row() {
    let mut f = settings("Keybindings");
    let id = skiff_ui::ui::Id::new(("settings-key", Action::ClosePane.config_name()));
    f.frame(vec![Event::A11y { target: id, action: skiff_ui::a11y::A11yAction::Focus }]);
    f.frame(vec![]);
    let b = f.h.tree().by_id(id).expect("close pane row").bounds;
    assert!(b.h > 0 && b.bottom() <= 800 - 24, "fully above the status bar: {b:?}");
}

/// Two snippets for switching between.
const TWO_SNIPPETS: &str = r#"
[[snippet]]
name = "greet"
command = "ls -l"

[[snippet]]
name = "other"
command = "ls"
"#;

/// Switching away from an edited snippet asks first (Esc and Keep keep edits); an untouched new form does not ask.
#[test]
fn switching_snippets_asks_before_discarding_edits() {
    let mut f = snippets_page(TWO_SNIPPETS);
    f.click_text("greet");
    set_field(&mut f, "Name", "greet2");
    f.click_text("other");
    assert!(f.try_find("Discard unsaved changes to greet?").is_some(), "{:?}", f.texts());
    assert_eq!(f.h.state.focused(), Some(super::discard_id("settings-snip").with(("button", 0usize))), "focus on Keep");
    f.frame(vec![Event::KeyDown { key: Key::Escape, mods: Modifiers::NONE, text: None }]);
    f.frame(vec![]);
    let form = f.app.settings.snippets.form.as_ref().unwrap();
    assert_eq!((form.editing.as_deref(), form.name.as_str()), (Some("greet"), "greet2"), "edits kept");
    f.click_text("Add");
    assert!(f.try_find("Discard unsaved changes to greet?").is_some(), "Add asks too: {:?}", f.texts());
    f.click_text("Keep editing");
    f.click_text("other");
    f.click_text("Discard");
    assert_eq!(f.app.settings.snippets.form.as_ref().unwrap().name, "other");
    // An untouched new form is not an edit.
    f.click_text("Add");
    f.click_text("greet");
    assert!(f.texts().iter().all(|t| !t.starts_with("Discard unsaved")), "{:?}", f.texts());
    assert_eq!(f.app.settings.snippets.form.as_ref().unwrap().name, "greet");
}

/// Removing a command line keeps the remaining line's input id (and so its focus and cursor).
#[test]
fn snippet_line_inputs_keep_their_ids() {
    let mut f = snippets_page(SNIPPET);
    f.click_text("greet");
    let id_of = |f: &Fixture, v: &str| f.h.tree().nodes.iter().find(|n| n.value.as_deref() == Some(v)).map(|n| n.id);
    let before = id_of(&f, "echo two").expect("line 2");
    activate(&mut f, "Remove line 1");
    assert_eq!(f.app.settings.snippets.form.as_ref().unwrap().lines, vec!["echo two"]);
    assert_eq!(id_of(&f, "echo two"), Some(before), "its input (focus, cursor) stays with the line");
}

/// Opening another host over an edited connection form asks first; an untouched new form does not ask.
#[test]
fn connection_form_asks_before_discarding_edits() {
    let mut f = Fixture::new();
    f.right_click_text("beta");
    f.click_text("Edit");
    set_field(&mut f, "Port", "3333"); // unsaved edit
    f.right_click_text("alpha");
    f.click_text("Edit"); // from the sidebar
    assert!(f.try_find("Discard unsaved changes to beta?").is_some(), "{:?}", f.texts());
    assert_eq!(f.h.state.focused(), Some(super::discard_id("settings-conn").with(("button", 0usize))), "focus on Keep");
    f.frame(vec![Event::KeyDown { key: Key::Escape, mods: Modifiers::NONE, text: None }]);
    f.frame(vec![]);
    assert!(f.texts().iter().any(|t| t == "3333"), "edits kept: {:?}", f.texts());
    f.click_text("Add");
    assert!(f.try_find("Discard unsaved changes to beta?").is_some(), "{:?}", f.texts());
    f.click_text("Discard");
    assert!(f.try_find("New host").is_some(), "{:?}", f.texts());
    // An untouched new form is not an edit.
    f.right_click_text("alpha");
    f.click_text("Edit");
    assert!(f.texts().iter().all(|t| !t.starts_with("Discard unsaved")), "{:?}", f.texts());
    assert_eq!(f.app.settings.conn.form.as_ref().and_then(|f| f.editing.as_deref()), Some("alpha"));
}
