# Architecture

Skiff is an SSH and SFTP client with two front-ends, a terminal UI and a
desktop GUI, over one shared core. It never implements SSH itself: every
connection is the system OpenSSH client (`ssh`), so `ProxyJump`, agents,
keys, `known_hosts`, `Include` and the rest of a user's OpenSSH setup behave
exactly as on the command line.

## Crates

```text
                 skiff-core  (config, hosts, ssh_config editing, terminal model,
                 /     \      SFTP engine, askpass, forwards, status probes)
                /       \
   skiff-ssh (TUI)    skiff-gui (desktop app)
   bin `skiff`        bin `skiff-gui`
   ratatui+crossterm        |
                         skiff-ui  (in-house immediate-mode toolkit + AccessKit)

   skiff-testserver: local SSH/SFTP server on 127.0.0.1, dev-dependency of the others
```

| Crate | Path | Responsibility |
|---|---|---|
| `skiff-core` | `crates/skiff-core` | Everything that isn't drawing: `config.toml`, `state.toml`, `history.toml`, themes, the host list, reading and editing ssh config files, the CLI parser, terminal sessions (pty + `vt100`), terminal helpers (search, paste rules, mouse reporting), split-pane divider rules, the SFTP engine and transfers, the askpass broker, port forwards and reachability probes. No UI code. |
| `skiff-ssh` | `crates/skiff-tui` | The `skiff` binary: CLI commands (`list`, `info`, `completion`, `skiff host cmd`), and the full-screen TUI built with `ratatui` + `crossterm`; terminals are drawn with `tui-term`. |
| `skiff-gui` | `crates/skiff-gui` | The `skiff-gui` binary: sidebar, tabs (terminals with nested, draggable split panes, file transfer, forwards, settings), a Settings gear on the tab strip, quick switcher. Draws a cell grid for terminals itself. |
| `skiff-ui` | `crates/skiff-ui` | A from-scratch GUI toolkit: window shell (`winit` + `softbuffer`), CPU renderer with tile damage tracking, text (`fontdue` behind a trait), layout, focus, widgets and an accessibility tree bridged to AccessKit. Knows nothing about SSH. |
| `skiff-testserver` | `crates/skiff-testserver` | A `russh` server bound to 127.0.0.1 with SFTP, an optional echo shell, exec and local forwards, plus a fixture that generates keys and an isolated ssh config. Used by tests only; never shipped. |

## Data flow

### Host list

`hosts::collect` merges skiff's own `[[connection]]` entries with the hosts
of the user's ssh config (`sshconfig`, following `Include`). Each entry
remembers where it came from, which decides how it is edited and how `ssh`
is invoked: an ssh config host runs as `ssh <alias>` so OpenSSH applies its
options; a custom connection is turned into explicit `-l`, `-p`, `-i`, `-J`
and `extra_args`. `history` adds last-used times for "Recent" sorting.

### Terminal sessions (connect via system ssh)

`terminal::TerminalSession` opens a pseudo-terminal with `portable-pty`
(ConPTY on Windows) and runs `ssh <args>` in it. A reader thread per session
feeds output into a `vt100::Parser` behind a mutex, answers cursor-position
queries (ConPTY withholds output until it gets one), bumps a generation
counter and calls a notify callback so the front-end redraws. Keys and
pastes are encoded to bytes (`terminal::encode_key`, `sanitize_paste`) and
written to the pty; resizing resizes both pty and parser.

`termx` holds the logic both front-ends share on top of the parser: find
in scrollback (matches stay anchored to text as lines scroll off), deciding
when a multi-line paste must be confirmed (no bracketed paste), mouse
reporting to programs that enable it, snippets.

The TUI draws the parser's screen with `tui-term`; the GUI converts it to
toolkit cell rows (`skiff-gui/src/term/render.rs`) drawn by the display list.
A split pane is simply another `TerminalSession` to the same host.

### Split panes

Every pane is its own `TerminalSession` to the same host; moving panes
never restarts a session. The divider rules both front-ends share live in
`skiff_core::panes`: the snap shares (25 / 33 / 50 / 67 / 75%), `snap` (the
nearest share within a tolerance), `step_ratio` (keyboard: the next share in
a direction, then `KEY_STEP` = 5% past the last one), `MIN_RATIO` (10%) and
the smallest pane (`MIN_COLS` x `MIN_ROWS` = 10 x 3 cells). A ratio is
always the first (left / top) side's share.

