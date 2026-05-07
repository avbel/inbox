use crate::error::Result;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Deserialize)]
#[allow(dead_code)]
pub struct Settings {
    pub snapshot_dir: Option<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
#[allow(dead_code)]
pub struct ProfileDef {
    pub based_on: Option<String>,
    #[serde(default)]
    pub ro: Vec<String>,
    #[serde(default)]
    pub rw: Vec<String>,
    #[serde(default)]
    pub ephemeral: Vec<String>,
    #[serde(default)]
    pub hide: Vec<String>,
}

#[allow(dead_code)]
#[derive(Debug, Default)]
pub struct Config {
    pub settings: Settings,
    pub profiles: HashMap<String, ProfileDef>,
}

// Modules are forward-declared for components implemented in later tasks.
#[allow(dead_code)]
impl Config {
    pub fn load() -> Result<Self> {
        let config_path = Self::default_path();
        Self::load_from(&config_path)
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self {
                    settings: Settings::default(),
                    profiles: HashMap::new(),
                });
            }
            Err(e) => return Err(e.into()),
        };
        let raw: HashMap<String, serde_yaml::Value> = serde_yaml::from_str(&content)?;

        let mut settings = Settings::default();
        let mut profiles = HashMap::new();

        for (key, value) in raw {
            if key == "settings" {
                settings = serde_yaml::from_value(value)?;
            } else {
                let profile: ProfileDef = serde_yaml::from_value(value)?;
                profiles.insert(key, profile);
            }
        }

        Ok(Self { settings, profiles })
    }

    fn default_path() -> PathBuf {
        std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/tmp"))
            .join(".config")
            .join("inbox.yaml")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_config(dir: &TempDir, content: &str) -> std::path::PathBuf {
        let path = dir.path().join("inbox.yaml");
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn loads_settings_and_profiles() {
        let dir = TempDir::new().unwrap();
        let path = write_config(
            &dir,
            r#"
settings:
  snapshot_dir: /tmp/test
profile1:
  ro:
    - ~/.zshrc
  hide:
    - "**/.env"
"#,
        );
        let config = Config::load_from(&path).unwrap();
        assert_eq!(
            config.settings.snapshot_dir,
            Some(std::path::PathBuf::from("/tmp/test"))
        );
        assert!(config.profiles.contains_key("profile1"));
        assert_eq!(config.profiles["profile1"].ro, vec!["~/.zshrc"]);
        assert_eq!(config.profiles["profile1"].hide, vec!["**/.env"]);
    }

    #[test]
    fn missing_file_returns_defaults() {
        let config =
            Config::load_from(std::path::Path::new("/nonexistent/path/inbox.yaml")).unwrap();
        assert!(config.settings.snapshot_dir.is_none());
        assert!(config.profiles.is_empty());
    }

    #[test]
    fn profile_with_based_on() {
        let dir = TempDir::new().unwrap();
        let path = write_config(
            &dir,
            r#"
base:
  ro:
    - ~/.ssh
derived:
  based_on: base
  hide:
    - ~/.aws
"#,
        );
        let config = Config::load_from(&path).unwrap();
        assert_eq!(
            config.profiles["derived"].based_on,
            Some("base".to_string())
        );
    }
}
