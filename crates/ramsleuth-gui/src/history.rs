//! History ring buffer + hand-rolled immediate-mode line plot
//! (Cycle 6 item 6, plan D-C4).
//!
//! The dashboard's 10-minute trend window:
//!
//! - [`RingBuffer<T>`] — a fixed-capacity FIFO (default
//!   [`HISTORY_CAPACITY`] = 300 samples = 10 min at the 2 s poll
//!   cadence). `push` appends the newest sample and evicts the oldest
//!   once the buffer is full. Backed by a pre-allocated
//!   `Vec<Option<T>>` that never grows past the capacity (bounded
//!   memory, D-C4).
//! - [`HistoryState`] — the three trend series in lockstep: MCLK
//!   (MHz, `ClockReadout.mclk_mhz`), VDDCR_SOC (mV,
//!   `VoltageSet.vddcr_soc_mv`), and memory bandwidth (GB/s). The
//!   background poller appends one sample per successful poll and
//!   clears the state on reconnect (C6-25 wires this into
//!   `TelemetryData`; the poller remains the only writer, D6).
//! - [`render_history`] — the immediate-mode renderer: a titled SLATE
//!   frame (the zone idiom) with one sparkline row per series.
//! - [`plot_series`] — the single-series primitive: normalizes a
//!   `&[f64]` (oldest → newest) onto the row rect and draws it with
//!   `egui::Painter` only — `line_segment` for the polyline, a filled
//!   dot for a single sample, dim min/max labels.
//!
//! **No new dependency (D-C4):** egui has no built-in chart, so the
//! plot is a few dozen painter calls — no chart crate, and the
//! MSRV-1.75 lockfile stays untouched.
//!
//! **Generic buffer (documented choice):** [`RingBuffer<T>`] keeps the
//! plan's frozen generic shape; `HistoryState` instantiates it with
//! `f64` for the three series. Monomorphization makes the genericity
//! free, and a later chunk can store any sample type (e.g. a
//! timestamped tuple) without a second buffer.
//!
//! **No-panic contract (D5):** the buffer never panics — the
//! capacity is clamped to ≥ 1 (every index op is a modulo over a
//! nonzero length) and eviction is an in-place overwrite. The plot
//! guards every division (rect width/height, sample span), skips
//! non-finite samples, and an empty / one-sample / all-NaN series
//! draws its label row but no geometry — never a panic, never a
//! division by zero.
//!
//! **Pure core:** [`sample_points`] / [`finite_min_max`] are I/O-free
//! and deterministic (the unit tests exercise them without an egui
//! context); [`render_history`] / [`plot_series`] are the thin `egui`
//! surface over them (the live render is verified in the QA phase).

use egui::{Align2, Color32, FontId, Margin, Pos2, Rect, RichText, Sense, Stroke, Vec2};

use crate::{AMBER, CYAN, SLATE};

/// Default history depth: 300 samples = 10 minutes at the 2 s poll
/// cadence (plan D-C4 — bounded memory, no unbounded growth).
pub const HISTORY_CAPACITY: usize = 300;

/// Sparkline row height (points) in [`plot_series`].
const PLOT_ROW_HEIGHT: f32 = 40.0;
/// Sparkline stroke width (points).
const PLOT_LINE_WIDTH: f32 = 1.5;
/// Radius of the single-sample dot (points).
const PLOT_DOT_RADIUS: f32 = 2.0;
/// Corner margin for the label / min / max text (points).
const PLOT_TEXT_MARGIN: f32 = 3.0;
/// The plot row background (a step darker than the SLATE frame fill).
const PLOT_BG: Color32 = Color32::from_rgb(0x16, 0x16, 0x1C);
/// The zone title (the dashboard's history zone).
const HISTORY_TITLE: &str = "TREND HISTORY (LAST 10 MIN)";

// ---------------------------------------------------------------------
// The fixed-capacity ring buffer (bounded memory, D-C4).
// ---------------------------------------------------------------------

