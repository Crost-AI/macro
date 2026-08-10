# WP-01: Turso core wasm feasibility spike

Measured on 2026-08-10 in the repository direnv shell.

## Outcome

**Conditional pass for WP-01's direct-core build and SQL gate.** The pinned
Turso core compiles directly to `wasm32-unknown-unknown`, runs the required
schema and SQL in WASM with custom-memory I/O, and emits one unshared memory
with no atomic instructions, thread imports, or worker imports.

**Do not approve Gate G0 from WP-01 alone.** The revision has material API,
clock, dependency, and size constraints that WP-00/WP-02 must accept or fix:

- Turso's built-in `MemoryIO` panics on `wasm32-unknown-unknown` because its
  `Clock` calls unsupported `std::time::Instant::now`.
- `BEGIN IMMEDIATE` and `BEGIN EXCLUSIVE` internally create a temp database
  backed by that same `MemoryIO`, so they panic even when the main database has
  a WASM-safe custom `IO`. This remains unresolved; deferred `BEGIN` is only
  viable under the explicit single-connection serialization invariant below.
- `File` has no close method. The OPFS adapter must close its handle registry
  out of band, after `Connection::close` and all Turso `Arc`s are dropped.
- The minimal post-wasm-bindgen release module is 8.71 MB raw; wasm-opt `-Oz`
  reduces it to 6.68 MB raw / 1.82 MB Brotli. This needs an explicit budget.

## Exact revision and features

Recommended reviewed baseline for WP-00:

```text
repository: https://github.com/tursodatabase/turso.git
package:    turso_core 0.8.0-pre.3
revision:   ed15b13f8e5f77d7ae24af321a63d7cd0fa53365
tag:        v0.8.0-pre.3 (dereferenced commit; annotated tag object is a245de9...)
commit date: 2026-08-06
```

Pin the commit, not the moving branch or annotated tag object:

```toml
turso_core = {
  git = "https://github.com/tursodatabase/turso.git",
  rev = "ed15b13f8e5f77d7ae24af321a63d7cd0fa53365",
  default-features = false,
  features = ["fs", "uuid"],
}
```

The exact source is also locked in this nested workspace's `Cargo.lock`.
The WASM minimum is `fs + uuid`:

- `fs` lets `Database::open` resolve a custom `IO::open_file` into Turso's
  `DatabaseStorage`. Without it, callers must separately implement
  `DatabaseStorage` and pass `OpenOptions::storage`.
- `uuid` is unexpectedly mandatory at this revision: core's unconditional
  `incremental/dbsp.rs` references `uuid` even though the dependency is
  optional. The preserved no-UUID probe fails with five `E0433` errors.
- Native tests in this Nix shell additionally enable `pure-rust-crypto` because
  the unconditional `aegis` C build cannot find `errno.h`. The WASM build does
  not need that feature.

Disabled Turso defaults are `time`, `json`, `series`, `encryption`, and
`percentile`; `fts` is also disabled. Despite that, the target dependency tree
still has 193 unique cargo-tree entries and compiles substantial unconditional
code/dependencies including ICU collation/locale, regex, chrono, aristo,
tracing-subscriber, bigdecimal, AES/AES-GCM/aegis, tempfile, roaring, and the
incremental engine. The final WASM also exports Turso UUID and regexp symbols,
which limits dead-code elimination.

No Turso npm package is used or added.

## Spike contents

- `harness/src/lib.rs`: custom-memory `IO`/`Clock`, proposed schema, exact SQL,
  wasm-bindgen exports, and opt-in failing runtime probes.
- `harness/src/test.rs`: native semantic test.
- `tools/inspect-wasm`: wasmparser-based exact memory/import/operator contract,
  explicit generated-web import allowlist, and negative WAT fixtures.
- `scripts/verify.sh`: targeted native, optimized Node/web WASM, generated web
  glue, allowlist, and negative-fixture verification.
- `scripts/measure.sh`: version-gated build/optimization, sizes, hashes, five raw
  optimized samples, computed medians, web execution, and tool provenance.
- `scripts/reproduce-blockers.sh`: preserved no-UUID compile failure and both
  `std::time::Instant` WASM panics.

The spike is a standalone nested Cargo workspace and does not touch the root
Cargo manifests/lockfile or web package manifests.

## Reproduction

Run from the repository root so the repository direnv shell supplies Rust,
wasm-bindgen, Node, gzip, Brotli, and sccache:

```sh
direnv exec . bash -lc '\cd apps/web/spikes/graphql-cache-turso-core && scripts/verify.sh'
direnv exec . bash -lc '\cd apps/web/spikes/graphql-cache-turso-core && scripts/measure.sh'
direnv exec . bash -lc '\cd apps/web/spikes/graphql-cache-turso-core && scripts/reproduce-blockers.sh'
```

