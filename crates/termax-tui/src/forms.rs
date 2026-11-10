//! The host editor and the port-forward form (modal dialogs).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use termax_core::forward::{Forward, ForwardKind};
use termax_core::hostform::{Dest, Field, HostForm};
use termax_core::sshconfig::edit::{ConfigSet, display_path};

use crate::input::TextInput;

/// What a key press did to a form.
pub enum Outcome {
    Keep,
    Cancel,
    Save,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Dest,
    Field(Field),
    Hidden,
    Other(usize),
}

pub struct Editor {
    pub form: HostForm,
    /// The ssh config files (empty when ssh config editing is unavailable).
    pub set: ConfigSet,
    pub inputs: Vec<(Field, TextInput)>,
    /// "Other options", one input per line (ssh config hosts only).
    pub other: Vec<TextInput>,
    pub focus: usize,
    pub error: Option<String>,
}

impl Editor {
    pub fn new(form: HostForm, set: ConfigSet) -> Editor {
        let mut e = Editor { form, set, inputs: vec![], other: vec![], focus: 0, error: None };
        e.load();
        e
    }

    fn load(&mut self) {
        self.inputs = self.form.fields().into_iter().map(|f| (f, TextInput::new(self.form.get(f)))).collect();
        self.other = self.form.other.lines().map(TextInput::new).collect();
        if self.other.is_empty() {
            self.other.push(TextInput::default());
        }
    }

    /// Copies the inputs into the form.
    pub fn sync(&mut self) {
        for (f, i) in &self.inputs {
            *self.form.get_mut(*f) = i.value.clone();
        }
        self.form.other = self.other.iter().map(|i| i.value.clone()).collect::<Vec<_>>().join("\n");
    }

    fn ssh_dest(&self) -> bool {
        matches!(self.form.dest, Dest::SshConfig(_))
    }

    pub fn rows(&self) -> Vec<Row> {
        let mut r = Vec::new();
        if self.form.can_change_dest() && !self.set.docs.is_empty() {
            r.push(Row::Dest);
        }
        r.extend(self.inputs.iter().map(|(f, _)| Row::Field(*f)));
        r.push(Row::Hidden);
        if self.ssh_dest() {
            r.extend((0..self.other.len()).map(Row::Other));
        }
        r
    }

