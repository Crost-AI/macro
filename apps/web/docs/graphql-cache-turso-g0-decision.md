# GraphQL cache Turso Gate G0 decision

Status: **GO — production work packages WP-05 through WP-12 are authorized**

This decision records Wave 0 and the product-owner decisions that reopen
[`graphql-cache-turso-worker-migration-plan.md`](./graphql-cache-turso-worker-migration-plan.md).
It authorizes production implementation, not production exposure. The rollout
and performance gates remain in WP-11 and WP-12.

## Decision

Proceed with the direct Rust Turso/OPFS architecture. Compile `turso_core` into
the existing cache WASM module, keep one elected DedicatedWorker as the only
OPFS owner, and preserve the SharedWorker coordinator and disposable-database
recovery model.

The production Cargo dependency is pinned to the remotely available commit:

```text
repository: https://github.com/seanaye/turso
branch:     fix/avoid-unused-temp-db-init
revision:   be9acfe9e5e6efb17911af84047e4855cace53a3
parent:     79163249538197d01dec5ea7f65519454ed792e2
```

That revision has source tree
`0adf7c52e8d139f9f24db9fdccd549afcc04a878`, identical to the fully verified
local commit `cf7de76172d61057007097e2dee7c47002cdc559`. No Turso npm package,
second Turso WASM module, fallback backend, shared memory, or cross-origin
isolation is approved.

## Evidence

| Work package | Result | Evidence |
|---|---|---|
| WP-01: Turso core WASM | Baseline compile/API/size evidence | [`../spikes/graphql-cache-turso-core/README.md`](../spikes/graphql-cache-turso-core/README.md) |
| WP-02: Rust OPFS adapter | Baseline adapter/safety evidence | [`../spikes/graphql-cache-turso-opfs/README.md`](../spikes/graphql-cache-turso-opfs/README.md) |
| WP-03: coordinator topology | Pass | [`../spikes/graphql-cache-turso-coordinator/README.md`](../spikes/graphql-cache-turso-coordinator/README.md) |
| WP-04: storage contract | Frozen with owner-approved SQL scope | [`graphql-cache-turso-storage-design.md`](./graphql-cache-turso-storage-design.md) |
| Fork transaction verification | Pass | [`../spikes/graphql-cache-turso-core-fix-verify/README.md`](../spikes/graphql-cache-turso-core-fix-verify/README.md) |
| Fork OPFS/browser verification | Pass for production cache routes | [`../spikes/graphql-cache-turso-opfs-cf7de761/README.md`](../spikes/graphql-cache-turso-opfs-cf7de761/README.md) |

The evidence proves:

- Turso core compiles directly to `wasm32-unknown-unknown` in one unshared,
  single-threaded module with no atomics, nested workers, WASI, or shared-memory
  imports.
- The fork fixes the unused-temp `BEGIN IMMEDIATE` and `BEGIN EXCLUSIVE` trap.
  The parent traps through `ensure_temp_database` and
  `std::time::Instant::now`; the fixed tree does not materialize an unused temp
  database.
- The selected cache SQL executes with compound `(__typename, id)` records,
  canonical key ordering, BLOBs, queue consistency joins, foreign-key
  enforcement and cascades, immediate transactions, rollback, fencing, and
  `quick_check`.
- Numeric handle IDs keep JavaScript OPFS handles worker-local while satisfying
  Turso's `Send + Sync` trait bounds without unsafe implementations.
- Chromium and Firefox pass main/WAL persistence, close/reopen, worker kill,
  bounded recovery, deletion/recreation, injected failures, Web Lock
  exclusion, and the enumerated transaction/cache routes without a production
  WASM trap.
- The SharedWorker coordinator proves one physical owner, epochs, graceful
  handoff, abrupt-loss wipe, and stale-response rejection.

## Product-owner scope decisions

### Explicit temp storage is out of scope

