//! Terminal initialization, cleanup, and panic safety.

use std::io::{self, Stdout, stdout};

use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::ExecutableCommand as _;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

/// The terminal type used throughout the application.
pub type Tui = Terminal<CrosstermBackend<Stdout>>;

/// Enter raw mode and the alternate screen.
pub fn init() -> io::Result<Tui> {
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    Terminal::new(CrosstermBackend::new(stdout()))
}

/// Leave the alternate screen and restore cooked mode.
pub fn restore() -> io::Result<()> {
    stdout().execute(LeaveAlternateScreen)?;
    disable_raw_mode()
}

/// Install a panic hook that restores the terminal before printing.
pub fn install_panic_hook() {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore();
        original(info);
    }));
}
