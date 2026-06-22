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

fn main() -> anyhow::Result<()> {
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
