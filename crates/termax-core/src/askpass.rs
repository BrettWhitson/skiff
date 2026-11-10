//! Lets a front-end answer ssh's password, one-time-code and host-key prompts
//! when ssh has no terminal (SFTP connections).
//!
//! ssh runs `SSH_ASKPASS` (a bare program path) with the prompt as its only
//! argument and reads the answer from its stdout. termax points it at its own
//! executable; that process sees `TERMAX_ASKPASS_ENDPOINT`, runs
//! [`helper_main`], and relays the prompt over a local socket (a named pipe on
//! Windows) to the [`Broker`] in the running app. Answers are never stored.

use std::collections::hash_map::RandomState;
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, Hasher};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use interprocess::local_socket::tokio::{Stream as TokioStream, prelude::*};
use interprocess::local_socket::traits::Stream as _;
use interprocess::local_socket::{GenericFilePath, GenericNamespaced, ListenerOptions, Name, Stream};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader as TokioBufReader};
use tokio::runtime::Handle;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub const ENV_ENDPOINT: &str = "TERMAX_ASKPASS_ENDPOINT";
pub const ENV_TAG: &str = "TERMAX_ASKPASS_TAG";
/// Per-broker secret; a question without it is refused, so another local
/// user who finds the socket can't pose as ssh and collect a password.
pub const ENV_TOKEN: &str = "TERMAX_ASKPASS_TOKEN";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskKind {
    /// A password or code: input should be masked.
    Secret,
    /// A yes/no question, e.g. accepting a new host key.
    Confirm,
    /// Information only, such as "touch your security key": nothing to
    /// answer. Show it without blocking; it is withdrawn (`cancelled`) when
    /// ssh is done with it.
    Notice,
}

/// The variable ssh sets for the helper to say what kind of answer it wants.
pub const ENV_PROMPT_HINT: &str = "SSH_ASKPASS_PROMPT";

impl AskKind {
    pub fn of(prompt: &str) -> Self {
        Self::with_hint(prompt, "")
    }

    /// `hint` is ssh's `SSH_ASKPASS_PROMPT` (`confirm`, `none`, or unset).
    pub fn with_hint(prompt: &str, hint: &str) -> Self {
        match hint {
            "none" => AskKind::Notice,
            "confirm" => AskKind::Confirm,
            _ if prompt.contains("(yes/no") => AskKind::Confirm,
            _ => AskKind::Secret,
        }
    }
}

pub struct AskRequest {
    /// Identifies which connection is asking (whatever the caller passed to [`Broker::env`]).
    pub tag: String,
    pub prompt: String,
    pub kind: AskKind,
    /// `None` cancels.
    pub reply: oneshot::Sender<Option<String>>,
    /// Fires when the question is withdrawn (see [`Broker::retire`]); the
    /// front-end should then stop showing the prompt.
    pub cancelled: CancellationToken,
}

#[derive(Serialize, Deserialize)]
struct Question {
    token: String,
    tag: String,
    prompt: String,
    /// ssh's `SSH_ASKPASS_PROMPT`, if set.
    #[serde(default)]
    hint: String,
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
    shared: Arc<Shared>,
}

struct Shared {
    token: String,
    retired: Mutex<HashSet<String>>,
    pending: Mutex<HashMap<String, Vec<CancellationToken>>>,
}

