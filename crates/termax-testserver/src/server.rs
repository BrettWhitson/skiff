use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::Result;
use russh::keys::{PrivateKey, PublicKey};
use russh::server::{Auth, ChannelOpenHandle, Msg, PublicKeyOffer, Server as _, Session, VerifiedPublicKey};
use russh::{Channel, ChannelId, MethodKind, MethodSet};
use tokio::sync::oneshot;

use crate::sftp_fs::SftpFs;

#[derive(Clone)]
pub struct ServerOptions {
    /// Directory served as `/` over SFTP.
    pub root: PathBuf,
    pub user: String,
    /// If set, password auth with this password is offered (and keys are not).
    pub password: Option<String>,
    /// If set, this key may log in.
    pub authorized_key: Option<PublicKey>,
    /// A `shell` request echoes input back (for terminal tests).
    pub shell_echo: bool,
    pub host_key: PrivateKey,
}

/// A running server on `127.0.0.1:port`. Stops when dropped.
pub struct TestServer {
    pub port: u16,
    host_pub: String,
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl TestServer {
    pub fn start(opts: ServerOptions) -> Result<Self> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let host_pub = opts.host_key.public_key().to_openssh()?;

        let methods: &[MethodKind] =
            if opts.password.is_some() { &[MethodKind::Password] } else { &[MethodKind::PublicKey] };
        let config = Arc::new(russh::server::Config {
            keys: vec![opts.host_key.clone()],
            methods: MethodSet::from(methods),
            auth_rejection_time: Duration::from_millis(10),
            auth_rejection_time_initial: Some(Duration::ZERO),
            ..Default::default()
        });

        let (tx, rx) = oneshot::channel();
        let opts = Arc::new(opts);
        let thread = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
            rt.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                let mut srv = Srv { opts };
                let running = srv.run_on_socket(config, &listener);
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
    fn drop(&mut self) {
        self.kill();
    }
}

struct Srv {
    opts: Arc<ServerOptions>,
}

impl russh::server::Server for Srv {
    type Handler = Conn;

    fn new_client(&mut self, _: Option<SocketAddr>) -> Conn {
        Conn { opts: self.opts.clone(), channels: HashMap::new() }
    }
}

struct Conn {
    opts: Arc<ServerOptions>,
    channels: HashMap<ChannelId, Channel<Msg>>,
}

fn reject() -> Auth {
    Auth::Reject { proceed_with_methods: None, partial_success: false }
}

impl Conn {
    fn key_ok(&self, user: &str, key: &PublicKey) -> Auth {
        let ok = user == self.opts.user
            && self.opts.authorized_key.as_ref().is_some_and(|k| k.key_data() == key.key_data());
        if ok { Auth::Accept } else { reject() }
    }
}

impl russh::server::Handler for Conn {
    type Error = anyhow::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth> {
        let ok = user == self.opts.user && self.opts.password.as_deref() == Some(password);
        Ok(if ok { Auth::Accept } else { reject() })
    }

    async fn auth_publickey_offered(&mut self, user: &str, offer: PublicKeyOffer<'_>) -> Result<Auth> {
        Ok(self.key_ok(user, offer.public_key))
    }

    async fn auth_publickey(&mut self, user: &str, key: VerifiedPublicKey<'_>) -> Result<Auth> {
        Ok(self.key_ok(user, key.public_key))
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<()> {
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

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

    async fn shell_request(&mut self, channel: ChannelId, session: &mut Session) -> Result<()> {
        if self.opts.shell_echo {
            session.channel_success(channel)?;
            session.data(channel, b"termax-testserver shell\r\n".to_vec())?;
        } else {
            session.channel_failure(channel)?;
        }
        Ok(())
    }

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

    async fn channel_eof(&mut self, channel: ChannelId, session: &mut Session) -> Result<()> {
        session.close(channel)?;
        Ok(())
    }
}
