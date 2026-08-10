# Browser normalized cache: IndexedDB to Turso migration plan

Status: **proposed; feasibility gates must pass before production implementation**

Scope: browser normalized GraphQL cache only. The Tauri native
`cache-sqlite` host stays unchanged. The `idb` use in collaboration storage and
per-query persistence is not part of this migration.

Primary references:

- [Notion: How we sped up Notion in the browser with WASM SQLite](https://www.notion.com/blog/how-we-sped-up-notion-in-the-browser-with-wasm-sqlite)
- [Turso in the Browser](https://turso.tech/blog/introducing-turso-in-the-browser)
- [`@tursodatabase/database-wasm`](https://www.npmjs.com/package/@tursodatabase/database-wasm)
- Existing cache design: [`graphql-normalized-cache-plan.md`](./graphql-normalized-cache-plan.md)

## 1. Goal

Replace the browser normalized cache's Rust `IdbStorage` backend with a local
Turso database persisted in OPFS, while preserving the existing `CacheHost`,
Rust cache engine, urql semantics, durable optimistic mutation queue, bounded
hot tier, multi-tab behavior, identity isolation, and offline reads.

The production result must:

1. keep Turso and both WASM modules off the page's main thread;
2. allow exactly one browser worker to open the OPFS database at a time;
3. let every tab use that one active database owner;
4. fail over when the owner tab closes or crashes without corrupting the DB or
   duplicating queued mutations;
5. avoid delaying initial page load on the Turso download/compile/open path;
6. race slow cache reads against the API where the request policy permits it;
7. preserve queued user intent during rollout from IndexedDB;
8. fall back to the existing IDB host during rollout and to the no-op/network
   path on unsupported browsers after IDB retirement.

Turso Cloud and Turso Sync are explicitly out of scope. This is an embedded,
local OPFS database.

## 2. Important findings and constraints

These are design inputs, not incidental implementation details.

### 2.1 What to carry over from Notion

Notion's useful lessons are:

- OPFS is not a multi-writer coordination mechanism. Enforce one database
  owner rather than trusting concurrent connections.
- A SharedWorker can coordinate tabs while a dedicated worker performs the
  database work.
- Use a long-held Web Lock to detect when a tab disappears, not only
  `pagehide` or an explicit disconnect message.
- Load the database WASM asynchronously. A faster cache must not make initial
  application boot slower.
- Disk is not always faster than the API, especially on slow devices. Race
  eligible disk and network reads and measure p95, not only the median.
- Corruption detection and single-writer failover need production telemetry
  and staged rollout.

### 2.2 Turso is not the exact SQLite configuration Notion shipped

Notion ultimately used SQLite's OPFS SyncAccessHandle Pool VFS specifically to
avoid cross-origin isolation. The current Turso browser package has different
requirements.

Research was performed against exact version
`@tursodatabase/database-wasm@0.7.2`; pin that exact version during the spike
and rollout because the package is pre-1.0.

At that version Turso:

- requires `SharedArrayBuffer`, and therefore COOP/COEP headers and a
  `crossOriginIsolated` document;
- executes database compute in the context that imports the package and
  creates an internal dedicated worker for synchronous OPFS I/O;
- creates that internal worker during module initialization;
- uses a shared WASM memory with an initial 4,000 pages (250 MiB of address
  space); committed/resident memory must be measured rather than inferred;
- ships an approximately 10.6 MiB raw WASM binary (about 2.4 MiB with Brotli
  in the inspected package), in addition to the existing cache WASM;
- is tested by its package in Chromium and Firefox, but not WebKit;
- is pre-1.0 and recommends backups. Most normalized records are disposable,
  but this cache also stores queued user mutations, which are not disposable.

### 2.3 Turso cannot be dropped into the current SharedWorker

The current browser topology runs `cache-wasm` directly in
`cache.shared-worker.ts`. Turso then tries to call `new Worker(...)` for its
OPFS worker. The existing probe recorded nested workers as available in a
DedicatedWorker but unavailable in a SharedWorker on Chromium.

Therefore the current topology must change. The SharedWorker becomes a
coordinator/router. The elected tab owns a dedicated cache engine worker; that
worker imports Turso and Turso creates its nested OPFS worker.

This assumption is a mandatory browser spike. Do not begin the production
port until the exact bundled arrangement works in the browser matrix.

### 2.4 Cross-origin isolation is a product-wide change

`Cross-Origin-Opener-Policy: same-origin` and
`Cross-Origin-Embedder-Policy: require-corp` can affect much more than the
cache:

- GA, GTM, and Facebook scripts are dynamically loaded from third-party
  origins;
- external avatars, email/media resources, S3 assets, PDF resources, and
  third-party iframes may be blocked without CORS/CORP;
- COOP changes opener relationships for OAuth and other popup flows;
- production routing is split between the web-app asset stack, a route Lambda,
  preview CloudFront, and a separate website-infra stack.

The migration cannot ship by changing only Vite development headers. The
cross-origin audit and production header ownership are Phase 0 gates.

### 2.5 The current cache has non-disposable user intent

`mutation_queue` and `optimistic_layers` durably own optimistic mutations and
network retries. A backend cutover must not silently abandon or duplicate
those rows. Copying normalized records is optional; safely handing off queued
mutations is mandatory.

The new active worker is also tied to a tab. If it dies while a mutating RPC is
in flight, the coordinator may not know whether the operation committed. In
particular, blindly retrying `enqueue_mutation` can duplicate user intent.
Request idempotency and indeterminate-outcome behavior must be designed and
tested before failover is enabled.

## 3. Current architecture and replacement boundary

Current browser path:

```text
page / urql
  -> CacheHost (`host/worker-host.ts`)
  -> SharedWorker (`cache.shared-worker.ts`)
  -> CacheWorkerCore
  -> cache-wasm (`Engine<IdbStorage>`)
  -> IndexedDB (`cache-idb`)
```

Unchanged components:

- `cache-core` normalization, denormalization, dependencies, hot LRU, identity
  witness, and optimistic queue semantics;
- public page-side `CacheHost` behavior;
- urql normalized cache exchange, except for readiness/racing behavior;
- Tauri's native `Engine<SqliteStorage>` path;
- the GraphQL wire data and postcard record representation.

Replacement components:

- browser storage backend;
- browser worker topology;
- browser initialization/read scheduling;
- browser deployment headers and compatibility checks;
- browser backend migration and rollout controls.

## 4. Target browser architecture

```text
┌──────────────────────────── tab A (requester) ────────────────────────────┐
│ urql -> CacheHost -> SharedWorker MessagePort                            │
│ page also holds a per-tab liveness Web Lock                             │
└────────────────────────────────┬─────────────────────────────────────────┘
                                 │ request/response and pushes
                                 ▼
┌────────────────────── SharedWorker coordinator ──────────────────────────┐
│ one named coordinator per cache scope                                   │
│ - registers tabs and monitors their liveness locks                       │
│ - elects exactly one active tab                                          │
│ - queues/routes RPCs and maps responses back to requesters               │
│ - rejects stale responses by active-owner epoch                          │
│ - broadcasts cache pushes and owner-change/reset notices                 │
│ - never imports cache-wasm or Turso                                      │
└────────────────────────────────┬─────────────────────────────────────────┘
                                 │ transferred MessageChannel
                                 ▼
┌──────────────────── active tab's DedicatedWorker ────────────────────────┐
│ CacheWorkerCore + cache-wasm                                             │
│   -> Engine<JsStorageBridge>                                             │
│   -> same-worker TypeScript TursoStorage adapter                         │
│   -> @tursodatabase/database-wasm/vite                                   │
│        (database compute in this dedicated worker)                       │
└────────────────────────────────┬─────────────────────────────────────────┘
                                 │ Turso-created nested worker + shared mem
                                 ▼
┌──────────────────────── Turso OPFS worker ────────────────────────────────┐
│ owns SyncAccessHandles for `<anonymous-scope>.db` and its WAL            │
└──────────────────────────────────────────────────────────────────────────┘
```

A page creates a dedicated engine worker only after the coordinator elects it.
Standby tabs do not download/instantiate Turso. The page transfers opposite
ends of a `MessageChannel` to the coordinator and the engine worker so routed
traffic does not continue through the main thread.

### 4.1 Coordinator state machine

The coordinator must have explicit states:

- `waiting-for-tab`;
- `activating { tabId, epoch }`;
- `active { tabId, epoch, enginePort }`;
- `failing-over { previousTabId, nextEpoch }`.

Rules:

1. Only the coordinator increments the owner epoch and selects an owner.
2. The active engine acquires a scope-specific exclusive Web Lock before
   opening Turso. This is a second single-writer guard in addition to the OPFS
   SyncAccessHandle.
3. The coordinator routes nothing until the active engine acknowledges DB and
   cache-engine readiness for the current epoch.
4. A graceful owner drains work, closes the Rust engine/Turso connection and
   OPFS handles, releases the DB lock, then acknowledges deactivation.
5. An abrupt owner loss starts bounded reconnect/backoff; the next owner must
   acquire the DB lock before opening the file.
6. Responses and pushes tagged with an old epoch are ignored.
7. Read-only requests may be rerouted after failover. Mutating requests follow
   the idempotency/indeterminate-outcome contract in Section 7.
8. Engine replacement loses in-memory dependencies. Every page is told to
   re-register/re-execute all active operations after failover.

### 4.2 Liveness locks

Each page holds a unique lock such as
`graphql-cache-tab:<scope>:<tabId>` for its lifetime. The coordinator waits on
the same lock; acquiring it means the page released it or disappeared. Keep
explicit `dispose`/`pagehide` messages as a fast path, but correctness must not
depend on them.

If SharedWorker, Web Locks, nested dedicated workers, OPFS sync handles,
`SharedArrayBuffer`, or `crossOriginIsolated` are unavailable, do not attempt a
partially coordinated Turso connection. Use the rollout fallback (IDB while it
exists, then the no-op/network host).

## 5. Turso storage design

### 5.1 Database identity

Use one anonymous OPFS file per existing cache scope, for example:

```text
graphql-cache-<uuid>.db
```

Do not put user IDs, emails, team IDs, or other PII in the filename. Preserve
the existing anonymous scope and identity-witness behavior.

The physical filename remains stable across cache format/schema epochs so
queued mutations can be found. Store these metadata values in `meta`:

- `scope`;
- `namespace` from `cache_core::codec::cache_namespace(scope)`;
- a separate Turso storage schema version;
- migration/cutover state where required.

On namespace mismatch, clear disposable normalized records but retain queued
mutations exactly as the existing backends do. On scope mismatch, clear both
records and queued user intent.

### 5.2 Proposed SQL schema

Freeze the exact schema only after the Turso compatibility spike. The baseline
is intentionally schema-neutral and mirrors the IDB encoding:

```sql
CREATE TABLE meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE records (
  key TEXT PRIMARY KEY,
  value BLOB NOT NULL
);

CREATE TABLE mutation_queue (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  enqueue_key TEXT NOT NULL UNIQUE,
  value BLOB NOT NULL
);

CREATE TABLE optimistic_layers (
  mutation_id INTEGER PRIMARY KEY,
  value BLOB NOT NULL,
  FOREIGN KEY (mutation_id) REFERENCES mutation_queue(id) ON DELETE CASCADE
);
```

The idempotency design may add claim/settlement metadata or a bounded command
ledger. That decision belongs to the failover design review; do not improvise
it independently in the storage adapter.

Store `Record`, `StoredMutation`, and `PersistedOptimisticLayer` as the existing
postcard bytes. Queue IDs cross JavaScript as decimal strings, never unchecked
JS numbers. Verify Turso's BigInt/safe-integer behavior in the spike.

Enable and verify foreign keys. Select journal/synchronous pragmas based on
Turso's actual compatibility table and kill tests; do not copy the native
rusqlite pragmas without validation.

### 5.3 Rust-to-TypeScript storage contract

Add a WASM-only `Storage` implementation that wraps a JavaScript backend
object. Keep SQL and Turso package use in TypeScript; keep postcard
encode/decode and cache domain types in Rust.

The contract should be frozen before the two implementation agents begin. It
needs operations equivalent to:

```ts
interface BrowserStorageBackend {
  getBatch(keys: string[]): Promise<Array<Uint8Array | null>>;
  putBatch(entries: Array<{ key: string; value: Uint8Array }>): Promise<void>;
  deleteBatch(keys: string[]): Promise<void>;
  scanRecords(args: {
    typeNames: string[];
    after?: string;
    limit: number;
  }): Promise<Array<{ key: string; value: Uint8Array }>>;

  enqueueMutation(args: {
    enqueueKey: string;
    mutation: Uint8Array;
    optimistic: Uint8Array;
  }): Promise<{ id: string; inserted: boolean }>;
  loadMutationQueue(): Promise<Array<{
    id: string;
    mutation: Uint8Array;
    optimistic: Uint8Array;
  }>>;
  claimNextMutation(args: unknown): Promise<unknown>;
  deferMutation(args: unknown): Promise<unknown>;
  completeMutation(args: unknown): Promise<unknown>;
  discardMutation(args: unknown): Promise<unknown>;

  clear(): Promise<void>;
  close(): Promise<void>;
}
```

Define concrete claim/settlement payloads in a dedicated typed bridge module;
`unknown` above is only a planning placeholder. Every bridge rejection must
become a stable Rust storage error with operation context, without logging
record contents, GraphQL variables, identities, or BLOBs.

Required transaction boundaries:

| Storage operation | Required atomic behavior |
|---|---|
| `putBatch`, `deleteBatch` | whole input batch |
| `enqueueMutation` | queue row + optimistic layer + enqueue dedupe |
| `claimNextMutation` | strict-head check + lease update |
| `deferMutation` | claim validation + lease release/backoff update |
| `completeMutation` | real record upserts + queue/layer removal + settlement dedupe |
| `discardMutation` | claim validation + queue/layer removal + settlement dedupe |
| `clear` | records + queue + optimistic layers |

Do not issue one JS/SQL transaction per record. Batch at the same boundaries as
`Storage`.

### 5.4 Corruption and quota behavior

At open and after unclean termination, run the cheapest validated integrity
check supported by Turso. Classify errors as:

- unsupported/unavailable storage: disable the cache for the session;
- corrupt disposable records with a readable queue: clear records and rebuild;
- unreadable DB with a non-empty or unknown queue: do not silently delete the
  file; emit a high-severity diagnostic and use the approved queue-recovery
  path;
- quota/full disk: preserve already queued intent, stop accepting new durable
  optimism, and route non-optimistic operations to the network;
- namespace incompatibility: clear records, re-normalize queued optimistic
  source data, preserve queue order.

Request persistent storage with `navigator.storage.persist()` on a best-effort
basis and record the result. Do not make cache availability depend on the
request being granted.

## 6. Non-blocking initialization and cache/API racing

Turso must be dynamically imported only after browser capability checks and
only in the elected dedicated worker. It must not enter the page entry chunk or
run during module evaluation in the page.

Desired request behavior:

| Request policy/use | Cold engine | Warm engine |
|---|---|---|
| `network-only` | dispatch network immediately | dispatch network immediately |
| `cache-and-network` | dispatch network immediately; warm cache in parallel | start cache and network together; emit cache only if it wins and is still current |
| `cache-first` | give cache a measured small head start, then race network | normal cache-first, with a slow-read escape budget |
| `cache-only` / offline fallback | wait for cache readiness up to its own timeout; never touch network | cache only |
| writes/optimistic queue | await durable readiness; never silently drop or speculative-retry | serialize through active engine |

Do not hard-code the cache head-start budget until the Phase 0 measurements are
available. Add sequence guards so a late stale cache read cannot overwrite or
emit after a fresher network result.

The page should be interactive and able to dispatch initial API work while the
active worker downloads/compiles Turso. Measure initial page load separately
from subsequent navigation, as Notion did.

## 7. Failover and idempotency contract

This section is a release blocker.

### 7.1 Routed command identity

Every routed request gets a globally unique command ID plus coordinator owner
epoch. Every optimistic enqueue additionally gets a stable client-generated
enqueue key that survives rerouting. A retry with the same enqueue key must
return the original queue ID and must not add a second optimistic layer.

Extend the cache-core/storage enqueue result to distinguish `inserted` from
`already present`, or provide an equivalent engine-level ensure operation. All
backends, including SQLite and the temporary IDB backend, must implement the
same semantics so behavior does not vary by host.

### 7.2 Retry classes

Document every `CacheRequest` kind in one of these classes:

- **read-only, safe to reroute**: reads and inspections;
- **idempotent, safe to ensure**: explicit record upserts/deletes, invalidate,
  clear, teardown, after their result semantics are verified;
- **durable keyed command**: enqueue, claim, defer, commit, rollback;
- **never automatically retried**: any command without a durable way to
  determine its previous outcome.

For keyed queue commands, either persist enough request/settlement metadata to
return the prior outcome or expose a reconciliation operation. Do not turn an
indeterminate cache enqueue into an immediate unqueued network mutation: that
can send once now and again when the recovered durable queue drains.

### 7.3 Engine replacement

After active worker loss:

1. stop routing to the old epoch;
2. elect and initialize a new owner;
3. recover SQLite/Turso WAL and hydrate optimistic layers;
4. reconcile in-flight durable commands by command/enqueue key;
5. broadcast an engine-replaced notice;
6. have each host reexecute all locally active operations to rebuild
   dependency registration;
7. resume queue draining without skipping the strict head.

Kill tests must cover termination immediately before, during, and after every
transaction boundary listed in Section 5.3.

## 8. IndexedDB cutover strategy

Normalized records may start cold in Turso. Do not spend rollout complexity on
copying them unless measurements show that a cold rebuild is unacceptable.

Queued mutations require a deliberate handoff. Use a two-release strategy:

### Release A: make the existing IDB backend handoff-aware

- add stable enqueue keys/idempotent queue behavior;
- expose queue status needed by the migration controller;
- add a cross-version backend/owner beacon;
- teach the old host to quiesce new queue writes during an acknowledged
  handoff;
- keep IDB as the only production backend;
- soak long enough that supported clients have the handoff-aware version.

### Release B: dual backend rollout

- add Turso behind a separate browser-cache backend flag;
- keep selection sticky per profile/scope so tabs agree on a backend;
- only switch a scope after an acknowledged handoff and an empty IDB queue, or
  after an approved transactional export/import protocol;
- start with a fresh Turso `records` table;
- retain the legacy IDB path for rollback;
- prevent simultaneous IDB and Turso queue runners for one scope;
- account for an old tab left open across deployment; do not assume all tabs
  reload together.

### Release C: retirement

After Turso has soaked at 100% and rollback is no longer needed:

- close and delete the legacy normalized-cache IDB database after confirming
  its queue is empty/migrated;
- remove `cache-idb` from `cache-wasm` and the Cargo workspace;
- remove legacy SharedWorker code and rollout flags;
- update crate docs, cache design docs, workflow path filters, dependency
  closures, and generated artifacts;
- do not remove unrelated JavaScript `idb` dependencies.

If Release A cannot provide a safe cross-version handoff, stop and design a
server-side mutation idempotency guarantee before enabling Release B.

## 9. Work packages for multiple agents

Agents should use separate jj workspaces/revisions. The integration owner is
the only agent who edits shared manifests/lockfiles and generated workflows.
Each package below must end with tests and a short handoff note containing
revision ID, changed paths, commands run, and unresolved risks.

### WP-00 — integration owner and decision log

**Owner paths**

- this plan and an eventual ADR;
- `package.json`, `apps/web/package.json`, `bun.lock`;
- root `Cargo.toml`, `Cargo.lock`;
- generated workflow integration and final conflict resolution.

**Tasks**

1. Pin the exact Turso version for the spike.
2. Freeze the JS storage bridge and coordinator envelope after Phase 0.
3. Record go/no-go decisions and approved browser/performance budgets.
4. Land work packages in dependency order and keep the legacy path buildable.

**Depends on:** none.

### WP-01 — Turso-in-dedicated-worker feasibility spike

**Owner paths**

- a new isolated `apps/web/spikes/graphql-cache-turso/` directory;
- a results file under that spike directory.

**Tasks**

1. Import `@tursodatabase/database-wasm/vite` inside an outer
   DedicatedWorker and verify Turso's nested worker is emitted and starts.
2. Verify OPFS create, reopen, close, WAL recovery, BLOB round trips,
   transactions, foreign keys, ordered scans, `quick_check`, BigInt IDs, and
   file deletion.
3. Prove a second connection is rejected/blocked and a new owner can open
   after terminating the first.
4. Test Chromium, Firefox, and WebKit/Safari; record exact versions and any
   fallback decision.
5. Measure first load, warm open, query/transaction latency, transferred bytes,
   WASM compile time, worker count, and committed/resident memory.
6. Compare representative `get_batch`, 1,000-record put, scan, and queue
   transaction workloads with the current IDB probe/baseline.
7. Verify Vite dev and production asset URLs, MIME type, compression, and
   source maps.

**Deliverable:** reproducible benchmark/capability report, not production code.

**Depends on:** WP-00 adding the pinned dependency.

### WP-02 — cross-origin-isolation and web platform audit

**Owner paths**

- spike-only Vite/header configuration;
- `infra/stacks/web-app-preview/**` for a preview experiment;
- an audit report in the spike directory.

**Tasks**

1. Make a preview return COOP/COEP for the document, route-Lambda HTML,
   workers, WASM, and static assets.
2. Identify the production website-infra owner and exact change required;
   `infra/stacks/web-app` alone does not own the production CloudFront policy.
3. Inventory and exercise analytics scripts, PostHog, OAuth/popups, SSO,
   external avatars/images, email media, PDF resources, S3/static assets,
   LiveKit, iframes, downloads, and service calls.
4. Decide whether `require-corp` is viable. Evaluate `credentialless` only if
   Turso and the full browser matrix work with it.
5. Verify `crossOriginIsolated` in local, preview, dev, staging, and production-
   equivalent routes.
6. Produce a list of resources requiring CORS/CORP, same-origin proxying, or
   removal.

**Deliverable:** signed-off compatibility report and infrastructure ownership
map.

**Depends on:** none; runs in parallel with WP-01.

### WP-03 — coordinator/election topology spike

**Owner paths**

- new spike coordinator, host, and worker files only.

**Tasks**

1. Build a SharedWorker router with three tabs and one active outer
   DedicatedWorker.
2. Transfer a direct `MessageChannel` between coordinator and active worker.
3. Implement tab liveness locks, DB-owner lock, owner epochs, graceful close,
   abrupt close, and bounded reacquisition.
4. Demonstrate requests from every tab, active-tab closure, standby election,
   stale-response rejection, and exactly one DB owner.
5. Record behavior when SharedWorker/Web Locks/OPFS are unavailable and in
   private browsing.
6. Enumerate unsafe in-flight command cases for WP-07.

**Deliverable:** tested topology state machine and protocol proposal.

**Depends on:** none; can initially use an in-memory fake DB.

### WP-04 — durable queue and version-skew audit

**Owner paths**

- analysis/tests only under a new migration test area;
- no production queue changes yet.

**Tasks**

1. Trace every enqueue/claim/defer/commit/rollback failure path in the Rust
   engine, browser exchange, and Tauri host.
2. Determine which GraphQL mutations already have server idempotency/nonces.
3. Model old-tab/new-tab overlap across Releases A and B.
4. Propose the minimal enqueue key, claim reconciliation, and settlement
   metadata required for failover.
5. Specify kill points and expected outcomes for the production test suite.

**Deliverable:** reviewed idempotency ADR input for WP-07.

**Depends on:** none; runs in Phase 0.

### Gate G0 — approve or stop

WP-00 records explicit approval only if all of the following are true:

- Turso works inside the outer dedicated worker with the required nested
  worker in the supported browser matrix;
- cross-origin isolation does not break critical app/auth/media flows, or all
  required remediations are owned and scheduled;
- startup bytes, compile time, and measured resident memory fit an approved
  budget;
- basic transactions and kill/reopen tests show no corruption;
- a safe queue failover and IDB handoff design has been approved;
- production and preview header owners are known;
- unsupported browsers have an explicit fallback policy.

If G0 fails, retain IDB and reconsider official SQLite WASM/SAH Pool or a Turso
build that does not require cross-origin isolation/nested-worker routing.

### WP-05 — TypeScript Turso storage adapter

**Owner paths**

- new `apps/web/src/lib/graphql-cache/storage/**` files and their unit/browser
  tests;
- no Rust shell or coordinator files.

**Tasks**

1. Implement schema creation and metadata initialization.
2. Implement the frozen `BrowserStorageBackend` contract with prepared/batched
   SQL and exact transaction boundaries.
3. Preserve postcard bytes as BLOBs without JSON/base64 conversion.
4. Implement namespace clearing, scope clearing, close, integrity checks, and
   stale-file cleanup.
5. Add typed error classification without payload logging.
6. Add unit tests against a fake DB and browser tests against real Turso.

**Depends on:** G0 and the frozen bridge contract.

### WP-06 — Rust JavaScript-backed `Storage` bridge

**Owner paths**

- `crates/client/cache-wasm/**`;
- optionally one new WASM-only crate under `crates/client/` if approved;
- bridge-focused wasm tests.

**Tasks**

1. Replace the shell's hard-coded `Engine<IdbStorage>` type with a browser
   storage abstraction that can wrap the injected JS backend.
2. Encode/decode records and queue values with existing cache-core codecs.
3. Convert queue IDs through checked decimal strings.
4. Map rejected JS promises to stable Rust storage errors.
5. Preserve async serialization and operation interning.
6. Keep a legacy IDB constructor/export during dual-backend rollout.
7. Add engine-over-fake-JS-storage and engine-over-real-Turso browser tests.

**Depends on:** G0 and the frozen bridge contract. Can run in parallel with
WP-05.

### WP-07 — failover-safe queue commands

**Owner paths**

- `crates/client/cache-core/src/{queue,store,engine}.rs` and queue tests;
- `crates/client/cache-sqlite/**`;
- temporary `crates/client/cache-idb/**`;
- Tauri cache plugin queue glue/tests where the protocol changes require it.

**Tasks**

1. Add stable enqueue keys and idempotent ensure-enqueue semantics.
2. Implement the approved claim and settlement reconciliation design.
3. Preserve strict queue order and lease fencing.
4. Update every storage backend; Tauri behavior must remain equivalent.
5. Add duplicate-command and every-kill-point tests.
6. Bump cache/queue format metadata only when the persisted representation
   requires it; preserve source GraphQL/optimistic JSON across the bump.

**Depends on:** WP-04 ADR and G0. Can run in parallel with WP-05/WP-06.

### WP-08 — production coordinator and dedicated engine worker

**Owner paths**

- new coordinator/topology files under
  `apps/web/src/lib/graphql-cache/worker/`;
- topology tests;
- avoid `host/worker-host.ts` until WP-09 integration.

**Tasks**

1. Implement the reviewed coordinator state machine and typed topology
   envelopes.
2. Add lazy active-worker construction and direct `MessageChannel` wiring.
3. Run `CacheWorkerCore` in the active dedicated worker.
4. Dynamically load cache WASM and Turso only after election.
5. Implement liveness and DB-owner locks, epochs, graceful shutdown, abrupt
   failover, and read rerouting.
6. Implement engine-replaced pushes and queue reconciliation hooks.
7. Ensure all worker construction remains lazy on Tauri/iOS code paths per
   `apps/web/AGENTS.md`.

**Depends on:** WP-03, WP-05, WP-06, and WP-07 contracts.

### WP-09 — CacheHost/protocol integration

**Owner paths**

- `apps/web/src/lib/graphql-cache/host/worker-host.ts` and tests;
- `apps/web/src/lib/graphql-cache/protocol.ts` and tests;
- `worker/worker-core.ts` integration changes;
- cache lifecycle/scope comments and tests.

**Tasks**

1. Select legacy IDB or Turso topology using the sticky rollout selector.
2. Add capability detection and typed initialization failures.
3. Track local active operation keys so engine replacement can re-register
   them.
4. Preserve the existing rule: read-only RPCs time out; mutating RPCs are not
   naively retried after an unknown outcome.
5. Map coordinator pushes back to only the source client's urql operation
   keys.
6. Keep Tauri transport and public `CacheHost` behavior unchanged.

**Depends on:** WP-08. Coordinate shared-file edits through WP-00.

### WP-10 — non-blocking load and network race semantics

**Owner paths**

- `exchange/normalized-cache-exchange.ts` and focused tests;
- GraphQL Soup cache client initialization tests.

**Tasks**

1. Implement the Section 6 request-policy table.
2. Ensure initial cache initialization cannot delay eligible API dispatch.
3. Add slow-read escape and late-result sequence guards.
4. Ensure `.toPromise()` consumers still resolve fresh data as intended.
5. Ensure `cache-only` never touches the network and offline replay still
   waits for a bounded cache open.
6. Preserve durable optimistic enqueue-before-network behavior.

**Depends on:** a readiness interface from WP-09 and performance input from
WP-01.

### WP-11 — Release A IDB handoff support and Release B migration controller

**Owner paths**

- new migration/controller files under `graphql-cache/migration/**`;
- legacy IDB export/status glue;
- migration/version-skew browser tests.

**Tasks**

1. Implement queue status and acknowledged quiesce/handoff.
2. Persist a sticky backend selection per anonymous scope.
3. Prevent simultaneous queue runners across backends.
4. Handle old tabs, abandoned handoffs, rollback to IDB, and reloads at every
   point.
5. Copy no normalized records by default.
6. If queue import is approved, preserve IDs/order/enqueue keys and make the
   import resumable/idempotent.
7. Delete legacy storage only after proving its queue is empty/migrated.

**Depends on:** WP-04, WP-07, and WP-09.

### WP-12 — infrastructure and packaging

**Owner paths**

- `apps/web/vite.base.ts`;
- `infra/stacks/web-app/**` and `infra/stacks/web-app-preview/**`;
- the external website-infra change through its owner;
- cache build/asset smoke tests.

**Tasks**

1. Apply approved COOP/COEP headers in dev, preview, dev/staging/prod routes,
   route-Lambda HTML, worker scripts, WASM, and static responses.
2. Apply all CORS/CORP/proxy remediations from WP-02.
3. Ensure Turso WASM is an external lazy asset, has the correct MIME type and
   compression, and is not base64-inlined or entry-preloaded.
4. Verify the active worker resolves cache WASM and Turso nested-worker URLs in
   both Vite dev and production.
5. Add a build assertion that Turso is absent from the page entry chunk.

**Depends on:** WP-02 approval and G0. Production headers should first ship in
an isolated rollout.

### WP-13 — E2E, telemetry, and rollout controls

**Owner paths**

- new Playwright multi-page tests;
- cache-specific observability modules/tests;
- feature flag wiring and dashboards/alerts.

**Tasks**

1. Add the browser matrix and failure tests in Section 11.
2. Add metrics in Section 12 without cache keys, identities, queries, variables,
   or record contents.
3. Add kill switches for Turso backend and racing behavior independently.
4. Stage employee/dev, preview, small production cohort, browser-specific
   cohorts, then general rollout.
5. Define automatic rollback thresholds before exposure begins.

**Depends on:** WP-08 through WP-12.

### WP-14 — IDB retirement and documentation

**Owner paths**

- remove `crates/client/cache-idb/**` and legacy worker files;
- `crates/client/README.md` and cache design docs;
- `tooling/xtask/.../web_artifact_paths.rs`;
- generated workflows via their xtask source, not by hand.

**Tasks**

1. Remove legacy code only after Release C criteria pass.
2. Remove Cargo dependencies/workspace member and regenerate lock/dependency
   artifacts.
3. Replace IDB path filters with the new browser storage paths and regenerate
   generated workflows using the repository generator.
4. Update stale IndexedDB comments throughout cache-core, WASM declarations,
   scope/lifecycle, and tests.
5. Confirm unrelated IDB users remain intact.

**Depends on:** completed production soak and explicit rollback-window closure.

## 10. Parallel execution schedule

```text
Wave 0 (parallel): WP-01, WP-02, WP-03, WP-04
                         │
                         ▼
                    Gate G0 / ADR
                         │
Wave 1 (parallel): WP-05, WP-06, WP-07, approved WP-12 header remediation
                         │
Wave 2:            WP-08 -> WP-09
                         │       └-> WP-10
                         └----------> WP-11
Wave 3:                    WP-12 packaging + WP-13 E2E/telemetry
                         │
Wave 4:             Release A -> Release B staged rollout
                         │
Wave 5:             Release C -> WP-14 cleanup
```

Integration rules:

- WP-00 freezes the bridge/protocol types before Wave 1.
- Agents do not concurrently edit `package.json`, lockfiles, root Cargo
  manifests, generated workflows, `protocol.ts`, or `worker-host.ts`.
- Prefer new files in owned directories; send small shared-file patches to the
  integration owner.
- Every Rust backend change updates backend tests. No SQLx preparation is
  needed for Turso/rusqlite code, but normal Rust/wasm checks still apply.
- Each verified work package gets its own jj revision before handoff.

## 11. Required test matrix

### 11.1 Storage contract

Run the same conformance suite against in-memory/fake JS storage, Turso, IDB
while retained, and native SQLite where applicable:

- aligned batch get with misses;
- atomic upsert/delete;
- deterministic type-prefix scan and cursor pagination;
- queue enqueue/order/reopen;
- duplicate enqueue key returns one row/layer;
- lease fencing and strict-head blocking;
- defer/commit/discard retry reconciliation;
- complete mutation atomically writes records and removes optimism;
- namespace mismatch retains queue but clears records;
- scope mismatch and logout clear all user data;
- corrupt postcard value is classified, not panicked.

### 11.2 Worker and multi-tab E2E

For Chromium, Firefox, and the approved WebKit/Safari target:

1. Open three tabs with one scope.
2. Assert one coordinator, one active engine worker, and one Turso OPFS owner.
3. Read/write from every tab and observe cross-tab affected-operation pushes.
4. Close a standby tab; owner remains stable.
5. Gracefully close the active tab; a standby acquires ownership and all active
   operations re-register.
6. Abruptly terminate the active worker/tab during each storage transaction;
   WAL recovers and commands meet the idempotency contract.
7. Queue multiple optimistic mutations, fail over, and verify exactly-once
   replay in original order.
8. Go offline, restart the owner, and read a previously cached query.
9. Change identity and confirm records/queue are wiped and every operation
   reexecutes.
10. Exercise logout, failed clear/scope rotation, quota denial, private mode,
    storage eviction, and integrity failure.
11. Run one old-IDB tab and one new-Turso tab through every migration state.
12. Verify no Turso import/worker is created on Tauri or iOS native paths.

### 11.3 Build and regression checks

At minimum, final integration runs:

```sh
cargo test -p cache-core -p cache-sqlite
cargo check --target wasm32-unknown-unknown -p cache-wasm --all-targets
just build-cache-wasm
bun run test
bun run check
just build-dev
```

Add the real Turso/worker browser suite as a dedicated command; Vitest/jsdom
alone cannot validate OPFS, cross-origin isolation, nested workers, or
multi-tab failover. Test individual changed Rust crates before integration, in
accordance with repository guidance.

## 12. Telemetry and rollout gates

Collect by backend/browser/app version:

- capability/fallback reason;
- cross-origin-isolated status;
- Turso asset download, compile, connect, schema-init, and first-ready times;
- cache read hit/miss/error and p50/p95/p99 latency;
- network-won/cache-won/slow-cache-escape counts;
- initial page load and navigation timing control vs treatment;
- estimated active-worker and per-tab memory;
- OPFS quota/usage and persistence-granted status;
- transaction latency by storage operation kind;
- owner elections, graceful handoffs, failovers, lock wait, and recovery time;
- integrity-check failures and reset/recovery outcomes;
- queue depth/oldest age, duplicate-key reconciliation, lease expiry, and
  indeterminate commands;
- late/stale response drops;
- Turso fallback and rollback rates.

Never emit entity keys, cache scope UUIDs, user identity, GraphQL documents,
variables, record bytes, mutation payloads, or DB filenames.

Define numeric budgets and automatic rollback thresholds after WP-01/WP-02
baseline measurements and before Release B. At a minimum, initial page load
must not regress, navigation p95 must not regress, corruption/queue loss must
remain zero, and one scope must never have two active OPFS owners.

## 13. Principal risks and mitigations

| Risk | Mitigation / stop condition |
|---|---|
| COOP/COEP breaks third-party assets or OAuth | WP-02 is a hard gate; proxy/remediate first or stop |
| Turso cannot nest its worker from the outer worker | WP-01 hard gate; do not patch production topology speculatively |
| 250 MiB initial shared memory is too costly | measure committed memory on representative devices; reject if budget is not approved |
| Large WASM slows boot | active-tab-only lazy import, no preload, non-blocking network, compressed asset, rollout telemetry |
| Active tab dies mid-command | epochs, stable command/enqueue keys, reconciliation, kill tests |
| OPFS corruption/concurrency | exclusive DB Web Lock, one elected owner, WAL/integrity checks, staged rollout |
| Turso pre-1.0 regression | exact pin, compatibility suite, kill switch, retain IDB rollback until soak completes |
| Durable queue lost during backend/version skew | Release A handoff, sticky backend, no cutover with unacknowledged queue |
| Standby tabs duplicate WASM/memory | create/initialize engine worker only after election |
| Engine failover loses dependency index | engine-replaced push and reexecute all active local operations |
| Unsupported/private browsers | fail closed to legacy IDB during rollout, then no-op/network host |
| Scope rotation leaks OPFS files | close handles and bounded stale-scope cleanup after queue safety checks |
| Two WASM memories copy BLOBs through JS | batch contract and WP-01 profiling; reconsider direct integration only if measured cost is material |

## 14. Definition of done

The migration is complete only when:

- all G0 gates and browser support decisions are recorded;
- Turso is loaded only in the elected dedicated worker;
- coordinator failover and single-writer invariants pass the multi-tab kill
  suite;
- the storage conformance suite passes for Turso and native SQLite;
- queued mutations survive restart, failover, identity rules, and IDB handoff
  without loss or duplication;
- initial page load and navigation performance meet approved budgets;
- cross-origin isolation is verified on every production route and critical
  app/auth/media regression tests pass;
- Turso has completed staged rollout and rollback soak;
- legacy normalized-cache IDB code/storage is removed without touching other
  IDB users;
- design docs, generated workflows, manifests, and operational runbooks match
  the final architecture.
