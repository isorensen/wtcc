//! TDD RED contract for issue #52: per-worktree agent presets, selected through
//! the EXISTING palette/input mechanism (NO new overlay widget).
//!
//! These tests pin the issue's acceptance criteria and the contract in its
//! Technical notes. They are expected to FAIL (compile errors) until the
//! production API exists:
//!   - `wtcc::config::AgentPreset`, `Config.agents`, `Config.worktree_agents`,
//!     and `Config::{presets, agent_cmd_for, set_worktree_agent}`,
//!   - `App::set_worktree_agent` (persist with rollback + restart + status),
//!   - `Prompt::SwitchAgent` plus keymap `Action::SwitchAgent`
//!     (a primary key + `in_palette()`), wired through the data-driven keymap.
//!
//! No production code is written here. The agent process is never spawned: the
//! tests drive pure config resolution and the App/event state machine, with
//! worktrees injected directly (no real git, no tmux).

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use wtcc::app::{App, Overlay, Prompt};
use wtcc::config::{AgentPreset, Config};
use wtcc::event::handle_key;
use wtcc::keymap::{self, Action, PRIMARY};
use wtcc::repository::Repository;
use wtcc::session::SessionManager;
use wtcc::ui::palette;
use wtcc::worktree::Worktree;

// --- helpers ----------------------------------------------------------------

fn key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

fn preset(name: &str, cmd: &str) -> AgentPreset {
    AgentPreset {
        name: name.to_string(),
        cmd: cmd.to_string(),
    }
}

/// An App with one repo at a non-existent path and a single selected worktree
/// `main`, plus two presets (`claude`, `codex`). No real git or tmux is touched.
fn app_with_presets() -> App {
    let cfg = Config {
        repos: vec![Repository {
            name: "demo".to_string(),
            path: PathBuf::from("/tmp/wtcc-issue52-does-not-exist"),
            setup: None,
            archive: None,
            archived: Vec::new(),
            base_ref: None,
            copy_on_create: Vec::new(),
            run: None,
            kind: wtcc::repository::RepoKind::Git,
        }],
        agents: vec![
            preset("claude", "claude"),
            preset("codex", "codex --model x"),
        ],
        ..Default::default()
    };
    let mut app = App::new(cfg);
    app.worktrees = vec![Worktree {
        path: PathBuf::from("/repo/main"),
        branch: "main".to_string(),
        head: "abc".to_string(),
        is_bare: false,
        is_detached: false,
    }];
    app.worktree_repo = vec![0];
    app.selected_worktree = Some(0);
    app.status = None;
    app.overlay = Overlay::None;
    app
}

