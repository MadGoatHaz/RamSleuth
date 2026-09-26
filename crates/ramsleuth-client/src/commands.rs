//! The `bench` + `status` + `probe` + `burn` commands over the shared
//! transport (P3-20; `probe` / `burn` added for the TUI `[F]` / `[X]` +
//! GUI parity).
//!
//! [`bench`] starts a streamed benchmark run: one `StartBenchmark`
//! request, then the daemon's reply stream on the same connection (the
//! P3-11 incremental contract — the leftover buffer survives across
//! `recv` calls): a `BenchStarted` ack, one `BenchProgress` per
//! completed bandwidth cell, and exactly one terminal frame
//! (`BenchResult` / `BenchCancelled` / `Error`). Every event prints as
//! it arrives (a `Full` run streams 12 progress lines over minutes —
//! the user watches it live), and the terminal [`BenchmarkGrid`]
//! renders through the pure [`render_grid`] (the AIDA64-style 4×4
//! table: tier rows × Read/Write/Copy/Latency columns, GB/s + ns/hop
//! per the Phase 1 unit convention, an unmeasured cell — `0.0` on the
//! wire, the grid carries no Na arm — prints `N/A`).
//!
//! [`status`] is the one-RPC health summary: `GetTelemetry` → one line
//! per major section (`CPU` / `AMD` / `Intel` / `SPD`) saying whether
//! the section holds live data (`ok`) or degraded to `N/A (<reason>)`,
//! plus the daemon socket line.
//!
//! [`probe`] fetches the consent-gated "Submit Probe Report" (one
//! `GetProbeReport` round trip): the `ProbeReport` reply renders to the
//! chunk-2 markdown (the telemetry crate's `render_probe_report_md` —
//! the same document the TUI `[w]` and the GUI preview carry) and is
//! written to `~/.ramsleuth/probe-report.md` (the `$HOME` rule with the
//! TUI's CWD fallback — [`default_probe_report_path`]) by default,
//! printed to stdout with `--stdout`, or emitted as the raw `ProbeReport`
//! JSON with `--json`.
//!
//! [`burn`] starts a burn-in soak (one `StartBurnIn { Full,
//! duration_minutes }` — the daemon's single-flight run class, D-1):
//! the `BenchStarted { run_id }` ack prints the confirmation (the
//! duration, the run id) + how the run stops (itself at the finite
//! deadline, or via the daemon's `CancelBenchmark` from any client —
//! the TUI `[C]` key / the GUI cancel), then the command exits — the
//! daemon-side run keeps going without its owner (the TUI-close
//! precedent), so the CLI need not sit on the tick stream.
//!
//! **Print + return:** every command prints its output and returns the
//! exact same `String` — the unit tests assert on the returned text
//! (capturing this process's own stdout in a test is not reliable: std
//! dups fd 1 on first use). The P3-21 bin ignores the return value.

use std::path::{Path, PathBuf};

use ramsleuth_bench::{BenchmarkGrid, BenchOp, Metric, StreamProgress, StreamTarget, Tier};
use ramsleuth_protocol::{BenchMode, Request, Response};
use ramsleuth_telemetry::error::Section;
use ramsleuth_telemetry::probe::render_probe_report_md;
use ramsleuth_telemetry::SystemMemoryTelemetry;

use crate::client::{Client, ClientError};

/// The grid's tier row order (the [`Tier`] discriminants, 0..4).
const TIER_ORDER: [Tier; 4] = [Tier::Memory, Tier::L1, Tier::L2, Tier::L3];

/// The grid's metric column order (the [`Metric`] discriminants, 0..4:
/// the three bandwidth columns, then the latency column).
const METRIC_ORDER: [Metric; 4] = [Metric::Read, Metric::Write, Metric::Copy, Metric::Latency];

/// The column headers, in [`METRIC_ORDER`] order.
const METRIC_NAMES: [&str; 4] = ["Read", "Write", "Copy", "Latency"];

/// The display name of a grid tier row (the `Debug` form would do too,
/// but the name is user-facing text — owned here, stable).
fn tier_name(tier: &Tier) -> &'static str {
    match tier {
        Tier::Memory => "Memory",
        Tier::L1 => "L1",
        Tier::L2 => "L2",
        Tier::L3 => "L3",
    }
}

/// The display name of a bandwidth op (the progress-line op label).
fn op_name(op: &BenchOp) -> &'static str {
    match op {
        BenchOp::Read => "Read",
        BenchOp::Write => "Write",
        BenchOp::Copy => "Copy",
    }
}

/// One grid cell as text: an unmeasured value (`0.0` — unrequested
/// cells stay zero on the wire and the grid carries no Na arm) renders
/// `N/A` instead of a bogus zero; the bandwidth cells render GB/s, the
/// latency cell ns/hop (two decimals each, the Phase 1 convention).
fn cell_text(grid: &BenchmarkGrid, tier: &Tier, metric: &Metric) -> String {
    let value = grid.cell(*tier, *metric);
    if value == 0.0 {
        "N/A".to_owned()
    } else if matches!(metric, Metric::Latency) {
        format!("{value:.2} ns/hop")
    } else {
        format!("{value:.2} GB/s")
    }
}

