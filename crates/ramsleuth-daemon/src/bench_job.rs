//! Single-flight benchmark job manager (P3-15, plan D6): at most one
//! benchmark runs at a time — a bandwidth run monopolizes every
//! pinned core, so a concurrent run is meaningless. A second
//! [`BenchJobManager::start`] while a run is active fails with
//! [`JobError::Busy`], which the daemon maps onto the wire's
//! `Response::Error("benchmark already running")` (P3-16).
//!
//! Each run executes the frozen P3-09 contract [`run_streamed`] on a
//! blocking-pool thread ([`tokio::task::spawn_blocking`] — the run is
//! CPU-bound and must not occupy a runtime worker) and streams its
//! events on the [`JobHandle`] channel: one [`JobEvent::Progress`] per
//! completed bandwidth cell (the P3-09 [`StreamProgress`] verbatim)
//! and exactly one terminal event — [`JobEvent::Result`] (the measured
//! [`BenchmarkGrid`]), [`JobEvent::Cancelled`] (the run's cancel flag
//! was set at a P3-09 gate), or [`JobEvent::Error`] (a
//! [`StreamError`]'s `Display` text, or the blocking task's join
//! failure — no-panic contract, plan D5: a failed run ends with a
//! structured event, never a panic). After the terminal event the job
//! drops every sender, closing the channel and ending the owner's
//! event iteration deterministically.
//!
//! # (target, mode) mapping
//!
//! The protocol's `Request::StartBenchmark` carries a
//! [`StreamTarget`] plus a [`BenchMode`]; this module is the single
//! place that folds the pair onto the effective [`StreamTarget`]
//! (plan D6): `Full` keeps the requested target as-is, and
//! `MemoryOnly` restricts the run to the memory tier — `Full` /
//! `Tier(·)` → the memory tier (its three cells plus its latency
//! pass), `Cell(·, op)` → the memory tier's `op` cell (the op is
//! preserved, the tier clamped).
//!
//! # Cancellation
//!
//! [`BenchJobManager::cancel`] sets the run's cancel flag (relaxed);
//! the in-flight pass (milliseconds) always finishes and the run
//! stops at the next P3-09 gate with [`StreamError::Cancelled`] — a
//! clean stop between passes/cells, never a mid-pass kill (plan D6).
//!
//! # Slot release
//!
//! The shared state lives in `Arc`s, so the spawned job releases the
//! slot itself on completion (clearing `active` only if it still
//! holds this run's id, then dropping `busy`) — the manager is
//! borrow-free and re-entrant: a new `start` may succeed immediately
//! after a terminal event.

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use ramsleuth_bench::{
    run_streamed, BenchmarkGrid, StreamError, StreamOptions, StreamProgress, StreamTarget, Tier,
};
use ramsleuth_protocol::BenchMode;

/// The single-flight slot: the active run's id + its cancel flag, or
/// `None` while no run is active.
type ActiveSlot = Option<(u64, Arc<AtomicBool>)>;

/// One event on a run's stream: a progress tick or the terminal.
///
/// The owner connection (P3-16) receives every [`Progress`] in order
/// plus exactly one terminal event; the channel closes immediately
/// after (all senders dropped by the job), so iteration over
/// [`JobHandle::events`] always ends with the terminal.
#[derive(Debug, Clone, PartialEq)]
pub enum JobEvent {
    /// One completed bandwidth cell (the P3-09 [`StreamProgress`]
    /// verbatim — the protocol wraps it with the `run_id`).
    Progress(StreamProgress),
    /// Terminal: the run completed; `grid` holds the measured cells
    /// (unrequested cells stay `0.0`, P3-09 contract).
    Result(BenchmarkGrid),
    /// Terminal: the run's cancel flag was set at a P3-09 gate; the
    /// in-flight pass finished and the partial grid was discarded.
    Cancelled,
    /// Terminal: the run failed — a [`StreamError`]'s `Display` text
    /// (worker failure, topology detection) or the blocking task's
    /// join error. Never a panic (plan D5).
    Error(String),
}