/// A fixed-capacity FIFO ring buffer: `push` appends the newest
/// sample and evicts the oldest once the capacity is reached.
///
/// Generic over `T` (documented in the module docs — `HistoryState`
/// instantiates it with `f64`). The backing store is a pre-allocated
/// `Vec<Option<T>>` sized at construction: **no heap growth past the
/// fixed capacity** (D-C4 — a bounded 10-minute window). The capacity
/// is clamped to ≥ 1 so every index computation is a modulo over a
/// nonzero length (no-panic contract, D5).
///
/// Invariant: the live samples occupy `data[head .. head + len]`
/// (wrapping at the capacity), each guaranteed `Some`; every other
/// slot is `None`.
#[derive(Debug, Clone)]
pub struct RingBuffer<T> {
    /// Pre-allocated storage (see the type invariant).
    data: Vec<Option<T>>,
    /// Index of the oldest live sample (0 while the buffer is not
    /// full; advances with each evicting push).
    head: usize,
    /// Number of live samples (always ≤ `data.len()`).
    len: usize,
}

impl<T> RingBuffer<T> {
    /// A buffer at the default [`HISTORY_CAPACITY`] depth.
    pub fn new() -> Self {
        Self::with_capacity(HISTORY_CAPACITY)
    }

    /// A buffer at `capacity` (clamped to ≥ 1, see the type docs).
    pub fn with_capacity(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        // `resize_with` (not `vec![None; n]`): no `T: Clone` bound.
        let mut data: Vec<Option<T>> = Vec::with_capacity(capacity);
        data.resize_with(capacity, || None);
        Self { data, head: 0, len: 0 }
    }

    /// Append `value` as the newest sample. Once the buffer is full
    /// the oldest sample is evicted (its slot overwritten in place —
    /// no reallocation, ever).
    pub fn push(&mut self, value: T) {
        if self.len == self.data.len() {
            // Full: overwrite the oldest slot, then advance the head.
            self.data[self.head] = Some(value);
            self.head = (self.head + 1) % self.data.len();
        } else {
            let idx = (self.head + self.len) % self.data.len();
            self.data[idx] = Some(value);
            self.len += 1;
        }
    }

    /// The number of live samples.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether no samples are stored.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Whether the buffer holds its full capacity.
    pub fn is_full(&self) -> bool {
        self.len == self.data.len()
    }

    /// The fixed capacity (the clamp-≥1 value from construction).
    pub fn capacity(&self) -> usize {
        self.data.len()
    }

    /// The newest sample (the last one pushed), if any.
    pub fn last(&self) -> Option<&T> {
        if self.len == 0 {
            return None;
        }
        // The live window's last slot (the invariant makes it `Some`).
        self.data[(self.head + self.len - 1) % self.data.len()].as_ref()
    }

    /// The live samples, oldest → newest.
    pub fn iter(&self) -> RingIter<'_, T> {
        RingIter {
            data: &self.data,
            idx: self.head,
            remaining: self.len,
        }
    }

    /// Drop every sample (the head rewinds; the storage is kept and
    /// reused — no reallocation).
    pub fn clear(&mut self) {
        for slot in &mut self.data {
            *slot = None;
        }
        self.head = 0;
        self.len = 0;
    }
}

impl<T> Default for RingBuffer<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// An iterator over a [`RingBuffer`]'s live samples, oldest → newest.
pub struct RingIter<'a, T> {
    data: &'a [Option<T>],
    idx: usize,
    remaining: usize,
}

impl<'a, T> Iterator for RingIter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<&'a T> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        // The live window's slot is guaranteed `Some` (the invariant);
        // the `?` is a panic-free tripwire, not a reachable failure.
        let value = self.data[self.idx].as_ref()?;
        self.idx = (self.idx + 1) % self.data.len();
        Some(value)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

// ---------------------------------------------------------------------
// The three trend series (one sample per successful poll, C6-25).
// ---------------------------------------------------------------------

