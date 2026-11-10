//! Clipboard access behind a trait, so widgets are testable.

pub trait Clipboard {
    fn get(&mut self) -> Option<String>;
    fn set(&mut self, text: &str);
}

/// The OS clipboard (via arboard). Opened lazily; failures read as empty.
#[derive(Default)]
pub struct SystemClipboard {
    inner: Option<arboard::Clipboard>,
}

impl SystemClipboard {
    fn inner(&mut self) -> Option<&mut arboard::Clipboard> {
        if self.inner.is_none() {
            self.inner = arboard::Clipboard::new().ok();
        }
        self.inner.as_mut()
    }
}

impl Clipboard for SystemClipboard {
    fn get(&mut self) -> Option<String> {
        self.inner()?.get_text().ok()
    }

    fn set(&mut self, text: &str) {
        if let Some(c) = self.inner() {
            let _ = c.set_text(text.to_owned());
        }
    }
}

/// In-memory clipboard for tests and headless use.
#[derive(Default, Debug, Clone)]
pub struct MemoryClipboard {
    pub text: Option<String>,
}

impl Clipboard for MemoryClipboard {
    fn get(&mut self) -> Option<String> {
        self.text.clone()
    }

    fn set(&mut self, text: &str) {
        self.text = Some(text.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_clipboard_roundtrip() {
        let mut c = MemoryClipboard::default();
        assert_eq!(c.get(), None);
        c.set("héllo");
        assert_eq!(c.get().as_deref(), Some("héllo"));
    }
}
