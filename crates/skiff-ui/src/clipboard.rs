//! Clipboard access behind a trait, so widgets are testable: the shell
//! hands `Ui` the OS clipboard, tests hand it a `MemoryClipboard`.

/// Plain-text clipboard used by text inputs and the app.
pub trait Clipboard {
    /// The clipboard's text, or `None` if it is empty or unreadable.
    fn get(&mut self) -> Option<String>;
    /// Replaces the clipboard's contents with `text`.
    fn set(&mut self, text: &str);
}

/// The OS clipboard (via arboard). Opened lazily; failures read as empty.
#[derive(Default)]
pub struct SystemClipboard {
    inner: Option<arboard::Clipboard>,
}

impl SystemClipboard {
    /// Opens the OS clipboard on first use; tries again on later calls if that failed.
    fn inner(&mut self) -> Option<&mut arboard::Clipboard> {
        if self.inner.is_none() {
            self.inner = arboard::Clipboard::new().ok();
        }
        self.inner.as_mut()
    }
}

impl Clipboard for SystemClipboard {
    /// Reads text from the OS clipboard; any error reads as `None`.
    fn get(&mut self) -> Option<String> {
        self.inner()?.get_text().ok()
    }

    /// Writes text to the OS clipboard; errors are ignored.
    fn set(&mut self, text: &str) {
        if let Some(c) = self.inner() {
            let _ = c.set_text(text.to_owned());
        }
    }
}

/// In-memory clipboard for tests and headless use.
#[derive(Default, Debug, Clone)]
pub struct MemoryClipboard {
    /// The stored text, if anything was set.
    pub text: Option<String>,
}

impl Clipboard for MemoryClipboard {
    /// Returns a copy of the stored text.
    fn get(&mut self) -> Option<String> {
        self.text.clone()
    }

    /// Stores `text`.
    fn set(&mut self, text: &str) {
        self.text = Some(text.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Text set on a memory clipboard (including non-ASCII) reads back unchanged.
    #[test]
    fn memory_clipboard_roundtrip() {
        let mut c = MemoryClipboard::default();
        assert_eq!(c.get(), None);
        c.set("héllo");
        assert_eq!(c.get().as_deref(), Some("héllo"));
    }
}
