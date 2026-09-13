//! ramsleuth-tui — Terminal user interface (ratatui + crossterm).
//!
//! Renders the dense timing matrix and the AIDA64-style benchmark grid in
//! the terminal (ideal for headless servers / SSH). Keybindings: `[R]`
//! Refresh, `[S]` Export snapshot, `[Q]` Quit — the frozen `Action` table
//! and the pure key mapping live in the library (`ramsleuth_tui::events`,
//! P3-22); the three-zone renderer lands in P3-23.
//!
//! Scaffold stub: no behavior yet — P3-24 rewrites this entry point (raw
//! mode + alternate screen with a Drop-safe restore, the background 2 s
//! telemetry updater over `ramsleuth-client`, the `poll_event`/draw loop).

fn main() {
    // Scaffold stub: no behavior yet (P3-24).
}
