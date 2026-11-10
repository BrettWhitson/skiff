//! The `skiff` command line. Every run uses a temp config and a temp ssh
//! config; the only host ever contacted is the test server on 127.0.0.1.

use std::path::PathBuf;
use std::process::{Command, Output};

use skiff_core::config::{Config, Connection};
use skiff_testserver::{FixtureAuth, TestFixture};

/// A temp directory holding a skiff config that points at a temp ssh config.
struct Env {
    /// Keeps the temp directory alive for the test.
    _dir: tempfile::TempDir,
    /// The skiff config file, passed via `SKIFF_CONFIG`.
    cfg: PathBuf,
}

/// Two hosts: `alpha`, and `secret`, which is marked hidden.
const SSH_CONFIG: &str = "Host alpha\n  HostName 10.9.9.1\n  User ops\n\nHost secret\n  # skiff: hidden\n  HostName 10.9.9.2\n";

/// Writes [`SSH_CONFIG`] and a skiff config using it, with `conns` as the
/// saved skiff connections.
fn env(conns: Vec<Connection>) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let ssh = dir.path().join("ssh_config");
    std::fs::write(&ssh, SSH_CONFIG).unwrap();
    let cfg = dir.path().join("config.toml");
    let base = Config::load_or_create(&cfg).unwrap();
    let mut c = base.clone();
    c.settings.ssh_config_path = Some(ssh.display().to_string());
    c.connections = conns;
    c.save(&base, &cfg).unwrap();
    Env { _dir: dir, cfg }
}

/// Runs the built `skiff` binary with `args` against `e`'s config.
fn skiff(e: &Env, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_skiff")).args(args).env("SKIFF_CONFIG", &e.cfg).output().unwrap()
}

/// stdout as text.
fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// `list` leaves out hidden hosts; `list --json --all` includes them, flagged.
#[test]
fn list_and_json_hide_hidden() {
    let e = env(vec![]);
    let o = skiff(&e, &["list"]);
    assert!(o.status.success());
    assert_eq!(out(&o), "alpha\tops@10.9.9.1\t\n");
    let all: serde_json::Value = serde_json::from_slice(&skiff(&e, &["list", "--json", "--all"]).stdout).unwrap();
    assert_eq!(all.as_array().unwrap().len(), 2);
    assert_eq!(all[1]["name"], "secret");
    assert_eq!(all[1]["hidden"], true);
}

/// `info` on an unknown host exits 2 and suggests close names; a hidden host
/// can still be looked up by name.
#[test]
fn info_unknown_exits_2_with_suggestions() {
    let e = env(vec![]);
    let o = skiff(&e, &["info", "alph"]);
    assert_eq!(o.status.code(), Some(2));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("unknown host: alph") && err.contains("did you mean: alpha"), "{err}");
    let o = skiff(&e, &["info", "secret"]);
    assert!(out(&o).contains("Host: 10.9.9.2"));
}

/// The bash completion script registers `_skiff` and calls `skiff list`.
#[test]
fn completion_bash_mentions_list() {
    let e = env(vec![]);
    let s = out(&skiff(&e, &["completion", "bash"]));
    assert!(s.contains("complete -F _skiff skiff") && s.contains("skiff list"));
}

/// Running a command on a host passes the remote exit code through, and each
/// run that reached the host (even a failing command) is counted in the history.
#[test]
fn exec_passes_exit_code_and_records_history() {
    let fx = TestFixture::new(FixtureAuth::Key).unwrap();
    let e = env(vec![Connection { name: "fx".into(), host: fx.destination(), extra_args: fx.ssh_options(), ..Default::default() }]);
    let o = skiff(&e, &["--host", "fx", "exit", "3"]);
    assert_eq!(o.status.code(), Some(3), "{}", String::from_utf8_lossy(&o.stderr));
    let o = skiff(&e, &["fx", "echo", "hi"]);
    assert_eq!(out(&o), "echo hi\n");
    let h = skiff_core::history::History::load(&skiff_core::history::History::path_for(&e.cfg));
    assert_eq!(h.hosts["fx"].count, 2);
}

/// `TERMAX_CONFIG` (the variable's name before the rename) is still honoured
/// when `SKIFF_CONFIG` is unset.
#[test]
fn pre_rename_config_variable_still_works() {
    let e = env(vec![]);
    let o = Command::new(env!("CARGO_BIN_EXE_skiff"))
        .arg("--config-path")
        .env_remove("SKIFF_CONFIG")
        .env("TERMAX_CONFIG", &e.cfg)
        .output()
        .unwrap();
    assert_eq!(out(&o).trim_end(), e.cfg.display().to_string());
}
