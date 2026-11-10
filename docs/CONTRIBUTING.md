# Contributing

See [ARCHITECTURE.md](ARCHITECTURE.md) for how the crates fit together and
[CONFIGURATION.md](CONFIGURATION.md) for every config key.

## Development setup

You need:

- Rust stable with edition 2024 support (1.85 or newer), with `rustfmt` and
  `clippy` (`rustup component add rustfmt clippy`).
- The OpenSSH client (`ssh`) on `PATH`, version 8.4 or newer for the
  password/askpass tests. Many tests start a real `ssh` against the local
  test server.

Per platform:

- **Windows**: the built-in OpenSSH client (Settings → Optional features →
  OpenSSH Client) is enough. Terminals use ConPTY. Builds with the MSVC
  toolchain.
- **Linux**: `openssh-client`. The GUI talks to X11 or Wayland at run time
  (through `winit`, loaded dynamically) and to the screen reader over D-Bus
  (AccessKit); no extra `-dev` packages are needed to build. Forwards use
  `PR_SET_PDEATHSIG` so they stop with skiff.
- **macOS**: the system `ssh` works. Forwards are tied to skiff's lifetime
  on a best-effort basis only.

## Build and run

```bash
cargo build --workspace                 # everything
cargo run -p skiff-ssh                  # the TUI (binary `skiff`)
cargo run -p skiff-ssh -- web uptime    # CLI usage
cargo run -p skiff-gui --release        # the GUI
cargo run -p skiff-testserver -- --shell --password secret   # a local server to connect to
cargo run -p skiff-ui --example demo    # toolkit widget showcase
cargo run -p skiff-gui --example screenshots -- OUT_DIR       # offscreen screenshots
cargo run -p skiff-gui --release --example flood -- 50        # 50 MB of output: run it in a terminal tab, check typing stays responsive
cargo doc --workspace --no-deps         # API docs (must build without warnings)
```

To try the apps against the test server rather than your own hosts, run
them with `SKIFF_CONFIG=scratch.toml` and `load_ssh_config = false`; see
[Trying it without a real server](CONFIGURATION.md#trying-it-without-a-real-server).

## Tests

```bash
cargo test --workspace
cargo test -p skiff-gui --release --test perf -- --ignored --nocapture   # timing checks
```

Rules every test follows:

- **Never a real server.** Anything that connects uses `skiff-testserver`
  (`TestFixture`), which listens on 127.0.0.1 only, generates throwaway
  keys and writes its own ssh config and `known_hosts`. Every `ssh` a test
  spawns gets `-F <fixture config>` (or `-F none`) plus the fixture's
  options, so the user's `~/.ssh/config`, keys and agent are never used.
- **Never real config files.** Tests write `config.toml`, `history.toml`,
  `state.toml` and ssh config files under a `tempfile` directory and pass
  those paths in (for example `settings.ssh_config_path`). Nothing reads
  or writes the real skiff config, history or `~/.ssh/config`.
- **GUI tests run headless.** `skiff_ui::ui::testing::Harness` and
  `shell::render_offscreen` drive frames without a window;
  `skiff-gui/src/testing.rs` has helpers that wait for a pane to reach a
  state. Accessibility is tested through the offscreen tree.
- Keep tests lean: one focused test per behavior, extending an existing one
  where that is natural. Larger modules keep their tests in a sibling
  `*_tests.rs` / `tests_*.rs` file.

## Formatting and lints

- `cargo fmt --all` with the repo's `rustfmt.toml` (`max_width = 140`,
  `use_small_heuristics = "Max"`).
- `cargo clippy --workspace --all-targets -- -D warnings` must be clean.
- `cargo doc --workspace --no-deps` must build without warnings. Doc-comment
  code blocks are doctests: mark non-Rust or illustrative blocks `text`.
- Every file starts with a short `//!` saying what it is for; functions get
  a `///` line saying what they do and why. Comment blocks whose purpose
  isn't obvious, not single obvious lines.
- No new dependencies without a clear need; say why in the commit message.

## Commits

Conventional style, small and focused, one logical change each:

```text
fix(tui): symbol chords ignore Shift; refuse binding the shell's control keys
feat(gui): …
docs: overwrite behaviour and Ctrl+Shift chords
refactor: rename termax to skiff
```

Common scopes: `core`, `tui`, `gui`, `ui`, `sshconfig`, `transfer`,
`askpass`, `testserver`. Update `README.md` and the files in `docs/` when
user-visible behavior changes, including the "Known limitations" list.

## How to…

### Add a GUI keybinding

1. In `crates/skiff-gui/src/keys.rs`, add a variant to `Action` and to
   `Action::ALL` (bump the array length).
2. Give it a `config_name` (the `[keys]` name), a `label` (shown in
   Settings → Keybindings) and `defaults` for both
   Windows/Linux and macOS. Avoid plain `Ctrl`/`Alt` + character chords:
   inside a terminal those always go to the shell.
3. Handle the action where `Keymap::match_key` results are dispatched in
   `crates/skiff-gui/src/app.rs`.
4. Add or extend a test in `keys_tests.rs`, and add the action to the
   `[keys]` table in `docs/CONFIGURATION.md` and the README key table.

### Add a TUI keybinding

1. In `crates/skiff-tui/src/keys.rs`, add the variant to `Action` and
   `Action::ALL`, with `config_name`, `label` and `defaults`. Defaults need
   `Ctrl` or `Alt` (or an F-key), and must not be one of the shell's control
   keys.
2. Handle it in `crates/skiff-tui/src/app.rs` where `Keymap::match_key`
   results are dispatched; the help screen and status line pick up the
   chords from the keymap.
3. The `every_action_has_a_unique_name` and `defaults_are_current_bindings`
   tests cover naming and defaults; document the action in
   `docs/CONFIGURATION.md` under `[tui_keys]`.

### Add a setting

1. Add the field to `Settings` (both apps) or `GuiSettings` (GUI only) in
   `crates/skiff-core/src/config.rs`, with a `///` doc, and its default in
   the matching `Default` impl (`#[serde(default)]` makes old files load).
   If new users should see it, add it to the first-run `TEMPLATE`.
2. Read it in the front-end(s). For the GUI, put a control on the right
   Settings page (`crates/skiff-gui/src/settings/general.rs` or
   `appearance.rs`) that saves through `GuiApp::save_config`, which only
   rewrites changed keys.
3. Add a config round-trip or behavior test, and document the key with its
   default in `docs/CONFIGURATION.md`.

### Add a built-in theme

1. Add a `def("name", [10 UI colors], [16 ANSI colors])` entry to
   `builtin()` in `crates/skiff-core/src/theme.rs`. The UI colors go in the
   order bg, fg, accent, muted, border, selection bg, selection fg, success,
   warning, error.
2. Run the tests: core checks that every built-in parses and has a valid
   16-color palette, and `skiff-gui/src/theme_tests.rs` checks every
   built-in meets the contrast targets (4.5:1 text, 3:1 outlines and focus).
3. Add the name to the theme list in `docs/CONFIGURATION.md` and the README.
