use std::path::PathBuf;
use thiserror::Error;

// Variants are forward-declared for modules implemented in later tasks.
#[allow(dead_code)]
#[derive(Debug, Error)]
pub enum InboxError {
    #[error("profile '{0}' not found")]
    ProfileNotFound(String),
    #[error("profile cycle detected: {0}")]
    ProfileCycle(String),
    #[error("snapshot dir creation failed at {path}: {source}")]
    SnapshotDirCreate {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("restore failed for {path}: {source}")]
    RestoreFailed {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("kernel too old: need 5.13+, got {0}")]
    KernelTooOld(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("YAML error: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("nix error: {0}")]
    Nix(#[from] nix::Error),
    #[error("glob pattern error: {0}")]
    Glob(String),
}

pub type Result<T> = std::result::Result<T, InboxError>;
