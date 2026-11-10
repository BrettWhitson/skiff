# Skiff

An SSH and SFTP client written in Rust, with a terminal UI (`skiff`) and a
native desktop GUI (`skiff-gui`) that share one config.

Skiff drives the system OpenSSH client (`ssh`), so `ProxyJump`, agents, keys,
`known_hosts`, `Include` and everything else in your OpenSSH setup work
exactly as they do on the command line.

## Features

- **Hosts** from `~/.ssh/config` plus your own custom connections, with a
  fuzzy filter, tags, hidden hosts and "Recent" sorting.
- **Edits `~/.ssh/config` in place**: add, edit, duplicate, move and delete
  hosts, with comments and formatting kept, a conflict check and backups.
- **Tabs** for interactive terminals (with split panes, find, snippets and a
  multi-line paste warning) and dual-pane **file transfer** (local↔remote and
  remote↔remote, drag and drop in the GUI, safe overwrite and cancel).
- **Passwords and host-key questions** answered inside the app for SFTP and
  forwards, never stored.
- **Port forwards** (local, remote, SOCKS), saved and started from the app.
- Opt-in **reachability status** that only reads the SSH greeting.
- **Command line**: `skiff web`, `skiff web uptime`, `skiff list --json`,
  shell completion.
- **Themes**: ten built-in (two high-contrast) plus your own, with live preview.
- **Accessible**: screen readers in the GUI (AccessKit), full keyboard use,
  WCAG-checked contrast, interface scaling, a plain TUI mode.

## Install

From a clone of this repository:

```bash
cargo install --path crates/skiff-tui    # the `skiff` command (TUI and CLI)
cargo install --path crates/skiff-gui    # the `skiff-gui` app
```

`cargo install` puts the programs in Cargo's bin folder (`~/.cargo/bin`, or
`%USERPROFILE%\.cargo\bin` on Windows), which the Rust installer adds to
`PATH`, so `skiff` and `skiff-gui` work from any terminal. Re-run the same
commands to update. To try it without installing:

```bash
cargo run -p skiff-ssh --release     # TUI
cargo run -p skiff-gui --release     # GUI
```

On Windows the `.exe` files carry the Skiff icon. On Linux, add a menu
entry with the icon (per user; use `/usr/share` instead of
`~/.local/share` for everyone):

```bash
cp -r assets/linux/icons ~/.local/share/
cp assets/linux/skiff.desktop ~/.local/share/applications/
gtk-update-icon-cache ~/.local/share/icons/hicolor 2>/dev/null || true
```

On macOS, `assets/icon/skiff.icns` is ready for an app bundle. The icon
files are generated from one definition; after changing it run
`cargo run -p skiff-ui --release --example gen_icons`.

Skiff isn't on crates.io yet. Once it is, the TUI will install with
`cargo install skiff-ssh` (the name `skiff` there belongs to an unrelated
project).