The direct minimal WASM build is:

```sh
direnv exec . bash -lc '\cd apps/web/spikes/graphql-cache-turso-core && \
  cargo build --locked --release --target wasm32-unknown-unknown \
  -p turso-core-wasm-spike --lib \
  --no-default-features --features wasm-minimum'
```

Both build scripts require exactly wasm-bindgen 0.2.121 and wasm-opt 117.
They fail rather than silently skipping optimization or using the unrelated
wasm-opt 129 on `PATH`. The measured run used wasm-opt 117 with `-Oz`.

## Required SQL result

The same Rust function passes natively and through the generated WASM in Node.
It verifies:

- `PRAGMA journal_mode = WAL` reports `wal`;
- `PRAGMA synchronous = NORMAL` reports numeric mode `1`;
- `PRAGMA foreign_keys = ON` reports `1`;
- all four proposed tables and the compound records primary key;
- TEXT, INTEGER, NULL, and BLOB bindings;
- compound-key upsert and ordered/cursored scan, including a colon-containing
  ID and `ROOT_QUERY`'s empty ID;
- `INTEGER PRIMARY KEY AUTOINCREMENT` starts at `1`;
- mutation/optimistic-layer `INNER JOIN`, order, and limit;
- the complete strict-head row selection, runnable check, and generation-fenced
  lease update in one deferred transaction;
- a leased strict head blocking an independently runnable later mutation;
- a two-connection stale-claim probe, which deterministically returned
  `BusySnapshot` and preserved the first connection's lease;
- explicit commit and rollback;
- orphan foreign-key rejection and `ON DELETE CASCADE`;
- explicit connection close.

Representative result:

```text
ok journal_mode=wal synchronous=1 record_count=2 mutation_id=1 \
foreign_key_rejected=true cascade_deleted_layer=true \
rollback_discarded_record=true compound_key_scan_ok=true \
strict_head_claim_ok=true competing_connection_fenced=true \
competing_connection_result=busy_snapshot
```

### Required production serialization invariant

Passing this harness does **not** make deferred transactions generally safe.
The production design must preserve all of the following together:

1. `CacheWorkerCore` serializes every engine call.
2. `cache-wasm` additionally protects `Engine` with its async mutex.
3. `TursoStorage` owns exactly one Turso `Connection` for that engine.
4. No engine/storage call re-enters while a transaction is open, and no
   transaction is nested on that connection.

Under that invariant, no second local connection can read the old queue head
between selection and lease update. The SQL still repeats strict-head,
runnable, and `lease_generation` predicates as a defensive fence. The probe
shows why removing the invariant is unsafe: two deferred connections can both
read generation zero; after the first commits, the stale writer receives
`BusySnapshot` and must explicitly roll back/retry. `BEGIN IMMEDIATE` remains
an unresolved Turso blocker, not a contract silently weakened to deferred mode.

## Exact Rust API inventory

### Open and connect

```rust
let io: Arc<dyn turso_core::IO> = Arc::new(OpfsOrMemoryIo::new());
let db = turso_core::Database::open(
    io,
    path,
    turso_core::OpenOptions::new(Arc::new(turso_core::SqliteDialect)),
)?;
let connection: Arc<turso_core::Connection> = db.connect()?;
```

`Database::open` synchronously drives `IOResult::IO` completions. That fits the
planned async JS pre-open followed by synchronous registered-handle lookup and
sync access-handle operations. `Database::open_async` is available as a
re-entrant state machine but requires `OpenOptions::storage` to be set by the
caller; it does not itself perform the normal path-to-`IO::open_file` setup.

`IO` is `Clock + Send + Sync`. Relevant methods are:

```rust
fn open_file(&self, path: &str, flags: OpenFlags, direct: bool)
    -> Result<Arc<dyn File>>;
fn remove_file(&self, path: &str) -> Result<()>;
fn file_id(&self, path: &str) -> Result<FileId>;
fn step(&self) -> Result<()>; // default no-op
```

A custom OPFS backend must provide a WASM-safe `Clock` and a stable `file_id`
(for example, `FileId::from_path_hash`), in addition to the pre-registered path
lookup.

### Prepare, bind, and reuse

```rust
let mut statement = connection.prepare(sql)?;
statement.parameters_count();
statement.parameter_index(name);
statement.bind_at(NonZeroUsize, Value)?;
statement.clear_bindings();
statement.reset()?;
```

Bindings used successfully:

```rust
Value::Null
Value::from_i64(value)
Value::from_f64(value)
Value::from_text(value)
Value::from_blob(bytes)
Value::from_slice(bytes)?
```