/// The GUI's trend state: the last [`HISTORY_CAPACITY`] (300 = 10 min
/// at the 2 s poll) samples of the three dashboard series — MCLK
/// (MHz), VDDCR_SOC (mV), and memory bandwidth (GB/s) — all in
/// lockstep: the buffers share one capacity and receive one push per
/// poll, so each holds the same number of samples.
///
/// The background poller is the only writer (D6 — C6-25 appends one
/// sample per successful poll and clears the state on reconnect).
#[derive(Debug, Clone, Default)]
pub struct HistoryState {
    /// Memory clock (MHz, `ClockReadout.mclk_mhz`).
    pub mclk: RingBuffer<f64>,
    /// VDDCR_SOC rail (mV, `VoltageSet.vddcr_soc_mv`).
    pub vddcr_soc: RingBuffer<f64>,
    /// Memory bandwidth (GB/s) — the live / terminal bench figure
    /// (C6-25 picks the exact source; the field carries the series).
    pub bandwidth: RingBuffer<f64>,
}

impl HistoryState {
    /// Append one poll's sample to all three series (each evicts its
    /// oldest once full).
    pub fn push(&mut self, mclk_mhz: f64, vddcr_soc_mv: f64, bandwidth_gbps: f64) {
        self.mclk.push(mclk_mhz);
        self.vddcr_soc.push(vddcr_soc_mv);
        self.bandwidth.push(bandwidth_gbps);
    }

    /// The number of samples stored (all three series in lockstep).
    pub fn len(&self) -> usize {
        self.mclk.len()
    }

    /// Whether no samples have been appended yet.
    pub fn is_empty(&self) -> bool {
        self.mclk.is_empty()
    }

    /// Drop every sample from all three series (the reconnect prime —
    /// C6-25; the storage is kept and reused).
    pub fn clear(&mut self) {
        self.mclk.clear();
        self.vddcr_soc.clear();
        self.bandwidth.clear();
    }
}

// ---------------------------------------------------------------------
// The pure plot geometry (testable: no egui context, no I/O).
// ---------------------------------------------------------------------

/// The (min, max) of the finite samples, or `None` when no sample is
/// finite (an empty / all-NaN / all-±inf series draws nothing).
fn finite_min_max(samples: &[f64]) -> Option<(f64, f64)> {
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for value in samples {
        if value.is_finite() {
            min = min.min(*value);
            max = max.max(*value);
        }
    }
    if min == f64::INFINITY {
        // No finite sample (empty or all non-finite).
        None
    } else {
        Some((min, max))
    }
}

/// Map `samples` (oldest → newest) onto `rect` as pixel points:
/// evenly spaced left → right across the width (first at the left
/// edge, last at the right edge), and value-normalized bottom → top
/// across the height. A flat series (min == max) sits on the rect's
/// midline; a single sample is one point at the center.
///
/// Degenerate inputs yield an empty list (the caller draws nothing):
/// no finite sample, or a rect with zero / negative width or height
/// (the `Rect::NOTHING` / clipped-empty case) — every division is
/// guarded, so nothing panics and nothing divides by zero (D5).
fn sample_points(samples: &[f64], rect: Rect) -> Vec<Pos2> {
    let Some((min, max)) = finite_min_max(samples) else {
        return Vec::new();
    };
    if rect.width() <= 1.0 || rect.height() <= 1.0 {
        return Vec::new();
    }
    let finite: Vec<f64> = samples.iter().copied().filter(|value| value.is_finite()).collect();
    let count = finite.len();
    let span = max - min; // ≥ 0.0; 0.0 = a flat series (the midline).
    finite
        .iter()
        .enumerate()
        .map(|(i, value)| {
            let x = if count == 1 {
                rect.center().x
            } else {
                rect.left() + rect.width() * ((i as f32) / ((count - 1) as f32))
            };
            let y = if span == 0.0 {
                rect.center().y
            } else {
                rect.bottom() - rect.height() * (((value - min) as f32) / (span as f32))
            };
            Pos2::new(x, y)
        })
        .collect()
}

