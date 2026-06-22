use std::path::Path;
use std::process::Command;

use tempfile::TempDir;
use wtcc::app::App;
use wtcc::config::Config;

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Creates a real git repository in a fresh tempdir. Falls back to a bare `.git`
/// directory when git is not on PATH, since `repository::register` only checks
/// for the presence of a `.git` entry.
fn init_repo() -> TempDir {
    let dir = tempfile::tempdir().expect("create tempdir");
    let path = dir.path();

    if git_available() {
        let output = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["-c", "init.defaultBranch=main", "init"])
            .output()
            .expect("failed to spawn git init");
        assert!(
            output.status.success(),
            "git init failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    } else {
        std::fs::create_dir(path.join(".git")).expect("create .git dir");
    }

    dir
}

fn app_with_temp_config(config_path: &Path) -> App {
    let mut app = App::new(Config::default());
    app.config_path = Some(config_path.to_path_buf());
    app
}

#[test]
fn register_adds_repo_and_persists_to_config() {
    let repo = init_repo();
    let config_dir = tempfile::tempdir().expect("config tempdir");
    let config_path = config_dir.path().join("config.toml");

    let mut app = app_with_temp_config(&config_path);
    app.register_repository(repo.path().to_str().unwrap());

    assert_eq!(app.config.repos.len(), 1, "repo should be added");
    assert_eq!(
        app.config.repos[0].path,
        repo.path(),
        "registered path should match input"
    );
    assert_eq!(app.selected_repo, Some(0), "new repo should be selected");

    let persisted = Config::load_from(&config_path).expect("config should be saved");
    assert_eq!(
        persisted.repos.len(),
        1,
        "config on disk should contain the repo"
    );
    assert_eq!(
        persisted, app.config,
        "in-memory and disk config must match"
    );
}

#[test]
fn register_invalid_path_sets_status_without_adding() {
    let config_dir = tempfile::tempdir().expect("config tempdir");
    let config_path = config_dir.path().join("config.toml");

    let mut app = app_with_temp_config(&config_path);
    app.register_repository("/no/such/path/anywhere");

    assert!(app.config.repos.is_empty(), "no repo should be added");
    assert!(app.status.is_some(), "status should report the failure");
    assert!(
        !config_path.exists(),
        "config must not be written on failure"
    );
}

#[test]
fn register_non_git_directory_sets_status_without_adding() {
    let plain = tempfile::tempdir().expect("plain dir");
    let config_dir = tempfile::tempdir().expect("config tempdir");
    let config_path = config_dir.path().join("config.toml");

    let mut app = app_with_temp_config(&config_path);
    app.register_repository(plain.path().to_str().unwrap());

    assert!(app.config.repos.is_empty(), "non-git dir must not register");
    assert!(app.status.is_some(), "status should report the failure");
}

#[test]
fn register_duplicate_path_does_not_add_twice() {
    let repo = init_repo();
    let config_dir = tempfile::tempdir().expect("config tempdir");
    let config_path = config_dir.path().join("config.toml");

    let mut app = app_with_temp_config(&config_path);
    let path = repo.path().to_str().unwrap();
    app.register_repository(path);
    app.register_repository(path);

    assert_eq!(app.config.repos.len(), 1, "duplicate should be rejected");
}

#[test]
fn register_empty_or_whitespace_sets_status_without_adding() {
    let config_dir = tempfile::tempdir().expect("config tempdir");
    let config_path = config_dir.path().join("config.toml");

    let mut app = app_with_temp_config(&config_path);
    app.register_repository("   ");

    assert!(app.config.repos.is_empty(), "no repo should be added");
    assert!(app.status.is_some(), "status should report the failure");
    assert!(
        !config_path.exists(),
        "config must not be written on failure"
    );
}
