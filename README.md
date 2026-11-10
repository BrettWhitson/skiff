# termax

A terminal UI for SSH and file transfer, written in Rust.

- Lists hosts from `~/.ssh/config` plus your own **custom connections**, and
  **edits `~/.ssh/config` in place** (comments and formatting kept, with backups)
- Opens each connection in its own **tab**, as an interactive terminal or a dual-pane **SFTP browser**
- **Port forwards**, opt-in **reachability status**, **recent hosts**, hidden hosts
- A **command line**: `termax web`, `termax web uptime`, `termax list --json`, shell completion
- **Themes**, including built-in ones and your own, with live preview
- **Accessible**: screen readers (GUI), full keyboard use, high-contrast
  themes, interface scaling, a plain TUI mode

termax drives the system OpenSSH client (`ssh`), so `ProxyJump`, agents, keys,
`known_hosts`, `Include` and everything else in your OpenSSH setup work exactly
as they do on the command line.

## Build and run

```bash
cargo run --release
```

```bash
cargo test
```

`termax --config-path` prints where the config file lives; `termax --help`
lists every command.

## Command line

```bash
termax                       # host list
termax web                   # terminal to "web"
termax --files web           # file transfer with "web"
termax web uptime            # run a command; exits with its status
termax -t web htop           # same, with a terminal (interactive programs)
termax list                  # name<TAB>user@host:port<TAB>tags (hidden hosts: --all)
termax list --json           # the same as JSON
termax info web [--json]     # one host's details
termax --host list uptime    # a host literally named "list"
termax-gui web               # the GUI, opening "web"
```

An unknown name exits with status 2 and suggests the closest matches.
Shell completion (host names included):

```bash
eval "$(termax completion bash)"     # ~/.bashrc
eval "$(termax completion zsh)"      # ~/.zshrc
termax completion fish > ~/.config/fish/completions/termax.fish
termax completion powershell | Out-String | Invoke-Expression   # $PROFILE
```

## How it works

| Piece | Implementation |
|---|---|
| Terminal tabs | `ssh <args>` runs in a pseudo-terminal (`portable-pty`, ConPTY on Windows). Output is parsed by `vt100` and drawn with `tui-term`. |
| File transfer | `ssh -s <host> sftp` starts the SFTP subsystem over stdio, and `russh-sftp` speaks the protocol over those pipes. |
| UI | `ratatui` + `crossterm`. Background SFTP work runs on a small `tokio` runtime and reports back over a channel. |

## Config

On first run termax writes a commented template to:

- Windows: `%APPDATA%\termax\config\config.toml`
- Linux: `~/.config/termax/config.toml`
- macOS: `~/Library/Application Support/termax/config.toml`

You can override the location with `TERMAX_CONFIG=/path/to/config.toml`.

```toml
[settings]
theme = "nord"
ssh_binary = "ssh"
load_ssh_config = true
# ssh_config_path = "~/.ssh/config"
scrollback = 5000
sftp_batch_mode = true      # SFTP fails instead of prompting (see below)
show_hidden_files = false
# local_start_dir = "~/Downloads"
status_checks = "manual"    # or "on_start": check which hosts are reachable at start
confirm_multiline_paste = true  # ask before pasting several lines into a terminal

[[connection]]
name = "homelab"
host = "192.168.1.10"
user = "admin"
port = 22
identity_file = "~/.ssh/id_ed25519"
proxy_jump = "bastion"
# proxy_command = "ssh -W %h:%p bastion"
extra_args = ["-A"]
remote_dir = "/srv"
tags = ["home"]
# hidden = true             # left out of host lists (the CLI still finds it)

[[snippet]]
name = "follow logs"
command = "journalctl -f"
host = "homelab"            # optional: only in this host's terminals
run = true                  # press Enter after typing it

[[forward]]
name = "web admin"
host = "homelab"
kind = "local"              # local | remote | dynamic (SOCKS)
listen = 8080               # bind defaults to 127.0.0.1
target = "localhost:80"

[[theme]]
name = "my-theme"
bg = "#101010"
fg = "#e0e0e0"
accent = "#ff9e64"
muted = "#707070"
border = "#303030"
selection_bg = "#303030"
selection_fg = "#ffffff"
success = "#9ece6a"
warning = "#e0af68"
error = "#f7768e"
```

Saving from the app only changes the settings you edited; comments and
formatting in `config.toml` are kept.

## Host management