Requirements: the OpenSSH client on `PATH`. Answering passwords inside the
app for SFTP and forwards needs OpenSSH 8.4 or newer; with an older `ssh`,
use keys or an agent. Skiff was called termax before; your old settings are
copied over on first start (see [Coming from termax](docs/CONFIGURATION.md#coming-from-termax)
and the [changelog](CHANGELOG.md)).

## Quick start

**TUI.** Run `skiff`. Your ssh config hosts are listed; `Enter` opens a
terminal, `s` a file browser, `/` filters, `n` adds a host, `?` shows
every key. Everywhere else, shortcuts start with `Alt+O` (then `1`…`9` for a
tab, `w` to close it, `?` for help); `Alt+[` / `Alt+]` switch tabs.

**GUI.** Run `skiff-gui`. Pick a host in the sidebar (right-click for file
transfer, forwards and editing), or press `Ctrl+P` (`Cmd+P`; in a terminal
tab `Ctrl+Shift+P`) for the command palette: type to search
hosts, tabs and commands, or start with `>` for commands, `@` for hosts,
`:` for port forwards and `#` for snippets. `Ctrl+Shift+,` (or the gear at
the right end of the tab strip) opens Settings. Notices and errors appear
as toasts at the bottom right; errors stay until closed.

**Command line.**

```bash
skiff                       # host list
skiff web                   # terminal to "web"
skiff --files web           # file transfer with "web"
skiff web uptime            # run a command; exits with its status
skiff -t web htop           # same, with a terminal (interactive programs)
skiff list                  # name<TAB>user@host:port<TAB>tags (hidden hosts: --all)
skiff list --json           # the same as JSON
skiff info web [--json]     # one host's details
skiff --host list uptime    # a host literally named "list"
skiff --plain               # TUI without borders and symbols
skiff --config-path         # where config.toml lives
skiff-gui web               # the GUI, opening "web"
skiff-gui --files web       # the GUI, opening a file transfer with "web"
```

An unknown name exits with status 2 and suggests the closest matches.
Shell completion (host names included):

```bash
eval "$(skiff completion bash)"     # ~/.bashrc
eval "$(skiff completion zsh)"      # ~/.zshrc
skiff completion fish > ~/.config/fish/completions/skiff.fish
skiff completion powershell | Out-String | Invoke-Expression   # $PROFILE
```

## Configuration

One `config.toml` serves both apps (`%APPDATA%\skiff\config\` on Windows,
`~/.config/skiff/` on Linux, `~/Library/Application Support/skiff/` on
macOS; `SKIFF_CONFIG` overrides it). Saving from the app only changes what
you edited.

```toml
[settings]
theme = "nord"
status_checks = "manual"    # or "on_start"

[[connection]]
name = "homelab"
host = "192.168.1.10"
user = "admin"
tags = ["home"]

[[snippet]]
name = "follow logs"
command = "journalctl -f"
run = true

[[forward]]
host = "homelab"
kind = "local"              # local | remote | dynamic (SOCKS)
listen = 8080
target = "localhost:80"
```

Every key, the `[gui]` options, custom themes, key remapping (`[keys]`,
`[tui_keys]`) and the rules for editing ssh config files are in
**[docs/CONFIGURATION.md](docs/CONFIGURATION.md)**.

## Using Skiff

**Terminals.** All keys go to the remote shell except the app's shortcuts
(by default `Alt+O` and a key in the TUI, `Ctrl+Shift+` chords in the GUI).

- **Find** searches scrollback and screen as you type; case-insensitive
  unless the query has a capital. Matches stay on their text as output scrolls.
- **Split panes**: each pane is its own session to the same host, and
  sessions keep running through every move.
  - *TUI*: two panes per tab. `Alt+O` then an arrow moves the divider between
    25 / 33 / 50 / 67 / 75%, `Alt+O =` equalizes, `Alt+O x` swaps. With
    `tui_mouse = "always"` drag the divider line (or, stacked, the lower
    pane's title rule); it snaps to those shares (hold `Alt` to place it
    freely). A pane keeps at least 10 columns by 3 rows. A lone terminal has
    no frame at all; split panes get a one-line title rule, heavy and in the
    accent color on the focused pane.
  - *GUI*: split any pane as often as you like. Drag a divider (it snaps to
    the same shares, `Alt` places it freely, double-click equalizes). Drag a
    pane by its grip (the dots at its top centre) or with `Alt+Shift`+drag
    (plain `Alt`+click stays the terminal's) onto another pane: the left, right, top or bottom half splits that pane there,
    the centre swaps the two, and the tab strip gives it a tab of its own.
    Drag a terminal tab onto a pane's edge to join it into that tab. `Esc`
    cancels a drag.
- **Paste warning**: text with a line break pasted into a shell without
  bracketed paste would run each line, so Skiff asks first
  (`confirm_multiline_paste = false` turns it off).
- **Mouse reporting**: programs that ask for the mouse (vim, htop, tmux) get
  clicks and the wheel; hold `Shift` to select text instead. In the TUI,
  `tui_mouse = "always"` gives Skiff the mouse for tab clicks, wheel
  scrollback and selection (copied with OSC 52).
- **Snippets** (`[[snippet]]`) are listed under `Alt+O s` in the TUI and in the
  GUI command palette (`#`); a snippet with `host` only appears in that host's
  terminals.

**File transfer.** Two panes: in the TUI local and remote, in the GUI each
side Local or any host. Copies run in the background with progress, ask
before overwriting (once, or for all) and can be cancelled or retried.
A copy is written to `.<name>.<id>.skiff-part` and moved into place only
when complete, so cancelling never damages the file being overwritten; a
leftover `*.skiff-part` only appears if Skiff was killed mid-copy and is
safe to delete. Details: [ARCHITECTURE.md](docs/ARCHITECTURE.md#sftp-engine).
In the GUI each pane has a breadcrumb path (click a folder, or click the
current one or press `Ctrl+L` to type a path), Name / Size / Modified
column headers that sort, type-ahead (type a name to select it) and a
footer with the item count and selection size; copies show in a
collapsible drawer under the panes with a progress bar, rate and time left
per file, and a summary such as "2 active · 1 waiting · 1 failed". An
overwrite question opens the drawer and waits there for Overwrite or Skip.

**Passwords.** SFTP and forwards have no terminal, so ssh asks Skiff
(`SSH_ASKPASS`): a dialog in the TUI (`y` / `n` for host keys, `Esc`
cancels), inline in the GUI pane. A question opens only on its own tab; other
tabs are flagged. Security-key "touch" notices show on the status line.

**Port forwards** (`f` in the TUI, "Port forward…" in the GUI) run
`ssh -N -L/-R/-D …` and are saved as `[[forward]]`. In the TUI a forward is
a tab (close it to stop); the GUI's Forwards tab lists them with their
status and starts or stops each one, with Edit and Delete in its "More
actions" menu. Forwards stop when Skiff exits, even if it is killed (best
effort on macOS).

**Status** (`p` in the TUI, "Check status" in the GUI) connects to each
host's port and reads the `SSH-` greeting, never logging in. It runs only
when asked, since probing many servers can trip intrusion alarms.

**History.** Skiff remembers when you last used each host for Recent
sorting (`o` in the TUI, the sidebar sort menu in the GUI); Settings →
General → Clear history forgets it.

## Keys

### TUI

Shortcuts are two presses: `Alt+O`, then a key (written `Alt+O, w`; Alt
may stay held). Bash's readline binds nearly every other Alt key, so they
stay the shell's. The footer shows the tab's own keys on the left and the
keys after the prefix on the right, behind an `Alt+O ▸` badge (`Alt+O ▸ 1…9
tab  w close …`); on a narrow screen the less used hints drop first, `? help`
never. After `Alt+O` a `PREFIX` badge appears at once, and after a short
pause (`which_key_delay_ms`, 300 ms) a box lists every key that can follow.
`Alt+O` twice sends `Alt+O` itself; `Esc` cancels. On the host list and in
file tabs, `?` shows all keys and `q` quits / closes the tab.

| Where | Key | Action |
|---|---|---|
| Everywhere | `Alt+O` `1`…`9` | Switch tab |
| | `Alt+[` / `Alt+]` (or `Alt+O` `p` / `n`) | Previous / next tab |
| | `Alt+O` `w` | Close tab (in a split tab: the focused pane) |
| | `Alt+O` `t` / `?` (or `h`) / `q` | Theme picker / help / quit |
| Hosts | `Enter` / `s` | Open terminal / SFTP browser |
| | `/` | Fuzzy filter (name, host, user, tags) |
| | `n` / `e` / `d` | New / edit / delete host (ssh config hosts too) |
| | `p` | Check which hosts are reachable |
| | `o` | Sort by name / most recent |
| | `H` | Show / hide hidden hosts |
| | `f` / `F` | Forward a port / saved forwards |
| | `r` | Reload config files |
| Host editor | `Tab` / `↑` `↓`, `Ctrl+S` | Move between fields, save |
| | `Space` | Toggle "Save to" / Hidden |
| | `Enter` in Other options | Add a line |
| Terminal | `Shift+PgUp` / `Shift+PgDn` | Scrollback |
| | `Alt+O` `/` | Find (`Enter` older match, `Shift+Enter` newer, `Esc` close) |
| | `Alt+O` `s` | Snippets (`Enter` sends) |
| | `Alt+O` `\|` / `-` | Split side by side / one above the other |
| | `Alt+O` `o` | Focus the other pane |
| | `Alt+O` `x` | Swap the two panes |
| | `Alt+O` `←` `→` `↑` `↓` | Move the divider to the next of 25 / 33 / 50 / 67 / 75% |
| | `Alt+O` `=` | Make the panes equal |
| | `Enter` after disconnect | Reconnect |
| SFTP | `Tab` / `←` `→` | Switch pane |
| | `Enter` / `Backspace` | Open directory / go up |
| | `c` or `F5` | Copy to the other pane (directories recursively) |
| | `m` or `F7` | Make directory |
| | `R` or `F2` | Rename |
| | `d`, `Del` or `F8` | Delete (asks first) |
| | `g` | Go to path |
| | `X` | Cancel the newest running transfer (again for the next) |
| | `.` / `r` / `x` | Toggle hidden files / refresh / clear finished transfers |
| | `q` / `?` | Close the file tab / help |
| File exists | `o` / `s` / `a` / `S` | Overwrite / skip / overwrite all / skip all |
| | `c` or `Esc` | Cancel the transfer |

The global shortcuts can be remapped in `[tui_keys]`, including back to
single Alt chords; every other key goes to the shell. termax's `Alt+` chords
now reach the shell (readline word movement, digit arguments, completion);
see [Coming from termax](docs/CONFIGURATION.md#coming-from-termax) to put
them back.

### GUI

| Action | Windows / Linux | macOS |
|---|---|---|
| Command palette (hosts, tabs, commands, forwards, snippets, themes) | `Ctrl+P`*, `Ctrl+Shift+P` | `Cmd+P`, `Cmd+Shift+P` |
| New tab / close tab | `Ctrl+Shift+T` / `Ctrl+Shift+W` | `Cmd+T` / `Cmd+W` |
| Next / previous tab | `Ctrl+Tab` / `Ctrl+Shift+Tab`, `Ctrl+PgDn` / `Ctrl+PgUp` | `Ctrl+Tab` / `Ctrl+Shift+Tab`, `Cmd+Shift+]` / `Cmd+Shift+[` |
| Copy / paste in a terminal | `Ctrl+Shift+C` / `Ctrl+Shift+V` | `Cmd+C` / `Cmd+V` |
| Find in a terminal (`Enter` / `F3` older, `Shift+Enter` / `Shift+F3` newer) | `Ctrl+Shift+F` | `Cmd+F` |
| Terminal zoom in / out / reset | `Ctrl+Shift+=` / `-` / `0` | `Cmd+=` / `-` / `0` |
| Interface larger / smaller / normal | `Ctrl+Alt+=` / `-` / `0` | `Cmd+Alt+=` / `-` / `0` |
| Toggle sidebar | `Ctrl+Shift+B` | `Cmd+B` |
| Settings | `Ctrl+Shift+,` | `Cmd+,` |
| Split pane right / down | `Ctrl+Shift+D` / `Ctrl+Shift+E` | `Cmd+D` / `Cmd+Shift+D` |
| Next pane | `Ctrl+Shift+O` (or `F6`) | `Cmd+]` (or `F6`) |
| Close pane | `Ctrl+Shift+X` | `Cmd+Alt+W` |
| Swap focused pane with its neighbour | `Ctrl+Alt+Shift+←` `→` `↑` `↓` | `Cmd+Ctrl+Shift+←` `→` `↑` `↓` |
| Move the divider next to the focused pane (next snap point) | `Alt+Shift+←` `→` `↑` `↓` | `Cmd+Ctrl+←` `→` `↑` `↓` |
| Equalize panes | `Alt+Shift+=` | `Cmd+Ctrl+=` |

\* Plain `Ctrl`+letter belongs to the shell inside a terminal, so `Ctrl+P`
stays the shell's previous-history key there and `Ctrl+Shift+P` opens the
palette from a terminal tab (`Cmd+P` works everywhere on macOS). All of these can be changed in
Settings → Keybindings or `[keys]`.

Keyboard navigation (no mouse needed):

| Key | Does |
|---|---|
| `F6` / `Shift+F6` | Next / previous area: sidebar divider, hosts, tabs, content; each split pane, then the pane dividers |
| `Tab` / `Shift+Tab` | Next / previous control within the area |
| `↑` `↓` `PgUp` `PgDn` `Home` `End` | Move in lists; `←` `→` switch tabs on the tab strip |
| Arrow keys on a pane divider | Move the divider to the next snap point (25 / 33 / 50 / 67 / 75%, then 5% per press) |
| `←` `→` on the sidebar divider | Narrow / widen the sidebar |
| `Enter` / `Space` | Open the selected row; press buttons and checkboxes |
| `Shift+F10` or the Menu key | Context menu of the selected host, file or tab |
| A shortcut shown in an open menu | Chooses that item (e.g. `Del` in a host's menu) |
| `Delete` | Delete the selected host or file (asks first) |
| `Ctrl+L` | Type a path in the focused file pane |
| Letters in a file list | Select the first name that starts with them |
| `Ctrl+Shift+F6` | Send F6 to the program in the terminal |

## Accessibility

- **Screen readers (GUI).** Through [AccessKit](https://accesskit.dev),
  Narrator, NVDA and JAWS (Windows), VoiceOver (macOS) and Orca (Linux) see
  every control with its role, name and state. New terminal output is
  announced politely (at most every half second, without the echo of what
  you typed); `announce_output = "focused"` or `false` limits it. The tree
  is built only while a screen reader is connected.
- **Focus** shows as a 2 px accent ring, and the current row of a focused
  list has a 3 px bar, so focus never relies on color alone.
- **Contrast.** Every built-in theme reaches 4.5:1 for text (WCAG AA);
  `high-contrast` and `high-contrast-light` reach 7:1. Custom themes are
  adjusted toward readable contrast. On Windows, `theme = "auto"` follows
  the system high-contrast setting.
- **Size and motion.** `ui_scale` (or `Ctrl+Alt+=` / `-`) scales the whole
  interface, with Windows' text size applied on top; `cursor_blink = false`
  or Windows' animation setting stops blinking.
- **TUI.** The terminal cursor sits on the selected row so screen readers
  that follow it read the selection; messages start with `Error:` (or, in
  plain mode, `Info:`). `skiff --plain` (or `tui_plain = true`) drops
  borders and turns symbols into words. With `NO_COLOR` set the TUI uses no colors (bold and reverse
  video only), and `tui_ascii_borders = true` draws `+ - |` boxes for fonts
  without box-drawing characters.

Please report anything a screen reader gets wrong.

## Documentation

- [docs/CONFIGURATION.md](docs/CONFIGURATION.md): every config key with
  defaults, themes, key remapping, ssh config editing, environment variables.
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md): crates, data flow,
  threading, the GUI toolkit and accessibility design, key files.
- [docs/CONTRIBUTING.md](docs/CONTRIBUTING.md): setup, build, tests, style,
  and how to add a keybinding, setting or theme.
- [CHANGELOG.md](CHANGELOG.md): what changed in each release, including the
  rename from termax and the moved TUI keys.

## Known limitations

- The status check can't see past a jump host: for `ProxyJump` hosts it
  reports only whether the first jump host answers, and `ProxyCommand` hosts
  show "unknown".
- Answering prompts inside the app needs OpenSSH 8.4+ (`SSH_ASKPASS_REQUIRE`).
- On macOS, a forward can outlive a killed Skiff until ssh notices.
- A TUI tab holds at most two panes; port-forward tabs can't be split.
- Windows consoles have no bracketed paste, so the TUI there treats a burst
  of 8+ keys with a line break as a paste.
- Some ssh config hosts can't be edited in place (shared `Host` lines,
  hosts defined twice, wildcard and `Match` blocks); Skiff shows why.
