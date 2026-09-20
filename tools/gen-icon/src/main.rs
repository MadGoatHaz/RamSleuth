//! RamSleuth application icon generator (chunk C21-25).
//!
//! Renders the "RAM sleuth" mark — a flat cyan magnifier (lens ring + 45°
//! handle) inspecting a light DIMM silhouette (5 chip cut-outs + an amber
//! edge connector with a key notch) on a full-bleed dark-slate squircle —
//! as the freedesktop hicolor PNG set (16/24/32/48/64/128/256/512) plus
//! the 256 px window-embed master, `assets/icons/ramsleuth-256.png`
//! (C21-26's `include_bytes!` source).
//!
//! Design of record: `plans/PLAN-CYCLE21-STREAMLINE-INSTALL.md` §7.2 — a
//! single parametric reference layout in 512 space, scaled by
//! `size / 512` per target. Every primitive is a signed-distance test
//! (filled circle, rounded rect, thick capsule) evaluated at 4×4
//! supersampled subpixel centres and box-averaged to RGBA8, so edges stay
//! smooth at every size (the 16 px read-through gate) and the output is
//! fully deterministic: pure arithmetic, and the `png` encoder writes no
//! timestamps or other metadata → repeated runs are byte-identical.
//!
//! No-panic contract: every fallible step (directory creation, write,
//! encode, post-write decode self-check) degrades to an `Err` surfaced on
//! stderr with exit 1; a failed run never reports success.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Target sizes, freedesktop hicolor order (plan §7.2 deliverables).
const SIZES: [usize; 8] = [16, 24, 32, 48, 64, 128, 256, 512];

/// Supersampling factor: each output pixel is SS×SS SDF tests.
const SS: u32 = 4;

/// Reference space of the parametric layout (plan §7.2).
const REF: f64 = 512.0;

// Palette — the verified GUI palette (style.rs L37–48); no new colors.
const SLATE: (u8, u8, u8) = (0x1E, 0x1E, 0x24); // canvas / chips / notch
const CYAN: (u8, u8, u8) = (0x00, 0xD4, 0xFF); // lens ring + handle
const LIGHT: (u8, u8, u8) = (0xE6, 0xE6, 0xEC); // DIMM body
const AMBER: (u8, u8, u8) = (0xFF, 0xB3, 0x00); // edge connector

// Reference layout, 512 space (plan §7.2, as specified).
/// Canvas: full-bleed square, corner radius 115 (≈ 22.5 % of the side).
const CANVAS_C: f64 = 256.0;
const CANVAS_HW: f64 = 256.0;
const CANVAS_R: f64 = 115.0;
/// DIMM body: rounded rect (160, 250)–(440, 358), radius 14.
const BODY_CX: f64 = 300.0;
const BODY_CY: f64 = 304.0;
const BODY_HW: f64 = 140.0;
const BODY_HH: f64 = 54.0;
const BODY_R: f64 = 14.0;
/// Chips: 5 rounded rects 32×36 with 16 px gutters (the §7.2 reference
/// is 40×52 / 10 px), inset 12 px from the body top (y 262–298), centred
/// on the body (x 188–412, 48 px pitch). Rebalanced per the plan's
/// explicit rule that the 16-px read-through is the gate, not the
/// reference numbers: at 1/32 scale the 86 %-slate 40×52 band erased
/// the light body entirely; the 33 %-light mix keeps the chip band
/// readable as a midtone against the slate canvas at 16 px.
const CHIP_HW: f64 = 16.0;
const CHIP_HH: f64 = 18.0;
const CHIP_R: f64 = 5.0;
const CHIP_CY: f64 = 280.0;
const CHIP_CXS: [f64; 5] = [204.0, 252.0, 300.0, 348.0, 396.0];
/// Edge connector: the amber bottom 14 px of the body (y 344–358); its
/// corners inherit the body rounding through the body clip.
const CONN_Y0: f64 = 344.0;
const CONN_Y1: f64 = 358.0;
/// Key notch: 26×14 slate slot in the visible amber gap — centred at
/// x 314 (x 301–327). The §7.2 reference position (x 218–244) is fully
/// occluded by the lens ring's bottom arc in the reference geometry
/// itself (the ring reaches y 357 at x 240); the plan's rebalance rule
/// (16-px read-through is the gate, not the numbers) moves it into the
/// clear strip between the ring (right edge x ≤ 297) and the handle
/// (left edge x ≥ ~320) where it stays visible at 512.
const NOTCH_CX: f64 = 314.0;
const NOTCH_CY: f64 = 351.0;
const NOTCH_HW: f64 = 13.0;
const NOTCH_HH: f64 = 7.0;
/// Lens: circle centre (240, 225), outer radius 132, ring 36 thick; the
/// interior stays transparent (canvas + DIMM show through).
const LENS_CX: f64 = 240.0;
const LENS_CY: f64 = 225.0;
const LENS_RO: f64 = 132.0;
const LENS_RI: f64 = 96.0;
/// Handle: capsule width 48 from the lens rim toward (432, 424).
const HANDLE_END: (f64, f64) = (432.0, 424.0);
const HANDLE_HW: f64 = 24.0;

