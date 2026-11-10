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
  folder is copied over once and left as it was. A failed copy writes no
  fresh config, so the next start tries again; an interrupted or concurrent
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

- Split panes in both front-ends: drag panes and tabs to split, swap, join
  and pop out (GUI), divider snapping, keyboard move / resize / equalize.
- Settings gear at the end of the GUI tab strip; Snippets page in Settings.
- Remappable TUI shortcuts (`[tui_keys]`), SFTP password prompts and
  cancellable transfers in the TUI.

### Fixed

- GUI pane shortcuts that have nothing to do pass their key to the shell.
- The TUI split divider no longer stays stuck dragging after a lost mouse
  release.
- The settings-copy note shows as the TUI's first status line.
- Transfers keep destination mode, owner and symlinks, use private temp
  files, and a cancelled overwrite keeps the original.
