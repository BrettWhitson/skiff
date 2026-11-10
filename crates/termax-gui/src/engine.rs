//! Background work: a tokio runtime plus a message channel back to the UI
//! thread. Every send wakes the event loop, so the UI never polls.

use std::mem::ManuallyDrop;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use tokio::runtime::Runtime;

/// Called after each message is queued (wakes the window's event loop).
pub type WakeFn = Arc<dyn Fn() + Send + Sync>;

/// Messages from background work. Every variant names the tab it belongs
/// to, so results for a closed tab can be dropped.
pub enum Msg {
    /// A status-bar notice (`tab: None` = app-wide).
    Notice {
        tab: Option<u64>,
        text: String,
    },
    /// Background file work for pane `side` of a file tab.
    Files {
        tab: u64,
        side: usize,
        ev: crate::files::FilesEvent,
    },
    /// Progress of a transfer in a file tab.
    Transfer {
        tab: u64,
        ev: termax_core::fs::transfer::TransferEvent,
    },
    /// ssh asks for a password / host-key answer (tag = "tab:side").
    Ask(termax_core::askpass::AskRequest),
    HostStatus {
        host: String,
        status: termax_core::status::Status,
    },
}

impl Msg {
    pub fn tab(&self) -> Option<u64> {
        match self {
            Msg::Notice { tab, .. } => *tab,
            Msg::Files { tab, .. } | Msg::Transfer { tab, .. } => Some(*tab),
            Msg::Ask(req) => crate::files::parse_tag(&req.tag).map(|(t, _)| t),
            Msg::HostStatus { .. } => None,
        }
    }
}

#[derive(Clone)]
pub struct MsgSender {
    tx: Sender<Msg>,
    wake: Option<WakeFn>,
}

impl MsgSender {
    pub fn send(&self, m: Msg) {
        if self.tx.send(m).is_ok()
            && let Some(w) = &self.wake
        {
            w();
        }
    }
}

pub struct Engine {
    /// Dropped with `shutdown_background`: closing a window or tab never
    /// waits for a stuck connection.
    pub rt: ManuallyDrop<Runtime>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    wake: Option<WakeFn>,
}

impl Engine {
    pub fn new(wake: Option<WakeFn>) -> Engine {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("termax-engine")
            .enable_all()
            .build()
            .expect("tokio runtime");
        let (tx, rx) = channel();
        Engine { rt: ManuallyDrop::new(rt), tx, rx, wake }
    }

    pub fn sender(&self) -> MsgSender {
        MsgSender { tx: self.tx.clone(), wake: self.wake.clone() }
    }

    pub fn wake_fn(&self) -> Option<WakeFn> {
        self.wake.clone()
    }

    /// Everything queued so far (call `drain` from the UI thread only).
    pub fn drain(&self) -> Vec<Msg> {
        self.rx.try_iter().collect()
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // SAFETY: `rt` is never used again after this.
        let rt = unsafe { ManuallyDrop::take(&mut self.rt) };
        rt.shutdown_background();
    }
}