/// Render the 4×4 [`BenchmarkGrid`] (tier rows × Read/Write/Copy/
/// Latency columns) as aligned text.
///
/// Pure: no I/O, no client — the same grid always yields the same
/// `String` (the TUI zone-2 export reuses it). Every line comes out the
/// same width: the tier column pads to the widest tier name, each metric
/// column pads to the wider of its header and its four cells.
///
/// ```text
///         Read         Write        Copy         Latency
/// Memory  512.00 GB/s  410.00 GB/s  455.00 GB/s  88.00 ns/hop
/// L1      897.50 GB/s  823.00 GB/s  851.50 GB/s   1.10 ns/hop
/// L2      402.00 GB/s  311.50 GB/s  349.00 GB/s   3.40 ns/hop
/// L3      198.50 GB/s  152.00 GB/s  176.50 GB/s  12.70 ns/hop
/// ```
///
/// (a partial target prints `N/A` where its cells never ran)
pub fn render_grid(grid: &BenchmarkGrid) -> String {
    // One cell text per (tier row, metric column): built once, reused by
    // the width pass and the output pass.
    let mut cells: [[String; 4]; 4] =
        std::array::from_fn(|_| std::array::from_fn(|_| String::new()));
    for (row, tier) in TIER_ORDER.iter().enumerate() {
        for (col, metric) in METRIC_ORDER.iter().enumerate() {
            cells[row][col] = cell_text(grid, tier, metric);
        }
    }

    // Each metric column's width: the header name vs the column's four
    // cells (all lines pad to these widths, so they come out equal).
    let mut widths = [0usize; 4];
    for (col, name) in METRIC_NAMES.iter().enumerate() {
        widths[col] = name.len();
    }
    for row_cells in &cells {
        for (col, cell) in row_cells.iter().enumerate() {
            widths[col] = widths[col].max(cell.len());
        }
    }
    let tier_width = TIER_ORDER
        .iter()
        .map(|tier| tier_name(tier).len())
        .max()
        .unwrap_or(0);

    let mut out = String::new();
    // The header line: the blank corner cell + the four column names,
    // each gap-first and padded to its column width — the same field
    // shape as the rows, so every line comes out the same width.
    out.push_str(&" ".repeat(tier_width));
    for (col, name) in METRIC_NAMES.iter().enumerate() {
        out.push_str(&format!("  {:<width$}", name, width = widths[col]));
    }
    out.push('\n');
    // One line per tier: the name + the four cells, column by column.
    for (row, tier) in TIER_ORDER.iter().enumerate() {
        out.push_str(&format!("{:<width$}", tier_name(tier), width = tier_width));
        for (col, cell) in cells[row].iter().enumerate() {
            out.push_str(&format!("  {:<width$}", cell, width = widths[col]));
        }
        out.push('\n');
    }
    out
}

/// Start a benchmark run over `client` and stream its progress.
///
/// Sends one `StartBenchmark { target, mode }`, then reads the daemon's
/// reply stream on the same connection until the terminal frame:
///
/// - `BenchStarted { run_id }` → `benchmark started (run {run_id})`;
/// - `BenchProgress` → one live line per completed bandwidth cell,
///   `  [{cell}/{total}] {tier}/{op}: {value} GB/s`;
/// - `BenchResult { grid }` → [`render_grid`] of the terminal grid;
/// - `BenchCancelled { run_id }` → `benchmark cancelled (run {run_id})`;
/// - `Error(msg)` → [`ClientError::Protocol`] with the daemon's text;
/// - `BurnInProgress` → [`ClientError::Protocol`] (a burn-in frame in
///   a normal-bench stream is a contract violation — burn-in ticks
///   stream only on the owning `StartBurnIn` connection, D-1/D-2);
/// - any other response → [`ClientError::Protocol`] (a protocol
///   violation for this request).
///
/// Prints every line as it arrives and returns the full text (what was
/// printed) — `Ok` on a result or a clean cancel, `Err` on a structured
/// error reply or a transport failure (the no-panic contract, D5).
pub fn bench(
    client: &mut Client,
    target: StreamTarget,
    mode: BenchMode,
) -> Result<String, ClientError> {
    client.send(&Request::StartBenchmark { target, mode })?;
    let mut out = String::new();
    loop {
        match client.recv()? {
            Response::BenchStarted { run_id } => {
                let line = format!("benchmark started (run {run_id})");
                println!("{line}");
                out.push_str(&line);
                out.push('\n');
            }
            Response::BenchProgress(progress) => {
                let line = progress_line(&progress);
                println!("{line}");
                out.push_str(&line);
                out.push('\n');
            }
            Response::BenchResult { grid, .. } => {
                let grid_text = render_grid(&grid);
                println!("{grid_text}");
                out.push_str(&grid_text);
                return Ok(out);
            }
            Response::BenchCancelled { run_id } => {
                let line = format!("benchmark cancelled (run {run_id})");
                println!("{line}");
                out.push_str(&line);
                out.push('\n');
                return Ok(out);
            }
            Response::Error(msg) => return Err(ClientError::Protocol(msg)),
            Response::Telemetry(_) => {
                return Err(ClientError::Protocol(
                    "unexpected response during benchmark".to_owned(),
                ))
            }
            Response::BurnInProgress(_) => {
                // A burn-in frame in a normal-bench stream violates the
                // wire contract (burn-in ticks stream only on the
                // owning `StartBurnIn` connection, D-1/D-2); the `burn`
                // subcommand is the separate `StartBurnIn` path.
                return Err(ClientError::Protocol(
                    "unexpected burn-in frame during benchmark".to_owned(),
                ))
            }
            Response::ProbeReport(_) => {
                // A probe-report frame in a normal-bench stream
                // violates the wire contract (probe reports reply only
                // to `GetProbeReport`, chunk probe-1a) — this
                // subcommand only ever starts a `StartBenchmark`.
                return Err(ClientError::Protocol(
                    "unexpected probe report during benchmark".to_owned(),
                ))
            }
        }
    }
}

/// One live progress line: `  [{cell}/{total}] {tier}/{op}: {value} GB/s`
/// (progress events are bandwidth cells only — the latency pass carries
/// none, so the unit is always GB/s).
fn progress_line(progress: &StreamProgress) -> String {
    let tier = tier_name(&progress.tier);
    let op = op_name(&progress.op);
    format!(
        "  [{cell}/{total}] {tier}/{op}: {value:.2} GB/s",
        cell = progress.cell_index,
        total = progress.total_cells,
        tier = tier,
        op = op,
        value = progress.value,
    )
}

/// Fetch the live snapshot and print the per-section summary.
///
/// One `GetTelemetry` round trip over `client`: the `Telemetry` reply
/// renders as one line per major section — `CPU`, `AMD`, `Intel`,
/// `SPD` — each `ok` (the section holds live data: a populated branch /
/// a non-empty module list) or `N/A (<reason>)` (a degraded `Na`
/// branch), plus the daemon socket line. Prints the summary and returns
/// it; a structured `Error` reply maps onto [`ClientError::Protocol`],
/// any other reply is a protocol violation for this request (the
/// no-panic contract, D5).
pub fn status(client: &mut Client) -> Result<String, ClientError> {
    let response = client.request(&Request::GetTelemetry)?;
    match response {
        Response::Telemetry(telemetry) => {
            let summary = summarize(&telemetry, client.socket_path());
            println!("{summary}");
            Ok(summary)
        }
        Response::Error(msg) => Err(ClientError::Protocol(msg)),
        _ => Err(ClientError::Protocol("expected telemetry".to_owned())),
    }
}

