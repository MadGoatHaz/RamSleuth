//! `dram_spike` — C10-03 (D-3) — a short DRAM load ("spike") before the
//! daemon re-reads the SMU PM table.
//!
//! **Why:** while the DIMMs sit in a low-power idle state, the live SMU
//! PM-table `MCLK` (the memory clock the daemon samples and the GUI
//! displays) reads the *idle* frequency, not the operating frequency.
//! Before every re-read the daemon calls [`spike`]: a bounded ~256 MiB /
//! ~250 ms read+write load that pulls the memory controller out of idle
//! so the next `MCLK` sample is taken at the operating frequency. It is
//! pure userspace (no privilege, no hardware data) and runs on the
//! blocking pool immediately before `collect()`.
//!
//! **Contract (no-panic / footprint):**
//! - Single-flight: a global `SPIKE_RUNNING` gate (`AtomicBool`) ensures
//!   at most one spike in flight; a second caller no-ops (`enter`
//!   returns `None`). The gate is released by an RAII `RunningGuard` on
//!   every exit path.
//! - Footprint: the buffer is allocated per spike (`vec!`, zeroed — the
//!   one-time touch is itself a write pass over every page), used for
//!   the duration, then freed by drop. No static buffer, no growth, no
//!   `unsafe`.
//! - No-panic: no `unwrap`/`expect`/`panic!` on any derived data; the
//!   strided index stays in-bounds by construction and degrades via
//!   `match` on the (always-`Ok`) `try_into`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Buffer size for one spike: 256 MiB (the low end of the requested
/// 256 MiB–1 GiB band — enough sustained traffic to pull the DIMMs out
/// of low-power idle, small enough not to pressure a 64 GiB host).
pub const SPIKE_BUF_BYTES: usize = 256 * 1024 * 1024;

/// Duration of one spike: 250 ms of strided read+write traffic.
pub const SPIKE_DURATION: Duration = Duration::from_millis(250);

/// Single-flight gate: `true` while a spike is in flight. Defended under
/// the already-serializing cache mutex (the collector runs single-flight
/// by construction); a busy gate makes a second caller a no-op.
static SPIKE_RUNNING: AtomicBool = AtomicBool::new(false);

/// RAII guard that releases its `AtomicBool` gate on drop.
///
/// Holds the gate it acquired; dropping it stores `false` (release
/// ordering) so the next caller can proceed. Constructed only by
/// [`enter`]; private to the module.
struct RunningGuard<'a> {
    gate: &'a AtomicBool,
}

impl<'a> Drop for RunningGuard<'a> {
    fn drop(&mut self) {
        self.gate.store(false, Ordering::Release);
    }
}

/// Try to acquire a single-flight gate.
///
/// Returns [`Some`] with a `RunningGuard` on a successful CAS
/// `false -> true` (the gate is held until the guard drops), or [`None`]
/// if the gate is already busy (a spike is in flight). Takes a
/// `&AtomicBool` (rather than only the global) so the test module can
/// exercise the logic against a local gate.
fn enter(gate: &AtomicBool) -> Option<RunningGuard<'_>> {
    match gate.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed) {
        Ok(_) => Some(RunningGuard { gate }),
        Err(_) => None,
    }
}

/// Run one DRAM spike: acquire the gate (no-op if a spike is already in
/// flight), allocate the buffer, drive strided read+write traffic for
/// [`SPIKE_DURATION`], then free the buffer and release the gate.
///
/// Safe to call from the blocking pool; pure userspace; no hardware
/// data; no panic. An in-flight load already covers the DIMMs, so a
/// concurrent caller simply skips rather than stacking a second
/// 256 MiB allocation.
pub fn spike() {
    // Acquire the single-flight gate; no-op if a spike is already running.
    // Keep-alive binding: holds the gate for the whole body (RAII drop
    // after `buf` is freed); `_` prefix suppresses the unused-variable
    // lint (the guard is intentionally never read).
    let _guard = match enter(&SPIKE_RUNNING) {
        Some(guard) => guard,
        None => return,
    };
    // Zeroed allocation: the one-time touch is itself a write pass over
    // every page (no `unsafe`, no pre-filled static). Declared after the
    // guard so it frees *before* the gate is released (RAII drop order).
    let mut buf = vec![0u8; SPIKE_BUF_BYTES];
    let sink = spike_run(&mut buf, SPIKE_DURATION);
    // Sink the accumulator so the compiler cannot elide the loop.
    std::hint::black_box(sink);
    // `buf` drops (frees the 256 MiB), then `guard` drops (releases the
    // gate) — on every exit path.
}

