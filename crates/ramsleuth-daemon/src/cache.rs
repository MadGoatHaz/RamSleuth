//! TTL lazy telemetry cache (P3-14, plan D5) with a **read/write split**
//! (DAEMON-3): `collect()` is expensive (CPUID + sysfs + `/dev/mem` + SPD
//! EEPROM — the 256 MiB `spike()` + `CLOCK_SETTLE` + `collect()`), so
//! repeated `GetTelemetry` RPCs inside the TTL must reuse the cached
//! snapshot. The split lets concurrent frontends (TUI + GUI + CLI) serve
//! the last-good snapshot **in parallel** (a shared read lock — a cheap
//! clone) instead of queueing behind the one **exclusive** re-collect
//! (the write lock), which only the collector takes.
//!
//! The collector is **injectable**: [`TelemetryCache::new`] takes any
//! `Fn() -> SystemMemoryTelemetry + Send + Sync + 'static`. Production
//! passes `ramsleuth_telemetry::collect` (no-panic by the Phase 2
//! contract); tests pass a counting / sleeping mock, so the cache is
//! fully testable without hardware.
//!
//! Semantics:
//! - the **first** [`TelemetryCache::get`] on a cold cache collects
//!   **twice** (C14, M2 first-read warm-up): the warm-up read is
//!   discarded — its spike + settle wakes and settles the SMU — and the
//!   second read is stored with a fresh [`std::time::Instant`] and
//!   returned, so the first *served and cached* sample is the settled
//!   one, not a cold idle-frequency transient;
//! - a later `get()` on a stale snapshot (`last_at.elapsed() >= ttl`)
//!   re-collects once, stores the snapshot plus a fresh
//!   [`std::time::Instant`], and returns it;
//! - a `get()` inside the TTL returns a **clone** of the cached
//!   snapshot under the **shared** read lock — no collector call, and
//!   many readers at once;
//! - a re-collect takes the **exclusive** write lock; a reader that
//!   arrives while one is in flight briefly waits for the *updated*
//!   snapshot (RwLock semantics), then reads it fresh. Two racers that
//!   both see "stale" collapse to a **single** re-collect (the loser
//!   re-checks under the write lock and finds the snapshot already
//!   refreshed).
//! - nothing in this module panics by itself: poisoned locks are
//!   recovered (`into_inner`) — the no-panic contract (plan D5). The
//!   only dynamic work is the injected collector call (a panicking
//!   collector is the collector's fault — the real `collect()` is
//!   no-panic) and a clone of the wire-safe `SystemMemoryTelemetry`
//!   (P3-06).
//!
//! The cache is `&self`-accessed (the `RwLock` is **internal**): the
//! daemon shares one `Arc<TelemetryCache>` across every connection
//! (P3-16) and calls `get()` from the blocking pool — no external
//! `Mutex` serializes the readers any more.

use std::sync::{PoisonError, RwLock};
use std::time::{Duration, Instant};

use ramsleuth_telemetry::SystemMemoryTelemetry;

/// The mutable half of the cache: the last-good snapshot, the instant it
/// was collected, and the warm-up flag. Behind an [`RwLock`]: readers (a
/// fresh snapshot) share the read lock; the re-collect takes the write
/// lock.
struct CacheState {
    /// The most recent snapshot; `None` while the cache is cold.
    last: Option<SystemMemoryTelemetry>,
    /// The instant [`last`] was collected; `None` while cold.
    last_at: Option<Instant>,
    /// `false` until the first `get()` completes its warm-up double-read;
    /// `true` forever after (C14, M2).
    warmed: bool,
}

/// A TTL lazy cache over an injectable telemetry collector (P3-14) with
/// the DAEMON-3 read/write split.
///
/// - `collector`: the snapshot source — `ramsleuth_telemetry::collect`
///   in production, a counting / sleeping mock in tests.
/// - `ttl`: maximum age of a cached snapshot before `get()` re-collects
///   (the daemon's `--max-age`, default 2 s, P3-17).
/// - `state`: the last-good snapshot + instant + warm-up flag, behind an
///   internal [`RwLock`] (shared reads, exclusive re-collects).
pub struct TelemetryCache {
    collector: Box<dyn Fn() -> SystemMemoryTelemetry + Send + Sync>,
    ttl: Duration,
    state: RwLock<CacheState>,
}

