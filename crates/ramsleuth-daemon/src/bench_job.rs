//! Single-flight benchmark job manager (P3-15, plan D6): at most one
//! run of either class (a single-pass benchmark or a multi-pass
//! burn-in, C7-07/D-1) is active at a time — a bandwidth run
//! monopolizes every pinned core, so a concurrent run is meaningless.
//! A second [`BenchJobManager::start`] / [`BenchJobManager::start_burn_in`]
//! while a run is active fails with [`JobError::Busy`], which the
//! daemon maps onto the wire's
//! `Response::Error("benchmark already running")` (P3-16).
//!
//! Each benchmark run executes the frozen P3-09 contract
//! [`run_streamed`] and each burn-in run the C7-06 contract
//! [`run_burn_in`], both on a blocking-pool thread
//! ([`tokio::task::spawn_blocking`] — the run is CPU-bound and must
//! not occupy a runtime worker) and both stream their events on the
//! [`JobHandle`] channel: one [`JobEvent::Progress`] (the P3-09
//! [`StreamProgress`] verbatim) per completed bandwidth cell of a
//! benchmark run, one [`JobEvent::BurnInTick`] (the C7-06
//! [`BurnInTick`] verbatim) per completed cell / per-tier latency pass
//! of each burn-in iteration, and exactly one terminal event —
//!
//! **Burn-in cadence (FE-1, single-source clock):** the daemon is the
//! authoritative source of the burn-in elapsed clock. In addition to the
//! per-cell / per-latency ticks, a ~1 s timer emits an **elapsed-update**
//! [`BurnInTick`] (both `bandwidth` and `latency_ns` `None` — the marker
//! an owner uses to tell it from a data tick) carrying the run's
//! `elapsed_secs` stamped from the daemon's own start instant. The ticks
//! are forwarded to the owner **in real time** (a forwarder streams the
//! run's tick channel as it lands, not a final burst), so the frontends'
//! burn-in clock advances smoothly even when no cell completes for a
//! while — the owner renders `m:ss` from `elapsed_secs`, never a
//! client-side clock.
//! [`JobEvent::Result`] (the measured [`BenchmarkGrid`] — for a
//! burn-in, its last completed pass), [`JobEvent::Cancelled`] (the
//! run's cancel flag was set at a gate), or [`JobEvent::Error`] (a
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
use std::time::{Duration, Instant};

use ramsleuth_bench::{
    run_burn_in, run_streamed, BenchmarkGrid, BurnInOptions, BurnInTick, StreamError,
    StreamOptions, StreamProgress, StreamTarget, Tier,
};
use ramsleuth_protocol::BenchMode;

/// The maximum sanctioned burn-in duration, in minutes (DAEMON-4): 24
/// hours. The client's `burn` command clamps to this range as a
/// convenience, but the **privileged daemon re-clamps** the wire value it
/// acts on — a hostile client cannot push a `u32::MAX` duration onto the
/// run (which the bench crate's `duration_deadline` would turn into a
/// ~138-year deadline). `0` stays `0` (infinite, cancel-only);
/// `> 1440` is capped at `1440`.
const MAX_BURN_IN_MINUTES: u32 = 1440;

