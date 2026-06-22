use std::path::{Path, PathBuf};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

use crate::repository::Repository;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    #[serde(default)]
    pub repos: Vec<Repository>,
    #[serde(default = "default_agent_cmd")]
    pub agent_cmd: String,
}

fn default_agent_cmd() -> String {
    "claude".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Config {
            repos: Vec::new(),
            agent_cmd: default_agent_cmd(),
        }
    }
}

fn config_path() -> anyhow::Result<PathBuf> {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => dirs::config_dir().context("could not determine config directory")?,
    };
    Ok(base.join("wtcc").join("config.toml"))
}

impl Config {
    pub fn load() -> anyhow::Result<Config> {
        Self::load_from(&config_path()?)
    }

    pub fn save(&self) -> anyhow::Result<()> {
        self.save_to(&config_path()?)
    }

    pub fn load_from(path: &Path) -> anyhow::Result<Config> {
        if !path.exists() {
            return Ok(Config::default());
        }
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config at {}", path.display()))?;
        toml::from_str(&contents)
            .with_context(|| format!("failed to parse config at {}", path.display()))
    }

    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create config dir {}", parent.display()))?;
        }
        let contents = toml::to_string_pretty(self).context("failed to serialize config")?;
        std::fs::write(path, contents)
            .with_context(|| format!("failed to write config at {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_agent_cmd_is_claude() {
        assert_eq!(Config::default().agent_cmd, "claude");
        assert!(Config::default().repos.is_empty());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.toml");

        let original = Config {
            repos: vec![Repository {
                name: "my-repo".to_string(),
                path: PathBuf::from("/home/user/my-repo"),
            }],
            agent_cmd: "claude".to_string(),
        };

        original.save_to(&path).unwrap();
        let loaded = Config::load_from(&path).unwrap();

        assert_eq!(loaded, original);
    }

    #[test]
    fn load_from_missing_path_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.toml");

        let loaded = Config::load_from(&path).unwrap();
        assert_eq!(loaded, Config::default());
    }
}
