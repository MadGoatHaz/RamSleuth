//! GUI semantic style + export helpers (P3-25, Grand Design §3.2).
//!
//! The visual contract of the desktop GUI: the four semantic palette
//! colors (cyan / amber / slate / crimson), the dark-slate
//! [`build_style`], and the two file exports the app shell (P3-30)
//! triggers on F2 / F3:
//!
//! - [`export_json`] — a [`SystemMemoryTelemetry`] snapshot as pretty
//!   JSON (F3).
//! - [`snapshot_png`] — a [`BenchmarkGrid`] as a small
//!   data-visualization PNG (F2): the 4×4 Tier×metric layout, each cell
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

/// Build the dark-slate app style: SLATE window/panel fills, light
/// text, CYAN accents (hover/active strokes, hyperlinks, text cursor,
/// selection). AMBER/CRIMSON are consumed by the zones that render the
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

/// F3: write one telemetry snapshot to `path` as pretty JSON.
///
/// Serializes the reused wire root (`SystemMemoryTelemetry` is
/// serde-derived since P3-06 — no type duplication, D2). A JSON
/// failure maps to [`GuiError::Json`], a file write to [`GuiError::Io`].
pub fn export_json(telemetry: &SystemMemoryTelemetry, path: &Path) -> Result<(), GuiError> {
    let json = serde_json::to_string_pretty(telemetry)
        .map_err(|err| GuiError::Json(err.to_string()))?;
    std::fs::write(path, json).map_err(GuiError::Io)?;
    Ok(())
}

// ---------------------------------------------------------------------
// F2: PNG snapshot of the benchmark grid.
// ---------------------------------------------------------------------

/// One snapshot cell, in pixels.
const CELL: usize = 64;
/// Gutter between cells and margin around the grid, in pixels.
const GUTTER: usize = 8;
const MARGIN: usize = 16;
/// Grid extent: the AIDA64 layout is 4 tiers × 4 metrics, square.
const DIM: usize = MARGIN * 2 + 4 * CELL + 3 * GUTTER; // 312
/// `ln` value at which the color ramp saturates (≈ 999 → 6.9).
const RAMP_FULL: f64 = 6.9;
/// Unmeasured cell (0.0 on the wire): dim slate, distinct from the
/// background.
const DIM_CELL: egui::Color32 = egui::Color32::from_rgb(0x3A, 0x3A, 0x44);

/// F2: encode `grid` as a data-visualization PNG and write it to `path`.
///
/// A 4×4 block of cells — one per [`Tier`] row × [`Metric`] column, the
/// AIDA64 layout — each colored by its value magnitude along the
/// CYAN → AMBER → CRIMSON ramp on a SLATE background (an unmeasured
/// 0.0 cell, the wire's `N/A`, renders dim). Produces a valid RGB8 PNG
/// by design — a small data grid, not a screen capture (the app shell
/// is the only place window pixels come from). An encoder failure maps
/// to [`GuiError::Png`], a file write to [`GuiError::Io`].
pub fn snapshot_png(grid: &BenchmarkGrid, path: &Path) -> Result<(), GuiError> {
    let tiers = [Tier::Memory, Tier::L1, Tier::L2, Tier::L3];
    let metrics = [Metric::Read, Metric::Write, Metric::Copy, Metric::Latency];
    let mut pixels = vec![0u8; DIM * DIM * 3];

    // SLATE background.
    for p in pixels.chunks_exact_mut(3) {
        p[0] = SLATE.r();
        p[1] = SLATE.g();
        p[2] = SLATE.b();
    }
    // One colored cell per Tier × metric.
    for (row, tier) in tiers.iter().enumerate() {
        for (col, metric) in metrics.iter().enumerate() {
            let c = cell_color(grid.cell(*tier, *metric));
            let x0 = MARGIN + col * (CELL + GUTTER);
            let y0 = MARGIN + row * (CELL + GUTTER);
            for y in y0..y0 + CELL {
                for x in x0..x0 + CELL {
                    let idx = (y * DIM + x) * 3;
                    pixels[idx] = c.r();
                    pixels[idx + 1] = c.g();
                    pixels[idx + 2] = c.b();
                }
            }
        }
    }

    let mut out: Vec<u8> = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, DIM as u32, DIM as u32);
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
// Tests (headless: no window, no display).
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Unique temp path per test (pid-scoped — the client's
    /// temp-socket precedent).
    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("ramsleuth-gui-{name}-{}", std::process::id()))
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

    /// (b) `export_json` on a live snapshot writes a file that parses
    /// back as a JSON object with the snapshot's four wire keys, and
    /// round-trips into an equal `SystemMemoryTelemetry`.
    #[test]
    fn export_json_writes_parseable_snapshot() {
        let telemetry = ramsleuth_telemetry::collect();
        let path = temp_path("export.json");
        export_json(&telemetry, &path)
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
        let expected: std::collections::BTreeSet<String> =
            ["amd", "cpu", "intel", "spd"].into_iter().map(str::to_owned).collect();
        assert_eq!(keys, expected);

        // The wire root is serde round-trip safe (P3-06).
        let back: SystemMemoryTelemetry =
            serde_json::from_str(&text).expect("the file must re-parse into the wire root");
        assert_eq!(back, telemetry);

        let _ = std::fs::remove_file(&path);
    }

    /// (c) `snapshot_png` on a small grid writes a VALID PNG: the
    /// 8-byte signature, non-empty, and a 312×312 RGB8 image per the
    /// layout.
    #[test]
    fn snapshot_png_writes_valid_png() {
        let path = temp_path("snapshot.png");
        snapshot_png(&fixture_grid(), &path)
            .expect("snapshot_png must not fail for a writable temp path");

        let bytes = std::fs::read(&path).expect("the snapshot file must exist");
        assert!(!bytes.is_empty(), "the snapshot must be non-empty");
        assert_eq!(&bytes[..8], &PNG_SIG, "the file must start with the PNG signature");

        let reader = png::Decoder::new(&bytes[..])
            .read_info()
            .expect("a valid PNG has a header");
        let info = reader.info();
        assert_eq!((info.width, info.height), (DIM as u32, DIM as u32));
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
        snapshot_png(&grid, &path).expect("a mixed grid must encode");
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
        let err = export_json(&telemetry, &path.join("missing-dir")).expect_err("a missing parent dir must fail");
        assert!(matches!(err, GuiError::Io(_)), "a missing parent dir must map to Io, got {err:?}");
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
}
