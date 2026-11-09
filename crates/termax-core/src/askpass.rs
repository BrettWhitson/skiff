//! Lets a front-end answer ssh's password, one-time-code and host-key prompts
//! when ssh has no terminal (SFTP connections).
//!
//! ssh runs `SSH_ASKPASS` (a bare program path) with the prompt as its only
//! argument and reads the answer from its stdout. termax points it at its own
//! executable; that process sees `TERMAX_ASKPASS_ENDPOINT`, runs
//! [`helper_main`], and relays the prompt over a local socket (a named pipe on
//! Windows) to the [`Broker`] in the running app. Answers are never stored.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use interprocess::local_socket::tokio::{prelude::*, Stream as TokioStream};
use interprocess::local_socket::traits::Stream as _;
use interprocess::local_socket::{GenericFilePath, GenericNamespaced, ListenerOptions, Name, Stream};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as TokioBufReader};
use tokio::runtime::Handle;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

pub const ENV_ENDPOINT: &str = "TERMAX_ASKPASS_ENDPOINT";
pub const ENV_TAG: &str = "TERMAX_ASKPASS_TAG";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskKind {
    /// A password or code: input should be masked.
    Secret,
    /// A yes/no question, e.g. accepting a new host key.
    Confirm,
}

impl AskKind {
    pub fn of(prompt: &str) -> Self {
        if prompt.contains("(yes/no") { AskKind::Confirm } else { AskKind::Secret }
    }
}

pub struct AskRequest {
    /// Identifies which connection is asking (whatever the caller passed to [`Broker::env`]).
    pub tag: String,
    pub prompt: String,
    pub kind: AskKind,
    /// `None` cancels.
    pub reply: oneshot::Sender<Option<String>>,
}

#[derive(Serialize, Deserialize)]
struct Question {
    tag: String,
    prompt: String,
}

#[derive(Serialize, Deserialize, Default)]
struct Answer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    answer: Option<String>,
    #[serde(default)]
    cancel: bool,
}

pub struct Broker {
    endpoint: String,
    helper: PathBuf,
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Endpoint strings carry their kind so the helper can rebuild the name:
/// `ns:<name>` (named pipe / abstract socket) or `fs:<path>` (socket file).
fn endpoint_name(endpoint: &str) -> std::io::Result<Name<'_>> {
    match endpoint.split_once(':') {
        Some(("ns", n)) => n.to_ns_name::<GenericNamespaced>(),
        Some(("fs", p)) => p.to_fs_name::<GenericFilePath>(),
        _ => Err(std::io::Error::other("bad askpass endpoint")),
    }
}

impl Broker {
    /// Starts listening. `helper` is the executable ssh should run (one that
    /// calls [`helper_main`] when [`is_helper_invocation`] is true).
    pub fn start(rt: &Handle, helper: PathBuf) -> Result<(Self, UnboundedReceiver<AskRequest>)> {
        let unique = format!(
            "termax-askpass-{}-{}-{}.sock",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
        );
        let endpoint = if GenericNamespaced::is_supported() {
            format!("ns:{unique}")
        } else {
            // untested on this machine (macOS path)
            format!("fs:{}", std::env::temp_dir().join(&unique).display())
        };
        let (tx, rx) = unbounded_channel();
        let listener = {
            let _guard = rt.enter();
            ListenerOptions::new().name(endpoint_name(&endpoint)?).create_tokio()?
        };
        rt.spawn(async move {
            loop {
                let Ok(conn) = listener.accept().await else { continue };
                let tx = tx.clone();
                tokio::spawn(async move {
                    let _ = serve(conn, tx).await;
                });
            }
        });
        Ok((Self { endpoint, helper }, rx))
    }

    /// Environment for an `ssh` process whose prompts should come to this broker.
    pub fn env(&self, tag: &str) -> Vec<(String, String)> {
        let mut env = vec![
            ("SSH_ASKPASS".to_string(), self.helper.display().to_string()),
            ("SSH_ASKPASS_REQUIRE".to_string(), "force".to_string()),
            (ENV_ENDPOINT.to_string(), self.endpoint.clone()),
            (ENV_TAG.to_string(), tag.to_string()),
        ];
        if cfg!(unix) && std::env::var_os("DISPLAY").is_none() {
            // untested on this machine: older ssh only uses askpass with DISPLAY set.
            env.push(("DISPLAY".to_string(), "termax".to_string()));
        }
        env
    }
}

async fn serve(conn: TokioStream, tx: UnboundedSender<AskRequest>) -> Result<()> {
    let mut line = String::new();
    TokioBufReader::new(&conn).read_line(&mut line).await?;
    let mut write = &conn;
    let q: Question = serde_json::from_str(line.trim())?;
    let (reply, rx) = oneshot::channel();
    let kind = AskKind::of(&q.prompt);
    tx.send(AskRequest { tag: q.tag, prompt: q.prompt, kind, reply })?;
    let answer = match rx.await {
        Ok(Some(a)) => Answer { answer: Some(a), cancel: false },
        _ => Answer { answer: None, cancel: true },
    };
    let mut out = serde_json::to_string(&answer)?;
    out.push('\n');
    write.write_all(out.as_bytes()).await?;
    write.flush().await?;
    Ok(())
}

/// True when this process was started by ssh as the askpass helper.
pub fn is_helper_invocation() -> bool {
    std::env::var_os(ENV_ENDPOINT).is_some()
}

/// Body of the askpass helper process: relays argv[1] to the broker and prints
/// the answer for ssh. Returns the process exit code (0 = answered).
pub fn helper_main() -> i32 {
    match run_helper() {
        Ok(Some(answer)) => {
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{answer}");
            let _ = out.flush();
            0
        }
        _ => 1,
    }
}

fn run_helper() -> Result<Option<String>> {
    let endpoint = std::env::var(ENV_ENDPOINT)?;
    let tag = std::env::var(ENV_TAG).unwrap_or_default();
    let prompt = std::env::args().nth(1).unwrap_or_default();
    let mut conn = BufReader::new(Stream::connect(endpoint_name(&endpoint)?)?);
    let mut q = serde_json::to_string(&Question { tag, prompt })?;
    q.push('\n');
    conn.get_mut().write_all(q.as_bytes())?;
    let mut line = String::new();
    conn.read_line(&mut line)?;
    let a: Answer = serde_json::from_str(line.trim())?;
    Ok(if a.cancel { None } else { a.answer })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_prompts() {
        assert_eq!(AskKind::of("tester@127.0.0.1's password: "), AskKind::Secret);
        assert_eq!(AskKind::of("Are you sure you want to continue connecting (yes/no/[fingerprint])? "), AskKind::Confirm);
    }
}
