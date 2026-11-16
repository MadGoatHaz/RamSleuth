//! Action dispatch (extracted from `main.rs`).
//!
//! TUI-22 + chunk probe-4: the frozen 17-key table (P3-22 + TUI-01/02 +
//! the probe-report action) over the shared state: the state-only
//! actions (the toggles, the cycles, the run keys, the cancel) mutate
//! the state and report no side effect; the I/O actions report the
//! effect the main loop performs with the lock released.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::RwLock;

use ramsleuth_bench::StreamTarget;
use ramsleuth_protocol::BenchMode;
use ramsleuth_tui::requirements::diagnose;
use ramsleuth_tui::{Action, AppState};

use super::{state_read, state_write};
use crate::bench_worker::TuiBenchCmd;
use crate::polling::run_in_flight;

/// The I/O side effect one dispatched action asks the main loop to
/// perform with the state lock released (TUI-22 — the dispatch
/// split that makes the key `match` unit-testable without a TTY;
/// the state-only actions report [`None`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideEffect {
    /// No side effect: the action mutated the state only (the
    /// toggles, the cycles, the run keys, the cancel).
    None,
    /// `[R]`: force one immediate `poll_once` on the main thread.
    Refresh,
    /// `[S]`: write the timestamped text snapshot to the CWD.
    Snapshot,
    /// `[E]`: write the JSON export to `$HOME` (the CWD fallback).
    Export,
    /// `[F]`: fetch the probe report (one RPC on a fresh
    /// connection) + open the consent overlay (chunk probe-4).
    ProbeReport,
    /// `[Q]`: quit the TUI (exit 0).
    Quit,
}

/// The poll-interval presets in cycle order (plan §2.2 `[p]`):
/// 100 ms → 500 ms → 1 s → 2 s → 5 s → 10 s → 30 s → 60 s — the
/// 2 s default sits mid-list (the current TUI behavior).
const POLL_PRESETS_MS: [u64; 8] = [100, 500, 1_000, 2_000, 5_000, 10_000, 30_000, 60_000];

/// The graphs window presets in cycle order (plan §2.2 `[w]`,
/// minutes): 1 → 5 → 15 → 60 — the 5-min default (the current TUI
/// behavior).
const WINDOW_PRESETS_MIN: [u32; 4] = [1, 5, 15, 60];

/// The dim status note a skipped run key records (TUI-22 — the GUI
/// single-flight UX over the transient `daemon_status` mechanism):
/// the next poll overwrites it, as the `[S]`napshot path does.
pub const RUN_IN_FLIGHT_NOTE: &str = "a run is already in flight (one at a time)";

/// The next poll-interval preset after `ms` (the `[p]` cycle, plan
/// §2.2): the first preset strictly above the current value — an
/// off-preset stored value (a hand-edited default) advances to the
/// next preset above it, and past the last (60 s) the cycle wraps to
/// the first (100 ms). Total — never panics (D5).
pub fn cycle_poll(ms: u64) -> u64 {
    POLL_PRESETS_MS
        .iter()
        .copied()
        .find(|preset| *preset > ms)
        .unwrap_or(POLL_PRESETS_MS[0])
}

/// The next graphs window preset after `min` (the `[w]` cycle, plan
/// §2.2): the first preset strictly above the current value,
/// wrapping past 60 min to 1 min — total (D5).
pub fn cycle_window(min: u32) -> u32 {
    WINDOW_PRESETS_MIN
        .iter()
        .copied()
        .find(|preset| *preset > min)
        .unwrap_or(WINDOW_PRESETS_MIN[0])
}

/// The `TuiBenchCmd` one run key starts (plan §2.2): the cell
/// target + the scope + the run-class discriminator — `[b]` a full
/// run, `[m]` a memory-only run, `[x]` a 5-minute burn-in (the GUI
/// default duration; `0` = infinite is not key-reachable, plan
/// §5.6). The non-run actions carry no command.
fn bench_cmd(action: Action) -> Option<TuiBenchCmd> {
    match action {
        Action::BenchFull => Some(TuiBenchCmd {
            target: StreamTarget::Full,
            mode: BenchMode::Full,
            duration_minutes: None,
        }),
        Action::BenchMemory => Some(TuiBenchCmd {
            target: StreamTarget::Full,
            mode: BenchMode::MemoryOnly,
            duration_minutes: None,
        }),
        Action::BurnIn => Some(TuiBenchCmd {
            target: StreamTarget::Full,
            mode: BenchMode::Full,
            duration_minutes: Some(5),
        }),
        _ => None,
    }
}