Indexes are one-based `NonZeroUsize`s. There is no rusqlite-style `params!`
helper in core.

### Step and read rows

```rust
match statement.step()? {
    StepResult::Done => { /* finished */ }
    StepResult::Row => { let row = statement.row().unwrap(); }
    StepResult::IO | StepResult::Yield => { /* drive/yield I/O */ }
    StepResult::Busy => { /* busy */ }
    StepResult::Interrupt => { /* interrupted */ }
}
```

For a nonblocking adapter, use `Statement::take_io_completions` and resume after
completion; the spike's immediate custom-memory backend can call `IO::step`.
Rows are invalidated by the next step. `Row::get` supports `i64`, `f64`,
`String`, `&str`, and `&Value`; it does **not** support `Vec<u8>`. Read BLOBs
through `row.get_value(index).to_blob()` and copy before stepping again.

Convenience drivers exist (`run_ignore_rows`, `run_collect_rows`,
`run_with_row_callback`, and nonblocking variants), but production helpers
should explicitly classify `Busy`, `Interrupt`, `IO`, and `Yield`.

### Transactions

There is no public RAII transaction object on the core API. Prepare/step or
`Connection::execute` SQL transaction statements:

```sql
BEGIN;
COMMIT;
ROLLBACK;
```

The claim harness executes this sequence without committing between steps:

```sql
BEGIN;
SELECT id, query, operation_name, variables_json, identity, attempt_count,
       next_attempt_at_ms, lease_owner, lease_generation,
       lease_expires_at_ms, last_error, created_at_ms
FROM mutation_queue ORDER BY id ASC LIMIT 1;
-- Rust checks next_attempt_at_ms <= now and lease_expires_at_ms <= now.
UPDATE mutation_queue
SET attempt_count = ?2, next_attempt_at_ms = NULL, lease_owner = ?3,
    lease_generation = ?4, lease_expires_at_ms = ?5
WHERE id = ?1 AND lease_generation = ?6
  AND id = (SELECT id FROM mutation_queue ORDER BY id ASC LIMIT 1)
  AND (next_attempt_at_ms IS NULL OR next_attempt_at_ms <= ?7)
  AND (lease_expires_at_ms IS NULL OR lease_expires_at_ms <= ?7);
COMMIT;
```

The update must affect exactly one row. Zero means a stale/lost claim and
`Busy`/`BusySnapshot` requires rollback/retry at the serialized operation
boundary. Deferred transactions are accepted only with the production
serialization invariant above. Avoid `BEGIN IMMEDIATE`/`EXCLUSIVE` at this
revision on WASM unless Turso fixes the internal temp `MemoryIO` clock path.

### File operations and close

`File: Send + Sync` exposes:

```rust
lock_file, unlock_file, pread, pwrite, pwritev, sync, size, truncate
```

`pread`, `pwrite`, `pwritev`, `sync`, and `truncate` take/return Turso
`Completion`s. Immediate OPFS sync operations must complete those completions
with the exact byte count or error while retaining buffers through completion.

Close is:

```rust
connection.close()?; // rolls back an active transaction; checkpoints files
drop(statement);
drop(connection);
drop(database);
```

There is no `Database::close` and no `File::close`. The browser adapter therefore
needs a documented `close_all_registered_handles` operation outside the Turso
traits, invoked only after Turso objects are gone. `IO::remove_file` covers
removal but not close.

## WASM inspection

Inspected the raw cargo module plus wasm-bindgen 0.2.121 `nodejs` and `web`
outputs before and after required wasm-opt 117 `-Oz`. The inspector now rejects
zero memories, multiple memories, memory64, shared memory, atomic instructions,
and suspicious thread/worker/WASI imports. Committed negative WAT fixtures test
every memory/atomic rejection.

The web artifacts must also match the explicit import allowlist in
`tools/inspect-wasm/expected-web-imports.tsv`; the measured modules had zero
unexpected imports. The generated web ESM glue is separately checked for its
relative WASM URL and exports, and for absence of shared-memory, atomic, and
worker constructs. The final optimized web module has:

```text
memory count:           1
initial memory:         47 pages / 3,080,192 bytes
maximum:                unset
shared:                 false
memory64:               false
atomic operator count:  0
thread-related imports: 0
worker-related imports: 0
```

Generated web JS contains no `SharedArrayBuffer`, `Atomics`, `new Worker`,
`worker_threads`, or `{ shared: true }`. Its SHA-256 is committed with the
inspection result. Imports are the explicitly reviewed wasm-bindgen
Date/time/random/crypto and runtime glue. No WASI or nested-worker import
exists. The optimized web module executed successfully through `initSync` in
Node. The module does not need WASM threads, shared memory, cross-origin
isolation, or COOP/COEP.

