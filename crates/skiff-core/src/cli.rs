//! The `skiff` command line, parsed by hand (no extra crates).

use crate::hosts::{HostEntry, Source, fuzzy_score};

/// Shells `skiff completion` can print a script for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
    PowerShell,
}

/// What the command line asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cli {
    /// Start the TUI, optionally opening a host (`true` = file transfer).
    Tui {
        /// Host name and whether to open the file browser instead of a terminal.
        open: Option<(String, bool)>,
    },
    /// Run a command on a host and exit with its status.
    Exec {
        /// Host to run on.
        host: String,
        /// The command and its arguments, passed to ssh as given.
        cmd: Vec<String>,
        /// Allocate a remote terminal (`-t`), for interactive commands.
        tty: bool,
    },
    /// `skiff list`: print the host list.
    List {
        /// Print JSON instead of tab-separated text.
        json: bool,
        /// Include hidden hosts.
        all: bool,
    },
    /// `skiff info <host>`: print one host's details.
    Info {
        /// Host to show.
        host: String,
        /// Print JSON instead of text.
        json: bool,
    },
    /// `skiff completion <shell>`: print a completion script.
    Completion(Shell),
    /// `-h` / `--help`.
    Help,
    /// `-V` / `--version`.
    Version,
    /// `--config-path`: print the config file path.
    ConfigPath,
}

/// The `--help` text.
pub const HELP: &str = "\
Skiff — SSH and SFTP from one place

Usage:
  skiff                      open the host list
  skiff <host>               open a terminal to <host>
  skiff --files <host>       open a file transfer with <host>
  skiff [-t] <host> <cmd…>   run a command on <host> and exit with its status
                              (-t: allocate a terminal, for interactive commands)
  skiff list [--json] [--all]   list hosts (--all includes hidden ones)
  skiff info <host> [--json]    show a host's details
  skiff completion <bash|zsh|fish|powershell>   print a shell completion script
  skiff --host <name> …      use <name> as the host even if it is a command word

Options:
  -h, --help          show this help
  -V, --version       show the version
      --config-path   print the config file path
      --plain         no borders or symbols in the host list (screen readers)

Host names come from ~/.ssh/config and Skiff's own connections.
";

/// Parses a shell name for `skiff completion`.
fn shell(s: &str) -> Result<Shell, String> {
    match s {
        "bash" => Ok(Shell::Bash),
        "zsh" => Ok(Shell::Zsh),
        "fish" => Ok(Shell::Fish),
        "powershell" | "pwsh" => Ok(Shell::PowerShell),
        _ => Err(format!("unknown shell: {s} (bash, zsh, fish or powershell)")),
    }
}

/// Parses the arguments after the program name.
/// Parses the arguments after the program name. Leading options come first;
/// the first non-option word is a subcommand or a host, and any words after a
/// host are a command to run.
pub fn parse(args: &[String]) -> Result<Cli, String> {
    let (mut files, mut tty, mut host) = (false, false, None::<String>);
    let mut i = 0;
    // Leading options. `--host` takes the next word as the host verbatim, so a
    // host can be named like a subcommand; it ends option parsing.
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "-h" | "--help" => return Ok(Cli::Help),
            "-V" | "--version" => return Ok(Cli::Version),
            "--config-path" => return Ok(Cli::ConfigPath),
            "--files" => files = true,
            "-t" => tty = true,
            "--host" => {
                i += 1;
                host = Some(args.get(i).ok_or("--host needs a host name")?.clone());
                i += 1;
                break;
            }
            _ if a.starts_with('-') => return Err(format!("unknown option: {a}\n\n{HELP}")),
            _ => break,
        }
        i += 1;
    }
    let rest = &args[i.min(args.len())..];
    let host = match host {
        Some(h) => h,
        None => {
            // No host given: with no more words start the TUI, unless an option needed
            // a host.
            let Some(word) = rest.first() else {
                if files {
                    return Err("--files needs a host name".into());
                }
                if tty {
                    return Err("-t needs a host and a command".into());
                }
                return Ok(Cli::Tui { open: None });
            };
            // Subcommands accept only the listed flags after them.
            let extra = &rest[1..];
            let flags = |allowed: &[&str]| -> Result<Vec<&str>, String> {
                extra
                    .iter()
                    .map(|e| if allowed.contains(&e.as_str()) { Ok(e.as_str()) } else { Err(format!("unexpected argument: {e}")) })
                    .collect()
            };
            match word.as_str() {
                "list" => {
                    let f = flags(&["--json", "--all"])?;
                    return Ok(Cli::List { json: f.contains(&"--json"), all: f.contains(&"--all") });
                }
                "info" => {
                    let h = extra.first().ok_or("info needs a host name")?;
                    if let Some(e) = extra[1..].iter().find(|e| *e != "--json") {
                        return Err(format!("unexpected argument: {e}"));
                    }
                    return Ok(Cli::Info { host: h.clone(), json: extra.len() > 1 });
                }
                "completion" => {
                    let s = extra.first().ok_or("completion needs a shell: bash, zsh, fish or powershell")?;
                    if extra.len() > 1 {
                        return Err(format!("unexpected argument: {}", extra[1]));
                    }
                    return Ok(Cli::Completion(shell(s)?));
                }
                _ => {}
            }
            // Any other word is the host; the remaining words are the command.
            let h = word.clone();
            return finish(h, &rest[1..], files, tty);
        }
    };
    finish(host, rest, files, tty)
}

