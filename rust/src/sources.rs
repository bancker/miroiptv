//! The sources the app can play from, Xtream portals and M3U lists, as
//! `BongoIPTVPlayer.ini` keeps them, and which one is in use.
//!
//! The ini of 0.6.1 holds one line, `portal=user:pass@host:port`. It stays, as
//! the first Xtream source's address, so a downgrade still starts on the
//! portal. Each source is a line `source=<id>;<kind>;<name>;<address>` (the
//! address last: a password may hold `;`), and `active=<id>` says which one is
//! in use. One Xtream source alone, with id 1, no name of its own and in use,
//! is written as the `portal=` line only: 0.6.1's ini, word for word.

use crate::i18n::{t, Lang, T};
use std::path::Path;

/// A source's short, fixed number: its data files carry it, never its address.
pub type SourceId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Xtream,
    M3u,
}

impl SourceKind {
    /// The ini's word for it.
    pub fn code(self) -> &'static str {
        match self {
            SourceKind::Xtream => "xtream",
            SourceKind::M3u => "m3u",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        match code.trim().to_ascii_lowercase().as_str() {
            "xtream" => Some(SourceKind::Xtream),
            "m3u" => Some(SourceKind::M3u),
            _ => None,
        }
    }

    /// The name a source of this kind has when the user gave it none.
    pub fn default_name(self, lang: Lang) -> &'static str {
        t(
            lang,
            match self {
                SourceKind::Xtream => T::DefaultPortalName,
                SourceKind::M3u => T::DefaultListName,
            },
        )
    }

    /// Whether `name` is this kind's default name in any language, spaces
    /// aside. Such a name is no name of the user's own: it is stored empty and
    /// shown in the language in use, also when a form was made in another one.
    pub fn is_default_name(self, name: &str) -> bool {
        let name = name.trim();
        Lang::ALL
            .iter()
            .any(|lang| self.default_name(*lang) == name)
    }
}

/// `Debug` for the tests only: it would print the address, a credential, so a
/// `{:?}` of it in a log line or a toast does not build.
#[derive(Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(Debug))]
pub struct Source {
    pub id: SourceId,
    pub kind: SourceKind,
    /// The user's name for it; empty: the default name of its kind.
    pub name: String,
    /// `user:pass@host:port` for Xtream; an http(s) URL or a file for M3U.
    pub address: String,
}

impl Source {
    pub fn display_name(&self, lang: Lang) -> String {
        if self.name.is_empty() {
            self.kind.default_name(lang).to_owned()
        } else {
            self.name.clone()
        }
    }
}

/// Which source the settings' form shows, and so which one its Save is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Editing {
    /// A saved source.
    Saved(SourceId),
    /// The portal of `--xtream`, which is not saved; saved, it becomes the
    /// first Xtream source.
    CommandLine,
    /// One that is being added.
    New,
}

/// What a Save in the settings' sources section hands the app. `Debug` for the
/// tests only, as for [`Source`].
#[derive(Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(Debug))]
pub struct SourceDraft {
    pub editing: Editing,
    pub kind: SourceKind,
    pub name: String,
    /// The credentials line (`user:pass@host:port`) of an Xtream source, the
    /// URL or file of an M3U one.
    pub address: String,
}

/// Every saved source, in the order they were added, and the one in use.
/// `Debug` for the tests only, as for [`Source`].
#[derive(Clone, Default, PartialEq, Eq)]
#[cfg_attr(test, derive(Debug))]
pub struct Sources {
    list: Vec<Source>,
    active: Option<SourceId>,
}

impl Sources {
    pub const EMPTY: Sources = Sources {
        list: Vec::new(),
        active: None,
    };

