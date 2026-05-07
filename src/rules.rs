use crate::error::{InboxError, Result};
use std::path::{Path, PathBuf};

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    Ro,
    Rw,
    Ephemeral,
    Hide,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct Rule {
    pub path: PathBuf,
    pub mode: Mode,
}

#[allow(dead_code)]
pub struct RuleSet {
    rules: Vec<Rule>,
}

impl RuleSet {
    #[allow(dead_code)]
    pub fn from_patterns(patterns: &[(String, Mode)]) -> Result<Self> {
        let home = home_dir();
        let mut rules = Vec::new();

        for (raw, mode) in patterns {
            let pattern = match raw.strip_prefix("~/") {
                Some(rest) => format!("{}/{}", home.display(), rest),
                None => raw.clone(),
            };

            if is_glob(&pattern) {
                let matched = glob::glob(&pattern)
                    .map_err(|e| InboxError::Glob(e.to_string()))?
                    .filter_map(|r| r.ok())
                    .collect::<Vec<_>>();
                for path in matched {
                    // Canonicalize so SBPL rules match kernel paths (e.g. /tmp → /private/tmp on macOS)
                    let path = std::fs::canonicalize(&path).unwrap_or(path);
                    rules.push(Rule {
                        path,
                        mode: mode.clone(),
                    });
                }
            } else {
                let path = PathBuf::from(&pattern);
                if path.exists() {
                    // Canonicalize so SBPL rules match kernel paths (e.g. /tmp → /private/tmp on macOS)
                    let path = std::fs::canonicalize(&path).unwrap_or(path);
                    rules.push(Rule {
                        path,
                        mode: mode.clone(),
                    });
                }
                // non-existent literal paths silently ignored
            }
        }

        Ok(Self { rules })
    }

    #[allow(dead_code)]
    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    #[allow(dead_code)]
    pub fn ephemeral_paths(&self) -> Vec<&Path> {
        self.rules
            .iter()
            .filter(|r| r.mode == Mode::Ephemeral)
            .map(|r| r.path.as_path())
            .collect()
    }

    #[allow(dead_code)]
    pub fn has_ephemeral(&self) -> bool {
        self.rules.iter().any(|r| r.mode == Mode::Ephemeral)
    }

    #[cfg(test)]
    pub fn from_rules(rules: Vec<Rule>) -> Self {
        Self { rules }
    }
}

#[allow(dead_code)]
fn is_glob(s: &str) -> bool {
    s.contains('*') || s.contains('?') || s.contains('[')
}

#[allow(dead_code)]
fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn literal_path_existing() {
        let dir = TempDir::new().unwrap();
        let f = dir.path().join("file.txt");
        fs::write(&f, "").unwrap();
        let rules = RuleSet::from_patterns(&[(f.to_str().unwrap().to_string(), Mode::Ro)]).unwrap();
        assert_eq!(rules.rules().len(), 1);
        assert_eq!(rules.rules()[0].mode, Mode::Ro);
    }

    #[test]
    fn literal_path_nonexistent_ignored() {
        let rules =
            RuleSet::from_patterns(&[("/nonexistent/path/xyz".to_string(), Mode::Ro)]).unwrap();
        assert!(rules.rules().is_empty());
    }

    #[test]
    fn tilde_expands_to_home() {
        let rules =
            RuleSet::from_patterns(&[("~/nonexistent_xyz_abc".to_string(), Mode::Ro)]).unwrap();
        // nonexistent so 0 rules, but no error
        assert!(rules.rules().is_empty());
    }

    #[test]
    fn glob_matches_files() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(".env"), "").unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("sub").join(".env"), "").unwrap();
        let pattern = format!("{}/**/.env", dir.path().display());
        let rules = RuleSet::from_patterns(&[(pattern, Mode::Hide)]).unwrap();
        assert_eq!(rules.rules().len(), 2);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn symlink_path_is_canonicalized() {
        // On macOS /tmp is a symlink to /private/tmp. Rules must store the
        // canonical path so SBPL patterns match what the kernel uses.
        let dir = TempDir::new().unwrap();
        let f = dir.path().join("file.txt");
        fs::write(&f, "").unwrap();

        // Create a symlink pointing at the real file
        let link = dir.path().join("link.txt");
        std::os::unix::fs::symlink(&f, &link).unwrap();

        let rules =
            RuleSet::from_patterns(&[(link.to_str().unwrap().to_string(), Mode::Ro)]).unwrap();
        assert_eq!(rules.rules().len(), 1);
        // Rule path should be the real file, not the symlink
        assert_eq!(rules.rules()[0].path, std::fs::canonicalize(&f).unwrap());
    }

    #[test]
    fn ephemeral_paths_filter() {
        let dir = TempDir::new().unwrap();
        let f = dir.path().join("x");
        fs::write(&f, "").unwrap();
        let rules =
            RuleSet::from_patterns(&[(f.to_str().unwrap().to_string(), Mode::Ephemeral)]).unwrap();
        assert_eq!(rules.ephemeral_paths().len(), 1);
    }
}
