//! Interactive SSH sessions: the system `ssh` client runs inside a pseudo-terminal
//! and its output is parsed by `vt100` for rendering in a tab.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

/// Called from the reader thread after output is processed or the session ends.
pub type NotifyFn = Arc<dyn Fn() + Send + Sync>;

pub struct TerminalSession {
    pub parser: Arc<Mutex<vt100::Parser>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    exited: Arc<AtomicBool>,
    size: (u16, u16),
    program: String,
    args: Vec<String>,
    scrollback: usize,
    notify: Arc<Mutex<Option<NotifyFn>>>,
    generation: Arc<AtomicU64>,
}

impl TerminalSession {
    pub fn spawn(program: &str, args: &[String], rows: u16, cols: u16, scrollback: usize) -> Result<Self> {
        Self::spawn_with(program, args, rows, cols, scrollback, Arc::default(), Arc::default())
    }

    fn spawn_with(
        program: &str,
        args: &[String],
        rows: u16,
        cols: u16,
        scrollback: usize,
        notify: Arc<Mutex<Option<NotifyFn>>>,
        generation: Arc<AtomicU64>,
    ) -> Result<Self> {
        let rows = rows.max(2);
        let cols = cols.max(10);
        let pair = native_pty_system()
            .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .context("opening pseudo-terminal")?;

        let mut cmd = CommandBuilder::new(program);
        cmd.args(args);
        if std::env::var_os("TERM").is_none() {
            cmd.env("TERM", "xterm-256color");
        }
        let child = pair.slave.spawn_command(cmd).with_context(|| format!("starting `{program}`"))?;
        // The slave handle must be closed here or the reader never sees EOF.
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader()?;
        let writer = Arc::new(Mutex::new(pair.master.take_writer()?));
        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, scrollback)));
        let exited = Arc::new(AtomicBool::new(false));

        let (p, e, w) = (parser.clone(), exited.clone(), writer.clone());
        let (nf, gen_) = (notify.clone(), generation.clone());
        let ping = move || {
            let f = nf.lock().ok().and_then(|g| g.clone());
            if let Some(f) = f {
                f();
            }
        };
        std::thread::spawn(move || {
            let mut buf = [0u8; 16 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if std::env::var_os("TERMAX_DEBUG_PTY").is_some() {
                            eprintln!("pty read {n}: {:?}", String::from_utf8_lossy(&buf[..n]));
                        }
                        let chunk = &buf[..n];
                        let mut reply = Vec::new();
                        if let Ok(mut parser) = p.lock() {
                            parser.process(chunk);
                            // Answer cursor-position queries (DSR). Windows ConPTY sends one
                            // at startup and withholds all output until it gets a reply.
                            for _ in 0..count(chunk, b"\x1b[6n") {
                                let (row, col) = parser.screen().cursor_position();
                                reply.extend(format!("\x1b[{};{}R", row + 1, col + 1).into_bytes());
                            }
                        }
                        if !reply.is_empty() {
                            if let Ok(mut w) = w.lock() {
                                let _ = w.write_all(&reply);
                                let _ = w.flush();
                            }
                        }
                        gen_.fetch_add(1, Ordering::SeqCst);
                        ping();
                    }
                }
            }
            e.store(true, Ordering::SeqCst);
            gen_.fetch_add(1, Ordering::SeqCst);
            ping();
        });

        Ok(Self {
            parser,
            writer,
            master: pair.master,
            child,
            exited,
            size: (rows, cols),
            program: program.to_string(),
            args: args.to_vec(),
            scrollback,
            notify,
            generation,
        })
    }

    /// Calls `f` (from the reader thread) whenever output arrives or the
    /// session ends. Survives `respawn`.
    pub fn set_notify(&mut self, f: NotifyFn) {
        if let Ok(mut g) = self.notify.lock() {
            *g = Some(f);
        }
    }

    /// Increases each time output is processed; unchanged means the screen is too.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Starts the same command again in a fresh pty.
    pub fn respawn(&mut self) -> Result<()> {
        let _ = self.child.kill();
        *self = Self::spawn_with(
            &self.program.clone(),
            &self.args.clone(),
            self.size.0,
            self.size.1,
            self.scrollback,
            self.notify.clone(),
            self.generation.clone(),
        )?;
        Ok(())
    }

    pub fn write(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.set_scroll(0);
        if let Ok(mut w) = self.writer.lock() {
            let _ = w.write_all(bytes);
            let _ = w.flush();
        }
    }

    pub fn paste(&mut self, text: &str) {
        let bracketed = self.parser.lock().map(|p| p.screen().bracketed_paste()).unwrap_or(false);
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        if bracketed {
            self.write(format!("\x1b[200~{text}\x1b[201~").as_bytes());
        } else {
            self.write(text.as_bytes());
        }
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        let (rows, cols) = (rows.max(2), cols.max(10));
        if (rows, cols) == self.size {
            return;
        }
        self.size = (rows, cols);
        let _ = self.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
        if let Ok(mut p) = self.parser.lock() {
            p.screen_mut().set_size(rows, cols);
        }
    }

    /// Scrolls the view back by `delta` lines (negative scrolls toward live output).
    pub fn scroll(&mut self, delta: i32) {
        let cur = self.scroll_offset() as i32;
        self.set_scroll((cur + delta).max(0) as usize);
    }

    fn set_scroll(&mut self, rows: usize) {
        if let Ok(mut p) = self.parser.lock() {
            p.screen_mut().set_scrollback(rows);
        }
    }

    pub fn scroll_offset(&self) -> usize {
        self.parser.lock().map(|p| p.screen().scrollback()).unwrap_or(0)
    }

    pub fn rows(&self) -> u16 {
        self.size.0
    }

    pub fn is_alive(&mut self) -> bool {
        if self.exited.load(Ordering::SeqCst) {
            return false;
        }
        matches!(self.child.try_wait(), Ok(None))
    }

    pub fn application_cursor(&self) -> bool {
        self.parser.lock().map(|p| p.screen().application_cursor()).unwrap_or(false)
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

/// Encodes a key press as the bytes an xterm-compatible terminal would send.
pub fn encode_key(key: KeyEvent, app_cursor: bool) -> Vec<u8> {
    let m = key.modifiers;
    let ctrl = m.contains(KeyModifiers::CONTROL);
    let alt = m.contains(KeyModifiers::ALT);
    let shift = m.contains(KeyModifiers::SHIFT);
    // xterm modifier parameter: 1 + shift + 2*alt + 4*ctrl
    let modp = 1 + shift as u8 + 2 * alt as u8 + 4 * ctrl as u8;

    let csi_letter = |letter: char| -> Vec<u8> {
        if modp > 1 {
            format!("\x1b[1;{modp}{letter}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{letter}").into_bytes()
        } else {
            format!("\x1b[{letter}").into_bytes()
        }
    };
    let csi_tilde = |n: u8| -> Vec<u8> {
        if modp > 1 {
            format!("\x1b[{n};{modp}~").into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };

    let mut out = match key.code {
        KeyCode::Char(c) if ctrl => {
            let b = match c.to_ascii_lowercase() {
                c @ 'a'..='z' => (c as u8) & 0x1f,
                ' ' | '@' | '2' => 0,
                '[' | '3' => 0x1b,
                '\\' | '4' => 0x1c,
                ']' | '5' => 0x1d,
                '^' | '6' => 0x1e,
                '_' | '-' | '7' => 0x1f,
                '8' | '?' => 0x7f,
                other => {
                    let mut s = [0u8; 4];
                    return with_alt(alt, other.encode_utf8(&mut s).as_bytes().to_vec());
                }
            };
            vec![b]
        }
        KeyCode::Char(c) => {
            let mut s = [0u8; 4];
            c.encode_utf8(&mut s).as_bytes().to_vec()
        }
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => {
            if ctrl {
                vec![0x08]
            } else {
                vec![0x7f]
            }
        }
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => return csi_letter('A'),
        KeyCode::Down => return csi_letter('B'),
        KeyCode::Right => return csi_letter('C'),
        KeyCode::Left => return csi_letter('D'),
        KeyCode::Home => return csi_letter('H'),
        KeyCode::End => return csi_letter('F'),
        KeyCode::Insert => return csi_tilde(2),
        KeyCode::Delete => return csi_tilde(3),
        KeyCode::PageUp => return csi_tilde(5),
        KeyCode::PageDown => return csi_tilde(6),
        KeyCode::F(n @ 1..=4) => {
            let letter = (b'P' + n - 1) as char;
            return if modp > 1 {
                format!("\x1b[1;{modp}{letter}").into_bytes()
            } else {
                format!("\x1bO{letter}").into_bytes()
            };
        }
        KeyCode::F(n @ 5..=12) => {
            let code = [15, 17, 18, 19, 20, 21, 23, 24][(n - 5) as usize];
            return csi_tilde(code);
        }
        _ => return vec![],
    };
    if alt {
        out = with_alt(true, out);
    }
    out
}

fn count(hay: &[u8], needle: &[u8]) -> usize {
    hay.windows(needle.len()).filter(|w| *w == needle).count()
}

fn with_alt(alt: bool, bytes: Vec<u8>) -> Vec<u8> {
    if alt {
        let mut v = vec![0x1b];
        v.extend(bytes);
        v
    } else {
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, m)
    }

    #[test]
    fn pty_session_captures_output() {
        #[cfg(windows)]
        let (prog, args) = ("cmd.exe", vec!["/c".to_string(), "echo termax-ok".to_string()]);
        #[cfg(not(windows))]
        let (prog, args) = ("sh", vec!["-c".to_string(), "echo termax-ok".to_string()]);
        let mut s = TerminalSession::spawn(prog, &args, 10, 40, 100).unwrap();
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_secs(10) {
            if s.parser.lock().unwrap().screen().contents().contains("termax-ok") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let contents = s.parser.lock().unwrap().screen().contents();
        assert!(contents.contains("termax-ok"), "screen was: {contents:?}");
        s.resize(20, 60);
        assert_eq!(s.rows(), 20);
    }

    #[test]
    fn encodes_keys() {
        let n = KeyModifiers::NONE;
        assert_eq!(encode_key(k(KeyCode::Char('a'), n), false), b"a");
        assert_eq!(encode_key(k(KeyCode::Char('c'), KeyModifiers::CONTROL), false), [3]);
        assert_eq!(encode_key(k(KeyCode::Char('b'), KeyModifiers::ALT), false), b"\x1bb");
        assert_eq!(encode_key(k(KeyCode::Up, n), false), b"\x1b[A");
        assert_eq!(encode_key(k(KeyCode::Up, n), true), b"\x1bOA");
        assert_eq!(encode_key(k(KeyCode::Right, KeyModifiers::CONTROL), false), b"\x1b[1;5C");
        assert_eq!(encode_key(k(KeyCode::Delete, n), false), b"\x1b[3~");
        assert_eq!(encode_key(k(KeyCode::F(1), n), false), b"\x1bOP");
        assert_eq!(encode_key(k(KeyCode::F(12), n), false), b"\x1b[24~");
        assert_eq!(encode_key(k(KeyCode::Enter, n), false), b"\r");
        assert_eq!(encode_key(k(KeyCode::Char('é'), n), false), "é".as_bytes());
    }
}