/// Turns a host plus optional command into a TUI open (no command) or an
/// exec (with one), rejecting flag combinations that make no sense.
fn finish(host: String, cmd: &[String], files: bool, tty: bool) -> Result<Cli, String> {
    if cmd.is_empty() {
        if tty {
            return Err("-t needs a command".into());
        }
        return Ok(Cli::Tui { open: Some((host, files)) });
    }
    if files {
        return Err("--files takes only a host name".into());
    }
    Ok(Cli::Exec { host, cmd: cmd.to_vec(), tty })
}

/// Parses `skiff-gui` arguments: `skiff-gui [<host>] [--files <host>]`.
/// Returns the host to open and whether to open its files.
/// `skiff-gui [<host>] [--files <host>]`.
pub fn parse_gui(args: &[String]) -> Result<Option<(String, bool)>, String> {
    match args {
        [] => Ok(None),
        [f, h] if f == "--files" => Ok(Some((h.clone(), true))),
        [h] if !h.starts_with('-') => Ok(Some((h.clone(), false))),
        _ => Err(format!("usage: skiff-gui [<host>] [--files <host>] (got: {})", args.join(" "))),
    }
}

/// Finds a host by name (hidden hosts too); unknown names get suggestions.
pub fn resolve<'a>(hosts: &'a [HostEntry], name: &str) -> Result<&'a HostEntry, String> {
    // Exact name first, then a case-insensitive match.
    if let Some(h) = hosts.iter().find(|h| h.name == name).or_else(|| hosts.iter().find(|h| h.name.eq_ignore_ascii_case(name))) {
        return Ok(h);
    }
    // No match: suggest the three best fuzzy matches.
    let mut scored: Vec<(i64, &str)> = hosts.iter().filter_map(|h| fuzzy_score(name, h).map(|s| (s, h.name.as_str()))).collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
    let names: Vec<&str> = scored.iter().take(3).map(|s| s.1).collect();
    let mut msg = format!("unknown host: {name}");
    if !names.is_empty() {
        msg.push_str(&format!("\ndid you mean: {}?", names.join(", ")));
    }
    Err(msg)
}

/// The hosts a listing shows: all of them, or only non-hidden ones.
fn shown(hosts: &[HostEntry], all: bool) -> impl Iterator<Item = &HostEntry> {
    hosts.iter().filter(move |h| all || !h.hidden)
}

/// One host per line: `name<TAB>target<TAB>tags` (easy to `cut -f1`).
pub fn list_text(hosts: &[HostEntry], all: bool) -> String {
    shown(hosts, all).map(|h| format!("{}\t{}\t{}\n", h.name, h.target(), h.tags.join(","))).collect()
}

/// A host as a JSON object (shared by `list --json` and `info --json`).
fn host_json(h: &HostEntry) -> serde_json::Value {
    serde_json::json!({
        "name": h.name,
        "hostname": h.hostname,
        "user": h.user,
        "port": h.port,
        "identity_file": h.identity_file,
        "proxy_jump": h.proxy_jump,
        "proxy_command": h.proxy_command,
        "remote_dir": h.remote_dir,
        "tags": h.tags,
        "hidden": h.hidden,
        "source": match h.source { Source::SshConfig => "ssh_config", Source::Custom(_) => "skiff" },
        "source_file": h.source_file.as_ref().map(|p| p.display().to_string()),
    })
}

/// `skiff list --json`: a pretty-printed JSON array of hosts.
pub fn list_json(hosts: &[HostEntry], all: bool) -> String {
    let v: Vec<serde_json::Value> = shown(hosts, all).map(host_json).collect();
    serde_json::to_string_pretty(&v).unwrap_or_default() + "\n"
}

