//! GUI semantic style + export helpers (P3-25, Grand Design §3.2).
//!
//! The visual contract of the desktop GUI: the four semantic palette
//! colors (cyan / amber / slate / crimson) + the muted N/A gray
//! ([`NA_GRAY`], C8-05 — unavailable, not critical), the dark-slate
//! [`build_style`], and the two file exports the app shell (P3-30)
//! triggers on F2 / F3:
//!
//! - [`export_json`] — the F3 snapshot: the
//!   [`SystemMemoryTelemetry`] wire root + the terminal
//!   [`BenchmarkGrid`], as pretty JSON — the seven telemetry wire
//!   keys at the top level (byte-compatible with the pre-C6-29
//!   file) + a `bench` key (the grid object, or `null` before the
//!   first completed run).
//! - [`snapshot_png`] — the F2 validation card: a 640×420 PNG with a
//!   title row (`RamSleuth v2.1.0` + the UTC wall clock), a CPU line
//!   (brand + clock) and a RAM line (capacity + channel, honest `N/A`
//!   for absent cells) over the 4×4 [`BenchmarkGrid`], each cell
//!   colored by its value magnitude along the CYAN → AMBER → CRIMSON
//!   ramp on a SLATE background.
//!
//! **No-panic contract (D5):** every failure is a structured
//! [`GuiError`]. Nothing here opens a window or touches a display, so
//! the whole module is testable headless.

use std::fmt;
use std::path::Path;

use ramsleuth_bench::{BenchmarkGrid, Metric, Tier};
use ramsleuth_telemetry::SystemMemoryTelemetry;

// ---------------------------------------------------------------------
// Semantic palette (Grand Design §3.2, exact).
// ---------------------------------------------------------------------

/// Primary accent: live timings / bandwidth values.
pub const CYAN: egui::Color32 = egui::Color32::from_rgb(0x00, 0xD4, 0xFF);
/// Secondary accent: 1:1 sync clocks, voltages, low latency.
pub const AMBER: egui::Color32 = egui::Color32::from_rgb(0xFF, 0xB3, 0x00);
/// Neutral: backgrounds, separators, panel/window fills.
pub const SLATE: egui::Color32 = egui::Color32::from_rgb(0x1E, 0x1E, 0x24);
/// Alert: 1:2 desync, out-of-spec voltages, errors.
pub const CRIMSON: egui::Color32 = egui::Color32::from_rgb(0xFF, 0x3B, 0x30);
/// Muted / dimmed gray: N/A / unavailable sensors (C8-05, D-5) — the
/// "absent, not critical" semantic; readable on the SLATE panel,
/// distinct from `DIM_CELL` (the F2 card fill for unmeasured cells)
/// and from the 0xE6E6EC body text.
pub const NA_GRAY: egui::Color32 = egui::Color32::from_rgb(0x8A, 0x8A, 0x94);

/// Build the dark-slate app style: SLATE window/panel fills, light
/// text, CYAN accents (hover/active strokes, hyperlinks, text cursor,
/// selection). AMBER/CRIMSON and the muted [`NA_GRAY`] (C8-05: the
/// fifth *text* color for N/A / unavailable cells, the zone-local
/// `STATUS_DONE` precedent) are consumed by the zones that render the
/// semantic values (P3-27/28), not by the base style.
pub fn build_style() -> egui::Style {
    let text = egui::Color32::from_rgb(0xE6, 0xE6, 0xEC);
    let mut visuals = egui::Visuals::dark();

    // Surfaces: slate everywhere a background is painted.
    visuals.window_fill = SLATE;
    visuals.panel_fill = SLATE;
    visuals.extreme_bg_color = SLATE;
    visuals.override_text_color = Some(text);

    // Widget family: slate fills, cyan interaction accents.
    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = SLATE;
    widgets.noninteractive.fg_stroke.color = text;
    widgets.inactive.bg_fill = egui::Color32::from_rgb(0x2A, 0x2A, 0x33);
    widgets.inactive.fg_stroke.color = text;
    widgets.hovered.bg_fill = egui::Color32::from_rgb(0x34, 0x34, 0x40);
    widgets.hovered.weak_bg_fill = egui::Color32::from_rgb(0x2E, 0x2E, 0x38);
    widgets.hovered.fg_stroke.color = CYAN;
    widgets.active.bg_fill = egui::Color32::from_rgb(0x3E, 0x3E, 0x4C);
    widgets.active.weak_bg_fill = egui::Color32::from_rgb(0x34, 0x34, 0x40);
    widgets.active.fg_stroke.color = CYAN;

    // Selection + text affordances: cyan.
    visuals.selection.bg_fill = egui::Color32::from_rgba_unmultiplied(0x00, 0xD4, 0xFF, 64);
    visuals.selection.stroke.color = CYAN;
    visuals.hyperlink_color = CYAN;

    egui::Style { visuals, ..Default::default() }
}

// ---------------------------------------------------------------------
// Export errors (no-panic: the caller shows the message, never crashes).
// ---------------------------------------------------------------------

/// Failure of one of the file-export helpers.
#[derive(Debug)]
pub enum GuiError {
    /// A file-system failure while writing an export.
    Io(std::io::Error),
    /// The PNG encoder rejected the payload.
    Png(String),
    /// JSON serialization failed (unreachable for the wire-safe payload types).
    Json(String),
}

