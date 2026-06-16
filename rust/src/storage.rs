use std::path::{Path, PathBuf};

pub struct Storage {
    root: PathBuf,
}

impl Storage {
    /// Standard location: %APPDATA%\tvplayer\ on Windows.
    pub fn standard() -> anyhow::Result<Self> {
        let proj = directories::ProjectDirs::from("", "", "tvplayer")
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
    pub fn config_path(&self) -> PathBuf {
        self.root.join("config.json")
    }
}
