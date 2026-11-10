//! [`TestServer`]: an in-process russh SSH server on 127.0.0.1 with SFTP, an
//! optional echo shell, trivial exec commands and loopback-only local forwards.
//! It runs on its own thread and tokio runtime so sync and async tests can use it.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::Result;
use russh::keys::{PrivateKey, PublicKey};
use russh::server::{Auth, ChannelOpenHandle, Msg, PublicKeyOffer, Server as _, Session, VerifiedPublicKey};
use russh::{Channel, ChannelId, ChannelOpenFailure, MethodKind, MethodSet};
use tokio::sync::oneshot;

use crate::sftp_fs::SftpFs;

/// How a [`TestServer`] is set up.
#[derive(Clone)]
pub struct ServerOptions {
    /// Directory served as `/` over SFTP.
    pub root: PathBuf,
    /// The only user name that may log in.
    pub user: String,
    /// If set, password auth with this password is offered (and keys are not).
    pub password: Option<String>,
    /// If set, this key may log in.
    pub authorized_key: Option<PublicKey>,
    /// A `shell` request echoes input back (for terminal tests).
    pub shell_echo: bool,
    /// The server's host key.
    pub host_key: PrivateKey,
}

/// A running server on `127.0.0.1:port`. Stops when dropped.
pub struct TestServer {
    /// The port picked by the OS when binding `127.0.0.1:0`.
    pub port: u16,
    /// Host public key in OpenSSH form (`ssh-ed25519 AAAA… comment`).
    host_pub: String,
    /// Fires to stop the accept loop; `None` once stopped.
    shutdown: Option<oneshot::Sender<()>>,
    /// The thread that owns the server's runtime; `None` once joined.
    thread: Option<JoinHandle<()>>,
}

