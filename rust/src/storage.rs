use crate::sources::SourceId;
use std::path::{Path, PathBuf};

pub struct Storage {
    root: PathBuf,
    /// The source whose favorites, presets, last channel and defect marks are
    /// in use.
    source: SourceId,
}

impl Storage {
    /// Standard location: %APPDATA%\BongoIPTVPlayer\config\ on Windows. The
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

    /// From now on the favorites, presets, last channel and defect marks are
    /// source `id`'s.
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
    /// The defect marks of the source in use (`bad_streams.rs`): stream ids and
    /// times only, never an address.
    pub fn bad_streams_path(&self) -> PathBuf {
        self.source_file("bad_streams", self.source)
    }
    /// What the source in use watched, its list and its thumbs
    /// (`vod::watching`): numbers and times only.
    pub fn watching_path(&self) -> PathBuf {
        self.source_file("watching", self.source)
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

    /// Where a write of M3U source `id`'s saved list puts it first
    /// (`portal::m3u`); a write that was cut off leaves it, stream URLs and all.
    fn m3u_temp_path(&self, id: SourceId) -> PathBuf {
        self.m3u_cache_path(id).with_extension("m3u.tmp")
    }

    /// Removes source `id`'s favorites, presets, last channel, defect marks,
    /// what it watched (and the `.bad` copies of that file) and saved list, with
    /// the temp file of that list; how many of them there were.
    pub fn remove_source_files(&self, id: SourceId) -> usize {
        let watching = self.source_file("watching", id);
        [
            self.source_file("favorites", id),
            self.source_file("presets", id),
            self.source_file("last_watched", id),
            self.source_file("bad_streams", id),
            watching.clone(),
            self.m3u_cache_path(id),
            self.m3u_temp_path(id),
        ]
        .into_iter()
        .chain(crate::vod::watching::bad_copies(&watching))
        .filter(|path| remove_file(path))
        .count()
    }

    /// Removes M3U source `id`'s saved list and its temp file: another address
    /// makes them another list's.
    pub fn remove_saved_list(&self, id: SourceId) {
        for path in [self.m3u_cache_path(id), self.m3u_temp_path(id)] {
            remove_file(&path);
        }
    }

    /// Removes source `id`'s defect marks: another address makes its list
    /// another one, whose working channels the old list's stream ids would hide
    /// for seven days.
    pub fn remove_bad_streams(&self, id: SourceId) {
        remove_file(&self.source_file("bad_streams", id));
    }
}

/// Removes the file at `path`; whether it was there. A failure other than its
/// not being there is logged by the file's name, which names a source by its
/// id only: the folder holds the Windows user's name.
fn remove_file(path: &Path) -> bool {
    match std::fs::remove_file(path) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            tracing::warn!("storage: {} not removed: {}", name, e);
            false
        }
    }
}