/// Re-clamp a client-supplied burn-in duration to the sanctioned
/// `0..=1440` range (DAEMON-4): the server enforces its own invariant on
/// a wire value it acts on (the client clamp is convenience only).
/// `0` is infinite (cancel-only) and passes through unchanged; a value
/// above [`MAX_BURN_IN_MINUTES`] is capped. Pure and total (no I/O, no
/// panic) — unit-testable.
pub(crate) fn clamp_burn_in_duration(minutes: u32) -> u32 {
    minutes.min(MAX_BURN_IN_MINUTES)
}

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
    /// One burn-in tick (the C7-06 [`BurnInTick`] verbatim — the
    /// protocol wraps it in its `BurnInProgress` arm, D-2): one per
    /// completed bandwidth cell and per completed per-tier latency
    /// pass of each burn-in iteration, **plus** the ~1 s elapsed-update
    /// tick (FE-1: both `bandwidth` and `latency_ns` `None`, carrying
    /// only the daemon's `elapsed_secs`) that keeps the owner's clock
    /// smooth between cell completions.
    BurnInTick(BurnInTick),
    /// Terminal: the run completed; `grid` holds the measured cells
    /// (unrequested cells stay `0.0`, P3-09 contract; for a burn-in,
    /// the grid of its last completed pass).
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
    /// The run's event stream (see the type docs: progress or
    /// burn-in ticks, in order, plus exactly one terminal).
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
        let (run_id, cancel) = self.claim_slot()?;
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

    /// Start a burn-in run (single-flight, D-1): the same slot
    /// claim, `run_id` allocation, cancel-flag registration, and slot
    /// self-release as [`start`] — a burn-in and a normal benchmark
    /// are mutually exclusive.
    ///
    /// The job runs [`run_burn_in`] (C7-06) on a blocking-pool
    /// thread, streaming one [`JobEvent::BurnInTick`] per completed
    /// bandwidth cell and per completed per-tier latency pass of each
    /// iteration plus exactly one terminal: [`JobEvent::Result`] (the
    /// grid of the last completed pass) on the duration deadline,
    /// [`JobEvent::Cancelled`] when the run's cancel flag is set at a
    /// gate, or [`JobEvent::Error`] (no-panic contract, plan D5).
    /// Returns the owner's [`JobHandle`].
    ///
    /// `target` selects the cells every pass runs (the protocol's
    /// `StartBurnIn` arm carries it verbatim — no `BenchMode` fold: a
    /// burn-in is always a full-scope duration run, D-1). `threads`
    /// bounds the pinned workers (C7-06: `0` = auto, one per detected
    /// physical core). `duration_minutes` = `0` means infinite
    /// (stop only via [`BenchJobManager::cancel`]).
    ///
    /// # Errors
    ///
    /// [`JobError::Busy`] when a run is already active;
    /// [`JobError::Spawn`] if the run task could not be spawned
    /// (defensive — see the arm docs).
    pub async fn start_burn_in(
        &self,
        target: StreamTarget,
        duration_minutes: u32,
        threads: usize,
    ) -> Result<JobHandle, JobError> {
        // DAEMON-4: the privileged daemon re-clamps the client-supplied
        // duration to the sanctioned `0..=1440` range *before* it acts on
        // it (the client clamp is convenience only) — a hostile `u32::MAX`
        // cannot become a ~138-year burn-in deadline. `0` stays `0`
        // (infinite, cancel-only).
        let duration_minutes = clamp_burn_in_duration(duration_minutes);
        let (run_id, cancel) = self.claim_slot()?;
        let options = BurnInOptions {
            target,
            duration_minutes,
            threads,
            cancel: Arc::clone(&cancel),
        };

        // Three producers, one owner channel (`tx`): (1) `run_burn_in`
        // on a blocking thread sends the per-cell / per-latency ticks
        // into `tick_tx`; (2) a forwarder streams `tick_rx` into `tx` in
        // real time (the owner sees the live grid, not a final burst);
        // (3) a ~1 s elapsed-update timer (FE-1) sends an
        // `elapsed_secs`-only tick so the owner's clock advances
        // smoothly between cell completions. The terminal is sent last,
        // after the run, the forwarder, and the timer have all stopped.
        let (tx, rx) = mpsc::channel::<JobEvent>();
        let (tick_tx, tick_rx) = mpsc::channel::<BurnInTick>();

        let active = Arc::clone(&self.active);
        let busy = Arc::clone(&self.busy);

        // The run's start instant: the daemon's burn-in clock source
        // (FE-1) — the elapsed-update timer stamps `elapsed_secs` from
        // it, so the clock is the daemon's, never a client's.
        let run_start = Instant::now();

        tokio::spawn(async move {
            // `finished` is set once `run_burn_in` has returned (its
            // tick sender dropped); the timer + forwarder stop on it so
            // the terminal is always last.
            let finished = Arc::new(AtomicBool::new(false));

            // (1) The per-cell / per-latency ticks from `run_burn_in`.
            let run_task =
                tokio::task::spawn_blocking(move || run_burn_in(&options, Some(tick_tx)));

            // (2) Real-time forwarder: stream `tick_rx` into `tx` as the
            // ticks land. It exits when `tick_tx` is dropped (the run
            // finished), having flushed every buffered tick. The daemon
            // is the single source of the burn-in clock (FE-1): each
            // per-cell tick's `elapsed_secs` is re-stamped from
            // `run_start` (the bench crate stamps it from its own,
            // slightly-later start) so the whole stream is monotonic by
            // construction.
            let forwarder = tokio::spawn({
                let tx = tx.clone();
                async move {
                    while let Ok(mut tick) = tick_rx.recv() {
                        tick.elapsed_secs = (Instant::now() - run_start).as_secs_f64();
                        let _ = tx.send(JobEvent::BurnInTick(tick));
                    }
                }
            });

            // (3) The ~1 s elapsed-update timer (FE-1 smooth cadence):
            // each fire sends an `elapsed_secs`-only tick (both
            // `bandwidth` and `latency_ns` `None`) stamped from
            // `run_start`. It stops on `finished`.
            let timer = tokio::spawn({
                let tx = tx.clone();
                let finished = Arc::clone(&finished);
                async move {
                    let mut iv = tokio::time::interval(Duration::from_secs(1));
                    iv.tick().await; // the first tick fires immediately
                    loop {
                        if finished.load(Ordering::Relaxed) {
                            break;
                        }
                        iv.tick().await;
                        if finished.load(Ordering::Relaxed) {
                            break;
                        }
                        let elapsed = (Instant::now() - run_start).as_secs_f64();
                        let _ = tx.send(JobEvent::BurnInTick(elapsed_update_tick(
                            elapsed,
                        )));
                    }
                }
            });

            let outcome = run_task.await;
            // The run is over: stop the timer, let the forwarder flush
            // the remaining buffered ticks, then send the terminal last.
            finished.store(true, Ordering::Relaxed);
            let _ = forwarder.await;
            let _ = timer.await;
            let terminal = match outcome {
                Ok(Ok(grid)) => JobEvent::Result(grid),
                Ok(Err(StreamError::Cancelled)) => JobEvent::Cancelled,
                Ok(Err(err)) => JobEvent::Error(err.to_string()),
                // The blocking task panicked: caught here as a
                // structured event (no-panic contract, plan D5) —
                // `run_burn_in` is no-panic by design, so this is
                // defensive.
                Err(join) => JobEvent::Error(format!("burn-in job task failed: {join}")),
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

    /// Claim the single-flight slot for a new run (the shared claim of
    /// [`start`] + [`start_burn_in`], D-1): `swap` reports the
    /// previous state, so exactly one concurrent claim wins; the
    /// rejection path changes no state, and nothing after this point
    /// can fail, so no rollback is needed. On success, registers the
    /// new run's id + cancel flag in `active` and returns them.
    fn claim_slot(&self) -> Result<(u64, Arc<AtomicBool>), JobError> {
        if self.busy.swap(true, Ordering::AcqRel) {
            return Err(JobError::Busy);
        }
        let run_id = self.next_run_id.fetch_add(1, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        *lock(&self.active) = Some((run_id, Arc::clone(&cancel)));
        Ok((run_id, cancel))
    }

    /// Cancel the active run `run_id`: set its cancel flag (relaxed)
    /// so the run stops at its next gate (the in-flight pass finishes
    /// first — clean stop, plan D6).
    ///
    /// Serves both run classes (a benchmark and a burn-in share the
    /// one slot, D-1).
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
/// cell (op preserved, tier clamped), `Latency(·)` → the memory tier's
/// latency pass (BENCH-G1, tier clamped).
fn mapped_target(target: StreamTarget, mode: BenchMode) -> StreamTarget {
    match mode {
        BenchMode::Full => target,
        BenchMode::MemoryOnly => match target {
            StreamTarget::Full | StreamTarget::Tier(_) => StreamTarget::Tier(Tier::Memory),
            StreamTarget::Cell(_, op) => StreamTarget::Cell(Tier::Memory, op),
            StreamTarget::Latency(_) => StreamTarget::Latency(Tier::Memory),
        },
    }
}

/// The ~1 s **elapsed-update** burn-in tick (FE-1): carries only the
/// daemon's `elapsed_secs` (stamped from the run's start instant) with
/// both `bandwidth` and `latency_ns` `None` — the marker an owner uses to
/// tell it from a data tick. The `iteration` / `tier` are dummies (the
/// owner ignores them for an elapsed-update tick and updates only its
/// clock). No new wire field: this is the C7-06 [`BurnInTick`] verbatim,
/// just a new emission cadence.
fn elapsed_update_tick(elapsed_secs: f64) -> BurnInTick {
    BurnInTick {
        iteration: 0,
        elapsed_secs,
        tier: Tier::Memory,
        bandwidth: None,
        latency_ns: None,
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

    /// (g) A burn-in (C7-07/D-1) streams its ticks in real time and ends
    /// with the `Cancelled` terminal when cancelled: `duration_minutes = 0`
    /// is infinite (cancel-only stop). The test cancels the run itself
    /// (a short async sleep — no worker is blocked) and then collects the
    /// whole stream: the data ticks (≥ one completed iteration on any sane
    /// host) + the ~1 s elapsed-update ticks (FE-1) in order, then the
    /// `Cancelled` terminal, then the released slot. The `Result`-terminal
    /// mapping is the shared code exercised by (a); a burn-in duration
    /// terminal needs ≥ 1 minute of wall time at the wire's minute
    /// granularity, so it is not unit-tested here.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn start_burn_in_streams_ticks_then_cancelled() {
        let mgr = BenchJobManager::new();
        let handle = mgr
            .start_burn_in(StreamTarget::Tier(Tier::L1), 0, 1)
            .await
            .expect("first burn-in on a fresh manager must succeed");
        let run_id = handle.run_id;

        // Let the run complete at least one iteration (L1 passes are
        // fast), then cancel it: the infinite run is still active, so the
        // cancel must land. The sleep is 1.5 s (not 1.0 s) so the FE-1
        // ~1 s elapsed-update timer's *first* send (at ~1.0 s) is
        // guaranteed to land **before** the cancel — cancelling at exactly
        // 1.0 s raced the timer under load (the run could stop and set
        // `finished` before the timer's 1 s check, dropping the tick),
        // which made the `!elapsed.is_empty()` assertion flaky.
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(mgr.cancel(run_id), "the active burn-in must be cancellable");

        let events = collect(handle).await;
        assert!(
            matches!(events.last(), Some(JobEvent::Cancelled)),
            "the cancelled burn-in must end with the Cancelled terminal: {events:?}"
        );
        let ticks: Vec<&BurnInTick> = events
            .iter()
            .filter_map(|ev| match ev {
                JobEvent::BurnInTick(t) => Some(t),
                _ => None,
            })
            .collect();
        // The stream carries two tick kinds: the data ticks (exactly one
        // of `bandwidth` / `latency_ns` `Some`) and the ~1 s
        // elapsed-update ticks (both `None` — the FE-1 marker).
        let data: Vec<&BurnInTick> = ticks
            .iter()
            .copied()
            .filter(|t| t.bandwidth.is_some() != t.latency_ns.is_some())
            .collect();
        let elapsed: Vec<&BurnInTick> = ticks
            .iter()
            .copied()
            .filter(|t| t.bandwidth.is_none() && t.latency_ns.is_none())
            .collect();

        assert!(
            !data.is_empty(),
            "a second of L1 iterations must stream data ticks: {events:?}"
        );
        assert!(
            !elapsed.is_empty(),
            "the ~1 s timer must stream elapsed-update ticks (FE-1): {events:?}"
        );

        // The data ticks: a tier target only ticks its tier, exactly one
        // of bandwidth / latency, iterations 1-based and non-decreasing.
        for t in &data {
            assert_eq!(t.tier, Tier::L1, "a tier target only ticks its tier");
            assert!(
                t.bandwidth.is_some() != t.latency_ns.is_some(),
                "exactly one of bandwidth / latency per data tick"
            );
            assert!(t.iteration >= 1, "iterations are 1-based");
            assert!(
                t.elapsed_secs.is_finite() && t.elapsed_secs >= 0.0,
                "elapsed must be finite and non-negative: {}",
                t.elapsed_secs
            );
        }
        for w in data.windows(2) {
            assert!(
                w[1].iteration >= w[0].iteration,
                "iterations must not decrease"
            );
        }
        // The elapsed-update ticks: both `None`, the daemon's clock —
        // finite, non-negative, and non-decreasing (the forwarder
        // re-stamps from `run_start`, so the whole stream is monotonic).
        for t in &elapsed {
            assert!(
                t.bandwidth.is_none() && t.latency_ns.is_none(),
                "an elapsed-update tick carries no data"
            );
            assert!(
                t.elapsed_secs.is_finite() && t.elapsed_secs >= 0.0,
                "elapsed must be finite and non-negative: {}",
                t.elapsed_secs
            );
        }
        for w in elapsed.windows(2) {
            assert!(
                w[1].elapsed_secs + 0.001 >= w[0].elapsed_secs,
                "elapsed-update must not decrease"
            );
        }
        // After the terminal the slot is released: cancel is false.
        assert!(
            !mgr.cancel(run_id),
            "after the terminal the slot is released: cancel must be false"
        );
    }

    /// TC-FE1 (daemon cadence, the FE-1 confirming test): a burn-in over
    /// the slow Memory tier (no cell completes for >2 s) still emits the
    /// elapsed stream at ≤1 s cadence — the ~1 s elapsed-update timer
    /// fills the gaps the sparse per-cell ticks leave. Over a ~3 s run:
    /// (a) at least two elapsed-update ticks (both `bandwidth` and
    /// `latency_ns` `None`) land, and (b) the whole stream's
    /// `elapsed_secs` advances (non-decreasing, a small jitter allowance
    /// for the two-producer send-order race) with bounded (≤ ~1.5 s)
    /// deltas — the smooth clock the frontends render from one field.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tc_fe1_burn_in_elapsed_cadence_is_bounded() {
        let mgr = BenchJobManager::new();
        // The Memory tier is the slow target: a full pass (3 bandwidth
        // cells over ≥256 MiB + the latency chase) takes several seconds,
        // so no cell completes for >2 s — exactly the gap the FE-1
        // elapsed-update timer must fill.
        let handle = mgr
            .start_burn_in(StreamTarget::Tier(Tier::Memory), 0, 1)
            .await
            .expect("first burn-in on a fresh manager must succeed");
        let run_id = handle.run_id;

        // Run ~3 s (enough for ≥2 elapsed-update fires at the 1 s
        // cadence), then cancel.
        tokio::time::sleep(Duration::from_millis(3000)).await;
        assert!(mgr.cancel(run_id), "the active burn-in must be cancellable");

        let events = collect(handle).await;
        let ticks: Vec<&BurnInTick> = events
            .iter()
            .filter_map(|ev| match ev {
                JobEvent::BurnInTick(t) => Some(t),
                _ => None,
            })
            .collect();

        // (a) At least two elapsed-update ticks (both `None`) in the
        // ~3 s window.
        let elapsed_updates: Vec<&BurnInTick> = ticks
            .iter()
            .copied()
            .filter(|t| t.bandwidth.is_none() && t.latency_ns.is_none())
            .collect();
        assert!(
            elapsed_updates.len() >= 2,
            "a ~3 s run must emit ≥2 elapsed-update ticks (the 1 s cadence): {events:?}"
        );

        // (b) The whole stream's `elapsed_secs` advances (non-decreasing,
        // a 50 ms jitter allowance for the two-producer send-order race)
        // with bounded (≤ ~1.5 s) deltas — the smooth clock.
        for w in ticks.windows(2) {
            assert!(
                w[1].elapsed_secs + 0.05 >= w[0].elapsed_secs,
                "elapsed must not decrease (beyond 50 ms jitter): {} -> {}",
                w[0].elapsed_secs,
                w[1].elapsed_secs
            );
            let gap = w[1].elapsed_secs - w[0].elapsed_secs;
            assert!(
                gap <= 1.5,
                "consecutive tick elapsed gap must be ≤ ~1.5 s (the 1 s cadence + jitter): {gap}"
            );
        }
        // After the terminal the slot is released: cancel is false.
        assert!(
            !mgr.cancel(run_id),
            "after the terminal the slot is released: cancel must be false"
        );
    }

    /// (h) A burn-in and a normal benchmark share the one
    /// single-flight slot (D-1): each refuses the other while active
    /// with `Busy`, and each still terminates cleanly with
    /// `Cancelled` when stopped (both are long Memory-tier runs on
    /// one worker, so the immediately-issued cancel wins its race).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn burn_in_and_bench_share_the_single_flight_slot() {
        let mgr = BenchJobManager::new();

        // Phase 1: a normal bench is active → a concurrent burn-in is
        // refused; the bench still ends with the Cancelled terminal.
        let bench = mgr
            .start(StreamTarget::Tier(Tier::Memory), BenchMode::Full, 1)
            .await
            .expect("the bench start must succeed");
        let bench_id = bench.run_id;
        let refused = mgr.start_burn_in(StreamTarget::Full, 0, 1).await;
        assert!(
            matches!(refused, Err(JobError::Busy)),
            "a concurrent burn-in must be refused single-flight"
        );
        assert!(mgr.cancel(bench_id), "the active bench must be cancellable");
        let events = collect(bench).await;
        assert!(
            matches!(events.last(), Some(JobEvent::Cancelled)),
            "the bench must end with the Cancelled terminal: {events:?}"
        );

        // Phase 2: a burn-in is active → a concurrent bench is
        // refused; the (infinite) burn-in ends with Cancelled.
        let burn = mgr
            .start_burn_in(StreamTarget::Tier(Tier::Memory), 0, 1)
            .await
            .expect("the burn-in must start after the bench released the slot");
        let burn_id = burn.run_id;
        let refused = mgr
            .start(StreamTarget::Cell(Tier::L1, BenchOp::Read), BenchMode::Full, 1)
            .await;
        assert!(
            matches!(refused, Err(JobError::Busy)),
            "a concurrent bench must be refused single-flight"
        );
        assert!(mgr.cancel(burn_id), "the active burn-in must be cancellable");
        let events = collect(burn).await;
        assert!(
            matches!(events.last(), Some(JobEvent::Cancelled)),
            "the burn-in must end with the Cancelled terminal: {events:?}"
        );
    }

    /// (i) Run ids advance monotonically across run classes (D-1): a
    /// burn-in started after a normal bench's terminal gets the next
    /// id; the refused claims of (h) consumed none in between.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_ids_advance_across_run_classes() {
        let mgr = BenchJobManager::new();
        let first = mgr
            .start(StreamTarget::Cell(Tier::L1, BenchOp::Read), BenchMode::Full, 1)
            .await
            .expect("first start must succeed");
        let first_id = first.run_id;
        let events = collect(first).await;
        assert!(
            matches!(events.last(), Some(JobEvent::Result(_))),
            "the bench must complete with a Result: {events:?}"
        );

        let burn = mgr
            .start_burn_in(StreamTarget::Tier(Tier::Memory), 0, 1)
            .await
            .expect("the burn-in must start after the bench's terminal");
        assert_eq!(
            burn.run_id,
            first_id + 1,
            "run ids must advance across run classes"
        );
        // Stop the infinite run (best-effort: the slot is still held).
        let _ = mgr.cancel(burn.run_id);
        let events = collect(burn).await;
        assert!(
            matches!(
                events.last(),
                Some(JobEvent::Cancelled) | Some(JobEvent::Result(_))
            ),
            "the burn-in must reach a terminal: {events:?}"
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

    /// TC-DAEMON4: the privileged daemon re-clamps a client-supplied
    /// burn-in duration to the sanctioned `0..=1440` range (the client
    /// clamp is convenience only — the server enforces its own invariant
    /// on a wire value it acts on). `u32::MAX` is capped to `1440` (not
    /// a ~138-year deadline); `0` stays `0` (infinite / cancel-only); a
    /// normal value is unchanged.
    #[test]
    fn tc_daemon4_burn_in_duration_is_reclamped_in_the_daemon() {
        // A hostile value is capped to the sanctioned maximum.
        assert_eq!(clamp_burn_in_duration(u32::MAX), 1440);
        assert_eq!(clamp_burn_in_duration(1441), 1440);
        assert_eq!(clamp_burn_in_duration(MAX_BURN_IN_MINUTES + 1), 1440);
        // `0` is infinite (cancel-only) and passes through unchanged.
        assert_eq!(clamp_burn_in_duration(0), 0);
        // Normal values are unchanged.
        assert_eq!(clamp_burn_in_duration(1), 1);
        assert_eq!(clamp_burn_in_duration(60), 60);
        assert_eq!(clamp_burn_in_duration(1440), 1440);
    }
}
