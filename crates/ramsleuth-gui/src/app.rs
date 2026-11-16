//! The eframe app (P3-30): the shared state, the poller's command
//! channel, the stop / cancel flags, the settings-panel visibility,
//! the one-click setup outcome (C21-06), the window icon (C21-26),
//! the export dir, the poller thread, and the transient header
//! notice.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use std::time::Instant;

use ramsleuth_gui::first_run::{
    render_requirements_strip_with_setup, requirements_strip_visible, setup_with_dkms, SetupOutcome,
};
use ramsleuth_gui::{
    diagnose, render_bench_zone, render_settings_panel, render_status_zone, render_telemetry_zone,
    BenchCmd, GuiAction, ProbeResult, TelemetryData, CYAN, SLATE,
};
use ramsleuth_telemetry::probe::render_probe_report_md;

/// The eframe app (P3-30): the shared state, the poller's command
/// channel, the stop / cancel flags, the settings-panel visibility,
/// the one-click setup outcome (C21-06), the window icon (C21-26),
/// the export dir, the poller thread, and the transient header
/// notice.
pub struct RamSleuthApp {
    /// The shared presentation state — includes the in-memory
    /// `GuiSettings` knobs (C6-26 / C6-27 / C6-30: the poll cadence +
    /// refresh gate + the daemon socket live here; the poller
    /// re-reads them every tick, and the settings panel is the render
    /// thread's one permitted write: no I/O, D6). The background
    /// poller is the only writer of the data fields; the render loop
    /// only ever takes a brief read.
    pub state: Arc<RwLock<TelemetryData>>,
    /// The poller's benchmark channel (the bench zone's run buttons
    /// send into it; the poller owns the socket).
    pub bench_tx: Sender<BenchCmd>,
    /// The shared stop flag (the poller exits when it is set).
    pub stop: Arc<AtomicBool>,
    /// The shared benchmark cancel flag (the bench zone's Cancel button
    /// sets it; the poller's run checks it between frames).
    pub cancel: Arc<AtomicBool>,
    /// Whether the settings panel (the header's `Settings` toggle,
    /// C6-30) is open: while open it renders as its own top strip
    /// below the header and mutates `state.settings` (the render
    /// thread's one permitted write — no I/O, D6).
    pub settings_open: bool,
    /// Whether the SETUP requirements strip (C18, D-18.5) is open:
    /// auto-shown at first launch while a requirement is present
    /// (initialized `true` — the strip renders only while [`diagnose`]
    /// reports one); the header's `Setup` toggle and the strip's
    /// "Got it" both write it — the render thread's one-permitted
    /// write class, no I/O, D6.
    pub requirements_open: bool,
    /// The one-click setup outcome (C21-06): shared between the
    /// render thread (the wizard button's one permitted write —
    /// flipping `running`, no I/O, D6) and the detached `pkexec`
    /// worker (which clears `running` and sets `done` / `failure` —
    /// a brief per-mutation lock hand-off, never held across the
    /// spawn, C14-03).
    pub setup: Arc<RwLock<SetupOutcome>>,
    /// The window-embed icon (C21-26): the C21-25 master decoded
    /// once at startup — the root viewport + the Graphs child share
    /// it (a decode failure degrades to `None`, no-panic, D5).
    pub icon: Option<Arc<egui::IconData>>,
    /// Whether the Graphs window (C7-21, D-3 — the eframe deferred
    /// child viewport) is open: while set, `update` re-registers
    /// the viewport every frame (the keep-alive — egui GCs a child
    /// the first frame the root stops registering it, which is the
    /// close). The header's `Graphs` button (a render-thread click
    /// — no I/O, D6) and the child's own WM close button both
    /// write this shared flag (D-C7: the two close paths are
    /// behaviorally identical).
    pub graphs_open: Arc<AtomicBool>,
    /// The pre-open `settings.refresh_enabled` saved by the Graphs
    /// telemetry lifecycle detector (C9-02, D-2): `Some` while the
    /// window is open (the opening edge force-enabled the gate),
    /// restored + cleared on the closing edge — either close path
    /// clears the same flag, D-C7.
    pub saved_refresh: Option<bool>,
    /// The previous frame's `graphs_open` value: the C9-02 edge
    /// detector's memory (rising = the window opened, falling = it
    /// closed).
    pub last_graphs_open: bool,
    /// The export destination dir (F2 / F3 write here — `$HOME`).
    pub out_dir: PathBuf,
    /// The background poller thread (joined — bounded — on drop so the
    /// process exits cleanly).
    pub poller: Option<JoinHandle<()>>,
    /// The transient header notice: the last F2 / F3 result text +
    /// when it was set (it fades after [`crate::NOTICE_TTL`]).
    pub notice: Option<(String, Instant)>,
    /// The consent-gated "Submit Probe Report" flow state (chunk
    /// probe-3): `Idle` → (the header's Probe button) `Consent` →
    /// (the poller lands the report) `Preview(md)` → (any closing
    /// path: [Cancel] / [Copy to Clipboard] / [Open GitHub Issue] /
    /// Esc) `Idle` — no terminal state, the flow is re-openable.
    pub probe_state: crate::probe_ui::ProbeState,
    /// The pre-filled GitHub issue title for the current probe report
    /// (chunk probe-3): `[Probe] <cpu_brand> · <cpu_gen> · <os> ·
    /// v<version>` — set alongside the `Preview` transition, used to
    /// build the `issues/new` URL.
    pub probe_title: Option<String>,
    /// Whether the About/Help modal (chunk gui-about) is open: the
    /// header's `About` button opens it (a render-thread write — no
    /// I/O, D6); the dialog's [Close] + the modal's Esc dismiss
    /// close it — every closing path returns to the dashboard (a
    /// plain bool, no terminal state).
    pub about_open: bool,
}

