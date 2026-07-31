//! Thread pool and parallel-for: the engine's parallelism seam.
//!
//! # Contract
//!
//! - **All engine parallelism goes through this crate.** Workers are named
//!   threads from the platform seam; nothing else in the engine spawns
//!   threads.
//! - **Not for simulation.** Chunk-to-thread assignment and interleaving
//!   are deliberately unspecified, so simulation state must never be
//!   touched from a pool — deterministic parallel scheduling is a future,
//!   separate design. This crate's manifest says `simulation = false` and
//!   simulation crates must not depend on it.
//! - **Jobs must never panic.** A panicking job is a contract violation:
//!   a worker-side panic surfaces as a debug assertion in the dispatching
//!   call, a panic in the caller's own chunk propagates directly, and
//!   either way the pool is poisoned — every later dispatch, pooled or
//!   inline, fails loudly instead of running silently degraded. Release
//!   builds abort at the panic site.
//! - **One dispatch at a time, enforced at compile time.** `parallel_for`
//!   takes `&mut self`, so nested dispatch (a job body reaching back into
//!   the same pool) and concurrent dispatch from two threads are borrow
//!   errors, not runtime hazards.
//! - **Steady-state dispatch allocates nothing.** [`JobPool::new`] is
//!   the pool's only allocating operation (configuration building has
//!   its own small strings); a dispatch is a stack-resident batch, a
//!   borrowed closure, and two machine words through a pre-existing
//!   mutex, and drop only joins. Pinned by an allocation-counting
//!   integration test, not assumed.
//!
//! `unsafe` is confined to the scoped-dispatch seam — four language
//! items: the `Send` impl for the erased task pointer, the `Sync` impl
//! for the slice task's shared raw base, the type-erased entry point,
//! and the disjoint-chunk materialization — each carrying the invariant
//! it relies on at the site; the scheduled checks workflow runs this
//! crate under Miri.

// Diagnostics go through sinks; the standard output macros are banned in
// this crate by construction, not convention.
#![deny(clippy::print_stdout, clippy::print_stderr)]
// The scoped-dispatch seam cannot be expressed in safe Rust; the
// exception is scoped to this crate and every block carries SAFETY.
#![allow(unsafe_code)]

use core::num::NonZeroUsize;
use core::ops::Range;
use core::sync::atomic::{AtomicUsize, Ordering};
use std::fmt;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use renew_platform::thread::{self, ThreadError, ThreadHandle};

/// Configuration for [`JobPool::new`]. The caller chooses the worker
/// count — the pool never inspects CPU topology or the environment.
#[derive(Debug, Clone)]
pub struct PoolConfig {
    workers: usize,
    name_prefix: String,
}

impl PoolConfig {
    /// `workers` threads IN ADDITION to the calling thread, which always
    /// participates in its own dispatches. `0` builds an inline pool:
    /// every `parallel_for` runs entirely on the caller (no workers, no
    /// waiting — one uncontended poison check is the only lock touch).
    #[must_use]
    pub fn new(workers: usize) -> Self {
        Self {
            workers,
            name_prefix: "renew-jobs".to_string(),
        }
    }

    /// Worker thread names become `{prefix}-{index}`.
    #[must_use]
    pub fn thread_name_prefix(mut self, prefix: &str) -> Self {
        self.name_prefix = prefix.to_string();
        self
    }

    /// The configured worker count.
    #[must_use]
    pub fn worker_count(&self) -> usize {
        self.workers
    }
}

/// Why pool construction failed. Workers spawned before the failure were
/// joined before this was returned — the error path leaks no threads.
#[derive(Debug)]
#[non_exhaustive]
pub enum PoolError {
    /// Spawning worker `worker_index` failed; `source` is the platform
    /// seam's error (invalid name, or the operating system refused).
    Spawn {
        worker_index: usize,
        source: ThreadError,
    },
}

impl fmt::Display for PoolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn {
                worker_index,
                source,
            } => write!(f, "spawning worker {worker_index} failed: {source}"),
        }
    }
}

impl std::error::Error for PoolError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Spawn { source, .. } => Some(source),
        }
    }
}

/// A fixed-size pool of named worker threads. A passed context object —
/// never a global. Dropping the pool joins every worker deterministically.
pub struct JobPool {
    shared: Arc<Shared>,
    workers: Vec<ThreadHandle<()>>,
}

impl JobPool {
    /// Spawn the configured workers (`{prefix}-0` … `{prefix}-{n-1}`).
    /// The pool's only allocating operation: dispatch and drop perform
    /// no heap allocation.
    ///
    /// # Errors
    ///
    /// [`PoolError::Spawn`] when a worker cannot be spawned; workers
    /// spawned before the failure are already joined.
    pub fn new(config: &PoolConfig) -> Result<Self, PoolError> {
        Self::build(config, None)
    }

