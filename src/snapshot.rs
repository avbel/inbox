use std::path::PathBuf;

#[allow(dead_code)]
pub fn resolve_snapshot_dir(
    cli_override: Option<PathBuf>,
    config_override: Option<PathBuf>,
) -> PathBuf {
    cli_override
        .or(config_override)
        .unwrap_or_else(default_snapshot_dir)
}

#[allow(dead_code)]
fn default_snapshot_dir() -> PathBuf {
    std::env::temp_dir().join(".inbox").join("snapshots")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_flag_wins_over_all() {
        let result = resolve_snapshot_dir(Some(PathBuf::from("/cli/path")), None);
        assert_eq!(result, PathBuf::from("/cli/path"));
    }

    #[test]
    fn config_wins_over_default() {
        let result = resolve_snapshot_dir(None, Some(PathBuf::from("/config/path")));
        assert_eq!(result, PathBuf::from("/config/path"));
    }

    #[test]
    fn default_is_temp_dir() {
        let result = resolve_snapshot_dir(None, None);
        assert!(result.ends_with(".inbox/snapshots"));
    }
}