/// The one-line-per-section summary of a snapshot (the `status` body).
///
/// CPU is the snapshot root — it always carries the detected vendor +
/// brand, so it is `ok` by construction; AMD / Intel are `ok` when
/// their branch holds a `Value`, `N/A (<reason>)` when degraded; SPD is
/// `ok (n modules)` when the list is non-empty, `N/A (no modules)`
/// otherwise; the final line names the daemon socket.
fn summarize(telemetry: &SystemMemoryTelemetry, socket_path: &Path) -> String {
    let mut out = String::new();
    out.push_str("CPU: ok\n");
    out.push_str(&format!("AMD: {}\n", section_state(&telemetry.amd)));
    out.push_str(&format!("Intel: {}\n", section_state(&telemetry.intel)));
    if telemetry.spd.is_empty() {
        out.push_str("SPD: N/A (no modules)\n");
    } else {
        let modules = telemetry.spd.len();
        let unit = if modules == 1 { "module" } else { "modules" };
        out.push_str(&format!("SPD: ok ({modules} {unit})\n"));
    }
    out.push_str(&format!("daemon: {}\n", socket_path.display()));
    out
}

/// One vendor section's state line: `ok` when the branch holds a
/// `Value`, `N/A (<reason>)` when it degraded (the reason's variant
/// name in its `Debug` form — `DriverMissing`, `UnsupportedHardware`,
/// `InsufficientPrivilege`, …).
fn section_state<T>(section: &Section<T>) -> String {
    match section {
        Section::Value(_) => "ok".to_owned(),
        Section::Na(reason) => format!("N/A ({reason:?})"),
    }
}

// ---------------------------------------------------------------------------
// `probe` — the consent-gated "Submit Probe Report" fetch (the TUI `[F]`
// / GUI Probe parity path).
// ---------------------------------------------------------------------------

/// The default `probe` destination: `~/.ramsleuth/probe-report.md` — the
/// exact `~` location the TUI's `[w]` writes (the `$HOME` rule with the
/// CWD fallback when `HOME` is unset — the GUI F2/F3 rule; never a
/// panic).
pub fn default_probe_report_path() -> PathBuf {
    let home =
        std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join(".ramsleuth").join("probe-report.md")
}

/// The `probe` command: fetch the report and write it to the default
/// location (or print it, per the flags) — see [`probe_to`] for the
/// full contract. This convenience is what the bin calls.
pub fn probe(client: &mut Client, to_stdout: bool, as_json: bool) -> Result<String, ClientError> {
    probe_to(client, to_stdout, as_json, &default_probe_report_path())
}

/// One `GetProbeReport` round trip over `client` (the TUI `[F]`
/// one-shot request class — its own request, not a telemetry poll) and
/// the `ProbeReport` reply, in the requested form:
///
/// - `as_json` → the raw [`ProbeReport`] as pretty-printed JSON,
///   printed to stdout (the machine-readable form — `--json`);
/// - `to_stdout` (without `as_json`) → the chunk-2 markdown (the
///   telemetry crate's [`render_probe_report_md`] — the identical
///   document the TUI `[w]` and the GUI preview carry), printed to
///   stdout with the `report printed to stdout` confirmation on stderr
///   (so the report stays the sole stdout stream) — `--stdout`;
/// - neither → the same markdown written to `path` (the parent dir
///   created as needed — the TUI `[w]` file semantics), the
///   `report written to <path>` confirmation printed to stdout.
///
/// Prints the output and returns the same text (`report written to …`
/// for a file write, the report text itself for a stdout print) — the
/// P3-20 print + return contract. A structured `Error` reply maps onto
/// [`ClientError::Protocol`] with the daemon's text, any other reply is
/// a protocol violation, and a file-write i/o failure is
/// [`ClientError::Io`] (the no-panic contract, D5).
pub fn probe_to(
    client: &mut Client,
    to_stdout: bool,
    as_json: bool,
    path: &Path,
) -> Result<String, ClientError> {
    let response = client.request(&Request::GetProbeReport)?;
    let Response::ProbeReport(report) = response else {
        return match response {
            Response::Error(msg) => Err(ClientError::Protocol(msg)),
            _ => Err(ClientError::Protocol(
                "expected a probe report reply to GetProbeReport".to_owned(),
            )),
        };
    };
    let text = if as_json {
        serde_json::to_string_pretty(&report).map_err(|e| {
            ClientError::Protocol(format!("failed to encode the probe report as JSON: {e}"))
        })?
    } else {
        render_probe_report_md(&report)
    };
    if to_stdout || as_json {
        println!("{text}");
        eprintln!("report printed to stdout");
        Ok(text)
    } else {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(ClientError::Io)?;
        }
        std::fs::write(path, &text).map_err(ClientError::Io)?;
        let line = format!("report written to {}", path.display());
        println!("{line}");
        Ok(line)
    }
}

// ---------------------------------------------------------------------------
// `burn` — the burn-in soak start (the TUI `[X]` / GUI Run Burn-In
// parity path).
// ---------------------------------------------------------------------------

/// The burn-in duration clamp (minutes): the GUI's `0..=1440` range
/// (a `u32` is never negative, so only the upper bound bites; `0` =
/// infinite) — parity with `burn_in_minutes_clamped`.
fn burn_in_minutes_clamped(minutes: u32) -> u32 {
    const BURN_IN_MINUTES_MAX: u32 = 1440;
    minutes.min(BURN_IN_MINUTES_MAX)
}

