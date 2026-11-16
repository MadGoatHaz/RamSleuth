//! The window-embed icon (C21-26): the embedded hicolor master, decoded
//! once at startup; a decode failure degrades to eframe's default icon
//! (no-panic, D5).

use std::sync::Arc;

/// The window-embed icon (C21-26): the C21-25 committed hicolor
/// master — 256×256 RGBA8 (covers any title-bar DPI).
pub const ICON_PNG: &[u8] = include_bytes!("../../../assets/icons/ramsleuth-256.png");
/// The icon decode's dimension cap (no-panic, D5): a corrupt PNG
/// header must not drive an unbounded RGBA allocation.
pub const MAX_ICON_DIM: u32 = 1024;

/// Decode an RGBA8 PNG payload into native window-icon data (the egui
/// [`IconData`] the root + the Graphs child viewport carry — C21-26).
/// Any failure — a bad signature, a corrupt frame, a payload that is
/// not 8-bit-per-sample RGBA, a dimension over [`MAX_ICON_DIM`] —
/// returns `None` (the no-panic degradation, plan D5: the app runs
/// with eframe's default icon).
pub fn decode_icon_png(bytes: &[u8]) -> Option<egui::IconData> {
    let mut reader = png::Decoder::new(bytes).read_info().ok()?;
    let info = reader.info();
    let (width, height) = (info.width, info.height);
    if width > MAX_ICON_DIM || height > MAX_ICON_DIM {
        return None;
    }
    // The icon data must be 8-bit-per-sample RGBA (`IconData`'s
    // layout); the committed master is (the C21-25 generator).
    let (color, depth) = reader.output_color_type();
    if color != png::ColorType::Rgba || depth != png::BitDepth::Eight {
        return None;
    }
    let mut rgba = vec![0u8; reader.output_buffer_size()];
    reader.next_frame(&mut rgba).ok()?;
    Some(egui::IconData {
        rgba,
        width,
        height,
    })
}

/// The app's window icon (C21-26): the embedded master — decoded
/// once at startup (`main()`), then shared by the root viewport and
/// the Graphs child (via the app). A decode failure degrades to
/// `None` — eframe's default icon — with a single note (no-panic,
/// D5: never a startup crash).
pub fn window_icon() -> Option<Arc<egui::IconData>> {
    match decode_icon_png(ICON_PNG) {
        Some(icon) => Some(Arc::new(icon)),
        None => {
            eprintln!("ramsleuth: icon decode failed; using eframe's default");
            None
        }
    }
}
