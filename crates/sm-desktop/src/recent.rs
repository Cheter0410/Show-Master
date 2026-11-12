//! "Open recent": the last shows opened or saved, kept in a small file in the
//! per-user configuration folder.

use std::path::{Path, PathBuf};

const MAX: usize = 10;

/// Per-platform folder for Show Master's own settings.
pub fn config_dir() -> Option<PathBuf> {
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(|p| PathBuf::from(p).join("ShowMaster"))
    } else if cfg!(target_os = "macos") {
        home().map(|h| {
            h.join("Library")
                .join("Application Support")
                .join("ShowMaster")
        })
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home().map(|h| h.join(".config")))
            .map(|c| c.join("showmaster"))
    }
}

pub struct Recent {
    file: Option<PathBuf>,
    pub paths: Vec<PathBuf>,
}

impl Recent {
    pub fn load() -> Self {
        Self::load_from(config_dir().map(|d| d.join("recent.json")))
    }

    pub fn load_from(file: Option<PathBuf>) -> Self {
        let paths = file
            .as_ref()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|t| serde_json::from_str::<Vec<PathBuf>>(&t).ok())
            .unwrap_or_default();
        Self { file, paths }
    }

    /// Put `path` at the top (most recent first) and save.
    pub fn add(&mut self, path: &Path) {
        if self.paths.first().is_some_and(|p| p == path) {
            return;
        }
        self.paths.retain(|p| p != path);
        self.paths.insert(0, path.to_path_buf());
        self.paths.truncate(MAX);
        self.save();
    }

    pub fn clear(&mut self) {
        self.paths.clear();
        self.save();
    }

    /// Entries whose file still exists.
    pub fn existing(&self) -> impl Iterator<Item = &PathBuf> {
        self.paths.iter().filter(|p| p.is_file())
    }

    fn save(&self) {
        let Some(file) = &self.file else { return };
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let Ok(text) = serde_json::to_string_pretty(&self.paths) else {
            return;
        };
        let tmp = file.with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, file);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn most_recent_first_unique_and_persistent() {
        let dir = std::env::temp_dir().join(format!("sm-recent-{}", std::process::id()));
        let file = dir.join("recent.json");
        let mut r = Recent::load_from(Some(file.clone()));
        r.add(Path::new("/a.smshow"));
        r.add(Path::new("/b.smshow"));
        r.add(Path::new("/a.smshow"));
        assert_eq!(
            r.paths,
            vec![PathBuf::from("/a.smshow"), PathBuf::from("/b.smshow")]
        );
        for i in 0..20 {
            r.add(&PathBuf::from(format!("/{i}.smshow")));
        }
        assert_eq!(r.paths.len(), MAX);
        let again = Recent::load_from(Some(file));
        assert_eq!(again.paths, r.paths);
        let _ = std::fs::remove_dir_all(dir);
    }
}