    /// The number of worker threads (excluding the calling thread).
    #[must_use]
    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }

    /// Run `body` over `range` in chunks of `grain` indices (the last
    /// chunk may be shorter), on the workers AND the calling thread,
    /// returning only when every chunk has run and no thread can still
    /// touch this call's state — on every exit path, including unwind.
    ///
    /// Chunk `k` is exactly `start + k*grain .. min(start + (k+1)*grain,
    /// end)`; boundaries are deterministic and documented, but claim
    /// order, interleaving, and thread assignment are UNSPECIFIED — the
    /// reason this crate never serves simulation. Borrows caller data
    /// freely: no `'static`, no boxing, no per-dispatch allocation.
    /// `F: Sync` because all threads share one `&F`; the closure never
    /// moves. An empty range returns immediately without locking.
    /// Single-chunk dispatches (and every dispatch on a zero-worker
    /// pool) run inline on the caller — no workers wake, but the panic
    /// contract still binds them (poison check; unwind poisons).
    pub fn parallel_for<F>(&mut self, range: Range<usize>, grain: NonZeroUsize, body: F)
    where
        F: Fn(Range<usize>) + Sync,
    {
        let plan = Plan::new(range, grain.get());
        if plan.chunks == 0 {
            return;
        }
        let task = ForChunks { body };
        if self.workers.is_empty() || plan.chunks == 1 {
            inline_dispatch(&self.shared, &plan, &task);
            return;
        }
        // SAFETY-relevant compile-time witness: the batch shared across
        // threads is Sync exactly when F is.
        assert_sync::<Batch<'_, ForChunks<F>>>();
        dispatch(&self.shared, &plan, &task);
    }

    /// Safe mutable parallelism: disjoint `&mut` chunks of one slice,
    /// each visited exactly once. `body(offset, chunk)` receives
    /// `chunk == &mut data[offset .. offset + chunk.len()]`. Exists so
    /// consumers never reach for per-element atomics or their own
    /// `unsafe`. Same execution contract as [`Self::parallel_for`].
    pub fn parallel_for_slice_mut<T, F>(&mut self, data: &mut [T], grain: NonZeroUsize, body: F)
    where
        T: Send,
        F: Fn(usize, &mut [T]) + Sync,
    {
        let plan = Plan::new(0..data.len(), grain.get());
        if plan.chunks == 0 {
            return;
        }
        let task = ForSlice {
            base: data.as_mut_ptr(),
            len: data.len(),
            body,
        };
        if self.workers.is_empty() || plan.chunks == 1 {
            // Inline: the exclusive borrow of `data` is held by this
            // frame for the whole call; chunks are disjoint by plan.
            inline_dispatch(&self.shared, &plan, &task);
            return;
        }
        assert_sync::<Batch<'_, ForSlice<T, F>>>();
        dispatch(&self.shared, &plan, &task);
    }

    /// Construction with an injectable failure point (tests only): when
    /// `fail_at == Some(k)`, worker `k`'s spawn is treated as refused so
    /// the cleanup path is exercised with `k` real workers running.
    fn build(config: &PoolConfig, fail_at: Option<usize>) -> Result<Self, PoolError> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                epoch: 0,
                task: None,
                active: 0,
                panicked: false,
                poisoned: false,
                shutdown: false,
            }),
            work_ready: Condvar::new(),
            work_done: Condvar::new(),
        });
        let mut workers = Vec::with_capacity(config.workers);
        for index in 0..config.workers {
            let spawned = if fail_at == Some(index) {
                Err(ThreadError::SpawnFailed {
                    name: format!("{}-{index}", config.name_prefix),
                    kind: renew_platform::ErrorKind::OutOfMemory,
                })
            } else {
                let worker_shared = Arc::clone(&shared);
                thread::spawn_named(&format!("{}-{index}", config.name_prefix), move || {
                    worker_main(&worker_shared);
                })
            };
            match spawned {
                Ok(handle) => workers.push(handle),
                Err(source) => {
                    shutdown_and_join(&shared, workers);
                    return Err(PoolError::Spawn {
                        worker_index: index,
                        source,
                    });
                }
            }
        }
        Ok(Self { shared, workers })
    }

    #[cfg(test)]
    fn new_failing_at(config: &PoolConfig, fail_at: usize) -> Result<Self, PoolError> {
        Self::build(config, Some(fail_at))
    }
}

impl Drop for JobPool {
    fn drop(&mut self) {
        // No dispatch can be in flight: parallel_for takes &mut self and
        // blocks until its barrier closes; Drop takes ownership.
        debug_assert!(
            self.shared.locked().task.is_none(),
            "renew-jobs: a batch was live at drop (unreachable: dispatch blocks and Drop owns)"
        );
        shutdown_and_join(&self.shared, core::mem::take(&mut self.workers));
    }
}

/// One implementation for both callers (Drop and the construction error
/// path), so they cannot drift: flag shutdown, wake everyone, join in
/// index order. Join errors are logged, never asserted — a worker panic
/// already surfaced at dispatch time, and asserting here during an
/// unwind would abort.
fn shutdown_and_join(shared: &Shared, workers: Vec<ThreadHandle<()>>) {
    {
        let mut state = shared.locked();
        state.shutdown = true;
    }
    shared.work_ready.notify_all();
    for handle in workers {
        if let Err(error) = handle.join() {
            // The error names the thread; no allocation on this path.
            renew_diag::error!(target: "renew-jobs", "worker had panicked: {error}");
        }
    }
}

// ---- shared state ------------------------------------------------------

struct Shared {
    state: Mutex<State>,
    /// Workers wait here for a new epoch or shutdown.
    work_ready: Condvar,
    /// Dispatchers wait here for `active` to return to zero.
    work_done: Condvar,
}

struct State {
    /// Bumped once per publish; a worker joins a given epoch at most once.
    epoch: u64,
    /// `Some` exactly while a batch is live.
    task: Option<TaskRef>,
    /// Workers currently inside the bracket (holding the erased pointer).
    active: usize,
    /// A worker guard observed a panicking job this dispatch.
    panicked: bool,
    /// Sticky: a job panicked at some point; later dispatches assert.
    poisoned: bool,
    shutdown: bool,
}

