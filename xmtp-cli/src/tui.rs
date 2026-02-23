//! Terminal setup and teardown.

use std::io::{self, Stdout, stdout};
use std::panic;

use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::ExecutableCommand as _;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

/// The terminal type used throughout the application.
pub type Tui = Terminal<CrosstermBackend<Stdout>>;

/// Enter raw mode and the alternate screen.
///
/// # Errors
///
/// Returns an error if terminal initialization fails.
pub fn init() -> io::Result<Tui> {
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    Terminal::new(CrosstermBackend::new(stdout()))
}

/// Leave the alternate screen and restore cooked mode.
///
/// # Errors
///
/// Returns an error if terminal restoration fails.
pub fn restore() -> io::Result<()> {
    stdout().execute(LeaveAlternateScreen)?;
    disable_raw_mode()
}

/// Install a panic hook that restores the terminal before printing.
pub fn install_panic_hook() {
    let original = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let _ = restore();
        original(info);
    }));
}
