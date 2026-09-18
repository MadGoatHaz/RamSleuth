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
/// - `warmed`: `false` until the first `get()` completes its warm-up
///   double-read; `true` forever after (C14, M2).
pub struct TelemetryCache {
    collector: Box<dyn Fn() -> SystemMemoryTelemetry + Send + Sync>,
    ttl: Duration,
    last: Option<SystemMemoryTelemetry>,
    last_at: Option<Instant>,
    warmed: bool,
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
            warmed: false,
        }
    }

    /// Return the current snapshot.
    ///
    /// Cold (`!warmed`) → the collector runs **twice**: the first
    /// (warm-up) result is discarded — its spike + settle wakes and
    /// settles the SMU — and the second is stored with
    /// [`Instant::now()`] and returned (C14, M2). Warm and fresh
    /// (`last_at.elapsed() < ttl`) → a **clone** of the cached
    /// snapshot, no collector call. Warm and stale → re-collect once,
    /// store the result plus [`Instant::now()`], and return it. Never
    /// panics by itself (a panicking collector is the collector's
    /// fault — the real `collect()` is no-panic by the Phase 2
    /// contract).
    pub fn get(&mut self) -> SystemMemoryTelemetry {
        if self.warmed {
            if let (Some(last), Some(last_at)) = (&self.last, &self.last_at) {
                if last_at.elapsed() < self.ttl {
                    return last.clone();
                }
            }
        } else {
            // Cold cache: warm-up read (discarded) — its spike+settle+read
            // wakes and settles the SMU — then the settled read is cached.
            let _warmup = (self.collector)();
            self.warmed = true;
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

    /// (a) The first `get()` on a cold cache calls the collector twice
    /// (warm-up + settled read) and returns the second call's value.
    #[test]
    fn first_get_warms_then_collects() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut cache = TelemetryCache::new(
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
        let mut cache = TelemetryCache::new(
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
        let mut cache = TelemetryCache::new(
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
        let mut cache = TelemetryCache::new(
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
}
