//! The Graphs window (C7-21, D-3): the eframe 0.27.2 native
//! multi-viewport spawn — one deferred child OS window sharing the
//! app's egui context + event loop (no second eframe lifecycle,
//! Wayland-safe), re-registered by the root every frame while open
//! (the keep-alive; egui GCs a child the first frame the root stops
//! registering it, which is the close).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use ramsleuth_gui::{build_style, render_graphs_window, TelemetryData};

/// The Graphs child viewport's stable id (C7-21, D-3): the same id
/// on every re-registration, so `show_viewport_deferred` patches
/// the existing registration in place (no duplicate window, no
/// title-bar reshuffle) and the keep-alive / GC mechanics key off
/// one identity. A function, not a `const` — `egui::Id::new` is
/// not const in the pinned egui 0.27.2.
pub fn graphs_viewport_id() -> egui::ViewportId {
    egui::ViewportId(egui::Id::new("ramsleuth_graphs_window"))
}

/// The header's `Graphs` button (C7-21, D-3): `selected` while the
/// window is open (the shared flag read live each frame), and a
/// click toggles the flag — `false` → the window spawns on the next
/// frame, `true` → the root stops re-registering the viewport, so
/// egui GCs the child and the OS window closes (D-C7: the button's
/// close and the WM's close button are behaviorally identical).
pub fn graphs_button(ui: &mut egui::Ui, graphs_open: &Arc<AtomicBool>) -> egui::Response {
    let open = graphs_open.load(Ordering::Relaxed);
    let response = ui.add(egui::Button::new(egui::RichText::new("Graphs")).selected(open));
    if response.clicked() {
        graphs_open.store(!open, Ordering::Relaxed);
    }
    response
}

/// Register (or re-register) the Graphs deferred child viewport
/// (C7-21, D-3) — the keep-alive: the app calls this every frame
/// while `graphs_open` is set; the same builder + id each time
/// patches the existing registration in place (idempotent), and a
/// frame without a call lets egui GC the child (the close). The
/// child's body ([`run_graphs_child_frame`]) runs on its own native
/// window's frame with its own viewport input over this one shared
/// context — no second eframe lifecycle, no second event loop (the
/// Wayland-safe design, D-3). The viewport also carries the
/// window icon (C21-26 — the same decode as the root).
pub fn show_graphs_viewport(
    ctx: &egui::Context,
    state: &Arc<RwLock<TelemetryData>>,
    graphs_open: &Arc<AtomicBool>,
    icon: &Option<Arc<egui::IconData>>,
) {
    let shared = Arc::clone(state);
    let flag = Arc::clone(graphs_open);
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("RamSleuth — Graphs")
        .with_inner_size(crate::GRAPHS_WINDOW_SIZE)
        .with_min_inner_size(crate::GRAPHS_WINDOW_MIN_SIZE)
        .with_app_id("RamSleuth");
    // The same decoded icon as the root (C21-26 — one decode at
    // startup); `None` (a decode failure) stays legal — eframe's
    // default icon (no-panic, D5).
    viewport.icon = icon.clone();
    ctx.show_viewport_deferred(graphs_viewport_id(), viewport, move |child_ctx, _class| {
        run_graphs_child_frame(child_ctx, &shared, &flag);
    });
}

/// The Graphs child viewport's frame body (C7-21, D-3): one
/// idempotent style set over the shared context (the main window's
/// `AppCreator` already set it — this keeps a fresh context correct
/// too), one brief write of the shared state for the render — the
/// C9-03 `Poll` combo's write of the shared
/// `settings.poll_interval_ms` knob (the settings-panel D-2 / D6
/// write precedent: the render thread's one permitted write, no
/// I/O; the poller stays the only writer of the data fields) — the
/// window's render, the child's own ~60 FPS cadence, and the WM
/// close path — a `close_requested()` clears the shared flag so
/// the root stops re-registering and egui GCs the window (the
/// button's close is the same path, D-C7).
pub fn run_graphs_child_frame(
    ctx: &egui::Context,
    state: &Arc<RwLock<TelemetryData>>,
    graphs_open: &Arc<AtomicBool>,
) {
    ctx.set_style(build_style());
    {
        // The C9-03 co-land: a write lock (the settings-panel D6
        // write precedent — the `Poll` combo writes the shared
        // `settings.poll_interval_ms` knob the poller re-reads
        // live; the data fields stay poller-written). The knob is
        // copied out (a `u64`) and written back: a shared borrow of
        // `data.graph` and a mutable borrow of the knob through the
        // guard's deref cannot coexist in one expression.
        let mut data = crate::state_write(state);
        let mut poll_interval_ms = data.settings.poll_interval_ms;
        render_graphs_window(ctx, &data.graph, &mut poll_interval_ms);
        data.settings.poll_interval_ms = poll_interval_ms;
    }
    ctx.request_repaint_after(crate::REPAINT);
    if ctx.input(|i| i.viewport().close_requested()) {
        graphs_open.store(false, Ordering::Relaxed);
    }
}

/// The Graphs window's telemetry lifecycle transition (C9-02, D-2):
/// the pure edge logic over the detector's memory — the app's
/// [`RamSleuthApp::apply_graphs_lifecycle`] runs it on each edge
/// under a brief write lock. The opening edge (closed → open) saves
/// the current refresh gate and forces it on (the window's live
/// streaming runs regardless of the auto-refresh knob); the closing
/// edge (open → closed) restores the saved pre-open value and
/// clears it. Both close paths (the header button toggle + the
/// child's WM `close_requested`) clear the same shared flag, so one
/// falling edge covers both (D-C7). A no-edge frame writes nothing.
/// Returns the `refresh_enabled` to write, or `None` for no change.
pub fn apply_refresh_transition(
    saved: &mut Option<bool>,
    last: &mut bool,
    now_open: bool,
    current: bool,
) -> Option<bool> {
    let change = if now_open && !*last {
        // Opening edge: save the original, then force on.
        *saved = Some(current);
        Some(true)
    } else if !now_open && *last {
        // Closing edge: restore the saved original (`None` — the
        // defensive no-op when nothing was saved).
        saved.take()
    } else {
        // No edge: nothing changes.
        None
    };
    *last = now_open;
    change
}
