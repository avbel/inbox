use crate::error::Result;
use std::path::{Path, PathBuf};

#[allow(dead_code)]
#[derive(Debug)]
pub struct DiffItem {
    pub path: PathBuf,
    pub old_content: Option<String>,
    pub new_content: Option<String>,
}

#[allow(dead_code)]
pub fn compute_diff(_paths: &[PathBuf], _snapshot_dir: &Path) -> Result<Vec<DiffItem>> {
    Ok(vec![])
}
