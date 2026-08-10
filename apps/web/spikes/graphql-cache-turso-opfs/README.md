# WP-02: direct Rust Turso OPFS adapter spike

Measured on 2026-08-10 in the repository direnv shell. Integration-review
fixes were remeasured on the same pinned toolchain.

## Outcome

**Partial technical pass; Gate G0 must not be approved.**

Actually-run browsers:

- Playwright Chromium 145.0.7632.6: operational probe passed.
- Playwright Firefox 146.0.1: operational probe passed.
- Playwright WebKit WPE 26.0: `navigator.storage.getDirectory()` is absent in
  its `DedicatedWorker`, so OPFS setup is impossible.

WebKit WPE is not Safari, WKWebView, or an iOS target. No Safari, WKWebView,
iOS simulator, or physical iOS browser was run. Their status remains explicitly
unproven.

`BEGIN IMMEDIATE` also remains blocked. The pinned core creates an internal temp
database with built-in `MemoryIO`, then traps at `std::time::Instant::now` on
`wasm32-unknown-unknown`. Both passing browsers preserve the
`ensure_temp_database`/`open_file_with_flags` and `Instant::now` causes in the
recorded stack.

## Exact pin and tools

```text
turso_core: 0.8.0-pre.3
revision:   ed15b13f8e5f77d7ae24af321a63d7cd0fa53365
features:   fs, uuid
```

`measurements/expected-toolchain.json` pins and every verification checks exact
Rust, Cargo, wasm-bindgen 0.2.121, Node 24.18.0, Playwright 1.58.2, and browser
Nix derivations/versions. The spike uses existing tooling only. It does not add
an npm package, Turso npm package, or JavaScript SQL adapter.

## Adapter and lifecycle design

`src/lib.rs` implements `turso_core::IO` and `turso_core::File` directly over
`FileSystemSyncAccessHandle`.

JavaScript handles never inhabit a `Send + Sync` value:

1. One `thread_local! RefCell<HandleRegistry>` owns every JS handle.
2. A claimed worker owner receives a registry-issued `OwnerId`.
3. Each pre-open receives a registry-issued `SessionId`; each file receives a
   monotonically allocated `HandleId`.
4. `OpfsIo`/`OpfsFile` contain only numeric owner/session/handle tokens plus the
   numeric clock state. Rust auto traits satisfy Turso's bounds without unsafe
   code.
5. `#![forbid(unsafe_code)]` and compile-time tests reject an unsafe Send/Sync
   escape hatch.
6. Every file lookup validates owner, active session, allowed path, and live ID.
   Tokens and Rust objects never cross dedicated workers.

The registry enforces this state machine:

```text
Unowned -> Idle(owner) -> Opening(session) -> Active(session)
Active -> Closing -> Closed(close token) -> Idle       (release)
Closed(close token) -> Resetting -> Idle               (delete/recreate)
Opening/Closing/Resetting failure -> Poisoned           (no reopen/delete)
```

A successful close consumes the active session and produces the only token that
can release or reset it. Failed handles are retained in poisoned state. A close
failure therefore cannot be mistaken for success, deletion is rejected, and a
new session cannot start. Browser probes also force real `removeEntry()` and
real recreation failures; each reset poisons deterministically, rejects a second
reset with the consumed token, and rejects reopen.

Known path policy is fixed by session kind:

- database: `graphql-cache.db` (`direct = true`) and
  `graphql-cache.db-wal` (`direct = false`);
- direct operation probe: `direct-file.bin` (`direct = true`);
- failing immediate probe: `begin-immediate.db` and its WAL.

Unknown paths, direct-mode mismatches, read-only opens, and `NoLock` outside the
WAL are rejected. `file_id` is restricted to the same allowlist. `IO::remove_file`
returns `Unsupported`; async deletion is available only after consuming a
successful close token.

The dedicated worker serializes every command through one promise queue. Rust
also rejects a second top-level operation while one is active. Two concurrent
page RPCs pass only because the worker serializes their complete
open/SQL/close/release lifecycles.

## Async pre-open and sync Turso open

OPFS lookup and `createSyncAccessHandle()` are asynchronous, while
`IO::open_file` is synchronous. A session therefore:

1. requires the worker's exclusive origin Web Lock owner token;
2. transitions the Rust registry to `Opening`;
3. asynchronously creates every allowed sync handle in Rust;
4. transitions to `Active` only after all paths are registered;
5. lets synchronous `Database::open` do validated numeric-ID lookups only.