/// Start a burn-in soak (the daemon's multi-pass duration run class,
/// D-1 — the TUI `[X]` / GUI `Run Burn-In` parity path): sends
/// `StartBurnIn { Full, duration_minutes }` (the full 4×4 grid, the
/// same target + the GUI/TUI default-scope burn-in; the minutes are
/// clamped to `0..=1440` as in the GUI, `0` = infinite), reads the
/// daemon's first reply, and exits:
///
/// - `BenchStarted { run_id }` → the confirmation: `burn-in started
///   (run {run_id})` with the full grid + the duration (`0` = infinite
///   — runs until cancelled; a finite `n` — the daemon stops the run
///   itself at the `n × 60 s` deadline), then how to stop it early:
///   the daemon's `CancelBenchmark { run_id }` from *any* client (the
///   TUI `[C]` key / the GUI cancel button) — the run keeps going
///   daemon-side after this command exits (a closed owner does not
///   cancel it — the TUI-close-mid-run precedent);
/// - `Error(msg)` → [`ClientError::Protocol`] with the daemon's text
///   (the single-flight busy case: `benchmark already running`);
/// - any other reply → [`ClientError::Protocol`] (a protocol
///   violation).
///
/// Prints the confirmation and returns it (the P3-20 print + return
/// contract). The CLI does not sit on the tick stream — that is the
/// dashboard frontends' job; `burn` starts the soak, confirms, and
/// exits.
pub fn burn(client: &mut Client, duration_minutes: u32) -> Result<String, ClientError> {
    let duration_minutes = burn_in_minutes_clamped(duration_minutes);
    client.send(&Request::StartBurnIn {
        target: StreamTarget::Full,
        duration_minutes,
    })?;
    let response = client.recv()?;
    let Response::BenchStarted { run_id } = response else {
        return match response {
            Response::Error(msg) => Err(ClientError::Protocol(msg)),
            _ => Err(ClientError::Protocol(
                "expected a burn-in start reply to StartBurnIn".to_owned(),
            )),
        };
    };
    let duration = if duration_minutes == 0 {
        "infinite (0 — the run stops only when cancelled)".to_owned()
    } else {
        let unit = if duration_minutes == 1 { "minute" } else { "minutes" };
        format!("{duration_minutes} {unit} (the daemon stops it at the deadline)")
    };
    let text = format!(
        "burn-in started (run {run_id}) — full 4x4 grid, {duration}\n\
         stop it early: cancel run {run_id} from any client (the TUI [C] key or the \
         GUI cancel button — the daemon's CancelBenchmark); `ramsleuth-client status` \
         shows the daemon's health"
    );
    println!("{text}");
    Ok(text)
}

#[cfg(test)]
mod tests {
    //! Unit tests run against a real **in-process daemon stand-in**
    //! (the P3-18 pattern): a `std::thread` binds a `UnixListener` on a
    //! unique temp socket path (pid-qualified so parallel runs never
    //! collide), asserts the incoming request, and speaks the frozen
    //! P3-10/P3-11 frame protocol back.

    use std::io::{Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::process;
    use std::thread;

    use ramsleuth_protocol::{decode_frame, encode_frame, FrameError, Message};
    use ramsleuth_telemetry::amd_readout::EccStatus;
    use ramsleuth_telemetry::cpuid::{CpuInfo, CpuVendor, IntelGen};
    use ramsleuth_telemetry::error::{NaReason, Section};
    use ramsleuth_telemetry::intel_readout::{decode_channel, IntelReadout};
    use ramsleuth_telemetry::spd_decode::SpdModule;
    use ramsleuth_telemetry::{ProbeReport, ProbeSystem, SystemMemoryTelemetry, SystemPlatform};

    use super::*;

    /// A unique temp socket path owned by a guard that removes the file
    /// on drop (best-effort cleanup; the path is pid-qualified so
    /// parallel test runs / processes never collide).
    struct TempSocket {
        path: PathBuf,
    }

    impl TempSocket {
        fn new(name: &str) -> Self {
            let path = PathBuf::from(format!(
                "/tmp/ramsleuth-commands-{name}-{}.sock",
                process::id()
            ));
            let _ = std::fs::remove_file(&path); // stale file from a crashed earlier run
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempSocket {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    /// Server-side incremental frame reader (the P3-11 contract on the
    /// other end): append every received byte and decode until one
    /// frame is complete (`None` on a clean EOF before a frame).
    fn read_one_message(stream: &mut UnixStream) -> Option<Message> {
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            match decode_frame(&buf) {
                Err(FrameError::Incomplete) => {}
                other => return other.ok().map(|frame| frame.message),
            }
            let n = stream.read(&mut chunk).ok()?;
            if n == 0 {
                return None;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
    }

    /// A scripted stand-in: binds the temp socket, accepts one
    /// connection, asserts the incoming request frame is `expect`, and
    /// writes every canned reply in order. `join` reaps the thread (a
    /// handler panic fails the test instead of hanging it).
    fn spawn_standin(
        sock: &TempSocket,
        expect: Request,
        replies: Vec<Response>,
    ) -> thread::JoinHandle<()> {
        let listener = UnixListener::bind(sock.path()).expect("test socket must bind");
        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                match read_one_message(&mut stream) {
                    Some(Message::Request(req)) => {
                        assert_eq!(req, expect, "stand-in must get the expected request");
                    }
                    other => panic!("stand-in expected a request frame, got {other:?}"),
                }
                for reply in replies {
                    let bytes =
                        encode_frame(&Message::Response(reply)).expect("reply must encode");
                    stream.write_all(&bytes).expect("stand-in write must not fail");
                }
            }
        })
    }

    /// A fully-populated 4×4 grid (every cell measured).
    fn full_grid() -> BenchmarkGrid {
        BenchmarkGrid {
            read_gbps: [512.0, 897.5, 402.0, 198.5],
            write_gbps: [410.0, 823.0, 311.5, 152.0],
            copy_gbps: [455.0, 851.5, 349.0, 176.5],
            latency_ns: [88.0, 1.1, 3.4, 12.7],
        }
    }

    /// A `Cell(Memory, Read)`-shaped partial grid: only the Memory/Read
    /// cell measured, the other 15 zero on the wire (→ `N/A`).
    fn partial_grid() -> BenchmarkGrid {
        let mut grid = BenchmarkGrid {
            read_gbps: [0.0; 4],
            write_gbps: [0.0; 4],
            copy_gbps: [0.0; 4],
            latency_ns: [0.0; 4],
        };
        grid.read_gbps[Tier::Memory as usize] = 512.0;
        grid
    }

    /// A mixed snapshot: an Intel CPU, an `Na` AMD branch (the driver
    /// missing on an Intel host), a populated single-channel Intel
    /// readout (built through the public `decode_channel`), one SPD
    /// module.
    fn mixed_snapshot() -> SystemMemoryTelemetry {
        // MCS_COMMAND_0: tCL / tRCD / tRP / tRAS (ticks).
        let cmd0: u32 = 16 | (16 << 8) | (16 << 16) | (32 << 24);
        // MCS_COMMAND_1: 1N command rate + gear 1 + RTL 6 ticks.
        let cmd1: u32 = 6 << 4;
        // MCS_COMMAND_2: tCCD_S / tCCD_L (ticks).
        let cmd2: u32 = 4 | (12 << 8);
        // MCS_COMMAND_3: tRDRD / tRDWR / tWRWR / tWRRD (ticks).
        let cmd3: u32 = 10 | (8 << 8) | (12 << 16) | (4 << 24);
        SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor: CpuVendor::Intel(IntelGen::AlderLake),
                brand: "Intel Core i7-12700K".to_owned(),
            },
            amd: Section::na(NaReason::DriverMissing),
            intel: Section::Value(IntelReadout {
                channels: vec![decode_channel(
                    0,
                    Some(160),
                    [Some(cmd0), Some(cmd1), Some(cmd2), Some(cmd3)],
                )],
                channel_mode: None,
                ecc_status: EccStatus::Unknown,
            }),
            spd: vec![SpdModule {
                index: 0x50,
                is_ddr5: false,
                maker: Section::Value("0xC1".to_owned()),
                die_maker: Section::Value("SK hynix".to_owned()),
                die_type: Section::na(NaReason::NotApplicable),
                devices: Section::Value(8),
                part: Section::Value("M391A2K40DB".to_owned()),
                serial: Section::Value("S064531ABC".to_owned()),
                rank: Section::Value(1),
                density_mbit: Section::Value(16_384),
                speed_mts: Section::Value(3_200),
                profiles: Vec::new(),
            }],
            platform: SystemPlatform {
                cpu_clock_mhz: Section::Value(3500.0),
                motherboard: Section::Value("Test Board".to_owned()),
                bios: Section::Value("1.0".to_owned()),
                agesa: Section::na(NaReason::NotApplicable),
                smu_version: Section::na(NaReason::NotApplicable),
            },
            total_capacity: Section::Value(16.0),
            dimm_sizes: vec![Section::Value(16.0)],
        }
    }

