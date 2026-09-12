# RamSleuth v2 — Developer Log / Lease Board

Base branch: `v2-development`. Plan: `plans/PLAN-PHASE2.md` (Phase 1 plan retained as `plans/PLAN.md`). Sign in/out under `@@@ ACTIVE_WORKERS @@@` per the lease protocol. Durable cycle history lives in `MASTER_LOG.md`.

@@@ ACTIVE_WORKERS @@@
@@@ HISTORY @@@
- [DONE] ID: P2-03 | STATUS: SUCCESS | BRANCH: branch/chunk-P2-03
DECISION: Implemented amd_smu.rs: frozen SmuContext{version,pm} + acquire() (AMD vendor gate before any I/O -> sysfs /sys/kernel/ryzen_smu/pm_table -> /dev/ryzen_smu char-dev read via nix open/read/close, FdGuard RAII, zero unsafe) + pure helpers vendor_gate/extract_version (LE u32 @ offset 0)/classify_io_error (NotFound->DriverMissing, PermissionDenied->InsufficientPrivilege, other->Io); added nix 0.29 as sole Phase-2 dep; wired pub mod amd_smu; acquire() on this host -> DriverMissing{ryzen_smu} (driver not loaded).
AHEAD: Two brief deviations for reviewer: (1) nix 0.29 has no "mmap"/"err" features — manifest uses real names [fs, ioctl, mman] (err = always-on errno), all pre-added so P2-06 need not touch Cargo.toml; (2) brief's "PM-table ioctl" implemented as the driver's read(2) uAPI — ryzen_smu's only ioctl is RSMU_IOC_GET_VERSION and the frozen version= blob-offset-0 contract precludes it as the version source (ioctl feature stays enabled per D6). Plan P2-02/P2-03 text still stale vs frozen error.rs — reconcile at review.
- [DONE] ID: P2-02 | STATUS: SUCCESS | BRANCH: branch/chunk-P2-02
DECISION: Implemented error.rs — TelemetryError (manual Clone/PartialEq/Eq: std io::Error is neither), Display, Error::source, TelemetryResult, NaReason, Section<T> no-panic contract; wired `pub mod error;` into lib.rs.
AHEAD: clippy clean, 14/14 tests green, release build ok; plan §D5/P2-02 text still shows the older variant list (UnsupportedVendor/NoDevmem/…/Section{Na(TelemetryError)}) — reconcile plan with this freeze at review/merge.
- [DONE] ID: review-P2-02 | STATUS: SUCCESS | BRANCH: v2-development
DECISION: Merged P2-02 (no-ff): code matches the shipped freeze — 6 TelemetryError variants, manual Clone/PartialEq/Eq sound (Io by kind+raw_os_code, Clone reconstructs), source() Some only for Io, NaReason(6), Section<T> value/is_na/na + From<T>→Value; zero warnings under clippy -D warnings, 14/14 tests, release build ok, no new deps, no hardware access.
AHEAD: plan text is stale vs the freeze (§D5 + P2-02 scope: old variants UnsupportedVendor/NoDevmem/InvalidValue, Section{Na(TelemetryError)} + is_value/as_option/reason) — reconcile via plan edit before P2-03; downstream P2-06/P2-07/P2-10 specs cite those removed identifiers.

@@@ CURRENT_STATE @@@
P2-02 merged to v2-development; ready for P2-03.
