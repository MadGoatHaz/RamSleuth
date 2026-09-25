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
//! | key         | action                         | effect (P3-24)                                 |
//! |------------|-------------------------------|-----------------------------------------------|
//! | `r` / `R`   | [`Action::Refresh`]            | force a telemetry refresh now                  |
//! | `s` / `S`   | [`Action::Snapshot`]           | write a timestamped `.txt` snapshot to the CWD |
//! | `q` / `Q`   | [`Action::Quit`]               | leave the TUI (exit 0)                         |
//! | `b` / `B`   | [`Action::BenchFull`]          | start a full bench run (`StartBenchmark`)      |
//! | `m` / `M`   | [`Action::BenchMemory`]        | start a memory-only bench (`StartBenchmark`)   |
//! | `x` / `X`   | [`Action::BurnIn`]             | start a 5-min burn-in soak (`StartBurnIn`)     |
//! | `c` / `C`   | [`Action::Cancel`]             | cancel the in-flight run (`CancelBenchmark`)   |
//! | `g` / `G`   | [`Action::ToggleGraphs`]       | toggle the graphs panel                        |
//! | `t` / `T`   | [`Action::ToggleSettings`]     | toggle the settings strip                      |
//! | `d` / `D`   | [`Action::ToggleRequirements`] | toggle the requirements strip                  |
//! | `e` / `E`   | [`Action::ExportJson`]         | write `{ telemetry, bench }` JSON to `$HOME`   |
//! | `p` / `P`   | [`Action::CyclePoll`]          | cycle poll interval (100 ms → 60 s, wrap)      |
//! | `u` / `U`   | [`Action::ToggleCapacity`]     | toggle capacity GiB ↔ GB                       |
//! | `k` / `K`   | [`Action::ToggleClock`]        | toggle clock MHz ↔ GHz                         |
//! | `a` / `A`   | [`Action::ToggleRefresh`]      | toggle refresh on ↔ off                        |
//! | `w` / `W`   | [`Action::CycleWindow`]        | cycle graphs window (1 → 60 min)               |
//! | `f` / `F`   | [`Action::ProbeReport`]        | consent → preview → file / clipboard (probe-4) |
//!
//! Everything else (other chars, `Esc`, `Enter`, arrows, function keys,
//! mouse, resize) maps to `None` and is ignored — the terminal re-reads
//! the surface size each frame, so resize needs no action of its own.

use crossterm::event::{Event, KeyEvent, KeyCode};

/// The user-facing TUI actions: the P3-22 trio (`Refresh`/`Snapshot`/
/// `Quit`), the TUI-01 bench-class quartet (`BenchFull`/`BenchMemory`/
/// `BurnIn`/`Cancel`), the TUI-02 view class (`ToggleGraphs`/
/// `ToggleSettings`/`ToggleRequirements`/`ExportJson`/`CyclePoll`/
/// `ToggleCapacity`/`ToggleClock`/`ToggleRefresh`/`CycleWindow`),
/// and the probe-report action (`ProbeReport`, chunk probe-4 — the
/// consent-gated `GetProbeReport` fetch), additive per the
/// TUI-parity plan.
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
    /// Toggle the graphs overlay panel (the 5-series sparkline view).
    ToggleGraphs,
    /// Toggle the settings strip (poll interval, units, refresh, socket).
    ToggleSettings,
    /// Toggle the requirements strip (setup prerequisites, presence-driven).
    ToggleRequirements,
    /// Write a `{ telemetry, bench }` JSON export to `$HOME` (F3 parity).
    ExportJson,
    /// Cycle the poll-interval presets: 100 ms … 60 s (default 2 s).
    CyclePoll,
    /// Toggle the capacity units: GiB ↔ GB.
    ToggleCapacity,
    /// Toggle the clock units: MHz ↔ GHz.
    ToggleClock,
    /// Toggle auto-refresh on ↔ off (`r`/`R` still forces a poll).
    ToggleRefresh,
    /// Cycle the graphs window presets: 1 → 5 → 15 → 60 min (default 5).
    CycleWindow,
    /// Gather the probe report (chunk probe-4): the one-shot
    /// `GetProbeReport` fetch → the consent prompt → the markdown
    /// preview (`[w]` write / `[c]` copy / `[q]` quit preview).
    /// Bound to `f`/`F` — `r`/`R` is the frozen P3-22
    /// `[R]`efresh key (the table is case-insensitive).
    ProbeReport,
}