fn random_token() -> String {
    // RandomState is seeded from the OS's random source.
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    (0..2u64)
        .map(|i| {
            let mut h = RandomState::new().build_hasher();
            h.write_u128(nanos);
            h.write_u64(i);
            format!("{:016x}", h.finish())
        })
        .collect()
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
        let shared = Arc::new(Shared { token: random_token(), retired: Mutex::new(HashSet::new()), pending: Mutex::new(HashMap::new()) });
        let listener = {
            let _guard = rt.enter();
            ListenerOptions::new().name(endpoint_name(&endpoint)?).create_tokio()?
        };
        let shared2 = shared.clone();
        rt.spawn(async move {
            loop {
                let Ok(conn) = listener.accept().await else { continue };
                let (tx, shared) = (tx.clone(), shared2.clone());
                tokio::spawn(async move {
                    let _ = serve(conn, tx, shared).await;
                });
            }
        });
        Ok((Self { endpoint, helper, shared }, rx))
    }

    /// Environment for an `ssh` process whose prompts should come to this broker.
    pub fn env(&self, tag: &str) -> Vec<(String, String)> {
        let mut env = vec![
            ("SSH_ASKPASS".to_string(), self.helper.display().to_string()),
            ("SSH_ASKPASS_REQUIRE".to_string(), "force".to_string()),
            (ENV_ENDPOINT.to_string(), self.endpoint.clone()),
            (ENV_TAG.to_string(), tag.to_string()),
            (ENV_TOKEN.to_string(), self.shared.token.clone()),
        ];
        if cfg!(unix) && std::env::var_os("DISPLAY").is_none() {
            // untested on this machine: older ssh only uses askpass with DISPLAY set.
            env.push(("DISPLAY".to_string(), "termax".to_string()));
        }
        env
    }

    /// Withdraws every pending and future question for `tag` (e.g. the
    /// connection was closed or failed). Use a fresh tag per connection.
    pub fn retire(&self, tag: &str) {
        self.shared.retired.lock().unwrap().insert(tag.to_string());
        if let Some(tokens) = self.shared.pending.lock().unwrap().remove(tag) {
            for t in tokens {
                t.cancel();
            }
        }
    }
}

async fn serve(conn: TokioStream, tx: UnboundedSender<AskRequest>, shared: Arc<Shared>) -> Result<()> {
    let mut line = String::new();
    let mut read = TokioBufReader::new(&conn);
    read.read_line(&mut line).await?;
    let mut write = &conn;
    let q: Question = serde_json::from_str(line.trim())?;
    let refused = q.token != shared.token || shared.retired.lock().unwrap().contains(&q.tag);
    let answer = if refused {
        None
    } else {
        let (reply, rx) = oneshot::channel();
        let cancelled = CancellationToken::new();
        shared.pending.lock().unwrap().entry(q.tag.clone()).or_default().push(cancelled.clone());
        let kind = AskKind::with_hint(&q.prompt, &q.hint);
        let tag = q.tag.clone();
        tx.send(AskRequest { tag: q.tag, prompt: q.prompt, kind, reply, cancelled: cancelled.clone() })?;
        // The helper sends nothing more: end of input means it is gone (ssh
        // killed it, e.g. once a security key was touched).
        let mut rest = [0u8; 1];
        let answer = tokio::select! {
            a = rx => a.ok().flatten(),
            _ = cancelled.cancelled() => None,
            _ = read.read(&mut rest) => {
                cancelled.cancel();
                None
            }
        };
        if let Some(list) = shared.pending.lock().unwrap().get_mut(&tag) {
            list.retain(|t| !t.is_cancelled() && !std::ptr::eq(t, &cancelled));
        }
        answer
    };
    let answer = match answer {
        Some(a) => Answer { answer: Some(a), cancel: false },
        None => Answer { answer: None, cancel: true },
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
    let token = std::env::var(ENV_TOKEN).unwrap_or_default();
    let prompt = std::env::args().nth(1).unwrap_or_default();
    let hint = std::env::var(ENV_PROMPT_HINT).unwrap_or_default();
    let mut conn = BufReader::new(Stream::connect(endpoint_name(&endpoint)?)?);
    let mut q = serde_json::to_string(&Question { token, tag, prompt, hint })?;
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
        assert_eq!(AskKind::with_hint("Allow use of key?", "confirm"), AskKind::Confirm);
        assert_eq!(AskKind::with_hint("Confirm user presence for key", "none"), AskKind::Notice);
    }
}