- **TUI**: at most two panes per tab, a direction and one ratio. `ui.rs`
  lays them out (keeping each above the minimum, the divider column or
  title rule included) and maps a mouse drag on the divider to a ratio that
  snaps within one cell
  unless `Alt` is held; `app.rs` handles swap, resize (`step_ratio`) and
  equalize.
- **GUI** (`skiff-gui/src/term/split.rs`): a terminal tab is a `TermSplit`
  holding its panes, each with a stable `u64` key that is never reused in
  the tab, and a `Layout` binary tree. Leaves are `Pane(key)`; inner nodes
  are `Split { dir: Right | Down, ratio, a, b }`. Nodes are addressed by a
  path of turns. Each frame `Layout::arrange` turns the tree into pane
  rectangles (in reading order, which is also the "pane n of m" numbering)
  plus one divider per split. The minimum pane size is enforced on a
  per-frame copy of the tree, so stored ratios come back when the window
  grows again.
  - *Dividers* snap within 8 logical px of a share during a drag (`Alt`
    drags freely, a double-click equalizes by pane count) and step with the
    arrow keys or screen-reader increment / decrement. Each divider's
    widget id is keyed by the first pane key of the split's second side,
    which is unique and survives restructuring, so keyboard focus stays on
    a divider while the tree changes around it.
  - *Drag and drop*: a pane is dragged by its grip or with `Alt+Shift`+drag
    (plain `Alt`+click is left to programs reading the mouse). Over
    a target pane the middle third is the centre zone, elsewhere the nearest
    edge wins; an edge is offered only if both halves would keep the minimum
    size. Dropping on an edge removes the pane and re-inserts it beside the
    target at 50/50; the centre swaps the two. Dropping on the tab strip
    pops the pane out into its own tab. A whole terminal tab dragged onto a
    pane's edge has its subtree grafted in beside that pane (keys shifted so
    they stay unique); onto the centre it just switches to that tab.

### SFTP engine

`fs::connect` starts `ssh … -s <host> sftp` with piped stdio and runs the
`russh-sftp` client over those pipes, so authentication is still OpenSSH's.
`fs::Endpoint` is one side of a browser or transfer: the local disk or a
remote session. `fs::transfer` copies a file or tree between any two
endpoints in chunks (remote→remote is relayed through this machine without
touching the local disk), reports progress, honours cancellation and asks
before overwriting.