// `std::io::Error` is neither `Clone` nor `PartialEq`, so both traits
// are hand-implemented on the `Io` arm (variant + kind + message — the
// workspace `ClientError` / `RpcError` precedent).
impl Clone for GuiError {
    fn clone(&self) -> Self {
        match self {
            Self::Io(err) => Self::Io(std::io::Error::new(err.kind(), err.to_string())),
            Self::Png(msg) => Self::Png(msg.clone()),
            Self::Json(msg) => Self::Json(msg.clone()),
        }
    }
}

impl PartialEq for GuiError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Io(a), Self::Io(b)) => a.kind() == b.kind() && a.to_string() == b.to_string(),
            (Self::Png(a), Self::Png(b)) => a == b,
            (Self::Json(a), Self::Json(b)) => a == b,
            _ => false,
        }
    }
}

impl fmt::Display for GuiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "export I/O error: {err}"),
            Self::Png(msg) => write!(f, "PNG encode error: {msg}"),
            Self::Json(msg) => write!(f, "JSON serialize error: {msg}"),
        }
    }
}

impl std::error::Error for GuiError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------
// F3: JSON export.
// ---------------------------------------------------------------------

/// The F3 wire snapshot (C6-29): the frozen [`SystemMemoryTelemetry`]
/// root flattened into the top level (its seven wire keys, unchanged
/// — no type duplication, D2) plus the `bench` key.
#[derive(serde::Serialize)]
struct ExportSnapshot<'a> {
    #[serde(flatten)]
    telemetry: &'a SystemMemoryTelemetry,
    /// The terminal 4×4 benchmark grid (the wire's unmeasured `N/A`
    /// cells are the honest `0.0`); `null` before the first completed
    /// run.
    bench: Option<&'a BenchmarkGrid>,
}

/// F3: write one telemetry + benchmark snapshot to `path` as pretty
/// JSON.
///
/// The wire root (`SystemMemoryTelemetry`, serde-derived since P3-06)
/// serializes under its own seven wire keys, and the terminal
/// [`BenchmarkGrid`] rides alongside it under the `bench` key (C6-29,
/// item 8b): the grid object when a run has completed, the JSON
/// `null` before the first run. Both absent-grid shapes — no run yet
/// (`None`) and the all-`0.0`/`N/A` grid — export as-is, never a
/// panic (D5). A JSON failure maps to [`GuiError::Json`], a file
/// write to [`GuiError::Io`].
pub fn export_json(
    telemetry: &SystemMemoryTelemetry,
    bench: Option<&BenchmarkGrid>,
    path: &Path,
) -> Result<(), GuiError> {
    let snapshot = ExportSnapshot { telemetry, bench };
    let json = serde_json::to_string_pretty(&snapshot)
        .map_err(|err| GuiError::Json(err.to_string()))?;
    std::fs::write(path, json).map_err(GuiError::Io)?;
    Ok(())
}

// ---------------------------------------------------------------------
// F2: PNG snapshot of the benchmark grid.
// ---------------------------------------------------------------------

// The F2 validation card (C6-28): a 640×420 PNG — the title row
// (`RamSleuth v2.1.0` + the UTC wall clock), the CPU line (brand +
// clock), and the RAM line (capacity + channel), each the 5×7 font at
// 2× scale, a dim separator, then the 4×4 [`BenchmarkGrid`] pinned to
// the bottom margin (4 × 64 + 3 × 16 = 304 tall, 4 × 140 + 3 × 16 =
// 608 wide = 640 − 2 × 16), leaving the 16…100 band for the header.
const CARD_W: usize = 640;
const CARD_H: usize = 420;
const MARGIN: usize = 16;
/// One grid cell (the AIDA64 layout is 4 tiers × 4 metrics).
const CELL_W: usize = 140;
const CELL_H: usize = 64;
/// Gutter between cells; the outer margin is shared with the header.
const GUTTER: usize = 16;
/// The 5×7 font's 2×-scaled line height.
const LINE_H: usize = 14;
/// The header block's lines: title / CPU / RAM.
const TITLE_Y0: usize = MARGIN; // 16
const CPU_Y0: usize = MARGIN + LINE_H + 8; // 38
const RAM_Y0: usize = MARGIN + 2 * (LINE_H + 8); // 60
/// The 4×4 grid's top, pinned to the bottom margin (420 − 16 − 304).
const GRID_Y0: usize = CARD_H - MARGIN - (4 * CELL_H + 3 * GUTTER); // 100
/// A dim 1px separator halfway between the RAM line and the grid.
const SEPARATOR_Y: usize = (RAM_Y0 + LINE_H + GRID_Y0) / 2; // 87
/// The header lines' text color (the same light gray as
/// `build_style`'s text).
const TEXT: egui::Color32 = egui::Color32::from_rgb(0xE6, 0xE6, 0xEC);
/// `ln` value at which the color ramp saturates (≈ 999 → 6.9).
const RAMP_FULL: f64 = 6.9;
/// Unmeasured cell (0.0 on the wire): dim slate, distinct from the
/// background.
const DIM_CELL: egui::Color32 = egui::Color32::from_rgb(0x3A, 0x3A, 0x44);

