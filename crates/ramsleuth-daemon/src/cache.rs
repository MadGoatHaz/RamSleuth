//! TTL lazy telemetry cache (P3-14, plan D5): `collect()` is expensive
//! (CPUID + sysfs + `/dev/mem` + SPD EEPROM), so repeated `GetTelemetry`
//! RPCs inside the TTL must reuse the cached snapshot instead of
//! re-collecting.
//!
//! The collector is **injectable**: [`TelemetryCache::new`] takes any
//! `Fn() -> SystemMemoryTelemetry + Send + Sync + 'static`. Production
//! passes `ramsleuth_telemetry::collect` (no-panic by the Phase 2
//! contract); tests pass a counting mock, so the cache is fully testable
//! without hardware.
//!
//! Semantics:
//! - [`TelemetryCache::get`] on a cold cache (or one whose snapshot is
//!   stale, `last_at.elapsed() >= ttl`) re-collects, stores the snapshot
//!   plus a fresh [`std::time::Instant`], and returns it;
//! - a `get()` inside the TTL returns a **clone** of the cached
//!   snapshot — no collector call;
//! - nothing in this module panics by itself: the only dynamic work is
//!   the injected collector call (a panicking collector is the
//!   collector's fault — the real `collect()` is no-panic) and a clone
//!   of the wire-safe `SystemMemoryTelemetry` (P3-06).
//!
//! The cache is `&mut self`-accessed (no internal lock): the daemon
//! serves `GetTelemetry` serially per cache; P3-16 (rpc) wraps the cache
//! at its call site (`Arc` + `spawn_blocking`).

use std::time::{Duration, Instant};

use ramsleuth_telemetry::SystemMemoryTelemetry;

/// A TTL lazy cache over an injectable telemetry collector (P3-14).
///
/// - `collector`: the snapshot source — `ramsleuth_telemetry::collect`
///   in production, a counting mock in tests.
/// - `ttl`: maximum age of a cached snapshot before `get()` re-collects
///   (the daemon's `--max-age`, default 2 s, P3-17).
/// - `last` / `last_at`: the most recent snapshot and the instant it was
///   collected; both `None` while the cache is cold.
pub struct TelemetryCache {
    collector: Box<dyn Fn() -> SystemMemoryTelemetry + Send + Sync>,
    ttl: Duration,
    last: Option<SystemMemoryTelemetry>,
    last_at: Option<Instant>,
}

impl TelemetryCache {
    /// Create a cache with the given collector and TTL.
    ///
    /// Production: `TelemetryCache::new(ramsleuth_telemetry::collect,
    /// max_age)`; tests: a counting mock collector.
    pub fn new(
        collector: impl Fn() -> SystemMemoryTelemetry + Send + Sync + 'static,
        ttl: Duration,
    ) -> Self {
        Self {
            collector: Box::new(collector),
            ttl,
            last: None,
            last_at: None,
        }
    }

    /// Return the current snapshot.
    ///
    /// Fresh (`last`/`last_at` both set and `last_at.elapsed() < ttl`)
    /// → a **clone** of the cached snapshot, no collector call. Stale or
    /// cold → call the collector, store the result plus
    /// [`Instant::now()`], and return it. Never panics by itself (a
    /// panicking collector is the collector's fault — the real
    /// `collect()` is no-panic by the Phase 2 contract).
    pub fn get(&mut self) -> SystemMemoryTelemetry {
        if let (Some(last), Some(last_at)) = (&self.last, &self.last_at) {
            if last_at.elapsed() < self.ttl {
                return last.clone();
            }
        }
        let snap = (self.collector)();
        self.last = Some(snap.clone());
        self.last_at = Some(Instant::now());
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

    /// (a) The first `get()` on a cold cache calls the collector exactly
    /// once and returns its value.
    #[test]
    fn first_get_collects_once() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut cache = TelemetryCache::new(
            counting_collector(Arc::clone(&counter)),
            Duration::from_secs(5),
        );

        let snap = cache.get();

        assert_eq!(counter.load(Ordering::SeqCst), 1);
        assert_eq!(snap, mock_snapshot());
    }

    /// (b) A second `get()` inside the TTL does not re-collect and
    /// returns a snapshot equal to the first (a clone of the cache).
    #[test]
    fn second_get_within_ttl_does_not_recollect() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut cache = TelemetryCache::new(
            counting_collector(Arc::clone(&counter)),
            Duration::from_secs(5),
        );

        let first = cache.get();
        let second = cache.get();

        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "TTL not yet expired: no second collect"
        );
        assert_eq!(second, first, "the cached snapshot is returned by clone");
        assert_eq!(second, mock_snapshot());
    }

    /// (c) A `get()` after the TTL expires re-collects (counter == 2).
    /// A 1 ms TTL + a 20 ms sleep makes staleness deterministic without
    /// sleeping anywhere near the production 2 s default.
    #[test]
    fn get_after_ttl_expiry_recollects() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut cache = TelemetryCache::new(
            counting_collector(Arc::clone(&counter)),
            Duration::from_millis(1),
        );

        cache.get();
        std::thread::sleep(Duration::from_millis(20));
        let snap = cache.get();

        assert_eq!(
            counter.load(Ordering::SeqCst),
            2,
            "stale snapshot must be re-collected"
        );
        assert_eq!(snap, mock_snapshot());
    }

    /// (d) Returned values equal the mock's snapshot field-for-field,
    /// and the `ttl` accessor reports the configured TTL.
    #[test]
    fn returned_value_matches_mock_and_ttl_accessor_reports_config() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut cache = TelemetryCache::new(
            counting_collector(Arc::clone(&counter)),
            Duration::from_secs(3),
        );

        assert_eq!(cache.ttl(), Duration::from_secs(3));
        let snap = cache.get();

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
}