fn run_palette(app: &mut App, query: &str) {
    handle_key(app, key(':'));
    for c in query.chars() {
        handle_key(app, key(c));
    }
    handle_key(app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
}

// --- keymap: a primary key + palette membership, no chord collisions --------

#[test]
fn primary_binds_a_key_to_switch_agent() {
    assert_eq!(
        keymap::dispatch(PRIMARY, key('A')),
        Some(Action::SwitchAgent)
    );
}

#[test]
fn switch_agent_is_offered_in_the_palette() {
    assert!(Action::SwitchAgent.in_palette());
    assert!(palette::filter("").contains(&Action::SwitchAgent));
}

#[test]
fn switch_agent_action_has_a_label() {
    assert_eq!(Action::SwitchAgent.label(), "Switch agent");
}

#[test]
fn registering_switch_agent_introduces_no_chord_collisions_in_primary() {
    let chords: Vec<_> = PRIMARY
        .iter()
        .flat_map(|b| b.chords.iter().copied())
        .collect();
    for i in 0..chords.len() {
        for j in (i + 1)..chords.len() {
            assert_ne!(
                chords[i], chords[j],
                "duplicate chord within the PRIMARY keymap"
            );
        }
    }
}

// --- event: the key + palette open the SwitchAgent Input prompt --------------

#[test]
fn the_switch_agent_key_opens_the_switch_agent_input_prompt() {
    let mut app = app_with_presets();
    handle_key(&mut app, key('A'));
    assert!(
        matches!(
            app.overlay,
            Overlay::Input {
                prompt: Prompt::SwitchAgent,
                ..
            }
        ),
        "the switch-agent key must open an Input overlay, got {:?}",
        app.overlay
    );
}

#[test]
fn the_palette_switch_agent_command_opens_the_input_prompt() {
    let mut app = app_with_presets();
    run_palette(&mut app, "switch agent");
    assert!(
        matches!(
            app.overlay,
            Overlay::Input {
                prompt: Prompt::SwitchAgent,
                ..
            }
        ),
        "the palette Switch agent command must open the SwitchAgent Input overlay, got {:?}",
        app.overlay
    );
}

#[test]
fn switch_agent_with_no_worktree_selected_is_rejected() {
    let mut app = app_with_presets();
    app.worktrees.clear();
    app.selected_worktree = None;
    handle_key(&mut app, key('A'));
    assert_eq!(
        app.overlay,
        Overlay::None,
        "no Input prompt may open without a selected worktree"
    );
    assert_eq!(app.status.as_deref(), Some("no worktree selected"));
}

// --- event: submitting the prompt records, persists, and restarts ------------

#[test]
fn submitting_the_switch_agent_input_records_persists_and_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("config.toml");
    let mut app = app_with_presets();
    app.config_path = Some(cfg_path.clone());
    // Pretend the worktree's agent is live so the restart can clear it.
    app.active_session = Some(SessionManager::session_name(&app.worktree_key(0, "main")));

    handle_key(&mut app, key('A'));
    for c in "codex".chars() {
        handle_key(&mut app, key(c));
    }
    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(app.overlay, Overlay::None, "submit must close the overlay");
    assert_eq!(
        app.config.worktree_agents.get(&app.worktree_key(0, "main")),
        Some(&"codex".to_string()),
        "submitting a preset name must record it for the selected worktree (composite key)"
    );
    assert_eq!(
        app.active_session, None,
        "switching must restart the worktree's agent (clear the live session)"
    );
    assert!(app.status.is_some(), "the switch must report status");

    let persisted = Config::load_from(&cfg_path).unwrap();
    assert_eq!(
        persisted.worktree_agents.get(&app.worktree_key(0, "main")),
        Some(&"codex".to_string()),
        "the choice must persist so it survives a wtcc restart"
    );
}

// --- spawn-time resolution: chosen cmd is used, unchosen falls back ----------

#[test]
fn a_recorded_choice_resolves_to_that_presets_cmd_at_spawn_time() {
    let mut app = app_with_presets();
    let key = app.worktree_key(0, "main");
    app.config.set_worktree_agent(&key, "codex");
    // `ensure_active_session` spawns with `agent_cmd_for(<composite key>)`.
    assert_eq!(app.config.agent_cmd_for(&key), "codex --model x");
    // An untouched worktree still uses the default (first) preset.
    assert_eq!(app.config.agent_cmd_for("untouched"), "claude");
}

// --- no-panic guards (issue acceptance criterion) ----------------------------

#[test]
fn switching_to_an_unknown_preset_does_not_panic_and_falls_back() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with_presets();
    app.config_path = Some(dir.path().join("config.toml"));

    app.set_worktree_agent("main", "ghost-agent");

    // No panic; resolution totally falls back to the first preset's cmd.
    assert_eq!(
        app.config.agent_cmd_for(&app.worktree_key(0, "main")),
        "claude"
    );
    assert!(app.status.is_some(), "an outcome must be reported");
}

#[test]
fn a_failed_save_rolls_the_choice_back_and_reports_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with_presets();
    // Point config_path at an EXISTING DIRECTORY so the write fails.
    app.config_path = Some(dir.path().to_path_buf());

    app.set_worktree_agent("main", "codex");

    assert!(
        !app.config
            .worktree_agents
            .contains_key(&app.worktree_key(0, "main")),
        "a failed save must roll the choice back out of the in-memory config"
    );
    let status = app.status.clone().unwrap_or_default().to_lowercase();
    assert!(
        status.contains("save"),
        "a save failure must be reported in status, got {status:?}"
    );
}
