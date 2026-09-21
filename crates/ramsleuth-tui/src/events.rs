//! P3-22 — the TUI input contract: key events -> actions.
//!
//! Two layers, deliberately split:
//!
//! - [`key_to_action`] is **pure** — a `crossterm::event::KeyEvent` in, an
//!   [`Option`] of [`Action`] out; no terminal, no I/O, fully
//!   deterministic. The whole keybinding table is unit-testable
//!   headlessly — the P3-22 exit criterion: the input contract is frozen
//!   and testable without a terminal.
//! - [`poll_event`] is the one place that touches crossterm's event
//!   stream (`poll` + `read`), so everything else in the crate stays
//!   I/O-free and testable. P3-24's main loop drives it with a 250 ms
//!   timeout; it is compile-checked here (a live poll needs a TTY).
//!
//! The frozen table (case-insensitive, modifiers ignored):
//!
//! | key         | action                 | effect (P3-24)                                 |
//! |-------------|------------------------|------------------------------------------------|
//! | `r` / `R`   | [`Action::Refresh`]    | force a telemetry refresh now                  |
//! | `s` / `S`   | [`Action::Snapshot`]   | write a timestamped `.txt` snapshot to the CWD |
//! | `q` / `Q`   | [`Action::Quit`]       | leave the TUI (exit 0)                         |
//! | `b` / `B`   | [`Action::BenchFull`]  | start a full bench run (`StartBenchmark`)      |
//! | `m` / `M`   | [`Action::BenchMemory`]| start a memory-only bench (`StartBenchmark`)   |
//! | `x` / `X`   | [`Action::BurnIn`]     | start a 5-min burn-in soak (`StartBurnIn`)     |
//! | `c` / `C`   | [`Action::Cancel`]     | cancel the in-flight run (`CancelBenchmark`)   |
//!
//! Everything else (other chars, `Esc`, `Enter`, arrows, function keys,
//! mouse, resize) maps to `None` and is ignored — the terminal re-reads
//! the surface size each frame, so resize needs no action of its own.

use crossterm::event::{Event, KeyEvent, KeyCode};

/// The user-facing TUI actions: the P3-22 trio (`Refresh`/`Snapshot`/
/// `Quit`) plus the TUI-01 bench-class quartet (`BenchFull`/`BenchMemory`/
/// `BurnIn`/`Cancel`), additive per the TUI-parity plan.
///
/// `Copy` so the P3-24 main loop can dispatch on owned values; the derived
/// `Eq` keeps the dispatch a plain `match`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Force a telemetry refresh immediately (next poll cycle).
    Refresh,
    /// Write a timestamped dashboard snapshot to the CWD (P3-24).
    Snapshot,
    /// Quit the TUI: restore the terminal, exit 0 (P3-24).
    Quit,
    /// Start a full benchmark run: `StartBenchmark { Full, Full }`.
    BenchFull,
    /// Start a memory-only benchmark run: `StartBenchmark { Full, MemoryOnly }`.
    BenchMemory,
    /// Start a burn-in soak: `StartBurnIn { Full, 5 }` (the GUI default minutes).
    BurnIn,
    /// Cancel the in-flight bench/burn-in run (shared flag + `CancelBenchmark`).
    Cancel,
}

/// Pure key -> action mapping: the whole keybinding table in one function.
///
/// Case-insensitive on the seven action keys (`r`/`R`, `s`/`S`, `q`/`Q`,
/// `b`/`B`, `m`/`M`, `x`/`X`, `c`/`C`); key modifiers are **ignored** (a
/// `Ctrl`- or `Alt`-prefixed action char still maps — simple and
/// deterministic, per the P3-22 scope boundary).
/// Every other key (`Esc`, `Enter`, arrows, function keys, other chars)
/// maps to `None`.
///
/// # Examples
///
/// ```
/// use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
/// use ramsleuth_tui::{key_to_action, Action};
///
/// assert_eq!(
///     key_to_action(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE)),
///     Some(Action::Refresh)
/// );
/// assert_eq!(
///     key_to_action(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
///     None
/// );
/// ```
pub fn key_to_action(key: KeyEvent) -> Option<Action> {
    match key.code {
        KeyCode::Char('r') | KeyCode::Char('R') => Some(Action::Refresh),
        KeyCode::Char('s') | KeyCode::Char('S') => Some(Action::Snapshot),
        KeyCode::Char('q') | KeyCode::Char('Q') => Some(Action::Quit),
        KeyCode::Char('b') | KeyCode::Char('B') => Some(Action::BenchFull),
        KeyCode::Char('m') | KeyCode::Char('M') => Some(Action::BenchMemory),
        KeyCode::Char('x') | KeyCode::Char('X') => Some(Action::BurnIn),
        KeyCode::Char('c') | KeyCode::Char('C') => Some(Action::Cancel),
        _ => None,
    }
}

