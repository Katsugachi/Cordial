//! Filesystem layout for Cordial.
//!
//! Everything lives under the platform data directory so the app is portable
//! and uninstallable in the normal way:
//!
//! ```text
//! <data>/Cordial/
//! ├── models/      LFM3-VL-2B GGUF files
//! ├── sessions/    one JSON file per conversation
//! ├── workspace/   the folder file tools operate in
//! └── settings.json
//! ```

use std::path::{Path, PathBuf};

use crate::error::CoreError;

/// Resolved locations for this installation.
#[derive(Debug, Clone)]
pub struct Paths {
    root: PathBuf,
}

impl Paths {
    /// Uses the standard per-user data directory.
    pub fn discover() -> Result<Self, CoreError> {
        let base = dirs::data_dir().ok_or(CoreError::NoDataDir)?;
        Ok(Self {
            root: base.join("Cordial"),
        })
    }

    /// Uses an explicit root, which tests and CI rely on.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn models_dir(&self) -> PathBuf {
        self.root.join("models")
    }

    pub fn sessions_dir(&self) -> PathBuf {
        self.root.join("sessions")
    }

    pub fn workspace_dir(&self) -> PathBuf {
        self.root.join("workspace")
    }

    pub fn settings_file(&self) -> PathBuf {
        self.root.join("settings.json")
    }

    /// Creates every directory the app writes into.
    pub fn ensure(&self) -> Result<(), CoreError> {
        for dir in [
            self.root.clone(),
            self.models_dir(),
            self.sessions_dir(),
            self.workspace_dir(),
        ] {
            std::fs::create_dir_all(&dir)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_is_under_one_root() {
        let paths = Paths::at("/tmp/cordial-test");
        assert!(paths.models_dir().starts_with("/tmp/cordial-test"));
        assert!(paths.sessions_dir().starts_with("/tmp/cordial-test"));
        assert_eq!(
            paths.workspace_dir(),
            Path::new("/tmp/cordial-test/workspace")
        );
    }
}