impl Shared {
    /// The pool's single lock discipline. Poison recovery is sound here
    /// because no user code ever runs under this mutex — every critical
    /// section is a handful of field reads and writes.
    fn locked(&self) -> MutexGuard<'_, State> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// A type-erased pointer to a [`Batch`] on the dispatcher's stack, plus
/// the monomorphized entry point that knows its real type. Both words are
/// created together at the single erasure site in [`dispatch`].
#[derive(Clone, Copy)]
struct TaskRef {
    call: unsafe fn(*const ()),
    ctx: *const (),
}

// SAFETY: `ctx` crosses to worker threads by value but is dereferenced
// only inside `run_batch`, only between the worker's `active += 1` and
// `active -= 1`, both taken under the slot mutex. The dispatch barrier
// cannot complete while `active != 0` and cannot be skipped (it is a
// drop guard, running on return and unwind alike), so the pointee — the
// dispatcher's stack frame — strictly outlives every dereference. The
// pointee's thread-safety is compile-checked at the erasure site:
// `Batch<T>` is `Sync` exactly when the task is (`assert_sync`).
// `Sync` is deliberately NOT implemented; workers copy the two words
// under the mutex and never share a `&TaskRef`.
unsafe impl Send for TaskRef {}

/// Keeps the claim cursor off the cache line holding the batch's
/// read-only fields.
#[repr(align(64))]
struct PaddedAtomicUsize(AtomicUsize);

/// Lives on the DISPATCHER'S STACK for exactly one dispatch — the claim
/// cursor structurally cannot survive the call it belongs to.
struct Batch<'a, T> {
    next: PaddedAtomicUsize,
    plan: Plan,
    task: &'a T,
}

/// Pure chunk geometry. Saturating arithmetic throughout: extreme ranges
/// near `usize::MAX` must plan, not panic (dev builds have overflow
/// checks on).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Plan {
    start: usize,
    end: usize,
    grain: usize,
    chunks: usize,
}

impl Plan {
    fn new(range: Range<usize>, grain: usize) -> Self {
        let (start, end) = if range.start >= range.end {
            (0, 0)
        } else {
            (range.start, range.end)
        };
        let len = end - start;
        let chunks = len.div_ceil(grain.max(1));
        Self {
            start,
            end,
            grain: grain.max(1),
            chunks,
        }
    }

    /// Bounds of chunk `index` (callers pass `index < self.chunks`).
    fn chunk(&self, index: usize) -> Range<usize> {
        let offset = index.saturating_mul(self.grain);
        let chunk_start = self.start.saturating_add(offset).min(self.end);
        let chunk_end = chunk_start.saturating_add(self.grain).min(self.end);
        chunk_start..chunk_end
    }
}

/// The two task shapes share one dispatch protocol.
trait Task: Sync {
    fn run_chunk(&self, chunk: Range<usize>);
}

struct ForChunks<F> {
    body: F,
}

impl<F: Fn(Range<usize>) + Sync> Task for ForChunks<F> {
    fn run_chunk(&self, chunk: Range<usize>) {
        (self.body)(chunk);
    }
}

struct ForSlice<T, F> {
    base: *mut T,
    len: usize,
    body: F,
}

// SAFETY: sharing `&ForSlice` across threads shares the raw base pointer
// and the closure. The pointer is only turned into `&mut [T]` chunks in
// `run_chunk`, whose ranges are pairwise disjoint (plan geometry) and
// each claimed at most once (cursor RMW uniqueness), while the caller's
// exclusive `&mut [T]` borrow pins the whole slice for the blocking
// call. `T: Send` bounds the cross-thread transfer of exclusive access;
// `F: Sync` bounds the shared closure.
unsafe impl<T: Send, F: Sync> Sync for ForSlice<T, F> {}

impl<T: Send, F: Fn(usize, &mut [T]) + Sync> Task for ForSlice<T, F> {
    fn run_chunk(&self, chunk: Range<usize>) {
        debug_assert!(
            chunk.start <= chunk.end && chunk.end <= self.len,
            "renew-jobs: planner produced an out-of-bounds slice chunk"
        );
        // SAFETY: `chunk` is in-bounds of the slice this task was built
        // from (plan end == len), claimed exactly once, and disjoint
        // from every other chunk; the caller's `&mut` borrow guarantees
        // no other access exists for the duration of the dispatch.
        let part = unsafe {
            core::slice::from_raw_parts_mut(self.base.add(chunk.start), chunk.end - chunk.start)
        };
        (self.body)(chunk.start, part);
    }
}

fn assert_sync<T: Sync>() {}

/// The inline paths (zero workers, or a single chunk) bypass the worker
/// protocol but NOT the panic contract: a poisoned pool refuses inline
/// dispatch too, and an inline unwind poisons the pool — no path runs
/// quietly after a defect.
fn inline_dispatch<T: Task>(shared: &Shared, plan: &Plan, task: &T) {
    let was_poisoned = shared.locked().poisoned;
    debug_assert!(
        !was_poisoned,
        "renew-jobs: dispatch on a poisoned pool — a job panicked earlier; jobs must never panic"
    );
    if was_poisoned {
        return;
    }
    let guard = InlineGuard { shared };
    for index in 0..plan.chunks {
        task.run_chunk(plan.chunk(index));
    }
    core::mem::forget(guard);
}

/// Poisons the pool when an inline chunk unwinds; forgotten on the
/// normal path.
struct InlineGuard<'a> {
    shared: &'a Shared,
}

impl Drop for InlineGuard<'_> {
    fn drop(&mut self) {
        self.shared.locked().poisoned = true;
    }
}

// ---- the dispatch protocol ----------------------------------------------

