//! ramsleuth-gui — the eframe app shell (P3-30).
//!
//! This binary is the P3-30 target: the 60 FPS eframe window (1400×900
//! initial viewport, `style::build_style` on the context), the
//! `update::start_updater` background poller behind
//! `Arc<RwLock<TelemetryData>>` (no render-thread I/O, D6), the three
//! Grand Design §3 zones, and the F2 / F3 / Q actions. Until P3-30 it
//! is a placeholder that exits 0.

fn main() {
    // P3-30: the eframe app shell lands here (see `src/lib.rs`).
}