impl TestServer {
    /// Binds a free port on 127.0.0.1 and starts serving on a background
    /// thread. Returns once the socket is bound, so clients can connect at once.
    pub fn start(opts: ServerOptions) -> Result<Self> {
        // Bind synchronously so the port is known (and listening) before returning.
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let host_pub = opts.host_key.public_key().to_openssh()?;

        // Offer exactly one auth method; quick rejections keep failing tests fast.
        let methods: &[MethodKind] = if opts.password.is_some() { &[MethodKind::Password] } else { &[MethodKind::PublicKey] };
        let config = Arc::new(russh::server::Config {
            keys: vec![opts.host_key.clone()],
            methods: MethodSet::from(methods),
            auth_rejection_time: Duration::from_millis(10),
            auth_rejection_time_initial: Some(Duration::ZERO),
            ..Default::default()
        });

        // The server gets a private runtime on its own thread, so it works from
        // plain #[test]s and is unaffected by the test's own runtime.
        let (tx, rx) = oneshot::channel();
        let opts = Arc::new(opts);
        let thread = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
            rt.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                let mut srv = Srv { opts };
                let running = srv.run_on_socket(config, &listener);
                // Serve until the accept loop ends or `kill` signals (or drops) the sender.
                tokio::select! {
                    _ = running => {}
                    _ = rx => {}
                }
            });
            // Dropping the runtime aborts every open session.
            rt.shutdown_timeout(Duration::from_millis(200));
        });
        Ok(Self { port, host_pub, shutdown: Some(tx), thread: Some(thread) })
    }

    /// `[127.0.0.1]:PORT ssh-ed25519 AAAA…`, for a known_hosts file.
    pub fn host_key_line(&self) -> String {
        let mut parts = self.host_pub.split_whitespace();
        let (kind, key) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        format!("[127.0.0.1]:{} {kind} {key}", self.port)
    }

    /// Stops the server and drops every connection.
    pub fn kill(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for TestServer {
    /// Stops the server so no thread or port outlives the test.
    fn drop(&mut self) {
        self.kill();
    }
}

/// The russh server factory: hands each new connection the shared options.
struct Srv {
    opts: Arc<ServerOptions>,
}

impl russh::server::Server for Srv {
    type Handler = Conn;

    /// Creates the handler for one incoming connection.
    fn new_client(&mut self, _: Option<SocketAddr>) -> Conn {
        Conn { opts: self.opts.clone(), channels: HashMap::new() }
    }
}

/// Per-connection handler.
struct Conn {
    opts: Arc<ServerOptions>,
    /// Session channels opened but not yet claimed by a subsystem request.
    channels: HashMap<ChannelId, Channel<Msg>>,
}

/// A plain auth rejection with no further methods suggested.
fn reject() -> Auth {
    Auth::Reject { proceed_with_methods: None, partial_success: false }
}

impl Conn {
    /// Accepts only the configured user with the configured authorized key.
    fn key_ok(&self, user: &str, key: &PublicKey) -> Auth {
        let ok = user == self.opts.user && self.opts.authorized_key.as_ref().is_some_and(|k| k.key_data() == key.key_data());
        if ok { Auth::Accept } else { reject() }
    }
}

impl russh::server::Handler for Conn {
    type Error = anyhow::Error;

    /// Accepts the configured user and password; anything else is rejected.
    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth> {
        let ok = user == self.opts.user && self.opts.password.as_deref() == Some(password);
        Ok(if ok { Auth::Accept } else { reject() })
    }

    /// Answers the client's "would this key be accepted?" probe.
    async fn auth_publickey_offered(&mut self, user: &str, offer: PublicKeyOffer<'_>) -> Result<Auth> {
        Ok(self.key_ok(user, offer.public_key))
    }

    /// Final key check after the client has proved it holds the private key.
    async fn auth_publickey(&mut self, user: &str, key: VerifiedPublicKey<'_>) -> Result<Auth> {
        Ok(self.key_ok(user, key.public_key))
    }

    /// Accepts every session channel and keeps it until a subsystem claims it.
    async fn channel_open_session(&mut self, channel: Channel<Msg>, reply: ChannelOpenHandle, _session: &mut Session) -> Result<()> {
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    /// Local forwards (`ssh -L`). Only 127.0.0.1/localhost targets: the test
    /// server never reaches out anywhere else.
    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host: &str,
        port: u32,
        _originator: &str,
        _originator_port: u32,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<()> {
        let Ok(port) = u16::try_from(port) else {
            reply.reject(ChannelOpenFailure::ConnectFailed).await;
            return Ok(());
        };
        if !matches!(host, "127.0.0.1" | "localhost") {
            reply.reject(ChannelOpenFailure::AdministrativelyProhibited).await;
            return Ok(());
        }
        // Connect first so an unreachable port is reported as an open failure,
        // then pump bytes both ways until either side closes.
        match tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
            Ok(mut tcp) => {
                reply.accept().await;
                tokio::spawn(async move {
                    let mut ch = channel.into_stream();
                    let _ = tokio::io::copy_bidirectional(&mut ch, &mut tcp).await;
                });
            }
            Err(_) => reply.reject(ChannelOpenFailure::ConnectFailed).await,
        }
        Ok(())
    }

    /// Runs the SFTP server on the channel for `sftp`; any other subsystem
    /// (or an unknown channel) is refused.
    async fn subsystem_request(&mut self, id: ChannelId, name: &str, session: &mut Session) -> Result<()> {
        match (name, self.channels.remove(&id)) {
            ("sftp", Some(channel)) => {
                session.channel_success(id)?;
                russh_sftp::server::run(channel.into_stream(), SftpFs::new(self.opts.root.clone())).await;
            }
            _ => session.channel_failure(id)?,
        }
        Ok(())
    }

    /// Accepts any PTY request; size and modes are ignored.
    #[allow(clippy::too_many_arguments)]
    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _term: &str,
        _cols: u32,
        _rows: u32,
        _pw: u32,
        _ph: u32,
        _modes: &[(russh::Pty, u32)],
        session: &mut Session,
    ) -> Result<()> {
        session.channel_success(channel)?;
        Ok(())
    }

    /// `exit N` exits with status N; any other command is echoed back.
    async fn exec_request(&mut self, channel: ChannelId, data: &[u8], session: &mut Session) -> Result<()> {
        let cmd = String::from_utf8_lossy(data).into_owned();
        session.channel_success(channel)?;
        let code = cmd.strip_prefix("exit ").and_then(|n| n.trim().parse::<u32>().ok());
        if code.is_none() {
            session.data(channel, format!("{cmd}\n").into_bytes())?;
        }
        session.exit_status_request(channel, code.unwrap_or(0))?;
        session.eof(channel)?;
        session.close(channel)?;
        Ok(())
    }

    /// With `shell_echo`, accepts and prints a banner; otherwise refuses.
    async fn shell_request(&mut self, channel: ChannelId, session: &mut Session) -> Result<()> {
        if self.opts.shell_echo {
            session.channel_success(channel)?;
            session.data(channel, b"skiff-testserver shell\r\n".to_vec())?;
        } else {
            session.channel_failure(channel)?;
        }
        Ok(())
    }

    /// In echo-shell mode, sends input straight back, turning each CR into
    /// CR LF the way a terminal's line discipline would.
    async fn data(&mut self, channel: ChannelId, data: &[u8], session: &mut Session) -> Result<()> {
        if self.opts.shell_echo {
            let mut out = Vec::with_capacity(data.len());
            for &b in data {
                out.push(b);
                if b == b'\r' {
                    out.push(b'\n');
                }
            }
            session.data(channel, out)?;
        }
        Ok(())
    }

    /// Closes the channel once the client signals end of input.
    async fn channel_eof(&mut self, channel: ChannelId, session: &mut Session) -> Result<()> {
        session.close(channel)?;
        Ok(())
    }
}
