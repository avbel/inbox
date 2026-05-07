use crate::error::{InboxError, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub uuid: String,
    pub paths: Vec<PathBuf>,
    pub snapshot_dir: PathBuf,
    pub started_at: String,
}

#[allow(dead_code)]
pub struct EphemeralManager {
    pub snapshot_root: PathBuf,
    pub uuid: String,
    pub paths: Vec<PathBuf>,
    after_called: bool,
}

impl EphemeralManager {
    #[allow(dead_code)]
    pub fn new(snapshot_root: PathBuf) -> Self {
        Self {
            snapshot_root,
            uuid: Uuid::new_v4().to_string(),
            paths: vec![],
            after_called: false,
        }
    }

    /// Create snapshot root, copy ephemeral paths, write manifest.
    #[allow(dead_code)]
    pub fn before(&mut self, paths: &[PathBuf]) -> Result<()> {
        self.paths = paths.to_vec();
        let snap_dir = self.snapshot_dir();
        std::fs::create_dir_all(&snap_dir).map_err(|e| InboxError::SnapshotDirCreate {
            path: snap_dir.clone(),
            source: e,
        })?;

        for path in paths {
            let dest = self.snapshot_path_for(path);
            copy_recursive(path, &dest)?;
        }

        let manifest = Manifest {
            uuid: self.uuid.clone(),
            paths: self.paths.clone(),
            snapshot_dir: snap_dir.clone(),
            started_at: chrono::Utc::now().to_rfc3339(),
        };
        let manifest_path = self.snapshot_root.join(&self.uuid).join("manifest.json");
        std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)?;

