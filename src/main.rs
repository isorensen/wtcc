//! wtcc — WorkTree Command Center.
//!
//! Entry point: owns the terminal lifecycle and the draw/poll/update loop.
//! All domain and UI logic lives in the library crate (`wtcc`).

use std::io::{self, Stdout};
use std::time::Duration;

use anyhow::Context as _;
use crossterm::event::{self, Event};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;

use wtcc::app::App;
use wtcc::config::Config;
use wtcc::event::handle_key;
use wtcc::ui;

const POLL: Duration = Duration::from_millis(16);

/// Parsed result of the command-line arguments. Kept pure (no I/O) so it is
/// unit-testable without process exit.
#[derive(Debug, PartialEq, Eq)]
enum Cli {
    Run,
    Version,
    Help,
    Unknown(String),
}

/// Classify the CLI arguments. Skips argv[0] (the binary name).
fn parse_args(mut args: impl Iterator<Item = String>) -> Cli {
    // Skip the binary name.
    args.next();
    match args.next().as_deref() {
        None => Cli::Run,
        Some("--version" | "-V") => Cli::Version,
        Some("--help" | "-h") => Cli::Help,
        Some(flag) if flag.starts_with('-') => Cli::Unknown(flag.to_string()),
        Some(_) => Cli::Run,
    }
}

fn main() -> anyhow::Result<()> {
    match parse_args(std::env::args()) {
        Cli::Version => {
            println!("wtcc {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Cli::Help => {
            println!(
                "wtcc {} — WorkTree Command Center\n\
                \n\
                A full-screen TUI for orchestrating Claude Code agents across git worktrees.\n\
                There are no subcommands; wtcc opens the interface directly.\n\
                \n\
                Runtime requirements:\n\
                  git    required\n\
                  tmux   required\n\
                  gh     optional (PR/CI status badges)\n\
                  claude optional (agent pane)\n\
                \n\
                Press ? inside wtcc to see keybindings.",
                env!("CARGO_PKG_VERSION"),
            );
            return Ok(());
        }
        Cli::Unknown(flag) => {
            eprintln!("error: unknown option `{flag}`");
            eprintln!("Usage: wtcc [--version | --help]");
            std::process::exit(2);
        }
        Cli::Run => {}
    }

    install_panic_hook();

    let config = Config::load().context("failed to load config")?;
    let mut terminal = setup_terminal().context("failed to set up terminal")?;

    let result = run(&mut terminal, App::new(config));

    restore_terminal().ok();
    result
}

fn run(terminal: &mut Terminal<CrosstermBackend<Stdout>>, mut app: App) -> anyhow::Result<()> {
    let mut last_size: Option<(u16, u16)> = None;
    while !app.should_quit {
        let size = terminal.size()?;
        let area = Rect::new(0, 0, size.width, size.height);
        let (rows, cols) = ui::agent_pane_size(area);
        app.ensure_active_session(rows, cols);
        app.drain_vcs();
        if last_size != Some((rows, cols)) {
            app.session_manager.resize_all(rows, cols);
            last_size = Some((rows, cols));
        }

        terminal.draw(|frame| ui::draw(frame, &app))?;

        if event::poll(POLL)?
            && let Event::Key(key) = event::read()?
            && key.kind == event::KeyEventKind::Press
        {
            handle_key(&mut app, key);
        }
    }
    Ok(())
}

fn setup_terminal() -> anyhow::Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    Ok(Terminal::new(CrosstermBackend::new(stdout))?)
}

fn restore_terminal() -> anyhow::Result<()> {
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen, crossterm::cursor::Show)?;
    Ok(())
}

/// Installs a panic hook that always restores the terminal before the default
/// hook prints. Load-bearing: without it a panic would leave the user's
/// terminal in raw mode on the alternate screen with the cursor hidden.
fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal().ok();
        default(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> impl Iterator<Item = String> {
        v.iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn parse_args_no_args_is_run() {
        assert_eq!(parse_args(args(&["wtcc"])), Cli::Run);
    }

    #[test]
    fn parse_args_version_flags() {
        assert_eq!(parse_args(args(&["wtcc", "--version"])), Cli::Version);
        assert_eq!(parse_args(args(&["wtcc", "-V"])), Cli::Version);
    }

    #[test]
    fn parse_args_help_flags() {
        assert_eq!(parse_args(args(&["wtcc", "--help"])), Cli::Help);
        assert_eq!(parse_args(args(&["wtcc", "-h"])), Cli::Help);
    }

    #[test]
    fn parse_args_unknown_flag() {
        assert_eq!(
            parse_args(args(&["wtcc", "--bogus"])),
            Cli::Unknown("--bogus".to_string())
        );
    }
}