/// Pure key -> action mapping: the whole keybinding table in one function.
///
/// Case-insensitive on the seventeen action keys (`r`/`s`/`q`/`b`/
/// `m`/`x`/`c`/`g`/`t`/`d`/`e`/`p`/`u`/`k`/`a`/`w`/`f`); key modifiers
/// are **ignored** (a `Ctrl`- or `Alt`-prefixed action char still
/// maps — simple and deterministic, per the P3-22 scope boundary).
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
        KeyCode::Char('g') | KeyCode::Char('G') => Some(Action::ToggleGraphs),
        KeyCode::Char('t') | KeyCode::Char('T') => Some(Action::ToggleSettings),
        KeyCode::Char('d') | KeyCode::Char('D') => Some(Action::ToggleRequirements),
        KeyCode::Char('e') | KeyCode::Char('E') => Some(Action::ExportJson),
        KeyCode::Char('p') | KeyCode::Char('P') => Some(Action::CyclePoll),
        KeyCode::Char('u') | KeyCode::Char('U') => Some(Action::ToggleCapacity),
        KeyCode::Char('k') | KeyCode::Char('K') => Some(Action::ToggleClock),
        KeyCode::Char('a') | KeyCode::Char('A') => Some(Action::ToggleRefresh),
        KeyCode::Char('w') | KeyCode::Char('W') => Some(Action::CycleWindow),
        KeyCode::Char('f') | KeyCode::Char('F') => Some(Action::ProbeReport),
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
        for code in [KeyCode::Char('0'), KeyCode::Char(' ')] {
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
            Action::ToggleGraphs,
            Action::ToggleSettings,
            Action::ToggleRequirements,
            Action::ExportJson,
            Action::CyclePoll,
            Action::ToggleCapacity,
            Action::ToggleClock,
            Action::ToggleRefresh,
            Action::CycleWindow,
            Action::ProbeReport,
        ];
        for i in 0..all.len() {
            for j in (i + 1)..all.len() {
                assert_ne!(all[i], all[j], "every Action variant is distinct");
            }
        }
    }

    // (k) 'g' / 'G' -> ToggleGraphs
    #[test]
    fn g_key_maps_to_toggle_graphs() {
        assert_eq!(key_to_action(key(KeyCode::Char('g'))), Some(Action::ToggleGraphs));
        assert_eq!(key_to_action(key(KeyCode::Char('G'))), Some(Action::ToggleGraphs));
    }

    // (l) 't' / 'T' -> ToggleSettings
    #[test]
    fn t_key_maps_to_toggle_settings() {
        assert_eq!(key_to_action(key(KeyCode::Char('t'))), Some(Action::ToggleSettings));
        assert_eq!(key_to_action(key(KeyCode::Char('T'))), Some(Action::ToggleSettings));
    }

    // (m) 'd' / 'D' -> ToggleRequirements
    #[test]
    fn d_key_maps_to_toggle_requirements() {
        assert_eq!(key_to_action(key(KeyCode::Char('d'))), Some(Action::ToggleRequirements));
        assert_eq!(key_to_action(key(KeyCode::Char('D'))), Some(Action::ToggleRequirements));
    }

    // (n) 'e' / 'E' -> ExportJson
    #[test]
    fn e_key_maps_to_export_json() {
        assert_eq!(key_to_action(key(KeyCode::Char('e'))), Some(Action::ExportJson));
        assert_eq!(key_to_action(key(KeyCode::Char('E'))), Some(Action::ExportJson));
    }

    // (o) 'p' / 'P' -> CyclePoll
    #[test]
    fn p_key_maps_to_cycle_poll() {
        assert_eq!(key_to_action(key(KeyCode::Char('p'))), Some(Action::CyclePoll));
        assert_eq!(key_to_action(key(KeyCode::Char('P'))), Some(Action::CyclePoll));
    }

    // (p) 'u' / 'U' -> ToggleCapacity
    #[test]
    fn u_key_maps_to_toggle_capacity() {
        assert_eq!(key_to_action(key(KeyCode::Char('u'))), Some(Action::ToggleCapacity));
        assert_eq!(key_to_action(key(KeyCode::Char('U'))), Some(Action::ToggleCapacity));
    }

    // (q) 'k' / 'K' -> ToggleClock
    #[test]
    fn k_key_maps_to_toggle_clock() {
        assert_eq!(key_to_action(key(KeyCode::Char('k'))), Some(Action::ToggleClock));
        assert_eq!(key_to_action(key(KeyCode::Char('K'))), Some(Action::ToggleClock));
    }

    // (r) 'a' / 'A' -> ToggleRefresh
    #[test]
    fn a_key_maps_to_toggle_refresh() {
        assert_eq!(key_to_action(key(KeyCode::Char('a'))), Some(Action::ToggleRefresh));
        assert_eq!(key_to_action(key(KeyCode::Char('A'))), Some(Action::ToggleRefresh));
    }

    // (s) 'w' / 'W' -> CycleWindow
    #[test]
    fn w_key_maps_to_cycle_window() {
        assert_eq!(key_to_action(key(KeyCode::Char('w'))), Some(Action::CycleWindow));
        assert_eq!(key_to_action(key(KeyCode::Char('W'))), Some(Action::CycleWindow));
    }

    // (t) 'f' / 'F' -> ProbeReport (chunk probe-4 — `r`/`R` stays
    // the frozen P3-22 Refresh key; the table is case-insensitive)
    #[test]
    fn f_key_maps_to_probe_report() {
        assert_eq!(key_to_action(key(KeyCode::Char('f'))), Some(Action::ProbeReport));
        assert_eq!(key_to_action(key(KeyCode::Char('F'))), Some(Action::ProbeReport));
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
            assert_eq!(key_to_action(KeyEvent::new(KeyCode::Char('g'), mods)), Some(Action::ToggleGraphs));
            assert_eq!(key_to_action(KeyEvent::new(KeyCode::Char('t'), mods)), Some(Action::ToggleSettings));
            assert_eq!(key_to_action(KeyEvent::new(KeyCode::Char('d'), mods)), Some(Action::ToggleRequirements));
            assert_eq!(key_to_action(KeyEvent::new(KeyCode::Char('e'), mods)), Some(Action::ExportJson));
            assert_eq!(key_to_action(KeyEvent::new(KeyCode::Char('p'), mods)), Some(Action::CyclePoll));
            assert_eq!(key_to_action(KeyEvent::new(KeyCode::Char('u'), mods)), Some(Action::ToggleCapacity));
            assert_eq!(key_to_action(KeyEvent::new(KeyCode::Char('k'), mods)), Some(Action::ToggleClock));
            assert_eq!(key_to_action(KeyEvent::new(KeyCode::Char('a'), mods)), Some(Action::ToggleRefresh));
            assert_eq!(key_to_action(KeyEvent::new(KeyCode::Char('w'), mods)), Some(Action::CycleWindow));
            assert_eq!(key_to_action(KeyEvent::new(KeyCode::Char('f'), mods)), Some(Action::ProbeReport));
        }
    }
}