/// Publish a batch, participate, then hold the barrier until no thread
/// can still touch this frame. The soundness bracket lives here.
fn dispatch<T: Task>(shared: &Arc<Shared>, plan: &Plan, task: &T) {
    let batch = Batch {
        next: PaddedAtomicUsize(AtomicUsize::new(0)),
        plan: *plan,
        task,
    };
    let task_ref = TaskRef {
        call: run_batch::<T>,
        ctx: (&raw const batch).cast(),
    };

    // Publish under the lock; both assertions are raised OUTSIDE the
    // critical section so a firing assertion never poisons the pool
    // mutex mid-protocol.
    let (was_poisoned, was_live) = {
        let mut state = shared.locked();
        let was_live = state.task.is_some();
        let was = state.poisoned;
        if !was && !was_live {
            state.task = Some(task_ref);
            state.epoch = state.epoch.wrapping_add(1);
            state.panicked = false;
        }
        (was, was_live)
    };
    debug_assert!(
        !was_live,
        "renew-jobs: a batch is already live (unreachable: dispatch takes exclusive self)"
    );
    debug_assert!(
        !was_poisoned,
        "renew-jobs: dispatch on a poisoned pool — a job panicked earlier; jobs must never panic"
    );
    if was_poisoned || was_live {
        // Release builds (which abort at the original panic site) can
        // never get here; a dev build that somehow continues past the
        // debug assertions still must not touch the dead pool.
        return;
    }
    shared.work_ready.notify_all();

    // The barrier is a drop guard: it runs on return AND on unwind (a
    // panic in the caller's own chunk), so the batch frame can never die
    // while a worker is inside the bracket.
    let guard = DispatchGuard { shared };

    // The caller participates in its own dispatch.
    // SAFETY: same frame — `batch` is trivially live.
    unsafe { run_batch::<T>(task_ref.ctx) };

    let job_panicked = guard.finish();
    debug_assert!(
        !job_panicked,
        "renew-jobs: a job panicked inside parallel_for — jobs must never panic"
    );
}

/// The barrier. `finish` is the normal path; `Drop` covers unwinding.
struct DispatchGuard<'a> {
    shared: &'a Shared,
}

impl DispatchGuard<'_> {
    /// Wait out the bracket, retire the task, report whether any job
    /// panicked during this dispatch.
    fn finish(self) -> bool {
        let panicked = drain(self.shared, false);
        core::mem::forget(self);
        panicked
    }
}

impl Drop for DispatchGuard<'_> {
    fn drop(&mut self) {
        // Only reached while unwinding out of the caller's own chunk:
        // the workers must drain before this stack frame dies, and the
        // pool is poisoned because a dispatch died mid-flight.
        drain(self.shared, true);
    }
}

