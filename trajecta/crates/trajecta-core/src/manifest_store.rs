//! # Contract: atomic run-manifest persistence
//!
//! Writes validated manifests through same-directory temporary files and
//! atomic rename. Interrupted runs remain `running`.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::manifest::RunManifest;
use crate::runner::RunManifestStore;

/// Same-directory temporary file + atomic replace manifest store.
#[derive(Clone, Debug)]
pub struct AtomicRunManifestStore {
    path: PathBuf,
}

impl AtomicRunManifestStore {
    /// Creates a store targeting one absolute or relative manifest path.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Returns the canonical target path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl RunManifestStore for AtomicRunManifestStore {
    fn persist(&mut self, manifest: &RunManifest) -> Result<(), String> {
        manifest
            .validate()
            .map_err(|error| format!("manifest validation failed: {error:?}"))?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let file_name = self
            .path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| "manifest path missing file name".to_owned())?;
        let tmp_name = format!(".{file_name}.tmp");
        let tmp_path = self
            .path
            .parent()
            .map(|parent| parent.join(&tmp_name))
            .unwrap_or_else(|| PathBuf::from(&tmp_name));
        let body = serde_json::to_vec_pretty(manifest).map_err(|error| error.to_string())?;
        {
            let mut file = File::create(&tmp_path).map_err(|error| error.to_string())?;
            file.write_all(&body).map_err(|error| error.to_string())?;
            file.write_all(b"\n").map_err(|error| error.to_string())?;
            file.sync_all().map_err(|error| error.to_string())?;
        }
        fs::rename(&tmp_path, &self.path).map_err(|error| error.to_string())?;
        // Directory fsync is best-effort. Windows often returns access-denied for
        // directory handles; the file itself was already sync_all'd above.
        if let Some(parent) = self.path.parent() {
            if let Ok(dir) = File::open(parent) {
                let _ = dir.sync_all();
            }
        }
        Ok(())
    }
}