An opening failure closes already-created handles. Cleanup failure poisons the
registry.

## File-operation correctness

`pwrite` and `pwritev` retry positive partial writes with advanced slices and
offsets until every byte is consumed. Both aggregate `usize` counts pass through
checked `i32::try_from`; an aggregate above `i32::MAX` is
`CompletionError::ShortWrite`. An empty aggregate is a successful no-op reported
as `0` without calling the write backend or validating its otherwise-unused
offset. Zero or oversized progress on a
non-empty write is `ShortWrite`. Specific `CompletionError`s are not collapsed
into a generic I/O error.

Native and browser tests cover:

- a 2-byte injected write limit requiring three writes for six bytes;
- empty and `i32`-out-of-range aggregate semantics in native tests;
- browser empty-write success with exactly one `Ok(0)` callback;
- exactly one Turso callback after each aggregate succeeds;
- zero-progress `ShortWrite` with exactly one callback;
- injected ordinary write failure preserved exactly;
- injected `ErrorKind::StorageFull` quota failure preserved exactly;
- full read, EOF (`0` bytes), and short read (`2` bytes), each with one callback;
- a read callback detecting and preserving `CompletionError::ShortRead`;
- flush and truncate callback counts;
- close failure, poison retention, delete rejection, and reopen rejection;
- a non-empty directory causing the real `removeEntry()` call to fail;
- an injected post-delete directory conflict causing the real recreation
  `getFileHandle()` call to fail.

All OPFS operations are synchronous before their Turso completion fires, so no
Rust buffer crosses an async boundary. Offsets/sizes above JavaScript's maximum
safe integer are rejected.

## Real SQL, persistence, and abrupt recovery

Chromium and Firefox both pass:

- real Turso WAL schema, deferred transaction, upsert, and query;
- same-worker and cross-worker persistence of `persisted-v1`;
- exactly one Web Lock owner and a denied contender;
- deterministic close/delete/recreate of both main and WAL;
- zero-byte recreated files;
- a fresh SQL database with old row count zero and new `recovered-fresh` row.

### Active worker-kill proof

The kill RPC no longer reports “started” and resolves. It remains pending while
Rust runs a 10,000-commit SQL/BLOB loop. Rust posts `kill-first-commit` only
after the first `COMMIT` succeeds and includes the registered main/WAL sizes.
The page waits for that event, terminates the worker immediately, and asserts
that the still-pending RPC rejects because of termination. Before wiping,
recovery queries `kill_probe` and requires `1 <= committed_rows < 10,000`.
Consequently rejection cannot be explained by a completed finite loop whose
response merely remained unprocessed.

Observed first-commit data in both passing browsers:

```text
commit_count: 1
pre:  main=8192 B, WAL=0 B
post: main=8192 B, WAL=49472 B
```

Recovery observes persisted files and committed rows before deleting them.
The committed raw matrix records exact timing, attempts, row count, and pre-open
sizes for each run.

The 30-second monotonic deadline covers each candidate's Web Lock attempt,
main/WAL pre-open, SQL count, Turso close, both `removeEntry()` calls, and both
recreations—not only lock acquisition. Each RPC receives the remaining
duration and has a page-side timeout for that duration. Every unsuccessful
candidate is terminated before retrying expected sync-handle contention; the
successful candidate must finish the full scope inside the deadline. The metric
records this exact scope, start/completion timestamps, elapsed time, attempts,
and candidate termination count.

## No threads, isolation, or nested worker

The module runs with `crossOriginIsolated === false`; responses have no COOP or
COEP headers. There is no shared memory, thread, or nested worker.

Evidence is layered:

- browser runtime confirms zero workers before first use;
- a worker-local constructor monitor records zero nested constructions;
- source inspection permits exactly one `new Worker` site, in the page's lazy
  `ProbeWorker` constructor, and none in the dedicated worker;
- generated-glue inspection rejects worker constructors, SharedWorker,
  `importScripts`, SharedArrayBuffer, Atomics, worker_threads, and shared-memory
  options;
- structural WASM inspection rejects zero/multiple memories, shared memory,
  memory64, every atomic operator, and suspicious thread/worker/WASI imports.

## Structural WASM inspection