/// F2: encode the validation card — the [`BenchmarkGrid`] plus the
/// header block (title + UTC wall clock, the CPU line, the RAM line) —
/// as a 640×420 data-visualization PNG and write it to `path`.
///
/// The card (C6-28): a SLATE background, the 5×7-font title row
/// `RamSleuth v2.1.0 <YYYY-MM-DD HH:MM:SS>`, the CPU line (brand +
/// platform clock), the RAM line (total capacity + channel mode from
/// the bound-DIMM count), a dim separator, then the 4×4 block of
/// cells — one per [`Tier`] row × [`Metric`] column, the AIDA64
/// layout — each colored by its value magnitude along the CYAN →
/// AMBER → CRIMSON ramp (an unmeasured 0.0 cell, the wire's `N/A`,
/// renders dim). Absent telemetry or cells degrade to the honest `N/A`
/// text — never a panic. A line longer than the card is clipped at the
/// right margin (no wrap). Produces a valid RGB8 PNG by design — a
/// small data card, not a screen capture (the app shell is the only
/// place window pixels come from). An encoder failure maps to
/// [`GuiError::Png`], a file write to [`GuiError::Io`].
pub fn snapshot_png(
    grid: &BenchmarkGrid,
    telemetry: Option<&SystemMemoryTelemetry>,
    path: &Path,
) -> Result<(), GuiError> {
    let mut pixels = vec![0u8; CARD_W * CARD_H * 3];

    // SLATE background.
    for p in pixels.chunks_exact_mut(3) {
        p[0] = SLATE.r();
        p[1] = SLATE.g();
        p[2] = SLATE.b();
    }
    // The header block: title + wall clock, the CPU line, the RAM
    // line — absent telemetry / cells degrade to the honest `N/A`
    // text (never a panic).
    let stamp = wall_clock_string(now_epoch_secs());
    let title = format!("RamSleuth v{}  {}", env!("CARGO_PKG_VERSION"), stamp);
    let cpu = telemetry.map(snapshot_cpu_line).unwrap_or_else(|| "CPU: N/A".to_owned());
    let ram = telemetry.map(snapshot_ram_line).unwrap_or_else(|| "RAM: N/A".to_owned());
    draw_text(&mut pixels, MARGIN, TITLE_Y0, &title, CYAN);
    draw_text(&mut pixels, MARGIN, CPU_Y0, &cpu, TEXT);
    draw_text(&mut pixels, MARGIN, RAM_Y0, &ram, TEXT);
    // A dim 1px separator between the header block and the grid.
    for x in MARGIN..CARD_W - MARGIN {
        let idx = (SEPARATOR_Y * CARD_W + x) * 3;
        pixels[idx] = DIM_CELL.r();
        pixels[idx + 1] = DIM_CELL.g();
        pixels[idx + 2] = DIM_CELL.b();
    }
    // The 4×4 benchmark grid (repositioned + scaled under the header).
    let tiers = [Tier::Memory, Tier::L1, Tier::L2, Tier::L3];
    let metrics = [Metric::Read, Metric::Write, Metric::Copy, Metric::Latency];
    for (row, tier) in tiers.iter().enumerate() {
        for (col, metric) in metrics.iter().enumerate() {
            let c = cell_color(grid.cell(*tier, *metric));
            let x0 = MARGIN + col * (CELL_W + GUTTER);
            let y0 = GRID_Y0 + row * (CELL_H + GUTTER);
            for y in y0..y0 + CELL_H {
                for x in x0..x0 + CELL_W {
                    let idx = (y * CARD_W + x) * 3;
                    pixels[idx] = c.r();
                    pixels[idx + 1] = c.g();
                    pixels[idx + 2] = c.b();
                }
            }
        }
    }

    let mut out: Vec<u8> = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, CARD_W as u32, CARD_H as u32);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|err| GuiError::Png(err.to_string()))?;
        writer.write_image_data(&pixels).map_err(|err| GuiError::Png(err.to_string()))?;
    }
    std::fs::write(path, out).map_err(GuiError::Io)?;
    Ok(())
}

/// Map one cell value to its color: an unmeasured cell (0.0, or any
/// non-finite) is dim slate; positive magnitudes walk CYAN → AMBER →
/// CRIMSON on a log scale (`log1p`, saturating at [`RAMP_FULL`]) so DRAM
/// GB/s and ns/hop latencies share one ramp.
fn cell_color(value: f64) -> egui::Color32 {
    if !value.is_finite() || value <= 0.0 {
        return DIM_CELL;
    }
    let t = (value.ln() / RAMP_FULL).clamp(0.0, 1.0);
    if t < 0.5 {
        lerp(CYAN, AMBER, t * 2.0)
    } else {
        lerp(AMBER, CRIMSON, (t - 0.5) * 2.0)
    }
}

/// Linear interpolation between two colors in RGB space.
fn lerp(a: egui::Color32, b: egui::Color32, t: f64) -> egui::Color32 {
    let mix = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * t).round() as u8;
    egui::Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

// ---------------------------------------------------------------------
// The card's header block (C6-28): the 5×7 bitmap font + the pure
// line / wall-clock builders (unit-tested below, no display needed).
// ---------------------------------------------------------------------

