# GraphQL cache Turso Gate G0 decision

Status: **NO-GO — production work packages WP-05 through WP-12 are stopped**

This decision records the result of Wave 0 in
[`graphql-cache-turso-worker-migration-plan.md`](./graphql-cache-turso-worker-migration-plan.md).
It is not approval to replace the production IndexedDB cache.

## Decision

Do not begin the production `turso-opfs`, `cache-turso`, `cache-wasm`, or worker
cutover work. The direct Rust Turso/OPFS architecture is technically viable in
the Chromium and Firefox versions exercised by the spikes, but Gate G0 has not
passed for the product's browser matrix, storage-integrity contract, or
approved resource budgets.

No Turso dependency has been added to the production Cargo workspace, no npm
package has been added, and the existing browser/Tauri cache paths remain
unchanged.

## Evidence

| Work package | Result | Evidence |
|---|---|---|
| WP-01: Turso core WASM | Partial pass | [`../spikes/graphql-cache-turso-core/README.md`](../spikes/graphql-cache-turso-core/README.md) |
| WP-02: Rust OPFS adapter | Partial pass; recommends NO-GO | [`../spikes/graphql-cache-turso-opfs/README.md`](../spikes/graphql-cache-turso-opfs/README.md) |
| WP-03: coordinator topology | Pass as a spike | [`../spikes/graphql-cache-turso-coordinator/README.md`](../spikes/graphql-cache-turso-coordinator/README.md) |
| WP-04: storage contract | Design complete; selected SQL executed, one required pragma failed | [`graphql-cache-turso-storage-design.md`](./graphql-cache-turso-storage-design.md) |
| Fork transaction verification | Unused-temp fix passes; explicit temp remains unsupported | [`../spikes/graphql-cache-turso-core-fix-verify/README.md`](../spikes/graphql-cache-turso-core-fix-verify/README.md) |
| Fork OPFS/browser verification | Transaction/OPFS routes pass; integrity pragma fails | [`../spikes/graphql-cache-turso-opfs-cf7de761/README.md`](../spikes/graphql-cache-turso-opfs-cf7de761/README.md) |

The original baseline was `turso_core` `v0.8.0-pre.3` at
`ed15b13f8e5f77d7ae24af321a63d7cd0fa53365`. Follow-up verification used the
clean local fork at:

```text
head:   cf7de76172d61057007097e2dee7c47002cdc559
parent: 79163249538197d01dec5ea7f65519454ed792e2
branch: fix/avoid-unused-temp-db-init
```

The fork commit remains an evaluation candidate, not an approved or remotely
available production dependency.

## What passed

- Turso core compiles directly to `wasm32-unknown-unknown` without a Turso npm
  package.
- The inspected modules use one unshared 32-bit WASM memory, no atomics,
  threads, shared memory, nested worker, or cross-origin isolation.
- The fork fixes the unused-temp `BEGIN IMMEDIATE` and `BEGIN EXCLUSIVE` WASM
  trap. The exact parent traps through `ensure_temp_database` and
  `std::time::Instant::now`; the fixed head succeeds without materializing
  temp.
- Selected WP-04 SQL executes with compound `(__typename, id)` records,
  canonical concatenated-key ordering, BLOBs, queue consistency joins,
  foreign-key cascades, immediate transactions, rollback, fencing,
  `quick_check`, and classified error probes.
- A direct Rust `IO`/`File` adapter over OPFS sync access handles works in the
  actually-run Chromium 145 and Firefox 146 builds.
- Those browsers passed direct file operations, real Turso main/WAL
  persistence, immediate/exclusive transactions, cross-worker reopen,
  exclusive Web Locks, active worker termination, bounded handle recovery,
  full deletion/recreation, and injected close/delete/recreation failures.
- Two cold and two warm fork-head runs in each Chromium and Firefox observed
  zero WASM environment traps or unhandled failures on every enumerated
  production/control worker route. The generated module has one unshared
  32-bit memory, zero atomics, and an exact import allowlist with no
  thread/WASI/worker imports.