// ---------------------------------------------------------------------
// The egui surface (the hand-rolled immediate-mode plot, D-C4).
// ---------------------------------------------------------------------

/// One hand-rolled sparkline row (D-C4 — no chart crate): allocates a
/// fixed-height row in `ui`, normalizes `samples` (oldest → newest)
/// onto the row rect with [`sample_points`], and draws it with
/// `egui::Painter` — a filled dot for a single sample,
/// `line_segment` for the rest — plus a dim series label (top-left),
/// a dim max (top-right), and a dim min (bottom-right).
///
/// Graceful degeneration (no-panic, D5): an empty or all-non-finite
/// series draws only the background + label (no geometry, no
/// min/max); a zero-area rect draws nothing at all — every division
/// is guarded upstream in [`sample_points`] / [`finite_min_max`].
pub fn plot_series(
    ui: &mut egui::Ui,
    samples: &[f64],
    label: &str,
    color: Color32,
) -> egui::Response {
    let row_width = ui.available_width().max(1.0);
    let desired = Rect::from_min_size(ui.cursor().min, Vec2::new(row_width, PLOT_ROW_HEIGHT));
    let rect = desired.intersect(ui.available_rect_before_wrap());
    let response = ui.allocate_rect(rect, Sense::hover());
    let dim = ui.visuals().weak_text_color();
    let painter = ui.painter();

    // The row background (always, so an empty series still shows its
    // label + the plot area's extent).
    painter.rect_filled(rect, 3.0, PLOT_BG);
    // The series label (always, over the plot).
    painter.text(
        rect.left_top() + Vec2::new(PLOT_TEXT_MARGIN, PLOT_TEXT_MARGIN),
        Align2::LEFT_TOP,
        label,
        FontId::monospace(11.0),
        dim,
    );
    if rect.width() > 1.0 && rect.height() > 1.0 {
        let points = sample_points(samples, rect);
        match points.len() {
            0 => {
                // Empty / all non-finite: the label row only.
            }
            1 => {
                // A single sample: one dot at the center.
                painter.circle_filled(points[0], PLOT_DOT_RADIUS, color);
            }
            _ => {
                // The polyline: one segment between consecutive points.
                for pair in points.windows(2) {
                    painter.line_segment([pair[0], pair[1]], Stroke::new(PLOT_LINE_WIDTH, color));
                }
            }
        }
        if let Some((min, max)) = finite_min_max(samples) {
            // The min / max at the right (newest) edge, dim.
            painter.text(
                rect.right_top() + Vec2::new(-PLOT_TEXT_MARGIN, PLOT_TEXT_MARGIN),
                Align2::RIGHT_TOP,
                format!("{max:.1}"),
                FontId::monospace(11.0),
                dim,
            );
            painter.text(
                rect.right_bottom() + Vec2::new(-PLOT_TEXT_MARGIN, -PLOT_TEXT_MARGIN),
                Align2::RIGHT_BOTTOM,
                format!("{min:.1}"),
                FontId::monospace(11.0),
                dim,
            );
        }
    }
    response
}