/// The client-facing handle for one started run.
///
/// `run_id` tags the wire's `BenchStarted` / `CancelBenchmark` frames;
/// `events` yields the run's [`JobEvent`] stream (progress + exactly
/// one terminal) and then closes (all senders dropped by the job
/// after its terminal). The receiver is neither `Clone` nor `Sync`:
/// the owner connection is the sole consumer (plan D6).
pub struct JobHandle {
    /// This run's daemon-assigned id (monotonic, first run is 1).
    pub run_id: u64,
    /// The run's event stream (see the type docs).
    pub events: mpsc::Receiver<JobEvent>,
}

/// Failure to start a benchmark run.
#[derive(Debug, Clone, PartialEq)]
pub enum JobError {
    /// Single-flight: a run is already active (the daemon maps this to
    /// the wire's `Response::Error("benchmark already running")`).
    Busy,
    /// The run task could not be spawned. Defensive arm: this `async`
    /// fn is only ever polled by a tokio runtime (P3-16's accept
    /// loop), so `tokio::spawn` cannot fail in practice — the arm
    /// exists so a future runtime-context change degrades to a
    /// structured error instead of a panic.
    Spawn(String),
}

impl fmt::Display for JobError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => write!(f, "benchmark already running"),
            Self::Spawn(msg) => write!(f, "failed to spawn benchmark job: {msg}"),
        }
    }
}

impl std::error::Error for JobError {}

/// The daemon's single-flight benchmark job manager (plan D6).
///
/// At most one run is active at a time; [`start`] claims the slot
/// atomically and refuses a concurrent run with [`JobError::Busy`].
/// The shared state lives in `Arc`s so the spawned job releases the
/// slot itself on completion without borrowing the manager — the
/// manager is cheaply `Clone`-able (the daemon holds one instance
/// behind the P3-16 accept loop).
///
/// - `busy`: the single-flight claim (swapped on `start`, cleared by
///   the job on its terminal event);
/// - `next_run_id`: the monotonic run-id source (first run is 1);
/// - `active`: the active run's id + cancel flag (read by `cancel`,
///   cleared by the job).
#[derive(Clone)]
pub struct BenchJobManager {
    busy: Arc<AtomicBool>,
    next_run_id: Arc<AtomicU64>,
    active: Arc<Mutex<ActiveSlot>>,
}

impl Default for BenchJobManager {
    fn default() -> Self {
        Self::new()
    }
}

impl BenchJobManager {
    /// A fresh manager: no active run, next run id 1.
    pub fn new() -> Self {
        Self {
            busy: Arc::new(AtomicBool::new(false)),
            next_run_id: Arc::new(AtomicU64::new(1)),
            active: Arc::new(Mutex::new(None)),
        }
    }