WP-02 now uses WP-01's `VisitOperator` implementation rather than stringifying
operators. Negative WAT fixtures prove rejection of:

- zero memories;
- multiple memories;
- shared memory;
- memory64;
- atomic instructions;
- imports outside an explicit allowlist.

`tools/inspect-wasm/expected-opfs-web-imports.tsv` pins all 75 generated OPFS
web imports by module, name, and kind.

Current post-wasm-bindgen release result:

```text
bytes:                   8,802,344
sha256:                  cca4b1dcf5bb5be79d65acba9b17559f02fd76a80eb463e15bcf6270a7a9051e
memory count:            1
initial memory:          47 pages / 3,080,192 bytes
maximum:                 unset
shared:                  false
memory64:                false
atomic operator count:   0
unexpected imports:      0
```

This does not replace WP-01's wasm-opt size budget decision.

## Proposed production API

The spike validates a consuming, owner/session-scoped shape:

```rust
pub struct OwnerLease { /* issued only inside the DB Web Lock callback */ }
pub struct OpeningSession { /* fixed allowed paths */ }
pub struct RegisteredOpfs { /* owner + session + TLS handles */ }
pub struct ClosedOpfs { /* unforgeable consuming close token */ }
pub struct PoisonedOpfs { /* retained uncertain handles/reason */ }

impl OpeningSession {
    pub async fn preopen(owner: &OwnerLease, paths: OpfsPaths)
        -> Result<RegisteredOpfs, OpfsError>;
}

impl RegisteredOpfs {
    pub fn io(&self) -> Arc<dyn turso_core::IO>;
    pub fn close_after_turso_drop(self)
        -> Result<ClosedOpfs, PoisonedOpfs>;
}

impl ClosedOpfs {
    pub fn release(self) -> OwnerLease;
    pub async fn delete_and_recreate(self)
        -> Result<OwnerLease, PoisonedOpfs>;
}
```

Production requirements frozen by the spike:

- one Web Lock owner, one serialized worker operation, and one Turso connection;
- private thread-local JS handles and numeric Turso-facing tokens;
- fixed path/OpenFlags/direct policy;
- no deletion without a consumed successful close;
- retained/poisoned uncertain close state;
- bounded time-based post-kill recovery and telemetry;
- explicit async reset rather than synchronous `IO::remove_file`;
- no shared memory, threads, nested worker, npm Turso, or JS SQL adapter.

## Preserved blockers

The minimal exports remain:

```rust
run_builtin_memory_io_probe();
run_begin_immediate_probe(owner, session);
```

Both trap through `std::time::Instant::now`. The immediate probe additionally
records Turso's temp-database path. Deferred `BEGIN` remains acceptable only
under all WP-01 serialization invariants; it is not a general replacement for
`BEGIN IMMEDIATE`.

Other blockers:

1. Safari/WKWebView remains unrun and unproven; WPE cannot substitute for it.
2. WebKit WPE 26.0 lacks worker OPFS.
3. Turso has no `File::close`; production needs consuming lifecycle types.
4. OPFS deletion is async while `IO::remove_file` is synchronous.
5. Chromium showed about two seconds of post-kill lock/handle release latency.
6. G0 size/memory budgets and the separate coordinator gate remain unresolved.

## Measurements and verification

Regenerate all committed toolchain, hash, inspection, deterministic summary,
and raw browser results:

```sh
direnv exec . bash -lc '\cd apps/web/spikes/graphql-cache-turso-opfs && scripts/update-measurements.sh'
```

`verify-measurements.sh` regenerates deterministic artifacts and compares them
byte-for-byte with the committed versions. Dynamic timestamps, exact recovery
latency, and pre/post sizes remain in committed `browser-matrix.json`; the
stable invariant summary is `summary.json`.

Full verification:

```sh
direnv exec . bash -lc '\cd apps/web/spikes/graphql-cache-turso-opfs && scripts/verify.sh'
apps/web/spikes/graphql-cache-turso-opfs/scripts/source-boundary.sh
```

The standalone nested workspace does not modify shared/root manifests, shared
package locks, the migration plan, prior spikes, or production files.

## Gate G0 recommendation

**STOP / NO-GO. Do not approve G0.**

The reviewed adapter is viable in the actually-run Chromium and Firefox
versions. G0 still requires the approved Safari/WKWebView target and acceptable
required transaction behavior. Neither condition is met.
