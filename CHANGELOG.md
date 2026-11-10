# Changelog

All notable changes to Skiff are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
uses [Semantic Versioning](https://semver.org/).

## 0.1.0 - Unreleased

First release under the name Skiff (formerly termax).

### Changed

- **Renamed termax to Skiff.** The TUI and CLI command is `skiff` (crate
  `skiff-ssh`), the desktop app `skiff-gui`.
- **Config directory.** Settings live in the `skiff` folder
  (`%APPDATA%\skiff\config\`, `~/.config/skiff/`,
  `~/Library/Application Support/skiff/`). On first start the old termax
  folder is copied over once and left as it was. After a failed copy that
  session writes nothing into the new folder (history and window state stay
  in memory, settings edits are refused), so the next start tries again; an interrupted or concurrent
  first start (TUI and GUI together) is handled. See
  [Coming from termax](docs/CONFIGURATION.md#coming-from-termax).
- **`SKIFF_CONFIG`** sets the config file; `TERMAX_CONFIG` is still honoured
  for this release when `SKIFF_CONFIG` is unset.
- **ssh config comments** written by Skiff are `# skiff:`; old `# termax:`
  comments are still read and rewritten when the host is edited.
- **Partial transfer files** are named `*.skiff-part`; `*.termax-part`
  leftovers are skipped like them.
- **`skiff list --json`**: hosts defined in the Skiff config report
  `"source": "skiff"` (was `"termax"`).
- **TUI default keys moved.** Bash's readline binds nearly every Alt key, so
  TUI shortcuts are now `Alt+O` followed by a key (`Alt+O 1`…`9`, `Alt+O w`,
  `Alt+O ?`, `Alt+O /`, ...); `Alt+[` / `Alt+]` still switch tabs. The old
  `Alt+` chords reach the shell; the `[tui_keys]` docs show how to restore
  them.
- **GUI pane drag** with the mouse needs `Alt+Shift` (or the grip), so plain
  `Alt+click` reaches programs that read the mouse.

### Added

- **TUI refresh.** Rounded borders with the focused panel in the accent
  color and titles (counts, sort, status) in the borders; a two-group
  footer (the tab's keys, then the keys after the prefix behind an
  `Alt+O ▸` badge) that drops the least used hints on narrow screens but
  always keeps `? help`; a `PREFIX` badge and, after `which_key_delay_ms`
  (300 ms, `0` for at once), a which-key box with the keys that can follow;
  dialogs over a dimmed screen with their keys on the bottom border and
  deletes in red; a grouped, scrollable help with the config path on its
  bottom border; host details that wrap between items and elide long paths
  in the middle; empty states with the keys that help; borderless
  terminals (split panes get a thin title rule); SFTP column headers,
  elided path titles and a one-line transfer strip with a progress bar;
  status marks `●` up, `◌` connecting, `✗` failed, `○` unknown.
- `NO_COLOR` support (bold and reverse only) and `tui_ascii_borders` for
  fonts without box drawing.
- GUI design system: spacing / radius / height / type-scale tokens, a
  semibold UI face, vector icons, an overlay shadow, button variants and
  icon buttons, badges and status dots, sortable table headers, steppers,
  empty states, modal dialogs with a focus trap, toasts, and menus with
  icons, separators and danger items (shown in the `skiff-ui` demo). The
  keyboard focus ring is 2 px with a 2 px gap; Windows' "show animations"
  setting turns animations off.
- **GUI refresh.** A command palette replaces the quick switcher
  (`Ctrl+Shift+P` / `Cmd+Shift+P`, plus the old shortcuts): Recent and
  Suggested on an empty query, `>` commands, `@` hosts, `:` port forwards
  (start / stop), `#` snippets, with icons, kind badges, highlighted matches
  and shortcuts. Notices are toasts (errors stay until closed) and the
  status bar is slimmer: the active view's state, the hovered host and
  what is running. The tab strip shows connection dots and icons, with
  `+` and the gear as icon buttons. The sidebar has a search field with an
  icon, caption group headers with counts, rows with a semibold name,
  mono address and tag chips, a stronger selection and an empty state
  (Add host / Import ~/.ssh/config). Context menus have icons, groups and
  a red Delete and open beside the pointer. Delete, close-with-transfers
  and multi-line paste confirmations are modal dialogs. Split panes get a
  wider divider hit zone, dimmed inactive panes (`dim_inactive_panes`),
  a grip shown on hover and dashed drop zones.
- Split panes in both front-ends: drag panes and tabs to split, swap, join
  and pop out (GUI), divider snapping, keyboard move / resize / equalize.
- Settings gear at the end of the GUI tab strip; Snippets page in Settings.
- Remappable TUI shortcuts (`[tui_keys]`), SFTP password prompts and
  cancellable transfers in the TUI.

### Fixed

- The paper theme's bright terminal colours were hard to read on its light
  background; ANSI colours on light themes are now darkened to readable
  contrast.

- GUI pane shortcuts that have nothing to do pass their key to the shell.
- The TUI split divider no longer stays stuck dragging after a lost mouse
  release.
- The settings-copy note shows as the TUI's first status line.
- Transfers keep destination mode, owner and symlinks, use private temp
  files, and a cancelled overwrite keeps the original.