    /// Start a benchmark run (single-flight).
    ///
    /// Claims the slot, allocates a `run_id`, registers the run's
    /// cancel flag, and spawns the job: `run_streamed` (P3-09) on a
    /// blocking-pool thread, streaming one [`JobEvent`] per completed
    /// cell plus exactly one terminal; the job then releases the slot.
    /// Returns the owner's [`JobHandle`].
    ///
    /// `target` + `mode` fold onto the effective [`StreamTarget`] (see
    /// the module docs): `Full` keeps `target` as-is, `MemoryOnly`
    /// restricts the run to the memory tier. `threads` bounds the
    /// pinned workers (P3-09: `0` = auto, one per detected physical
    /// core).
    ///
    /// # Errors
    ///
    /// [`JobError::Busy`] when a run is already active;
    /// [`JobError::Spawn`] if the run task could not be spawned
    /// (defensive — see the arm docs).
    pub async fn start(
        &self,
        target: StreamTarget,
        mode: BenchMode,
        threads: usize,
    ) -> Result<JobHandle, JobError> {
        // Single-flight claim: `swap` reports the previous state, so
        // exactly one concurrent `start` wins; the rejection path
        // changes no state, and nothing after this point can fail, so
        // no rollback is needed.
        if self.busy.swap(true, Ordering::AcqRel) {
            return Err(JobError::Busy);
        }

        let run_id = self.next_run_id.fetch_add(1, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        *lock(&self.active) = Some((run_id, Arc::clone(&cancel)));

        let options = StreamOptions {
            target: mapped_target(target, mode),
            threads,
            cancel: Arc::clone(&cancel),
        };

        // Two channels, one owner: the P3-09 progress channel carries
        // `StreamProgress` into `run_streamed`; the job pumps it into
        // the owner's `JobEvent` channel (in order) and then sends the
        // single terminal, so the owner sees progress in order + the
        // terminal last.
        let (tx, rx) = mpsc::channel::<JobEvent>();
        let (progress_tx, progress_rx) = mpsc::channel::<StreamProgress>();

        let active = Arc::clone(&self.active);
        let busy = Arc::clone(&self.busy);

        tokio::spawn(async move {
            let outcome =
                tokio::task::spawn_blocking(move || run_streamed(&options, Some(progress_tx)))
                    .await;
            // `run_streamed` has returned, so its progress sender is
            // dropped: drain every buffered progress event (FIFO
            // order) into the owner channel before the terminal, so
            // the terminal is always last.
            while let Ok(p) = progress_rx.recv() {
                let _ = tx.send(JobEvent::Progress(p));
            }
            let terminal = match outcome {
                Ok(Ok(grid)) => JobEvent::Result(grid),
                Ok(Err(StreamError::Cancelled)) => JobEvent::Cancelled,
                Ok(Err(err)) => JobEvent::Error(err.to_string()),
                // The blocking task panicked: caught here as a
                // structured event (no-panic contract, plan D5) —
                // `run_streamed` is no-panic by design, so this is
                // defensive.
                Err(join) => JobEvent::Error(format!("benchmark job task failed: {join}")),
            };
            // The receiver may already be gone (owner disconnected) —
            // the send is best-effort; dropping `tx` below closes the
            // channel either way.
            let _ = tx.send(terminal);
            release(&active, run_id, &busy);
        });

        Ok(JobHandle {
            run_id,
            events: rx,
        })
    }

    /// Cancel the active run `run_id`: set its cancel flag (relaxed)
    /// so the run stops at its next P3-09 gate (the in-flight pass
    /// finishes first — clean stop, plan D6).
    ///
    /// Returns `true` when the run was active and the flag was set;
    /// `false` for an unknown id or a run that already terminated and
    /// released the slot.
    pub fn cancel(&self, run_id: u64) -> bool {
        let mut slot = lock(&self.active);
        match slot.as_mut() {
            Some((id, flag)) if *id == run_id => {
                flag.store(true, Ordering::Relaxed);
                true
            }
            _ => false,
        }
    }
}

/// Lock the active slot, recovering from poisoning instead of
/// panicking: a poisoned slot means a prior holder panicked, but the
/// daemon's no-panic contract (plan D5) takes the lock anyway — the
/// slot contents remain structurally consistent.
fn lock(active: &Mutex<ActiveSlot>) -> MutexGuard<'_, ActiveSlot> {
    active.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Release the single-flight slot after a run terminates: clear
/// `active` only if it still holds this run's id (invariant hygiene —
/// no newer run can hold the slot while this one ran, but a stale
/// entry must never be cleared by a late arriver), then drop `busy`
/// so the next `start` can claim the slot.
fn release(active: &Mutex<ActiveSlot>, run_id: u64, busy: &AtomicBool) {
    let mut slot = lock(active);
    if matches!(*slot, Some((id, _)) if id == run_id) {
        *slot = None;
    }
    busy.store(false, Ordering::Release);
}

/// Fold the protocol's `(target, mode)` pair onto the effective
/// [`StreamTarget`] (plan D6, single source): `Full` keeps the
/// requested target as-is; `MemoryOnly` restricts the run to the
/// memory tier — `Full` / `Tier(·)` → the memory tier (its three cells
/// plus its latency pass), `Cell(·, op)` → the memory tier's `op`
/// cell (op preserved, tier clamped).
fn mapped_target(target: StreamTarget, mode: BenchMode) -> StreamTarget {
    match mode {
        BenchMode::Full => target,
        BenchMode::MemoryOnly => match target {
            StreamTarget::Full | StreamTarget::Tier(_) => StreamTarget::Tier(Tier::Memory),
            StreamTarget::Cell(_, op) => StreamTarget::Cell(Tier::Memory, op),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use ramsleuth_bench::{BenchOp, Metric};

    use super::*;

    /// Drain a handle's full event stream (progress + exactly one
    /// terminal) by moving the blocking std receiver onto a forwarder
    /// task and awaiting its completion, with a 30 s guard: the stream
    /// closes when the job drops all senders after its terminal event,
    /// so a healthy run always ends — the guard turns a real hang into
    /// a test failure. The forwarder needs a multi-thread runtime
    /// (every async test below uses one): the forwarder thread-blocks
    /// in `recv` while waiting, so the job's continuation must be able
    /// to run on a different worker.
    async fn collect(handle: JobHandle) -> Vec<JobEvent> {
        let events = handle.events;
        let forwarder = tokio::spawn(async move {
            let mut out = Vec::new();
            while let Ok(ev) = events.recv() {
                out.push(ev);
            }
            out
        });
        match tokio::time::timeout(Duration::from_secs(30), forwarder).await {
            Ok(Ok(out)) => out,
            other => panic!("event stream did not close within 30 s: {other:?}"),
        }
    }

    /// (a) `start` on a fresh manager returns a handle with a run id,
    /// and the stream yields the single-cell progress event followed
    /// by exactly one terminal `Result` grid (the measured cell
    /// filled). Minimal real target: one L1 Read cell, one pinned
    /// worker — the smallest run `run_streamed` accepts.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn start_streams_one_progress_then_terminal_result() {
        let mgr = BenchJobManager::new();
        let handle = mgr
            .start(StreamTarget::Cell(Tier::L1, BenchOp::Read), BenchMode::Full, 1)
            .await
            .expect("first start on a fresh manager must succeed");
        assert!(handle.run_id > 0);

        let events = collect(handle).await;
        assert_eq!(events.len(), 2, "one progress + one terminal: {events:?}");
        match &events[0] {
            JobEvent::Progress(p) => {
                assert_eq!(p.tier, Tier::L1);
                assert_eq!(p.op, BenchOp::Read);
                assert_eq!(p.cell_index, 0);
                assert_eq!(p.total_cells, 1);
                assert!(p.value.is_finite() && p.value > 0.0);
            }
            other => panic!("first event must be the cell's Progress, got {other:?}"),
        }
        match &events[1] {
            JobEvent::Result(grid) => {
                assert!(grid.cell(Tier::L1, Metric::Read) > 0.0);
            }
            other => panic!("terminal must be the Result grid, got {other:?}"),
        }
    }

    /// (b) Single-flight: a second `start` while the first run is
    /// still active is refused with `Busy`, and the first run still
    /// terminates normally with `Result`. The first run is a whole
    /// Memory tier on one pinned worker (3 bandwidth passes over
    /// >=256 MiB + the 128 MiB latency chase) — reliably still running
    /// when the second `start` arrives microseconds later.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn second_start_while_running_is_busy() {
        let mgr = BenchJobManager::new();
        let first = mgr
            .start(StreamTarget::Tier(Tier::Memory), BenchMode::Full, 1)
            .await
            .expect("first start must succeed");

        let second = mgr
            .start(StreamTarget::Cell(Tier::L1, BenchOp::Read), BenchMode::Full, 1)
            .await;
        assert!(
            matches!(second, Err(JobError::Busy)),
            "a concurrent start must be refused single-flight"
        );

        let events = collect(first).await;
        assert!(
            matches!(events.last(), Some(JobEvent::Result(_))),
            "the first run must still complete with a Result: {events:?}"
        );
    }

    /// (c) After a run's terminal event the slot is released: a new
    /// `start` succeeds again and the run ids advance monotonically
    /// (a refused `start` consumes none).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn start_succeeds_again_after_terminal_event() {
        let mgr = BenchJobManager::new();
        let first = mgr
            .start(StreamTarget::Cell(Tier::L1, BenchOp::Read), BenchMode::Full, 1)
            .await
            .expect("first start must succeed");
        let first_id = first.run_id;
        let events = collect(first).await;
        assert!(
            matches!(events.last(), Some(JobEvent::Result(_))),
            "first run must complete: {events:?}"
        );

        let second = mgr
            .start(StreamTarget::Cell(Tier::L1, BenchOp::Write), BenchMode::Full, 1)
            .await
            .expect("start after the terminal event must succeed (busy cleared)");
        assert_eq!(second.run_id, first_id + 1, "run ids must advance monotonically");

        let events = collect(second).await;
        assert_eq!(events.len(), 2, "one progress + one terminal: {events:?}");
        match &events[1] {
            JobEvent::Result(grid) => {
                assert!(grid.cell(Tier::L1, Metric::Write) > 0.0);
            }
            other => panic!("terminal must be the Result grid, got {other:?}"),
        }
    }

