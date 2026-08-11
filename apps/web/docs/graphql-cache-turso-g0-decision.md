# GraphQL cache Turso Gate G0 decision

Status: **NO-GO — production work packages WP-05 through WP-12 are stopped**

This decision records the result of Wave 0 in
[`graphql-cache-turso-worker-migration-plan.md`](./graphql-cache-turso-worker-migration-plan.md).
It is not approval to replace the production IndexedDB cache.

## Decision

Do not begin the production `turso-opfs`, `cache-turso`, `cache-wasm`, or worker
cutover work. The direct Rust Turso/OPFS architecture is technically viable in
the Chromium and Firefox versions exercised by the spikes, but Gate G0 has not
passed for the product's browser matrix, transaction contract, or approved
resource budgets.

No Turso dependency has been added to the production Cargo workspace, no npm
package has been added, and the existing browser/Tauri cache paths remain
unchanged.

## Evidence

| Work package | Result | Evidence |
|---|---|---|
| WP-01: Turso core WASM | Partial pass | [`../spikes/graphql-cache-turso-core/README.md`](../spikes/graphql-cache-turso-core/README.md) |
| WP-02: Rust OPFS adapter | Partial pass; recommends NO-GO | [`../spikes/graphql-cache-turso-opfs/README.md`](../spikes/graphql-cache-turso-opfs/README.md) |
| WP-03: coordinator topology | Pass as a spike | [`../spikes/graphql-cache-turso-coordinator/README.md`](../spikes/graphql-cache-turso-coordinator/README.md) |
| WP-04: storage contract | Design complete; full SQL conformance remains unexecuted | [`graphql-cache-turso-storage-design.md`](./graphql-cache-turso-storage-design.md) |

The candidate Turso core revision evaluated by WP-01 and WP-02 is:

```text
ed15b13f8e5f77d7ae24af321a63d7cd0fa53365
v0.8.0-pre.3
```

It remains an evaluation pin, not an approved production dependency.

## What passed

- Turso core compiles directly to `wasm32-unknown-unknown` without a Turso npm
  package.
- The inspected modules use one unshared 32-bit WASM memory, no atomics,
  threads, shared memory, nested worker, or cross-origin isolation.
- A representative cache DDL/SQL subset, compound `(__typename, id)` records,
  BLOBs, foreign-key cascades, queue operations, rollback, and fenced claims
  execute in the core spike.
- A direct Rust `IO`/`File` adapter over OPFS sync access handles works in the
  actually-run Chromium 145 and Firefox 146 builds.
- Those browsers passed direct file operations, real Turso main/WAL
  persistence, cross-worker reopen, exclusive Web Locks, active worker
  termination, bounded handle recovery, full deletion/recreation, and injected
  close/delete/recreation failures.
- The numeric-handle design keeps JavaScript handles in worker-local storage;
  Turso trait objects contain only checked IDs and need no unsafe `Send`/`Sync`
  implementation.
- The SharedWorker coordinator spike passed Chromium, Firefox, and the
  available matching WebKit harness for owner epochs, graceful handoff,
  abrupt-loss wipe, stale-response rejection, and physical owner-lock
  exclusion.

## Failed or unresolved G0 conditions

### 1. Approved Apple browser behavior is unproven

The available WebKit WPE 26 worker did not expose
`navigator.storage.getDirectory()`. WPE is not Safari or WKWebView, so this is
not evidence that the product's Apple targets pass or fail. No real
Safari/WKWebView run was available.

Gate G0 requires an explicitly approved browser matrix. Proceeding requires
one of:

- real Safari and applicable WKWebView tests that pass the WP-02 harness; or
- an explicit product decision that those targets are outside the browser
  normalized-cache support matrix.

There is no IDB fallback in the proposed production design.

### 2. Immediate transactions trap in the evaluated Turso revision

`BEGIN IMMEDIATE` and `BEGIN EXCLUSIVE` enter Turso's internal temporary
`MemoryIO`, which calls `std::time::Instant::now()` and traps on
`wasm32-unknown-unknown` at the evaluated revision.

Deferred `BEGIN` passed strict-head selection, fenced claim updates, rollback,
and a competing-connection `BusySnapshot` probe. It is acceptable only if the
production contract formally requires:

- exactly one `TursoStorage` connection per database owner;
- the existing `CacheWorkerCore` serialized command queue;
- the cache-wasm async engine mutex;
- no re-entry or nested transaction; and
- rollback/retry on `Busy`/`BusySnapshot` at the serialized operation boundary.

Before G0 can pass, WP-00 must either approve that deferred contract or select
and verify a Turso patch/revision that fixes immediate transactions.

### 3. The complete storage SQL contract has not run

WP-01 proves a representative subset, not every query required by WP-04. In
particular, it does not yet prove the canonical-key scan expression
`(__typename || ':' || id) COLLATE BINARY` with prefix typenames, the queue
consistency `LEFT JOIN` queries, `PRAGMA quick_check`,
`PRAGMA foreign_key_check`, or the complete classified-error behavior. Its
compound-key probe orders by the tuple, which WP-04 explicitly does not permit
for multi-typename cursor semantics.

The exact WP-04 SQL, binding, result-shape, transaction, pragma, and error
contract must execute against the selected Turso revision before G0 can pass.

### 4. Resource budgets are not approved or measured end to end

WP-01 measured the optimized Turso-core spike at approximately 6.68 MB raw and
1.82 MB Brotli after wasm-bindgen/wasm-opt. That excludes `cache-core`, the
production OPFS adapter, and the full cache-wasm API. Its Node proxy reached
about 16.1 MB of linear memory after the SQL exercise and approximately
102 MB RSS delta; these are development proxies rather than browser budgets.

WP-02's broader post-wasm-bindgen spike was approximately 8.80 MB before a
production combined-module size pass. No product owner has approved download,
startup, or active-worker memory limits, and no representative combined gate
artifact has been measured against such limits.

### 5. Lifecycle integration still needs a production contract

Turso's `File` trait has no close method, and OPFS deletion is asynchronous
while `IO::remove_file` is synchronous. The spike proved a consuming lifecycle:
close/drop Turso first, close all registered handles, then asynchronously
remove/recreate files while retaining the owner lease. This API must be frozen
and reviewed before production implementation.

## Conditions to reopen production work

WP-00 may change this decision to GO only after all of the following are
recorded:

1. The supported browser matrix is explicit, and every target passes both the
   WP-02 OPFS capability/recovery harness and the WP-03 coordinator/failover
   harness.
2. The immediate-transaction fix or deferred serialized-connection contract is
   approved.
3. The complete WP-04 SQL, pragma, transaction, result-shape, and error
   conformance contract passes against the selected core revision.
4. Combined WASM download, startup, and active-worker memory budgets are
   numeric and approved, and a representative gate artifact containing Turso
   core, `cache-core`, the OPFS adapter, and the cache-wasm shell meets them.
5. The consuming OPFS owner/session/close/reset API is frozen.
6. Gate evidence remains reproducible against the exact Turso revision and
   browser/tool derivations.

Until then, WP-05 through WP-12 remain blocked. The spikes may be used to
resolve these gates, but production backend or cutover code must not begin.