        Ok(())
    }

    /// Restore all ephemeral paths silently, then clean up snapshot.
    #[allow(dead_code)]
    pub fn after_silent(&mut self) -> Result<()> {
        self.after_called = true;
        self.restore()?;
        let _ = std::fs::remove_dir_all(self.snapshot_root.join(&self.uuid));
        Ok(())
    }

    /// Return the list of DiffItems for the review TUI.
    #[allow(dead_code)]
    pub fn diff(&self) -> Result<Vec<crate::review::DiffItem>> {
        crate::review::compute_diff(
            &self.paths,
            &self.snapshot_root.join(&self.uuid).join("snapshot"),
        )
    }

    /// Apply selected items from a review and clean up.
    #[allow(dead_code)]
    pub fn after_review(&mut self, kept: &[PathBuf]) -> Result<()> {
        self.after_called = true;
        self.restore()?;
        let _ = kept; // kept paths will be re-applied from DiffItem data
        let _ = std::fs::remove_dir_all(self.snapshot_root.join(&self.uuid));
        Ok(())
    }

    fn restore(&self) -> Result<()> {
        for path in &self.paths {
            let snap = self.snapshot_path_for(path);
            if snap.exists() {
                if path.is_dir() {
                    let _ = std::fs::remove_dir_all(path);
                } else {
                    let _ = std::fs::remove_file(path);
                }
                copy_recursive(&snap, path).map_err(|e| InboxError::RestoreFailed {
                    path: path.clone(),
                    source: std::io::Error::other(e),
                })?;
            } else if path.exists() {
                if path.is_dir() {
                    let _ = std::fs::remove_dir_all(path);
                } else {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
        Ok(())
    }

    fn snapshot_dir(&self) -> PathBuf {
        self.snapshot_root.join(&self.uuid).join("snapshot")
    }

    fn snapshot_path_for(&self, path: &Path) -> PathBuf {
        let rel = path.strip_prefix("/").unwrap_or(path);
        self.snapshot_dir().join(rel)
    }
}

impl Drop for EphemeralManager {
    fn drop(&mut self) {
        if !self.after_called && !self.paths.is_empty() {
            let _ = self.restore();
            let _ = std::fs::remove_dir_all(self.snapshot_root.join(&self.uuid));
        }
    }
}

/// Scan snapshot root for orphaned manifests from previous runs.
#[allow(dead_code)]
pub fn scan_orphaned(snapshot_root: &Path) -> Vec<Manifest> {
    let mut orphans = vec![];
    let Ok(entries) = std::fs::read_dir(snapshot_root) else {
        return orphans;
    };
    for entry in entries.flatten() {
        let manifest_path = entry.path().join("manifest.json");
        if manifest_path.exists()
            && let Ok(content) = std::fs::read_to_string(&manifest_path)
            && let Ok(manifest) = serde_json::from_str::<Manifest>(&content)
        {
            orphans.push(manifest);
        }
    }
    orphans
}

/// Restore an orphaned snapshot by UUID.
#[allow(dead_code)]
pub fn restore_orphan(snapshot_root: &Path, uuid: &str) -> Result<()> {
    let manifest_path = snapshot_root.join(uuid).join("manifest.json");
    let content = std::fs::read_to_string(&manifest_path)?;
    let manifest: Manifest = serde_json::from_str(&content)?;

    let mgr = EphemeralManager {
        snapshot_root: snapshot_root.to_path_buf(),
        uuid: uuid.to_string(),
        paths: manifest.paths,
        after_called: false,
    };
    mgr.restore()?;
    let _ = std::fs::remove_dir_all(snapshot_root.join(uuid));
    Ok(())
}

/// Delete an orphaned snapshot by UUID without restoring.
#[allow(dead_code)]
pub fn discard_orphan(snapshot_root: &Path, uuid: &str) -> Result<()> {
    std::fs::remove_dir_all(snapshot_root.join(uuid))?;
    Ok(())
}

#[allow(dead_code)]
fn copy_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    if src.is_dir() {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &dst.join(entry.file_name()))?;
        }
    } else if src.exists() {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(src, dst)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn before_creates_snapshot_and_manifest() {
        let snap_dir = TempDir::new().unwrap();
        let source_dir = TempDir::new().unwrap();
        fs::write(source_dir.path().join("file.txt"), "original").unwrap();

        let mut mgr = EphemeralManager::new(snap_dir.path().to_path_buf());
        mgr.before(&[source_dir.path().to_path_buf()]).unwrap();

        let snapshot_path = snap_dir.path().join(&mgr.uuid).join("snapshot");
        assert!(snapshot_path.exists());
        let manifest_path = snap_dir.path().join(&mgr.uuid).join("manifest.json");
        assert!(manifest_path.exists());
    }

    #[test]
    fn after_silent_restores_original() {
        let snap_dir = TempDir::new().unwrap();
        let source_dir = TempDir::new().unwrap();
        let file = source_dir.path().join("file.txt");
        fs::write(&file, "original").unwrap();

        let mut mgr = EphemeralManager::new(snap_dir.path().to_path_buf());
        mgr.before(&[source_dir.path().to_path_buf()]).unwrap();

        fs::write(&file, "modified").unwrap();
        mgr.after_silent().unwrap();

        assert_eq!(fs::read_to_string(&file).unwrap(), "original");
    }

    #[test]
    fn after_silent_removes_new_files() {
        let snap_dir = TempDir::new().unwrap();
        let source_dir = TempDir::new().unwrap();

        let mut mgr = EphemeralManager::new(snap_dir.path().to_path_buf());
        mgr.before(&[source_dir.path().to_path_buf()]).unwrap();

        let new_file = source_dir.path().join("new.txt");
        fs::write(&new_file, "new").unwrap();
        mgr.after_silent().unwrap();

        assert!(!new_file.exists());
    }

    #[test]
    fn drop_restores_if_after_not_called() {
        let snap_dir = TempDir::new().unwrap();
        let source_dir = TempDir::new().unwrap();
        let file = source_dir.path().join("file.txt");
        fs::write(&file, "original").unwrap();

        {
            let mut mgr = EphemeralManager::new(snap_dir.path().to_path_buf());
            mgr.before(&[source_dir.path().to_path_buf()]).unwrap();
            fs::write(&file, "modified").unwrap();
            // mgr drops here without after() call
        }

        assert_eq!(fs::read_to_string(&file).unwrap(), "original");
    }
}