/// The one-action dispatch over the shared state (TUI-22 — the
/// main loop's key `match` split into this pure, unit-testable fn +
/// the I/O arms the loop performs from its return value, so the
/// dispatch is testable without a TTY): the state-only actions
/// mutate `state` and return [`SideEffect::None`] — the `[g]`/`[t]`
/// toggles flip the `settings.*_open` flags, the `[d]` key toggles
/// the requirements strip *and* drives the `requirements_dismissed`
/// latch (closing dismisses the auto-open — a new requirement stays
/// hidden until the user toggles the strip open again; reopening
/// clears the dismissal — the auto-open applies again), the `[p]`
/// poll / `[w]` window cycles advance the presets (the renderer
/// reads `settings.graph_window_min` — the `[w]` cycle is where
/// that value is supplied), the `[u]`/`[k]`/`[a]` toggles flip the
/// unit / refresh knobs, the `[b]`/`[m]`/`[x]` run keys queue a
/// `TuiBenchCmd` into the updater's bench channel (a channel send —
/// no I/O; skipped + a dim status note while any run is in flight,
/// the compound single-flight guard), and `[c]` sets the shared
/// cancel flag; the I/O actions return their effect for the loop to
/// perform with the lock released: `[R]` → [`SideEffect::Refresh`],
/// `[S]` → [`SideEffect::Snapshot`], `[E]` → [`SideEffect::Export`],
/// `[F]` → [`SideEffect::ProbeReport`], `[Q]` →
/// [`SideEffect::Quit`].
pub fn dispatch_action(
    action: Action,
    state: &mut AppState,
    requirements_dismissed: &mut bool,
    bench_tx: &Sender<TuiBenchCmd>,
    cancel: &AtomicBool,
) -> SideEffect {
    match action {
        Action::Refresh => SideEffect::Refresh,
        Action::Snapshot => SideEffect::Snapshot,
        Action::Quit => SideEffect::Quit,
        Action::BenchFull | Action::BenchMemory | Action::BurnIn => {
            if run_in_flight(&state.bench) {
                // The compound single-flight guard (TUI-19/20): a
                // second start is skipped (the daemon would answer
                // with its structured `Error` anyway) + a dim
                // status note recorded (the GUI single-flight UX,
                // TUI-22 — the transient `daemon_status`
                // mechanism, as the `[S]`napshot path).
                state.daemon_status = RUN_IN_FLIGHT_NOTE.to_owned();
            } else if let Some(cmd) = bench_cmd(action) {
                // The command goes to the updater (a channel send —
                // no I/O, the one permitted main-thread mutation);
                // the run streams on the updater thread.
                let _ = bench_tx.send(cmd);
            }
            SideEffect::None
        }
        Action::Cancel => {
            // The shared flag (TUI-19/20): the in-flight run's
            // worker checks it between frames and stops the run
            // daemon-side. No I/O on the key.
            cancel.store(true, Ordering::Relaxed);
            SideEffect::None
        }
        Action::ExportJson => SideEffect::Export,
        Action::ProbeReport => SideEffect::ProbeReport,
        Action::ToggleGraphs => {
            state.settings.graphs_open = !state.settings.graphs_open;
            SideEffect::None
        }
        Action::ToggleSettings => {
            state.settings.settings_open = !state.settings.settings_open;
            SideEffect::None
        }
        Action::ToggleRequirements => {
            // The auto-open-until-closed rule (the plan's
            // `requirements_dismissed` latch): closing the strip
            // dismisses the auto-open (a later requirement
            // re-appearance stays hidden until the user toggles it
            // open again); reopening clears the dismissal (the
            // auto-open applies again while a requirement exists).
            if state.settings.requirements_open {
                state.settings.requirements_open = false;
                *requirements_dismissed = true;
            } else {
                state.settings.requirements_open = true;
                *requirements_dismissed = false;
            }
            SideEffect::None
        }
        Action::CyclePoll => {
            state.settings.poll_interval_ms = cycle_poll(state.settings.poll_interval_ms);
            SideEffect::None
        }
        Action::ToggleCapacity => {
            state.settings.capacity_gib = !state.settings.capacity_gib;
            SideEffect::None
        }
        Action::ToggleClock => {
            state.settings.clock_mhz = !state.settings.clock_mhz;
            SideEffect::None
        }
        Action::ToggleRefresh => {
            state.settings.refresh = !state.settings.refresh;
            SideEffect::None
        }
        Action::CycleWindow => {
            // The renderer reads `settings.graph_window_min` (ui.rs
            // `render` → `render_graphs_panel`): this cycle is where
            // that value is supplied.
            state.settings.graph_window_min = cycle_window(state.settings.graph_window_min);
            SideEffect::None
        }
    }
}

/// The requirements strip's auto-open-until-closed rule (TUI-22 —
/// the main-thread half of the plan's TUI-16 presence rule; the
/// renderer only reads the flag): while
/// [`ramsleuth_tui::requirements::diagnose`] reports a requirement
/// and the user has not dismissed the strip (the `[d]` key closed
/// it — the `requirements_dismissed` latch), the
/// `requirements_open` flag is force-true — a new requirement
/// re-opens the strip on its own; a dismissed strip stays closed
/// until the user toggles it open again. The read scope decides,
/// the (rare) write sets one field — never held long (C14-03).
pub fn apply_requirements_auto_open(state: &RwLock<AppState>, dismissed: bool) {
    if dismissed {
        return;
    }
    let needs_open = {
        let app = state_read(state);
        !app.settings.requirements_open && !diagnose(&app).is_empty()
    };
    if needs_open {
        state_write(state).settings.requirements_open = true;
    }
}

/// The `[d]` screen's + the settings strip's visibility [`render`]
/// draws: the controls block (the 17-key map) its `requirements_open`
/// flag alone (the screen's informational companion — drawn while the
/// screen is open, regardless of the strip's presence), the
/// requirements strip presence-driven — its `requirements_open` flag
/// AND a non-empty [`ramsleuth_tui::requirements::diagnose`] (the
/// strip auto-vanishes on the daemon-connect transition — `diagnose`
/// empties — so the flag alone would miss it) — and the settings strip
/// its `settings_open` flag alone. `render` applies this rule inline;
/// this helper is retained (test-only, FE-4) to pin the presence-driven
/// auto-vanish invariant.
#[cfg(test)]
pub fn strip_visibility(state: &AppState) -> (bool, bool, bool) {
    (
        state.settings.requirements_open,
        !diagnose(state).is_empty() && state.settings.requirements_open,
        state.settings.settings_open,
    )
}
