use crate::sources::SourceId;
use std::path::{Path, PathBuf};

pub struct Storage {
    root: PathBuf,
    /// The source whose favorites, presets and last channel are in use.
    source: SourceId,
}

impl Storage {
    /// Standard location: %APPDATA%\BongoPlayer\config\ on Windows. The
    /// `BONGO_CONFIG_DIR` environment variable points it elsewhere: the
    /// end-to-end tests use that so they never touch the user's own files.
    pub fn standard() -> anyhow::Result<Self> {
        if let Some(dir) = std::env::var_os("BONGO_CONFIG_DIR") {
            return Ok(Self::with_root(PathBuf::from(dir)));
        }
        let proj = directories::ProjectDirs::from("", "", crate::APP_ID)
            .ok_or_else(|| anyhow::anyhow!("no project dir"))?;
        Ok(Self {
            root: proj.config_dir().to_path_buf(),
            source: 1,
        })
    }

    pub fn with_root(root: PathBuf) -> Self {
        Self { root, source: 1 }
    }

    pub fn config_dir(&self) -> &Path {
        &self.root
    }

    pub fn ensure_config_dir(&self) -> std::io::Result<PathBuf> {
        std::fs::create_dir_all(&self.root)?;
        Ok(self.root.clone())
    }

    /// From now on the favorites, presets and last channel are source `id`'s.
    pub fn set_source(&mut self, id: SourceId) {
        self.source = id;
    }

    pub fn source(&self) -> SourceId {
        self.source
    }

    /// A file of one source: source 1 keeps 0.6.1's name (`favorites.json`),
    /// the others carry their id (`favorites-2.json`).
    fn source_file(&self, stem: &str, id: SourceId) -> PathBuf {
        if id == 1 {
            self.root.join(format!("{stem}.json"))
        } else {
            self.root.join(format!("{stem}-{id}.json"))
        }
    }

    pub fn favorites_path(&self) -> PathBuf {
        self.source_file("favorites", self.source)
    }
    pub fn presets_path(&self) -> PathBuf {
        self.source_file("presets", self.source)
    }
    pub fn last_watched_path(&self) -> PathBuf {
        self.source_file("last_watched", self.source)
    }
    pub fn settings_path(&self) -> PathBuf {
        self.root.join("settings.json")
    }

    /// What the app can build again (the guide's EPG, logos): `cache\` in the
    /// config folder, so `BONGO_CONFIG_DIR` takes it along.
    pub fn cache_dir(&self) -> PathBuf {
        self.root.join("cache")
    }

    /// Where M3U source `id` keeps the last list that came. Its name holds the
    /// id, never the address.
    pub fn m3u_cache_path(&self, id: SourceId) -> PathBuf {
        self.cache_dir().join(format!("m3u-{id}.m3u"))
    }

    /// Removes source `id`'s favorites, presets, last channel and saved list;
    /// how many of them there were.
    pub fn remove_source_files(&self, id: SourceId) -> usize {
        [
            self.source_file("favorites", id),
            self.source_file("presets", id),
            self.source_file("last_watched", id),
            self.m3u_cache_path(id),
        ]
        .iter()
        .filter(|path| std::fs::remove_file(path).is_ok())
        .count()
    }
}
