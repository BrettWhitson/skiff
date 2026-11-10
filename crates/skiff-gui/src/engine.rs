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
        /// Owning tab id, or `None` for an app-wide notice.
        tab: Option<u64>,
        /// The message shown to the user.
        text: String,
    },
    /// Background file work for pane `side` of a file tab.
    Files {
        /// File tab id.
        tab: u64,
        /// Which pane: 0 = left, 1 = right.
        side: usize,
        /// The result (listing, error, ...).
        ev: crate::files::FilesEvent,
    },
    /// Progress of a transfer in a file tab.
    Transfer {
        /// File tab id.
        tab: u64,
        /// Progress, completion or failure of one transfer.
        ev: skiff_core::fs::transfer::TransferEvent,
    },
    /// ssh asks for a password / host-key answer (tag = "tab:side").
    Ask(skiff_core::askpass::AskRequest),
    /// A reachability probe finished for a sidebar host.
    HostStatus {
        /// The host's alias.
        host: String,
        /// Probe result (drives the sidebar status dot).
        status: skiff_core::status::Status,
    },
}

impl Msg {
    /// The tab this message belongs to, if any; `None` means app-wide
    /// (an `Ask` with an unparsable tag also yields `None`).
    pub fn tab(&self) -> Option<u64> {
        match self {
            Msg::Notice { tab, .. } => *tab,
            Msg::Files { tab, .. } | Msg::Transfer { tab, .. } => Some(*tab),
            Msg::Ask(req) => crate::files::parse_tag(&req.tag).map(|(t, _)| t),
            Msg::HostStatus { .. } => None,
        }
    }
}

/// Cloneable handle background tasks use to post [`Msg`]s to the UI thread.
#[derive(Clone)]
pub struct MsgSender {
    /// Channel into the engine's queue.
    tx: Sender<Msg>,
    /// Wakes the event loop after a send (`None` in headless tests).
    wake: Option<WakeFn>,
}

impl MsgSender {
    /// Queues `m` and wakes the UI; a send after the engine is gone is
    /// silently dropped.
    pub fn send(&self, m: Msg) {
        if self.tx.send(m).is_ok()
            && let Some(w) = &self.wake
        {
            w();
        }
    }
}

/// Owns the background runtime and the receiving end of the UI message
/// queue. One per app window.
pub struct Engine {
    /// Dropped with `shutdown_background`: closing a window or tab never
    /// waits for a stuck connection.
    pub rt: ManuallyDrop<Runtime>,
    /// Kept so [`Engine::sender`] can hand out more senders.
    tx: Sender<Msg>,
    /// Drained by the UI thread each frame.
    rx: Receiver<Msg>,
    /// Event-loop waker passed on to every sender.
    wake: Option<WakeFn>,
}

impl Engine {
    /// Starts a two-worker tokio runtime; `wake` (if any) is called after
    /// every message so the window redraws. Panics if the runtime can't start.
    pub fn new(wake: Option<WakeFn>) -> Engine {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("skiff-engine")
            .enable_all()
            .build()
            .expect("tokio runtime");
        let (tx, rx) = channel();
        Engine { rt: ManuallyDrop::new(rt), tx, rx, wake }
    }

    /// A new sender for a background task.
    pub fn sender(&self) -> MsgSender {
        MsgSender { tx: self.tx.clone(), wake: self.wake.clone() }
    }

    /// The event-loop waker, for code that must wake the UI without a message
    /// (e.g. terminal output).
    pub fn wake_fn(&self) -> Option<WakeFn> {
        self.wake.clone()
    }

    /// Everything queued so far (call `drain` from the UI thread only).
    pub fn drain(&self) -> Vec<Msg> {
        self.rx.try_iter().collect()
    }
}

impl Drop for Engine {
    /// Shuts the runtime down without waiting for running tasks.
    fn drop(&mut self) {
        // SAFETY: `rt` is never used again after this.
        let rt = unsafe { ManuallyDrop::take(&mut self.rt) };
        rt.shutdown_background();
    }
}