/// Signed distance to a circle (negative inside).
fn circle_sdf(x: f64, y: f64, cx: f64, cy: f64, r: f64) -> f64 {
    (x - cx).hypot(y - cy) - r
}

/// Signed distance to a rounded rect centred at (cx, cy) (negative inside).
fn rrect_sdf(x: f64, y: f64, cx: f64, cy: f64, hw: f64, hh: f64, r: f64) -> f64 {
    let qx = (x - cx).abs() - (hw - r);
    let qy = (y - cy).abs() - (hh - r);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.min(0.0).max(qy.min(0.0)) - r
}

/// Signed distance to a capsule from (ax, ay) to (bx, by), half-width hw.
fn capsule_sdf(x: f64, y: f64, ax: f64, ay: f64, bx: f64, by: f64, hw: f64) -> f64 {
    let px = x - ax;
    let py = y - ay;
    let dx = bx - ax;
    let dy = by - ay;
    let t = ((px * dx + py * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    (px - t * dx).hypot(py - t * dy) - hw
}

/// Handle start: the lens-rim point along the direction toward the
/// handle end (plan §7.2: "from the lens rim at 135° toward (432, 424)").
fn handle_start() -> (f64, f64) {
    let dx = HANDLE_END.0 - LENS_CX;
    let dy = HANDLE_END.1 - LENS_CY;
    let len = dx.hypot(dy);
    (LENS_CX + dx / len * LENS_RO, LENS_CY + dy / len * LENS_RO)
}

/// Top-to-bottom composite of the reference layout: the first primitive
/// containing the point wins (flat design; the only blending is the
/// supersample box average in `render`).
fn color_at(x: f64, y: f64, hx: f64, hy: f64) -> [u8; 4] {
    // Handle (drawn over the ring at the junction).
    if capsule_sdf(x, y, hx, hy, HANDLE_END.0, HANDLE_END.1, HANDLE_HW) <= 0.0 {
        return [CYAN.0, CYAN.1, CYAN.2, 255];
    }
    // Lens ring: inside the outer circle, outside the inner one.
    if circle_sdf(x, y, LENS_CX, LENS_CY, LENS_RO) <= 0.0
        && circle_sdf(x, y, LENS_CX, LENS_CY, LENS_RI) > 0.0
    {
        return [CYAN.0, CYAN.1, CYAN.2, 255];
    }
    // Key notch (slate) over the connector.
    if rrect_sdf(x, y, NOTCH_CX, NOTCH_CY, NOTCH_HW, NOTCH_HH, 0.0) <= 0.0 {
        return [SLATE.0, SLATE.1, SLATE.2, 255];
    }
    // Edge connector: the amber band clipped to the body.
    if (CONN_Y0..=CONN_Y1).contains(&y)
        && rrect_sdf(x, y, BODY_CX, BODY_CY, BODY_HW, BODY_HH, BODY_R) <= 0.0
    {
        return [AMBER.0, AMBER.1, AMBER.2, 255];
    }
    // Chips: slate cut-outs (canvas colour) over the body.
    for &cx in &CHIP_CXS {
        if rrect_sdf(x, y, cx, CHIP_CY, CHIP_HW, CHIP_HH, CHIP_R) <= 0.0 {
            return [SLATE.0, SLATE.1, SLATE.2, 255];
        }
    }
    // DIMM body.
    if rrect_sdf(x, y, BODY_CX, BODY_CY, BODY_HW, BODY_HH, BODY_R) <= 0.0 {
        return [LIGHT.0, LIGHT.1, LIGHT.2, 255];
    }
    // Canvas squircle.
    if rrect_sdf(x, y, CANVAS_C, CANVAS_C, CANVAS_HW, CANVAS_HW, CANVAS_R) <= 0.0 {
        return [SLATE.0, SLATE.1, SLATE.2, 255];
    }
    [0, 0, 0, 0]
}

/// Round a box average of 8-bit channel values (already in 0..=255).
fn round8(v: f64) -> u8 {
    v.clamp(0.0, 255.0).round() as u8
}

/// Render one target size: the reference layout scaled by `size / REF`,
/// SDF-tested at SS×SS subpixel centres, box-averaged to RGBA8.
fn render(size: usize, hx: f64, hy: f64) -> Vec<[u8; 4]> {
    let inv = REF / size as f64;
    let sub = 1.0 / f64::from(SS);
    let n = f64::from(SS * SS);
    let mut out = Vec::with_capacity(size * size);
    for py in 0..size {
        for px in 0..size {
            let mut acc = [0.0f64; 4];
            for sy in 0..SS {
                let y = (py as f64 + (sy as f64 + 0.5) * sub) * inv;
                for sx in 0..SS {
                    let x = (px as f64 + (sx as f64 + 0.5) * sub) * inv;
                    let c = color_at(x, y, hx, hy);
                    acc[0] += c[0] as f64;
                    acc[1] += c[1] as f64;
                    acc[2] += c[2] as f64;
                    acc[3] += c[3] as f64;
                }
            }
            out.push([
                round8(acc[0] / n),
                round8(acc[1] / n),
                round8(acc[2] / n),
                round8(acc[3] / n),
            ]);
        }
    }
    out
}

/// Encode one RGBA8 frame; the `png` crate writes signature + IHDR + IDAT
/// only — no timestamps/metadata, so identical input → identical bytes.
fn encode_png(size: usize, pixels: &[[u8; 4]]) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(size * size * 4);
    {
        let mut encoder = png::Encoder::new(&mut out, size as u32, size as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|e| format!("PNG header: {e}"))?;
        let flat: Vec<u8> = pixels
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], p[3]])
            .collect();
        writer
            .write_image_data(&flat)
            .map_err(|e| format!("PNG data: {e}"))?;
    }
    Ok(out)
}

