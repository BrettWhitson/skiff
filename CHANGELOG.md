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
- **GUI Settings apply as you go** (text fields on `Enter` or leaving the field);
  the General and Appearance pages no longer have Save / Apply buttons.
  The host and snippet editors keep an explicit Save.
- "Discard changes?" and delete questions are modal dialogs instead of
  inline bars.

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
- Split panes in both front-ends: drag panes and tabs to split, swap, join
  and pop out (GUI), divider snapping, keyboard move / resize / equalize.
- Settings gear at the end of the GUI tab strip; Snippets page in Settings.
- **GUI Files.** Breadcrumb path bar (click a segment, or `Ctrl+L` / click
  the last one to type a path), sortable Name / Size / Modified headers,
  file / folder / link icons, a footer with the item count and selection,
  type-ahead selection, a grouped toolbar with Delete last in red, a
  labelled "Hidden files" toggle, and delete confirmed in a dialog.
  Copies show in a collapsible drawer: a 4 px progress bar, size, rate and
  time left per file, Cancel, Retry on failure, and a summary in its header
  ("2 active · 1 failed").
- **GUI Forwards.** A table with a header row and status pills; Start /
  Stop per row with Edit and Delete in a "More actions" menu; deleting asks
  in a dialog; an empty state with "Add a forward".
- **GUI Settings.** Search across every page, a 200 px nav, section
  headers, number steppers, Keybindings grouped with keycaps, a filter,
  a "changed" dot and reset; Connections and Snippets have Add in the list
  header, and the host editor is grouped into Basic and Advanced.
- Remappable TUI shortcuts (`[tui_keys]`), SFTP password prompts and
  cancellable transfers in the TUI.

### Fixed

- GUI pane shortcuts that have nothing to do pass their key to the shell.
- The TUI split divider no longer stays stuck dragging after a lost mouse
  release.
- The settings-copy note shows as the TUI's first status line.
- Transfers keep destination mode, owner and symlinks, use private temp
  files, and a cancelled overwrite keeps the original.
