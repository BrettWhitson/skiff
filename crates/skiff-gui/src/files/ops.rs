//! File operations run on the engine; results come back as `Msg::Files`.

use skiff_core::fs::FileEntry;
use skiff_core::fs::connect::{SshLaunch, connect_sftp};
use skiff_core::fs::endpoint::Endpoint;

use super::FilesEvent;
use crate::engine::{Engine, Msg};

#[derive(Debug, Clone)]
pub enum Op {
    Mkdir(String),
    Rename {
        from: String,
        to: String,
    },
    /// (path, is_dir)
    Delete(Vec<(String, bool)>),
}

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

pub fn list(engine: &Engine, tab: u64, side: usize, seq: u64, ep: Endpoint, dir: String) {
    let tx = engine.sender();
    engine.rt.spawn(async move {
        let result: Result<Vec<FileEntry>, String> = ep.list(&dir).await.map_err(err);
        tx.send(Msg::Files { tab, side, ev: FilesEvent::Listed { seq, cwd: dir, result } });
    });
}

/// Opens an SFTP session; `note` is appended to a failure message.
/// Aborting the returned handle kills the ssh process.
pub fn connect(engine: &Engine, tab: u64, side: usize, seq: u64, launch: SshLaunch, note: Option<String>) -> tokio::task::AbortHandle {
    let tx = engine.sender();
    engine
        .rt
        .spawn(async move {
            let result = connect_sftp(launch).await.map(Box::new).map_err(|e| match note {
                Some(n) => format!("{} \u{2014} {n}", e.trim()),
                None => e.trim().to_string(),
            });
            tx.send(Msg::Files { tab, side, ev: FilesEvent::Connected { seq, result } });
        })
        .abort_handle()
}

pub fn run(engine: &Engine, tab: u64, side: usize, ep: Endpoint, op: Op) {
    let tx = engine.sender();
    engine.rt.spawn(async move {
        let result = match &op {
            Op::Mkdir(p) => ep.mkdir(p).await.map_err(err),
            Op::Rename { from, to } => ep.rename(from, to).await.map_err(err),
            Op::Delete(items) => {
                let mut r = Ok(());
                for (p, is_dir) in items {
                    if let Err(e) = ep.remove(p, *is_dir).await {
                        r = Err(err(e));
                        break;
                    }
                }
                r
            }
        };
        tx.send(Msg::Files { tab, side, ev: FilesEvent::OpDone { result } });
    });
}