- The numeric-handle design keeps JavaScript handles in worker-local storage;
  Turso trait objects contain only checked IDs and need no unsafe `Send`/`Sync`
  implementation.
- The SharedWorker coordinator spike passed Chromium, Firefox, and the
  available matching WebKit harness for owner epochs, graceful handoff,
  abrupt-loss wipe, stale-response rejection, and physical owner-lock
  exclusion.

## Resolved and bounded by the fork verification

The former immediate-transaction blocker is resolved for unopened, unused temp
storage by `cf7de761`. This is differential native, Node/V8 WASM, Chromium, and
Firefox evidence, not merely a compile result.

The conclusion about other WASM environment issues is deliberately bounded:
no trap was observed on the enumerated cache SQL, OPFS, lifecycle, failure, or
recovery routes. Built-in `MemoryIO` and explicit temp tables still call
`std::time::Instant::now()` and trap on WASM. The cache schema does not use temp
storage, so production must prohibit unreviewed temp-dependent SQL and retain a
negative regression probe. No conclusion extends to unenumerated SQL/VFS paths
or unrun browsers.

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

### 2. `PRAGMA foreign_key_check` is a silent no-op

The fork harness executed the selected WP-04 SQL in native and repeated WASM
runs, including the exact canonical scan expression, queue consistency joins,
`quick_check`, immediate/exclusive transactions, rollback, and error probes.

However, the browser OPFS harness disabled enforcement on its connection,
inserted this deliberate orphan, and then observed zero result columns and rows
from `PRAGMA foreign_key_check`:

```text
optimistic_layers(mutation_id=9999999) -> mutation_queue(id)
```

The required SQLite result is one four-column row identifying
`optimistic_layers`, rowid `9999999`, parent `mutation_queue`, and foreign-key
index `0`. A separate core harness used a second connection to insert orphan
`77777` and likewise observed no violation row. Silently returning no violation
in either probe is a storage-integrity failure.
Before G0 can pass, the selected core must implement the pragma or WP-00 must
approve and test an equally strong schema-specific integrity query.

The core coverage matrix also keeps five integration requirements explicitly
unapproved: rollback-I/O outcome classification, consuming reset after an
uncertain transaction, physical reset for compatibility/integrity mismatch,
real `cache-core` codec/`Storage` conformance, and real-browser quota/private-
mode/eviction/device-crash behavior.

### 3. Resource budgets are not approved or measured end to end

WP-01 measured the optimized Turso-core spike at approximately 6.68 MB raw and
1.82 MB Brotli after wasm-bindgen/wasm-opt. That excludes `cache-core`, the
production OPFS adapter, and the full cache-wasm API. Its Node proxy reached
about 16.1 MB of linear memory after the SQL exercise and approximately
102 MB RSS delta; these are development proxies rather than browser budgets.

The fork OPFS post-wasm-bindgen verification module is approximately 8.82 MB,
but it is still not the production combined module. No product owner has
approved download, startup, or active-worker memory limits, and no
representative combined gate artifact has been measured against such limits.

### 4. Lifecycle integration still needs a production contract

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
2. The selected production Turso revision contains the verified unused-temp
   fix, and regression tests enforce that production cache SQL never opens
   explicit temp storage.
3. `foreign_key_check` or an approved schema-specific equivalent detects the
   deliberate orphan, and the remaining WP-04 integration conformance matrix
   passes.
4. Combined WASM download, startup, and active-worker memory budgets are
   numeric and approved, and a representative gate artifact containing Turso
   core, `cache-core`, the OPFS adapter, and the cache-wasm shell meets them.
5. The consuming OPFS owner/session/close/reset API is frozen.
6. Gate evidence remains reproducible against the exact Turso revision and
   browser/tool derivations.

Until then, WP-05 through WP-12 remain blocked. The spikes may be used to
resolve these gates, but production backend or cutover code must not begin.