    /// (d) `cancel` on the active run returns `true` and the stream
    /// ends with `Cancelled`: the Memory tier on one worker is large
    /// enough that the flag (set microseconds after `start`) lands at
    /// a P3-09 gate before the run could finish. `cancel` on an
    /// unknown id — before or after the terminal — returns `false`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancel_active_run_ends_stream_with_cancelled() {
        let mgr = BenchJobManager::new();
        assert!(!mgr.cancel(9999), "no active run: cancel must be false");

        let handle = mgr
            .start(StreamTarget::Tier(Tier::Memory), BenchMode::Full, 1)
            .await
            .expect("start must succeed");
        let run_id = handle.run_id;
        assert!(mgr.cancel(run_id), "cancel of the active run_id must return true");
        assert!(!mgr.cancel(run_id + 1), "cancel of an unknown run_id must return false");

        let events = collect(handle).await;
        assert!(
            matches!(events.last(), Some(JobEvent::Cancelled)),
            "the cancelled run must end with the Cancelled terminal: {events:?}"
        );
        assert!(
            !mgr.cancel(run_id),
            "after the terminal the slot is released: cancel must be false"
        );
    }

    /// (e) The `(target, mode)` fold: `Full` keeps the requested
    /// target as-is; `MemoryOnly` restricts the run to the memory
    /// tier (a `Cell` keeps its op, the tier clamped).
    #[test]
    fn mapped_target_folds_mode_onto_the_memory_tier() {
        // Full scope: the requested target, as-is.
        assert_eq!(mapped_target(StreamTarget::Full, BenchMode::Full), StreamTarget::Full);
        assert_eq!(
            mapped_target(StreamTarget::Tier(Tier::L2), BenchMode::Full),
            StreamTarget::Tier(Tier::L2)
        );
        assert_eq!(
            mapped_target(StreamTarget::Cell(Tier::L3, BenchOp::Copy), BenchMode::Full),
            StreamTarget::Cell(Tier::L3, BenchOp::Copy)
        );
        // MemoryOnly: restricted to the memory tier.
        assert_eq!(
            mapped_target(StreamTarget::Full, BenchMode::MemoryOnly),
            StreamTarget::Tier(Tier::Memory)
        );
        assert_eq!(
            mapped_target(StreamTarget::Tier(Tier::L1), BenchMode::MemoryOnly),
            StreamTarget::Tier(Tier::Memory)
        );
        assert_eq!(
            mapped_target(StreamTarget::Tier(Tier::Memory), BenchMode::MemoryOnly),
            StreamTarget::Tier(Tier::Memory)
        );
        assert_eq!(
            mapped_target(StreamTarget::Cell(Tier::L2, BenchOp::Read), BenchMode::MemoryOnly),
            StreamTarget::Cell(Tier::Memory, BenchOp::Read)
        );
    }

    /// (f) `JobError` displays the wire's single-flight text (the
    /// daemon maps `Busy` onto `Response::Error` verbatim) and
    /// implements `std::error::Error`.
    #[test]
    fn job_error_display_and_error_trait() {
        assert_eq!(JobError::Busy.to_string(), "benchmark already running");
        let spawn = JobError::Spawn("no runtime".to_owned());
        assert!(spawn.to_string().contains("no runtime"));
        assert!(matches!(spawn.clone(), JobError::Spawn(_)));
        let as_err: &dyn std::error::Error = &JobError::Busy;
        assert_eq!(as_err.to_string(), "benchmark already running");
    }
}
