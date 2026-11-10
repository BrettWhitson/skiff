# Configuration reference

Skiff (both the `skiff` TUI and `skiff-gui`) reads one TOML file,
`config.toml`. Every key is optional: a missing key or section takes the
default listed here. This page is derived from
`crates/skiff-core/src/config.rs` and the modules it points to.

## Where the files live

| Platform | `config.toml` |
|---|---|
| Windows | `%APPDATA%\skiff\config\config.toml` |
| Linux | `~/.config/skiff/config.toml` |
| macOS | `~/Library/Application Support/skiff/config.toml` |

`skiff --config-path` prints the path in use. On first run Skiff writes a
commented template there.

Next to `config.toml` Skiff also keeps:

| File | Contents |
|---|---|
| `state.toml` | GUI window size, sidebar width and collapsed state, host sort mode, "show hidden hosts". Kept apart so dragging a splitter never rewrites your config. |
| `history.toml` | When each host was last connected, how often and how (terminal, files, forward). Drives "Recent" sorting and the switcher's recent hosts. Settings → General → Clear history empties it. |
| `backups/` | Copies of ssh config files taken before each edit (see [ssh config editing](#ssh-config-editing)). |

Saving from the app rewrites only the keys you changed: comments,
formatting and keys Skiff doesn't know are kept. The GUI notices outside
edits to `config.toml` within about a second while its window has focus;
the TUI reloads with `r`.

### Environment variables

| Variable | Effect |
|---|---|
| `SKIFF_CONFIG` | Path of the config file to use instead of the platform default (`state.toml`, `history.toml` and `backups/` move with it). Ignored when empty. |
| `TERMAX_CONFIG` | Pre-rename name of `SKIFF_CONFIG`; still honoured when `SKIFF_CONFIG` is unset. |
| `SKIFF_DEBUG_PTY` | When set, every chunk read from a terminal's pty is printed to stderr (debugging only). |
| `SKIFF_TEST_TRACE` | Test-only: GUI integration tests log pane states while waiting. |
| `TERM` | If unset, terminal sessions get `TERM=xterm-256color`. |

Skiff sets `SSH_ASKPASS`, `SSH_ASKPASS_REQUIRE=force`,
`SKIFF_ASKPASS_ENDPOINT`, `SKIFF_ASKPASS_TAG` and `SKIFF_ASKPASS_TOKEN` on
the `ssh` processes it starts for SFTP and forwards; they are internal and
not meant to be set by hand.

### Coming from termax

Skiff was called termax. On its first start, when the Skiff folder doesn't
exist yet and the old termax one does, Skiff copies everything in it
(config, history, window state, ssh config backups) into its own folder and
says so once. The old folder is left as it was and nothing that already
exists is overwritten. This is skipped when `SKIFF_CONFIG` or
`TERMAX_CONFIG` is set.

| Old (termax) | New (Skiff) |
|---|---|
| `%APPDATA%\termax\config\` | `%APPDATA%\skiff\config\` |
| `~/.config/termax/` | `~/.config/skiff/` |
| `~/Library/Application Support/termax/` | `~/Library/Application Support/skiff/` |
| `TERMAX_CONFIG` | `SKIFF_CONFIG` (`TERMAX_CONFIG` still works in this release) |
| `# termax:` comments in ssh config | `# skiff:` (old ones are read, and rewritten when the host is edited) |
| `*.termax-part` leftovers | `*.skiff-part` (both are skipped when copying folders) |

## `[settings]`

Shared by both front-ends.

| Key | Default | Meaning |
|---|---|---|
| `theme` | `"terminal"` | Theme name: a built-in, a `[[theme]]` of yours, or `"auto"`. See [Themes](#themes). |
| `ssh_binary` | `"ssh"` | The OpenSSH client to run. |
| `load_ssh_config` | `true` | List the hosts from your ssh config. |
| `ssh_config_path` | `~/.ssh/config` | Use another ssh config file. `~` is expanded. |
| `scrollback` | `5000` | Lines of scrollback per terminal. |
| `sftp_batch_mode` | `true` | Only matters when Skiff can't answer ssh's prompts itself (OpenSSH older than 8.4): SFTP connections get `-o BatchMode=yes` and fail at once with ssh's message instead of hanging on a prompt. |
| `show_hidden_files` | `false` | Show dot-files in file panes. |
| `local_start_dir` | home directory | Folder the local file pane starts in. `~` is expanded. |
| `status_checks` | `"manual"` | `"manual"`: reachability is checked only when you ask. `"on_start"`: also once at start. |
| `tui_plain` | `false` | TUI without borders, and with words instead of status symbols (for screen readers). `skiff --plain` does the same for one run. |
| `confirm_multiline_paste` | `true` | Ask before pasting text with a line break into a program that hasn't turned on bracketed paste. |
| `tui_mouse` | `"auto"` | TUI only. `"auto"`: take the mouse only while a program in the active terminal asks for it. `"always"`: always take it (tab clicks, wheel scrollback, Skiff's own selection copied over OSC 52). |

```toml
[settings]
theme = "nord"
ssh_binary = "ssh"
load_ssh_config = true
# ssh_config_path = "~/.ssh/config"
scrollback = 5000
sftp_batch_mode = true
show_hidden_files = false
# local_start_dir = "~/Downloads"
status_checks = "manual"
tui_plain = false
confirm_multiline_paste = true
tui_mouse = "auto"
```

## `[gui]`

Read only by `skiff-gui`; the TUI ignores this section.

| Key | Default | Meaning |
|---|---|---|
| `font_path` | bundled/system default | Terminal (monospace) font file. Settings → Appearance → "Choose terminal font…" fills it in. |
| `ui_font_path` | bundled/system default | Interface font file. |
| `font_size` | `14.0` | Terminal font size in points (clamped to 6–72). |
| `ui_scale` | `1.0` | Interface scale (clamped to 0.5–3). Windows' text-size setting is applied on top. |
| `announce_output` | `true` | New terminal output read by screen readers: `true` (all terminals), `"focused"` (only the focused one) or `false`. |
| `cursor_blink` | `true` | Blinking text and terminal cursors. Windows' "animation effects" off also stops blinking. |
| `font_family` | unset | Accepted but currently unused. |

```toml
[gui]
font_path = "C:/Windows/Fonts/consola.ttf"
ui_font_path = "C:/Windows/Fonts/segoeui.ttf"
font_size = 14.0
ui_scale = 1.0
announce_output = true
cursor_blink = true
```

## `[[connection]]`

Your own hosts, listed before the ones from ssh config. Each runs as
`ssh` with the matching options. The TUI (`n` / `e`) and GUI (Settings →
Connections) edit these.

| Key | Required | Meaning |
|---|---|---|
| `name` | yes | Name shown in lists and used on the command line (`skiff <name>`). |
| `host` | yes | Host name or address. |
| `user` | no | Login user (`-l`). |
| `port` | no | Port (`-p`), 1–65535. |
| `identity_file` | no | Private key (`-i`). |
| `proxy_jump` | no | Jump host (`-J`). |
| `proxy_command` | no | `ProxyCommand`. |
| `extra_args` | no | More ssh arguments, one list item each. |
| `remote_dir` | no | Folder the remote file pane starts in. |
| `tags` | no | Tags, shown in lists and matched by the filter. |
| `hidden` | no | `true` leaves the host out of host lists (the CLI still finds it). |

```toml
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
# hidden = true
```

Hosts from ssh config keep these extras (tags, hidden, remote directory) in
a comment inside their block instead; see [ssh config editing](#ssh-config-editing).

## `[[snippet]]`

Saved commands, sent to a terminal from the GUI quick switcher
("Snippet: …") or the TUI's `Alt+S` list. GUI: Settings → Snippets.

| Key | Default | Meaning |
|---|---|---|
| `name` | required | Name shown in the list (unique in the GUI editor). |
| `command` | required | Text to type; may span several lines (then it goes through the paste warning). |
| `host` | unset | Offer the snippet only in this host's terminals. |
| `run` | `false` | Press Enter after typing it. |

```toml
[[snippet]]
name = "follow logs"
command = "journalctl -f"
host = "homelab"
run = true
```

## `[[forward]]`

Saved port forwards. Each runs as
`ssh -N -o ExitOnForwardFailure=yes -L|-R|-D <spec> <host>`.

| Key | Default | Meaning |
|---|---|---|
| `name` | the host | Display name. |
| `host` | required | Host name from the host list. |
| `kind` | `"local"` | `"local"` (`-L`), `"remote"` (`-R`) or `"dynamic"` (`-D`, a SOCKS proxy). |
| `bind` | `127.0.0.1` (remote: the server's default) | Address to listen on. |
| `listen` | required | Port to listen on. |
| `target` | — | `host:port` to connect to (`[ipv6]:port` works); not used by `dynamic`. |

```toml
[[forward]]
name = "web admin"
host = "homelab"
kind = "local"
listen = 8080
target = "localhost:80"
```

Renaming a host also renames it in saved forwards.

## Themes

`theme` under `[settings]` picks one of:

- Built-in: `terminal` (uses your terminal's colors in the TUI), `midnight`,
  `dracula`, `nord`, `gruvbox`, `solarized-dark`, `catppuccin`, `paper`,
  `high-contrast` (black, at least 7:1), `high-contrast-light` (white, at
  least 7:1).
- `"auto"`: `terminal`, except that the GUI on Windows uses `high-contrast`
  while Windows high contrast is on. The default `"terminal"` behaves the
  same way in the GUI.
- A `[[theme]]` of your own. A custom theme with a built-in's name replaces it.

```toml
[[theme]]
name = "my-theme"
bg = "#101010"            # window / page background
fg = "#e0e0e0"            # normal text
accent = "#ff9e64"        # focus, active tab, headings
muted = "#707070"         # secondary text
border = "#303030"        # borders and separators
selection_bg = "#303030"  # selected row background
selection_fg = "#ffffff"  # selected row text
success = "#9ece6a"
warning = "#e0af68"
error = "#f7768e"
# Optional 16-color terminal palette (black, red, green, yellow, blue,
# magenta, cyan, white, then the bright variants). The GUI uses a default
# palette when it is missing.
# ansi = ["#000000", "#cd3131", ...]
```

All ten UI colors are required. A color is `"#rrggbb"`, a name (`black`,
`red`, `green`, `yellow`, `blue`, `magenta`, `cyan`, `gray`, `darkgray`,
`lightred`, `lightgreen`, `lightyellow`, `lightblue`, `lightmagenta`,
`lightcyan`, `white`; case, `-`, `_` and spaces are ignored, `grey` works),
an ANSI index `0`–`255`, or `"reset"` (the terminal's own color, TUI only).

The GUI adjusts custom themes for readability: dimmed and status colors move
toward the text color, and the selection toward the background, until they
reach the contrast targets (4.5:1 for text, 3:1 for outlines), while dimmed
text stays visibly dimmer. A theme whose colors are too close for that gets
a warning.

## `[keys]` (GUI shortcuts)

Each entry is `action = "chord"` or several chords separated by commas.
A chord is modifiers and a key joined by `+`:

- Modifiers: `ctrl` (`control`), `shift`, `alt` (`option`, `opt`), `cmd`
  (`command`, `super`, `win`, `logo`, `meta`).
- Keys: a single character, `tab`, `enter` (`return`), `esc` (`escape`),
  `space`, `up`, `down`, `left`, `right`, `home`, `end`, `pageup` (`pgup`),
  `pagedown` (`pgdn`), `insert` (`ins`), `delete` (`del`), `backspace`,
  `plus`, `minus`, `f1`–`f24`. Shifted US symbols match their base key, so
  `ctrl+shift+=` also matches when the key arrives as `+`.

While a terminal has focus, a chord of plain `Ctrl` or plain `Alt` plus a
character always goes to the shell, so such a binding only works outside
terminals. An unknown action or a chord that doesn't parse is reported as a
warning and that action keeps its default. Settings → Keybindings records
chords and flags ones shared by two actions.

| Action | Windows / Linux default | macOS default |
|---|---|---|
| `quick_switcher` | `ctrl+shift+k, ctrl+k` | `cmd+k` |
| `new_tab` | `ctrl+shift+t` | `cmd+t` |
| `close_tab` | `ctrl+shift+w` | `cmd+w` |
| `next_tab` | `ctrl+tab, ctrl+pagedown` | `ctrl+tab, cmd+shift+]` |
| `prev_tab` | `ctrl+shift+tab, ctrl+pageup` | `ctrl+shift+tab, cmd+shift+[` |
| `copy` | `ctrl+shift+c` | `cmd+c` |
| `paste` | `ctrl+shift+v` | `cmd+v` |
| `find` | `ctrl+shift+f` | `cmd+f` |
| `zoom_in` / `zoom_out` / `zoom_reset` | `ctrl+shift+=` / `ctrl+shift+-` / `ctrl+shift+0` | `cmd+=` / `cmd+-` / `cmd+0` |
| `toggle_sidebar` | `ctrl+shift+b` | `cmd+b` |
| `settings` | `ctrl+shift+,` | `cmd+,` |
| `ui_scale_up` / `ui_scale_down` / `ui_scale_reset` | `ctrl+alt+=` / `ctrl+alt+-` / `ctrl+alt+0` | `cmd+alt+=` / `cmd+alt+-` / `cmd+alt+0` |
| `split_right` | `ctrl+shift+d` | `cmd+d` |
| `split_down` | `ctrl+shift+e` | `cmd+shift+d` |
| `focus_next_pane` | `ctrl+shift+o` | `cmd+]` |
| `close_pane` | `ctrl+shift+x` | `cmd+alt+w` |

```toml
[keys]
close_tab = "ctrl+shift+q"
quick_switcher = "ctrl+shift+p, f1"
```

## `[tui_keys]` (TUI shortcuts)

Same idea for the TUI's global shortcuts. A chord is modifiers (`ctrl`,
`alt`, `shift`) and a key joined by `+`: a character, `left`, `right`, `up`,
`down`, `home`, `end`, `pageup`, `pagedown`, `tab`, `enter`, `esc`, `space`,
`backspace`, `delete`, `insert`, `plus`, `minus` or `f1`–`f12`.

- Every chord needs `ctrl` or `alt`, except the F-keys.
- `Ctrl+C`, `Ctrl+D`, `Ctrl+Z`, `Ctrl+\` and `Ctrl+[` belong to the shell
  and are refused (`ctrl+shift+c` and the like are fine).
- For symbols such as `alt+?` or `alt++`, `Shift` is ignored, since many
  layouts need it to type them. A letter shortcut also works with Caps Lock on.
- An unknown action, a chord that doesn't parse, or a chord already bound to
  another action is reported at start (and at the top of the help); that
  entry keeps its default. The help (`Alt+H`) and status line always show
  the keys in effect.

| Action | Default |
|---|---|
| `tab_1` … `tab_9` | `alt+1` … `alt+9` |
| `next_tab` / `prev_tab` | `alt+right, alt+]` / `alt+left, alt+[` |
| `close_tab` | `alt+w` (in a split tab: the focused pane) |
| `theme` | `alt+t` |
| `help` | `alt+h` |
| `quit` | `alt+q` |
| `find` | `alt+f` (terminal tabs) |
| `snippets` | `alt+s` (terminal tabs) |
| `split_right` / `split_down` | `alt+\` / `alt+-` (terminal tabs) |
| `focus_next_pane` | `alt+o` (terminal tabs) |

```toml
[tui_keys]
close_tab = "alt+x"
split_right = "alt+v"
help = "f1, alt+h"
```

## ssh config editing

Hosts from your ssh config (and the files it `Include`s) can be added,
edited, duplicated, hidden, moved between files and deleted from either
app. Skiff changes only the lines you changed: comments, indentation,
`Key=value` style, quoting, line endings and blank lines stay as they were.
Before every write it:

- refuses if the file changed on disk since it was read ("reopen to see the
  new version"), so an edit made in another editor always wins;
- copies the old file to `backups/<file>-<path hash>-<date>-<time>.bak` next
  to `config.toml` (the 10 newest of each file are kept; the hash keeps two
  files named `config` in different folders apart);
- writes a temp file and renames it over the original, so a crash never
  leaves half a file (on Unix the file keeps its permissions; a new one is
  `0600`).

Skiff's own extras for a host live in one comment inside its block, which
OpenSSH ignores:

```text
Host web
    HostName 10.0.0.5
    # skiff: tags=prod,web; hidden; dir=/srv/app
```

Some hosts are listed but not editable, with the reason shown: a host that
shares a `Host` line with others (`Host web db`), a host defined in more than
one place, and wildcard or `Match` blocks. New hosts go before the first
wildcard `Host *` / `Match` block, so your defaults keep applying to them; a
duplicate goes right after the original. Moving a host to another file takes
the comment lines directly above it along. Renaming a host also renames it in
saved forwards and in the connection history.

## Status checks

"Check status" (`p` in the TUI) connects to each host's port and reads the
`SSH-` greeting; it never logs in. It is off unless you ask
(`status_checks = "on_start"` runs it at start) because probing many servers
unprompted can trip intrusion alarms. For a host reached through `ProxyJump`
(including one inherited from `Host *`), Skiff checks the first jump host the
same way and shows "via bastion: reachable"; the host behind it may still be
down. Hosts behind a `ProxyCommand` show "unknown". The check reads your ssh
config and the system one (`/etc/ssh/ssh_config`, or
`%PROGRAMDATA%\ssh\ssh_config` on Windows) to find where to connect.

## Trying it without a real server

`skiff-testserver` runs a throwaway SSH server on 127.0.0.1 and prints the
`ssh` options it needs:

```bash
cargo run -p skiff-testserver -- --shell --password secret
```

Point Skiff at a scratch config so nothing touches your real hosts:

```toml
# scratch.toml (run with SKIFF_CONFIG=scratch.toml)
[settings]
load_ssh_config = false

[[connection]]
name = "testserver"
host = "127.0.0.1"
# everything the server printed between `ssh` and `127.0.0.1`, one item each:
extra_args = ["-F", "C:/.../ssh_config", "-o", "UserKnownHostsFile=C:/.../known_hosts", "...", "-p", "PORT"]
```