impl TelemetryCache {
    /// Create a cache with the given collector and TTL.
    ///
    /// Production: `TelemetryCache::new(ramsleuth_telemetry::collect,
    /// max_age)`; tests: a counting / sleeping mock collector.
    pub fn new(
        collector: impl Fn() -> SystemMemoryTelemetry + Send + Sync + 'static,
        ttl: Duration,
    ) -> Self {
        Self {
            collector: Box::new(collector),
            ttl,
            state: RwLock::new(CacheState {
                last: None,
                last_at: None,
                warmed: false,
            }),
        }
    }

    /// Return the current snapshot.
    ///
    /// **Cheap path (shared read lock):** a warmed cache whose last
    /// snapshot is within the TTL returns a **clone** of it — no
    /// collector call, and any number of readers at once.
    ///
    /// **Exclusive path (write lock):** a cold cache (first `get`) runs
    /// the warm-up double-read (the first (warm-up) result is discarded —
    /// its spike + settle wakes and settles the SMU — and the second is
    /// stored with [`Instant::now()`] and returned, C14 M2); a stale
    /// snapshot re-collects once, stores the result plus
    /// [`Instant::now()`], and returns it. Only the collector holds the
    /// write lock, so concurrent fresh readers never queue behind a
    /// re-collect that isn't happening, and two racers that both see
    /// "stale" collapse to a single re-collect (the loser re-checks under
    /// the write lock and finds the snapshot already refreshed).
    ///
    /// `&self` (the `RwLock` is internal): the daemon shares one
    /// `Arc<TelemetryCache>` across connections. Never panics by itself
    /// (poisoned locks are recovered — the no-panic contract, plan D5; a
    /// panicking collector is the collector's fault — the real
    /// `collect()` is no-panic by the Phase 2 contract).
    pub fn get(&self) -> SystemMemoryTelemetry {
        // Cheap "is it fresh?" read: the shared lock, no collector call.
        {
            let state = self.state.read().unwrap_or_else(PoisonError::into_inner);
            if state.warmed {
                if let (Some(last), Some(last_at)) = (&state.last, &state.last_at) {
                    if last_at.elapsed() < self.ttl {
                        return last.clone();
                    }
                }
            }
        }
        // Stale or cold: the exclusive re-collect (the only writer).
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        // Re-check under the write lock: a concurrent collector may have
        // refreshed the snapshot while we waited for the lock — avoid a
        // redundant re-collect.
        if state.warmed {
            if let (Some(last), Some(last_at)) = (&state.last, &state.last_at) {
                if last_at.elapsed() < self.ttl {
                    return last.clone();
                }
            }
        } else {
            // Cold cache: warm-up read (discarded) — its spike+settle+read
            // wakes and settles the SMU — then the settled read is cached.
            let _warmup = (self.collector)();
            state.warmed = true;
        }
        let snap = (self.collector)();
        state.last = Some(snap.clone());
        state.last_at = Some(Instant::now());
        snap
    }

