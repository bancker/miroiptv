use std::path::{Path, PathBuf};

pub struct Storage {
    root: PathBuf,
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
        })
    }

    pub fn with_root(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn config_dir(&self) -> &Path {
        &self.root
    }

    pub fn ensure_config_dir(&self) -> std::io::Result<PathBuf> {
        std::fs::create_dir_all(&self.root)?;
        Ok(self.root.clone())
    }

    pub fn favorites_path(&self) -> PathBuf {
        self.root.join("favorites.json")
    }
    pub fn presets_path(&self) -> PathBuf {
        self.root.join("presets.json")
    }
    pub fn last_watched_path(&self) -> PathBuf {
        self.root.join("last_watched.json")
    }
    pub fn settings_path(&self) -> PathBuf {
        self.root.join("settings.json")
    }
}