/// One crossterm poll + read, isolated in a single function.
///
/// Wraps `crossterm::event::poll(timeout)` + `crossterm::event::read()`:
/// `Ok(Some(event))` when an event arrived within `timeout`, `Ok(None)` on
/// a clean timeout, `Err(io)` when the event stream itself fails (a
/// raw-mode teardown error degrades to a structured value here, never a
/// panic — the no-panic contract, plan D5). P3-24 maps `Event::Key(k)`
/// through [`key_to_action`] and ignores the rest.
pub fn poll_event(timeout: std::time::Duration) -> Result<Option<Event>, std::io::Error> {
    if !crossterm::event::poll(timeout)? {
        return Ok(None);
    }
    crossterm::event::read().map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    /// Build a plain (no-modifier) key event the way crossterm delivers a
    /// keypress — the tests assert only on the pure mapping, never on a
    /// live terminal (`poll_event` needs a TTY and is compile-checked only).
    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    // (a) 'r' / 'R' -> Refresh
    #[test]
    fn r_key_maps_to_refresh() {
        assert_eq!(key_to_action(key(KeyCode::Char('r'))), Some(Action::Refresh));
        assert_eq!(key_to_action(key(KeyCode::Char('R'))), Some(Action::Refresh));
    }

    // (b) 's' / 'S' -> Snapshot
    #[test]
    fn s_key_maps_to_snapshot() {
        assert_eq!(key_to_action(key(KeyCode::Char('s'))), Some(Action::Snapshot));
        assert_eq!(key_to_action(key(KeyCode::Char('S'))), Some(Action::Snapshot));
    }

    // (c) 'q' / 'Q' -> Quit
    #[test]
    fn q_key_maps_to_quit() {
        assert_eq!(key_to_action(key(KeyCode::Char('q'))), Some(Action::Quit));
        assert_eq!(key_to_action(key(KeyCode::Char('Q'))), Some(Action::Quit));
    }

    // (d) other chars -> None
    #[test]
    fn other_chars_map_to_none() {
        for code in [
            KeyCode::Char('a'),
            KeyCode::Char('0'),
            KeyCode::Char(' '),
        ] {
            assert_eq!(key_to_action(key(code)), None, "{code:?} must not be an action");
        }
    }

    // (e) non-char keys -> None
    #[test]
    fn non_char_keys_map_to_none() {
        for code in [
            KeyCode::Esc,
            KeyCode::Enter,
            KeyCode::Backspace,
            KeyCode::Tab,
            KeyCode::Left,
            KeyCode::Up,
            KeyCode::F(1),
        ] {
            assert_eq!(key_to_action(key(code)), None, "{code:?} must not be an action");
        }
    }

    // (f) 'b' / 'B' -> BenchFull
    #[test]
    fn b_key_maps_to_bench_full() {
        assert_eq!(key_to_action(key(KeyCode::Char('b'))), Some(Action::BenchFull));
        assert_eq!(key_to_action(key(KeyCode::Char('B'))), Some(Action::BenchFull));
    }

    // (g) 'm' / 'M' -> BenchMemory
    #[test]
    fn m_key_maps_to_bench_memory() {
        assert_eq!(key_to_action(key(KeyCode::Char('m'))), Some(Action::BenchMemory));
        assert_eq!(key_to_action(key(KeyCode::Char('M'))), Some(Action::BenchMemory));
    }

    // (h) 'x' / 'X' -> BurnIn
    #[test]
    fn x_key_maps_to_burn_in() {
        assert_eq!(key_to_action(key(KeyCode::Char('x'))), Some(Action::BurnIn));
        assert_eq!(key_to_action(key(KeyCode::Char('X'))), Some(Action::BurnIn));
    }

    // (i) 'c' / 'C' -> Cancel
    #[test]
    fn c_key_maps_to_cancel() {
        assert_eq!(key_to_action(key(KeyCode::Char('c'))), Some(Action::Cancel));
        assert_eq!(key_to_action(key(KeyCode::Char('C'))), Some(Action::Cancel));
    }

    // (j) every Action variant is distinct from every other
    #[test]
    fn all_actions_are_distinct() {
        let all = [
            Action::Refresh,
            Action::Snapshot,
            Action::Quit,
            Action::BenchFull,
            Action::BenchMemory,
            Action::BurnIn,
            Action::Cancel,
        ];
        for i in 0..all.len() {
            for j in (i + 1)..all.len() {
                assert_ne!(all[i], all[j], "every Action variant is distinct");
            }
        }
    }

    // modifiers are ignored: every action key maps with any modifier set
    #[test]
    fn modifiers_are_ignored() {
        for mods in [KeyModifiers::CONTROL, KeyModifiers::ALT, KeyModifiers::SHIFT] {
            assert_eq!(key_to_action(KeyEvent::new(KeyCode::Char('r'), mods)), Some(Action::Refresh));
            assert_eq!(key_to_action(KeyEvent::new(KeyCode::Char('s'), mods)), Some(Action::Snapshot));
            assert_eq!(key_to_action(KeyEvent::new(KeyCode::Char('q'), mods)), Some(Action::Quit));
            assert_eq!(key_to_action(KeyEvent::new(KeyCode::Char('b'), mods)), Some(Action::BenchFull));
            assert_eq!(key_to_action(KeyEvent::new(KeyCode::Char('m'), mods)), Some(Action::BenchMemory));
            assert_eq!(key_to_action(KeyEvent::new(KeyCode::Char('x'), mods)), Some(Action::BurnIn));
            assert_eq!(key_to_action(KeyEvent::new(KeyCode::Char('c'), mods)), Some(Action::Cancel));
        }
    }
}
