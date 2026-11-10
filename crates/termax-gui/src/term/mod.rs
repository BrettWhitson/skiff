//! Terminal tabs: system ssh in a pty, rendered on our cell grid.

use termax_core::hosts::HostEntry;
use termax_core::terminal::TerminalSession;

pub struct TermTab {
    pub host: HostEntry,
    pub session: Option<TerminalSession>,
    pub error: Option<String>,
}

impl TermTab {
    pub fn new(host: HostEntry) -> Self {
        TermTab { host, session: None, error: None }
    }

    /// Status-bar text.
    pub fn status(&mut self) -> String {
        let state = match (&mut self.session, &self.error) {
            (_, Some(e)) => e.clone(),
            (Some(s), _) => if s.is_alive() { "connected".into() } else { "session ended".into() },
            (None, _) => "starting".into(),
        };
        format!("{}: {state}", self.host.name)
    }
}