impl Drop for RamSleuthApp {
    fn drop(&mut self) {
        // The window is gone (Q, the OS close button, or an eframe
        // failure): stop the poller and join it (bounded) — the render
        // thread never leaves a dangling daemon connection.
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.poller.take() {
            let deadline = Instant::now() + crate::POLLER_JOIN_DEADLINE;
            while !handle.is_finished() {
                if Instant::now() >= deadline {
                    // The poller is still draining a benchmark frame;
                    // the process is exiting — detach it (the TUI
                    // P3-24 bounded-join precedent).
                    eprintln!(
                        "ramsleuth: the poller did not finish within {:?}; detaching",
                        crate::POLLER_JOIN_DEADLINE
                    );
                    break;
                }
                std::thread::sleep(crate::JOIN_POLL);
            }
            if handle.is_finished() {
                let _ = handle.join();
            }
        }
    }
}

impl eframe::App for RamSleuthApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // No render-thread I/O (plan D6): one brief read of the shared
        // state (the poller is its only writer) drives the header +
        // the three zones; the F2 / F3 side effect is one file write.
        // Fade the transient notice once it has lived its TTL.
        let expired = self
            .notice
            .as_ref()
            .is_some_and(|(_, set_at)| set_at.elapsed() > crate::NOTICE_TTL);
        if expired {
            self.notice = None;
        }
        // The probe flow (chunk probe-3): consume the poller's
        // probe-report result (the consent dialog's [Allow] edge sets
        // `probe_requested`; the poller does the daemon I/O and lands
        // `probe_result`) — the consent → preview transition.
        self.advance_probe();
        // The probe-flow dialog's open state (chunk probe-3): the
        // consent + preview modals block the key legend + the
        // button dispatch — their full-screen layer consumes the
        // pointer over the rest of the UI. (The C21-36 post-setup
        // restart prompt is gone — a successful setup auto-reconnects:
        // the worker's `reconnect_requested` edge forces the
        // poller's immediate poll, and the strip unmounts on its own
        // once the daemon serves.)
        let probe_open = matches!(
            self.probe_state,
            crate::probe_ui::ProbeState::Consent | crate::probe_ui::ProbeState::Preview(_)
        );
        let modal_open = probe_open || self.about_open;
        // The keyboard (C6-30, the spec's key legend): a fresh
        // key-down — egui marks OS key-repeats `repeat: true`, so a
        // held key fires exactly once, the button's click semantics —
        // routes through the same [`GuiAction`] dispatch as the
        // status zone's buttons (D-C7: keys and buttons are
        // behaviorally identical).
        let keyed_action = if modal_open {
            // The modal dialog blocks the key legend (F2 / F3 / Q)
            // while open.
            None
        } else {
            ctx.input(|i| {
                i.events
                    .iter()
                    .filter_map(|event| match event {
                        egui::Event::Key {
                            key,
                            pressed: true,
                            repeat: false,
                            ..
                        } => crate::key_action(*key),
                        _ => None,
                    })
                    .next()
            })
        };
        // The top strips allocate in a fixed per-frame order (header,
        // requirements while one is present, settings while open,
        // then the central panel): each takes its own brief lock
        // (the render thread does no I/O, D6 — the settings strip is
        // its one permitted write).
        let header_action = {
            let data = crate::state_read(&self.state);
            Self::render_header(
                ctx,
                &data,
                &mut self.settings_open,
                &mut self.requirements_open,
                &self.graphs_open,
                &self.notice,
            )
        };
        // The SETUP requirements strip (C18, D-18.5) — the liveness
        // trigger: [`requirements_strip_visible`] allocates it
        // between the header and the settings strip while the
        // header's `Setup` toggle is open AND a requirement is
        // present (case 1 = the daemon not serving: never polled,
        // down, or a groupless client's refused connect), so a
        // daemon-less launch shows the one-click setup prompt and the
        // strip disappears on its own once the daemon serves — even
        // with all-`N/A` telemetry (and it can be re-opened any time;
        // the strip's "Got it" + the toggle write the same flag, D-C7).
        let requirements_present = {
            let data = crate::state_read(&self.state);
            requirements_strip_visible(self.requirements_open, &data)
        };
        if requirements_present {
            self.render_requirements_area(ctx);
        }
        // The one-click setup worker (C21-06): the wizard button's
        // click (the render thread's one permitted write, D6) flips
        // `setup.running`; this per-tick hand-off consumes the edge
        // (a brief lock, C14-03 — never held across the spawn) and
        // detaches the `pkexec` worker.
        self.maybe_spawn_setup_worker();
        if self.settings_open {
            self.render_settings_area(ctx);
        }
        let button_action = {
            let data = crate::state_read(&self.state);
            self.render_zones(ctx, &data)
        };
        // One dispatch path for both sources: the button (the
        // explicit affordance) runs first, then a distinct key
        // action; a duplicate (button + key, same action) fires once.
        // While the C21-36 modal prompt is open, no dispatch — the
        // dialog's full-screen layer has consumed the pointer and
        // the key legend is gated above: the dialog's two buttons
        // are the only interactive content.
        if !modal_open {
            // The header's "Probe" button (chunk probe-3): open the
            // consent flow (the state machine guards the Idle source —
            // a click while a dialog is open is unreachable, the
            // modal block covers the header).
            if header_action == crate::probe_ui::HeaderAction::Probe {
                self.probe_state.apply(crate::probe_ui::ProbeEvent::Open);
            }
            // The header's "About" button (chunk gui-about): open
            // the About/Help modal (a plain flag — no state machine;
            // the modal block covers the header while it is open).
            if header_action == crate::probe_ui::HeaderAction::About {
                self.about_open = true;
            }
            self.handle_action(ctx, button_action);
            if let Some(keyed_action) = keyed_action {
                if keyed_action != button_action {
                    self.handle_action(ctx, keyed_action);
                }
            }
        }

        // The modals' Esc dismiss (the TUI's close contract): a
        // fresh, non-repeated Esc key-down closes the topmost open
        // dialog — the probe modal renders last (topmost), so it
        // takes the Esc over the setup prompt + the About modal;
        // with no probe dialog open, the About modal's Esc returns
        // to the dashboard. Every closing path resets (the buttons
        // are re-openable); the modals block the key legend above.
        let esc_down = ctx.input(|i| {
            i.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::Key {
                        key: egui::Key::Escape,
                        pressed: true,
                        repeat: false,
                        ..
                    }
                )
            })
        });
        if probe_open && esc_down {
            self.handle_probe_esc();
        } else if self.about_open && esc_down {
            self.about_open = false;
        }

        // The Graphs window's telemetry lifecycle (C9-02, D-2): the
        // per-frame transition detector — force-on on the opening
        // edge, restore on the closing edge (both close paths clear
        // the same flag, D-C7) — before the keep-alive re-
        // registration below.
        self.apply_graphs_lifecycle();

        // The Graphs window (C7-21, D-3): while open, re-register
        // the deferred child viewport every frame — the keep-alive
        // (egui GCs the child the first frame the root stops
        // registering it, which is how both close paths destroy the
        // OS window).
        if self.graphs_open.load(Ordering::Relaxed) {
            crate::graphs_viewport::show_graphs_viewport(
                ctx,
                &self.state,
                &self.graphs_open,
                &self.icon,
            );
        }

        // The About/Help modal (chunk gui-about): allocated before
        // the probe modal (the probe modal renders last — topmost —
        // and takes the Esc over it).
        if self.about_open {
            self.render_about_dialog(ctx);
        }
        // The probe-flow modal (chunk probe-3): allocated last
        // (topmost) — the consent dialog, then the preview dialog,
        // matching the current `probe_state`.
        if probe_open {
            match &self.probe_state {
                crate::probe_ui::ProbeState::Consent => self.render_probe_consent_dialog(ctx),
                crate::probe_ui::ProbeState::Preview(_) => self.render_probe_preview_dialog(ctx),
                _ => {}
            }
        }

        // ~60 FPS: eframe's vsync drives the present; this only asks
        // for the next frame on the 16 ms cadence.
        ctx.request_repaint_after(crate::REPAINT);
    }
}