/// Drive a time-boxed strided read+write loop over `buf` for `duration`,
/// returning an accumulator `sink` (fed to `black_box` by the caller).
///
/// Both the DRAM read and write buses stay active: each iteration reads
/// an 8-byte little-endian word at `idx`, folds it into `sink` (wrapping
/// add + multiply — a cheap non-linear mix), and XOR-writes it back.
/// `idx` advances by a 64-byte stride and wraps to 0 before `idx + 8 >
/// len`, so every access is in-bounds by construction. A `len < 8`
/// buffer returns 0 (nothing to stride). No `unwrap`/`expect`/`panic!`.
fn spike_run(buf: &mut [u8], duration: Duration) -> u64 {
    let len = buf.len();
    if len < 8 {
        return 0;
    }
    let deadline = Instant::now() + duration;
    let mut idx: usize = 0;
    let mut sink: u64 = 0;
    // Strided read+write loop. In-bounds by construction, so `try_into`
    // is `Ok`; the `while let` degrades (exits) rather than panic on the
    // impossible.
    while let Ok(bytes) = buf[idx..idx + 8].try_into() {
        let word = u64::from_le_bytes(bytes);
        // Fold into the accumulator (wrapping — no overflow panic).
        sink = sink.wrapping_add(word).wrapping_mul(0x9E3779B97F4A7C15);
        // XOR write-back (activates the write bus). Equal length, no panic.
        let out = (word ^ 0x5A5A5A5A5A5A5A5A).to_le_bytes();
        buf[idx..idx + 8].copy_from_slice(&out);
        // Advance by a 64-byte stride; wrap to 0 before `idx + 8 > len`.
        idx += 64;
        if idx + 8 > len {
            idx = 0;
        }
        // Time-box: stop once the deadline is reached.
        if Instant::now() >= deadline {
            break;
        }
    }
    sink
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (a) Single-flight: a second `enter` returns `None` while the first
    /// guard is held; the gate reads `true` throughout.
    #[test]
    fn enter_is_single_flight() {
        let gate = AtomicBool::new(false);
        let first = enter(&gate).expect("first enter acquires the free gate");
        assert!(gate.load(Ordering::SeqCst), "gate is true while the guard is held");
        assert!(enter(&gate).is_none(), "a second enter while the first is held -> None");
        drop(first);
    }

    /// (b) The guard releases the gate on drop: `enter` on a free gate
    /// yields `Some` + `true`; dropping the guard stores `false`.
    #[test]
    fn guard_releases_on_drop() {
        let gate = AtomicBool::new(false);
        {
            let _g = enter(&gate).expect("enter acquires the free gate");
            assert!(gate.load(Ordering::SeqCst), "gate true while held");
        } // `_g` drops here.
        assert!(!gate.load(Ordering::SeqCst), "gate false after the guard drops");
    }

    /// (c) `spike_run` on a 4096-byte zeroed buffer for 10 ms: the buffer
    /// is modified (XOR write-back observed) and the call is time-bounded.
    #[test]
    fn spike_run_modifies_and_is_time_bounded() {
        let mut buf = vec![0u8; 4096];
        let start = Instant::now();
        let sink = spike_run(&mut buf, Duration::from_millis(10));
        let elapsed = start.elapsed();
        std::hint::black_box(sink);
        assert!(!buf.iter().all(|&b| b == 0), "write-back must modify the buffer");
        assert!(elapsed >= Duration::from_millis(10), "ran at least the target: {elapsed:?}");
        assert!(elapsed < Duration::from_secs(10), "time-bounded (not stuck): {elapsed:?}");
    }

    /// (d) `spike_run` on a 4-byte buffer returns 0 without panicking
    /// (`len < 8` -> nothing to stride).
    #[test]
    fn spike_run_short_buffer_returns_zero_no_panic() {
        let mut buf = vec![0u8; 4];
        let sink = spike_run(&mut buf, Duration::from_millis(5));
        assert_eq!(sink, 0, "len < 8 returns 0");
        assert_eq!(buf, vec![0u8; 4], "buffer left untouched");
    }

    /// (e) The global `spike()` path (256 MiB) returns, leaves
    /// `SPIKE_RUNNING` released, and is bounded well under 3 s.
    #[test]
    fn spike_global_path_releases_gate_and_is_bounded() {
        assert!(!SPIKE_RUNNING.load(Ordering::SeqCst), "gate free before the spike");
        let start = Instant::now();
        spike();
        let elapsed = start.elapsed();
        assert!(!SPIKE_RUNNING.load(Ordering::SeqCst), "gate released after spike() returns");
        assert!(elapsed < Duration::from_secs(3), "spike() is bounded: {elapsed:?}");
    }
}