/// Wait until no worker is inside the bracket, then retire the task in
/// the same critical section — after this, no thread holds or can
/// acquire the erased pointer.
fn drain(shared: &Shared, unwinding: bool) -> bool {
    let mut state = shared.locked();
    while state.active != 0 {
        state = match shared.work_done.wait(state) {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
    }
    let panicked = state.panicked;
    state.task = None;
    if unwinding {
        state.poisoned = true;
    }
    panicked
}

/// The monomorphized entry point both the caller and the workers run.
///
/// # Safety
///
/// `ctx` must be the address of a live `Batch<T>` whose `T` matches this
/// monomorphization. Callers are exactly: the dispatcher (same stack
/// frame) and workers inside the active bracket (see [`TaskRef`]'s
/// invariant). The reference never escapes this function.
unsafe fn run_batch<T: Task>(ctx: *const ()) {
    // SAFETY: per the function contract — created at the single erasure
    // site in `dispatch` from this exact type; provenance preserved
    // (pointer-to-pointer casts only, no integer round trips).
    let batch = unsafe { &*ctx.cast::<Batch<'_, T>>() };
    loop {
        let index = batch.next.0.fetch_add(1, Ordering::Relaxed);
        if index >= batch.plan.chunks {
            return;
        }
        batch.task.run_chunk(batch.plan.chunk(index));
    }
}

/// What every worker thread runs. Waits for a fresh epoch, brackets its
/// participation with the active count, and parks again. A worker joins
/// a given epoch at most once, so a batch it already helped finish can
/// never be re-entered through a stale wakeup.
fn worker_main(shared: &Arc<Shared>) {
    let mut last_epoch = 0u64;
    loop {
        let mut state = shared.locked();
        // The wait loop is left WITH the batch in hand, so the two words
        // are copied and the bracket is opened in the SAME critical
        // section that observed the fresh task — there is no window in
        // which the batch could be retired in between, and no reachable
        // state in which a woken worker has no task to run.
        let task = loop {
            if state.shutdown {
                return;
            }
            if state.epoch != last_epoch
                && let Some(fresh) = state.task
            {
                break fresh;
            }
            state = match shared.work_ready.wait(state) {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
        };
        last_epoch = state.epoch;
        state.active += 1;
        drop(state);
        let guard = WorkerGuard { shared };
        // SAFETY: this thread holds an active-count registration taken
        // under the mutex in the same critical section that observed
        // `task.is_some()`; the dispatcher's barrier cannot retire the
        // batch until this bracket closes (WorkerGuard), so the pointee
        // is live for the whole call — including if the job unwinds.
        unsafe { (task.call)(task.ctx) };
        drop(guard);
    }
}

/// Closes the worker's bracket on every exit path. If the job panicked,
/// the flag is set so the dispatcher's own call surfaces the defect, and
/// the pool is poisoned so nothing runs silently degraded afterwards.
struct WorkerGuard<'a> {
    shared: &'a Shared,
}

impl Drop for WorkerGuard<'_> {
    fn drop(&mut self) {
        let mut state = self.shared.locked();
        if std::thread::panicking() {
            state.panicked = true;
            state.poisoned = true;
        }
        state.active -= 1;
        if state.active == 0 {
            self.shared.work_done.notify_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicBool, AtomicU8};

    /// Long enough that a worker released from a gate is still inside
    /// its chunk when the dispatcher reaches the barrier a few dozen
    /// instructions later; short under Miri, whose interpreter is slow.
    const SLOW_ITERATIONS: usize = if cfg!(miri) { 50 } else { 200_000 };

    fn grain(n: usize) -> NonZeroUsize {
        NonZeroUsize::new(n).expect("test grains are nonzero")
    }

    /// Yield-based gate: bounded, so a broken build fails instead of
    /// hanging the suite.
    fn wait_for(flag: &AtomicBool) {
        let mut yields = 0u32;
        while !flag.load(Ordering::Acquire) {
            yields += 1;
            assert!(yields < 10_000_000, "gated event never happened");
            std::thread::yield_now();
        }
    }

    /// The text of a caught panic, whichever payload shape it carries.
    fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
        payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(ToString::to_string))
            .unwrap_or_default()
    }

    /// A task whose closure is erased behind a trait object. The
    /// protocol tests that call [`dispatch`] directly all use this one
    /// shape, so they drive a SINGLE instantiation of the dispatch
    /// machinery — the successful dispatch below and the refusals that
    /// follow exercise the very same code, not sibling copies of it.
    type ErasedTask<'a> = ForChunks<&'a (dyn Fn(Range<usize>) + Sync)>;

    /// A pool's shared state, standing alone — for the protocol pieces
    /// that are exercised without workers.
    fn fresh_shared() -> Shared {
        Shared {
            state: Mutex::new(State {
                epoch: 0,
                task: None,
                active: 0,
                panicked: false,
                poisoned: false,
                shutdown: false,
            }),
            work_ready: Condvar::new(),
            work_done: Condvar::new(),
        }
    }

    #[test]
    fn zero_worker_pool_runs_everything_inline() {
        let mut pool = JobPool::new(&PoolConfig::new(0)).expect("inline pool");
        assert_eq!(pool.worker_count(), 0);
        let caller = std::thread::current().id();
        let mut seen = [false; 100];
        let cells: Vec<AtomicU8> = (0..100).map(|_| AtomicU8::new(0)).collect();
        pool.parallel_for(0..100, grain(7), |chunk| {
            assert_eq!(std::thread::current().id(), caller);
            for index in chunk {
                cells[index].fetch_add(1, Ordering::Relaxed);
            }
        });
        for (index, cell) in cells.iter().enumerate() {
            seen[index] = cell.load(Ordering::Relaxed) == 1;
        }
        assert!(seen.iter().all(|&s| s), "every index exactly once");
    }

    #[test]
    fn every_index_runs_exactly_once_with_workers() {
        let mut pool = JobPool::new(&PoolConfig::new(4)).expect("pool");
        let cells: Vec<AtomicU8> = (0..10_000).map(|_| AtomicU8::new(0)).collect();
        pool.parallel_for(0..10_000, grain(64), |chunk| {
            for index in chunk {
                cells[index].fetch_add(1, Ordering::Relaxed);
            }
        });
        assert!(
            cells.iter().all(|c| c.load(Ordering::Relaxed) == 1),
            "every index exactly once"
        );
    }

    #[test]
    fn results_match_a_serial_oracle() {
        let mut pool = JobPool::new(&PoolConfig::new(3)).expect("pool");
        let input: Vec<u64> = (0..4096).collect();
        let parallel_sums: Vec<AtomicUsize> = (0..4096).map(|_| AtomicUsize::new(0)).collect();
        pool.parallel_for(0..input.len(), grain(100), |chunk| {
            for index in chunk {
                let value = usize::try_from(input[index] * 3 + 1).expect("fits");
                parallel_sums[index].store(value, Ordering::Relaxed);
            }
        });
        for (index, cell) in parallel_sums.iter().enumerate() {
            let expected = usize::try_from(input[index] * 3 + 1).expect("fits");
            assert_eq!(cell.load(Ordering::Relaxed), expected);
        }
    }

    #[test]
    fn borrowed_stack_data_flows_in_and_results_flow_out() {
        // The API's reason to exist: plain borrows, no 'static, no Arc.
        let mut pool = JobPool::new(&PoolConfig::new(2)).expect("pool");
        let weights = [1.5f32, 2.5, 3.5];
        let mut output = vec![0.0f32; 3000];
        pool.parallel_for_slice_mut(&mut output, grain(128), |offset, chunk| {
            for (i, slot) in chunk.iter_mut().enumerate() {
                *slot = weights[(offset + i) % weights.len()];
            }
        });
        for (index, value) in output.iter().enumerate() {
            assert!((value - weights[index % 3]).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn slice_chunks_are_disjoint_and_complete() {
        let mut pool = JobPool::new(&PoolConfig::new(4)).expect("pool");
        let mut data = vec![0u32; 5000];
        pool.parallel_for_slice_mut(&mut data, grain(37), |offset, chunk| {
            for (i, slot) in chunk.iter_mut().enumerate() {
                *slot += u32::try_from(offset + i).expect("fits") + 1;
            }
        });
        for (index, value) in data.iter().enumerate() {
            assert_eq!(*value, u32::try_from(index).expect("fits") + 1);
        }
    }

    #[test]
    fn empty_ranges_and_empty_slices_are_no_ops() {
        let mut pool = JobPool::new(&PoolConfig::new(2)).expect("pool");
        let ran = AtomicUsize::new(0);
        let count = |_: Range<usize>| {
            ran.fetch_add(1, Ordering::Relaxed);
        };
        let count_slice = |_: usize, _: &mut [u8]| {
            ran.fetch_add(1, Ordering::Relaxed);
        };

        // The same probes run over real work first: a probe that cannot
        // fire would make "nothing ran" prove nothing.
        pool.parallel_for(0..1, grain(4), count);
        let mut one = [0u8; 1];
        pool.parallel_for_slice_mut(&mut one, grain(4), count_slice);
        assert_eq!(ran.load(Ordering::Relaxed), 2, "the probes do fire");

        pool.parallel_for(10..10, grain(4), count);
        let (reversed_low, reversed_high) = (10, 5);
        pool.parallel_for(reversed_low..reversed_high, grain(4), count);
        let mut nothing: Vec<u8> = Vec::new();
        pool.parallel_for_slice_mut(&mut nothing, grain(4), count_slice);
        assert_eq!(ran.load(Ordering::Relaxed), 2, "no empty shape ran a chunk");
    }

    #[test]
    fn workers_carry_their_configured_names() {
        let mut pool =
            JobPool::new(&PoolConfig::new(1).thread_name_prefix("renew-jobs-test")).expect("pool");
        // Participation is forced by the *chunk*, not by whichever thread
        // runs it: whoever claims the first chunk parks inside it until the
        // second has run, so no thread can hold both, and with two chunks
        // and two participants each runs exactly one. Threads are still
        // told apart by id, so a MISnamed worker records a false and fails
        // the assertion rather than being mistaken for the caller.
        //
        // The gate used to key on thread id — the caller parked in "its"
        // chunk — which quietly assumed the caller claimed a chunk at all.
        // Nothing guarantees that, and which thread wins is a property of
        // the machine: with the gate disabled the caller drains both chunks
        // 400 runs out of 400 here, while the host that first reported the
        // caller's branch unexecuted had the worker winning instead. A test
        // whose two threads' roles depend on the hardware is not testing
        // what it says.
        let caller = std::thread::current().id();
        let worker_named_ok = AtomicBool::new(false);
        let worker_ran = AtomicBool::new(false);
        let second_chunk_done = AtomicBool::new(false);
        pool.parallel_for(0..2, grain(1), |chunk| {
            if std::thread::current().id() != caller {
                let named_ok = std::thread::current().name() == Some("renew-jobs-test-0");
                worker_named_ok.store(named_ok, Ordering::Release);
                worker_ran.store(true, Ordering::Release);
            }
            if chunk.start == 0 {
                // Bounded, so a pool that never woke its worker fails
                // here instead of hanging the suite.
                wait_for(&second_chunk_done);
            } else {
                second_chunk_done.store(true, Ordering::Release);
            }
        });
        assert!(
            worker_ran.load(Ordering::Acquire),
            "the worker must run one of the two chunks; the caller cannot hold both"
        );
        assert!(
            worker_named_ok.load(Ordering::Acquire),
            "the worker must observe its configured name from inside a job"
        );
    }

    /// The slice entry point dispatches too, and nothing said so.
    ///
    /// `parallel_for_slice_mut` documents the same execution contract as
    /// `parallel_for`, and `parallel_for` has the test above proving work
    /// reaches a worker. The slice form had only result assertions — and
    /// running every chunk inline on the caller produces exactly the same
    /// results, so inverting its inline-versus-dispatch decision passed
    /// the whole suite.
    ///
    /// Forced the same way as its sibling: whoever claims the first chunk
    /// parks until the second has run, so a build that never dispatches
    /// fails the bounded wait instead of quietly serialising.
    #[test]
    fn the_slice_form_reaches_a_worker() {
        let mut pool = JobPool::new(&PoolConfig::new(1)).expect("pool");
        let caller = std::thread::current().id();
        let worker_ran = AtomicBool::new(false);
        let second_chunk_done = AtomicBool::new(false);
        let mut data = [0usize; 2];

        pool.parallel_for_slice_mut(&mut data, grain(1), |offset, slice| {
            if std::thread::current().id() != caller {
                worker_ran.store(true, Ordering::Release);
            }
            if offset == 0 {
                wait_for(&second_chunk_done);
            } else {
                second_chunk_done.store(true, Ordering::Release);
            }
            for cell in slice {
                *cell = offset + 1;
            }
        });

        assert_eq!(data, [1, 2], "every chunk must still run exactly once");
        assert!(
            worker_ran.load(Ordering::Acquire),
            "the slice form must dispatch: the caller cannot hold both chunks"
        );
    }

    /// Fewer chunks than workers still completes every chunk exactly once.
    ///
    /// The dispatch wakes `chunks - 1` workers, so with a pool wider than
    /// the plan most of it is never notified at all. What must hold is
    /// that no chunk is stranded by a worker that stayed asleep: the
    /// caller drains, and every awake participant re-enters the claim
    /// loop after each chunk.
    ///
    /// Asserting *which* thread ran a chunk, or how many woke, would
    /// assert something `parallel_for` explicitly refuses to promise.
    /// The claim here is the one the contract does make — each element
    /// visited once — over a spread of plans narrower than the pool.
    #[test]
    fn a_plan_narrower_than_the_pool_still_runs_every_chunk() {
        let mut pool = JobPool::new(&PoolConfig::new(7)).expect("pool");
        for chunks in 1..=6 {
            let mut data = vec![0_u32; chunks];
            pool.parallel_for_slice_mut(&mut data, grain(1), |offset, slice| {
                for (index, slot) in slice.iter_mut().enumerate() {
                    *slot = u32::try_from(offset + index).unwrap_or(u32::MAX) + 1;
                }
            });
            let expected: Vec<u32> = (1..=u32::try_from(chunks).unwrap_or(u32::MAX)).collect();
            assert_eq!(
                data, expected,
                "a {chunks}-chunk plan on a 7-worker pool left an element unvisited"
            );
        }
    }

    #[test]
    fn nul_prefix_fails_before_spawning_anything() {
        let error = JobPool::new(&PoolConfig::new(3).thread_name_prefix("bad\0prefix"))
            .err()
            .expect("a NUL prefix must fail");
        let PoolError::Spawn { worker_index, .. } = error;
        assert_eq!(worker_index, 0, "the very first spawn must be refused");
    }

    #[test]
    fn many_sequential_dispatches_reuse_the_pool() {
        let mut pool = JobPool::new(&PoolConfig::new(2)).expect("pool");
        let total = AtomicUsize::new(0);
        for _ in 0..1000 {
            pool.parallel_for(0..64, grain(8), |chunk| {
                total.fetch_add(chunk.len(), Ordering::Relaxed);
            });
        }
        assert_eq!(total.load(Ordering::Relaxed), 64 * 1000);
    }

    #[test]
    fn construction_failure_joins_the_already_spawned_workers() {
        for fail_at in [0usize, 1, 3] {
            let error = JobPool::new_failing_at(&PoolConfig::new(4), fail_at)
                .err()
                .expect("injected failure must surface");
            let PoolError::Spawn { worker_index, .. } = error;
            assert_eq!(worker_index, fail_at);
            // Reaching here without hanging IS the joined-workers proof:
            // shutdown_and_join blocks until every pre-failure worker
            // exits.
        }
    }

    #[test]
    fn pool_error_displays_index_and_source() {
        let error = JobPool::new_failing_at(&PoolConfig::new(2), 1)
            .err()
            .expect("injected failure must surface");
        let text = error.to_string();
        assert!(text.contains("worker 1"), "got: {text}");
        let source = std::error::Error::source(&error);
        assert!(source.is_some(), "the platform error is the source");
    }

    #[test]
    fn config_reports_its_worker_count() {
        assert_eq!(PoolConfig::new(7).worker_count(), 7);
        assert_eq!(PoolConfig::new(0).thread_name_prefix("x").worker_count(), 0);
    }

    #[test]
    fn drop_joins_workers_without_a_dispatch() {
        let pool = JobPool::new(&PoolConfig::new(4)).expect("pool");
        drop(pool);
        // Returning at all is the assertion: Drop joined every worker.
    }

    #[test]
    fn locking_recovers_a_poisoned_mutex_with_its_state_intact() {
        // Poison recovery is sound because no user code runs under this
        // mutex: whatever poisoned it left the fields consistent, and
        // refusing to lock would strand workers mid-protocol instead.
        let shared = fresh_shared();
        let poisoning = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut state = shared.locked();
            state.epoch = 42;
            panic!("poison the pool mutex");
        }));
        assert!(poisoning.is_err(), "the poisoning panic must unwind");
        assert!(shared.state.is_poisoned(), "the mutex is poisoned now");
        // The recovery arm hands back exactly the state the panic left.
        assert_eq!(shared.locked().epoch, 42);
    }

    #[test]
    fn a_poisoned_mutex_does_not_break_a_live_dispatch() {
        // A mutex poisoned by something outside the protocol must not
        // strand a dispatch: the barrier, the worker bracket and the
        // park loop all recover it. Constructed, not raced — the caller
        // poisons the mutex from inside its own chunk while the worker
        // is demonstrably gated inside the bracket, and the released
        // worker burns a slow loop so the dispatcher reaches its barrier
        // (and blocks there, with `active` still 1) first.
        let mut pool = JobPool::new(&PoolConfig::new(1)).expect("pool");
        let shared = Arc::clone(&pool.shared);
        let caller = std::thread::current().id();
        let worker_in = AtomicBool::new(false);
        let released = AtomicBool::new(false);
        let ran = AtomicUsize::new(0);

        pool.parallel_for(0..2, grain(1), |_| {
            if std::thread::current().id() == caller {
                wait_for(&worker_in);
                let poisoning = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let _state = shared.locked();
                    panic!("poison the pool mutex");
                }));
                assert!(poisoning.is_err(), "the poisoning panic must unwind");
                released.store(true, Ordering::Release);
            } else {
                worker_in.store(true, Ordering::Release);
                wait_for(&released);
                for _ in 0..SLOW_ITERATIONS {
                    std::hint::spin_loop();
                }
            }
            ran.fetch_add(1, Ordering::Relaxed);
        });
        assert!(pool.shared.state.is_poisoned(), "the mutex stayed poisoned");
        assert_eq!(ran.load(Ordering::Relaxed), 2, "both chunks ran");

        // A parked worker must ignore a wakeup that brings no new epoch,
        // poisoned mutex included. Notified repeatedly rather than once
        // so a wakeup lands after the worker has re-parked.
        for _ in 0..64 {
            pool.shared.work_ready.notify_all();
            std::thread::yield_now();
        }

        // None of this is a job defect: the pool is not poisoned in the
        // engine sense, and the next dispatch runs normally.
        assert!(!pool.shared.locked().poisoned, "no job panicked");
        let again = AtomicUsize::new(0);
        pool.parallel_for(0..1024, grain(16), |chunk| {
            again.fetch_add(chunk.len(), Ordering::Relaxed);
        });
        assert_eq!(again.load(Ordering::Relaxed), 1024);
    }

    /// The publish/participate/retire bracket standing alone, and the
    /// guard that keeps it exclusive.
    ///
    /// First, the clean slot: with no workers the caller runs the whole
    /// plan itself, and the barrier must hand the slot back EMPTY —
    /// a retired batch is precisely what makes the next publish legal.
    ///
    /// Then the corrupt one. Publishing over a LIVE batch would overwrite
    /// the erased pointer in-flight workers are about to dereference:
    /// the dangling read the soundness argument exists to exclude.
    /// `parallel_for`'s `&mut self` makes that unreachable from outside
    /// the crate, so the state is planted here. The refusal half proves
    /// the ASSERTION and the publish guard beside it, not the `return`
    /// beneath them — assertion and return read the same `was_live`, so
    /// with debug assertions on (every test build) the assertion is what
    /// fires. What it pins is that the planted batch is still the live
    /// one afterwards: the refusal ran no chunk and clobbered nothing.
    #[test]
    fn a_batch_is_retired_by_the_barrier_and_never_published_over() {
        let shared = Arc::new(fresh_shared());
        let counted = AtomicUsize::new(0);
        let body = |chunk: Range<usize>| {
            counted.fetch_add(chunk.len(), Ordering::Relaxed);
        };
        let task: ErasedTask<'_> = ForChunks { body: &body };

        // The probe runs over real work first: one that cannot fire would
        // make "no chunk ran" below prove nothing.
        dispatch(&shared, &Plan::new(0..64, 8), &task);
        assert_eq!(counted.load(Ordering::Relaxed), 64, "the whole plan ran");
        {
            let state = shared.locked();
            assert!(state.task.is_none(), "the barrier retires the batch");
            assert_eq!(state.epoch, 1, "exactly one publish");
            assert!(!state.panicked, "nothing panicked");
            assert!(!state.poisoned, "a clean dispatch poisons nothing");
        }

        // A batch the protocol now believes is in flight. Its `ctx` is
        // null and stays null: the refusal happens before any thread is
        // woken, so nothing ever reads it — and null is exactly what
        // tells the planted entry apart from a freshly published one.
        {
            let mut state = shared.locked();
            state.task = Some(TaskRef {
                call: run_batch::<ErasedTask<'_>>,
                ctx: core::ptr::null(),
            });
            state.epoch = 7;
        }
        let refused = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            dispatch(&shared, &Plan::new(0..64, 8), &task);
        }));

        // Which way the refusal is expressed depends on the profile,
        // and the tests run in both: with debug assertions the
        // assertion fires, without them the guard behind it returns.
        // Either way nothing runs — that is the contract being pinned,
        // and asserting only the dev half made this test fail the
        // moment it was run under the bench profile.
        // Selected with `#[cfg]`, not `cfg!`: the macro keeps both
        // paths in the binary, so the one for the build the tests are
        // not running under becomes a region no coverage run can enter
        // — a permanent gap. The attribute compiles only the arm that
        // applies, and the other leaves nothing behind to measure.
        #[cfg(debug_assertions)]
        {
            let message = panic_text(
                refused
                    .expect_err("publishing over a live batch must be refused")
                    .as_ref(),
            );
            assert!(
                message.contains("a batch is already live"),
                "unexpected payload: {message}"
            );
        }
        #[cfg(not(debug_assertions))]
        assert!(
            refused.is_ok(),
            "with assertions off the publish is refused by returning, not by panicking"
        );
        assert_eq!(
            counted.load(Ordering::Relaxed),
            64,
            "the refused dispatch ran no chunk"
        );

        let state = shared.locked();
        assert!(
            state.task.is_some_and(|live| live.ctx.is_null()),
            "the planted batch must still be the live one"
        );
        assert_eq!(state.epoch, 7, "a refused publish opens no new epoch");
        assert!(!state.panicked, "the live dispatch's flag is untouched");
    }

    mod plan {
        use super::super::Plan;
        use proptest::prelude::*;
        use proptest::test_runner::RngSeed;

        proptest! {
            // Fixed RNG seed: same inputs on every run and machine —
            // fresh exploration is a deliberate seed change.
            #![proptest_config(ProptestConfig {
                rng_seed: RngSeed::Fixed(0x0000_11A5),
                cases: 256,
                ..ProptestConfig::default()
            })]

            /// The planner over the FULL usize domain, including ranges
            /// touching usize::MAX: bounds, contiguity, grain respect,
            /// and no panic anywhere — checked arithmetically without
            /// materializing chunks (sampled indices: first, last, mid,
            /// and one adjacent pair).
            #[test]
            fn plans_any_range_without_panicking(
                start in any::<usize>(),
                len in any::<usize>(),
                grain in 1usize..=usize::MAX,
            ) {
                let end = start.saturating_add(len);
                let plan = Plan::new(start..end, grain);
                let true_len = end - start;
                prop_assert_eq!(plan.chunks, true_len.div_ceil(grain));
                if plan.chunks > 0 {
                    prop_assert_eq!(plan.chunk(0).start, start);
                    prop_assert_eq!(plan.chunk(plan.chunks - 1).end, end);
                    let mid = plan.chunks / 2;
                    for index in [0, mid, plan.chunks - 1] {
                        let chunk = plan.chunk(index);
                        prop_assert!(chunk.start <= chunk.end);
                        prop_assert!(chunk.start >= start && chunk.end <= end);
                        prop_assert!(chunk.len() <= grain);
                        prop_assert!(!chunk.is_empty());
                    }
                    if plan.chunks > 1 {
                        // Contiguity at a sampled seam: disjoint AND
                        // gap-free, which with the bounds above gives
                        // exactly-once coverage.
                        prop_assert_eq!(plan.chunk(mid.max(1)).start, plan.chunk(mid.max(1) - 1).end);
                    }
                }
            }
        }

        #[test]
        fn covers_the_range_exactly() {
            let plan = Plan::new(10..107, 25);
            assert_eq!(plan.chunks, 4);
            assert_eq!(plan.chunk(0), 10..35);
            assert_eq!(plan.chunk(1), 35..60);
            assert_eq!(plan.chunk(2), 60..85);
            assert_eq!(plan.chunk(3), 85..107);
        }

        #[test]
        fn near_max_ranges_plan_without_panicking() {
            let plan = Plan::new(usize::MAX - 10..usize::MAX, 3);
            assert_eq!(plan.chunks, 4);
            let mut covered = 0usize;
            for index in 0..plan.chunks {
                let chunk = plan.chunk(index);
                assert!(chunk.start >= usize::MAX - 10);
                covered += chunk.len();
            }
            assert_eq!(covered, 10);
        }
    }
}
