# Browser normalized cache: IndexedDB to Turso migration plan

Status: **WP-11 packaging evidence complete; exposure remains blocked on owner budget acceptance and WP-12 Safari/navigation rollout evidence**

Scope: browser normalized GraphQL cache only. The Tauri native
`cache-sqlite` host stays unchanged. The `idb` use in collaboration storage and
per-query persistence is not part of this migration.

Primary references:

- [Notion: How we sped up Notion in the browser with WASM SQLite](https://www.notion.com/blog/how-we-sped-up-notion-in-the-browser-with-wasm-sqlite)
- [Turso Rust core](https://github.com/tursodatabase/turso/tree/main/core)
- Existing cache design: [`graphql-normalized-cache-plan.md`](./graphql-normalized-cache-plan.md)
- Gate result: [`graphql-cache-turso-g0-decision.md`](./graphql-cache-turso-g0-decision.md)

Wave 0 executed WP-01 through WP-04 and the follow-up fork verification.
Gate G0 now authorizes production work under the scope decisions recorded in
the gate document.

## 1. Goal

Replace the browser normalized cache's Rust `IdbStorage` backend with Turso's
Rust core, compiled directly into our `cache-wasm` module and persisted in
OPFS. Turso and the cache engine run together inside the elected dedicated
cache worker.

The production result must:

1. use Turso's Rust core directly; do not add or use any Turso JavaScript/npm
   package;
2. compile `cache-core`, the Turso-backed `Storage` implementation, Turso core,
   and the wasm-bindgen shell into one WASM module;
3. keep that combined module and all database work off the page's main thread;
4. allow exactly one dedicated browser worker to own the OPFS database while
   every tab can route cache requests to it;
5. fail over to a new owner without permitting concurrent database access;
6. preserve the current `CacheHost`, urql exchange semantics, hot LRU,
   normalization behavior, identity isolation, offline reads, and optimistic
   queue behavior while the local database remains healthy;
7. treat the entire browser database as disposable on cutover, incompatible
   format, corruption, uncertain failover, or storage failure.

Explicit non-goals:

- no cache/API racing or changes to request-policy timing;
- no migration of normalized records, queued mutations, or optimistic layers
  from IndexedDB;
- no requirement to retain the existing IDB backend during rollout;
- no new fallback design for unsupported browsers;
- no COOP/COEP headers, cross-origin isolation, `SharedArrayBuffer`, or WASM
  threads;
- no JavaScript SQL adapter or Rust-to-JavaScript storage bridge;
- no Turso Cloud or Turso Sync.

## 2. Design inputs

### 2.1 What to carry over from Notion

The applicable lessons from Notion are:

- OPFS is not a multi-writer coordination mechanism. Enforce one database
  owner rather than trusting concurrent connections.
- A SharedWorker can coordinate tabs while a dedicated worker owns the
  database.
- Use a long-held Web Lock to detect when a tab disappears, not only
  `pagehide` or an explicit disconnect message.
- Load and compile database WASM in a worker, outside the page's main thread.
- Corruption detection, owner failover, and navigation performance need
  production telemetry and staged rollout.

We are not copying Notion's SQLite package or VFS. We are applying its
single-owner worker topology to a custom Turso-core WASM build.

### 2.2 Turso integration approach

Use an exact reviewed Turso Rust core revision/version in Cargo. Do not consume
`@tursodatabase/database-wasm` or any other Turso npm package.

The intended Rust dependency graph is:

```text
cache-wasm
  ├── cache-core
  └── cache-turso
        ├── turso-opfs
        │     └── turso_core::{IO, File, Completion, ...}
        └── turso_core::{Database, Connection, Statement, ...}
```

Build Turso core with `default-features = false` and only the SQL/features the
cache schema actually needs. Pin the exact source revision in Cargo so upstream
changes cannot silently alter the browser database.

The combined module targets `wasm32-unknown-unknown` through the existing
wasm-pack build. It must use unshared WASM memory and must not emit atomics,
thread imports, or a nested worker.

### 2.3 Rust OPFS I/O adapter

Turso core exposes Rust `IO` and `File` traits. Add a browser-specific Rust
implementation backed directly by OPFS `FileSystemSyncAccessHandle`s.

The adapter is created in a DedicatedWorker because sync access handles are not
available in the current SharedWorker topology on Chromium. The SharedWorker
only coordinates tabs and routes messages; it never imports Turso or opens
OPFS.

Opening OPFS directory/file handles is asynchronous, while Turso's `IO::open_file`
entrypoint is synchronous. The browser adapter must therefore:

1. asynchronously open/create the database and WAL files before opening Turso;
2. create their sync access handles in the dedicated worker;
3. register those handles in a Rust-owned path/handle table;
4. let Turso's synchronous `open_file` resolve only pre-registered paths;
5. implement `pread`, `pwrite`, `sync`, `truncate`, `size`, and lock/unlock
   through those handles; synchronous `IO::remove_file` must reject deletion;
6. complete Turso I/O completions immediately and correctly; and
7. finalize statements, explicitly drive Turso `Connection::close()` to
   completion, drop every connection, database, and adapter/IO reference, then
   close every sync handle before releasing database ownership.
   Deletion/recreation is a separate asynchronous owner-session operation.

Turso's `File` and `IO` traits require `Send + Sync`, while browser JS handles
may not implement those traits. The feasibility spike must find a sound
single-threaded representation, such as storing only numeric handle IDs in the
trait objects and keeping actual JS handles in worker-local state. Do not add
unchecked `unsafe impl Send/Sync` without a documented safety argument and
review.

No database pages or BLOBs cross into JavaScript. JavaScript is limited to the
existing worker/RPC glue and the browser APIs wasm-bindgen calls.

### 2.4 The browser database is entirely disposable

The Turso OPFS database contains normalized records, queue rows, optimistic
layers, and metadata, but none of it must survive a backend cutover or an
unrecoverable database event.

Wipe the entire Turso database when:

- the storage schema or cache compatibility namespace is incompatible;
- integrity validation fails;
- an OPFS operation fails in a way that leaves database state uncertain;
- the active worker dies while a mutating cache RPC may be in flight;
- the user logs out or identity handling requests a clear;
- a test/debug reset is requested.

A wipe closes all handles, removes the main and WAL files, and opens a new
empty database. The network remains authoritative. There is no IDB-to-Turso
record or queue handoff and no cross-backend command reconciliation.

A graceful owner handoff may preserve the Turso database after the old owner
drains its request queue and closes cleanly. An abrupt/uncertain handoff starts
from an empty Turso database. In-flight cache RPCs from the failed owner are
rejected rather than replayed speculatively.

## 3. Current architecture and replacement boundary

Pre-cutover browser path (removed by WP-10):

```text
page / urql
  -> CacheHost (`host/worker-host.ts`)
  -> SharedWorker (`cache.shared-worker.ts`)
  -> CacheWorkerCore
  -> cache-wasm (`Engine<IdbStorage>`)
  -> IndexedDB (`cache-idb`)
```

Current browser path:

```text
page / urql
  -> CacheHost
  -> SharedWorker coordinator/router
  -> elected dedicated cache worker
  -> one cache-wasm module
       -> Engine<TursoStorage>
       -> TursoStorage
       -> turso_core
       -> Rust OpfsIo / OpfsFile
  -> OPFS database + WAL
```

Unchanged components:

- `cache-core` normalization, denormalization, dependency tracking, hot LRU,
  identity witness, and optimistic composition;
- page-side `CacheHost` behavior;
- urql `normalizedCacheExchange` behavior and request policies;
- GraphQL/postcard data representation;
- Tauri's native `Engine<SqliteStorage>` path.

Replacement components:

- browser storage crate;
- browser WASM shell storage type;
- browser worker topology;
- browser cache build and packaging;
- browser cache tests, telemetry, and rollout controls.

## 4. Target worker architecture

```text
┌──────────────────────────── tab A (requester) ────────────────────────────┐
│ urql -> CacheHost -> SharedWorker MessagePort                            │
│ page holds a per-tab liveness Web Lock                                   │
└────────────────────────────────┬─────────────────────────────────────────┘
                                 │ request/response and pushes
                                 ▼
┌────────────────────── SharedWorker coordinator ──────────────────────────┐
│ one named coordinator per anonymous cache scope                         │
│ - registers tabs and monitors liveness locks                             │
│ - elects exactly one active tab                                          │
│ - queues/routes RPCs and maps responses to requesters                     │
│ - tags ownership with monotonically increasing epochs                    │
│ - broadcasts cache pushes and engine-replaced notices                    │
│ - never imports cache-wasm, Turso, or OPFS handles                        │
└────────────────────────────────┬─────────────────────────────────────────┘
                                 │ transferred MessageChannel
                                 ▼
┌──────────────────── active tab's DedicatedWorker ────────────────────────┐
│ one wasm-bindgen module                                                  │
│                                                                          │
│ CacheWorkerCore -> CacheEngine -> Engine<TursoStorage>                   │
│                                  -> turso_core                           │
│                                  -> Rust OpfsIo / OpfsFile               │
│                                  -> SyncAccessHandles                    │
└────────────────────────────────┬─────────────────────────────────────────┘
                                 ▼
┌──────────────────────────────── OPFS ─────────────────────────────────────┐
│ anonymous-scope database file + WAL                                      │
└──────────────────────────────────────────────────────────────────────────┘
```

A page creates the dedicated cache worker only after the coordinator elects
it. Standby tabs do not instantiate the combined WASM module. The page
transfers opposite ends of a `MessageChannel` to the coordinator and dedicated
worker so normal routed traffic does not relay through the page main thread.

### 4.1 Coordinator state machine

The coordinator has explicit states:

- `waiting-for-tab`;
- `activating { tabId, epoch }`;
- `active { tabId, epoch, enginePort }`;
- `draining { tabId, epoch }`;
- `resetting-after-loss { previousTabId, nextEpoch }`.

Rules:

1. Only the coordinator increments the owner epoch and selects an owner.
2. The dedicated worker acquires a scope-specific exclusive Web Lock before it
   opens OPFS/Turso.
3. The coordinator routes nothing until the worker acknowledges current-epoch
   database and engine readiness.
4. Graceful deactivation stops new routing, drains queued requests, closes
   Turso and every sync handle, releases the DB-owner lock, then acknowledges.
5. Abrupt owner loss rejects all old-epoch in-flight requests. The next owner
   acquires the DB-owner lock, deletes the main and WAL files, and initializes
   a fresh database before becoming active.
6. Responses and pushes tagged with an old epoch are ignored.
7. Engine replacement loses the in-memory dependency index. Every page is told
   to reexecute locally active operations after activation.
8. At no point may two dedicated workers hold the DB-owner lock or OPFS sync
   handles for the same scope.

### 4.2 Liveness locks

Each page holds a unique lock such as
`graphql-cache-tab:<scope>:<tabId>` for its lifetime. The coordinator waits on
the same lock; acquiring it means the page released it or disappeared. Keep
explicit `dispose`/`pagehide` messages as a fast path, but correctness must not
depend on them.

The supported-browser capability contract must include SharedWorker, Web
Locks, DedicatedWorker, OPFS, and sync access handles. Behavior outside the
approved support matrix is out of scope for this migration plan.

## 5. Rust crate and storage design

### 5.1 Proposed repo layout

```text
crates/client/
  cache-core/                 # unchanged domain engine
  turso-opfs/                 # wasm-only Turso IO/File implementation
  cache-turso/                # Storage over turso_core SQL
  cache-sqlite/               # unchanged Tauri backend
  cache-wasm/                 # Engine<TursoStorage> wasm-bindgen shell

apps/web/src/lib/graphql-cache/
  host/                       # CacheHost transport
  worker/
    cache.coordinator.shared-worker.ts
    cache.engine-worker.ts
    coordinator-core.ts
    worker-core.ts
  wasm/                       # generated combined cache + Turso WASM package
```

New Rust crates must use `#![deny(missing_docs)]` and document all public
browser-safety and ownership invariants.

### 5.2 Database identity and reset

Use one anonymous OPFS filename per existing cache scope, for example:

```text
graphql-cache-<uuid>.db
```

Do not put user IDs, emails, team IDs, or other PII in the filename.

Store in `meta`:

- the anonymous scope;
- `cache_core::codec::cache_namespace(scope)`;
- a separate Turso browser-storage schema version;
- a clean-shutdown marker if the chosen recovery design needs one.

Any mismatch wipes every table/file. Unlike the native SQLite backend, the
browser Turso backend does not preserve queue rows across an incompatible
namespace.

### 5.3 Proposed SQL schema

The first implementation should mirror the native `cache-sqlite` schema and
semantics where useful, without sharing rusqlite-specific code:

```sql
CREATE TABLE meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE records (
  __typename TEXT NOT NULL,
  id TEXT NOT NULL,
  value BLOB NOT NULL,
  PRIMARY KEY (__typename, id)
);

CREATE TABLE mutation_queue (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  query TEXT NOT NULL,
  operation_name TEXT,
  variables_json TEXT NOT NULL,
  identity TEXT,
  attempt_count INTEGER NOT NULL DEFAULT 0,
  next_attempt_at_ms INTEGER,
  lease_owner TEXT,
  lease_generation INTEGER NOT NULL DEFAULT 0,
  lease_expires_at_ms INTEGER,
  last_error TEXT,
  created_at_ms INTEGER NOT NULL
);

CREATE TABLE optimistic_layers (
  mutation_id INTEGER PRIMARY KEY,
  optimistic_data_json TEXT NOT NULL,
  normalized_updates BLOB NOT NULL,
  FOREIGN KEY (mutation_id) REFERENCES mutation_queue(id) ON DELETE CASCADE
);
```

Records and normalized optimistic updates remain postcard BLOBs. Query source,
variables, and optimistic source retain their current representations so
`cache-core` behavior does not change.

`TursoStorage` maps the existing canonical `EntityKey` into the compound key:

- `Typename:id` becomes `(__typename = "Typename", id = "id")`;
- if an entity key contains more than one colon, split only at the first one so
  the complete remaining key suffix stays in `id`;
- the `ROOT_QUERY` singleton becomes
  `(__typename = "ROOT_QUERY", id = "")` and reconstructs as `ROOT_QUERY`;
- internal keys such as `__meta:identity` follow the normal first-colon rule.

Reject any other entity key that cannot be represented or round-tripped. Add
unit tests for the root singleton, internal keys, ordinary IDs, and IDs that
contain colons. The compound primary key's leftmost `__typename` column makes a
future exact-typename lookup indexable without adding that lookup API in this
migration. Existing `Storage::scan_records` ordering and exclusive-cursor
semantics must remain unchanged.

Freeze the schema only after verifying the required SQL subset against the
pinned Turso core revision. Enable and test foreign keys. Select
journal/synchronous settings from Turso core's supported behavior and browser
kill tests rather than copying rusqlite pragmas blindly.

### 5.4 `Storage` implementation

`cache-turso::TursoStorage` implements every `cache_core::store::Storage`
method directly in Rust:

- `get_batch`;
- `put_batch`;
- `delete_batch`;
- `scan_records`;
- `enqueue_mutation`;
- `load_mutation_queue`;
- `claim_next_mutation`;
- `defer_mutation`;
- `complete_mutation`;
- `discard_mutation`;
- `clear`.

Required transaction boundaries:

| Storage operation | Required atomic behavior |
|---|---|
| `put_batch`, `delete_batch` | whole input batch |
| `enqueue_mutation` | queue row + optimistic layer |
| `claim_next_mutation` | strict-head check + lease update |
| `defer_mutation` | claim validation + lease release/backoff update |
| `complete_mutation` | real record upserts + queue/layer removal |
| `discard_mutation` | claim validation + queue/layer removal |
| `clear` | records + queue + optimistic layers |

Batch operations at the `Storage` boundary; do not prepare/commit once per
record. Add small Turso-core helpers for binding values, driving statements to
completion, collecting rows, and running transactions. Keep those helpers
private to `cache-turso` unless another Rust backend demonstrates a real need.

### 5.5 Error and reset behavior

Classify errors without logging record contents, queries, variables,
identities, or BLOBs:

- SQL/schema error;
- OPFS open/read/write/flush/truncate/close error;
- lock/ownership error;
- corrupt database/integrity failure;
- corrupt postcard payload;
- quota/full storage;
- unexpected Turso I/O state-machine result.

On an error that leaves state uncertain, use the consuming close lifecycle and
request a full browser-database reset. If close/delete/recreate cleanup is
uncertain, poison the session, retain ownership until coordinator-driven worker
replacement, and reject reuse. Surface an initialization/storage error;
designing a separate backend fallback is out of scope.

Request persistent storage with `navigator.storage.persist()` on a best-effort
basis and record the result. Cache initialization must not depend on the
request being granted.

## 6. WASM build and loading

The existing `just build-cache-wasm` command remains the single build entry
point. It now compiles Turso core into `cache_wasm_bg.wasm` through normal Cargo
dependencies.

Requirements:

1. no Turso npm dependency, JS loader, or separately downloaded Turso WASM;
2. one unshared WASM memory;
3. no WASM atomics, thread sections/imports, or `SharedArrayBuffer`;
4. no COOP/COEP or cross-origin-isolation configuration;
5. no nested worker created by the cache worker;
6. the generated module is loaded only in the elected dedicated worker;
7. production emits the WASM as a lazy external asset with the correct MIME
   type and compression;
8. the page entry chunk neither imports nor preloads the cache/Turso module;
9. Tauri and iOS native paths never construct the browser cache worker.

Do not modify `normalizedCacheExchange` to race cache and network requests.
Keep existing cache read/write and request-policy ordering.

Measure compilation time, raw/compressed WASM size, instantiation time, first
DB open, warm reopen, and active-worker memory. Use those measurements to tune
Turso feature flags and release build settings before rollout.

## 7. Direct IndexedDB cutover

The first Turso-enabled version starts with a new empty OPFS database.

Cutover steps:

1. remove `cache-idb` from the browser cache build;
2. switch `cache-wasm` to `Engine<TursoStorage>`;
3. after the Turso browser host's first actual lazy start, fire-and-forget one
   raw `indexedDB.deleteDatabase("graphql-cache:<scope>")` attempt per scope and
   page session. Never enumerate or open IDB. Success, error, and blocked all
   settle safely; a blocked request may complete later after an old tab closes;
4. do not read, copy, import, reconcile, or wait for any IDB records, mutation
   queue, or optimistic layer;
5. do not run IDB and Turso browser cache hosts side by side in the new code;
6. keep unrelated collaboration and per-query IDB databases untouched;
7. roll back by disabling/reverting the Turso build, not by synchronizing local
   state between backends.

An old tab running an old application version may continue using its old IDB
cache until it reloads. No cross-version local-state handoff is required.

## 8. Work packages for multiple agents

Agents should use separate jj workspaces/revisions. The integration owner is
the only agent who edits shared Cargo manifests/lockfiles and generated
workflows. Each package ends with tests and a handoff note containing revision
ID, changed paths, commands run, and unresolved risks.

### WP-00 — integration owner and Turso revision

**Owner paths**

- this plan and an eventual ADR;
- root `Cargo.toml` and `Cargo.lock`;
- shared cache manifests;
- generated workflow integration and final conflict resolution.

**Tasks**

1. Pin the exact reviewed `turso_core` revision/version.
2. Add only Cargo dependencies; confirm no Turso npm package enters
   `package.json` or `bun.lock`.
3. Freeze the `turso-opfs` API and worker coordinator protocol after Gate G0.
4. Record browser, WASM size, memory, and performance decisions.
5. Land work packages in dependency order.

**Depends on:** none.

### WP-01 — Turso core wasm feasibility spike

**Owner paths**

- a new isolated Rust spike under `crates/client/` or
  `apps/web/spikes/graphql-cache-turso/` as approved by WP-00;
- its results document.

**Tasks**

1. Compile the pinned `turso_core` directly for `wasm32-unknown-unknown` with
   minimal features.
2. Open an in-memory DB and exercise the required schema/SQL subset.
3. Inventory transitive features and dependencies that do not compile or add
   unnecessary size.
4. Verify the emitted module uses unshared memory and no atomics/thread
   imports.
5. Measure incremental/release compile time, WASM size, instantiation time,
   and memory.
6. Document the exact Turso core API needed to open a database, prepare/bind,
   drive I/O, step rows, transact, and close.

**Deliverable:** reproducible compile/API/size report, not production storage.

**Depends on:** WP-00 pinning the Rust dependency.

### WP-02 — Rust OPFS I/O feasibility spike

**Owner paths**

- spike-only Rust OPFS files and browser harness;
- no production cache shell changes.

**Tasks**

1. Implement the smallest `turso_core::IO`/`File` adapter over sync access
   handles.
2. Resolve asynchronous pre-open versus synchronous `open_file`.
3. Prove a sound representation satisfying Turso's `Send + Sync` bounds in a
   single-threaded worker.
4. Exercise read/write/flush/truncate/size/close and database/WAL reopen.
5. Terminate the worker during writes and verify the chosen wipe/recreate
   sequence.
6. Record the executed Chromium/Firefox matrix. The product owner accepts the
   latest stable macOS Safari feature set; WP-12 records the exact Safari
   release in production E2E.
7. Verify that no nested worker, shared memory, or cross-origin isolation is
   involved.

**Deliverable:** browser capability and safety report plus proposed
`turso-opfs` API.

**Depends on:** WP-01's minimal successful build/API findings.

### WP-03 — coordinator/election topology spike

**Owner paths**

- new spike coordinator, host, and dedicated worker files only.

**Tasks**

1. Build a SharedWorker router with three tabs and one active dedicated worker.
2. Transfer a direct `MessageChannel` between coordinator and active worker.
3. Implement tab liveness locks, DB-owner lock, epochs, graceful drain, and
   abrupt loss.
4. Demonstrate that graceful handoff preserves a fake DB and abrupt loss wipes
   it before the next owner becomes active.
5. Demonstrate stale-response rejection and exactly one owner.
6. Record the executed Chromium/Firefox/coordinator-WebKit harness behavior.
   The product owner accepts latest stable macOS Safari capability; WP-12 runs
   and records the exact Safari release. WebKit WPE is not a product target.

**Deliverable:** tested topology state machine and protocol proposal.

**Depends on:** none; use a fake database and run in parallel with WP-01.

### WP-04 — storage schema and conformance design

**Owner paths**

- design/tests only; no production backend implementation.

**Tasks**

1. Map every `Storage` method to Turso SQL and transaction boundaries.
2. Specify canonical `EntityKey` conversion to and from the compound
   `(__typename, id)` record key, including `ROOT_QUERY`.
3. List the exact SQL syntax/features used by `cache-sqlite` and verify which
   should be retained.
4. Define the Turso storage conformance suite.
5. Define full-reset behavior for schema mismatch, corruption, quota errors,
   logout, and abrupt failover.

**Deliverable:** reviewed schema and conformance-test specification.

**Depends on:** none; finalize after WP-01 results.

### Gate G0 — approve or stop

WP-00 records explicit approval only if:

- pinned Turso core compiles into the existing wasm32 target;
- the resulting module is single-threaded and needs no shared memory or
  cross-origin isolation;
- a sound Rust OPFS `IO`/`File` implementation works in the exercised browser
  engines and the product owner accepts the supported feature matrix;
- main/WAL open, close, recovery, deletion, and fresh recreation work;
- required production SQL and transaction behavior is supported within its
  explicitly approved scope;
- baseline release size and memory are measured, with the combined production
  artifact retained as a WP-11 packaging/exposure gate; and
- the coordinator proves one owner and deterministic wipe after abrupt loss.

If G0 fails, stop the migration and resolve the Turso-core/OPFS design. Do not
substitute an npm package or add COOP/COEP as a workaround.

**Recorded result: GO.** WP-01 through WP-04 and the follow-up fork
verification produced reproducible evidence and a frozen storage/lifecycle
contract. Production pins the remotely available revision `be9acfe9` from
`seanaye/turso`; its source tree is identical to the verified local
`cf7de761` commit. Explicit SQL temp storage is out of scope, foreign-key
enforcement is never disabled, the latest stable macOS Safari feature set is
accepted (WP-12 records the exact tested release), and combined resource
measurements move to WP-11's packaging/exposure gate. See
[`graphql-cache-turso-g0-decision.md`](./graphql-cache-turso-g0-decision.md).
WP-05 through WP-12 are authorized in dependency order.

### WP-05 — production `turso-opfs` crate

**Owner paths**

- new `crates/client/turso-opfs/**`;
- crate-local native/unit and wasm browser tests.

**Tasks**

1. Implement documented worker-local path/handle registration.
2. Implement Turso `Clock`, `IO`, and `File` requirements.
3. Implement async pre-open and the frozen consuming close/preserve/reset
   lifecycle; never delete through synchronous `IO::remove_file`.
4. Enforce one owner and reject unregistered paths, stale sessions, and
   reentrant operations.
5. Add completion/error mapping and safe buffer lifetime handling.
6. Add tests for every file operation, WAL file, close, worker termination,
   and reset, including partial opening cleanup, uncertain-cleanup poisoning,
   invalid/consumed-token no-mutation, and stale/reentrant rejection.

**Depends on:** G0 and frozen API from WP-02.

### WP-06 — production `cache-turso` storage crate

**Owner paths**

- new `crates/client/cache-turso/**`;
- storage conformance and engine integration tests.

**Tasks**

1. Implement schema initialization and full namespace reset.
2. Implement all `Storage` methods directly over `turso_core`.
3. Implement checked conversion between `EntityKey` and the compound
   `(__typename, id)` record key.
4. Implement bindings, row conversion, transaction helpers, and statement/I/O
   driving in Rust.
5. Preserve postcard BLOB representations and checked queue ID conversion.
6. Add Turso error classification and reset-required signaling.
7. Run the shared storage contract and engine-over-Turso tests.

**Depends on:** WP-04, WP-05, and G0.

### WP-07 — cache-wasm integration

**Owner paths**

- `crates/client/cache-wasm/**`;
- wasm shell browser tests;
- generated API declarations as needed.

**Tasks**

1. Replace `Engine<IdbStorage>` with `Engine<TursoStorage>`.
2. Make `openCache` asynchronously initialize OPFS, Turso, schema, and the
   engine in the dedicated worker.
3. Implement close and full-database reset/destroy exports.
4. Keep operation interning and all existing cache APIs unchanged.
5. Remove IDB-specific close/destroy assumptions and comments.
6. Verify one generated WASM module contains both cache and Turso core.

**Depends on:** WP-06.

### WP-08 — production coordinator and dedicated engine worker

**Owner paths**

- new coordinator/topology files under
  `apps/web/src/lib/graphql-cache/worker/`;
- topology tests;
- avoid `host/worker-host.ts` until WP-09 integration.

**Tasks**

1. Implement the reviewed coordinator states and typed envelopes.
2. Add lazy elected-worker construction and direct `MessageChannel` wiring.
3. Move `CacheWorkerCore` execution into the dedicated engine worker.
4. Load the combined WASM only after election.
5. Implement liveness/owner locks, epochs, graceful drain/close, and
   abrupt-loss wipe/recreate.
6. Broadcast engine-replaced notices after failover.
7. Keep worker construction lazy on Tauri/iOS code paths per
   `apps/web/AGENTS.md`.

**Depends on:** WP-03 and WP-07.

### WP-09 — CacheHost/protocol integration

**Owner paths**

- `apps/web/src/lib/graphql-cache/host/worker-host.ts` and tests;
- `apps/web/src/lib/graphql-cache/protocol.ts` and tests;
- `worker/worker-core.ts` integration changes;
- cache lifecycle/scope comments and tests.

**Tasks**

1. Connect the existing `CacheHost` RPC surface to the coordinator.
2. Preserve current read timeouts and mutation ordering without adding
   cache/API racing.
3. Reject old-epoch in-flight requests after abrupt loss; do not replay them.
4. Track local active operation keys and reexecute them after engine
   replacement.
5. Preserve push filtering by client operation prefix.
6. Keep Tauri transport behavior unchanged.

**Depends on:** WP-08. Coordinate shared-file edits through WP-00.

### WP-10 — direct IDB removal and repository integration

**Status:** non-root cutover complete. WP-00 still owns removal of the root
workspace member/lockfile entry and regeneration of workflow outputs.

**Owner paths**

- remove `crates/client/cache-idb/**`;
- root/cache Cargo manifests through WP-00;
- `crates/client/README.md` and cache design docs;
- `tooling/xtask/.../web_artifact_paths.rs`;
- workflow generator sources and generated outputs.

**Tasks**

1. Remove `cache-idb` from `cache-wasm` and the Cargo workspace.
2. Remove the legacy IDB SharedWorker implementation after the coordinator
   path replaces it.
3. Add a best-effort old normalized-cache IDB deletion helper without adding a
   general IDB backend or npm dependency.
4. Replace workflow/deployment path filters with `cache-turso` and
   `turso-opfs`, then regenerate generated workflows using the repository
   generator.
5. Update stale IndexedDB comments throughout cache-core, scope/lifecycle,
   tests, and docs.
6. Confirm unrelated IDB users remain intact.

**Depends on:** WP-07 through WP-09.

### WP-11 — WASM packaging and performance

**Status:** implementation and candidate-gate evidence complete. All proposed
numeric gates pass in the recorded environment, but they are not approved
rollout budgets. Exposure remains blocked on product-owner acceptance and
WP-12's Safari/navigation/telemetry evidence. See
[`graphql-cache-turso-wp11-report.md`](./graphql-cache-turso-wp11-report.md).

**Owner paths**

- `apps/web/justfile` and cache WASM build tooling;
- Vite worker/asset configuration only where required;
- build smoke tests and size reports.

**Tasks**

1. Keep `just build-cache-wasm` as the one combined Rust WASM build.
2. Ensure the WASM is external, compressed, lazy, and has the correct MIME
   type.
3. Assert no Turso npm artifact, extra Turso WASM, shared memory, atomics, or
   global Worker-derived reference is emitted; content-inspect the separately
   allowlisted Loro WASM.
4. Assert the page entry does not import/preload the combined WASM.
5. Measure and report build time, raw/compressed size, instantiation, DB open,
   and memory against approved budgets.
6. Verify development and production asset URLs, decoded artifact hashes, and
   source maps.

**Depends on:** WP-07 and G0 measurements.

### WP-12 — E2E, telemetry, and rollout controls

**Owner paths**

- new Playwright multi-page tests;
- cache-specific observability modules/tests;
- backend feature flag and dashboards/alerts.

**Tasks**

1. Add the browser/failure tests in Section 10.
2. Add the metrics in Section 11 without database payload data.
3. Add a kill switch for the Turso browser cache.
4. Stage employee/dev, preview, small production cohorts, browser-specific
   cohorts, then general rollout.
5. Define automatic rollback thresholds before exposure begins.

**Depends on:** WP-08 through WP-11.

## 9. Parallel execution schedule

```text
Wave 0 (parallel): WP-01, WP-03, WP-04
                         │
                         └-> WP-02 (after WP-01 API build)
                                   │
                                   ▼
                              Gate G0 / ADR
                                   │
Wave 1:                 WP-05 -> WP-06 -> WP-07
                                   │
Wave 2:                 WP-08 -> WP-09
                                   │
Wave 3 (parallel):       WP-10, WP-11, WP-12
                                   │
Wave 4:                  staged rollout and cleanup
```

Integration rules:

- WP-00 freezes the Turso revision, OPFS API, SQL schema, and coordinator
  protocol before production work begins.
- Agents do not concurrently edit root Cargo manifests/lockfiles, generated
  workflows, `protocol.ts`, or `worker-host.ts`.
- There must be no Turso-related `package.json` or `bun.lock` changes.
- Prefer new files in owned crates/directories; send small shared-file patches
  to the integration owner.
- Every Rust backend change updates backend tests. No SQLx preparation is
  needed for Turso/rusqlite code.
- Each verified work package gets its own jj revision before handoff.

## 10. Required test matrix

### 10.1 Rust storage contract

Run the same semantic suite against `InMemoryStorage`, browser
`TursoStorage`, and native `SqliteStorage` where applicable:

- aligned batch get with misses;
- atomic compound-key upsert/delete;
- entity-key round trips for ordinary IDs, colon-containing IDs,
  `ROOT_QUERY`, and internal keys;
- deterministic typename-filtered scan and cursor pagination;
- queue enqueue/order/reopen;
- lease fencing and strict-head blocking;
- defer/commit/discard behavior;
- complete mutation atomically writes records and removes optimism;
- clean close/reopen preserves current Turso state;
- namespace mismatch wipes records and queue;
- scope mismatch and logout wipe all state;
- corrupt postcard data requests full reset rather than panicking.

### 10.2 OPFS I/O tests

- async pre-open registers database and WAL paths;
- unregistered paths are rejected;
- positional reads and writes preserve bytes and offsets;
- short reads and completion results are correct;
- flush, truncate, size, and close work;
- buffer lifetimes remain valid until completion;
- owner lock prevents a second worker from opening the same scope;
- opening failure closes every opened handle or poisons uncertain state;
- clean close drops all Turso/adapter references before releasing handles;
- preserve/reset consumes one valid close token;
- invalid, stale, or consumed tokens return an error without state mutation;
- uncertain close/reset cleanup poisons the session;
- reentrant operations and stale owner/session IDs are rejected;
- abrupt worker termination releases the Web Lock;
- next owner removes main/WAL and creates a clean DB;
- quota and deletion errors are surfaced deterministically.

### 10.3 Worker and multi-tab E2E

For Chromium, Firefox, and latest stable macOS Safari (with exact versions
recorded by WP-12):

1. Open three tabs with one scope.
2. Assert one coordinator, one active engine worker, one combined WASM
   instance, and one OPFS owner.
3. Read/write from every tab and observe affected-operation pushes.
4. Close a standby tab; owner remains stable.
5. Gracefully close the active tab; a standby takes ownership and preserves
   the Turso database.
6. Abruptly terminate the active worker/tab during a read; the next owner
   wipes/reopens and the old request rejects.
7. Abruptly terminate during every mutating storage transaction; the next
   owner starts empty and does not replay old-epoch RPCs.
8. After engine replacement, all tabs reexecute active operations and rebuild
   dependency registration.
9. Go offline, cleanly restart the owner, and read a previously cached query.
10. Change identity and confirm a complete local reset.
11. Exercise logout, incompatible namespace, corruption, quota denial, private
    mode, and storage eviction.
12. Upgrade from the IDB build and confirm Turso starts empty without opening
    or reading IDB state; only the exact former `graphql-cache:<scope>` database
    is deleted, and blocked deletion does not delay cache APIs.
13. Verify no browser cache worker/WASM is created on Tauri or iOS native paths.
14. Inspect the page and worker environment to confirm `crossOriginIsolated`
    is not required and no `SharedArrayBuffer` is created.

### 10.4 Build and regression checks

At minimum, final integration runs:

```sh
cargo test -p cache-core -p cache-sqlite -p cache-turso -p turso-opfs
cargo check --target wasm32-unknown-unknown -p cache-turso -p turso-opfs -p cache-wasm --all-targets
just build-cache-wasm
bun run test
bun run check
just build-dev
```

Add a real-browser command for OPFS, workers, and multi-tab behavior;
Vitest/jsdom alone cannot validate them. Add a WASM inspection command that
fails if the module uses shared memory, atomics, or thread-related imports.
Test each changed Rust crate before integration.

## 11. Telemetry and rollout gates

Collect by browser and app version:

- Turso/cache WASM download, compile, instantiate, schema-init, and first-ready
  times;
- raw/compressed combined WASM size;
- cache read hit/miss/error and p50/p95/p99 latency;
- initial page load and navigation timing control vs treatment;
- active-worker memory;
- OPFS quota/usage and persistence-granted status;
- storage transaction latency by operation kind;
- owner elections, graceful drains, abrupt losses, lock wait, and activation
  time;
- complete-reset count and reason category;
- integrity-check and OPFS failures;
- queue depth/oldest age during normal healthy operation;
- stale-epoch response drops;
- unexpected multiple-owner detection.

Never emit entity keys, cache scope UUIDs, user identity, GraphQL documents,
variables, record bytes, mutation payloads, or DB filenames.

Define numeric size, memory, initialization, navigation, and error budgets
after Gate G0 measurements and before production exposure. Corruption must not
escape as wrong cached data, and one scope must never have two active OPFS
owners.

## 12. Principal risks and mitigations

| Risk | Mitigation / stop condition |
|---|---|
| Turso core does not compile cleanly for `wasm32-unknown-unknown` | WP-01 hard gate; pin/patch Rust core or stop |
| Turso `IO`/`File` `Send + Sync` bounds cannot be satisfied soundly with JS handles | worker-local numeric handle table; no unaudited unsafe impl |
| OPFS sync handles differ across browsers | WP-02 browser gate and explicit supported matrix |
| Turso core adds excessive WASM size or memory | minimal features, release profiling, approved hard budgets |
| Active tab dies mid-write | reject old requests and wipe main/WAL before new owner starts |
| OPFS corruption/concurrency | exclusive owner Web Lock, one elected owner, integrity check, full reset |
| Standby tabs duplicate WASM/memory | construct/instantiate only after election |
| Engine failover loses dependency index | engine-replaced push and reexecute active operations |
| Turso upstream changes behavior | exact Cargo revision plus conformance/kill suite |
| IDB state is lost at cutover | accepted: all browser cache state is disposable |
| Abrupt reset drops queued optimism | accepted: reset the entire cache and recover from network state |
| Combined WASM blocks page main thread | load and execute only in elected dedicated worker |

## 13. Definition of done

The migration is complete only when:

- Turso core is a pinned Rust dependency compiled into the single cache WASM;
- no Turso npm package or separately built Turso WASM exists;
- the module uses unshared memory and requires no COOP/COEP,
  `SharedArrayBuffer`, threads, or nested workers;
- Rust `OpfsIo`/`OpfsFile` passes its browser and termination suite;
- `TursoStorage` passes the shared storage and engine integration suite;
- the coordinator enforces one owner and handles graceful preservation versus
  abrupt full reset correctly;
- current `CacheHost`, urql request-policy, and Tauri behavior remain intact;
- the first Turso version starts empty and performs no IDB state migration;
- legacy normalized-cache `cache-idb` code is removed without touching other
  IDB users;
- combined WASM size, initialization, memory, navigation, and error rates meet
  approved rollout budgets;
- docs, Cargo manifests, generated workflows, and operational runbooks match
  the final architecture.