    /// The sources in an ini's text. Lines that make no source are skipped; so
    /// are `#` and `;` comments and a BOM. Without `source=` lines the first
    /// line with credentials (0.6.1's rule: `key=value` when the key holds no
    /// `@` or `:`, else the whole line) is source 1, Xtream, in use. An
    /// `active=` that names no source falls back to the first.
    pub fn parse(text: &str) -> Self {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut list: Vec<Source> = Vec::new();
        let mut active = None;
        let mut portal = None;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            if let Some(rest) = line.strip_prefix("source=") {
                if let Some(source) = source_line(rest) {
                    if !list.iter().any(|s| s.id == source.id) {
                        list.push(source);
                    }
                }
            } else if let Some(rest) = line.strip_prefix("active=") {
                active = rest.trim().parse().ok();
            } else if portal.is_none() {
                let value = match line.split_once('=') {
                    Some((key, value)) if !key.contains(['@', ':']) => value.trim(),
                    _ => line,
                };
                if value.contains('@') {
                    portal = Some(value.to_owned());
                }
            }
        }
        if list.is_empty() {
            if let Some(address) = portal {
                list.push(Source {
                    id: 1,
                    kind: SourceKind::Xtream,
                    name: String::new(),
                    address,
                });
                active = Some(1);
            }
        }
        let active = active
            .filter(|id| list.iter().any(|s| s.id == *id))
            .or_else(|| list.first().map(|s| s.id));
        Sources { list, active }
    }

    /// The ini's text for these sources (see the module's doc).
    pub fn to_text(&self) -> String {
        let mut text = String::new();
        if let Some(portal) = self.first_xtream() {
            text.push_str(&format!("portal={}\n", portal.address));
        }
        let only_the_portal = matches!(
            self.list.as_slice(),
            [s] if s.id == 1 && s.kind == SourceKind::Xtream && s.name.is_empty()
        ) && self.active == Some(1);
        if only_the_portal {
            return text;
        }
        for s in &self.list {
            text.push_str(&format!(
                "source={};{};{};{}\n",
                s.id,
                s.kind.code(),
                s.name,
                s.address
            ));
        }
        if let Some(id) = self.active {
            text.push_str(&format!("active={id}\n"));
        }
        text
    }

    /// The sources in the ini at `path`; none when it is missing or unreadable.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    /// Writes the ini through a temp file beside `path`, renamed over it: a
    /// failed write leaves the old ini as it was, and no temp file with
    /// passwords in it.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let tmp = path.with_extension("ini.tmp");
        let saved = std::fs::write(&tmp, self.to_text()).and_then(|()| std::fs::rename(&tmp, path));
        if saved.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        saved
    }

    pub fn list(&self) -> &[Source] {
        &self.list
    }

    pub fn get(&self, id: SourceId) -> Option<&Source> {
        self.list.iter().find(|s| s.id == id)
    }

    pub fn active_id(&self) -> Option<SourceId> {
        self.active
    }

    pub fn active(&self) -> Option<&Source> {
        self.get(self.active?)
    }

    pub fn first_xtream(&self) -> Option<&Source> {
        self.list.iter().find(|s| s.kind == SourceKind::Xtream)
    }

    /// Puts source `id` in use; false when there is no such source.
    pub fn set_active(&mut self, id: SourceId) -> bool {
        let known = self.get(id).is_some();
        if known {
            self.active = Some(id);
        }
        known
    }

    /// Keeps a source as `editing` says, and returns its id. A saved one
    /// changes its name and address, and keeps its kind. The command line's
    /// portal goes into the first Xtream source. A new one, or a command-line
    /// portal with no Xtream source to go into, is added with an id of its own
    /// (see `new_id`); `None`, and nothing kept, when no id is left for it (a
    /// hand-edited ini holds the highest there is). Names lose `;`, CR and LF;
    /// both lose the spaces around them.
    pub fn keep(
        &mut self,
        editing: Editing,
        kind: SourceKind,
        name: &str,
        address: &str,
    ) -> Option<SourceId> {
        let target = match editing {
            Editing::Saved(id) => self.list.iter().position(|s| s.id == id),
            Editing::CommandLine => self.list.iter().position(|s| s.kind == SourceKind::Xtream),
            Editing::New => None,
        };
        let name = clean(name).replace(';', ",");
        let address = clean(address);
        match target {
            Some(i) => {
                let source = &mut self.list[i];
                source.name = name;
                source.address = address;
                Some(source.id)
            }
            None => {
                let id = self.new_id(kind)?;
                self.list.push(Source {
                    id,
                    kind,
                    name,
                    address,
                });
                Some(id)
            }
        }
    }

    /// The id a new source of `kind` gets. Source 1 keeps 0.6.1's files
    /// (`favorites.json` and the rest), which are the portal's: an M3U list
    /// never gets 1, but 1 + the highest id, 2 at least. A portal gets 1 when
    /// no portal is saved and 1 is free, else 1 + the highest. An ini that
    /// holds a list as source 1 already is read as it is. `None` past the
    /// highest id there is: adding one would wrap to 0, which is no id.
    fn new_id(&self, kind: SourceKind) -> Option<SourceId> {
        let highest = self.list.iter().map(|s| s.id).max().unwrap_or(0);
        match kind {
            SourceKind::M3u => highest.max(1).checked_add(1),
            SourceKind::Xtream if self.first_xtream().is_none() && self.get(1).is_none() => Some(1),
            SourceKind::Xtream => highest.checked_add(1),
        }
    }

    /// Takes source `id` out; if it was in use, none is.
    pub fn remove(&mut self, id: SourceId) -> Option<Source> {
        let i = self.list.iter().position(|s| s.id == id)?;
        if self.active == Some(id) {
            self.active = None;
        }
        Some(self.list.remove(i))
    }
}

/// `source=` without its key: `<id>;<kind>;<name>;<address>`. `None` for an
/// id that is no number above 0, an unknown kind, or no address.
fn source_line(rest: &str) -> Option<Source> {
    let mut fields = rest.splitn(4, ';');
    let id: SourceId = fields.next()?.trim().parse().ok().filter(|id| *id > 0)?;
    let kind = SourceKind::from_code(fields.next()?)?;
    let name = fields.next()?.trim().to_owned();
    let address = fields.next()?.trim().to_owned();
    (!address.is_empty()).then_some(Source {
        id,
        kind,
        name,
        address,
    })
}

/// `text` without line breaks and the spaces around it.
fn clean(text: &str) -> String {
    crate::args::without_newlines(text).trim().to_owned()
}

/// Whether an M3U source's address is a web address to fetch (http or https,
/// with a host) rather than a file.
pub fn is_web_address(address: &str) -> bool {
    reqwest::Url::parse(address.trim())
        .is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some())
}
