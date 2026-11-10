//! [`TestFixture`]: a throwaway directory plus a running [`TestServer`], with
//! the `ssh` command-line options that point the system client at it and
//! nowhere else.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use russh::keys::{PrivateKey, PublicKey};
use tempfile::TempDir;

use crate::server::{ServerOptions, TestServer};

/// How the fixture's server lets the client log in.
pub enum FixtureAuth {
    /// Public-key auth with a key generated for the fixture (passed via `-i`).
    Key,
    /// Password auth with this password; key auth is not offered.
    Password(String),
}

/// A temp directory with keys, an empty ssh config, a known_hosts file and a
/// running [`TestServer`]. `ssh_options()` makes the system `ssh` talk only to
/// this server and never read the user's own config, keys or agent.
pub struct TestFixture {
    server: TestServer,
    /// Client private key file, used only when `with_key` is set.
    key_path: PathBuf,
    /// known_hosts holding just this server's host key.
    known_hosts: PathBuf,
    /// Empty ssh config passed with `-F`.
    config: PathBuf,
    /// Directory served as `/` over SFTP.
    root: PathBuf,
    /// Whether the server accepts the generated client key (vs. a password).
    with_key: bool,
    /// Owns the temp directory; deleted when the fixture is dropped.
    _dir: TempDir,
}

/// Creates an ed25519 key pair with `ssh-keygen` (no passphrase).
pub fn generate_key(dir: &Path, name: &str) -> Result<(PrivateKey, PublicKey, PathBuf)> {
    let path = dir.join(name);
    let out = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "skiff-test", "-f"])
        .arg(&path)
        .output()
        .context("running ssh-keygen")?;
    if !out.status.success() {
        bail!("ssh-keygen failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    let private = PrivateKey::read_openssh_file(&path)?;
    let public = private.public_key().clone();
    Ok((private, public, path))
}

impl TestFixture {
    /// Starts a fixture whose server refuses shell requests.
    pub fn new(auth: FixtureAuth) -> Result<Self> {
        Self::with_options(auth, false)
    }

    /// Like `new`, with a shell that echoes input (for terminal tests).
    pub fn with_shell(auth: FixtureAuth) -> Result<Self> {
        Self::with_options(auth, true)
    }

    /// Builds the temp layout (root dir, host and client keys), starts the
    /// server for user `tester`, then writes known_hosts and an empty config.
    fn with_options(auth: FixtureAuth, shell_echo: bool) -> Result<Self> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().join("root");
        std::fs::create_dir(&root)?;
        let (host_key, _, _) = generate_key(dir.path(), "host_key")?;
        // The client key is always generated, but only authorized in Key mode.
        let (_, client_pub, key_path) = generate_key(dir.path(), "id_test")?;
        let (password, with_key) = match auth {
            FixtureAuth::Key => (None, true),
            FixtureAuth::Password(p) => (Some(p), false),
        };
        let server = TestServer::start(ServerOptions {
            root: root.clone(),
            user: "tester".into(),
            password,
            authorized_key: with_key.then_some(client_pub),
            shell_echo,
            host_key,
        })?;
        let known_hosts = dir.path().join("known_hosts");
        std::fs::write(&known_hosts, format!("{}\n", server.host_key_line()))?;
        let config = dir.path().join("ssh_config");
        std::fs::write(&config, "")?;
        Ok(Self { server, key_path, known_hosts, config, root, with_key, _dir: dir })
    }

    /// Options for the system `ssh`, placed before the destination.
    pub fn ssh_options(&self) -> Vec<String> {
        // Empty config, private known_hosts with strict checking, no agent and
        // no default identities: only this fixture's files are ever consulted.
        let mut v: Vec<String> = vec![
            "-F".into(),
            self.config.display().to_string(),
            "-o".into(),
            format!("UserKnownHostsFile={}", self.known_hosts.display()),
            "-o".into(),
            "StrictHostKeyChecking=yes".into(),
            "-o".into(),
            "IdentitiesOnly=yes".into(),
            "-o".into(),
            "IdentityAgent=none".into(),
            "-o".into(),
            "ConnectTimeout=10".into(),
            "-p".into(),
            self.server.port.to_string(),
            "-l".into(),
            "tester".into(),
        ];
        if self.with_key {
            v.extend(["-i".into(), self.key_path.display().to_string()]);
        }
        v
    }

    /// The server's port on 127.0.0.1.
    pub fn port(&self) -> u16 {
        self.server.port
    }

    /// The host to pass to `ssh` after [`ssh_options`](Self::ssh_options): always `127.0.0.1`.
    pub fn destination(&self) -> String {
        "127.0.0.1".into()
    }

    /// Directory the server exposes as `/`.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Path of the fixture's known_hosts file (holds only this server's key).
    pub fn known_hosts_path(&self) -> &Path {
        &self.known_hosts
    }

    /// Direct access to the server, e.g. to [`TestServer::kill`] it mid-test.
    pub fn server_mut(&mut self) -> &mut TestServer {
        &mut self.server
    }
}
