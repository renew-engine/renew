# renew-jobs

The engine's parallelism seam: a fixed-size pool of named worker threads
with a blocking, chunk-granular `parallel_for` — every parallel workload
in the engine routes through it.

- `JobPool` — a passed context object (never a global), built from
  `PoolConfig` with an explicit worker count; the pool never inspects
  CPU topology or the environment. `0` workers builds an inline pool
  (every dispatch runs on the caller — the shape minimal builds use).
- `parallel_for(range, grain, body)` — deterministic chunk boundaries,
  unspecified assignment and interleaving; runs on the workers AND the
  calling thread; borrows caller data freely (no `'static`, no `Arc`).
- `parallel_for_slice_mut(data, grain, body)` — disjoint `&mut` chunks
  of one slice, each visited exactly once, so consumers never reach for
  per-element atomics or their own `unsafe`.

## Contract

- **Not for simulation.** Chunk-to-thread assignment is deliberately
  nondeterministic, so simulation state must never be touched from a
  pool; simulation crates must not depend on this crate. Deterministic
  parallel scheduling is a future, separate design.
- **Jobs never panic.** A panicking job is a defect: a worker-side
  panic surfaces as a debug assertion in the dispatching call, a panic
  in the caller's own chunk propagates directly, and either way the
  pool is poisoned — every later dispatch (pooled or inline) fails
  loudly instead of running silently degraded. Release builds abort at
  the panic site. There is no `Result` for job panics — a contract
  violation is not a recoverable condition.
- **One dispatch at a time, at compile time.** `parallel_for` takes
  `&mut self`: nested dispatch and concurrent dispatch are borrow
  errors, not runtime hazards.
- **Steady-state dispatch allocates nothing.** `JobPool::new` is the
  pool's only allocating operation (dispatch and drop perform no heap
  allocation; configuration building has its own small strings); an
  allocation-counting integration test pins the dispatch zero, per
  platform, on every push.
- **Workers are named platform threads** (`renew-jobs-0` …), spawned
  through the platform seam only; `Drop` joins every worker
  deterministically, and construction failure joins the already-spawned
  workers before returning the error.

## Thread safety and ownership

The pool is `Send`; dispatch requires `&mut self`, so all shared-state
coordination is internal (one mutex, two condvars — every flag lives
under the single lock). Job closures are `Sync` and shared by reference
across threads for the duration of one blocking call; they never move
and are never stored. The dispatch barrier holds the calling frame
alive until no worker can touch it — on every exit path, including
unwind.

## Testing note

The threaded-system regime applies and is in place: seeded stress
campaign (short tier always on; long tier in the scheduled sanitizer
workflow), property tests for the exactly-once law, slice partitioning,
and the chunk planner over the full index domain, deterministic
panic-path tests, an allocation-counting gate, and the scheduled
workflow runs this crate's suites under Miri because it carries
`unsafe` (four language items, each with its invariant stated at the
site).

## Status

Early-stage: the surface is exactly a pool plus parallel-for; work
stealing, job handles, and dependency graphs are deliberately absent
until a consumer demands them. The `[package.metadata.renew]` table in
[Cargo.toml](Cargo.toml) is authoritative for maturity and all manifest
metadata. The crate's contract lints live in
[clippy.toml](clippy.toml): raw thread spawning, clock reads, and
filesystem access are rejected at lint time.

## Key decisions

- **Scoped dispatch over `'static` jobs.** Borrowing caller data with
  zero per-dispatch allocation is the entire value of the crate; the
  soundness bracket (an active-count registration under one mutex,
  closed by drop guards on every path) is the same shape the standard
  library's scoped threads use internally, amortized over persistent
  named workers.
- **`&mut self` dispatch.** Makes the two deadlock-shaped misuses
  unrepresentable instead of documented; loosening later is additive.
- **One mutex.** Every cross-thread data edge rides one lock's
  acquire/release; the only atomic is the per-dispatch claim cursor,
  which lives on the dispatcher's stack and cannot outlive its call.