/// `skiff info`: one `Label: value` line per field, `-` for unset values.
pub fn info_text(h: &HostEntry) -> String {
    let o = |v: &Option<String>| v.clone().unwrap_or_else(|| "-".into());
    let mut s =
        format!("Name: {}\nHost: {}\nUser: {}\nPort: {}\n", h.name, h.hostname, o(&h.user), h.port.map_or("-".into(), |p| p.to_string()));
    s += &format!("Identity file: {}\nProxyJump: {}\nProxyCommand: {}\n", o(&h.identity_file), o(&h.proxy_jump), o(&h.proxy_command));
    s += &format!("Remote dir: {}\nTags: {}\n", o(&h.remote_dir), if h.tags.is_empty() { "-".into() } else { h.tags.join(", ") });
    s += &format!("Source: {}\n", h.source_label);
    if h.hidden {
        s += "Hidden: yes\n";
    }
    s
}

/// `skiff info --json`: one host as a pretty-printed JSON object.
pub fn info_json(h: &HostEntry) -> String {
    serde_json::to_string_pretty(&host_json(h)).unwrap_or_default() + "\n"
}

/// The completion script for `s`, ready to print.
pub fn completion_script(s: Shell) -> &'static str {
    match s {
        Shell::Bash => BASH,
        Shell::Zsh => ZSH,
        Shell::Fish => FISH,
        Shell::PowerShell => POWERSHELL,
    }
}

/// Bash completion: subcommands, options and host names from `skiff list`.
const BASH: &str = r##"# skiff bash completion. Add to ~/.bashrc:  eval "$(skiff completion bash)"
_skiff() {
    local cur="${COMP_WORDS[COMP_CWORD]}"
    local hosts
    hosts="$(skiff list 2>/dev/null | cut -f1)"
    if [ "$COMP_CWORD" -eq 1 ]; then
        COMPREPLY=( $(compgen -W "list info completion --files --host --help --version $hosts" -- "$cur") )
    elif [ "$COMP_CWORD" -eq 2 ]; then
        case "${COMP_WORDS[1]}" in
            info|--files|--host|-t) COMPREPLY=( $(compgen -W "$hosts" -- "$cur") ) ;;
            completion) COMPREPLY=( $(compgen -W "bash zsh fish powershell" -- "$cur") ) ;;
            list) COMPREPLY=( $(compgen -W "--json --all" -- "$cur") ) ;;
        esac
    fi
}
complete -F _skiff skiff
"##;

/// Zsh completion, same behaviour as the bash script.
const ZSH: &str = r##"#compdef skiff
# skiff zsh completion. Add to ~/.zshrc:  eval "$(skiff completion zsh)"
_skiff() {
    local -a hosts
    hosts=(${(f)"$(skiff list 2>/dev/null | cut -f1)"})
    if (( CURRENT == 2 )); then
        compadd -- list info completion --files --host --help --version $hosts
    elif (( CURRENT == 3 )); then
        case $words[2] in
            info|--files|--host|-t) compadd -- $hosts ;;
            completion) compadd -- bash zsh fish powershell ;;
            list) compadd -- --json --all ;;
        esac
    fi
}
compdef _skiff skiff
"##;

/// Fish completion, same behaviour as the bash script.
const FISH: &str = r##"# skiff fish completion. Save as ~/.config/fish/completions/skiff.fish:
#   skiff completion fish > ~/.config/fish/completions/skiff.fish
complete -c skiff -f
complete -c skiff -n "__fish_use_subcommand" -a "list info completion (skiff list 2>/dev/null | cut -f1)"
complete -c skiff -n "__fish_seen_subcommand_from info" -a "(skiff list 2>/dev/null | cut -f1)"
complete -c skiff -n "__fish_seen_subcommand_from completion" -a "bash zsh fish powershell"
complete -c skiff -n "__fish_seen_subcommand_from list" -l json -l all
complete -c skiff -l files -d "open a file transfer" -a "(skiff list 2>/dev/null | cut -f1)"
complete -c skiff -l host -d "use this host name" -a "(skiff list 2>/dev/null | cut -f1)"
"##;