/// The dashboard's history zone (plan D-C4, item 6): a titled SLATE
/// frame (the zone idiom) with the three trend series as sparkline
/// rows — MCLK (MHz), VDDCR_SOC (mV), memory bandwidth (GB/s) — one
/// sample per successful poll over the last 300 polls (10 min).
///
/// Pure immediate-mode drawing over [`HistoryState`] (no I/O, D6): an
/// empty state renders the three dim label rows and never panics; the
/// live run (the QA gate) shows the growing sparklines. C6-25 wires
/// the poller to append one sample per poll and to clear on
/// reconnect.
pub fn render_history(ui: &mut egui::Ui, state: &HistoryState) {
    let frame = egui::Frame::default()
        .fill(SLATE)
        .stroke(Stroke::new(1.0_f32, CYAN))
        .inner_margin(Margin::symmetric(10.0, 6.0));
    let _ = frame.show(ui, |ui| {
        ui.label(RichText::new(HISTORY_TITLE).strong().color(CYAN));
        ui.add_space(4.0);
        // One sparkline row per series (the buffers share a capacity
        // and receive one push per poll — always in lockstep). The
        // transient `Vec` is a render-only copy (300 f64 ≈ 2.4 KB);
        // the buffers themselves never grow (D-C4).
        let mclk: Vec<f64> = state.mclk.iter().copied().collect();
        let vddcr_soc: Vec<f64> = state.vddcr_soc.iter().copied().collect();
        let bandwidth: Vec<f64> = state.bandwidth.iter().copied().collect();
        // Palette semantics (style.rs): clocks + voltages AMBER,
        // bandwidth CYAN.
        plot_series(ui, &mclk, "MCLK (MHz)", AMBER);
        ui.add_space(2.0);
        plot_series(ui, &vddcr_soc, "VDDCR_SOC (mV)", AMBER);
        ui.add_space(2.0);
        plot_series(ui, &bandwidth, "BANDWIDTH (GB/s)", CYAN);
    });
}

