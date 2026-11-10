//! The host form shared by the TUI and GUI: one set of fields that saves
//! either to termax's config (`[[connection]]`) or in place to an ssh config
//! file, plus the quick host actions (hide, delete, duplicate).

use crate::config::{CONNECTION_FIELDS, Config, Connection};
use crate::hosts::{HostEntry, Source};
use crate::sshconfig::doc::HostFields;
use crate::sshconfig::edit::{ConfigSet, Locate, validate_with};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Name,
    Host,
    User,
    Port,
    Identity,
    ProxyJump,
    ProxyCommand,
    ExtraArgs,
    RemoteDir,
    Tags,
}

impl Field {
    /// In `Connection::to_fields` order.
    pub const ALL: [Field; CONNECTION_FIELDS] = [
        Field::Name,
        Field::Host,
        Field::User,
        Field::Port,
        Field::Identity,
        Field::ProxyJump,
        Field::ProxyCommand,
        Field::ExtraArgs,
        Field::RemoteDir,
        Field::Tags,
    ];

    pub fn label(self) -> &'static str {
        crate::config::CONNECTION_LABELS[self as usize]
    }

    /// A short hint shown with the field.
    pub fn hint(self) -> &'static str {
        match self {
            Field::Host => "name or address",
            Field::Port => "default 22",
            Field::ProxyJump => "jump host, e.g. bastion",
            Field::ProxyCommand => "e.g. ssh -W %h:%p bastion",
            Field::ExtraArgs => "shell-quoted, e.g. -A -o Compression=yes",
            Field::RemoteDir => "where file transfer opens",
            Field::Tags => "comma-separated",
            _ => "",
        }
    }
}

/// Where a host form saves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dest {
    /// termax's own `config.toml`.
    Termax,
    /// An ssh config file (index into `ConfigSet::docs`).
    SshConfig(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Editing {
    New,
    Custom(usize),
    Ssh(String),
}

/// What a successful save changed, so the caller writes the right file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Saved {
    Termax(String),
    Ssh(String),
}

