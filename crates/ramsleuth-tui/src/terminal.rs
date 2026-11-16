//! Terminal lifecycle guard (extracted from `main.rs`).
//!
//! [`TerminalGuard`] owns the initialized ratatui terminal after raw mode
//! + the alternate screen are enabled, and restores everything on drop:
//! disable raw mode, leave the alternate screen, show the cursor.
//! Because the restore is a `Drop` impl, the terminal is restored on
//! *every* exit path — a normal `[Q]`uit, an early `return`, or an
//! unwind — and the restore errors are swallowed (a half-dead terminal
//! is still better than a raw-mode zombie; the workspace no-panic
//! contract expects neither anyway).

use std::io;

use crossterm::cursor::{Hide, Show};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

/// A `Drop` guard around the initialized terminal.
pub struct TerminalGuard {
    /// The active ratatui terminal (until dropped); `pub` so the main
    /// loop's `while let Some(terminal) = guard.terminal.as_mut()` draw
    /// cadence can borrow it directly.
    pub terminal: Option<Terminal<CrosstermBackend<io::Stdout>>>,
}

impl TerminalGuard {
    /// Enable raw mode, enter the alternate screen (hiding the cursor),
    /// create the ratatui terminal, and clear it. Every partial failure
    /// unwinds what it already changed before returning the error — raw
    /// mode is never left on. The entry clear is the ghosting baseline:
    /// the alternate screen arrives with whatever pixels it last held
    /// (the main screen's content, a stale prior TUI's buffer), and the
    /// first frame must draw onto a blank surface.
    pub fn new() -> Result<Self, String> {
        enable_raw_mode().map_err(|e| format!("failed to enable raw mode: {e}"))?;
        let mut stdout = io::stdout();
        if let Err(e) = execute!(stdout, EnterAlternateScreen, Hide) {
            let _ = disable_raw_mode();
            return Err(format!("failed to enter the alternate screen: {e}"));
        }
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = match Terminal::new(backend) {
            Ok(terminal) => terminal,
            Err(e) => {
                let _ = execute!(io::stdout(), LeaveAlternateScreen, Show);
                let _ = disable_raw_mode();
                return Err(format!("failed to initialize the terminal: {e}"));
            }
        };
        if let Err(e) = terminal.clear() {
            let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen, Show);
            let _ = disable_raw_mode();
            return Err(format!("failed to clear the terminal: {e}"));
        }
        Ok(Self {
            terminal: Some(terminal),
        })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if let Some(mut terminal) = self.terminal.take() {
            let _ = disable_raw_mode();
            let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen, Show);
        }
    }
}