/// The 5×7 bitmap font for printable ASCII (0x20…0x7E): one byte per
/// row, bit 0 = the leftmost pixel — a public-domain dot-matrix
/// pattern set.
const FONT_5X7: [[u8; 7]; 95] = [
    [0, 0, 0, 0, 0, 0, 0], // ' '
    [4, 4, 4, 4, 4, 0, 4], // !
    [10, 10, 10, 0, 0, 0, 0], // "
    [10, 27, 10, 10, 27, 10, 10], // #
    [14, 9, 10, 6, 21, 9, 14], // $
    [19, 19, 2, 4, 1, 25, 25], // %
    [6, 9, 9, 6, 10, 9, 14], // &
    [4, 4, 0, 0, 0, 0, 0], // '
    [6, 2, 1, 1, 1, 2, 6], // (
    [12, 8, 16, 16, 16, 8, 12], // )
    [0, 10, 14, 31, 14, 10, 0], // *
    [0, 4, 4, 31, 4, 4, 0], // +
    [0, 0, 0, 0, 4, 4, 2], // ,
    [0, 0, 0, 31, 0, 0, 0], // -
    [0, 0, 0, 0, 0, 4, 4], // .
    [16, 8, 8, 4, 2, 2, 1], // /
    [14, 17, 25, 21, 19, 17, 14], // 0
    [4, 2, 4, 4, 4, 4, 14], // 1
    [14, 17, 16, 12, 2, 1, 31], // 2
    [14, 17, 16, 12, 16, 17, 14], // 3
    [8, 12, 10, 9, 31, 16, 16], // 4
    [31, 1, 1, 14, 16, 17, 14], // 5
    [14, 1, 1, 31, 17, 17, 14], // 6
    [31, 16, 8, 4, 2, 2, 2], // 7
    [14, 17, 17, 14, 17, 17, 14], // 8
    [14, 17, 17, 30, 16, 16, 14], // 9
    [0, 4, 4, 0, 4, 4, 0], // :
    [0, 4, 4, 0, 4, 2, 0], // ;
    [16, 8, 4, 2, 4, 8, 16], // <
    [0, 0, 31, 0, 31, 0, 0], // =
    [1, 2, 4, 8, 4, 2, 1], // >
    [14, 17, 16, 12, 0, 0, 4], // ?
    [14, 17, 21, 13, 1, 1, 14], // @
    [14, 17, 17, 31, 17, 17, 17], // A
    [30, 17, 17, 30, 17, 17, 30], // B
    [14, 17, 1, 1, 1, 17, 14], // C
    [30, 17, 17, 17, 17, 17, 30], // D
    [31, 1, 1, 30, 1, 1, 31], // E
    [31, 1, 1, 30, 1, 1, 1], // F
    [14, 17, 1, 21, 17, 17, 14], // G
    [17, 17, 17, 31, 17, 17, 17], // H
    [14, 4, 4, 4, 4, 4, 14], // I
    [28, 16, 16, 16, 16, 17, 6], // J
    [17, 9, 5, 3, 5, 9, 17], // K
    [1, 1, 1, 1, 1, 1, 31], // L
    [17, 27, 21, 21, 17, 17, 17], // M
    [17, 19, 21, 25, 17, 17, 17], // N
    [14, 17, 17, 17, 17, 17, 14], // O
    [30, 17, 17, 30, 1, 1, 1], // P
    [14, 17, 17, 17, 21, 9, 22], // Q
    [30, 17, 17, 30, 5, 9, 17], // R
    [14, 17, 1, 14, 16, 17, 14], // S
    [31, 4, 4, 4, 4, 4, 4], // T
    [17, 17, 17, 17, 17, 17, 14], // U
    [17, 17, 17, 17, 17, 10, 4], // V
    [17, 17, 17, 21, 21, 27, 17], // W
    [17, 17, 10, 4, 10, 17, 17], // X
    [17, 17, 10, 4, 4, 4, 4], // Y
    [31, 16, 8, 4, 2, 1, 31], // Z
    [30, 2, 2, 2, 2, 2, 30], // [
    [1, 2, 2, 4, 8, 16, 16], // \
    [30, 8, 8, 8, 8, 8, 30], // ]
    [4, 10, 17, 0, 0, 0, 0], // ^
    [0, 0, 0, 0, 0, 0, 31], // _
    [4, 2, 1, 0, 0, 0, 0], // `
    [0, 0, 14, 16, 30, 17, 30], // a
    [1, 1, 30, 17, 17, 17, 30], // b
    [0, 0, 14, 1, 1, 17, 14], // c
    [16, 16, 30, 17, 17, 17, 30], // d
    [0, 0, 14, 17, 31, 1, 14], // e
    [12, 2, 30, 2, 2, 2, 2], // f
    [0, 30, 17, 17, 30, 16, 14], // g
    [1, 1, 30, 17, 17, 17, 17], // h
    [4, 0, 6, 4, 4, 4, 14], // i
    [16, 0, 12, 16, 16, 17, 6], // j
    [1, 1, 9, 5, 3, 5, 9], // k
    [6, 4, 4, 4, 4, 4, 14], // l
    [0, 0, 11, 21, 21, 21, 21], // m
    [0, 0, 30, 17, 17, 17, 17], // n
    [0, 0, 14, 17, 17, 17, 14], // o
    [0, 30, 17, 17, 30, 1, 1], // p
    [0, 30, 17, 17, 30, 16, 16], // q
    [0, 0, 21, 19, 1, 1, 1], // r
    [0, 0, 30, 1, 14, 16, 30], // s
    [2, 2, 30, 2, 2, 2, 12], // t
    [0, 0, 17, 17, 17, 17, 30], // u
    [0, 0, 17, 17, 17, 10, 4], // v
    [0, 0, 17, 17, 21, 21, 10], // w
    [0, 0, 17, 10, 4, 10, 17], // x
    [0, 17, 17, 17, 30, 16, 14], // y
    [0, 0, 31, 8, 4, 2, 31], // z
    [8, 4, 4, 2, 4, 4, 8], // {
    [4, 4, 4, 4, 4, 4, 4], // |
    [2, 4, 4, 8, 4, 4, 2], // }
    [0, 0, 18, 9, 0, 0, 0], // ~
];

