use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

use crate::pr::MergeStrategy;
use crate::repository::Repository;

/// A named agent command, e.g. `{name = "codex", cmd = "codex --model x"}`. The
/// `cmd` is whitespace-split into an argv at spawn time (no shell).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentPreset {
    pub name: String,
    pub cmd: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    #[serde(default)]
    pub repos: Vec<Repository>,
    #[serde(default = "default_agent_cmd")]
    pub agent_cmd: String,
    /// Named agent presets. Empty in legacy configs; a single `default` preset is
    /// then synthesized from `agent_cmd` (see [`Config::presets`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<AgentPreset>,
    /// Per-worktree agent choice, keyed by branch -> preset name. Empty in legacy
    /// configs; an unmapped branch uses the first preset.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub worktree_agents: HashMap<String, String>,
    /// Whether to fire a desktop notification (via `notify-send`) when an agent
    /// goes quiet and needs input. Defaults to `true`; absent in legacy configs.
    #[serde(default = "default_notify")]
    pub notify: bool,
    /// Strategy used by the "Merge PR" action. Defaults to `Squash`; absent in
    /// legacy configs.
    #[serde(default)]
    pub merge_strategy: MergeStrategy,
}

fn default_agent_cmd() -> String {
    "claude".to_string()
}

fn default_notify() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Config {
            repos: Vec::new(),
            agent_cmd: default_agent_cmd(),
            agents: Vec::new(),
            worktree_agents: HashMap::new(),
            notify: default_notify(),
            merge_strategy: MergeStrategy::default(),
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

    /// The agent presets, with a single `default` synthesized from the scalar
    /// `agent_cmd` when no `agents` are defined (back-compat for legacy configs).
    pub fn presets(&self) -> Vec<AgentPreset> {
        if self.agents.is_empty() {
            vec![AgentPreset {
                name: "default".to_string(),
                cmd: self.agent_cmd.clone(),
            }]
        } else {
            self.agents.clone()
        }
    }

    /// Resolves the agent command for `branch`: its chosen preset's `cmd`, with a
    /// total fallback to the first preset when the branch is unmapped or its
    /// mapping names a preset that no longer exists. `presets()` is never empty,
    /// so this always returns a command.
    pub fn agent_cmd_for(&self, branch: &str) -> String {
        let presets = self.presets();
        self.worktree_agents
            .get(branch)
            .and_then(|name| presets.iter().find(|p| &p.name == name))
            .or_else(|| presets.first())
            .map(|p| p.cmd.clone())
            .unwrap_or_default()
    }

    /// Records `branch`'s agent choice by preset `name`. Persistence is the
    /// caller's responsibility.
    pub fn set_worktree_agent(&mut self, branch: &str, name: &str) {
        self.worktree_agents
            .insert(branch.to_string(), name.to_string());
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
                setup: None,
                archive: None,
                archived: Vec::new(),
                base_ref: None,
                copy_on_create: Vec::new(),
                run: None,
            }],
            agent_cmd: "claude".to_string(),
            notify: true,
            merge_strategy: MergeStrategy::default(),
            ..Default::default()
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

    // --- issue #47: notify defaults to true, legacy configs still load -------

    #[test]
    fn default_notify_is_true() {
        assert!(Config::default().notify);
    }

    #[test]
    fn legacy_config_without_notify_field_deserializes_to_true() {
        // A config.toml written before #47 has no `notify` key.
        let cfg: Config = toml::from_str("agent_cmd = \"claude\"\n").unwrap();
        assert!(cfg.notify);
    }

    // --- issue #49: per-repo setup/archive lifecycle scripts ----------------
    //
    // TDD RED: the config is fully additive/back-compatible. A repos entry
    // written before #49 has no `setup`/`archive` keys and must load with both
    // `None` (serde default); when a script is unset it must be omitted on save
    // (skip_serializing_if), and a populated entry must round-trip.

    #[test]
    fn legacy_repo_entry_without_lifecycle_fields_loads_as_none() {
        let cfg: Config = toml::from_str(
            "agent_cmd = \"claude\"\n[[repos]]\nname = \"demo\"\npath = \"/tmp/demo\"\n",
        )
        .unwrap();
        assert_eq!(cfg.repos[0].setup, None);
        assert_eq!(cfg.repos[0].archive, None);
    }

    #[test]
    fn lifecycle_scripts_round_trip_and_none_is_omitted_on_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let original = Config {
            repos: vec![Repository {
                name: "demo".to_string(),
                path: PathBuf::from("/home/user/demo"),
                setup: Some("npm install".to_string()),
                archive: None,
                archived: Vec::new(),
                base_ref: None,
                copy_on_create: Vec::new(),
                run: None,
            }],
            agent_cmd: "claude".to_string(),
            notify: true,
            merge_strategy: MergeStrategy::default(),
            ..Default::default()
        };

        original.save_to(&path).unwrap();
        let serialized = std::fs::read_to_string(&path).unwrap();
        assert!(
            serialized.contains("setup"),
            "a populated setup must be serialized, got:\n{serialized}"
        );
        assert!(
            !serialized.contains("archive"),
            "a None archive must be omitted via skip_serializing_if, got:\n{serialized}"
        );

        let loaded = Config::load_from(&path).unwrap();
        assert_eq!(loaded, original);
        assert_eq!(loaded.repos[0].setup.as_deref(), Some("npm install"));
        assert_eq!(loaded.repos[0].archive, None);
    }

    // --- issue #52: per-worktree agent presets ------------------------------
    //
    // TDD RED: presets are additive and back-compatible. `agents` (named presets)
    // and `worktree_agents` (branch -> preset name) both default to empty and are
    // omitted from the serialized output when empty. `presets()` derives a single
    // `default` preset from the scalar `agent_cmd` when none are defined, else
    // returns the defined list verbatim. `agent_cmd_for(branch)` resolves the
    // branch's chosen preset cmd, with a TOTAL fallback to the first preset's cmd
    // (unmapped branch, or a mapping naming a preset that no longer exists).
    // `Config::set_worktree_agent` records a branch -> preset-name choice.

    use std::collections::HashMap;

    fn preset(name: &str, cmd: &str) -> AgentPreset {
        AgentPreset {
            name: name.to_string(),
            cmd: cmd.to_string(),
        }
    }

    #[test]
    fn presets_derives_single_default_from_scalar_when_agents_empty() {
        let cfg = Config {
            agent_cmd: "claude".to_string(),
            ..Default::default()
        };
        let presets = cfg.presets();
        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].name, "default");
        assert_eq!(presets[0].cmd, "claude");
    }

    #[test]
    fn presets_returns_defined_agents_verbatim() {
        let agents = vec![
            preset("claude", "claude"),
            preset("codex", "codex --model x"),
        ];
        let cfg = Config {
            agents: agents.clone(),
            ..Default::default()
        };
        assert_eq!(cfg.presets(), agents);
    }

    #[test]
    fn agent_cmd_for_returns_the_mapped_presets_cmd() {
        let mut worktree_agents = HashMap::new();
        worktree_agents.insert("main".to_string(), "codex".to_string());
        let cfg = Config {
            agents: vec![
                preset("claude", "claude"),
                preset("codex", "codex --model x"),
            ],
            worktree_agents,
            ..Default::default()
        };
        assert_eq!(cfg.agent_cmd_for("main"), "codex --model x");
    }

    #[test]
    fn agent_cmd_for_preserves_a_multi_token_cmd_for_whitespace_argv_split() {
        // argv-split happens at spawn time (whitespace); the resolved cmd must be
        // returned intact so `codex --model x` becomes 3 argv elements downstream.
        let mut worktree_agents = HashMap::new();
        worktree_agents.insert("main".to_string(), "codex".to_string());
        let cfg = Config {
            agents: vec![preset("codex", "codex --model x")],
            worktree_agents,
            ..Default::default()
        };
        assert_eq!(cfg.agent_cmd_for("main"), "codex --model x");
        assert_eq!(cfg.agent_cmd_for("main").split_whitespace().count(), 3);
    }

    #[test]
    fn agent_cmd_for_falls_back_to_the_first_preset_when_branch_unmapped() {
        let cfg = Config {
            agents: vec![preset("claude", "claude"), preset("codex", "codex")],
            ..Default::default()
        };
        assert_eq!(cfg.agent_cmd_for("unmapped-branch"), "claude");
    }

    #[test]
    fn agent_cmd_for_falls_back_when_the_mapped_preset_name_is_missing() {
        let mut worktree_agents = HashMap::new();
        worktree_agents.insert("main".to_string(), "deleted-preset".to_string());
        let cfg = Config {
            agents: vec![preset("claude", "claude"), preset("codex", "codex")],
            worktree_agents,
            ..Default::default()
        };
        // The mapping names a preset that no longer exists -> total fallback.
        assert_eq!(cfg.agent_cmd_for("main"), "claude");
    }

    #[test]
    fn agent_cmd_for_uses_the_scalar_default_when_no_presets_defined() {
        let cfg = Config {
            agent_cmd: "claude".to_string(),
            ..Default::default()
        };
        assert_eq!(cfg.agent_cmd_for("anything"), "claude");
    }

    #[test]
    fn set_worktree_agent_records_the_branch_to_preset_choice() {
        let mut cfg = Config {
            agents: vec![preset("codex", "codex")],
            ..Default::default()
        };
        cfg.set_worktree_agent("main", "codex");
        assert_eq!(cfg.worktree_agents.get("main"), Some(&"codex".to_string()));
    }

    #[test]
    fn legacy_scalar_only_config_loads_with_empty_presets_and_a_working_default() {
        // A config.toml written before #52 has only repos + agent_cmd.
        let cfg: Config = toml::from_str("agent_cmd = \"claude\"\n").unwrap();
        assert!(cfg.agents.is_empty(), "no [[agents]] -> empty list");
        assert!(
            cfg.worktree_agents.is_empty(),
            "no worktree_agents -> empty map"
        );
        let presets = cfg.presets();
        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].name, "default");
        assert_eq!(presets[0].cmd, "claude");
        assert_eq!(cfg.agent_cmd_for("any-branch"), "claude");
    }

    #[test]
    fn agents_and_worktree_agents_round_trip_and_empty_maps_are_omitted() {
        let dir = tempfile::tempdir().unwrap();

        // Empty maps must be omitted from the serialized output.
        let empty_path = dir.path().join("empty.toml");
        Config {
            agent_cmd: "claude".to_string(),
            ..Default::default()
        }
        .save_to(&empty_path)
        .unwrap();
        let empty_serialized = std::fs::read_to_string(&empty_path).unwrap();
        assert!(
            !empty_serialized.contains("agents"),
            "empty agents/worktree_agents must be omitted, got:\n{empty_serialized}"
        );

        // Populated maps must serialize and round-trip exactly.
        let mut worktree_agents = HashMap::new();
        worktree_agents.insert("main".to_string(), "codex".to_string());
        let original = Config {
            agent_cmd: "claude".to_string(),
            agents: vec![
                preset("claude", "claude"),
                preset("codex", "codex --model x"),
            ],
            worktree_agents,
            ..Default::default()
        };
        let path = dir.path().join("populated.toml");
        original.save_to(&path).unwrap();
        let serialized = std::fs::read_to_string(&path).unwrap();
        assert!(
            serialized.contains("codex"),
            "populated agents must serialize, got:\n{serialized}"
        );

        let loaded = Config::load_from(&path).unwrap();
        assert_eq!(loaded, original);
    }

    // --- issue #54: per-repo base ref for NEW-branch worktrees --------------
    //
    // TDD RED: `Repository.base_ref: Option<String>` is additive and
    // back-compatible. A repos entry written before #54 has no `base_ref` key and
    // must load as `None` (serde default); an unset `base_ref` is omitted on save
    // (skip_serializing_if), and a populated one must round-trip.

    #[test]
    fn legacy_repo_entry_without_base_ref_loads_as_none() {
        let cfg: Config = toml::from_str(
            "agent_cmd = \"claude\"\n[[repos]]\nname = \"demo\"\npath = \"/tmp/demo\"\n",
        )
        .unwrap();
        assert_eq!(cfg.repos[0].base_ref, None);
    }

    #[test]
    fn base_ref_round_trips_and_none_is_omitted_on_save() {
        let dir = tempfile::tempdir().unwrap();

        // An unset base_ref must be omitted from the serialized output.
        let none_path = dir.path().join("none.toml");
        Config {
            repos: vec![Repository {
                name: "demo".to_string(),
                path: PathBuf::from("/home/user/demo"),
                base_ref: None,
                ..Default::default()
            }],
            ..Default::default()
        }
        .save_to(&none_path)
        .unwrap();
        let none_serialized = std::fs::read_to_string(&none_path).unwrap();
        assert!(
            !none_serialized.contains("base_ref"),
            "a None base_ref must be omitted via skip_serializing_if, got:\n{none_serialized}"
        );

        // A populated base_ref must serialize and round-trip exactly.
        let populated_path = dir.path().join("populated.toml");
        let original = Config {
            repos: vec![Repository {
                name: "demo".to_string(),
                path: PathBuf::from("/home/user/demo"),
                base_ref: Some("origin/main".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        };
        original.save_to(&populated_path).unwrap();
        let serialized = std::fs::read_to_string(&populated_path).unwrap();
        assert!(
            serialized.contains("base_ref") && serialized.contains("origin/main"),
            "a populated base_ref must be serialized, got:\n{serialized}"
        );

        let loaded = Config::load_from(&populated_path).unwrap();
        assert_eq!(loaded, original);
        assert_eq!(loaded.repos[0].base_ref.as_deref(), Some("origin/main"));
    }

    // --- issue #55: per-repo copy_on_create allowlist -----------------------
    //
    // TDD RED: `Repository.copy_on_create: Vec<String>` is its OWN additive,
    // back-compatible field (no shared bump). A repos entry written before #55
    // has no `copy_on_create` key and must load as an empty Vec (serde default);
    // an empty list is omitted on save (skip_serializing_if), and a populated one
    // round-trips exactly. The list holds relative paths from the repo root.

    #[test]
    fn legacy_repo_entry_without_copy_on_create_loads_as_empty() {
        let cfg: Config = toml::from_str(
            "agent_cmd = \"claude\"\n[[repos]]\nname = \"demo\"\npath = \"/tmp/demo\"\n",
        )
        .unwrap();
        assert!(cfg.repos[0].copy_on_create.is_empty());
    }

    #[test]
    fn copy_on_create_round_trips_and_empty_is_omitted_on_save() {
        let dir = tempfile::tempdir().unwrap();

        // An empty copy_on_create must be omitted from the serialized output.
        let empty_path = dir.path().join("empty.toml");
        Config {
            repos: vec![Repository {
                name: "demo".to_string(),
                path: PathBuf::from("/home/user/demo"),
                copy_on_create: Vec::new(),
                run: None,
                ..Default::default()
            }],
            ..Default::default()
        }
        .save_to(&empty_path)
        .unwrap();
        let empty_serialized = std::fs::read_to_string(&empty_path).unwrap();
        assert!(
            !empty_serialized.contains("copy_on_create"),
            "an empty copy_on_create must be omitted via skip_serializing_if, got:\n{empty_serialized}"
        );

        // A populated copy_on_create must serialize and round-trip exactly.
        let populated_path = dir.path().join("populated.toml");
        let original = Config {
            repos: vec![Repository {
                name: "demo".to_string(),
                path: PathBuf::from("/home/user/demo"),
                copy_on_create: vec![".env".to_string(), "config/local.toml".to_string()],
                ..Default::default()
            }],
            ..Default::default()
        };
        original.save_to(&populated_path).unwrap();
        let serialized = std::fs::read_to_string(&populated_path).unwrap();
        assert!(
            serialized.contains("copy_on_create") && serialized.contains(".env"),
            "a populated copy_on_create must be serialized, got:\n{serialized}"
        );

        let loaded = Config::load_from(&populated_path).unwrap();
        assert_eq!(loaded, original);
        assert_eq!(
            loaded.repos[0].copy_on_create,
            vec![".env".to_string(), "config/local.toml".to_string()]
        );
    }

    // --- issue #56: per-repo `run` command ----------------------------------
    //
    // TDD RED: `Repository.run: Option<String>` is additive and back-compatible,
    // exactly like setup/archive/base_ref. A repos entry written before #56 has
    // no `run` key and must load as `None` (serde default); an unset `run` is
    // omitted on save (skip_serializing_if), and a populated one round-trips.

    #[test]
    fn legacy_repo_entry_without_run_loads_as_none() {
        let cfg: Config = toml::from_str(
            "agent_cmd = \"claude\"\n[[repos]]\nname = \"demo\"\npath = \"/tmp/demo\"\n",
        )
        .unwrap();
        assert_eq!(cfg.repos[0].run, None);
    }

    #[test]
    fn run_round_trips_and_none_is_omitted_on_save() {
        let dir = tempfile::tempdir().unwrap();

        // An unset run must be omitted from the serialized output.
        let none_path = dir.path().join("none.toml");
        Config {
            repos: vec![Repository {
                name: "demo".to_string(),
                path: PathBuf::from("/home/user/demo"),
                run: None,
                ..Default::default()
            }],
            ..Default::default()
        }
        .save_to(&none_path)
        .unwrap();
        let none_serialized = std::fs::read_to_string(&none_path).unwrap();
        assert!(
            !none_serialized.contains("run"),
            "a None run must be omitted via skip_serializing_if, got:\n{none_serialized}"
        );

        // A populated run must serialize and round-trip exactly.
        let populated_path = dir.path().join("populated.toml");
        let original = Config {
            repos: vec![Repository {
                name: "demo".to_string(),
                path: PathBuf::from("/home/user/demo"),
                run: Some("pnpm dev".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        };
        original.save_to(&populated_path).unwrap();
        let serialized = std::fs::read_to_string(&populated_path).unwrap();
        assert!(
            serialized.contains("run") && serialized.contains("pnpm dev"),
            "a populated run must be serialized, got:\n{serialized}"
        );

        let loaded = Config::load_from(&populated_path).unwrap();
        assert_eq!(loaded, original);
        assert_eq!(loaded.repos[0].run.as_deref(), Some("pnpm dev"));
    }
}