Production cache SQL never creates or accesses the SQL `temp` schema. Internal
sorters and ephemeral query structures are not SQL temp tables and passed the
selected query routes. `CREATE TEMP TABLE`, temp views, temp triggers, and
unreviewed temp-schema SQL remain unsupported, but that is not a G0 blocker.
Regression tests must ensure the fixed `BEGIN IMMEDIATE` path does not open an
unused temp database.

### Foreign-key enforcement is never disabled

Every production connection executes `PRAGMA foreign_keys = ON`, verifies it,
and keeps it enabled for its full lifetime. The product does not disable
enforcement and later use `PRAGMA foreign_key_check` to discover deliberately
inserted orphans. Turso's missing violation rows for that unsupported test are
therefore not a production blocker.

Normal inserts must reject missing parents, `ON DELETE CASCADE` must work, and
queue hydration/settlement must reject any observed queue/layer inconsistency.
An inconsistency or failed `quick_check` requests a full disposable-database
reset.

### Safari capability is accepted

The supported macOS Safari target provides the required worker OPFS, Web Lock,
SharedWorker, DedicatedWorker, and WebAssembly features. The unrelated WebKit
WPE harness limitation is not a product blocker. Browser E2E remains required
before exposure, but absence of a WPE result does not stop implementation.

### Resource measurements move to the packaging gate

The optimized core spike measured approximately 6.68 MB raw and 1.82 MB Brotli;
the broader OPFS verification module measured approximately 8.82 MB, but it is
not the production combined artifact. WP-11 must measure the actual combined
`turso_core` + `cache-core` + OPFS + cache-wasm artifact and report download,
startup, and active-worker memory before rollout. These measurements are an
optimization and exposure gate, not a reason to block WP-05 implementation.

## Frozen OPFS consuming lifecycle

WP-05 must preserve these consuming semantics:

1. The elected DedicatedWorker holds the scope-specific exclusive owner lock
   before opening a session.
2. Session creation asynchronously pre-opens exactly the approved main and WAL
   paths and registers their sync access handles under checked numeric IDs.
3. Turso receives only an owner/session-bound `IO`; unregistered paths, stale
   tokens, concurrent owners, and reentrant operations are rejected without
   mutating a healthy session.
4. An opening failure closes every handle opened so far. It returns to the idle
   owner state only when cleanup is certain; uncertain cleanup poisons the
   worker-local session.
5. Graceful close first drains operations and finalizes statements, explicitly
   drives Turso `Connection::close()` to completion, then drops the connection,
   database, and every adapter/IO reference. Only then does it close every
   JavaScript sync access handle and return a one-use closed-session token.
6. Preservation consumes that token without deleting files and returns to the
   idle owner state; the coordinator may then release the owner lock for a
   graceful handoff. Reset consumes the token while retaining the owner lock,
   asynchronously removes and recreates every approved path, and returns to an
   idle owner state ready for a fresh session.
7. An invalid or already-consumed token returns an error without changing
   state. An uncertain close, failed opening cleanup, partial
   deletion/recreation, or unexpected handle state poisons the session. It
   cannot be reopened or released as healthy; the coordinator replaces the
   worker and follows the abrupt-loss wipe path.
8. Abrupt owner loss never attempts handoff of a connection, request queue, or
   handle. In-flight requests fail, the next epoch wipes main/WAL, and the cache
   starts empty.

Exact Rust type names may be crate-private, but ownership must be encoded by
consuming APIs rather than caller convention. OPFS deletion must never be
implemented through Turso's synchronous `IO::remove_file`.

## Implementation gates after G0

G0 authorizes WP-05 through WP-12 in dependency order. The following remain
acceptance criteria for their owning packages rather than blockers to starting
production code:

1. WP-05 tests every file operation and the frozen close/reset lifecycle.
2. WP-06 runs real `cache-core` codec, `Storage`, transaction, reset, and error
   classification conformance against Turso.
3. WP-08/WP-09 run production coordinator, epoch, failover, and host ordering
   tests.
4. WP-11 reports the one-module artifact and resource measurements.
5. WP-12 passes supported-browser failure/recovery E2E before exposure.

The existing IndexedDB browser backend remains active until the direct cutover
package is verified; no dual-backend runtime or fallback path is introduced.