**Editing `~/.ssh/config`.** Hosts from your ssh config (and the files it
`Include`s) can be added, edited, duplicated, hidden, moved between files and
deleted, from the TUI (`n` / `e` / `d`) or the GUI (sidebar right-click, or
Settings → Connections). termax changes only the lines you changed: comments,
indentation, `Key=value` style, quoting, line endings and blank lines stay as
they were. Before every write it:

- refuses if the file changed on disk since it was read ("reopen to see the
  new version") — your edit in another editor always wins;
- copies the old file to `backups/<file>-<date>-<time>.bak` next to
  `config.toml` (the 10 newest are kept);
- writes a temp file and renames it over the original, so a crash never
  leaves half a file (on Unix the file keeps its permissions; a new one is 0600).

termax keeps its own extras (tags, hidden, remote directory) in one comment
inside the host's block, e.g. `# termax: tags=prod,web; hidden; dir=/srv/app`.
OpenSSH ignores it.

Some hosts are shown but left alone, with the reason: a host that shares a
`Host` line with others (`Host web db`), a host defined in more than one place,
and wildcard or `Match` blocks. New hosts go before the first wildcard
`Host *` / `Match` block, so your defaults keep applying to them.

**Status** (`p` in the TUI, "Check status" in the GUI) connects to each host's
port and reads the `SSH-` greeting — no login. It is off until you ask
(`status_checks = "on_start"` runs it at start) because probing many servers
unprompted can trip intrusion alarms. Hosts reached through `ProxyJump` /
`ProxyCommand` (including ones inherited from `Host *`) show "unknown" rather
than being probed directly.

**History.** termax remembers when you last connected to each host
(`history.toml` next to `config.toml`): sort by Recent (`o` in the TUI, the
sidebar sort menu in the GUI), and the GUI switcher lists recent hosts first.
Settings → General → Clear history forgets it.

**Port forwards** (`f` on a host in the TUI, "Port forward…" in the GUI) run
`ssh -N -o ExitOnForwardFailure=yes -L/-R/-D …` and are saved as `[[forward]]`.
In the TUI a forward runs in its own tab (close the tab to stop it); in the
GUI the Forwards tab starts and stops them and asks for passwords inline.
Forwards stop when termax exits.

Built-in themes: `terminal` (uses your terminal's colors), `midnight`, `dracula`,
`nord`, `gruvbox`, `solarized-dark`, `catppuccin`, `paper`.

## Keys

| Where | Key | Action |
|---|---|---|
| Everywhere | `Alt+1`…`Alt+9` | Switch tab |
| | `Alt+←` / `Alt+→` (or `Alt+[` / `Alt+]`) | Previous / next tab |
| | `Alt+W` | Close tab |
| | `Alt+T` | Theme picker |
| | `Alt+H` | Help |
| | `Alt+Q` | Quit |
| Hosts | `Enter` | Open terminal |
| | `s` | Open SFTP browser |
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
| | `Alt+F` | Find in the output (`Enter` older match, `Shift+Enter` newer, `Esc` close) |
| | `Alt+S` | Snippets (`Enter` sends) |
| | `Enter` after disconnect | Reconnect |
| SFTP | `Tab` / `←` `→` | Switch pane |
| | `Enter` / `Backspace` | Open directory / go up |
| | `c` or `F5` | Copy selected item to the other pane (directories copy recursively) |
| | `m` or `F7` | Make directory |
| | `R` or `F2` | Rename |
| | `d`, `Del` or `F8` | Delete (asks first) |
| | `g` | Go to path |
| | `.` / `r` / `x` | Toggle hidden files / refresh / clear finished transfers |

All other keys in a terminal tab go straight to the remote shell. Only `Alt+`
shortcuts are intercepted.

### Terminal tabs (both apps)

- **Find** searches the scrollback and screen as you type. It ignores case
  unless the query has a capital letter. The newest match is shown first;
  stepping goes to older ones, and closing returns to live output.
- **Paste warning.** Pasting text with a line break into a shell that hasn't
  turned on bracketed paste would run each line, so termax asks first
  ("Paste 3 lines into homelab?"). Programs that use bracketed paste (most
  modern shells and editors) are never interrupted. In the GUI, focus starts
  on Cancel, so a reflexive Enter doesn't paste; in the TUI press `y`. Windows
  consoles have no bracketed paste, so the TUI there treats a burst of 8+
  keys with a line break as a paste. Turn the prompt off with
  `confirm_multiline_paste = false`.
- **Mouse reporting.** When a program asks for the mouse (vim, htop, tmux,
  less with `--mouse`), clicks, drags and the wheel go to it. Hold `Shift` to
  select text instead. In the TUI the mouse is only captured while such a
  program runs, so your terminal's own selection works the rest of the time.
- **Snippets** are saved commands (`[[snippet]]` in the config, see above).
  They are listed in the GUI quick switcher ("Snippet: …") and under `Alt+S`
  in the TUI. A snippet with `host` only appears in that host's terminals. A
  multi-line snippet goes through the paste warning.

## GUI (`termax-gui`)

A native window version of termax with the same config file. It draws
everything itself (winit + softbuffer + fontdue, no web view).

```bash
cargo run -p termax-gui --release
```

- **Sidebar** lists hosts grouped by where they are defined, with a status dot
  and first tag (hover a host for its address, status and when you last used
  it). The header has the filter, Name/Recent sort and a "⋯" menu (new host,
  check status of all, show hidden hosts, port forwards). Right-click a host to
  open a terminal or file transfer, forward a port, check its status, edit,
  duplicate, move it to another ssh config file, hide or delete it.
- **Tabs** hold terminals, file transfers and settings. Drag a tab to move it.
- **File transfer** shows two panes; each can be Local or any host, so
  remote↔remote copies work too. Drag files between panes (or from Explorer /
  Finder onto a pane), or select and press `F5` / `Copy →`. Transfers list
  progress, ask before overwriting, and can be cancelled or retried.
- **Passwords** are asked for inline in the pane that needs them and are never
  stored. This uses `SSH_ASKPASS_REQUIRE`, so it needs OpenSSH 8.4 or newer;
  with an older `ssh`, use keys or an agent.
- **Settings** (`Ctrl+Shift+,`) edits connections, theme, fonts, keybindings
  and general options. Under Appearance, "Choose terminal font…" and "Choose
  interface font…" list the installed fonts (the terminal list shows only
  monospace ones; per-user installed fonts included), with a filter box;
  picking one applies it at once. Edits to `config.toml` made outside termax are picked
  up within a second while the window has focus.

| Action | Windows / Linux | macOS |
|---|---|---|
| Quick switcher (hosts, tabs, themes, commands) | `Ctrl+Shift+K`, `Ctrl+K`* | `Cmd+K` |
| New tab | `Ctrl+Shift+T` | `Cmd+T` |
| Close tab | `Ctrl+Shift+W` | `Cmd+W` |
| Next / previous tab | `Ctrl+Tab` / `Ctrl+Shift+Tab`, `Ctrl+PgDn` / `Ctrl+PgUp` | `Ctrl+Tab` / `Ctrl+Shift+Tab`, `Cmd+Shift+]` / `Cmd+Shift+[` |
| Copy / paste in a terminal | `Ctrl+Shift+C` / `Ctrl+Shift+V` | `Cmd+C` / `Cmd+V` |
| Find in a terminal (`Enter` / `F3` older, `Shift+Enter` / `Shift+F3` newer, `Esc` close) | `Ctrl+Shift+F` | `Cmd+F` |
| Zoom in / out / reset | `Ctrl+Shift+=` / `Ctrl+Shift+-` / `Ctrl+Shift+0` | `Cmd+=` / `Cmd+-` / `Cmd+0` |
| Toggle sidebar | `Ctrl+Shift+B` | `Cmd+B` |
| Settings | `Ctrl+Shift+,` | `Cmd+,` |

\* Plain `Ctrl`+letter belongs to the shell inside a terminal tab, so `Ctrl+K`
opens the switcher only outside terminals.

GUI-only config:

```toml
[gui]
font_path = "C:/Windows/Fonts/consola.ttf"   # terminal font file (optional)
ui_font_path = "C:/Windows/Fonts/segoeui.ttf" # interface font file (optional)
font_size = 14.0                              # terminal font size
ui_scale = 1.0                                # interface scale (0.5 - 3)
announce_output = true                        # read new terminal output aloud: true, false or "focused"
cursor_blink = true                           # blinking terminal cursor

[keys]
# action = "chord" or "chord1, chord2"; actions: quick_switcher, new_tab,
# close_tab, next_tab, prev_tab, copy, paste, find, zoom_in, zoom_out,
# zoom_reset, toggle_sidebar, settings, ui_scale_up, ui_scale_down,
# ui_scale_reset
close_tab = "ctrl+shift+q"
```

Window size and sidebar width are kept in `state.toml` next to `config.toml`.

### Trying it without a real server

`termax-testserver` runs a throwaway SSH server on 127.0.0.1 (for testing
only). It prints the `ssh` options it needs:

```bash
cargo run -p termax-testserver -- --shell --password secret
```

Point termax at a scratch config so nothing touches your real hosts:

```toml
# scratch.toml (run with TERMAX_CONFIG=scratch.toml)
[settings]
load_ssh_config = false

[[connection]]
name = "testserver"
host = "127.0.0.1"
# everything the server printed between `ssh` and `127.0.0.1`, one item each:
extra_args = ["-F", "C:/.../ssh_config", "-o", "UserKnownHostsFile=C:/.../known_hosts", "...", "-p", "PORT"]
```

`cargo run -p termax-gui --release --example flood -- 50` prints 50 MB of mixed
output; run it in a terminal tab to check that typing stays responsive.

Performance checks (release build):

```bash
cargo test -p termax-gui --release --test perf -- --ignored --nocapture
```

## Accessibility

**Screen readers (GUI).** termax-gui exposes its interface through
[AccessKit](https://accesskit.dev): Narrator, NVDA and JAWS on Windows,
VoiceOver on macOS and Orca on Linux see every control with its role, name and
state. Hosts are read as "web, online 23 ms, ops@10.0.0.5". A terminal tab is a
terminal whose text the screen reader can read, and new output is announced
politely: at most once every half second, the last 20 lines (2,000 characters),
without the echo of what you typed. `announce_output = "focused"` limits that to
the focused tab and `false` turns it off. Activating a host (the screen
reader's default action twice, or Enter) opens it. The tree is only built while
a screen reader is connected. Tested with the offscreen tree here; please report
anything a screen reader gets wrong.

**Keyboard.** Everything works without a mouse:

| Key | Does |
|---|---|
| `F6` / `Shift+F6` | Next / previous area: hosts, tabs, content (also out of a terminal) |
| `Tab` / `Shift+Tab` | Next / previous control |
| `↑` `↓` `PgUp` `PgDn` `Home` `End` | Move in lists; `←` `→` switch tabs on the tab strip |
| `Enter` | Open the selected row; `Enter` / `Space` press buttons and checkboxes |
| `Shift+F10` or the Menu key | Context menu of the selected host, file or tab |
| `Delete` | Delete the selected host (asks first) |
| `Ctrl+Alt+=` / `-` / `0` | Larger / smaller / normal interface (`Cmd+Alt` on macOS) |
| `Ctrl+Shift+F6` | Send F6 to the program in the terminal |

The focused control gets a 2 px ring in the accent color while you move with
the keyboard.

**Contrast.** `theme = "high-contrast"` (black) and `"high-contrast-light"`
(white) reach at least 7:1 for text. Every built-in theme reaches 4.5:1 for
text (WCAG AA); a test checks this. On Windows, `theme = "auto"` (or the
default) switches to `high-contrast` while Windows high contrast is on.
`cursor_blink = false` (or turning off Windows animation effects) stops the
cursor blinking.

**TUI.** The TUI runs in your terminal, whose screen reader support applies.
termax keeps the terminal cursor on the selected row of every list, so readers
that follow the cursor read the selection; the selection is also marked with
`›`, and messages start with `Error:` or `Info:`. `termax --plain` (or
`tui_plain = true` under `[settings]`) drops borders and turns status symbols
into words.

## Known limitations / next steps

These apply to the terminal UI (`termax`); the GUI has mouse support,
password prompts, cancellable transfers, overwrite prompts and remappable keys.


- **SFTP needs non-interactive auth.** The SFTP channel has no terminal, so with
  `sftp_batch_mode = true` it uses keys or an agent and fails instead of prompting
  for a password. A future step is an `SSH_ASKPASS` helper that prompts inside termax.
- Keybindings are fixed. A `[keys]` config section would make them remappable.
- The status check never probes through jump hosts (those show "unknown").
- The mouse goes only to programs that ask for it (no tab clicks or own selection).
- Transfers can't be cancelled, and there's no overwrite confirmation.
- The scrollback is view-only (search scrolls to matches but doesn't highlight them).
- Split panes (two sessions side by side in one tab) aren't supported.
