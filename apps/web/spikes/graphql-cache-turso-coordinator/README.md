# GraphQL cache Turso coordinator spike (Wave-0 WP-03)

This directory is a self-contained feasibility spike. It does **not** import or
modify the production GraphQL cache host, protocol, SharedWorker, or engine
worker. The durable stand-in is a spike-only IndexedDB key/value store; no
Turso package, cache WASM, OPFS adapter, or production transport is involved.

## What it proves

- Three real same-origin browser tabs connect to one SharedWorker coordinator.
- The first tab is elected at owner epoch 1; standby tabs do not construct a
  dedicated worker.
- An elected tab lazily constructs one fake dedicated engine worker.
- The tab transfers opposite ends of a `MessageChannel` to the SharedWorker
  and dedicated worker. Requests route page -> coordinator -> engine without
  relaying engine traffic through the elected page.
- Every page holds `graphql-cache-tab:<scope>:<tabId>` for its lifetime. The
  coordinator waits for that lock; acquiring it is the correctness signal for
  page loss.
- The engine holds `graphql-cache-owner:<scope>` before opening the fake DB and
  until close completes. A replacement cannot open or wipe concurrently.
- A graceful departure stops routing, drains prior messages, closes the fake
  DB, and preserves it for the next epoch.
- Abrupt page loss rejects old-epoch in-flight requests. The replacement
  acquires the owner lock, wipes, opens, and acknowledges that ordering before
  activation.
- An uncaught dedicated-worker error while its page remains alive causes the
  page adapter to explicitly terminate and clear the failed worker before it
  reports owner loss. The same registered page can then host a wipe-required
  replacement without leaving the old owner lock held.
- Old-epoch responses are dropped, and the state machine can represent only
  zero or one routing-active owner.

The browser scenario intentionally closes the epoch-2 owner tab without a
`dispose` message, then induces an uncaught error in the epoch-3 worker without
closing its page. This separately exercises tab-liveness and worker-only loss.
The stale-response adapter probe sends an old-epoch engine envelope through a
real `MessageChannel`; it does not call `CoordinatorCore.engineResponse`
directly. It uses a probe port because the real failed-engine port is closed
immediately and the old worker is terminated, so that physical route cannot
legitimately deliver a later message.

## Files

- `coordinator-core.ts`: pure deterministic state machine.
- `coordinator-core.test.ts`: three-tab, epoch, drain, loss, wipe-proof,
  liveness, stale-response, and single-owner tests.
- `coordinator.shared-worker.ts`: browser coordinator/router and liveness-lock
  watcher.
- `fake-engine.worker.ts`: exclusive-owner-lock engine and fake durable DB.
- `tab.ts`: page adapter; the only `new Worker` is inside the election handler.
- `harness.ts`: three-real-tab automated/manual scenario.
- `browser.e2e.ts`: Playwright assertion over the browser report.
- `BROWSER_REPORT.md`: executed matrix and manual reproduction steps.

## Recommended production protocol

Freeze a coordinator-specific envelope separate from the existing cache RPC
payloads. The envelope should contain:

1. **Tab registration** — `scope`, random `tabId`, and the deterministic
   liveness-lock name. Registration is valid only after the page has acquired
   and is holding that lock.
2. **Election** — coordinator-issued `tabId`, monotonically increasing
   `ownerEpoch`, owner-lock name, and `databaseAction` (`open-existing` or
   `wipe-before-open`). Only the coordinator allocates epochs.
3. **Direct engine attachment** — `tabId`, `ownerEpoch`, and one transferred
   `MessagePort`. Accept it only for the current `activating` tuple.
4. **Ready acknowledgement** — `tabId`, `ownerEpoch`, an assertion that the
   owner lock is held, and the completed database action. This is a trusted
   engine acknowledgement, not independent proof. A wipe-required epoch must
   never accept an ordinary open acknowledgement; browser tests should also
   inspect Web Locks and validate data was empty before routing begins.
5. **Routed RPC** — coordinator-generated `routeId`, `ownerEpoch`, and the
   unchanged cache RPC payload. Responses and pushes repeat `ownerEpoch` and
   `routeId` where applicable.
6. **Drain** — current `tabId`/`ownerEpoch`; `drained` means all earlier direct
   port messages completed, Turso and sync handles closed, and owner-lock
   release is imminent. The replacement still independently waits for the
   owner lock.
7. **Loss/reset** — reject every old-epoch in-flight route without replay,
   enter `resetting-after-loss`, and elect the next owner with
   `wipe-before-open`.
8. **Engine replacement** — broadcast the new epoch only after readiness so
   pages reexecute active operations and rebuild dependency registration.

Do not use MessagePort closure as the only failure detector; it has no reliable
cross-browser disconnect event. Use the tab liveness locks for document loss
and add a bounded current-epoch engine heartbeat/watchdog for a dedicated
worker that dies while its page remains alive. On either `Worker.onerror` or a
watchdog timeout, the page must terminate and clear its failed worker before
reporting loss; coordinator-detected port/activation failures must command the
page to do the same. Every failure path remains fenced by the exclusive owner
lock and epoch.

## Evidence boundaries

The report intentionally separates three kinds of evidence:

- `routerMaxActiveOwners` is a pure coordinator invariant, not a measurement of
  physical lock holders.
- `engine-ready` and lock lifecycle events are trusted worker
  acknowledgements/timestamps. Their ordering checks the adapter sequence but
  is not an independent observer.
- `navigator.locks.query()` counts and non-blocking exclusive lock probes are
  independent page observations at each active epoch. They observed one held
  owner lock and could not acquire a competing lock. Sampling cannot prove
  that no unobserved microsecond overlap occurred; the physical exclusion
  contract ultimately comes from the browser's exclusive Web Lock.

The fake-DB reads after epochs 3 and 4 independently confirm that no request
was routed until the wipe-required replacement exposed an empty database.

## Scope limits

The fake DB validates preservation-versus-wipe sequencing, not OPFS durability.
WP-02 must separately validate sync access handles, main/WAL deletion, worker
kill recovery, and the approved Safari/WKWebView target before Gate G0.