/// PowerShell completion, same behaviour as the bash script.
const POWERSHELL: &str = r##"# skiff PowerShell completion. Add to $PROFILE:
#   skiff completion powershell | Out-String | Invoke-Expression
Register-ArgumentCompleter -Native -CommandName skiff -ScriptBlock {
    param($wordToComplete, $commandAst, $cursorPosition)
    $words = @($commandAst.CommandElements | Select-Object -Skip 1 | ForEach-Object { $_.ToString() })
    if ($wordToComplete) { $words = @($words | Select-Object -SkipLast 1) }
    $hosts = @(skiff list 2>$null | ForEach-Object { ($_ -split "`t")[0] })
    $cands = if ($words.Count -eq 0) { @('list', 'info', 'completion', '--files', '--host', '--help', '--version') + $hosts }
        elseif ($words.Count -eq 1 -and $words[0] -in 'info', '--files', '--host', '-t') { $hosts }
        elseif ($words.Count -eq 1 -and $words[0] -eq 'completion') { @('bash', 'zsh', 'fish', 'powershell') }
        elseif ($words[0] -eq 'list') { @('--json', '--all') }
        else { @() }
    $cands | Where-Object { $_ -like "$wordToComplete*" } |
        ForEach-Object { [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_) }
}
"##;

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses a space-separated argument string.
    fn p(s: &str) -> Result<Cli, String> {
        parse(&s.split_whitespace().map(String::from).collect::<Vec<_>>())
    }

    /// The expected result for opening `h` in the TUI.
    fn open(h: &str, files: bool) -> Cli {
        Cli::Tui { open: Some((h.into(), files)) }
    }

    /// Covers every command form, `--host` overriding subcommand words, and the
    /// error cases.
    #[test]
    fn parse_cases() {
        assert_eq!(p(""), Ok(Cli::Tui { open: None }));
        assert_eq!(p("web"), Ok(open("web", false)));
        assert_eq!(p("--files web"), Ok(open("web", true)));
        assert_eq!(
            p("web ls -la /tmp"),
            Ok(Cli::Exec { host: "web".into(), cmd: vec!["ls".into(), "-la".into(), "/tmp".into()], tty: false })
        );
        assert_eq!(p("-t web top"), Ok(Cli::Exec { host: "web".into(), cmd: vec!["top".into()], tty: true }));
        assert_eq!(p("list"), Ok(Cli::List { json: false, all: false }));
        assert_eq!(p("list --json --all"), Ok(Cli::List { json: true, all: true }));
        assert_eq!(p("info web --json"), Ok(Cli::Info { host: "web".into(), json: true }));
        assert_eq!(p("completion zsh"), Ok(Cli::Completion(Shell::Zsh)));
        assert_eq!(p("--host list"), Ok(open("list", false)));
        assert_eq!(p("--host info uptime"), Ok(Cli::Exec { host: "info".into(), cmd: vec!["uptime".into()], tty: false }));
        assert_eq!(p("--help"), Ok(Cli::Help));
        assert_eq!(p("-V"), Ok(Cli::Version));
        assert_eq!(p("--config-path"), Ok(Cli::ConfigPath));

        assert!(p("info").is_err());
        assert!(p("completion tcsh").is_err());
        assert!(p("--bogus").is_err());
        assert!(p("-t web").is_err());
        assert!(p("--files").is_err());
        assert!(p("list extra").is_err());
    }

    /// `skiff-gui` accepts no args, a host, or `--files <host>`, and nothing else.
    #[test]
    fn gui_args() {
        let g = |s: &str| parse_gui(&s.split_whitespace().map(String::from).collect::<Vec<_>>());
        assert_eq!(g(""), Ok(None));
        assert_eq!(g("web"), Ok(Some(("web".into(), false))));
        assert_eq!(g("--files web"), Ok(Some(("web".into(), true))));
        assert!(g("web extra").is_err());
    }

    /// Exact names (hidden ones too) resolve; an unknown name lists close matches.
    #[test]
    fn resolve_suggests() {
        let mut cfg = crate::config::Config::default();
        cfg.settings.load_ssh_config = false;
        cfg.connections = ["web-prod", "web-stage", "db", "cache"]
            .iter()
            .map(|n| crate::config::Connection { name: n.to_string(), host: "h".into(), ..Default::default() })
            .collect();
        cfg.connections[3].hidden = true;
        let hosts = crate::hosts::collect(&cfg).0;
        assert_eq!(resolve(&hosts, "db").unwrap().name, "db");
        assert_eq!(resolve(&hosts, "cache").unwrap().name, "cache");
        let e = resolve(&hosts, "web").unwrap_err();
        assert!(e.starts_with("unknown host: web\ndid you mean: web-prod, web-stage"), "{e}");
    }

    /// Text listing hides hidden hosts and JSON escapes names and includes them
    /// with `--all`; `info` shows tags.
    #[test]
    fn list_formats() {
        let mut cfg = crate::config::Config::default();
        cfg.settings.load_ssh_config = false;
        cfg.connections = vec![
            crate::config::Connection { name: "a\"b".into(), host: "h".into(), tags: vec!["x".into()], ..Default::default() },
            crate::config::Connection { name: "hid".into(), host: "h".into(), hidden: true, ..Default::default() },
        ];
        let hosts = crate::hosts::collect(&cfg).0;
        assert_eq!(list_text(&hosts, false), "a\"b\th\tx\n");
        let j: serde_json::Value = serde_json::from_str(&list_json(&hosts, true)).unwrap();
        assert_eq!(j[0]["name"], "a\"b");
        assert_eq!(j[1]["hidden"], true);
        assert!(info_text(&hosts[0]).contains("Tags: x"));
    }
}
