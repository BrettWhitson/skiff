//! The `termax` command line. Every run uses a temp config and a temp ssh
//! config; the only host ever contacted is the test server on 127.0.0.1.

use std::path::PathBuf;
use std::process::{Command, Output};

use termax_core::config::{Config, Connection};
use termax_testserver::{FixtureAuth, TestFixture};

struct Env {
    _dir: tempfile::TempDir,
    cfg: PathBuf,
}

const SSH_CONFIG: &str = "Host alpha\n  HostName 10.9.9.1\n  User ops\n\nHost secret\n  # termax: hidden\n  HostName 10.9.9.2\n";

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

fn termax(e: &Env, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_termax")).args(args).env("TERMAX_CONFIG", &e.cfg).output().unwrap()
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

#[test]
fn list_and_json_hide_hidden() {
    let e = env(vec![]);
    let o = termax(&e, &["list"]);
    assert!(o.status.success());
    assert_eq!(out(&o), "alpha\tops@10.9.9.1\t\n");
    let all: serde_json::Value = serde_json::from_slice(&termax(&e, &["list", "--json", "--all"]).stdout).unwrap();
    assert_eq!(all.as_array().unwrap().len(), 2);
    assert_eq!(all[1]["name"], "secret");
    assert_eq!(all[1]["hidden"], true);
}

#[test]
fn info_unknown_exits_2_with_suggestions() {
    let e = env(vec![]);
    let o = termax(&e, &["info", "alph"]);
    assert_eq!(o.status.code(), Some(2));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("unknown host: alph") && err.contains("did you mean: alpha"), "{err}");
    let o = termax(&e, &["info", "secret"]);
    assert!(out(&o).contains("Host: 10.9.9.2"));
}

#[test]
fn completion_bash_mentions_list() {
    let e = env(vec![]);
    let s = out(&termax(&e, &["completion", "bash"]));
    assert!(s.contains("complete -F _termax termax") && s.contains("termax list"));
}

#[test]
fn exec_passes_exit_code_and_records_history() {
    let fx = TestFixture::new(FixtureAuth::Key).unwrap();
    let e = env(vec![Connection {
        name: "fx".into(),
        host: fx.destination(),
        extra_args: fx.ssh_options(),
        ..Default::default()
    }]);
    let o = termax(&e, &["--host", "fx", "exit", "3"]);
    assert_eq!(o.status.code(), Some(3), "{}", String::from_utf8_lossy(&o.stderr));
    let o = termax(&e, &["fx", "echo", "hi"]);
    assert_eq!(out(&o), "echo hi\n");
    let h = termax_core::history::History::load(&termax_core::history::History::path_for(&e.cfg));
    assert_eq!(h.hosts["fx"].count, 2);
}