/// Create the parent directory (if any) and write the file.
fn write_png(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    fs::write(path, bytes).map_err(|e| format!("write {}: {e}", path.display()))
}

/// Post-write self-check (plan §7.2): re-decode every file written —
/// valid PNG signature, expected dimensions, RGBA8 at 8 bits.
fn verify(path: &Path, expected: usize) -> Result<(), String> {
    let bytes = fs::read(path).map_err(|e| format!("re-read {}: {e}", path.display()))?;
    let decoder = png::Decoder::new(&bytes[..]);
    let mut reader = decoder
        .read_info()
        .map_err(|e| format!("{}: not a decodable PNG: {e}", path.display()))?;
    let info = reader.info();
    if (info.width, info.height) != (expected as u32, expected as u32) {
        return Err(format!(
            "{}: dimensions {}x{}, expected {}x{}",
            path.display(),
            info.width,
            info.height,
            expected,
            expected
        ));
    }
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err(format!(
            "{}: not RGBA8 (color_type={:?}, bit_depth={:?})",
            path.display(),
            info.color_type,
            info.bit_depth
        ));
    }
    let mut buf = vec![0u8; info.bytes_per_pixel() * info.width as usize * info.height as usize];
    let out = reader
        .next_frame(&mut buf)
        .map_err(|e| format!("{}: decode failed: {e}", path.display()))?;
    if out.buffer_size() != buf.len() {
        return Err(format!(
            "{}: frame size {} != expected buffer {}",
            path.display(),
            out.buffer_size(),
            buf.len()
        ));
    }
    Ok(())
}

/// The repository root (this crate lives at `tools/gen-icon/`).
fn repo_root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(Path::parent)
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn run(root: &Path) -> Result<(), String> {
    let (hx, hy) = handle_start();

    // hicolor set: assets/icons/hicolor/<size>/apps/ramsleuth.png.
    for &size in &SIZES {
        let path = root
            .join("assets")
            .join("icons")
            .join("hicolor")
            .join(size.to_string())
            .join("apps")
            .join("ramsleuth.png");
        let bytes = encode_png(size, &render(size, hx, hy))?;
        write_png(&path, &bytes)?;
        verify(&path, size)?;
        println!("ok  {size:>3}  {}", path.display());
    }

    // Window-embed master (C21-26's include_bytes! source): the 256 px
    // render, written independently of the hicolor copy.
    let master = root.join("assets").join("icons").join("ramsleuth-256.png");
    let bytes = encode_png(256, &render(256, hx, hy))?;
    write_png(&master, &bytes)?;
    verify(&master, 256)?;
    println!("ok  256  {} (window-embed master)", master.display());

    println!("wrote 9 RGBA8 PNGs (deterministic — re-runs are byte-identical)");
    Ok(())
}

fn main() {
    if let Err(err) = run(&repo_root()) {
        eprintln!("ramsleuth-gen-icon: {err}");
        std::process::exit(1);
    }
}