    /// The configured TTL (accessor for tests/diagnostics).
    pub fn ttl(&self) -> Duration {
        self.ttl
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use ramsleuth_telemetry::cpuid::{CpuInfo, CpuVendor};
    use ramsleuth_telemetry::error::{NaReason, Section};
    use ramsleuth_telemetry::SystemPlatform;

    use super::*;

    /// Mock snapshot built via the telemetry crate's public API
    /// (host-independent — no hardware, no real `collect()`): vendor
    /// branches all-`Na` + empty SPD, representative platform /
    /// capacity values (C6-13/14).
    fn mock_snapshot() -> SystemMemoryTelemetry {
        SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor: CpuVendor::Unknown,
                brand: "Mock CPU".to_owned(),
            },
            amd: Section::na(NaReason::NotApplicable),
            intel: Section::na(NaReason::NotApplicable),
            spd: Vec::new(),
            platform: SystemPlatform {
                cpu_clock_mhz: Section::Value(3500.0),
                motherboard: Section::Value("Test Board".to_owned()),
                bios: Section::Value("1.0".to_owned()),
                agesa: Section::na(NaReason::NotApplicable),
                smu_version: Section::na(NaReason::NotApplicable),
            },
            total_capacity: Section::Value(32.0),
            dimm_sizes: vec![Section::Value(16.0), Section::Value(16.0)],
            vcore: Section::na(NaReason::NotApplicable),
        }
    }

    /// Mock collector: bumps the shared call counter and returns the
    /// fixed snapshot — the real `collect()` is never exercised.
    fn counting_collector(
        counter: Arc<AtomicUsize>,
    ) -> impl Fn() -> SystemMemoryTelemetry + Send + Sync + 'static {
        move || {
            counter.fetch_add(1, Ordering::SeqCst);
            mock_snapshot()
        }
    }

    /// (a) The first `get()` on a cold cache calls the collector twice
    /// (warm-up + settled read) and returns the second call's value.
    #[test]
    fn first_get_warms_then_collects() {
        let counter = Arc::new(AtomicUsize::new(0));
        let cache = TelemetryCache::new(
            counting_collector(Arc::clone(&counter)),
            Duration::from_secs(5),
        );

        let snap = cache.get();

        assert_eq!(
            counter.load(Ordering::SeqCst),
            2,
            "cold cache: warm-up read + settled read"
        );
        assert_eq!(snap, mock_snapshot());
    }

    /// (b) A second `get()` inside the TTL does not re-collect: the
    /// first `get()` already consumed the warm-up double-read, so two
    /// `get()`s leave counter == 2 — and returns a snapshot equal to the
    /// first (a clone of the cache).
    #[test]
    fn second_get_within_ttl_does_not_recollect() {
        let counter = Arc::new(AtomicUsize::new(0));
        let cache = TelemetryCache::new(
            counting_collector(Arc::clone(&counter)),
            Duration::from_secs(5),
        );

        let first = cache.get();
        let second = cache.get();

        assert_eq!(
            counter.load(Ordering::SeqCst),
            2,
            "warm-up double-read from the first get; in-TTL second get adds no collect"
        );
        assert_eq!(second, first, "the cached snapshot is returned by clone");
        assert_eq!(second, mock_snapshot());
    }

    /// (c) A `get()` after the TTL expires re-collects exactly once
    /// (counter == 3: 2 warm-up + 1 TTL re-collect). A 1 ms TTL + a 20 ms
    /// sleep makes staleness deterministic without sleeping anywhere near
    /// the production 2 s default.
    #[test]
    fn get_after_ttl_expiry_recollects() {
        let counter = Arc::new(AtomicUsize::new(0));
        let cache = TelemetryCache::new(
            counting_collector(Arc::clone(&counter)),
            Duration::from_millis(1),
        );

        cache.get();
        std::thread::sleep(Duration::from_millis(20));
        let snap = cache.get();

        assert_eq!(
            counter.load(Ordering::SeqCst),
            3,
            "2 warm-up collects + 1 TTL-expired re-collect"
        );
        assert_eq!(snap, mock_snapshot());
    }

    /// (d) Returned values equal the mock's snapshot field-for-field
    /// (the cold first `get()` consumed the warm-up double-read,
    /// counter == 2), and the `ttl` accessor reports the configured TTL.
    #[test]
    fn returned_value_matches_mock_and_ttl_accessor_reports_config() {
        let counter = Arc::new(AtomicUsize::new(0));
        let cache = TelemetryCache::new(
            counting_collector(Arc::clone(&counter)),
            Duration::from_secs(3),
        );

        assert_eq!(cache.ttl(), Duration::from_secs(3));
        let snap = cache.get();

        assert_eq!(
            counter.load(Ordering::SeqCst),
            2,
            "cold cache: warm-up read + settled read"
        );
        // Field-level equality against the mock (SystemMemoryTelemetry
        // derives PartialEq; the mock's vendor branches are all-Na +
        // empty SPD).
        assert_eq!(
            snap.cpu,
            CpuInfo {
                vendor: CpuVendor::Unknown,
                brand: "Mock CPU".to_owned(),
            }
        );
        assert_eq!(snap.amd, Section::na(NaReason::NotApplicable));
        assert_eq!(snap.intel, Section::na(NaReason::NotApplicable));
        assert!(snap.spd.is_empty());
        assert_eq!(snap, mock_snapshot());
    }

    // ------------------------------------------------------------------
    // TC-DAEMON3: the read/write split. A slow collector makes the
    // distinction observable: fresh reads are cheap + concurrent, the
    // re-collect is the one expensive + exclusive operation.
    // ------------------------------------------------------------------

    /// A collector that sleeps `slow_ms` per call and bumps `counter` —
    /// stands in for the real `spike()` + `CLOCK_SETTLE` + `collect()`.
    fn sleeping_collector(
        counter: Arc<AtomicUsize>,
        slow_ms: u64,
    ) -> impl Fn() -> SystemMemoryTelemetry + Send + Sync + 'static {
        move || {
            counter.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(slow_ms));
            mock_snapshot()
        }
    }

    /// TC-DAEMON3 (concurrent fresh reads): with a **slow** collector, a
    /// `GetTelemetry` on a **fresh** cache returns the last-good snapshot
    /// **immediately** — it takes the shared read lock (a cheap clone)
    /// and does **not** call the collector or queue behind a re-collect.
    /// Two concurrent readers both finish well under one slow-collector
    /// sleep, and the collector call count is unchanged by the reads.
    #[test]
    fn tc_daemon3_concurrent_fresh_reads_do_not_queue_behind_the_collector() {
        const SLOW_MS: u64 = 150;
        let counter = Arc::new(AtomicUsize::new(0));
        let cache = Arc::new(TelemetryCache::new(
            sleeping_collector(Arc::clone(&counter), SLOW_MS),
            Duration::from_secs(5), // long TTL: once primed, stays fresh
        ));

        // Prime: a cold `get()` = warm-up + settled (two slow calls).
        let primed = cache.get();
        assert_eq!(
            counter.load(Ordering::SeqCst),
            2,
            "cold prime = warm-up + settled read"
        );

        // Now fresh: two concurrent reads must take the shared read lock,
        // call the collector zero times, and return long before one slow
        // collector sleep.
        let start = Instant::now();
        let c1 = Arc::clone(&cache);
        let c2 = Arc::clone(&cache);
        let t1 = std::thread::spawn(move || c1.get());
        let t2 = std::thread::spawn(move || c2.get());
        let s1 = t1.join().expect("reader 1 must not panic");
        let s2 = t2.join().expect("reader 2 must not panic");
        let elapsed = start.elapsed();

        assert_eq!(
            counter.load(Ordering::SeqCst),
            2,
            "fresh reads must not re-collect"
        );
        assert_eq!(s1, primed, "reader 1 gets the last-good snapshot");
        assert_eq!(s2, primed, "reader 2 gets the last-good snapshot");
        assert!(
            elapsed < Duration::from_millis(SLOW_MS),
            "concurrent fresh reads returned in {elapsed:?} — they must not block behind the slow collector (one re-collect takes >= {SLOW_MS} ms)"
        );
    }

    /// TC-DAEMON3 (exclusive re-collect, then fast again): a poll that
    /// **triggers** a re-collect (the TTL expired) blocks for exactly one
    /// slow collector call (the exclusive write lock); the **next** poll
    /// is fresh again and returns immediately (the shared read lock)
    /// without re-collecting.
    #[test]
    fn tc_daemon3_stale_recollect_is_exclusive_then_fast_again() {
        const SLOW_MS: u64 = 100;
        let counter = Arc::new(AtomicUsize::new(0));
        let cache = Arc::new(TelemetryCache::new(
            sleeping_collector(Arc::clone(&counter), SLOW_MS),
            Duration::from_millis(50), // short TTL: expires well inside the sleep below
        ));

        // Prime: a cold `get()` = warm-up + settled (two slow calls).
        let _ = cache.get();
        assert_eq!(counter.load(Ordering::SeqCst), 2);

        // Let the 50 ms TTL expire (100 ms sleep >> TTL).
        std::thread::sleep(Duration::from_millis(100));

        // A poll that triggers a re-collect blocks for (about) one slow
        // collector call.
        let start = Instant::now();
        let snap = cache.get();
        let re_collect_elapsed = start.elapsed();
        assert_eq!(
            counter.load(Ordering::SeqCst),
            3,
            "the stale poll re-collects exactly once"
        );
        assert!(
            re_collect_elapsed >= Duration::from_millis(SLOW_MS),
            "the re-collect poll must block for the slow collector: {re_collect_elapsed:?}"
        );
        assert_eq!(snap, mock_snapshot());

        // The next poll is fresh again: fast, no re-collect.
        let start = Instant::now();
        let snap2 = cache.get();
        let fast_elapsed = start.elapsed();
        assert_eq!(
            counter.load(Ordering::SeqCst),
            3,
            "the fresh poll must not re-collect"
        );
        assert_eq!(snap2, snap, "the fresh poll returns the just-collected snapshot");
        assert!(
            fast_elapsed < Duration::from_millis(SLOW_MS),
            "the post-re-collect poll must be fast (shared read): {fast_elapsed:?}"
        );
    }
}