/// Draw `text` at `(x0, y0)` in the 2×-scaled [`FONT_5X7`] font: one
/// color square per lit font pixel, clipped to the card (a line
/// longer than the card is cut at the right margin — no wrap, no
/// panic). A character outside printable ASCII renders a 3×7 block
/// marker.
fn draw_text(pixels: &mut [u8], x0: usize, y0: usize, text: &str, color: egui::Color32) {
    const SCALE: usize = 2;
    for (i, ch) in text.chars().enumerate() {
        let glyph: [u8; 7] = match ch {
            ' '..='~' => FONT_5X7[(ch as u8 - 0x20) as usize],
            _ => [7u8; 7],
        };
        let gx0 = x0 + i * (6 * SCALE); // 5 font columns + 1 spacing
        if gx0 >= CARD_W {
            break;
        }
        for (row, bits) in glyph.iter().enumerate() {
            let gy0 = y0 + row * SCALE;
            if gy0 >= CARD_H {
                break;
            }
            for col in 0..5 {
                if *bits & (1 << col) == 0 {
                    continue;
                }
                let px0 = gx0 + col * SCALE;
                if px0 >= CARD_W {
                    break;
                }
                for sy in 0..SCALE {
                    let gy = gy0 + sy;
                    if gy >= CARD_H {
                        break;
                    }
                    for sx in 0..SCALE {
                        let px = px0 + sx;
                        if px >= CARD_W {
                            break;
                        }
                        let idx = (gy * CARD_W + px) * 3;
                        pixels[idx] = color.r();
                        pixels[idx + 1] = color.g();
                        pixels[idx + 2] = color.b();
                    }
                }
            }
        }
    }
}

/// The wall clock as `YYYY-MM-DD HH:MM:SS` in UTC (the card carries
/// no local-time dependency — any host reads the same instant).
/// Total over `i64`: a pre-epoch count degrades through `div_euclid`
/// to the honest 1969 rendering — never a panic.
fn wall_clock_string(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400) as u32;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02}:{s:02}")
}

/// Howard Hinnant's `civil_from_days` (days since 1970-01-01 →
/// calendar year / month / day) — the public-domain algorithm, total
/// over `i64` (the era division handles pre-epoch day counts).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146_096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (y + i64::from(m <= 2), m as u32, d)
}

/// The current wall clock in epoch seconds; a pre-epoch clock
/// (impossible on Linux) degrades to 0 — never a panic (the app
/// shell's `unix_timestamp` precedent).
fn now_epoch_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

/// The card's CPU line: the CPUID brand + the platform clock (MHz →
/// GHz, two decimals); an absent clock renders the honest `N/A`
/// (never a panic).
fn snapshot_cpu_line(t: &SystemMemoryTelemetry) -> String {
    let clock = match t.platform.cpu_clock_mhz.value() {
        Some(mhz) => format!("{:.2} GHz", mhz / 1000.0),
        None => "N/A".to_owned(),
    };
    format!("CPU: {} @ {}", t.cpu.brand, clock)
}

/// The card's RAM line: the total capacity (GiB → one-decimal GB) +
/// the channel mode from the bound-DIMM count; both degrade to `N/A`.
fn snapshot_ram_line(t: &SystemMemoryTelemetry) -> String {
    let total = match t.total_capacity.value() {
        Some(gib) => format!("{gib:.1} GB"),
        None => "N/A".to_owned(),
    };
    format!("RAM: {} | {}", total, channel_name(t.dimm_sizes.len()))
}

/// Channel mode from the bound-DIMM count (the C6-20 header rule):
/// 1 / 2 / 4 → Single- / Dual- / Quad-Channel; any other count (0,
/// odd) degrades to `N/A`.
fn channel_name(dimm_count: usize) -> String {
    match dimm_count {
        1 => "Single-Channel".to_owned(),
        2 => "Dual-Channel".to_owned(),
        4 => "Quad-Channel".to_owned(),
        _ => "N/A".to_owned(),
    }
}

