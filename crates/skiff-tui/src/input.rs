use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// A single-line text field. `cursor` is a char index.
#[derive(Debug, Clone, Default)]
pub struct TextInput {
    pub value: String,
    pub cursor: usize,
}

impl TextInput {
    pub fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        let cursor = value.chars().count();
        Self { value, cursor }
    }

    fn byte_idx(&self, char_idx: usize) -> usize {
        self.value.char_indices().nth(char_idx).map(|(i, _)| i).unwrap_or(self.value.len())
    }

    pub fn insert_str(&mut self, s: &str) {
        let s: String = s.chars().filter(|c| !c.is_control()).collect();
        let at = self.byte_idx(self.cursor);
        self.value.insert_str(at, &s);
        self.cursor += s.chars().count();
    }

    /// Text before the cursor, used to place the terminal cursor when rendering.
    pub fn before_cursor(&self) -> &str {
        &self.value[..self.byte_idx(self.cursor)]
    }

    /// Returns true if the value changed.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let len = self.value.chars().count();
        match key.code {
            KeyCode::Char('u') if ctrl => {
                let at = self.byte_idx(self.cursor);
                self.value.drain(..at);
                self.cursor = 0;
                true
            }
            KeyCode::Char('a') if ctrl => {
                self.cursor = 0;
                false
            }
            KeyCode::Char('e') if ctrl => {
                self.cursor = len;
                false
            }
            KeyCode::Char('w') if ctrl => {
                let before: Vec<char> = self.value.chars().take(self.cursor).collect();
                let mut i = before.len();
                while i > 0 && before[i - 1] == ' ' {
                    i -= 1;
                }
                while i > 0 && before[i - 1] != ' ' {
                    i -= 1;
                }
                let (start, end) = (self.byte_idx(i), self.byte_idx(self.cursor));
                self.value.drain(start..end);
                self.cursor = i;
                true
            }
            KeyCode::Char(c) if !ctrl => {
                let at = self.byte_idx(self.cursor);
                self.value.insert(at, c);
                self.cursor += 1;
                true
            }
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                let at = self.byte_idx(self.cursor);
                self.value.remove(at);
                true
            }
            KeyCode::Delete if self.cursor < len => {
                let at = self.byte_idx(self.cursor);
                self.value.remove(at);
                true
            }
            KeyCode::Left => {
                self.cursor = self.cursor.saturating_sub(1);
                false
            }
            KeyCode::Right => {
                self.cursor = (self.cursor + 1).min(len);
                false
            }
            KeyCode::Home => {
                self.cursor = 0;
                false
            }
            KeyCode::End => {
                self.cursor = len;
                false
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    #[test]
    fn edits() {
        let mut t = TextInput::new("héllo");
        t.handle_key(key(KeyCode::Left));
        t.handle_key(key(KeyCode::Backspace));
        assert_eq!(t.value, "hélo");
        t.handle_key(key(KeyCode::Home));
        t.handle_key(key(KeyCode::Char('>')));
        assert_eq!(t.value, ">hélo");
        assert_eq!(t.before_cursor(), ">");
        t.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));
        t.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(t.value, "");
    }
}