impl Saved {
    pub fn name(&self) -> &str {
        match self {
            Saved::Termax(n) | Saved::Ssh(n) => n,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HostForm {
    pub dest: Dest,
    pub editing: Editing,
    values: [String; CONNECTION_FIELDS],
    pub hidden: bool,
    /// Other ssh options, one per line (ssh config hosts only).
    pub other: String,
    /// A new ssh config host goes right after this one (duplicates).
    pub after: Option<String>,
}

impl HostForm {
    pub fn new_host(dest: Dest) -> HostForm {
        HostForm { dest, editing: Editing::New, values: Default::default(), hidden: false, other: String::new(), after: None }
    }

    pub fn for_custom(i: usize, c: &Connection) -> HostForm {
        HostForm {
            dest: Dest::Termax,
            editing: Editing::Custom(i),
            values: c.to_fields(),
            hidden: c.hidden,
            other: String::new(),
            after: None,
        }
    }

    pub fn for_ssh(doc: usize, alias: &str, f: &HostFields) -> HostForm {
        let mut form = HostForm::new_host(Dest::SshConfig(doc));
        form.editing = Editing::Ssh(alias.to_string());
        let v = &mut form.values;
        v[Field::Name as usize] = f.alias.clone();
        v[Field::Host as usize] = f.hostname.clone();
        v[Field::User as usize] = f.user.clone();
        v[Field::Port as usize] = f.port.clone();
        v[Field::Identity as usize] = f.identity_file.clone();
        v[Field::ProxyJump as usize] = f.proxy_jump.clone();
        v[Field::ProxyCommand as usize] = f.proxy_command.clone();
        v[Field::RemoteDir as usize] = f.remote_dir.clone();
        v[Field::Tags as usize] = f.tags.join(", ");
        form.hidden = f.hidden;
        form.other = f.other.join("\n");
        form
    }

    /// The form for host `h`, or why it can't be edited.
    pub fn for_entry(h: &HostEntry, cfg: &Config, set: &ConfigSet) -> Result<HostForm, String> {
        match h.source {
            Source::Custom(i) => Ok(HostForm::for_custom(i, &cfg.connections[i])),
            Source::SshConfig => match set.locate(&h.name) {
                Locate::Editable { doc, .. } => Ok(HostForm::for_ssh(doc, &h.name, &set.fields(&h.name).expect("editable"))),
                Locate::ReadOnly(why) => Err(why),
                Locate::NotFound => Err(format!("{} is no longer in the ssh config — reload", h.name)),
            },
        }
    }

    /// The fields shown for this form's destination.
    pub fn fields(&self) -> Vec<Field> {
        Field::ALL.into_iter().filter(|f| !(matches!(self.dest, Dest::SshConfig(_)) && *f == Field::ExtraArgs)).collect()
    }

    pub fn get(&self, f: Field) -> &str {
        &self.values[f as usize]
    }

    pub fn get_mut(&mut self, f: Field) -> &mut String {
        &mut self.values[f as usize]
    }

    /// The destination can change only while adding.
    pub fn can_change_dest(&self) -> bool {
        self.editing == Editing::New
    }

    /// The name of the host this form edits, as it was before any change
    /// (`None` for a new host). Call it before [`HostForm::save`].
    pub fn original_name(&self, cfg: &Config) -> Option<String> {
        match &self.editing {
            Editing::New => None,
            Editing::Custom(i) => cfg.connections.get(*i).map(|c| c.name.clone()),
            Editing::Ssh(a) => Some(a.clone()),
        }
    }

    fn current_name(&self) -> Option<&str> {
        match &self.editing {
            Editing::New => None,
            Editing::Custom(_) => None,
            Editing::Ssh(a) => Some(a),
        }
    }

    fn host_fields(&self) -> HostFields {
        let v = |f: Field| self.get(f).trim().to_string();
        HostFields {
            alias: v(Field::Name),
            hostname: v(Field::Host),
            user: v(Field::User),
            port: v(Field::Port),
            identity_file: v(Field::Identity),
            proxy_jump: v(Field::ProxyJump),
            proxy_command: v(Field::ProxyCommand),
            remote_dir: v(Field::RemoteDir),
            tags: split_tags(self.get(Field::Tags)),
            hidden: self.hidden,
            other: self.other.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect(),
        }
    }

    /// Applies the form to `cfg` or `set` (in memory). `taken` is every host
    /// name in use; the caller then saves the file named by the result.
    pub fn save(&self, cfg: &mut Config, set: &mut ConfigSet, taken: &[String]) -> Result<Saved, String> {
        match self.dest {
            Dest::Termax => {
                let mut c = Connection::from_fields(&self.values)?;
                c.hidden = self.hidden;
                let own = match self.editing {
                    Editing::Custom(i) => cfg.connections.get(i).map(|c| c.name.clone()),
                    _ => None,
                };
                if own.as_deref() != Some(c.name.as_str()) && taken.contains(&c.name) {
                    return Err(format!("A host named {} already exists", c.name));
                }
                let name = c.name.clone();
                match self.editing {
                    Editing::Custom(i) if i < cfg.connections.len() => cfg.connections[i] = c,
                    _ => cfg.connections.push(c),
                }
                Ok(Saved::Termax(name))
            }
            Dest::SshConfig(doc) => {
                let f = self.host_fields();
                let existing = self.current_name().and_then(|a| set.fields(a)).map(|h| h.other).unwrap_or_default();
                validate_with(&f, taken, self.current_name(), &existing)?;
                match (self.current_name(), &self.after) {
                    (Some(alias), _) => set.update_host(alias, &f)?,
                    (None, Some(after)) => set.add_host_after(after, &f)?,
                    (None, None) => set.add_host(doc, &f)?,
                }
                Ok(Saved::Ssh(f.alias))
            }
        }
    }
}

/// After host `old` was renamed to `new`: points saved `[[forward]]`s at
/// the new name. Returns whether any changed (config.toml then needs saving).
pub fn rename_forwards(cfg: &mut Config, old: &str, new: &str) -> bool {
    let mut changed = false;
    for f in cfg.forwards.iter_mut().filter(|f| f.host == old) {
        f.host = new.to_string();
        changed = true;
    }
    changed
}

pub fn split_tags(s: &str) -> Vec<String> {
    s.split(',').map(str::trim).filter(|t| !t.is_empty()).map(String::from).collect()
}

/// Every host name in use.
pub fn taken_names(hosts: &[HostEntry]) -> Vec<String> {
    hosts.iter().map(|h| h.name.clone()).collect()
}

/// Hides or shows host `h`.
pub fn set_hidden(h: &HostEntry, hidden: bool, cfg: &mut Config, set: &mut ConfigSet) -> Result<Saved, String> {
    let mut form = HostForm::for_entry(h, cfg, set)?;
    form.hidden = hidden;
    let taken: Vec<String> = Vec::new();
    form.save(cfg, set, &taken)
}

/// Removes host `h`.
pub fn delete(h: &HostEntry, cfg: &mut Config, set: &mut ConfigSet) -> Result<Saved, String> {
    match h.source {
        Source::Custom(i) if i < cfg.connections.len() => {
            let c = cfg.connections.remove(i);
            Ok(Saved::Termax(c.name))
        }
        Source::Custom(_) => Err("That connection is gone — reload".into()),
        Source::SshConfig => {
            set.delete_host(&h.name)?;
            Ok(Saved::Ssh(h.name.clone()))
        }
    }
}

/// `<name>-copy`, then `<name>-copy-2`, … — the first one not in `taken`.
pub fn duplicate_name(name: &str, taken: &[String]) -> String {
    let base = format!("{name}-copy");
    if !taken.contains(&base) {
        return base;
    }
    (2..).map(|n| format!("{base}-{n}")).find(|c| !taken.contains(c)).expect("unbounded")
}

/// A new form pre-filled from host `h`, named [`duplicate_name`].
pub fn duplicate(h: &HostEntry, cfg: &Config, set: &ConfigSet, taken: &[String]) -> Result<HostForm, String> {
    let mut form = HostForm::for_entry(h, cfg, set)?;
    if let Editing::Ssh(alias) = &form.editing {
        form.after = Some(alias.clone());
    }
    form.editing = Editing::New;
    *form.get_mut(Field::Name) = duplicate_name(&h.name, taken);
    Ok(form)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_names() {
        let taken = vec!["web".to_string(), "web-copy".to_string()];
        assert_eq!(duplicate_name("db", &taken), "db-copy");
        assert_eq!(duplicate_name("web", &taken), "web-copy-2");
    }

    #[test]
    fn ssh_duplicate_goes_after_the_original() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("config");
        std::fs::write(&p, "Host a\n    User x\nHost *\n    User y\n").unwrap();
        let mut cfg = Config::default();
        cfg.settings.ssh_config_path = Some(p.display().to_string());
        let hosts = crate::hosts::collect(&cfg).0;
        let mut set = ConfigSet::load(&p).unwrap();
        let taken = taken_names(&hosts);
        let a = hosts.iter().find(|h| h.name == "a").unwrap();
        duplicate(a, &cfg, &set, &taken).unwrap().save(&mut cfg, &mut set, &taken).unwrap();
        assert_eq!(set.docs[0].render(), "Host a\n    User x\n\nHost a-copy\n    User x\n\nHost *\n    User y\n");
    }

    #[test]
    fn rename_points_forwards_at_the_new_name() {
        let mut cfg = Config::default();
        cfg.connections.push(Connection { name: "old".into(), host: "h".into(), ..Default::default() });
        let fwd = |host: &str| crate::forward::Forward { host: host.into(), listen: 1, ..Default::default() };
        cfg.forwards = vec![fwd("old"), fwd("other")];
        let form = HostForm::for_custom(0, &cfg.connections[0]);
        assert_eq!(form.original_name(&cfg).as_deref(), Some("old"));
        assert!(rename_forwards(&mut cfg, "old", "new"));
        assert_eq!(cfg.forwards, [fwd("new"), fwd("other")]);
        assert!(!rename_forwards(&mut cfg, "old", "new"));
    }

    #[test]
    fn saves_to_either_destination_and_rejects_taken_names() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("config");
        std::fs::write(&p, "Host a\n    HostName 1.1.1.1\n").unwrap();
        let mut set = ConfigSet::load(&p).unwrap();
        let mut cfg = Config::default();
        let taken = vec!["a".to_string()];

        let mut f = HostForm::new_host(Dest::Termax);
        *f.get_mut(Field::Name) = "a".into();
        *f.get_mut(Field::Host) = "h".into();
        assert_eq!(f.save(&mut cfg, &mut set, &taken), Err("A host named a already exists".into()));

        f.dest = Dest::SshConfig(0);
        *f.get_mut(Field::Name) = "b".into();
        f.other = "ForwardAgent yes\n\n".into();
        f.hidden = true;
        assert_eq!(f.save(&mut cfg, &mut set, &taken), Ok(Saved::Ssh("b".into())));
        assert_eq!(set.docs[0].render(), "Host a\n    HostName 1.1.1.1\n\nHost b\n    # termax: hidden\n    HostName h\n    ForwardAgent yes\n");
        assert!(!f.fields().contains(&Field::ExtraArgs));
    }
}