    pub fn title(&self) -> &'static str {
        if self.form.can_change_dest() { " New host " } else { " Edit host " }
    }

    pub fn dest_label(&self) -> String {
        match self.form.dest {
            Dest::Termax => "termax config".into(),
            Dest::SshConfig(i) => self.set.docs.get(i).map(|d| display_path(&d.path)).unwrap_or_default(),
        }
    }

    /// Cycles termax config → each ssh config file → termax config.
    fn next_dest(&mut self) {
        self.sync();
        self.form.dest = match self.form.dest {
            Dest::Termax if !self.set.docs.is_empty() => Dest::SshConfig(0),
            Dest::SshConfig(i) if i + 1 < self.set.docs.len() => Dest::SshConfig(i + 1),
            _ => Dest::Termax,
        };
        self.load();
    }

    pub fn input(&self, f: Field) -> Option<&TextInput> {
        self.inputs.iter().find(|(g, _)| *g == f).map(|(_, i)| i)
    }

    pub fn focused_input(&mut self) -> Option<&mut TextInput> {
        match self.rows().get(self.focus).copied()? {
            Row::Field(f) => self.inputs.iter_mut().find(|(g, _)| *g == f).map(|(_, i)| i),
            Row::Other(i) => self.other.get_mut(i),
            _ => None,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        let rows = self.rows();
        let n = rows.len();
        self.focus = self.focus.min(n - 1);
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
            self.sync();
            return Outcome::Save;
        }
        match (key.code, rows[self.focus]) {
            (KeyCode::Esc, _) => return Outcome::Cancel,
            (KeyCode::Tab | KeyCode::Down, _) => self.focus = (self.focus + 1) % n,
            (KeyCode::BackTab | KeyCode::Up, _) => self.focus = (self.focus + n - 1) % n,
            (KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right | KeyCode::Enter, Row::Dest) => self.next_dest(),
            (KeyCode::Char(' ') | KeyCode::Enter, Row::Hidden) => self.form.hidden = !self.form.hidden,
            (KeyCode::Enter, Row::Other(i)) => {
                self.other.insert(i + 1, TextInput::default());
                self.focus += 1;
            }
            (KeyCode::Backspace, Row::Other(i)) if self.other[i].value.is_empty() && self.other.len() > 1 => {
                self.other.remove(i);
                self.focus -= 1;
            }
            (KeyCode::Enter, _) if self.focus == n - 1 => {
                self.sync();
                return Outcome::Save;
            }
            (KeyCode::Enter, _) => self.focus += 1,
            _ => {
                if let Some(i) = self.focused_input() {
                    i.handle_key(key);
                }
            }
        }
        Outcome::Keep
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FwdRow {
    Kind,
    Name,
    Bind,
    Listen,
    Target,
}

pub struct ForwardForm {
    pub host: String,
    pub kind: ForwardKind,
    pub name: TextInput,
    pub bind: TextInput,
    pub listen: TextInput,
    pub target: TextInput,
    /// Index into `Config::forwards` when editing.
    pub editing: Option<usize>,
    pub focus: usize,
    pub error: Option<String>,
}

impl ForwardForm {
    pub fn new(host: &str) -> ForwardForm {
        ForwardForm {
            host: host.into(),
            kind: ForwardKind::Local,
            name: TextInput::default(),
            bind: TextInput::default(),
            listen: TextInput::default(),
            target: TextInput::new("localhost:"),
            editing: None,
            focus: 0,
            error: None,
        }
    }

    /// The form for saved forward `f` (index `i` in `Config::forwards`).
    pub fn edit(f: &termax_core::forward::Forward, i: usize) -> ForwardForm {
        ForwardForm {
            kind: f.kind,
            name: TextInput::new(f.name.clone().unwrap_or_default()),
            bind: TextInput::new(f.bind.clone().unwrap_or_default()),
            listen: TextInput::new(f.listen.to_string()),
            target: TextInput::new(f.target.clone().unwrap_or_default()),
            editing: Some(i),
            ..ForwardForm::new(&f.host)
        }
    }

    pub fn rows(&self) -> Vec<FwdRow> {
        let mut r = vec![FwdRow::Kind, FwdRow::Name, FwdRow::Bind, FwdRow::Listen];
        if self.kind != ForwardKind::Dynamic {
            r.push(FwdRow::Target);
        }
        r
    }

    pub fn label(row: FwdRow) -> &'static str {
        match row {
            FwdRow::Kind => "Kind",
            FwdRow::Name => "Name",
            FwdRow::Bind => "Bind address",
            FwdRow::Listen => "Listen port",
            FwdRow::Target => "Target",
        }
    }

    pub fn input(&self, row: FwdRow) -> Option<&TextInput> {
        match row {
            FwdRow::Kind => None,
            FwdRow::Name => Some(&self.name),
            FwdRow::Bind => Some(&self.bind),
            FwdRow::Listen => Some(&self.listen),
            FwdRow::Target => Some(&self.target),
        }
    }

    pub fn focused_input(&mut self) -> Option<&mut TextInput> {
        match self.rows().get(self.focus)? {
            FwdRow::Kind => None,
            FwdRow::Name => Some(&mut self.name),
            FwdRow::Bind => Some(&mut self.bind),
            FwdRow::Listen => Some(&mut self.listen),
            FwdRow::Target => Some(&mut self.target),
        }
    }

    pub fn to_forward(&self) -> Result<Forward, String> {
        let opt = |i: &TextInput| Some(i.value.trim().to_string()).filter(|s| !s.is_empty());
        let listen = self.listen.value.trim().parse::<u16>().ok().filter(|p| *p > 0).ok_or("Listen port must be 1–65535")?;
        let target = if self.kind == ForwardKind::Dynamic { None } else { opt(&self.target) };
        let f = Forward { name: opt(&self.name), host: self.host.clone(), kind: self.kind, bind: opt(&self.bind), listen, target };
        f.validate()?;
        Ok(f)
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        let rows = self.rows();
        let n = rows.len();
        self.focus = self.focus.min(n - 1);
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
            return Outcome::Save;
        }
        match (key.code, rows[self.focus]) {
            (KeyCode::Esc, _) => return Outcome::Cancel,
            (KeyCode::Tab | KeyCode::Down, _) => self.focus = (self.focus + 1) % n,
            (KeyCode::BackTab | KeyCode::Up, _) => self.focus = (self.focus + n - 1) % n,
            (KeyCode::Char(' ') | KeyCode::Right | KeyCode::Enter, FwdRow::Kind) => {
                let i = ForwardKind::ALL.iter().position(|k| *k == self.kind).unwrap_or(0);
                self.kind = ForwardKind::ALL[(i + 1) % ForwardKind::ALL.len()];
            }
            (KeyCode::Left, FwdRow::Kind) => {
                let i = ForwardKind::ALL.iter().position(|k| *k == self.kind).unwrap_or(0);
                self.kind = ForwardKind::ALL[(i + ForwardKind::ALL.len() - 1) % ForwardKind::ALL.len()];
            }
            (KeyCode::Enter, _) if self.focus == n - 1 => return Outcome::Save,
            (KeyCode::Enter, _) => self.focus += 1,
            _ => {
                if let Some(i) = self.focused_input() {
                    i.handle_key(key);
                }
            }
        }
        Outcome::Keep
    }
}