    /// A fully degraded snapshot: both vendor branches `Na`, no SPD
    /// modules (the driver-less host state).
    fn all_na_snapshot() -> SystemMemoryTelemetry {
        SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor: CpuVendor::Unknown,
                brand: "Mock CPU".to_owned(),
            },
            amd: Section::na(NaReason::DriverMissing),
            intel: Section::na(NaReason::InsufficientPrivilege),
            spd: Vec::new(),
            platform: SystemPlatform {
                cpu_clock_mhz: Section::na(NaReason::NotApplicable),
                motherboard: Section::na(NaReason::NotApplicable),
                bios: Section::na(NaReason::NotApplicable),
                agesa: Section::na(NaReason::NotApplicable),
                smu_version: Section::na(NaReason::NotApplicable),
            },
            total_capacity: Section::na(NaReason::NotApplicable),
            dimm_sizes: Vec::new(),
        }
    }

    /// (a) `bench` over the stand-in: the `StartBenchmark` request goes
    /// out, the `BenchStarted` ack + one progress + the terminal grid
    /// stream back; the returned text carries every printed line
    /// (started, progress, and the rendered grid) and the call is `Ok`.
    #[test]
    fn bench_streams_started_progress_and_grid() {
        let sock = TempSocket::new("bench-ok");
        let progress = StreamProgress {
            cell_index: 0,
            total_cells: 12,
            tier: Tier::Memory,
            op: BenchOp::Read,
            value: 512.0,
            label: "Memory · Read (GB/s)".to_owned(),
        };
        let handle = spawn_standin(
            &sock,
            Request::StartBenchmark {
                target: StreamTarget::Full,
                mode: BenchMode::Full,
            },
            vec![
                Response::BenchStarted { run_id: 1 },
                Response::BenchProgress(progress),
                Response::BenchResult {
                    run_id: 1,
                    grid: full_grid(),
                },
            ],
        );
        let mut client = Client::connect(sock.path()).expect("must connect");
        let text = bench(&mut client, StreamTarget::Full, BenchMode::Full)
            .expect("bench must succeed on a result terminal");
        handle.join().expect("stand-in thread must not panic");

        assert!(text.contains("benchmark started (run 1)"), "started line, got: {text}");
        assert!(
            text.contains("  [0/12] Memory/Read: 512.00 GB/s"),
            "progress line, got: {text}"
        );
        // the terminal grid (header + units + a few cells)
        assert!(text.contains("Read"), "grid header, got: {text}");
        assert!(text.contains("Latency"), "grid header, got: {text}");
        assert!(text.contains("512.00 GB/s"), "grid bandwidth cell, got: {text}");
        assert!(text.contains("88.00 ns/hop"), "grid latency cell, got: {text}");
        assert!(!text.contains("N/A"), "a full grid has no N/A cells, got: {text}");
    }

    /// (b) `bench` on a cancelled run: the stand-in answers
    /// `BenchStarted` + `BenchCancelled`; the returned text carries the
    /// cancel line and the call is `Ok` (a clean cancel is a terminal,
    /// not an error).
    #[test]
    fn bench_cancel_terminal_is_ok() {
        let sock = TempSocket::new("bench-cancel");
        let handle = spawn_standin(
            &sock,
            Request::StartBenchmark {
                target: StreamTarget::Tier(Tier::L1),
                mode: BenchMode::MemoryOnly,
            },
            vec![
                Response::BenchStarted { run_id: 7 },
                Response::BenchCancelled { run_id: 7 },
            ],
        );
        let mut client = Client::connect(sock.path()).expect("must connect");
        let text = bench(&mut client, StreamTarget::Tier(Tier::L1), BenchMode::MemoryOnly)
            .expect("a cancelled run is a clean terminal");
        handle.join().expect("stand-in thread must not panic");

        assert!(text.contains("benchmark started (run 7)"), "got: {text}");
        assert!(text.contains("benchmark cancelled (run 7)"), "got: {text}");
    }

    /// (c) `bench` on a structured error reply (the single-flight busy
    /// case) maps onto `ClientError::Protocol` with the daemon's text
    /// verbatim.
    #[test]
    fn bench_structured_error_reply_is_protocol() {
        let sock = TempSocket::new("bench-err");
        let handle = spawn_standin(
            &sock,
            Request::StartBenchmark {
                target: StreamTarget::Full,
                mode: BenchMode::Full,
            },
            vec![Response::Error("benchmark already running".to_owned())],
        );
        let mut client = Client::connect(sock.path()).expect("must connect");
        let err = bench(&mut client, StreamTarget::Full, BenchMode::Full)
            .expect_err("an Error reply must fail bench");
        handle.join().expect("stand-in thread must not panic");

        assert_eq!(
            err,
            ClientError::Protocol("benchmark already running".to_owned())
        );
    }

    /// (c2) `bench` on a contract-violating `BurnInProgress` frame in
    /// the stream (burn-in ticks belong to the owning `StartBurnIn`
    /// connection, D-1/D-2) maps onto `ClientError::Protocol` with the
    /// guard's text.
    #[test]
    fn bench_burn_in_progress_frame_is_protocol() {
        let sock = TempSocket::new("bench-burnin");
        let handle = spawn_standin(
            &sock,
            Request::StartBenchmark {
                target: StreamTarget::Full,
                mode: BenchMode::Full,
            },
            vec![
                Response::BenchStarted { run_id: 1 },
                Response::BurnInProgress(ramsleuth_bench::BurnInTick {
                    iteration: 1,
                    elapsed_secs: 0.5,
                    tier: Tier::Memory,
                    bandwidth: Some((BenchOp::Read, 512.0)),
                    latency_ns: None,
                }),
            ],
        );
        let mut client = Client::connect(sock.path()).expect("must connect");
        let err = bench(&mut client, StreamTarget::Full, BenchMode::Full)
            .expect_err("a BurnInProgress frame must fail bench");
        handle.join().expect("stand-in thread must not panic");

        assert_eq!(
            err,
            ClientError::Protocol("unexpected burn-in frame during benchmark".to_owned())
        );
    }

    /// (d) `status` over the stand-in: the mixed snapshot (a `Na` AMD
    /// branch, a populated Intel readout, one SPD module) renders one
    /// line per section with the right state, plus the socket line.
    #[test]
    fn status_summary_reflects_the_section_states() {
        let sock = TempSocket::new("status-mixed");
        let handle = spawn_standin(
            &sock,
            Request::GetTelemetry,
            vec![Response::Telemetry(mixed_snapshot())],
        );
        let mut client = Client::connect(sock.path()).expect("must connect");
        let text = status(&mut client).expect("status must succeed on a Telemetry reply");
        handle.join().expect("stand-in thread must not panic");

        assert!(text.contains("CPU: ok"), "got: {text}");
        assert!(text.contains("AMD: N/A (DriverMissing)"), "got: {text}");
        assert!(text.contains("Intel: ok"), "got: {text}");
        assert!(text.contains("SPD: ok (1 module)"), "got: {text}");
        let path = sock.path().to_string_lossy().into_owned();
        assert!(text.contains(&format!("daemon: {path}")), "socket line, got: {text}");
    }

    /// (e) `status` on a fully degraded snapshot: every vendor section
    /// `N/A (<reason>)`, SPD empty, CPU still `ok` (the snapshot root
    /// always carries the detected CPU).
    #[test]
    fn status_summary_on_all_na_snapshot() {
        let sock = TempSocket::new("status-na");
        let handle = spawn_standin(
            &sock,
            Request::GetTelemetry,
            vec![Response::Telemetry(all_na_snapshot())],
        );
        let mut client = Client::connect(sock.path()).expect("must connect");
        let text = status(&mut client).expect("status must succeed");
        handle.join().expect("stand-in thread must not panic");

        assert!(text.contains("CPU: ok"), "got: {text}");
        assert!(text.contains("AMD: N/A (DriverMissing)"), "got: {text}");
        assert!(text.contains("Intel: N/A (InsufficientPrivilege)"), "got: {text}");
        assert!(text.contains("SPD: N/A (no modules)"), "got: {text}");
    }

    /// (f) `status` on a structured error reply maps onto
    /// `ClientError::Protocol` with the daemon's text verbatim.
    #[test]
    fn status_structured_error_reply_is_protocol() {
        let sock = TempSocket::new("status-err");
        let handle = spawn_standin(
            &sock,
            Request::GetTelemetry,
            vec![Response::Error("nope".to_owned())],
        );
        let mut client = Client::connect(sock.path()).expect("must connect");
        let err = status(&mut client).expect_err("an Error reply must fail status");
        handle.join().expect("stand-in thread must not panic");

        assert_eq!(err, ClientError::Protocol("nope".to_owned()));
    }

    /// (g) `render_grid` on a fully-populated grid: the 4×4 table
    /// (header + four tier rows), every line the same width (aligned),
    /// GB/s + ns/hop units, and no `N/A` cells.
    #[test]
    fn render_grid_full_is_aligned_and_unit_correct() {
        let text = render_grid(&full_grid());
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 5, "header + 4 tier rows, got: {text}");
        let width = lines[0].len();
        assert!(
            lines.iter().all(|line| line.len() == width),
            "every line must be the same width (aligned), got: {text}"
        );
        assert!(lines[0].contains("Read"), "got: {text}");
        assert!(lines[0].contains("Write"), "got: {text}");
        assert!(lines[0].contains("Copy"), "got: {text}");
        assert!(lines[0].contains("Latency"), "got: {text}");
        assert!(lines[1].starts_with("Memory"), "got: {text}");
        assert!(lines[2].starts_with("L1"), "got: {text}");
        assert!(lines[3].starts_with("L2"), "got: {text}");
        assert!(lines[4].starts_with("L3"), "got: {text}");
        assert!(lines[1].contains("512.00 GB/s"), "got: {text}");
        assert!(lines[1].contains("88.00 ns/hop"), "got: {text}");
        assert!(lines[4].contains("12.70 ns/hop"), "got: {text}");
        assert!(!text.contains("N/A"), "a full grid has no N/A cells, got: {text}");
    }

    /// (h) `render_grid` on a partial grid (a single-cell run): the one
    /// measured cell prints its value, the 15 unmeasured cells (zero on
    /// the wire) print `N/A`, and the table stays aligned.
    #[test]
    fn render_grid_partial_marks_unmeasured_cells_na() {
        let text = render_grid(&partial_grid());
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 5, "header + 4 tier rows, got: {text}");
        let width = lines[0].len();
        assert!(
            lines.iter().all(|line| line.len() == width),
            "every line must be the same width (aligned), got: {text}"
        );
        assert_eq!(
            text.matches("N/A").count(),
            15,
            "15 unmeasured cells, got: {text}"
        );
        assert!(lines[1].contains("512.00 GB/s"), "the measured cell, got: {text}");
        assert!(lines[1].contains("N/A"), "unmeasured Memory cells, got: {text}");
    }

    // ------------------------------------------------------------------
    // `probe` (the TUI `[F]` / GUI Probe parity path) — the stand-in
    // answers `GetProbeReport` with a canned `ProbeReport`.
    // ------------------------------------------------------------------

    /// A canned [`ProbeReport`] over the mixed snapshot (Intel branch
    /// populated, `raw` absent) with a fully-populated system identity
    /// (host-independent).
    fn fixture_report() -> ProbeReport {
        ProbeReport {
            telemetry: mixed_snapshot(),
            raw: None,
            system: ProbeSystem {
                cpu_brand: "Intel Core i7-12700K".to_owned(),
                cpu_vendor: "Intel".to_owned(),
                cpu_gen: "AlderLake".to_owned(),
                pci_host_bridge: Some("0x8086:0x46F2".to_owned()),
                kernel: "6.6.0-test".to_owned(),
                os: "Linux / Arch".to_owned(),
                arch: "x86_64".to_owned(),
                ramsleuth_version: "2.4.5".to_owned(),
                telemetry_source: "ramsleuth_intel".to_owned(),
            },
        }
    }

    /// (p1) `probe_to` in the default (file) form: the `GetProbeReport`
    /// request rides the wire, the `ProbeReport` reply renders to the
    /// chunk-2 markdown, the file (a nested temp dir — the `create_dir_all`
    /// path) is written, and the returned text is the `report written
    /// to <path>` confirmation.
    #[test]
    fn probe_writes_the_markdown_to_the_given_path() {
        let sock = TempSocket::new("probe-write");
        let handle = spawn_standin(&sock, Request::GetProbeReport, vec![Response::ProbeReport(fixture_report())]);
        let dir = std::env::temp_dir().join(format!(
            "ramsleuth-client-probe-{}-{}",
            process::id(),
            "write"
        ));
        let _ = std::fs::remove_dir_all(&dir); // stale from a crashed earlier run
        let path = dir.join("probe-report.md");
        assert!(!path.exists(), "the nested dir must not exist yet");
        let mut client = Client::connect(sock.path()).expect("must connect");
        let text = probe_to(&mut client, false, false, &path)
            .expect("probe must succeed on a ProbeReport reply");
        handle.join().expect("stand-in thread must not panic");

        assert_eq!(
            text,
            format!("report written to {}", path.display()),
            "the confirmation names the path"
        );
        let written = std::fs::read_to_string(&path).expect("the report file must exist");
        assert!(
            written.starts_with("# RamSleuth Probe Report"),
            "the written file is the chunk-2 markdown, got: {written}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (p2) `probe_to --stdout` (without `--json`): the markdown is the
    /// returned text (the confirmation goes to stderr, so the returned
    /// text is the report itself) and no file is touched.
    #[test]
    fn probe_stdout_returns_the_markdown() {
        let sock = TempSocket::new("probe-stdout");
        let handle = spawn_standin(&sock, Request::GetProbeReport, vec![Response::ProbeReport(fixture_report())]);
        let mut client = Client::connect(sock.path()).expect("must connect");
        let path = std::env::temp_dir().join(format!(
            "ramsleuth-client-probe-{}-{}",
            process::id(),
            "stdout-never-written.md"
        ));
        let text = probe_to(&mut client, true, false, &path)
            .expect("probe must succeed on a ProbeReport reply");
        handle.join().expect("stand-in thread must not panic");

        assert_eq!(
            text,
            render_probe_report_md(&fixture_report()),
            "--stdout returns the rendered markdown"
        );
        assert!(!path.exists(), "--stdout must not write the default file");
    }

    /// (p3) `probe_to --json`: the raw [`ProbeReport`] is the returned
    /// text as pretty JSON — it parses back to the exact fixture (the
    /// serde round trip), and no file is touched.
    #[test]
    fn probe_json_returns_the_raw_report_json() {
        let sock = TempSocket::new("probe-json");
        let handle = spawn_standin(&sock, Request::GetProbeReport, vec![Response::ProbeReport(fixture_report())]);
        let mut client = Client::connect(sock.path()).expect("must connect");
        let path = std::env::temp_dir().join(format!(
            "ramsleuth-client-probe-{}-{}",
            process::id(),
            "json-never-written.md"
        ));
        let text = probe_to(&mut client, false, true, &path)
            .expect("probe must succeed on a ProbeReport reply");
        handle.join().expect("stand-in thread must not panic");

        let back: ProbeReport =
            serde_json::from_str(&text).expect("the returned text must be the report JSON");
        assert_eq!(back, fixture_report(), "--json round-trips the ProbeReport");
        assert!(!path.exists(), "--json must not write the default file");
    }

    /// (p4) `probe_to` on a structured error reply maps onto
    /// `ClientError::Protocol` with the daemon's text verbatim.
    #[test]
    fn probe_structured_error_reply_is_protocol() {
        let sock = TempSocket::new("probe-err");
        let handle = spawn_standin(&sock, Request::GetProbeReport, vec![Response::Error("nope".to_owned())]);
        let mut client = Client::connect(sock.path()).expect("must connect");
        let err = probe_to(&mut client, false, false, Path::new("/tmp/never"))
            .expect_err("an Error reply must fail probe");
        handle.join().expect("stand-in thread must not panic");

        assert_eq!(err, ClientError::Protocol("nope".to_owned()));
    }

    /// (p5) `probe_to` on a contract-violating reply (a `Telemetry`
    /// frame where a probe report is due) maps onto
    /// `ClientError::Protocol` with the guard's text.
    #[test]
    fn probe_unexpected_reply_is_protocol() {
        let sock = TempSocket::new("probe-badreply");
        let handle =
            spawn_standin(&sock, Request::GetProbeReport, vec![Response::Telemetry(mixed_snapshot())]);
        let mut client = Client::connect(sock.path()).expect("must connect");
        let err = probe_to(&mut client, false, false, Path::new("/tmp/never"))
            .expect_err("a non-ProbeReport reply must fail probe");
        handle.join().expect("stand-in thread must not panic");

        assert_eq!(
            err,
            ClientError::Protocol("expected a probe report reply to GetProbeReport".to_owned())
        );
    }

    // ------------------------------------------------------------------
    // `burn` (the TUI `[X]` / GUI Run Burn-In parity path) — the
    // stand-in asserts the `StartBurnIn` request and answers.
    // ------------------------------------------------------------------

    /// (b1) `burn` with a finite duration: the `StartBurnIn { Full, 5 }`
    /// request rides the wire, the `BenchStarted { run_id }` ack yields
    /// the confirmation (the run id, the duration, and the stop guidance
    /// — the daemon's `CancelBenchmark` from any client), and the call
    /// is `Ok`.
    #[test]
    fn burn_starts_a_finite_soa() {
        let sock = TempSocket::new("burn-finite");
        let handle = spawn_standin(
            &sock,
            Request::StartBurnIn {
                target: StreamTarget::Full,
                duration_minutes: 5,
            },
            vec![Response::BenchStarted { run_id: 3 }],
        );
        let mut client = Client::connect(sock.path()).expect("must connect");
        let text = burn(&mut client, 5).expect("burn must succeed on a BenchStarted ack");
        handle.join().expect("stand-in thread must not panic");

        assert!(text.contains("burn-in started (run 3)"), "the run id, got: {text}");
        assert!(text.contains("5 minutes"), "the finite duration, got: {text}");
        assert!(
            text.contains("cancel run 3"),
            "the stop guidance names the run, got: {text}"
        );
        assert!(
            text.contains("CancelBenchmark"),
            "the stop guidance names the daemon's cancel, got: {text}"
        );
    }

    /// (b2) `burn` with the infinite duration (`0`): the `StartBurnIn {
    /// Full, 0 }` request rides the wire, and the confirmation announces
    /// the infinite run (stops only when cancelled).
    #[test]
    fn burn_infinite_soa_announces_the_cancel_path() {
        let sock = TempSocket::new("burn-infinite");
        let handle = spawn_standin(
            &sock,
            Request::StartBurnIn {
                target: StreamTarget::Full,
                duration_minutes: 0,
            },
            vec![Response::BenchStarted { run_id: 7 }],
        );
        let mut client = Client::connect(sock.path()).expect("must connect");
        let text = burn(&mut client, 0).expect("burn must succeed on a BenchStarted ack");
        handle.join().expect("stand-in thread must not panic");

        assert!(text.contains("burn-in started (run 7)"), "got: {text}");
        assert!(text.contains("infinite"), "the infinite duration, got: {text}");
        assert!(
            text.contains("cancel run 7"),
            "an infinite run stops only via the cancel, got: {text}"
        );
    }

    /// (b3) `burn` clamps an over-large duration to the GUI's 1440-max
    /// before it rides the wire (the stand-in asserts the clamped
    /// request).
    #[test]
    fn burn_clamps_an_overlarge_duration() {
        let sock = TempSocket::new("burn-clamp");
        let handle = spawn_standin(
            &sock,
            Request::StartBurnIn {
                target: StreamTarget::Full,
                duration_minutes: 1440,
            },
            vec![Response::BenchStarted { run_id: 1 }],
        );
        let mut client = Client::connect(sock.path()).expect("must connect");
        let text = burn(&mut client, 9999).expect("burn must succeed on a BenchStarted ack");
        handle.join().expect("stand-in thread must not panic");

        assert!(
            text.contains("1440 minutes"),
            "the clamped duration, got: {text}"
        );
    }

    /// (b4) `burn` on a structured error reply (the single-flight busy
    /// case) maps onto `ClientError::Protocol` with the daemon's text
    /// verbatim.
    #[test]
    fn burn_busy_reply_is_protocol() {
        let sock = TempSocket::new("burn-busy");
        let handle = spawn_standin(
            &sock,
            Request::StartBurnIn {
                target: StreamTarget::Full,
                duration_minutes: 5,
            },
            vec![Response::Error("benchmark already running".to_owned())],
        );
        let mut client = Client::connect(sock.path()).expect("must connect");
        let err = burn(&mut client, 5).expect_err("an Error reply must fail burn");
        handle.join().expect("stand-in thread must not panic");

        assert_eq!(
            err,
            ClientError::Protocol("benchmark already running".to_owned())
        );
    }

    /// (b5) `burn` on a contract-violating reply (a `Telemetry` frame
    /// where a burn-in start is due) maps onto `ClientError::Protocol`
    /// with the guard's text.
    #[test]
    fn burn_unexpected_reply_is_protocol() {
        let sock = TempSocket::new("burn-badreply");
        let handle =
            spawn_standin(&sock, Request::StartBurnIn {
                target: StreamTarget::Full,
                duration_minutes: 5,
            }, vec![Response::Telemetry(mixed_snapshot())]);
        let mut client = Client::connect(sock.path()).expect("must connect");
        let err = burn(&mut client, 5).expect_err("a non-BenchStarted reply must fail burn");
        handle.join().expect("stand-in thread must not panic");

        assert_eq!(
            err,
            ClientError::Protocol("expected a burn-in start reply to StartBurnIn".to_owned())
        );
    }
}
