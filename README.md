# termax

A terminal UI for SSH and file transfer, written in Rust.

- Lists hosts from `~/.ssh/config` plus your own **custom connections**
- Opens each connection in its own **tab**, as an interactive terminal or a dual-pane **SFTP browser**
- **Themes**, including built-in ones and your own, with live preview

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

`termax --config-path` prints where the config file lives.

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

[[connection]]
name = "homelab"
host = "192.168.1.10"
user = "admin"
port = 22
identity_file = "~/.ssh/id_ed25519"
proxy_jump = "bastion"
extra_args = ["-A"]
remote_dir = "/srv"
tags = ["home"]

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

Hosts from `~/.ssh/config` are read-only in termax. Pressing `e` on one creates
an editable termax copy. Saving from the app only changes the settings you
edited; comments and formatting in the file are kept.

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
| | `n` / `e` / `d` | New / edit / delete connection |
| | `r` | Reload config files |
| Terminal | `Shift+PgUp` / `Shift+PgDn` | Scrollback |
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

## Known limitations / next steps

- **SFTP needs non-interactive auth.** The SFTP channel has no terminal, so with
  `sftp_batch_mode = true` it uses keys or an agent and fails instead of prompting
  for a password. A future step is an `SSH_ASKPASS` helper that prompts inside termax.
- Keybindings are fixed. A `[keys]` config section would make them remappable.
- No mouse support yet (tab clicks, scrolling, selection).
- Transfers can't be cancelled, and there's no overwrite confirmation.
- Terminal tabs have no search, and the scrollback is view-only.
- Split panes (two sessions side by side in one tab) aren't supported.