// ---------------------------------------------------------------------
// Tests (headless: no window, no display).
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use ramsleuth_telemetry::cpuid::{AmdZen, CpuInfo, CpuVendor};
    use ramsleuth_telemetry::error::{NaReason, Section};
    use ramsleuth_telemetry::SystemPlatform;

    use super::*;

    /// Unique temp path per test (pid-scoped — the client's
    /// temp-socket precedent).
    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("ramsleuth-{name}-{}", std::process::id()))
    }

    /// The PNG signature (first 8 bytes of every valid PNG file).
    const PNG_SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

    /// A representative grid: live DRAM bandwidth + cache tiers +
    /// pointer-chase latency (Phase 1 reference magnitudes).
    fn fixture_grid() -> BenchmarkGrid {
        BenchmarkGrid {
            read_gbps: [26.35, 35.10, 30.40, 12.11],
            write_gbps: [43.63, 38.20, 33.05, 14.02],
            copy_gbps: [12.11, 36.40, 31.80, 11.05],
            latency_ns: [86.84, 1.12, 4.20, 13.90],
        }
    }

    /// (d) The palette constants decode to the exact Grand Design
    /// §3.2 RGB values.
    #[test]
    fn palette_consts_are_exact_rgb() {
        assert_eq!((CYAN.r(), CYAN.g(), CYAN.b()), (0x00, 0xD4, 0xFF));
        assert_eq!((AMBER.r(), AMBER.g(), AMBER.b()), (0xFF, 0xB3, 0x00));
        assert_eq!((SLATE.r(), SLATE.g(), SLATE.b()), (0x1E, 0x1E, 0x24));
        assert_eq!((CRIMSON.r(), CRIMSON.g(), CRIMSON.b()), (0xFF, 0x3B, 0x30));
    }

    /// (d2) C8-05: the N/A gray decodes to the exact muted RGB and is
    /// distinct from the fault-red CRIMSON (unavailable, not critical).
    #[test]
    fn na_gray_is_muted_and_distinct_from_crimson() {
        assert_eq!(NA_GRAY, egui::Color32::from_rgb(0x8A, 0x8A, 0x94));
        assert_ne!(NA_GRAY, CRIMSON);
    }

    /// (a) `build_style` returns the dark-slate Style: SLATE panel and
    /// window fills, CYAN accents, and a panel fill distinct from the
    /// stock `Visuals::dark()` default.
    #[test]
    fn build_style_returns_dark_slate_style() {
        let style = build_style();
        assert_eq!(style.visuals.panel_fill, SLATE);
        assert_eq!(style.visuals.window_fill, SLATE);
        assert_eq!(style.visuals.hyperlink_color, CYAN);
        assert_ne!(style.visuals.panel_fill, egui::Visuals::dark().panel_fill);
    }

    /// (b) `export_json` on a live snapshot + terminal grid writes a
    /// file that parses back as a JSON object with the snapshot's
    /// eight wire keys (the seven Cycle 6 telemetry keys: cpu / amd /
    /// intel / spd / platform / total_capacity / dimm_sizes — plus
    /// the C6-29 `bench` key), with the telemetry sub-object
    /// byte-compatible with the pre-C6-29 file and the `bench` key
    /// round-tripping into an equal `BenchmarkGrid`; the whole file
    /// still re-parses into an equal `SystemMemoryTelemetry` (the
    /// wire root ignores the extra `bench` key).
    #[test]
    fn export_json_writes_parseable_snapshot() {
        let telemetry = ramsleuth_telemetry::collect();
        let grid = fixture_grid();
        let path = temp_path("export.json");
        export_json(&telemetry, Some(&grid), &path)
            .expect("export_json must not fail for a writable temp path");

        let text = std::fs::read_to_string(&path).expect("the exported file must exist");
        let value: serde_json::Value = serde_json::from_str(&text).expect("export must be valid JSON");
        assert!(value.is_object(), "a snapshot must serialize to a JSON object");
        let keys = value
            .as_object()
            .expect("checked is_object")
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        let expected: std::collections::BTreeSet<String> = [
            "amd", "bench", "cpu", "intel", "spd", "platform", "total_capacity", "dimm_sizes",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        assert_eq!(keys, expected);

        // The C6-29 `bench` key carries the grid verbatim.
        let back_grid: BenchmarkGrid =
            serde_json::from_value(value["bench"].clone()).expect("bench must re-parse into the grid");
        assert_eq!(back_grid, grid);

        // The telemetry sub-object is byte-compatible: every one of
        // the seven wire keys matches the pre-C6-29 serialization.
        let base = serde_json::to_value(&telemetry).expect("the wire root must serialize");
        for key in ["cpu", "amd", "intel", "spd", "platform", "total_capacity", "dimm_sizes"] {
            assert_eq!(value[key], base[key], "the {key} key must stay byte-compatible");
        }

        // The wire root is serde round-trip safe (P3-06); the extra
        // `bench` key is ignored on the way back.
        let back: SystemMemoryTelemetry =
            serde_json::from_str(&text).expect("the file must re-parse into the wire root");
        assert_eq!(back, telemetry);

        let _ = std::fs::remove_file(&path);
    }

    /// (c) `snapshot_png` on a small grid writes a VALID PNG: the
    /// 8-byte signature, non-empty, and a 640×420 RGB8 card per the
    /// layout (no telemetry — the header lines render the honest N/A).
    #[test]
    fn snapshot_png_writes_valid_png() {
        let path = temp_path("snapshot.png");
        snapshot_png(&fixture_grid(), None, &path)
            .expect("snapshot_png must not fail for a writable temp path");

        let bytes = std::fs::read(&path).expect("the snapshot file must exist");
        assert!(!bytes.is_empty(), "the snapshot must be non-empty");
        assert_eq!(&bytes[..8], &PNG_SIG, "the file must start with the PNG signature");

        let reader = png::Decoder::new(&bytes[..])
            .read_info()
            .expect("a valid PNG has a header");
        let info = reader.info();
        assert_eq!((info.width, info.height), (CARD_W as u32, CARD_H as u32));
        assert_eq!(info.color_type, png::ColorType::Rgb);

        let _ = std::fs::remove_file(&path);
    }

    /// Unmeasured cells (0.0, the wire's `N/A`) still produce a valid
    /// PNG — a mixed grid with whole rows unmeasured.
    #[test]
    fn snapshot_png_handles_unmeasured_cells() {
        let grid = BenchmarkGrid {
            read_gbps: [26.0, 0.0, 0.0, 0.0],
            write_gbps: [0.0; 4],
            copy_gbps: [0.0; 4],
            latency_ns: [86.0, 1.1, 4.0, 13.9],
        };
        let path = temp_path("snapshot-na.png");
        snapshot_png(&grid, None, &path).expect("a mixed grid must encode");
        let bytes = std::fs::read(&path).expect("the snapshot file must exist");
        assert!(!bytes.is_empty());
        assert_eq!(&bytes[..8], &PNG_SIG);
        let _ = std::fs::remove_file(&path);
    }

    /// The cell ramp: unmeasured / non-finite cells render dim; the
    /// endpoints are exact (t = 0 → CYAN, t = 1 → CRIMSON); mid values
    /// land strictly between them.
    #[test]
    fn cell_color_tracks_the_cyan_amber_crimson_ramp() {
        assert_eq!(cell_color(0.0), DIM_CELL);
        assert_eq!(cell_color(-3.0), DIM_CELL);
        assert_eq!(cell_color(f64::NAN), DIM_CELL);
        assert_eq!(cell_color(1e-6), CYAN);
        assert_eq!(cell_color(2000.0), CRIMSON);
        let mid = cell_color(100.0);
        assert_ne!(mid, CYAN);
        assert_ne!(mid, CRIMSON);
        assert_ne!(mid, DIM_CELL);
    }

    /// `export_json` maps a write failure to `GuiError::Io` (the
    /// no-panic contract: a missing parent dir is an error, not a panic).
    #[test]
    fn export_json_maps_write_failure_to_io() {
        let telemetry = ramsleuth_telemetry::collect();
        let path = temp_path("no-such-dir.json");
        let err = export_json(&telemetry, None, &path.join("missing-dir")).expect_err("a missing parent dir must fail");
        assert!(matches!(err, GuiError::Io(_)), "a missing parent dir must map to Io, got {err:?}");
        let _ = std::fs::remove_file(&path);
    }

    /// (b2) `bench` is `None` (no run has completed yet) → the
    /// `bench` key is the JSON `null`: the snapshot shape stays stable
    /// and parseable, the telemetry sub-object is untouched, never a
    /// panic.
    #[test]
    fn export_json_bench_none_is_null() {
        let telemetry = ramsleuth_telemetry::collect();
        let path = temp_path("export-nobench.json");
        export_json(&telemetry, None, &path)
            .expect("export_json must not fail for a writable temp path");

        let text = std::fs::read_to_string(&path).expect("the exported file must exist");
        let value: serde_json::Value = serde_json::from_str(&text).expect("export must be valid JSON");
        assert!(value.is_object(), "a snapshot must serialize to a JSON object");
        assert!(
            value["bench"].is_null(),
            "no run yet must export `bench: null`, got: {}",
            value["bench"]
        );

        let back: SystemMemoryTelemetry =
            serde_json::from_str(&text).expect("the file must re-parse into the wire root");
        assert_eq!(back, telemetry);

        let _ = std::fs::remove_file(&path);
    }

    /// (b3) The all-`N/A` grid (every cell `0.0` — the wire's
    /// unmeasured marker) exports as-is: the `bench` object
    /// round-trips into the same zero grid, never a panic.
    #[test]
    fn export_json_all_na_grid_exports_as_is() {
        let telemetry = ramsleuth_telemetry::collect();
        let grid = BenchmarkGrid {
            read_gbps: [0.0; 4],
            write_gbps: [0.0; 4],
            copy_gbps: [0.0; 4],
            latency_ns: [0.0; 4],
        };
        let path = temp_path("export-nagrid.json");
        export_json(&telemetry, Some(&grid), &path)
            .expect("export_json must not fail for a writable temp path");

        let text = std::fs::read_to_string(&path).expect("the exported file must exist");
        let value: serde_json::Value = serde_json::from_str(&text).expect("export must be valid JSON");
        let back_grid: BenchmarkGrid =
            serde_json::from_value(value["bench"].clone()).expect("bench must re-parse into the grid");
        assert_eq!(back_grid, grid, "the all-N/A grid must export as-is");

        let _ = std::fs::remove_file(&path);
    }

    /// `GuiError` implements the required traits: `Clone`/`PartialEq`
    /// on the `Io` arm by kind + message, `Display` text, and
    /// `source()` on the `Io` arm only.
    #[test]
    fn gui_error_traits() {
        let a = GuiError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "gone"));
        let b = a.clone();
        assert_eq!(a, b, "clone must preserve kind + message");
        assert_eq!(a, GuiError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "gone")));
        assert_ne!(a, GuiError::Io(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "gone")));
        assert_ne!(a, GuiError::Png("x".to_owned()));
        assert_eq!(a.to_string(), "export I/O error: gone");
        assert_eq!(GuiError::Png("bad idat".into()).to_string(), "PNG encode error: bad idat");
        assert_eq!(GuiError::Json("bad json".into()).to_string(), "JSON serialize error: bad json");
        assert!(std::error::Error::source(&a).is_some(), "the Io arm must expose its source");
        assert!(std::error::Error::source(&GuiError::Png("x".into())).is_none());
    }

    /// A header-line snapshot fixture (the C6-20 style: one populated
    /// AMD CPU; the clock / capacity / DIMM cells are test-configured,
    /// the remaining platform cells Na by default).
    fn fixture_telemetry(
        clock: Section<f64>,
        total: Section<f64>,
        dimms: Vec<Section<f64>>,
    ) -> SystemMemoryTelemetry {
        SystemMemoryTelemetry {
            cpu: CpuInfo { vendor: CpuVendor::Amd(AmdZen::Zen3), brand: "Ryzen 9 5950X".to_owned() },
            amd: Section::na(NaReason::NotApplicable),
            intel: Section::na(NaReason::NotApplicable),
            spd: Vec::new(),
            platform: SystemPlatform {
                cpu_clock_mhz: clock,
                motherboard: Section::na(NaReason::NotApplicable),
                bios: Section::na(NaReason::NotApplicable),
                agesa: Section::na(NaReason::NotApplicable),
                smu_version: Section::na(NaReason::NotApplicable),
            },
            total_capacity: total,
            dimm_sizes: dimms,
        }
    }

    /// (c2) The card's header lines (C6-28): the CPU line is brand +
    /// clock (MHz → GHz, two decimals), the RAM line is total capacity
    /// (one-decimal GB) + the channel mode from the bound-DIMM count.
    #[test]
    fn snapshot_lines_populated() {
        let t = fixture_telemetry(
            Section::Value(3600.0),
            Section::Value(32.0),
            vec![Section::Value(16.0), Section::Value(16.0)],
        );
        assert_eq!(snapshot_cpu_line(&t), "CPU: Ryzen 9 5950X @ 3.60 GHz");
        assert_eq!(snapshot_ram_line(&t), "RAM: 32.0 GB | Dual-Channel");
    }

    /// (c2) Absent cells degrade to the honest `N/A` text (no-panic
    /// contract): an Na clock, an Na capacity, zero bound DIMMs.
    #[test]
    fn snapshot_lines_degrade_to_na() {
        let t =
            fixture_telemetry(Section::na(NaReason::NotApplicable), Section::na(NaReason::NotApplicable), Vec::new());
        assert_eq!(snapshot_cpu_line(&t), "CPU: Ryzen 9 5950X @ N/A");
        assert_eq!(snapshot_ram_line(&t), "RAM: N/A | N/A");
    }

    /// (c2) The channel-mode map (the C6-20 rule): 1 / 2 / 4 →
    /// Single- / Dual- / Quad-Channel, any other count → `N/A`.
    #[test]
    fn channel_name_maps_dimm_counts() {
        assert_eq!(channel_name(1), "Single-Channel");
        assert_eq!(channel_name(2), "Dual-Channel");
        assert_eq!(channel_name(4), "Quad-Channel");
        assert_eq!(channel_name(0), "N/A");
        assert_eq!(channel_name(3), "N/A");
    }

    /// (c2) The wall clock: known epoch seconds format to their UTC
    /// `YYYY-MM-DD HH:MM:SS` — including the pre-epoch count (the
    /// honest 1969 rendering), never a panic.
    #[test]
    fn wall_clock_string_formats_utc() {
        assert_eq!(wall_clock_string(0), "1970-01-01 00:00:00");
        assert_eq!(wall_clock_string(-1), "1969-12-31 23:59:59");
        assert_eq!(wall_clock_string(946_684_800), "2000-01-01 00:00:00");
        assert_eq!(wall_clock_string(1_789_439_400), "2026-09-15 02:30:00");
    }

    /// (c2) The live wall clock is a plausible post-2020 / pre-2100
    /// second count (the app shell's `unix_timestamp` test precedent).
    #[test]
    fn now_epoch_secs_is_plausible() {
        let secs = now_epoch_secs();
        assert!(secs > 1_577_836_800, "expected a post-2020 count, got: {secs}");
        assert!(secs < 4_102_444_800, "expected a pre-2100 count, got: {secs}");
    }

    /// (c2) The card layout (C6-28): a 640×420 RGB8 card with the
    /// CYAN title glyph at the top-left margin, the SLATE gap between
    /// the separator and the grid, the DIM_CELL separator line, and a
    /// measured grid cell painted over the background.
    #[test]
    fn snapshot_card_layout_header_and_grid() {
        let t = fixture_telemetry(
            Section::Value(3600.0),
            Section::Value(32.0),
            vec![Section::Value(16.0), Section::Value(16.0)],
        );
        let path = temp_path("snapshot-card.png");
        snapshot_png(&fixture_grid(), Some(&t), &path)
            .expect("the card must not fail for a writable temp path");
        let bytes = std::fs::read(&path).expect("the card file must exist");
        assert_eq!(&bytes[..8], &PNG_SIG);

        let mut reader =
            png::Decoder::new(&bytes[..]).read_info().expect("a valid PNG has a header");
        let info = reader.info();
        assert_eq!((info.width, info.height), (CARD_W as u32, CARD_H as u32));
        assert_eq!(info.color_type, png::ColorType::Rgb);
        let mut img = vec![0u8; reader.output_buffer_size()];
        reader.next_frame(&mut img).expect("the card payload must decode");

        let slate = (SLATE.r(), SLATE.g(), SLATE.b());
        // The title's first glyph (the `R` of `RamSleuth`) sits at the
        // top-left margin (the 2×-scaled 5×7 font): its top row leaves
        // the corner pixel inkless, but row 1 column 0 and the top
        // row's column 1 are lit CYAN.
        assert_eq!(pixel(&img, 16, 16), slate, "the glyph corner is inkless");
        assert_eq!(
            pixel(&img, 16, 18),
            (CYAN.r(), CYAN.g(), CYAN.b()),
            "the title glyph must be CYAN"
        );
        assert_eq!(
            pixel(&img, 18, 16),
            (CYAN.r(), CYAN.g(), CYAN.b()),
            "the title glyph must be CYAN"
        );
        // The gap between the separator and the grid stays SLATE...
        assert_eq!(pixel(&img, 320, 95), slate, "the header/grid gap must be SLATE");
        // ...the separator line renders DIM_CELL...
        assert_eq!(
            pixel(&img, 320, SEPARATOR_Y),
            (DIM_CELL.r(), DIM_CELL.g(), DIM_CELL.b())
        );
        // ...and a measured grid cell is painted (not the background).
        assert_ne!(pixel(&img, 86, 132), slate, "a measured cell must be painted");
        let _ = std::fs::remove_file(&path);
    }

    /// One decoded card pixel (RGB8) as a channel triple.
    fn pixel(img: &[u8], x: usize, y: usize) -> (u8, u8, u8) {
        let i = (y * CARD_W + x) * 3;
        (img[i], img[i + 1], img[i + 2])
    }
}