Writes go to a temp file `.<name>.<id>.skiff-part` next to the destination
and are renamed into place only when complete, so a cancel or failure never
damages the old file. When overwriting, the temp file is private (`0600`)
while written and then takes the old file's group and permission bits,
checked before the rename. Over SFTP the old file is removed just before the
rename (servers don't rename onto an existing file). A destination that is a
symlink, owned by someone else, in a directory that refuses new files, or
whose mode/group can't be reproduced is instead overwritten in place, so
the link, owner and ACL survive. Copies refuse directories, FIFOs, devices,
broken symlinks, symlinks owned by another user and (Windows) read-only
files as destinations.

### Askpass broker

An SFTP or forward `ssh` has no terminal to prompt on. With OpenSSH 8.4+
(`sshver`), skiff sets `SSH_ASKPASS` to its own executable and
`SSH_ASKPASS_REQUIRE=force`, plus `SKIFF_ASKPASS_ENDPOINT`, a per-connection
`SKIFF_ASKPASS_TAG` and a per-broker secret `SKIFF_ASKPASS_TOKEN`
(`askpass::Broker::env`).

```text
 ssh ──runs──> skiff[-gui] (helper mode: askpass::helper_main)
                    │  prompt + tag + token over a local socket
                    ▼   (named pipe on Windows, Unix socket elsewhere)
              askpass::Broker in the running app ──Ask──> UI dialog / inline prompt
                    ▲                                         │
                    └──────────── answer (never stored) ──────┘
```

Both binaries check for `SKIFF_ASKPASS_ENDPOINT` first thing in `main` and,
if present, act only as the helper. The broker refuses questions without
the token, classifies prompts (secret, yes/no host key, or a notice such as
"touch your security key") and withdraws questions for a tag once its
connection closes (`retire`). Without a new enough OpenSSH,
`sftp_batch_mode` makes ssh fail fast instead.

### Config and ssh_config editing

`config::Config` is serde-backed TOML. Saving diffs the new value against
the file with `toml_edit` and rewrites only changed keys, so comments and
unknown keys survive and two front-ends don't clobber each other. A
`FileWatcher` (mtime) lets the GUI pick up outside edits.

`sshconfig::doc` holds one ssh config file losslessly: an untouched file
renders back byte for byte. `sshconfig::edit` adds, updates, duplicates,
moves and deletes `Host` blocks touching only changed lines, keeps skiff's
metadata in a `# skiff:` comment, and saves with a conflict check (file
changed on disk → refuse), a rotating backup under `backups/` and an atomic
temp-file rename. `hostform` is the one host form both front-ends use; it
saves either to `[[connection]]` or in place to an ssh config file.

### Status probes

Opt-in only. `sshconfig::resolve` works out in-process (never `ssh -G`)
where to connect: the host itself, the first `ProxyJump` hop, or `Unknown`
for `ProxyCommand` and anything it can't evaluate. `status::check_all` runs
the probes on the tokio runtime, a few at a time: TCP connect, read the
first line, expect `SSH-`. It never authenticates or runs ssh.

### Port forwards

`forward::Forward` is a saved `[[forward]]`; `ForwardHandle::start` runs
`ssh -N -o ExitOnForwardFailure=yes -L|-R|-D …` with askpass wired up, and
reports when the forward is up or why it failed. Forwards die with skiff:
on Windows the child joins a kill-on-close job object; on Linux it gets
`PR_SET_PDEATHSIG`; on macOS this is best effort.

## Threading and async model

- **skiff-core** exposes blocking constructors plus functions that take a
  `tokio::runtime::Handle`; it never creates a runtime of its own. Each
  terminal session owns one OS reader thread.
- **TUI**: `main` builds a 2-worker multi-thread tokio runtime. The UI loop
  runs on the main thread: `crossterm::event::poll` with a 30 ms timeout,
  then draining all pending input, then draining background messages
  (`tokio::sync::mpsc` unbounded channel of `Msg`: connections, listings,
  transfer progress, askpass questions, status results) and redrawing.
- **GUI**: `skiff_gui::engine::Engine` owns a multi-thread tokio runtime
  and a `std::sync::mpsc` queue of `Msg`. Every send also calls a wake
  function that wakes the `winit` event loop, so the UI never polls; the UI
  thread drains the queue at the start of each frame. Messages carry their
  tab id so results for a closed tab are dropped. Closing a window shuts the
  runtime down in the background so a stuck connection never blocks.
- **skiff-ui** renders on the UI thread, splitting large repaints across
  several threads by tile band.

## GUI toolkit and accessibility

`skiff-ui` is immediate mode: there is no retained widget tree. Each frame
the app calls `Ui::run`; widgets are functions over `&mut Ui` that lay
themselves out, ask `Ui::interact` what happened to their rectangle and push
primitives into a `DisplayList`. State that must outlive a frame (focus, the
pressed widget, scroll offsets, drags) lives in `UiState`, keyed by hashed
`Id`s. Hit-testing uses the previous frame's rectangles so overlays win.

`TileRenderer` hashes what reaches each 64 px tile and repaints only changed
tiles into a CPU `Canvas`; the shell copies the damaged areas to the window
with `softbuffer`. Text uses `fontdue` behind a `GlyphRasterizer` trait,
font stacks with fallbacks, and a glyph cache. Everything also runs
headless (`ui::testing::Harness`, `shell::render_offscreen`), which is how
the GUI is tested.

Accessibility is part of the widget contract: while drawing, each widget
adds a node (role, name, value, state, bounds) to an `a11y` tree. The tree
is built only while an assistive technology is connected; `a11y_kit`
converts it into AccessKit updates for Narrator/NVDA/JAWS, VoiceOver and
Orca, and AccessKit actions come back as ordinary `Event::A11y` input the
next frame. Keyboard access follows the same model: `Tab` cycles controls
within a focus region, `F6` moves between regions (sidebar divider, hosts,
tab strip, content, each split pane, pane dividers), and a focus ring plus a
list focus bar show focus without relying on color. Every divider, the
sidebar's `splitter_h` included, is a focusable `Splitter` node whose value
is the first side's share; the arrow keys and the screen reader's
increment / decrement move it. The GUI's terminal announces new
output politely and rate-limited (`term/announce.rs`), skipping the echo of
typed keys. `skiff-gui/src/theme.rs` nudges theme colors to meet WCAG
contrast, and `os.rs` follows Windows high contrast, animation and text size.

### Design system

`skiff-ui/src/style.rs` holds the tokens every widget uses: the spacing
scale (2–48), radii (4 inputs and buttons, 6 rows and tabs, 10 overlays),
heights (control 30, row 28 / 24, tab 34), the type scale (`TextRole`:
caption 12, body 13, title 15, heading 18, display 22; titles semibold),
motion durations and the overlay shadow. They are logical pixels, so the
interface zoom scales them all. `Palette` names the colour roles (three
surfaces, text / muted / disabled, accent, focus, border / border-strong,
selection, hover, success / warning / danger / info, scrim); contrast tests
in `style.rs` and `skiff-gui/src/theme_tests.rs` cover every role on every
surface it is drawn on. Semibold text uses the system's semibold face
(Segoe UI Semibold, DejaVu Sans Bold) when present, else the UI font
emboldened (`text::Emboldened`). Icons (`icons.rs`) are strokes, arcs and
dots on a 16-unit grid rasterized with anti-aliasing at the drawn size and
cached per size; overlays cast one soft shadow (`Prim::Shadow`, a cached
falloff table). The components (button variants, icon buttons, badges and
status dots, section and sortable table headers, steppers, empty states,
modal dialogs with a focus trap, toasts, menus with icons and separators,
middle / start elision) are showcased in the toolkit demo
(`cargo run -p skiff-ui --release --example demo`). The focus ring shows
only after keyboard navigation; `Style::reduce_motion` (from the OS
animation setting) stops pulses and transitions.

### GUI pages

The Files, Forwards and Settings tabs are built from those components.
A file pane (`files/pane.rs`) is a card: location picker, Up / Refresh and a
breadcrumb path bar (`crumbs`), a grouped toolbar ending in Delete, a
sortable table (`sort_entries`, folders first) with type-ahead and a footer
(`footer_text`). The transfer drawer (`files/transfers.rs`) collapses to its
header, whose `summary` reads like "2 active · 1 failed"; finished batches
leave a notice (`FilesTab::take_notices`) for the app to show as a toast.
Settings (`settings/mod.rs`) has a 200 px nav with a search box over an
index of every setting (`index`, `search`; each entry knows the control to
focus), and content at most 720 px wide (list + editor pages are wider).
Deletes and "discard changes?" questions are toolkit dialogs; settings
notices also go out through `SettingsState::take_notices`.

## Key files

| Area | Files |
|---|---|
| Config, state, history, migration | `skiff-core/src/{config,state,history,migrate}.rs` |
| Themes | `skiff-core/src/theme.rs`, `skiff-gui/src/theme.rs`, `skiff-tui/src/tui_theme.rs` |
| Host list and form | `skiff-core/src/{hosts,hostform}.rs` |
| ssh config read/edit/resolve | `skiff-core/src/sshconfig/{mod,doc,edit,resolve}.rs` |
| CLI | `skiff-core/src/cli.rs`, `skiff-tui/src/main.rs` |
| Terminal sessions | `skiff-core/src/{terminal,termx}.rs` |
| Split-pane rules | `skiff-core/src/panes.rs`, `skiff-gui/src/term/split.rs`, `skiff-tui/src/ui.rs` (`split_areas`) |
| SFTP and transfers | `skiff-core/src/fs/{connect,endpoint,path,transfer}.rs` |
| Askpass | `skiff-core/src/{askpass,sshver}.rs`, `skiff-core/src/bin/askpass-shim.rs` (tests) |
| Forwards, status | `skiff-core/src/{forward,status}.rs` |
| TUI | `skiff-tui/src/{main,app,ui,widgets,keys,forms,input,sftp_tab}.rs` |
| GUI app | `skiff-gui/src/{main,app,engine,sidebar,switcher,forwards,keys,gear,os}.rs` |
| GUI terminals | `skiff-gui/src/term/` (render, input, mouse, select, search, split, announce) |
| GUI files, settings | `skiff-gui/src/files/`, `skiff-gui/src/settings/` |
| Toolkit core | `skiff-ui/src/{ui,shell,display,canvas,layout,input}.rs` |
| Toolkit text, widgets, a11y | `skiff-ui/src/text/`, `skiff-ui/src/widgets/`, `skiff-ui/src/{a11y,a11y_kit}.rs` |
| Toolkit design system | `skiff-ui/src/{style,icons,demo}.rs` |
| Test server | `skiff-testserver/src/{server,sftp_fs,fixture}.rs` |