// ---------------------------------------------------------------------
// Tests (headless: no window, no display).
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A 100×50 rect at (10, 20) for the geometry tests.
    fn plot_rect() -> Rect {
        Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(100.0, 50.0))
    }

    /// Approximate f32 equality (the geometry is exact for the test
    /// values, but the comparison stays robust).
    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() <= 1e-4
    }

    /// (a) A fresh buffer is empty, at the default capacity, and not
    /// full; `last` / `iter` report nothing.
    #[test]
    fn new_buffer_is_empty_at_default_capacity() {
        let buf: RingBuffer<f64> = RingBuffer::new();
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);
        assert_eq!(buf.capacity(), HISTORY_CAPACITY);
        assert!(!buf.is_full());
        assert!(buf.last().is_none());
        assert!(buf.iter().next().is_none());
    }

    /// (b) A partial fill: `len` tracks the pushes, `iter` runs
    /// oldest → newest, `last` is the newest, and it is not full.
    #[test]
    fn partial_fill_iter_order_and_last() {
        let mut buf: RingBuffer<u32> = RingBuffer::with_capacity(5);
        for value in [10, 20, 30] {
            buf.push(value);
        }
        assert_eq!(buf.len(), 3);
        assert!(!buf.is_full());
        assert_eq!(buf.last(), Some(&30));
        let order: Vec<u32> = buf.iter().copied().collect();
        assert_eq!(order, vec![10, 20, 30], "iter must run oldest → newest");
    }

    /// (c) Wrap-around: push capacity + K — the oldest K are evicted,
    /// the last capacity remain in order, and the capacity never grows.
    #[test]
    fn wraparound_evicts_the_oldest() {
        let mut buf: RingBuffer<u32> = RingBuffer::with_capacity(5);
        for value in 0..8 {
            buf.push(value); // 3 past the capacity
        }
        assert!(buf.is_full());
        assert_eq!(buf.capacity(), 5, "the capacity must never grow");
        assert_eq!(buf.len(), 5);
        assert_eq!(buf.last(), Some(&7));
        let order: Vec<u32> = buf.iter().copied().collect();
        assert_eq!(order, vec![3, 4, 5, 6, 7], "the oldest three (0, 1, 2) must be evicted, in order");
    }

    /// (d) The default 300-capacity buffer: push 350 — the first 50
    /// are evicted, 300 remain (10 min at the 2 s poll).
    #[test]
    fn default_capacity_wrap() {
        let mut buf: RingBuffer<f64> = RingBuffer::new();
        for value in 0..350 {
            buf.push(f64::from(value));
        }
        assert!(buf.is_full());
        assert_eq!(buf.len(), HISTORY_CAPACITY);
        assert_eq!(buf.last(), Some(&349.0));
        assert_eq!(buf.iter().next(), Some(&50.0), "the 50 oldest must be evicted");
    }

    /// (e) `clear` drops every sample (the head rewinds) and pushes
    /// after it still wrap correctly — the freed slots are reused, no
    /// growth.
    #[test]
    fn clear_resets_and_reuse_wraps() {
        let mut buf: RingBuffer<u32> = RingBuffer::with_capacity(3);
        for value in [1, 2, 3, 4] {
            buf.push(value); // full; the head has advanced
        }
        assert!(buf.is_full());
        buf.clear();
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);
        assert!(buf.last().is_none());
        assert!(buf.iter().next().is_none());
        // Fill past the capacity again: the freed slots must be
        // reused without a reallocation.
        for value in [9, 8, 7, 6] {
            buf.push(value);
        }
        assert!(buf.is_full());
        assert_eq!(buf.last(), Some(&6));
        let order: Vec<u32> = buf.iter().copied().collect();
        assert_eq!(order, vec![8, 7, 6]);
    }

    /// (f) A zero capacity is clamped to one (no modulo-by-zero —
    /// the no-panic contract, D5).
    #[test]
    fn zero_capacity_is_clamped() {
        let mut buf: RingBuffer<u8> = RingBuffer::with_capacity(0);
        assert_eq!(buf.capacity(), 1);
        buf.push(1);
        buf.push(2);
        assert!(buf.is_full());
        assert_eq!(buf.last(), Some(&2));
        assert_eq!(buf.iter().count(), 1);
    }

    /// (g) `HistoryState`: pushes land in lockstep across the three
    /// series; `len` / `is_empty` / `clear` follow the buffers.
    #[test]
    fn history_state_pushes_in_lockstep() {
        let mut state = HistoryState::default();
        assert!(state.is_empty());
        assert_eq!(state.len(), 0);
        state.push(1800.0, 1150.0, 26.3);
        state.push(1600.0, 1100.0, 30.1);
        assert!(!state.is_empty());
        assert_eq!(state.len(), 2);
        let mclk: Vec<f64> = state.mclk.iter().copied().collect();
        assert_eq!(mclk, vec![1800.0, 1600.0]);
        assert_eq!(state.vddcr_soc.last(), Some(&1100.0));
        assert_eq!(state.bandwidth.iter().next(), Some(&26.3));
        state.clear();
        assert!(state.is_empty());
        assert!(state.vddcr_soc.last().is_none());
    }

    /// (h) `finite_min_max` skips the non-finite and is exact on the
    /// rest; empty / all-non-finite → `None`.
    #[test]
    fn finite_min_max_skips_non_finite() {
        assert_eq!(finite_min_max(&[]), None);
        assert_eq!(finite_min_max(&[f64::NAN]), None);
        assert_eq!(finite_min_max(&[f64::INFINITY, f64::NEG_INFINITY, f64::NAN]), None);
        assert_eq!(finite_min_max(&[3.0, 1.0, 9.0]), Some((1.0, 9.0)));
        assert_eq!(
            finite_min_max(&[f64::INFINITY, 2.0, f64::NEG_INFINITY, 8.0]),
            Some((2.0, 8.0))
        );
        assert_eq!(finite_min_max(&[5.0, 5.0]), Some((5.0, 5.0)));
    }

    /// (i) Degenerate inputs draw nothing (no panic, no division by
    /// zero): empty, all non-finite, a `Rect::NOTHING`, and a
    /// zero-size rect.
    #[test]
    fn sample_points_degenerate_inputs_are_empty() {
        assert!(sample_points(&[], plot_rect()).is_empty());
        assert!(sample_points(&[f64::NAN, f64::INFINITY, f64::NEG_INFINITY], plot_rect()).is_empty());
        assert!(sample_points(&[1.0, 2.0, 3.0], Rect::NOTHING).is_empty());
        assert!(
            sample_points(&[1.0, 2.0, 3.0], Rect::from_min_size(Pos2::new(5.0, 5.0), Vec2::ZERO))
                .is_empty()
        );
    }

    /// (j) A single sample is one point at the rect's center (the
    /// flat-span guard).
    #[test]
    fn sample_points_single_is_centered() {
        let points = sample_points(&[42.0], plot_rect());
        assert_eq!(points.len(), 1);
        assert!(close(points[0].x, 60.0));
        assert!(close(points[0].y, 45.0));
    }

    /// (k) Two extremes normalize onto the rect's corners: the min at
    /// the bottom-left, the max at the top-right.
    #[test]
    fn sample_points_extremes_hit_the_rect_corners() {
        let points = sample_points(&[0.0, 10.0], plot_rect());
        assert_eq!(points.len(), 2);
        assert!(
            close(points[0].x, 10.0) && close(points[0].y, 70.0),
            "the min sits bottom-left"
        );
        assert!(
            close(points[1].x, 110.0) && close(points[1].y, 20.0),
            "the max sits top-right"
        );
    }

    /// (l) A flat series (min == max) sits on the rect's midline —
    /// the span division never fires.
    #[test]
    fn sample_points_flat_series_sits_on_the_midline() {
        let points = sample_points(&[7.0, 7.0, 7.0], plot_rect());
        assert_eq!(points.len(), 3);
        for point in &points {
            assert!(close(point.y, 45.0), "a flat series must sit on the midline");
        }
        assert!(close(points[0].x, 10.0));
        assert!(close(points[2].x, 110.0));
    }

    /// (m) A mid value of three lands halfway up, centered in x.
    #[test]
    fn sample_points_mid_value_half_height() {
        let points = sample_points(&[0.0, 5.0, 10.0], plot_rect());
        assert_eq!(points.len(), 3);
        assert!(close(points[1].x, 60.0));
        assert!(close(points[1].y, 45.0));
    }

    /// One headless frame on a fresh context (the `begin_frame`
    /// pattern from the `egui` docs — the fonts load there), running
    /// `draw` inside a central panel. The 10000×10000 default screen
    /// rect gives every row a real area, so the full paint path
    /// executes — what is under test is the no-panic contract.
    fn run_headless_frame(draw: impl FnOnce(&mut egui::Ui)) {
        let ctx = egui::Context::default();
        ctx.begin_frame(egui::RawInput::default());
        egui::CentralPanel::default().show(&ctx, |ui| draw(ui));
    }

    /// (n) The full widgets run headless without panicking: an empty
    /// state, a one-sample state, a full 300-sample state, and the
    /// `plot_series` primitive on empty / single slices.
    #[test]
    fn render_history_runs_headless_without_panicking() {
        // Empty state: three label rows, no geometry.
        run_headless_frame(|ui| {
            render_history(ui, &HistoryState::default());
        });

        // One sample each: a dot per row.
        let mut one = HistoryState::default();
        one.push(1800.0, 1150.0, 26.3);
        run_headless_frame(|ui| {
            render_history(ui, &one);
        });

        // A full 300-sample state: the line path (the `windows(2)`
        // segments).
        let mut full = HistoryState::default();
        for i in 0..HISTORY_CAPACITY {
            full.push(
                1800.0 + 0.1 * (i as f64),
                1100.0 + 0.05 * (i as f64),
                20.0 + 0.01 * (i as f64),
            );
        }
        assert!(!full.is_empty());
        run_headless_frame(|ui| {
            render_history(ui, &full);
        });

        // The primitive directly: empty + single-sample slices.
        run_headless_frame(|ui| {
            let _ = plot_series(ui, &[], "EMPTY", CYAN);
            let _ = plot_series(ui, &[3.5], "ONE", CYAN);
        });
    }
}