impl RamSleuthApp {
    /// The header strip (Grand Design §3.1): the spec's 3-line
    /// header — line 1 a left anchor (the `RamSleuth v<version>`
    /// title, the platform badge — the CPU vendor's short pill,
    /// the compact daemon-status dot + `Daemon OK` / `Daemon`
    /// label, the live settings socket on the label's hover
    /// tooltip — C6-30) and a right-anchored button cluster (the
    /// `About`, `Probe`, `Setup`, `Settings`, and `Graphs`
    /// toggles — the `About` modal chunk gui-about; the latter
    /// C7-21, D-3; the `Setup` toggle C18, D-18.5); line 2
    /// the CPU + platform identity; line 3 the RAM summary
    /// (capacity / breakdown / speed / channel / ECC) and sync mode
    /// (the capacity + clock segments render in the live `units`
    /// knob's units — C7-11) — plus
    /// the transient notice line (the last F2 / F3 result) while
    /// one is showing. No telemetry yet (never polled) → the
    /// placeholder lines (a missing daemon never crashes the GUI,
    /// plan D5).
    ///
    /// An associated function (no `self`): it needs only the
    /// snapshot + the `settings_open` toggle (the `Settings` button
    /// flips it, C6-30) + the `requirements_open` toggle (the
    /// `Setup` button flips it, C18) + the shared `graphs_open`
    /// flag (the `Graphs` button toggles it, C7-21) + the transient
    /// notice (the last F2 / F3 result line) — so the call site can
    /// hold the state's read guard and the `settings_open` /
    /// `requirements_open` / `graphs_open` / `notice` field borrows
    /// at once (the field split the borrow checker enforces).
    pub(crate) fn render_header(
        ctx: &egui::Context,
        data: &TelemetryData,
        settings_open: &mut bool,
        requirements_open: &mut bool,
        graphs_open: &Arc<AtomicBool>,
        notice: &Option<(String, Instant)>,
    ) -> crate::probe_ui::HeaderAction {
        // The line builders over the current snapshot (the capacity +
        // clock segments honor the live `units` knob — C7-11) — or
        // the placeholders when no poll has landed yet (never a panic).
        let (cpu_line, ram_prefix, ram_mode, ram_mode_color) = match &data.telemetry {
            Some(t) => {
                let units = &data.settings.units;
                let (mode, color) =
                    crate::text::intel_mode(t).unwrap_or_else(|| crate::text::sync_mode(t, units));
                (
                    crate::text::cpu_line_text(t, units),
                    crate::text::ram_line_prefix(t, units),
                    mode,
                    color,
                )
            }
            None => (
                "CPU: —".to_owned(),
                "RAM: —".to_owned(),
                String::new(),
                None,
            ),
        };
        // Line 1's left anchor: the badge is the CPU vendor's short
        // word (`Unknown` when no poll has landed yet); the daemon
        // status is the dot + label pair. The live settings socket
        // (C6-30) — the panel's `Socket` field, seeded from the CLI
        // `--socket` at startup and read live by the poller — rides
        // the label's hover tooltip, built in place below.
        let badge = data
            .telemetry
            .as_ref()
            .map(|t| crate::text::platform_badge(&t.cpu.vendor))
            .unwrap_or("Unknown");
        let (dot, daemon_label) = crate::text::daemon_status_indicator(data);
        // The "Probe" (chunk probe-3) and "About" (chunk gui-about)
        // buttons' click edges: captured in the closure, reported as
        // the function's return value.
        let mut probe_clicked = false;
        let mut about_clicked = false;
        egui::TopBottomPanel::top("ramsleuth_header")
            .frame(
                egui::Frame::default()
                    .fill(SLATE)
                    .stroke(egui::Stroke::new(1.0_f32, crate::HEADER_STROKE)),
            )
            .show(ctx, |ui| {
                // Line 1: the left anchor (the title, the platform
                // badge, the compact daemon status) + the
                // right-anchored button cluster — the two ends of
                // the row never meet at the 968 px default (the
                // former verbose daemon string + the hotkey legend
                // collided there).
                ui.horizontal(|ui| {
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new(format!("RamSleuth v{}", env!("CARGO_PKG_VERSION")))
                            .strong()
                            .color(CYAN)
                            .size(20.0),
                    );
                    ui.separator();
                    // The platform badge: the vendor's short word
                    // in a small muted pill.
                    egui::Frame::default()
                        .fill(crate::BADGE_BG)
                        .rounding(4.0)
                        .inner_margin(egui::Margin::symmetric(6.0, 2.0))
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new(badge).small().weak());
                        });
                    ui.separator();
                    // The compact daemon status: the colored dot +
                    // the short label — the full socket path rides
                    // the hover tooltip.
                    let (dot_rect, _) =
                        ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                    ui.painter()
                        .circle(dot_rect.center(), 6.0, dot, egui::Stroke::NONE);
                    ui.label(daemon_label)
                        .on_hover_text(format!("IPC: {}", data.settings.socket));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(10.0);
                        // The Graphs window (C7-21, D-3): the
                        // dedicated second OS window — rightmost,
                        // next to Settings; selected while open, a
                        // click toggles the shared open flag (spawn
                        // or close, D-C7).
                        crate::graphs_viewport::graphs_button(ui, graphs_open);
                        ui.add_space(8.0);
                        // The settings toggle (C6-30): opens the
                        // settings panel strip below the header.
                        if ui
                            .add(
                                egui::Button::new(egui::RichText::new("Settings"))
                                    .selected(*settings_open),
                            )
                            .clicked()
                        {
                            *settings_open = !*settings_open;
                        }
                        ui.add_space(8.0);
                        // The Setup toggle (C18, D-18.5): left of
                        // Settings in the right-to-left cluster —
                        // selected while the requirements strip is
                        // open; a click flips the local flag (the
                        // strip's "Got it" writes the same flag —
                        // D-C7: the two close paths are
                        // behaviorally identical).
                        if ui
                            .add(
                                egui::Button::new(egui::RichText::new("Setup"))
                                    .selected(*requirements_open),
                            )
                            .clicked()
                        {
                            *requirements_open = !*requirements_open;
                        }
                        ui.add_space(8.0);
                        // The Probe button (chunk probe-3): the
                        // consent-gated "Submit Probe Report" flow — a
                        // click sets the captured edge (reported as
                        // `HeaderAction::Probe`; the app shell opens
                        // the consent dialog).
                        if ui
                            .add(egui::Button::new(egui::RichText::new("Probe")))
                            .clicked()
                        {
                            probe_clicked = true;
                        }
                        ui.add_space(8.0);
                        // The About button (chunk gui-about): the
                        // leftmost of the right-anchored cluster —
                        // a click sets the captured edge (reported
                        // as `HeaderAction::About`; the app shell
                        // opens the About/Help modal).
                        if ui
                            .add(egui::Button::new(egui::RichText::new("About")))
                            .clicked()
                        {
                            about_clicked = true;
                        }
                    });
                });
                // Line 2: the CPU + platform identity.
                ui.add_space(2.0);
                ui.label(egui::RichText::new(&cpu_line));
                // Line 3: the RAM summary + channel + the mode segment
                // (AMBER 1:1, CRIMSON 1:2, the default color for N/A).
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(&ram_prefix));
                    if !ram_mode.is_empty() {
                        match ram_mode_color {
                            Some(color) => ui.label(egui::RichText::new(&ram_mode).color(color)),
                            None => ui.label(egui::RichText::new(&ram_mode)),
                        };
                    }
                });
                if let Some((text, _)) = notice {
                    ui.add_space(2.0);
                    ui.label(egui::RichText::new(text).color(crate::text::notice_color(text)));
                }
            });
        if probe_clicked {
            crate::probe_ui::HeaderAction::Probe
        } else if about_clicked {
            crate::probe_ui::HeaderAction::About
        } else {
            crate::probe_ui::HeaderAction::None
        }
    }

    /// The three zones: the telemetry matrix (zone 1) on the left; the
    /// benchmark grid + controls (zone 2) stacked over the hardware /
    /// SPD status (zone 3) on the right as two slices (C9-05, D-4b:
    /// the bench at its natural height, the status at the remaining
    /// column height, the 8 pt gap preserved) — the columns take the
    /// central panel's full height (C7-19 removed the embedded
    /// 10-minute trend history strip; the trend data now lives in
    /// the Graphs window, C7-21) — all visible, non-scrolling at the
    /// 968×600 size (in a small window the right column's slices
    /// degrade to a scroll area — the overflow fallback). Returns the
    /// [`GuiAction`] the status zone reported this frame (`None` when
    /// no button was clicked).
    pub(crate) fn render_zones(&self, ctx: &egui::Context, data: &TelemetryData) -> GuiAction {
        let mut action = GuiAction::None;
        let bench_tx = &self.bench_tx;
        let cancel = &self.cancel;
        egui::CentralPanel::default()
            // FE-5: the panel's inner margin carries the symmetric
            // outer margin (the hand-rolled `add_space(OUTER_MARGIN)`
            // bookkeeping is gone) — the `ui.columns` primitive below
            // splits the margin-reduced width into the two zone
            // columns. A pathological window collapses the content to
            // zero (never a negative allocation, never a panic).
            .frame(
                egui::Frame::default()
                    .fill(SLATE)
                    .inner_margin(crate::OUTER_MARGIN),
            )
            .show(ctx, |ui| {
                // The style's item_spacing, captured before the column
                // gap is set below; each column's content restores it —
                // the zones' grids read `item_spacing.x` for their inner
                // gaps (D-15.1).
                let item_spacing = ui.spacing().item_spacing;
                // FE-5: the two zone columns via egui's layout primitive
                // (`ui.columns`) — the hand-rolled 55% width arithmetic
                // is gone. The gap between the columns is `COLUMN_GAP`
                // (the style's `item_spacing.x`, restored inside each
                // column so the zones keep their inner gaps).
                ui.style_mut().spacing.item_spacing = egui::vec2(crate::COLUMN_GAP, item_spacing.y);
                // `ui.columns` hands the closure a `&mut [Ui]` — one
                // column per element (egui 0.27), so `columns[0]` is the
                // left (telemetry) zone and `columns[1]` the right
                // (bench + status) zone. Each column restores the style's
                // `item_spacing` (the zones read `item_spacing.x` for
                // their inner gaps).
                ui.columns(2, |columns: &mut [egui::Ui]| {
                    // Left: zone 1 (the live timing matrix, its own
                    // bounded scroll area).
                    {
                        let ui = &mut columns[0];
                        ui.style_mut().spacing.item_spacing = item_spacing;
                        render_telemetry_zone(ui, data);
                    }
                    // Right: zone 2 over zone 3 (C9-05, D-4b). The
                    // `ScrollArea` is the genuine overflow fallback
                    // (a small window scrolls, it never clips): inside it
                    // the bench sizes **naturally** (FE-5 — no zero-height
                    // `allocate_ui_with_layout` read-back footgun) and the
                    // status fills the **constrained remainder** via a
                    // bounded `Frame` (not an arithmetic `row_h − bench_h
                    // − 8` slice).
                    {
                        let ui = &mut columns[1];
                        ui.style_mut().spacing.item_spacing = item_spacing;
                        let _ = egui::ScrollArea::vertical().show(ui, |ui| {
                            // The bench at its natural height — it
                            // renders directly in the top-down column and
                            // advances the layout by its drawn `min_rect`
                            // (a non-zero, content-sized allocation).
                            render_bench_zone(ui, data, bench_tx, cancel);
                            ui.add_space(8.0);
                            // The status fills the constrained remainder:
                            // a `Frame` bounded to the column's available
                            // size after the bench (egui's
                            // `available_size` — the layout primitive, not
                            // a manual subtraction). Zero budget when the
                            // bench alone overflows (the scroll fallback
                            // above).
                            let remaining = ui.available_size();
                            let _ = egui::Frame::default().show(ui, |ui| {
                                ui.set_max_size(egui::vec2(
                                    remaining.x.max(0.0),
                                    remaining.y.max(0.0),
                                ));
                                action = render_status_zone(ui, data);
                            });
                        });
                    }
                });
            });
        action
    }

    /// The settings strip (C6-30): shown below the header while
    /// `settings_open` — its own top panel (the per-frame allocation
    /// order: header, settings, central). The grid mutates
    /// `TelemetryData.settings` under the write lock — the render
    /// thread's one permitted write (no I/O, D6); the poller re-reads
    /// the knobs live every tick (C6-27), including the socket
    /// (C6-30), so a panel change takes effect without a restart.
    fn render_settings_area(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("ramsleuth_settings")
            .frame(
                egui::Frame::default()
                    .fill(SLATE)
                    .stroke(egui::Stroke::new(1.0_f32, crate::HEADER_STROKE)),
            )
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.label(egui::RichText::new("Settings").strong().color(CYAN));
                ui.add_space(2.0);
                // The one permitted render-thread write (D6): no I/O.
                let mut guard = crate::state_write(&self.state);
                render_settings_panel(ui, &mut guard.settings);
            });
    }

    /// The SETUP requirements strip (C18, D-18.5 + the C21-06
    /// one-click setup wizard): shown below the header (above the
    /// settings strip while both are open — the per-frame
    /// allocation order: header, requirements, settings, central)
    /// while `requirements_open` — presence-driven: the caller
    /// allocates it only while [`diagnose`] reports a requirement,
    /// so it disappears on its own once the daemon connects / the
    /// driver loads (and the header's `Setup` toggle re-opens it
    /// any time). The strip's "Got it" writes `requirements_open`
    /// (the render thread's one-permitted-write class, no I/O,
    /// D6); its `Copy` buttons touch only the clipboard (D-18.5,
    /// risk (a)); the wizard's `Set up RamSleuth` click flips
    /// `setup.running` (the detached worker consumes the edge per
    /// tick, C21-06). No-panic: a daemon-less snapshot yields at
    /// most the single daemon requirement (plan D5).
    fn render_requirements_area(&mut self, ctx: &egui::Context) {
        // One brief read snapshot (no I/O, D6) drives the strip.
        let requirements = {
            let data = crate::state_read(&self.state);
            diagnose(&data)
        };
        egui::TopBottomPanel::top("ramsleuth_requirements")
            .frame(
                egui::Frame::default()
                    .fill(SLATE)
                    .stroke(egui::Stroke::new(1.0_f32, crate::HEADER_STROKE)),
            )
            .show(ctx, |ui| {
                // A brief per-frame write (the C14-03 pattern — the
                // guard drops at the end of the closure, never held
                // across I/O): the wizard's click flips `running`,
                // the worker's terminal state is painted here.
                let mut setup = crate::state_write(&self.setup);
                render_requirements_strip_with_setup(
                    ui,
                    &requirements,
                    &mut self.requirements_open,
                    &mut setup,
                );
            });
    }

    /// The one-click setup worker's per-tick hand-off (C21-06): the
    /// wizard button's click (the render thread's one permitted
    /// write — no I/O, D6) flips `setup.running`; when this tick
    /// observes the edge, a brief write-lock block (released before
    /// the spawn — never held across it, C14-03) consumes it,
    /// gathers the worker's inputs (the `--with-dkms` decision from
    /// the host's CPU vendor — the unified one-click action, NOT the
    /// diagnosed requirement list — + the current user, explicit
    /// under `pkexec` — `SUDO_USER` may be unset, the C21-01
    /// contract), and detaches [`spawn_setup_worker`]. An
    /// unresolvable user degrades to the `failure` state with the
    /// manual `sudo` pointer (no-panic, plan D5 / risk 1).
    fn maybe_spawn_setup_worker(&mut self) {
        // The common case: no edge — one brief read, no spawn.
        if !crate::state_read(&self.setup).running {
            return;
        }
        // Consume the edge + gather the inputs (a brief write, the
        // C14-03 pattern — released before the spawn below). The
        // `--with-dkms` decision is the host-vendor one (the same
        // state snapshot the strip renders from): the single unified
        // action carries the driver on AMD/Intel silicon from the
        // very first launch — no second stage.
        let (with_dkms, user) = {
            let mut setup = crate::state_write(&self.setup);
            setup.running = false;
            let data = crate::state_read(&self.state);
            (setup_with_dkms(&data), crate::setup_worker::current_user())
        };
        match user {
            Some(user) => crate::setup_worker::spawn_setup_worker(
                Arc::clone(&self.setup),
                Arc::clone(&self.state),
                with_dkms,
                user,
            ),
            None => {
                // The mandatory `--user` value is unresolvable here:
                // degrade to the manual floor (plan risk 1).
                let mut setup = crate::state_write(&self.setup);
                setup.done = false;
                setup.failure = Some(crate::setup_worker::with_setup_log_pointer(
                    "cannot resolve the current user for the setup helper — run \
                     `sudo ramsleuth-setup --user <you>` in a terminal",
                ));
            }
        }
    }

    /// Execute the status zone's [`GuiAction`] — the one side effect
    /// the render loop performs: F2 / F3 run [`perform_export`] (a
    /// one-shot file write into `out_dir`) and flash the result as the
    /// transient notice; Q sets the shared stop flag and closes the
    /// viewport (the poller is joined on app drop).
    fn handle_action(&mut self, ctx: &egui::Context, action: GuiAction) {
        match action {
            GuiAction::SnapshotPng | GuiAction::ExportJson => {
                let data = crate::state_read(&self.state);
                let text = match crate::perform_export(action, &data, &self.out_dir) {
                    Ok(Some(path)) => format!("wrote {}", path.display()),
                    Ok(None) => "nothing to export yet (no data)".to_owned(),
                    Err(error) => format!("! {error}"),
                };
                self.notice = Some((text, Instant::now()));
            }
            GuiAction::Quit => {
                self.stop.store(true, Ordering::Relaxed);
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            GuiAction::None => {}
        }
    }

    /// The Graphs window's telemetry lifecycle (C9-02, D-2): run
    /// once per frame from `update` — the single `graphs_open`
    /// transition detector. The opening edge saves the current
    /// refresh gate and forces it on (the window's live streaming
    /// runs regardless of the auto-refresh knob — the poller
    /// re-reads it every tick, C6-27, so no poller change is
    /// needed); the closing edge restores the saved pre-open value
    /// and clears it. Both close paths (the header button toggle +
    /// the child's WM `close_requested`) clear the same shared
    /// flag, so one falling edge covers both (D-C7) — no per-site
    /// hooks. The settings write is the render thread's one
    /// permitted mutation (no I/O, D6 — the settings-panel
    /// precedent); the brief write lock is taken only on an edge,
    /// never per frame.
    pub(crate) fn apply_graphs_lifecycle(&mut self) {
        let now_open = self.graphs_open.load(Ordering::Relaxed);
        if now_open != self.last_graphs_open {
            let mut guard = crate::state_write(&self.state);
            if let Some(refresh) = crate::graphs_viewport::apply_refresh_transition(
                &mut self.saved_refresh,
                &mut self.last_graphs_open,
                now_open,
                guard.settings.refresh_enabled,
            ) {
                guard.settings.refresh_enabled = refresh;
            }
        }
    }
    /// The consent-gated "Submit Probe Report" flow's state advance
    /// (chunk probe-3): consume the poller's landed `probe_result` —
    /// `Ok(report)` renders it to markdown + the pre-filled title and
    /// moves to `Preview`; `Err(text)` flashes a notice and returns to
    /// `Idle`; `Pending` / `None` (in flight / not yet processed) stay
    /// in `Consent`. A brief read, then a brief write on completion
    /// (the C14-03 pattern — never held across a frame).
    pub(crate) fn advance_probe(&mut self) {
        if !matches!(self.probe_state, crate::probe_ui::ProbeState::Consent) {
            return;
        }
        let result = crate::state_read(&self.state).probe_result.clone();
        match result {
            ProbeResult::Ok(report) => {
                let md = render_probe_report_md(&report);
                self.probe_title = Some(crate::probe_ui::probe_issue_title(&report));
                self.probe_state = crate::probe_ui::ProbeState::Preview(md);
                crate::state_write(&self.state).probe_result = ProbeResult::None;
            }
            ProbeResult::Err(message) => {
                self.probe_state = crate::probe_ui::ProbeState::Idle;
                self.probe_title = None;
                self.notice = Some((format!("! {message}"), Instant::now()));
                crate::state_write(&self.state).probe_result = ProbeResult::None;
            }
            ProbeResult::Pending | ProbeResult::None => {
                // In flight / the poller has not processed the edge
                // yet: stay in `Consent` (the dialog shows progress).
            }
        }
    }

    /// The probe-flow modal's Esc dismiss (the TUI's close contract):
    /// close the open probe dialog — Consent / Preview → Idle (every
    /// closing path resets the flow, so the header's Probe button is
    /// re-openable without a restart). A no-op when no probe dialog
    /// is open.
    pub(crate) fn handle_probe_esc(&mut self) {
        self.probe_state.apply(
            if matches!(self.probe_state, crate::probe_ui::ProbeState::Consent) {
                crate::probe_ui::ProbeEvent::CancelConsent
            } else {
                crate::probe_ui::ProbeEvent::CancelPreview
            },
        );
    }

    /// The consent dialog (chunk probe-3): the same modal pattern
    /// as the About dialog — two topmost
    /// (`Order::Foreground`) areas, the dimmed full-screen block
    /// (allocated first, consumes every pointer event over the
    /// dashboard) + the centered "Submit Probe Report" box (allocated
    /// after — its buttons beat the block). The body is the exact
    /// disclosure of what the report collects + the no-PII note;
    /// [Allow] (the primary CYAN fill) hands the request to the
    /// poller (the render thread's one probe write, no I/O — D6),
    /// [Cancel] / Esc return to `Idle`. While the poller is fetching
    /// (`probe_result == Pending`), a spinner shows and the buttons
    /// stay inert.
    pub(crate) fn render_probe_consent_dialog(&mut self, ctx: &egui::Context) {
        let screen = ctx.screen_rect();
        egui::Area::new(egui::Id::new("ramsleuth_probe_consent_block"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                ui.set_max_size(screen.size());
                ui.painter()
                    .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(96));
                let _ = ui.allocate_exact_size(screen.size(), egui::Sense::click());
            });
        egui::Area::new(egui::Id::new("ramsleuth_probe_consent"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                ui.set_max_size(screen.size());
                ui.with_layout(
                    egui::Layout::from_main_dir_and_cross_align(
                        egui::Direction::TopDown,
                        egui::Align::Center,
                    ),
                    |ui| {
                        let frame = egui::Frame::default()
                            .fill(SLATE)
                            .stroke(egui::Stroke::new(1.0_f32, CYAN))
                            .inner_margin(egui::Margin::symmetric(12.0, 10.0));
                        frame.show(ui, |ui| {
                            ui.label(
                                egui::RichText::new("Submit Probe Report")
                                    .strong()
                                    .color(CYAN),
                            );
                            ui.add_space(4.0);
                            ui.add(
                                egui::Label::new(crate::probe_ui::PROBE_CONSENT_BODY).wrap(true),
                            );
                            ui.add_space(8.0);
                            // The in-flight indicator (the poller is
                            // fetching): a spinner + the disabled
                            // buttons.
                            let in_flight =
                                crate::state_read(&self.state).probe_result == ProbeResult::Pending;
                            ui.horizontal(|ui| {
                                if in_flight {
                                    ui.add(egui::Spinner::new());
                                    ui.label("Gathering report…");
                                }
                                // Primary (the wizard button's
                                // precedent): the CYAN fill + the
                                // SLATE text — the consent edge.
                                if !in_flight
                                    && ui
                                        .add(
                                            egui::Button::new(
                                                egui::RichText::new("Allow").color(SLATE),
                                            )
                                            .fill(CYAN),
                                        )
                                        .clicked()
                                {
                                    // Hand the request to the poller
                                    // (the render thread's one probe
                                    // write, no I/O — D6): the poller
                                    // clears the flag + does the
                                    // daemon I/O, landing
                                    // `probe_result`.
                                    crate::state_write(&self.state).probe_requested = true;
                                }
                                // Secondary (the `Got it` precedent):
                                // plain — return to `Idle`.
                                if !in_flight && ui.add(egui::Button::new("Cancel")).clicked() {
                                    self.probe_state
                                        .apply(crate::probe_ui::ProbeEvent::CancelConsent);
                                }
                            });
                        });
                    },
                );
            });
    }

    /// The preview dialog (chunk probe-3): the same modal pattern —
    /// the dimmed full-screen block + the centered "Probe Report
    /// Preview" box. The body is a scrollable read-only
    /// `TextEdit::multiline` over the rendered markdown;
    /// [Open GitHub Issue] (the primary CYAN fill) copies the full
    /// markdown report to the clipboard, opens the pre-filled
    /// `issues/new` URL (the title only — the body would exceed
    /// GitHub's issue-form limit, the 404) in the OS browser,
    /// flashes the "paste it into the issue body" notice in the
    /// header, + returns the flow to `Idle`; [Copy to Clipboard]
    /// copies the markdown + returns to `Idle`; [Cancel] / Esc
    /// return to `Idle`. Every closing path resets the flow — the
    /// header's Probe button is re-openable without a restart.
    pub(crate) fn render_probe_preview_dialog(&mut self, ctx: &egui::Context) {
        let screen = ctx.screen_rect();
        egui::Area::new(egui::Id::new("ramsleuth_probe_preview_block"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                ui.set_max_size(screen.size());
                ui.painter()
                    .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(96));
                let _ = ui.allocate_exact_size(screen.size(), egui::Sense::click());
            });
        egui::Area::new(egui::Id::new("ramsleuth_probe_preview"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                ui.set_max_size(screen.size());
                ui.with_layout(
                    egui::Layout::from_main_dir_and_cross_align(
                        egui::Direction::TopDown,
                        egui::Align::Center,
                    ),
                    |ui| {
                        // The markdown to show (cloned out of the
                        // state before the mutable button borrows).
                        let mut md = match &self.probe_state {
                            crate::probe_ui::ProbeState::Preview(md) => md.clone(),
                            _ => return,
                        };
                        let frame = egui::Frame::default()
                            .fill(SLATE)
                            .stroke(egui::Stroke::new(1.0_f32, CYAN))
                            .inner_margin(egui::Margin::symmetric(12.0, 10.0));
                        frame.show(ui, |ui| {
                            ui.label(
                                egui::RichText::new("Probe Report Preview")
                                    .strong()
                                    .color(CYAN),
                            );
                            ui.add_space(4.0);
                            // A scrollable read-only multi-line edit
                            // over the rendered markdown (a bounded
                            // box inside the centered frame; the
                            // `desired_width` fills the column).
                            let max_size = egui::vec2(
                                (screen.size().x - 60.0).max(320.0),
                                (screen.size().y * 0.6).max(200.0),
                            );
                            egui::ScrollArea::vertical()
                                .max_width(max_size.x)
                                .max_height(max_size.y)
                                .show(ui, |ui| {
                                    ui.add(
                                        egui::TextEdit::multiline(&mut md)
                                            .interactive(false)
                                            .font(egui::FontId::monospace(12.0))
                                            .desired_width(f32::INFINITY),
                                    );
                                });
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                // Primary (the wizard button's
                                // precedent): the CYAN fill + the
                                // SLATE text — copy the report to
                                // the clipboard, open the
                                // pre-filled (title-only) GitHub
                                // issue, flash the paste notice,
                                // return the flow to `Idle` (the
                                // dialog closes — the Probe button
                                // is re-openable).
                                if ui
                                    .add(
                                        egui::Button::new(
                                            egui::RichText::new("Open GitHub Issue").color(SLATE),
                                        )
                                        .fill(CYAN),
                                    )
                                    .clicked()
                                {
                                    // The full report rides the
                                    // clipboard (a 4–5 KB markdown
                                    // body exceeds GitHub's
                                    // issue-form URL limit — the
                                    // 404); the copy precedes the
                                    // URL open so the text is in
                                    // place before the browser
                                    // takes focus.
                                    ctx.copy_text(md.clone());
                                    self.notice = Some((
                                        crate::probe_ui::PROBE_ISSUE_COPY_NOTICE.to_owned(),
                                        Instant::now(),
                                    ));
                                    if let Some(title) = self.probe_title.clone() {
                                        ctx.open_url(egui::OpenUrl::same_tab(
                                            crate::probe_ui::probe_issue_url(&title),
                                        ));
                                    }
                                    self.probe_state.apply(crate::probe_ui::ProbeEvent::Submit);
                                }
                                // Secondary: copy the markdown +
                                // return to `Idle` (the dialog
                                // closes — the Probe button is
                                // re-openable).
                                if ui.add(egui::Button::new("Copy to Clipboard")).clicked() {
                                    ctx.copy_text(md.clone());
                                    self.probe_state.apply(crate::probe_ui::ProbeEvent::Copy);
                                }
                                // Tertiary (the `Got it` precedent):
                                // plain — return to `Idle`.
                                if ui.add(egui::Button::new("Cancel")).clicked() {
                                    self.probe_state
                                        .apply(crate::probe_ui::ProbeEvent::CancelPreview);
                                }
                            });
                        });
                    },
                );
            });
    }

    /// The About/Help modal (chunk gui-about): the same modal
    /// pattern as the probe consent dialog — two topmost
    /// (`Order::Foreground`) areas, the dimmed full-screen block
    /// (allocated first, consumes every pointer event over the
    /// dashboard) + the centered "About RamSleuth" box (allocated
    /// after — its button beats the block). The body is the
    /// [`about_body`] sections (what the app is, the daemon model,
    /// the capabilities, the Intel/AMD split, the version, the
    /// full-guide pointer) in a bounded scroll area (the probe
    /// preview dialog's sizing convention); [Close] / Esc return
    /// to the dashboard.
    pub(crate) fn render_about_dialog(&mut self, ctx: &egui::Context) {
        let screen = ctx.screen_rect();
        egui::Area::new(egui::Id::new("ramsleuth_about_block"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                ui.set_max_size(screen.size());
                ui.painter()
                    .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(96));
                let _ = ui.allocate_exact_size(screen.size(), egui::Sense::click());
            });
        egui::Area::new(egui::Id::new("ramsleuth_about"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                ui.set_max_size(screen.size());
                ui.with_layout(
                    egui::Layout::from_main_dir_and_cross_align(
                        egui::Direction::TopDown,
                        egui::Align::Center,
                    ),
                    |ui| {
                        let frame = egui::Frame::default()
                            .fill(SLATE)
                            .stroke(egui::Stroke::new(1.0_f32, CYAN))
                            .inner_margin(egui::Margin::symmetric(12.0, 10.0));
                        frame.show(ui, |ui| {
                            ui.label(egui::RichText::new("About RamSleuth").strong().color(CYAN));
                            ui.add_space(4.0);
                            // The body: the section headings + the
                            // wrapped text in a bounded scroll
                            // area (the probe preview dialog's
                            // sizing convention).
                            let max_size = egui::vec2(
                                (screen.size().x - 60.0).max(360.0),
                                (screen.size().y * 0.65).max(240.0),
                            );
                            egui::ScrollArea::vertical()
                                .max_width(max_size.x)
                                .max_height(max_size.y)
                                .show(ui, |ui| {
                                    for (heading, body) in crate::text::about_body() {
                                        ui.label(egui::RichText::new(heading).strong());
                                        ui.add(egui::Label::new(body).wrap(true));
                                        ui.add_space(6.0);
                                    }
                                });
                            ui.add_space(8.0);
                            // Secondary (the `Got it` precedent):
                            // plain — return to the dashboard.
                            if ui.add(egui::Button::new("Close")).clicked() {
                                self.about_open = false;
                            }
                        });
                    },
                );
            });
    }
}