## Measurements

Environment: Rust 1.94.0, wasm-bindgen 0.2.121, wasm-opt 117 `-Oz`, Node
24.18.0, `opt-level = "z"`, LTO, one codegen unit, panic abort, stripped
debuginfo. `measurements/generated/` commits the raw samples, computed JSON
summary, TSV sizes/hashes/source hashes, inspections, dependency tree, web run,
and exact tool versions. Large WASM and target artifacts remain ignored.

### Build time

| Build | Seconds |
|---|---:|
| Clean target directory with warm sccache | 8.864 |
| No-op incremental release build | 0.096 |

These are local wall-clock measurements, not CI benchmarks. `RUSTC_WRAPPER`
was sccache 0.16.0.

### Release size

| Artifact | Raw | gzip -9 | Brotli -11 |
|---|---:|---:|---:|
| cargo release WASM | 9,976,133 B | 2,950,478 B | 2,161,273 B |
| post-wasm-bindgen node/web WASM | 8,705,907 B | 2,798,227 B | 2,076,370 B |
| wasm-opt 117 `-Oz` node/web WASM | 6,678,531 B | 2,465,564 B | 1,823,970 B |

This is Turso core plus the small spike shell, not `cache-core`, OPFS, or the
production wasm-bindgen API. The eventual combined module will be larger.

### Runtime proxy (five fresh Node processes, median)

| Metric | Median |
|---|---:|
| synchronous optimized load + compile + instantiate | 12.157 ms |
| first open/connect/close | 7.725 ms |
| warm open/connect/close | 0.464 ms |
| first full open + schema + SQL + competing-connection probe + close | 56.749 ms |
| warm full exercise | 6.559 ms |
| WASM linear memory before work | 3,080,192 B |
| after first open/close | 3,145,728 B |
| after full SQL exercise | 16,121,856 B |
| Node process RSS delta after exercise | 102,469,632 B |

The optimized `--target web` artifact also executed successfully: 9.288 ms
`initSync`, 77.562 ms for the full SQL exercise, and 16,121,856 B linear memory
afterward in that raw sample.

These are in-memory Node proxies, not browser OPFS or active-worker memory. They
include Node/V8 JIT overhead and now include the second-connection contention
probe, explaining the memory increase from the original review. WP-02 and the
browser performance package must measure real worker/OPFS behavior.

## Exact blockers and risks

1. **`uuid` feature cannot be removed.** `scripts/reproduce-blockers.sh`
   preserves the minimal failing build. `incremental/dbsp.rs` references the
   optional crate unconditionally.
2. **Built-in `MemoryIO` is not WASM-runtime safe.** Its `Clock` calls
   `std::time::Instant::now`, which traps on `wasm32-unknown-unknown`. The spike
   fixes the main I/O path with a custom deterministic `Clock`.
3. **Immediate/exclusive transaction trap.** Turso emits a transaction for the
   temp DB, `Connection::create_temp_database` constructs built-in `MemoryIO`,
   and WAL construction calls the bad clock. The failing export and stack are
   preserved by `scripts/reproduce-blockers.sh`. This is unresolved.
4. **Deferred mode depends on serialization.** A competing connection probe
   let both transactions read generation zero, then returned `BusySnapshot` to
   the stale writer. Production must keep one connection, serialized engine
   calls, an async engine mutex, no re-entry, and the SQL generation fence.
5. **No `File::close`.** OPFS handle shutdown must be an out-of-band adapter
   operation with strict Turso object-drop ordering.
6. **Large minimum module/transitive graph.** Disabled SQL features do not
   remove many unconditional dependencies or exported UUID/regexp symbols.
7. **Native crypto build requires a workaround in this shell.** Native tests
   need `pure-rust-crypto`; WASM minimum does not.
8. **Browser results are still pending.** This spike proves core WASM and SQL,
   not `FileSystemSyncAccessHandle`, WAL reopen/recovery, kill behavior, or the
   `Send + Sync` representation. Those remain WP-02/G0 requirements.

## Recommendation

Use exact revision `ed15b13f8e5f77d7ae24af321a63d7cd0fa53365` as the
reviewed WP-00 candidate, with `default-features = false` and WASM features
`fs, uuid`. Do not move to `main` silently.

Proceed to WP-02 only with these constraints recorded:

- custom `Clock` is mandatory;
- `BEGIN IMMEDIATE` stays blocked; deferred claim transactions are allowed only
  while all four production serialization invariants and the SQL fence remain;
- OPFS adapter owns explicit handle close-all outside `File`;
- WP-00 sets a size budget against at least 6.68 MB raw / 1.82 MB Brotli before
  adding cache-core and OPFS;
- upstream pin upgrades must rerun this full spike and blocker probes.
